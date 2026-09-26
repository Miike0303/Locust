//! Seek only to the selected LocRes; validate stored header, SHA-1, block
//! geometry and exact decompressed length before handing bytes to LocRes.
use super::*;

pub fn read_payload<T: Read + Seek>(
    reader: &mut T,
    record: &PakRecord,
    version: u32,
    file_len: u64,
    file: &str,
) -> Result<Vec<u8>, PakError> {
    if record.encrypted {
        return Err(err(
            file,
            format!("encrypted pak record is unsupported: {}", record.name),
        ));
    }
    if record.deleted {
        return Err(err(
            file,
            format!("deleted pak record is unsupported: {}", record.name),
        ));
    }
    if record.size > MAX_LOCRES_PAYLOAD || record.uncompressed_size > MAX_LOCRES_PAYLOAD {
        return Err(err(
            file,
            "compressed or uncompressed LocRes exceeds payload safety limit",
        ));
    }
    if record.compression_method != 0
        && !matches!(
            record.compression_name.to_ascii_lowercase().as_str(),
            "zlib" | "gzip"
        )
    {
        return Err(err(file, format!("compressed LocRes codec '{}' (method {}) unavailable; supported codecs: Zlib, Gzip: {}", record.compression_name, record.compression_method, record.name)));
    }
    let header_size = record_header_size(record, version);
    let start = record
        .offset
        .checked_add(header_size as u64)
        .ok_or_else(|| err(file, "payload offset overflow"))?;
    let end = start
        .checked_add(record.size)
        .filter(|end| *end <= file_len)
        .ok_or_else(|| err(file, "record payload past EOF"))?;
    reader
        .seek(SeekFrom::Start(record.offset))
        .map_err(|e| err(file, format!("seek data header: {e}")))?;
    let mut header_bytes = vec![0; header_size];
    reader
        .read_exact(&mut header_bytes)
        .map_err(|e| err(file, format!("read data header: {e}")))?;
    let mut hr = R {
        data: &header_bytes,
        pos: 0,
        file,
    };
    let header = read_entry_body(&mut hr, version)?;
    if hr.pos != header_size
        || (header.offset != 0 && header.offset != record.offset)
        || header.size != record.size
        || header.uncompressed_size != record.uncompressed_size
        || header.compression_method != record.compression_method
        || header.encrypted != record.encrypted
        || header.deleted != record.deleted
        || header.compression_blocks != record.compression_blocks
        || (record.hash_in_index && header.sha1 != record.sha1)
    {
        return Err(err(
            file,
            format!("data header/index mismatch: {}", record.name),
        ));
    }
    let mut stored = vec![0; record.size as usize];
    reader
        .read_exact(&mut stored)
        .map_err(|e| err(file, format!("read payload: {e}")))?;
    if sha1_bytes(&stored) != header.sha1 {
        return Err(err(
            file,
            format!("payload SHA-1 mismatch: {}", record.name),
        ));
    }
    if record.compression_method == 0 {
        if record.size != record.uncompressed_size {
            return Err(err(file, "uncompressed size mismatch"));
        }
        return Ok(stored);
    }
    if record.compression_blocks.is_empty() || header.compression_block_size == 0 {
        return Err(err(
            file,
            "compressed LocRes has no blocks or zero block size",
        ));
    }
    let expected_count = record
        .uncompressed_size
        .div_ceil(header.compression_block_size as u64);
    if expected_count != record.compression_blocks.len() as u64 {
        return Err(err(file, "compression block count/size mismatch"));
    }
    let mut out = Vec::with_capacity(record.uncompressed_size as usize);
    let mut previous_end = start;
    for &(block_start, block_end) in &record.compression_blocks {
        let base = if version >= 5 { record.offset } else { 0 };
        let a = base
            .checked_add(block_start)
            .ok_or_else(|| err(file, "block start overflow"))?;
        let b = base
            .checked_add(block_end)
            .ok_or_else(|| err(file, "block end overflow"))?;
        if a != previous_end || b <= a || b > end {
            return Err(err(file, "invalid compressed block range"));
        }
        previous_end = b;
        let input = &stored[(a - start) as usize..(b - start) as usize];
        let expected = (record.uncompressed_size as usize - out.len())
            .min(header.compression_block_size as usize);
        // The decoder is capped per block, preventing expansion beyond either
        // the declared size or the global cap, even for adversarial streams.
        let mut decoded = Vec::new();
        let consumed = if record.compression_name.eq_ignore_ascii_case("zlib") {
            let mut decoder = flate2::bufread::ZlibDecoder::new(input);
            decoder
                .by_ref()
                .take(expected as u64 + 1)
                .read_to_end(&mut decoded)
                .map_err(|e| err(file, format!("zlib block decode: {e}")))?;
            input.len() - decoder.into_inner().len()
        } else {
            let mut decoder = flate2::bufread::GzDecoder::new(input);
            decoder
                .by_ref()
                .take(expected as u64 + 1)
                .read_to_end(&mut decoded)
                .map_err(|e| err(file, format!("gzip block decode: {e}")))?;
            input.len() - decoder.into_inner().len()
        };
        if decoded.len() != expected || consumed != input.len() {
            return Err(err(
                file,
                "decompressed block size or compressed stream length mismatch",
            ));
        }
        out.extend_from_slice(&decoded);
    }
    if previous_end != end || out.len() as u64 != record.uncompressed_size {
        return Err(err(file, "compressed payload size mismatch"));
    }
    Ok(out)
}
