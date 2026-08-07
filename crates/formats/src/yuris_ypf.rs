//! YU-RIS YPF archive reader/writer (common modern versions).
//!
//! # Spec sources
//! - Layout, name length swap table, name XOR, entry fields, zlib packing:
//!   https://raw.githubusercontent.com/morkt/GARbro/master/ArcFormats/YuRis/ArcYPF.cs
//!   (header `YPF\0` + version + count + dir_size/data_offset at +12; index from 0x20;
//!   per entry: CRC32 name hash, obfuscated name length, XOR'd CP932 name, type, pack
//!   flag, unpacked/packed size, offset, Adler-32 checksum; optional extra trailer by version).
//!
//! # Supported versions
//! Practical modern range including `0x1E4` (ExtraHeaderSize 4 when `version >= 0x1D9`).
//! Unsupported versions return `Err` naming the version number.
//!
//! # Out of scope
//! Embedded YPF inside MZ/`YSER` exe overlays; per-title custom swap schemes beyond
//! the three standard tables; YSTB script-key schemes applied inside the archive layer
//! (YSTB XOR is handled by the outer Yuris plugin after inflate).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use tracing::warn;

/// Magic `YPF\0` as LE u32 `0x00465059`.
pub const YPF_MAGIC: &[u8; 4] = b"YPF\0";
const HEADER_SIZE: usize = 0x20;

/// Fixed trailer after name: type(1) + pack(1) + unpacked(4) + packed(4) + offset(4) + adler(4) = 0x12.
const ENTRY_FIXED_AFTER_NAME: usize = 0x12;

// ─── Swap tables (GARbro YpfOpener) ────────────────────────────────────────

/// Default for most modern versions (including 0x1E4).
static SWAP_TABLE_00: &[u8] = &[
    0x03, 0x48, 0x06, 0x35, 0x0C, 0x10, 0x11, 0x19, 0x1C, 0x1E, 0x09, 0x0B, 0x0D, 0x13, 0x15,
    0x1B, 0x20, 0x23, 0x26, 0x29, 0x2C, 0x2F, 0x2E, 0x32,
];
/// version < 0x100
static SWAP_TABLE_04: &[u8] = &[
    0x0C, 0x10, 0x11, 0x19, 0x1C, 0x1E, 0x09, 0x0B, 0x0D, 0x13, 0x15, 0x1B, 0x20, 0x23, 0x26,
    0x29, 0x2C, 0x2F, 0x2E, 0x32,
];
/// 0x12C <= version < 0x196
static SWAP_TABLE_10: &[u8] = &[
    0x09, 0x0B, 0x0D, 0x13, 0x15, 0x1B, 0x20, 0x23, 0x26, 0x29, 0x2C, 0x2F, 0x2E, 0x32,
];

#[derive(Debug)]
pub struct YpfError {
    pub file: String,
    pub message: String,
}

impl std::fmt::Display for YpfError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.file, self.message)
    }
}

impl std::error::Error for YpfError {}

fn err(file: &str, message: impl Into<String>) -> YpfError {
    YpfError {
        file: file.into(),
        message: message.into(),
    }
}

// ─── Checksums ─────────────────────────────────────────────────────────────

/// RFC 1950 Adler-32 (GARbro uses this for entry payload checksums).
pub fn adler32(data: &[u8]) -> u32 {
    const MOD: u32 = 65521;
    let mut a: u32 = 1;
    let mut b: u32 = 0;
    for &byte in data {
        a = (a + u32::from(byte)) % MOD;
        b = (b + a) % MOD;
    }
    (b << 16) | a
}

/// IEEE CRC-32 (GARbro uses this for the plaintext name hash).
pub fn crc32(data: &[u8]) -> u32 {
    let mut crc: u32 = 0xFFFF_FFFF;
    for &b in data {
        let idx = ((crc ^ u32::from(b)) & 0xFF) as usize;
        crc = CRC32_TABLE[idx] ^ (crc >> 8);
    }
    !crc
}

static CRC32_TABLE: [u32; 256] = make_crc32_table();

const fn make_crc32_table() -> [u32; 256] {
    let mut table = [0u32; 256];
    let mut n = 0;
    while n < 256 {
        let mut c = n as u32;
        let mut k = 0;
        while k < 8 {
            if c & 1 != 0 {
                c = 0xEDB8_8320 ^ (c >> 1);
            } else {
                c >>= 1;
            }
            k += 1;
        }
        table[n] = c;
        n += 1;
    }
    table
}

// ─── Version helpers ───────────────────────────────────────────────────────

/// Whether we implement this YPF version.
pub fn is_supported_version(version: u32) -> bool {
    // Cover common shipping range including 0x1E4 / 0x1F4 / legacy 0xF7.
    (0xF0..=0x300).contains(&version)
}

fn swap_table_for(version: u32) -> &'static [u8] {
    if version < 0x100 {
        SWAP_TABLE_04
    } else if (0x12C..0x196).contains(&version) {
        SWAP_TABLE_10
    } else {
        SWAP_TABLE_00
    }
}

fn extra_header_size(version: u32) -> usize {
    if version >= 0x1D9 {
        4
    } else if version == 0xDE {
        8
    } else {
        0
    }
}

/// GARbro `Parser.DecryptLength` — pairwise swap; self-inverse.
pub fn decrypt_length(table: &[u8], value: u8) -> u8 {
    match table.iter().position(|&b| b == value) {
        Some(pos) if pos & 1 != 0 => table[pos - 1],
        Some(pos) => table[pos + 1],
        None => value,
    }
}

/// Stored name-length byte for a plaintext length (GARbro Create).
pub fn encode_name_length(table: &[u8], len: u8) -> u8 {
    !decrypt_length(table, len)
}

// ─── LE helpers (checked) ──────────────────────────────────────────────────

fn read_u32(data: &[u8], off: usize) -> Result<u32, YpfError> {
    if off.checked_add(4).filter(|e| *e <= data.len()).is_none() {
        return Err(err("ypf", "truncated u32"));
    }
    Ok(u32::from_le_bytes([
        data[off],
        data[off + 1],
        data[off + 2],
        data[off + 3],
    ]))
}

// ─── Entry / archive ───────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct YpfEntry {
    /// Decoded path (CP932 → lossy UTF-8; `/` normalized).
    pub name: String,
    /// Raw encrypted name bytes as stored in the index (for rewrite fidelity).
    pub index_name: Vec<u8>,
    pub name_hash: u32,
    pub file_type: u8,
    pub is_packed: bool,
    pub unpacked_size: u32,
    pub packed_size: u32,
    pub offset: u32,
    pub checksum: u32,
    /// Version-dependent trailer after checksum (0, 4, or 8 bytes).
    pub extra: Vec<u8>,
}

#[derive(Clone, Debug)]
pub struct YpfArchive {
    pub path: PathBuf,
    pub data: Vec<u8>,
    pub version: u32,
    pub entries: Vec<YpfEntry>,
    /// XOR key used for names (guessed or 0xFF default when building).
    pub name_key: u8,
}

impl YpfArchive {
    pub fn open(path: &Path) -> Result<Self, YpfError> {
        let label = path.display().to_string();
        let data = std::fs::read(path).map_err(|e| err(&label, format!("read failed: {e}")))?;
        Self::from_bytes(path.to_path_buf(), data)
    }

    pub fn from_bytes(path: PathBuf, data: Vec<u8>) -> Result<Self, YpfError> {
        let label = path.display().to_string();
        if data.len() < HEADER_SIZE {
            return Err(err(&label, "file too small for YPF header"));
        }
        if &data[0..4] != YPF_MAGIC {
            return Err(err(
                &label,
                format!(
                    "missing YPF magic (got {:?})",
                    String::from_utf8_lossy(&data[0..4])
                ),
            ));
        }
        let version = read_u32(&data, 4)?;
        if !is_supported_version(version) {
            return Err(err(
                &label,
                format!("unsupported YPF version {version:#x} ({version})"),
            ));
        }
        let count = read_u32(&data, 8)? as usize;
        let dir_size_field = read_u32(&data, 12)? as usize;
        // dir_size field is data_offset in GARbro Create (= 0x20 + index length).
        let index_len = if dir_size_field >= HEADER_SIZE {
            dir_size_field - HEADER_SIZE
        } else {
            dir_size_field
        };
        if count > 100_000 {
            return Err(err(&label, format!("implausible entry count {count}")));
        }
        let index_end = HEADER_SIZE
            .checked_add(index_len)
            .filter(|e| *e <= data.len())
            .ok_or_else(|| err(&label, "YPF index size exceeds file"))?;

        let table = swap_table_for(version);
        let extra_sz = extra_header_size(version);
        let mut pos = HEADER_SIZE;
        let mut entries = Vec::with_capacity(count);
        let mut name_key: Option<u8> = None;

        for i in 0..count {
            let remaining = index_end.saturating_sub(pos);
            let min_need = 5usize
                .checked_add(ENTRY_FIXED_AFTER_NAME)
                .and_then(|n| n.checked_add(extra_sz))
                .ok_or_else(|| err(&label, "size overflow"))?;
            if remaining < min_need {
                return Err(err(
                    &label,
                    format!("truncated index at entry {i} (need {min_need}, have {remaining})"),
                ));
            }
            let name_hash = read_u32(&data, pos)?;
            let stored_len = data[pos + 4];
            let name_len = decrypt_length(table, stored_len ^ 0xFF) as usize;
            if name_len == 0 || name_len > 0xFF {
                return Err(err(
                    &label,
                    format!("invalid name length {name_len} at entry {i}"),
                ));
            }
            let name_start = pos
                .checked_add(5)
                .ok_or_else(|| err(&label, "offset overflow"))?;
            let name_end = name_start
                .checked_add(name_len)
                .filter(|e| *e <= index_end)
                .ok_or_else(|| err(&label, format!("name exceeds index at entry {i}")))?;
            let mut index_name = data[name_start..name_end].to_vec();

            if name_key.is_none() {
                // The index stores CRC32 of the plaintext name, so a key guess is
                // verifiable — and cheap to brute-force when the ".ext" heuristic fails.
                let verify = |k: u8| {
                    let plain: Vec<u8> = index_name.iter().map(|b| b ^ k).collect();
                    crc32(&plain) == name_hash
                };
                let guess = if name_len >= 4 {
                    Some(index_name[name_len - 4] ^ b'.')
                } else {
                    None
                };
                let key = guess
                    .filter(|&k| verify(k))
                    .or_else(|| (0u8..=255).find(|&k| verify(k)))
                    .or_else(|| {
                        // Hash scheme unknown for this version: trust the heuristic.
                        guess.inspect(|_| {
                            warn!(
                                archive = %label,
                                "YPF name hash did not verify any XOR key; using '.ext' heuristic"
                            );
                        })
                    })
                    .ok_or_else(|| err(&label, "cannot determine name XOR key"))?;
                name_key = Some(key);
            }
            let key = name_key.unwrap();
            for b in &mut index_name {
                *b ^= key;
            }
            // index_name is now plaintext for hashing/string; keep XOR'd form separately
            let plain_name = index_name.clone();
            // re-encrypt for storage of original index_name field
            let mut stored_name = plain_name.clone();
            for b in &mut stored_name {
                *b ^= key;
            }

            let after = name_end;
            let fixed_end = after
                .checked_add(ENTRY_FIXED_AFTER_NAME + extra_sz)
                .filter(|e| *e <= index_end)
                .ok_or_else(|| err(&label, format!("entry trailer exceeds index at {i}")))?;

            let file_type = data[after];
            let is_packed = data[after + 1] != 0;
            let unpacked_size = read_u32(&data, after + 2)?;
            let packed_size = read_u32(&data, after + 6)?;
            let offset = read_u32(&data, after + 10)?;
            let checksum = read_u32(&data, after + 14)?;
            let extra = data[after + 18..fixed_end].to_vec();

            // Bounds for payload
            let off = offset as usize;
            let psz = packed_size as usize;
            if off
                .checked_add(psz)
                .filter(|e| *e <= data.len())
                .is_none()
            {
                return Err(err(
                    &label,
                    format!(
                        "entry {} payload out of range (offset={offset}, packed={packed_size})",
                        String::from_utf8_lossy(&plain_name)
                    ),
                ));
            }

            let name = decode_cp932_lossy(&plain_name).replace('\\', "/");
            entries.push(YpfEntry {
                name,
                index_name: stored_name,
                name_hash,
                file_type,
                is_packed,
                unpacked_size,
                packed_size,
                offset,
                checksum,
                extra,
            });
            pos = fixed_end;
        }

        Ok(Self {
            path,
            data,
            version,
            entries,
            name_key: name_key.unwrap_or(0xFF),
        })
    }

    /// Read entry payload; inflate zlib when packed. Adler mismatch → warn only.
    pub fn read_entry(&self, entry: &YpfEntry) -> Result<Vec<u8>, YpfError> {
        let label = self.path.display().to_string();
        let start = entry.offset as usize;
        let end = start
            .checked_add(entry.packed_size as usize)
            .filter(|e| *e <= self.data.len())
            .ok_or_else(|| err(&label, format!("payload OOB for {}", entry.name)))?;
        let slice = &self.data[start..end];
        let plain = if entry.is_packed {
            miniz_oxide::inflate::decompress_to_vec_zlib(slice).map_err(|e| {
                err(
                    &label,
                    format!("zlib inflate failed for {}: {e:?}", entry.name),
                )
            })?
        } else {
            slice.to_vec()
        };
        if entry.checksum != 0 {
            let got = adler32(&plain);
            // GARbro checksums the written stream (compressed when packed). Try both.
            let got_stored = adler32(slice);
            if got != entry.checksum && got_stored != entry.checksum {
                warn!(
                    archive = %self.path.display(),
                    entry = %entry.name,
                    expected = entry.checksum,
                    adler_plain = got,
                    adler_stored = got_stored,
                    "YPF entry checksum mismatch; using payload anyway"
                );
            }
        }
        Ok(plain)
    }

    pub fn ybn_entries(&self) -> impl Iterator<Item = &YpfEntry> {
        self.entries.iter().filter(|e| {
            Path::new(&e.name)
                .extension()
                .and_then(|x| x.to_str())
                .map(|x| x.eq_ignore_ascii_case("ybn"))
                .unwrap_or(false)
        })
    }
}

fn decode_cp932_lossy(bytes: &[u8]) -> String {
    let (cow, _, _) = encoding_rs::SHIFT_JIS.decode(bytes);
    cow.into_owned()
}

fn encode_cp932(name: &str) -> Result<Vec<u8>, YpfError> {
    let (bytes, _, had_errors) = encoding_rs::SHIFT_JIS.encode(name);
    if had_errors {
        return Err(err("ypf", format!("name not encodable as CP932: {name}")));
    }
    Ok(bytes.into_owned())
}

// ─── Writer ────────────────────────────────────────────────────────────────

/// Build a new YPF (version, name_key) from `(name, payload, is_packed)` list.
/// Used for synthetic fixtures and full rebuilds.
pub fn write_ypf(
    version: u32,
    name_key: u8,
    files: &[(String, Vec<u8>, bool)],
) -> Result<Vec<u8>, YpfError> {
    if !is_supported_version(version) {
        return Err(err(
            "ypf",
            format!("unsupported YPF version {version:#x} for write"),
        ));
    }
    let table = swap_table_for(version);
    let extra_sz = extra_header_size(version);

    struct Planned {
        plain_name: Vec<u8>,
        name_hash: u32,
        file_type: u8,
        is_packed: bool,
        unpacked: u32,
        packed_data: Vec<u8>,
        checksum: u32,
    }

    let mut planned = Vec::with_capacity(files.len());
    for (name, payload, is_packed) in files {
        let plain_name = encode_cp932(&name.replace('/', "\\"))?;
        if plain_name.len() > 0xFF {
            return Err(err("ypf", "file name too long"));
        }
        let name_hash = crc32(&plain_name);
        let packed_data = if *is_packed {
            miniz_oxide::deflate::compress_to_vec_zlib(payload, 6)
        } else {
            payload.clone()
        };
        // GARbro Create checksums the written stream via Adler32 on the sink.
        let checksum = adler32(&packed_data);
        planned.push(Planned {
            plain_name,
            name_hash,
            file_type: if name.to_ascii_lowercase().ends_with(".ybn") {
                0
            } else {
                1
            },
            is_packed: *is_packed,
            unpacked: payload.len() as u32,
            packed_data,
            checksum,
        });
    }
    planned.sort_by_key(|p| p.name_hash);

    // Index size
    let mut index_len = 0usize;
    for p in &planned {
        index_len = index_len
            .checked_add(5 + p.plain_name.len() + ENTRY_FIXED_AFTER_NAME + extra_sz)
            .ok_or_else(|| err("ypf", "index size overflow"))?;
    }
    let data_offset = HEADER_SIZE
        .checked_add(index_len)
        .ok_or_else(|| err("ypf", "data offset overflow"))?;

    let mut out = vec![0u8; data_offset];
    out[0..4].copy_from_slice(YPF_MAGIC);
    out[4..8].copy_from_slice(&version.to_le_bytes());
    out[8..12].copy_from_slice(&(planned.len() as u32).to_le_bytes());
    out[12..16].copy_from_slice(&(data_offset as u32).to_le_bytes());

    let mut idx = HEADER_SIZE;
    let mut data_cursor = data_offset as u32;
    let mut payloads: Vec<(u32, Vec<u8>)> = Vec::new();

    for p in &planned {
        // hash
        out[idx..idx + 4].copy_from_slice(&p.name_hash.to_le_bytes());
        idx += 4;
        // length
        out[idx] = encode_name_length(table, p.plain_name.len() as u8);
        idx += 1;
        // encrypted name
        for &b in &p.plain_name {
            out[idx] = b ^ name_key;
            idx += 1;
        }
        out[idx] = p.file_type;
        out[idx + 1] = if p.is_packed { 1 } else { 0 };
        out[idx + 2..idx + 6].copy_from_slice(&p.unpacked.to_le_bytes());
        let psz = p.packed_data.len() as u32;
        out[idx + 6..idx + 10].copy_from_slice(&psz.to_le_bytes());
        out[idx + 10..idx + 14].copy_from_slice(&data_cursor.to_le_bytes());
        out[idx + 14..idx + 18].copy_from_slice(&p.checksum.to_le_bytes());
        // extra zeros
        for j in 0..extra_sz {
            out[idx + 18 + j] = 0;
        }
        idx += ENTRY_FIXED_AFTER_NAME + extra_sz;

        payloads.push((data_cursor, p.packed_data.clone()));
        data_cursor = data_cursor
            .checked_add(psz)
            .ok_or_else(|| err("ypf", "payload offset overflow"))?;
    }

    for (_, blob) in &payloads {
        out.extend_from_slice(blob);
    }
    Ok(out)
}

/// Rebuild archive: replace named entries' logical payloads; copy others' stored
/// packed bytes byte-identically (no re-compress).
pub fn rebuild_ypf(
    original: &YpfArchive,
    replacements: &HashMap<String, Vec<u8>>,
) -> Result<Vec<u8>, YpfError> {
    let label = original.path.display().to_string();
    let version = original.version;
    let table = swap_table_for(version);
    let extra_sz = extra_header_size(version);
    let name_key = original.name_key;

    #[derive(Clone)]
    struct Built {
        name_hash: u32,
        plain_name: Vec<u8>,
        file_type: u8,
        is_packed: bool,
        unpacked: u32,
        packed: Vec<u8>,
        checksum: u32,
        extra: Vec<u8>,
    }

    let mut built = Vec::with_capacity(original.entries.len());
    for e in &original.entries {
        let key = e.name.replace('\\', "/");
        if let Some(new_payload) = replacements.get(&key) {
            let packed = if e.is_packed {
                miniz_oxide::deflate::compress_to_vec_zlib(new_payload, 6)
            } else {
                new_payload.clone()
            };
            let plain_name = encode_cp932(&e.name.replace('/', "\\"))?;
            built.push(Built {
                name_hash: crc32(&plain_name),
                plain_name,
                file_type: e.file_type,
                is_packed: e.is_packed,
                unpacked: new_payload.len() as u32,
                checksum: adler32(&packed),
                packed,
                extra: vec![0u8; extra_sz],
            });
        } else {
            let start = e.offset as usize;
            let end = start
                .checked_add(e.packed_size as usize)
                .filter(|x| *x <= original.data.len())
                .ok_or_else(|| err(&label, format!("cannot copy entry {}", e.name)))?;
            let packed = original.data[start..end].to_vec();
            // Recover plaintext name from stored index_name
            let mut plain_name = e.index_name.clone();
            for b in &mut plain_name {
                *b ^= name_key;
            }
            built.push(Built {
                name_hash: e.name_hash,
                plain_name,
                file_type: e.file_type,
                is_packed: e.is_packed,
                unpacked: e.unpacked_size,
                packed,
                checksum: e.checksum,
                extra: if e.extra.len() == extra_sz {
                    e.extra.clone()
                } else {
                    vec![0u8; extra_sz]
                },
            });
        }
    }
    built.sort_by_key(|b| b.name_hash);

    let mut index_len = 0usize;
    for b in &built {
        index_len = index_len
            .checked_add(5 + b.plain_name.len() + ENTRY_FIXED_AFTER_NAME + extra_sz)
            .ok_or_else(|| err(&label, "index size overflow"))?;
    }
    let data_offset = HEADER_SIZE
        .checked_add(index_len)
        .ok_or_else(|| err(&label, "data offset overflow"))?;

    let mut out = vec![0u8; data_offset];
    out[0..4].copy_from_slice(YPF_MAGIC);
    out[4..8].copy_from_slice(&version.to_le_bytes());
    out[8..12].copy_from_slice(&(built.len() as u32).to_le_bytes());
    out[12..16].copy_from_slice(&(data_offset as u32).to_le_bytes());

    let mut idx = HEADER_SIZE;
    let mut data_cursor = data_offset as u32;
    let mut blobs: Vec<Vec<u8>> = Vec::new();

    for b in &built {
        out[idx..idx + 4].copy_from_slice(&b.name_hash.to_le_bytes());
        idx += 4;
        out[idx] = encode_name_length(table, b.plain_name.len() as u8);
        idx += 1;
        for &byte in &b.plain_name {
            out[idx] = byte ^ name_key;
            idx += 1;
        }
        out[idx] = b.file_type;
        out[idx + 1] = if b.is_packed { 1 } else { 0 };
        out[idx + 2..idx + 6].copy_from_slice(&b.unpacked.to_le_bytes());
        let psz = b.packed.len() as u32;
        out[idx + 6..idx + 10].copy_from_slice(&psz.to_le_bytes());
        out[idx + 10..idx + 14].copy_from_slice(&data_cursor.to_le_bytes());
        out[idx + 14..idx + 18].copy_from_slice(&b.checksum.to_le_bytes());
        for j in 0..extra_sz {
            out[idx + 18 + j] = b.extra.get(j).copied().unwrap_or(0);
        }
        idx += ENTRY_FIXED_AFTER_NAME + extra_sz;
        data_cursor = data_cursor
            .checked_add(psz)
            .ok_or_else(|| err(&label, "payload offset overflow"))?;
        blobs.push(b.packed.clone());
    }
    for blob in blobs {
        out.extend_from_slice(&blob);
    }
    Ok(out)
}

// ─── Tests ─────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_adler_and_crc_smoke() {
        assert_eq!(adler32(b""), 1);
        assert_eq!(adler32(b"Wikipedia"), 0x11E6_0398);
        // CRC32 of empty is 0
        assert_eq!(crc32(b""), 0);
    }

    #[test]
    fn test_length_swap_self_inverse() {
        let t = SWAP_TABLE_00;
        for len in 0u8..=40 {
            let enc = encode_name_length(t, len);
            let dec = decrypt_length(t, enc ^ 0xFF);
            assert_eq!(dec, len, "len={len}");
        }
    }

    #[test]
    fn test_hand_crafted_name_deobfuscation() {
        let table = SWAP_TABLE_00;
        let plain = b"test.ybn";
        let key = 0xAA;
        let mut enc = plain.to_vec();
        for b in &mut enc {
            *b ^= key;
        }
        let stored_len = encode_name_length(table, plain.len() as u8);
        assert_eq!(decrypt_length(table, stored_len ^ 0xFF) as usize, plain.len());
        let guessed = enc[plain.len() - 4] ^ b'.';
        assert_eq!(guessed, key);
        for b in &mut enc {
            *b ^= key;
        }
        assert_eq!(&enc[..], plain.as_slice());
    }

    #[test]
    fn test_roundtrip_writer_reader_payloads() {
        let files = vec![
            ("yst00000.ybn".into(), b"YSTBhello".to_vec(), false),
            ("img/a.png".into(), vec![0x89, 0x50, 0x4E, 0x47], false),
        ];
        let bytes = write_ypf(0x1E4, 0xFF, &files).unwrap();
        let arch = YpfArchive::from_bytes(PathBuf::from("t.ypf"), bytes).unwrap();
        assert_eq!(arch.version, 0x1E4);
        assert_eq!(arch.entries.len(), 2);
        for (name, payload, _) in &files {
            let e = arch
                .entries
                .iter()
                .find(|e| e.name.replace('\\', "/") == *name)
                .unwrap_or_else(|| panic!("missing {name}"));
            assert_eq!(&arch.read_entry(e).unwrap(), payload);
        }
    }

    #[test]
    fn test_zlib_and_raw_entries() {
        let files = vec![
            ("a.ybn".into(), b"rawyyyyyyyyyyyyyyy".to_vec(), false),
            ("b.ybn".into(), b"zlibzzzzzzzzzzzzzzz".to_vec(), true),
        ];
        let bytes = write_ypf(0x1E4, 0xCC, &files).unwrap();
        let arch = YpfArchive::from_bytes(PathBuf::from("z.ypf"), bytes).unwrap();
        let a = arch.entries.iter().find(|e| e.name == "a.ybn").unwrap();
        let b = arch.entries.iter().find(|e| e.name == "b.ybn").unwrap();
        assert!(!a.is_packed);
        assert!(b.is_packed);
        assert_eq!(arch.read_entry(a).unwrap(), b"rawyyyyyyyyyyyyyyy");
        assert_eq!(arch.read_entry(b).unwrap(), b"zlibzzzzzzzzzzzzzzz");
    }

    #[test]
    fn test_bad_magic_errors() {
        let e = YpfArchive::from_bytes(PathBuf::from("x.ypf"), b"NOT!".to_vec())
            .unwrap_err()
            .to_string();
        assert!(e.contains("magic") || e.contains("small"), "{e}");
    }

    #[test]
    fn test_truncated_index_errors() {
        let mut data = Vec::new();
        data.extend_from_slice(YPF_MAGIC);
        data.extend_from_slice(&0x1E4u32.to_le_bytes());
        data.extend_from_slice(&1u32.to_le_bytes()); // one entry
        data.extend_from_slice(&0x100u32.to_le_bytes()); // huge dir
        data.resize(HEADER_SIZE, 0);
        // no index body
        let e = YpfArchive::from_bytes(PathBuf::from("t.ypf"), data)
            .unwrap_err()
            .to_string();
        assert!(
            e.contains("exceeds") || e.contains("truncated") || e.contains("index"),
            "{e}"
        );
    }

    #[test]
    fn test_unsupported_version_errors_with_number() {
        let mut data = vec![0u8; HEADER_SIZE];
        data[0..4].copy_from_slice(YPF_MAGIC);
        data[4..8].copy_from_slice(&0x9999u32.to_le_bytes());
        let e = YpfArchive::from_bytes(PathBuf::from("u.ypf"), data)
            .unwrap_err()
            .to_string();
        assert!(e.contains("9999") || e.contains("unsupported"), "{e}");
    }

    #[test]
    fn test_huge_declared_payload_errors() {
        // Valid single empty-ish archive then corrupt offset
        let files = vec![("a.ybn".into(), b"xx".to_vec(), false)];
        let mut bytes = write_ypf(0x1E4, 0xFF, &files).unwrap();
        // Find packed size / offset in index and set packed size huge
        // Safer: open, then mutate entry's region — just append tiny header fake
        // Instead craft: after successful write, set packed_size field to 0xFFFFFFF0
        let arch = YpfArchive::from_bytes(PathBuf::from("h.ypf"), bytes.clone()).unwrap();
        let e = &arch.entries[0];
        // packed size sits at offset+6 of trailer; recompute index position hard.
        // Mutate last 4 bytes of file claiming huge size by rewriting first entry trailer.
        // Walk: HEADER + hash(4)+len(1)+name
        let name_len = b"a.ybn".len();
        let trailer_at = HEADER_SIZE + 5 + name_len;
        // packed size at trailer_at+6
        let psz_off = trailer_at + 6;
        bytes[psz_off..psz_off + 4].copy_from_slice(&0xFFFF_FFF0u32.to_le_bytes());
        let err = YpfArchive::from_bytes(PathBuf::from("h.ypf"), bytes)
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("out of range") || err.contains("OOB") || err.contains("range"),
            "{err}"
        );
        let _ = e.name;
    }

    #[test]
    fn test_rebuild_replaces_one_keeps_other() {
        let files = vec![
            ("a.ybn".into(), b"AAAA".to_vec(), false),
            ("b.ybn".into(), b"BBBB".to_vec(), false),
        ];
        let bytes = write_ypf(0x1E4, 0xFF, &files).unwrap();
        let arch = YpfArchive::from_bytes(PathBuf::from("r.ypf"), bytes).unwrap();
        let mut repl = HashMap::new();
        repl.insert("a.ybn".into(), b"ZZZZ".to_vec());
        let rebuilt = rebuild_ypf(&arch, &repl).unwrap();
        let again = YpfArchive::from_bytes(PathBuf::from("r2.ypf"), rebuilt).unwrap();
        let a = again.entries.iter().find(|e| e.name == "a.ybn").unwrap();
        let b = again.entries.iter().find(|e| e.name == "b.ybn").unwrap();
        assert_eq!(again.read_entry(a).unwrap(), b"ZZZZ");
        assert_eq!(again.read_entry(b).unwrap(), b"BBBB");
    }
}
