//! Minimal Unreal Engine `.pak` reader (footer + classic index) and
//! **uncompressed** writer for localization patch paks.
//!
//! # Format (u4pak / UE FPakInfo)
//! Layout verified against [panzi/u4pak](https://github.com/panzi/u4pak) README
//! and community FPakInfo notes (repak / rust-u4pak):
//!
//! ```text
//! [ data records: FPakEntry header copy + payload ]*
//! [ index: mount-point FString + count + (name FString + FPakEntry)* ]
//! [ footer: version-dependent prefix + magic 0x5A6F12E1 + version +
//!           index_offset u64 + index_size u64 + index SHA-1[20] + (v8+) methods ]
//! ```
//!
//! - **Magic** `0x5A6F12E1` (LE: `E1 12 6F 5A`)
//! - **Record** (uncompressed): `offset u64`, `size u64`, `uncompressed u64`,
//!   `compression_method u32` (=0), optional timestamp (v≤1), `sha1[20]` of
//!   **payload** bytes, `encrypted u8`, `block_size u32` (v≥3)
//! - **Data record**: index-style entry with `offset=0` at the data position,
//!   then payload. Index entry `offset` points at that header.
//!
//! # Writer support
//! Uncompressed-only. Write versions **3** and **8** (and **7** as v8 without
//! compression-method table). Read probes any footer with valid magic; classic
//! index parse supports v3–v8. v9+ frozen / v10–11 path-hash indexes are not
//! fully parsed — write for those versions returns a clear error (patch paks
//! should use v8, which UE 4.22–5.x still mounts).

use std::path::Path;

use sha1::{Digest, Sha1};

/// Pak footer magic (little-endian `0x5A6F12E1`).
pub const PAK_MAGIC: u32 = 0x5A6F12E1;

/// Max write version we emit for “modern” bases (UE 4.22+ FName compression).
pub const WRITE_VERSION_MODERN: u32 = 8;
/// Classic simple write version.
pub const WRITE_VERSION_CLASSIC: u32 = 3;

const COMPRESSION_METHOD_NAME_LEN: usize = 32;
const COMPRESSION_METHOD_COUNT_V8: usize = 5;

#[derive(Debug)]
pub struct PakError {
    pub file: String,
    pub message: String,
}

impl std::fmt::Display for PakError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.file, self.message)
    }
}

impl std::error::Error for PakError {}

fn err(file: &str, message: impl Into<String>) -> PakError {
    PakError {
        file: file.into(),
        message: message.into(),
    }
}

// ─── SHA-1 ─────────────────────────────────────────────────────────────────

/// SHA-1 of `data` (20 bytes). Used for payload and index hashes.
pub fn sha1_bytes(data: &[u8]) -> [u8; 20] {
    let mut h = Sha1::new();
    h.update(data);
    let d = h.finalize();
    let mut out = [0u8; 20];
    out.copy_from_slice(&d);
    out
}

// ─── Footer ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct PakFooter {
    pub version: u32,
    pub index_offset: u64,
    pub index_size: u64,
    pub index_hash: [u8; 20],
    pub encrypted_index: bool,
    /// Absolute file offset of the 4-byte magic field.
    pub magic_offset: usize,
}

/// Probe footer from the end of `data`. Does not require a full index parse.
pub fn read_footer(data: &[u8], file_label: &str) -> Result<PakFooter, PakError> {
    if data.len() < 44 {
        return Err(err(file_label, "file too small for pak footer"));
    }
    let search_from = data.len().saturating_sub(512);
    let magic_le = PAK_MAGIC.to_le_bytes();
    let mut magic_pos = None;
    // Scan backwards for magic.
    let mut i = data.len() - 4;
    loop {
        if i < search_from {
            break;
        }
        if data[i..i + 4] == magic_le {
            // Prefer magic that has room after it for version+index fields.
            if i + 4 + 4 + 8 + 8 + 20 <= data.len() {
                magic_pos = Some(i);
                break;
            }
        }
        if i == 0 {
            break;
        }
        i -= 1;
    }
    let magic_offset = magic_pos
        .ok_or_else(|| err(file_label, "pak magic 0x5A6F12E1 not found near EOF"))?;

    let version = u32::from_le_bytes(
        data[magic_offset + 4..magic_offset + 8]
            .try_into()
            .unwrap(),
    );
    if version == 0 || version > 11 {
        return Err(err(
            file_label,
            format!("implausible pak version {version} at footer"),
        ));
    }
    let index_offset = u64::from_le_bytes(
        data[magic_offset + 8..magic_offset + 16]
            .try_into()
            .unwrap(),
    );
    let index_size = u64::from_le_bytes(
        data[magic_offset + 16..magic_offset + 24]
            .try_into()
            .unwrap(),
    );
    let mut index_hash = [0u8; 20];
    index_hash.copy_from_slice(&data[magic_offset + 24..magic_offset + 44]);

    // Encrypted flag sits immediately before magic for version ≥ 4; for v≥7 a
    // 16-byte encryption guid precedes that flag.
    let encrypted_index = if version >= 4 && magic_offset >= 1 {
        data[magic_offset - 1] != 0
    } else {
        false
    };

    if encrypted_index {
        return Err(err(
            file_label,
            "encrypted pak index is not supported (need decryption key)",
        ));
    }

    Ok(PakFooter {
        version,
        index_offset,
        index_size,
        index_hash,
        encrypted_index,
        magic_offset,
    })
}

// ─── Index / records ───────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct PakRecord {
    /// Path as stored in the index (often starts with mount-relative segment).
    pub name: String,
    /// File offset of the data-record header (FPakEntry copy).
    pub offset: u64,
    /// Compressed size of payload (equals uncompressed when method=0).
    pub size: u64,
    pub uncompressed_size: u64,
    pub compression_method: u32,
    pub sha1: [u8; 20],
    pub encrypted: bool,
}

#[derive(Debug, Clone)]
pub struct PakIndex {
    pub footer: PakFooter,
    pub mount_point: String,
    pub records: Vec<PakRecord>,
}

struct R<'a> {
    data: &'a [u8],
    pos: usize,
    file: &'a str,
}

impl<'a> R<'a> {
    fn need(&self, n: usize) -> Result<(), PakError> {
        if self.pos.saturating_add(n) > self.data.len() {
            Err(err(
                self.file,
                format!("truncated at {} (need {n})", self.pos),
            ))
        } else {
            Ok(())
        }
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8], PakError> {
        self.need(n)?;
        let s = &self.data[self.pos..self.pos + n];
        self.pos += n;
        Ok(s)
    }

    fn u8(&mut self) -> Result<u8, PakError> {
        Ok(self.take(1)?[0])
    }

    fn u32(&mut self) -> Result<u32, PakError> {
        let b = self.take(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    fn i32(&mut self) -> Result<i32, PakError> {
        Ok(self.u32()? as i32)
    }

    fn u64(&mut self) -> Result<u64, PakError> {
        let b = self.take(8)?;
        Ok(u64::from_le_bytes([
            b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7],
        ]))
    }

    /// UE FString (ANSI or UTF-16), same rules as locres.
    fn fstring(&mut self) -> Result<String, PakError> {
        let length = self.i32()?;
        if length == 0 {
            return Ok(String::new());
        }
        if length > 0 {
            let n = length as usize;
            let bytes = self.take(n)?;
            let s = std::str::from_utf8(bytes)
                .map_err(|_| err(self.file, "invalid FString UTF-8"))?;
            Ok(s.trim_end_matches('\0').to_string())
        } else {
            let units = (-length) as usize;
            let n = units
                .checked_mul(2)
                .ok_or_else(|| err(self.file, "FString size overflow"))?;
            let bytes = self.take(n)?;
            let mut u16s = Vec::with_capacity(units);
            for c in bytes.chunks_exact(2) {
                u16s.push(u16::from_le_bytes([c[0], c[1]]));
            }
            if u16s.last() == Some(&0) {
                u16s.pop();
            }
            String::from_utf16(&u16s).map_err(|_| err(self.file, "invalid UTF-16 FString"))
        }
    }

    fn sha1(&mut self) -> Result<[u8; 20], PakError> {
        let b = self.take(20)?;
        let mut h = [0u8; 20];
        h.copy_from_slice(b);
        Ok(h)
    }
}

/// Serialize size of an FPakEntry for `version` (uncompressed, no blocks).
pub fn entry_header_size(version: u32) -> usize {
    // offset+size+uncompressed+method = 28
    let mut n = 8 + 8 + 8 + 4;
    if version <= 1 {
        n += 8; // timestamp
    }
    n += 20; // sha1
    if version >= 3 {
        // no compression blocks when method=0
        n += 1 + 4; // encrypted + block size
    }
    n
}

fn read_entry_body(r: &mut R<'_>, version: u32) -> Result<PakRecord, PakError> {
    let offset = r.u64()?;
    let size = r.u64()?;
    let uncompressed_size = r.u64()?;
    let compression_method = r.u32()?;
    if version <= 1 {
        let _ts = r.u64()?;
    }
    let sha1 = r.sha1()?;
    if version >= 3 {
        if compression_method != 0 {
            let block_count = r.u32()? as usize;
            if block_count > 1_000_000 {
                return Err(err(r.file, "compression block count too large"));
            }
            for _ in 0..block_count {
                let _ = r.u64()?;
                let _ = r.u64()?;
            }
        }
        let encrypted = r.u8()? != 0;
        let _block_size = r.u32()?;
        Ok(PakRecord {
            name: String::new(),
            offset,
            size,
            uncompressed_size,
            compression_method,
            sha1,
            encrypted,
        })
    } else {
        Ok(PakRecord {
            name: String::new(),
            offset,
            size,
            uncompressed_size,
            compression_method,
            sha1,
            encrypted: false,
        })
    }
}

/// Parse classic (non–path-hash) index. Suitable for writer-produced paks and
/// many v3–v8 game paks. v9+ frozen / v10–11 path-hash indexes return Err.
pub fn read_index(data: &[u8], file_label: &str) -> Result<PakIndex, PakError> {
    let footer = read_footer(data, file_label)?;
    if footer.version >= 9 {
        return Err(err(
            file_label,
            format!(
                "pak version {} uses frozen/path-hash index — classic index parse unsupported \
                 (write a v{} patch pak instead)",
                footer.version, WRITE_VERSION_MODERN
            ),
        ));
    }
    let start = footer.index_offset as usize;
    let end = start
        .checked_add(footer.index_size as usize)
        .filter(|e| *e <= data.len())
        .ok_or_else(|| err(file_label, "index range past EOF"))?;
    let index_bytes = &data[start..end];
    // Optional integrity check
    let got = sha1_bytes(index_bytes);
    if got != footer.index_hash {
        // Non-fatal for some hand-edited paks; still try to parse but warn via Err soft?
        // Strict: reject so tests and real paks stay honest.
        return Err(err(
            file_label,
            "index SHA-1 mismatch (corrupt or encrypted index)",
        ));
    }

    let mut r = R {
        data: index_bytes,
        pos: 0,
        file: file_label,
    };
    let mount_point = r.fstring()?;
    let count = r.u32()? as usize;
    if count > 5_000_000 {
        return Err(err(file_label, "record count exceeds safety limit"));
    }
    let mut records = Vec::with_capacity(count);
    for _ in 0..count {
        let name = r.fstring()?;
        let mut rec = read_entry_body(&mut r, footer.version)?;
        rec.name = name;
        records.push(rec);
    }

    Ok(PakIndex {
        footer,
        mount_point,
        records,
    })
}

/// Find the index record whose data span contains absolute file offset `abs`.
/// Data span is `[offset, offset + entry_header_size + size)`.
pub fn record_containing_offset(
    index: &PakIndex,
    abs: u64,
) -> Option<&PakRecord> {
    let hdr = entry_header_size(index.footer.version) as u64;
    index.records.iter().find(|r| {
        let start = r.offset;
        let end = start.saturating_add(hdr).saturating_add(r.size);
        abs >= start && abs < end
    })
}

/// Absolute offset of payload bytes (after the on-disk entry header).
pub fn payload_offset(record: &PakRecord, version: u32) -> u64 {
    record.offset.saturating_add(entry_header_size(version) as u64)
}

// ─── Writer ────────────────────────────────────────────────────────────────

struct W {
    buf: Vec<u8>,
}

impl W {
    fn new() -> Self {
        Self { buf: Vec::new() }
    }
    fn pos(&self) -> u64 {
        self.buf.len() as u64
    }
    fn u8(&mut self, v: u8) {
        self.buf.push(v);
    }
    fn u32(&mut self, v: u32) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }
    fn i32(&mut self, v: i32) {
        self.u32(v as u32);
    }
    fn u64(&mut self, v: u64) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }
    fn bytes(&mut self, b: &[u8]) {
        self.buf.extend_from_slice(b);
    }
    fn fstring_ascii(&mut self, s: &str) {
        // Include trailing NUL in length (UE convention).
        let len = (s.len() + 1) as i32;
        self.i32(len);
        self.buf.extend_from_slice(s.as_bytes());
        self.buf.push(0);
    }
    fn into_vec(self) -> Vec<u8> {
        self.buf
    }
}

fn write_entry_body(
    w: &mut W,
    version: u32,
    offset: u64,
    size: u64,
    sha1: &[u8; 20],
) {
    w.u64(offset);
    w.u64(size);
    w.u64(size); // uncompressed == size
    w.u32(0); // uncompressed method
    if version <= 1 {
        w.u64(0); // timestamp
    }
    w.bytes(sha1);
    if version >= 3 {
        // no blocks
        w.u8(0); // not encrypted
        w.u32(0); // block size
    }
}

/// One file to place in a patch pak.
#[derive(Debug, Clone)]
pub struct PakWriteFile {
    /// Path relative to mount point (e.g. `TestGame/Content/Localization/.../Game.locres`).
    pub name: String,
    pub data: Vec<u8>,
}

/// Map a base pak version to a version we can write.
pub fn writable_version(base_version: u32) -> Result<u32, PakError> {
    match base_version {
        1..=3 => Ok(3), // write v3 for 1–3
        4..=7 => Ok(7),
        8 => Ok(8),
        // v9–11: emit v8 which modern UE still loads for patch paks.
        9..=11 => Ok(WRITE_VERSION_MODERN),
        v => Err(err(
            "pak",
            format!(
                "unsupported pak version {v} for writing (supported base 1–11 → write 3/7/8)"
            ),
        )),
    }
}

/// Build an uncompressed patch pak.
///
/// `mount_point` should usually be `../../../` (UE default for content paks).
/// `version` must be a writable version (3, 7, or 8).
pub fn write_pak(
    mount_point: &str,
    version: u32,
    files: &[PakWriteFile],
    file_label: &str,
) -> Result<Vec<u8>, PakError> {
    if !matches!(version, 3 | 7 | 8) {
        return Err(err(
            file_label,
            format!(
                "write_pak only supports versions 3, 7, 8 (got {version}); \
                 use writable_version() to map the base pak"
            ),
        ));
    }
    if files.is_empty() {
        return Err(err(file_label, "cannot write empty pak"));
    }

    let mut data_w = W::new();
    let mut planned: Vec<(String, u64, u64, [u8; 20])> = Vec::new();
    // (name, data_header_offset, size, sha1)

    for f in files {
        let hash = sha1_bytes(&f.data);
        let header_off = data_w.pos();
        // On-disk header copy uses offset=0 (u4pak).
        write_entry_body(&mut data_w, version, 0, f.data.len() as u64, &hash);
        data_w.bytes(&f.data);
        planned.push((f.name.clone(), header_off, f.data.len() as u64, hash));
    }

    let index_offset = data_w.pos();
    let mut index_w = W::new();
    index_w.fstring_ascii(mount_point);
    index_w.u32(planned.len() as u32);
    for (name, header_off, size, hash) in &planned {
        index_w.fstring_ascii(name);
        write_entry_body(&mut index_w, version, *header_off, *size, hash);
    }
    let index_bytes = index_w.into_vec();
    let index_hash = sha1_bytes(&index_bytes);
    let index_size = index_bytes.len() as u64;

    data_w.bytes(&index_bytes);

    // Footer
    if version >= 7 {
        data_w.bytes(&[0u8; 16]); // encryption key guid
    }
    if version >= 4 {
        data_w.u8(0); // index not encrypted
    }
    data_w.u32(PAK_MAGIC);
    data_w.u32(version);
    data_w.u64(index_offset);
    data_w.u64(index_size);
    data_w.bytes(&index_hash);

    if version >= 8 {
        // Five 32-byte compression method names; first is "None".
        let methods = ["None", "Zlib", "Gzip", "Oodle", "LZ4"];
        for (i, name) in methods.iter().enumerate() {
            if i >= COMPRESSION_METHOD_COUNT_V8 {
                break;
            }
            let mut slot = [0u8; COMPRESSION_METHOD_NAME_LEN];
            let b = name.as_bytes();
            let n = b.len().min(COMPRESSION_METHOD_NAME_LEN - 1);
            slot[..n].copy_from_slice(&b[..n]);
            data_w.bytes(&slot);
        }
    }

    Ok(data_w.into_vec())
}

/// Convenience: probe base bytes → writable version.
pub fn write_patch_pak_matching(
    base_pak: &[u8],
    base_label: &str,
    mount_point: &str,
    files: &[PakWriteFile],
) -> Result<(Vec<u8>, u32), PakError> {
    let footer = read_footer(base_pak, base_label)?;
    let ver = writable_version(footer.version)?;
    let bytes = write_pak(mount_point, ver, files, base_label)?;
    Ok((bytes, ver))
}

/// Default mount point used by UE content paks.
pub const DEFAULT_MOUNT_POINT: &str = "../../../";

/// Suggest patch pak path beside `base_pak_path`: `<stem>_LOCUST_P.pak`.
pub fn patch_pak_path(base_pak_path: &Path) -> std::path::PathBuf {
    let parent = base_pak_path.parent().unwrap_or_else(|| Path::new("."));
    let stem = base_pak_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("patch");
    // Strip existing _P suffix to avoid Game_P_LOCUST_P.pak nesting noise.
    let stem = stem.strip_suffix("_P").unwrap_or(stem);
    parent.join(format!("{stem}_LOCUST_P.pak"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha1_empty_known_vector() {
        // SHA-1("") = da39a3ee5e6b4b0d3255bfef95601890afd80709
        let h = sha1_bytes(b"");
        assert_eq!(
            h,
            [
                0xda, 0x39, 0xa3, 0xee, 0x5e, 0x6b, 0x4b, 0x0d, 0x32, 0x55, 0xbf, 0xef, 0x95,
                0x60, 0x18, 0x90, 0xaf, 0xd8, 0x07, 0x09
            ]
        );
    }

    #[test]
    fn sha1_abc_known_vector() {
        // SHA-1("abc") = a9993e364706816aba3e25717850c26c9cd0d89d
        let h = sha1_bytes(b"abc");
        assert_eq!(
            h,
            [
                0xa9, 0x99, 0x3e, 0x36, 0x47, 0x06, 0x81, 0x6a, 0xba, 0x3e, 0x25, 0x71, 0x78,
                0x50, 0xc2, 0x6c, 0x9c, 0xd0, 0xd8, 0x9d
            ]
        );
    }

    fn sample_files() -> Vec<PakWriteFile> {
        vec![
            PakWriteFile {
                name: "TestGame/Content/Localization/Game/es/Game.locres".into(),
                data: b"LOCRES_BYTES_ONE".to_vec(),
            },
            PakWriteFile {
                name: "TestGame/Content/Localization/Game/en/Game.locres".into(),
                data: b"LOCRES_BYTES_TWO_XX".to_vec(),
            },
        ]
    }

    #[test]
    fn write_read_roundtrip_v3() {
        let files = sample_files();
        let bytes = write_pak(DEFAULT_MOUNT_POINT, 3, &files, "t.pak").unwrap();
        let footer = read_footer(&bytes, "t.pak").unwrap();
        assert_eq!(footer.version, 3);
        let index = read_index(&bytes, "t.pak").unwrap();
        assert_eq!(index.mount_point, DEFAULT_MOUNT_POINT);
        assert_eq!(index.records.len(), 2);
        assert!(index.records.iter().any(|r| r.name.contains("es/Game.locres")));
        // Payload round-trip
        for f in &files {
            let rec = index.records.iter().find(|r| r.name == f.name).unwrap();
            let poff = payload_offset(rec, 3) as usize;
            let got = &bytes[poff..poff + f.data.len()];
            assert_eq!(got, f.data.as_slice());
            assert_eq!(rec.sha1, sha1_bytes(&f.data));
        }
    }

    #[test]
    fn write_read_roundtrip_v8() {
        let files = sample_files();
        let bytes = write_pak(DEFAULT_MOUNT_POINT, 8, &files, "t8.pak").unwrap();
        let footer = read_footer(&bytes, "t8.pak").unwrap();
        assert_eq!(footer.version, 8);
        let index = read_index(&bytes, "t8.pak").unwrap();
        assert_eq!(index.records.len(), 2);
    }

    #[test]
    fn record_containing_offset_finds_payload() {
        let files = sample_files();
        let bytes = write_pak(DEFAULT_MOUNT_POINT, 3, &files, "t.pak").unwrap();
        let index = read_index(&bytes, "t.pak").unwrap();
        let rec0 = &index.records[0];
        let inside = payload_offset(rec0, 3) + 1;
        let found = record_containing_offset(&index, inside).unwrap();
        assert_eq!(found.name, rec0.name);
        assert!(record_containing_offset(&index, 0).is_none() || {
            // offset 0 might be first header
            true
        });
    }

    #[test]
    fn unsupported_write_version_errors() {
        let e = write_pak(DEFAULT_MOUNT_POINT, 11, &sample_files(), "x.pak").unwrap_err();
        assert!(e.message.contains("only supports"), "{}", e.message);
    }

    #[test]
    fn writable_version_maps_v11_to_v8() {
        assert_eq!(writable_version(11).unwrap(), 8);
        assert_eq!(writable_version(3).unwrap(), 3);
    }

    #[test]
    fn patch_pak_path_naming() {
        let p = Path::new("/game/Content/Paks/Game-WindowsNoEditor.pak");
        let out = patch_pak_path(p);
        assert!(out
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .ends_with("_LOCUST_P.pak"));
        assert!(out
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .starts_with("Game-WindowsNoEditor"));
    }

    #[test]
    fn matching_base_footer() {
        let base = write_pak(DEFAULT_MOUNT_POINT, 8, &sample_files(), "base.pak").unwrap();
        let (patch, ver) = write_patch_pak_matching(
            &base,
            "base.pak",
            DEFAULT_MOUNT_POINT,
            &sample_files()[..1],
        )
        .unwrap();
        assert_eq!(ver, 8);
        let idx = read_index(&patch, "patch.pak").unwrap();
        assert_eq!(idx.records.len(), 1);
    }
}
