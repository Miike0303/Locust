//! Experimental, read-only standalone Godot PCK v2 **metadata** helper.
//!
//! This is not a registered format or native Godot translation support. No payload
//! is read, decoded or hashed; `stored_md5` is an unauthenticated index field.
//!
//! # Primary contract and supported profile
//!
//! Based on Godot commit `77dcf97d82cbfe4e4615475fa52ca03da645dbd8`,
//! [`core/io/file_access_pack.h`](https://github.com/godotengine/godot/blob/77dcf97d82cbfe4e4615475fa52ca03da645dbd8/core/io/file_access_pack.h),
//! `PackedSourcePCK::try_open_pack` in `file_access_pack.cpp`, and
//! `PCKPacker::{pck_start,flush}` in `pck_packer.cpp` at that same commit.
//!
//! Only an offset-zero `GDPC`, little-endian v2 header is accepted. Engine versions
//! 4.0 through 4.3 inclusive are the deliberately bounded profile, with any patch
//! number (the primary reader ignores patch). Older majors and later minors are
//! unsupported, not assertions that their archives are necessarily malformed.
//! Header bytes: magic/version/major/minor/patch/flags (six u32s), file base (u64),
//! sixteen reserved u32s, count (u32): 100 bytes. Reserved words are ignored by
//! Godot, so we retain them without inventing a zero-only validity requirement.
//! Directory flag bit 0 and member flag bit 0 mean encryption: both are refused.
//! Header bit 1 means file base relative to PCK start; at start zero it is an
//! identity operation and is supported. All unknown flag bits are refused.
//!
//! Entries contain a u32 padded path length, UTF-8 bytes, u64 relative offset,
//! u64 size, 16 stored MD5 bytes, and u32 flags. The primary packer pads paths
//! with 0–3 zero bytes to a four-byte boundary (no mandatory NUL terminator).
//! We require that encoding. Paths must be `res://` virtual names, never host
//! paths. Redundant `/` and `.` components are canonicalized; `..`, backslashes,
//! colons in the tail, control characters, empty names and directory-only names
//! are refused. Names are case-sensitive, with no Unicode normalization.
//!
//! File base is not an index boundary: Godot adds it to each stored offset, so
//! base zero with absolute stored offsets is valid. Resolved ranges must lie
//! after the entire index and inside the file. This conservative profile refuses
//! **all nonempty overlaps**, including exact shared ranges, rather than claiming
//! that a future writer can safely mutate aliases. Empty members have no occupied
//! bytes; their offsets must still be at/after index end and at/before EOF, and
//! may coincide with each other or with nonempty members. Empty archives work.
//!
//! # Bounds and separation
//!
//! At most 16 MiB of serialized index (excluding the 100-byte header), 100,000
//! entries, and 16 KiB per padded path. Count feasibility is checked before entry
//! allocation; complete record and remaining-count bounds precede path allocation.
//! Heap usage is O(bounded index bytes + bounded entry count), not a 16 MiB total
//! heap guarantee. No full-file buffering, embedded/executable scanning, payload
//! reads, file writes, or dependency on the physical archive's filename occur.
//! Raw field positions and values remain available for a future streaming writer;
//! retaining positions does not authorize writing or prove archive authenticity.

use std::collections::HashSet;
use std::io::{self, Read, Seek, SeekFrom};

pub const HEADER_SIZE: u64 = 100;
pub const MAX_INDEX_BYTES: u64 = 16 * 1024 * 1024;
pub const MAX_ENTRIES: u32 = 100_000;
pub const MAX_PATH_BYTES: u32 = 16 * 1024;
// Smallest accepted path is "res://a", padded to eight bytes.
const MIN_RECORD_BYTES: u64 = 4 + 8 + 36;
const PACK_DIR_ENCRYPTED: u32 = 1;
const PACK_REL_FILEBASE: u32 = 2;
const PACK_FILE_ENCRYPTED: u32 = 1;

/// One index record, in original directory order. All positions are absolute.
#[derive(Debug)]
pub struct PckEntry {
    /// Canonical, case-sensitive virtual identity; never a filesystem path.
    pub path: String,
    pub path_length_position: u64,
    pub path_position: u64,
    /// Original padded byte length, not the canonical name's length.
    pub path_length: u32,
    pub offset_position: u64,
    pub size_position: u64,
    pub md5_position: u64,
    pub flags_position: u64,
    /// Raw offset field, relative to the header's file base.
    pub stored_offset: u64,
    /// Checked absolute offset (`file_base + stored_offset`).
    pub offset: u64,
    pub size: u64,
    /// Stored only: no payload has been read or authenticated.
    pub stored_md5: [u8; 16],
    pub flags: u32,
}

/// Validated index structure, not authenticated payloads or a live-file snapshot.
#[derive(Debug)]
pub struct PckIndex {
    pub file_size: u64,
    pub format_version: u32,
    pub engine_version: [u32; 3],
    pub flags: u32,
    /// Raw header field at byte 24; relative-to-start is identical at start zero.
    pub file_base: u64,
    /// Opaque reserved words at bytes 32..96, preserved even when nonzero.
    pub reserved: [u32; 16],
    /// First byte after the index, excluding any padding before payloads.
    pub index_end: u64,
    pub entries: Vec<PckEntry>,
}

fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

fn u32_at(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(bytes[at..at + 4].try_into().expect("fixed field slice"))
}

fn u64_at(bytes: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(bytes[at..at + 8].try_into().expect("fixed field slice"))
}

fn canonical_path(bytes: &[u8]) -> io::Result<String> {
    let text_end = bytes.iter().position(|b| *b == 0).unwrap_or(bytes.len());
    let padding = &bytes[text_end..];
    if padding.len() > 3 || padding.iter().any(|b| *b != 0) {
        return Err(invalid("PCK path has invalid zero padding or interior NUL"));
    }
    let text = std::str::from_utf8(&bytes[..text_end])
        .map_err(|_| invalid("PCK path is not strict UTF-8"))?;
    let tail = text
        .strip_prefix("res://")
        .ok_or_else(|| invalid("PCK path must be a res:// virtual name"))?;
    if tail.is_empty()
        || tail.starts_with('/')
        || tail.ends_with('/')
        || tail.contains(['\\', ':'])
        || tail.chars().any(char::is_control)
    {
        return Err(invalid("PCK path is not a safe virtual file name"));
    }
    let mut canonical = String::from("res://");
    for component in tail.split('/') {
        if component == ".." {
            return Err(invalid("PCK path traversal is unsupported"));
        }
        if component.is_empty() || component == "." {
            continue;
        }
        if canonical.len() > 6 {
            canonical.push('/');
        }
        canonical.push_str(component);
    }
    if canonical.len() == 6 || tail.ends_with("/.") {
        return Err(invalid("PCK path names a directory, not a file"));
    }
    Ok(canonical)
}

/// Read only the header and bounded index from a stable `Read + Seek` source.
///
/// Always seeks to absolute byte zero, never searches for another header. On
/// success the cursor is at `index_end`; on failure its position is unspecified.
/// Seeking to EOF measures the file but does not read its payload. Callers must
/// prevent concurrent changes if they need a consistent snapshot; metadata alone
/// cannot authenticate the archive. Errors contain no member or physical paths.
pub fn read_index<R: Read + Seek>(reader: &mut R) -> io::Result<PckIndex> {
    let file_size = reader.seek(SeekFrom::End(0))?;
    if file_size < HEADER_SIZE {
        return Err(invalid("PCK header is truncated (requires 100 bytes)"));
    }
    reader.seek(SeekFrom::Start(0))?;
    let mut header = [0; HEADER_SIZE as usize];
    reader.read_exact(&mut header)?;
    if &header[..4] != b"GDPC" {
        return Err(invalid(
            "PCK magic missing at offset zero; embedded/executable packs are unsupported",
        ));
    }
    let format_version = u32_at(&header, 4);
    if format_version != 2 {
        return Err(invalid("PCK format version unsupported (requires v2)"));
    }
    let engine_version = [u32_at(&header, 8), u32_at(&header, 12), u32_at(&header, 16)];
    if engine_version[0] != 4 || engine_version[1] > 3 {
        return Err(invalid(
            "PCK engine version unsupported (requires Godot 4.0–4.3)",
        ));
    }
    let flags = u32_at(&header, 20);
    if flags & PACK_DIR_ENCRYPTED != 0 {
        return Err(invalid("PCK encrypted directory is unsupported"));
    }
    if flags & !(PACK_DIR_ENCRYPTED | PACK_REL_FILEBASE) != 0 {
        return Err(invalid("PCK unknown header flags are unsupported"));
    }
    let file_base = u64_at(&header, 24);
    if file_base > file_size {
        return Err(invalid("PCK file base exceeds EOF"));
    }
    let mut reserved = [0; 16];
    for (i, word) in reserved.iter_mut().enumerate() {
        *word = u32_at(&header, 32 + i * 4);
    }
    let count = u32_at(&header, 96);
    if count > MAX_ENTRIES {
        return Err(invalid("PCK entry count exceeds 100000 limit"));
    }
    let index_limit = file_size.min(HEADER_SIZE + MAX_INDEX_BYTES);
    let minimum_bytes = u64::from(count) * MIN_RECORD_BYTES;
    if minimum_bytes > index_limit - HEADER_SIZE {
        return Err(invalid("PCK entry count cannot fit bounded index"));
    }
    let mut entries = Vec::with_capacity(count as usize);
    let mut position = HEADER_SIZE;
    for number in 0..count {
        let path_length_position = position;
        // The previous feasibility check covers at least the length word.
        let mut length = [0; 4];
        reader.read_exact(&mut length)?;
        let path_length = u32::from_le_bytes(length);
        if path_length == 0 || path_length > MAX_PATH_BYTES || path_length % 4 != 0 {
            return Err(invalid(
                "PCK padded path length is invalid or exceeds 16 KiB",
            ));
        }
        let path_position = position + 4;
        let record_end = path_position
            .checked_add(u64::from(path_length))
            .and_then(|p| p.checked_add(36))
            .ok_or_else(|| invalid("PCK index arithmetic overflow"))?;
        let remaining = u64::from(count - number - 1) * MIN_RECORD_BYTES;
        if record_end
            .checked_add(remaining)
            .is_none_or(|end| end > index_limit)
        {
            return Err(invalid("PCK record exceeds bounded index or is truncated"));
        }
        let mut path_bytes = vec![0; path_length as usize];
        reader.read_exact(&mut path_bytes)?;
        let path = canonical_path(&path_bytes)?;
        let offset_position = path_position + u64::from(path_length);
        let mut fields = [0; 36];
        reader.read_exact(&mut fields)?;
        let stored_offset = u64_at(&fields, 0);
        let size = u64_at(&fields, 8);
        let stored_md5 = fields[16..32].try_into().expect("fixed MD5 field");
        let flags = u32_at(&fields, 32);
        if flags & PACK_FILE_ENCRYPTED != 0 {
            return Err(invalid("PCK encrypted member is unsupported"));
        }
        if flags != 0 {
            return Err(invalid("PCK unknown member flags are unsupported"));
        }
        let offset = file_base
            .checked_add(stored_offset)
            .ok_or_else(|| invalid("PCK file base plus offset overflow"))?;
        let end = offset
            .checked_add(size)
            .ok_or_else(|| invalid("PCK member range overflow"))?;
        if end > file_size {
            return Err(invalid("PCK member range exceeds EOF"));
        }
        entries.push(PckEntry {
            path,
            path_length_position,
            path_position,
            path_length,
            offset_position,
            size_position: offset_position + 8,
            md5_position: offset_position + 16,
            flags_position: offset_position + 32,
            stored_offset,
            offset,
            size,
            stored_md5,
            flags,
        });
        position = record_end;
    }
    let mut names = HashSet::with_capacity(entries.len());
    let mut ranges = Vec::with_capacity(entries.len());
    for entry in &entries {
        if !names.insert(entry.path.as_str()) {
            return Err(invalid("PCK duplicate canonical virtual identity"));
        }
        if entry.offset < position {
            return Err(invalid("PCK member overlaps header or index"));
        }
        if entry.size != 0 {
            // Addition was checked above, including for empty members.
            ranges.push((entry.offset, entry.offset + entry.size));
        }
    }
    ranges.sort_unstable();
    if ranges.windows(2).any(|pair| pair[1].0 < pair[0].1) {
        return Err(invalid(
            "PCK overlapping or shared nonempty ranges are unsupported",
        ));
    }
    Ok(PckIndex {
        file_size,
        format_version,
        engine_version,
        flags,
        file_base,
        reserved,
        index_end: position,
        entries,
    })
}
