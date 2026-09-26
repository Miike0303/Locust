// Frozen v4 writer retained only as a byte-level test oracle.
enum PlannedBlock<'a> {
    Ready(Cow<'a, [u8]>),
    Recompress,
}

struct BlockPlan<'a> {
    blocks: Vec<StorageBlock>,
    pieces: Vec<PlannedBlock<'a>>,
    encoded_len: usize,
    staged_capacity: usize,
    reused: usize,
}

fn plan_blocks<'a>(
    archive: &'a UnityFsArchive,
    label: &str,
    staging_limit: usize,
) -> Result<BlockPlan<'a>, UnityFsError> {
    let expected = archive
        .blocks
        .iter()
        .try_fold(0usize, |n, b| n.checked_add(b.uncompressed_size as usize))
        .filter(|&n| n == archive.uncompressed.len() && n <= MAX_UNCOMPRESSED_BYTES)
        .ok_or_else(|| err(label, "invalid storage block sizes or uncompressed length"))?;
    let mut plan = BlockPlan {
        blocks: Vec::with_capacity(archive.blocks.len()),
        pieces: Vec::with_capacity(archive.blocks.len()),
        encoded_len: 0,
        staged_capacity: 0,
        reused: 0,
    };
    let mut cursor = 0usize;
    for (index, block) in archive.blocks.iter().enumerate() {
        let n = block.uncompressed_size as usize;
        let plain = &archive.uncompressed[cursor..cursor + n];
        cursor += n;
        if let Some(encoded) = cached_encoded(archive, index, block, plain, label) {
            plan.reused += 1;
            plan.blocks.push(block.clone());
            plan.pieces
                .push(PlannedBlock::Ready(Cow::Borrowed(encoded)));
            plan.encoded_len = plan
                .encoded_len
                .checked_add(encoded.len())
                .ok_or_else(|| err(label, "encoded storage size overflow"))?;
        } else {
            let (encoded, flags) = pack_block(plain, block);
            let len = encoded.len();
            let piece = match encoded {
                Cow::Borrowed(bytes) => PlannedBlock::Ready(Cow::Borrowed(bytes)),
                Cow::Owned(bytes)
                    if bytes.capacity() <= staging_limit.saturating_sub(plan.staged_capacity) =>
                {
                    plan.staged_capacity += bytes.capacity();
                    PlannedBlock::Ready(Cow::Owned(bytes))
                }
                Cow::Owned(_) => PlannedBlock::Recompress,
            };
            plan.blocks.push(StorageBlock {
                uncompressed_size: block.uncompressed_size,
                compressed_size: u32::try_from(len)
                    .map_err(|_| err(label, "compressed block exceeds u32"))?,
                flags,
            });
            plan.pieces.push(piece);
            plan.encoded_len = plan
                .encoded_len
                .checked_add(len)
                .ok_or_else(|| err(label, "encoded storage size overflow"))?;
        }
    }
    debug_assert_eq!(cursor, expected);
    Ok(plan)
}

fn write_archive_with_staging_limit(
    archive: &UnityFsArchive,
    label: &str,
    staging_limit: usize,
) -> Result<Vec<u8>, UnityFsError> {
    let block_count =
        i32::try_from(archive.blocks.len()).map_err(|_| err(label, "too many storage blocks"))?;
    let node_count =
        i32::try_from(archive.nodes.len()).map_err(|_| err(label, "too many directory nodes"))?;
    let mut info_size = 24usize
        .checked_add(
            archive
                .blocks
                .len()
                .checked_mul(10)
                .ok_or_else(|| err(label, "block table overflow"))?,
        )
        .ok_or_else(|| err(label, "blocks-info size overflow"))?;
    for node in &archive.nodes {
        archive.node_bytes(node)?;
        if node.path.contains('\0') {
            return Err(err(label, "NUL in directory node path"));
        }
        info_size = info_size
            .checked_add(21)
            .and_then(|n| n.checked_add(node.path.len()))
            .filter(|&n| n <= MAX_INFO_BYTES)
            .ok_or_else(|| err(label, "blocks-info exceeds 64 MiB"))?;
    }
    if info_size > MAX_INFO_BYTES {
        return Err(err(label, "blocks-info exceeds 64 MiB"));
    }
    if archive.header.unity_version.contains('\0') || archive.header.unity_revision.contains('\0') {
        return Err(err(label, "NUL in UnityFS header string"));
    }
    let plan = plan_blocks(archive, label, staging_limit)?;
    let new_blocks = &plan.blocks;
    let mut info_plain = Vec::with_capacity(info_size);
    info_plain.extend_from_slice(&archive.info_hash);
    write_i32_be(&mut info_plain, block_count);
    for b in new_blocks {
        write_u32_be(&mut info_plain, b.uncompressed_size);
        write_u32_be(&mut info_plain, b.compressed_size);
        write_u16_be(&mut info_plain, b.flags);
    }
    write_i32_be(&mut info_plain, node_count);
    for n in &archive.nodes {
        write_i64_be(&mut info_plain, n.offset);
        write_i64_be(&mut info_plain, n.size);
        write_u32_be(&mut info_plain, n.flags);
        write_cstring(&mut info_plain, &n.path);
    }
    let (info_compr, info_comp_type) = pack_blob(&info_plain);
    let mut flags = archive.header.flags;
    flags = (flags & !COMPRESSION_MASK) | info_comp_type | FLAG_COMBINED;
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
    if !info_at_end {
        out.extend_from_slice(&info_compr);
    }
    if pad_start {
        pad_to(&mut out, 16);
    }
    let final_size = out
        .len()
        .checked_add(plan.encoded_len)
        .and_then(|n| n.checked_add(if info_at_end { info_compr.len() } else { 0 }))
        .ok_or_else(|| err(label, "final bundle size overflow"))?;
    let final_i64 =
        i64::try_from(final_size).map_err(|_| err(label, "final bundle size exceeds i64"))?;
    out.try_reserve_exact(final_size - out.len())
        .map_err(|e| err(label, format!("cannot reserve final UnityFS output: {e}")))?;
    let mut cursor = 0usize;
    let mut recompressed_twice = 0usize;
    for (index, piece) in plan.pieces.into_iter().enumerate() {
        let descriptor = &new_blocks[index];
        let n = descriptor.uncompressed_size as usize;
        match piece {
            PlannedBlock::Ready(bytes) => out.extend_from_slice(&bytes),
            PlannedBlock::Recompress => {
                let (bytes, flags) = pack_block(
                    &archive.uncompressed[cursor..cursor + n],
                    &archive.blocks[index],
                );
                if bytes.len() != descriptor.compressed_size as usize || flags != descriptor.flags {
                    return Err(err(
                        label,
                        "block encoding changed between sizing and writing",
                    ));
                }
                out.extend_from_slice(&bytes);
                recompressed_twice += 1;
            }
        }
        cursor += n;
    }
    if info_at_end {
        out.extend_from_slice(&info_compr);
    }
    if out.len() != final_size {
        return Err(err(label, "final bundle size disagrees with block plan"));
    }
    out[size_pos..size_pos + 8].copy_from_slice(&final_i64.to_be_bytes());
    tracing::debug!(
        reused_blocks = plan.reused,
        rebuilt_blocks = new_blocks.len() - plan.reused,
        staged_capacity = plan.staged_capacity,
        recompressed_twice,
        "UnityFS direct block assembly"
    );
    Ok(out)
}

