use super::*;

fn fixture(gaps: bool) -> UnityFsArchive {
    let a: Vec<_> = (0u8..31).collect();
    let b: Vec<_> = (31u8..77).collect();
    let c: Vec<_> = (77u8..111).collect();
    let bytes = build_test_bundle(
        &[("first", &a), ("middle", &b), ("last.resS", &c)],
        true,
        8,
        true,
        true,
    );
    let mut archive = UnityFsArchive::parse(bytes, "fixture").unwrap();
    if gaps {
        archive.nodes[0].offset = 7;
        archive.nodes[0].size = 19;
        archive.nodes[1].offset = 39;
        archive.nodes[1].size = 23;
        archive.nodes[2].offset = 81;
        archive.nodes[2].size = 17;
    }
    archive.blocks = vec![StorageBlock {
        uncompressed_size: 111,
        compressed_size: 0,
        flags: 2,
    }];
    archive.discard_storage_cache();
    UnityFsArchive::parse(archive.write_bytes().unwrap(), "fixture").unwrap()
}

// Independent reference: concatenate original spans in physical node order,
// matching the frozen v2 layout without modifying the source buffer.
fn reference(
    archive: &UnityFsArchive,
    target: &str,
    replacement: &[u8],
) -> (Vec<u8>, Vec<DirectoryNode>) {
    let mut nodes = archive.nodes.clone();
    let order = archive.relayout_order().unwrap();
    let mut out = Vec::new();
    let mut cursor = 0;
    for i in order {
        let n = &archive.nodes[i];
        let start = n.offset as usize;
        let end = start + n.size as usize;
        out.extend_from_slice(&archive.uncompressed[cursor..start]);
        out.resize(out.len() + (start.wrapping_sub(out.len()) & 15), 0);
        nodes[i].offset = out.len() as i64;
        if n.path == target {
            out.extend_from_slice(replacement);
            nodes[i].size = replacement.len() as i64;
        } else {
            out.extend_from_slice(&archive.uncompressed[start..end]);
        }
        cursor = end;
    }
    out.extend_from_slice(&archive.uncompressed[cursor..]);
    (out, nodes)
}

#[test]
fn every_node_growth_shrink_and_padding_matches_frozen_layout_in_place() {
    let mut cases = 0;
    for gaps in [false, true] {
        for target in ["first", "middle", "last.resS"] {
            for length in [0, 1, 2, 15, 16, 17, 19, 23, 31, 34, 46, 63, 64, 127, 256] {
                let mut archive = fixture(gaps);
                let replacement: Vec<_> = (0..length).map(|i| (i % 251) as u8).collect();
                let (expected, nodes) = reference(&archive, target, &replacement);
                archive.uncompressed.try_reserve_exact(512).unwrap();
                let ptr = archive.uncompressed.as_ptr();
                let capacity = archive.uncompressed.capacity();
                archive.resize_node(target, &replacement).unwrap();
                assert_eq!(
                    archive.uncompressed, expected,
                    "gaps={gaps} target={target} length={length}"
                );
                assert_eq!(
                    archive.uncompressed.as_ptr(),
                    ptr,
                    "sufficient capacity must avoid reallocation"
                );
                assert_eq!(archive.uncompressed.capacity(), capacity);
                for (a, b) in archive.nodes.iter().zip(nodes) {
                    assert_eq!((a.offset, a.size), (b.offset, b.size));
                }
                let after =
                    UnityFsArchive::parse(archive.write_bytes().unwrap(), "result").unwrap();
                assert_eq!(after.uncompressed, expected);
                cases += 1;
            }
        }
    }
    println!("UNITY_INPLACE_MATRIX cases={cases}");
}

#[test]
fn zero_size_nodes_same_offsets_and_unsorted_directory_keep_exact_layout() {
    let mut archive = fixture(false);
    archive.nodes.insert(
        0,
        DirectoryNode {
            offset: 31,
            size: 0,
            flags: 4,
            path: "empty".into(),
        },
    );
    archive.nodes.reverse();
    for (target, text) in [
        ("empty", b"new data".as_slice()),
        ("first", b"".as_slice()),
        ("middle", b"smaller".as_slice()),
        (
            "last.resS",
            b"a much longer replacement for the final streaming node".as_slice(),
        ),
    ] {
        let (expected, nodes) = reference(&archive, target, text);
        archive.resize_node(target, text).unwrap();
        assert_eq!(archive.uncompressed, expected);
        for (a, b) in archive.nodes.iter().zip(nodes) {
            assert_eq!((a.offset, a.size), (b.offset, b.size));
        }
    }
}

#[test]
fn exact_size_bounds_allow_shrink_at_limit_and_reject_overflow_without_allocation() {
    assert_eq!(
        checked_resize_len(MAX_UNCOMPRESSED_BYTES, 100, 20, 12, "test").unwrap(),
        MAX_UNCOMPRESSED_BYTES - 68
    );
    assert_eq!(
        checked_resize_len(MAX_UNCOMPRESSED_BYTES, 100, 100, 0, "test").unwrap(),
        MAX_UNCOMPRESSED_BYTES
    );
    assert!(checked_resize_len(MAX_UNCOMPRESSED_BYTES, 100, 100, 1, "test").is_err());
    assert!(checked_resize_len(10, 11, 0, 0, "test").is_err());
    assert!(checked_resize_len(10, 1, usize::MAX, 0, "test").is_err());
    assert!(checked_resize_len(10, 1, 1, usize::MAX, "test").is_err());
}

#[test]
fn failed_layout_or_block_plan_leaves_bytes_nodes_blocks_and_cache_unchanged() {
    for bad in [0, 1, 2] {
        let mut archive = fixture(false);
        match bad {
            0 => archive.nodes[1].offset = 1,
            1 => archive.blocks[0].uncompressed_size = u32::MAX,
            _ => archive.header.flags |= 0x1000,
        }
        let bytes = archive.uncompressed.clone();
        let nodes = archive.nodes.clone();
        let blocks = archive.blocks.clone();
        let cache = archive.storage_cache.len();
        assert!(archive.resize_node("middle", b"change").is_err());
        assert_eq!(archive.uncompressed, bytes);
        assert_eq!(archive.blocks, blocks);
        assert_eq!(archive.storage_cache.len(), cache);
        for (a, b) in archive.nodes.iter().zip(nodes) {
            assert_eq!((a.offset, a.size), (b.offset, b.size));
        }
    }
}

#[test]
fn decode_headroom_is_bounded_and_small_growth_keeps_the_original_allocation() {
    assert_eq!(storage_capacity_hint(0), 0);
    assert_eq!(storage_capacity_hint(1024), 1040);
    assert_eq!(
        storage_capacity_hint(MAX_UNCOMPRESSED_BYTES),
        MAX_UNCOMPRESSED_BYTES
    );
    assert_eq!(
        storage_capacity_hint(MAX_UNCOMPRESSED_BYTES - 1),
        MAX_UNCOMPRESSED_BYTES
    );
    assert_eq!(storage_capacity_hint(700 * 1024 * 1024), 708 * 1024 * 1024);
    let text = vec![0x22; 1024 * 1024];
    let source = build_test_bundle(
        &[("target", &text), ("tail", b"opaque")],
        true,
        8,
        false,
        false,
    );
    let mut archive = UnityFsArchive::parse(source, "headroom").unwrap();
    let old = archive.uncompressed.as_ptr();
    let replacement = vec![0x44; text.len() + 4096];
    archive.resize_node("target", &replacement).unwrap();
    assert_eq!(archive.uncompressed.as_ptr(), old);
    assert_eq!(
        archive.node_bytes(archive.node("tail").unwrap()).unwrap(),
        b"opaque"
    );
}
