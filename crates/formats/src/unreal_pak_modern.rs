//! Bounded v10/v11 primary + full directory index reader.
//! Layout reference: https://github.com/trumank/repak (pak.rs, entry.rs).
//! Names cannot be recovered from path hashes alone; require the full directory.
use super::*;
use std::collections::{HashMap, HashSet};

const MAX_RECORDS: usize = 1_000_000;

#[derive(Clone)]
struct IndexSpan {
    offset: u64,
    size: u64,
    hash: [u8; 20],
}

fn span(r: &mut R<'_>) -> Result<Option<IndexSpan>, PakError> {
    match r.u32()? {
        0 => Ok(None),
        1 => Ok(Some(IndexSpan {
            offset: r.u64()?,
            size: r.u64()?,
            hash: r.sha1()?,
        })),
        _ => Err(err(r.file, "invalid secondary-index presence flag")),
    }
}

fn read_span<T: Read + Seek>(
    reader: &mut T,
    span: &IndexSpan,
    file: &str,
) -> Result<Vec<u8>, PakError> {
    reader
        .seek(SeekFrom::Start(span.offset))
        .map_err(|e| err(file, format!("seek secondary index: {e}")))?;
    let mut bytes = vec![0; span.size as usize];
    reader
        .read_exact(&mut bytes)
        .map_err(|e| err(file, format!("read secondary index: {e}")))?;
    if sha1_bytes(&bytes) != span.hash {
        return Err(err(file, "secondary index SHA-1 mismatch"));
    }
    Ok(bytes)
}

fn count(r: &mut R<'_>, min_bytes: usize) -> Result<usize, PakError> {
    let n = r.u32()? as usize;
    if n > MAX_RECORDS || n > r.data.len().saturating_sub(r.pos) / min_bytes {
        return Err(err(
            r.file,
            "index count exceeds safety limit or remaining bytes",
        ));
    }
    Ok(n)
}

fn encoded(r: &mut R<'_>, version: u32) -> Result<PakRecord, PakError> {
    let bits = r.u32()?;
    let compression_method = (bits >> 23) & 63;
    let encrypted = bits & (1 << 22) != 0;
    let blocks = ((bits >> 6) & 65535) as usize;
    let compression_block_size = if bits & 63 == 63 {
        r.u32()?
    } else {
        (bits & 63) << 11
    };
    let offset = if bits & (1 << 31) != 0 {
        r.u32()? as u64
    } else {
        r.u64()?
    };
    let uncompressed_size = if bits & (1 << 30) != 0 {
        r.u32()? as u64
    } else {
        r.u64()?
    };
    let size = if compression_method == 0 {
        uncompressed_size
    } else if bits & (1 << 29) != 0 {
        r.u32()? as u64
    } else {
        r.u64()?
    };
    if compression_method == 0 && (blocks != 0 || compression_block_size != 0) {
        return Err(err(
            r.file,
            "uncompressed encoded entry has compression blocks",
        ));
    }
    let mut compression_blocks = Vec::new();
    let mut start = (entry_header_size(version)
        + if compression_method != 0 {
            4 + blocks * 16
        } else {
            0
        }) as u64;
    for _ in 0..blocks {
        let n = if blocks == 1 && !encrypted {
            size
        } else {
            r.u32()? as u64
        };
        let end = start
            .checked_add(n)
            .ok_or_else(|| err(r.file, "encoded block offset overflow"))?;
        compression_blocks.push((start, end));
        start = if encrypted {
            end.checked_add(15)
                .ok_or_else(|| err(r.file, "encoded block alignment overflow"))?
                & !15
        } else {
            end
        };
    }
    Ok(PakRecord {
        name: String::new(),
        offset,
        size,
        uncompressed_size,
        compression_method,
        sha1: [0; 20],
        encrypted,
        compression_blocks,
        compression_block_size,
        compression_name: String::new(),
        hash_in_index: false,
        deleted: false,
    })
}

pub(super) fn parse_index<T: Read + Seek>(
    reader: &mut T,
    bytes: &[u8],
    footer: PakFooter,
    file: &str,
) -> Result<PakIndex, PakError> {
    if sha1_bytes(bytes) != footer.index_hash {
        return Err(err(file, "index SHA-1 mismatch"));
    }
    let mut r = R {
        data: bytes,
        pos: 0,
        file,
    };
    let mount_point = r.fstring()?;
    let declared = r.u32()? as usize;
    if declared > MAX_RECORDS {
        return Err(err(file, "record count exceeds safety limit"));
    }
    let _seed = r.u64()?;
    let phi = span(&mut r)?;
    let fdi = span(&mut r)?.ok_or_else(|| err(file, "unsupported path-hash-only pak index: full directory index required to enumerate LocRes"))?;
    // Bound the aggregate primary and secondary index allocation, and reject
    // overlapping spans (including footer and primary index).
    let mut total = bytes.len() as u64;
    let mut spans = vec![(footer.index_offset, footer.index_offset + footer.index_size)];
    for s in phi.iter().chain(std::iter::once(&fdi)) {
        total = total
            .checked_add(s.size)
            .ok_or_else(|| err(file, "index size overflow"))?;
        if total > MAX_INDEX_SIZE {
            return Err(err(file, "combined pak index exceeds safety limit"));
        }
        let end = s
            .offset
            .checked_add(s.size)
            .filter(|end| *end <= footer_start(&footer))
            .ok_or_else(|| err(file, "secondary index range past EOF or overlaps footer"))?;
        if s.size < 4 || spans.iter().any(|(a, b)| s.offset < *b && end > *a) {
            return Err(err(file, "secondary index span is empty or overlapping"));
        }
        spans.push((s.offset, end));
    }
    let encoded_size = r.u32()? as usize;
    let encoded_bytes = r.take(encoded_size)?;
    // Decode sequentially once: directory references must hit entry boundaries,
    // not arbitrary offsets in another record's fields.
    let mut er = R {
        data: encoded_bytes,
        pos: 0,
        file,
    };
    let mut compact = HashMap::new();
    while er.pos < er.data.len() {
        if compact.len() >= declared {
            return Err(err(file, "encoded record count exceeds declared count"));
        }
        let offset = er.pos as i32;
        let rec = encoded(&mut er, footer.version)?;
        compact.insert(offset, rec);
    }
    let n = count(&mut r, entry_header_size(footer.version))?;
    if n + compact.len() > declared {
        return Err(err(file, "entry count exceeds declared count"));
    }
    let mut ordinary = Vec::with_capacity(n);
    for _ in 0..n {
        ordinary.push(read_entry_body(&mut r, footer.version)?);
    }
    if r.pos != bytes.len() {
        return Err(err(file, "trailing primary index bytes"));
    }
    let valid_location = |location: i32| {
        location == i32::MIN
            || if location >= 0 {
                compact.contains_key(&location)
            } else {
                (-(location as i64) - 1) < ordinary.len() as i64
            }
    };
    if let Some(phi) = &phi {
        let data = read_span(reader, phi, file)?;
        let mut pr = R {
            data: &data,
            pos: 0,
            file,
        };
        let n = count(&mut pr, 12)?;
        if n > declared {
            return Err(err(file, "path hash count exceeds declared count"));
        }
        for _ in 0..n {
            let _hash = pr.u64()?;
            if !valid_location(pr.i32()?) {
                return Err(err(file, "invalid path-hash entry location"));
            }
        }
        // Remaining bytes are the engine's pruned directory index, which is
        // irrelevant when the complete, separately authenticated index exists.
    }
    let data = read_span(reader, &fdi, file)?;
    let mut dr = R {
        data: &data,
        pos: 0,
        file,
    };
    let dirs = count(&mut dr, 8)?;
    let mut records = Vec::new();
    let mut names = HashSet::new();
    let mut locations = HashSet::new();
    let mut files_seen = 0usize;
    let mut expanded_path_bytes = 0usize;
    for _ in 0..dirs {
        let directory = dr.fstring()?;
        let n = count(&mut dr, 8)?;
        files_seen = files_seen
            .checked_add(n)
            .filter(|n| *n <= declared)
            .ok_or_else(|| err(file, "directory files exceed declared record count"))?;
        for _ in 0..n {
            let leaf = dr.fstring()?;
            if leaf.is_empty() || leaf.contains(['/', '\\']) {
                return Err(err(file, "invalid directory filename"));
            }
            // A directory prefix is stored once but expands for every file.
            // Bound that expansion separately from the serialized index size.
            let path_len = directory
                .len()
                .saturating_add(leaf.len())
                .saturating_add(mount_point.len());
            expanded_path_bytes = expanded_path_bytes.saturating_add(path_len);
            if path_len > 4096 || expanded_path_bytes > MAX_INDEX_SIZE as usize {
                return Err(err(file, "expanded directory paths exceed safety limit"));
            }
            let name = format!(
                "{}{}",
                directory.strip_prefix('/').unwrap_or(&directory),
                leaf
            );
            let canonical = canonical_inner_name(&name)?;
            if !names.insert(canonical) {
                return Err(err(file, "duplicate directory resource path"));
            }
            let location = dr.i32()?;
            // Deleted/pruned sentinels have no extractable record. Keep an
            // explicit diagnostic for LocRes rather than reviving an old file.
            if location == i32::MIN {
                if is_locres_record_name(&name) {
                    return Err(err(file, "unsupported deleted/pruned LocRes entry"));
                }
                continue;
            }
            if !locations.insert(location) {
                return Err(err(file, "duplicate full-directory entry location"));
            }
            let mut rec = if location >= 0 {
                compact.remove(&location)
            } else {
                ordinary.get((-(location as i64) - 1) as usize).cloned()
            }
            .ok_or_else(|| err(file, "invalid full-directory entry location"))?;
            rec.name = name;
            assign_compression(&mut rec, &footer);
            let end = rec
                .offset
                .checked_add(record_header_size(&rec, footer.version) as u64)
                .and_then(|v| v.checked_add(rec.size))
                .ok_or_else(|| err(file, "record data range overflow"))?;
            if end > footer_start(&footer) || spans.iter().any(|(a, b)| rec.offset < *b && end > *a)
            {
                return Err(err(file, "record data overlaps index/footer or past EOF"));
            }
            records.push(rec);
        }
    }
    if dr.pos != data.len() || files_seen != declared {
        return Err(err(file, "full directory count/size mismatch"));
    }
    Ok(PakIndex {
        footer,
        mount_point,
        records,
    })
}
