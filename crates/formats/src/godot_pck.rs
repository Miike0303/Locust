//! Experimental standalone Godot PCK v2/v3 metadata and streaming rebuild helpers.
//!
//! This is not a registered format or native Godot translation support.
//! `read_index` reads metadata only: `stored_md5` remains an unauthenticated index
//! field. Explicit payload APIs check plain member bytes against that field.
//! MD5 provides format integrity, not authenticity against a malicious producer.
//!
//! # Primary contract and supported profile
//!
//! Based on Godot commit `77dcf97d82cbfe4e4615475fa52ca03da645dbd8`,
//! [`core/io/file_access_pack.h`](https://github.com/godotengine/godot/blob/77dcf97d82cbfe4e4615475fa52ca03da645dbd8/core/io/file_access_pack.h),
//! `PackedSourcePCK::try_open_pack` in `file_access_pack.cpp`, and
//! `PCKPacker::{pck_start,flush}` in `pck_packer.cpp` at that same commit.
//!
//! Only offset-zero `GDPC`, little-endian headers are accepted. Explicit profiles
//! admit v2 / Godot 4.0–4.4 and v3 / Godot 4.5+ (major 4), with any patch number.
//! V2 has six u32s, file base (u64), sixteen reserved u32s and count: 100 bytes.
//! V3 inserts directory_offset (u64) at byte 32, moves reserved words to byte 40,
//! and puts count at that directory: 104 fixed header bytes. V3 follows upstream
//! [4.5 file_access_pack.cpp](https://github.com/godotengine/godot/blob/4.5/core/io/file_access_pack.cpp)
//! and [pck_packer.cpp](https://github.com/godotengine/godot/blob/4.5/core/io/pck_packer.cpp).
//! Reserved words are opaque and retained even when nonzero.
//! Directory flag bit 0 and member flag bit 0 mean encryption: both are refused.
//! Header bit 1 means file base relative to PCK start; at start zero it is an
//! identity operation and is supported. V3 file base and directory offset are
//! always relative to PCK start. Sparse bundles (header bit 2), removal entries
//! (member bit 1), and all unknown flag bits are refused.
//!
//! Entries contain a u32 padded path length, UTF-8 bytes, u64 relative offset,
//! u64 size, 16 stored MD5 bytes, and u32 flags. The primary packer pads paths
//! with 0–3 zero bytes to a four-byte boundary (no mandatory NUL terminator).
//! We require that encoding. Paths can be bare or `res://` virtual names, never
//! host paths; canonical identity always has `res://`. Redundant `/` and `.`
//! components are canonicalized; `..`, backslashes,
//! colons in the tail, control characters, empty names and directory-only names
//! are refused. Names are case-sensitive, with no Unicode normalization.
//!
//! File base is not an index boundary: Godot adds it to each stored offset, so
//! base zero with absolute stored offsets is valid. Checked ranges must be in
//! bounds and disjoint from both header and directory. Payloads may precede or
//! follow the directory. This conservative profile refuses
//! **all nonempty overlaps**, including exact shared ranges, rather than claiming
//! that a future writer can safely mutate aliases. Empty members have no occupied
//! bytes; their offsets must still be outside metadata and at/before EOF, and
//! may coincide with each other or with nonempty members. Metadata ends are
//! permitted; metadata starts are excluded. Empty archives work.
//!
//! # Bounds and separation
//!
//! At most 16 MiB of directory (v2 records; v3 count plus records), 100,000
//! entries, and 16 KiB per padded path. Count feasibility is checked before entry
//! allocation; complete record and remaining-count bounds precede path allocation.
//! Heap usage is O(bounded index bytes + bounded entry count), not a 16 MiB total
//! heap guarantee. No full-file buffering, embedded/executable scanning, payload
//! reads or file writes occur in `read_index`.
//!
//! # Rebuild layout and installation
//!
//! The writer follows [`PCKPacker`](https://github.com/godotengine/godot/blob/77dcf97d82cbfe4e4615475fa52ca03da645dbd8/core/io/pck_packer.cpp#L38-L261):
//! `_get_pad`, `_bind_methods` and `add_file` establish configurable alignment
//! (default 32), plain-source MD5 and aligned relative offsets; `flush` writes
//! four-byte zero-padded UTF-8 paths, offset/size/MD5/flags records, an aligned
//! file base, and aligned payloads including final padding. We choose 32 bytes;
//! the source's original alignment is not encoded and cannot be inferred safely.
//! Original padded path bytes, directory order, versions, flags and reserved
//! words survive. Replacements keep the original path encoding and member flags;
//! additions use canonical paths and zero flags, and follow caller order.
//! Inter-member gaps, header padding and trailing bytes are regenerated, not
//! preserved. V2 `file_base` becomes aligned index end. V3 is preplanned as
//! header, aligned payloads, then count/directory, with file base 128 and the new
//! directory offset in the header. Empty final payloads receive a gap before the
//! directory so their offsets remain outside metadata. All offsets are rebased.
//! This agrees with the reader's
//! [file-base addition and record layout](https://github.com/godotengine/godot/blob/77dcf97d82cbfe4e4615475fa52ca03da645dbd8/core/io/file_access_pack.cpp#L192-L247),
//! including relative-to-start bit 1 at standalone offset zero; flag definitions
//! are in [`file_access_pack.h:39-49`](https://github.com/godotengine/godot/blob/77dcf97d82cbfe4e4615475fa52ca03da645dbd8/core/io/file_access_pack.h#L39-L49).
//!
//! Rebuild revalidates source metadata, then authenticates untouched members
//! while copying each exactly once through a 64 KiB buffer. A late mismatch can
//! leave partial output in the sink: discard it on **any** error. Never use the
//! source file as that sink. `verify_rebuild` independently rereads the completed
//! output, compares all planned fields and raw paths, and verifies every MD5.
//! `replace_archive` performs both steps in owned staging under the caller's
//! GameLock, closes source handles before install, and keeps the previous backup.
//! Sources must remain stable throughout the operation; the lock excludes
//! cooperating writers, not arbitrary external changes. No engine run is implied.

use md5::{Digest, Md5};
use std::collections::{HashMap, HashSet};
use std::io::{self, Read, Seek, SeekFrom, Write};

pub const HEADER_SIZE: u64 = 100;
pub const V3_HEADER_SIZE: u64 = 104;
pub const MAX_INDEX_BYTES: u64 = 16 * 1024 * 1024;
pub const MAX_ENTRIES: u32 = 100_000;
pub const MAX_PATH_BYTES: u32 = 16 * 1024;
// Smallest accepted serialized path is bare "a", padded to four bytes.
const MIN_RECORD_BYTES: u64 = 4 + 4 + 36;
const PACK_DIR_ENCRYPTED: u32 = 1;
const PACK_REL_FILEBASE: u32 = 2;
const PACK_SPARSE_BUNDLE: u32 = 4;
const PACK_FILE_ENCRYPTED: u32 = 1;
const PACK_FILE_REMOVAL: u32 = 2;
const PAYLOAD_BUFFER_BYTES: usize = 64 * 1024;
const ALIGNMENT: u64 = 32;

#[derive(Clone, Copy)]
enum VersionProfile {
    V2,
    V3,
}

impl VersionProfile {
    fn from_version(version: u32) -> io::Result<Self> {
        match version {
            2 => Ok(Self::V2),
            3 => Ok(Self::V3),
            // Retain the legacy refusal diagnostic for existing callers.
            _ => Err(invalid(
                "PCK format version unsupported (requires v2) for Godot 4.0–4.4, or v3 for Godot 4.5+",
            )),
        }
    }

    fn header_size(self) -> u64 {
        match self {
            Self::V2 => HEADER_SIZE,
            Self::V3 => V3_HEADER_SIZE,
        }
    }

    fn reserved_position(self) -> usize {
        match self {
            Self::V2 => 32,
            Self::V3 => 40,
        }
    }

    fn check_engine(self, version: [u32; 3]) -> io::Result<()> {
        match self {
            Self::V2 if version[0] == 4 && version[1] <= 4 => Ok(()),
            Self::V3 if version[0] == 4 && version[1] >= 5 => Ok(()),
            Self::V2 => Err(invalid(
                "PCK engine version unsupported (requires Godot 4.0–4.4)",
            )),
            Self::V3 => Err(invalid(
                "PCK v3 engine version unsupported (requires Godot 4.5+)",
            )),
        }
    }
}

/// One index record, in original directory order. All positions are absolute.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PckEntry {
    /// Canonical, case-sensitive virtual identity; never a filesystem path.
    pub path: String,
    pub path_length_position: u64,
    pub path_position: u64,
    /// Original padded byte length, not the canonical name's length.
    pub path_length: u32,
    /// Original UTF-8 spelling including its 0-3 zero padding bytes.
    pub padded_path: Vec<u8>,
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
#[derive(Debug, PartialEq, Eq)]
pub struct PckIndex {
    pub file_size: u64,
    pub format_version: u32,
    pub engine_version: [u32; 3],
    pub flags: u32,
    /// Raw header field at byte 24; relative-to-start is identical at start zero.
    pub file_base: u64,
    /// Opaque reserved words at bytes 32..96 (v2) or 40..104 (v3).
    pub reserved: [u32; 16],
    /// V2 first record (100); v3 count position from the header's byte-32 field.
    pub directory_offset: u64,
    /// First byte after the directory, excluding any following padding.
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
    let tail = text.strip_prefix("res://").unwrap_or(text);
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
    let mut header = [0; V3_HEADER_SIZE as usize];
    reader.read_exact(&mut header[..HEADER_SIZE as usize])?;
    if &header[..4] != b"GDPC" {
        return Err(invalid(
            "PCK magic missing at offset zero; embedded/executable packs are unsupported",
        ));
    }
    let format_version = u32_at(&header, 4);
    let profile = VersionProfile::from_version(format_version)?;
    if file_size < profile.header_size() {
        return Err(invalid("PCK v3 header is truncated (requires 104 bytes)"));
    }
    if matches!(profile, VersionProfile::V3) {
        reader.read_exact(&mut header[HEADER_SIZE as usize..])?;
    }
    let engine_version = [u32_at(&header, 8), u32_at(&header, 12), u32_at(&header, 16)];
    profile.check_engine(engine_version)?;
    let flags = u32_at(&header, 20);
    if flags & PACK_DIR_ENCRYPTED != 0 {
        return Err(invalid("PCK encrypted directory is unsupported"));
    }
    if flags & PACK_SPARSE_BUNDLE != 0 {
        return Err(invalid(
            "PCK unknown header flags are unsupported: sparse bundles",
        ));
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
        *word = u32_at(&header, profile.reserved_position() + i * 4);
    }
    let (directory_offset, mut position, count) = match profile {
        VersionProfile::V2 => (HEADER_SIZE, HEADER_SIZE, u32_at(&header, 96)),
        VersionProfile::V3 => {
            let directory_offset = u64_at(&header, 32);
            let records = directory_offset
                .checked_add(4)
                .ok_or_else(|| invalid("PCK directory offset overflow"))?;
            if directory_offset < profile.header_size() {
                return Err(invalid("PCK directory overlaps header"));
            }
            if records > file_size {
                return Err(invalid("PCK directory count is truncated or exceeds EOF"));
            }
            reader.seek(SeekFrom::Start(directory_offset))?;
            let mut count = [0; 4];
            reader.read_exact(&mut count)?;
            (directory_offset, records, u32::from_le_bytes(count))
        }
    };
    if count > MAX_ENTRIES {
        return Err(invalid("PCK entry count exceeds 100000 limit"));
    }
    let index_limit = file_size.min(directory_offset.saturating_add(MAX_INDEX_BYTES));
    let minimum_bytes = u64::from(count) * MIN_RECORD_BYTES;
    if minimum_bytes > index_limit - position {
        return Err(invalid("PCK entry count cannot fit bounded index"));
    }
    let mut entries = Vec::with_capacity(count as usize);
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
        if flags & PACK_FILE_REMOVAL != 0 {
            return Err(invalid(
                "PCK unknown member flags are unsupported: removal entries",
            ));
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
            padded_path: path_bytes,
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
        let end = entry.offset + entry.size; // Both additions checked above.
        let in_header = entry.offset < profile.header_size();
        let in_directory =
            entry.offset < position && (end > directory_offset || entry.offset >= directory_offset);
        if in_header || in_directory {
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
        directory_offset,
        index_end: position,
        entries,
    })
}

// Exactly size bytes are read; neither hashing nor copying probes the next byte.
fn copy_hashed<R: Read + Seek, W: Write>(
    reader: &mut R,
    entry: &PckEntry,
    writer: &mut W,
) -> io::Result<bool> {
    entry
        .offset
        .checked_add(entry.size)
        .ok_or_else(|| invalid("PCK member range overflow"))?;
    reader.seek(SeekFrom::Start(entry.offset))?;
    let mut buffer = [0; PAYLOAD_BUFFER_BYTES];
    let mut remaining = entry.size;
    let mut digest = Md5::new();
    while remaining != 0 {
        let n = remaining.min(buffer.len() as u64) as usize;
        reader.read_exact(&mut buffer[..n])?;
        digest.update(&buffer[..n]);
        writer.write_all(&buffer[..n])?;
        remaining -= n as u64;
    }
    Ok(<[u8; 16]>::from(digest.finalize()) == entry.stored_md5)
}

/// Stream only this validated entry's range and compare its plain-byte MD5.
/// Returns false on mismatch; truncation and source I/O failures are errors.
/// The entry must come from `read_index` on this stable source. No index or
/// unselected payload is read, and the entry's metadata is never marked verified.
pub fn verify_member<R: Read + Seek>(reader: &mut R, entry: &PckEntry) -> io::Result<bool> {
    copy_hashed(reader, entry, &mut io::sink())
}

/// Read a selected member only if its size fits the caller's cap and its MD5
/// matches. A cap refusal performs no source I/O; a mismatch returns no bytes.
pub fn read_member<R: Read + Seek>(
    reader: &mut R,
    entry: &PckEntry,
    max_bytes: u64,
) -> io::Result<Vec<u8>> {
    if entry.size > max_bytes {
        return Err(invalid("PCK member exceeds selected read size cap"));
    }
    let size = usize::try_from(entry.size).map_err(|_| invalid("PCK member cannot fit memory"))?;
    let mut bytes = Vec::new();
    bytes
        .try_reserve_exact(size)
        .map_err(|_| invalid("PCK selected member allocation failed"))?;
    if !copy_hashed(reader, entry, &mut bytes)? {
        return Err(invalid("PCK member MD5 mismatch"));
    }
    Ok(bytes)
}

/// Opaque expectation produced by a successful rebuild, not by parsing its
/// output. Retain it for independent verification before committing the sink.
#[derive(Debug)]
pub struct RebuildPlan {
    index: PckIndex,
}

/// A canonical virtual path and caller-owned replacement/addition bytes.
/// Slices preserve input order and let duplicate keys be rejected explicitly.
pub type MemberBytes<'a> = (&'a str, &'a [u8]);

fn checked_add(a: u64, b: u64) -> io::Result<u64> {
    a.checked_add(b)
        .ok_or_else(|| invalid("PCK rebuild arithmetic overflow"))
}

fn aligned(position: u64) -> io::Result<u64> {
    checked_add(position, (ALIGNMENT - position % ALIGNMENT) % ALIGNMENT)
}

fn check_key(path: &str) -> io::Result<()> {
    // A maximal bare serialized name gains six bytes in canonical identity.
    if path.len() > MAX_PATH_BYTES as usize + "res://".len()
        || canonical_path(path.as_bytes())? != path
    {
        return Err(invalid(
            "PCK change key must be a bounded canonical virtual path",
        ));
    }
    Ok(())
}

/// Write a whole new standalone PCK to an empty sink positioned at byte zero.
/// The sink needs only `Write`; output offsets are planned before the first write.
/// Metadata is reread and must equal `index` (including original padded paths).
/// Both sets of keys must already be canonical according to `read_index` rules.
/// Replacements must exist; additions must be new, distinct names. All planning
/// refusals occur before output. Untouched payloads are hashed while copying;
/// on any error discard the partial sink. The source and sink must be distinct.
/// Call `verify_rebuild` before installing or otherwise trusting the result.
pub fn rebuild<R: Read + Seek, W: Write>(
    source: &mut R,
    index: &PckIndex,
    replacements: &[MemberBytes<'_>],
    additions: &[MemberBytes<'_>],
    output: &mut W,
) -> io::Result<RebuildPlan> {
    // Also protects against stale or caller-mutated public metadata.
    if read_index(source)? != *index {
        return Err(invalid("PCK source index changed before rebuild"));
    }
    let count = index
        .entries
        .len()
        .checked_add(additions.len())
        .filter(|count| *count <= MAX_ENTRIES as usize)
        .ok_or_else(|| invalid("PCK rebuilt entry count exceeds limit"))?;
    let mut names: HashSet<&str> = index.entries.iter().map(|e| e.path.as_str()).collect();
    let mut changes = HashMap::new();
    for &(path, bytes) in replacements {
        check_key(path)?;
        if !names.contains(path) {
            return Err(invalid("PCK replacement path is unknown"));
        }
        if changes.insert(path, bytes).is_some() {
            return Err(invalid("PCK duplicate replacement key"));
        }
    }
    for &(path, _) in additions {
        check_key(path)?;
        if path.len().div_ceil(4) * 4 > MAX_PATH_BYTES as usize {
            return Err(invalid("PCK added padded path exceeds 16 KiB"));
        }
        if !names.insert(path) {
            return Err(invalid(
                "PCK addition collides with an existing name or key",
            ));
        }
    }
    let mut entries = Vec::with_capacity(count);
    let mut payloads = Vec::with_capacity(count);
    for old in &index.entries {
        let mut entry = old.clone();
        let payload = changes.get(old.path.as_str()).copied();
        if let Some(bytes) = payload {
            entry.size = bytes.len() as u64;
            entry.stored_md5 = Md5::digest(bytes).into();
        }
        entries.push(entry);
        payloads.push(payload);
    }
    for &(path, bytes) in additions {
        let mut padded_path = path.as_bytes().to_vec();
        padded_path.resize(path.len().div_ceil(4) * 4, 0);
        entries.push(PckEntry {
            path: path.to_owned(),
            path_length_position: 0,
            path_position: 0,
            path_length: padded_path.len() as u32,
            padded_path,
            offset_position: 0,
            size_position: 0,
            md5_position: 0,
            flags_position: 0,
            stored_offset: 0,
            offset: 0,
            size: bytes.len() as u64,
            stored_md5: Md5::digest(bytes).into(),
            flags: 0,
        });
        payloads.push(Some(bytes));
    }
    let profile = VersionProfile::from_version(index.format_version)?;
    let mut directory_bytes = if matches!(profile, VersionProfile::V3) {
        4
    } else {
        0
    };
    for entry in &entries {
        directory_bytes = checked_add(directory_bytes, 40 + u64::from(entry.path_length))?;
    }
    if directory_bytes > MAX_INDEX_BYTES {
        return Err(invalid("PCK rebuilt index exceeds bounded index"));
    }
    let file_base = match profile {
        VersionProfile::V2 => aligned(checked_add(HEADER_SIZE, directory_bytes)?)?,
        VersionProfile::V3 => aligned(V3_HEADER_SIZE)?,
    };
    let mut payload_position = file_base;
    for entry in &mut entries {
        entry.offset = payload_position;
        entry.stored_offset = payload_position - file_base;
        payload_position = aligned(checked_add(payload_position, entry.size)?)?;
    }
    let directory_offset = match profile {
        VersionProfile::V2 => HEADER_SIZE,
        VersionProfile::V3 => {
            // Keep zero-length members' offsets outside directory metadata too.
            if entries
                .iter()
                .any(|e| e.size == 0 && e.offset == payload_position)
            {
                checked_add(payload_position, ALIGNMENT)?
            } else {
                payload_position
            }
        }
    };
    let index_end = checked_add(directory_offset, directory_bytes)?;
    let mut position = match profile {
        VersionProfile::V2 => directory_offset,
        VersionProfile::V3 => checked_add(directory_offset, 4)?,
    };
    for entry in &mut entries {
        entry.path_length_position = position;
        entry.path_position = checked_add(position, 4)?;
        entry.offset_position = checked_add(entry.path_position, u64::from(entry.path_length))?;
        entry.size_position = checked_add(entry.offset_position, 8)?;
        entry.md5_position = checked_add(entry.offset_position, 16)?;
        entry.flags_position = checked_add(entry.offset_position, 32)?;
        position = checked_add(entry.offset_position, 36)?;
    }
    let plan = RebuildPlan {
        index: PckIndex {
            file_size: match profile {
                VersionProfile::V2 => payload_position,
                VersionProfile::V3 => index_end,
            },
            format_version: index.format_version,
            engine_version: index.engine_version,
            flags: index.flags,
            file_base,
            reserved: index.reserved,
            directory_offset,
            index_end,
            entries,
        },
    };
    output.write_all(b"GDPC")?;
    for word in [
        index.format_version,
        index.engine_version[0],
        index.engine_version[1],
        index.engine_version[2],
        index.flags,
    ] {
        output.write_all(&word.to_le_bytes())?;
    }
    output.write_all(&file_base.to_le_bytes())?;
    if matches!(profile, VersionProfile::V3) {
        output.write_all(&directory_offset.to_le_bytes())?;
    }
    for word in index.reserved {
        output.write_all(&word.to_le_bytes())?;
    }
    let padding = [0; ALIGNMENT as usize];
    match profile {
        VersionProfile::V2 => {
            write_directory(output, &plan.index)?;
            output.write_all(&padding[..(file_base - index_end) as usize])?;
        }
        VersionProfile::V3 => {
            output.write_all(&padding[..(file_base - V3_HEADER_SIZE) as usize])?;
        }
    }
    for (number, entry) in plan.index.entries.iter().enumerate() {
        if let Some(bytes) = payloads[number] {
            for chunk in bytes.chunks(PAYLOAD_BUFFER_BYTES) {
                output.write_all(chunk)?;
            }
        } else if !copy_hashed(source, &index.entries[number], output)? {
            return Err(invalid("PCK untouched member MD5 mismatch during rebuild"));
        }
        let end = entry.offset + entry.size; // Checked during planning, including padding.
        output.write_all(&padding[..(aligned(end)? - end) as usize])?;
    }
    if matches!(profile, VersionProfile::V3) {
        output.write_all(&padding[..(directory_offset - payload_position) as usize])?;
        write_directory(output, &plan.index)?;
    }
    Ok(plan)
}

fn write_directory<W: Write>(output: &mut W, index: &PckIndex) -> io::Result<()> {
    output.write_all(&(index.entries.len() as u32).to_le_bytes())?;
    for entry in &index.entries {
        output.write_all(&entry.path_length.to_le_bytes())?;
        output.write_all(&entry.padded_path)?;
        output.write_all(&entry.stored_offset.to_le_bytes())?;
        output.write_all(&entry.size.to_le_bytes())?;
        output.write_all(&entry.stored_md5)?;
        output.write_all(&entry.flags.to_le_bytes())?;
    }
    Ok(())
}

/// Independently parse the completed output, compare every planned header/entry
/// field, original padded path, order and total length, then stream every MD5.
/// A plan never adopts metadata from the generated file. Matching stored MD5s
/// alone are insufficient: this also checks them against the expected plan.
pub fn verify_rebuild<R: Read + Seek>(output: &mut R, plan: &RebuildPlan) -> io::Result<()> {
    let actual = read_index(output)?;
    if actual != plan.index {
        return Err(invalid("PCK generated index differs from rebuild plan"));
    }
    for entry in &actual.entries {
        if !verify_member(output, entry)? {
            return Err(invalid("PCK generated member MD5 mismatch"));
        }
    }
    Ok(())
}

/// Rebuild and independently verify in owned staging before transactional
/// installation. The caller must acquire the game lock before reading the source
/// and retain it through this call. Returns the retained previous-archive backup.
/// Uses the shared archive replacement/recovery mechanism; no format registration.
pub fn replace_archive(
    lock: &locust_core::patch::GameLock,
    path: &std::path::Path,
    replacements: &[MemberBytes<'_>],
    additions: &[MemberBytes<'_>],
) -> locust_core::error::Result<std::path::PathBuf> {
    crate::archive_replace::replace_file_streamed(lock, path, |staging| {
        let mut source = std::fs::File::open(path)?;
        let index = read_index(&mut source)?;
        let plan = rebuild(&mut source, &index, replacements, additions, staging)?;
        verify_rebuild(staging, &plan)?;
        // Release the original before the shared replacement moves it aside.
        drop(source);
        Ok(())
    })
}
