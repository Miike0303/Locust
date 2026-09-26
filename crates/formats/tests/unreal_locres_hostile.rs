use locust_formats::unreal_locres::{
    LocresFile, LocresNamespace, LocresString, LocresVersion, LOCRES_MAGIC,
};
use std::collections::HashMap;

fn sample(version: LocresVersion) -> LocresFile {
    LocresFile {
        version,
        namespaces: vec![LocresNamespace {
            name: "UI".into(),
            name_hash: if version as u8 >= 2 { 123 } else { 0 },
            strings: vec![LocresString {
                key: "Hello".into(),
                value: "Hello 世界".into(),
                source_string_hash: 0x12345678,
                key_hash: if version as u8 >= 2 { 456 } else { 0 },
            }],
        }],
    }
}

#[test]
fn regression_minimum_fstring_length_must_not_panic() {
    let mut data = 1i32.to_le_bytes().to_vec();
    data.extend(i32::MIN.to_le_bytes());
    data.extend(0i32.to_le_bytes());
    let result = std::panic::catch_unwind(|| LocresFile::parse(&data, "min.locres"));
    assert!(result.is_ok(), "hostile FString length panicked");
    assert!(result.unwrap().is_err());
}

#[test]
fn regression_key_count_bound_precedes_allocation() {
    let mut data = 1i32.to_le_bytes().to_vec();
    data.extend(0i32.to_le_bytes());
    data.extend(1000i32.to_le_bytes());
    let error = LocresFile::parse(&data, "counts.locres").unwrap_err();
    assert!(
        error.message.contains("remaining bytes"),
        "{}",
        error.message
    );
}

#[test]
fn regression_string_table_cannot_alias_namespace_count() {
    let mut data = LOCRES_MAGIC.to_vec();
    data.push(1);
    data.extend(25i64.to_le_bytes());
    data.extend(0i32.to_le_bytes());
    assert!(LocresFile::parse(&data, "overlap.locres").is_err());
}

#[test]
fn regression_optimized_negative_entry_count_is_invalid() {
    let mut data = sample(LocresVersion::Optimized).serialize().unwrap();
    data[25..29].copy_from_slice(&(-1i32).to_le_bytes());
    assert!(LocresFile::parse(&data, "negative.locres").is_err());
}

#[test]
fn regression_fstring_requires_serialized_terminator() {
    let mut data = 1i32.to_le_bytes().to_vec();
    data.extend(1i32.to_le_bytes());
    data.push(b'N');
    data.extend(0i32.to_le_bytes());
    assert!(LocresFile::parse(&data, "unterminated.locres").is_err());
}

#[test]
fn regression_ambiguous_flat_ids_do_not_translate_multiple_entries() {
    let mut file = LocresFile {
        version: LocresVersion::Compact,
        namespaces: vec![
            LocresNamespace {
                name: "a/b".into(),
                name_hash: 0,
                strings: vec![LocresString {
                    key: "c".into(),
                    value: "first".into(),
                    source_string_hash: 10,
                    key_hash: 0,
                }],
            },
            LocresNamespace {
                name: "a".into(),
                name_hash: 0,
                strings: vec![LocresString {
                    key: "b/c".into(),
                    value: "second".into(),
                    source_string_hash: 20,
                    key_hash: 0,
                }],
            },
        ],
    };
    let before = file.clone();
    assert_eq!(
        file.apply_translations(&HashMap::from([("a/b/c".into(), "Hola".into())])),
        0
    );
    assert_eq!(file, before);
}

#[test]
fn shared_string_table_expansion_is_rejected_before_cloning_values() {
    // < 0.5 MiB serialized input would otherwise clone over 156 MiB of values.
    // No large expanded buffer is constructed by this fixture or hardened parser.
    let count = 40_000i32;
    let mut bytes = LOCRES_MAGIC.to_vec();
    bytes.push(1);
    bytes.extend(0i64.to_le_bytes());
    bytes.extend(1i32.to_le_bytes());
    bytes.extend(0i32.to_le_bytes());
    bytes.extend(count.to_le_bytes());
    for _ in 0..count {
        bytes.extend(0i32.to_le_bytes());
        bytes.extend(42u32.to_le_bytes());
        bytes.extend(0i32.to_le_bytes());
    }
    let offset = bytes.len() as i64;
    bytes[17..25].copy_from_slice(&offset.to_le_bytes());
    bytes.extend(1i32.to_le_bytes());
    bytes.extend(4097i32.to_le_bytes());
    bytes.extend(vec![b'X'; 4096]);
    bytes.push(0);
    assert!(bytes.len() < 500_000);
    let error = LocresFile::parse(&bytes, "expanded.locres").unwrap_err();
    assert!(
        error.message.contains("decoded LocRes allocation"),
        "{}",
        error.message
    );
}

#[test]
fn locres_string_table_counts_and_references_are_bounded() {
    let mut bytes = sample(LocresVersion::Optimized).serialize().unwrap();
    let offset = i64::from_le_bytes(bytes[17..25].try_into().unwrap()) as usize;
    bytes[offset..offset + 4].copy_from_slice(&1000i32.to_le_bytes());
    assert!(LocresFile::parse(&bytes, "bad-count")
        .unwrap_err()
        .message
        .contains("remaining bytes"));
    let mut bytes = sample(LocresVersion::Optimized).serialize().unwrap();
    let len = bytes.len();
    bytes[len - 4..].copy_from_slice(&(-1i32).to_le_bytes());
    assert!(LocresFile::parse(&bytes, "bad-refcount")
        .unwrap_err()
        .message
        .contains("reference count"));
    let mut bytes = sample(LocresVersion::Optimized).serialize().unwrap();
    bytes[25..29].copy_from_slice(&0i32.to_le_bytes());
    assert!(LocresFile::parse(&bytes, "bad-total").is_err());
}

#[test]
fn locres_sparse_oversize_file_is_rejected_before_loading() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("huge.locres");
    std::fs::File::create(&path)
        .unwrap()
        .set_len(64 * 1024 * 1024 + 1)
        .unwrap();
    assert!(LocresFile::parse_path(&path)
        .unwrap_err()
        .message
        .contains("64 MiB"));
}

#[test]
fn locres_deterministic_corruption_and_truncation_corpus_never_panics() {
    let mut mutations = 0;
    let mut rejected = 0;
    let mut accepted = 0;
    for version in [
        LocresVersion::Legacy,
        LocresVersion::Compact,
        LocresVersion::Optimized,
        LocresVersion::OptimizedCityHash64Utf16,
    ] {
        let original = sample(version).serialize().unwrap();
        for cut in 0..original.len() {
            let parsed =
                std::panic::catch_unwind(|| LocresFile::parse(&original[..cut], "truncated"));
            assert!(parsed.is_ok(), "panic at version {version:?}, prefix {cut}");
            assert!(
                parsed.unwrap().is_err(),
                "accepted truncated version {version:?}, prefix {cut}"
            );
            mutations += 1;
            rejected += 1;
        }
        for index in 0..original.len() {
            for mask in [1, 0x80, 0xff] {
                let mut data = original.clone();
                data[index] ^= mask;
                let parsed = std::panic::catch_unwind(|| LocresFile::parse(&data, "mutated"));
                assert!(
                    parsed.is_ok(),
                    "panic at version {version:?}, byte {index}, mask {mask}"
                );
                mutations += 1;
                match parsed.unwrap() {
                    Ok(file) => {
                        let serialized = file.serialize().unwrap();
                        let reparsed = LocresFile::parse(&serialized, "roundtrip").unwrap();
                        assert!(file.semantic_eq(&reparsed));
                        accepted += 1;
                    }
                    Err(_) => {
                        rejected += 1;
                    }
                }
            }
        }
    }
    eprintln!("LOCRES_CORPUS cases={mutations} rejected={rejected} accepted_semantic_roundtrips={accepted}");
}

#[test]
fn locres_translation_preserves_hashes_and_embedded_nuls() {
    for version in [
        LocresVersion::Legacy,
        LocresVersion::Compact,
        LocresVersion::Optimized,
        LocresVersion::OptimizedCityHash64Utf16,
    ] {
        let mut file = sample(version);
        file.namespaces[0].strings[0].value = "ascii\0".into();
        let parsed = LocresFile::parse(&file.serialize().unwrap(), "nul").unwrap();
        assert!(file.semantic_eq(&parsed));
        let before_hash = file.namespaces[0].strings[0].source_string_hash;
        let before_key = file.namespaces[0].strings[0].key_hash;
        let before_namespace = file.namespaces[0].name_hash;
        assert_eq!(
            file.apply_translations(&HashMap::from([(
                "UI/Hello".into(),
                "Hola 世界 mucho más largo".into()
            )])),
            1
        );
        let reparsed = LocresFile::parse(&file.serialize().unwrap(), "translated").unwrap();
        assert_eq!(
            reparsed.namespaces[0].strings[0].source_string_hash,
            before_hash
        );
        assert_eq!(reparsed.namespaces[0].strings[0].key_hash, before_key);
        assert_eq!(reparsed.namespaces[0].name_hash, before_namespace);
    }
}
