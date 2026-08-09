//! Unity SerializedFile container — slices 1–2:
//! - **Slice 1:** header, type table, object table, TextAsset (class_id 49)
//!   `m_Name` / `m_Script` reads + in-place rewrite (payload pad with `0x20`;
//!   length-prefix u32 left byte-identical so BE assets stay valid).
//! - **Slice 2:** skip type-tree blobs (no field interpretation), MonoBehaviour
//!   (class_id 114 **or negative** script-type ids) base layout (`m_GameObject`,
//!   `m_Enabled`, `m_Script`, `m_Name`) plus sequential aligned-string fields
//!   after the base for extract/in-place rewrite (also recovers Unity `string[]` /
//!   `List<string>` as i32 count + N aligned strings when count is small);
//!   **TextMesh** (class_id 141) `m_Text` after `m_GameObject` PPtr; **GUIText**
//!   (class_id 132) `m_Text` after Behaviour base + `m_PixelOffset`. Heuristic
//!   scan also **skips MonoScript (115) + Shader (48)** ranges (type-name / HLSL
//!   noise). Full type-tree walks remain out of scope.
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
/// Unity class ID for Shader (HLSL source — not player-facing text).
pub const CLASS_ID_SHADER: i32 = 48;
/// Unity class ID for MonoBehaviour.
pub const CLASS_ID_MONO_BEHAVIOUR: i32 = 114;
/// Unity class ID for MonoScript (assembly type metadata — heuristic noise).
pub const CLASS_ID_MONO_SCRIPT: i32 = 115;
/// Unity class ID for legacy TextMesh (3D text component).
pub const CLASS_ID_TEXT_MESH: i32 = 141;
/// Unity class ID for legacy GUIText (screen-space text component).
pub const CLASS_ID_GUI_TEXT: i32 = 132;

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

/// TextMesh / GUIText `m_Text` field (class_id 141 / 132).
#[derive(Debug, Clone)]
pub struct TextMeshData {
    pub path_id: i64,
    pub text: String,
    /// Absolute file offset of the `m_Text` length prefix (u32).
    pub text_len_offset: usize,
    /// Original `m_Text` string byte length (not including length prefix / align).
    pub text_byte_len: usize,
}

/// Alias: same shape as [`TextMeshData`] (aligned `m_Text` + absolute offsets).
pub type GuiTextData = TextMeshData;

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

    pub fn text_mesh_objects(&self) -> impl Iterator<Item = &ObjectInfo> {
        self.objects
            .iter()
            .filter(|o| o.class_id == CLASS_ID_TEXT_MESH)
    }

    pub fn gui_text_objects(&self) -> impl Iterator<Item = &ObjectInfo> {
        self.objects
            .iter()
            .filter(|o| o.class_id == CLASS_ID_GUI_TEXT)
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
        // When a length prefix is implausible (typical int/float between strings),
        // skip up to MAX_MONO_NON_STRING_SKIPS × 4-byte words and keep scanning —
        // recovers `string, int, string` layouts without a full type-tree walk.
        // Small peeks (1..=64) try Unity `string[]` / `List<string>` first so an
        // array count is not consumed as a short garbage string (which desyncs).
        const MAX_MONO_NON_STRING_SKIPS: usize = 16;
        const MAX_MONO_STRING_ARRAY_LEN: u32 = 64;
        let mut field_index = 1usize;
        let mut non_string_skips = 0usize;
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
                if non_string_skips < MAX_MONO_NON_STRING_SKIPS {
                    // Skip one 4-byte word (int/float/enum) and try again.
                    let _ = r.take(4);
                    non_string_skips += 1;
                    continue;
                }
                break;
            }

            // Prefer string-array when the u32 looks like a small element count.
            if (1..=MAX_MONO_STRING_ARRAY_LEN).contains(&len_peek) {
                if let Some(items) =
                    try_read_mono_string_array(&mut r, end, len_peek as usize)
                {
                    non_string_skips = 0;
                    for (text, len_offset, byte_len) in items {
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
                        field_index += 1;
                    }
                    continue;
                }
            }

            match r.aligned_string() {
                Ok((text, len_offset, byte_len)) => {
                    if r.pos > end {
                        // Overran — discard
                        break;
                    }
                    non_string_skips = 0;
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
                    if non_string_skips < MAX_MONO_NON_STRING_SKIPS {
                        let _ = r.take(4);
                        non_string_skips += 1;
                        continue;
                    }
                    break;
                }
            }
        }

        Ok(out)
    }

    /// Read TextMesh `m_Text` at `path_id`.
    ///
    /// Layout (Component base + TextMesh fields): `PPtr m_GameObject`, then
    /// aligned string `m_Text`. Remaining floats/ints/font/color are ignored.
    pub fn read_text_mesh(&self, path_id: i64) -> Result<TextMeshData, SerializedError> {
        let label = self.path.display().to_string();
        let obj = self
            .objects
            .iter()
            .find(|o| o.path_id == path_id && o.class_id == CLASS_ID_TEXT_MESH)
            .ok_or_else(|| err(&label, format!("no TextMesh with path_id={path_id}")))?;

        let start = obj.data_abs as usize;
        let end = (start + obj.byte_size as usize).min(self.data.len());
        let mut r = R {
            data: &self.data[..end],
            pos: start,
            file: &label,
            endian: self.header.endian,
        };

        // m_GameObject PPtr (FileID i32 + PathID i64)
        let _go_file = r.i32()?;
        let _go_path = r.i64()?;
        let (text, text_len_offset, text_byte_len) = r.aligned_string()?;
        Ok(TextMeshData {
            path_id,
            text,
            text_len_offset,
            text_byte_len,
        })
    }

    /// Read GUIText `m_Text` at `path_id`.
    ///
    /// Layout (Behaviour base + GUIText): `PPtr m_GameObject`, `u8 m_Enabled` +
    /// align4, `Vector2 m_PixelOffset` (2×f32), then aligned string `m_Text`.
    pub fn read_gui_text(&self, path_id: i64) -> Result<GuiTextData, SerializedError> {
        let label = self.path.display().to_string();
        let obj = self
            .objects
            .iter()
            .find(|o| o.path_id == path_id && o.class_id == CLASS_ID_GUI_TEXT)
            .ok_or_else(|| err(&label, format!("no GUIText with path_id={path_id}")))?;

        let start = obj.data_abs as usize;
        let end = (start + obj.byte_size as usize).min(self.data.len());
        let mut r = R {
            data: &self.data[..end],
            pos: start,
            file: &label,
            endian: self.header.endian,
        };

        // m_GameObject PPtr
        let _go_file = r.i32()?;
        let _go_path = r.i64()?;
        // m_Enabled + align4 (Behaviour)
        let _enabled = r.u8()?;
        r.align4();
        // m_PixelOffset Vector2 (2 × f32)
        let _px = r.take(4)?;
        let _py = r.take(4)?;
        let (text, text_len_offset, text_byte_len) = r.aligned_string()?;
        Ok(GuiTextData {
            path_id,
            text,
            text_len_offset,
            text_byte_len,
        })
    }

    /// Absolute byte ranges of TextAsset + MonoBehaviour objects (heuristic skip).
    pub fn text_asset_byte_ranges(&self) -> Vec<(usize, usize)> {
        self.structural_object_byte_ranges()
    }

    /// Absolute byte ranges of objects handled structurally
    /// (TextAsset + MonoBehaviour + TextMesh + GUIText).
    pub fn structural_object_byte_ranges(&self) -> Vec<(usize, usize)> {
        self.objects
            .iter()
            .filter(|o| is_structural_extract_class(o.class_id))
            .map(|o| {
                let s = o.data_abs as usize;
                (s, s + o.byte_size as usize)
            })
            .collect()
    }

    /// Ranges the heuristic length-prefix scan must not re-read: structural
    /// extract classes **plus** known non-text blobs (MonoScript type names,
    /// Shader source) that flood extracts with engine identifiers.
    pub fn heuristic_skip_byte_ranges(&self) -> Vec<(usize, usize)> {
        self.objects
            .iter()
            .filter(|o| {
                is_structural_extract_class(o.class_id) || is_heuristic_noise_class(o.class_id)
            })
            .map(|o| {
                let s = o.data_abs as usize;
                (s, s + o.byte_size as usize)
            })
            .collect()
    }
}

/// Classes we extract via dedicated structural readers.
#[inline]
pub fn is_structural_extract_class(class_id: i32) -> bool {
    class_id == CLASS_ID_TEXT_ASSET
        || is_monobehaviour_class(class_id)
        || class_id == CLASS_ID_TEXT_MESH
        || class_id == CLASS_ID_GUI_TEXT
}

/// Classes that are never player-facing dialogue but often contain length-prefixed
/// ASCII identifiers (type names, HLSL). Heuristic scan skips their byte ranges.
#[inline]
pub fn is_heuristic_noise_class(class_id: i32) -> bool {
    class_id == CLASS_ID_MONO_SCRIPT || class_id == CLASS_ID_SHADER
}

/// Try reading Unity `string[]` / `List<string>`: i32/u32 count already at `r.pos`,
/// then `count` aligned strings. On failure restores `r.pos` and returns `None`.
///
/// Accepts only when every element parses and at least one passes the script-field
/// filter (avoids treating a real short string like `"Yes"` as array count 3).
fn try_read_mono_string_array(
    r: &mut R<'_>,
    end: usize,
    count: usize,
) -> Option<Vec<(String, usize, usize)>> {
    let start_pos = r.pos;
    // Consume count u32.
    if r.take(4).is_err() {
        r.pos = start_pos;
        return None;
    }
    let mut items = Vec::with_capacity(count);
    for _ in 0..count {
        if r.pos + 4 > end {
            r.pos = start_pos;
            return None;
        }
        let elem_len = {
            let b = &r.data[r.pos..r.pos + 4];
            match r.endian {
                Endian::Little => u32::from_le_bytes([b[0], b[1], b[2], b[3]]),
                Endian::Big => u32::from_be_bytes([b[0], b[1], b[2], b[3]]),
            }
        };
        let rem = end.saturating_sub(r.pos + 4);
        if elem_len as usize > rem || elem_len > 4 * 1024 * 1024 {
            r.pos = start_pos;
            return None;
        }
        match r.aligned_string() {
            Ok((text, len_offset, byte_len)) => {
                if r.pos > end {
                    r.pos = start_pos;
                    return None;
                }
                // Non-empty binary-looking elements reject the array hypothesis.
                if !text.is_empty() && is_binary_looking_script(&text) {
                    r.pos = start_pos;
                    return None;
                }
                items.push((text, len_offset, byte_len));
            }
            Err(_) => {
                r.pos = start_pos;
                return None;
            }
        }
    }
    if !items
        .iter()
        .any(|(t, _, _)| mono_script_field_worth_extracting(t))
    {
        r.pos = start_pos;
        return None;
    }
    Some(items)
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
    if is_mono_engine_noise(t) {
        return false;
    }
    // Need ≥2 letters so crumbs like `v'` / `A!` don't slip through.
    t.chars().filter(|c| c.is_alphabetic()).count() >= 2
}

/// High-precision engine metadata that floods MonoBehaviour walks (BOXMAN/Naninovel).
/// Kept separate so real UI verbs (`Play`, `Save`) and short labels (`Q.SAVE`) survive.
fn is_mono_engine_noise(t: &str) -> bool {
    let t = t.trim();
    // Managed assembly names.
    if t == "Assembly-CSharp"
        || t == "Assembly-CSharp-firstpass"
        || t.starts_with("Assembly-CSharp")
    {
        return true;
    }
    // Full/short .NET assembly-qualified type names.
    if looks_like_assembly_qualified_type(t) {
        return true;
    }
    // Namespace / type tokens: `Naninovel.Commands`, `UnityEngine.DMAT`.
    // Preserves UI-ish `Q.SAVE` / `Q.LOAD` (short ALL-CAPS segments).
    if looks_like_dotted_type_name(t) {
        return true;
    }
    // Unity Selectable ColorBlock state labels (not player-facing copy).
    if matches!(
        t,
        "Normal" | "Highlighted" | "Pressed" | "Selected" | "Disabled" | "Focused"
    ) {
        return true;
    }
    // Framework product token + ubiquitous serialized default label.
    if matches!(t, "Naninovel" | "Default") {
        return true;
    }
    // Naninovel / script control-flow commands (never player-facing copy).
    // Keep UI verbs: Play, Wait, Stop, Skip, Save, Load, Config, …
    if matches!(t, "Gosub" | "Goto" | "Else") {
        return true;
    }
    // Naninovel scenario script blobs (`@hideUI …`, multi-line `@novel` blocks).
    if looks_like_naninovel_script(t) {
        return true;
    }
    // Designer placeholder copy.
    if looks_like_lorem_ipsum(t) {
        return true;
    }
    // Live2D / face blend-shape parameter labels (BOXMAN).
    if looks_like_face_or_blend_param(t) {
        return true;
    }
    // Pure template / variable tokens: `{g_saveslot}`.
    if t.starts_with('{') && t.ends_with('}') && t.len() >= 3 && !t[1..t.len() - 1].contains(' ')
    {
        return true;
    }
    // Mixer / resource path fragments without sentence whitespace: `Master/HFX`.
    if !t.contains(' ') && t.contains('/') {
        return true;
    }
    // Asset / guid-ish hex blobs mis-read as strings (BOXMAN ~70 rows).
    if looks_like_hex_token(t) {
        return true;
    }
    // Unity component / pipeline tokens that are not player-facing copy.
    // Keep real UI verbs (Play/Wait/Save) and labels (Master volume may be UI — allow).
    if matches!(
        t,
        "Fader" | "Clip" | "Canvas" | "Sprites" | "trigger" | "Author Name"
    ) {
        return true;
    }
    false
}

/// Pure hexadecimal id / guid fragment: `72010b7a`, `7d24045dcfc9abb4…`.
fn looks_like_hex_token(t: &str) -> bool {
    let t = t.trim();
    // 6+ hex digits avoids short numerics; pure hex only (no spaces).
    t.len() >= 6
        && t.chars()
            .all(|c| c.is_ascii_hexdigit())
        // Require at least one a-f so pure decimal numbers can stay (rare UI).
        && t.chars().any(|c| matches!(c, 'a'..='f' | 'A'..='F'))
}

/// Naninovel (and similar) scenario commands: every non-empty line is an `@cmd…`.
/// BOXMAN stores these as MonoBehaviour string fields next to real UI labels.
pub(crate) fn looks_like_naninovel_script(t: &str) -> bool {
    let t = t.trim();
    if t.is_empty() {
        return false;
    }
    // Single-line or leading `@hideUI TutorialUI` / `@else` / `@moveMode state:"drive"`.
    if t.starts_with('@') {
        return true;
    }
    // Multi-line block where every non-empty line is a command.
    let lines: Vec<&str> = t
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    lines.len() >= 2 && lines.iter().all(|l| l.starts_with('@'))
}

/// Classic designer placeholder (BOXMAN ships Lorem blocks in UI prefabs).
pub(crate) fn looks_like_lorem_ipsum(t: &str) -> bool {
    let lower = t.to_ascii_lowercase();
    lower.contains("lorem ipsum")
}

/// Live2D / facial rig parameter names: `EyeR Open`, `Eyeball Y`, `Mouth Form`,
/// bare `Brows` / `Breath`. Not player-facing dialogue.
fn looks_like_face_or_blend_param(t: &str) -> bool {
    let t = t.trim();
    if matches!(
        t,
        "Brows" | "Breath" | "Splat" | "Crop" | "Mouth" | "Jaw" | "Cheek"
    ) {
        return true;
    }
    let mut parts = t.split_whitespace();
    let Some(first) = parts.next() else {
        return false;
    };
    // EyeL / EyeR / Eyeball / BrowL / BrowR / Mouth / Jaw + short axis/shape token.
    let face_head = matches!(
        first,
        "EyeL"
            | "EyeR"
            | "Eyeball"
            | "BrowL"
            | "BrowR"
            | "Brow"
            | "Brows"
            | "Mouth"
            | "Jaw"
            | "Cheek"
    );
    if !face_head {
        return false;
    }
    // Single token already handled above for bare names; multi-token: ≤2 short tails.
    let rest: Vec<&str> = parts.collect();
    if rest.is_empty() {
        return true;
    }
    rest.len() <= 2
        && rest.iter().all(|p| {
            p.len() <= 8
                && p.chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        })
}

/// .NET / Unity assembly-qualified type name.
///
/// Short form: `UnityEngine.Object, UnityEngine`
/// Full form (BOXMAN Naninovel configs):  
/// `Naninovel.Script, Elringus.Naninovel.Runtime, Version=0.0.0.0, Culture=neutral, PublicKeyToken=null`
pub(crate) fn looks_like_assembly_qualified_type(t: &str) -> bool {
    let t = t.trim();
    if t.len() < 8 || !t.contains(',') || !t.contains('.') {
        return false;
    }
    // Canonical full AQN markers.
    if t.contains("Version=")
        && (t.contains("PublicKeyToken=") || t.contains("Culture="))
    {
        return true;
    }
    // Common short assembly suffixes after the type name.
    if t.contains(", UnityEngine")
        || t.contains(", UnityEditor")
        || t.contains(", Assembly-CSharp")
        || t.contains(", Elringus.")
        || t.contains(", TMPro")
        || t.contains(", Unity.")
        || t.contains(", System.")
        || t.contains(", mscorlib")
    {
        return true;
    }
    // Compact form with no spaces: `Foo.Bar,Baz.Qux`
    if !t.contains(' ') && t.contains('.') && t.contains(',') {
        return true;
    }
    false
}

/// `Foo.Bar` / `A.B.C` type or namespace tokens (no spaces).
/// Returns false for short UI abbreviations like `Q.SAVE`.
fn looks_like_dotted_type_name(t: &str) -> bool {
    if t.contains(' ') || !t.contains('.') {
        return false;
    }
    if !t
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_')
    {
        return false;
    }
    let parts: Vec<&str> = t.split('.').filter(|p| !p.is_empty()).collect();
    if parts.len() < 2 {
        return false;
    }
    if !parts
        .iter()
        .all(|p| p.chars().next().is_some_and(|c| c.is_ascii_alphabetic()))
    {
        return false;
    }
    // `Q.SAVE` / `Q.LOAD`: every segment is short UPPER/digit — keep as UI.
    if parts.iter().all(|p| {
        p.len() <= 4
            && p.chars()
                .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
    }) {
        return false;
    }
    // At least one real identifier segment (avoids `A.B` toy tokens).
    parts.iter().any(|p| p.len() >= 4)
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
    // Assembly-qualified type refs (short + full .NET AQN with Version=…).
    if looks_like_assembly_qualified_type(t) {
        return false;
    }
    // Engine API / serialized method tokens (not player-facing).
    if matches!(
        t,
        "set_text"
            | "get_text"
            | "set_enabled"
            | "get_enabled"
            | "set_active"
            | "get_active"
    ) {
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

/// True for PascalCase / camelCase / `name2` style tokens — not plain words
/// like `"Hola"` / `"Save"`. Shared by MonoBehaviour field filter and heuristic
/// `is_unity_translatable`.
pub(crate) fn looks_like_code_identifier(t: &str) -> bool {
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

/// Whether a TextAsset `m_Script` body is worth extracting as translateable text.
/// Drops empty/binary payloads and line-break **character-class tables** (TMP/ICU
/// style bags of punctuation / small kana with no Latin words — BOXMAN 830/831).
pub fn is_textasset_script_worth_extracting(script: &str) -> bool {
    let t = script.trim();
    if t.is_empty() || is_binary_looking_script(script) {
        return false;
    }
    if looks_like_linebreak_charset_table(t) {
        return false;
    }
    let total = t.chars().count();
    if total >= 20 {
        let letters = t.chars().filter(|c| c.is_alphabetic()).count();
        let ratio = letters as f64 / total as f64;
        // Pure punctuation/symbol tables (no CJK letters either).
        if ratio < 0.12 {
            return false;
        }
    }
    true
}

/// TMP / ICU line-break character-class tables: almost no whitespace, no Latin
/// word of length ≥3, dense symbols (and often small kana which *are* alphabetic).
fn looks_like_linebreak_charset_table(t: &str) -> bool {
    let total = t.chars().count();
    if total < 20 || total > 400 {
        return false;
    }
    let ws = t.chars().filter(|c| c.is_whitespace()).count();
    // Real scripts have spaces/newlines between words; charset tables are one blob.
    if ws > 3 {
        return false;
    }
    // Any run of 3+ ASCII letters ⇒ likely prose / code / locale text.
    let mut run = 0usize;
    for c in t.chars() {
        if c.is_ascii_alphabetic() {
            run += 1;
            if run >= 3 {
                return false;
            }
        } else {
            run = 0;
        }
    }
    true
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

    /// Format version 22 (LargeFilesSupport): extended header + u64 byte_start.
    #[test]
    fn parse_v22_text_asset_large_files_support() {
        let bytes = write_v22_textasset_fixture("Dlg", "Hello from v22 assets");
        let sf = SerializedFile::parse(bytes, "v22.assets").unwrap();
        assert_eq!(sf.header.version, 22);
        assert!(sf.header.data_offset >= 48, "extended header pushes metadata past 20");
        let texts: Vec<_> = sf.text_asset_objects().collect();
        assert_eq!(texts.len(), 1);
        assert_eq!(texts[0].path_id, 1);
        let ta = sf.read_text_asset(1).unwrap();
        assert_eq!(ta.name, "Dlg");
        assert_eq!(ta.script, "Hello from v22 assets");
    }

    #[test]
    fn rewrite_v22_text_asset_inplace() {
        let bytes = write_v22_textasset_fixture("N", "ABCDEFGH");
        let sf = SerializedFile::parse(bytes.clone(), "v22.assets").unwrap();
        let ta = sf.read_text_asset(1).unwrap();
        let mut file = bytes;
        rewrite_text_asset_script_inplace(
            &mut file,
            ta.script_len_offset,
            ta.script_byte_len,
            "Hola",
            "v22.assets",
        )
        .unwrap();
        let again = SerializedFile::parse(file, "v22.assets").unwrap();
        let ta2 = again.read_text_asset(1).unwrap();
        assert!(
            ta2.script.starts_with("Hola"),
            "v22 inject: {:?}",
            ta2.script
        );
        assert_eq!(again.header.version, 22);
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
    fn textasset_script_worth_extracting_rejects_charset_tables() {
        let charset = "([｛〔〈《「『【〘〖〝‘“｟«$—…‥〳〴〵\\［（{£¥\"々〇〉》」＄｠￥￦ #)]｝〕〉》」』】〙〗〟’”｠»";
        assert!(!is_textasset_script_worth_extracting(charset));
        // CJK small-kana line-break class (letters, but no Latin words / spaces).
        let kana_table = ")]｝〕〉》」』】〙〗〟’”｠»ヽゴミ袋ァィゥェォッャュョヮヵヶぁぃぅぇぉっゃゅょゎゕゖㇰㇱㇲㇳㇴㇵㇶㇷㇸㇹㇺㇻㇼㇽㇾㇿ々〻‐゠–〜?!‼⁇⁈⁉・、%,.:;。！？］）：；＝}¢°\"†‡℃〆％，．";
        assert!(
            !is_textasset_script_worth_extracting(kana_table),
            "kana linebreak table must be rejected"
        );
        assert!(!is_textasset_script_worth_extracting(""));
        assert!(!is_textasset_script_worth_extracting("   \n"));
        assert!(is_textasset_script_worth_extracting(
            "Gallery.Scene1: Change of Heart\r\nTitleMenu.START: NEW GAME"
        ));
        assert!(is_textasset_script_worth_extracting(
            "ITEM_CATEGORY,ITEM_NAME\r\nElectronics,mp3 player"
        ));
        assert!(is_textasset_script_worth_extracting(
            "TitleMenu.START: NEW GAME"
        ));
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

    /// Format ≥19 type-tree nodes are 32 bytes (RefTypeHash); wrong size desyncs object table.
    #[test]
    fn parse_v19_with_type_tree_32byte_nodes_skipped() {
        let bytes = write_v19_fixture_with_type_tree("V19Name", "V19 script body here");
        let sf = SerializedFile::parse(bytes, "tt19.assets").unwrap();
        assert_eq!(sf.header.version, 19);
        assert_eq!(sf.objects.len(), 1);
        let ta = sf.read_text_asset(1).unwrap();
        assert_eq!(ta.name, "V19Name");
        assert_eq!(ta.script, "V19 script body here");
    }

    /// Big-endian metadata + object data (endian flag ≠ 0).
    #[test]
    fn parse_v17_big_endian_text_asset() {
        let bytes = write_v17_be_textasset_fixture("BEName", "Big endian script");
        let sf = SerializedFile::parse(bytes, "be.assets").unwrap();
        assert_eq!(sf.header.endian, Endian::Big);
        let ta = sf.read_text_asset(1).unwrap();
        assert_eq!(ta.name, "BEName");
        assert_eq!(ta.script, "Big endian script");
    }

    #[test]
    fn rewrite_v17_big_endian_text_asset_preserves_prefix() {
        let bytes = write_v17_be_textasset_fixture("N", "ABCDEFGH");
        let sf = SerializedFile::parse(bytes.clone(), "be.assets").unwrap();
        let ta = sf.read_text_asset(1).unwrap();
        let mut file = bytes;
        let prefix = file[ta.script_len_offset..ta.script_len_offset + 4].to_vec();
        assert_eq!(
            prefix,
            8u32.to_be_bytes().to_vec(),
            "BE length prefix expected"
        );
        rewrite_text_asset_script_inplace(
            &mut file,
            ta.script_len_offset,
            ta.script_byte_len,
            "Hi",
            "be.assets",
        )
        .unwrap();
        assert_eq!(
            &file[ta.script_len_offset..ta.script_len_offset + 4],
            prefix.as_slice()
        );
        let again = SerializedFile::parse(file, "be.assets").unwrap();
        let ta2 = again.read_text_asset(1).unwrap();
        assert!(ta2.script.starts_with("Hi"), "{:?}", ta2.script);
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

    #[test]
    fn is_heuristic_noise_class_monoscript_and_shader() {
        assert!(is_heuristic_noise_class(CLASS_ID_MONO_SCRIPT));
        assert!(is_heuristic_noise_class(CLASS_ID_SHADER));
        assert!(!is_heuristic_noise_class(CLASS_ID_TEXT_ASSET));
        assert!(!is_heuristic_noise_class(CLASS_ID_MONO_BEHAVIOUR));
        assert!(!is_heuristic_noise_class(1)); // GameObject — may still be scanned
    }

    #[test]
    fn heuristic_skip_includes_monoscript_range() {
        let bytes = write_v17_monoscript_noise_fixture();
        let sf = SerializedFile::parse(bytes, "ms.assets").unwrap();
        let structural = sf.structural_object_byte_ranges();
        let skip = sf.heuristic_skip_byte_ranges();
        assert!(
            skip.len() > structural.len(),
            "MonoScript range must expand heuristic skip beyond structural"
        );
        // MonoScript object path_id=2
        let ms = sf
            .objects
            .iter()
            .find(|o| o.class_id == CLASS_ID_MONO_SCRIPT)
            .expect("fixture has MonoScript");
        let ms_start = ms.data_abs as usize;
        assert!(
            skip.iter().any(|&(s, e)| s <= ms_start && ms_start < e),
            "MonoScript body must be in heuristic skip ranges"
        );
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

    /// `string, int (implausible length), string` — skip the int and keep both strings.
    #[test]
    fn parse_mono_skips_int_between_strings() {
        let bytes = write_v17_mono_fixture_with_int_gap(
            "Box",
            "First line of dialogue",
            0x7fff_ff00u32, // way larger than remaining → not a string length
            "Second line after int",
        );
        let sf = SerializedFile::parse(bytes, "gap.assets").unwrap();
        let fields = sf.read_mono_strings(10).unwrap();
        let texts: Vec<&str> = fields.iter().map(|f| f.text.as_str()).collect();
        assert!(
            texts.iter().any(|t| *t == "First line of dialogue"),
            "fields: {texts:?}"
        );
        assert!(
            texts.iter().any(|t| *t == "Second line after int"),
            "must recover string after non-string int gap: {texts:?}"
        );
    }

    /// Offsets from gap-skipping extract must rewrite the post-gap string in place.
    #[test]
    fn rewrite_mono_string_after_int_gap() {
        let bytes = write_v17_mono_fixture_with_int_gap(
            "Box",
            "First line of dialogue", // long enough to leave pad room
            0x7fff_ff00u32,
            "Second line after int",
        );
        let sf = SerializedFile::parse(bytes.clone(), "gap.assets").unwrap();
        let fields = sf.read_mono_strings(10).unwrap();
        let second = fields
            .iter()
            .find(|f| f.text == "Second line after int")
            .expect("second string present");
        let mut file = bytes;
        rewrite_text_asset_script_inplace(
            &mut file,
            second.len_offset,
            second.byte_len,
            "Hola linea dos", // shorter UTF-8
            "gap.assets",
        )
        .unwrap();
        let again = SerializedFile::parse(file, "gap.assets").unwrap();
        let fields2 = again.read_mono_strings(10).unwrap();
        assert!(
            fields2
                .iter()
                .any(|f| f.text.starts_with("Hola linea dos")),
            "post-gap inject must land on second string: {fields2:?}"
        );
        // Gap must not have destroyed the first string.
        assert!(
            fields2
                .iter()
                .any(|f| f.text == "First line of dialogue"),
            "first string must remain: {fields2:?}"
        );
    }

    #[test]
    fn parse_v17_textmesh_m_text() {
        let bytes = write_v17_textmesh_fixture("Hello, world!");
        let sf = SerializedFile::parse(bytes, "tm.assets").unwrap();
        let meshes: Vec<_> = sf.text_mesh_objects().collect();
        assert_eq!(meshes.len(), 1);
        assert_eq!(meshes[0].path_id, 7);
        assert_eq!(meshes[0].class_id, CLASS_ID_TEXT_MESH);

        let tm = sf.read_text_mesh(7).unwrap();
        assert_eq!(tm.text, "Hello, world!");
        assert_eq!(tm.text_byte_len, "Hello, world!".len());
        assert_eq!(sf.structural_object_byte_ranges().len(), 1);
    }

    #[test]
    fn rewrite_textmesh_m_text_inplace() {
        let bytes = write_v17_textmesh_fixture("Hello, world!"); // 13 bytes
        let sf = SerializedFile::parse(bytes.clone(), "tm.assets").unwrap();
        let tm = sf.read_text_mesh(7).unwrap();
        let mut file = bytes;
        rewrite_text_asset_script_inplace(
            &mut file,
            tm.text_len_offset,
            tm.text_byte_len,
            "Hola!",
            "tm.assets",
        )
        .unwrap();
        let again = SerializedFile::parse(file, "tm.assets").unwrap();
        let tm2 = again.read_text_mesh(7).unwrap();
        assert!(
            tm2.text.starts_with("Hola!"),
            "post-rewrite: {:?}",
            tm2.text
        );
        assert_eq!(tm2.text_byte_len, 13);
    }

    #[test]
    fn parse_v17_guitext_m_text() {
        let bytes = write_v17_guitext_fixture("Press Start");
        let sf = SerializedFile::parse(bytes, "gt.assets").unwrap();
        let guis: Vec<_> = sf.gui_text_objects().collect();
        assert_eq!(guis.len(), 1);
        assert_eq!(guis[0].path_id, 8);
        assert_eq!(guis[0].class_id, CLASS_ID_GUI_TEXT);

        let gt = sf.read_gui_text(8).unwrap();
        assert_eq!(gt.text, "Press Start");
        assert_eq!(gt.text_byte_len, "Press Start".len());
        assert_eq!(sf.structural_object_byte_ranges().len(), 1);
    }

    #[test]
    fn rewrite_guitext_m_text_inplace() {
        let bytes = write_v17_guitext_fixture("Press Start"); // 11 bytes
        let sf = SerializedFile::parse(bytes.clone(), "gt.assets").unwrap();
        let gt = sf.read_gui_text(8).unwrap();
        let mut file = bytes;
        rewrite_text_asset_script_inplace(
            &mut file,
            gt.text_len_offset,
            gt.text_byte_len,
            "Pulsa",
            "gt.assets",
        )
        .unwrap();
        let again = SerializedFile::parse(file, "gt.assets").unwrap();
        let gt2 = again.read_gui_text(8).unwrap();
        assert!(
            gt2.text.starts_with("Pulsa"),
            "post-rewrite: {:?}",
            gt2.text
        );
        assert_eq!(gt2.text_byte_len, 11);
    }

    #[test]
    fn mono_script_filter_drops_assembly_and_api_tokens() {
        let bytes = write_v17_mono_fixture(
            "Holder",
            &[
                "Portable Speaker",
                "UnityEngine.Object, UnityEngine",
                // Full .NET AQN as serialized in BOXMAN Naninovel configs:
                "Naninovel.Script, Elringus.Naninovel.Runtime, Version=0.0.0.0, Culture=neutral, PublicKeyToken=null",
                "UnityEditor.DefaultAsset, UnityEditor, Version=0.0.0.0, Culture=neutral, PublicKeyToken=null",
                "TMPro.TMP_FontAsset, Unity.TextMeshPro, Version=0.0.0.0, Culture=neutral, PublicKeyToken=null",
                "set_text",
                "Welcome home",
            ],
        );
        let sf = SerializedFile::parse(bytes, "filt.assets").unwrap();
        let fields = sf.read_mono_strings(10).unwrap();
        let texts: Vec<&str> = fields.iter().map(|f| f.text.as_str()).collect();
        assert!(
            texts.iter().any(|t| *t == "Portable Speaker"),
            "keep dialogue-ish: {texts:?}"
        );
        assert!(
            texts.iter().any(|t| *t == "Welcome home"),
            "keep sentence: {texts:?}"
        );
        assert!(
            !texts.iter().any(|t| t.contains("UnityEngine")),
            "drop assembly type: {texts:?}"
        );
        assert!(
            !texts.iter().any(|t| t.contains("Version=") || t.contains("PublicKeyToken=")),
            "drop full AQN: {texts:?}"
        );
        assert!(
            !texts.iter().any(|t| t.contains("Naninovel.Script") || t.contains("TMP_FontAsset")),
            "drop Naninovel/TMPro AQN: {texts:?}"
        );
        assert!(
            !texts.iter().any(|t| *t == "set_text"),
            "drop API token: {texts:?}"
        );
    }

    /// BOXMAN-class flood: namespaces, Assembly-CSharp, Selectable color states,
    /// engine product tokens — while keeping real UI (Q.SAVE, Play, Save).
    #[test]
    fn mono_script_filter_drops_namespace_assembly_uistate_noise() {
        let bytes = write_v17_mono_fixture(
            "Naninovel", // m_Name noise
            &[
                "Naninovel.Commands",
                "Assembly-CSharp",
                "Highlighted",
                "Pressed",
                "Normal",
                "Disabled",
                "UnityEngine.DMAT",
                "ControlPanel.Config",
                "Master/HFX",
                "Q.SAVE",
                "Play",
                "Save game now",
                "Could be a replacement part",
            ],
        );
        let sf = SerializedFile::parse(bytes, "noise.assets").unwrap();
        let fields = sf.read_mono_strings(10).unwrap();
        let texts: Vec<&str> = fields.iter().map(|f| f.text.as_str()).collect();
        // m_Name "Naninovel" must not extract
        assert!(
            !texts.iter().any(|t| *t == "Naninovel"),
            "drop engine product m_Name: {texts:?}"
        );
        for drop in [
            "Naninovel.Commands",
            "Assembly-CSharp",
            "Highlighted",
            "Pressed",
            "Normal",
            "Disabled",
            "UnityEngine.DMAT",
            "ControlPanel.Config",
            "Master/HFX",
        ] {
            assert!(
                !texts.iter().any(|t| *t == drop),
                "expected drop {drop}: {texts:?}"
            );
        }
        assert!(
            texts.iter().any(|t| *t == "Q.SAVE"),
            "keep quick-save UI: {texts:?}"
        );
        assert!(
            texts.iter().any(|t| *t == "Play"),
            "keep short UI verb: {texts:?}"
        );
        assert!(
            texts.iter().any(|t| *t == "Save game now"),
            "keep sentence: {texts:?}"
        );
        assert!(
            texts.iter().any(|t| t.contains("replacement")),
            "keep dialogue: {texts:?}"
        );
    }

    #[test]
    fn mono_script_filter_drops_naninovel_scripts_and_lorem() {
        let bytes = write_v17_mono_fixture(
            "Holder",
            &[
                "Play",
                "@novel\n@dotween name:\"ItemList\" dir:1\n@stop",
                "@hideUI TutorialUI",
                "@else",
                "@moveMode state:\"drive\"",
                "Lorem ipsum dolor sit amet, consectetur adipiscing elit",
                "Emily",
                "Save game",
            ],
        );
        let sf = SerializedFile::parse(bytes, "nani.assets").unwrap();
        let fields = sf.read_mono_strings(10).unwrap();
        let texts: Vec<&str> = fields.iter().map(|f| f.text.as_str()).collect();
        assert!(
            texts.iter().any(|t| *t == "Play"),
            "keep UI: {texts:?}"
        );
        assert!(
            texts.iter().any(|t| *t == "Emily"),
            "keep name: {texts:?}"
        );
        assert!(
            texts.iter().any(|t| *t == "Save game"),
            "keep sentence: {texts:?}"
        );
        for drop_sub in ["@novel", "@hideUI", "@else", "@moveMode", "Lorem ipsum"] {
            assert!(
                !texts.iter().any(|t| t.contains(drop_sub)),
                "expected drop containing {drop_sub}: {texts:?}"
            );
        }
    }

    #[test]
    fn mono_script_filter_drops_hex_ids_and_component_tokens() {
        let bytes = write_v17_mono_fixture(
            "Holder",
            &[
                "72010b7a",
                "7d24045dcfc9abb4b809014e4a26b613",
                "ecbbacfc",
                "Fader",
                "Clip",
                "Canvas",
                "Sprites",
                "trigger",
                "Author Name",
                "v'",
                "Play",
                "Emily",
                "Q.SAVE",
                "Message speed:",
            ],
        );
        let sf = SerializedFile::parse(bytes, "hex.assets").unwrap();
        let fields = sf.read_mono_strings(10).unwrap();
        let texts: Vec<&str> = fields.iter().map(|f| f.text.as_str()).collect();
        for drop in [
            "72010b7a",
            "7d24045dcfc9abb4b809014e4a26b613",
            "ecbbacfc",
            "Fader",
            "Clip",
            "Canvas",
            "Sprites",
            "trigger",
            "Author Name",
            "v'",
        ] {
            assert!(
                !texts.iter().any(|t| *t == drop),
                "expected drop {drop}: {texts:?}"
            );
        }
        for keep in ["Play", "Emily", "Q.SAVE", "Message speed:"] {
            assert!(
                texts.iter().any(|t| *t == keep),
                "expected keep {keep}: {texts:?}"
            );
        }
    }

    #[test]
    fn mono_script_filter_drops_script_cmds_and_face_params() {
        let bytes = write_v17_mono_fixture(
            "Actor",
            &[
                "Gosub",
                "Goto",
                "Else",
                "EyeR Open",
                "EyeL Open",
                "Eyeball Y",
                "Eyeball X",
                "Mouth Form",
                "Mouth Open",
                "BrowL Y",
                "Brows",
                "Breath",
                "{g_saveslot}",
                "Play",
                "Wait",
                "Option A",
                "Emily",
                "Q.LOAD",
                "Message speed:",
            ],
        );
        let sf = SerializedFile::parse(bytes, "face.assets").unwrap();
        let fields = sf.read_mono_strings(10).unwrap();
        let texts: Vec<&str> = fields.iter().map(|f| f.text.as_str()).collect();
        for drop in [
            "Gosub",
            "Goto",
            "Else",
            "EyeR Open",
            "EyeL Open",
            "Eyeball Y",
            "Eyeball X",
            "Mouth Form",
            "Mouth Open",
            "BrowL Y",
            "Brows",
            "Breath",
            "{g_saveslot}",
        ] {
            assert!(
                !texts.iter().any(|t| *t == drop),
                "expected drop {drop}: {texts:?}"
            );
        }
        for keep in [
            "Play",
            "Wait",
            "Option A",
            "Emily",
            "Q.LOAD",
            "Message speed:",
        ] {
            assert!(
                texts.iter().any(|t| *t == keep),
                "expected keep {keep}: {texts:?}"
            );
        }
    }

    /// `List<string>` / `string[]`: i32 count + N aligned strings after m_Name.
    #[test]
    fn parse_mono_string_array_after_name() {
        let bytes = write_v17_mono_fixture_with_string_array(
            "MenuLabels",
            &["New Game", "Load Game", "Options"],
        );
        let sf = SerializedFile::parse(bytes, "arr.assets").unwrap();
        let fields = sf.read_mono_strings(10).unwrap();
        let texts: Vec<&str> = fields.iter().map(|f| f.text.as_str()).collect();
        assert!(
            texts.iter().any(|t| *t == "New Game"),
            "fields: {texts:?}"
        );
        assert!(
            texts.iter().any(|t| *t == "Load Game"),
            "fields: {texts:?}"
        );
        assert!(
            texts.iter().any(|t| *t == "Options"),
            "fields: {texts:?}"
        );
        // Count u32 must not appear as a garbage 3-byte "string".
        assert!(
            !texts.iter().any(|t| t.len() == 3 && t.as_bytes().iter().all(|b| *b < 0x20)),
            "array count must not be consumed as a short string: {texts:?}"
        );
    }

    #[test]
    fn rewrite_mono_string_array_element_inplace() {
        let bytes = write_v17_mono_fixture_with_string_array(
            "MenuLabels",
            &["New Game", "Load Game", "Options"],
        );
        let sf = SerializedFile::parse(bytes.clone(), "arr.assets").unwrap();
        let fields = sf.read_mono_strings(10).unwrap();
        let load = fields
            .iter()
            .find(|f| f.text == "Load Game")
            .expect("Load Game present");
        let mut file = bytes;
        rewrite_text_asset_script_inplace(
            &mut file,
            load.len_offset,
            load.byte_len,
            "Cargar",
            "arr.assets",
        )
        .unwrap();
        let again = SerializedFile::parse(file, "arr.assets").unwrap();
        let fields2 = again.read_mono_strings(10).unwrap();
        assert!(
            fields2.iter().any(|f| f.text.starts_with("Cargar")),
            "array element inject: {fields2:?}"
        );
        assert!(
            fields2.iter().any(|f| f.text == "New Game"),
            "sibling array element must remain: {fields2:?}"
        );
        assert!(
            fields2.iter().any(|f| f.text == "Options"),
            "sibling array element must remain: {fields2:?}"
        );
    }

    /// Real short string (length 3) must not be misread as array count 3.
    #[test]
    fn parse_mono_short_string_not_array_count() {
        let bytes = write_v17_mono_fixture("Box", &["Yes", "See you later."]);
        let sf = SerializedFile::parse(bytes, "short.assets").unwrap();
        let fields = sf.read_mono_strings(10).unwrap();
        let texts: Vec<&str> = fields.iter().map(|f| f.text.as_str()).collect();
        assert!(
            texts.iter().any(|t| *t == "Yes"),
            "short string must extract as itself: {texts:?}"
        );
        assert!(
            texts.iter().any(|t| *t == "See you later."),
            "following string must still extract: {texts:?}"
        );
    }
}

/// MonoBehaviour: m_Name + i32 count + N aligned strings (`string[]` / `List<string>`).
#[cfg(test)]
pub fn write_v17_mono_fixture_with_string_array(mono_name: &str, items: &[&str]) -> Vec<u8> {
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
    write_pptr(&mut payload, 0, 0);
    payload.push(1);
    while payload.len() % 4 != 0 {
        payload.push(0);
    }
    write_pptr(&mut payload, 0, 0);
    write_aligned_string(&mut payload, mono_name);
    payload.extend_from_slice(&(items.len() as i32).to_le_bytes());
    for s in items {
        write_aligned_string(&mut payload, s);
    }

    let mut meta = Vec::new();
    meta.extend_from_slice(b"2019.4.0f1\0");
    meta.extend_from_slice(&1u32.to_le_bytes());
    meta.push(0);
    meta.extend_from_slice(&1i32.to_le_bytes());
    meta.extend_from_slice(&CLASS_ID_MONO_BEHAVIOUR.to_le_bytes());
    meta.push(0);
    meta.extend_from_slice(&0i16.to_le_bytes());
    meta.extend_from_slice(&[0u8; 16]);
    meta.extend_from_slice(&[0u8; 16]);
    meta.extend_from_slice(&1i32.to_le_bytes());
    while meta.len() % 4 != 0 {
        meta.push(0);
    }
    meta.extend_from_slice(&10i64.to_le_bytes());
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

/// v17 fixture: one TextMesh (class 141) with `m_Text` only (minimal tail).
#[cfg(test)]
pub fn write_v17_textmesh_fixture(m_text: &str) -> Vec<u8> {
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
    write_pptr(&mut payload, 0, 1); // m_GameObject
    write_aligned_string(&mut payload, m_text);
    // Minimal remainder so byte_size covers a realistic object (floats after m_Text).
    payload.extend_from_slice(&0f32.to_le_bytes()); // m_OffsetZ
    payload.extend_from_slice(&1f32.to_le_bytes()); // m_CharacterSize

    let mut meta = Vec::new();
    meta.extend_from_slice(b"2019.4.0f1\0");
    meta.extend_from_slice(&1u32.to_le_bytes());
    meta.push(0); // no type tree
    meta.extend_from_slice(&1i32.to_le_bytes());
    meta.extend_from_slice(&CLASS_ID_TEXT_MESH.to_le_bytes());
    meta.push(0);
    meta.extend_from_slice(&(-1i16).to_le_bytes());
    meta.extend_from_slice(&[0u8; 16]); // old_type_hash
    meta.extend_from_slice(&1i32.to_le_bytes());
    while meta.len() % 4 != 0 {
        meta.push(0);
    }
    meta.extend_from_slice(&7i64.to_le_bytes()); // path_id
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

/// Two TextMesh objects with the **same** `m_Text` (distinct path_ids 7 and 8).
/// Used to prove structural extract keeps both instances for inject.
#[cfg(test)]
pub fn write_v17_dual_textmesh_same_text(m_text: &str) -> Vec<u8> {
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
    fn textmesh_payload(m_text: &str) -> Vec<u8> {
        let mut payload = Vec::new();
        write_pptr(&mut payload, 0, 1);
        write_aligned_string(&mut payload, m_text);
        payload.extend_from_slice(&0f32.to_le_bytes());
        payload.extend_from_slice(&1f32.to_le_bytes());
        payload
    }

    let p0 = textmesh_payload(m_text);
    let p1 = textmesh_payload(m_text);
    let p0_len = p0.len() as u32;
    let p1_len = p1.len() as u32;

    let mut meta = Vec::new();
    meta.extend_from_slice(b"2019.4.0f1\0");
    meta.extend_from_slice(&1u32.to_le_bytes());
    meta.push(0);
    meta.extend_from_slice(&1i32.to_le_bytes()); // 1 type
    meta.extend_from_slice(&CLASS_ID_TEXT_MESH.to_le_bytes());
    meta.push(0);
    meta.extend_from_slice(&(-1i16).to_le_bytes());
    meta.extend_from_slice(&[0u8; 16]);
    meta.extend_from_slice(&2i32.to_le_bytes()); // 2 objects
    while meta.len() % 4 != 0 {
        meta.push(0);
    }
    // obj0 path_id=7 byte_start=0
    meta.extend_from_slice(&7i64.to_le_bytes());
    meta.extend_from_slice(&0u32.to_le_bytes());
    meta.extend_from_slice(&p0_len.to_le_bytes());
    meta.extend_from_slice(&0i32.to_le_bytes());
    while meta.len() % 4 != 0 {
        meta.push(0);
    }
    // obj1 path_id=8 byte_start after p0
    meta.extend_from_slice(&8i64.to_le_bytes());
    meta.extend_from_slice(&p0_len.to_le_bytes());
    meta.extend_from_slice(&p1_len.to_le_bytes());
    meta.extend_from_slice(&0i32.to_le_bytes());

    let header_len = 20usize;
    let mut data_offset = header_len + meta.len();
    data_offset = (data_offset + 15) & !15;
    let file_size = data_offset + p0.len() + p1.len();

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
    out.extend_from_slice(&p0);
    out.extend_from_slice(&p1);
    out
}

/// v17 fixture: one GUIText (class 132) with Behaviour base + m_PixelOffset + m_Text.
#[cfg(test)]
pub fn write_v17_guitext_fixture(m_text: &str) -> Vec<u8> {
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
    write_pptr(&mut payload, 0, 1); // m_GameObject
    payload.push(1); // m_Enabled
    while payload.len() % 4 != 0 {
        payload.push(0);
    }
    payload.extend_from_slice(&0f32.to_le_bytes()); // m_PixelOffset.x
    payload.extend_from_slice(&0f32.to_le_bytes()); // m_PixelOffset.y
    write_aligned_string(&mut payload, m_text);
    // Minimal tail (anchor / alignment)
    payload.extend_from_slice(&0i32.to_le_bytes());
    payload.extend_from_slice(&0i32.to_le_bytes());

    let mut meta = Vec::new();
    meta.extend_from_slice(b"2019.4.0f1\0");
    meta.extend_from_slice(&1u32.to_le_bytes());
    meta.push(0);
    meta.extend_from_slice(&1i32.to_le_bytes());
    meta.extend_from_slice(&CLASS_ID_GUI_TEXT.to_le_bytes());
    meta.push(0);
    meta.extend_from_slice(&(-1i16).to_le_bytes());
    meta.extend_from_slice(&[0u8; 16]);
    meta.extend_from_slice(&1i32.to_le_bytes());
    while meta.len() % 4 != 0 {
        meta.push(0);
    }
    meta.extend_from_slice(&8i64.to_le_bytes()); // path_id
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

/// v22 fixture: one TextAsset with LargeFilesSupport header (u64 sizes + byte_start).
#[cfg(test)]
pub fn write_v22_textasset_fixture(text_name: &str, text_script: &str) -> Vec<u8> {
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
    meta.extend_from_slice(b"2021.3.0f1\0");
    meta.extend_from_slice(&1u32.to_le_bytes()); // target platform
    meta.push(0); // no type tree
    meta.extend_from_slice(&1i32.to_le_bytes()); // 1 type
    meta.extend_from_slice(&CLASS_ID_TEXT_ASSET.to_le_bytes());
    meta.push(0); // not stripped
    meta.extend_from_slice(&(-1i16).to_le_bytes());
    meta.extend_from_slice(&[0u8; 16]); // old_type_hash
    meta.extend_from_slice(&1i32.to_le_bytes()); // 1 object
    while meta.len() % 4 != 0 {
        meta.push(0);
    }
    // Object: path_id i64, byte_start u64 (v22), byte_size u32, type_id i32
    meta.extend_from_slice(&1i64.to_le_bytes());
    meta.extend_from_slice(&0u64.to_le_bytes()); // byte_start
    meta.extend_from_slice(&(text_payload.len() as u32).to_le_bytes());
    meta.extend_from_slice(&0i32.to_le_bytes());

    // Classic header (20) + extended (4+8+8+8=28) = 48 bytes before metadata.
    let classic_header = 20usize;
    let extended = 28usize;
    let header_and_ext = classic_header + extended;
    let mut data_offset = header_and_ext + meta.len();
    data_offset = (data_offset + 15) & !15;
    let file_size = (data_offset + text_payload.len()) as u64;
    let metadata_size = meta.len() as u32;

    let mut out = Vec::new();
    // Classic BE header (placeholders; real sizes come from extended block).
    out.extend_from_slice(&0u32.to_be_bytes()); // metadata_size stub
    out.extend_from_slice(&0u32.to_be_bytes()); // file_size stub
    out.extend_from_slice(&22u32.to_be_bytes()); // version
    out.extend_from_slice(&0u32.to_be_bytes()); // data_offset stub
    out.push(0); // little-endian metadata
    out.extend_from_slice(&[0, 0, 0]);
    // Extended LargeFilesSupport header (still big-endian).
    out.extend_from_slice(&metadata_size.to_be_bytes());
    out.extend_from_slice(&file_size.to_be_bytes());
    out.extend_from_slice(&(data_offset as u64).to_be_bytes());
    out.extend_from_slice(&0u64.to_be_bytes()); // unknown
    out.extend_from_slice(&meta);
    while out.len() < data_offset {
        out.push(0);
    }
    out.extend_from_slice(&text_payload);
    out
}

/// v17 fixture: one TextAsset, enable_type_tree=1 with a minimal skippable blob.
#[cfg(test)]
pub fn write_v17_fixture_with_type_tree(text_name: &str, text_script: &str) -> Vec<u8> {
    write_typed_textasset_with_type_tree(17, 24, text_name, text_script, Endian::Little)
}

/// v19 fixture: type-tree nodes are 32 bytes (+ optional string buffer).
#[cfg(test)]
pub fn write_v19_fixture_with_type_tree(text_name: &str, text_script: &str) -> Vec<u8> {
    write_typed_textasset_with_type_tree(19, 32, text_name, text_script, Endian::Little)
}

/// Big-endian v17 TextAsset (metadata + object payload use BE length prefixes).
#[cfg(test)]
pub fn write_v17_be_textasset_fixture(text_name: &str, text_script: &str) -> Vec<u8> {
    write_typed_textasset_with_type_tree(17, 0, text_name, text_script, Endian::Big)
}

/// Shared TextAsset fixture writer.
/// `type_tree_node_size` 0 = no type tree; else enable_type_tree with one node of that size.
#[cfg(test)]
fn write_typed_textasset_with_type_tree(
    version: u32,
    type_tree_node_size: usize,
    text_name: &str,
    text_script: &str,
    data_endian: Endian,
) -> Vec<u8> {
    fn align4(n: usize) -> usize {
        (n + 3) & !3
    }
    fn write_u32(buf: &mut Vec<u8>, v: u32, endian: Endian) {
        match endian {
            Endian::Little => buf.extend_from_slice(&v.to_le_bytes()),
            Endian::Big => buf.extend_from_slice(&v.to_be_bytes()),
        }
    }
    fn write_i32(buf: &mut Vec<u8>, v: i32, endian: Endian) {
        write_u32(buf, v as u32, endian);
    }
    fn write_i64(buf: &mut Vec<u8>, v: i64, endian: Endian) {
        match endian {
            Endian::Little => buf.extend_from_slice(&v.to_le_bytes()),
            Endian::Big => buf.extend_from_slice(&v.to_be_bytes()),
        }
    }
    fn write_i16(buf: &mut Vec<u8>, v: i16, endian: Endian) {
        match endian {
            Endian::Little => buf.extend_from_slice(&v.to_le_bytes()),
            Endian::Big => buf.extend_from_slice(&v.to_be_bytes()),
        }
    }
    fn write_aligned_string(buf: &mut Vec<u8>, s: &str, endian: Endian) {
        let b = s.as_bytes();
        write_u32(buf, b.len() as u32, endian);
        buf.extend_from_slice(b);
        let pad = align4(b.len()) - b.len();
        buf.extend(std::iter::repeat_n(0u8, pad));
    }

    let mut text_payload = Vec::new();
    write_aligned_string(&mut text_payload, text_name, data_endian);
    write_aligned_string(&mut text_payload, text_script, data_endian);

    let mut meta = Vec::new();
    meta.extend_from_slice(b"2019.4.0f1\0");
    write_u32(&mut meta, 1, data_endian); // target platform
    let enable_tt = type_tree_node_size > 0;
    meta.push(if enable_tt { 1 } else { 0 });

    write_i32(&mut meta, 1, data_endian); // 1 type
    write_i32(&mut meta, CLASS_ID_TEXT_ASSET, data_endian);
    meta.push(0);
    write_i16(&mut meta, -1, data_endian);
    meta.extend_from_slice(&[0u8; 16]); // old_type_hash
    if enable_tt {
        write_i32(&mut meta, 1, data_endian); // node count
        // Non-empty string buffer exercises skip of both nodes and buffer.
        let str_buf = b"m_Name\0m_Script\0";
        write_i32(&mut meta, str_buf.len() as i32, data_endian);
        meta.extend(std::iter::repeat_n(0u8, type_tree_node_size));
        meta.extend_from_slice(str_buf);
    }

    write_i32(&mut meta, 1, data_endian); // 1 object
    while meta.len() % 4 != 0 {
        meta.push(0);
    }
    write_i64(&mut meta, 1, data_endian); // path_id
    write_u32(&mut meta, 0, data_endian); // byte_start
    write_u32(&mut meta, text_payload.len() as u32, data_endian);
    write_i32(&mut meta, 0, data_endian); // type index

    let header_len = 20usize;
    let mut data_offset = header_len + meta.len();
    data_offset = (data_offset + 15) & !15;
    let file_size = data_offset + text_payload.len();

    let mut out = Vec::new();
    out.extend_from_slice(&(meta.len() as u32).to_be_bytes());
    out.extend_from_slice(&(file_size as u32).to_be_bytes());
    out.extend_from_slice(&version.to_be_bytes());
    out.extend_from_slice(&(data_offset as u32).to_be_bytes());
    out.push(match data_endian {
        Endian::Little => 0,
        Endian::Big => 1,
    });
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

/// MonoBehaviour payload: m_Name + string_a + raw u32 gap + string_b.
#[cfg(test)]
pub fn write_v17_mono_fixture_with_int_gap(
    mono_name: &str,
    first: &str,
    gap_u32: u32,
    second: &str,
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
    write_pptr(&mut payload, 0, 0);
    payload.push(1);
    while payload.len() % 4 != 0 {
        payload.push(0);
    }
    write_pptr(&mut payload, 0, 0);
    write_aligned_string(&mut payload, mono_name);
    write_aligned_string(&mut payload, first);
    payload.extend_from_slice(&gap_u32.to_le_bytes());
    write_aligned_string(&mut payload, second);

    let mut meta = Vec::new();
    meta.extend_from_slice(b"2019.4.0f1\0");
    meta.extend_from_slice(&1u32.to_le_bytes());
    meta.push(0);
    meta.extend_from_slice(&1i32.to_le_bytes());
    meta.extend_from_slice(&CLASS_ID_MONO_BEHAVIOUR.to_le_bytes());
    meta.push(0);
    meta.extend_from_slice(&0i16.to_le_bytes());
    meta.extend_from_slice(&[0u8; 16]);
    meta.extend_from_slice(&[0u8; 16]);
    meta.extend_from_slice(&1i32.to_le_bytes());
    while meta.len() % 4 != 0 {
        meta.push(0);
    }
    meta.extend_from_slice(&10i64.to_le_bytes());
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

/// v17: TextAsset + MonoScript (class 115). MonoScript body holds a type-name
/// string that must be covered by [`SerializedFile::heuristic_skip_byte_ranges`].
#[cfg(test)]
pub fn write_v17_monoscript_noise_fixture() -> Vec<u8> {
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
    write_aligned_string(&mut text_payload, "Note");
    write_aligned_string(&mut text_payload, "Hello traveler welcome!");

    // Fake MonoScript-ish aligned strings (class names flood heuristics).
    let mut ms_payload = Vec::new();
    write_aligned_string(&mut ms_payload, "Naninovel");
    write_aligned_string(&mut ms_payload, "QuaternionTween");

    let mut meta = Vec::new();
    meta.extend_from_slice(b"2019.4.0f1\0");
    meta.extend_from_slice(&1u32.to_le_bytes());
    meta.push(0); // enable_type_tree
    meta.extend_from_slice(&2i32.to_le_bytes()); // types
    // type 0: TextAsset
    meta.extend_from_slice(&CLASS_ID_TEXT_ASSET.to_le_bytes());
    meta.push(0);
    meta.extend_from_slice(&(-1i16).to_le_bytes());
    meta.extend_from_slice(&[0u8; 16]);
    // type 1: MonoScript
    meta.extend_from_slice(&CLASS_ID_MONO_SCRIPT.to_le_bytes());
    meta.push(0);
    meta.extend_from_slice(&(-1i16).to_le_bytes());
    meta.extend_from_slice(&[0u8; 16]);

    meta.extend_from_slice(&2i32.to_le_bytes()); // objects
    while meta.len() % 4 != 0 {
        meta.push(0);
    }
    meta.extend_from_slice(&1i64.to_le_bytes()); // path_id TextAsset
    meta.extend_from_slice(&0u32.to_le_bytes());
    meta.extend_from_slice(&(text_payload.len() as u32).to_le_bytes());
    meta.extend_from_slice(&0i32.to_le_bytes());

    while meta.len() % 4 != 0 {
        meta.push(0);
    }
    meta.extend_from_slice(&2i64.to_le_bytes()); // path_id MonoScript
    meta.extend_from_slice(&(text_payload.len() as u32).to_le_bytes());
    meta.extend_from_slice(&(ms_payload.len() as u32).to_le_bytes());
    meta.extend_from_slice(&1i32.to_le_bytes());

    let header_len = 20usize;
    let mut data_offset = header_len + meta.len();
    data_offset = (data_offset + 15) & !15;
    let file_size = data_offset + text_payload.len() + ms_payload.len();

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
    out.extend_from_slice(&ms_payload);
    out
}

