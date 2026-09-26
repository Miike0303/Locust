use super::*;

#[test]
fn direct_output_matches_frozen_staged_writer_for_all_supported_layouts() {
    let mut cases = 0;
    for version in [6, 7, 8] {
        for end in [false, true] {
            for pad in [false, true] {
                for compressed in [false, true] {
                    for cached in [false, true] {
                        let before = vec![0x42; 2048];
                        let target = vec![0x51; 1024];
                        let ress = b"opaque resource bytes\0\xff";
                        let source = build_test_bundle(
                            &[
                                ("before", &before),
                                ("target", &target),
                                ("payload.resS", ress),
                            ],
                            compressed,
                            version,
                            pad,
                            end,
                        );
                        let mut archive = UnityFsArchive::parse(source, "fixture").unwrap();
                        archive.resize_node("target", &vec![0x61; 2305]).unwrap();
                        if !cached {
                            archive.discard_storage_cache();
                        }
                        let reference = staged_reference(&archive, "reference").unwrap();
                        let actual = archive.write_bytes().unwrap();
                        assert_eq!(actual,reference,"v={version} end={end} pad={pad} compressed={compressed} cached={cached}");
                        let parsed = UnityFsArchive::parse(actual, "parsed").unwrap();
                        assert_eq!(parsed.uncompressed, archive.uncompressed);
                        cases += 1;
                    }
                }
            }
        }
    }
    println!("UNITY_DIRECT_WRITER_MATRIX cases={cases}");
}

#[test]
fn staging_budget_and_recompression_roundtrip_without_large_test_allocations() {
    let payload = vec![0x22; 64 * 1024];
    let source = build_test_bundle(&[("large", &payload)], false, 8, true, true);
    let mut archive = UnityFsArchive::parse(source, "fixture").unwrap();
    archive.discard_storage_cache();
    archive.blocks[0].flags = COMPRESSION_LZ4 as u16;
    let plan = plan_blocks(&archive, "plan", 0).unwrap();
    assert_eq!(plan.staged_capacity, 0);
    assert!(matches!(plan.pieces.as_slice(), [PlannedBlock::Recompress]));
    let expected = staged_reference(&archive, "reference").unwrap();
    let bytes = write_archive_with_staging_limit(&archive, "forced limited staging", 0).unwrap();
    assert_eq!(bytes, expected);
    assert_eq!(
        UnityFsArchive::parse(bytes, "result").unwrap().uncompressed,
        payload
    );
}

#[test]
fn uncached_raw_blocks_are_borrowed_without_staging_copies() {
    let source = build_test_bundle(&[("raw", b"uncompressed contents")], false, 6, false, false);
    let mut archive = UnityFsArchive::parse(source, "raw").unwrap();
    archive.discard_storage_cache();
    let plan = plan_blocks(&archive, "raw", 0).unwrap();
    assert_eq!(plan.staged_capacity, 0);
    assert!(matches!(
        plan.pieces.as_slice(),
        [PlannedBlock::Ready(Cow::Borrowed(_))]
    ));
    assert_eq!(
        archive.write_bytes().unwrap(),
        staged_reference(&archive, "reference").unwrap()
    );
}

#[test]
fn stale_cache_is_revalidated_before_borrowing_output_bytes() {
    let source = build_test_bundle(&[("text", &vec![0x42; 4096])], true, 8, true, true);
    let mut archive = UnityFsArchive::parse(source, "stale").unwrap();
    archive.uncompressed[11] = 0x99;
    let plan = plan_blocks(&archive, "stale", MAX_STAGED_COMPRESSED_BYTES).unwrap();
    assert_eq!(plan.reused, 0);
    let after = UnityFsArchive::parse(archive.write_bytes().unwrap(), "result").unwrap();
    assert_eq!(after.uncompressed[11], 0x99);
}

#[test]
fn malformed_public_sizes_and_directory_paths_return_errors_without_input_changes() {
    let source = build_test_bundle(&[("node", b"payload")], true, 8, true, false);
    for bad in 0..7 {
        let mut archive = UnityFsArchive::parse(source.clone(), "invalid").unwrap();
        match bad {
            0 => archive.blocks[0].uncompressed_size = u32::MAX,
            1 => archive.nodes[0].size = i64::MAX,
            2 => archive.nodes[0].path = "embedded\0nul".into(),
            3 => {
                archive.blocks[0].uncompressed_size = u32::MAX;
                archive.blocks.push(archive.blocks[0].clone());
            }
            4 => archive.nodes[0].offset = -1,
            5 => archive.header.unity_version = "embedded\0nul".into(),
            _ => archive.header.unity_revision = "embedded\0nul".into(),
        }
        let before = archive.uncompressed.clone();
        assert!(archive.write_bytes().is_err());
        assert_eq!(archive.uncompressed, before);
    }
}

fn staged_reference(archive: &UnityFsArchive, label: &str) -> Result<Vec<u8>, UnityFsError> {
    let expected: usize = archive
        .blocks
        .iter()
        .map(|b| b.uncompressed_size as usize)
        .sum();
    if archive.uncompressed.len() != expected {
        return Err(err(
            label,
            format!(
                "uncompressed length {} != sum of block sizes {expected}",
                archive.uncompressed.len()
            ),
        ));
    }

    let mut new_blocks = Vec::with_capacity(archive.blocks.len());
    let mut packed_data = Vec::new();
    let mut cursor = 0usize;
    let mut reused_blocks = 0usize;
    for (index, block) in archive.blocks.iter().enumerate() {
        let n = block.uncompressed_size as usize;
        let slice = &archive.uncompressed[cursor..cursor + n];
        cursor += n;
        if let Some(encoded) = cached_encoded(archive, index, block, slice, label) {
            reused_blocks += 1;
            new_blocks.push(block.clone());
            packed_data.extend_from_slice(encoded);
        } else {
            let (compr, flags) = pack_block(slice, block);
            new_blocks.push(StorageBlock {
                uncompressed_size: block.uncompressed_size,
                compressed_size: compr.len() as u32,
                flags,
            });
            packed_data.extend_from_slice(&compr);
        }
    }
    tracing::debug!(
        reused_blocks,
        rebuilt_blocks = new_blocks.len() - reused_blocks,
        "UnityFS storage block reuse"
    );

    let mut info_plain = Vec::new();
    info_plain.extend_from_slice(&archive.info_hash);
    write_i32_be(&mut info_plain, new_blocks.len() as i32);
    for b in &new_blocks {
        write_u32_be(&mut info_plain, b.uncompressed_size);
        write_u32_be(&mut info_plain, b.compressed_size);
        write_u16_be(&mut info_plain, b.flags);
    }
    write_i32_be(&mut info_plain, archive.nodes.len() as i32);
    for n in &archive.nodes {
        write_i64_be(&mut info_plain, n.offset);
        write_i64_be(&mut info_plain, n.size);
        write_u32_be(&mut info_plain, n.flags);
        write_cstring(&mut info_plain, &n.path);
    }

    let (info_compr, info_comp_type) = pack_blob(&info_plain);

    let mut flags = archive.header.flags;
    flags &= !COMPRESSION_MASK;
    flags |= info_comp_type;
    flags |= FLAG_COMBINED;

    let version = archive.header.version;
    let info_at_end = flags & FLAG_BLOCKS_INFO_AT_END != 0;
    let pad_start = flags & FLAG_BLOCK_INFO_PAD_START != 0;

    let mut out = Vec::new();
    write_cstring(&mut out, UNITY_FS_SIGNATURE);
    write_u32_be(&mut out, version);
    write_cstring(&mut out, &archive.header.unity_version);
    write_cstring(&mut out, &archive.header.unity_revision);
    let size_pos = out.len();
    write_i64_be(&mut out, 0);
    write_u32_be(&mut out, info_compr.len() as u32);
    write_u32_be(&mut out, info_plain.len() as u32);
    write_u32_be(&mut out, flags);

    if version >= 7 {
        pad_to(&mut out, 16);
    }

    if info_at_end {
        if pad_start {
            pad_to(&mut out, 16);
        }
        out.extend_from_slice(&packed_data);
        out.extend_from_slice(&info_compr);
    } else {
        out.extend_from_slice(&info_compr);
        if pad_start {
            pad_to(&mut out, 16);
        }
        out.extend_from_slice(&packed_data);
    }

    let total = out.len() as i64;
    out[size_pos..size_pos + 8].copy_from_slice(&total.to_be_bytes());
    Ok(out)
}
