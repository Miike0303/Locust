//! Unity SerializedFile container — slices 1–2:
//! - **Slice 1:** header, type table, object table, TextAsset (class_id 49)
//!   `m_Name` / `m_Script` reads + validated variable-length reconstruction.
//!   Legacy fixed-slot rewrite remains available (payload pad with `0x20`;
//!   length-prefix u32 left byte-identical so BE assets stay valid).
//! - **Slice 2:** bounded type-tree field layouts, MonoBehaviour
//!   (class_id 114 **or negative** script-type ids) base layout (`m_GameObject`,
//!   `m_Enabled`, `m_Script`, `m_Name`) plus sequential aligned-string fields
//!   after the base for extract/in-place rewrite (also recovers Unity `string[]` /
//!   `List<string>` as i32 count + N aligned strings when count is small);
//!   **TextMesh** (class_id 141) `m_Text` after `m_GameObject` PPtr; **GUIText**
//!   (class_id 132) `m_Text` after Behaviour base + `m_PixelOffset`. Heuristic
//!   scan also skips MonoScript (115), Shader (48) and InputManager (13)
//!   ranges (type names, HLSL and input bindings). Supported UI schemas identify
//!   display fields separately from event callbacks and resource identifiers.
//!
//! # Format (AssetStudio / AssetsTools.NET conventions)
//! Header fields through `data_offset` are **big-endian**. From version ≥ 9 the
//! endianness byte selects the endianness of metadata + object data (usually
//! little). Version ≥ 22 (LargeFilesSupport) extends the header with u64
//! file_size / data_offset.
//!
//! Metadata (file endian): unity version c-string, target platform u32,
//! enable_type_tree bool, type count, then per-type class_id and script hashes.
//! When `enable_type_tree` is set, the node table and string buffer supply
//! bounded field layouts. Unsupported schemas remain opaque.
//!
//! Object table (v≥16): count i32; each object 4-aligned: path_id i64,
//! byte_start u32 (u64 when v≥22), byte_size u32, type_id i32 (index into types).
//!
//! TextAsset object body: aligned string `m_Name`, aligned string `m_Script`
//! (u32 length + bytes + pad to 4).
//!
//! MonoBehaviour object body (release, v≥14 path IDs): PPtr `m_GameObject`,
//! u8 `m_Enabled` + align4, PPtr `m_Script`, aligned string `m_Name`, then
//! script-defined fields. UI fields use complete in-file trees or verified
//! MonoScript properties-hash layouts; unknown stripped classes retain the
//! conservative sequential-string fallback.

use std::path::Path;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex, OnceLock},
};

#[cfg(test)]
std::thread_local! {
    /// Object-table entries examined by the MonoBehaviour/TextMesh/GUIText wrappers.
    pub(crate) static PATH_ID_LOOKUP_STEPS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Unity class ID for TextAsset.
pub const CLASS_ID_TEXT_ASSET: i32 = 49;
/// Unity class ID for Shader (HLSL source — not player-facing text).
pub const CLASS_ID_SHADER: i32 = 48;
/// InputManager stores control/axis identifiers, not localized UI labels.
pub const CLASS_ID_INPUT_MANAGER: i32 = 13;
/// Tag/layer and shader registry identifiers are runtime configuration.
pub const CLASS_ID_TAG_MANAGER: i32 = 78;
pub const CLASS_ID_SHADER_NAME_REGISTRY: i32 = 94;
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
/// Shared with core's explicit serialized-v1 capability contract.
pub const MAX_REBUILT_TEXT_ASSET_BYTES: usize = 1024 * 1024;
const MAX_REBUILT_FILE_BYTES: usize = 1024 * 1024 * 1024;

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
    /// Absolute metadata position of byte_start followed by byte_size.
    pub table_offset: usize,
}

#[derive(Debug, Clone)]
pub struct TextAssetData {
    pub path_id: i64,
    pub name: String,
    pub script: String,
    /// Absolute file offset of the `m_Script` length prefix (u32).
    pub script_len_offset: usize,
    /// Original `m_Script` string byte length (not including length prefix / align).
    /// Loc-line and CSV cells extracted from this asset share this whole-blob
    /// capacity; they must not be given per-cell budgets.
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
    type_layouts: Vec<Option<Vec<LayoutOp>>>,
    has_type_tree: bool,
    externals: Vec<String>,
    local_scripts: OnceLock<HashMap<i64, MonoScriptIdentity>>,
    external_scripts: Vec<OnceLock<Arc<HashMap<i64, MonoScriptIdentity>>>>,
}

#[derive(Debug)]
struct BundleScriptCache {
    path: std::path::PathBuf,
    stamp: (u64, std::time::SystemTime),
    nodes: HashMap<String, Arc<HashMap<i64, MonoScriptIdentity>>>,
}

// Cache only script identities, never object payloads or translated strings.
// Multiple SerializedFiles in a player bundle share the same script metadata.
static BUNDLE_SCRIPT_CACHE: OnceLock<Mutex<Vec<BundleScriptCache>>> = OnceLock::new();

#[derive(Debug)]
struct MonoScriptIdentity {
    class: String,
    namespace: String,
    assembly: String,
    properties_hash: [u8; 16],
}

impl MonoScriptIdentity {
    fn ui_kind(&self) -> Option<UiKind> {
        let assembly = self.assembly.trim_end_matches(".dll");
        match (self.namespace.as_str(), self.class.as_str(), assembly) {
            ("TMPro", "TextMeshPro" | "TextMeshProUGUI", "Unity.TextMeshPro") => Some(UiKind::Tmp),
            ("UnityEngine.UI", "Text", "UnityEngine.UI") => Some(UiKind::Text),
            ("TMPro", "TMP_Dropdown", "Unity.TextMeshPro")
            | ("UnityEngine.UI", "Dropdown", "UnityEngine.UI") => Some(UiKind::Dropdown),
            ("Naninovel", "ManagedTextProvider", "Elringus.Naninovel.Runtime") => {
                Some(UiKind::Managed)
            }
            ("", "DialogButton", "Assembly-CSharp") => Some(UiKind::Dialog),
            ("", "TooltipTargetUI", "Assembly-CSharp") => Some(UiKind::Tooltip),
            _ => None,
        }
    }
    fn is_text_renderer(&self) -> bool {
        (self.namespace == "TMPro"
            && matches!(self.class.as_str(), "TextMeshPro" | "TextMeshProUGUI"))
            || (self.namespace == "UnityEngine.UI" && self.class == "Text")
    }

    fn has_technical_object_name(&self) -> bool {
        let assembly = self.assembly.trim_end_matches(".dll");
        self.ui_kind().is_some()
            || self.is_text_renderer()
            || self.namespace == "UnityEngine.Rendering"
            || self.namespace.starts_with("UnityEngine.Rendering.")
            || (self.namespace == "UnityEngine.InputSystem" && self.class == "InputActionAsset")
            || (self.namespace == "UnityEngine.Tilemaps" && self.class == "Tile")
            || self.namespace.starts_with("Live2D.Cubism.")
            || (self.namespace == "TMPro"
                && matches!(
                    self.class.as_str(),
                    "TMP_FontAsset" | "TMP_SpriteAsset" | "TMP_StyleSheet" | "TMP_Settings"
                ))
            || ((self.namespace == "Naninovel" || self.namespace.starts_with("Naninovel."))
                && (self.class.ends_with("Configuration")
                    || matches!(
                        self.class.as_str(),
                        "Script" | "ScriptAsset" | "ProjectResources" | "EngineVersion"
                    )))
            || (self.namespace == "DG.Tweening.Core"
                && self.class == "DOTweenSettings"
                && assembly == "DOTween")
            || (self.namespace == "BlendModes" && self.class == "ShaderResources")
            || (self.namespace == "FunkyCode.LightingSettings"
                && matches!(self.class.as_str(), "Profile" | "ProjectSettings"))
    }
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
            Endian::Little => u64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]),
            Endian::Big => u64::from_be_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]),
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
    pub fn parse(
        data: Vec<u8>,
        path: impl Into<std::path::PathBuf>,
    ) -> Result<Self, SerializedError> {
        let mut parsed = Self::parse_metadata(&data, path.into())?;
        parsed.data = data;
        Ok(parsed)
    }

    // Also used by the fixed-slot writer without cloning the complete file.
    fn parse_metadata(data: &[u8], path: std::path::PathBuf) -> Result<Self, SerializedError> {
        let label = path.display().to_string();
        if data.len() < 20 {
            return Err(err(&label, "file too small for SerializedFile header"));
        }

        // Header is big-endian until endianness is known.
        let mut hr = R {
            data,
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

        if file_size != data.len() as u64
            || data_offset < hr.pos as u64
            || data_offset > file_size
            || endian_byte > 1
        {
            return Err(err(
                &label,
                "invalid SerializedFile header bounds or endianness",
            ));
        }
        let metadata_end = hr
            .pos
            .checked_add(metadata_size as usize)
            .filter(|&end| end <= data_offset as usize)
            .ok_or_else(|| err(&label, "metadata overlaps object data"))?;

        // Metadata uses file endian.
        let mut r = R {
            data: &data[..metadata_end],
            pos: hr.pos,
            file: &label,
            endian,
        };

        let unity_version = r.cstring()?;
        let _target_platform = r.u32()?;
        let enable_type_tree = r.u8()? != 0;

        let type_count = r.i32()?;
        if !(0..=100_000).contains(&type_count) {
            return Err(err(&label, format!("implausible type count {type_count}")));
        }
        r.need(type_count as usize * 23)?;
        let mut types = Vec::with_capacity(type_count as usize);
        let mut type_layouts = Vec::with_capacity(type_count as usize);
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
            let mut layout = None;
            if enable_type_tree {
                layout = read_type_tree_layout(&mut r, version)?;
                if version >= 21 {
                    let count = r.i32()?;
                    if count < 0 {
                        return Err(err(&label, "negative type dependency count"));
                    }
                    let bytes = (count as usize)
                        .checked_mul(4)
                        .ok_or_else(|| err(&label, "type dependency size overflow"))?;
                    r.take(bytes)?;
                }
            }
            types.push(SerializedType {
                class_id,
                is_stripped,
                script_type_index,
            });
            type_layouts.push(layout);
        }

        // Object table (v >= 14 uses i64 path_id; v >= 16 type_id is type index)
        let object_count = r.i32()?;
        if !(0..=5_000_000).contains(&object_count) {
            return Err(err(
                &label,
                format!("implausible object count {object_count}"),
            ));
        }
        r.need(object_count as usize * if version >= 22 { 24 } else { 20 })?;
        let mut objects = Vec::with_capacity(object_count as usize);
        for _ in 0..object_count {
            r.align4();
            let path_id = r.i64()?;
            let table_offset = r.pos;
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
                    format!(
                        "object type_id {type_id} out of bounds ({} types)",
                        types.len()
                    ),
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
                table_offset,
            });
        }

        // Older synthetic/stripped files may omit the optional reference tables.
        // An unreadable reference leaves the owning script unresolved; it must
        // never turn an unknown custom asset name into a technical name.
        let externals = read_external_paths(&mut r).unwrap_or_default();
        let external_scripts = (0..externals.len()).map(|_| OnceLock::new()).collect();
        Ok(Self {
            path,
            header,
            unity_version,
            types,
            objects,
            data: Vec::new(),
            type_layouts,
            has_type_tree: enable_type_tree,
            externals,
            local_scripts: OnceLock::new(),
            external_scripts,
        })
    }

    fn script_identity(
        &self,
        data: &[u8],
        file_id: i32,
        path_id: i64,
    ) -> Option<&MonoScriptIdentity> {
        if file_id == 0 {
            self.local_scripts
                .get_or_init(|| self.read_script_identities(data))
                .get(&path_id)
        } else {
            let index = usize::try_from(file_id).ok()?.checked_sub(1)?;
            self.external_scripts
                .get(index)?
                .get_or_init(|| self.read_external_scripts(index).unwrap_or_default())
                .get(&path_id)
        }
    }

    fn read_script_identities(&self, data: &[u8]) -> HashMap<i64, MonoScriptIdentity> {
        let label = self.path.display().to_string();
        self.objects
            .iter()
            .filter(|obj| obj.class_id == CLASS_ID_MONO_SCRIPT)
            .filter_map(|obj| {
                let end = (obj.data_abs as usize).checked_add(obj.byte_size as usize)?;
                let mut r = R {
                    data: data.get(..end)?,
                    pos: obj.data_abs as usize,
                    file: &label,
                    endian: self.header.endian,
                };
                r.aligned_string().ok()?; // m_Name
                r.i32().ok()?; // m_ExecutionOrder
                let properties_hash = r.take(16).ok()?.try_into().ok()?;
                let (class, _, _) = r.aligned_string().ok()?;
                let (namespace, _, _) = r.aligned_string().ok()?;
                let (assembly, _, _) = r.aligned_string().ok()?;
                if class.is_empty()
                    || assembly.is_empty()
                    || [&class, &namespace, &assembly]
                        .iter()
                        .any(|s| s.contains('\u{FFFD}') || s.chars().any(char::is_control))
                {
                    return None;
                }
                Some((
                    obj.path_id,
                    MonoScriptIdentity {
                        class,
                        namespace,
                        assembly,
                        properties_hash,
                    },
                ))
            })
            .collect()
    }

    fn read_external_scripts(&self, index: usize) -> Option<Arc<HashMap<i64, MonoScriptIdentity>>> {
        // Injection labels use "bundle / node"; extraction uses bundle.join(node).
        let label = self.path.to_string_lossy();
        let path = label.split_once(" / ").map_or_else(
            || self.path.clone(),
            |(bundle, node)| Path::new(bundle).join(node),
        );
        let bundle = path.ancestors().skip(1).find(|p| p.is_file());
        let parent = bundle
            .and_then(Path::parent)
            .or_else(|| path.parent())
            .unwrap_or_else(|| Path::new(""));
        let normalized = self.externals.get(index)?.replace('\\', "/");
        let name = normalized.rsplit('/').next()?;
        if name.is_empty() || name == "." || name == ".." {
            return None;
        }
        if let Some(bundle) = bundle {
            if let Some(scripts) = Self::bundle_script_identities(bundle, &normalized, name) {
                return Some(scripts);
            }
        }
        // Unity's Library/Resources builtins ship under *_Data/Resources.
        let external_path = [parent.join(name), parent.join("Resources").join(name)]
            .into_iter()
            .find(|p| p.is_file())?;
        let bytes = std::fs::read(&external_path).ok()?;
        let sf = Self::parse_metadata(&bytes, external_path).ok()?;
        Some(Arc::new(sf.read_script_identities(&bytes)))
    }

    fn bundle_script_identities(
        path: &Path,
        node_path: &str,
        name: &str,
    ) -> Option<Arc<HashMap<i64, MonoScriptIdentity>>> {
        let metadata = path.metadata().ok()?;
        let stamp = (metadata.len(), metadata.modified().ok()?);
        let mut cache = BUNDLE_SCRIPT_CACHE
            .get_or_init(|| Mutex::new(Vec::new()))
            .lock()
            .ok()?;
        if let Some(entry) = cache.iter().find(|e| e.path == path && e.stamp == stamp) {
            return entry
                .nodes
                .get(node_path)
                .or_else(|| entry.nodes.get(name))
                .cloned();
        }
        let archive = crate::unity_fs::UnityFsArchive::parse_path(path).ok()?;
        let nodes = archive
            .nodes
            .iter()
            .filter_map(|node| {
                let bytes = archive.node_bytes(node).ok()?;
                let sf = Self::parse_metadata(bytes, path.join(&node.path)).ok()?;
                Some((
                    node.path.replace('\\', "/"),
                    Arc::new(sf.read_script_identities(bytes)),
                ))
            })
            .collect();
        cache.retain(|e| e.path != path);
        if cache.len() >= 4 {
            cache.remove(0);
        }
        cache.push(BundleScriptCache {
            path: path.into(),
            stamp,
            nodes,
        });
        let entry = cache.last()?;
        entry
            .nodes
            .get(node_path)
            .or_else(|| entry.nodes.get(name))
            .cloned()
    }

    pub fn parse_path(path: &Path) -> Result<Self, SerializedError> {
        let label = path.display().to_string();
        let data = std::fs::read(path).map_err(|e| err(&label, format!("read failed: {e}")))?;
        Self::parse(data, path)
    }

    /// Check every object span before advertising structural relocation. Other
    /// objects and all uninterpreted metadata remain opaque to the writer.
    fn relocation_order(&self) -> Result<Vec<&ObjectInfo>, SerializedError> {
        let label = self.path.display().to_string();
        if self.data.len() > MAX_REBUILT_FILE_BYTES {
            return Err(err(&label, "SerializedFile exceeds 1 GiB rewrite limit"));
        }
        let mut ids = std::collections::HashSet::new();
        let mut order: Vec<_> = self.objects.iter().collect();
        order.sort_by_key(|o| (o.data_abs, o.byte_size));
        let mut cursor = self.header.data_offset;
        for obj in &order {
            if !ids.insert(obj.path_id) || obj.data_abs < cursor {
                return Err(err(
                    &label,
                    "duplicate object ID or overlapping object spans",
                ));
            }
            cursor = obj
                .data_abs
                .checked_add(u64::from(obj.byte_size))
                .filter(|&end| end <= self.data.len() as u64)
                .ok_or_else(|| err(&label, "object span exceeds SerializedFile"))?;
        }
        Ok(order)
    }

    /// Only the fully consumed release TextAsset Name+Script layout is known.
    /// Unknown trailing fields, invalid UTF-8, and oversized scripts keep their
    /// fixed-slot behavior and never receive a resize capability.
    pub fn rewriteable_text_assets(
        &self,
    ) -> Result<std::collections::BTreeMap<i64, TextAssetData>, SerializedError> {
        self.relocation_order()?;
        let mut assets = std::collections::BTreeMap::new();
        for obj in self.text_asset_objects() {
            let Ok(ta) = self.read_text_asset_object(obj) else {
                continue;
            };
            let end = ta.script_len_offset + 4 + ta.script_byte_len;
            let aligned_end = (end + 3) & !3;
            if aligned_end != obj.data_abs as usize + obj.byte_size as usize
                || ta.script_byte_len > MAX_REBUILT_TEXT_ASSET_BYTES
                || std::str::from_utf8(&self.data[ta.script_len_offset + 4..end]).is_err()
            {
                continue;
            }
            assets.insert(obj.path_id, ta);
        }
        Ok(assets)
    }

    /// Relocate a plan of TextAsset replacements in one pass. Callers must
    /// finish every edit using original byte offsets before invoking this.
    pub fn rewrite_text_assets(
        self,
        replacements: &std::collections::BTreeMap<i64, String>,
    ) -> Result<Vec<u8>, SerializedError> {
        if replacements.is_empty() {
            return Ok(self.data);
        }
        let label = self.path.display().to_string();
        let assets = self.rewriteable_text_assets()?;
        let order = self.relocation_order()?;
        let mut upper = self.data.len();
        for (id, text) in replacements {
            if !assets.contains_key(id) {
                return Err(err(
                    &label,
                    format!("TextAsset {id} has no supported complete layout"),
                ));
            }
            if text.len() > MAX_REBUILT_TEXT_ASSET_BYTES {
                return Err(err(&label, "TextAsset replacement exceeds 1 MiB"));
            }
            upper = upper
                .checked_add(text.len() + 7)
                .filter(|&n| n <= MAX_REBUILT_FILE_BYTES)
                .ok_or_else(|| err(&label, "SerializedFile rewrite exceeds 1 GiB"))?;
        }
        let mut out = Vec::with_capacity(upper);
        let mut cursor = 0usize;
        for obj in order {
            let start = obj.data_abs as usize;
            let end = start + obj.byte_size as usize;
            out.extend_from_slice(&self.data[cursor..start]);
            // Preserve the original alignment modulo 8, including unusual but
            // readable source alignment, without interpreting unknown bodies.
            let pad = (start.wrapping_sub(out.len())) & 7;
            out.resize(out.len() + pad, 0);
            let new_start = out.len();
            if let Some(text) = replacements.get(&obj.path_id) {
                let ta = &assets[&obj.path_id];
                out.extend_from_slice(&self.data[start..ta.script_len_offset]);
                let len = text.len() as u32;
                let encoded = match self.header.endian {
                    Endian::Little => len.to_le_bytes(),
                    Endian::Big => len.to_be_bytes(),
                };
                out.extend_from_slice(&encoded);
                out.extend_from_slice(text.as_bytes());
                let aligned = (out.len() + 3) & !3;
                out.resize(aligned, 0);
            } else {
                out.extend_from_slice(&self.data[start..end]);
            }
            let size = u32::try_from(out.len() - new_start)
                .map_err(|_| err(&label, "object size overflow"))?;
            let relative = (new_start as u64) - self.header.data_offset;
            let at = obj.table_offset;
            let size_at = if self.header.version >= 22 {
                let bytes = match self.header.endian {
                    Endian::Little => relative.to_le_bytes(),
                    Endian::Big => relative.to_be_bytes(),
                };
                out[at..at + 8].copy_from_slice(&bytes);
                at + 8
            } else {
                let relative =
                    u32::try_from(relative).map_err(|_| err(&label, "object offset overflow"))?;
                let bytes = match self.header.endian {
                    Endian::Little => relative.to_le_bytes(),
                    Endian::Big => relative.to_be_bytes(),
                };
                out[at..at + 4].copy_from_slice(&bytes);
                at + 4
            };
            let encoded = match self.header.endian {
                Endian::Little => size.to_le_bytes(),
                Endian::Big => size.to_be_bytes(),
            };
            out[size_at..size_at + 4].copy_from_slice(&encoded);
            cursor = end;
        }
        out.extend_from_slice(&self.data[cursor..]);
        let file_size = out.len() as u64;
        if self.header.version >= 22 {
            out[24..32].copy_from_slice(&file_size.to_be_bytes());
        } else {
            out[4..8].copy_from_slice(&(file_size as u32).to_be_bytes());
        }
        Ok(out)
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

        self.read_text_asset_object(obj)
    }

    /// Avoid a repeated whole-table search while walking TextAsset objects.
    pub(crate) fn read_text_asset_object(
        &self,
        obj: &ObjectInfo,
    ) -> Result<TextAssetData, SerializedError> {
        let label = self.path.display().to_string();

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
            path_id: obj.path_id,
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
            .find(|o| {
                #[cfg(test)]
                PATH_ID_LOOKUP_STEPS.with(|steps| steps.set(steps.get() + 1));
                o.path_id == path_id && is_monobehaviour_class(o.class_id)
            })
            .ok_or_else(|| err(&label, format!("no MonoBehaviour with path_id={path_id}")))?;

        self.read_mono_strings_object(obj)
    }

    /// Avoid a repeated whole-table search while walking MonoBehaviour objects.
    pub(crate) fn read_mono_strings_object(
        &self,
        obj: &ObjectInfo,
    ) -> Result<Vec<MonoStringData>, SerializedError> {
        let label = self.path.display().to_string();
        let path_id = obj.path_id;

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
        let script_file = r.i32()?;
        let script_path = r.i64()?;
        // m_Name
        let (mono_name, name_off, name_len) = r.aligned_string()?;
        let script = self.script_identity(&self.data, script_file, script_path);
        let text_renderer = script.is_some_and(MonoScriptIdentity::is_text_renderer);
        let font_name_range =
            script.and_then(|script| technical_font_name_range(&r, script, &mono_name));

        let mut out = Vec::new();
        // Base Object.m_Name is an asset identifier for identified engine types.
        // Unknown/custom ScriptableObjects can expose their name as a UI label.
        if !script.is_some_and(MonoScriptIdentity::has_technical_object_name)
            && mono_name_worth_extracting(&mono_name)
        {
            out.push(MonoStringData {
                path_id,
                mono_name: mono_name.clone(),
                field_index: 0,
                text: mono_name.clone(),
                len_offset: name_off,
                byte_len: name_len,
            });
        }

        // A present tree is authoritative. Stripped UI classes require a
        // verified complete layout; an unsupported variant must not fall back
        // to guessing numeric words as string lengths.
        let kind = script.and_then(MonoScriptIdentity::ui_kind);
        let layout = if self.has_type_tree {
            self.type_layouts
                .get(obj.type_index as usize)
                .and_then(Option::as_deref)
        } else {
            script
                .filter(|s| s.ui_kind().is_some())
                .and_then(stripped_layout)
        };
        if self.has_type_tree || kind.is_some() {
            if let Some(layout) = layout {
                let fields =
                    read_layout_strings(&self.data, obj, self.header.endian, &label, layout);
                if let Ok(fields) = fields {
                    for (index, field) in fields
                        .into_iter()
                        .filter(|f| f.path != "m_Name")
                        .enumerate()
                    {
                        if field_is_display(kind, &field.path, &field.text) {
                            out.push(MonoStringData {
                                path_id,
                                mono_name: mono_name.clone(),
                                field_index: index + 1,
                                text: field.text,
                                len_offset: field.offset,
                                byte_len: field.len,
                            });
                        }
                    }
                }
            }
            return Ok(out);
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
                if let Some(items) = try_read_mono_string_array(&mut r, end, len_peek as usize) {
                    non_string_skips = 0;
                    for (text, len_offset, byte_len) in items {
                        if mono_field_worth_extracting(&text, text_renderer, &mono_name)
                            && font_name_range.is_none_or(|(start, _)| start != len_offset)
                        {
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
                    if mono_field_worth_extracting(&text, text_renderer, &mono_name)
                        && font_name_range.is_none_or(|(start, _)| start != len_offset)
                    {
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
            .find(|o| {
                #[cfg(test)]
                PATH_ID_LOOKUP_STEPS.with(|steps| steps.set(steps.get() + 1));
                o.path_id == path_id && o.class_id == CLASS_ID_TEXT_MESH
            })
            .ok_or_else(|| err(&label, format!("no TextMesh with path_id={path_id}")))?;

        self.read_text_mesh_object(obj)
    }

    /// Avoid a repeated whole-table search while walking TextMesh objects.
    pub(crate) fn read_text_mesh_object(
        &self,
        obj: &ObjectInfo,
    ) -> Result<TextMeshData, SerializedError> {
        let label = self.path.display().to_string();
        let path_id = obj.path_id;

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
            .find(|o| {
                #[cfg(test)]
                PATH_ID_LOOKUP_STEPS.with(|steps| steps.set(steps.get() + 1));
                o.path_id == path_id && o.class_id == CLASS_ID_GUI_TEXT
            })
            .ok_or_else(|| err(&label, format!("no GUIText with path_id={path_id}")))?;

        self.read_gui_text_object(obj)
    }

    /// Avoid a repeated whole-table search while walking GUIText objects.
    pub(crate) fn read_gui_text_object(
        &self,
        obj: &ObjectInfo,
    ) -> Result<GuiTextData, SerializedError> {
        let label = self.path.display().to_string();
        let path_id = obj.path_id;

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

    /// Validated injection exclusions, distinct from the broader extraction
    /// skip ranges. Only the technical base name/alias of a MonoBehaviour is
    /// forbidden; its display fields remain writable.
    pub(crate) fn forbidden_injection_byte_ranges(&self) -> Vec<(usize, usize)> {
        let label = self.path.display().to_string();
        let mut ranges = Vec::new();
        for obj in &self.objects {
            let Some((start, end)) = usize::try_from(obj.data_abs).ok().and_then(|start| {
                start
                    .checked_add(obj.byte_size as usize)
                    .map(|end| (start, end))
            }) else {
                continue;
            };
            let Some(data) = self.data.get(..end) else {
                continue;
            };
            if start >= end {
                continue;
            }
            if is_heuristic_noise_class(obj.class_id) {
                ranges.push((start, end));
                continue;
            }
            if !is_monobehaviour_class(obj.class_id) {
                continue;
            }
            // Supported versions (17–22) use 64-bit PPtr path IDs. Read the
            // release base with the file's endian, bounded by this object.
            let mut r = R {
                data,
                pos: start,
                file: &label,
                endian: self.header.endian,
            };
            let base = (|| {
                r.i32()?; // m_GameObject.fileID
                r.i64()?; // m_GameObject.pathID
                r.u8()?; // m_Enabled
                r.align4();
                let file_id = r.i32()?;
                let path_id = r.i64()?;
                let (name, offset, len) = r.aligned_string()?;
                r.need(0)?; // alignment must also stay inside the object
                Ok::<_, SerializedError>((file_id, path_id, name, offset, len))
            })();
            let Ok((file_id, path_id, name, offset, len)) = base else {
                continue;
            };
            let Some(script) = self.script_identity(&self.data, file_id, path_id) else {
                continue;
            };
            if script.has_technical_object_name() {
                if let Some(name_end) = offset.checked_add(4).and_then(|n| n.checked_add(len)) {
                    if name_end <= end {
                        ranges.push((offset, name_end));
                    }
                }
            }
            if let Some(range) = technical_font_name_range(&r, script, &name) {
                ranges.push(range);
            }
        }
        ranges.sort_unstable();
        let mut merged: Vec<(usize, usize)> = Vec::with_capacity(ranges.len());
        for (start, end) in ranges {
            if let Some(last) = merged.last_mut().filter(|last| start <= last.1) {
                last.1 = last.1.max(end);
            } else {
                merged.push((start, end));
            }
        }
        merged
    }

    /// Ranges the heuristic length-prefix scan must not re-read: structural
    /// extract classes **plus** known non-text blobs (MonoScript type names,
    /// Shader source and input configuration) that contain engine identifiers.
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
/// ASCII identifiers (type names, HLSL, control bindings). The heuristic scan
/// skips their byte ranges; old heuristic injection targets are rejected too.
#[inline]
pub fn is_heuristic_noise_class(class_id: i32) -> bool {
    matches!(
        class_id,
        CLASS_ID_MONO_SCRIPT
            | CLASS_ID_SHADER
            | CLASS_ID_INPUT_MANAGER
            | CLASS_ID_TAG_MANAGER
            | CLASS_ID_SHADER_NAME_REGISTRY
    )
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum UiKind {
    Text,
    Tmp,
    Dropdown,
    Managed,
    Dialog,
    Tooltip,
}

#[derive(Debug, Clone)]
enum LayoutOp {
    Bytes(usize),
    Align,
    String(String),
    Array(Vec<LayoutOp>),
}

struct LayoutString {
    path: String,
    text: String,
    offset: usize,
    len: usize,
}

// This small schema notation is internal, immutable data: byte widths, align4,
// named strings, and repeated element layouts. Both tree and stripped readers
// execute the same bounded operations and require complete object consumption.
fn parse_layout_schema(schema: &str) -> Vec<LayoutOp> {
    fn parse(tokens: &mut std::str::SplitWhitespace<'_>) -> Vec<LayoutOp> {
        let mut out = Vec::new();
        while let Some(token) = tokens.next() {
            out.push(match token {
                "]" => break,
                "[" => LayoutOp::Array(parse(tokens)),
                "a" => LayoutOp::Align,
                s if s.starts_with("s:") => LayoutOp::String(s[2..].into()),
                n => LayoutOp::Bytes(n.parse().expect("static layout byte width")),
            });
        }
        out
    }
    parse(&mut schema.split_whitespace())
}

fn read_layout_strings(
    data: &[u8],
    obj: &ObjectInfo,
    endian: Endian,
    label: &str,
    layout: &[LayoutOp],
) -> Result<Vec<LayoutString>, SerializedError> {
    fn walk(
        r: &mut R<'_>,
        ops: &[LayoutOp],
        out: &mut Vec<LayoutString>,
        budget: &mut usize,
    ) -> Result<(), SerializedError> {
        for op in ops {
            *budget = budget
                .checked_sub(1)
                .ok_or_else(|| err(r.file, "layout operation limit"))?;
            match op {
                LayoutOp::Bytes(n) => {
                    r.take(*n)?;
                }
                LayoutOp::Align => {
                    r.align4();
                    r.need(0)?;
                }
                LayoutOp::String(path) => {
                    let (text, offset, len) = r.aligned_string()?;
                    r.need(0)?;
                    if std::str::from_utf8(&r.data[offset + 4..offset + 4 + len]).is_err() {
                        return Err(err(r.file, "invalid UTF-8 in typed string"));
                    }
                    out.push(LayoutString {
                        path: path.clone(),
                        text,
                        offset,
                        len,
                    });
                }
                LayoutOp::Array(element) => {
                    let count = r.i32()?;
                    if !(0..=100_000).contains(&count) || count as usize > r.data.len() - r.pos {
                        return Err(err(r.file, "invalid typed array count"));
                    }
                    for _ in 0..count {
                        let start = r.pos;
                        walk(r, element, out, budget)?;
                        if r.pos == start {
                            return Err(err(r.file, "zero-width array element"));
                        }
                    }
                }
            }
        }
        Ok(())
    }
    let start = obj.data_abs as usize;
    let end = start
        .checked_add(obj.byte_size as usize)
        .filter(|&end| end <= data.len())
        .ok_or_else(|| err(label, "typed object past EOF"))?;
    let mut r = R {
        data: &data[..end],
        pos: start,
        file: label,
        endian,
    };
    let mut out = Vec::new();
    walk(&mut r, layout, &mut out, &mut 1_000_000)?;
    if r.pos != end {
        return Err(err(label, "unsupported trailing typed object data"));
    }
    Ok(out)
}

fn field_is_display(kind: Option<UiKind>, path: &str, text: &str) -> bool {
    // Event arguments and animation/resource identifiers are technical even
    // when their values happen to be normal UI verbs or ordinary words.
    if path.contains(".m_PersistentCalls.")
        || path.starts_with("m_AnimationTriggers.")
        || matches!(
            path.rsplit('.').next().unwrap_or(path),
            "m_MethodName" | "TargetObjectBundle" | "TargetObjectInBundle" | "RoomName"
        )
    {
        return false;
    }
    let display = match kind {
        Some(UiKind::Text) => path == "m_Text",
        Some(UiKind::Tmp) => path == "m_text",
        Some(UiKind::Dropdown) => path == "m_Options.m_Options[].m_Text",
        Some(UiKind::Managed) => path == "defaultValue",
        Some(UiKind::Dialog) => matches!(path, "Text" | "_warningWindowLabel"),
        Some(UiKind::Tooltip) => path == "Text",
        None => return mono_script_field_worth_extracting(text),
    };
    let t = text.trim();
    display
        && !t.is_empty()
        && t.chars().any(char::is_alphabetic)
        && !t.contains('\u{FFFD}')
        && !t
            .chars()
            .any(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t'))
        && !looks_like_lorem_ipsum(t)
        && t != "Author Name"
}

/// Read the blob node table, resolving local and Unity common string offsets.
/// Unsupported node kinds/trees remain opaque rather than triggering scanning.
fn read_type_tree_layout(
    r: &mut R<'_>,
    version: u32,
) -> Result<Option<Vec<LayoutOp>>, SerializedError> {
    let node_count = r.i32()?;
    let string_buffer_size = r.i32()?;
    if !(0..=1_000_000).contains(&node_count) || !(0..=50_000_000).contains(&string_buffer_size) {
        return Err(err(r.file, "invalid type-tree blob size"));
    }
    let node_size = if version >= 19 { 32 } else { 24 };
    let nodes = r.take(node_count as usize * node_size)?;
    let strings = r.take(string_buffer_size as usize)?;
    fn word(bytes: &[u8], endian: Endian) -> u32 {
        let bytes = bytes.try_into().expect("four-byte tree word");
        match endian {
            Endian::Little => u32::from_le_bytes(bytes),
            Endian::Big => u32::from_be_bytes(bytes),
        }
    }
    fn name(offset: u32, strings: &[u8]) -> Option<&str> {
        if offset & 0x8000_0000 != 0 {
            // Only common strings needed to interpret structure/primitives.
            // Unknown leaf types fail closed; compound names are not required.
            return Some(match offset & 0x7fff_ffff {
                49 => "Array",
                55 => "Base",
                76 => "bool",
                81 => "char",
                106 => "data",
                117 => "double",
                161 => "float",
                222 => "int",
                231 => "long long",
                263 => "MonoBehaviour",
                349 => "m_Enabled",
                374 => "m_GameObject",
                427 => "m_Name",
                490 => "m_Script",
                789 => "short",
                795 => "size",
                800 => "SInt16",
                807 => "SInt32",
                814 => "SInt64",
                821 => "SInt8",
                840 => "string",
                894 => "TypelessData",
                907 => "UInt16",
                914 => "UInt32",
                921 => "UInt64",
                928 => "UInt8",
                934 => "unsigned int",
                947 => "unsigned long long",
                966 => "unsigned short",
                981 => "vector",
                // These common compound type names have explicit child nodes.
                _ => "<common compound>",
            });
        }
        let bytes = strings.get(offset as usize..)?;
        std::str::from_utf8(&bytes[..bytes.iter().position(|&b| b == 0)?]).ok()
    }
    struct Node<'a> {
        level: u8,
        ty: &'a str,
        name: &'a str,
        size: i32,
        flags: u32,
    }
    fn compile(
        nodes: &[Node<'_>],
        at: &mut usize,
        parent: &str,
        depth: usize,
    ) -> Option<Vec<LayoutOp>> {
        if depth > 64 {
            return None;
        }
        let node = nodes.get(*at)?;
        *at += 1;
        let begin = *at;
        while *at < nodes.len() && nodes[*at].level > node.level {
            *at += 1;
        }
        let end = *at;
        let path = if matches!(node.name, "Base" | "Array" | "data") {
            parent.to_string()
        } else if parent.is_empty() {
            node.name.into()
        } else {
            format!("{parent}.{}", node.name)
        };
        let mut out = if node.ty == "string" {
            vec![LayoutOp::String(path)]
        } else if node.ty == "Array" {
            if begin + 1 >= end
                || nodes[begin].name != "size"
                || nodes[begin].ty != "int"
                || nodes[begin + 1].name != "data"
            {
                return None;
            }
            let mut child = begin + 1;
            let element = compile(nodes, &mut child, &format!("{path}[]"), depth + 1)?;
            if child != end {
                return None;
            }
            vec![LayoutOp::Array(element)]
        } else if begin < end {
            let mut out = Vec::new();
            let mut child = begin;
            while child < end {
                if nodes[child].level != node.level + 1 {
                    return None;
                }
                out.extend(compile(nodes, &mut child, &path, depth + 1)?);
            }
            out
        } else {
            let size = match node.ty {
                "bool" | "char" | "UInt8" | "SInt8" => 1,
                "short" | "unsigned short" | "SInt16" | "UInt16" => 2,
                "int" | "unsigned int" | "SInt32" | "UInt32" | "float" => 4,
                "SInt64" | "UInt64" | "long long" | "unsigned long long" | "double" => 8,
                _ => return None,
            };
            if node.size != size && node.size != -1 {
                return None;
            }
            vec![LayoutOp::Bytes(size as usize)]
        };
        if node.flags & 0x4000 != 0 {
            out.push(LayoutOp::Align);
        }
        Some(out)
    }
    let parsed: Option<Vec<_>> = nodes
        .chunks_exact(node_size)
        .map(|n| {
            Some(Node {
                level: n[2],
                ty: name(word(&n[4..8], r.endian), strings)?,
                name: name(word(&n[8..12], r.endian), strings)?,
                size: word(&n[12..16], r.endian) as i32,
                flags: word(&n[20..24], r.endian),
            })
        })
        .collect();
    Ok(parsed.and_then(|nodes| {
        if nodes.first()?.level != 0 {
            return None;
        }
        let mut at = 0;
        let layout = compile(&nodes, &mut at, "", 0)?;
        (at == nodes.len()).then_some(layout)
    }))
}

// Verified release layouts from shipped MonoScript properties hashes and DLL field trees.
// A hash binds the complete schema, not just the offset of a promising string.
fn stripped_layout(script: &MonoScriptIdentity) -> Option<&'static [LayoutOp]> {
    type Profiles = HashMap<(UiKind, [u8; 16]), Vec<LayoutOp>>;
    static PROFILES: OnceLock<Profiles> = OnceLock::new();
    let profiles = PROFILES.get_or_init(|| {
        [
            // Sunkissed_windows_full/TextMeshPro: TMPro.TextMeshPro (Unity.TextMeshPro).
            ((UiKind::Tmp, [0xdb, 0x61, 0x33, 0x68, 0x75, 0x72, 0xf9, 0xdc, 0x24, 0xf1, 0xfd, 0x42, 0x38, 0xef, 0x9b, 0x83]),
             "12 a 1 a 12 a s:m_Name a 29 a 17 a [ 12 s:m_OnCullStateChanged.m_PersistentCalls.m_Calls[].m_TargetAssemblyTypeName s:m_OnCullStateChanged.m_PersistentCalls.m_Calls[].m_MethodName 16 s:m_OnCullStateChanged.m_PersistentCalls.m_Calls[].m_Arguments.m_ObjectArgumentAssemblyTypeName 8 s:m_OnCullStateChanged.m_PersistentCalls.m_Calls[].m_Arguments.m_StringArgument 1 a 4 ] a s:m_text 1 a 24 [ 12 ] a 12 [ 12 ] a 21 a 93 a 17 a 17 a 85 a [ 4 ] a 1 a 1 a 1 a 1 a 1 a 1 a 1 a 17 a 1 a 1 a 21 a 1 a 13 a 16"),
            // Sunkissed_windows_full/TextMeshProUGUI: TMPro.TextMeshProUGUI (Unity.TextMeshPro).
            ((UiKind::Tmp, [0xcf, 0x62, 0xaa, 0xd9, 0x7f, 0x6c, 0x2d, 0xd5, 0xd4, 0x82, 0xe2, 0x50, 0x9c, 0x6c, 0x95, 0x28]),
             "12 a 1 a 12 a s:m_Name a 29 a 17 a [ 12 s:m_OnCullStateChanged.m_PersistentCalls.m_Calls[].m_TargetAssemblyTypeName s:m_OnCullStateChanged.m_PersistentCalls.m_Calls[].m_MethodName 16 s:m_OnCullStateChanged.m_PersistentCalls.m_Calls[].m_Arguments.m_ObjectArgumentAssemblyTypeName 8 s:m_OnCullStateChanged.m_PersistentCalls.m_Calls[].m_Arguments.m_StringArgument 1 a 4 ] a s:m_text 1 a 24 [ 12 ] a 12 [ 12 ] a 21 a 93 a 17 a 17 a 85 a [ 4 ] a 1 a 1 a 1 a 1 a 1 a 1 a 1 a 17 a 1 a 1 a 21 a 1 a 1 a 28"),
            // Sunkissed_windows_full/TooltipTargetUI: .TooltipTargetUI (Assembly-CSharp).
            ((UiKind::Tooltip, [0x09, 0xd5, 0xdc, 0xcc, 0xc3, 0x8c, 0xb5, 0xf9, 0x76, 0xad, 0xbb, 0x80, 0x0e, 0xb1, 0xa1, 0x06]),
             "12 a 1 a 12 a s:m_Name a s:Text"),
            // CCTV_USSR_WINDOWS_FULL/TMP_Dropdown: TMPro.TMP_Dropdown (Unity.TextMeshPro).
            ((UiKind::Dropdown, [0xe1, 0x8d, 0xfd, 0x2b, 0x91, 0x3b, 0x09, 0xfb, 0xbb, 0x24, 0x05, 0x16, 0x5a, 0x2e, 0x6a, 0x44]),
             "12 a 1 a 12 a s:m_Name a 5 a 188 s:m_AnimationTriggers.m_NormalTrigger s:m_AnimationTriggers.m_HighlightedTrigger s:m_AnimationTriggers.m_PressedTrigger s:m_AnimationTriggers.m_SelectedTrigger s:m_AnimationTriggers.m_DisabledTrigger 1 a 89 a [ s:m_Options.m_Options[].m_Text 28 ] a [ 12 s:m_OnValueChanged.m_PersistentCalls.m_Calls[].m_TargetAssemblyTypeName s:m_OnValueChanged.m_PersistentCalls.m_Calls[].m_MethodName 16 s:m_OnValueChanged.m_PersistentCalls.m_Calls[].m_Arguments.m_ObjectArgumentAssemblyTypeName 8 s:m_OnValueChanged.m_PersistentCalls.m_Calls[].m_Arguments.m_StringArgument 1 a 4 ] a 4"),
            // CCTV_USSR_WINDOWS_FULL/DialogButton: .DialogButton (Assembly-CSharp).
            ((UiKind::Dialog, [0x3e, 0x1b, 0x75, 0x86, 0xbb, 0x85, 0x09, 0x70, 0x32, 0x86, 0x52, 0x35, 0x8a, 0xc0, 0xe3, 0x20]),
             "12 a 1 a 12 a s:m_Name a s:Text 12 s:TargetObjectBundle s:TargetObjectInBundle 13 a s:_warningWindowLabel [ 12 s:OnClickAlways.m_PersistentCalls.m_Calls[].m_TargetAssemblyTypeName s:OnClickAlways.m_PersistentCalls.m_Calls[].m_MethodName 16 s:OnClickAlways.m_PersistentCalls.m_Calls[].m_Arguments.m_ObjectArgumentAssemblyTypeName 8 s:OnClickAlways.m_PersistentCalls.m_Calls[].m_Arguments.m_StringArgument 1 a 4 ] a 1 a s:RequiredSessionRules s:RequiredNOTSessionRules [ s:RequiredStatsAbove[].Key 16 ] a [ s:RequiredStatsBelow[].Key 16 ] a 4 s:AddsSessionRules s:RemovesSessionRules [ s:AddStats[].Key 16 ] a 5 a 1 a s:RelevantCharacterKey"),
            // Sunkissed_windows_full/Text: UnityEngine.UI.Text (UnityEngine.UI).
            ((UiKind::Text, [0xb5, 0x00, 0x76, 0x73, 0xbb, 0xbd, 0xb9, 0xd1, 0x25, 0x97, 0xa1, 0xd8, 0x20, 0x13, 0x70, 0xf5]),
             "12 a 1 a 12 a s:m_Name a 29 a 17 a [ 12 s:m_OnCullStateChanged.m_PersistentCalls.m_Calls[].m_TargetAssemblyTypeName s:m_OnCullStateChanged.m_PersistentCalls.m_Calls[].m_MethodName 16 s:m_OnCullStateChanged.m_PersistentCalls.m_Calls[].m_Arguments.m_ObjectArgumentAssemblyTypeName 8 s:m_OnCullStateChanged.m_PersistentCalls.m_Calls[].m_Arguments.m_StringArgument 1 a 4 ] a 21 a 13 a 1 a 12 s:m_Text"),
            // Sunkissed_windows_full/DialogButton: .DialogButton (Assembly-CSharp).
            ((UiKind::Dialog, [0x26, 0x90, 0xb4, 0xd2, 0xd5, 0x27, 0xb6, 0xd3, 0xee, 0xac, 0x54, 0xb7, 0x37, 0x96, 0x0e, 0xdd]),
             "12 a 1 a 12 a s:m_Name a s:Text 12 s:RoomName s:TargetObjectBundle s:TargetObjectInBundle 13 a s:_warningWindowLabel [ 12 s:OnClickAlways.m_PersistentCalls.m_Calls[].m_TargetAssemblyTypeName s:OnClickAlways.m_PersistentCalls.m_Calls[].m_MethodName 16 s:OnClickAlways.m_PersistentCalls.m_Calls[].m_Arguments.m_ObjectArgumentAssemblyTypeName 8 s:OnClickAlways.m_PersistentCalls.m_Calls[].m_Arguments.m_StringArgument 1 a 4 ] a 1 a s:RequiredSessionRules s:RequiredNOTSessionRules [ s:RequiredStatsAbove[].Key 4 ] a [ s:RequiredStatsBelow[].Key 4 ] a s:RequiredMemoriesForCharacter 16 s:RequireSpecificMemories s:AddsSessionRules s:RemovesSessionRules [ s:AddStats[].Key 4 ] a s:AddsMemories s:AddsMemoriesToCharacter 21 a 1 a s:RelevantCharacterKey"),
            // es/BOXMAN_v0.5.02_x64/Text: UnityEngine.UI.Text (UnityEngine.UI.dll).
            ((UiKind::Text, [0xae, 0xb6, 0x2f, 0x72, 0x9c, 0x52, 0xc8, 0x14, 0xde, 0xc8, 0x6c, 0x47, 0xdc, 0xac, 0x14, 0x71]),
             "12 a 1 a 12 a s:m_Name a 29 a 1 a [ 12 s:m_OnCullStateChanged.m_PersistentCalls.m_Calls[].m_MethodName 16 s:m_OnCullStateChanged.m_PersistentCalls.m_Calls[].m_Arguments.m_ObjectArgumentAssemblyTypeName 8 s:m_OnCullStateChanged.m_PersistentCalls.m_Calls[].m_Arguments.m_StringArgument 1 a 4 ] a 21 a 13 a 1 a 12 s:m_Text"),
            // es/BOXMAN_v0.5.02_x64/Dropdown: UnityEngine.UI.Dropdown (UnityEngine.UI.dll).
            ((UiKind::Dropdown, [0x2a, 0xee, 0xec, 0x1b, 0x1a, 0x14, 0x4c, 0x46, 0x9c, 0xd2, 0xd3, 0x77, 0x76, 0xbc, 0x23, 0x15]),
             "12 a 1 a 12 a s:m_Name a 192 s:m_AnimationTriggers.m_NormalTrigger s:m_AnimationTriggers.m_HighlightedTrigger s:m_AnimationTriggers.m_PressedTrigger s:m_AnimationTriggers.m_SelectedTrigger s:m_AnimationTriggers.m_DisabledTrigger 1 a 76 [ s:m_Options.m_Options[].m_Text 12 ] a [ 12 s:m_OnValueChanged.m_PersistentCalls.m_Calls[].m_MethodName 16 s:m_OnValueChanged.m_PersistentCalls.m_Calls[].m_Arguments.m_ObjectArgumentAssemblyTypeName 8 s:m_OnValueChanged.m_PersistentCalls.m_Calls[].m_Arguments.m_StringArgument 1 a 4 ] a 4"),
            // es/BOXMAN_v0.5.02_x64/ManagedTextProvider: Naninovel.ManagedTextProvider (Elringus.Naninovel.Runtime.dll).
            ((UiKind::Managed, [0xf9, 0x52, 0x43, 0x26, 0xb3, 0xae, 0x4f, 0x11, 0xb6, 0xea, 0xde, 0xe7, 0x49, 0xae, 0xd9, 0xee]),
             "12 a 1 a 12 a s:m_Name a s:category s:key s:defaultValue [ 12 s:onValueChanged.m_PersistentCalls.m_Calls[].m_MethodName 16 s:onValueChanged.m_PersistentCalls.m_Calls[].m_Arguments.m_ObjectArgumentAssemblyTypeName 8 s:onValueChanged.m_PersistentCalls.m_Calls[].m_Arguments.m_StringArgument 1 a 4 ] a"),
            // es/BOXMAN_v0.5.02_x64/TextMeshProUGUI: TMPro.TextMeshProUGUI (Unity.TextMeshPro.dll).
            ((UiKind::Tmp, [0x69, 0x39, 0x77, 0x2c, 0xb3, 0xce, 0x77, 0x5a, 0x8e, 0x8d, 0x99, 0xf3, 0x32, 0x53, 0x44, 0x91]),
             "12 a 1 a 12 a s:m_Name a 29 a 1 a [ 12 s:m_OnCullStateChanged.m_PersistentCalls.m_Calls[].m_MethodName 16 s:m_OnCullStateChanged.m_PersistentCalls.m_Calls[].m_Arguments.m_ObjectArgumentAssemblyTypeName 8 s:m_OnCullStateChanged.m_PersistentCalls.m_Calls[].m_Arguments.m_StringArgument 1 a 4 ] a s:m_text 1 a 24 [ 12 ] a 12 [ 12 ] a 21 a 93 a 17 a 17 a 49 a 33 a 1 a 1 a 1 a 1 a 1 a 1 a 17 a 1 a 1 a 21 a 1 a 1 a 28"),
        ].into_iter().map(|(hash, schema)| (hash, parse_layout_schema(schema))).collect()
    });
    profiles
        .get(&(script.ui_kind()?, script.properties_hash))
        .map(Vec::as_slice)
}

fn read_external_paths(r: &mut R<'_>) -> Result<Vec<String>, SerializedError> {
    let script_count = r.i32()?;
    if !(0..=100_000).contains(&script_count) {
        return Err(err(r.file, "invalid script reference count"));
    }
    for _ in 0..script_count {
        r.i32()?;
        r.align4();
        r.i64()?;
    }
    let count = r.i32()?;
    if !(0..=100_000).contains(&count) {
        return Err(err(r.file, "invalid external reference count"));
    }
    let mut paths = Vec::with_capacity(count as usize);
    for _ in 0..count {
        r.cstring()?;
        r.take(20)?; // GUID + external type
        paths.push(r.cstring()?);
    }
    Ok(paths)
}

fn mono_field_worth_extracting(s: &str, text_renderer: bool, name: &str) -> bool {
    mono_script_field_worth_extracting(s)
        // Transfer an eligible renderer label from the base name to its actual
        // script field (MENU / MENU), preserving its display occurrence without
        // changing the general script-field content policy.
        || (text_renderer
            && s == name
            && mono_name_worth_extracting(s)
            && s.trim().chars().all(|c| c.is_ascii_uppercase()))
}

/// TMP 1.1.0 FaceInfo duplicates some asset IDs in m_FamilyName. These are
/// technical font metadata, not renderer m_text. Recognize the actual layout
/// and owning type; equal strings in renderers/custom scripts stay eligible.
fn technical_font_name_range(
    after_name: &R<'_>,
    script: &MonoScriptIdentity,
    name: &str,
) -> Option<(usize, usize)> {
    if script.namespace != "TMPro" || script.class != "TMP_FontAsset" || name.is_empty() {
        return None;
    }
    let mut r = R {
        data: after_name.data,
        pos: after_name.pos,
        file: after_name.file,
        endian: after_name.endian,
    };
    let (version, _, _) = r.aligned_string().ok()?;
    if version != "1.1.0" {
        return None;
    }
    r.i32().ok()?; // FaceInfo.m_FaceIndex
    let (family, offset, len) = r.aligned_string().ok()?;
    let end = offset.checked_add(4)?.checked_add(len)?;
    (family == name && r.pos <= r.data.len()).then_some((offset, end))
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
    // A single CJK character can be a complete UI label (是/否/存). Keep the
    // two-letter floor for other scripts so crumbs like `v'` / `A!` stay out.
    t.chars().filter(|c| c.is_alphabetic()).count() >= 2
        || (t.chars().count() == 1 && t.chars().all(crate::unity::is_cjk_script_char))
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
    if t.starts_with('{') && t.ends_with('}') && t.len() >= 3 && !t[1..t.len() - 1].contains(' ') {
        return true;
    }
    // Mixer / resource path fragments without sentence whitespace: `Master/HFX`.
    if !t.contains(' ') && t.contains('/') {
        return true;
    }
    // Asset hierarchy paths that include spaces (BOXMAN audio/tilemap/shader flood).
    // Keeps real UI like `START / LOAD` and `Fridge / Microwave`.
    if looks_like_unity_asset_path(t) {
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
    // Scene/dev markers: `------------------------------ SETUP LIGHTS IN GALLERY MODE`
    if looks_like_dev_separator_banner(t) {
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

/// Unity project resource paths (often with spaces) that are not player copy.
/// e.g. `Naninovel/Audio/BGM/…`, `Tilemap/Pillar Sprite_11`, `Shaders/TMP_SDF Overlay`.
/// Does **not** match spaced UI phrases like `START / LOAD` or `Fridge / Microwave`.
pub(crate) fn looks_like_unity_asset_path(t: &str) -> bool {
    let t = t.trim();
    if !t.contains('/') {
        return false;
    }
    let lower = t.to_ascii_lowercase();
    const PREFIXES: &[&str] = &[
        "naninovel/",
        "shaders/",
        "shader graph/",
        "tilemap/",
        "sprites/",
        "textures/",
        "fonts & materials/",
        "fonts/",
        "style sheets/",
        "sprite assets/",
        "color gradient",
        "profiles/",
        "settings/",
        "other/",
        "ui/",
    ];
    if PREFIXES.iter().any(|p| lower.starts_with(p)) {
        return true;
    }
    // Lighting / post profile crumbs: `Day/1 Centered`, `Night/2 Centered`.
    if let Some(rest) = lower
        .strip_prefix("day/")
        .or_else(|| lower.strip_prefix("night/"))
    {
        if rest.chars().next().is_some_and(|c| c.is_ascii_digit()) {
            return true;
        }
    }
    false
}

/// Editor/scene section banners padded with dashes (BOXMAN gallery setup notes).
/// e.g. `------------------------------ SETUP LIGHTS IN GALLERY MODE`
fn looks_like_dev_separator_banner(t: &str) -> bool {
    let t = t.trim();
    if t.len() < 12 {
        return false;
    }
    let leading = t
        .chars()
        .take_while(|c| matches!(c, '-' | '=' | '_' | '*'))
        .count();
    if leading < 8 {
        return false;
    }
    let rest = t[leading..].trim();
    if rest.is_empty() {
        return true;
    }
    // Remainder is a shouty ALL-CAPS dev note (allow spaces / digits / `/`).
    let letters: Vec<char> = rest.chars().filter(|c| c.is_alphabetic()).collect();
    if letters.is_empty() {
        return true;
    }
    let upper = letters.iter().filter(|c| c.is_ascii_uppercase()).count();
    upper * 100 / letters.len() >= 70
        && rest.chars().all(|c| {
            c.is_ascii_alphanumeric()
                || c.is_ascii_whitespace()
                || matches!(c, '/' | '_' | '-' | ':' | '.')
        })
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
    let lines: Vec<&str> = t.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
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
    if t.contains("Version=") && (t.contains("PublicKeyToken=") || t.contains("Culture=")) {
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
        "set_text" | "get_text" | "set_enabled" | "get_enabled" | "set_active" | "get_active"
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
    if !(20..=400).contains(&total) {
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
    // This writer also receives legacy MonoBehaviour field_index=0 entries.
    // Revalidate against the current object and its owning MonoScript, rather
    // than trusting extraction metadata or a blacklist of the name's text.
    if let Ok(sf) = SerializedFile::parse_metadata(file_bytes, file_label.into()) {
        for obj in sf.mono_behaviour_objects() {
            let object_end = obj.data_abs as usize + obj.byte_size as usize;
            if script_len_offset >= object_end || need <= obj.data_abs as usize {
                continue;
            }
            if script_len_offset < obj.data_abs as usize || need > object_end {
                return Err(err(
                    file_label,
                    "translation slot crosses MonoBehaviour boundary",
                ));
            }
            let mut r = R {
                data: &file_bytes[..object_end],
                pos: obj.data_abs as usize + 16,
                file: file_label,
                endian: sf.header.endian,
            };
            if let Ok((file_id, path_id, name, name_end)) = (|| {
                let file_id = r.i32()?;
                let path_id = r.i64()?;
                let (name, _, _) = r.aligned_string()?;
                r.need(0)?;
                Ok::<_, SerializedError>((file_id, path_id, name, r.pos))
            })() {
                let script = sf.script_identity(file_bytes, file_id, path_id);
                let kind = script.and_then(MonoScriptIdentity::ui_kind);
                if sf.has_type_tree || kind.is_some() {
                    let layout = if sf.has_type_tree {
                        sf.type_layouts
                            .get(obj.type_index as usize)
                            .and_then(Option::as_deref)
                    } else {
                        script.and_then(stripped_layout)
                    };
                    let fields = layout.and_then(|layout| {
                        read_layout_strings(file_bytes, obj, sf.header.endian, file_label, layout)
                            .ok()
                    });
                    if !fields.is_some_and(|fields| {
                        fields.iter().any(|field| {
                            field.offset == script_len_offset
                                && field.len == orig_script_byte_len
                                && if field.path == "m_Name" {
                                    !script
                                        .is_some_and(MonoScriptIdentity::has_technical_object_name)
                                        && mono_name_worth_extracting(&field.text)
                                } else {
                                    field_is_display(kind, &field.path, &field.text)
                                }
                        })
                    }) {
                        return Err(err(
                            file_label,
                            "unsupported or technical MonoBehaviour translation slot",
                        ));
                    }
                }
                if let Some(script) = script {
                    let font_name = technical_font_name_range(&r, script, &name);
                    if (script_len_offset < name_end && script.has_technical_object_name())
                        || font_name
                            .is_some_and(|(start, end)| script_len_offset < end && need > start)
                    {
                        return Err(err(
                            file_label,
                            "technical Object.m_Name or font-name alias is not a translation slot",
                        ));
                    }
                }
            } else {
                return Err(err(file_label, "unsupported MonoBehaviour base layout"));
            }
        }
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
    write_v17_fixture_ex(
        text_name,
        text_script,
        Some(("Dummy", "not a text asset body")),
    )
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

// The `write_*_fixture` builders below this module are themselves `#[cfg(test)]`
// and are deliberately grouped at the end of the file, out of the way of the
// parser code. Moving ~700 lines to satisfy a layout lint buys nothing.
#[allow(clippy::items_after_test_module)]
#[cfg(test)]
mod tests {
    use super::*;
    // Fixtures encode the serialized primitives themselves, without using the
    // production layout reader. A local MonoScript and a neighbouring opaque
    // object exercise class resolution and the object boundary in both endians.
    const UI05_TMP_HASH: [u8; 16] = [
        0xcf, 0x62, 0xaa, 0xd9, 0x7f, 0x6c, 0x2d, 0xd5, 0xd4, 0x82, 0xe2, 0x50, 0x9c, 0x6c, 0x95,
        0x28,
    ];
    const UI05_TMP_SCHEMA: &str = "12 a 1 a 12 a s:m_Name a 29 a 17 a [ 12 s:m_TargetAssemblyTypeName s:m_MethodName 16 s:m_ObjectArgumentAssemblyTypeName 8 s:m_StringArgument 1 a 4 ] a s:m_text 1 a 24 [ 12 ] a 12 [ 12 ] a 21 a 93 a 17 a 17 a 85 a [ 4 ] a 1 a 1 a 1 a 1 a 1 a 1 a 1 a 17 a 1 a 1 a 21 a 1 a 1 a 28";
    const UI05_DROPDOWN_HASH: [u8; 16] = [
        0xe1, 0x8d, 0xfd, 0x2b, 0x91, 0x3b, 0x09, 0xfb, 0xbb, 0x24, 0x05, 0x16, 0x5a, 0x2e, 0x6a,
        0x44,
    ];
    const UI05_DROPDOWN_SCHEMA: &str = "12 a 1 a 12 a s:m_Name a 5 a 188 s:m_NormalTrigger s:m_HighlightedTrigger s:m_PressedTrigger s:m_SelectedTrigger s:m_DisabledTrigger 1 a 89 a [ s:m_Text 28 ] a [ 12 s:m_TargetAssemblyTypeName s:m_MethodName 16 s:m_ObjectArgumentAssemblyTypeName 8 s:m_StringArgument 1 a 4 ] a 4";
    const UI05_MANAGED_HASH: [u8; 16] = [
        0xf9, 0x52, 0x43, 0x26, 0xb3, 0xae, 0x4f, 0x11, 0xb6, 0xea, 0xde, 0xe7, 0x49, 0xae, 0xd9,
        0xee,
    ];
    const UI05_MANAGED_SCHEMA: &str = "12 a 1 a 12 a s:m_Name a s:category s:key s:defaultValue [ 12 s:m_MethodName 16 s:m_ObjectArgumentAssemblyTypeName 8 s:m_StringArgument 1 a 4 ] a";

    fn ui05_word(out: &mut Vec<u8>, n: u32, be: bool) {
        out.extend_from_slice(&if be { n.to_be_bytes() } else { n.to_le_bytes() });
    }
    fn ui05_string(out: &mut Vec<u8>, s: &str, be: bool) {
        ui05_word(out, s.len() as u32, be);
        out.extend_from_slice(s.as_bytes());
        out.resize((out.len() + 3) & !3, 0);
    }
    fn ui05_payload(schema: &str, be: bool, values: &[(&str, &str)], counts: &[usize]) -> Vec<u8> {
        fn emit(
            tokens: &[&str],
            at: &mut usize,
            out: &mut Vec<u8>,
            be: bool,
            values: &[(&str, &str)],
            counts: &mut std::slice::Iter<'_, usize>,
            option: &mut usize,
        ) {
            while *at < tokens.len() {
                let token = tokens[*at];
                *at += 1;
                match token {
                    "]" => return,
                    "a" => out.resize((out.len() + 3) & !3, 0),
                    "[" => {
                        let n = *counts.next().unwrap_or(&0);
                        ui05_word(out, n as u32, be);
                        let start = *at;
                        let mut end = start;
                        let mut depth = 1;
                        while depth > 0 {
                            match tokens[end] {
                                "[" => depth += 1,
                                "]" => depth -= 1,
                                _ => {}
                            }
                            end += 1;
                        }
                        for _ in 0..n {
                            let mut pos = start;
                            emit(&tokens[..end], &mut pos, out, be, values, counts, option);
                        }
                        *at = end;
                    }
                    s if s.starts_with("s:") => {
                        let key = &s[2..];
                        let value = if key == "m_Text" {
                            let v = ["Low", "Medium", "High"][*option % 3];
                            *option += 1;
                            v
                        } else {
                            values
                                .iter()
                                .find(|(k, _)| *k == key)
                                .map_or("", |(_, v)| *v)
                        };
                        ui05_string(out, value, be);
                    }
                    n => {
                        let n: usize = n.parse().unwrap();
                        out.resize(out.len() + n, 0);
                    }
                }
            }
        }
        let mut out = Vec::new();
        emit(
            &schema.split_whitespace().collect::<Vec<_>>(),
            &mut 0,
            &mut out,
            be,
            values,
            &mut counts.iter(),
            &mut 0,
        );
        out[12] = 1;
        out[20..28].copy_from_slice(&if be {
            20i64.to_be_bytes()
        } else {
            20i64.to_le_bytes()
        });
        out
    }

    type Ui05Node<'a> = (u8, &'a str, &'a str, i32, u32);
    fn ui05_asset(
        identity: (&str, &str, &str),
        hash: [u8; 16],
        payload: Vec<u8>,
        be: bool,
        tree: Option<&[Ui05Node<'_>]>,
    ) -> Vec<u8> {
        let mut script = Vec::new();
        ui05_string(&mut script, identity.0, be);
        script.extend_from_slice(&[0; 4]);
        script.extend_from_slice(&hash);
        for s in [identity.0, identity.1, identity.2] {
            ui05_string(&mut script, s, be);
        }
        let payloads = [
            payload.clone(),
            payload,
            script,
            b"opaque neighboring object bytes!".to_vec(),
        ];
        let mut meta = b"6000.0.24f1\0".to_vec();
        ui05_word(&mut meta, 1, be);
        meta.push(u8::from(tree.is_some()));
        ui05_word(&mut meta, 3, be);
        for class in [114i32, 115, 1] {
            ui05_word(&mut meta, class as u32, be);
            meta.push(0);
            meta.extend_from_slice(&[255; 2]);
            if class == 114 {
                meta.extend_from_slice(&[0; 16]);
            }
            meta.extend_from_slice(&[0; 16]);
            if let Some(nodes) = tree {
                let nodes = if class == 114 { nodes } else { &[] };
                let mut strings = Vec::new();
                let mut blob = Vec::new();
                for &(level, ty, name, size, flags) in nodes {
                    blob.extend_from_slice(&if be {
                        1u16.to_be_bytes()
                    } else {
                        1u16.to_le_bytes()
                    });
                    blob.extend_from_slice(&[level, 0]);
                    for text in [ty, name] {
                        ui05_word(&mut blob, strings.len() as u32, be);
                        strings.extend_from_slice(text.as_bytes());
                        strings.push(0);
                    }
                    for n in [size as u32, 0, flags] {
                        ui05_word(&mut blob, n, be);
                    }
                }
                ui05_word(&mut meta, nodes.len() as u32, be);
                ui05_word(&mut meta, strings.len() as u32, be);
                meta.extend(blob);
                meta.extend(strings);
            }
        }
        ui05_word(&mut meta, 4, be);
        let mut offset = 0;
        for ((id, ty), p) in [(10i64, 0), (11, 0), (20, 1), (30, 2)]
            .into_iter()
            .zip(&payloads)
        {
            meta.resize((meta.len() + 3) & !3, 0); // header is four-aligned
            meta.extend_from_slice(&if be {
                id.to_be_bytes()
            } else {
                id.to_le_bytes()
            });
            for n in [offset, p.len() as u32, ty] {
                ui05_word(&mut meta, n, be);
            }
            offset += p.len() as u32;
        }
        ui05_word(&mut meta, 0, be);
        ui05_word(&mut meta, 0, be);
        meta.push(0);
        let data_offset = (20 + meta.len() + 15) & !15;
        let mut out = Vec::new();
        for n in [
            meta.len() as u32,
            data_offset as u32 + offset,
            17,
            data_offset as u32,
        ] {
            out.extend_from_slice(&n.to_be_bytes());
        }
        out.extend_from_slice(&[u8::from(be), 0, 0, 0]);
        out.extend(meta);
        out.resize(data_offset, 0);
        for p in payloads {
            out.extend(p);
        }
        out
    }
    fn ui05_assert_inject(bytes: &[u8], fields: &[MonoStringData]) {
        let mut actual = bytes.to_vec();
        let mut expected = bytes.to_vec();
        for f in fields {
            rewrite_text_asset_script_inplace(
                &mut actual,
                f.len_offset,
                f.byte_len,
                "X",
                "field.assets",
            )
            .unwrap();
            expected[f.len_offset + 4..f.len_offset + 4 + f.byte_len].fill(b' ');
            expected[f.len_offset + 4] = b'X';
        }
        assert_eq!(actual, expected, "only requested payloads may change");
        let parsed = SerializedFile::parse(actual, "field.assets").unwrap();
        assert!(parsed
            .read_mono_strings(10)
            .unwrap()
            .iter()
            .all(|f| f.text.trim_end() == "X"));
    }
    #[test]
    fn ui05_stripped_renderer_uppercase_and_callbacks() {
        for be in [false, true] {
            for text in ["MENU", "HIDE", "EXIT", "Welcome friend"] {
                let p = ui05_payload(
                    UI05_TMP_SCHEMA,
                    be,
                    &[
                        ("m_text", text),
                        ("m_MethodName", "Show"),
                        ("m_StringArgument", "Play"),
                    ],
                    &[1, 0, 0, 0],
                );
                let bytes = ui05_asset(
                    ("TextMeshProUGUI", "TMPro", "Unity.TextMeshPro"),
                    UI05_TMP_HASH,
                    p,
                    be,
                    None,
                );
                let sf = SerializedFile::parse(bytes.clone(), "field.assets").unwrap();
                let fields = sf.read_mono_strings(10).unwrap();
                assert_eq!(
                    fields.iter().map(|f| f.text.as_str()).collect::<Vec<_>>(),
                    [text]
                );
                ui05_assert_inject(&bytes, &fields);
                let callback = bytes.windows(4).position(|x| x == b"Show").unwrap();
                let mut copy = bytes.clone();
                assert!(rewrite_text_asset_script_inplace(
                    &mut copy,
                    callback - 4,
                    4,
                    "Hide",
                    "field.assets"
                )
                .is_err());
                assert_eq!(copy, bytes);
            }
        }
    }
    #[test]
    fn ui05_stripped_dropdown_actual_headers_options_and_event() {
        for be in [false, true] {
            let p = ui05_payload(
                UI05_DROPDOWN_SCHEMA,
                be,
                &[("m_NormalTrigger", "Normal"), ("m_MethodName", "Play")],
                &[3, 1],
            );
            let bytes = ui05_asset(
                ("TMP_Dropdown", "TMPro", "Unity.TextMeshPro"),
                UI05_DROPDOWN_HASH,
                p,
                be,
                None,
            );
            let sf = SerializedFile::parse(bytes.clone(), "field.assets").unwrap();
            let fields = sf.read_mono_strings(10).unwrap();
            assert_eq!(
                fields.iter().map(|f| f.text.as_str()).collect::<Vec<_>>(),
                ["Low", "Medium", "High"]
            );
            ui05_assert_inject(&bytes, &fields);
        }
    }
    #[test]
    fn ui05_stripped_managed_fallback_preserves_lookup_and_set_text() {
        for be in [false, true] {
            let p = ui05_payload(
                UI05_MANAGED_SCHEMA,
                be,
                &[
                    ("category", "Tips"),
                    ("key", "Show"),
                    ("defaultValue", "TIPS"),
                    ("m_MethodName", "set_text"),
                ],
                &[1],
            );
            let bytes = ui05_asset(
                (
                    "ManagedTextProvider",
                    "Naninovel",
                    "Elringus.Naninovel.Runtime.dll",
                ),
                UI05_MANAGED_HASH,
                p,
                be,
                None,
            );
            let sf = SerializedFile::parse(bytes.clone(), "field.assets").unwrap();
            let fields = sf.read_mono_strings(10).unwrap();
            assert_eq!(
                fields.iter().map(|f| f.text.as_str()).collect::<Vec<_>>(),
                ["TIPS"]
            );
            ui05_assert_inject(&bytes, &fields);
            for value in [b"Tips".as_slice(), b"Show", b"set_text"] {
                let at = bytes.windows(value.len()).position(|x| x == value).unwrap();
                let mut copy = bytes.clone();
                assert!(rewrite_text_asset_script_inplace(
                    &mut copy,
                    at - 4,
                    value.len(),
                    "X",
                    "field.assets"
                )
                .is_err());
                assert_eq!(copy, bytes);
            }
        }
    }
    #[test]
    fn ui05_type_tree_renderer_field_identity_and_false_length() {
        for be in [false, true] {
            for (class, namespace, assembly, field) in [
                ("TextMeshProUGUI", "TMPro", "Unity.TextMeshPro", "m_text"),
                ("Text", "UnityEngine.UI", "UnityEngine.UI", "m_Text"),
            ] {
                let tree = [
                    (0, "MonoBehaviour", "Base", -1, 0),
                    (1, "int", "header", 4, 0),
                    (1, "SInt64", "go", 8, 0),
                    (1, "UInt8", "m_Enabled", 1, 0x4000),
                    (1, "int", "scriptFile", 4, 0),
                    (1, "SInt64", "scriptPath", 8, 0),
                    (1, "string", "m_Name", -1, 0),
                    (1, "int", "falseLength", 4, 0),
                    (1, "int", "falseText", 4, 0),
                    (1, "string", field, -1, 0),
                    (1, "string", "TargetObjectInBundle", -1, 0),
                    (1, "UnityEvent", "onClick", -1, 0),
                    (2, "PersistentCallGroup", "m_PersistentCalls", -1, 0),
                    (3, "string", "m_MethodName", -1, 0),
                ];
                for text in ["MENU", "HIDE", "EXIT", "Welcome friend"] {
                    let mut p = ui05_payload("12 a 1 a 12 a s:m_Name a", be, &[], &[]);
                    ui05_word(&mut p, 4, be);
                    p.extend_from_slice(b"Play");
                    for s in [text, "morning", "Show"] {
                        ui05_string(&mut p, s, be);
                    }
                    let bytes =
                        ui05_asset((class, namespace, assembly), [0; 16], p, be, Some(&tree));
                    let sf = SerializedFile::parse(bytes.clone(), "field.assets").unwrap();
                    let fields = sf.read_mono_strings(10).unwrap();
                    assert_eq!(
                        fields.iter().map(|f| f.text.as_str()).collect::<Vec<_>>(),
                        [text]
                    );
                    ui05_assert_inject(&bytes, &fields);
                }
            }
        }
    }
    #[test]
    fn ui05_unsupported_layouts_and_object_boundaries_fail_closed() {
        for be in [false, true] {
            let identity = ("TextMeshProUGUI", "TMPro", "Unity.TextMeshPro");
            let p = ui05_payload(UI05_TMP_SCHEMA, be, &[("m_text", "MENU")], &[]);
            for (hash, mut payload) in [
                ([0; 16], p.clone()),
                (UI05_TMP_HASH, p.clone()),
                (UI05_TMP_HASH, p.clone()),
                (UI05_TMP_HASH, p.clone()),
            ]
            .into_iter()
            .enumerate()
            .map(|(i, (h, mut p))| {
                if i == 1 {
                    p.extend_from_slice(&[0; 4]);
                }
                if i == 2 {
                    p.truncate(88);
                }
                if i == 3 {
                    p[28..32].copy_from_slice(&if be {
                        u32::MAX.to_be_bytes()
                    } else {
                        u32::MAX.to_le_bytes()
                    });
                }
                (h, p)
            }) {
                let bytes = ui05_asset(identity, hash, std::mem::take(&mut payload), be, None);
                let sf = SerializedFile::parse(bytes.clone(), "field.assets").unwrap();
                assert!(sf
                    .read_mono_strings(10)
                    .map_or(true, |fields| fields.is_empty()));
                let mut copy = bytes.clone();
                assert!(rewrite_text_asset_script_inplace(
                    &mut copy,
                    sf.objects[0].data_abs as usize + 32,
                    4,
                    "X",
                    "field.assets"
                )
                .is_err());
                assert_eq!(copy, bytes);
            }
        }
        // A class name in another namespace/assembly must retain conservative
        // fallback policy rather than gaining uppercase display slots.
        let p = ui05_payload(
            "12 a 1 a 12 a s:m_Name a s:m_text",
            false,
            &[("m_text", "MENU")],
            &[],
        );
        let sf = SerializedFile::parse(
            ui05_asset(
                ("TextMeshProUGUI", "Game", "Assembly-CSharp"),
                UI05_TMP_HASH,
                p,
                false,
                None,
            ),
            "unknown.assets",
        )
        .unwrap();
        assert!(sf.read_mono_strings(10).unwrap().is_empty());
    }
    #[test]
    fn ui05_stale_source_and_technical_legacy_entries_are_skipped() {
        use locust_core::extraction::FormatPlugin;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("resources.assets");
        let p = ui05_payload(
            UI05_MANAGED_SCHEMA,
            false,
            &[
                ("category", "Tips"),
                ("key", "Show"),
                ("defaultValue", "TIPS"),
                ("m_MethodName", "set_text"),
            ],
            &[1],
        );
        let bytes = ui05_asset(
            (
                "ManagedTextProvider",
                "Naninovel",
                "Elringus.Naninovel.Runtime.dll",
            ),
            UI05_MANAGED_HASH,
            p,
            false,
            None,
        );
        std::fs::write(&path, &bytes).unwrap();
        let plugin = crate::unity::UnityPlugin::new();
        let entries = plugin.extract(&path).unwrap();
        let mut entry = entries
            .into_iter()
            .find(|e| e.source == "TIPS")
            .expect("fallback row");
        entry.translation = Some("TIP".into());
        let mut stale = bytes.clone();
        let off = entry.metadata["mono_string_offset"].as_u64().unwrap() as usize;
        stale[off + 4] = b'B';
        std::fs::write(&path, &stale).unwrap();
        let report = plugin.inject(dir.path(), &[entry.clone()]).unwrap();
        assert_eq!(report.strings_written, 0);
        assert_eq!(report.strings_skipped, 1);
        assert_eq!(std::fs::read(&path).unwrap(), stale);
        std::fs::write(&path, &bytes).unwrap();
        let at = bytes.windows(4).position(|s| s == b"Show").unwrap();
        entry.source = "Show".into();
        entry
            .metadata
            .insert("mono_string_offset".into(), serde_json::json!(at - 4));
        let report = plugin.inject(dir.path(), &[entry]).unwrap();
        assert_eq!(report.strings_written, 0);
        assert_eq!(report.strings_skipped, 1);
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
    }

    #[test]
    fn path_id_readers_preserve_data_and_lookup_errors() {
        let sf = SerializedFile::parse(write_v17_mixed_objects_fixture(1), "mixed.assets").unwrap();
        for obj in &sf.objects {
            if is_monobehaviour_class(obj.class_id) {
                let fields = sf.read_mono_strings(obj.path_id).unwrap();
                let direct = sf.read_mono_strings_object(obj).unwrap();
                let field_data = |field: MonoStringData| {
                    (
                        field.path_id,
                        field.mono_name,
                        field.field_index,
                        field.text,
                        field.len_offset,
                        field.byte_len,
                    )
                };
                let expected: &[&str] = if obj.class_id == CLASS_ID_MONO_BEHAVIOUR {
                    &["DialogBox", "Welcome, traveler!", "See you later."]
                } else {
                    &["Menu", "Choose your destination."]
                };
                assert_eq!(
                    fields
                        .iter()
                        .map(|field| field.text.as_str())
                        .collect::<Vec<_>>(),
                    expected
                );
                assert!(fields.iter().all(|field| field.path_id == obj.path_id));
                assert_eq!(
                    fields.into_iter().map(field_data).collect::<Vec<_>>(),
                    direct.into_iter().map(field_data).collect::<Vec<_>>()
                );
            }
            let text_data = |field: TextMeshData| {
                (
                    field.path_id,
                    field.text,
                    field.text_len_offset,
                    field.text_byte_len,
                )
            };
            if obj.class_id == CLASS_ID_TEXT_MESH {
                let field = sf.read_text_mesh(obj.path_id).unwrap();
                assert_eq!(field.path_id, obj.path_id);
                assert_eq!(field.text, "Hello, world!");
                assert_eq!(field.text_byte_len, "Hello, world!".len());
                assert_eq!(
                    text_data(field),
                    text_data(sf.read_text_mesh_object(obj).unwrap())
                );
            }
            if obj.class_id == CLASS_ID_GUI_TEXT {
                let field = sf.read_gui_text(obj.path_id).unwrap();
                assert_eq!(field.path_id, obj.path_id);
                assert_eq!(field.text, "Press Start");
                assert_eq!(field.text_byte_len, "Press Start".len());
                assert_eq!(
                    text_data(field),
                    text_data(sf.read_gui_text_object(obj).unwrap())
                );
            }
        }

        // Include existing ids of every class as well as one absent id.
        for path_id in sf.objects.iter().map(|obj| obj.path_id).chain([999]) {
            let class_id = sf
                .objects
                .iter()
                .find(|obj| obj.path_id == path_id)
                .map(|obj| obj.class_id);
            for (kind, matches_class, result) in [
                (
                    "MonoBehaviour",
                    class_id.is_some_and(is_monobehaviour_class),
                    sf.read_mono_strings(path_id).map(|_| ()),
                ),
                (
                    "TextMesh",
                    class_id == Some(CLASS_ID_TEXT_MESH),
                    sf.read_text_mesh(path_id).map(|_| ()),
                ),
                (
                    "GUIText",
                    class_id == Some(CLASS_ID_GUI_TEXT),
                    sf.read_gui_text(path_id).map(|_| ()),
                ),
            ] {
                if matches_class {
                    result.unwrap();
                } else {
                    let error = result.unwrap_err();
                    assert_eq!(error.file, "mixed.assets");
                    assert_eq!(error.message, format!("no {kind} with path_id={path_id}"));
                    assert_eq!(
                        error.to_string(),
                        format!("mixed.assets: no {kind} with path_id={path_id}")
                    );
                }
            }
        }
    }

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
        assert!(
            sf.header.data_offset >= 48,
            "extended header pushes metadata past 20"
        );
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
        let bytes = write_v17_mono_fixture("DialogBox", &["Welcome, traveler!", "See you later."]);
        let sf = SerializedFile::parse(bytes, "mono.assets").unwrap();
        let monos: Vec<_> = sf.mono_behaviour_objects().collect();
        assert_eq!(monos.len(), 1);
        assert_eq!(monos[0].path_id, 10);
        assert_eq!(monos[0].class_id, CLASS_ID_MONO_BEHAVIOUR);

        let fields = sf.read_mono_strings(10).unwrap();
        // Unresolved custom script: its name remains eligible as a label.
        assert_eq!(fields.len(), 3, "fields: {fields:?}");
        assert_eq!(fields[0].field_index, 0);
        assert_eq!(fields[0].text, "DialogBox");
        assert_eq!(fields[1].text, "Welcome, traveler!");
        assert_eq!(fields[2].text, "See you later.");
        assert_eq!(fields[1].mono_name, "DialogBox");
    }

    /// Real MonoScript metadata, including local/external PPtrs; names alone
    /// deliberately provide no evidence about the owning script type.
    fn identified_mono_fixture(
        identity: (&str, &str, &str),
        name: &str,
        fields: &[&str],
        script_file: i32,
    ) -> Vec<u8> {
        identified_mono_fixture_endian(identity, name, fields, script_file, false)
    }

    fn identified_mono_fixture_endian(
        identity: (&str, &str, &str),
        name: &str,
        fields: &[&str],
        script_file: i32,
        be: bool,
    ) -> Vec<u8> {
        fn string(out: &mut Vec<u8>, value: &str, be: bool) {
            ui05_word(out, value.len() as u32, be);
            out.extend_from_slice(value.as_bytes());
            out.resize((out.len() + 3) & !3, 0);
        }
        let mut mono = vec![0; 16];
        mono[12] = 1;
        mono.extend_from_slice(&if be {
            script_file.to_be_bytes()
        } else {
            script_file.to_le_bytes()
        });
        mono.extend_from_slice(&if be {
            20i64.to_be_bytes()
        } else {
            20i64.to_le_bytes()
        });
        string(&mut mono, name, be);
        for field in fields {
            string(&mut mono, field, be);
        }
        let mut script = Vec::new();
        string(&mut script, identity.0, be);
        script.extend_from_slice(&[0; 20]); // execution order + properties hash
        for value in [identity.0, identity.1, identity.2] {
            string(&mut script, value, be);
        }
        let payloads = [
            mono.clone(),
            mono,
            script,
            b"opaque other object bytes".to_vec(),
        ];
        let classes = [CLASS_ID_MONO_BEHAVIOUR, CLASS_ID_MONO_SCRIPT, 1];
        let mut meta = b"2019.4.0f1\0".to_vec();
        meta.extend_from_slice(&if be {
            1u32.to_be_bytes()
        } else {
            1u32.to_le_bytes()
        });
        meta.push(0);
        meta.extend_from_slice(&if be {
            3i32.to_be_bytes()
        } else {
            3i32.to_le_bytes()
        });
        for class in classes {
            meta.extend_from_slice(&if be {
                class.to_be_bytes()
            } else {
                class.to_le_bytes()
            });
            meta.push(0);
            meta.extend_from_slice(&if be {
                (-1i16).to_be_bytes()
            } else {
                (-1i16).to_le_bytes()
            });
            if is_monobehaviour_class(class) {
                meta.extend_from_slice(&[0; 16]);
            }
            meta.extend_from_slice(&[0; 16]);
        }
        meta.extend_from_slice(&if be {
            4i32.to_be_bytes()
        } else {
            4i32.to_le_bytes()
        });
        let mut offset = 0usize;
        for ((id, ty), payload) in [(10i64, 0i32), (11, 0), (20, 1), (30, 2)]
            .into_iter()
            .zip(&payloads)
        {
            meta.resize((meta.len() + 3) & !3, 0);
            meta.extend_from_slice(&if be {
                id.to_be_bytes()
            } else {
                id.to_le_bytes()
            });
            meta.extend_from_slice(&if be {
                (offset as u32).to_be_bytes()
            } else {
                (offset as u32).to_le_bytes()
            });
            meta.extend_from_slice(&if be {
                (payload.len() as u32).to_be_bytes()
            } else {
                (payload.len() as u32).to_le_bytes()
            });
            meta.extend_from_slice(&if be {
                ty.to_be_bytes()
            } else {
                ty.to_le_bytes()
            });
            offset += payload.len();
        }
        meta.extend_from_slice(&if be {
            0i32.to_be_bytes()
        } else {
            0i32.to_le_bytes()
        }); // script reference table
        meta.extend_from_slice(&if be {
            1i32.to_be_bytes()
        } else {
            1i32.to_le_bytes()
        }); // external table
        meta.extend_from_slice(&[0; 21]); // empty string + GUID + external type
        meta.extend_from_slice(b"globalgamemanagers.assets\0");
        meta.extend_from_slice(&if be {
            0i32.to_be_bytes()
        } else {
            0i32.to_le_bytes()
        });
        meta.push(0); // user information
        let data_offset = (20 + meta.len() + 15) & !15;
        let mut bytes = Vec::new();
        for value in [
            meta.len() as u32,
            (data_offset + offset) as u32,
            17,
            data_offset as u32,
        ] {
            bytes.extend_from_slice(&value.to_be_bytes());
        }
        bytes.extend_from_slice(&[u8::from(be), 0, 0, 0]);
        bytes.extend_from_slice(&meta);
        bytes.resize(data_offset, 0);
        for payload in payloads {
            bytes.extend_from_slice(&payload);
        }
        bytes
    }

    fn inject_legacy_name(
        root: &Path,
        path: &Path,
        source: &str,
        offset: Option<usize>,
        be: bool,
    ) -> locust_core::extraction::InjectionReport {
        use locust_core::{extraction::FormatPlugin, models::StringEntry};
        let mut entry = StringEntry::new("legacy-technical-name", source, path.to_path_buf());
        entry.translation = Some("X".into());
        entry
            .metadata
            .insert("extraction_method".into(), serde_json::json!("heuristic"));
        if let Some(offset) = offset {
            entry
                .metadata
                .insert("binary_offset".into(), serde_json::json!(offset));
            entry.metadata.insert(
                "length_endian".into(),
                serde_json::json!(if be { "be" } else { "le" }),
            );
        }
        crate::unity::UnityPlugin::new()
            .inject(root, &[entry])
            .unwrap()
    }

    #[test]
    fn legacy_technical_name_local_external_bundle() {
        assert_legacy_technical_name_fixture(
            (
                "LiftGammaGain",
                "UnityEngine.Rendering",
                "Unity.RenderPipelines.Core.Runtime.dll",
            ),
            "LiftGammaGain",
            &["Hello traveler!"],
        );
    }

    #[test]
    fn legacy_technical_font_alias_local_external_bundle() {
        assert_legacy_technical_name_fixture(
            ("TMP_FontAsset", "TMPro", "Unity.TextMeshPro.dll"),
            "OCR-A",
            &["1.1.0", "", "OCR-A", "Hello traveler!"],
        );
    }

    fn assert_legacy_technical_name_fixture(
        identity: (&str, &str, &str),
        name: &str,
        fields: &[&str],
    ) {
        for be in [false, true] {
            for location in ["local", "external", "bundle"] {
                let dir = tempfile::tempdir().unwrap();
                let file_id = i32::from(location != "local");
                let bytes = identified_mono_fixture_endian(identity, name, fields, file_id, be);
                let scripts =
                    identified_mono_fixture_endian(identity, "Script metadata", &[], 0, be);
                let path = if location == "bundle" {
                    dir.path().join("data.unity3d/resources.assets")
                } else {
                    dir.path().join("resources.assets")
                };
                let bundle_path = dir.path().join("data.unity3d");
                let original = if location == "bundle" {
                    let bundle = crate::unity_fs::build_test_bundle(
                        &[
                            ("resources.assets", &bytes),
                            ("globalgamemanagers.assets", &scripts),
                        ],
                        true,
                        8,
                        true,
                        false,
                    );
                    std::fs::write(&bundle_path, &bundle).unwrap();
                    bundle
                } else {
                    std::fs::write(&path, &bytes).unwrap();
                    if location == "external" {
                        std::fs::write(dir.path().join("globalgamemanagers.assets"), &scripts)
                            .unwrap();
                    }
                    bytes.clone()
                };
                let sf = SerializedFile::parse(bytes.clone(), &path).unwrap();
                let name_offset = sf.objects[0].data_abs as usize + 28;
                let mut offsets = vec![Some(name_offset), None];
                if identity.0 == "TMP_FontAsset" {
                    offsets.insert(0, Some(name_offset + 12 + 12 + 4));
                }
                for offset in offsets {
                    let report = inject_legacy_name(dir.path(), &path, name, offset, be);
                    assert_eq!(
                        report.strings_written, 0,
                        "{identity:?}, {location}, be={be}, {offset:?}"
                    );
                    assert_eq!(
                        report.skip_reasons.get("invalid_target"),
                        Some(&1),
                        "{identity:?}, {location}, be={be}, {offset:?}: {report:?}"
                    );
                    assert_eq!(report.files_modified, 0);
                    assert_eq!(
                        std::fs::read(if location == "bundle" {
                            &bundle_path
                        } else {
                            &path
                        })
                        .unwrap(),
                        original
                    );
                }
            }
        }
    }

    #[test]
    fn legacy_technical_names_keep_equal_display_fields_writable() {
        use locust_core::extraction::FormatPlugin;
        for be in [false, true] {
            for (class, namespace, assembly, field) in [
                ("Text", "UnityEngine.UI", "UnityEngine.UI", "m_Text"),
                ("TextMeshPro", "TMPro", "Unity.TextMeshPro", "m_text"),
            ] {
                let dir = tempfile::tempdir().unwrap();
                let path = dir.path().join("resources.assets");
                let tree = [
                    (0, "MonoBehaviour", "Base", -1, 0),
                    (1, "int", "goFile", 4, 0),
                    (1, "SInt64", "goPath", 8, 0),
                    (1, "UInt8", "m_Enabled", 1, 0x4000),
                    (1, "int", "scriptFile", 4, 0),
                    (1, "SInt64", "scriptPath", 8, 0),
                    (1, "string", "m_Name", -1, 0),
                    (1, "string", field, -1, 0),
                ];
                let p = ui05_payload(
                    "12 a 1 a 12 a s:m_Name a s:display a",
                    be,
                    &[("m_Name", "LiftGammaGain"), ("display", "LiftGammaGain")],
                    &[],
                );
                let bytes = ui05_asset((class, namespace, assembly), [0; 16], p, be, Some(&tree));
                let sf = SerializedFile::parse(bytes.clone(), &path).unwrap();
                let display = sf.read_mono_strings(10).unwrap().remove(0);
                std::fs::write(&path, &bytes).unwrap();
                let report = inject_legacy_name(
                    dir.path(),
                    &path,
                    "LiftGammaGain",
                    Some(display.len_offset),
                    be,
                );
                assert_eq!(report.strings_written, 1, "{class}, be={be}: {report:?}");
                let mut expected = bytes.clone();
                expected[display.len_offset..display.len_offset + 4].copy_from_slice(&if be {
                    1u32.to_be_bytes()
                } else {
                    1u32.to_le_bytes()
                });
                expected[display.len_offset + 4..display.len_offset + 4 + display.byte_len].fill(0);
                expected[display.len_offset + 4] = b'X';
                assert_eq!(std::fs::read(&path).unwrap(), expected);
                std::fs::write(&path, &bytes).unwrap();
                let mut row = crate::unity::UnityPlugin::new()
                    .extract(&path)
                    .unwrap()
                    .into_iter()
                    .find(|e| e.metadata.get("path_id").and_then(|v| v.as_i64()) == Some(10))
                    .unwrap();
                row.translation = Some("X".into());
                let report = crate::unity::UnityPlugin::new()
                    .inject(dir.path(), &[row])
                    .unwrap();
                assert_eq!(report.strings_written, 1);
                let mut expected = bytes.clone();
                expected[display.len_offset + 4..display.len_offset + 4 + display.byte_len]
                    .fill(b' ');
                expected[display.len_offset + 4] = b'X';
                assert_eq!(std::fs::read(&path).unwrap(), expected);
            }
        }
    }

    #[test]
    fn legacy_technical_names_unresolved_custom_name_still_writes() {
        for be in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("resources.assets");
            // The same path ID exists locally, but the real script reference
            // points at a missing external file. Do not infer its identity.
            let mut bytes = identified_mono_fixture_endian(
                (
                    "LiftGammaGain",
                    "UnityEngine.Rendering",
                    "Unity.RenderPipelines.Core.Runtime",
                ),
                "Custom UI label",
                &[],
                1,
                be,
            );
            let sf = SerializedFile::parse(bytes.clone(), &path).unwrap();
            // Make the second object's name distinct so an offset-less row
            // has one candidate, without changing its layout or lengths.
            let second = sf.objects[1].data_abs as usize + 32;
            bytes[second..second + 15].copy_from_slice(b"Other UI label!");
            let offset = sf.objects[0].data_abs as usize + 28;
            for pinned in [false, true] {
                std::fs::write(&path, &bytes).unwrap();
                let report = inject_legacy_name(
                    dir.path(),
                    &path,
                    "Custom UI label",
                    pinned.then_some(offset),
                    be,
                );
                assert_eq!(
                    report.strings_written, 1,
                    "pinned={pinned}, be={be}: {report:?}"
                );
                assert_eq!(report.strings_skipped, 0);
                let mut expected = bytes.clone();
                expected[offset..offset + 4].copy_from_slice(&if be {
                    1u32.to_be_bytes()
                } else {
                    1u32.to_le_bytes()
                });
                expected[offset + 4..offset + 19].fill(0);
                expected[offset + 4] = b'X';
                assert_eq!(std::fs::read(&path).unwrap(), expected);
            }
        }
    }

    #[test]
    fn do_not_extract_object_name() {
        let bytes = identified_mono_fixture(
            (
                "LiftGammaGain",
                "UnityEngine.Rendering.Universal",
                "Unity.RenderPipelines.Universal.Runtime",
            ),
            "LiftGammaGain",
            &["Hello traveler!"],
            0,
        );
        let sf = SerializedFile::parse(bytes.clone(), "technical.assets").unwrap();
        let fields = sf.read_mono_strings(10).unwrap();
        assert_eq!(
            fields.len(),
            1,
            "technical m_Name must be excluded: {fields:?}"
        );
        assert_eq!(fields[0].field_index, 1);
        assert_eq!(fields[0].text, "Hello traveler!");
        let mut after = bytes.clone();
        rewrite_text_asset_script_inplace(
            &mut after,
            fields[0].len_offset,
            fields[0].byte_len,
            "Hola viajero!",
            "technical.assets",
        )
        .unwrap();
        let start = fields[0].len_offset + 4;
        assert_eq!(
            &after[..start],
            &bytes[..start],
            "header, base name, prefix"
        );
        assert_eq!(
            &after[start + fields[0].byte_len..],
            &bytes[start + fields[0].byte_len..],
            "other objects and padding"
        );
        let parsed = SerializedFile::parse(after, "technical.assets").unwrap();
        assert_eq!(
            parsed.read_mono_strings(10).unwrap()[0].text.trim_end(),
            "Hola viajero!"
        );
        assert_eq!(
            parsed.read_mono_strings(11).unwrap()[0].text,
            "Hello traveler!"
        );
    }

    #[test]
    fn technical_object_name_legacy_writes_are_rejected() {
        use locust_core::extraction::FormatPlugin;
        use locust_core::models::StringEntry;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("resources.assets");
        let bytes = identified_mono_fixture(
            (
                "LiftGammaGain",
                "UnityEngine.Rendering.Universal",
                "Unity.RenderPipelines.Universal.Runtime",
            ),
            "LiftGammaGain",
            &["Hello traveler!"],
            0,
        );
        std::fs::write(&path, &bytes).unwrap();
        let sf = SerializedFile::parse(bytes.clone(), &path).unwrap();
        let name_offset = sf.objects[0].data_abs as usize + 28;
        let mut entry = StringEntry::new("monobehaviour/10/0", "LiftGammaGain", path.clone());
        entry.translation = Some("Nombre".into());
        entry.metadata.insert(
            "extraction_method".into(),
            serde_json::json!("monobehaviour"),
        );
        entry
            .metadata
            .insert("field_index".into(), serde_json::json!(0));
        entry
            .metadata
            .insert("path_id".into(), serde_json::json!(10));
        entry
            .metadata
            .insert("mono_string_offset".into(), serde_json::json!(name_offset));
        entry
            .metadata
            .insert("mono_string_byte_len".into(), serde_json::json!(13));
        let report = crate::unity::UnityPlugin::new()
            .inject(dir.path(), &[entry])
            .unwrap();
        assert_eq!(report.strings_written, 0);
        assert_eq!(report.strings_skipped, 1);
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        // Even a stale offset into the name, or a slot covering name + text,
        // cannot bypass protection through the shared fixed-slot writer.
        for (offset, len) in [(name_offset, 13), (name_offset + 4, 8), (name_offset, 32)] {
            let mut after = bytes.clone();
            assert!(rewrite_text_asset_script_inplace(
                &mut after,
                offset,
                len,
                "Changed",
                path.to_str().unwrap()
            )
            .is_err());
            assert_eq!(after, bytes);
        }
    }

    #[test]
    fn technical_object_name_tmp_menu_keeps_display_and_duplicate_instances() {
        for text in ["MENU", "存"] {
            let p = ui05_payload(
                UI05_TMP_SCHEMA,
                false,
                &[("m_text", text), ("m_Name", "MENU")],
                &[],
            );
            let bytes = ui05_asset(
                ("TextMeshProUGUI", "TMPro", "Unity.TextMeshPro"),
                UI05_TMP_HASH,
                p,
                false,
                None,
            );
            let sf = SerializedFile::parse(bytes, "menu.assets").unwrap();
            for id in [10, 11] {
                let fields = sf.read_mono_strings(id).unwrap();
                assert_eq!(
                    fields
                        .iter()
                        .map(|f| (f.field_index, f.text.as_str()))
                        .collect::<Vec<_>>(),
                    [(1, text)]
                );
            }
        }
    }

    #[test]
    fn technical_object_name_unknown_custom_labels_remain_eligible() {
        for (class, namespace) in [
            ("MemoryType", ""),
            ("LiftGammaGain", "Game.Custom"),
            ("Script", "Game.Custom"),
        ] {
            let bytes = identified_mono_fixture(
                (class, namespace, "Assembly-CSharp"),
                "LiftGammaGain",
                &["Hello traveler!"],
                0,
            );
            let sf = SerializedFile::parse(bytes, "custom.assets").unwrap();
            let fields = sf.read_mono_strings(10).unwrap();
            assert_eq!(
                fields.iter().map(|f| f.field_index).collect::<Vec<_>>(),
                [0, 1]
            );
        }
        let bytes = write_v17_mono_fixture("LiftGammaGain", &["Hello traveler!"]);
        let sf = SerializedFile::parse(bytes, "unresolved.assets").unwrap();
        assert_eq!(sf.read_mono_strings(10).unwrap().len(), 2);
    }

    #[test]
    fn technical_object_name_type_categories_keep_actual_script_strings() {
        for identity in [
            (
                "Tile",
                "UnityEngine.Tilemaps",
                "UnityEngine.TilemapModule.dll",
            ),
            ("CubismMoc", "Live2D.Cubism.Core", "Assembly-CSharp.dll"),
            (
                "AudioConfiguration",
                "Naninovel",
                "Elringus.Naninovel.Runtime.dll",
            ),
            ("Script", "Naninovel", "Elringus.Naninovel.Runtime.dll"),
            ("TMP_FontAsset", "TMPro", "Unity.TextMeshPro"),
            ("Text", "UnityEngine.UI", "UnityEngine.UI.dll"),
            ("TextMeshPro", "TMPro", "Unity.TextMeshPro.dll"),
            (
                "LensDistortion",
                "UnityEngine.Rendering.Universal",
                "Unity.RenderPipelines.Universal.Runtime",
            ),
        ] {
            // Natural-looking names must be excluded by owning type too.
            let bytes = identified_mono_fixture(
                identity,
                "Player facing label",
                &["Hello traveler!", "存"],
                0,
            );
            let sf = SerializedFile::parse(bytes.clone(), "typed.assets").unwrap();
            let fields = sf.read_mono_strings(10).unwrap();
            assert_eq!(
                fields.iter().map(|f| f.field_index).collect::<Vec<_>>(),
                if matches!(identity.0, "Text" | "TextMeshPro") {
                    vec![]
                } else {
                    vec![1, 2]
                },
                "{identity:?}: unsupported UI hashes must stay opaque"
            );
            let mut after = bytes.clone();
            assert!(rewrite_text_asset_script_inplace(
                &mut after,
                sf.objects[0].data_abs as usize + 28,
                19,
                "Cambio",
                "typed.assets"
            )
            .is_err());
            assert_eq!(after, bytes);
        }
    }

    #[test]
    fn technical_object_name_font_family_alias_preserved() {
        let bytes = identified_mono_fixture(
            ("TMP_FontAsset", "TMPro", "Unity.TextMeshPro"),
            "OCR-A",
            &["1.1.0", "", "OCR-A", "Hello traveler!"],
            0,
        );
        let sf = SerializedFile::parse(bytes.clone(), "font.assets").unwrap();
        let fields = sf.read_mono_strings(10).unwrap();
        assert_eq!(
            fields
                .iter()
                .map(|f| (f.field_index, f.text.as_str()))
                .collect::<Vec<_>>(),
            [(4, "Hello traveler!")]
        );
        let mut after = bytes.clone();
        let family_offset = sf.objects[0].data_abs as usize + 28 + 12 + 12 + 4;
        assert_eq!(&bytes[family_offset + 4..family_offset + 9], b"OCR-A");
        assert!(rewrite_text_asset_script_inplace(
            &mut after,
            family_offset,
            5,
            "X",
            "font.assets"
        )
        .is_err());
        assert_eq!(after, bytes);
        rewrite_text_asset_script_inplace(
            &mut after,
            fields[0].len_offset,
            fields[0].byte_len,
            "Hola!",
            "font.assets",
        )
        .unwrap();
        assert_eq!(
            &after[..fields[0].len_offset + 4],
            &bytes[..fields[0].len_offset + 4]
        );
    }

    #[test]
    fn technical_object_name_external_and_bundle_script_identity() {
        let dir = tempfile::tempdir().unwrap();
        let identity = ("Script", "Naninovel", "Elringus.Naninovel.Runtime.dll");
        let bytes = identified_mono_fixture(identity, "Scenario", &["Hello traveler!"], 1);
        let scripts = identified_mono_fixture(identity, "Scenario", &[], 0);
        let path = dir.path().join("resources.assets");
        std::fs::write(dir.path().join("globalgamemanagers.assets"), &scripts).unwrap();
        let sf = SerializedFile::parse(bytes.clone(), &path).unwrap();
        assert_eq!(sf.read_mono_strings(10).unwrap().len(), 1);
        // Missing external metadata cannot be inferred from a same-ID local script.
        let unresolved = SerializedFile::parse(bytes.clone(), "missing/resources.assets").unwrap();
        assert_eq!(unresolved.read_mono_strings(10).unwrap().len(), 2);
        let bundle_path = dir.path().join("data.unity3d");
        let bundle = crate::unity_fs::build_test_bundle(
            &[
                ("resources.assets", &bytes),
                ("globalgamemanagers.assets", &scripts),
            ],
            true,
            8,
            true,
            false,
        );
        std::fs::write(&bundle_path, bundle).unwrap();
        for label in [
            bundle_path.join("resources.assets"),
            format!("{} / resources.assets", bundle_path.display()).into(),
        ] {
            let sf = SerializedFile::parse(bytes.clone(), &label).unwrap();
            assert_eq!(sf.read_mono_strings(10).unwrap().len(), 1);
            let mut after = bytes.clone();
            assert!(rewrite_text_asset_script_inplace(
                &mut after,
                sf.objects[0].data_abs as usize + 28,
                8,
                "Cambio",
                label.to_str().unwrap()
            )
            .is_err());
            assert_eq!(after, bytes);
        }
        let custom_scripts = identified_mono_fixture(
            ("Script", "Game.Custom", "Assembly-CSharp.dll"),
            "Scenario",
            &[],
            0,
        );
        let changed_bundle = crate::unity_fs::build_test_bundle(
            &[
                ("resources.assets", &bytes),
                ("globalgamemanagers.assets", &custom_scripts),
                ("extra.bin", b"new"),
            ],
            true,
            8,
            true,
            false,
        );
        std::fs::write(&bundle_path, changed_bundle).unwrap();
        let changed = SerializedFile::parse(bytes, bundle_path.join("resources.assets")).unwrap();
        assert_eq!(
            changed.read_mono_strings(10).unwrap().len(),
            2,
            "changed bundle must invalidate its cached owning type"
        );
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
            .unwrap_or_else(|| panic!("post-rewrite fields: {fields2:?}"));
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
    fn mono_reextracts_single_cjk_labels_with_original_padding_and_capacity() {
        for (source, target) in [
            ("Quit", "退"),
            ("Yes", "是"),
            ("Save", "存"),
            ("Continue", "继续"),
        ] {
            let bytes = write_v17_mono_fixture("Box", &[source]);
            let sf = SerializedFile::parse(bytes.clone(), "m.assets").unwrap();
            let fields = sf.read_mono_strings(10).unwrap();
            let original = fields.iter().find(|field| field.text == source).unwrap();
            let mut patched = bytes;
            rewrite_text_asset_script_inplace(
                &mut patched,
                original.len_offset,
                original.byte_len,
                target,
                "m.assets",
            )
            .unwrap();
            let parsed = SerializedFile::parse(patched, "m.assets").unwrap();
            let fields = parsed.read_mono_strings(10).unwrap();
            let translated = fields
                .iter()
                .find(|field| field.field_index == original.field_index)
                .unwrap();
            assert_eq!(translated.text.trim_end_matches(' '), target);
            assert_eq!(translated.byte_len, original.byte_len);
            assert_eq!(translated.len_offset, original.len_offset);
        }
        assert!(!mono_name_worth_extracting("A!"));
        assert!(!mono_name_worth_extracting("◆"));
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
    fn is_heuristic_noise_class_excludes_runtime_configuration() {
        assert!(is_heuristic_noise_class(CLASS_ID_MONO_SCRIPT));
        assert!(is_heuristic_noise_class(CLASS_ID_SHADER));
        assert!(is_heuristic_noise_class(CLASS_ID_INPUT_MANAGER));
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
        let bytes = write_v17_mono_fixture_with_class(-42, "DialogBox", &["Welcome, traveler!"]);
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
            texts.contains(&"First line of dialogue"),
            "fields: {texts:?}"
        );
        assert!(
            texts.contains(&"Second line after int"),
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
            fields2.iter().any(|f| f.text.starts_with("Hola linea dos")),
            "post-gap inject must land on second string: {fields2:?}"
        );
        // Gap must not have destroyed the first string.
        assert!(
            fields2.iter().any(|f| f.text == "First line of dialogue"),
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
            texts.contains(&"Portable Speaker"),
            "keep dialogue-ish: {texts:?}"
        );
        assert!(texts.contains(&"Welcome home"), "keep sentence: {texts:?}");
        assert!(
            !texts.iter().any(|t| t.contains("UnityEngine")),
            "drop assembly type: {texts:?}"
        );
        assert!(
            !texts
                .iter()
                .any(|t| t.contains("Version=") || t.contains("PublicKeyToken=")),
            "drop full AQN: {texts:?}"
        );
        assert!(
            !texts
                .iter()
                .any(|t| t.contains("Naninovel.Script") || t.contains("TMP_FontAsset")),
            "drop Naninovel/TMPro AQN: {texts:?}"
        );
        assert!(!texts.contains(&"set_text"), "drop API token: {texts:?}");
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
            !texts.contains(&"Naninovel"),
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
            assert!(!texts.contains(&drop), "expected drop {drop}: {texts:?}");
        }
        assert!(texts.contains(&"Q.SAVE"), "keep quick-save UI: {texts:?}");
        assert!(texts.contains(&"Play"), "keep short UI verb: {texts:?}");
        assert!(texts.contains(&"Save game now"), "keep sentence: {texts:?}");
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
        assert!(texts.contains(&"Play"), "keep UI: {texts:?}");
        assert!(texts.contains(&"Emily"), "keep name: {texts:?}");
        assert!(texts.contains(&"Save game"), "keep sentence: {texts:?}");
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
            assert!(!texts.contains(&drop), "expected drop {drop}: {texts:?}");
        }
        for keep in ["Play", "Emily", "Q.SAVE", "Message speed:"] {
            assert!(texts.contains(&keep), "expected keep {keep}: {texts:?}");
        }
    }

    #[test]
    fn mono_script_filter_drops_asset_paths_with_spaces() {
        assert!(looks_like_unity_asset_path(
            "Naninovel/Audio/BGM/HSCENE_NTR/Erotic 01"
        ));
        assert!(looks_like_unity_asset_path("Tilemap/Pillar Sprite_11"));
        assert!(looks_like_unity_asset_path("Shaders/TMP_SDF Overlay"));
        assert!(looks_like_unity_asset_path("Day/1 Centered"));
        assert!(looks_like_unity_asset_path("Night/2 Centered"));
        assert!(looks_like_unity_asset_path("UI/btn arrow left"));
        assert!(looks_like_unity_asset_path(
            "Fonts & Materials/LiberationSans SDF - Outline"
        ));
        // Real UI with slash separators (not asset roots).
        assert!(!looks_like_unity_asset_path("START / LOAD"));
        assert!(!looks_like_unity_asset_path("Fridge / Microwave"));
        assert!(!looks_like_unity_asset_path("LOADING / PLEASE WAIT..."));
        assert!(!looks_like_unity_asset_path("MM / DD / YYYY"));
        assert!(!looks_like_unity_asset_path("Play"));

        let bytes = write_v17_mono_fixture(
            "Audio",
            &[
                "Naninovel/Audio/BGM/results_ (1)",
                "Tilemap/Demo Tilemap (Dungeon)",
                "Sprites/Floor Sprite",
                "Day/2 Standard",
                "START / LOAD",
                "Fridge / Microwave",
                "Play",
                "Emily",
            ],
        );
        let sf = SerializedFile::parse(bytes, "paths.assets").unwrap();
        let fields = sf.read_mono_strings(10).unwrap();
        let texts: Vec<&str> = fields.iter().map(|f| f.text.as_str()).collect();
        for drop in [
            "Naninovel/Audio/BGM/results_ (1)",
            "Tilemap/Demo Tilemap (Dungeon)",
            "Sprites/Floor Sprite",
            "Day/2 Standard",
        ] {
            assert!(!texts.contains(&drop), "expected drop {drop}: {texts:?}");
        }
        for keep in ["START / LOAD", "Fridge / Microwave", "Play", "Emily"] {
            assert!(texts.contains(&keep), "expected keep {keep}: {texts:?}");
        }
    }

    #[test]
    fn mono_script_filter_drops_dev_separator_banners() {
        assert!(looks_like_dev_separator_banner(
            "------------------------------ SETUP LIGHTS IN GALLERY MODE"
        ));
        assert!(looks_like_dev_separator_banner(
            "------------------------------ SPAWN TOPDOWN SPRITES"
        ));
        assert!(looks_like_dev_separator_banner("--------------------"));
        assert!(!looks_like_dev_separator_banner("---- short"));
        assert!(!looks_like_dev_separator_banner("----------")); // < 12 chars
        assert!(!looks_like_dev_separator_banner(
            "Please wait — loading your save..."
        ));
        assert!(!looks_like_dev_separator_banner("Play"));

        let bytes = write_v17_mono_fixture(
            "Scene",
            &[
                "------------------------------ SETUP LIGHTS IN GALLERY MODE",
                "------------------------------ SPAWN TOPDOWN SPRITES",
                "Play",
                "Emily",
                "Option A",
            ],
        );
        let sf = SerializedFile::parse(bytes, "banner.assets").unwrap();
        let fields = sf.read_mono_strings(10).unwrap();
        let texts: Vec<&str> = fields.iter().map(|f| f.text.as_str()).collect();
        assert!(
            !texts.iter().any(|t| t.contains("SETUP LIGHTS")),
            "drop gallery banner: {texts:?}"
        );
        assert!(
            !texts.iter().any(|t| t.contains("SPAWN TOPDOWN")),
            "drop spawn banner: {texts:?}"
        );
        for keep in ["Play", "Emily", "Option A"] {
            assert!(texts.contains(&keep), "expected keep {keep}: {texts:?}");
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
            assert!(!texts.contains(&drop), "expected drop {drop}: {texts:?}");
        }
        for keep in [
            "Play",
            "Wait",
            "Option A",
            "Emily",
            "Q.LOAD",
            "Message speed:",
        ] {
            assert!(texts.contains(&keep), "expected keep {keep}: {texts:?}");
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
        assert!(texts.contains(&"New Game"), "fields: {texts:?}");
        assert!(texts.contains(&"Load Game"), "fields: {texts:?}");
        assert!(texts.contains(&"Options"), "fields: {texts:?}");
        // Count u32 must not appear as a garbage 3-byte "string".
        assert!(
            !texts
                .iter()
                .any(|t| t.len() == 3 && t.as_bytes().iter().all(|b| *b < 0x20)),
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
            texts.contains(&"Yes"),
            "short string must extract as itself: {texts:?}"
        );
        assert!(
            texts.contains(&"See you later."),
            "following string must still extract: {texts:?}"
        );
    }
}

/// Build interleaved object tables from the existing single-object fixtures.
/// Each group has five MonoBehaviours (including negative script classes), one
/// TextMesh, one GUIText and three non-text objects, with descending unique ids.
#[cfg(test)]
pub(crate) fn write_v17_mixed_objects_fixture(groups: usize) -> Vec<u8> {
    let templates: Vec<_> = [
        write_v17_mono_fixture("DialogBox", &["Welcome, traveler!", "See you later."]),
        write_v17_mono_fixture_with_class(-123, "Menu", &["Choose your destination."]),
        write_v17_textmesh_fixture("Hello, world!"),
        write_v17_guitext_fixture("Press Start"),
        write_v17_mono_fixture_with_class(1, "", &[]),
    ]
    .into_iter()
    .map(|bytes| SerializedFile::parse(bytes, "template.assets").unwrap())
    .collect();
    let pattern = [4, 0, 1, 4, 0, 2, 1, 4, 0, 3];
    let object_count = groups * pattern.len();
    let mut meta = b"2019.4.0f1\0".to_vec();
    meta.extend_from_slice(&1u32.to_le_bytes()); // target platform
    meta.push(0); // no type tree
    meta.extend_from_slice(&(templates.len() as i32).to_le_bytes());
    for template in &templates {
        let ty = &template.types[0];
        meta.extend_from_slice(&ty.class_id.to_le_bytes());
        meta.push(0);
        meta.extend_from_slice(&ty.script_type_index.to_le_bytes());
        if is_monobehaviour_class(ty.class_id) {
            meta.extend_from_slice(&[0; 16]); // script_id
        }
        meta.extend_from_slice(&[0; 16]); // old_type_hash
    }
    meta.extend_from_slice(&(object_count as i32).to_le_bytes());
    let mut data = Vec::new();
    for (index, type_index) in pattern.into_iter().cycle().take(object_count).enumerate() {
        let template = &templates[type_index];
        let obj = &template.objects[0];
        let start = obj.data_abs as usize;
        meta.resize((meta.len() + 3) & !3, 0);
        data.resize((data.len() + 3) & !3, 0);
        meta.extend_from_slice(&((object_count - index) as i64).to_le_bytes());
        meta.extend_from_slice(&(data.len() as u32).to_le_bytes());
        meta.extend_from_slice(&obj.byte_size.to_le_bytes());
        meta.extend_from_slice(&(type_index as i32).to_le_bytes());
        data.extend_from_slice(&template.data[start..start + obj.byte_size as usize]);
    }
    meta.extend_from_slice(&0i32.to_le_bytes()); // script types
    meta.extend_from_slice(&0i32.to_le_bytes()); // externals
    meta.push(0); // user information
    let data_offset = (20 + meta.len() + 15) & !15;
    let mut out = Vec::new();
    out.extend_from_slice(&(meta.len() as u32).to_be_bytes());
    out.extend_from_slice(&((data_offset + data.len()) as u32).to_be_bytes());
    out.extend_from_slice(&17u32.to_be_bytes());
    out.extend_from_slice(&(data_offset as u32).to_be_bytes());
    out.extend_from_slice(&[0; 4]); // little endian + reserved
    out.extend_from_slice(&meta);
    out.resize(data_offset, 0);
    out.extend_from_slice(&data);
    out
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
    write_v17_technical_noise_fixture(CLASS_ID_MONO_SCRIPT, &["Naninovel", "QuaternionTween"])
}

#[cfg(test)]
pub fn write_v17_technical_noise_fixture(class_id: i32, strings: &[&str]) -> Vec<u8> {
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

    // Representative length-prefixed strings from a technical object.
    let mut ms_payload = Vec::new();
    for text in strings {
        write_aligned_string(&mut ms_payload, text);
    }

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
    // type 1: selected technical class
    meta.extend_from_slice(&class_id.to_le_bytes());
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

#[cfg(test)]
#[path = "unity_growth_tests.rs"]
mod growth_tests;
