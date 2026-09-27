#[cfg(test)]
use std::cell::Cell;
use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

#[cfg(test)]
#[path = "unreal_overlay_tests.rs"]
mod overlay_ownership_tests;

use locust_core::error::{LocustError, Result};
use locust_core::extraction::{FormatPlugin, InjectionReport};
use locust_core::models::{OutputMode, StringEntry};
use locust_core::patch::{stream::StagingDir, zipsec::ensure_no_links, GameLock, PatchStore};

use crate::unreal_iostore;
use crate::unreal_iostore_native;
use crate::unreal_locres::{self, LocresFile, LocresKey, LOCRES_TUPLE_ID_PREFIX};
use crate::unreal_pak::{
    self, canonical_inner_name, is_locres_record_name, mounted_resource_path, payload_offset,
    read_footer_from_reader, read_index_from_reader, read_payload, record_containing_offset,
    writable_version, write_pak, PakWriteFile, DEFAULT_MOUNT_POINT,
};

// Per-test-thread counter for find_pak_files (avoids races under --test-threads>1).
// Some(n) means this thread is counting; None means ignore.
#[cfg(test)]
thread_local! {
    static FIND_PAK_FILES_CALLS: Cell<Option<usize>> = const { Cell::new(None) };
    static PAK_MAGIC_PROBES: Cell<Option<usize>> = const { Cell::new(None) };
}

/// Plugin for Unreal Engine games.
/// Scans .pak files and loose localization files for translatable strings.
///
/// Unreal stores localization in:
///   Content/Localization/{target}/{culture}/{target}.locres (binary — structural)
///   Content/Localization/{target}/{culture}/{target}.po (text PO files — if present)
///   .pak files: classic indexes, non-frozen v9, and v10/v11 full directory indexes;
///   uncompressed, Zlib and Gzip LocRes records (bounded, seek-based).
///   Small fixture paks without a classic index still use the legacy UTF-16LE
///   heuristic under a size cap. IoStore selections include companion PAKs and
///   indexed ExternalFile LocRes (TOC v5/v7/v8, None/Zlib/LZ4). Encryption, signatures, frozen indexes and Zen asset text are unsupported.
pub struct UnrealPlugin;

/// Legacy UTF-16LE / magic-scan path is only for small synthetic fixtures.
/// Multi-GB archives must never take this path (OOM / silent false completeness).
const HEURISTIC_PAK_MAX_BYTES: u64 = 8 * 1024 * 1024;

/// Conventional override rank for `Content/Paks` filenames.
/// Higher tuple wins and replaces the **entire** LocRes resource (not per-key).
///
/// This is **not** Unreal's full runtime mount order (`DefaultEngine.ini` PakOrder,
/// IoStore, signed pak exclusivity, chunk pairing). Supported deterministic
/// conventional semantics only:
///   0: base (no `_P` suffix)
///   1: patch `*_P.pak`, conventional `*_N_P.pak` (`_1_P`, `_2_P`, `_10_P`
///      compared numerically so 10 > 2), and invented `*_P{n}.pak`
///   2: Locust generated `*_LOCUST_P.pak` (always highest — QA policy, not a
///      claim about engine mount order)
/// Equal rank: case-insensitive lexicographic filename, later wins.
/// Unknown: custom mount order, encrypted/signed/IoStore, non-`_P` names.
fn pak_override_key(path: &Path) -> (u8, i32, String) {
    let name = path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let stem = name.strip_suffix(".pak").unwrap_or(name.as_str());
    let class_num = pak_patch_rank(stem);
    (class_num.0, class_num.1, name)
}

fn pak_patch_rank(stem: &str) -> (u8, i32) {
    if stem.ends_with("_locust_p") {
        return (2, 0);
    }
    // Conventional `*_N_P.pak` / bare `*_P.pak` (must strip trailing `_P` first
    // so `_10_P` is numeric 10, not a lexical tie at rank 0).
    if let Some(prefix) = stem.strip_suffix("_p") {
        if let Some((base, num)) = prefix.rsplit_once('_') {
            if !base.is_empty() && !num.is_empty() && num.chars().all(|c| c.is_ascii_digit()) {
                return (1, num.parse().unwrap_or(0));
            }
        }
        return (1, 0);
    }
    // Invented `*_P{n}.pak` (e.g. Game_P2.pak) — still ranked, not engine order.
    if let Some((prefix, num)) = stem.rsplit_once("_p") {
        if !prefix.is_empty() && !num.is_empty() && num.chars().all(|c| c.is_ascii_digit()) {
            return (1, num.parse().unwrap_or(0));
        }
    }
    (0, 0)
}

fn locres_resource_from_virtual(virtual_path: &str) -> String {
    let mut segs: Vec<&str> = virtual_path
        .split('/')
        .filter(|s| !s.is_empty() && *s != ".")
        .collect();
    while segs.first() == Some(&"..") {
        segs.remove(0);
    }
    segs.join("/")
}

enum PackedLocres {
    Entries(Vec<StringEntry>),
    /// Higher-priority pak claimed this virtual path but Locust cannot read it.
    Unsupported(String),
}

type PakExtraction = (Vec<(String, PackedLocres)>, Vec<StringEntry>);

fn locres_culture_from_path(path: &str) -> Option<String> {
    let parts: Vec<&str> = path.split('/').collect();
    for i in 0..parts.len().saturating_sub(2) {
        if parts[i].eq_ignore_ascii_case("localization") {
            return Some(parts[i + 2].to_string());
        }
    }
    None
}

impl UnrealPlugin {
    pub fn new() -> Self {
        Self
    }

    fn find_pak_files(path: &Path, stop_after_first: bool) -> Vec<PathBuf> {
        #[cfg(test)]
        FIND_PAK_FILES_CALLS.with(|c| {
            if let Some(n) = c.get() {
                c.set(Some(n + 1));
            }
        });

        let mut paks = Vec::new();
        if path.is_file() && path.extension().is_some_and(|e| e == "pak") {
            if Self::looks_like_unreal_pak(path) {
                paks.push(path.to_path_buf());
            }
            return paks;
        }
        if path.is_dir() {
            for entry in walkdir::WalkDir::new(path)
                .max_depth(5)
                .follow_links(false)
                .into_iter()
                .filter_entry(crate::discovery::is_game_entry)
                .filter_map(|e| e.ok())
            {
                let p = entry.path();
                if p.extension().is_some_and(|e| e == "pak") && Self::looks_like_unreal_pak(p) {
                    paks.push(p.to_path_buf());
                    if stop_after_first {
                        break;
                    }
                }
            }
        }
        paks
    }

    /// True when a `.pak` has Unreal footer magic near EOF.
    /// Filters Chromium/NW.js packs (`resources.pak`, `nw_*.pak`, `locales/*.pak`)
    /// that otherwise made NW.js RPG Maker deploys misdetect as Unreal.
    fn looks_like_unreal_pak(path: &Path) -> bool {
        #[cfg(test)]
        PAK_MAGIC_PROBES.with(|c| {
            if let Some(n) = c.get() {
                c.set(Some(n + 1));
            }
        });
        if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
            let lower = name.to_ascii_lowercase();
            if lower == "resources.pak"
                || lower.starts_with("nw_")
                || lower.starts_with("chrome_")
                || lower == "icudtl.dat"
            {
                return false;
            }
            // Chromium locale packs live under locales/
            if path
                .parent()
                .and_then(|p| p.file_name())
                .and_then(|n| n.to_str())
                .is_some_and(|p| p.eq_ignore_ascii_case("locales"))
            {
                return false;
            }
        }
        let Ok(mut file) = std::fs::File::open(path) else {
            return false;
        };
        let Ok(meta) = file.metadata() else {
            return false;
        };
        has_pak_magic_in_tail(&mut file, meta.len())
    }

    fn has_unreal_structure(path: &Path) -> bool {
        if !path.is_dir() {
            return false;
        }
        // Typical Unreal layout — do not walk .pak files when this already matches.
        // Open runs detect then extract; extract still needs find_pak_files once.
        if path.join("Engine").is_dir() {
            return true;
        }
        let has_content_game = path
            .read_dir()
            .ok()
            .and_then(|mut d| {
                d.find(|e| {
                    e.as_ref().ok().is_some_and(|e| {
                        e.path().is_dir()
                            && !crate::discovery::is_internal_directory_name(&e.file_name())
                            && e.path().join("Content").is_dir()
                    })
                })
            })
            .is_some();
        if has_content_game {
            return true;
        }
        // Pak-only trees (no Engine / Content) still need a filtered .pak walk.
        !Self::find_pak_files(path, true).is_empty()
    }

    /// Loose `*.locres` under the game tree (Localization or anywhere, depth-capped).
    fn find_loose_locres(path: &Path) -> Vec<PathBuf> {
        let mut out = Vec::new();
        if path.is_file() {
            if is_locres_path(path) {
                out.push(path.to_path_buf());
            }
            return out;
        }
        if !path.is_dir() {
            return out;
        }
        for entry in walkdir::WalkDir::new(path)
            .max_depth(8)
            .follow_links(false)
            .into_iter()
            .filter_entry(crate::discovery::is_game_entry)
            .filter_map(|e| e.ok())
        {
            let p = entry.path();
            if p.is_file() && is_locres_path(p) {
                out.push(p.to_path_buf());
            }
        }
        out.sort();
        out
    }

    fn extract_from_locres_file(path: &Path) -> Result<Vec<StringEntry>> {
        let label = path.display().to_string();
        let file = LocresFile::parse_path(path).map_err(|e| LocustError::ParseError {
            file: label.clone(),
            message: e.message,
        })?;
        locres_to_entries(&file, path)
    }

    /// Extract UTF-16LE strings from PAK file using heuristic scanning, plus any
    /// embedded LocRes blobs (structural). Heuristic hits that equal a locres
    /// string value are dropped to avoid double-extraction.
    fn extract_strings_from_pak(
        bytes: &[u8],
        filename: &str,
        file_path: &Path,
    ) -> Result<Vec<StringEntry>> {
        let mut entries = Vec::new();
        let mut locres_values: std::collections::HashSet<String> = std::collections::HashSet::new();

        // Structural LocRes blobs embedded in the pak payload.
        for off in unreal_locres::find_locres_offsets(bytes) {
            let label = format!("{filename}+locres@{off}");
            match LocresFile::parse(&bytes[off..], &label) {
                Ok(file) => {
                    for (_ns, _key, value, _) in file.iter_entries() {
                        locres_values.insert(value.to_string());
                    }
                    // file_path stays the pak; inject builds a sibling *_LOCUST_P.pak.
                    let mut loc_entries = locres_to_entries(&file, file_path)?;
                    for e in &mut loc_entries {
                        // A pak carries one locres blob PER CULTURE with identical
                        // namespace/key sets — the blob offset keeps ids unique so
                        // cultures don't silently overwrite each other in the DB.
                        e.id = format!("locres@{off}/{}", e.id);
                        e.metadata
                            .insert("locres_embedded".to_string(), serde_json::Value::Bool(true));
                        e.metadata
                            .insert("locres_offset".to_string(), serde_json::json!(off));
                    }
                    entries.extend(loc_entries);
                }
                Err(_) => {
                    // Malformed blob at magic false-positive — ignore.
                }
            }
        }

        let mut seen = std::collections::HashSet::new();
        let regions = find_utf16le_strings(bytes);

        for (idx, (offset, text)) in regions.into_iter().enumerate() {
            if text.chars().count() < 5 {
                continue;
            }
            // Skip values already taken from structural locres.
            if locres_values.contains(&text) {
                continue;
            }
            if !seen.insert(text.clone()) {
                continue;
            }
            // Filter out paths, code-like strings, and binary artifacts
            if text.contains('/') && text.contains('.') {
                continue; // Likely asset path
            }
            if text.chars().all(|c| c.is_ascii_uppercase() || c == '_') {
                continue; // Likely enum/constant
            }
            if !has_natural_language(&text) {
                continue;
            }

            let id = format!("{}#offset_{}#{}", filename, offset, idx);
            let mut entry = StringEntry::new(id, &text, file_path.to_path_buf());
            entry.tags = vec!["unknown".to_string()];
            entry.metadata.insert(
                "extraction_method".to_string(),
                serde_json::Value::String("heuristic_utf16".to_string()),
            );
            // Inject replaces UTF-16LE in-place; validate before inject.
            entry.metadata.insert(
                "binary_slot".to_string(),
                serde_json::Value::String("utf16le".to_string()),
            );
            entries.push(entry);
        }

        Ok(entries)
    }

    /// Seek-based classic/modern LocRes extraction, grouped by virtual resource.
    /// Large archives never take the heuristic fallback.
    fn extract_from_pak_path(pak: &Path) -> Result<PakExtraction> {
        let label = pak.display().to_string();
        let mut file = std::fs::File::open(pak)?;
        let file_len = file.metadata()?.len();
        match read_index_from_reader(&mut file, file_len, &label) {
            Ok(index) => {
                let mut resources: Vec<(String, PackedLocres)> = Vec::new();
                for rec in &index.records {
                    if !is_locres_record_name(&rec.name) {
                        continue;
                    }
                    let virtual_path = mounted_resource_path(&index.mount_point, &rec.name)
                        .map_err(|e| LocustError::ParseError {
                            file: label.clone(),
                            message: e.message,
                        })?;
                    if rec.encrypted {
                        return Err(LocustError::ParseError {
                            file: label,
                            message: format!("encrypted pak record is not supported: {}", rec.name),
                        });
                    }
                    let payload = match read_payload(
                        &mut file,
                        rec,
                        index.footer.version,
                        file_len,
                        &label,
                    ) {
                        Ok(payload) => payload,
                        Err(e) => {
                            // Preserve unreadable winning resources; never resurrect a lower PAK.
                            resources
                                .push((virtual_path, PackedLocres::Unsupported(e.to_string())));
                            continue;
                        }
                    };
                    let loc_label = format!("{}+{}", filename_of(pak), rec.name);
                    let loc = LocresFile::parse(&payload, &loc_label).map_err(|e| {
                        LocustError::ParseError {
                            file: loc_label,
                            message: e.message,
                        }
                    })?;
                    let off = payload_offset(rec, index.footer.version);
                    let inner = locres_resource_from_virtual(&virtual_path);
                    let entries = match locres_to_embedded_entries(
                        &loc,
                        pak,
                        &index.mount_point,
                        &inner,
                        &virtual_path,
                        off,
                    ) {
                        Ok(entries) => entries,
                        Err(error) => {
                            resources
                                .push((virtual_path, PackedLocres::Unsupported(error.to_string())));
                            continue;
                        }
                    };
                    resources.push((virtual_path, PackedLocres::Entries(entries)));
                }
                Ok((resources, Vec::new()))
            }
            Err(e) => {
                if !index_error_allows_heuristic(&e, file_len) {
                    return Err(LocustError::ParseError {
                        file: label,
                        message: if file_len > HEURISTIC_PAK_MAX_BYTES
                            && !index_error_is_unsupported(&e)
                        {
                            format!(
                                "{}; refusing to load {file_len}-byte pak for heuristic scan \
                                 (classic v3–v8 index required; encrypted/signed/IoStore unsupported)",
                                e.message
                            )
                        } else {
                            e.message
                        },
                    });
                }
                let bytes = std::fs::read(pak)?;
                let filename = filename_of(pak);
                Ok((
                    Vec::new(),
                    Self::extract_strings_from_pak(&bytes, &filename, pak)?,
                ))
            }
        }
    }

    fn extract_from_iostore_index(
        index: &unreal_iostore_native::IoStoreIndex,
        total_native_bytes: &mut usize,
    ) -> Result<Vec<(String, PackedLocres)>> {
        let mut resources = Vec::new();
        for record in &index.records {
            let state = match index.read_locres(record) {
                Ok(payload) => {
                    *total_native_bytes += payload.len();
                    if *total_native_bytes > 512 * 1024 * 1024 {
                        return Err(LocustError::ParseError {
                            file: index.toc_path.display().to_string(),
                            message:
                                "native IoStore localization exceeds 512 MiB extraction budget"
                                    .into(),
                        });
                    }
                    match LocresFile::parse(&payload, &record.name) {
                        Ok(loc) => {
                            let mut entries = match locres_to_embedded_entries(
                                &loc,
                                &index.toc_path,
                                &index.mount_point,
                                &record.name,
                                &record.virtual_path,
                                0,
                            ) {
                                Ok(entries) => entries,
                                Err(error) => {
                                    resources.push((
                                        record.virtual_path.clone(),
                                        PackedLocres::Unsupported(error.to_string()),
                                    ));
                                    continue;
                                }
                            };
                            for entry in &mut entries {
                                entry.metadata.remove("locres_offset");
                                entry.metadata.insert(
                                    "iostore_external_file".into(),
                                    serde_json::json!(true),
                                );
                                entry.metadata.insert(
                                    "iostore_chunk_index".into(),
                                    serde_json::json!(record.chunk_index),
                                );
                                entry.metadata.insert(
                                    "iostore_toc_version".into(),
                                    serde_json::json!(index.version),
                                );
                            }
                            PackedLocres::Entries(entries)
                        }
                        Err(e) => PackedLocres::Unsupported(format!(
                            "invalid IoStore LocRes {}: {e}",
                            record.name
                        )),
                    }
                }
                Err(e) => PackedLocres::Unsupported(format!("IoStore {}: {e}", record.name)),
            };
            resources.push((record.virtual_path.clone(), state));
        }
        Ok(resources)
    }
}

fn filename_of(path: &Path) -> String {
    path.file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string()
}

fn index_error_is_unsupported(err: &unreal_pak::PakError) -> bool {
    let m = err.message.as_str();
    m.contains("encrypted")
        || m.contains("unsupported")
        || m.contains("frozen/path-hash")
        || m.contains("classic index parse unsupported")
}

fn index_error_allows_heuristic(err: &unreal_pak::PakError, file_len: u64) -> bool {
    if file_len > HEURISTIC_PAK_MAX_BYTES || index_error_is_unsupported(err) {
        return false;
    }
    let m = err.message.as_str();
    // Fixtures have magic but no real footer (version 0) or a truncated tail.
    m.contains("implausible pak version") || m.contains("pak magic") || m.contains("file too small")
}

fn locres_to_embedded_entries(
    file: &LocresFile,
    pak_path: &Path,
    mount: &str,
    resource: &str,
    virtual_path: &str,
    locres_offset: u64,
) -> Result<Vec<StringEntry>> {
    let culture = locres_culture_from_path(virtual_path);
    let mut entries = locres_to_entries(file, pak_path)?;
    for e in &mut entries {
        e.id = format!("{virtual_path}#{}", e.id);
        e.metadata
            .insert("locres_embedded".to_string(), serde_json::Value::Bool(true));
        e.metadata.insert(
            "locres_offset".to_string(),
            serde_json::json!(locres_offset),
        );
        e.metadata.insert(
            "locres_mount".to_string(),
            serde_json::Value::String(mount.to_string()),
        );
        e.metadata.insert(
            "locres_resource".to_string(),
            serde_json::Value::String(resource.to_string()),
        );
        e.metadata.insert(
            "locres_virtual_path".to_string(),
            serde_json::Value::String(virtual_path.to_string()),
        );
        if let Some(culture) = &culture {
            e.metadata.insert(
                "locres_culture".to_string(),
                serde_json::Value::String(culture.clone()),
            );
        }
    }
    Ok(entries)
}

fn is_locres_path(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| e.eq_ignore_ascii_case("locres"))
        .unwrap_or(false)
}

fn is_locres_entry(entry: &StringEntry) -> bool {
    entry
        .metadata
        .get("extraction_method")
        .and_then(|v| v.as_str())
        == Some("locres")
        || is_locres_path(&entry.file_path)
}

fn find_record_for_locres_offset(
    index: &unreal_pak::PakIndex,
    off: u64,
) -> Option<&unreal_pak::PakRecord> {
    if let Some(r) = record_containing_offset(index, off) {
        return Some(r);
    }
    let ver = index.footer.version;
    index.records.iter().find(|r| {
        let po = payload_offset(r, ver);
        off >= po && off < po.saturating_add(r.size.max(1))
    })
}

fn entry_locres_offset(entry: &StringEntry) -> Option<u64> {
    entry
        .metadata
        .get("locres_offset")
        .and_then(|v| v.as_u64())
        .or_else(|| {
            entry
                .metadata
                .get("locres_offset")
                .and_then(|v| v.as_i64())
                .map(|i| i as u64)
        })
}

fn locres_group_key(entry: &StringEntry) -> String {
    if let Some(vp) = entry
        .metadata
        .get("locres_virtual_path")
        .and_then(|v| v.as_str())
    {
        return format!("virt:{vp}");
    }
    if let Some(res) = entry
        .metadata
        .get("locres_resource")
        .and_then(|v| v.as_str())
    {
        let mount = entry
            .metadata
            .get("locres_mount")
            .and_then(|v| v.as_str())
            .unwrap_or(DEFAULT_MOUNT_POINT);
        return format!(
            "virt:{}",
            mounted_resource_path(mount, res).unwrap_or_else(|_| format!("{mount}/{res}"))
        );
    }
    match entry_locres_offset(entry) {
        Some(off) => format!("off:{off}"),
        None => format!("id:{}", entry.id),
    }
}

/// Group embedded locres entries by virtual resource (legacy: offset), apply
/// translations, write one `<base>_LOCUST_P.pak` beside the source pak.
///
/// Returns `(strings_written, strings_skipped, optional_patch_path)`.
fn inject_embedded_locres_patch_pak(
    pak_path: &Path,
    game_root: &Path,
    entries: &[&StringEntry],
    warnings: &mut Vec<String>,
    skip_reasons: &mut std::collections::BTreeMap<String, usize>,
) -> Result<(usize, usize, Option<PathBuf>)> {
    if pak_path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("utoc"))
    {
        return inject_iostore_locres_patch(pak_path, game_root, entries, warnings, skip_reasons);
    }
    let label = pak_path.display().to_string();
    let mut file = std::fs::File::open(pak_path)?;
    let file_len = file.metadata()?.len();
    let footer = read_footer_from_reader(&mut file, file_len, &label).map_err(|e| {
        LocustError::ParseError {
            file: label.clone(),
            message: e.message,
        }
    })?;
    let write_ver = writable_version(footer.version).map_err(|e| LocustError::ParseError {
        file: label.clone(),
        message: e.message,
    })?;

    let index = match read_index_from_reader(&mut file, file_len, &label) {
        Ok(idx) => Some(idx),
        Err(e) => {
            if !index_error_allows_heuristic(&e, file_len) {
                return Err(LocustError::ParseError {
                    file: label,
                    message: format!(
                        "cannot inject LocRes into pak without a supported pak index: {}",
                        e.message
                    ),
                });
            }
            None
        }
    };
    let mount = index
        .as_ref()
        .map(|i| i.mount_point.clone())
        .unwrap_or_else(|| DEFAULT_MOUNT_POINT.to_string());

    let mut by_group: HashMap<String, Vec<&StringEntry>> = HashMap::new();
    let mut skipped = 0usize;
    for e in entries {
        let key = locres_group_key(e);
        if key.starts_with("id:") && entry_locres_offset(e).is_none() {
            warnings.push(format!("entry '{}' missing locres_offset", e.id));
            skipped += 1;
            *skip_reasons.entry("invalid_target".into()).or_default() += 1;
            continue;
        }
        by_group.entry(key).or_default().push(*e);
    }

    let heuristic_bytes = if index.is_none() {
        Some(std::fs::read(pak_path)?)
    } else {
        None
    };

    let mut pak_files: Vec<PakWriteFile> = Vec::new();
    let mut written = 0usize;

    let mut groups: Vec<(String, Vec<&StringEntry>)> = by_group.into_iter().collect();
    groups.sort_by(|a, b| a.0.cmp(&b.0));

    for (group_key, group) in groups {
        let loc_bytes = if let Some(ref idx) = index {
            let rec = group
                .iter()
                .find_map(|e| {
                    e.metadata
                        .get("locres_resource")
                        .and_then(|v| v.as_str())
                        .and_then(|name| idx.records.iter().find(|r| r.name == name))
                })
                .or_else(|| {
                    group
                        .iter()
                        .find_map(|e| entry_locres_offset(e))
                        .and_then(|off| find_record_for_locres_offset(idx, off))
                });
            let Some(rec) = rec else {
                warnings.push(format!("no pak record for locres group {group_key}"));
                skipped += group.len();
                *skip_reasons.entry("missing_target".into()).or_default() += group.len();
                continue;
            };
            match read_payload(&mut file, rec, idx.footer.version, file_len, &label) {
                Ok(b) => b,
                Err(e) => {
                    warnings.push(format!("read locres {}: {e}", rec.name));
                    skipped += group.len();
                    *skip_reasons.entry("error".into()).or_default() += group.len();
                    continue;
                }
            }
        } else {
            let Some(off) = group.iter().find_map(|e| entry_locres_offset(e)) else {
                warnings.push(format!("entry group {group_key} missing locres_offset"));
                skipped += group.len();
                *skip_reasons.entry("invalid_target".into()).or_default() += group.len();
                continue;
            };
            let bytes = heuristic_bytes.as_ref().unwrap();
            if off as usize >= bytes.len() {
                warnings.push(format!("locres_offset {off} past EOF of {label}"));
                skipped += group.len();
                *skip_reasons.entry("missing_target".into()).or_default() += group.len();
                continue;
            }
            bytes[off as usize..].to_vec()
        };

        let off = group.iter().find_map(|e| entry_locres_offset(e));
        let mut loc = match LocresFile::parse(&loc_bytes, &format!("{label}+{group_key}")) {
            Ok(l) => l,
            Err(e) => {
                warnings.push(format!("parse locres {group_key}: {e}"));
                skipped += group.len();
                *skip_reasons.entry("error".into()).or_default() += group.len();
                continue;
            }
        };

        let (map, reasons) = checked_locres_translations(&loc, &group, off);
        let pending = map.len();
        for (reason, count) in reasons {
            skipped += count;
            *skip_reasons.entry(reason).or_default() += count;
        }
        if map.is_empty() {
            continue;
        }
        let n = loc.apply_translations_by_key(&map);
        if n == 0 {
            skipped += pending;
            *skip_reasons.entry("missing_target".into()).or_default() += pending;
            warnings.push(format!(
                "locres {group_key}: no keys matched for {pending} translation(s)"
            ));
            continue;
        }
        if pending > n {
            skipped += pending - n;
            *skip_reasons.entry("missing_target".into()).or_default() += pending - n;
        }

        let payload = loc.serialize().map_err(|e| LocustError::ParseError {
            file: label.clone(),
            message: format!("serialize locres {group_key}: {e}"),
        })?;

        let inner_name = group
            .iter()
            .find_map(|e| {
                e.metadata
                    .get("locres_resource")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
            })
            .or_else(|| {
                index.as_ref().and_then(|idx| {
                    off.and_then(|o| find_record_for_locres_offset(idx, o))
                        .map(|r| r.name.clone())
                })
            })
            .unwrap_or_else(|| {
                format!(
                    "TestGame/Content/Localization/locres_{}.locres",
                    off.unwrap_or(0)
                )
            });
        let inner_name = match canonical_inner_name(&inner_name) {
            Ok(name) => name,
            Err(e) => {
                warnings.push(format!("refusing unsafe locres record path: {e}"));
                skipped += n;
                *skip_reasons.entry("invalid_target".into()).or_default() += n;
                continue;
            }
        };
        written += n;

        pak_files.push(PakWriteFile {
            name: inner_name,
            data: payload,
        });
    }

    if pak_files.is_empty() {
        return Ok((written, skipped, None));
    }

    pak_files.sort_by(|a, b| a.name.cmp(&b.name));

    let patch_bytes =
        write_pak(&mount, write_ver, &pak_files, &label).map_err(|e| LocustError::ParseError {
            file: label.clone(),
            message: e.message,
        })?;

    let candidate = unreal_pak::patch_pak_path(pak_path);
    let out_path = if candidate == pak_path {
        let stem = pak_path.file_stem().unwrap_or_default().to_string_lossy();
        pak_path.with_file_name(format!("{stem}_NEXT_LOCUST_P.pak"))
    } else {
        candidate
    };
    write_overlay(game_root, &out_path, &patch_bytes, warnings)?;
    warnings.push(format!(
        "wrote localization patch pak {} (version {write_ver}, {} file(s))",
        out_path.display(),
        pak_files.len()
    ));
    Ok((written, skipped, Some(out_path)))
}

/// Validate the canonical destination against the selected game, including
/// reparse points at the original leaf and at the canonical component chain.
fn guard_unreal_target(game_root: &Path, path: &Path) -> Result<()> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let name = path
        .file_name()
        .ok_or_else(|| LocustError::PatchError("Unreal target has no filename".into()))?;
    ensure_no_links(parent, Path::new(name))?;
    let canonical_target = parent.canonicalize()?.join(name);
    let relative = canonical_target.strip_prefix(game_root).map_err(|_| {
        LocustError::PatchError(format!(
            "Unreal target is outside selected game: {}",
            path.display()
        ))
    })?;
    ensure_no_links(game_root, relative)
}

fn write_overlay(
    game_root: &Path,
    output: &Path,
    bytes: &[u8],
    warnings: &mut Vec<String>,
) -> Result<()> {
    write_overlay_with(game_root, output, warnings, |file| {
        file.write_all(bytes)?;
        Ok(())
    })
}

/// Caller holds GameLock from source validation through installation. The
/// previous overlay is retained in an exclusively created, extensionless file
/// so recursive PAK discovery never mounts a backup. Crash leftovers are never
/// reclaimed by filename: only this live staging guard owns its children.
fn write_overlay_with(
    game_root: &Path,
    output: &Path,
    warnings: &mut Vec<String>,
    write: impl FnOnce(&mut std::fs::File) -> Result<()>,
) -> Result<()> {
    guard_unreal_target(game_root, output)?;
    let parent = output
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let mut stage = StagingDir::create_prepared(parent)?;
    let mut file = stage.create_file("overlay")?;
    write(&mut file)?;
    file.sync_all()?;
    drop(file);
    let previous = match std::fs::symlink_metadata(output) {
        Ok(meta) if meta.is_file() => {
            let mut source = std::fs::File::open(output)?;
            let mut backup = stage.create_file("previous")?;
            std::io::copy(&mut source, &mut backup)?;
            backup.sync_all()?;
            Some(stage.child("previous"))
        }
        Ok(_) => {
            return Err(LocustError::PatchError(
                "overlay destination is not a regular file".into(),
            ))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(error.into()),
    };
    // Recheck immediately before the shared move-aside/restore transaction.
    guard_unreal_target(game_root, output)?;
    PatchStore::replace_file(&stage.child("overlay"), output)?;
    if let Some(previous) = previous {
        stage.disarm();
        warnings.push(format!(
            "previous overlay retained at {}",
            previous.display()
        ));
    }
    Ok(())
}

/// Native resources retain their directory-mapped path and are exported into a
/// PAK overlay. Original UTOC/UCAS files are never opened for writing.
fn inject_iostore_locres_patch(
    toc_path: &Path,
    game_root: &Path,
    entries: &[&StringEntry],
    warnings: &mut Vec<String>,
    skip_reasons: &mut std::collections::BTreeMap<String, usize>,
) -> Result<(usize, usize, Option<PathBuf>)> {
    let label = toc_path.display().to_string();
    let index =
        unreal_iostore_native::read_index(toc_path).map_err(|e| LocustError::ParseError {
            file: label.clone(),
            message: e.message,
        })?;
    let mut groups = std::collections::BTreeMap::<String, Vec<&StringEntry>>::new();
    let mut skipped = 0usize;
    for entry in entries {
        let resource = entry
            .metadata
            .get("locres_resource")
            .and_then(|v| v.as_str());
        if entry.metadata.get("iostore_external_file") != Some(&serde_json::json!(true))
            || resource.is_none()
        {
            skipped += 1;
            *skip_reasons.entry("invalid_target".into()).or_default() += 1;
            continue;
        }
        groups
            .entry(resource.unwrap().into())
            .or_default()
            .push(*entry);
    }
    let mut files = std::collections::BTreeMap::<String, PakWriteFile>::new();
    let companion_path = toc_path.with_extension("pak");
    let candidate = unreal_pak::patch_pak_path(&companion_path);
    let out_path = if candidate == companion_path {
        // An input container may itself use the reserved LOCUST_P suffix.
        // Never overwrite that container's original companion PAK.
        let stem = toc_path.file_stem().unwrap_or_default().to_string_lossy();
        toc_path.with_file_name(format!("{stem}_NATIVE_LOCUST_P.pak"))
    } else {
        candidate
    };
    if std::fs::symlink_metadata(&out_path).is_ok_and(|m| m.file_type().is_symlink()) {
        return Err(LocustError::ParseError {
            file: out_path.display().to_string(),
            message: "refusing to replace a symlink at the IoStore overlay output path".into(),
        });
    }
    let mut overlay_bytes = 0usize;
    // Keep previously emitted resources when the user translates another subset.
    // Existing overlay data is structurally validated before any write.
    if out_path.exists() {
        let overlay_label = out_path.display().to_string();
        let mut file = std::fs::File::open(&out_path)?;
        let size = file.metadata()?.len();
        let overlay = read_index_from_reader(&mut file, size, &overlay_label).map_err(|e| {
            LocustError::ParseError {
                file: overlay_label.clone(),
                message: e.message,
            }
        })?;
        for record in &overlay.records {
            let virtual_path =
                mounted_resource_path(&overlay.mount_point, &record.name).map_err(|e| {
                    LocustError::ParseError {
                        file: overlay_label.clone(),
                        message: e.message,
                    }
                })?;
            let name = locres_resource_from_virtual(&virtual_path);
            if record.uncompressed_size > (512 * 1024 * 1024 - overlay_bytes) as u64 {
                return Err(LocustError::ParseError {
                    file: overlay_label.clone(),
                    message: "IoStore overlay exceeds 512 MiB output budget".into(),
                });
            }
            let data = unreal_pak::read_uncompressed_payload(
                &mut file,
                record,
                overlay.footer.version,
                size,
                &overlay_label,
            )
            .map_err(|e| LocustError::ParseError {
                file: overlay_label.clone(),
                message: e.message,
            })?;
            overlay_bytes += data.len();
            files.insert(virtual_path, PakWriteFile { name, data });
        }
    }
    let mut written = 0usize;
    for (resource, group) in groups {
        let Some(record) = index.find_record(&resource) else {
            skipped += group.len();
            *skip_reasons.entry("missing_target".into()).or_default() += group.len();
            continue;
        };
        if group.iter().any(|e| {
            e.metadata
                .get("locres_virtual_path")
                .and_then(|v| v.as_str())
                != Some(record.virtual_path.as_str())
                || e.metadata
                    .get("iostore_chunk_index")
                    .and_then(|v| v.as_u64())
                    != Some(record.chunk_index as u64)
        }) {
            skipped += group.len();
            *skip_reasons.entry("invalid_target".into()).or_default() += group.len();
            continue;
        }
        let payload = index
            .read_locres(record)
            .map_err(|e| LocustError::ParseError {
                file: label.clone(),
                message: e.message,
            })?;
        let mut loc =
            LocresFile::parse(&payload, &resource).map_err(|e| LocustError::ParseError {
                file: label.clone(),
                message: e.message,
            })?;
        let (map, reasons) = checked_locres_translations(&loc, &group, None);
        for (reason, count) in reasons {
            skipped += count;
            *skip_reasons.entry(reason).or_default() += count;
        }
        if map.is_empty() {
            continue;
        }
        // Preserve translations from previous batches within this same resource,
        // only while their namespace/key/source hash still matches native input.
        if let Some(previous) = files.get(&record.virtual_path) {
            let old = LocresFile::parse(&previous.data, &resource).map_err(|e| {
                LocustError::ParseError {
                    file: out_path.display().to_string(),
                    message: e.message,
                }
            })?;
            let mut current = HashMap::new();
            for (ns, key, _, hash) in loc.iter_entries() {
                current
                    .entry((ns, key))
                    .and_modify(|value| *value = None)
                    .or_insert(Some(hash));
            }
            let mut previous = HashMap::new();
            for (ns, key, value, hash) in old.iter_entries() {
                previous
                    .entry((ns, key))
                    .and_modify(|value| *value = None)
                    .or_insert(Some((value, hash)));
            }
            let prior: HashMap<_, _> = previous
                .into_iter()
                .filter_map(|((ns, key), value)| {
                    let (value, hash) = value?;
                    (current.get(&(ns, key)) == Some(&Some(hash)))
                        .then(|| ((ns.to_owned(), key.to_owned()), value.to_owned()))
                })
                .collect();
            loc.apply_translations_by_key(&prior);
        }
        let count = loc.apply_translations_by_key(&map);
        written += count;
        if count < map.len() {
            skipped += map.len() - count;
            *skip_reasons.entry("missing_target".into()).or_default() += map.len() - count;
        }
        if count == 0 {
            continue;
        }
        let data = loc.serialize().map_err(|e| LocustError::ParseError {
            file: label.clone(),
            message: e.message,
        })?;
        let previous_size = files.get(&record.virtual_path).map_or(0, |f| f.data.len());
        overlay_bytes = overlay_bytes - previous_size + data.len();
        if overlay_bytes > 512 * 1024 * 1024 {
            return Err(LocustError::ParseError {
                file: out_path.display().to_string(),
                message: "IoStore overlay exceeds 512 MiB output budget".into(),
            });
        }
        // Normalize to the default virtual root without inventing any path.
        let name = locres_resource_from_virtual(&record.virtual_path);
        canonical_inner_name(&name).map_err(|e| LocustError::ParseError {
            file: label.clone(),
            message: e.message,
        })?;
        files.insert(record.virtual_path.clone(), PakWriteFile { name, data });
    }
    if written == 0 {
        return Ok((0, skipped, None));
    }
    let files: Vec<_> = files.into_values().collect();
    let bytes =
        write_pak(DEFAULT_MOUNT_POINT, 8, &files, &label).map_err(|e| LocustError::ParseError {
            file: label,
            message: e.message,
        })?;
    write_overlay(game_root, &out_path, &bytes, warnings)?;
    warnings.push(format!("wrote native IoStore localization overlay {}; container bytes unchanged; runtime PAK mounting requires game verification", out_path.display()));
    Ok((written, skipped, Some(out_path)))
}

fn locres_to_entries(file: &LocresFile, file_path: &Path) -> Result<Vec<StringEntry>> {
    let mut entries = Vec::new();
    let ids = file
        .extraction_ids(&file_path.display().to_string())
        .map_err(|e| LocustError::ParseError {
            file: file_path.display().to_string(),
            message: e.message,
        })?;
    for ((ns, key, value, source_hash), id) in file.iter_entries().zip(ids) {
        if value.trim().is_empty() {
            continue;
        }
        let mut entry = StringEntry::new(id, value, file_path.to_path_buf());
        entry.tags = vec!["locres".to_string()];
        if !ns.is_empty() {
            entry.context = Some(format!("namespace={ns}"));
        }
        entry.metadata.insert(
            "extraction_method".to_string(),
            serde_json::Value::String("locres".to_string()),
        );
        entry.metadata.insert(
            "locres_namespace".to_string(),
            serde_json::Value::String(ns.to_string()),
        );
        entry.metadata.insert(
            "locres_key".to_string(),
            serde_json::Value::String(key.to_string()),
        );
        entry.metadata.insert(
            "locres_source_hash".to_string(),
            serde_json::json!(source_hash),
        );
        // Variable-length — no binary_slot length budget.
        entries.push(entry);
    }
    Ok(entries)
}

/// Build a locres write set only when both the current physical value and,
/// when present, Unreal's stored source hash match the extraction baseline.
/// Older hand-built fixtures without hash metadata retain safe value matching.
fn checked_locres_translations(
    file: &LocresFile,
    entries: &[&StringEntry],
    _embedded_offset: Option<u64>,
) -> (
    HashMap<LocresKey, String>,
    std::collections::BTreeMap<String, usize>,
) {
    let mut current = HashMap::new();
    let mut legacy = HashMap::new();
    let mut alias_bytes = 0usize;
    for (namespace, key, value, hash) in file.iter_entries() {
        current
            .entry((namespace, key))
            .and_modify(|v| *v = None)
            .or_insert(Some((value, hash)));
        alias_bytes = alias_bytes
            .saturating_add(namespace.len())
            .saturating_add(key.len())
            .saturating_add(1);
        if alias_bytes > unreal_locres::MAX_LOCRES_DECODED_BYTES {
            return (
                HashMap::new(),
                std::collections::BTreeMap::from([("invalid_target".into(), entries.len())]),
            );
        }
        let flat = if namespace.is_empty() {
            key.to_owned()
        } else {
            format!("{namespace}/{key}")
        };
        legacy
            .entry(flat)
            .and_modify(|v| *v = None)
            .or_insert(Some((namespace, key)));
    }
    let mut translations = HashMap::new();
    let mut reasons = std::collections::BTreeMap::new();
    let mut repeated_requests = std::collections::HashSet::new();
    for entry in entries {
        let Some(translation) = entry.translation.as_ref() else {
            *reasons.entry("untranslated".into()).or_default() += 1;
            continue;
        };
        if translation == &entry.source {
            *reasons.entry("unchanged".into()).or_default() += 1;
            continue;
        }
        let key = match resolve_locres_key(entry, &current, &legacy) {
            Ok(key) => key,
            Err(reason) => {
                *reasons.entry(reason.into()).or_default() += 1;
                continue;
            }
        };
        let Some(candidate) = current.get(&(key.0.as_str(), key.1.as_str())) else {
            *reasons.entry("missing_target".into()).or_default() += 1;
            continue;
        };
        let Some(&(value, hash)) = candidate.as_ref() else {
            *reasons.entry("ambiguous_target".into()).or_default() += 1;
            continue;
        };
        if value != entry.source.as_str() {
            *reasons.entry("source_changed".into()).or_default() += 1;
            continue;
        }
        if let Some(stored) = entry.metadata.get("locres_source_hash") {
            let Some(stored) = stored.as_u64().and_then(|n| u32::try_from(n).ok()) else {
                *reasons.entry("invalid_target".into()).or_default() += 1;
                continue;
            };
            if stored != hash {
                *reasons.entry("source_changed".into()).or_default() += 1;
                continue;
            }
        }
        if repeated_requests.contains(&key) {
            *reasons.entry("ambiguous_request".into()).or_default() += 1;
        } else if translations.remove(&key).is_some() {
            repeated_requests.insert(key);
            *reasons.entry("ambiguous_request".into()).or_default() += 2;
        } else {
            translations.insert(key, translation.clone());
        }
    }
    (translations, reasons)
}

type CurrentLocres<'a> = HashMap<(&'a str, &'a str), Option<(&'a str, u32)>>;
type LegacyLocres<'a> = HashMap<String, Option<(&'a str, &'a str)>>;

fn resolve_locres_key(
    entry: &StringEntry,
    current: &CurrentLocres<'_>,
    legacy: &LegacyLocres<'_>,
) -> std::result::Result<LocresKey, &'static str> {
    // Existing project metadata is physical identity even when its saved ID is
    // the old colliding flat string. Never parse that ID to pivot the target.
    if let Some(key) = entry.metadata.get("locres_key") {
        let key = key.as_str().ok_or("invalid_target")?;
        let namespace = match entry.metadata.get("locres_namespace") {
            Some(value) => value.as_str().ok_or("invalid_target")?,
            None => "", // compatibility with old default-namespace metadata
        };
        return Ok((namespace.to_owned(), key.to_owned()));
    }
    if entry.metadata.contains_key("locres_namespace") {
        return Err("invalid_target");
    }
    let mut labels = vec![entry.id.as_str()];
    if let Some(offset) = entry_locres_offset(entry) {
        if let Some(rest) = entry.id.strip_prefix(&format!("locres@{offset}/")) {
            labels.push(rest);
        }
    }
    // Historical embedded IDs use '#'. Consider every possible split so a '#'
    // inside an actual key cannot redirect to a different physical identity.
    labels.extend(
        entry
            .id
            .match_indices('#')
            .map(|(at, _)| &entry.id[at + 1..]),
    );
    let mut candidates = std::collections::HashSet::new();
    for label in labels {
        if let Some(candidate) = legacy.get(label) {
            let Some((ns, key)) = candidate else {
                return Err("ambiguous_target");
            };
            candidates.insert(((*ns).to_owned(), (*key).to_owned()));
        }
        if let Some(encoded) = label.strip_prefix(LOCRES_TUPLE_ID_PREFIX) {
            if let Ok((ns, key)) = serde_json::from_str::<LocresKey>(encoded) {
                if current.contains_key(&(ns.as_str(), key.as_str())) {
                    candidates.insert((ns, key));
                }
            }
        }
        if candidates.len() > 1 {
            return Err("ambiguous_target");
        }
    }
    candidates.into_iter().next().ok_or("missing_target")
}

/// Find UTF-16LE string regions in binary data.
/// Returns (byte_offset, decoded_string).
fn find_utf16le_strings(bytes: &[u8]) -> Vec<(usize, String)> {
    let mut results = Vec::new();
    let len = bytes.len();
    if len < 2 {
        return results;
    }

    let mut i = 0;
    while i + 1 < len {
        // Look for start of UTF-16LE text (printable ASCII range or common Unicode)
        let lo = bytes[i];
        let hi = bytes[i + 1];

        if hi == 0 && (0x20..=0x7E).contains(&lo) {
            // Potential UTF-16LE ASCII start
            let start = i;
            let mut chars = Vec::new();

            while i + 1 < len {
                let lo = bytes[i];
                let hi = bytes[i + 1];

                if hi == 0 && (0x20..=0x7E).contains(&lo) {
                    chars.push(lo as char);
                    i += 2;
                } else if hi == 0 && lo == 0 {
                    // Null terminator
                    break;
                } else if hi > 0 && hi < 0xD8 {
                    // Higher Unicode (CJK, etc.)
                    let codepoint = (hi as u16) << 8 | lo as u16;
                    if let Some(ch) = char::from_u32(codepoint as u32) {
                        if ch.is_alphanumeric() || ch.is_whitespace() || ".,!?;:'\"()-".contains(ch)
                        {
                            chars.push(ch);
                            i += 2;
                            continue;
                        }
                    }
                    break;
                } else {
                    break;
                }
            }

            if chars.len() >= 3 {
                let text: String = chars.into_iter().collect();
                results.push((start, text));
            }
        } else {
            i += 2;
        }
    }

    results
}

fn has_natural_language(text: &str) -> bool {
    let char_count = text.chars().count();
    if char_count < 4 {
        return false;
    }

    // Count ASCII letters vs total chars — real text should be mostly ASCII or CJK
    let ascii_letters = text.chars().filter(|c| c.is_ascii_alphabetic()).count();
    let ascii_printable = text
        .chars()
        .filter(|c| c.is_ascii_graphic() || c.is_ascii_whitespace())
        .count();

    // For ASCII-heavy text: require high ratio of ASCII printable chars
    let ascii_ratio = ascii_printable as f64 / char_count as f64;
    if ascii_ratio < 0.8 {
        return false; // Too much non-ASCII garbage
    }

    // Must have actual letters (not just punctuation/numbers)
    if ascii_letters < 3 {
        return false;
    }

    // Must contain a space (multi-word) or be a short single word
    let has_space = text.contains(' ');
    if !has_space && char_count > 25 {
        return false; // Long strings without spaces are likely identifiers
    }

    // Filter out camelCase/PascalCase identifiers
    let upper_lower_transitions = text
        .as_bytes()
        .windows(2)
        .filter(|w| w[0].is_ascii_uppercase() && w[1].is_ascii_lowercase())
        .count();
    if !has_space && upper_lower_transitions >= 3 {
        return false; // Likely camelCase identifier
    }

    // All chars should be printable ASCII or common Unicode
    text.chars()
        .all(|c| c.is_ascii_graphic() || c.is_ascii_whitespace() || c.is_alphabetic())
}

impl Default for UnrealPlugin {
    fn default() -> Self {
        Self::new()
    }
}

impl FormatPlugin for UnrealPlugin {
    fn id(&self) -> &str {
        "unreal"
    }

    fn name(&self) -> &str {
        "Unreal Engine"
    }

    fn description(&self) -> &str {
        "Unreal localization in classic/modern PAKs, loose LocRes and indexed IoStore ExternalFile LocRes (v5/v7/v8, None/Zlib/LZ4)."
    }

    fn stability(&self) -> locust_core::extraction::FormatStability {
        // Modern PAKs have real fixture coverage; game-specific mounting and
        // encrypted containers still require additional validation.
        locust_core::extraction::FormatStability::Experimental
    }

    fn supported_extensions(&self) -> &[&str] {
        &[".pak", ".locres", ".utoc", ".ucas"]
    }

    fn supported_modes(&self) -> Vec<OutputMode> {
        vec![OutputMode::Replace]
    }

    fn detect(&self, path: &Path) -> bool {
        if path.is_file() {
            if unreal_iostore::is_container_path(path) {
                return unreal_iostore::toc_for_container(path).is_some();
            }
            return path
                .extension()
                .is_some_and(|e| e == "pak" || e.eq_ignore_ascii_case("locres"));
        }
        Self::has_unreal_structure(path)
            || !Self::find_loose_locres(path).is_empty()
            || unreal_iostore::find_toc(path).is_some()
    }

    fn extract(&self, path: &Path) -> Result<Vec<StringEntry>> {
        // A selected IoStore container uses sibling localization PAKs as a set,
        // so whole-resource override priority still includes update archives.
        let selected_toc = if path.is_file() && unreal_iostore::is_container_path(path) {
            Some(unreal_iostore::toc_for_container(path).ok_or_else(|| {
                LocustError::ParseError {
                    file: path.display().to_string(),
                    message: "invalid or missing matching IoStore .utoc header; open the complete game folder or a companion .pak/.locres file".to_string(),
                }
            })?)
        } else {
            None
        };
        let path = if selected_toc.is_some() {
            path.parent()
                .filter(|p| !p.as_os_str().is_empty())
                .unwrap_or(Path::new("."))
        } else {
            path
        };
        let paks = Self::find_pak_files(path, false);
        let tocs = unreal_iostore_native::find_tocs(path);
        let mut locres_files = Self::find_loose_locres(path);
        // Single-file .locres open
        if path.is_file() && is_locres_path(path) {
            locres_files = vec![path.to_path_buf()];
        }

        if paks.is_empty() && locres_files.is_empty() && tocs.is_empty() {
            return Err(LocustError::ParseError {
                file: path.display().to_string(),
                message: selected_toc
                    .clone()
                    .or_else(|| unreal_iostore::find_toc(path))
                    .map(|toc| unreal_iostore::no_localization_message(&toc))
                    .unwrap_or_else(|| "no .pak or .locres files found".to_string()),
            });
        }

        let mut all = Vec::new();
        let mut loose_locres_values: std::collections::HashSet<String> =
            std::collections::HashSet::new();

        for lr in &locres_files {
            match Self::extract_from_locres_file(lr) {
                Ok(entries) => {
                    for e in &entries {
                        loose_locres_values.insert(e.source.clone());
                    }
                    all.extend(entries);
                }
                Err(e) => {
                    return Err(e);
                }
            }
        }

        // Shared conventional archive rank; a same-stem PAK wins a tie over its
        // TOC. This is Locust's deterministic resource policy, not engine mount QA.
        let mut archives: Vec<(PathBuf, bool)> = paks
            .into_iter()
            .map(|p| (p, false))
            .chain(tocs.into_iter().map(|p| (p, true)))
            .collect();
        archives.sort_by_key(|(p, toc)| (pak_override_key(&p.with_extension("pak")), !toc));

        let mut winning: HashMap<String, PackedLocres> = HashMap::new();
        let mut heuristic = Vec::new();
        let mut native_unavailable = Vec::new();
        let mut total_native_bytes = 0usize;

        for (archive, is_toc) in &archives {
            let (resources, heur) = if *is_toc {
                match unreal_iostore_native::read_index(archive) {
                    Ok(index) => (
                        Self::extract_from_iostore_index(&index, &mut total_native_bytes)?,
                        Vec::new(),
                    ),
                    Err(e) if e.unsupported => {
                        native_unavailable.push(format!("{}: {e}", archive.display()));
                        continue;
                    }
                    Err(e) => {
                        return Err(LocustError::ParseError {
                            file: archive.display().to_string(),
                            message: e.message,
                        })
                    }
                }
            } else {
                Self::extract_from_pak_path(archive)?
            };
            for (virtual_path, state) in resources {
                winning.insert(virtual_path, state);
            }
            heuristic.extend(heur);
        }

        let mut packed = Vec::new();
        let mut winning: Vec<(String, PackedLocres)> = winning.into_iter().collect();
        winning.sort_by(|a, b| a.0.cmp(&b.0));
        for (virtual_path, state) in winning {
            match state {
                PackedLocres::Entries(entries) => packed.extend(entries),
                PackedLocres::Unsupported(message) => {
                    return Err(LocustError::ParseError {
                        file: path.display().to_string(),
                        message: format!(
                            "active LocRes '{virtual_path}' is unreadable ({message}); \
                             refusing to expose a superseded lower-priority copy"
                        ),
                    });
                }
            }
        }
        packed.sort_by(|a, b| a.id.cmp(&b.id));
        all.extend(packed);

        heuristic.retain(|e| {
            if e.metadata.get("extraction_method").and_then(|v| v.as_str()) == Some("locres") {
                return true;
            }
            !loose_locres_values.contains(&e.source)
        });
        all.extend(heuristic);

        if !native_unavailable.is_empty() && !all.is_empty() {
            tracing::warn!(
                "IoStore native extraction unavailable for some containers: {}",
                native_unavailable.join("; ")
            );
            for entry in &mut all {
                entry.metadata.insert(
                    "iostore_unread_containers".into(),
                    serde_json::json!(native_unavailable),
                );
            }
        }

        if all.is_empty() {
            if let Some(toc) = selected_toc.or_else(|| unreal_iostore::find_toc(path)) {
                return Err(LocustError::ParseError {
                    file: path.display().to_string(),
                    message: format!(
                        "{} Native index diagnostics: {}",
                        unreal_iostore::no_localization_message(&toc),
                        native_unavailable.join("; ")
                    ),
                });
            }
        }
        Ok(all)
    }

    fn inject(&self, path: &Path, entries: &[StringEntry]) -> Result<InjectionReport> {
        // Locres: structural rewrite (variable length). Other entries: UTF-16LE
        // in-place slot patch (identity skip, oversize skip, multi-pattern scan).
        // Directory selections are the actual game root used by core patch
        // operations. File selections have only their containing directory as
        // context; callers with a game root must pass it instead of a Paks child.
        let root = if path.is_dir() {
            path
        } else {
            path.parent()
                .filter(|p| !p.as_os_str().is_empty())
                .unwrap_or(Path::new("."))
        };
        let game_lock = GameLock::acquire(root)?;
        self.inject_under_lock(path, entries, &game_lock)
    }

    fn inject_under_lock(
        &self,
        path: &Path,
        entries: &[StringEntry],
        game_lock: &GameLock,
    ) -> Result<InjectionReport> {
        game_lock.validate_selection(path)?;
        // Validate all existing targets before any group can change a file.
        for entry in entries {
            if entry.file_path.exists() {
                guard_unreal_target(game_lock.root(), &entry.file_path)?;
            }
        }
        let mut files_modified = 0;
        let mut strings_written = 0;
        let mut strings_skipped = 0;
        let mut length_skipped = 0usize;
        let mut pad_noted = 0usize;
        let mut warnings = Vec::new();
        let mut files_written: Vec<PathBuf> = Vec::new();
        let mut skip_reasons = std::collections::BTreeMap::new();

        let mut by_file: HashMap<PathBuf, Vec<&StringEntry>> = HashMap::new();
        for entry in entries {
            by_file
                .entry(entry.file_path.clone())
                .or_default()
                .push(entry);
        }

        for (file_path, file_entries) in &by_file {
            if !file_path.exists() {
                strings_skipped += file_entries.len();
                *skip_reasons.entry("missing_target".into()).or_default() += file_entries.len();
                continue;
            }

            // ── Structural .locres inject ──────────────────────────────────
            let all_locres = file_entries.iter().all(|e| is_locres_entry(e));
            let any_locres = file_entries.iter().any(|e| is_locres_entry(e));
            if any_locres && is_locres_path(file_path) {
                let label = file_path.display().to_string();
                let mut loc = match LocresFile::parse_path(file_path) {
                    Ok(l) => l,
                    Err(e) => {
                        warnings.push(format!("cannot parse locres {label}: {e}"));
                        strings_skipped += file_entries.len();
                        *skip_reasons.entry("error".into()).or_default() += file_entries.len();
                        continue;
                    }
                };
                let (map, reasons) = checked_locres_translations(&loc, file_entries, None);
                for (reason, count) in reasons {
                    strings_skipped += count;
                    *skip_reasons.entry(reason).or_default() += count;
                }
                let pending = map.len();
                if map.is_empty() {
                    continue;
                }
                let n = loc.apply_translations_by_key(&map);
                if n == 0 {
                    strings_skipped += pending;
                    *skip_reasons.entry("missing_target".into()).or_default() += pending;
                    warnings.push(format!(
                        "locres {label}: no keys matched for {pending} translation(s)"
                    ));
                    continue;
                }
                match loc.serialize() {
                    Ok(bytes) => {
                        std::fs::write(file_path, &bytes)?;
                        files_modified += 1;
                        files_written.push(file_path.clone());
                        strings_written += n;
                        if pending > n {
                            strings_skipped += pending - n;
                            *skip_reasons.entry("missing_target".into()).or_default() +=
                                pending - n;
                        }
                    }
                    Err(e) => {
                        warnings.push(format!("serialize locres {label}: {e}"));
                        strings_skipped += pending;
                        *skip_reasons.entry("error".into()).or_default() += pending;
                    }
                }
                continue;
            }

            if any_locres && !is_locres_path(file_path) {
                // Embedded locres → rebuild into sibling *_LOCUST_P.pak (UE mounts
                // patch paks over the base; no base rewrite).
                let embedded: Vec<&StringEntry> = file_entries
                    .iter()
                    .copied()
                    .filter(|e| {
                        is_locres_entry(e)
                            && e.metadata.get("locres_embedded")
                                == Some(&serde_json::Value::Bool(true))
                    })
                    .collect();
                if !embedded.is_empty() {
                    match inject_embedded_locres_patch_pak(
                        file_path,
                        game_lock.root(),
                        &embedded,
                        &mut warnings,
                        &mut skip_reasons,
                    ) {
                        Ok((written, skipped, patch_path)) => {
                            strings_written += written;
                            strings_skipped += skipped;
                            if let Some(p) = patch_path {
                                files_modified += 1;
                                files_written.push(p);
                            }
                        }
                        Err(e) => {
                            warnings.push(format!(
                                "embedded locres patch pak for {}: {e}",
                                file_path.display()
                            ));
                            strings_skipped += embedded.len();
                            *skip_reasons.entry("error".into()).or_default() += embedded.len();
                        }
                    }
                }
                // Fall through for any non-locres entries sharing the same file_path.
                if all_locres {
                    continue;
                }
            }

            // ── Heuristic UTF-16LE slot inject ─────────────────────────────
            let mut bytes = std::fs::read(file_path)?;
            let mut modified = false;

            struct Work<'a> {
                entry: &'a StringEntry,
                needle: Vec<u8>,
                trans: Vec<u8>,
            }
            let mut work: Vec<Work<'_>> = Vec::new();
            for entry in file_entries {
                if is_locres_entry(entry) {
                    continue;
                }
                let translation = match &entry.translation {
                    Some(t) => t,
                    None => {
                        strings_skipped += 1;
                        *skip_reasons.entry("untranslated".into()).or_default() += 1;
                        continue;
                    }
                };

                let orig_utf16: Vec<u8> = entry
                    .source
                    .encode_utf16()
                    .flat_map(|c| c.to_le_bytes())
                    .collect();
                let trans_utf16: Vec<u8> = translation
                    .encode_utf16()
                    .flat_map(|c| c.to_le_bytes())
                    .collect();

                if trans_utf16 == orig_utf16 {
                    strings_skipped += 1;
                    *skip_reasons.entry("unchanged".into()).or_default() += 1;
                    continue;
                }

                if trans_utf16.len() > orig_utf16.len() {
                    if length_skipped < 5 {
                        warnings.push(format!(
                            "translation for '{}' longer than original in UTF-16LE ({} > {} bytes), skipping",
                            entry.id,
                            trans_utf16.len(),
                            orig_utf16.len()
                        ));
                    }
                    length_skipped += 1;
                    strings_skipped += 1;
                    *skip_reasons.entry("too_long".into()).or_default() += 1;
                    continue;
                }

                work.push(Work {
                    entry,
                    needle: orig_utf16,
                    trans: trans_utf16,
                });
            }

            let patterns: Vec<&[u8]> = work.iter().map(|w| w.needle.as_slice()).collect();
            let mut cursor = crate::binary_search::MatchCursor::from_patterns(&bytes, &patterns);

            for (i, w) in work.iter().enumerate() {
                if let Some(pos) = cursor.next_valid(i, &bytes, &w.needle) {
                    bytes[pos..pos + w.trans.len()].copy_from_slice(&w.trans);
                    for b in &mut bytes[pos + w.trans.len()..pos + w.needle.len()] {
                        *b = 0;
                    }
                    strings_written += 1;
                    modified = true;
                    if w.trans.len() < w.needle.len() {
                        if pad_noted < 5 {
                            warnings.push(format!(
                                "padded {} null bytes for '{}'",
                                w.needle.len() - w.trans.len(),
                                w.entry.id
                            ));
                        }
                        pad_noted += 1;
                    }
                } else {
                    strings_skipped += 1;
                    *skip_reasons.entry("source_changed".into()).or_default() += 1;
                }
            }

            if modified {
                std::fs::write(file_path, &bytes)?;
                files_modified += 1;
                files_written.push(file_path.clone());
            }
        }

        if length_skipped > 0 {
            warnings.push(format!(
                "{length_skipped} translation(s) skipped because they are longer than the \
                 original Unreal string (UTF-16LE byte length must be ≤ source). Shorten them or \
                 use a length-aware model; equal-length translations inject cleanly."
            ));
        }

        Ok(InjectionReport {
            skip_reasons,
            files_modified,
            strings_written,
            strings_skipped,
            warnings,
            files_written,
        })
    }
}

/// Same 1 MiB tail the previous full-file scan inspected.
const PAK_MAGIC_TAIL_WINDOW: u64 = 1024 * 1024;

/// Scan only the last `min(len, 1 MiB)` for `PAK_MAGIC`.
///
/// Magic that appears only near the start of a file larger than 1 MiB is
/// intentionally not a hit — detection never looked past that tail window.
fn has_pak_magic_in_tail<R: Read + Seek>(reader: &mut R, len: u64) -> bool {
    if len < 32 {
        return false;
    }
    let window = len.min(PAK_MAGIC_TAIL_WINDOW);
    let start = len - window;
    if reader.seek(SeekFrom::Start(start)).is_err() {
        return false;
    }
    let mut buf = vec![0u8; window as usize];
    if reader.read_exact(&mut buf).is_err() {
        return false;
    }
    let magic = unreal_pak::PAK_MAGIC.to_le_bytes();
    buf.windows(4).any(|w| w == magic)
}

#[cfg(test)]
mod tests {
    #[test]
    fn held_lock_injects_loose_locres_without_releasing_exclusion() {
        let root = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        let path = write_loose_locres(root.path(), crate::unreal_locres::LocresVersion::Compact);
        let original = fs::read(&path).unwrap();
        let plugin = UnrealPlugin::new();
        let mut entries = plugin.extract(root.path()).unwrap();
        entries.retain(|e| e.source == "Hello traveler");
        assert_eq!(entries.len(), 1);
        entries[0].translation = Some("Hola viajero".into());
        let wrong = GameLock::acquire(other.path()).unwrap();
        assert!(plugin
            .inject_under_lock(root.path(), &entries, &wrong)
            .is_err());
        assert_eq!(fs::read(&path).unwrap(), original);
        let lock = GameLock::acquire(root.path()).unwrap();
        assert!(plugin.inject(root.path(), &entries).is_err());
        assert_eq!(fs::read(&path).unwrap(), original);
        let report = plugin
            .inject_under_lock(root.path(), &entries, &lock)
            .unwrap();
        assert_eq!(report.strings_written, 1, "{report:?}");
        assert!(plugin
            .extract(root.path())
            .unwrap()
            .iter()
            .any(|e| e.source == "Hola viajero"));
        assert!(GameLock::acquire(root.path()).is_err());
        drop(lock);
        assert!(GameLock::acquire(root.path()).is_ok());
    }

    use super::*;
    use std::fs;
    use std::io::{Cursor, Read, Seek, SeekFrom};

    fn tempdir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("locust_ue_{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn create_pak_fixture(dir: &Path) -> PathBuf {
        let game_dir = dir.join("TestGame").join("Content").join("Paks");
        fs::create_dir_all(&game_dir).unwrap();

        // Create a fake PAK with embedded UTF-16LE strings
        let mut data: Vec<u8> = vec![0; 32]; // padding
                                             // "Hello World" in UTF-16LE
        for ch in "Hello World".encode_utf16() {
            data.extend_from_slice(&ch.to_le_bytes());
        }
        data.extend_from_slice(&[0, 0]); // null terminator
        data.extend_from_slice(&[0xFF; 16]); // padding
                                             // "Press Start" in UTF-16LE
        for ch in "Press Start".encode_utf16() {
            data.extend_from_slice(&ch.to_le_bytes());
        }
        data.extend_from_slice(&[0, 0]);
        data.extend_from_slice(&[0; 32]); // trailing
                                          // Footer magic — `looks_like_unreal_pak` requires it to tell real paks
                                          // apart from Chromium/NW.js packs.
        data.extend_from_slice(&unreal_pak::PAK_MAGIC.to_le_bytes());
        data.extend_from_slice(&[0; 40]);

        let pak_path = game_dir.join("TestGame.pak");
        fs::write(&pak_path, &data).unwrap();

        dir.to_path_buf()
    }

    fn pak_magic() -> [u8; 4] {
        unreal_pak::PAK_MAGIC.to_le_bytes()
    }

    fn blob_with_magic_at(len: usize, magic_at: usize) -> Vec<u8> {
        let mut data = vec![0u8; len];
        data[magic_at..magic_at + 4].copy_from_slice(&pak_magic());
        data
    }

    struct ReadCounter<R> {
        inner: R,
        bytes_read: usize,
    }

    impl<R: Read> Read for ReadCounter<R> {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            let n = self.inner.read(buf)?;
            self.bytes_read += n;
            Ok(n)
        }
    }

    impl<R: Seek> Seek for ReadCounter<R> {
        fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
            self.inner.seek(pos)
        }
    }

    #[test]
    fn test_pak_override_key_conventional_order() {
        assert!(
            pak_override_key(Path::new("Game.pak")) < pak_override_key(Path::new("Game_P.pak"))
        );
        assert!(
            pak_override_key(Path::new("Game_P.pak")) < pak_override_key(Path::new("Game_P2.pak"))
        );
        assert!(
            pak_override_key(Path::new("Game_1_P.pak"))
                < pak_override_key(Path::new("Game_2_P.pak"))
        );
        assert!(
            pak_override_key(Path::new("Game_2_P.pak"))
                < pak_override_key(Path::new("Game_10_P.pak")),
            "conventional *_10_P must outrank *_2_P numerically, not lexically"
        );
        assert!(
            pak_override_key(Path::new("Game_10_P.pak"))
                < pak_override_key(Path::new("Game_LOCUST_P.pak"))
        );
        assert!(
            pak_override_key(Path::new("Game_P2.pak"))
                < pak_override_key(Path::new("Game_LOCUST_P.pak"))
        );
        assert!(
            pak_override_key(Path::new("aaa_LOCUST_P.pak"))
                > pak_override_key(Path::new("zzz.pak"))
        );
    }

    #[test]
    fn test_pak_magic_in_last_mib_of_large_file_is_detected() {
        let data = blob_with_magic_at(2 * 1024 * 1024, 2 * 1024 * 1024 - 4);
        let len = data.len() as u64;
        let mut cursor = Cursor::new(data);
        assert!(has_pak_magic_in_tail(&mut cursor, len));
    }

    #[test]
    fn test_pak_magic_only_near_start_of_large_file_is_not_detected() {
        // Pins today's tail-only scan: magic at offset 0 of a >1 MiB file is ignored.
        let data = blob_with_magic_at(2 * 1024 * 1024, 0);
        let len = data.len() as u64;
        let mut cursor = Cursor::new(data);
        assert!(!has_pak_magic_in_tail(&mut cursor, len));
    }

    #[test]
    fn test_pak_magic_in_small_file_is_detected_under_32_rejected() {
        let data = blob_with_magic_at(64, 32);
        let len = data.len() as u64;
        let mut cursor = Cursor::new(data);
        assert!(has_pak_magic_in_tail(&mut cursor, len));

        let mut tiny = vec![0u8; 31];
        tiny[..4].copy_from_slice(&pak_magic());
        let tiny_len = tiny.len() as u64;
        let mut tiny_cursor = Cursor::new(tiny);
        assert!(!has_pak_magic_in_tail(&mut tiny_cursor, tiny_len));
    }

    #[test]
    fn test_pak_magic_tail_reads_only_the_window() {
        let data = blob_with_magic_at(2 * 1024 * 1024, 2 * 1024 * 1024 - 4);
        let len = data.len() as u64;
        let mut reader = ReadCounter {
            inner: Cursor::new(data),
            bytes_read: 0,
        };
        assert!(has_pak_magic_in_tail(&mut reader, len));
        assert_eq!(
            reader.bytes_read,
            1024 * 1024,
            "expected a 1 MiB tail window, not a full {len}-byte slurp"
        );
    }

    #[test]
    fn test_detect_unreal() {
        let dir = tempdir();
        create_pak_fixture(&dir);
        let plugin = UnrealPlugin::new();
        assert!(plugin.detect(&dir));
    }

    #[test]
    fn test_detect_skips_pak_walk_when_engine_present() {
        // Negative-tested: restoring the old always-walk detect makes this fail.
        let dir = tempdir();
        fs::create_dir_all(dir.join("Engine")).unwrap();
        // A real Unreal-looking pak that would be opened if detect walked.
        let paks = dir.join("Content").join("Paks");
        fs::create_dir_all(&paks).unwrap();
        let mut data = vec![0u8; 64];
        data[60..64].copy_from_slice(&pak_magic());
        fs::write(paks.join("Game.pak"), &data).unwrap();

        FIND_PAK_FILES_CALLS.with(|c| c.set(Some(0)));
        let plugin = UnrealPlugin::new();
        assert!(plugin.detect(&dir));
        let calls = FIND_PAK_FILES_CALLS.with(|c| c.replace(None));
        assert_eq!(
            calls,
            Some(0),
            "detect must not walk .pak files when Engine/ is already present"
        );
    }

    #[test]
    fn test_detect_skips_pak_walk_when_content_game_present() {
        let dir = tempdir();
        create_pak_fixture(&dir);

        FIND_PAK_FILES_CALLS.with(|c| c.set(Some(0)));
        let plugin = UnrealPlugin::new();
        assert!(plugin.detect(&dir));
        let calls = FIND_PAK_FILES_CALLS.with(|c| c.replace(None));
        assert_eq!(
            calls,
            Some(0),
            "detect must not walk .pak files when a */Content game folder exists"
        );
    }

    #[test]
    fn test_detect_pak_only_tree_still_walks() {
        let dir = tempdir();
        let mut data = vec![0u8; 64];
        data[60..64].copy_from_slice(&pak_magic());
        fs::write(dir.join("orphan.pak"), &data).unwrap();

        FIND_PAK_FILES_CALLS.with(|c| c.set(Some(0)));
        let plugin = UnrealPlugin::new();
        assert!(plugin.detect(&dir));
        let calls = FIND_PAK_FILES_CALLS.with(|c| c.replace(None));
        assert!(
            calls.is_some_and(|n| n >= 1),
            "pak-only trees still need find_pak_files during detect, got {calls:?}"
        );
    }

    #[test]
    fn test_detect_pak_only_tree_stops_after_first_valid_pak() {
        let dir = tempdir();
        let mut data = vec![0u8; 64];
        data[60..64].copy_from_slice(&pak_magic());
        for n in 0..20 {
            fs::write(dir.join(format!("archive_{n}.pak")), &data).unwrap();
        }

        PAK_MAGIC_PROBES.with(|c| c.set(Some(0)));
        assert!(UnrealPlugin::new().detect(&dir));
        let probes = PAK_MAGIC_PROBES.with(|c| c.replace(None));
        assert_eq!(probes, Some(1), "detect only needs the first valid pak");
        assert_eq!(UnrealPlugin::find_pak_files(&dir, false).len(), 20);
    }

    #[test]
    fn test_detect_non_unreal() {
        let dir = tempdir();
        let plugin = UnrealPlugin::new();
        assert!(!plugin.detect(&dir));
    }

    #[test]
    fn test_detect_ignores_nwjs_chromium_paks() {
        // RPG Maker MZ NW.js deploys ship Chromium packs (resources.pak, nw_*.pak,
        // locales/*.pak) that must not classify as Unreal Engine.
        let dir = tempdir();
        fs::write(dir.join("resources.pak"), b"not-an-unreal-pak").unwrap();
        fs::write(dir.join("nw_100_percent.pak"), b"also-not-unreal").unwrap();
        fs::create_dir_all(dir.join("locales")).unwrap();
        fs::write(dir.join("locales").join("en-US.pak"), b"locale-pack").unwrap();
        // Fake game shell like RM MZ
        fs::create_dir_all(dir.join("js")).unwrap();
        fs::write(dir.join("js").join("rmmz_core.js"), b"// mz").unwrap();
        fs::create_dir_all(dir.join("data")).unwrap();

        let plugin = UnrealPlugin::new();
        assert!(
            !plugin.detect(&dir),
            "NW.js/Chromium .pak files must not trigger Unreal detect"
        );
    }

    #[test]
    fn test_extract_utf16le_strings() {
        let dir = tempdir();
        create_pak_fixture(&dir);
        let plugin = UnrealPlugin::new();
        let entries = plugin.extract(&dir).unwrap();
        let sources: Vec<&str> = entries.iter().map(|e| e.source.as_str()).collect();
        assert!(sources.contains(&"Hello World"), "got: {:?}", sources);
        assert!(sources.contains(&"Press Start"), "got: {:?}", sources);
    }

    #[test]
    fn test_inject_shorter_succeeds() {
        let dir = tempdir();
        create_pak_fixture(&dir);
        let plugin = UnrealPlugin::new();
        let mut entries = plugin.extract(&dir).unwrap();

        for entry in &mut entries {
            if entry.source == "Hello World" {
                entry.translation = Some("Hola Mundo".to_string());
            }
        }

        let report = plugin.inject(&dir, &entries).unwrap();
        assert!(report.strings_written >= 1);
    }

    #[test]
    fn test_inject_longer_skips_not_hard_fail() {
        let dir = tempdir();
        create_pak_fixture(&dir);
        let plugin = UnrealPlugin::new();
        let mut entries = plugin.extract(&dir).unwrap();

        for entry in &mut entries {
            if entry.source == "Hello World" {
                entry.translation =
                    Some("This is a much longer translation that exceeds the original".to_string());
            }
        }

        let report = plugin.inject(&dir, &entries).unwrap();
        assert_eq!(report.files_modified, 0);
        assert!(report.strings_skipped >= 1);
        assert!(
            report
                .warnings
                .iter()
                .any(|w| w.contains("skipped because they are longer")),
            "expected length-skip summary, got: {:?}",
            report.warnings
        );
    }

    #[test]
    fn test_inject_identity_skips_write() {
        let dir = tempdir();
        create_pak_fixture(&dir);
        let plugin = UnrealPlugin::new();
        let mut entries = plugin.extract(&dir).unwrap();
        for entry in &mut entries {
            entry.translation = Some(entry.source.clone());
        }
        let report = plugin.inject(&dir, &entries).unwrap();
        assert_eq!(report.files_modified, 0);
        assert_eq!(report.strings_written, 0);
        assert!(report.strings_skipped >= 1);
    }

    /// Multi-pattern Unreal inject: duplicate UTF-16LE needle, identity, oversize.
    /// Entries planted manually (no extract filter dependency).
    #[test]
    fn test_inject_multi_pattern_semantics() {
        let dir = tempdir();
        let game_dir = dir.join("TestGame").join("Content").join("Paks");
        fs::create_dir_all(&game_dir).unwrap();

        fn utf16(s: &str) -> Vec<u8> {
            s.encode_utf16().flat_map(|c| c.to_le_bytes()).collect()
        }

        let mut data: Vec<u8> = vec![0; 32];
        for s in ["AlphaStr", "AlphaStr", "BetaStr!"] {
            data.extend_from_slice(&utf16(s));
            data.extend_from_slice(&[0, 0]);
            data.extend_from_slice(&[0xFF; 8]);
        }
        let pak = game_dir.join("Multi.pak");
        fs::write(&pak, &data).unwrap();

        let mk = |id: &str, source: &str, translation: Option<&str>| {
            let mut e = StringEntry::new(id, source, pak.clone());
            e.translation = translation.map(|s| s.to_string());
            e
        };
        // AlphaStr = 8 chars; AlfaStr! = 8 chars (equal UTF-16LE byte length).
        let inject = vec![
            mk("a1", "AlphaStr", Some("AlfaStr!")),
            mk("a2", "AlphaStr", Some("AlfaStr!")),
            mk("id", "BetaStr!", Some("BetaStr!")), // identity
            mk("over", "BetaStr!", Some("This translation is far too long")), // oversize
        ];

        let plugin = UnrealPlugin::new();
        let report = plugin.inject(&dir, &inject).unwrap();
        assert_eq!(
            report.strings_written, 2,
            "both AlphaStr occurrences: written={} skipped={} {:?}",
            report.strings_written, report.strings_skipped, report.warnings
        );
        assert_eq!(report.strings_skipped, 2, "identity + oversize");

        let out = fs::read(&pak).unwrap();
        let alfa = utf16("AlfaStr!");
        assert_eq!(
            out.windows(alfa.len())
                .filter(|w| *w == alfa.as_slice())
                .count(),
            2
        );
        let beta = utf16("BetaStr!");
        assert!(
            out.windows(beta.len()).any(|w| w == beta.as_slice()),
            "identity BetaStr! must remain"
        );
    }

    fn write_loose_locres(dir: &Path, version: crate::unreal_locres::LocresVersion) -> PathBuf {
        use crate::unreal_locres::{
            str_crc32_ue, LocresFile, LocresNamespace, LocresString, LocresVersion,
        };
        let loc_dir = dir
            .join("TestGame")
            .join("Content")
            .join("Localization")
            .join("Game")
            .join("es");
        fs::create_dir_all(&loc_dir).unwrap();
        let file = LocresFile {
            version,
            namespaces: vec![LocresNamespace {
                name: "Dialog".into(),
                name_hash: if matches!(
                    version,
                    LocresVersion::Optimized | LocresVersion::OptimizedCityHash64Utf16
                ) {
                    str_crc32_ue("Dialog")
                } else {
                    0
                },
                strings: vec![
                    LocresString {
                        key: "Greeting".into(),
                        value: "Hello traveler".into(),
                        source_string_hash: str_crc32_ue("Hello traveler"),
                        key_hash: if matches!(
                            version,
                            LocresVersion::Optimized | LocresVersion::OptimizedCityHash64Utf16
                        ) {
                            str_crc32_ue("Greeting")
                        } else {
                            0
                        },
                    },
                    LocresString {
                        key: "Farewell".into(),
                        value: "See you later".into(),
                        source_string_hash: str_crc32_ue("See you later"),
                        key_hash: if matches!(
                            version,
                            LocresVersion::Optimized | LocresVersion::OptimizedCityHash64Utf16
                        ) {
                            str_crc32_ue("Farewell")
                        } else {
                            0
                        },
                    },
                ],
            }],
        };
        let path = loc_dir.join("Game.locres");
        fs::write(&path, file.serialize().unwrap()).unwrap();
        path
    }

    #[test]
    fn test_locres_loose_extract_inject_e2e_compact() {
        use crate::unreal_locres::LocresVersion;
        let dir = tempdir();
        let loc_path = write_loose_locres(&dir, LocresVersion::Compact);
        // Minimal Unreal tree marker so detect is happy without a pak.
        fs::create_dir_all(dir.join("TestGame").join("Content")).unwrap();

        let plugin = UnrealPlugin::new();
        assert!(plugin.detect(&dir) || plugin.detect(&loc_path));
        let mut entries = plugin.extract(&dir).unwrap();
        assert!(
            entries.iter().any(|e| e.id == "Dialog/Greeting"),
            "ids: {:?}",
            entries.iter().map(|e| &e.id).collect::<Vec<_>>()
        );
        assert!(entries.iter().all(|e| {
            e.metadata.get("extraction_method").and_then(|v| v.as_str()) == Some("locres")
        }));

        for e in &mut entries {
            if e.id == "Dialog/Greeting" {
                e.translation = Some("Hola viajero — un texto mas largo".into());
            }
        }
        let report = plugin.inject(&dir, &entries).unwrap();
        assert!(report.strings_written >= 1, "{report:?}");
        assert!(report.files_modified >= 1);

        let again = plugin.extract(&dir).unwrap();
        assert!(
            again.iter().any(|e| e.source.contains("Hola viajero")),
            "re-extract: {:?}",
            again.iter().map(|e| &e.source).collect::<Vec<_>>()
        );
        // Farewell untouched
        assert!(again.iter().any(|e| e.source == "See you later"));
        let _ = loc_path;
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_locres_rejects_stale_value_and_preserves_other_entries() {
        use crate::unreal_locres::LocresVersion;
        let dir = tempdir();
        let loc_path = write_loose_locres(&dir, LocresVersion::Compact);
        let plugin = UnrealPlugin::new();
        let mut entries = plugin.extract(&loc_path).unwrap();
        entries
            .iter_mut()
            .find(|entry| entry.id == "Dialog/Greeting")
            .unwrap()
            .translation = Some("Hola viajero".into());

        let mut changed = LocresFile::parse_path(&loc_path).unwrap();
        changed.namespaces[0].strings[0].value = "Edited outside Locust".into();
        fs::write(&loc_path, changed.serialize().unwrap()).unwrap();

        let report = plugin.inject(&loc_path, &entries).unwrap();
        assert_eq!(report.strings_written, 0);
        assert_eq!(report.files_modified, 0);
        assert_eq!(report.skip_reasons.get("source_changed"), Some(&1));
        let after = LocresFile::parse_path(&loc_path).unwrap();
        let values: Vec<_> = after.iter_entries().map(|(_, _, value, _)| value).collect();
        assert!(values.contains(&"Edited outside Locust"));
        assert!(values.contains(&"See you later"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_locres_loose_extract_inject_e2e_optimized() {
        use crate::unreal_locres::LocresVersion;
        let dir = tempdir();
        write_loose_locres(&dir, LocresVersion::Optimized);
        fs::create_dir_all(dir.join("TestGame").join("Content")).unwrap();
        let plugin = UnrealPlugin::new();
        let mut entries = plugin.extract(&dir).unwrap();
        for e in &mut entries {
            if e.id == "Dialog/Farewell" {
                e.translation = Some("Hasta luego amigo".into());
            }
        }
        let report = plugin.inject(&dir, &entries).unwrap();
        assert!(report.strings_written >= 1, "{report:?}");
        let again = plugin.extract(&dir).unwrap();
        assert!(again.iter().any(|e| e.source.contains("Hasta luego")));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_locres_malformed_errors_loudly() {
        let dir = tempdir();
        let loc = dir.join("broken.locres");
        let mut bad = crate::unreal_locres::LOCRES_MAGIC.to_vec();
        bad.push(1); // Compact
                     // truncated — no offset / tables
        fs::write(&loc, &bad).unwrap();
        let plugin = UnrealPlugin::new();
        let err = plugin.extract(&loc).unwrap_err();
        assert!(
            err.to_string().contains("broken.locres") || err.to_string().contains("truncated"),
            "{err}"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_pak_heuristic_skips_values_also_in_loose_locres() {
        use crate::unreal_locres::LocresVersion;
        let dir = tempdir();
        create_pak_fixture(&dir); // contains "Hello World"
                                  // Locres with the same string — extract should prefer locres for that value
                                  // when both exist; at least not double-count as two independent heuristics.
        write_loose_locres(&dir, LocresVersion::Compact);
        // Force a locres string equal to a pak string
        let loc_dir = dir
            .join("TestGame")
            .join("Content")
            .join("Localization")
            .join("Game")
            .join("en");
        fs::create_dir_all(&loc_dir).unwrap();
        use crate::unreal_locres::{str_crc32_ue, LocresFile, LocresNamespace, LocresString};
        let f = LocresFile {
            version: LocresVersion::Compact,
            namespaces: vec![LocresNamespace {
                name: "UI".into(),
                name_hash: 0,
                strings: vec![LocresString {
                    key: "HelloKey".into(),
                    value: "Hello World".into(),
                    source_string_hash: str_crc32_ue("Hello World"),
                    key_hash: 0,
                }],
            }],
        };
        fs::write(loc_dir.join("Game.locres"), f.serialize().unwrap()).unwrap();

        let plugin = UnrealPlugin::new();
        let entries = plugin.extract(&dir).unwrap();
        let hello_hits: Vec<_> = entries
            .iter()
            .filter(|e| e.source == "Hello World")
            .collect();
        // Prefer structural locres — only one entry with that source, method locres.
        assert_eq!(
            hello_hits.len(),
            1,
            "expected de-duped Hello World, got {:?}",
            hello_hits
                .iter()
                .map(|e| (&e.id, e.metadata.get("extraction_method")))
                .collect::<Vec<_>>()
        );
        assert_eq!(
            hello_hits[0]
                .metadata
                .get("extraction_method")
                .and_then(|v| v.as_str()),
            Some("locres")
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_embedded_locres_inject_writes_locust_p_pak() {
        embedded_locres_inject_writes_locust_p_pak(false);
        embedded_locres_inject_writes_locust_p_pak(true);
    }

    fn embedded_locres_inject_writes_locust_p_pak(borrow_lock: bool) {
        use crate::unreal_locres::{
            str_crc32_ue, LocresFile, LocresNamespace, LocresString, LocresVersion,
        };
        use crate::unreal_pak::{read_index, write_pak, PakWriteFile, DEFAULT_MOUNT_POINT};

        let dir = tempdir();
        let paks = dir.join("TestGame").join("Content").join("Paks");
        fs::create_dir_all(&paks).unwrap();

        let loc = LocresFile {
            version: LocresVersion::Compact,
            namespaces: vec![LocresNamespace {
                name: "Dialog".into(),
                name_hash: 0,
                strings: vec![LocresString {
                    key: "Greeting".into(),
                    value: "Hello traveler".into(),
                    source_string_hash: str_crc32_ue("Hello traveler"),
                    key_hash: 0,
                }],
            }],
        };
        let loc_bytes = loc.serialize().unwrap();
        let inner = "TestGame/Content/Localization/Game/es/Game.locres";
        let base_bytes = write_pak(
            DEFAULT_MOUNT_POINT,
            8,
            &[PakWriteFile {
                name: inner.into(),
                data: loc_bytes,
            }],
            "base.pak",
        )
        .unwrap();
        let base_path = paks.join("Game.pak");
        fs::write(&base_path, &base_bytes).unwrap();

        let plugin = UnrealPlugin::new();
        let mut entries = plugin.extract(&dir).unwrap();
        let loc_entries: Vec<_> = entries
            .iter()
            .filter(|e| e.metadata.get("locres_embedded") == Some(&serde_json::Value::Bool(true)))
            .cloned()
            .collect();
        assert!(
            !loc_entries.is_empty(),
            "expected embedded locres entries, got {:?}",
            entries.iter().map(|e| &e.id).collect::<Vec<_>>()
        );

        for e in &mut entries {
            if e.source == "Hello traveler" {
                e.translation = Some("Hola viajero desde patch pak".into());
            }
        }
        let lock = borrow_lock.then(|| GameLock::acquire(&dir).unwrap());
        let report = match &lock {
            Some(lock) => plugin.inject_under_lock(&dir, &entries, lock),
            None => plugin.inject(&dir, &entries),
        }
        .unwrap();
        if borrow_lock {
            assert!(GameLock::acquire(&dir).is_err());
        }
        assert!(
            report.strings_written >= 1,
            "written={} warnings={:?}",
            report.strings_written,
            report.warnings
        );
        assert!(
            report
                .files_written
                .iter()
                .any(|p| p.to_string_lossy().contains("_LOCUST_P.pak")),
            "files_written={:?}",
            report.files_written
        );

        let patch_path = paks.join("Game_LOCUST_P.pak");
        assert!(patch_path.is_file(), "missing {}", patch_path.display());
        let patch = fs::read(&patch_path).unwrap();
        let idx = read_index(&patch, "patch").unwrap();
        assert_eq!(idx.records.len(), 1);
        assert!(idx.records[0].name.contains("Game.locres"));

        // Decode locres from patch payload
        let rec = &idx.records[0];
        let poff = crate::unreal_pak::payload_offset(rec, idx.footer.version) as usize;
        let payload = &patch[poff..poff + rec.size as usize];
        let parsed = LocresFile::parse(payload, "patch-locres").unwrap();
        assert!(
            parsed
                .iter_entries()
                .any(|(_, _, v, _)| v.contains("Hola viajero")),
            "patch locres missing translation"
        );
        // Base pak unchanged
        assert_eq!(fs::read(&base_path).unwrap(), base_bytes);
        drop(lock);
        let _ = fs::remove_dir_all(&dir);
    }
}
