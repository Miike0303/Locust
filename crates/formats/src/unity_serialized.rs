//! Unity SerializedFile container — slice 1: header, type table (no type trees),
//! object table, and TextAsset (class_id 49) `m_Name` / `m_Script` reads.
//!
//! # Format (AssetStudio / AssetsTools.NET conventions)
//! Header fields through `data_offset` are **big-endian**. From version ≥ 9 the
//! endianness byte selects the endianness of metadata + object data (usually
//! little). Version ≥ 22 (LargeFilesSupport) extends the header with u64
//! file_size / data_offset.
//!
//! Metadata (file endian): unity version c-string, target platform u32,
//! enable_type_tree bool, type count, then per-type class_id and script hashes
//! **without** type-tree blobs when `enable_type_tree` is false. Type trees are
//! out of scope for slice 1 — files with type trees return a clear error so
//! callers can fall back to heuristics.
//!
//! Object table (v≥16): count i32; each object 4-aligned: path_id i64,
//! byte_start u32 (u64 when v≥22), byte_size u32, type_id i32 (index into types).
//!
//! TextAsset object body: aligned string `m_Name`, aligned string `m_Script`
//! (u32 length + bytes + pad to 4).

use std::path::Path;

/// Unity class ID for TextAsset.
pub const CLASS_ID_TEXT_ASSET: i32 = 49;

/// SerializedFile format versions we fully support for slice 1.
pub const MIN_SUPPORTED_VERSION: u32 = 17;
pub const MAX_SUPPORTED_VERSION: u32 = 22;

#[derive(Debug)]
pub struct SerializedError {
    pub file: String,
    pub message: String,
}

impl std::fmt::Display for SerializedError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.file, self.message)
    }
}

impl std::error::Error for SerializedError {}

fn err(file: &str, message: impl Into<String>) -> SerializedError {
    SerializedError {
        file: file.into(),
        message: message.into(),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Endian {
    Little,
    Big,
}

#[derive(Debug, Clone)]
pub struct SerializedHeader {
    pub version: u32,
    pub metadata_size: u32,
    pub file_size: u64,
    pub data_offset: u64,
    pub endian: Endian,
}

#[derive(Debug, Clone)]
pub struct SerializedType {
    pub class_id: i32,
    pub is_stripped: bool,
    pub script_type_index: i16,
}

#[derive(Debug, Clone)]
pub struct ObjectInfo {
    pub path_id: i64,
    pub class_id: i32,
    /// Absolute file offset of object data (`data_offset + byte_start`).
    pub data_abs: u64,
    pub byte_size: u32,
    pub type_index: i32,
}

#[derive(Debug, Clone)]
pub struct TextAssetData {
    pub path_id: i64,
    pub name: String,
    pub script: String,
    /// Absolute file offset of the `m_Script` length prefix (u32).
    pub script_len_offset: usize,
    /// Original `m_Script` string byte length (not including length prefix / align).
    pub script_byte_len: usize,
}

#[derive(Debug)]
pub struct SerializedFile {
    pub path: std::path::PathBuf,
    pub header: SerializedHeader,
    pub unity_version: String,
    pub types: Vec<SerializedType>,
    pub objects: Vec<ObjectInfo>,
    /// Full file bytes (owned for inject / text-asset reads).
    pub data: Vec<u8>,
}

struct R<'a> {
    data: &'a [u8],
    pos: usize,
    file: &'a str,
    endian: Endian,
}

impl<'a> R<'a> {
    fn need(&self, n: usize) -> Result<(), SerializedError> {
        if self.pos.saturating_add(n) > self.data.len() {
            Err(err(
                self.file,
                format!("truncated at offset {} (need {n} bytes)", self.pos),
            ))
        } else {
            Ok(())
        }
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8], SerializedError> {
        self.need(n)?;
        let s = &self.data[self.pos..self.pos + n];
        self.pos += n;
        Ok(s)
    }

    fn u8(&mut self) -> Result<u8, SerializedError> {
        Ok(self.take(1)?[0])
    }

    fn u16(&mut self) -> Result<u16, SerializedError> {
        let b = self.take(2)?;
        Ok(match self.endian {
            Endian::Little => u16::from_le_bytes([b[0], b[1]]),
            Endian::Big => u16::from_be_bytes([b[0], b[1]]),
        })
    }

    fn u32(&mut self) -> Result<u32, SerializedError> {
        let b = self.take(4)?;
        Ok(match self.endian {
            Endian::Little => u32::from_le_bytes([b[0], b[1], b[2], b[3]]),
            Endian::Big => u32::from_be_bytes([b[0], b[1], b[2], b[3]]),
        })
    }

    fn i32(&mut self) -> Result<i32, SerializedError> {
        Ok(self.u32()? as i32)
    }

    fn i16(&mut self) -> Result<i16, SerializedError> {
        Ok(self.u16()? as i16)
    }

    fn u64(&mut self) -> Result<u64, SerializedError> {
        let b = self.take(8)?;
        Ok(match self.endian {
            Endian::Little => u64::from_le_bytes([
                b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7],
            ]),
            Endian::Big => u64::from_be_bytes([
                b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7],
            ]),
        })
    }

    fn i64(&mut self) -> Result<i64, SerializedError> {
        Ok(self.u64()? as i64)
    }

    fn align4(&mut self) {
        let rem = self.pos % 4;
        if rem != 0 {
            self.pos += 4 - rem;
        }
    }

    fn cstring(&mut self) -> Result<String, SerializedError> {
        let start = self.pos;
        while self.pos < self.data.len() && self.data[self.pos] != 0 {
            self.pos += 1;
        }
        if self.pos >= self.data.len() {
            return Err(err(self.file, "unterminated c-string in metadata"));
        }
        let s = std::str::from_utf8(&self.data[start..self.pos])
            .map_err(|_| err(self.file, "unity version string is not UTF-8"))?
            .to_string();
        self.pos += 1; // NUL
        Ok(s)
    }

    /// Unity Align(4) string: u32 length + bytes + pad to 4.
    fn aligned_string(&mut self) -> Result<(String, usize, usize), SerializedError> {
        let len_off = self.pos;
        let len = self.u32()? as usize;
        if len > 64 * 1024 * 1024 {
            return Err(err(
                self.file,
                format!("aligned string length {len} exceeds safety limit"),
            ));
        }
        let bytes = self.take(len)?;
        let text = String::from_utf8_lossy(bytes).into_owned();
        self.align4();
        Ok((text, len_off, len))
    }
}

impl SerializedFile {
    pub fn parse(data: Vec<u8>, path: impl Into<std::path::PathBuf>) -> Result<Self, SerializedError> {
        let path = path.into();
        let label = path.display().to_string();
        if data.len() < 20 {
            return Err(err(&label, "file too small for SerializedFile header"));
        }

        // Header is big-endian until endianness is known.
        let mut hr = R {
            data: &data,
            pos: 0,
            file: &label,
            endian: Endian::Big,
        };
        let mut metadata_size = hr.u32()?;
        let mut file_size = hr.u32()? as u64;
        let version = hr.u32()?;
        let mut data_offset = hr.u32()? as u64;

        if !(MIN_SUPPORTED_VERSION..=MAX_SUPPORTED_VERSION).contains(&version) {
            return Err(err(
                &label,
                format!(
                    "unsupported SerializedFile version {version} \
                     (slice 1 supports {MIN_SUPPORTED_VERSION}–{MAX_SUPPORTED_VERSION})"
                ),
            ));
        }

        let endian_byte = hr.u8()?;
        let _reserved = hr.take(3)?;
        let endian = if endian_byte == 0 {
            Endian::Little
        } else {
            Endian::Big
        };

        if version >= 22 {
            // LargeFilesSupport: re-read sizes as wider fields (still big-endian header).
            metadata_size = hr.u32()?;
            file_size = hr.u64()?;
            data_offset = hr.u64()?;
            let _unknown = hr.u64()?;
        }

        let header = SerializedHeader {
            version,
            metadata_size,
            file_size,
            data_offset,
            endian,
        };

        // Metadata uses file endian.
        let mut r = R {
            data: &data,
            pos: hr.pos,
            file: &label,
            endian,
        };

        let unity_version = r.cstring()?;
        let _target_platform = r.u32()?;
        let enable_type_tree = r.u8()? != 0;
        if enable_type_tree {
            return Err(err(
                &label,
                "SerializedFile has type trees enabled — not supported in TextAsset slice 1",
            ));
        }

        let type_count = r.i32()?;
        if !(0..=100_000).contains(&type_count) {
            return Err(err(
                &label,
                format!("implausible type count {type_count}"),
            ));
        }
        let mut types = Vec::with_capacity(type_count as usize);
        for _ in 0..type_count {
            let class_id = r.i32()?;
            // v >= 16
            let is_stripped = r.u8()? != 0;
            // v >= 17
            let script_type_index = r.i16()?;
            // script_id[16] for MonoBehaviour / negative class
            if class_id == 114 || class_id < 0 {
                let _script_id = r.take(16)?;
            }
            let _old_type_hash = r.take(16)?;
            // no type tree
            types.push(SerializedType {
                class_id,
                is_stripped,
                script_type_index,
            });
        }

        // Object table (v >= 14 uses i64 path_id; v >= 16 type_id is type index)
        let object_count = r.i32()?;
        if !(0..=5_000_000).contains(&object_count) {
            return Err(err(
                &label,
                format!("implausible object count {object_count}"),
            ));
        }
        let mut objects = Vec::with_capacity(object_count as usize);
        for _ in 0..object_count {
            r.align4();
            let path_id = r.i64()?;
            let byte_start = if version >= 22 {
                r.u64()?
            } else {
                r.u32()? as u64
            };
            let byte_size = r.u32()?;
            let type_id = r.i32()?;
            if type_id < 0 || type_id as usize >= types.len() {
                return Err(err(
                    &label,
                    format!("object type_id {type_id} out of bounds ({} types)", types.len()),
                ));
            }
            let class_id = types[type_id as usize].class_id;
            let data_abs = data_offset.saturating_add(byte_start);
            let end = data_abs.saturating_add(byte_size as u64);
            if end > data.len() as u64 {
                return Err(err(
                    &label,
                    format!(
                        "object path_id={path_id} data range [{data_abs}, {end}) past EOF ({})",
                        data.len()
                    ),
                ));
            }
            objects.push(ObjectInfo {
                path_id,
                class_id,
                data_abs,
                byte_size,
                type_index: type_id,
            });
        }

        Ok(Self {
            path,
            header,
            unity_version,
            types,
            objects,
            data,
        })
    }

    pub fn parse_path(path: &Path) -> Result<Self, SerializedError> {
        let label = path.display().to_string();
        let data = std::fs::read(path).map_err(|e| err(&label, format!("read failed: {e}")))?;
        Self::parse(data, path)
    }

    pub fn text_asset_objects(&self) -> impl Iterator<Item = &ObjectInfo> {
        self.objects
            .iter()
            .filter(|o| o.class_id == CLASS_ID_TEXT_ASSET)
    }

    /// Read TextAsset `m_Name` + `m_Script` at `path_id`.
    pub fn read_text_asset(&self, path_id: i64) -> Result<TextAssetData, SerializedError> {
        let label = self.path.display().to_string();
        let obj = self
            .objects
            .iter()
            .find(|o| o.path_id == path_id && o.class_id == CLASS_ID_TEXT_ASSET)
            .ok_or_else(|| err(&label, format!("no TextAsset with path_id={path_id}")))?;

        let start = obj.data_abs as usize;
        let end = start + obj.byte_size as usize;
        let mut r = R {
            data: &self.data[..end.min(self.data.len())],
            pos: start,
            file: &label,
            endian: self.header.endian,
        };
        let (name, _, _) = r.aligned_string()?;
        let (script, script_len_offset, script_byte_len) = r.aligned_string()?;
        Ok(TextAssetData {
            path_id,
            name,
            script,
            script_len_offset,
            script_byte_len,
        })
    }

    /// Absolute byte ranges of all TextAsset objects (for heuristic skip).
    pub fn text_asset_byte_ranges(&self) -> Vec<(usize, usize)> {
        self.objects
            .iter()
            .filter(|o| o.class_id == CLASS_ID_TEXT_ASSET)
            .map(|o| {
                let s = o.data_abs as usize;
                (s, s + o.byte_size as usize)
            })
            .collect()
    }
}

/// True if `script` looks like binary (high non-text ratio).
pub fn is_binary_looking_script(script: &str) -> bool {
    if script.is_empty() {
        return true;
    }
    let bytes = script.as_bytes();
    let non_text = bytes
        .iter()
        .filter(|&&b| {
            // Allow tab/lf/cr and printable ASCII; count other bytes as non-text.
            !(b == b'\t' || b == b'\n' || b == b'\r' || (0x20..=0x7E).contains(&b) || b >= 0x80)
        })
        .count();
    // Also treat high NUL density as binary.
    let nuls = bytes.iter().filter(|&&b| b == 0).count();
    if nuls * 5 > bytes.len() {
        return true;
    }
    (non_text as f64) / (bytes.len() as f64) > 0.20
}

/// In-place rewrite of `m_Script` when `new_script` UTF-8 length ≤ original.
/// Pads with `0x20` to keep the original string field size; object table unchanged.
pub fn rewrite_text_asset_script_inplace(
    file_bytes: &mut [u8],
    script_len_offset: usize,
    orig_script_byte_len: usize,
    new_script: &str,
    file_label: &str,
) -> Result<(), SerializedError> {
    let new_bytes = new_script.as_bytes();
    if new_bytes.len() > orig_script_byte_len {
        return Err(err(
            file_label,
            format!(
                "TextAsset script longer than original ({} > {})",
                new_bytes.len(),
                orig_script_byte_len
            ),
        ));
    }
    let need = script_len_offset
        .checked_add(4)
        .and_then(|p| p.checked_add(orig_script_byte_len))
        .ok_or_else(|| err(file_label, "script field offset overflow"))?;
    if need > file_bytes.len() {
        return Err(err(file_label, "script field past EOF"));
    }
    // Length prefix stays as **original** length so the field size (and align)
    // is unchanged; pad shorter text with spaces (Unity reads the full buffer).
    // User asked pad to original m_Script length with 0x20 — keep len = orig.
    file_bytes[script_len_offset..script_len_offset + 4]
        .copy_from_slice(&(orig_script_byte_len as u32).to_le_bytes());
    let payload = &mut file_bytes[script_len_offset + 4..script_len_offset + 4 + orig_script_byte_len];
    payload[..new_bytes.len()].copy_from_slice(new_bytes);
    for b in &mut payload[new_bytes.len()..] {
        *b = b' ';
    }
    Ok(())
}

// ─── Test / fixture writer (v17, little-endian, no type trees) ─────────────

/// Build a minimal v17 SerializedFile with one TextAsset and one dummy object.
#[cfg(test)]
pub fn write_v17_fixture(text_name: &str, text_script: &str) -> Vec<u8> {
    write_v17_fixture_ex(text_name, text_script, Some(("Dummy", "not a text asset body")))
}

#[cfg(test)]
pub fn write_v17_fixture_ex(
    text_name: &str,
    text_script: &str,
    extra_gameobject_like: Option<(&str, &str)>,
) -> Vec<u8> {
    fn align4(n: usize) -> usize {
        (n + 3) & !3
    }
    fn write_aligned_string(buf: &mut Vec<u8>, s: &str) {
        let b = s.as_bytes();
        buf.extend_from_slice(&(b.len() as u32).to_le_bytes());
        buf.extend_from_slice(b);
        let pad = align4(b.len()) - b.len();
        buf.extend(std::iter::repeat_n(0u8, pad));
    }
    fn write_be_u32(buf: &mut Vec<u8>, v: u32) {
        buf.extend_from_slice(&v.to_be_bytes());
    }

    // --- object payloads (little-endian) ---
    let mut text_payload = Vec::new();
    write_aligned_string(&mut text_payload, text_name);
    write_aligned_string(&mut text_payload, text_script);

    let mut other_payload = Vec::new();
    if let Some((n, s)) = extra_gameobject_like {
        // Fake "aligned strings" so we have a non-TextAsset blob of nonzero size.
        write_aligned_string(&mut other_payload, n);
        write_aligned_string(&mut other_payload, s);
    } else {
        other_payload.extend_from_slice(&[0u8; 16]);
    }

    // --- metadata (little-endian) ---
    let mut meta = Vec::new();
    // unity version cstr
    meta.extend_from_slice(b"2019.4.0f1\0");
    meta.extend_from_slice(&1u32.to_le_bytes()); // target platform
    meta.push(0); // enable_type_tree = false

    // 2 types: TextAsset (49), GameObject (1)
    meta.extend_from_slice(&2i32.to_le_bytes());
    // type 0: TextAsset
    meta.extend_from_slice(&CLASS_ID_TEXT_ASSET.to_le_bytes());
    meta.push(0); // stripped
    meta.extend_from_slice(&(-1i16).to_le_bytes()); // script_type_index
    meta.extend_from_slice(&[0u8; 16]); // old_type_hash
    // type 1: GameObject class 1
    meta.extend_from_slice(&1i32.to_le_bytes());
    meta.push(0);
    meta.extend_from_slice(&(-1i16).to_le_bytes());
    meta.extend_from_slice(&[0u8; 16]);

    // objects: 2
    meta.extend_from_slice(&2i32.to_le_bytes());
    // obj0 TextAsset path_id=1
    while meta.len() % 4 != 0 {
        meta.push(0);
    }
    let text_byte_start = 0u32;
    let text_byte_size = text_payload.len() as u32;
    meta.extend_from_slice(&1i64.to_le_bytes()); // path_id
    meta.extend_from_slice(&text_byte_start.to_le_bytes());
    meta.extend_from_slice(&text_byte_size.to_le_bytes());
    meta.extend_from_slice(&0i32.to_le_bytes()); // type index 0

    // obj1 other path_id=2
    while meta.len() % 4 != 0 {
        meta.push(0);
    }
    let other_byte_start = text_byte_size; // packed sequentially
    let other_byte_size = other_payload.len() as u32;
    meta.extend_from_slice(&2i64.to_le_bytes());
    meta.extend_from_slice(&other_byte_start.to_le_bytes());
    meta.extend_from_slice(&other_byte_size.to_le_bytes());
    meta.extend_from_slice(&1i32.to_le_bytes()); // type index 1

    // Header (big-endian) + metadata + data
    // data_offset aligned to 16 for cleanliness
    let header_len = 20usize; // v17: metadataSize,fileSize,version,dataOffset,endian+reserved
    let mut data_offset = header_len + meta.len();
    data_offset = (data_offset + 15) & !15;

    let file_size = data_offset + text_payload.len() + other_payload.len();
    let metadata_size = meta.len() as u32;

    let mut out = Vec::new();
    write_be_u32(&mut out, metadata_size);
    write_be_u32(&mut out, file_size as u32);
    write_be_u32(&mut out, 17); // version
    write_be_u32(&mut out, data_offset as u32);
    out.push(0); // little endian
    out.extend_from_slice(&[0, 0, 0]);

    out.extend_from_slice(&meta);
    while out.len() < data_offset {
        out.push(0);
    }
    out.extend_from_slice(&text_payload);
    out.extend_from_slice(&other_payload);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_v17_finds_text_asset() {
        let bytes = write_v17_fixture("HelloName", "Hello script body");
        let sf = SerializedFile::parse(bytes, "test.assets").unwrap();
        assert_eq!(sf.header.version, 17);
        assert_eq!(sf.objects.len(), 2);
        let texts: Vec<_> = sf.text_asset_objects().collect();
        assert_eq!(texts.len(), 1);
        assert_eq!(texts[0].path_id, 1);
        assert_eq!(texts[0].class_id, CLASS_ID_TEXT_ASSET);

        let ta = sf.read_text_asset(1).unwrap();
        assert_eq!(ta.name, "HelloName");
        assert_eq!(ta.script, "Hello script body");
        assert!(ta.script_byte_len == "Hello script body".len());
    }

    #[test]
    fn rewrite_shorter_pads_with_spaces() {
        let bytes = write_v17_fixture("N", "ABCDEFGH"); // 8 bytes
        let sf = SerializedFile::parse(bytes.clone(), "t.assets").unwrap();
        let ta = sf.read_text_asset(1).unwrap();
        let mut file = bytes;
        rewrite_text_asset_script_inplace(
            &mut file,
            ta.script_len_offset,
            ta.script_byte_len,
            "Hi",
            "t.assets",
        )
        .unwrap();
        let again = SerializedFile::parse(file, "t.assets").unwrap();
        let ta2 = again.read_text_asset(1).unwrap();
        // Length field kept at original size → script includes space padding
        assert!(ta2.script.starts_with("Hi"));
        assert_eq!(ta2.script.len(), 8);
        assert!(ta2.script.ends_with(' '));
    }

    #[test]
    fn rewrite_oversize_errors() {
        let bytes = write_v17_fixture("N", "AB");
        let sf = SerializedFile::parse(bytes.clone(), "t.assets").unwrap();
        let ta = sf.read_text_asset(1).unwrap();
        let mut file = bytes;
        let e = rewrite_text_asset_script_inplace(
            &mut file,
            ta.script_len_offset,
            ta.script_byte_len,
            "TOO LONG",
            "t.assets",
        )
        .unwrap_err();
        assert!(e.message.contains("longer"));
    }

    #[test]
    fn truncated_header_errors() {
        let e = SerializedFile::parse(vec![0u8; 10], "x.assets").unwrap_err();
        assert!(e.to_string().contains("x.assets"));
    }

    #[test]
    fn unsupported_version_errors() {
        let mut bytes = write_v17_fixture("N", "S");
        // Patch version field (BE u32 at offset 8) to 9
        bytes[8..12].copy_from_slice(&9u32.to_be_bytes());
        let e = SerializedFile::parse(bytes, "old.assets").unwrap_err();
        assert!(e.message.contains("version 9"), "{}", e.message);
    }

    #[test]
    fn binary_looking_script_detection() {
        assert!(is_binary_looking_script("\0\0\0\0\0\0\0\0"));
        assert!(!is_binary_looking_script("Hello, world!\nLine two."));
    }

    #[test]
    fn oob_object_detected() {
        let mut bytes = write_v17_fixture("N", "S");
        // Corrupt: set file tiny — re-parse should fail object range if we shrink data
        bytes.truncate(40);
        let e = SerializedFile::parse(bytes, "oob.assets");
        assert!(e.is_err());
    }
}
