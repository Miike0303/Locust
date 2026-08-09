use std::collections::HashMap;
use std::path::{Path, PathBuf};

use locust_core::error::{LocustError, Result};
use locust_core::extraction::{FormatPlugin, InjectionReport};
use locust_core::models::{OutputMode, StringEntry};

use crate::unity_serialized::{
    is_binary_looking_script, is_textasset_script_worth_extracting, looks_like_assembly_qualified_type,
    looks_like_code_identifier, looks_like_lorem_ipsum, looks_like_naninovel_script,
    rewrite_text_asset_script_inplace, SerializedFile,
};

/// Plugin for Unity Engine games.
/// Supports:
/// 1. Text-based VN scripts (SCRIPTS~/ directory with .txt dialogue files)
/// 2. Structural TextAsset + MonoBehaviour + TextMesh + GUIText extraction from
///    SerializedFile `.assets` / `level*` (type-tree blobs skipped; no full type-tree walk)
/// 3. Heuristic length-prefixed UTF-8 scan of the same files (skips structural ranges)
pub struct UnityPlugin;

impl UnityPlugin {
    pub fn new() -> Self {
        Self
    }

    fn has_unity_structure(path: &Path) -> bool {
        if !path.is_dir() {
            return false;
        }
        let has_unity_dll = path.join("UnityPlayer.dll").exists()
            || path.join("UnityPlayer.so").exists()
            || path.join("UnityPlayer.dylib").exists();
        if has_unity_dll {
            return true;
        }
        Self::find_data_dir(path).is_some()
    }

    fn find_data_dir(path: &Path) -> Option<PathBuf> {
        for entry in std::fs::read_dir(path).ok()?.flatten() {
            let p = entry.path();
            if p.is_dir() {
                let name = p.file_name()?.to_string_lossy().to_string();
                if name.ends_with("_Data") {
                    return Some(p);
                }
            }
        }
        None
    }

    /// Check if this Unity game has text-based VN scripts (SCRIPTS~ directory or similar)
    fn find_scripts_dir(path: &Path) -> Option<PathBuf> {
        let data_dir = Self::find_data_dir(path)?;
        // Look for any directory containing .txt script files
        // Check common names: SCRIPTS~, Scripts, scripts, SCRIPTS
        for name in &["SCRIPTS~", "Scripts", "scripts", "SCRIPTS"] {
            let scripts = data_dir.join(name);
            if scripts.is_dir() {
                return Some(scripts);
            }
        }
        // Also scan for directories with .txt files that look like scripts
        for entry in std::fs::read_dir(&data_dir).ok()?.flatten() {
            let p = entry.path();
            if p.is_dir() {
                let dir_name = p.file_name()?.to_string_lossy();
                if dir_name.contains("SCRIPT") || dir_name.contains("script") || dir_name.contains("Script") {
                    return Some(p);
                }
            }
        }
        None
    }

    // ─── Text Script Extraction (VN engine) ─────────────────────────────────

    /// Extract dialogue from text-based VN scripts.
    /// Format: lines like `CharacterID Dialogue text` or `CharacterID"Dialogue text"`
    /// Also extracts menu button labels: `button N "Label" ...`
    fn extract_text_scripts(scripts_dir: &Path) -> Result<Vec<StringEntry>> {
        let mut all = Vec::new();

        for entry in walkdir::WalkDir::new(scripts_dir)
            .follow_links(false)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            let fpath = entry.path();
            if fpath.extension().is_none_or(|e| e != "txt") {
                continue;
            }

            let content = match std::fs::read_to_string(fpath) {
                Ok(c) => c,
                Err(_) => continue,
            };

            let filename = fpath
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();

            for (line_idx, line) in content.lines().enumerate() {
                let line_num = line_idx + 1;
                let trimmed = line.trim();

                // Skip empty, comments, directives
                if trimmed.is_empty()
                    || trimmed.starts_with('#')
                    || trimmed.starts_with("version ")
                    || trimmed.starts_with("script ")
                    || trimmed.starts_with("index ")
                    || trimmed.starts_with("scene ")
                    || trimmed.starts_with("music ")
                    || trimmed.starts_with("ambient ")
                    || trimmed.starts_with("sound ")
                    || trimmed.starts_with("jump ")
                    || trimmed.starts_with("menu ")
                    || trimmed.starts_with("type ")
                    || trimmed.starts_with("load ")
                    || trimmed.starts_with("when ")
                    || trimmed.starts_with("{")
                    || trimmed.starts_with("}")
                    || trimmed.starts_with("+")
                    || trimmed.starts_with("game {")
                    || trimmed.starts_with("start ")
                    || trimmed.starts_with("combat ")
                    || trimmed.starts_with("gallery ")
                    || trimmed.starts_with("items ")
                    || trimmed.starts_with("name ")
                    || trimmed.starts_with("#region")
                    || trimmed.starts_with("#endregion")
                    || trimmed.starts_with("#if")
                    || trimmed.starts_with("#else")
                    || trimmed.starts_with("#endif")
                    || trimmed.starts_with("character ")
                {
                    continue;
                }

                // Menu button: `button N "Label" ...`
                if trimmed.starts_with("button ") {
                    if let Some(text) = extract_quoted_in_line(trimmed) {
                        let id = format!("{}#{}", filename, line_num);
                        let mut entry = StringEntry::new(id, text, fpath.to_path_buf());
                        entry.tags = vec!["menu".to_string()];
                        all.push(entry);
                    }
                    continue;
                }

                // Dialogue: `CharID Text here` or `CharID Text with \bformatting\b`
                if let Some((character, text)) = extract_vn_dialogue(trimmed) {
                    if !text.is_empty() && text.len() >= 2 {
                        // Strip format codes for translation, store clean text
                        let clean = strip_vn_format_codes(text);
                        if !clean.is_empty() && clean.len() >= 2 {
                            let id = format!("{}#{}", filename, line_num);
                            let mut entry = StringEntry::new(&id, &clean, fpath.to_path_buf());
                            entry.tags = vec!["dialogue".to_string()];
                            entry.context = Some(character.to_string());
                            // Store original text with format codes in metadata
                            entry.metadata.insert(
                                "original_with_codes".to_string(),
                                serde_json::Value::String(text.to_string()),
                            );
                            all.push(entry);
                        }
                    }
                }
            }
        }

        Ok(all)
    }

    fn inject_text_scripts(_path: &Path, entries: &[StringEntry]) -> Result<InjectionReport> {
        let mut files_modified = 0;
        let mut strings_written = 0;
        let mut strings_skipped = 0;
        let mut files_written: Vec<PathBuf> = Vec::new();

        let mut by_file: HashMap<PathBuf, Vec<&StringEntry>> = HashMap::new();
        for entry in entries {
            by_file.entry(entry.file_path.clone()).or_default().push(entry);
        }

        for (file_path, file_entries) in &by_file {
            if !file_path.exists() {
                continue;
            }
            let content = std::fs::read_to_string(file_path)?;
            let filename = file_path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();

            let mut line_translations: HashMap<usize, (&str, &str)> = HashMap::new();
            for entry in file_entries {
                let id_suffix = entry.id.strip_prefix(&format!("{}#", filename));
                if let Some(num_str) = id_suffix {
                    if let Ok(line_num) = num_str.parse::<usize>() {
                        if let Some(ref t) = entry.translation {
                            line_translations.insert(line_num, (&entry.source, t.as_str()));
                            strings_written += 1;
                        } else {
                            strings_skipped += 1;
                        }
                    }
                }
            }

            let mut new_lines = Vec::new();
            let mut modified = false;
            for (line_idx, line) in content.lines().enumerate() {
                let line_num = line_idx + 1;
                if let Some((source, translation)) = line_translations.get(&line_num) {
                    let trimmed = line.trim();

                    // Button lines: only replace the quoted label
                    if trimmed.starts_with("button ") {
                        let search = format!("\"{}\"", source);
                        let replace = format!("\"{}\"", translation);
                        if line.contains(&search) {
                            new_lines.push(line.replacen(&search, &replace, 1));
                            modified = true;
                            continue;
                        }
                        new_lines.push(line.to_string());
                        continue;
                    }

                    // Dialogue lines: CharID Text → CharID TranslatedText
                    // Source was stored with format codes stripped.
                    // Find the original text (with codes) in the line and replace,
                    // preserving format codes around the translation.
                    let trimmed_line = line.trim();
                    if let Some(space_pos) = trimmed_line.find(' ') {
                        let after_char = &trimmed_line[space_pos + 1..];
                        let (prefix_codes, _inner, suffix_codes) = split_format_codes(after_char);
                        // Reconstruct: indent + CharID + space + prefix_codes + translation + suffix_codes
                        let indent = &line[..line.len() - trimmed_line.len()];
                        let char_id = &trimmed_line[..space_pos];
                        let translated_with_codes = format!(
                            "{}{} {}{}{}",
                            indent, char_id,
                            prefix_codes, translation, suffix_codes
                        );
                        new_lines.push(translated_with_codes);
                        modified = true;
                        continue;
                    }
                }
                new_lines.push(line.to_string());
            }

            if modified {
                std::fs::write(file_path, new_lines.join("\n"))?;
                files_modified += 1;
                files_written.push(file_path.clone());
            }
        }

        Ok(InjectionReport {
            files_modified,
            strings_written,
            strings_skipped,
            warnings: Vec::new(),
            files_written,
        })
    }

    // ─── Binary .assets Extraction (fallback) ───────────────────────────────

    fn find_assets_files(path: &Path) -> Vec<PathBuf> {
        let mut assets = Vec::new();
        if path.is_file() {
            if is_unity_serialized_candidate(path) {
                assets.push(path.to_path_buf());
            }
            return assets;
        }
        let data_dir = if path.is_dir() {
            if let Some(d) = Self::find_data_dir(path) {
                d
            } else if path
                .file_name()
                .is_some_and(|n| n.to_string_lossy().ends_with("_Data"))
            {
                path.to_path_buf()
            } else {
                return assets;
            }
        } else {
            return assets;
        };

        // Depth 3 reaches e.g. `*_Data/subdir/level0` and keeps walk cheap.
        // (Addressable bundles under StreamingAssets/aa/… are not classic
        // SerializedFiles and are filtered by `is_unity_serialized_candidate`.)
        for entry in walkdir::WalkDir::new(&data_dir)
            .max_depth(3)
            .follow_links(false)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            let p = entry.path();
            if p.is_file() && is_unity_serialized_candidate(p) {
                assets.push(p.to_path_buf());
            }
        }
        assets
    }

    /// Structural TextAsset + heuristic scan (skipping TextAsset ranges).
    fn extract_strings_from_assets(
        bytes: &[u8],
        filename: &str,
        file_path: &Path,
    ) -> Vec<StringEntry> {
        let mut entries = Vec::new();
        let mut skip_ranges: Vec<(usize, usize)> = Vec::new();

        match SerializedFile::parse(bytes.to_vec(), file_path) {
            Ok(sf) => {
                // Structural ranges + MonoScript/Shader (type names / HLSL noise).
                skip_ranges = sf.heuristic_skip_byte_ranges();
                for obj in sf.text_asset_objects() {
                    match sf.read_text_asset(obj.path_id) {
                        Ok(ta) => {
                            if !is_textasset_script_worth_extracting(&ta.script) {
                                continue;
                            }
                            // Naninovel / ICU locale name catalogs (`af: Afrikaans`, …) —
                            // not game UI; skip whole asset (BOXMAN ~233 rows).
                            if is_locale_catalog_script(&ta.script) {
                                continue;
                            }
                            // Simple CSV tables (header + data): text columns as cells.
                            if let Some(csv) = parse_textasset_csv(&ta.script) {
                                let newline = if ta.script.contains("\r\n") {
                                    "\r\n"
                                } else {
                                    "\n"
                                };
                                for cell in csv.cells {
                                    let id = format!(
                                        "textasset/{}/csv/{}/{}",
                                        ta.path_id, cell.row, cell.col
                                    );
                                    let mut entry = StringEntry::new(
                                        id,
                                        cell.value.clone(),
                                        file_path.to_path_buf(),
                                    );
                                    entry.tags = vec![
                                        "textasset".to_string(),
                                        "textasset_csv".to_string(),
                                    ];
                                    entry.context = Some(if ta.name.is_empty() {
                                        format!("csv col={} row={}", cell.header, cell.row)
                                    } else {
                                        format!(
                                            "m_Name={} csv col={} row={}",
                                            ta.name, cell.header, cell.row
                                        )
                                    });
                                    entry.metadata.insert(
                                        "extraction_method".to_string(),
                                        serde_json::Value::String(
                                            "textasset_csv_cell".to_string(),
                                        ),
                                    );
                                    entry.metadata.insert(
                                        "path_id".to_string(),
                                        serde_json::json!(ta.path_id),
                                    );
                                    entry.metadata.insert(
                                        "name".to_string(),
                                        serde_json::Value::String(ta.name.clone()),
                                    );
                                    entry.metadata.insert(
                                        "textasset_script_offset".to_string(),
                                        serde_json::json!(ta.script_len_offset),
                                    );
                                    entry.metadata.insert(
                                        "textasset_script_byte_len".to_string(),
                                        serde_json::json!(ta.script_byte_len),
                                    );
                                    entry.metadata.insert(
                                        "csv_row".to_string(),
                                        serde_json::json!(cell.row),
                                    );
                                    entry.metadata.insert(
                                        "csv_col".to_string(),
                                        serde_json::json!(cell.col),
                                    );
                                    entry.metadata.insert(
                                        "csv_header".to_string(),
                                        serde_json::Value::String(cell.header),
                                    );
                                    entry.metadata.insert(
                                        "newline".to_string(),
                                        serde_json::Value::String(newline.to_string()),
                                    );
                                    entry.metadata.insert(
                                        "binary_slot".to_string(),
                                        serde_json::Value::String("utf8".to_string()),
                                    );
                                    entries.push(entry);
                                }
                                continue;
                            }
                            // Naninovel ManagedText / locale docs: split Key: Value lines
                            // so each UI string is a translateable row (inject rebuilds blob).
                            if let Some(lines) = parse_textasset_loc_lines(&ta.script) {
                                let newline = if ta.script.contains("\r\n") {
                                    "\r\n"
                                } else {
                                    "\n"
                                };
                                for (line_index, loc) in lines.into_iter().enumerate() {
                                    let id = format!(
                                        "textasset/{}/line/{}",
                                        ta.path_id, line_index
                                    );
                                    let mut entry = StringEntry::new(
                                        id,
                                        loc.value.clone(),
                                        file_path.to_path_buf(),
                                    );
                                    entry.tags = vec![
                                        "textasset".to_string(),
                                        "textasset_loc".to_string(),
                                    ];
                                    entry.context = Some(match (&ta.name, &loc.key) {
                                        (n, Some(k)) if !n.is_empty() => {
                                            format!("m_Name={n} key={k}")
                                        }
                                        (n, _) if !n.is_empty() => format!("m_Name={n}"),
                                        (_, Some(k)) => format!("key={k}"),
                                        _ => format!("line={line_index}"),
                                    });
                                    entry.metadata.insert(
                                        "extraction_method".to_string(),
                                        serde_json::Value::String(
                                            "textasset_loc_line".to_string(),
                                        ),
                                    );
                                    entry.metadata.insert(
                                        "path_id".to_string(),
                                        serde_json::json!(ta.path_id),
                                    );
                                    entry.metadata.insert(
                                        "name".to_string(),
                                        serde_json::Value::String(ta.name.clone()),
                                    );
                                    entry.metadata.insert(
                                        "textasset_script_offset".to_string(),
                                        serde_json::json!(ta.script_len_offset),
                                    );
                                    entry.metadata.insert(
                                        "textasset_script_byte_len".to_string(),
                                        serde_json::json!(ta.script_byte_len),
                                    );
                                    entry.metadata.insert(
                                        "line_index".to_string(),
                                        serde_json::json!(line_index),
                                    );
                                    entry.metadata.insert(
                                        "line_count".to_string(),
                                        serde_json::json!(loc.line_count),
                                    );
                                    entry.metadata.insert(
                                        "newline".to_string(),
                                        serde_json::Value::String(newline.to_string()),
                                    );
                                    if let Some(k) = &loc.key {
                                        entry.metadata.insert(
                                            "loc_key".to_string(),
                                            serde_json::Value::String(k.clone()),
                                        );
                                        entry.metadata.insert(
                                            "loc_sep".to_string(),
                                            serde_json::Value::String(loc.sep.clone()),
                                        );
                                    }
                                    // Per-value budget for length-aware translate; inject
                                    // still pads the whole m_Script blob.
                                    entry.metadata.insert(
                                        "binary_slot".to_string(),
                                        serde_json::Value::String("utf8".to_string()),
                                    );
                                    entry.metadata.insert(
                                        "line_value_byte_len".to_string(),
                                        serde_json::json!(loc.value.len()),
                                    );
                                    entries.push(entry);
                                }
                                continue;
                            }
                            // Keep every structural instance (unique path_id / inject offset).
                            let id = format!("textasset/{}", ta.path_id);
                            let mut entry =
                                StringEntry::new(id, ta.script.clone(), file_path.to_path_buf());
                            entry.tags = vec!["textasset".to_string()];
                            entry.context = if ta.name.is_empty() {
                                None
                            } else {
                                Some(format!("m_Name={}", ta.name))
                            };
                            entry.metadata.insert(
                                "extraction_method".to_string(),
                                serde_json::Value::String("textasset".to_string()),
                            );
                            entry.metadata.insert(
                                "path_id".to_string(),
                                serde_json::json!(ta.path_id),
                            );
                            entry.metadata.insert(
                                "name".to_string(),
                                serde_json::Value::String(ta.name),
                            );
                            entry.metadata.insert(
                                "textasset_script_offset".to_string(),
                                serde_json::json!(ta.script_len_offset),
                            );
                            entry.metadata.insert(
                                "textasset_script_byte_len".to_string(),
                                serde_json::json!(ta.script_byte_len),
                            );
                            // Length budget for oversize skip + length-aware retry.
                            entry.metadata.insert(
                                "binary_slot".to_string(),
                                serde_json::Value::String("utf8".to_string()),
                            );
                            entries.push(entry);
                        }
                        Err(e) => {
                            tracing::warn!(
                                file = %filename,
                                path_id = obj.path_id,
                                error = %e,
                                "TextAsset read failed; skipped"
                            );
                        }
                    }
                }
                // Slice 2: MonoBehaviour m_Name + sequential aligned-string fields.
                for obj in sf.mono_behaviour_objects() {
                    match sf.read_mono_strings(obj.path_id) {
                        Ok(fields) => {
                            for field in fields {
                                if field.text.trim().is_empty() {
                                    continue;
                                }
                                // Do not dedupe by text — repeated UI labels need each slot.
                                let id = format!(
                                    "monobehaviour/{}/{}",
                                    field.path_id, field.field_index
                                );
                                let mut entry = StringEntry::new(
                                    id,
                                    field.text.clone(),
                                    file_path.to_path_buf(),
                                );
                                entry.tags = vec!["monobehaviour".to_string()];
                                entry.context = if field.mono_name.is_empty() {
                                    Some(format!("field={}", field.field_index))
                                } else {
                                    Some(format!(
                                        "m_Name={} field={}",
                                        field.mono_name, field.field_index
                                    ))
                                };
                                entry.metadata.insert(
                                    "extraction_method".to_string(),
                                    serde_json::Value::String("monobehaviour".to_string()),
                                );
                                entry.metadata.insert(
                                    "path_id".to_string(),
                                    serde_json::json!(field.path_id),
                                );
                                entry.metadata.insert(
                                    "field_index".to_string(),
                                    serde_json::json!(field.field_index),
                                );
                                entry.metadata.insert(
                                    "name".to_string(),
                                    serde_json::Value::String(field.mono_name),
                                );
                                entry.metadata.insert(
                                    "mono_string_offset".to_string(),
                                    serde_json::json!(field.len_offset),
                                );
                                entry.metadata.insert(
                                    "mono_string_byte_len".to_string(),
                                    serde_json::json!(field.byte_len),
                                );
                                entry.metadata.insert(
                                    "binary_slot".to_string(),
                                    serde_json::Value::String("utf8".to_string()),
                                );
                                entries.push(entry);
                            }
                        }
                        Err(e) => {
                            tracing::debug!(
                                file = %filename,
                                path_id = obj.path_id,
                                error = %e,
                                "MonoBehaviour read failed; skipped"
                            );
                        }
                    }
                }
                // Slice 2: TextMesh m_Text (legacy 3D text component, class 141).
                for obj in sf.text_mesh_objects() {
                    match sf.read_text_mesh(obj.path_id) {
                        Ok(tm) => {
                            if tm.text.trim().is_empty() {
                                continue;
                            }
                            if is_binary_looking_script(&tm.text) {
                                continue;
                            }
                            let id = format!("textmesh/{}", tm.path_id);
                            let mut entry =
                                StringEntry::new(id, tm.text.clone(), file_path.to_path_buf());
                            entry.tags = vec!["textmesh".to_string()];
                            entry.context = Some("m_Text".to_string());
                            entry.metadata.insert(
                                "extraction_method".to_string(),
                                serde_json::Value::String("textmesh".to_string()),
                            );
                            entry.metadata.insert(
                                "path_id".to_string(),
                                serde_json::json!(tm.path_id),
                            );
                            entry.metadata.insert(
                                "textmesh_text_offset".to_string(),
                                serde_json::json!(tm.text_len_offset),
                            );
                            entry.metadata.insert(
                                "textmesh_text_byte_len".to_string(),
                                serde_json::json!(tm.text_byte_len),
                            );
                            entry.metadata.insert(
                                "binary_slot".to_string(),
                                serde_json::Value::String("utf8".to_string()),
                            );
                            entries.push(entry);
                        }
                        Err(e) => {
                            tracing::debug!(
                                file = %filename,
                                path_id = obj.path_id,
                                error = %e,
                                "TextMesh read failed; skipped"
                            );
                        }
                    }
                }
                // Slice 2: GUIText m_Text (legacy screen text, class 132).
                for obj in sf.gui_text_objects() {
                    match sf.read_gui_text(obj.path_id) {
                        Ok(gt) => {
                            if gt.text.trim().is_empty() {
                                continue;
                            }
                            if is_binary_looking_script(&gt.text) {
                                continue;
                            }
                            let id = format!("guitext/{}", gt.path_id);
                            let mut entry =
                                StringEntry::new(id, gt.text.clone(), file_path.to_path_buf());
                            entry.tags = vec!["guitext".to_string()];
                            entry.context = Some("m_Text".to_string());
                            entry.metadata.insert(
                                "extraction_method".to_string(),
                                serde_json::Value::String("guitext".to_string()),
                            );
                            entry.metadata.insert(
                                "path_id".to_string(),
                                serde_json::json!(gt.path_id),
                            );
                            entry.metadata.insert(
                                "guitext_text_offset".to_string(),
                                serde_json::json!(gt.text_len_offset),
                            );
                            entry.metadata.insert(
                                "guitext_text_byte_len".to_string(),
                                serde_json::json!(gt.text_byte_len),
                            );
                            entry.metadata.insert(
                                "binary_slot".to_string(),
                                serde_json::Value::String("utf8".to_string()),
                            );
                            entries.push(entry);
                        }
                        Err(e) => {
                            tracing::debug!(
                                file = %filename,
                                path_id = obj.path_id,
                                error = %e,
                                "GUIText read failed; skipped"
                            );
                        }
                    }
                }
            }
            Err(e) => {
                tracing::warn!(
                    file = %filename,
                    error = %e,
                    "SerializedFile parse failed; using pure heuristic"
                );
            }
        }

        // Heuristic length-prefixed UTF-8 scan, skipping structural object ranges.
        // Prefer little-endian length (PC Unity default); fall back to big-endian so
        // BE blobs still yield injectible strings. BE candidates must not look like
        // the classic off-by-3 shadow of a following LE length field (see
        // `heuristic_string_at`).
        let len = bytes.len();
        if len < 8 {
            return entries;
        }
        let mut i = 0;
        while i + 4 < len {
            if range_contains(&skip_ranges, i) {
                i += 1;
                continue;
            }
            let Some((str_len, endian)) = heuristic_string_at(bytes, i) else {
                i += 1;
                continue;
            };
            if range_overlaps(&skip_ranges, i, i + 4 + str_len) {
                i += 1;
                continue;
            }
            if let Ok(text) = std::str::from_utf8(&bytes[i + 4..i + 4 + str_len]) {
                // Keep every offset occurrence (do not de-dupe by text). Repeated
                // UI labels in binary blobs each need their own inject needle.
                if is_unity_translatable(text) {
                    let id = format!("{}#offset_{}#{}", filename, i, entries.len());
                    let mut entry = StringEntry::new(id, text, file_path.to_path_buf());
                    entry.tags = vec!["unknown".to_string()];
                    entry.metadata.insert(
                        "binary_slot".to_string(),
                        serde_json::Value::String("utf8".to_string()),
                    );
                    entry.metadata.insert(
                        "extraction_method".to_string(),
                        serde_json::Value::String("heuristic".to_string()),
                    );
                    entry.metadata.insert(
                        "length_endian".to_string(),
                        serde_json::Value::String(endian.as_meta().to_string()),
                    );
                    entries.push(entry);
                }
            }
            let aligned = (str_len + 3) & !3;
            i += 4 + aligned;
        }
        entries
    }
}

/// Endianness of a Unity length-prefixed UTF-8 string field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LengthEndian {
    Little,
    Big,
}

impl LengthEndian {
    fn as_meta(self) -> &'static str {
        match self {
            LengthEndian::Little => "le",
            LengthEndian::Big => "be",
        }
    }

    fn from_meta(s: &str) -> Option<Self> {
        match s {
            "le" | "little" => Some(LengthEndian::Little),
            "be" | "big" => Some(LengthEndian::Big),
            _ => None,
        }
    }

    fn encode_u32(self, v: u32) -> [u8; 4] {
        match self {
            LengthEndian::Little => v.to_le_bytes(),
            LengthEndian::Big => v.to_be_bytes(),
        }
    }
}

/// Probe `bytes[i..]` for a plausible length-prefixed UTF-8 string.
/// Prefers little-endian (PC Unity). BE is accepted only when LE is out of range
/// **and** the payload does not start with NUL — rejecting the off-by-3 shadow of
/// a following LE length field (`00 00 00 NN` BE=N overlapping pad + LE length).
fn heuristic_string_at(bytes: &[u8], i: usize) -> Option<(usize, LengthEndian)> {
    if i + 4 > bytes.len() {
        return None;
    }
    let le = u32::from_le_bytes([bytes[i], bytes[i + 1], bytes[i + 2], bytes[i + 3]]) as usize;
    let be = u32::from_be_bytes([bytes[i], bytes[i + 1], bytes[i + 2], bytes[i + 3]]) as usize;
    let le_ok = (5..=2000).contains(&le) && i + 4 + le <= bytes.len();
    if le_ok {
        return Some((le, LengthEndian::Little));
    }
    let be_ok = (5..=2000).contains(&be) && i + 4 + be <= bytes.len();
    if be_ok {
        // Reject payload that starts with NUL — false-positive BE shadows of LE
        // lengths always pull leading zeros into the "string".
        if bytes[i + 4] == 0 {
            return None;
        }
        return Some((be, LengthEndian::Big));
    }
    None
}

fn is_unity_serialized_candidate(path: &Path) -> bool {
    if path.extension().is_some_and(|e| e == "assets") {
        return true;
    }
    // Built-player SerializedFiles often have **no extension**:
    // - level0, level1, … (scenes)
    // - globalgamemanagers (player settings / managers; Unity 5+)
    // - resources (legacy / some builds)
    // Skip .resS / .resource / .dll companions.
    path.file_name()
        .and_then(|n| n.to_str())
        .map(|n| {
            let lower = n.to_ascii_lowercase();
            if lower.contains('.') {
                return false;
            }
            lower.starts_with("level")
                || lower == "globalgamemanagers"
                || lower == "resources"
        })
        .unwrap_or(false)
}

/// First occurrence of `needle` in `haystack`, or `None`.
fn find_bytes_once(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    haystack
        .windows(needle.len())
        .position(|w| w == needle)
}

fn range_contains(ranges: &[(usize, usize)], pos: usize) -> bool {
    ranges.iter().any(|&(s, e)| pos >= s && pos < e)
}

fn range_overlaps(ranges: &[(usize, usize)], start: usize, end: usize) -> bool {
    ranges.iter().any(|&(s, e)| start < e && end > s)
}

fn is_textasset_entry(entry: &StringEntry) -> bool {
    matches!(
        entry
            .metadata
            .get("extraction_method")
            .and_then(|v| v.as_str()),
        Some("textasset") | Some("textasset_loc_line") | Some("textasset_csv_cell")
    )
}

fn is_textasset_loc_line_entry(entry: &StringEntry) -> bool {
    entry
        .metadata
        .get("extraction_method")
        .and_then(|v| v.as_str())
        == Some("textasset_loc_line")
}

fn is_textasset_csv_cell_entry(entry: &StringEntry) -> bool {
    entry
        .metadata
        .get("extraction_method")
        .and_then(|v| v.as_str())
        == Some("textasset_csv_cell")
}

/// One non-empty line from a ManagedText-style localization document.
#[derive(Debug, Clone)]
struct TextAssetLocLine {
    /// Localization key when the line is `Key: Value` / `Key=Value`; else `None`.
    key: Option<String>,
    /// Separator between key and value (`": "` / `":"` / `"="`), empty if whole line.
    sep: String,
    /// Translatable text (value or full line).
    value: String,
    /// Total non-empty line count in the document (for inject sanity).
    line_count: usize,
}

/// Detect Naninovel ManagedText / locale docs:
/// - multi-line: ≥2 non-empty lines and ≥70% `Key: Value` / `Key=Value`
/// - single-line: one `Key: Value` with a dotted/identifier key (e.g. TitleMenu.START)
fn parse_textasset_loc_lines(script: &str) -> Option<Vec<TextAssetLocLine>> {
    let raw_lines: Vec<&str> = script.lines().collect();
    let non_empty: Vec<&str> = raw_lines
        .iter()
        .copied()
        .filter(|l| !l.trim().is_empty())
        .collect();
    if non_empty.is_empty() {
        return None;
    }
    // Single ManagedText asset that is just one key/value pair.
    if non_empty.len() == 1 {
        let line = non_empty[0];
        let (key, sep, value) = split_loc_kv(line)?;
        if value.trim().is_empty() || !looks_like_loc_key(key) {
            return None;
        }
        return Some(vec![TextAssetLocLine {
            key: Some(key.to_string()),
            sep: sep.to_string(),
            value: value.to_string(),
            line_count: 1,
        }]);
    }
    let kv_hits = non_empty.iter().filter(|l| split_loc_kv(l).is_some()).count();
    if kv_hits * 100 / non_empty.len() < 70 {
        return None;
    }
    let line_count = non_empty.len();
    let mut out = Vec::with_capacity(line_count);
    for line in non_empty {
        if let Some((key, sep, value)) = split_loc_kv(line) {
            // Skip empty values and pure key-only noise.
            if value.trim().is_empty() {
                continue;
            }
            // Locale-id keys belong in culture catalogs, not translate queues.
            if looks_like_bcp47_locale_id(key) {
                continue;
            }
            out.push(TextAssetLocLine {
                key: Some(key.to_string()),
                sep: sep.to_string(),
                value: value.to_string(),
                line_count,
            });
        } else {
            // Rare non-kv line in a loc doc — keep whole line for round-trip.
            out.push(TextAssetLocLine {
                key: None,
                sep: String::new(),
                value: line.to_string(),
                line_count,
            });
        }
    }
    if out.is_empty() {
        return None;
    }
    Some(out)
}

/// ManagedText-style keys: `TitleMenu.START`, `Confirmation.Yes`, `SaveGame`.
fn looks_like_loc_key(key: &str) -> bool {
    let key = key.trim();
    if key.is_empty() || key.len() > 80 {
        return false;
    }
    // Prefer dotted / snake keys; bare alnum ≥2 (not BCP-47 locale ids alone —
    // those are filtered at extract time via `looks_like_bcp47_locale_id`).
    key.contains('.')
        || key.contains('_')
        || key.contains('-')
        || (key.chars().all(|c| c.is_ascii_alphanumeric()) && key.len() >= 2)
}

/// BCP-47-ish culture id used as ManagedText key in locale name tables:
/// `af`, `af-ZA`, `zh-Hans`, `es-419`.
fn looks_like_bcp47_locale_id(key: &str) -> bool {
    let key = key.trim();
    if key.is_empty() || key.len() > 16 || key.contains('.') || key.contains('_') {
        return false;
    }
    let parts: Vec<&str> = key.split('-').collect();
    if parts.is_empty() || parts.len() > 3 {
        return false;
    }
    let lang = parts[0];
    if !(2..=3).contains(&lang.len()) || !lang.chars().all(|c| c.is_ascii_alphabetic()) {
        return false;
    }
    for p in &parts[1..] {
        let ok = match p.len() {
            // Region: US, ZA
            2 => p.chars().all(|c| c.is_ascii_alphabetic()),
            // UN M.49 region: 419
            3 => p.chars().all(|c| c.is_ascii_digit()),
            // Script: Hans, Hant, Latn
            4 => p.chars().all(|c| c.is_ascii_alphabetic()),
            _ => false,
        };
        if !ok {
            return false;
        }
    }
    true
}

/// Entire ManagedText blob is a culture→display-name catalog (Naninovel `Locales`).
fn is_locale_catalog_script(script: &str) -> bool {
    let mut keys = 0usize;
    let mut locale_keys = 0usize;
    for line in script.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Some((key, _, value)) = split_loc_kv(line) else {
            continue;
        };
        if value.trim().is_empty() {
            continue;
        }
        keys += 1;
        if looks_like_bcp47_locale_id(key) {
            locale_keys += 1;
        }
    }
    // Need a real catalog (≥8 cultures) that is overwhelmingly locale-keyed.
    keys >= 8 && locale_keys * 100 / keys >= 80
}

/// `Key: Value` (prefer `: `), bare `:`, or `Key=Value` with no spaces in key.
fn split_loc_kv(line: &str) -> Option<(&str, &str, &str)> {
    let line = line.trim_end_matches('\r');
    if let Some((k, v)) = line.split_once(": ") {
        let k = k.trim();
        if !k.is_empty() && !k.contains('\t') {
            return Some((k, ": ", v));
        }
    }
    if let Some((k, v)) = line.split_once('=') {
        let k = k.trim();
        if !k.is_empty() && !k.contains(' ') && !k.contains('\t') {
            return Some((k, "=", v));
        }
    }
    if let Some((k, v)) = line.split_once(':') {
        let k = k.trim();
        // Avoid matching times / ratios; require key-like token (alnum / . / _).
        if !k.is_empty()
            && !k.contains(' ')
            && k.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-')
        {
            return Some((k, ":", v.trim_start()));
        }
    }
    None
}

/// Simple unquoted CSV table for TextAsset split (BOXMAN item lists, etc.).
#[derive(Debug)]
struct TextAssetCsvCell {
    /// 1-based data row index (header is row 0, not extracted).
    row: usize,
    col: usize,
    header: String,
    value: String,
}

#[derive(Debug)]
struct TextAssetCsv {
    cells: Vec<TextAssetCsvCell>,
}

fn is_csv_header_token(h: &str) -> bool {
    let h = h.trim();
    !h.is_empty()
        && h.len() <= 64
        && h.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

fn is_pure_int_token(s: &str) -> bool {
    let s = s.trim();
    !s.is_empty() && s.chars().all(|c| c.is_ascii_digit() || c == '-' || c == '+')
}

/// Detect simple CSV: header of identifier columns, ≥2 data rows, no quotes,
/// consistent column counts. Emits cells from non-numeric columns only.
fn parse_textasset_csv(script: &str) -> Option<TextAssetCsv> {
    let t = script.trim();
    if t.is_empty() || t.contains('"') {
        return None;
    }
    // Prefer ManagedText over CSV when both could match (unlikely).
    if parse_textasset_loc_lines(script).is_some() {
        return None;
    }
    let lines: Vec<&str> = t
        .lines()
        .map(|l| l.trim_end_matches('\r'))
        .filter(|l| !l.trim().is_empty())
        .collect();
    if lines.len() < 3 {
        return None;
    }
    // Header must contain a comma.
    if !lines[0].contains(',') {
        return None;
    }
    let headers: Vec<&str> = lines[0].split(',').map(|s| s.trim()).collect();
    if headers.len() < 2 || !headers.iter().all(|h| is_csv_header_token(h)) {
        return None;
    }
    let ncols = headers.len();
    let mut rows: Vec<Vec<String>> = Vec::with_capacity(lines.len() - 1);
    for line in &lines[1..] {
        // Require at least one comma on data rows too.
        if !line.contains(',') {
            return None;
        }
        let cells: Vec<String> = line.split(',').map(|s| s.trim().to_string()).collect();
        if cells.len() != ncols {
            return None;
        }
        rows.push(cells);
    }
    if rows.len() < 2 {
        return None;
    }
    // Text columns: not ≥80% pure integers, and at least one alphabetic cell.
    let mut text_cols: Vec<usize> = Vec::new();
    for c in 0..ncols {
        let numeric = rows.iter().filter(|r| is_pure_int_token(&r[c])).count();
        if numeric * 100 / rows.len() >= 80 {
            continue;
        }
        if rows
            .iter()
            .any(|r| r[c].chars().any(|ch| ch.is_alphabetic()))
        {
            text_cols.push(c);
        }
    }
    if text_cols.is_empty() {
        return None;
    }
    let mut cells = Vec::new();
    for (ri, row) in rows.iter().enumerate() {
        for &c in &text_cols {
            let value = row[c].clone();
            if value.is_empty() {
                continue;
            }
            cells.push(TextAssetCsvCell {
                row: ri + 1, // 1-based data row
                col: c,
                header: headers[c].to_string(),
                value,
            });
        }
    }
    if cells.len() < 2 {
        return None;
    }
    Some(TextAssetCsv { cells })
}

/// Apply CSV cell translations onto the original script (re-parse + rewrite).
fn apply_csv_translations_to_script(
    original: &str,
    cells: &[&StringEntry],
) -> Option<String> {
    let newline = if original.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    };
    let lines: Vec<&str> = original
        .lines()
        .map(|l| l.trim_end_matches('\r'))
        .filter(|l| !l.trim().is_empty())
        .collect();
    if lines.is_empty() {
        return None;
    }
    let headers: Vec<&str> = lines[0].split(',').map(|s| s.trim()).collect();
    let ncols = headers.len();
    let mut rows: Vec<Vec<String>> = Vec::new();
    for line in &lines[1..] {
        let row: Vec<String> = line.split(',').map(|s| s.trim().to_string()).collect();
        if row.len() != ncols {
            return None;
        }
        rows.push(row);
    }
    for e in cells {
        let Some(row_i) = e.metadata.get("csv_row").and_then(|v| v.as_u64()) else {
            continue;
        };
        let Some(col) = e.metadata.get("csv_col").and_then(|v| v.as_u64()) else {
            continue;
        };
        let row_i = row_i as usize;
        let col = col as usize;
        // csv_row is 1-based data index.
        if row_i == 0 || row_i > rows.len() || col >= ncols {
            continue;
        }
        let text = e.translation.as_deref().unwrap_or(e.source.as_str());
        // Reject commas / newlines in translation (would break simple CSV).
        if text.contains(',') || text.contains('\n') || text.contains('\r') {
            continue;
        }
        rows[row_i - 1][col] = text.to_string();
    }
    let mut out = headers.join(",");
    for row in rows {
        out.push_str(newline);
        out.push_str(&row.join(","));
    }
    // Preserve a trailing newline if the original had one after the last row.
    if original.ends_with("\r\n") || original.ends_with('\n') {
        out.push_str(newline);
    }
    Some(out)
}

/// Rebuild a loc document from line entries (sorted by `line_index`).
fn rebuild_textasset_loc_script(lines: &[&StringEntry]) -> Option<String> {
    if lines.is_empty() {
        return None;
    }
    let mut ordered: Vec<&StringEntry> = lines.to_vec();
    ordered.sort_by_key(|e| {
        e.metadata
            .get("line_index")
            .and_then(|v| v.as_u64())
            .unwrap_or(0)
    });
    let newline = ordered[0]
        .metadata
        .get("newline")
        .and_then(|v| v.as_str())
        .unwrap_or("\n");
    let mut parts: Vec<String> = Vec::with_capacity(ordered.len());
    for e in ordered {
        let text = e.translation.as_deref().unwrap_or(e.source.as_str());
        let line = match (
            e.metadata.get("loc_key").and_then(|v| v.as_str()),
            e.metadata.get("loc_sep").and_then(|v| v.as_str()),
        ) {
            (Some(k), Some(sep)) => format!("{k}{sep}{text}"),
            _ => text.to_string(),
        };
        parts.push(line);
    }
    Some(parts.join(newline))
}

fn is_mono_entry(entry: &StringEntry) -> bool {
    entry
        .metadata
        .get("extraction_method")
        .and_then(|v| v.as_str())
        == Some("monobehaviour")
}

fn is_textmesh_entry(entry: &StringEntry) -> bool {
    entry
        .metadata
        .get("extraction_method")
        .and_then(|v| v.as_str())
        == Some("textmesh")
}

fn is_guitext_entry(entry: &StringEntry) -> bool {
    entry
        .metadata
        .get("extraction_method")
        .and_then(|v| v.as_str())
        == Some("guitext")
}

fn is_structural_entry(entry: &StringEntry) -> bool {
    is_textasset_entry(entry)
        || is_mono_entry(entry)
        || is_textmesh_entry(entry)
        || is_guitext_entry(entry)
}

/// Extract a quoted string from a line like `button 0 "Label" +link jump 5`
fn extract_quoted_in_line(line: &str) -> Option<&str> {
    let start = line.find('"')? + 1;
    let rest = &line[start..];
    let end = rest.find('"')?;
    let text = &rest[..end];
    if text.is_empty() { None } else { Some(text) }
}

/// Extract VN dialogue: `CharID Dialogue text here`
/// Character IDs are 1-5 char identifiers (letters, sometimes digits)
/// Returns (char_id, clean_text) where clean_text has format codes stripped
fn extract_vn_dialogue(line: &str) -> Option<(&str, &str)> {
    let trimmed = line.trim();
    if trimmed.is_empty() { return None; }

    let space_pos = trimmed.find(' ')?;
    let char_id = &trimmed[..space_pos];
    let text = trimmed[space_pos + 1..].trim();

    if char_id.is_empty() || char_id.len() > 8 { return None; }
    if !char_id.chars().next()?.is_ascii_uppercase() { return None; }
    if !char_id.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') { return None; }
    if text.is_empty() || text.starts_with('{') || text.starts_with('+') { return None; }

    Some((char_id, text))
}

/// Strip VN format codes from text for translation.
/// Codes like \i, \b, \- are engine formatting and should not be translated.
fn strip_vn_format_codes(text: &str) -> String {
    text.replace("\\i", "")
        .replace("\\b", "")
        .replace("\\-", "")
        .replace("\\p", "")
        .replace("\\n", " ")
        .trim()
        .to_string()
}

/// Find format code prefix/suffixes in the original text so we can restore them.
/// Returns (prefix_codes, inner_text, suffix_codes)
fn split_format_codes(text: &str) -> (String, String, String) {
    let mut prefix = String::new();
    let mut suffix = String::new();
    let inner = text.to_string();

    // Extract leading format codes
    let mut chars = inner.chars().peekable();
    let mut prefix_end = 0;
    while let Some(&ch) = chars.peek() {
        if ch == '\\' {
            chars.next();
            if let Some(&next) = chars.peek() {
                prefix.push('\\');
                prefix.push(next);
                chars.next();
                prefix_end += 2;
                // Skip any following whitespace
                while let Some(&ws) = chars.peek() {
                    if ws == ' ' { chars.next(); prefix_end += 1; }
                    else { break; }
                }
            }
        } else {
            break;
        }
    }

    let remaining = &inner[prefix_end..];

    // Check for trailing format codes
    let trimmed_end = remaining.trim_end();
    if trimmed_end.ends_with("\\i") || trimmed_end.ends_with("\\b") {
        let code_start = trimmed_end.len() - 2;
        suffix = remaining[code_start..].to_string();
        return (prefix, remaining[..code_start].trim().to_string(), suffix);
    }

    (prefix, remaining.to_string(), suffix)
}

fn is_unity_translatable(text: &str) -> bool {
    let s = text.trim();
    if s.is_empty() || s.len() < 5 {
        return false;
    }
    // Binary soup / mis-framed length prefixes leave C0 controls in the payload.
    // Allow tab/CR/LF for multi-line dialogue; reject other controls.
    if s.chars().any(|c| {
        let u = c as u32;
        u < 32 && c != '\t' && c != '\n' && c != '\r'
    }) {
        return false;
    }
    // Managed type refs flood heuristic scans of ScriptableObject blobs.
    if looks_like_assembly_qualified_type(s) {
        return false;
    }
    if looks_like_naninovel_script(s) || looks_like_lorem_ipsum(s) {
        return false;
    }
    let total = s.chars().count();
    let ascii_printable = s
        .chars()
        .filter(|c| c.is_ascii_graphic() || c.is_ascii_whitespace())
        .count();
    if (ascii_printable as f64 / total as f64) < 0.85 {
        return false;
    }
    let letters = s.chars().filter(|c| c.is_alphabetic()).count();
    if letters < 3 {
        return false;
    }
    let has_space = s.contains(' ');
    if !has_space && s.len() > 20 {
        return false;
    }
    if s.contains('/') && s.contains('.') && !s.contains(' ') {
        return false;
    }
    // Shader / material / built-in path crumbs (BOXMAN globalgamemanagers heuristic).
    if looks_like_unity_shader_or_engine_path(s) {
        return false;
    }
    // Hierarchy debug labels: `Mesh Renderer (Id :1)`.
    if looks_like_unity_renderer_id_label(s) {
        return false;
    }
    // Animator layer default name + `Base Layer.STATE` paths (BOXMAN).
    if s == "Base Layer" || s.starts_with("Base Layer.") {
        return false;
    }
    // TMP / custom font material object names.
    if s.ends_with(" Atlas Material") || s.ends_with(" Atlas") {
        return false;
    }
    // Built-in Light2D default object name (no hierarchy clone suffix).
    if s == "Light 2D" {
        return false;
    }
    // uGUI hierarchy defaults (ScrollRect / Mask / Scrollbar).
    if matches!(s, "Sliding Area" | "Viewport" | "Thumbnail") {
        return false;
    }
    // Pure all-lowercase ascii token (code/mode ids). Title Case UI stays.
    if looks_like_all_lowercase_code_token(s) {
        return false;
    }
    // Animation / timeline clips: `Worker_Deliver Order_03`
    if looks_like_animation_clip_name(s) {
        return false;
    }
    // Path crumbs with a pure-digit segment: `night/2 centered`
    if looks_like_slash_digit_path(s) {
        return false;
    }
    // Hierarchy / prefab names ending in asset type: `Pillar Sprite`
    if looks_like_asset_type_suffix_name(s) {
        return false;
    }
    // Editor selection suffixes on hierarchy names: `btn Night (Selected)`.
    if s.ends_with(" (Selected)")
        || s.ends_with(" (Highlighted)")
        || s.ends_with(" (Disabled)")
        || s.ends_with(" (Pressed)")
    {
        return false;
    }
    // Asset/addressable-ish path crumbs with spaces: `naninovel/audio/bgm/…`
    if s.starts_with("naninovel/") || s.contains("/audio/") || s.contains("/bgm/") {
        return false;
    }
    // Shader #define soup: `BLENDMODES_MODE_MULTIPLY ETC1_EXTERNAL_ALPHA`
    if s.contains("BLENDMODES_") || s.contains("ETC1_EXTERNAL_ALPHA") {
        return false;
    }
    if s.contains('\\') && s.contains('.') {
        return false;
    }
    if s
        .chars()
        .all(|c| c.is_ascii_uppercase() || c == '_' || c.is_ascii_digit())
        && s.chars().any(|c| c.is_ascii_uppercase())
    {
        return false;
    }
    // MonoScript / type-name noise: PascalCase, camelCase, Name2, snake_Case ids.
    if !has_space && looks_like_code_identifier(s) {
        return false;
    }
    // Underscore tokens without spaces (Command_POSGenerator, Sprite_Idle).
    if !has_space && s.contains('_') {
        return false;
    }
    if !has_space {
        let transitions = s
            .as_bytes()
            .windows(2)
            .filter(|w| w[0].is_ascii_lowercase() && w[1].is_ascii_uppercase())
            .count();
        if transitions >= 1 {
            return false;
        }
    }
    // Unity hierarchy clone names: "Light 2D (7)", "SpeechBubbleIcon (4)".
    if looks_like_unity_instance_name(s) {
        return false;
    }
    if s.starts_with("http") || s.starts_with("www.") {
        return false;
    }
    if s.contains("::") || (s.contains('.') && !s.contains(' ')) {
        return false;
    }
    if s.contains("(){")
        || s.contains("};")
        || s.starts_with("using ")
        || s.starts_with("import ")
        || s.starts_with("public ")
        || s.starts_with("private ")
    {
        return false;
    }
    let punct_ratio = s
        .chars()
        .filter(|c| !c.is_alphanumeric() && !c.is_whitespace())
        .count() as f64
        / total as f64;
    if punct_ratio > 0.4 {
        return false;
    }
    true
}

/// Unity editor hierarchy instance names end with `" (N)"` (clone index).
fn looks_like_unity_instance_name(s: &str) -> bool {
    let s = s.trim();
    if !s.ends_with(')') {
        return false;
    }
    let Some(open) = s.rfind(" (") else {
        return false;
    };
    let inner = &s[open + 2..s.len() - 1];
    !inner.is_empty() && inner.chars().all(|c| c.is_ascii_digit())
}

/// Built-in shader / material family paths that flood heuristic scans of
/// `globalgamemanagers` (not player-facing copy).
fn looks_like_unity_shader_or_engine_path(s: &str) -> bool {
    let s = s.trim();
    if s.contains("Shaders/")
        || s.starts_with("Hidden/")
        || s.starts_with("Legacy Shaders/")
        || s.starts_with("UI/")
        || s.starts_with("Skybox/")
    {
        return true;
    }
    // Common built-in families (single slash is enough): `Mobile/Diffuse`, `FX/Flare`.
    const PREFIXES: &[&str] = &[
        "Mobile/",
        "Nature/",
        "FX/",
        "Particles/",
        "Sprites/",
        "Unlit/",
        "GUI/",
        "VR/",
        "AR/",
        "TextMeshPro/",
        "Universal Render Pipeline/",
        "Autodesk/",
        "Standard/",
        "Legacy/",
    ];
    if PREFIXES.iter().any(|p| s.starts_with(p)) {
        return true;
    }
    // Multi-segment slash paths without sentence whitespace (shader-style).
    if !s.contains(' ') && s.matches('/').count() >= 2 {
        return true;
    }
    false
}

/// `Mesh Renderer (Id :1)` / `Collider (Id: 3)` editor debug labels.
fn looks_like_unity_renderer_id_label(s: &str) -> bool {
    let s = s.trim();
    if !(s.contains("(Id :") || s.contains("(Id:") || s.contains("(Id : ") || s.contains("(id :")) {
        // Case variants
        let lower = s.to_ascii_lowercase();
        if !(lower.contains("(id :") || lower.contains("(id:")) {
            return false;
        }
    }
    s.ends_with(')')
}

/// Single-token all-lowercase ascii identifier (`bezierpoint`, `workmode`).
/// Player-facing English UI is almost always Title Case / sentence case.
fn looks_like_all_lowercase_code_token(s: &str) -> bool {
    let s = s.trim();
    if s.len() < 5 || s.contains(' ') {
        return false;
    }
    s.chars().all(|c| c.is_ascii_lowercase())
}

/// Animator / timeline clip names that end with `_NN` (optionally with spaces).
/// e.g. `Worker_Deliver Order_03`, `Idle_Walk_12`.
fn looks_like_animation_clip_name(s: &str) -> bool {
    let s = s.trim();
    if !s.contains('_') {
        return false;
    }
    // Last underscore segment is pure digits (length ≥ 2 preferred for scene indices).
    let Some(last) = s.rsplit('_').next() else {
        return false;
    };
    last.len() >= 2 && last.chars().all(|c| c.is_ascii_digit())
}

/// Slash path with a pure-digit segment: `night/2 centered`, `maps/03/intro`.
fn looks_like_slash_digit_path(s: &str) -> bool {
    let s = s.trim();
    if !s.contains('/') {
        return false;
    }
    s.split(|c: char| c == '/' || c.is_whitespace())
        .any(|seg| !seg.is_empty() && seg.chars().all(|c| c.is_ascii_digit()))
}

/// Prefab / hierarchy names that are just `"… Sprite"` / `"… Mesh"` / `"… Collider"`.
fn looks_like_asset_type_suffix_name(s: &str) -> bool {
    let s = s.trim();
    const SUFFIXES: &[&str] = &[
        " Sprite",
        " Mesh",
        " Collider",
        " Renderer",
        " Material",
        " Texture",
        " Prefab",
    ];
    SUFFIXES.iter().any(|suf| {
        s.len() > suf.len()
            && s.ends_with(suf)
            // Require a simple left token (letters/digits/hyphen/underscore), not a sentence.
            && !s[..s.len() - suf.len()].contains(' ')
    })
}

impl Default for UnityPlugin {
    fn default() -> Self {
        Self::new()
    }
}

impl FormatPlugin for UnityPlugin {
    fn id(&self) -> &str {
        "unity"
    }

    fn name(&self) -> &str {
        "Unity Engine"
    }

    fn description(&self) -> &str {
        "Unity Engine (VN scripts + TextAsset/MonoBehaviour/TextMesh/GUIText structural + SerializedFile heuristic)"
    }

    fn stability(&self) -> locust_core::extraction::FormatStability {
        // Phase-2 apply proven (BOXMAN mock E2E); binary length constraints remain.
        locust_core::extraction::FormatStability::Experimental
    }

    fn supported_extensions(&self) -> &[&str] {
        &[".assets"]
    }

    fn supported_modes(&self) -> Vec<OutputMode> {
        vec![OutputMode::Replace]
    }

    fn detect(&self, path: &Path) -> bool {
        if path.is_file() {
            return path.extension().is_some_and(|e| e == "assets");
        }
        Self::has_unity_structure(path)
    }

    fn extract(&self, path: &Path) -> Result<Vec<StringEntry>> {
        // Prefer text scripts if available
        if let Some(scripts_dir) = Self::find_scripts_dir(path) {
            let entries = Self::extract_text_scripts(&scripts_dir)?;
            if !entries.is_empty() {
                return Ok(entries);
            }
        }

        // Fallback to binary .assets extraction
        let assets = Self::find_assets_files(path);
        if assets.is_empty() {
            return Err(LocustError::ParseError {
                file: path.display().to_string(),
                message: "no script files or .assets files found".to_string(),
            });
        }

        let mut all = Vec::new();
        for asset_file in &assets {
            let bytes = std::fs::read(asset_file)?;
            let filename = asset_file
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            all.extend(Self::extract_strings_from_assets(&bytes, &filename, asset_file));
        }
        Ok(all)
    }

    fn inject(&self, path: &Path, entries: &[StringEntry]) -> Result<InjectionReport> {
        // Check if entries come from text scripts (file_path ends in .txt)
        let from_text = entries.iter().any(|e| {
            e.file_path.extension().is_some_and(|ext| ext == "txt")
        });

        if from_text {
            return Self::inject_text_scripts(path, entries);
        }

        // Binary .assets injection
        let mut files_modified = 0;
        let mut strings_written = 0;
        let mut strings_skipped = 0;
        let mut length_skipped = 0usize;
        let mut warnings = Vec::new();
        let mut files_written: Vec<PathBuf> = Vec::new();

        let mut by_file: HashMap<PathBuf, Vec<&StringEntry>> = HashMap::new();
        for entry in entries {
            by_file.entry(entry.file_path.clone()).or_default().push(entry);
        }

        for (file_path, file_entries) in &by_file {
            if !file_path.exists() {
                continue;
            }
            let mut bytes = std::fs::read(file_path)?;
            let mut modified = false;
            let label = file_path.display().to_string();

            // ── TextAsset CSV cell groups (re-parse original blob, apply cells) ──
            {
                let mut csv_groups: HashMap<i64, Vec<&StringEntry>> = HashMap::new();
                for entry in file_entries.iter().filter(|e| is_textasset_csv_cell_entry(e)) {
                    let Some(path_id) = entry
                        .metadata
                        .get("path_id")
                        .and_then(|v| v.as_i64())
                    else {
                        warnings.push(format!(
                            "TextAsset csv cell '{}' missing path_id",
                            entry.id
                        ));
                        strings_skipped += 1;
                        continue;
                    };
                    csv_groups.entry(path_id).or_default().push(*entry);
                }
                for (_path_id, group) in csv_groups {
                    let Some(head) = group.first() else { continue };
                    let Some(script_off) = head
                        .metadata
                        .get("textasset_script_offset")
                        .and_then(|v| v.as_u64())
                        .map(|u| u as usize)
                    else {
                        strings_skipped += group.len();
                        continue;
                    };
                    let orig_len = head
                        .metadata
                        .get("textasset_script_byte_len")
                        .and_then(|v| v.as_u64())
                        .map(|u| u as usize)
                        .unwrap_or(0);
                    if orig_len == 0 || script_off + 4 + orig_len > bytes.len() {
                        strings_skipped += group.len();
                        continue;
                    }
                    // Payload starts after the u32 length prefix.
                    let payload =
                        &bytes[script_off + 4..script_off + 4 + orig_len];
                    let Ok(original) = std::str::from_utf8(payload) else {
                        strings_skipped += group.len();
                        continue;
                    };
                    let Some(mut rebuilt) =
                        apply_csv_translations_to_script(original, &group)
                    else {
                        strings_skipped += group.len();
                        continue;
                    };
                    if rebuilt.len() > orig_len {
                        if length_skipped < 5 {
                            warnings.push(format!(
                                "TextAsset CSV rebuild longer than slot ({} > {}) for '{}', skipping",
                                rebuilt.len(),
                                orig_len,
                                head.id
                            ));
                        }
                        length_skipped += 1;
                        strings_skipped += group.len();
                        continue;
                    }
                    while rebuilt.len() < orig_len {
                        rebuilt.push(' ');
                    }
                    match rewrite_text_asset_script_inplace(
                        &mut bytes,
                        script_off,
                        orig_len,
                        &rebuilt,
                        &label,
                    ) {
                        Ok(()) => {
                            let changed = group
                                .iter()
                                .filter(|e| {
                                    e.translation
                                        .as_ref()
                                        .map(|t| t != &e.source)
                                        .unwrap_or(false)
                                })
                                .count();
                            strings_written += changed.max(1);
                            strings_skipped += group.len().saturating_sub(changed.max(1));
                            modified = true;
                        }
                        Err(e) => {
                            warnings.push(format!("TextAsset CSV rewrite {}: {e}", head.id));
                            strings_skipped += group.len();
                        }
                    }
                }
            }

            // ── TextAsset loc-line groups (rebuild whole m_Script once per path_id) ──
            {
                let mut loc_groups: HashMap<i64, Vec<&StringEntry>> = HashMap::new();
                for entry in file_entries.iter().filter(|e| is_textasset_loc_line_entry(e)) {
                    let Some(path_id) = entry
                        .metadata
                        .get("path_id")
                        .and_then(|v| v.as_i64())
                    else {
                        warnings.push(format!(
                            "TextAsset loc line '{}' missing path_id",
                            entry.id
                        ));
                        strings_skipped += 1;
                        continue;
                    };
                    loc_groups.entry(path_id).or_default().push(*entry);
                }
                for (_path_id, group) in loc_groups {
                    let Some(head) = group.first() else { continue };
                    let Some(script_off) = head
                        .metadata
                        .get("textasset_script_offset")
                        .and_then(|v| v.as_u64())
                        .map(|u| u as usize)
                    else {
                        warnings.push(format!(
                            "TextAsset loc group missing textasset_script_offset ({})",
                            head.id
                        ));
                        strings_skipped += group.len();
                        continue;
                    };
                    let orig_len = head
                        .metadata
                        .get("textasset_script_byte_len")
                        .and_then(|v| v.as_u64())
                        .map(|u| u as usize)
                        .unwrap_or(0);
                    if orig_len == 0 {
                        strings_skipped += group.len();
                        continue;
                    }
                    let Some(mut rebuilt) = rebuild_textasset_loc_script(&group) else {
                        strings_skipped += group.len();
                        continue;
                    };
                    // Preserve original trailing newline style if present in budget.
                    if rebuilt.len() > orig_len {
                        if length_skipped < 5 {
                            warnings.push(format!(
                                "TextAsset loc rebuild longer than slot ({} > {} bytes) for '{}', skipping group",
                                rebuilt.len(),
                                orig_len,
                                head.id
                            ));
                        }
                        length_skipped += 1;
                        strings_skipped += group.len();
                        continue;
                    }
                    // Pad with spaces to keep m_Script byte length identical.
                    while rebuilt.len() < orig_len {
                        rebuilt.push(' ');
                    }
                    match rewrite_text_asset_script_inplace(
                        &mut bytes,
                        script_off,
                        orig_len,
                        &rebuilt,
                        &label,
                    ) {
                        Ok(()) => {
                            let changed = group
                                .iter()
                                .filter(|e| {
                                    e.translation
                                        .as_ref()
                                        .map(|t| t != &e.source)
                                        .unwrap_or(false)
                                })
                                .count();
                            strings_written += changed.max(1);
                            strings_skipped += group.len().saturating_sub(changed.max(1));
                            modified = true;
                        }
                        Err(e) => {
                            warnings.push(format!("TextAsset loc rewrite {}: {e}", head.id));
                            strings_skipped += group.len();
                        }
                    }
                }
            }

            // ── Structural TextAsset / MonoBehaviour / TextMesh / GUIText inject ──
            for entry in file_entries.iter().filter(|e| {
                is_structural_entry(e)
                    && !is_textasset_loc_line_entry(e)
                    && !is_textasset_csv_cell_entry(e)
            }) {
                let translation = match &entry.translation {
                    Some(t) => t,
                    None => {
                        strings_skipped += 1;
                        continue;
                    }
                };
                if translation == &entry.source {
                    strings_skipped += 1;
                    continue;
                }
                let (off_key, len_key, kind) = if is_textasset_entry(entry) {
                    (
                        "textasset_script_offset",
                        "textasset_script_byte_len",
                        "TextAsset",
                    )
                } else if is_textmesh_entry(entry) {
                    (
                        "textmesh_text_offset",
                        "textmesh_text_byte_len",
                        "TextMesh",
                    )
                } else if is_guitext_entry(entry) {
                    (
                        "guitext_text_offset",
                        "guitext_text_byte_len",
                        "GUIText",
                    )
                } else {
                    (
                        "mono_string_offset",
                        "mono_string_byte_len",
                        "MonoBehaviour",
                    )
                };
                let Some(script_off) = entry
                    .metadata
                    .get(off_key)
                    .and_then(|v| v.as_u64())
                    .map(|u| u as usize)
                else {
                    warnings.push(format!(
                        "{kind} entry '{}' missing {off_key}",
                        entry.id
                    ));
                    strings_skipped += 1;
                    continue;
                };
                let orig_len = entry
                    .metadata
                    .get(len_key)
                    .and_then(|v| v.as_u64())
                    .map(|u| u as usize)
                    .unwrap_or(entry.source.len());
                if translation.len() > orig_len {
                    if length_skipped < 5 {
                        warnings.push(format!(
                            "translation for '{}' longer than original {kind} string ({} > {} bytes), skipping",
                            entry.id,
                            translation.len(),
                            orig_len
                        ));
                    }
                    length_skipped += 1;
                    strings_skipped += 1;
                    continue;
                }
                match rewrite_text_asset_script_inplace(
                    &mut bytes,
                    script_off,
                    orig_len,
                    translation,
                    &label,
                ) {
                    Ok(()) => {
                        strings_written += 1;
                        modified = true;
                    }
                    Err(e) => {
                        warnings.push(format!("{kind} rewrite {}: {e}", entry.id));
                        strings_skipped += 1;
                    }
                }
            }

            // ── Heuristic length-prefixed inject (non-structural entries) ──
            struct Work {
                needle: Vec<u8>,
                /// Alternate endian needle when metadata did not pin endianness.
                alt_needle: Option<Vec<u8>>,
                endian: LengthEndian,
                alt_endian: Option<LengthEndian>,
                trans_bytes: Vec<u8>,
                orig_payload_len: usize,
            }
            let mut work: Vec<Work> = Vec::new();
            for entry in file_entries.iter().filter(|e| !is_structural_entry(e)) {
                let translation = match &entry.translation {
                    Some(t) => t,
                    None => {
                        strings_skipped += 1;
                        continue;
                    }
                };
                let orig_bytes = entry.source.as_bytes();
                let trans_bytes = translation.as_bytes();
                if trans_bytes == orig_bytes {
                    strings_skipped += 1;
                    continue;
                }
                if trans_bytes.len() > orig_bytes.len() {
                    if length_skipped < 5 {
                        warnings.push(format!(
                            "translation for '{}' longer than original ({} > {} bytes), skipping",
                            entry.id,
                            trans_bytes.len(),
                            orig_bytes.len()
                        ));
                    }
                    length_skipped += 1;
                    strings_skipped += 1;
                    continue;
                }
                let pinned = entry
                    .metadata
                    .get("length_endian")
                    .and_then(|v| v.as_str())
                    .and_then(LengthEndian::from_meta);
                let (endian, alt_endian) = match pinned {
                    Some(e) => (e, None),
                    None => (LengthEndian::Little, Some(LengthEndian::Big)),
                };
                let mut needle = Vec::with_capacity(4 + orig_bytes.len());
                needle.extend_from_slice(&endian.encode_u32(orig_bytes.len() as u32));
                needle.extend_from_slice(orig_bytes);
                let alt_needle = alt_endian.map(|ae| {
                    let mut n = Vec::with_capacity(4 + orig_bytes.len());
                    n.extend_from_slice(&ae.encode_u32(orig_bytes.len() as u32));
                    n.extend_from_slice(orig_bytes);
                    n
                });
                work.push(Work {
                    needle,
                    alt_needle,
                    endian,
                    alt_endian,
                    trans_bytes: trans_bytes.to_vec(),
                    orig_payload_len: orig_bytes.len(),
                });
            }

            if !work.is_empty() {
                let patterns: Vec<&[u8]> = work.iter().map(|w| w.needle.as_slice()).collect();
                let mut cursor =
                    crate::binary_search::MatchCursor::from_patterns(&bytes, &patterns);

                for (i, w) in work.iter().enumerate() {
                    let mut matched: Option<(usize, LengthEndian)> =
                        cursor.next_valid(i, &bytes, &w.needle).map(|p| (p, w.endian));
                    if matched.is_none() {
                        if let (Some(alt), Some(ae)) = (&w.alt_needle, w.alt_endian) {
                            if let Some(pos) = find_bytes_once(&bytes, alt) {
                                matched = Some((pos, ae));
                            }
                        }
                    }
                    if let Some((pos, used_endian)) = matched {
                        let new_len = w.trans_bytes.len() as u32;
                        bytes[pos..pos + 4].copy_from_slice(&used_endian.encode_u32(new_len));
                        bytes[pos + 4..pos + 4 + w.trans_bytes.len()]
                            .copy_from_slice(&w.trans_bytes);
                        for b in
                            &mut bytes[pos + 4 + w.trans_bytes.len()..pos + 4 + w.orig_payload_len]
                        {
                            *b = 0;
                        }
                        strings_written += 1;
                        modified = true;
                    } else {
                        strings_skipped += 1;
                    }
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
                 original Unity string (UTF-8 byte length must be ≤ source). Shorten them or \
                 use a length-aware model; equal-length translations inject cleanly."
            ));
        }

        Ok(InjectionReport { files_modified, strings_written, strings_skipped, warnings, files_written })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn tempdir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("locust_unity_{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn create_unity_fixture(dir: &Path) -> PathBuf {
        let data_dir = dir.join("TestGame_Data");
        fs::create_dir_all(&data_dir).unwrap();
        fs::write(dir.join("UnityPlayer.dll"), b"fake").unwrap();

        let mut data: Vec<u8> = vec![0; 64];
        let s1 = b"Hello World";
        data.extend_from_slice(&(s1.len() as u32).to_le_bytes());
        data.extend_from_slice(s1);
        data.push(0);
        data.extend_from_slice(&[0xFF; 8]);
        let s2 = b"Press any key to continue";
        data.extend_from_slice(&(s2.len() as u32).to_le_bytes());
        data.extend_from_slice(s2);
        data.extend_from_slice(&[0, 0, 0]);
        data.extend_from_slice(&[0; 32]);
        let assets_path = data_dir.join("resources.assets");
        fs::write(&assets_path, &data).unwrap();
        dir.to_path_buf()
    }

    fn create_vn_script_fixture(dir: &Path) -> PathBuf {
        let data_dir = dir.join("TestGame_Data");
        fs::create_dir_all(&data_dir).unwrap();
        fs::write(dir.join("UnityPlayer.dll"), b"fake").unwrap();

        let scripts_dir = data_dir.join("SCRIPTS~");
        fs::create_dir_all(&scripts_dir).unwrap();

        fs::write(scripts_dir.join("Chapter_1.txt"), r#"version 1.0

script Chapter_1_script chapter 1 {

  index 0
    scene black_screen 0
    Nar This is the beginning of our story.

  index 1
    J My name is Jamie.

  index 2
    J I'm waiting for my best friend!

  index 3
    menu MainMenu

  index 4
    J Let's go!
}
"#).unwrap();

        fs::write(scripts_dir.join("Menus.txt"), r#"version 1.0

  menu MainMenu {
    button 0 "Talk" jump 10
    button 1 "Examine" jump 20
    button 2 "Leave" +main jump 30
  }
"#).unwrap();

        dir.to_path_buf()
    }

    #[test]
    fn test_detect_unity() {
        let dir = tempdir();
        create_unity_fixture(&dir);
        let plugin = UnityPlugin::new();
        assert!(plugin.detect(&dir));
    }

    #[test]
    fn test_detect_non_unity() {
        let dir = tempdir();
        let plugin = UnityPlugin::new();
        assert!(!plugin.detect(&dir));
    }

    #[test]
    fn test_serialized_candidate_extensionless_managers_and_levels() {
        use std::path::PathBuf;
        assert!(is_unity_serialized_candidate(Path::new(
            "Game_Data/globalgamemanagers"
        )));
        assert!(is_unity_serialized_candidate(Path::new("Game_Data/level0")));
        assert!(is_unity_serialized_candidate(Path::new(
            "Game_Data/level12"
        )));
        assert!(is_unity_serialized_candidate(Path::new(
            "Game_Data/resources.assets"
        )));
        assert!(is_unity_serialized_candidate(Path::new(
            "Game_Data/resources"
        )));
        // Companions / non-serialized
        assert!(!is_unity_serialized_candidate(Path::new(
            "Game_Data/globalgamemanagers.assets.resS"
        )));
        assert!(!is_unity_serialized_candidate(Path::new(
            "Game_Data/resources.resource"
        )));
        assert!(!is_unity_serialized_candidate(Path::new(
            "Game_Data/UnityPlayer.dll"
        )));
        assert!(!is_unity_serialized_candidate(Path::new(
            "Game_Data/sharedassets0.assets.resS"
        )));
        // PathBuf for Windows-style
        let p = PathBuf::from(r"C:\Games\Boxman_Data\globalgamemanagers");
        assert!(is_unity_serialized_candidate(&p));
    }

    #[test]
    fn test_extract_finds_extensionless_globalgamemanagers() {
        let dir = tempdir();
        let data_dir = dir.join("TestGame_Data");
        fs::create_dir_all(&data_dir).unwrap();
        fs::write(dir.join("UnityPlayer.dll"), b"fake").unwrap();

        // Real SerializedFile (v17 TextAsset) stored under the classic
        // extensionless managers name — must be discovered and extracted.
        let bytes = crate::unity_serialized::write_v17_fixture(
            "Sys",
            "Extensionless managers dialogue",
        );
        fs::write(data_dir.join("globalgamemanagers"), &bytes).unwrap();

        let plugin = UnityPlugin::new();
        let found = UnityPlugin::find_assets_files(&dir);
        assert!(
            found.iter().any(|p| p
                .file_name()
                .is_some_and(|n| n == "globalgamemanagers")),
            "must discover globalgamemanagers: {found:?}"
        );
        let entries = plugin.extract(&dir).unwrap();
        assert!(
            entries
                .iter()
                .any(|e| e.source == "Extensionless managers dialogue"),
            "got: {:?}",
            entries.iter().map(|e| &e.source).collect::<Vec<_>>()
        );
    }

    #[test]
    fn test_find_assets_nested_depth3_level() {
        let dir = tempdir();
        let data_dir = dir.join("TestGame_Data");
        let nested = data_dir.join("Scenes").join("Act1");
        fs::create_dir_all(&nested).unwrap();
        fs::write(dir.join("UnityPlayer.dll"), b"fake").unwrap();
        let bytes =
            crate::unity_serialized::write_v17_fixture("N", "Nested level dialogue here");
        // Depth from *_Data: Scenes (1) / Act1 (2) / level0 (3)
        fs::write(nested.join("level0"), &bytes).unwrap();

        let found = UnityPlugin::find_assets_files(&dir);
        assert!(
            found.iter().any(|p| p.file_name().is_some_and(|n| n == "level0")),
            "max_depth 3 must reach nested level0: {found:?}"
        );
        let plugin = UnityPlugin::new();
        let entries = plugin.extract(&dir).unwrap();
        assert!(
            entries
                .iter()
                .any(|e| e.source == "Nested level dialogue here"),
            "got: {:?}",
            entries.iter().map(|e| &e.source).collect::<Vec<_>>()
        );
    }

    #[test]
    fn test_extract_assets_strings() {
        let dir = tempdir();
        create_unity_fixture(&dir);
        let plugin = UnityPlugin::new();
        let entries = plugin.extract(&dir).unwrap();
        let sources: Vec<&str> = entries.iter().map(|e| e.source.as_str()).collect();
        assert!(sources.contains(&"Hello World"), "got: {:?}", sources);
        assert!(sources.contains(&"Press any key to continue"), "got: {:?}", sources);
    }

    #[test]
    fn test_extract_assets_binary_slot_metadata() {
        let dir = tempdir();
        create_unity_fixture(&dir);
        let plugin = UnityPlugin::new();
        let entries = plugin.extract(&dir).unwrap();
        assert!(!entries.is_empty(), "fixture must yield .assets strings");
        for entry in &entries {
            assert_eq!(
                entry.metadata.get("binary_slot"),
                Some(&serde_json::Value::String("utf8".into())),
                "entry {} missing binary_slot for validate/inject preflight",
                entry.id
            );
        }
    }

    #[test]
    fn test_extract_vn_scripts() {
        let dir = tempdir();
        create_vn_script_fixture(&dir);
        let plugin = UnityPlugin::new();
        let entries = plugin.extract(&dir).unwrap();

        let sources: Vec<&str> = entries.iter().map(|e| e.source.as_str()).collect();
        assert!(sources.contains(&"This is the beginning of our story."), "got: {:?}", sources);
        assert!(sources.contains(&"My name is Jamie."), "got: {:?}", sources);
        assert!(sources.contains(&"I'm waiting for my best friend!"), "got: {:?}", sources);
        assert!(sources.contains(&"Let's go!"), "got: {:?}", sources);

        // Menu buttons
        assert!(sources.contains(&"Talk"), "got: {:?}", sources);
        assert!(sources.contains(&"Examine"), "got: {:?}", sources);
        assert!(sources.contains(&"Leave"), "got: {:?}", sources);
    }

    #[test]
    fn test_vn_script_dialogue_has_context() {
        let dir = tempdir();
        create_vn_script_fixture(&dir);
        let plugin = UnityPlugin::new();
        let entries = plugin.extract(&dir).unwrap();

        let jamie = entries.iter().find(|e| e.source == "My name is Jamie.").unwrap();
        assert_eq!(jamie.context, Some("J".to_string()));
        assert!(jamie.tags.contains(&"dialogue".to_string()));

        let nar = entries.iter().find(|e| e.source.contains("beginning")).unwrap();
        assert_eq!(nar.context, Some("Nar".to_string()));
    }

    #[test]
    fn test_inject_vn_scripts() {
        let dir = tempdir();
        create_vn_script_fixture(&dir);
        let plugin = UnityPlugin::new();
        let mut entries = plugin.extract(&dir).unwrap();

        for entry in &mut entries {
            if entry.source == "My name is Jamie." {
                entry.translation = Some("Mi nombre es Jamie.".to_string());
            }
            if entry.source == "Talk" {
                entry.translation = Some("Hablar".to_string());
            }
        }

        let report = plugin.inject(&dir, &entries).unwrap();
        assert!(report.strings_written >= 2);
        assert!(report.files_modified >= 1);

        // Verify replacement
        let content = fs::read_to_string(
            dir.join("TestGame_Data").join("SCRIPTS~").join("Chapter_1.txt")
        ).unwrap();
        assert!(content.contains("Mi nombre es Jamie."));
    }

    #[test]
    fn test_inject_assets_shorter_succeeds() {
        let dir = tempdir();
        create_unity_fixture(&dir);
        let plugin = UnityPlugin::new();
        let mut entries = plugin.extract(&dir).unwrap();

        for entry in &mut entries {
            if entry.source == "Hello World" {
                entry.translation = Some("Hola Mundo".to_string());
            }
        }

        let report = plugin.inject(&dir, &entries).unwrap();
        assert!(report.strings_written >= 1);
    }

    /// Big-endian length-prefixed strings must extract and inject without
    /// assuming little-endian u32, and must not steal LE strings via off-by-3
    /// BE shadows.
    #[test]
    fn test_heuristic_be_extract_and_inject() {
        let dir = tempdir();
        let data_dir = dir.join("TestGame_Data");
        fs::create_dir_all(&data_dir).unwrap();
        fs::write(dir.join("UnityPlayer.dll"), b"fake").unwrap();

        // Pad so this is not a valid SerializedFile header → pure heuristic path.
        // Leading non-zero pad prevents accidental LE length hits.
        let s = b"Hello World"; // 11 bytes
        let mut data: Vec<u8> = vec![0xFF; 64];
        data.extend_from_slice(&(s.len() as u32).to_be_bytes());
        data.extend_from_slice(s);
        data.extend_from_slice(&[0, 0, 0]);
        let assets = data_dir.join("resources.assets");
        fs::write(&assets, &data).unwrap();

        let plugin = UnityPlugin::new();
        let entries = plugin.extract(&dir).unwrap();
        let hello = entries
            .iter()
            .find(|e| e.source == "Hello World")
            .expect("BE length-prefixed Hello World must extract");
        assert_eq!(
            hello
                .metadata
                .get("length_endian")
                .and_then(|v| v.as_str()),
            Some("be"),
            "must record big-endian length: {:?}",
            hello.metadata
        );

        let mut inject_entries = entries;
        for e in &mut inject_entries {
            if e.source == "Hello World" {
                e.translation = Some("Hola Mundo".to_string()); // 10 < 11
            }
        }
        let report = plugin.inject(&dir, &inject_entries).unwrap();
        assert!(
            report.strings_written >= 1,
            "BE inject written={} skipped={} warn={:?}",
            report.strings_written,
            report.strings_skipped,
            report.warnings
        );
        let out = fs::read(&assets).unwrap();
        let be_ten = 10u32.to_be_bytes();
        assert!(
            out.windows(4 + 10)
                .any(|w| w[..4] == be_ten && &w[4..] == b"Hola Mundo"),
            "expected BE len=10 + Hola Mundo"
        );
    }

    #[test]
    fn test_heuristic_string_at_prefers_le() {
        let s = b"Hello World";
        let mut buf = Vec::new();
        buf.extend_from_slice(&(s.len() as u32).to_le_bytes());
        buf.extend_from_slice(s);
        let (len, end) = heuristic_string_at(&buf, 0).unwrap();
        assert_eq!(len, 11);
        assert_eq!(end, LengthEndian::Little);
    }

    #[test]
    fn test_heuristic_string_at_be_when_le_implausible() {
        let s = b"Hello World";
        let mut buf = Vec::new();
        buf.extend_from_slice(&(s.len() as u32).to_be_bytes());
        buf.extend_from_slice(s);
        let (len, end) = heuristic_string_at(&buf, 0).unwrap();
        assert_eq!(len, 11);
        assert_eq!(end, LengthEndian::Big);
    }

    #[test]
    fn test_heuristic_string_at_rejects_be_shadow_of_le() {
        // 3 zeros + LE length 11 + "Hello World" — BE at offset 0 is 11 but payload
        // starts with NUL → must reject so the real LE string is not skipped.
        let s = b"Hello World";
        let mut buf = vec![0u8; 3];
        buf.extend_from_slice(&(s.len() as u32).to_le_bytes());
        buf.extend_from_slice(s);
        assert!(
            heuristic_string_at(&buf, 0).is_none(),
            "BE shadow of LE length must be rejected"
        );
        let (len, end) = heuristic_string_at(&buf, 3).unwrap();
        assert_eq!((len, end), (11, LengthEndian::Little));
    }

    /// Synthetic multi-pattern inject: (a) needle twice, (c) identity skip,
    /// (d) oversize skip. Entries are planted manually so extract heuristics
    /// do not filter the fixture.
    #[test]
    fn test_inject_assets_multi_pattern_semantics() {
        let dir = tempdir();
        let data_dir = dir.join("TestGame_Data");
        fs::create_dir_all(&data_dir).unwrap();
        fs::write(dir.join("UnityPlayer.dll"), b"fake").unwrap();

        let s_dup = b"DupStr!!"; // 8 bytes, appears twice
        let s_id = b"SameSame"; // identity
        let s_long = b"SlotTxt!"; // oversize translation target
        let s_ok = b"OkText!!"; // normal same-length

        let mut data: Vec<u8> = vec![0; 16];
        for s in [
            s_dup.as_slice(),
            s_id.as_slice(),
            s_dup.as_slice(),
            s_long.as_slice(),
            s_ok.as_slice(),
        ] {
            data.extend_from_slice(&(s.len() as u32).to_le_bytes());
            data.extend_from_slice(s);
            data.push(0);
        }
        let assets = data_dir.join("sharedassets0.assets");
        fs::write(&assets, &data).unwrap();

        let mk = |id: &str, source: &str, translation: Option<&str>| {
            let mut e = StringEntry::new(id, source, assets.clone());
            e.translation = translation.map(|s| s.to_string());
            e
        };
        let inject_list = vec![
            mk("dup1", "DupStr!!", Some("DupOk!!!")), // 8 → 8
            mk("id", "SameSame", Some("SameSame")),   // identity → skip
            mk("dup2", "DupStr!!", Some("DupOk!!!")), // second occurrence
            mk("over", "SlotTxt!", Some("WAYTOOLONG")), // oversize → skip
            mk("ok", "OkText!!", Some("OkTxt!!!")),   // 8 → 8
        ];

        let plugin = UnityPlugin::new();
        let report = plugin.inject(&dir, &inject_list).unwrap();
        assert_eq!(
            report.strings_written, 3,
            "two dups + ok; got written={} skipped={} warnings={:?}",
            report.strings_written,
            report.strings_skipped,
            report.warnings
        );
        assert_eq!(report.strings_skipped, 2, "identity + oversize");
        assert!(
            report
                .warnings
                .iter()
                .any(|w| w.contains("longer") || w.contains("skipped because")),
            "oversize should warn: {:?}",
            report.warnings
        );

        let out = fs::read(&assets).unwrap();
        assert_eq!(
            out.windows(8).filter(|w| *w == b"DupOk!!!").count(),
            2,
            "both DupStr slots rewritten"
        );
        assert!(out.windows(8).any(|w| w == b"OkTxt!!!"));
        assert!(out.windows(8).any(|w| w == b"SameSame"), "identity unchanged");
        assert!(out.windows(8).any(|w| w == b"SlotTxt!"), "oversize target unchanged");
    }

    #[test]
    fn test_is_translatable() {
        assert!(is_unity_translatable("Hello World"));
        assert!(is_unity_translatable("Press any key to continue"));
        assert!(is_unity_translatable("Hello")); // plain word, not code id
        assert!(is_unity_translatable("Save game"));
        assert!(!is_unity_translatable("abc"));
        assert!(!is_unity_translatable("SOME_CONSTANT_NAME"));
        assert!(!is_unity_translatable("Assets/Textures/player.png"));
        assert!(!is_unity_translatable("UnityEngine.CoreModule"));
        // Full .NET AQN (spaces after commas — old filter missed these)
        assert!(!is_unity_translatable(
            "Naninovel.Script, Elringus.Naninovel.Runtime, Version=0.0.0.0, Culture=neutral, PublicKeyToken=null"
        ));
        assert!(!is_unity_translatable(
            "UnityEditor.DefaultAsset, UnityEditor, Version=0.0.0.0, Culture=neutral, PublicKeyToken=null"
        ));
        assert!(!is_unity_translatable(
            "@novel\n@dotween name:\"ItemList\" dir:1\n@stop"
        ));
        assert!(!is_unity_translatable("@hideUI TutorialUI"));
        assert!(!is_unity_translatable(
            "Lorem ipsum dolor sit amet, consectetur adipiscing elit"
        ));
        // MonoScript / type-name noise (BOXMAN heuristic flood)
        // Title-case single words like "Naninovel" stay filter-pass (same as "Hello");
        // MonoScript class 115 byte ranges are skipped instead.
        assert!(!is_unity_translatable("QuaternionTween"));
        assert!(!is_unity_translatable("ISpawnManager"));
        assert!(!is_unity_translatable("TMPro"));
        assert!(!is_unity_translatable("Command_POSGenerator"));
        // Unity hierarchy instance names
        assert!(!is_unity_translatable("Light 2D (7)"));
        assert!(!is_unity_translatable("SpeechBubbleIcon (4)"));
        assert!(!is_unity_translatable("PROPS_STRUCTURE_12 (1)"));
        // Shader path leftovers outside Shader object ranges
        assert!(!is_unity_translatable("Legacy Shaders/Reflective/Diffuse"));
        assert!(!is_unity_translatable("Hidden/Internal-GUITexture"));
        assert!(!is_unity_translatable("UI/Default Font"));
        assert!(!is_unity_translatable("Base Layer"));
        assert!(!is_unity_translatable(
            "BLENDMODES_MODE_MULTIPLY ETC1_EXTERNAL_ALPHA"
        ));
        assert!(!is_unity_translatable("Skybox/Procedural"));
        assert!(!is_unity_translatable("Light 2D"));
        assert!(!is_unity_translatable("btn Night (Selected)"));
        assert!(!is_unity_translatable(
            "naninovel/audio/bgm/hscene_ntr/erotic 01"
        ));
        // BOXMAN globalgamemanagers single-slash shader families
        assert!(!is_unity_translatable("Mobile/Diffuse"));
        assert!(!is_unity_translatable("FX/Flare"));
        assert!(!is_unity_translatable("Nature/Tree Creator Leaves Fast"));
        assert!(!is_unity_translatable(
            "Mobile/Bumped Specular (1 Directional Realtime Light)"
        ));
        assert!(!is_unity_translatable("Mesh Renderer (Id :1)"));
        // Animator state machine paths (layer.STATE) — BOXMAN heuristic flood
        assert!(!is_unity_translatable("Base Layer.SCENE_JAKE_COWGIRL_1"));
        assert!(!is_unity_translatable("Base Layer.ElectricityFX34"));
        // TMP / font material crumbs
        assert!(!is_unity_translatable("Roboto-Regular Atlas Material"));
        // Unity uGUI ScrollRect / mask hierarchy defaults
        assert!(!is_unity_translatable("Sliding Area"));
        assert!(!is_unity_translatable("Viewport"));
        // Binary soup mis-read as length-prefixed strings
        assert!(!is_unity_translatable("\x18$1>JVbmrvv"));
        assert!(!is_unity_translatable("\x16!,7@IOSSS"));
        // All-lowercase code tokens (no word break) — not Title Case UI
        assert!(!is_unity_translatable("bezierpoint"));
        assert!(!is_unity_translatable("workmode"));
        // Animation / timeline clip names ending in _NN
        assert!(!is_unity_translatable("Worker_Deliver Order_03"));
        assert!(!is_unity_translatable("Worker_Pickup Order_01"));
        // Resource path fragments with a pure-digit segment
        assert!(!is_unity_translatable("night/2 centered"));
        // Hierarchy asset-type suffix
        assert!(!is_unity_translatable("Pillar Sprite"));
        // Extra uGUI default
        assert!(!is_unity_translatable("Thumbnail"));
        // Real UI / dialogue with slash must still pass when clearly sentence-like.
        assert!(is_unity_translatable("Press Start"));
        assert!(is_unity_translatable("Save game now"));
        assert!(is_unity_translatable("Delete")); // short UI verb
        assert!(is_unity_translatable("Progress")); // short UI label
        assert!(is_unity_translatable("Clothing")); // inventory category
    }

    #[test]
    fn test_textasset_skips_linebreak_charset_tables() {
        let dir = tempdir();
        let data_dir = dir.join("TestGame_Data");
        fs::create_dir_all(&data_dir).unwrap();
        fs::write(dir.join("UnityPlayer.dll"), b"fake").unwrap();
        // Near-zero alphabetic content — TMP line-break character class table.
        let charset = "([｛〔〈《「『【〘〖〝‘“｟«$—…‥〳〴〵\\［（{£¥\"々〇〉》」＄｠￥￦ #)]｝〕〉》」』】〙〗〟’”｠»";
        assert!(
            !crate::unity_serialized::is_textasset_script_worth_extracting(charset),
            "charset table must be rejected"
        );
        assert!(crate::unity_serialized::is_textasset_script_worth_extracting(
            "TitleMenu.START: NEW GAME\r\nTitleMenu.CREDITS: CREDITS"
        ));
        let bytes = crate::unity_serialized::write_v17_fixture("LineBreak", charset);
        fs::write(data_dir.join("sharedassets0.assets"), bytes).unwrap();
        let plugin = UnityPlugin::new();
        let entries = plugin.extract(&dir).unwrap();
        let ta: Vec<_> = entries
            .iter()
            .filter(|e| e.tags.iter().any(|t| t == "textasset"))
            .collect();
        assert!(
            ta.is_empty(),
            "line-break charset TextAsset must not extract: {:?}",
            ta.iter().map(|e| &e.source).collect::<Vec<_>>()
        );
    }

    /// MonoScript bodies must not leak type names into heuristic extract.
    #[test]
    fn test_monoscript_range_skipped_by_heuristic() {
        let dir = tempdir();
        let data_dir = dir.join("TestGame_Data");
        fs::create_dir_all(&data_dir).unwrap();
        fs::write(dir.join("UnityPlayer.dll"), b"fake").unwrap();
        let bytes = crate::unity_serialized::write_v17_monoscript_noise_fixture();
        fs::write(data_dir.join("sharedassets0.assets"), bytes).unwrap();

        let plugin = UnityPlugin::new();
        let entries = plugin.extract(&dir).unwrap();
        let sources: Vec<&str> = entries.iter().map(|e| e.source.as_str()).collect();
        assert!(
            sources.iter().any(|s| s.contains("Hello traveler")),
            "TextAsset script still extracted: {:?}",
            sources
        );
        assert!(
            !sources.iter().any(|s| *s == "Naninovel" || *s == "QuaternionTween"),
            "MonoScript type names must not appear: {:?}",
            sources
        );
    }

    fn create_textasset_assets_fixture(dir: &Path) -> PathBuf {
        let data_dir = dir.join("TestGame_Data");
        fs::create_dir_all(&data_dir).unwrap();
        fs::write(dir.join("UnityPlayer.dll"), b"fake").unwrap();
        let bytes = crate::unity_serialized::write_v17_fixture(
            "Dialog",
            "Hello traveler, welcome!", // 25 chars — room to shorten
        );
        let path = data_dir.join("sharedassets0.assets");
        fs::write(&path, bytes).unwrap();
        path
    }

    #[test]
    fn test_textasset_extract_structural() {
        let dir = tempdir();
        create_textasset_assets_fixture(&dir);
        let plugin = UnityPlugin::new();
        let entries = plugin.extract(&dir).unwrap();
        let ta: Vec<_> = entries
            .iter()
            .filter(|e| e.tags.iter().any(|t| t == "textasset"))
            .collect();
        assert!(
            !ta.is_empty(),
            "expected TextAsset entries, got {:?}",
            entries.iter().map(|e| &e.id).collect::<Vec<_>>()
        );
        assert!(ta.iter().any(|e| e.id.starts_with("textasset/")));
        assert!(ta.iter().any(|e| e.source.contains("Hello traveler")));
        assert_eq!(
            ta[0]
                .metadata
                .get("extraction_method")
                .and_then(|v| v.as_str()),
            Some("textasset")
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_parse_textasset_csv_text_columns() {
        let csv = "\
ITEM_CATEGORY,ITEM_NAME,ITEM_TEST_ID\r\n\
Electronics,mp3 player,15\r\n\
Electronics,towel,14\r\n\
Electronics,video games,13\r\n";
        let parsed = parse_textasset_csv(csv).expect("csv");
        // CATEGORY + NAME are text; TEST_ID numeric skipped.
        assert!(
            parsed.cells.iter().any(|c| c.value == "mp3 player" && c.header == "ITEM_NAME"),
            "{:?}",
            parsed.cells
        );
        assert!(
            parsed.cells.iter().any(|c| c.value == "Electronics"),
            "{:?}",
            parsed.cells
        );
        assert!(
            !parsed.cells.iter().any(|c| c.value == "15"),
            "numeric col must not extract: {:?}",
            parsed.cells
        );
        assert!(parse_textasset_csv("Hello traveler, welcome!").is_none());
        assert!(parse_textasset_csv("a,b\n1,2").is_none()); // only 1 data row
    }

    #[test]
    fn test_textasset_csv_extract_and_inject() {
        let dir = tempdir();
        let data_dir = dir.join("TestGame_Data");
        fs::create_dir_all(&data_dir).unwrap();
        fs::write(dir.join("UnityPlayer.dll"), b"fake").unwrap();
        let mut script = String::from(
            "ITEM_CATEGORY,ITEM_NAME,ITEM_TEST_ID\r\n\
Electronics,mp3 player,15\r\n\
Electronics,towel,14\r\n\
Electronics,video games,13\r\n",
        );
        // Trailing spaces enlarge the m_Script budget for inject pad-in-place.
        script.push_str(&" ".repeat(48));
        let bytes = crate::unity_serialized::write_v17_fixture("Items", &script);
        let assets = data_dir.join("sharedassets0.assets");
        fs::write(&assets, &bytes).unwrap();

        let plugin = UnityPlugin::new();
        let mut entries = plugin.extract(&dir).unwrap();
        let cells: Vec<_> = entries
            .iter()
            .filter(|e| {
                e.metadata
                    .get("extraction_method")
                    .and_then(|v| v.as_str())
                    == Some("textasset_csv_cell")
            })
            .collect();
        assert!(
            cells.len() >= 4,
            "expected csv cells, got {:?}",
            entries.iter().map(|e| (&e.id, &e.source)).collect::<Vec<_>>()
        );
        assert!(cells.iter().any(|e| e.source == "towel"));

        for e in &mut entries {
            if e.source == "towel" {
                e.translation = Some("toalla".into());
            }
            if e.source == "mp3 player" {
                e.translation = Some("mp3".into());
            }
        }
        let report = plugin.inject(&dir, &entries).unwrap();
        assert!(
            report.strings_written >= 1,
            "written={} {:?}",
            report.strings_written,
            report.warnings
        );
        let again = plugin.extract(&dir).unwrap();
        let values: Vec<&str> = again
            .iter()
            .filter(|e| {
                e.metadata
                    .get("extraction_method")
                    .and_then(|v| v.as_str())
                    == Some("textasset_csv_cell")
            })
            .map(|e| e.source.as_str())
            .collect();
        assert!(values.iter().any(|v| *v == "toalla"), "{values:?}");
        assert!(values.iter().any(|v| *v == "mp3"), "{values:?}");
        assert!(values.iter().any(|v| *v == "video games"), "{values:?}");
    }

    #[test]
    fn test_parse_textasset_loc_lines_key_value() {
        let doc = "TitleMenu.START: NEW GAME\r\nTitleMenu.CREDITS: CREDITS\r\nConfirmation.Yes: YES\r\n";
        let lines = parse_textasset_loc_lines(doc).expect("loc doc");
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0].key.as_deref(), Some("TitleMenu.START"));
        assert_eq!(lines[0].value, "NEW GAME");
        assert_eq!(lines[1].value, "CREDITS");
        // Single-line ManagedText with dotted key → one loc line (value only).
        let one = parse_textasset_loc_lines("TitleMenu.START: NEW GAME").expect("single");
        assert_eq!(one.len(), 1);
        assert_eq!(one[0].value, "NEW GAME");
        assert_eq!(one[0].key.as_deref(), Some("TitleMenu.START"));
        // Prose / non-key single line: no split
        assert!(parse_textasset_loc_lines("Hello traveler, welcome!").is_none());
        assert!(parse_textasset_loc_lines("Not a key: has spaces in key side").is_none());
    }

    #[test]
    fn test_locale_catalog_keys_and_docs_skipped() {
        assert!(looks_like_bcp47_locale_id("af"));
        assert!(looks_like_bcp47_locale_id("af-ZA"));
        assert!(looks_like_bcp47_locale_id("zh-Hans"));
        assert!(looks_like_bcp47_locale_id("es-419"));
        assert!(looks_like_bcp47_locale_id("en-US"));
        assert!(!looks_like_bcp47_locale_id("TitleMenu.START"));
        assert!(!looks_like_bcp47_locale_id("Confirmation.Yes"));
        assert!(!looks_like_bcp47_locale_id("Carter"));
        assert!(!looks_like_bcp47_locale_id("START"));

        let catalog = "\
af: Afrikaans\r\n\
af-ZA: Afrikaans (South Africa)\r\n\
ar: Arabic\r\n\
ar-AE: Arabic (U.A.E.)\r\n\
en: English\r\n\
en-US: English (United States)\r\n\
es: Spanish\r\n\
es-419: Spanish (Latin America)\r\n\
zh-Hans: Chinese (Simplified)\r\n";
        assert!(is_locale_catalog_script(catalog));
        assert!(!is_locale_catalog_script(
            "TitleMenu.START: NEW GAME\r\nTitleMenu.CREDITS: CREDITS\r\n"
        ));

        // Locale keys dropped from mixed docs; game keys kept.
        let mixed = "\
af: Afrikaans\r\n\
TitleMenu.START: NEW GAME\r\n\
en-US: English (United States)\r\n\
Confirmation.Yes: YES\r\n";
        let lines = parse_textasset_loc_lines(mixed).expect("mixed");
        assert!(
            lines.iter().all(|l| {
                l.key
                    .as_deref()
                    .map(|k| !looks_like_bcp47_locale_id(k))
                    .unwrap_or(true)
            }),
            "locale keys must be filtered: {lines:?}"
        );
        assert!(lines.iter().any(|l| l.value == "NEW GAME"));
        assert!(lines.iter().any(|l| l.value == "YES"));
        assert!(!lines.iter().any(|l| l.value.contains("Afrikaans")));

        // Full extract skips Naninovel Locales catalog asset.
        let dir = tempdir();
        let data_dir = dir.join("TestGame_Data");
        fs::create_dir_all(&data_dir).unwrap();
        fs::write(dir.join("UnityPlayer.dll"), b"fake").unwrap();
        let bytes = crate::unity_serialized::write_v17_fixture("Locales", catalog);
        fs::write(data_dir.join("sharedassets0.assets"), &bytes).unwrap();
        let entries = UnityPlugin::new().extract(&dir).unwrap();
        assert!(
            !entries.iter().any(|e| e.source.contains("Afrikaans")),
            "locale catalog must not extract: {:?}",
            entries.iter().map(|e| &e.source).collect::<Vec<_>>()
        );
        assert!(
            !entries.iter().any(|e| e.source.contains("af:")),
            "must not fall back to whole-blob extract: {:?}",
            entries.iter().map(|e| &e.source).collect::<Vec<_>>()
        );
    }

    #[test]
    fn test_textasset_loc_line_extract_and_inject() {
        let dir = tempdir();
        let data_dir = dir.join("TestGame_Data");
        fs::create_dir_all(&data_dir).unwrap();
        fs::write(dir.join("UnityPlayer.dll"), b"fake").unwrap();
        // Pad script so inject can expand short values within total budget.
        let script = "\
TitleMenu.START: NEW GAME\r\n\
TitleMenu.CREDITS: CREDITS\r\n\
Confirmation.Yes: YES\r\n\
// spare padding for longer ES forms          ";
        let bytes = crate::unity_serialized::write_v17_fixture("ManagedText", script);
        let assets = data_dir.join("sharedassets0.assets");
        fs::write(&assets, &bytes).unwrap();

        let plugin = UnityPlugin::new();
        let mut entries = plugin.extract(&dir).unwrap();
        let loc: Vec<_> = entries
            .iter()
            .filter(|e| {
                e.metadata
                    .get("extraction_method")
                    .and_then(|v| v.as_str())
                    == Some("textasset_loc_line")
            })
            .collect();
        assert!(
            loc.len() >= 3,
            "expected loc lines, got {:?}",
            entries.iter().map(|e| (&e.id, &e.source)).collect::<Vec<_>>()
        );
        assert!(loc.iter().any(|e| e.source == "NEW GAME"));
        assert!(loc.iter().any(|e| e.source == "CREDITS"));
        assert!(loc.iter().any(|e| {
            e.metadata
                .get("loc_key")
                .and_then(|v| v.as_str())
                == Some("TitleMenu.START")
        }));

        for e in &mut entries {
            if e.source == "NEW GAME" {
                e.translation = Some("NUEVA".into());
            }
            if e.source == "YES" {
                e.translation = Some("SI".into());
            }
        }
        let report = plugin.inject(&dir, &entries).unwrap();
        assert!(
            report.strings_written >= 1,
            "written={} {:?}",
            report.strings_written,
            report.warnings
        );
        let again = plugin.extract(&dir).unwrap();
        let values: Vec<&str> = again
            .iter()
            .filter(|e| {
                e.metadata
                    .get("extraction_method")
                    .and_then(|v| v.as_str())
                    == Some("textasset_loc_line")
            })
            .map(|e| e.source.as_str())
            .collect();
        assert!(
            values.iter().any(|v| *v == "NUEVA"),
            "re-extract values: {values:?}"
        );
        assert!(
            values.iter().any(|v| *v == "SI"),
            "re-extract values: {values:?}"
        );
        assert!(
            values.iter().any(|v| *v == "CREDITS"),
            "untouched line kept: {values:?}"
        );
    }

    #[test]
    fn test_textasset_inject_same_and_shorter() {
        let dir = tempdir();
        let assets = create_textasset_assets_fixture(&dir);
        let plugin = UnityPlugin::new();
        let mut entries = plugin.extract(&dir).unwrap();
        for e in &mut entries {
            if e.tags.iter().any(|t| t == "textasset") && e.source.contains("Hello traveler") {
                // shorter — pad with spaces in place
                e.translation = Some("Hola viajero!".into());
            }
        }
        let report = plugin.inject(&dir, &entries).unwrap();
        assert!(
            report.strings_written >= 1,
            "written={} skipped={} {:?}",
            report.strings_written,
            report.strings_skipped,
            report.warnings
        );
        let again = plugin.extract(&dir).unwrap();
        assert!(
            again.iter().any(|e| e.source.starts_with("Hola viajero!")),
            "re-extract: {:?}",
            again.iter().map(|e| &e.source).collect::<Vec<_>>()
        );
        // Object table / file still parseable
        let sf = SerializedFile::parse_path(&assets).unwrap();
        assert_eq!(sf.objects.len(), 2);
        let _ = fs::remove_dir_all(&dir);
    }

    fn create_mono_assets_fixture(dir: &Path) -> PathBuf {
        let data_dir = dir.join("TestGame_Data");
        fs::create_dir_all(&data_dir).unwrap();
        fs::write(dir.join("UnityPlayer.dll"), b"fake").unwrap();
        let bytes = crate::unity_serialized::write_v17_mono_fixture(
            "DialogBox",
            &["Hello traveler, welcome!"],
        );
        let path = data_dir.join("sharedassets0.assets");
        fs::write(&path, bytes).unwrap();
        path
    }

    #[test]
    fn test_mono_extract_structural() {
        let dir = tempdir();
        create_mono_assets_fixture(&dir);
        let plugin = UnityPlugin::new();
        let entries = plugin.extract(&dir).unwrap();
        let mono: Vec<_> = entries
            .iter()
            .filter(|e| e.tags.iter().any(|t| t == "monobehaviour"))
            .collect();
        assert!(
            mono.len() >= 2,
            "expected m_Name + dialogue, got {:?}",
            entries.iter().map(|e| (&e.id, &e.source)).collect::<Vec<_>>()
        );
        assert!(mono.iter().any(|e| e.source == "DialogBox"));
        assert!(mono.iter().any(|e| e.source.contains("Hello traveler")));
        assert!(mono.iter().any(|e| e.id.starts_with("monobehaviour/")));
        assert_eq!(
            mono[0]
                .metadata
                .get("extraction_method")
                .and_then(|v| v.as_str()),
            Some("monobehaviour")
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_mono_inject_shorter() {
        let dir = tempdir();
        let assets = create_mono_assets_fixture(&dir);
        let plugin = UnityPlugin::new();
        let mut entries = plugin.extract(&dir).unwrap();
        for e in &mut entries {
            if e.tags.iter().any(|t| t == "monobehaviour") && e.source.contains("Hello traveler") {
                e.translation = Some("Hola!".into());
            }
        }
        let report = plugin.inject(&dir, &entries).unwrap();
        assert!(
            report.strings_written >= 1,
            "written={} {:?}",
            report.strings_written,
            report.warnings
        );
        let again = plugin.extract(&dir).unwrap();
        assert!(
            again
                .iter()
                .any(|e| e.tags.iter().any(|t| t == "monobehaviour") && e.source.starts_with("Hola!")),
            "re-extract: {:?}",
            again.iter().map(|e| &e.source).collect::<Vec<_>>()
        );
        let sf = SerializedFile::parse_path(&assets).unwrap();
        assert_eq!(sf.mono_behaviour_objects().count(), 1);
        let _ = fs::remove_dir_all(&dir);
    }

    fn create_textmesh_assets_fixture(dir: &Path) -> PathBuf {
        let data_dir = dir.join("TestGame_Data");
        fs::create_dir_all(&data_dir).unwrap();
        fs::write(dir.join("UnityPlayer.dll"), b"fake").unwrap();
        let bytes = crate::unity_serialized::write_v17_textmesh_fixture("Hello, world!");
        let path = data_dir.join("sharedassets0.assets");
        fs::write(&path, bytes).unwrap();
        path
    }

    fn create_dual_textmesh_same_text_fixture(dir: &Path) -> PathBuf {
        let data_dir = dir.join("TestGame_Data");
        fs::create_dir_all(&data_dir).unwrap();
        fs::write(dir.join("UnityPlayer.dll"), b"fake").unwrap();
        let bytes = crate::unity_serialized::write_v17_dual_textmesh_same_text("OK");
        let path = data_dir.join("sharedassets0.assets");
        fs::write(&path, bytes).unwrap();
        path
    }

    /// Heuristic scan must keep every offset of the same label (not de-dupe by text)
    /// so multi-pattern inject can rewrite all occurrences.
    #[test]
    fn test_heuristic_keeps_duplicate_text_offsets() {
        let dir = tempdir();
        let data_dir = dir.join("TestGame_Data");
        fs::create_dir_all(&data_dir).unwrap();
        fs::write(dir.join("UnityPlayer.dll"), b"fake").unwrap();
        // Not a SerializedFile — pure heuristic payload with the same string twice.
        let s = b"Press Start"; // passes is_unity_translatable
        let mut data = vec![0u8; 32];
        for _ in 0..2 {
            data.extend_from_slice(&(s.len() as u32).to_le_bytes());
            data.extend_from_slice(s);
            data.extend_from_slice(&[0u8; 8]); // gap so scan continues
        }
        let assets = data_dir.join("sharedassets0.assets");
        fs::write(&assets, &data).unwrap();

        let plugin = UnityPlugin::new();
        let entries = plugin.extract(&dir).unwrap();
        let hits: Vec<_> = entries
            .iter()
            .filter(|e| e.source == "Press Start")
            .collect();
        assert_eq!(
            hits.len(),
            2,
            "expected both heuristic offsets, got {:?}",
            entries.iter().map(|e| (&e.id, &e.source)).collect::<Vec<_>>()
        );
        assert!(hits.iter().all(|e| {
            e.metadata
                .get("extraction_method")
                .and_then(|v| v.as_str())
                == Some("heuristic")
        }));

        let mut inject_entries = entries;
        for e in &mut inject_entries {
            if e.source == "Press Start" {
                e.translation = Some("Pulsa!".into()); // 6 < 11
            }
        }
        let report = plugin.inject(&dir, &inject_entries).unwrap();
        assert!(
            report.strings_written >= 2,
            "both occurrences must inject: written={} {:?}",
            report.strings_written,
            report.warnings
        );
        let out = fs::read(&assets).unwrap();
        let count = out
            .windows(6)
            .filter(|w| w == b"Pulsa!")
            .count();
        assert!(
            count >= 2,
            "expected ≥2 rewritten payloads, found {count}"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    /// Two TextMeshes with identical m_Text must both extract (unique path_ids)
    /// so inject can rewrite every instance of a repeated UI label.
    #[test]
    fn test_structural_keeps_duplicate_text_instances() {
        let dir = tempdir();
        create_dual_textmesh_same_text_fixture(&dir);
        let plugin = UnityPlugin::new();
        let entries = plugin.extract(&dir).unwrap();
        let tm: Vec<_> = entries
            .iter()
            .filter(|e| e.tags.iter().any(|t| t == "textmesh"))
            .collect();
        assert_eq!(
            tm.len(),
            2,
            "expected both OK TextMeshes, got {:?}",
            entries.iter().map(|e| (&e.id, &e.source)).collect::<Vec<_>>()
        );
        assert!(tm.iter().any(|e| e.id == "textmesh/7"));
        assert!(tm.iter().any(|e| e.id == "textmesh/8"));
        assert!(tm.iter().all(|e| e.source == "OK"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_structural_inject_both_duplicate_instances() {
        let dir = tempdir();
        let assets = create_dual_textmesh_same_text_fixture(&dir);
        let plugin = UnityPlugin::new();
        let mut entries = plugin.extract(&dir).unwrap();
        for e in &mut entries {
            if e.tags.iter().any(|t| t == "textmesh") {
                e.translation = Some("Si".into());
            }
        }
        let report = plugin.inject(&dir, &entries).unwrap();
        assert!(
            report.strings_written >= 2,
            "both instances must inject: written={} {:?}",
            report.strings_written,
            report.warnings
        );
        let again = plugin.extract(&dir).unwrap();
        let tm: Vec<_> = again
            .iter()
            .filter(|e| e.tags.iter().any(|t| t == "textmesh"))
            .collect();
        assert_eq!(tm.len(), 2);
        assert!(
            tm.iter().all(|e| e.source.starts_with("Si")),
            "both rewritten: {:?}",
            tm.iter().map(|e| &e.source).collect::<Vec<_>>()
        );
        let sf = SerializedFile::parse_path(&assets).unwrap();
        assert_eq!(sf.text_mesh_objects().count(), 2);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_textmesh_extract_structural() {
        let dir = tempdir();
        create_textmesh_assets_fixture(&dir);
        let plugin = UnityPlugin::new();
        let entries = plugin.extract(&dir).unwrap();
        let tm: Vec<_> = entries
            .iter()
            .filter(|e| e.tags.iter().any(|t| t == "textmesh"))
            .collect();
        assert!(
            !tm.is_empty(),
            "expected TextMesh entries, got {:?}",
            entries.iter().map(|e| (&e.id, &e.source)).collect::<Vec<_>>()
        );
        assert!(tm.iter().any(|e| e.id.starts_with("textmesh/")));
        assert!(tm.iter().any(|e| e.source == "Hello, world!"));
        assert_eq!(
            tm[0]
                .metadata
                .get("extraction_method")
                .and_then(|v| v.as_str()),
            Some("textmesh")
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_textmesh_inject_shorter() {
        let dir = tempdir();
        let assets = create_textmesh_assets_fixture(&dir);
        let plugin = UnityPlugin::new();
        let mut entries = plugin.extract(&dir).unwrap();
        for e in &mut entries {
            if e.tags.iter().any(|t| t == "textmesh") && e.source == "Hello, world!" {
                e.translation = Some("Hola!".into());
            }
        }
        let report = plugin.inject(&dir, &entries).unwrap();
        assert!(
            report.strings_written >= 1,
            "written={} skipped={} {:?}",
            report.strings_written,
            report.strings_skipped,
            report.warnings
        );
        let again = plugin.extract(&dir).unwrap();
        assert!(
            again
                .iter()
                .any(|e| e.tags.iter().any(|t| t == "textmesh") && e.source.starts_with("Hola!")),
            "re-extract: {:?}",
            again.iter().map(|e| &e.source).collect::<Vec<_>>()
        );
        let sf = SerializedFile::parse_path(&assets).unwrap();
        assert_eq!(sf.text_mesh_objects().count(), 1);
        let _ = fs::remove_dir_all(&dir);
    }

    fn create_guitext_assets_fixture(dir: &Path) -> PathBuf {
        let data_dir = dir.join("TestGame_Data");
        fs::create_dir_all(&data_dir).unwrap();
        fs::write(dir.join("UnityPlayer.dll"), b"fake").unwrap();
        let bytes = crate::unity_serialized::write_v17_guitext_fixture("Press Start");
        let path = data_dir.join("sharedassets0.assets");
        fs::write(&path, bytes).unwrap();
        path
    }

    #[test]
    fn test_guitext_extract_structural() {
        let dir = tempdir();
        create_guitext_assets_fixture(&dir);
        let plugin = UnityPlugin::new();
        let entries = plugin.extract(&dir).unwrap();
        let gt: Vec<_> = entries
            .iter()
            .filter(|e| e.tags.iter().any(|t| t == "guitext"))
            .collect();
        assert!(
            !gt.is_empty(),
            "expected GUIText entries, got {:?}",
            entries.iter().map(|e| (&e.id, &e.source)).collect::<Vec<_>>()
        );
        assert!(gt.iter().any(|e| e.id.starts_with("guitext/")));
        assert!(gt.iter().any(|e| e.source == "Press Start"));
        assert_eq!(
            gt[0]
                .metadata
                .get("extraction_method")
                .and_then(|v| v.as_str()),
            Some("guitext")
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_guitext_inject_shorter() {
        let dir = tempdir();
        let assets = create_guitext_assets_fixture(&dir);
        let plugin = UnityPlugin::new();
        let mut entries = plugin.extract(&dir).unwrap();
        for e in &mut entries {
            if e.tags.iter().any(|t| t == "guitext") && e.source == "Press Start" {
                e.translation = Some("Pulsa".into());
            }
        }
        let report = plugin.inject(&dir, &entries).unwrap();
        assert!(
            report.strings_written >= 1,
            "written={} skipped={} {:?}",
            report.strings_written,
            report.strings_skipped,
            report.warnings
        );
        let again = plugin.extract(&dir).unwrap();
        assert!(
            again
                .iter()
                .any(|e| e.tags.iter().any(|t| t == "guitext") && e.source.starts_with("Pulsa")),
            "re-extract: {:?}",
            again.iter().map(|e| &e.source).collect::<Vec<_>>()
        );
        let sf = SerializedFile::parse_path(&assets).unwrap();
        assert_eq!(sf.gui_text_objects().count(), 1);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_textasset_inject_oversize_skips() {
        let dir = tempdir();
        create_textasset_assets_fixture(&dir);
        let plugin = UnityPlugin::new();
        let mut entries = plugin.extract(&dir).unwrap();
        for e in &mut entries {
            if e.tags.iter().any(|t| t == "textasset") {
                e.translation = Some(
                    "This translation is intentionally far longer than the original TextAsset script"
                        .into(),
                );
            }
        }
        let report = plugin.inject(&dir, &entries).unwrap();
        assert_eq!(report.strings_written, 0);
        assert!(report.strings_skipped >= 1);
        assert!(
            report
                .warnings
                .iter()
                .any(|w| w.contains("longer") || w.contains("skipped because")),
            "{:?}",
            report.warnings
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_garbage_assets_falls_back_to_heuristic() {
        let dir = tempdir();
        let data_dir = dir.join("TestGame_Data");
        fs::create_dir_all(&data_dir).unwrap();
        fs::write(dir.join("UnityPlayer.dll"), b"fake").unwrap();
        // Not a SerializedFile — pure heuristic payload
        let mut data = vec![0u8; 32];
        let s = b"Press any key to continue";
        data.extend_from_slice(&(s.len() as u32).to_le_bytes());
        data.extend_from_slice(s);
        data.extend_from_slice(&[0, 0, 0, 0]);
        fs::write(data_dir.join("resources.assets"), &data).unwrap();
        let plugin = UnityPlugin::new();
        let entries = plugin.extract(&dir).unwrap();
        assert!(
            entries.iter().any(|e| e.source.contains("Press any key")),
            "heuristic fallback should still find strings: {:?}",
            entries.iter().map(|e| &e.source).collect::<Vec<_>>()
        );
        let _ = fs::remove_dir_all(&dir);
    }
}
