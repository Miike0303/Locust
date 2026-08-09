//! Unity SerializedFile container — slices 1–2:
//! - **Slice 1:** header, type table, object table, TextAsset (class_id 49)
//!   `m_Name` / `m_Script` reads + in-place rewrite (payload pad with `0x20`;
//!   length-prefix u32 left byte-identical so BE assets stay valid).
//! - **Slice 2:** skip type-tree blobs (no field interpretation), MonoBehaviour
//!   (class_id 114 **or negative** script-type ids) base layout (`m_GameObject`,
//!   `m_Enabled`, `m_Script`, `m_Name`) plus sequential aligned-string fields
//!   after the base for extract/in-place rewrite. Full type-tree walks remain
//!   out of scope.
//!
//! # Format (AssetStudio / AssetsTools.NET conventions)
//! Header fields through `data_offset` are **big-endian**. From version ≥ 9 the
//! endianness byte selects the endianness of metadata + object data (usually
//! little). Version ≥ 22 (LargeFilesSupport) extends the header with u64
//! file_size / data_offset.
//!
//! Metadata (file endian): unity version c-string, target platform u32,
//! enable_type_tree bool, type count, then per-type class_id and script hashes.
//! When `enable_type_tree` is set, type-tree **blobs are skipped** (node table +
//! string buffer) so the object table is still reachable — field-level type
//! trees are not interpreted.
//!
//! Object table (v≥16): count i32; each object 4-aligned: path_id i64,
//! byte_start u32 (u64 when v≥22), byte_size u32, type_id i32 (index into types).
//!
//! TextAsset object body: aligned string `m_Name`, aligned string `m_Script`
//! (u32 length + bytes + pad to 4).
//!
//! MonoBehaviour object body (release, v≥14 path IDs): PPtr `m_GameObject`,
//! u8 `m_Enabled` + align4, PPtr `m_Script`, aligned string `m_Name`, then
//! script-defined fields (slice 2 only walks further **aligned strings**).

use std::path::Path;

/// Unity class ID for TextAsset.
pub const CLASS_ID_TEXT_ASSET: i32 = 49;
/// Unity class ID for MonoBehaviour.
pub const CLASS_ID_MONO_BEHAVIOUR: i32 = 114;

/// True when `class_id` is a MonoBehaviour type in the SerializedFile type table.
///
/// AssetsTools / AssetStudio convention: MonoBehaviour is **114**, and in some
/// older/stripped layouts a **negative** class id marks a MonoBehaviour script
/// type (script type index + script_id hash still present). Treat both as mono
/// for extract/inject so those titles are not silently skipped.
#[inline]
pub fn is_monobehaviour_class(class_id: i32) -> bool {
    class_id == CLASS_ID_MONO_BEHAVIOUR || class_id < 0
}

/// SerializedFile format versions we fully support for slices 1–2.
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

/// One string field extracted from a MonoBehaviour object.
#[derive(Debug, Clone)]
pub struct MonoStringData {
    pub path_id: i64,
    /// MonoBehaviour `m_Name` (may be empty).
    pub mono_name: String,
    /// 0 = `m_Name` itself; 1+ = sequential aligned strings after the base layout.
    pub field_index: usize,
    pub text: String,
    /// Absolute file offset of the string length prefix (u32).
    pub len_offset: usize,
    /// Original string byte length (not including length prefix / align).
    pub byte_len: usize,
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
            // script_id[16] for MonoBehaviour / negative class (script type)
            if is_monobehaviour_class(class_id) {
                let _script_id = r.take(16)?;
            }
            let _old_type_hash = r.take(16)?;
            // Slice 2: skip type-tree blobs without interpreting nodes.
            if enable_type_tree {
                skip_type_tree_blob(&mut r, version)?;
            }
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

    pub fn mono_behaviour_objects(&self) -> impl Iterator<Item = &ObjectInfo> {
        self.objects
            .iter()
            .filter(|o| is_monobehaviour_class(o.class_id))
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

    /// Read MonoBehaviour `m_Name` + sequential aligned-string fields after the
    /// fixed base layout. Stops at the first non-string-shaped field.
    pub fn read_mono_strings(&self, path_id: i64) -> Result<Vec<MonoStringData>, SerializedError> {
        let label = self.path.display().to_string();
        let obj = self
            .objects
            .iter()
            .find(|o| o.path_id == path_id && is_monobehaviour_class(o.class_id))
            .ok_or_else(|| {
                err(
                    &label,
                    format!("no MonoBehaviour with path_id={path_id}"),
                )
            })?;

        let start = obj.data_abs as usize;
        let end = (start + obj.byte_size as usize).min(self.data.len());
        let mut r = R {
            data: &self.data[..end],
            pos: start,
            file: &label,
            endian: self.header.endian,
        };

        // m_GameObject PPtr (FileID i32 + PathID i64 for v≥14 / our supported range)
        let _go_file = r.i32()?;
        let _go_path = r.i64()?;
        // m_Enabled + align
        let _enabled = r.u8()?;
        r.align4();
        // m_Script PPtr
        let _script_file = r.i32()?;
        let _script_path = r.i64()?;
        // m_Name
        let (mono_name, name_off, name_len) = r.aligned_string()?;

        let mut out = Vec::new();
        // m_Name: keep short natural labels; still drop binary / FFFD.
        if mono_name_worth_extracting(&mono_name) {
            out.push(MonoStringData {
                path_id,
                mono_name: mono_name.clone(),
                field_index: 0,
                text: mono_name.clone(),
                len_offset: name_off,
                byte_len: name_len,
            });
        }

        // Sequential aligned strings for simple script layouts (no type tree).
        let mut field_index = 1usize;
        while r.pos + 4 <= end {
            // Bound the length read so a non-string int does not walk off the object.
            let len_peek = {
                let b = match r.need(4) {
                    Ok(()) => &r.data[r.pos..r.pos + 4],
                    Err(_) => break,
                };
                match r.endian {
                    Endian::Little => u32::from_le_bytes([b[0], b[1], b[2], b[3]]),
                    Endian::Big => u32::from_be_bytes([b[0], b[1], b[2], b[3]]),
                }
            };
            let remaining = end - r.pos - 4;
            if len_peek as usize > remaining || len_peek > 4 * 1024 * 1024 {
                break;
            }
            match r.aligned_string() {
                Ok((text, len_offset, byte_len)) => {
                    if r.pos > end {
                        // Overran — discard
                        break;
                    }
                    if mono_script_field_worth_extracting(&text) {
                        out.push(MonoStringData {
                            path_id,
                            mono_name: mono_name.clone(),
                            field_index,
                            text,
                            len_offset,
                            byte_len,
                        });
                    }
                    // Always advance index for any string-shaped field we consumed.
                    field_index += 1;
                }
                Err(_) => {
                    break;
                }
            }
        }

        Ok(out)
    }

    /// Absolute byte ranges of TextAsset + MonoBehaviour objects (heuristic skip).
    pub fn text_asset_byte_ranges(&self) -> Vec<(usize, usize)> {
        self.structural_object_byte_ranges()
    }

    /// Absolute byte ranges of objects handled structurally (TextAsset + MonoBehaviour).
    pub fn structural_object_byte_ranges(&self) -> Vec<(usize, usize)> {
        self.objects
            .iter()
            .filter(|o| {
                o.class_id == CLASS_ID_TEXT_ASSET || is_monobehaviour_class(o.class_id)
            })
            .map(|o| {
                let s = o.data_abs as usize;
                (s, s + o.byte_size as usize)
            })
            .collect()
    }
}

/// Skip a SerializedFile type-tree blob (AssetStudio `TypeTreeBlobRead`).
/// Supported format versions are ≥17, which always use the blob layout.
fn skip_type_tree_blob(r: &mut R<'_>, version: u32) -> Result<(), SerializedError> {
    let node_count = r.i32()?;
    let string_buffer_size = r.i32()?;
    if !(0..=1_000_000).contains(&node_count) {
        return Err(err(
            r.file,
            format!("implausible type-tree node count {node_count}"),
        ));
    }
    if !(0..=50_000_000).contains(&string_buffer_size) {
        return Err(err(
            r.file,
            format!("implausible type-tree string buffer size {string_buffer_size}"),
        ));
    }
    // Each node: 24 bytes before format 19, 32 bytes with RefTypeHash at ≥19.
    let node_size: usize = if version >= 19 { 32 } else { 24 };
    let nodes_bytes = (node_count as usize).saturating_mul(node_size);
    r.take(nodes_bytes)?;
    r.take(string_buffer_size as usize)?;
    Ok(())
}

fn mono_name_worth_extracting(s: &str) -> bool {
    let t = s.trim();
    if t.len() < 2 {
        return false;
    }
    if t.contains('\u{FFFD}') || is_binary_looking_script(s) {
        return false;
    }
    if t.chars()
        .any(|c| c.is_control() && c != '\n' && c != '\r' && c != '\t')
    {
        return false;
    }
    t.chars().any(|c| c.is_alphabetic())
}

/// Script-field filter (field_index ≥ 1). Sequential walks mis-read ints as lengths
/// on complex MonoBehaviours — be strict so BOXMAN-class noise stays out.
fn mono_script_field_worth_extracting(s: &str) -> bool {
    if !mono_name_worth_extracting(s) {
        return false;
    }
    let t = s.trim();
    // Pure numeric / version-like.
    if t.chars()
        .all(|c| c.is_ascii_digit() || c == '-' || c == '.' || c == '+')
    {
        return false;
    }
    // `_CONST` / `ALL_CAPS_SNAKE` engine tokens.
    if t.starts_with('_') {
        return false;
    }
    if t.len() >= 3
        && t.chars()
            .all(|c| c.is_ascii_uppercase() || c == '_' || c.is_ascii_digit())
    {
        return false;
    }
    // Single-token PascalCase / camelCase identifiers (SpritesDefault, Live2DSceneHolder).
    // Keep multi-word / multi-line / script-ish text (`@showUI …`, `Portable Speaker`).
    let has_word_break = t.chars().any(|c| c.is_whitespace() || c == '@');
    if !has_word_break && looks_like_code_identifier(t) {
        return false;
    }
    true
}

fn looks_like_code_identifier(t: &str) -> bool {
    if t.is_empty() || !t.chars().next().unwrap().is_ascii_alphabetic() {
        return false;
    }
    if !t.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return false;
    }
    let chars: Vec<char> = t.chars().collect();
    // PascalCase / camelCase / name2 style — not plain words like "Hola" / "Save".
    let has_inner_upper = chars.len() >= 2 && chars[1..].iter().any(|c| c.is_ascii_uppercase());
    let has_digit = chars.iter().any(|c| c.is_ascii_digit());
    has_inner_upper || has_digit
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

/// In-place rewrite of an aligned string payload when `new_script` UTF-8 length
/// ≤ original. Pads with `0x20` to keep the original field size; object table
/// unchanged.
///
/// The **length prefix u32 is left byte-identical** (not re-encoded). That keeps
/// big-endian SerializedFiles valid — rewriting the prefix as little-endian
/// would byte-swap the stored length on BE assets. LE games keep the same
/// numeric length either way.
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
    // Do not rewrite the length prefix — leave endianness and value as on disk.
    // Field size stays fixed; pad shorter text with 0x20 (Unity reads the full buffer).
    let payload =
        &mut file_bytes[script_len_offset + 4..script_len_offset + 4 + orig_script_byte_len];
    payload[..new_bytes.len()].copy_from_slice(new_bytes);
    for b in &mut payload[new_bytes.len()..] {
        *b = b' ';
    }
    Ok(())
}

// ─── Test / fixture writer (v17, little-endian) ────────────────────────────

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

    /// Length prefix must stay byte-identical (big-endian assets break if we
    /// re-encode the u32 as little-endian).
    #[test]
    fn rewrite_preserves_length_prefix_bytes_including_be() {
        // Synthetic slot: BE u32 length=8 + 8 payload bytes.
        let mut buf = vec![0u8; 12];
        buf[0..4].copy_from_slice(&8u32.to_be_bytes());
        buf[4..12].copy_from_slice(b"ABCDEFGH");
        let prefix_before = buf[0..4].to_vec();
        rewrite_text_asset_script_inplace(&mut buf, 0, 8, "Hi", "be.assets").unwrap();
        assert_eq!(
            &buf[0..4],
            prefix_before.as_slice(),
            "length prefix must not be rewritten"
        );
        assert_eq!(&buf[4..6], b"Hi");
        assert_eq!(&buf[6..12], b"      "); // space pad
    }

    /// Multi-byte UTF-8 shorter rewrite pads with 0x20; length field unchanged.
    #[test]
    fn rewrite_utf8_multibyte_shorter_pads() {
        // "¿Seguro?" = 9 bytes UTF-8; rewrite to "Sí" (3 bytes) + spaces.
        let src = "¿Seguro?";
        assert_eq!(src.len(), 9);
        let bytes = write_v17_fixture("Q", src);
        let sf = SerializedFile::parse(bytes.clone(), "u8.assets").unwrap();
        let ta = sf.read_text_asset(1).unwrap();
        let mut file = bytes;
        let prefix = file[ta.script_len_offset..ta.script_len_offset + 4].to_vec();
        rewrite_text_asset_script_inplace(
            &mut file,
            ta.script_len_offset,
            ta.script_byte_len,
            "Sí",
            "u8.assets",
        )
        .unwrap();
        assert_eq!(
            &file[ta.script_len_offset..ta.script_len_offset + 4],
            prefix.as_slice()
        );
        let again = SerializedFile::parse(file, "u8.assets").unwrap();
        let ta2 = again.read_text_asset(1).unwrap();
        assert!(ta2.script.starts_with("Sí"), "got {:?}", ta2.script);
        assert_eq!(ta2.script.len(), 9);
        assert!(ta2.script.ends_with(' '));
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

    #[test]
    fn parse_v17_with_type_tree_blob_skipped() {
        let bytes = write_v17_fixture_with_type_tree("HelloName", "Hello script body");
        let sf = SerializedFile::parse(bytes, "tt.assets").unwrap();
        assert_eq!(sf.objects.len(), 1);
        let ta = sf.read_text_asset(1).unwrap();
        assert_eq!(ta.name, "HelloName");
        assert_eq!(ta.script, "Hello script body");
    }

    #[test]
    fn parse_v17_mono_behaviour_strings() {
        let bytes = write_v17_mono_fixture(
            "DialogBox",
            &["Welcome, traveler!", "See you later."],
        );
        let sf = SerializedFile::parse(bytes, "mono.assets").unwrap();
        let monos: Vec<_> = sf.mono_behaviour_objects().collect();
        assert_eq!(monos.len(), 1);
        assert_eq!(monos[0].path_id, 10);
        assert_eq!(monos[0].class_id, CLASS_ID_MONO_BEHAVIOUR);

        let fields = sf.read_mono_strings(10).unwrap();
        // m_Name + 2 script strings
        assert_eq!(fields.len(), 3, "fields: {fields:?}");
        assert_eq!(fields[0].field_index, 0);
        assert_eq!(fields[0].text, "DialogBox");
        assert_eq!(fields[1].text, "Welcome, traveler!");
        assert_eq!(fields[2].text, "See you later.");
        assert_eq!(fields[1].mono_name, "DialogBox");
    }

    #[test]
    fn rewrite_mono_script_field_inplace() {
        let bytes = write_v17_mono_fixture("Box", &["Hi world"]); // dialogue = 8 bytes
        let sf = SerializedFile::parse(bytes.clone(), "m.assets").unwrap();
        let fields = sf.read_mono_strings(10).unwrap();
        assert!(
            fields.iter().any(|f| f.text == "Hi world"),
            "pre-rewrite fields: {fields:?}"
        );
        let dialogue = fields.iter().find(|f| f.text == "Hi world").unwrap();
        let mut file = bytes;
        rewrite_text_asset_script_inplace(
            &mut file,
            dialogue.len_offset,
            dialogue.byte_len,
            "Hola",
            "m.assets",
        )
        .unwrap();
        let again = SerializedFile::parse(file, "m.assets").unwrap();
        let fields2 = again.read_mono_strings(10).unwrap();
        let d2 = fields2
            .iter()
            .find(|f| f.text.starts_with("Hola"))
            .expect(&format!("post-rewrite fields: {fields2:?}"));
        assert_eq!(d2.byte_len, 8);
    }

    #[test]
    fn structural_ranges_include_mono() {
        let bytes = write_v17_mono_fixture("N", &["Hi there"]);
        let sf = SerializedFile::parse(bytes, "m.assets").unwrap();
        let ranges = sf.structural_object_byte_ranges();
        assert_eq!(ranges.len(), 1);
        assert!(ranges[0].1 > ranges[0].0);
    }

    #[test]
    fn is_monobehaviour_class_accepts_114_and_negative() {
        assert!(is_monobehaviour_class(CLASS_ID_MONO_BEHAVIOUR));
        assert!(is_monobehaviour_class(-1));
        assert!(is_monobehaviour_class(-12345));
        assert!(!is_monobehaviour_class(CLASS_ID_TEXT_ASSET));
        assert!(!is_monobehaviour_class(1));
        assert!(!is_monobehaviour_class(0));
    }

    /// Negative type-table class ids are MonoBehaviour script types in some
    /// SerializedFiles — must extract sequential strings like class 114.
    #[test]
    fn parse_v17_negative_class_id_monobehaviour() {
        let bytes = write_v17_mono_fixture_with_class(
            -42,
            "DialogBox",
            &["Welcome, traveler!"],
        );
        let sf = SerializedFile::parse(bytes, "neg.assets").unwrap();
        let monos: Vec<_> = sf.mono_behaviour_objects().collect();
        assert_eq!(monos.len(), 1, "negative class_id must count as mono");
        assert_eq!(monos[0].class_id, -42);
        assert_eq!(monos[0].path_id, 10);

        let fields = sf.read_mono_strings(10).unwrap();
        assert!(
            fields.iter().any(|f| f.text == "Welcome, traveler!"),
            "fields: {fields:?}"
        );
        assert_eq!(sf.structural_object_byte_ranges().len(), 1);
    }
}

/// v17 fixture: one TextAsset, enable_type_tree=1 with a minimal skippable blob.
#[cfg(test)]
pub fn write_v17_fixture_with_type_tree(text_name: &str, text_script: &str) -> Vec<u8> {
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

    let mut text_payload = Vec::new();
    write_aligned_string(&mut text_payload, text_name);
    write_aligned_string(&mut text_payload, text_script);

    let mut meta = Vec::new();
    meta.extend_from_slice(b"2019.4.0f1\0");
    meta.extend_from_slice(&1u32.to_le_bytes());
    meta.push(1); // enable_type_tree = true

    meta.extend_from_slice(&1i32.to_le_bytes()); // 1 type
    meta.extend_from_slice(&CLASS_ID_TEXT_ASSET.to_le_bytes());
    meta.push(0);
    meta.extend_from_slice(&(-1i16).to_le_bytes());
    meta.extend_from_slice(&[0u8; 16]); // old_type_hash
    // type tree blob: 1 node (24 bytes for v17), empty string buffer
    meta.extend_from_slice(&1i32.to_le_bytes()); // node count
    meta.extend_from_slice(&0i32.to_le_bytes()); // string buffer size
    meta.extend_from_slice(&[0u8; 24]); // one node

    meta.extend_from_slice(&1i32.to_le_bytes()); // 1 object
    while meta.len() % 4 != 0 {
        meta.push(0);
    }
    meta.extend_from_slice(&1i64.to_le_bytes());
    meta.extend_from_slice(&0u32.to_le_bytes()); // byte_start
    meta.extend_from_slice(&(text_payload.len() as u32).to_le_bytes());
    meta.extend_from_slice(&0i32.to_le_bytes()); // type index

    let header_len = 20usize;
    let mut data_offset = header_len + meta.len();
    data_offset = (data_offset + 15) & !15;
    let file_size = data_offset + text_payload.len();

    let mut out = Vec::new();
    out.extend_from_slice(&(meta.len() as u32).to_be_bytes());
    out.extend_from_slice(&(file_size as u32).to_be_bytes());
    out.extend_from_slice(&17u32.to_be_bytes());
    out.extend_from_slice(&(data_offset as u32).to_be_bytes());
    out.push(0);
    out.extend_from_slice(&[0, 0, 0]);
    out.extend_from_slice(&meta);
    while out.len() < data_offset {
        out.push(0);
    }
    out.extend_from_slice(&text_payload);
    out
}

/// v17 fixture: one MonoBehaviour with `m_Name` + sequential string fields.
#[cfg(test)]
pub fn write_v17_mono_fixture(mono_name: &str, script_strings: &[&str]) -> Vec<u8> {
    write_v17_mono_fixture_with_class(CLASS_ID_MONO_BEHAVIOUR, mono_name, script_strings)
}

/// Like [`write_v17_mono_fixture`] but with an explicit type-table `class_id`
/// (e.g. negative script-type id).
#[cfg(test)]
pub fn write_v17_mono_fixture_with_class(
    class_id: i32,
    mono_name: &str,
    script_strings: &[&str],
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
    fn write_pptr(buf: &mut Vec<u8>, file_id: i32, path_id: i64) {
        buf.extend_from_slice(&file_id.to_le_bytes());
        buf.extend_from_slice(&path_id.to_le_bytes());
    }

    let mut payload = Vec::new();
    write_pptr(&mut payload, 0, 0); // m_GameObject
    payload.push(1); // m_Enabled
    while payload.len() % 4 != 0 {
        payload.push(0);
    }
    write_pptr(&mut payload, 0, 0); // m_Script
    write_aligned_string(&mut payload, mono_name);
    for s in script_strings {
        write_aligned_string(&mut payload, s);
    }

    let mut meta = Vec::new();
    meta.extend_from_slice(b"2019.4.0f1\0");
    meta.extend_from_slice(&1u32.to_le_bytes());
    meta.push(0); // no type tree

    meta.extend_from_slice(&1i32.to_le_bytes());
    meta.extend_from_slice(&class_id.to_le_bytes());
    meta.push(0);
    meta.extend_from_slice(&0i16.to_le_bytes()); // script_type_index
    // script_id present for mono 114 and negative script-type ids
    if is_monobehaviour_class(class_id) {
        meta.extend_from_slice(&[0u8; 16]);
    }
    meta.extend_from_slice(&[0u8; 16]); // old_type_hash

    meta.extend_from_slice(&1i32.to_le_bytes());
    while meta.len() % 4 != 0 {
        meta.push(0);
    }
    meta.extend_from_slice(&10i64.to_le_bytes()); // path_id
    meta.extend_from_slice(&0u32.to_le_bytes());
    meta.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    meta.extend_from_slice(&0i32.to_le_bytes());

    let header_len = 20usize;
    let mut data_offset = header_len + meta.len();
    data_offset = (data_offset + 15) & !15;
    let file_size = data_offset + payload.len();

    let mut out = Vec::new();
    out.extend_from_slice(&(meta.len() as u32).to_be_bytes());
    out.extend_from_slice(&(file_size as u32).to_be_bytes());
    out.extend_from_slice(&17u32.to_be_bytes());
    out.extend_from_slice(&(data_offset as u32).to_be_bytes());
    out.push(0);
    out.extend_from_slice(&[0, 0, 0]);
    out.extend_from_slice(&meta);
    while out.len() < data_offset {
        out.push(0);
    }
    out.extend_from_slice(&payload);
    out
}
