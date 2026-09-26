use super::*;

fn fixture(lz4: bool) -> UnityFsArchive {
    let before = vec![0x11; 128];
    let target = vec![0x22; 128];
    let ress = vec![0x33; 256];
    let bytes = build_test_bundle(
        &[
            ("before", &before),
            ("target", &target),
            ("payload.resS", &ress),
        ],
        lz4,
        8,
        true,
        true,
    );
    let mut archive = UnityFsArchive::parse(bytes, "fixture").unwrap();
    archive.blocks = (0..8)
        .map(|_| StorageBlock {
            uncompressed_size: 64,
            compressed_size: 0,
            flags: if lz4 { 2 } else { 0 },
        })
        .collect();
    archive.storage_cache.clear();
    UnityFsArchive::parse(archive.write_bytes().unwrap(), "fixture").unwrap()
}

fn encoded(archive: &UnityFsArchive, index: usize) -> Vec<u8> {
    let cached = archive.storage_cache[index].as_ref().unwrap();
    cached.source[cached.encoded.clone()].to_vec()
}

#[test]
fn untouched_compressed_blocks_survive_growth_shrink_and_second_relayout() {
    for compressed in [false, true] {
        let mut archive = fixture(compressed);
        let last = encoded(&archive, 7);
        let first = encoded(&archive, 0);
        archive.resize_node("target", &vec![0x44; 317]).unwrap();
        assert_eq!(
            archive.storage_cache.iter().filter(|e| e.is_some()).count(),
            6
        );
        let mut after = UnityFsArchive::parse(archive.write_bytes().unwrap(), "grown").unwrap();
        assert_eq!(encoded(&after, 0), first);
        assert_eq!(encoded(&after, after.blocks.len() - 1), last);
        assert_eq!(
            after
                .node_bytes(after.node("payload.resS").unwrap())
                .unwrap(),
            vec![0x33; 256]
        );
        assert_eq!(
            after.node_bytes(after.node("target").unwrap()).unwrap(),
            vec![0x44; 317]
        );
        after.resize_node("target", b"short").unwrap();
        after
            .resize_node("before", b"earlier resized node")
            .unwrap();
        let final_archive = UnityFsArchive::parse(after.write_bytes().unwrap(), "twice").unwrap();
        assert_eq!(
            encoded(&final_archive, final_archive.blocks.len() - 1),
            last
        );
        assert_eq!(
            final_archive
                .node_bytes(final_archive.node("target").unwrap())
                .unwrap(),
            b"short"
        );
        assert_eq!(
            final_archive
                .node_bytes(final_archive.node("before").unwrap())
                .unwrap(),
            b"earlier resized node"
        );
        assert_eq!(
            final_archive
                .node_bytes(final_archive.node("payload.resS").unwrap())
                .unwrap(),
            vec![0x33; 256]
        );
    }
}

#[test]
fn direct_public_plaintext_and_block_edits_cannot_reuse_stale_compressed_data() {
    let mut archive = fixture(true);
    let first = encoded(&archive, 0);
    let last = encoded(&archive, 7);
    archive.uncompressed[140] = 0x99;
    let after = UnityFsArchive::parse(archive.write_bytes().unwrap(), "mutated").unwrap();
    assert_eq!(after.uncompressed[140], 0x99);
    assert_eq!(encoded(&after, 0), first);
    assert_eq!(encoded(&after, 7), last);
    // Same total length with different block boundaries invalidates the indexed cache.
    archive.blocks[0].uncompressed_size = 32;
    archive.blocks[1].uncompressed_size = 96;
    let after = UnityFsArchive::parse(archive.write_bytes().unwrap(), "reblocked").unwrap();
    assert_eq!(after.uncompressed, archive.uncompressed);
}

#[test]
fn replacement_crossing_block_boundaries_preserves_only_proven_whole_spans() {
    let mut archive = fixture(true);
    archive.nodes[0].size = 150;
    archive.nodes[1].offset = 150;
    archive.nodes[1].size = 140;
    archive.nodes[2].offset = 290;
    archive.nodes[2].size = 222;
    let ress = archive
        .node_bytes(archive.node("payload.resS").unwrap())
        .unwrap()
        .to_vec();
    let expected_reused: Vec<_> = [0, 1, 5, 6, 7]
        .into_iter()
        .map(|i| encoded(&archive, i))
        .collect();
    archive
        .resize_node("target", b"new target with a different byte count")
        .unwrap();
    assert_eq!(
        archive.storage_cache.iter().filter(|c| c.is_some()).count(),
        5
    );
    let after = UnityFsArchive::parse(archive.write_bytes().unwrap(), "crossed").unwrap();
    let actual: Vec<_> = after
        .storage_cache
        .iter()
        .map(|c| {
            let c = c.as_ref().unwrap();
            c.source[c.encoded.clone()].to_vec()
        })
        .collect();
    for bytes in expected_reused {
        assert!(actual.contains(&bytes));
    }
    assert_eq!(
        after
            .node_bytes(after.node("payload.resS").unwrap())
            .unwrap(),
        ress
    );
    assert_eq!(
        after.node_bytes(after.node("target").unwrap()).unwrap(),
        b"new target with a different byte count"
    );
}

#[test]
fn same_size_replacement_and_corrupted_private_cache_fall_back_safely() {
    let mut archive = fixture(true);
    let first = encoded(&archive, 0);
    archive.replace_node("target", &[0x55; 128]).unwrap();
    let cached = archive.storage_cache[2].as_mut().unwrap();
    cached.encoded = usize::MAX - 1..usize::MAX;
    let after = UnityFsArchive::parse(archive.write_bytes().unwrap(), "same size").unwrap();
    assert_eq!(
        after.node_bytes(after.node("target").unwrap()).unwrap(),
        vec![0x55; 128]
    );
    assert_eq!(encoded(&after, 0), first);
}

#[test]
fn invalid_public_block_plan_rejects_resize_without_partial_mutation() {
    let mut archive = fixture(true);
    let original = archive.uncompressed.clone();
    archive.blocks[0].uncompressed_size = u32::MAX;
    assert!(archive.resize_node("target", b"replacement").is_err());
    assert_eq!(archive.uncompressed, original);
}

#[test]
fn original_lzma_and_lz4hc_block_encodings_remain_exact_when_untouched() {
    for compression in [1u16, 3u16] {
        let mut archive = fixture(true);
        let mut source = Vec::new();
        let mut ranges = Vec::new();
        for (i, block) in archive.blocks.iter_mut().enumerate() {
            let plain = &archive.uncompressed[i * 64..i * 64 + 64];
            let encoded = if compression == 1 {
                let mut encoded = Vec::new();
                lzma_rs::lzma_compress(&mut Cursor::new(plain), &mut encoded).unwrap();
                encoded.drain(5..13);
                encoded
            } else {
                lz4_flex::compress(plain)
            };
            let start = source.len();
            source.extend_from_slice(&encoded);
            ranges.push(start..source.len());
            block.flags = compression;
            block.compressed_size = encoded.len() as u32;
        }
        let source = Arc::new(source);
        archive.storage_cache = archive
            .blocks
            .iter()
            .zip(ranges)
            .map(|(block, encoded)| {
                Some(CachedBlock {
                    source: Arc::clone(&source),
                    encoded,
                    block: block.clone(),
                })
            })
            .collect();
        // Serialize and parse the fixture to exercise the actual wire reader.
        let mut archive =
            UnityFsArchive::parse(archive.write_bytes().unwrap(), "original compression").unwrap();
        let first = encoded(&archive, 0);
        let last = encoded(&archive, 7);
        archive.resize_node("target", b"a changed target").unwrap();
        let after = UnityFsArchive::parse(archive.write_bytes().unwrap(), "modified").unwrap();
        assert_eq!(after.blocks[0].flags, compression);
        assert_eq!(after.blocks.last().unwrap().flags, compression);
        assert_eq!(encoded(&after, 0), first);
        assert_eq!(encoded(&after, after.blocks.len() - 1), last);
        assert_eq!(
            after.node_bytes(after.node("target").unwrap()).unwrap(),
            b"a changed target"
        );
    }
}

#[test]
fn read_only_cache_discard_releases_shared_backing_and_preserves_write_correctness() {
    let mut archive = fixture(true);
    let weak = Arc::downgrade(&archive.storage_cache[0].as_ref().unwrap().source);
    let clone = archive.clone();
    archive.discard_storage_cache();
    assert!(weak.upgrade().is_some());
    drop(clone);
    assert!(weak.upgrade().is_none());
    let after = UnityFsArchive::parse(archive.write_bytes().unwrap(), "uncached").unwrap();
    assert_eq!(archive.uncompressed, after.uncompressed);
}
