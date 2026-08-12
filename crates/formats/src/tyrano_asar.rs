//! Electron `app.asar` archive reader/writer for the TyranoBuilder plugin.
//!
//! # Format (Chromium Pickle + JSON directory)
//! Layout matches [electron/asar](https://github.com/electron/asar) (Pickle-framed
//! JSON header, then concatenated file payloads). Framing (four LE u32s then JSON):
//! - `[0] = 4` (outer pickle payload size — one u32)
//! - `[1]` = total size of the header pickle buffer
//! - `[2]` = header pickle payload size
//! - `[3]` = JSON UTF-8 length
//! - JSON starts at absolute offset 16; payload base = `8 + [1]`.
//!
//! Each file node has `"offset"` (decimal **string**, relative to base) and
//! `"size"` (number). Directories are `{"files": { ... }}`. Nodes with
//! `"unpacked": true` store bytes under sibling `app.asar.unpacked/<path>`.
//!
//! # Integrity
//! Newer asar JSON may include per-file `"integrity"` hash blocks. On rebuild we
//! preserve unknown fields on **untouched** nodes; for **modified** files we drop
//! `"integrity"` (Electron only enforces it when packaged with fuses).
//!
//! # Out of scope
//! Compressed asar variants, fuse-gated integrity recompute.
//! (NW.js `package.nw` / `data.exe` live in [`crate::tyrano_nw`].)

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

/// One file listed in the asar JSON tree.
#[derive(Clone, Debug)]
pub struct AsarEntry {
    /// Archive-internal path with `/` separators (e.g. `data/scenario/a.ks`).
    pub path: String,
    /// Offset relative to payload base (ignored when `unpacked`).
    pub offset: u64,
    pub size: u64,
    pub unpacked: bool,
}

#[derive(Clone, Debug)]
pub struct AsarArchive {
    pub path: PathBuf,
    pub data: Vec<u8>,
    /// Absolute file offset where payloads begin (`8 + header_pickle_size`).
    pub base_offset: u64,
    /// Parsed JSON header root object.
    pub header: Value,
    pub entries: Vec<AsarEntry>,
}

#[derive(Debug)]
pub struct AsarError {
    pub file: String,
    pub message: String,
}

impl std::fmt::Display for AsarError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.file, self.message)
    }
}

impl std::error::Error for AsarError {}

fn err(file: &str, message: impl Into<String>) -> AsarError {
    AsarError {
        file: file.into(),
        message: message.into(),
    }
}

fn read_u32(data: &[u8], off: usize) -> Result<u32, AsarError> {
    if off.checked_add(4).filter(|e| *e <= data.len()).is_none() {
        return Err(err("asar", "truncated u32"));
    }
    Ok(u32::from_le_bytes([
        data[off],
        data[off + 1],
        data[off + 2],
        data[off + 3],
    ]))
}

// ─── Pickle helpers ────────────────────────────────────────────────────────

/// Chromium Pickle buffer holding a single UTF-8 string (header JSON).
fn pickle_write_string(s: &str) -> Vec<u8> {
    let bytes = s.as_bytes();
    let pad = (4 - (bytes.len() % 4)) % 4;
    let payload_size = 4 + bytes.len() + pad; // u32 len + data + pad
    let mut out = Vec::with_capacity(4 + payload_size);
    out.extend_from_slice(&(payload_size as u32).to_le_bytes());
    out.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
    out.extend_from_slice(bytes);
    out.extend(std::iter::repeat_n(0u8, pad));
    out
}

/// Outer size pickle: payload is one u32 (header pickle total length).
fn pickle_write_u32(v: u32) -> Vec<u8> {
    let mut out = Vec::with_capacity(8);
    out.extend_from_slice(&4u32.to_le_bytes());
    out.extend_from_slice(&v.to_le_bytes());
    out
}

// ─── Reader ────────────────────────────────────────────────────────────────

fn walk_files(
    node: &Value,
    prefix: &str,
    out: &mut Vec<AsarEntry>,
    label: &str,
) -> Result<(), AsarError> {
    let obj = node
        .as_object()
        .ok_or_else(|| err(label, "directory node is not an object"))?;
    if let Some(files) = obj.get("files") {
        let map = files
            .as_object()
            .ok_or_else(|| err(label, "\"files\" is not an object"))?;
        for (name, child) in map {
            let path = if prefix.is_empty() {
                name.clone()
            } else {
                format!("{prefix}/{name}")
            };
            if child.get("files").is_some() {
                walk_files(child, &path, out, label)?;
            } else {
                let unpacked = child
                    .get("unpacked")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                let size = child
                    .get("size")
                    .and_then(|v| v.as_u64())
                    .ok_or_else(|| err(label, format!("file node {path} missing numeric size")))?;
                let offset = if unpacked {
                    0
                } else {
                    let off_v = child
                        .get("offset")
                        .ok_or_else(|| err(label, format!("file node {path} missing offset")))?;
                    parse_offset(off_v, label, &path)?
                };
                out.push(AsarEntry {
                    path,
                    offset,
                    size,
                    unpacked,
                });
            }
        }
    }
    Ok(())
}

fn parse_offset(v: &Value, label: &str, path: &str) -> Result<u64, AsarError> {
    match v {
        Value::String(s) => s.parse::<u64>().map_err(|_| {
            err(
                label,
                format!("offset for {path} is not a decimal number: {s:?}"),
            )
        }),
        Value::Number(n) => n
            .as_u64()
            .ok_or_else(|| err(label, format!("offset for {path} is not a u64"))),
        _ => Err(err(
            label,
            format!("offset for {path} has invalid JSON type"),
        )),
    }
}

impl AsarArchive {
    pub fn open(path: &Path) -> Result<Self, AsarError> {
        let label = path.display().to_string();
        let data = std::fs::read(path).map_err(|e| err(&label, format!("read failed: {e}")))?;
        Self::from_bytes(path.to_path_buf(), data)
    }

    pub fn from_bytes(path: PathBuf, data: Vec<u8>) -> Result<Self, AsarError> {
        let label = path.display().to_string();
        if data.len() < 16 {
            return Err(err(&label, "file too small for asar pickle header"));
        }
        let u0 = read_u32(&data, 0)?;
        let header_pickle_size = read_u32(&data, 4)? as usize;
        let _payload_size = read_u32(&data, 8)?;
        let json_len = read_u32(&data, 12)? as usize;

        if u0 != 4 {
            return Err(err(
                &label,
                format!("unexpected asar size-pickle payload size {u0} (expected 4)"),
            ));
        }

        let json_end = 16usize
            .checked_add(json_len)
            .ok_or_else(|| err(&label, "JSON length overflow"))?;
        if json_end > data.len() {
            return Err(err(&label, "JSON header truncated"));
        }
        let base_offset = 8usize
            .checked_add(header_pickle_size)
            .ok_or_else(|| err(&label, "base offset overflow"))?;
        if base_offset > data.len() {
            return Err(err(
                &label,
                format!("header pickle size {header_pickle_size} exceeds file"),
            ));
        }

        let json_str = std::str::from_utf8(&data[16..json_end])
            .map_err(|_| err(&label, "JSON header is not valid UTF-8"))?;
        let header: Value = serde_json::from_str(json_str)
            .map_err(|e| err(&label, format!("JSON parse failed: {e}")))?;

        let mut entries = Vec::new();
        walk_files(&header, "", &mut entries, &label)?;

        // Bounds-check packed entries
        for e in &entries {
            if e.unpacked {
                continue;
            }
            let start = (base_offset as u64)
                .checked_add(e.offset)
                .ok_or_else(|| err(&label, format!("offset overflow for {}", e.path)))?;
            let end = start
                .checked_add(e.size)
                .ok_or_else(|| err(&label, format!("size overflow for {}", e.path)))?;
            if end > data.len() as u64 {
                return Err(err(
                    &label,
                    format!(
                        "entry {} payload past EOF (offset={}, size={})",
                        e.path, e.offset, e.size
                    ),
                ));
            }
        }

        Ok(Self {
            path,
            data,
            base_offset: base_offset as u64,
            header,
            entries,
        })
    }

    /// Directory next to the archive used for unpacked files (`foo.asar.unpacked`).
    pub fn unpacked_dir(&self) -> PathBuf {
        let mut s = self.path.to_string_lossy().into_owned();
        s.push_str(".unpacked");
        PathBuf::from(s)
    }

    pub fn read_entry(&self, entry: &AsarEntry) -> Result<Vec<u8>, AsarError> {
        let label = self.path.display().to_string();
        if entry.unpacked {
            let disk = self.unpacked_dir().join(entry.path.replace('/', std::path::MAIN_SEPARATOR_STR));
            return std::fs::read(&disk).map_err(|e| {
                err(
                    &label,
                    format!("unpacked read {} failed: {e}", entry.path),
                )
            });
        }
        let start = self
            .base_offset
            .checked_add(entry.offset)
            .ok_or_else(|| err(&label, "offset overflow"))? as usize;
        let end = start
            .checked_add(entry.size as usize)
            .filter(|e| *e <= self.data.len())
            .ok_or_else(|| err(&label, format!("payload OOB for {}", entry.path)))?;
        Ok(self.data[start..end].to_vec())
    }

    pub fn scenario_ks_entries(&self) -> impl Iterator<Item = &AsarEntry> {
        self.entries.iter().filter(|e| {
            let p = e.path.replace('\\', "/");
            let lower = p.to_ascii_lowercase();
            lower.contains("data/scenario/")
                && Path::new(&p)
                    .extension()
                    .and_then(|x| x.to_str())
                    .map(|x| x.eq_ignore_ascii_case("ks"))
                    .unwrap_or(false)
        })
    }

    /// Bounded header peek from disk (never loads payloads — game asars can be GBs).
    pub fn peek_header_mentions_scenario(path: &Path) -> bool {
        use std::io::Read;
        let Ok(mut f) = std::fs::File::open(path) else {
            return false;
        };
        let mut head = [0u8; 16];
        if f.read_exact(&mut head).is_err() {
            return false;
        }
        let Ok(json_len) = read_u32(&head, 12).map(|v| v as usize) else {
            return false;
        };
        // 64 MiB cap: a JSON directory bigger than that is not a real asar header.
        if json_len > 64 * 1024 * 1024 {
            return false;
        }
        let mut json = vec![0u8; json_len];
        if f.read_exact(&mut json).is_err() {
            return false;
        }
        String::from_utf8_lossy(&json).contains("scenario")
    }

    /// Cheap check: JSON text mentions scenario paths.
    pub fn header_mentions_scenario(data: &[u8]) -> bool {
        if data.len() < 16 {
            return false;
        }
        let Ok(json_len) = read_u32(data, 12).map(|v| v as usize) else {
            return false;
        };
        let end = 16usize.saturating_add(json_len).min(data.len());
        if end <= 16 {
            return false;
        }
        let s = String::from_utf8_lossy(&data[16..end]);
        s.contains("scenario")
    }
}

// ─── Writer / rebuild ──────────────────────────────────────────────────────

/// Build a fresh asar from `(path, bytes)` (all packed, no unpacked).
/// Paths use `/`. Used for synthetic fixtures.
pub fn write_asar(files: &[(String, Vec<u8>)]) -> Result<Vec<u8>, AsarError> {
    let mut ordered: Vec<(String, Vec<u8>)> = files.to_vec();
    ordered.sort_by(|a, b| a.0.cmp(&b.0));

    let mut files_map = Map::new();
    let mut cursor: u64 = 0;
    for (path, payload) in &ordered {
        insert_file_node_map(&mut files_map, path, cursor, payload.len() as u64, false)?;
        cursor = cursor
            .checked_add(payload.len() as u64)
            .ok_or_else(|| err("asar", "payload offset overflow"))?;
    }

    let mut root = Map::new();
    root.insert("files".into(), Value::Object(files_map));
    let header = Value::Object(root);
    let json = serde_json::to_string(&header)
        .map_err(|e| err("asar", format!("JSON serialize failed: {e}")))?;
    let header_pickle = pickle_write_string(&json);
    let size_pickle = pickle_write_u32(header_pickle.len() as u32);

    let mut out = Vec::with_capacity(size_pickle.len() + header_pickle.len() + cursor as usize);
    out.extend_from_slice(&size_pickle);
    out.extend_from_slice(&header_pickle);
    for (_, payload) in &ordered {
        out.extend_from_slice(payload);
    }
    Ok(out)
}

fn insert_file_node_map(
    files: &mut Map<String, Value>,
    path: &str,
    offset: u64,
    size: u64,
    unpacked: bool,
) -> Result<(), AsarError> {
    let parts: Vec<&str> = path.split('/').filter(|p| !p.is_empty()).collect();
    if parts.is_empty() {
        return Err(err("asar", "empty path"));
    }
    fn go(
        files: &mut Map<String, Value>,
        parts: &[&str],
        offset: u64,
        size: u64,
        unpacked: bool,
    ) -> Result<(), AsarError> {
        let part = parts[0];
        if parts.len() == 1 {
            let mut node = Map::new();
            node.insert("size".into(), Value::Number(size.into()));
            if unpacked {
                node.insert("unpacked".into(), Value::Bool(true));
            } else {
                node.insert("offset".into(), Value::String(offset.to_string()));
            }
            files.insert(part.to_string(), Value::Object(node));
            return Ok(());
        }
        if !files.contains_key(part) {
            let mut m = Map::new();
            m.insert("files".into(), Value::Object(Map::new()));
            files.insert(part.to_string(), Value::Object(m));
        }
        let child = files
            .get_mut(part)
            .and_then(|v| v.as_object_mut())
            .ok_or_else(|| err("asar", format!("path conflict at {part}")))?;
        let nested = child
            .get_mut("files")
            .and_then(|v| v.as_object_mut())
            .ok_or_else(|| err("asar", format!("path conflict at {part}")))?;
        go(nested, &parts[1..], offset, size, unpacked)
    }
    go(files, &parts, offset, size, unpacked)
}

/// Rebuild asar: replace listed payloads (inner path → bytes). Untouched packed
/// payloads are copied byte-identically. Modified nodes drop `"integrity"`.
/// Unpacked replacements are written to disk by the caller (this only rebuilds
/// the archive body for packed files; header still lists unpacked entries).
pub fn rebuild_asar(
    original: &AsarArchive,
    replacements: &HashMap<String, Vec<u8>>,
) -> Result<Vec<u8>, AsarError> {
    let label = original.path.display().to_string();
    let mut header = original.header.clone();

    // Collect file nodes in walk order with current metadata.
    let mut plan: Vec<Planned> = Vec::new();
    collect_plan(&header, "", &mut plan, &label)?;

    // Assign new offsets and sizes for packed files; copy or replace payloads.
    let mut cursor: u64 = 0;
    let mut blobs: Vec<Vec<u8>> = Vec::new();
    for p in &mut plan {
        if p.unpacked {
            if let Some(new) = replacements.get(&p.path) {
                // Caller writes unpacked to disk; update size in header only.
                p.new_size = Some(new.len() as u64);
            }
            continue;
        }
        let payload = if let Some(new) = replacements.get(&p.path) {
            p.modified = true;
            p.new_size = Some(new.len() as u64);
            new.clone()
        } else {
            // Byte-identical copy from original archive.
            let e = original
                .entries
                .iter()
                .find(|e| e.path == p.path)
                .ok_or_else(|| err(&label, format!("missing original entry {}", p.path)))?;
            original.read_entry(e)?
        };
        p.new_offset = Some(cursor);
        p.new_size = Some(payload.len() as u64);
        cursor = cursor
            .checked_add(payload.len() as u64)
            .ok_or_else(|| err(&label, "payload offset overflow"))?;
        blobs.push(payload);
    }

    // Patch JSON tree
    apply_plan(&mut header, "", &plan, &label)?;

    let json = serde_json::to_string(&header)
        .map_err(|e| err(&label, format!("JSON serialize failed: {e}")))?;
    let header_pickle = pickle_write_string(&json);
    let size_pickle = pickle_write_u32(header_pickle.len() as u32);

    let mut out = Vec::with_capacity(size_pickle.len() + header_pickle.len() + cursor as usize);
    out.extend_from_slice(&size_pickle);
    out.extend_from_slice(&header_pickle);
    for b in blobs {
        out.extend_from_slice(&b);
    }
    Ok(out)
}

#[derive(Clone, Debug)]
struct Planned {
    path: String,
    unpacked: bool,
    modified: bool,
    new_offset: Option<u64>,
    new_size: Option<u64>,
}

fn collect_plan(
    node: &Value,
    prefix: &str,
    out: &mut Vec<Planned>,
    label: &str,
) -> Result<(), AsarError> {
    let obj = node
        .as_object()
        .ok_or_else(|| err(label, "node is not object"))?;
    let Some(files) = obj.get("files").and_then(|v| v.as_object()) else {
        return Ok(());
    };
    for (name, child) in files {
        let path = if prefix.is_empty() {
            name.clone()
        } else {
            format!("{prefix}/{name}")
        };
        if child.get("files").is_some() {
            collect_plan(child, &path, out, label)?;
        } else {
            let unpacked = child
                .get("unpacked")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            out.push(Planned {
                path,
                unpacked,
                modified: false,
                new_offset: None,
                new_size: None,
            });
        }
    }
    Ok(())
}

fn apply_plan(
    node: &mut Value,
    prefix: &str,
    plan: &[Planned],
    label: &str,
) -> Result<(), AsarError> {
    let obj = node
        .as_object_mut()
        .ok_or_else(|| err(label, "node is not object"))?;
    let Some(files_v) = obj.get_mut("files") else {
        return Ok(());
    };
    let files = files_v
        .as_object_mut()
        .ok_or_else(|| err(label, "files not object"))?;

    // Collect keys first to avoid borrow issues
    let keys: Vec<String> = files.keys().cloned().collect();
    for name in keys {
        let path = if prefix.is_empty() {
            name.clone()
        } else {
            format!("{prefix}/{name}")
        };
        let child = files
            .get_mut(&name)
            .ok_or_else(|| err(label, "missing child"))?;
        if child.get("files").is_some() {
            apply_plan(child, &path, plan, label)?;
        } else if let Some(p) = plan.iter().find(|p| p.path == path) {
            let map = child
                .as_object_mut()
                .ok_or_else(|| err(label, "file node not object"))?;
            if let Some(sz) = p.new_size {
                map.insert("size".into(), Value::Number(sz.into()));
            }
            if !p.unpacked {
                if let Some(off) = p.new_offset {
                    map.insert("offset".into(), Value::String(off.to_string()));
                }
            }
            if p.modified {
                map.remove("integrity");
            }
        }
    }
    Ok(())
}

// ─── Tests ─────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn tempdir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("locust_asar_{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn test_pickle_framing_and_json() {
        let files = vec![
            ("data/scenario/a.ks".into(), b"hello".to_vec()),
            ("data/scenario/b.ks".into(), b"world!!".to_vec()),
        ];
        let bytes = write_asar(&files).unwrap();
        assert_eq!(read_u32(&bytes, 0).unwrap(), 4);
        let arch = AsarArchive::from_bytes(PathBuf::from("t.asar"), bytes).unwrap();
        assert_eq!(arch.entries.len(), 2);
        assert!(arch.base_offset >= 16);
        // Nested dirs present
        assert!(arch.header.pointer("/files/data/files/scenario/files/a.ks").is_some());
    }

    #[test]
    fn test_offset_as_string_and_payloads() {
        let files = vec![("f.txt".into(), b"ABCDEFGH".to_vec())];
        let bytes = write_asar(&files).unwrap();
        let arch = AsarArchive::from_bytes(PathBuf::from("t.asar"), bytes).unwrap();
        let e = &arch.entries[0];
        assert_eq!(e.offset, 0);
        assert_eq!(arch.read_entry(e).unwrap(), b"ABCDEFGH");
        // JSON offset is a string
        let off = arch
            .header
            .pointer("/files/f.txt/offset")
            .unwrap()
            .as_str()
            .unwrap();
        assert_eq!(off, "0");
    }

    #[test]
    fn test_unpacked_entry_from_sibling_dir() {
        let dir = tempdir();
        let asar_path = dir.join("app.asar");
        // Manual header with one unpacked file
        let json = r#"{"files":{"data":{"files":{"loose.ks":{"size":5,"unpacked":true}}}}}"#;
        let header_pickle = pickle_write_string(json);
        let size_pickle = pickle_write_u32(header_pickle.len() as u32);
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&size_pickle);
        bytes.extend_from_slice(&header_pickle);
        fs::write(&asar_path, &bytes).unwrap();

        let unpacked = dir.join("app.asar.unpacked").join("data");
        fs::create_dir_all(&unpacked).unwrap();
        fs::write(unpacked.join("loose.ks"), b"hello").unwrap();

        let arch = AsarArchive::open(&asar_path).unwrap();
        let e = arch.entries.iter().find(|e| e.unpacked).unwrap();
        assert_eq!(arch.read_entry(e).unwrap(), b"hello");
    }

    #[test]
    fn test_rebuild_resize_offsets_and_roundtrip() {
        let files = vec![
            ("a.txt".into(), b"AAA".to_vec()),
            ("b.txt".into(), b"BBBBBB".to_vec()),
            ("c.txt".into(), b"CC".to_vec()),
        ];
        let bytes = write_asar(&files).unwrap();
        let arch = AsarArchive::from_bytes(PathBuf::from("r.asar"), bytes).unwrap();
        let mut repl = HashMap::new();
        repl.insert("b.txt".into(), b"B-NEW-LONGER".to_vec());
        let rebuilt = rebuild_asar(&arch, &repl).unwrap();
        let again = AsarArchive::from_bytes(PathBuf::from("r2.asar"), rebuilt).unwrap();

        let a = again.entries.iter().find(|e| e.path == "a.txt").unwrap();
        let b = again.entries.iter().find(|e| e.path == "b.txt").unwrap();
        let c = again.entries.iter().find(|e| e.path == "c.txt").unwrap();
        assert_eq!(again.read_entry(a).unwrap(), b"AAA");
        assert_eq!(again.read_entry(b).unwrap(), b"B-NEW-LONGER");
        assert_eq!(again.read_entry(c).unwrap(), b"CC");
        // Offsets recomputed sequentially
        assert_eq!(a.offset, 0);
        assert_eq!(b.offset, 3);
        assert_eq!(c.offset, 3 + 12);
    }

    #[test]
    fn test_malformed_bad_framing() {
        let e = AsarArchive::from_bytes(PathBuf::from("x.asar"), b"short".to_vec())
            .unwrap_err()
            .to_string();
        assert!(e.contains("small") || e.contains("truncated"), "{e}");
    }

    #[test]
    fn test_malformed_json_truncated() {
        let mut data = vec![0u8; 16];
        data[0..4].copy_from_slice(&4u32.to_le_bytes());
        data[4..8].copy_from_slice(&100u32.to_le_bytes());
        data[8..12].copy_from_slice(&96u32.to_le_bytes());
        data[12..16].copy_from_slice(&50u32.to_le_bytes()); // claims 50 JSON bytes
        let e = AsarArchive::from_bytes(PathBuf::from("t.asar"), data)
            .unwrap_err()
            .to_string();
        assert!(e.contains("truncated") || e.contains("exceeds"), "{e}");
    }

    #[test]
    fn test_malformed_offset_past_eof() {
        // Valid tiny asar then claim huge size
        let files = vec![("a.txt".into(), b"hi".to_vec())];
        let mut bytes = write_asar(&files).unwrap();
        // Corrupt JSON size field by rebuilding is hard; inject bad entry via hand JSON
        let json = r#"{"files":{"a.txt":{"size":999999,"offset":"0"}}}"#;
        let header_pickle = pickle_write_string(json);
        let size_pickle = pickle_write_u32(header_pickle.len() as u32);
        bytes.clear();
        bytes.extend_from_slice(&size_pickle);
        bytes.extend_from_slice(&header_pickle);
        bytes.extend_from_slice(b"hi");
        let e = AsarArchive::from_bytes(PathBuf::from("t.asar"), bytes)
            .unwrap_err()
            .to_string();
        assert!(e.contains("EOF") || e.contains("past"), "{e}");
    }

    #[test]
    fn test_malformed_offset_not_number() {
        let json = r#"{"files":{"a.txt":{"size":1,"offset":true}}}"#;
        let header_pickle = pickle_write_string(json);
        let size_pickle = pickle_write_u32(header_pickle.len() as u32);
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&size_pickle);
        bytes.extend_from_slice(&header_pickle);
        bytes.push(b'x');
        let e = AsarArchive::from_bytes(PathBuf::from("t.asar"), bytes)
            .unwrap_err()
            .to_string();
        assert!(e.contains("offset") || e.contains("invalid"), "{e}");
    }

    #[test]
    fn test_header_mentions_scenario() {
        let files = vec![("data/scenario/x.ks".into(), b";".to_vec())];
        let bytes = write_asar(&files).unwrap();
        assert!(AsarArchive::header_mentions_scenario(&bytes));
        let other = write_asar(&[("readme.txt".into(), b"x".to_vec())]).unwrap();
        assert!(!AsarArchive::header_mentions_scenario(&other));
    }
}
