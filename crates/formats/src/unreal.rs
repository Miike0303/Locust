use std::collections::HashMap;
use std::path::{Path, PathBuf};

use locust_core::error::{LocustError, Result};
use locust_core::extraction::{FormatPlugin, InjectionReport};
use locust_core::models::{OutputMode, StringEntry};

use crate::unreal_locres::{self, LocresFile};
use crate::unreal_pak::{
    self, payload_offset, read_footer, read_index, record_containing_offset, write_pak,
    writable_version, PakWriteFile, DEFAULT_MOUNT_POINT,
};

/// Plugin for Unreal Engine games.
/// Scans .pak files and loose localization files for translatable strings.
///
/// Unreal stores localization in:
///   Content/Localization/{target}/{culture}/{target}.locres (binary — structural)
///   Content/Localization/{target}/{culture}/{target}.po (text PO files — if present)
///   .pak files contain packed assets (heuristic UTF-16LE + embedded .locres scan)
pub struct UnrealPlugin;

impl UnrealPlugin {
    pub fn new() -> Self {
        Self
    }

    fn find_pak_files(path: &Path) -> Vec<PathBuf> {
        let mut paks = Vec::new();
        if path.is_file() && path.extension().is_some_and(|e| e == "pak") {
            paks.push(path.to_path_buf());
            return paks;
        }
        if path.is_dir() {
            for entry in walkdir::WalkDir::new(path)
                .max_depth(5)
                .follow_links(false)
                .into_iter()
                .filter_map(|e| e.ok())
            {
                if entry.path().extension().is_some_and(|e| e == "pak") {
                    paks.push(entry.path().to_path_buf());
                }
            }
        }
        paks
    }

    fn has_unreal_structure(path: &Path) -> bool {
        if !path.is_dir() {
            return false;
        }
        // Check for typical Unreal folder structure
        let has_engine = path.join("Engine").is_dir();
        let game_name = path
            .read_dir()
            .ok()
            .and_then(|mut d| d.find(|e| {
                e.as_ref().ok().is_some_and(|e| {
                    e.path().is_dir()
                        && e.path().join("Content").is_dir()
                })
            }))
            .is_some();
        let has_content_paks = !Self::find_pak_files(path).is_empty();

        has_engine || game_name || has_content_paks
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
        Ok(locres_to_entries(&file, path))
    }

    /// Extract UTF-16LE strings from PAK file using heuristic scanning, plus any
    /// embedded LocRes blobs (structural). Heuristic hits that equal a locres
    /// string value are dropped to avoid double-extraction.
    fn extract_strings_from_pak(
        bytes: &[u8],
        filename: &str,
        file_path: &Path,
    ) -> Vec<StringEntry> {
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
                    let mut loc_entries = locres_to_entries(&file, file_path);
                    for e in &mut loc_entries {
                        // A pak carries one locres blob PER CULTURE with identical
                        // namespace/key sets — the blob offset keeps ids unique so
                        // cultures don't silently overwrite each other in the DB.
                        e.id = format!("locres@{off}/{}", e.id);
                        e.metadata.insert(
                            "locres_embedded".to_string(),
                            serde_json::Value::Bool(true),
                        );
                        e.metadata.insert(
                            "locres_offset".to_string(),
                            serde_json::json!(off),
                        );
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

        entries
    }
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

/// Group embedded locres entries by `locres_offset`, apply translations, write
/// one `<base>_LOCUST_P.pak` beside the source pak.
///
/// Returns `(strings_written, strings_skipped, optional_patch_path)`.
fn inject_embedded_locres_patch_pak(
    pak_path: &Path,
    entries: &[&StringEntry],
    warnings: &mut Vec<String>,
) -> Result<(usize, usize, Option<PathBuf>)> {
    let label = pak_path.display().to_string();
    let pak_bytes = std::fs::read(pak_path)?;
    let footer = read_footer(&pak_bytes, &label).map_err(|e| LocustError::ParseError {
        file: label.clone(),
        message: e.message,
    })?;
    let write_ver = writable_version(footer.version).map_err(|e| LocustError::ParseError {
        file: label.clone(),
        message: e.message,
    })?;

    // Classic index for path lookup; if unavailable (v9+ path-hash), fall back
    // to synthetic paths from offsets so we still produce a usable patch pak.
    let index = read_index(&pak_bytes, &label).ok();
    let mount = index
        .as_ref()
        .map(|i| i.mount_point.clone())
        .unwrap_or_else(|| DEFAULT_MOUNT_POINT.to_string());

    // offset → list of entries
    let mut by_off: HashMap<u64, Vec<&StringEntry>> = HashMap::new();
    let mut skipped = 0usize;
    for e in entries {
        let Some(off) = e
            .metadata
            .get("locres_offset")
            .and_then(|v| v.as_u64())
            .or_else(|| e.metadata.get("locres_offset").and_then(|v| v.as_i64()).map(|i| i as u64))
        else {
            warnings.push(format!("entry '{}' missing locres_offset", e.id));
            skipped += 1;
            continue;
        };
        by_off.entry(off).or_default().push(*e);
    }

    let mut pak_files: Vec<PakWriteFile> = Vec::new();
    let mut written = 0usize;

    for (off, group) in by_off {
        if off as usize >= pak_bytes.len() {
            warnings.push(format!("locres_offset {off} past EOF of {label}"));
            skipped += group.len();
            continue;
        }
        let mut loc = match LocresFile::parse(&pak_bytes[off as usize..], &format!("{label}+@{off}"))
        {
            Ok(l) => l,
            Err(e) => {
                warnings.push(format!("parse locres @{off}: {e}"));
                skipped += group.len();
                continue;
            }
        };

        let mut map = HashMap::new();
        let mut pending = 0usize;
        for e in &group {
            let Some(t) = e.translation.as_ref() else {
                skipped += 1;
                continue;
            };
            if t == &e.source {
                skipped += 1;
                continue;
            }
            // Strip "locres@<off>/" prefix from id for key lookup.
            let key_id = e
                .id
                .strip_prefix(&format!("locres@{off}/"))
                .unwrap_or(e.id.as_str());
            map.insert(key_id.to_string(), t.clone());
            pending += 1;
        }
        if map.is_empty() {
            continue;
        }
        let n = loc.apply_translations(&map);
        if n == 0 {
            skipped += pending;
            warnings.push(format!(
                "locres @{off}: no keys matched for {pending} translation(s)"
            ));
            continue;
        }
        if pending > n {
            skipped += pending - n;
        }
        written += n;

        let payload = loc.serialize().map_err(|e| LocustError::ParseError {
            file: label.clone(),
            message: format!("serialize locres @{off}: {e}"),
        })?;

        // Resolve inner path from pak index when possible.
        let inner_name = if let Some(ref idx) = index {
            find_record_for_locres_offset(idx, off)
                .map(|r| r.name.clone())
                .unwrap_or_else(|| {
                    format!("TestGame/Content/Localization/locres_{off}.locres")
                })
        } else {
            format!("TestGame/Content/Localization/locres_{off}.locres")
        };

        pak_files.push(PakWriteFile {
            name: inner_name,
            data: payload,
        });
    }

    if pak_files.is_empty() {
        return Ok((written, skipped, None));
    }

    let patch_bytes = write_pak(&mount, write_ver, &pak_files, &label).map_err(|e| {
        LocustError::ParseError {
            file: label.clone(),
            message: e.message,
        }
    })?;

    let out_path = unreal_pak::patch_pak_path(pak_path);
    if out_path.exists() {
        // Safety rename if replacing different content.
        let existing = std::fs::read(&out_path).unwrap_or_default();
        if existing != patch_bytes {
            let backup = {
                let mut s = out_path.to_string_lossy().into_owned();
                s.push_str(".locust-old");
                PathBuf::from(s)
            };
            let _ = std::fs::remove_file(&backup);
            let _ = std::fs::rename(&out_path, &backup);
        }
    }
    std::fs::write(&out_path, &patch_bytes)?;
    warnings.push(format!(
        "wrote localization patch pak {} (version {write_ver}, {} file(s))",
        out_path.display(),
        pak_files.len()
    ));
    Ok((written, skipped, Some(out_path)))
}

fn locres_to_entries(file: &LocresFile, file_path: &Path) -> Vec<StringEntry> {
    let mut entries = Vec::new();
    for (ns, key, value, source_hash) in file.iter_entries() {
        if value.trim().is_empty() {
            continue;
        }
        let id = if ns.is_empty() {
            key.to_string()
        } else {
            format!("{ns}/{key}")
        };
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
    entries
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
                        if ch.is_alphanumeric() || ch.is_whitespace() || ".,!?;:'\"()-".contains(ch) {
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
    let ascii_printable = text.chars().filter(|c| c.is_ascii_graphic() || c.is_ascii_whitespace()).count();

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
    let upper_lower_transitions = text.as_bytes().windows(2)
        .filter(|w| w[0].is_ascii_uppercase() && w[1].is_ascii_lowercase())
        .count();
    if !has_space && upper_lower_transitions >= 3 {
        return false; // Likely camelCase identifier
    }

    // All chars should be printable ASCII or common Unicode
    text.chars().all(|c| c.is_ascii_graphic() || c.is_ascii_whitespace() || c.is_alphabetic())
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
        "Unreal Engine (.pak heuristic UTF-16LE + structural .locres; patch *_LOCUST_P.pak)"
    }

    fn stability(&self) -> locust_core::extraction::FormatStability {
        // Phase-2 apply proven on Last Hope patch pak; base multi-GB paks are heuristic.
        locust_core::extraction::FormatStability::Experimental
    }

    fn supported_extensions(&self) -> &[&str] {
        &[".pak", ".locres"]
    }

    fn supported_modes(&self) -> Vec<OutputMode> {
        vec![OutputMode::Replace]
    }

    fn detect(&self, path: &Path) -> bool {
        if path.is_file() {
            return path.extension().is_some_and(|e| {
                e == "pak" || e.eq_ignore_ascii_case("locres")
            });
        }
        Self::has_unreal_structure(path) || !Self::find_loose_locres(path).is_empty()
    }

    fn extract(&self, path: &Path) -> Result<Vec<StringEntry>> {
        let root = if path.is_dir() {
            path.to_path_buf()
        } else {
            path.parent().unwrap_or(path).to_path_buf()
        };

        let paks = Self::find_pak_files(path);
        let mut locres_files = Self::find_loose_locres(path);
        // Single-file .locres open
        if path.is_file() && is_locres_path(path) {
            locres_files = vec![path.to_path_buf()];
        }

        if paks.is_empty() && locres_files.is_empty() {
            return Err(LocustError::ParseError {
                file: path.display().to_string(),
                message: "no .pak or .locres files found".to_string(),
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

        for pak in &paks {
            let bytes = std::fs::read(pak)?;
            let filename = pak
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            let mut from_pak = Self::extract_strings_from_pak(&bytes, &filename, pak);
            // Prefer loose locres for the same string values (better inject path).
            from_pak.retain(|e| {
                if e.metadata.get("extraction_method").and_then(|v| v.as_str())
                    == Some("locres")
                {
                    return true;
                }
                !loose_locres_values.contains(&e.source)
            });
            all.extend(from_pak);
        }

        let _ = root;
        Ok(all)
    }

    fn inject(&self, path: &Path, entries: &[StringEntry]) -> Result<InjectionReport> {
        // Locres: structural rewrite (variable length). Other entries: UTF-16LE
        // in-place slot patch (identity skip, oversize skip, multi-pattern scan).
        let _ = path;
        let mut files_modified = 0;
        let mut strings_written = 0;
        let mut strings_skipped = 0;
        let mut length_skipped = 0usize;
        let mut pad_noted = 0usize;
        let mut warnings = Vec::new();
        let mut files_written: Vec<PathBuf> = Vec::new();

        let mut by_file: HashMap<PathBuf, Vec<&StringEntry>> = HashMap::new();
        for entry in entries {
            by_file
                .entry(entry.file_path.clone())
                .or_default()
                .push(entry);
        }

        for (file_path, file_entries) in &by_file {
            if !file_path.exists() {
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
                        continue;
                    }
                };
                let mut map = HashMap::new();
                let mut pending = 0usize;
                for e in file_entries {
                    let Some(t) = e.translation.as_ref() else {
                        strings_skipped += 1;
                        continue;
                    };
                    if t == &e.source {
                        strings_skipped += 1;
                        continue;
                    }
                    // id is "ns/key" or "key"
                    map.insert(e.id.clone(), t.clone());
                    pending += 1;
                }
                if map.is_empty() {
                    continue;
                }
                let n = loc.apply_translations(&map);
                if n == 0 {
                    strings_skipped += pending;
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
                        }
                    }
                    Err(e) => {
                        warnings.push(format!("serialize locres {label}: {e}"));
                        strings_skipped += pending;
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
                        &embedded,
                        &mut warnings,
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
            files_modified,
            strings_written,
            strings_skipped,
            warnings,
            files_written,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

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

        let pak_path = game_dir.join("TestGame.pak");
        fs::write(&pak_path, &data).unwrap();

        dir.to_path_buf()
    }

    #[test]
    fn test_detect_unreal() {
        let dir = tempdir();
        create_pak_fixture(&dir);
        let plugin = UnrealPlugin::new();
        assert!(plugin.detect(&dir));
    }

    #[test]
    fn test_detect_non_unreal() {
        let dir = tempdir();
        let plugin = UnrealPlugin::new();
        assert!(!plugin.detect(&dir));
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
                entry.translation = Some(
                    "This is a much longer translation that exceeds the original".to_string(),
                );
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
            report.strings_written,
            report.strings_skipped,
            report.warnings
        );
        assert_eq!(report.strings_skipped, 2, "identity + oversize");

        let out = fs::read(&pak).unwrap();
        let alfa = utf16("AlfaStr!");
        assert_eq!(
            out.windows(alfa.len()).filter(|w| *w == alfa.as_slice()).count(),
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
                .map(|e| (
                    &e.id,
                    e.metadata.get("extraction_method")
                ))
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
            .filter(|e| {
                e.metadata.get("locres_embedded") == Some(&serde_json::Value::Bool(true))
            })
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
        let report = plugin.inject(&dir, &entries).unwrap();
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
        let _ = fs::remove_dir_all(&dir);
    }
}
