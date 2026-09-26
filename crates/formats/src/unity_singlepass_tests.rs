use super::*;

#[test]
fn reservation_strategies_match_staged_output_with_front_compaction() {
    let path = "repeated-directory-name/".repeat(256);
    let payload = vec![0x42; 32 * 1024];
    for version in [6, 7, 8] {
        for at_end in [false, true] {
            for padded in [false, true] {
                for compressed in [false, true] {
                    let source = build_test_bundle(
                        &[(&path, &payload), ("other.resS", b"opaque")],
                        compressed,
                        version,
                        padded,
                        at_end,
                    );
                    let mut archive = UnityFsArchive::parse(source, "fixture").unwrap();
                    archive.resize_node(&path, &vec![0x61; 36 * 1024]).unwrap();
                    let expected = write_archive_with_staging_limit(
                        &archive,
                        "staged",
                        MAX_STAGED_COMPRESSED_BYTES,
                    )
                    .unwrap();
                    for upper in [false, true] {
                        let actual = write_archive_single_pass(&archive, "single", upper).unwrap();
                        assert_eq!(actual, expected);
                        assert_eq!(
                            UnityFsArchive::parse(actual, "roundtrip")
                                .unwrap()
                                .uncompressed,
                            archive.uncompressed
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn conservative_hint_can_grow_without_a_second_payload_buffer() {
    let mut payload = vec![0x43; 9 * 1024 * 1024];
    payload[5 * 1024 * 1024..].fill(0x61);
    let source = build_test_bundle(&[("raw", &payload)], false, 8, true, false);
    let mut archive = UnityFsArchive::parse(source, "fixture").unwrap();
    archive.discard_storage_cache();
    archive.blocks = vec![
        StorageBlock {
            uncompressed_size: 5 * 1024 * 1024,
            compressed_size: 0,
            flags: 0,
        },
        StorageBlock {
            uncompressed_size: 4 * 1024 * 1024,
            compressed_size: 0,
            flags: 0,
        },
    ];
    let actual = write_archive_single_pass(&archive, "grow", false).unwrap();
    let expected =
        write_archive_with_staging_limit(&archive, "staged", MAX_STAGED_COMPRESSED_BYTES).unwrap();
    assert_eq!(actual, expected);
    assert_eq!(
        UnityFsArchive::parse(actual, "result")
            .unwrap()
            .uncompressed,
        payload
    );
}

#[test]
fn output_reservation_rejects_overflow_and_bounds_without_changes() {
    let mut out = vec![1, 2, 3];
    assert!(reserve_output(&mut out, usize::MAX, usize::MAX, "overflow").is_err());
    assert!(reserve_output(&mut out, 4, 6, "bound").is_err());
    assert_eq!(out, [1, 2, 3]);
    assert!(reserve_output(&mut out, 10, 13, "growth").unwrap());
    assert!(out.capacity() >= 13);
    assert_eq!(out, [1, 2, 3]);
}

#[test]
fn zero_length_payload_keeps_both_index_placements_byte_identical() {
    for at_end in [false, true] {
        let source = build_test_bundle(&[("empty", b"")], false, 8, true, at_end);
        let archive = UnityFsArchive::parse(source, "empty").unwrap();
        assert_eq!(
            write_archive_single_pass(&archive, "single", false).unwrap(),
            write_archive_with_staging_limit(&archive, "staged", MAX_STAGED_COMPRESSED_BYTES)
                .unwrap()
        );
    }
}
