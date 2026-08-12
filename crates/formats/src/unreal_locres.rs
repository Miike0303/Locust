//! Unreal Engine `.locres` binary localization reader/writer.
//!
//! # Format (UnrealLocres / UE LocRes)
//! Spec verified against [akintos/UnrealLocres](https://github.com/akintos/UnrealLocres)
//! `LocresFile.cs` + `BinaryReaderExtensions.ReadUnrealString`.
//!
//! - **Magic** (16 bytes) when version ≥ Compact:
//!   `0E 14 74 75 67 4A 03 FC 4A 15 90 9D C3 37 7F 1B`
//! - **Version byte**: `Legacy=0` (no magic), `Compact=1`, `Optimized=2`,
//!   `Optimized_CityHash64_UTF16=3`
//! - **Compact+**: `i64` absolute offset to string array; then namespace table;
//!   string array at that offset = `i32` count + FStrings (+ `i32` refCount for Optimized+)
//! - **Optimized+**: `i32` total entry count before namespaces; per-namespace and
//!   per-key `u32` name hashes; each key has `source_string_hash: u32` + string index
//! - **FString**: `i32` length — positive = ANSI/ASCII incl. trailing NUL;
//!   negative = UTF-16LE, `|length|` code units incl. NUL
//!
//! # Write policy
//! Emit the **same** version that was read. Key/namespace/source hashes are
//! **preserved** from the original entry (engine uses `source_string_hash` as the
//! identity of the source string, not the localized value). Version-3 CityHash
//! key hashes are therefore preserved without implementing CityHash64. New
//! synthetic fixtures use [`str_crc32_ue`] for source hashes.
//!
//! # Round-trip
//! Semantic equality always. Byte-identity for `serialize(parse(x))` holds for
//! files we serialize ourselves (stable string-table order = first-seen value).
//! Third-party files may differ in FString encoding choice (ASCII vs UTF-16) or
//! string-table order while remaining semantically equal.

use std::collections::HashMap;
use std::path::Path;

/// LocRes magic GUID (UnrealLocres byte order).
pub const LOCRES_MAGIC: [u8; 16] = [
    0x0E, 0x14, 0x74, 0x75, 0x67, 0x4A, 0x03, 0xFC, 0x4A, 0x15, 0x90, 0x9D, 0xC3, 0x37, 0x7F,
    0x1B,
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum LocresVersion {
    Legacy = 0,
    Compact = 1,
    Optimized = 2,
    OptimizedCityHash64Utf16 = 3,
}

impl LocresVersion {
    fn from_u8(v: u8) -> Option<Self> {
        match v {
            0 => Some(Self::Legacy),
            1 => Some(Self::Compact),
            2 => Some(Self::Optimized),
            3 => Some(Self::OptimizedCityHash64Utf16),
            _ => None,
        }
    }

    fn is_optimized(self) -> bool {
        matches!(self, Self::Optimized | Self::OptimizedCityHash64Utf16)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocresString {
    pub key: String,
    pub value: String,
    /// Hash of the **source** language string (preserved across translation).
    pub source_string_hash: u32,
    /// Key name hash as stored (Optimized+); 0 for Compact/Legacy.
    pub key_hash: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocresNamespace {
    pub name: String,
    /// Namespace name hash (Optimized+); 0 otherwise.
    pub name_hash: u32,
    pub strings: Vec<LocresString>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocresFile {
    pub version: LocresVersion,
    pub namespaces: Vec<LocresNamespace>,
}

#[derive(Debug)]
pub struct LocresError {
    pub file: String,
    pub message: String,
}

impl std::fmt::Display for LocresError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.file, self.message)
    }
}

impl std::error::Error for LocresError {}

fn err(file: &str, message: impl Into<String>) -> LocresError {
    LocresError {
        file: file.into(),
        message: message.into(),
    }
}

// ─── CRC (UE FCrc::StrCrc32 on UTF-16 code units) ───────────────────────────

/// Unreal `FCrc::StrCrc32` over UTF-16LE code units (used for source_string_hash
/// and Optimized key/namespace hashes in synthetic fixtures).
pub fn str_crc32_ue(s: &str) -> u32 {
    let table = crc_table();
    let mut crc: u32 = !0u32;
    for unit in s.encode_utf16() {
        let lo = (unit & 0xFF) as u8;
        let hi = (unit >> 8) as u8;
        crc = (crc >> 8) ^ table[((crc ^ lo as u32) & 0xFF) as usize];
        crc = (crc >> 8) ^ table[((crc ^ hi as u32) & 0xFF) as usize];
    }
    !crc
}

fn crc_table() -> &'static [u32; 256] {
    use std::sync::OnceLock;
    static TABLE: OnceLock<[u32; 256]> = OnceLock::new();
    TABLE.get_or_init(|| {
        let mut table = [0u32; 256];
        for i in 0..256u32 {
            let mut c = i;
            for _ in 0..8 {
                if c & 1 != 0 {
                    c = 0xEDB8_8320 ^ (c >> 1);
                } else {
                    c >>= 1;
                }
            }
            table[i as usize] = c;
        }
        table
    })
}

// ─── Binary helpers ────────────────────────────────────────────────────────

struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
    file: &'a str,
}

impl<'a> Reader<'a> {
    fn new(data: &'a [u8], file: &'a str) -> Self {
        Self { data, pos: 0, file }
    }

    fn remaining(&self) -> usize {
        self.data.len().saturating_sub(self.pos)
    }

    fn need(&self, n: usize) -> Result<(), LocresError> {
        if self.remaining() < n {
            Err(err(
                self.file,
                format!("truncated at offset {} (need {n} bytes)", self.pos),
            ))
        } else {
            Ok(())
        }
    }

    fn read_exact(&mut self, n: usize) -> Result<&'a [u8], LocresError> {
        self.need(n)?;
        let s = &self.data[self.pos..self.pos + n];
        self.pos += n;
        Ok(s)
    }

    fn u8(&mut self) -> Result<u8, LocresError> {
        Ok(self.read_exact(1)?[0])
    }

    fn i32(&mut self) -> Result<i32, LocresError> {
        let b = self.read_exact(4)?;
        Ok(i32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    fn u32(&mut self) -> Result<u32, LocresError> {
        let b = self.read_exact(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    fn i64(&mut self) -> Result<i64, LocresError> {
        let b = self.read_exact(8)?;
        Ok(i64::from_le_bytes([
            b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7],
        ]))
    }

    fn seek(&mut self, abs: usize) -> Result<(), LocresError> {
        if abs > self.data.len() {
            return Err(err(
                self.file,
                format!("seek {abs} past EOF ({})", self.data.len()),
            ));
        }
        self.pos = abs;
        Ok(())
    }

    /// Unreal FString (NUL-terminated in file; returned without NUL).
    fn fstring(&mut self) -> Result<String, LocresError> {
        let length = self.i32()?;
        if length == 0 {
            return Ok(String::new());
        }
        if length > 0 {
            let len = length as usize;
            let bytes = self.read_exact(len)?;
            // ANSI/UTF-8 path (UE uses "ANSI" — treat as UTF-8/latin1-ish ASCII).
            let s = std::str::from_utf8(bytes)
                .map_err(|_| err(self.file, format!("invalid ANSI FString at {}", self.pos)))?;
            Ok(s.trim_end_matches('\0').to_string())
        } else {
            let units = (-length) as usize;
            let byte_len = units
                .checked_mul(2)
                .ok_or_else(|| err(self.file, "UTF-16 FString size overflow"))?;
            let bytes = self.read_exact(byte_len)?;
            let mut u16s = Vec::with_capacity(units);
            for chunk in bytes.chunks_exact(2) {
                u16s.push(u16::from_le_bytes([chunk[0], chunk[1]]));
            }
            // Drop trailing NUL unit if present.
            if u16s.last() == Some(&0) {
                u16s.pop();
            }
            String::from_utf16(&u16s).map_err(|_| {
                err(
                    self.file,
                    format!("invalid UTF-16 FString at {}", self.pos),
                )
            })
        }
    }
}

struct Writer {
    buf: Vec<u8>,
}

impl Writer {
    fn new() -> Self {
        Self { buf: Vec::new() }
    }

    fn pos(&self) -> usize {
        self.buf.len()
    }

    fn u8(&mut self, v: u8) {
        self.buf.push(v);
    }

    fn i32(&mut self, v: i32) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    fn u32(&mut self, v: u32) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    fn i64(&mut self, v: i64) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    fn patch_i64(&mut self, at: usize, v: i64) {
        let b = v.to_le_bytes();
        self.buf[at..at + 8].copy_from_slice(&b);
    }

    fn patch_i32(&mut self, at: usize, v: i32) {
        let b = v.to_le_bytes();
        self.buf[at..at + 4].copy_from_slice(&b);
    }

    /// Write FString: ASCII+NUL when all chars are ASCII; else UTF-16LE+NUL.
    fn fstring(&mut self, s: &str) {
        if s.is_empty() {
            self.i32(0);
            return;
        }
        if s.is_ascii() {
            let len = (s.len() + 1) as i32; // incl NUL
            self.i32(len);
            self.buf.extend_from_slice(s.as_bytes());
            self.buf.push(0);
        } else {
            let units: Vec<u16> = s.encode_utf16().chain(std::iter::once(0)).collect();
            self.i32(-(units.len() as i32));
            for u in units {
                self.buf.extend_from_slice(&u.to_le_bytes());
            }
        }
    }

    fn into_vec(self) -> Vec<u8> {
        self.buf
    }
}

// ─── Parse ─────────────────────────────────────────────────────────────────

impl LocresFile {
    pub fn parse(data: &[u8], file_label: &str) -> Result<Self, LocresError> {
        if data.is_empty() {
            return Err(err(file_label, "empty file"));
        }
        let mut r = Reader::new(data, file_label);

        let version = if data.len() >= 16 && data[..16] == LOCRES_MAGIC {
            r.read_exact(16)?;
            let v = r.u8()?;
            LocresVersion::from_u8(v).ok_or_else(|| {
                err(file_label, format!("unsupported locres version byte {v}"))
            })?
        } else {
            LocresVersion::Legacy
        };

        let mut string_array: Vec<String> = Vec::new();

        if version as u8 >= LocresVersion::Compact as u8 {
            let array_offset = r.i64()?;
            if array_offset < 0 {
                return Err(err(file_label, "negative string array offset"));
            }
            let array_offset = array_offset as usize;
            let resume = r.pos;
            r.seek(array_offset)?;
            let count = r.i32()?;
            if count < 0 {
                return Err(err(file_label, "negative string array count"));
            }
            let count = count as usize;
            // Soft cap — locres string tables are not multi-GB.
            if count > 10_000_000 {
                return Err(err(
                    file_label,
                    format!("string array count {count} exceeds safety limit"),
                ));
            }
            string_array.reserve(count);
            for _ in 0..count {
                string_array.push(r.fstring()?);
                if version.is_optimized() {
                    let _ref_count = r.i32()?;
                }
            }
            r.seek(resume)?;
        }

        if version.is_optimized() {
            let _entries_count = r.i32()?;
        }

        let namespace_count = r.i32()?;
        if namespace_count < 0 {
            return Err(err(file_label, "negative namespace count"));
        }
        let namespace_count = namespace_count as usize;
        if namespace_count > 1_000_000 {
            return Err(err(
                file_label,
                format!("namespace count {namespace_count} exceeds safety limit"),
            ));
        }

        let mut namespaces = Vec::with_capacity(namespace_count);
        for _ in 0..namespace_count {
            let name_hash = if version.is_optimized() {
                r.u32()?
            } else {
                0
            };
            let name = r.fstring()?;
            let key_count = r.i32()?;
            if key_count < 0 {
                return Err(err(
                    file_label,
                    format!("negative key count in namespace {name:?}"),
                ));
            }
            let key_count = key_count as usize;
            let mut strings = Vec::with_capacity(key_count);
            for _ in 0..key_count {
                let key_hash = if version.is_optimized() {
                    r.u32()?
                } else {
                    0
                };
                let key = r.fstring()?;
                let source_string_hash = r.u32()?;
                let value = if version as u8 >= LocresVersion::Compact as u8 {
                    let idx = r.i32()?;
                    if idx < 0 || idx as usize >= string_array.len() {
                        return Err(err(
                            file_label,
                            format!(
                                "string index {idx} out of bounds (table len {}) for key {key}",
                                string_array.len()
                            ),
                        ));
                    }
                    string_array[idx as usize].clone()
                } else {
                    r.fstring()?
                };
                strings.push(LocresString {
                    key,
                    value,
                    source_string_hash,
                    key_hash,
                });
            }
            namespaces.push(LocresNamespace {
                name,
                name_hash,
                strings,
            });
        }

        Ok(Self {
            version,
            namespaces,
        })
    }

    pub fn parse_path(path: &Path) -> Result<Self, LocresError> {
        let label = path.display().to_string();
        let data = std::fs::read(path).map_err(|e| err(&label, format!("read failed: {e}")))?;
        Self::parse(&data, &label)
    }

    /// Flatten to (namespace, key, value, source_hash) for extract.
    pub fn iter_entries(&self) -> impl Iterator<Item = (&str, &str, &str, u32)> + '_ {
        self.namespaces.iter().flat_map(|ns| {
            ns.strings.iter().map(move |s| {
                (
                    ns.name.as_str(),
                    s.key.as_str(),
                    s.value.as_str(),
                    s.source_string_hash,
                )
            })
        })
    }

    /// Apply translations: map of `"namespace/key"` → new value. Preserves hashes.
    pub fn apply_translations(&mut self, translations: &HashMap<String, String>) -> usize {
        let mut n = 0;
        for ns in &mut self.namespaces {
            for s in &mut ns.strings {
                let id = if ns.name.is_empty() {
                    s.key.clone()
                } else {
                    format!("{}/{}", ns.name, s.key)
                };
                if let Some(t) = translations.get(&id) {
                    if s.value != *t {
                        s.value = t.clone();
                        n += 1;
                    }
                }
            }
        }
        n
    }

    pub fn serialize(&self) -> Result<Vec<u8>, LocresError> {
        match self.version {
            LocresVersion::Legacy => self.serialize_legacy(),
            _ => self.serialize_modern(),
        }
    }

    fn serialize_legacy(&self) -> Result<Vec<u8>, LocresError> {
        let mut w = Writer::new();
        w.i32(self.namespaces.len() as i32);
        for ns in &self.namespaces {
            w.fstring(&ns.name);
            w.i32(ns.strings.len() as i32);
            for s in &ns.strings {
                w.fstring(&s.key);
                w.u32(s.source_string_hash);
                w.fstring(&s.value);
            }
        }
        Ok(w.into_vec())
    }

    fn serialize_modern(&self) -> Result<Vec<u8>, LocresError> {
        let mut w = Writer::new();
        w.buf.extend_from_slice(&LOCRES_MAGIC);
        w.u8(self.version as u8);

        let array_offset_pos = w.pos();
        w.i64(0); // placeholder

        let entries_count_pos = if self.version.is_optimized() {
            let p = w.pos();
            w.i32(0);
            Some(p)
        } else {
            None
        };

        w.i32(self.namespaces.len() as i32);

        // Build string table (first-seen order) + write namespaces.
        let mut string_table: Vec<String> = Vec::new();
        let mut string_index: HashMap<String, i32> = HashMap::new();
        let mut ref_counts: Vec<i32> = Vec::new();
        let mut entry_count = 0i32;

        for ns in &self.namespaces {
            if self.version.is_optimized() {
                w.u32(ns.name_hash);
            }
            w.fstring(&ns.name);
            w.i32(ns.strings.len() as i32);
            for s in &ns.strings {
                if self.version.is_optimized() {
                    w.u32(s.key_hash);
                }
                w.fstring(&s.key);
                w.u32(s.source_string_hash);
                let idx = if let Some(&i) = string_index.get(&s.value) {
                    ref_counts[i as usize] += 1;
                    i
                } else {
                    let i = string_table.len() as i32;
                    string_table.push(s.value.clone());
                    ref_counts.push(1);
                    string_index.insert(s.value.clone(), i);
                    i
                };
                w.i32(idx);
                entry_count += 1;
            }
        }

        let string_table_offset = w.pos() as i64;
        w.i32(string_table.len() as i32);
        for (i, text) in string_table.iter().enumerate() {
            w.fstring(text);
            if self.version.is_optimized() {
                w.i32(ref_counts[i]);
            }
        }

        w.patch_i64(array_offset_pos, string_table_offset);
        if let Some(p) = entries_count_pos {
            w.patch_i32(p, entry_count);
        }
        Ok(w.into_vec())
    }

    /// Semantic equality (order-sensitive namespaces/keys; ignores raw padding).
    pub fn semantic_eq(&self, other: &Self) -> bool {
        self.version == other.version && self.namespaces == other.namespaces
    }
}

/// True if `data` begins with the LocRes magic (Compact+).
pub fn looks_like_locres(data: &[u8]) -> bool {
    data.len() >= 17 && data[..16] == LOCRES_MAGIC
}

/// Find absolute offsets of LocRes magic blobs inside a larger buffer (e.g. pak).
pub fn find_locres_offsets(data: &[u8]) -> Vec<usize> {
    let mut out = Vec::new();
    if data.len() < 17 {
        return out;
    }
    let mut i = 0;
    while i + 16 <= data.len() {
        if data[i..i + 16] == LOCRES_MAGIC {
            out.push(i);
            i += 16;
        } else {
            i += 1;
        }
    }
    out
}

/// Try to parse a locres starting at `offset` inside `data`. Uses a heuristic
/// end bound: next magic or EOF. Returns (file, end_offset exclusive) on success.
pub fn parse_embedded(
    data: &[u8],
    offset: usize,
    file_label: &str,
) -> Result<(LocresFile, usize), LocresError> {
    if offset >= data.len() {
        return Err(err(file_label, "embedded offset past EOF"));
    }
    // Parse from offset to EOF; LocRes parser stops after the last namespace
    // without needing an exact length. We do not know the exact blob end, so
    // pass the remainder — parse only consumes structured fields.
    let slice = &data[offset..];
    let file = LocresFile::parse(slice, file_label)?;
    // Approximate end: cannot know precisely without full layout walk; callers
    // use offset solely for de-dupe of heuristic hits via string identity.
    Ok((file, offset.saturating_add(slice.len())))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_file(version: LocresVersion) -> LocresFile {
        // Name/key hashes are only persisted by Optimized+ — Compact fixtures
        // must carry 0 there, matching what parse() reconstructs.
        let h = |s: &str| if version.is_optimized() { str_crc32_ue(s) } else { 0 };
        LocresFile {
            version,
            namespaces: vec![
                LocresNamespace {
                    name: "UI".into(),
                    name_hash: h("UI"),
                    strings: vec![
                        LocresString {
                            key: "Hello".into(),
                            value: "Hello World".into(),
                            source_string_hash: str_crc32_ue("Hello World"),
                            key_hash: h("Hello"),
                        },
                        LocresString {
                            key: "Bye".into(),
                            value: "Goodbye".into(),
                            source_string_hash: str_crc32_ue("Goodbye"),
                            key_hash: h("Bye"),
                        },
                    ],
                },
                LocresNamespace {
                    name: String::new(), // default namespace
                    name_hash: 0,
                    strings: vec![LocresString {
                        key: "Title".into(),
                        value: "ゲーム".into(), // forces UTF-16 FString
                        source_string_hash: str_crc32_ue("ゲーム"),
                        key_hash: h("Title"),
                    }],
                },
            ],
        }
    }

    #[test]
    fn roundtrip_compact_semantic_and_bytes() {
        let f = sample_file(LocresVersion::Compact);
        let bytes = f.serialize().unwrap();
        assert!(looks_like_locres(&bytes));
        let parsed = LocresFile::parse(&bytes, "test.locres").unwrap();
        assert!(f.semantic_eq(&parsed));
        let again = parsed.serialize().unwrap();
        assert_eq!(bytes, again, "self-serialized Compact should be byte-stable");
    }

    #[test]
    fn roundtrip_optimized_semantic_and_bytes() {
        let f = sample_file(LocresVersion::Optimized);
        let bytes = f.serialize().unwrap();
        let parsed = LocresFile::parse(&bytes, "opt.locres").unwrap();
        assert!(f.semantic_eq(&parsed));
        assert_eq!(bytes, parsed.serialize().unwrap());
    }

    #[test]
    fn roundtrip_optimized_cityhash_preserves_hashes() {
        // We preserve CityHash key/ns hashes from the in-memory structure.
        let mut f = sample_file(LocresVersion::OptimizedCityHash64Utf16);
        f.namespaces[0].name_hash = 0xDEAD_BEEF;
        f.namespaces[0].strings[0].key_hash = 0xCAFE_BABE;
        let bytes = f.serialize().unwrap();
        let parsed = LocresFile::parse(&bytes, "v3.locres").unwrap();
        assert_eq!(parsed.namespaces[0].name_hash, 0xDEAD_BEEF);
        assert_eq!(parsed.namespaces[0].strings[0].key_hash, 0xCAFE_BABE);
        assert!(f.semantic_eq(&parsed));
    }

    #[test]
    fn roundtrip_legacy() {
        let f = sample_file(LocresVersion::Legacy);
        // Zero out optimized-only hashes for semantic clarity
        let mut f = f;
        for ns in &mut f.namespaces {
            ns.name_hash = 0;
            for s in &mut ns.strings {
                s.key_hash = 0;
            }
        }
        let bytes = f.serialize().unwrap();
        assert!(!looks_like_locres(&bytes));
        let parsed = LocresFile::parse(&bytes, "legacy.locres").unwrap();
        assert_eq!(parsed.version, LocresVersion::Legacy);
        assert!(f.semantic_eq(&parsed));
    }

    #[test]
    fn utf16_and_ascii_strings() {
        let f = sample_file(LocresVersion::Compact);
        let bytes = f.serialize().unwrap();
        let p = LocresFile::parse(&bytes, "t.locres").unwrap();
        let title = p
            .namespaces
            .iter()
            .find(|n| n.name.is_empty())
            .unwrap()
            .strings
            .iter()
            .find(|s| s.key == "Title")
            .unwrap();
        assert_eq!(title.value, "ゲーム");
        let hello = &p.namespaces[0].strings[0];
        assert_eq!(hello.value, "Hello World");
    }

    #[test]
    fn apply_translations_preserves_source_hash() {
        let mut f = sample_file(LocresVersion::Optimized);
        let orig_hash = f.namespaces[0].strings[0].source_string_hash;
        let mut map = HashMap::new();
        map.insert("UI/Hello".into(), "Hola Mundo".into());
        assert_eq!(f.apply_translations(&map), 1);
        assert_eq!(f.namespaces[0].strings[0].value, "Hola Mundo");
        assert_eq!(f.namespaces[0].strings[0].source_string_hash, orig_hash);
        let bytes = f.serialize().unwrap();
        let p = LocresFile::parse(&bytes, "t.locres").unwrap();
        assert_eq!(p.namespaces[0].strings[0].value, "Hola Mundo");
        assert_eq!(p.namespaces[0].strings[0].source_string_hash, orig_hash);
    }

    #[test]
    fn bad_magic_version_errors() {
        let mut data = LOCRES_MAGIC.to_vec();
        data.push(99); // bad version
        data.extend_from_slice(&0i64.to_le_bytes());
        let e = LocresFile::parse(&data, "bad.locres").unwrap_err();
        assert!(e.to_string().contains("bad.locres"));
        assert!(e.message.contains("version"));
    }

    #[test]
    fn truncated_errors() {
        let e = LocresFile::parse(&LOCRES_MAGIC, "trunc.locres").unwrap_err();
        assert!(e.to_string().contains("trunc.locres"));
    }

    #[test]
    fn string_index_oob_errors() {
        // Craft a minimal corrupt structure with an out-of-bounds string index.
        // Magic + ver + offset placeholder + ns count 1 + ns name + 1 key
        let mut w = Writer::new();
        w.buf.extend_from_slice(&LOCRES_MAGIC);
        w.u8(1); // Compact
        let off_pos = w.pos();
        w.i64(0);
        w.i32(1); // 1 namespace
        w.fstring("NS");
        w.i32(1); // 1 key
        w.fstring("K");
        w.u32(0);
        w.i32(5); // OOB index
        let table_off = w.pos() as i64;
        w.i32(1); // 1 string in table
        w.fstring("only");
        w.patch_i64(off_pos, table_off);
        let e = LocresFile::parse(&w.into_vec(), "oob.locres").unwrap_err();
        assert!(e.message.contains("out of bounds"), "{}", e.message);
    }

    #[test]
    fn find_locres_offsets_in_buffer() {
        let f = sample_file(LocresVersion::Compact);
        let loc = f.serialize().unwrap();
        let mut buf = vec![0u8; 32];
        buf.extend_from_slice(&loc);
        buf.extend_from_slice(&[1, 2, 3]);
        let offs = find_locres_offsets(&buf);
        assert_eq!(offs, vec![32]);
    }
}
