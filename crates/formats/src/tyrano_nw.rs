//! NW.js container support for TyranoBuilder: `package.nw` and self-extracting
//! `data.exe` / game `.exe` with an appended ZIP.
//!
//! # Layout
//! - **package.nw** — plain ZIP (`package.json` + `data/scenario/*.ks` + assets).
//! - **data.exe** — PE/MZ stub bytes, then a ZIP (EOCD at the end). Central-directory
//!   offsets are relative to the ZIP start; the stub length is `zip_start` so rebuild
//!   can write `prefix || rebuilt_zip`.
//!
//! # Zip-bomb guard
//! Scenario archives are tiny. We refuse a declared uncompressed total above
//! [`MAX_NW_UNCOMPRESSED`] (1 GiB) before fully buffering entries.

use std::collections::HashMap;
use std::fs::File;
use std::io::{Cursor, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use zip::read::ZipArchive;
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

/// Default ceiling for sum of declared uncompressed sizes in one NW container.
pub const MAX_NW_UNCOMPRESSED: u64 = 1024 * 1024 * 1024; // 1 GiB

/// How far back from EOF we search for the EOCD signature (comment ≤ 64 KiB + record).
const EOCD_SEARCH_MAX: usize = 64 * 1024 + 22 + 256;

const EOCD_SIG: [u8; 4] = [0x50, 0x4b, 0x05, 0x06];
const LOCAL_SIG: [u8; 4] = [0x50, 0x4b, 0x03, 0x04];

#[derive(Debug)]
pub struct NwError {
    pub file: String,
    pub message: String,
}

impl std::fmt::Display for NwError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.file, self.message)
    }
}

impl std::error::Error for NwError {}

fn err(file: &str, message: impl Into<String>) -> NwError {
    NwError {
        file: file.into(),
        message: message.into(),
    }
}

/// One file entry listed in the NW ZIP.
#[derive(Clone, Debug)]
pub struct NwEntry {
    /// Archive-internal path with `/` separators.
    pub path: String,
    pub size: u64,
}

/// Opened NW.js package (`package.nw` or exe+zip).
#[derive(Clone, Debug)]
pub struct NwArchive {
    pub path: PathBuf,
    /// Full file bytes (prefix + zip for exe; pure zip for package.nw).
    pub data: Vec<u8>,
    /// Byte offset where the ZIP local/central records begin (0 for package.nw).
    pub zip_start: usize,
    pub entries: Vec<NwEntry>,
}

impl NwArchive {
    /// Slice of `data` that is the ZIP payload.
    pub fn zip_bytes(&self) -> &[u8] {
        &self.data[self.zip_start..]
    }

    /// MZ/PE (or other) stub before the ZIP; empty for pure `package.nw`.
    pub fn exe_prefix(&self) -> &[u8] {
        &self.data[..self.zip_start]
    }

    pub fn open(path: &Path) -> Result<Self, NwError> {
        let label = path.display().to_string();
        let data = std::fs::read(path).map_err(|e| err(&label, format!("read failed: {e}")))?;
        Self::from_bytes(path.to_path_buf(), data)
    }

    pub fn from_bytes(path: PathBuf, data: Vec<u8>) -> Result<Self, NwError> {
        let label = path.display().to_string();
        if data.len() < 22 {
            return Err(err(&label, "file too small to be a ZIP / NW package"));
        }
        let zip_start = locate_zip_start(&data, &label)?;
        if zip_start >= data.len() {
            return Err(err(&label, "ZIP start past EOF"));
        }
        let zip_slice = &data[zip_start..];
        check_declared_budget(zip_slice, &label)?;

        let cursor = Cursor::new(zip_slice.to_vec());
        let mut archive = ZipArchive::new(cursor)
            .map_err(|e| err(&label, format!("ZIP open failed: {e}")))?;

        let mut entries = Vec::with_capacity(archive.len());
        let mut total_declared = 0u64;
        for i in 0..archive.len() {
            let file = archive
                .by_index(i)
                .map_err(|e| err(&label, format!("ZIP entry {i}: {e}")))?;
            if file.is_dir() {
                continue;
            }
            let name = file.name().replace('\\', "/");
            // Skip absolute / traversal names — never surface them as inject targets.
            if name.is_empty()
                || name.starts_with('/')
                || name.split('/').any(|c| c == "..")
            {
                continue;
            }
            let size = file.size();
            total_declared = total_declared.saturating_add(size);
            if total_declared > MAX_NW_UNCOMPRESSED {
                return Err(err(
                    &label,
                    format!(
                        "declared uncompressed total exceeds {} bytes (zip-bomb guard)",
                        MAX_NW_UNCOMPRESSED
                    ),
                ));
            }
            entries.push(NwEntry { path: name, size });
        }

        Ok(Self {
            path,
            data,
            zip_start,
            entries,
        })
    }

    pub fn read_entry(&self, entry: &NwEntry) -> Result<Vec<u8>, NwError> {
        let label = self.path.display().to_string();
        let cursor = Cursor::new(self.zip_bytes().to_vec());
        let mut archive = ZipArchive::new(cursor)
            .map_err(|e| err(&label, format!("ZIP reopen failed: {e}")))?;
        // Two-step name resolution: NLL can't hold both by_name borrows at once.
        let name = if archive.index_for_name(&entry.path).is_some() {
            entry.path.clone()
        } else {
            entry.path.replace('/', "\\")
        };
        let mut file = archive
            .by_name(&name)
            .map_err(|e| err(&label, format!("missing entry {}: {e}", entry.path)))?;
        let mut buf = Vec::with_capacity(file.size() as usize);
        file.read_to_end(&mut buf)
            .map_err(|e| err(&label, format!("read {}: {e}", entry.path)))?;
        if (buf.len() as u64) > MAX_NW_UNCOMPRESSED {
            return Err(err(&label, "entry expanded past zip-bomb ceiling"));
        }
        Ok(buf)
    }

    pub fn scenario_ks_entries(&self) -> impl Iterator<Item = &NwEntry> {
        self.entries.iter().filter(|e| is_scenario_ks_path(&e.path))
    }
}

fn is_scenario_ks_path(path: &str) -> bool {
    let p = path.replace('\\', "/");
    let lower = p.to_ascii_lowercase();
    lower.contains("data/scenario/")
        && Path::new(&p)
            .extension()
            .and_then(|x| x.to_str())
            .map(|x| x.eq_ignore_ascii_case("ks"))
            .unwrap_or(false)
}

/// Locate ZIP start: try pure archive, else SFX math from EOCD, else first local header.
fn locate_zip_start(data: &[u8], label: &str) -> Result<usize, NwError> {
    // Fast path: file starts with local file header.
    if data.len() >= 4 && data[0..4] == LOCAL_SIG {
        // Confirm EOCD-based open works with start 0.
        if ZipArchive::new(Cursor::new(data)).is_ok() {
            return Ok(0);
        }
    }

    let eocd = find_eocd(data, label)?;
    // Physical CD sits immediately before EOCD for single-disk archives.
    let physical_cd = eocd
        .offset
        .checked_sub(eocd.cd_size)
        .ok_or_else(|| err(label, "EOCD central-directory size past start of record"))?;
    let zip_start = physical_cd
        .checked_sub(eocd.cd_offset)
        .ok_or_else(|| err(label, "computed ZIP start underflow (corrupt EOCD)"))?;

    if zip_start >= data.len() {
        return Err(err(label, "computed ZIP start past EOF"));
    }
    // Prefer a real local header at that offset when present.
    if zip_start + 4 <= data.len() && data[zip_start..zip_start + 4] == LOCAL_SIG {
        return Ok(zip_start);
    }
    // Fallback: scan forward a little for PK\x03\x04 (alignment / padding).
    let scan_end = (zip_start + 4096).min(data.len().saturating_sub(4));
    for i in zip_start..scan_end {
        if data[i..i + 4] == LOCAL_SIG {
            return Ok(i);
        }
    }
    // Last resort: trust EOCD math even without a local header signature.
    Ok(zip_start)
}

struct EocdInfo {
    /// Absolute file offset of the EOCD signature.
    offset: usize,
    cd_size: usize,
    /// Offset of central directory relative to the start of the ZIP.
    cd_offset: usize,
}

fn find_eocd(data: &[u8], label: &str) -> Result<EocdInfo, NwError> {
    if data.len() < 22 {
        return Err(err(label, "too small for EOCD"));
    }
    let search_from = data.len().saturating_sub(EOCD_SEARCH_MAX);
    // Scan backwards for EOCD signature.
    let mut i = data.len() - 22;
    loop {
        if i < search_from {
            break;
        }
        if data[i..i + 4] == EOCD_SIG {
            // comment length at +20
            let comment_len =
                u16::from_le_bytes([data[i + 20], data[i + 21]]) as usize;
            if i + 22 + comment_len == data.len() || i + 22 + comment_len <= data.len() {
                // Prefer EOCD that lands at/near EOF (standard).
                if i + 22 + comment_len == data.len() {
                    let cd_size =
                        u32::from_le_bytes(data[i + 12..i + 16].try_into().unwrap()) as usize;
                    let cd_offset =
                        u32::from_le_bytes(data[i + 16..i + 20].try_into().unwrap()) as usize;
                    return Ok(EocdInfo {
                        offset: i,
                        cd_size,
                        cd_offset,
                    });
                }
            }
        }
        if i == 0 {
            break;
        }
        i -= 1;
    }
    // Second pass: any EOCD in the window (comment not flush with EOF).
    let mut i = data.len() - 22;
    loop {
        if i < search_from {
            break;
        }
        if data[i..i + 4] == EOCD_SIG {
            let cd_size = u32::from_le_bytes(data[i + 12..i + 16].try_into().unwrap()) as usize;
            let cd_offset = u32::from_le_bytes(data[i + 16..i + 20].try_into().unwrap()) as usize;
            return Ok(EocdInfo {
                offset: i,
                cd_size,
                cd_offset,
            });
        }
        if i == 0 {
            break;
        }
        i -= 1;
    }
    Err(err(label, "no ZIP end-of-central-directory signature found"))
}

fn check_declared_budget(zip_slice: &[u8], label: &str) -> Result<(), NwError> {
    // Light pass: open archive and sum declared sizes without reading payloads fully.
    let cursor = Cursor::new(zip_slice);
    let mut archive = match ZipArchive::new(cursor) {
        Ok(a) => a,
        Err(e) => return Err(err(label, format!("ZIP open failed: {e}"))),
    };
    let mut total = 0u64;
    for i in 0..archive.len() {
        let f = archive
            .by_index(i)
            .map_err(|e| err(label, format!("ZIP entry {i}: {e}")))?;
        total = total.saturating_add(f.size());
        if total > MAX_NW_UNCOMPRESSED {
            return Err(err(
                label,
                format!(
                    "declared uncompressed total exceeds {} bytes (zip-bomb guard)",
                    MAX_NW_UNCOMPRESSED
                ),
            ));
        }
    }
    Ok(())
}

/// Rebuild ZIP with `replacements` (inner path → new UTF-8 payload).
/// Untouched entries are raw-copied when the zip crate allows it.
pub fn rebuild_nw_zip(
    original: &NwArchive,
    replacements: &HashMap<String, Vec<u8>>,
) -> Result<Vec<u8>, NwError> {
    let label = original.path.display().to_string();
    let zip_src = original.zip_bytes().to_vec();
    let mut src = ZipArchive::new(Cursor::new(zip_src))
        .map_err(|e| err(&label, format!("ZIP reopen for rebuild: {e}")))?;

    let mut out_cursor = Cursor::new(Vec::new());
    {
        let mut writer = ZipWriter::new(&mut out_cursor);
        for i in 0..src.len() {
            let file = src
                .by_index(i)
                .map_err(|e| err(&label, format!("ZIP entry {i}: {e}")))?;
            let name = file.name().replace('\\', "/");
            let is_dir = file.is_dir() || name.ends_with('/');

            if is_dir {
                writer
                    .add_directory(name.trim_end_matches('/'), SimpleFileOptions::default())
                    .map_err(|e| err(&label, format!("ZIP add_directory {name}: {e}")))?;
                continue;
            }

            let key = name.clone();
            let replacement = replacements
                .get(&key)
                .or_else(|| replacements.get(&key.replace('/', "\\")));
            if let Some(new_payload) = replacement {
                let method = file.compression();
                let opts = SimpleFileOptions::default().compression_method(match method {
                    CompressionMethod::Stored => CompressionMethod::Stored,
                    _ => CompressionMethod::Deflated,
                });
                // Drop the ZipFile before writing (we only needed metadata).
                drop(file);
                writer
                    .start_file(name, opts)
                    .map_err(|e| err(&label, format!("ZIP start_file {key}: {e}")))?;
                writer
                    .write_all(new_payload)
                    .map_err(|e| err(&label, format!("ZIP write {key}: {e}")))?;
            } else {
                // Byte-identical compressed payload (no recompress).
                writer
                    .raw_copy_file(file)
                    .map_err(|e| err(&label, format!("ZIP raw_copy {key}: {e}")))?;
            }
        }
        writer
            .finish()
            .map_err(|e| err(&label, format!("ZIP finish: {e}")))?;
    }

    let zip_out = out_cursor.into_inner();
    // Prefix (exe stub) + new zip.
    let mut result = Vec::with_capacity(original.zip_start + zip_out.len());
    result.extend_from_slice(original.exe_prefix());
    result.extend_from_slice(&zip_out);
    Ok(result)
}

// ─── Cheap disk probes (detect — never load full multi-GB candidates) ─────

/// True if `path` is named `package.nw` (case-insensitive).
pub fn is_package_nw_name(path: &Path) -> bool {
    path.file_name()
        .and_then(|n| n.to_str())
        .map(|n| n.eq_ignore_ascii_case("package.nw"))
        .unwrap_or(false)
}

/// True if path looks like a Windows PE candidate (`.exe`).
pub fn is_exe_name(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| e.eq_ignore_ascii_case("exe"))
        .unwrap_or(false)
}

/// EOCD-only probe: file has a ZIP trailer. Does not read the whole file.
pub fn probe_eocd_present(path: &Path) -> bool {
    let Ok(mut f) = File::open(path) else {
        return false;
    };
    let Ok(meta) = f.metadata() else {
        return false;
    };
    let len = meta.len() as usize;
    if len < 22 {
        return false;
    }
    let take = EOCD_SEARCH_MAX.min(len);
    if f.seek(SeekFrom::End(-(take as i64))).is_err() {
        return false;
    }
    let mut buf = vec![0u8; take];
    if f.read_exact(&mut buf).is_err() {
        return false;
    }
    find_eocd(&buf, "probe").is_ok()
}

/// Cheap: EOCD tail + central-directory name scan for `data/scenario` + `.ks`.
/// Reads only the tail and the CD region (not full payloads).
pub fn probe_scenario_in_zip_tail(path: &Path) -> bool {
    let Ok(mut f) = File::open(path) else {
        return false;
    };
    let Ok(meta) = f.metadata() else {
        return false;
    };
    let file_len = meta.len() as usize;
    if file_len < 22 {
        return false;
    }
    let take = EOCD_SEARCH_MAX.min(file_len);
    if f.seek(SeekFrom::End(-(take as i64))).is_err() {
        return false;
    }
    let mut tail = vec![0u8; take];
    if f.read_exact(&mut tail).is_err() {
        return false;
    }
    let Ok(eocd_rel) = find_eocd(&tail, "probe") else {
        return false;
    };
    // Map EOCD offset into absolute file coordinates.
    let tail_start = file_len - take;
    let eocd_abs = tail_start + eocd_rel.offset;
    let physical_cd = match eocd_abs.checked_sub(eocd_rel.cd_size) {
        Some(p) => p,
        None => return false,
    };
    let zip_start = match physical_cd.checked_sub(eocd_rel.cd_offset) {
        Some(s) => s,
        None => return false,
    };
    let cd_abs = zip_start.saturating_add(eocd_rel.cd_offset);
    if cd_abs >= file_len || eocd_rel.cd_size == 0 || eocd_rel.cd_size > MAX_NW_UNCOMPRESSED as usize
    {
        return false;
    }
    // Cap CD read to a few MiB (scenario name list is tiny).
    let cd_read = eocd_rel.cd_size.min(4 * 1024 * 1024);
    if f.seek(SeekFrom::Start(cd_abs as u64)).is_err() {
        return false;
    }
    let mut cd = vec![0u8; cd_read];
    if f.read_exact(&mut cd).is_err() {
        return false;
    }
    // Central directory file names are length-prefixed; a byte scan for the
    // ASCII path is enough for detect.
    let hay = String::from_utf8_lossy(&cd).to_ascii_lowercase();
    hay.contains("data/scenario/") && hay.contains(".ks")
}

/// List `package.nw` / scenario-bearing `.exe` under a game root (cheap probes).
pub fn find_nw_containers(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if root.is_file() {
        if (is_package_nw_name(root) && probe_eocd_present(root))
            || (is_exe_name(root) && probe_scenario_in_zip_tail(root))
        {
            out.push(root.to_path_buf());
        }
        return out;
    }
    if !root.is_dir() {
        return out;
    }
    // package.nw at root
    let pn = root.join("package.nw");
    if pn.is_file() && probe_eocd_present(&pn) {
        out.push(pn);
    }
    // Top-level *.exe only (not recursive — cheap detect).
    if let Ok(rd) = std::fs::read_dir(root) {
        for ent in rd.flatten() {
            let p = ent.path();
            if p.is_file() && is_exe_name(&p) && probe_scenario_in_zip_tail(&p) {
                out.push(p);
            }
        }
    }
    out.sort();
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn tempdir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("locust_tyrano_nw_{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn sample_ks() -> &'static str {
        "*start\n\
#akane\n\
Hello NW world.\n\
"
    }

    fn build_package_zip(scenario: &str) -> Vec<u8> {
        let mut buf = Cursor::new(Vec::new());
        {
            let mut z = ZipWriter::new(&mut buf);
            let opts = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
            z.start_file("package.json", opts).unwrap();
            z.write_all(br#"{"name":"test-tyrano","main":"index.html"}"#)
                .unwrap();
            z.start_file("data/scenario/first.ks", opts).unwrap();
            z.write_all(scenario.as_bytes()).unwrap();
            z.start_file("data/other/asset.bin", opts).unwrap();
            z.write_all(b"UNTOUCHED_ASSET_BYTES_xyz").unwrap();
            z.finish().unwrap();
        }
        buf.into_inner()
    }

    #[test]
    fn open_package_nw_lists_scenario() {
        let dir = tempdir();
        let zip = build_package_zip(sample_ks());
        let path = dir.join("package.nw");
        fs::write(&path, &zip).unwrap();
        let arch = NwArchive::open(&path).unwrap();
        assert_eq!(arch.zip_start, 0);
        assert!(arch.scenario_ks_entries().any(|e| e.path == "data/scenario/first.ks"));
        let body = arch
            .read_entry(
                arch.scenario_ks_entries()
                    .find(|e| e.path.ends_with("first.ks"))
                    .unwrap(),
            )
            .unwrap();
        assert!(String::from_utf8_lossy(&body).contains("Hello NW world"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn open_data_exe_preserves_prefix() {
        let dir = tempdir();
        let zip = build_package_zip(sample_ks());
        // Fake MZ header + padding + zip
        let mut exe = b"MZ\x90\x00FAKE_PE_STUB".to_vec();
        let prefix_len = exe.len();
        exe.extend_from_slice(&zip);
        let path = dir.join("data.exe");
        fs::write(&path, &exe).unwrap();

        let arch = NwArchive::open(&path).unwrap();
        assert_eq!(arch.zip_start, prefix_len);
        assert_eq!(arch.exe_prefix(), &exe[..prefix_len]);
        assert!(arch.scenario_ks_entries().count() >= 1);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn rebuild_keeps_untouched_entry_bytes() {
        let dir = tempdir();
        let zip = build_package_zip(sample_ks());
        let path = dir.join("package.nw");
        fs::write(&path, &zip).unwrap();
        let arch = NwArchive::open(&path).unwrap();

        let mut repl = HashMap::new();
        repl.insert(
            "data/scenario/first.ks".into(),
            b"*start\n#akane\nHola NW mundo.\n".to_vec(),
        );
        let rebuilt = rebuild_nw_zip(&arch, &repl).unwrap();
        fs::write(dir.join("out.nw"), &rebuilt).unwrap();
        let again = NwArchive::from_bytes(dir.join("out.nw"), rebuilt).unwrap();

        let asset = again
            .entries
            .iter()
            .find(|e| e.path == "data/other/asset.bin")
            .unwrap();
        assert_eq!(
            again.read_entry(asset).unwrap(),
            b"UNTOUCHED_ASSET_BYTES_xyz"
        );
        let ks = again
            .scenario_ks_entries()
            .find(|e| e.path.ends_with("first.ks"))
            .unwrap();
        assert!(String::from_utf8_lossy(&again.read_entry(ks).unwrap()).contains("Hola NW mundo"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn rebuild_data_exe_keeps_prefix() {
        let dir = tempdir();
        let zip = build_package_zip(sample_ks());
        let mut exe = b"MZ_PREFIX_12345".to_vec();
        let prefix = exe.clone();
        exe.extend_from_slice(&zip);
        let path = dir.join("game.exe");
        fs::write(&path, &exe).unwrap();
        let arch = NwArchive::open(&path).unwrap();
        let mut repl = HashMap::new();
        repl.insert(
            "data/scenario/first.ks".into(),
            b"*start\nTranslated.\n".to_vec(),
        );
        let out = rebuild_nw_zip(&arch, &repl).unwrap();
        assert!(out.starts_with(&prefix));
        let again = NwArchive::from_bytes(path.clone(), out).unwrap();
        assert_eq!(again.exe_prefix(), prefix.as_slice());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn malformed_zip_returns_err() {
        let dir = tempdir();
        let path = dir.join("package.nw");
        fs::write(&path, b"not a zip at all!!!!").unwrap();
        let e = NwArchive::open(&path).unwrap_err();
        assert!(e.to_string().contains("package.nw"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn probe_detects_package_and_exe() {
        let dir = tempdir();
        let zip = build_package_zip(sample_ks());
        let pn = dir.join("package.nw");
        fs::write(&pn, &zip).unwrap();
        assert!(probe_eocd_present(&pn));
        assert!(probe_scenario_in_zip_tail(&pn));

        let mut exe = b"MZ\x00\x00STUB".to_vec();
        exe.extend_from_slice(&zip);
        let ex = dir.join("data.exe");
        fs::write(&ex, &exe).unwrap();
        assert!(probe_scenario_in_zip_tail(&ex));

        let found = find_nw_containers(&dir);
        assert!(found.iter().any(|p| p.ends_with("package.nw")));
        assert!(found.iter().any(|p| p.ends_with("data.exe")));
        let _ = fs::remove_dir_all(&dir);
    }
}
