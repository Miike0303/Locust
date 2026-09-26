//! Neutral bounded scale/corruption fixture. Writer is linear and independent
//! of the reader; one named legacy LocRes per 64-byte logical block.
use locust_core::extraction::FormatPlugin;
use locust_formats::unreal::UnrealPlugin;
use locust_formats::unreal_iostore_native::read_index;
use std::{fs, path::PathBuf, time::Instant};

struct Fixture {
    dir: tempfile::TempDir,
    toc: PathBuf,
    bytes: Vec<u8>,
    payload: Vec<u8>,
    blocks: usize,
    directory: usize,
}
fn put32(v: &mut Vec<u8>, n: u32) {
    v.extend(n.to_le_bytes());
}
fn string(v: &mut Vec<u8>, s: &str) {
    put32(v, s.len() as u32 + 1);
    v.extend(s.as_bytes());
    v.push(0);
}
fn set32(v: &mut [u8], at: usize, n: u32) {
    v[at..at + 4].copy_from_slice(&n.to_le_bytes());
}
impl Fixture {
    fn new(count: usize) -> Self {
        Self::build(count, 64, "../../../Game/Content/")
    }
    fn build(count: usize, block_size: usize, mount: &str) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let toc = dir.path().join("scale.utoc");
        let mut payload = Vec::new();
        put32(&mut payload, 1);
        string(&mut payload, "Menu");
        put32(&mut payload, 1);
        string(&mut payload, "Key");
        put32(&mut payload, 0x12345678);
        string(&mut payload, "Hello traveler");
        assert!(payload.len() < 64);
        let mut directory = Vec::new();
        string(&mut directory, mount);
        put32(&mut directory, 1);
        for n in [u32::MAX, u32::MAX, u32::MAX, 0] {
            put32(&mut directory, n);
        }
        put32(&mut directory, count as u32);
        for i in 0..count {
            for n in [
                i as u32,
                if i + 1 < count {
                    i as u32 + 1
                } else {
                    u32::MAX
                },
                i as u32,
            ] {
                put32(&mut directory, n);
            }
        }
        put32(&mut directory, count as u32);
        for i in 0..count {
            string(&mut directory, &format!("resource{i:06}.locres"));
        }
        let mut bytes = vec![0u8; 144];
        bytes[..16].copy_from_slice(b"-==--==--==--==-");
        bytes[16] = 8;
        for (at, n) in [
            (20, 144),
            (24, count as u32),
            (28, (count * 64 / block_size) as u32),
            (32, 12),
            (40, 32),
            (44, block_size as u32),
            (48, directory.len() as u32),
            (52, 1),
        ] {
            set32(&mut bytes, at, n);
        }
        bytes[80] = 8;
        bytes[88..96].copy_from_slice(&u64::MAX.to_le_bytes());
        for i in 0..count {
            bytes.extend((i as u64 + 1).to_le_bytes());
            bytes.extend([0, 0, 0, 7]);
        }
        for i in 0..count {
            bytes.extend_from_slice(&(i as u64 * 64).to_be_bytes()[3..]);
            bytes.extend_from_slice(&(payload.len() as u64).to_be_bytes()[3..]);
        }
        let blocks = bytes.len();
        for i in 0..count * 64 / block_size {
            bytes.extend_from_slice(&(i as u64 * block_size as u64).to_le_bytes()[..5]);
            bytes.extend([block_size as u8, 0, 0, block_size as u8, 0, 0, 0]);
        }
        let directory_pos = bytes.len();
        bytes.extend(directory);
        for _ in 0..count {
            bytes.extend_from_slice(&blake3::hash(&payload).as_bytes()[..20]);
            bytes.extend([0; 4]);
        }
        let mut ucas = vec![0u8; count * 64];
        for chunk in ucas.chunks_exact_mut(64) {
            chunk[..payload.len()].copy_from_slice(&payload);
        }
        fs::write(toc.with_extension("ucas"), ucas).unwrap();
        fs::write(&toc, &bytes).unwrap();
        Self {
            dir,
            toc,
            bytes,
            payload,
            blocks,
            directory: directory_pos,
        }
    }
}

#[test]
#[ignore = "explicit performance measurement; no wall-clock assertion"]
fn measure_ten_and_twenty_thousand_locres() {
    for count in [10_000, 20_000] {
        let f = Fixture::new(count);
        let start = Instant::now();
        let index = read_index(&f.toc).unwrap();
        let index_time = start.elapsed();
        let start = Instant::now();
        for record in index.records.iter().rev() {
            assert_eq!(index.read_locres(record).unwrap(), f.payload);
        }
        let read_time = start.elapsed();
        let start = Instant::now();
        let entries = UnrealPlugin::new().extract(&f.toc).unwrap();
        let extract_time = start.elapsed();
        assert_eq!(entries.len(), count);
        eprintln!("NATIVE_SCALE count={count} toc_bytes={} ucas_bytes={} index_ms={} read_ms={} extract_ms={}",f.bytes.len(),count*64,index_time.as_millis(),read_time.as_millis(),extract_time.as_millis());
        assert!(f.dir.path().exists());
    }
}

#[test]
fn neutral_fixture_reads_small_resources() {
    let f = Fixture::new(8);
    let index = read_index(&f.toc).unwrap();
    assert!(f.blocks < f.directory);
    for record in &index.records {
        assert_eq!(index.read_locres(record).unwrap(), f.payload);
    }
}

#[test]
fn amplified_mount_is_rejected_before_expanding_all_virtual_paths() {
    let f = Fixture::build(1100, 64, &"M".repeat(16000));
    let error = read_index(&f.toc)
        .err()
        .expect("mount expansion must be charged against the aggregate path budget");
    assert!(
        error.message.contains("path") && error.message.contains("limit"),
        "{error}"
    );
}

#[test]
#[ignore = "explicit multi-block performance measurement"]
fn measure_multiblock_resources() {
    let f = Fixture::build(10_000, 16, "../../../Game/Content/");
    let index = read_index(&f.toc).unwrap();
    let start = Instant::now();
    for record in &index.records {
        assert_eq!(index.read_locres(record).unwrap(), f.payload);
    }
    eprintln!(
        "NATIVE_MULTIBLOCK count=10000 blocks_per_resource=4 read_ms={}",
        start.elapsed().as_millis()
    );
}

#[test]
fn immutable_lookup_rejects_forged_records_and_public_enumeration_edits() {
    let f = Fixture::new(2);
    let mut index = read_index(&f.toc).unwrap();
    let original = index.records[0].clone();
    for field in 0..4 {
        let mut forged = original.clone();
        match field {
            0 => forged.name.push('X'),
            1 => forged.virtual_path.push('X'),
            2 => forged.chunk_index = 1,
            _ => forged.chunk_type = 1,
        }
        assert!(index.read_locres(&forged).is_err());
        index.records[0] = forged.clone();
        assert!(
            index.read_locres(&forged).is_err(),
            "mutating public enumeration cannot authorize a forged record"
        );
    }
    index.records.clear();
    assert_eq!(
        index
            .read_locres(index.find_record(&original.name).unwrap())
            .unwrap(),
        f.payload
    );
}

#[test]
fn second_read_reopens_replaced_and_truncated_partition() {
    let f = Fixture::new(2);
    let index = read_index(&f.toc).unwrap();
    let record = &index.records[0];
    assert_eq!(index.read_locres(record).unwrap(), f.payload);
    let ucas = f.toc.with_extension("ucas");
    let old = f.toc.with_extension("original-ucas");
    fs::rename(&ucas, &old).unwrap();
    let mut corrupted = fs::read(&old).unwrap();
    corrupted[25] ^= 0x55;
    fs::write(&ucas, corrupted).unwrap();
    assert!(index
        .read_locres(record)
        .unwrap_err()
        .message
        .contains("hash mismatch"));
    fs::write(&ucas, [0; 4]).unwrap();
    assert!(index
        .read_locres(record)
        .unwrap_err()
        .message
        .contains("partition file"));
}

#[test]
fn deterministic_toc_and_ucas_mutations_never_panic_or_return_unverified_bytes() {
    let f = Fixture::new(2);
    let mut cases = 0;
    // Every byte position, two nonzero masks; includes all header counts,
    // links, offsets, strings, block descriptors, and BLAKE3 metadata.
    for offset in 0..f.bytes.len() {
        for mask in [1, 0x80] {
            let mut mutant = f.bytes.clone();
            mutant[offset] ^= mask;
            fs::write(&f.toc, mutant).unwrap();
            let outcome = std::panic::catch_unwind(|| {
                if let Ok(index) = read_index(&f.toc) {
                    for record in &index.records {
                        if let Ok(data) = index.read_locres(record) {
                            assert_eq!(
                                data, f.payload,
                                "unverified bytes at TOC mutation {offset}/{mask}"
                            );
                        }
                    }
                }
            });
            assert!(outcome.is_ok(), "panic at TOC mutation {offset}/{mask}");
            cases += 1;
        }
    }
    for end in 0..f.bytes.len() {
        fs::write(&f.toc, &f.bytes[..end]).unwrap();
        assert!(
            read_index(&f.toc).is_err(),
            "truncated index accepted at {end}"
        );
        cases += 1;
    }
    fs::write(&f.toc, &f.bytes).unwrap();
    let ucas = f.toc.with_extension("ucas");
    let base = fs::read(&ucas).unwrap();
    let index = read_index(&f.toc).unwrap();
    for offset in 0..f.payload.len() {
        let mut mutant = base.clone();
        mutant[offset] ^= 0x40;
        fs::write(&ucas, mutant).unwrap();
        assert!(
            index.read_locres(&index.records[0]).is_err(),
            "unchecked UCAS mutation {offset}"
        );
        cases += 1;
    }
    eprintln!("NATIVE_CORPUS cases={cases}");
}

#[test]
fn native_duplicate_tuples_error_and_slash_identities_translate_independently() {
    for pair in [[("a", "b"), ("a", "b")], [("a", "b/c"), ("a/b", "c")]] {
        let mut f = Fixture::new(1);
        let mut payload = Vec::new();
        put32(&mut payload, 2);
        for (namespace, key) in pair {
            string(&mut payload, namespace);
            put32(&mut payload, 1);
            string(&mut payload, key);
            put32(&mut payload, 123);
            string(&mut payload, "x");
        }
        assert!(payload.len() <= 64);
        f.bytes[144 + 12 + 5..144 + 12 + 10]
            .copy_from_slice(&(payload.len() as u64).to_be_bytes()[3..]);
        let meta = f.bytes.len() - 24;
        f.bytes[meta..meta + 20].copy_from_slice(&blake3::hash(&payload).as_bytes()[..20]);
        let mut ucas = vec![0; 64];
        ucas[..payload.len()].copy_from_slice(&payload);
        fs::write(&f.toc, &f.bytes).unwrap();
        fs::write(f.toc.with_extension("ucas"), &ucas).unwrap();
        let plugin = UnrealPlugin::new();
        if pair[0] == pair[1] {
            assert!(plugin
                .extract(&f.toc)
                .unwrap_err()
                .to_string()
                .contains("duplicate"));
            assert!(!f.dir.path().join("scale_LOCUST_P.pak").exists());
            continue;
        }
        let mut entries = plugin.extract(&f.toc).unwrap();
        assert_eq!(entries.len(), 2);
        assert_ne!(entries[0].id, entries[1].id);
        for entry in &mut entries {
            entry.translation = Some("translated".into());
        }
        let report = plugin.inject(&f.toc, &entries).unwrap();
        assert_eq!(report.strings_written, 2);
        assert_eq!(report.strings_skipped, 0);
        assert!(f.dir.path().join("scale_LOCUST_P.pak").exists());
        assert_eq!(fs::read(&f.toc).unwrap(), f.bytes);
        assert_eq!(fs::read(f.toc.with_extension("ucas")).unwrap(), ucas);
    }
}

#[test]
fn resource_reads_more_partitions_than_the_bounded_handle_cache() {
    let mut f = Fixture::build(1, 1, "../../../Game/Content/");
    let ucas = fs::read(f.toc.with_extension("ucas")).unwrap();
    set32(&mut f.bytes, 52, 64);
    f.bytes[88..96].copy_from_slice(&1u64.to_le_bytes());
    fs::write(&f.toc, &f.bytes).unwrap();
    for (i, byte) in ucas.into_iter().enumerate() {
        let path = if i == 0 {
            f.toc.with_extension("ucas")
        } else {
            f.dir.path().join(format!("scale_s{i}.ucas"))
        };
        fs::write(path, [byte]).unwrap();
    }
    let index = read_index(&f.toc).unwrap();
    assert_eq!(index.read_locres(&index.records[0]).unwrap(), f.payload);
}
