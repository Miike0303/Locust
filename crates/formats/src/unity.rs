use std::collections::HashMap;
use std::path::{Path, PathBuf};

use locust_core::error::{LocustError, Result};
use locust_core::extraction::{FormatPlugin, InjectionReport};
use locust_core::models::{OutputMode, StringEntry};
use locust_core::textasset_group::{self, GroupKind};

use crate::unity_serialized::{
    is_binary_looking_script, is_textasset_script_worth_extracting,
    looks_like_assembly_qualified_type, looks_like_code_identifier, looks_like_lorem_ipsum,
    looks_like_naninovel_script, looks_like_unity_asset_path, rewrite_text_asset_script_inplace,
    SerializedFile,
};

/// Plugin for Unity Engine games.
/// Supports:
/// 1. Text-based VN scripts (SCRIPTS~/ directory with .txt dialogue files)
/// 2. Structural TextAsset + MonoBehaviour + TextMesh + GUIText extraction from
///    SerializedFile `.assets` / `level*` (type-tree blobs skipped; no full type-tree walk)
/// 3. Heuristic length-prefixed UTF-8 scan of the same files (skips structural ranges)
/// 4. UnityFS `*_Data/data.unity3d` bundles (contained SerializedFiles as virtual paths)
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
                if dir_name.contains("SCRIPT")
                    || dir_name.contains("script")
                    || dir_name.contains("Script")
                {
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
            .filter_entry(crate::discovery::is_game_entry)
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

                // Declarations have a single display slot, not a dialogue body.
                if is_character_directive(line) {
                    if let Some(slot) = character_display_slot(line) {
                        let id = format!("{}#{}", filename, line_num);
                        let mut entry = StringEntry::new(id, slot.value, fpath.to_path_buf());
                        entry.tags = vec!["character_display_name".into()];
                        entry.context = Some(slot.character.into());
                        for (key, value) in [
                            ("record_kind", "character_display_name"),
                            ("character_id", slot.character),
                            ("declaration_prefix", &line[..slot.start]),
                            ("declaration_suffix", &line[slot.end..]),
                        ] {
                            entry.metadata.insert(key.into(), serde_json::json!(value));
                        }
                        entry
                            .metadata
                            .insert("quoted_value_start".into(), serde_json::json!(slot.start));
                        entry
                            .metadata
                            .insert("quoted_value_end".into(), serde_json::json!(slot.end));
                        all.push(entry);
                    }
                    continue;
                }

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
                if let Some(dialogue) = extract_vn_dialogue(trimmed) {
                    let text = dialogue.text;
                    // A recognized dialogue record needs only one visible
                    // character. Keep controls verbatim in the source; ignore
                    // them only when rejecting empty/control-only bodies.
                    if !vn_visible_text(text).is_empty() {
                        let id = format!("{}#{}", filename, line_num);
                        let mut entry = StringEntry::new(&id, text, fpath.to_path_buf());
                        entry.tags = vec!["dialogue".to_string()];
                        entry.context = Some(dialogue.character.to_string());
                        entry.metadata.insert(
                            "vn_engine_prefix".to_string(),
                            serde_json::Value::String(dialogue.engine_prefix.to_string()),
                        );
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

        Ok(all)
    }

    fn inject_text_scripts(_path: &Path, entries: &[&StringEntry]) -> Result<InjectionReport> {
        let mut report = empty_injection_report();
        let mut by_file: HashMap<PathBuf, Vec<&StringEntry>> = HashMap::new();
        for entry in entries {
            if entry_needs_write(entry, &mut report) {
                by_file
                    .entry(entry.file_path.clone())
                    .or_default()
                    .push(entry);
            }
        }
        for (file_path, file_entries) in by_file {
            if !file_path.is_file() {
                report.skip("target_missing", file_entries.len());
                continue;
            }
            let content = std::fs::read_to_string(&file_path)?;
            // Preserve original newline style and final newline while editing.
            let mut lines: Vec<String> =
                content.split_inclusive('\n').map(str::to_string).collect();
            let filename = file_path.file_name().unwrap_or_default().to_string_lossy();
            let mut modified = false;
            for entry in file_entries {
                let Some(line_num) = entry
                    .metadata
                    .get("unity_local_id")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or(&entry.id)
                    .strip_prefix(&format!("{filename}#"))
                    .and_then(|n| n.parse::<usize>().ok())
                    .and_then(|n| n.checked_sub(1))
                else {
                    report.skip("error", 1);
                    continue;
                };
                let Some(line) = lines.get_mut(line_num) else {
                    report.skip("target_missing", 1);
                    continue;
                };
                let body = line.trim_end_matches(['\r', '\n']);
                let ending = &line[body.len()..];
                let trimmed = body.trim();
                let translation = entry.translation.as_deref().unwrap();
                if translation.contains(['\r', '\n']) {
                    report.skip("error", 1);
                    continue;
                }
                let declaration = entry.metadata.get("record_kind").and_then(|v| v.as_str())
                    == Some("character_display_name");
                let replacement = if declaration {
                    let Some(slot) = character_display_slot(body) else {
                        report.skip("source_changed", 1);
                        continue;
                    };
                    if entry.injection_source().ok() != Some(slot.value)
                        || metadata_usize(entry, "quoted_value_start") != Some(slot.start)
                        || metadata_usize(entry, "quoted_value_end") != Some(slot.end)
                        || entry.metadata.get("character_id").and_then(|v| v.as_str())
                            != Some(slot.character)
                        || entry
                            .metadata
                            .get("declaration_prefix")
                            .and_then(|v| v.as_str())
                            != Some(&body[..slot.start])
                        || entry
                            .metadata
                            .get("declaration_suffix")
                            .and_then(|v| v.as_str())
                            != Some(&body[slot.end..])
                    {
                        report.skip("source_changed", 1);
                        continue;
                    }
                    if !safe_quoted_value(translation) {
                        report.skip("error", 1);
                        continue;
                    }
                    if vn_control_tokens(slot.value) != vn_control_tokens(translation) {
                        report.skip("unsafe_controls", 1);
                        continue;
                    }
                    format!("{}{translation}{}", &body[..slot.start], &body[slot.end..])
                } else if is_character_directive(body) {
                    // Old or malformed rows cannot use the dialogue route here.
                    report.skip("invalid_target", 1);
                    continue;
                } else if trimmed.starts_with("button ") {
                    if extract_quoted_in_line(trimmed) != Some(entry.source.as_str()) {
                        report.skip("source_changed", 1);
                        continue;
                    }
                    if translation.contains('"') {
                        report.skip("error", 1);
                        continue;
                    }
                    if vn_control_tokens(&entry.source) != vn_control_tokens(translation) {
                        report.skip("unsafe_controls", 1);
                        continue;
                    }
                    body.replacen(
                        &format!("\"{}\"", entry.source),
                        &format!("\"{translation}\""),
                        1,
                    )
                } else if let Some(dialogue) = extract_vn_dialogue(trimmed) {
                    let text = dialogue.text;
                    // Exact rich-source matching also rejects legacy stripped
                    // entries instead of silently replacing formatted dialogue.
                    if entry.injection_source().ok() != Some(text)
                        || entry
                            .context
                            .as_deref()
                            .is_some_and(|expected| expected != dialogue.character)
                        || match entry.metadata.get("vn_engine_prefix") {
                            Some(expected) => expected.as_str() != Some(dialogue.engine_prefix),
                            // Old entries can still target plain dialogue, but
                            // cannot vouch for a previously unextracted prefix.
                            None => !dialogue.engine_prefix.trim().is_empty(),
                        }
                    {
                        report.skip("source_changed", 1);
                        continue;
                    }
                    if vn_control_tokens(text) != vn_control_tokens(translation) {
                        report.skip("unsafe_controls", 1);
                        continue;
                    }
                    // Replace only the dialogue span; retain indentation,
                    // speaker spacing, trailing whitespace and the line ending.
                    let start = body.len() - body.trim_start().len() + trimmed.len() - text.len();
                    format!(
                        "{}{translation}{}",
                        &body[..start],
                        &body[start + text.len()..]
                    )
                } else {
                    report.skip("source_changed", 1);
                    continue;
                };
                *line = format!("{replacement}{ending}");
                report.strings_written += 1;
                modified = true;
            }
            if modified {
                std::fs::write(&file_path, lines.concat())?;
                report.files_modified += 1;
                report.files_written.push(file_path);
            }
        }
        Ok(report)
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
            .filter_entry(crate::discovery::is_game_entry)
            .filter_map(|e| e.ok())
        {
            let p = entry.path();
            if p.is_file() && is_unity_serialized_candidate(p) {
                assets.push(p.to_path_buf());
            }
        }
        assets
    }

    fn find_unityfs_files(path: &Path) -> Vec<PathBuf> {
        let mut out = Vec::new();
        if path.is_file() {
            if crate::unity_fs::is_unity_fs_file(path) {
                out.push(path.to_path_buf());
            }
            return out;
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
                return out;
            }
        } else {
            return out;
        };
        let candidate = data_dir.join("data.unity3d");
        if crate::unity_fs::is_unity_fs_file(&candidate) {
            out.push(candidate);
        }
        out
    }

    /// Walk parents until an existing UnityFS file is found; remainder is the node path.
    fn resolve_unityfs_virtual_path(file_path: &Path) -> Option<(PathBuf, String)> {
        let mut current = file_path.parent()?;
        loop {
            if current.is_file() && crate::unity_fs::is_unity_fs_file(current) {
                let rel = file_path.strip_prefix(current).ok()?;
                let node = rel.to_string_lossy().replace("\\", "/");
                if node.is_empty() {
                    return None;
                }
                return Some((current.to_path_buf(), node));
            }
            current = current.parent()?;
        }
    }

    fn inject_serialized_bytes(
        bytes: &mut Vec<u8>,
        file_entries: &[&StringEntry],
        label: &str,
        report: &mut InjectionReport,
    ) -> bool {
        let mut modified = false;
        let (rewriteable, technical_ranges): (_, Vec<_>) =
            SerializedFile::parse(bytes.clone(), label)
                .map(|sf| {
                    let ranges = sf.forbidden_injection_byte_ranges();
                    (sf.rewriteable_text_assets().unwrap_or_default(), ranges)
                })
                .unwrap_or_default();
        let mut textasset_plan = std::collections::BTreeMap::new();
        let mut planned_writes = 0usize;
        let active: Vec<&StringEntry> = file_entries
            .iter()
            .copied()
            .filter(|entry| entry_needs_write(entry, report))
            .collect();
        let file_entries = active.as_slice();
        // CSV cells and locale lines share one binary slot. Only changed, valid
        // entries count as writes; preserve every other line from the payload.
        let mut groups: HashMap<(i64, bool), Vec<&StringEntry>> = HashMap::new();
        for entry in file_entries
            .iter()
            .copied()
            .filter(|e| is_textasset_csv_cell_entry(e) || is_textasset_loc_line_entry(e))
        {
            let Some(path_id) = entry.metadata.get("path_id").and_then(|v| v.as_i64()) else {
                report.skip("error", 1);
                continue;
            };
            groups
                .entry((path_id, is_textasset_csv_cell_entry(entry)))
                .or_default()
                .push(entry);
        }
        for ((path_id, csv), group) in groups {
            let head = group[0];
            let Some(off) = metadata_usize(head, "textasset_script_offset") else {
                report.skip("error", group.len());
                continue;
            };
            let Some(len) = metadata_usize(head, "textasset_script_byte_len") else {
                report.skip("error", group.len());
                continue;
            };
            let Some(original) =
                slot_payload(bytes, off, len).and_then(|b| std::str::from_utf8(b).ok())
            else {
                report.skip("source_changed", group.len());
                continue;
            };
            let (mut rebuilt, changed) = apply_table_translations(original, &group, csv, report);
            if changed == 0 {
                continue;
            }
            if group.iter().all(|e| has_textasset_rewrite_capability(e)) {
                if !rewriteable
                    .get(&path_id)
                    .is_some_and(|ta| ta.script_len_offset == off && ta.script_byte_len == len)
                {
                    report.skip("invalid_target", changed);
                    report.warnings.push(format!("TextAsset {path_id}: resize capability cannot be verified against current object layout"));
                    continue;
                }
                if rebuilt.len() > crate::unity_serialized::MAX_REBUILT_TEXT_ASSET_BYTES {
                    report.skip("too_long", changed);
                } else if let std::collections::btree_map::Entry::Vacant(slot) =
                    textasset_plan.entry(path_id)
                {
                    slot.insert(rebuilt);
                    planned_writes += changed;
                } else {
                    report.skip("ambiguous_target", changed);
                }
                continue;
            }
            // A prior rewrite may have left spaces reserved at the end of the
            // shared slot. Consume only those needed for expansion, not rows.
            if rebuilt.len() > len {
                let excess = rebuilt.len() - len;
                let padding = rebuilt.len() - rebuilt.trim_end_matches(' ').len();
                if padding >= excess {
                    rebuilt.truncate(len);
                }
            }
            if rebuilt.len() > len {
                report.skip("too_long", changed);
                continue;
            }
            match rewrite_text_asset_script_inplace(bytes, off, len, &rebuilt, label) {
                Ok(()) => {
                    report.strings_written += changed;
                    modified = true;
                }
                Err(error) => {
                    report
                        .warnings
                        .push(format!("TextAsset table rewrite {}: {error}", head.id));
                    report.skip("error", changed);
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
                    report.skip("untranslated", 1);
                    continue;
                }
            };
            if translation == &entry.source {
                report.skip("unchanged", 1);
                continue;
            }
            let (off_key, len_key, kind) = if is_textasset_entry(entry) {
                (
                    "textasset_script_offset",
                    "textasset_script_byte_len",
                    "TextAsset",
                )
            } else if is_textmesh_entry(entry) {
                ("textmesh_text_offset", "textmesh_text_byte_len", "TextMesh")
            } else if is_guitext_entry(entry) {
                ("guitext_text_offset", "guitext_text_byte_len", "GUIText")
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
                report
                    .warnings
                    .push(format!("{kind} entry '{}' missing {off_key}", entry.id));
                report.skip("error", 1);
                continue;
            };
            let orig_len = entry
                .metadata
                .get(len_key)
                .and_then(|v| v.as_u64())
                .map(|u| u as usize)
                .unwrap_or(entry.source.len());
            if is_textasset_entry(entry) && has_textasset_rewrite_capability(entry) {
                let path_id = entry.metadata.get("path_id").and_then(|v| v.as_i64());
                let target = path_id.and_then(|id| rewriteable.get(&id));
                if let Some(ta) = target.filter(|ta| {
                    ta.script_len_offset == script_off && ta.script_byte_len == orig_len
                }) {
                    if ta.script != entry.source {
                        report.skip("source_changed", 1);
                    } else if translation.len()
                        > crate::unity_serialized::MAX_REBUILT_TEXT_ASSET_BYTES
                    {
                        report.skip("too_long", 1);
                    } else if let std::collections::btree_map::Entry::Vacant(slot) =
                        textasset_plan.entry(ta.path_id)
                    {
                        slot.insert(translation.clone());
                        planned_writes += 1;
                    } else {
                        report.skip("ambiguous_target", 1);
                    }
                    continue;
                }
                report.skip("invalid_target", 1);
                report.warnings.push(format!("TextAsset {}: resize capability cannot be verified against current object layout", entry.id));
                continue;
            }
            if translation.len() > orig_len {
                if report.skip_reasons.get("too_long").copied().unwrap_or(0) < 5 {
                    report.warnings.push(format!(
                            "translation for '{}' longer than original {kind} string ({} > {} bytes), skipping",
                            entry.id,
                            translation.len(),
                            orig_len
                        ));
                }
                report.skip("too_long", 1);
                continue;
            }
            if slot_payload(bytes, script_off, orig_len) != Some(entry.source.as_bytes()) {
                report.skip("source_changed", 1);
                continue;
            }
            match rewrite_text_asset_script_inplace(bytes, script_off, orig_len, translation, label)
            {
                Ok(()) => {
                    report.strings_written += 1;
                    modified = true;
                }
                Err(e) => {
                    report
                        .warnings
                        .push(format!("{kind} rewrite {}: {e}", entry.id));
                    report.skip("error", 1);
                }
            }
        }

        // ── Heuristic length-prefixed inject (non-structural entries) ──
        struct Work {
            id: String,
            needle: Vec<u8>,
            /// Alternate endian needle when metadata did not pin endianness.
            alt_needle: Option<Vec<u8>>,
            endian: LengthEndian,
            alt_endian: Option<LengthEndian>,
            explicit_offset: Option<usize>,
            trans_bytes: Vec<u8>,
            orig_payload_len: usize,
        }
        let mut work: Vec<Work> = Vec::new();
        for entry in file_entries.iter().filter(|e| !is_structural_entry(e)) {
            let translation = match &entry.translation {
                Some(t) => t,
                None => {
                    report.skip("untranslated", 1);
                    continue;
                }
            };
            let orig_bytes = entry.source.as_bytes();
            let trans_bytes = translation.as_bytes();
            if trans_bytes == orig_bytes {
                report.skip("unchanged", 1);
                continue;
            }
            if trans_bytes.len() > orig_bytes.len() {
                if report.skip_reasons.get("too_long").copied().unwrap_or(0) < 5 {
                    report.warnings.push(format!(
                        "translation for '{}' longer than original ({} > {} bytes), skipping",
                        entry.id,
                        trans_bytes.len(),
                        orig_bytes.len()
                    ));
                }
                report.skip("too_long", 1);
                continue;
            }
            let pinned = entry
                .metadata
                .get("length_endian")
                .and_then(|v| v.as_str())
                .and_then(LengthEndian::from_meta);
            let explicit_offset = match entry.metadata.get("binary_offset") {
                Some(value) => {
                    let Some(offset) = value.as_u64().and_then(|u| usize::try_from(u).ok()) else {
                        report.skip("invalid_target", 1);
                        continue;
                    };
                    if pinned.is_none() {
                        report.skip("invalid_target", 1);
                        continue;
                    }
                    Some(offset)
                }
                None => None,
            };
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
                id: entry.id.clone(),
                needle,
                alt_needle,
                endian,
                alt_endian,
                explicit_offset,
                trans_bytes: trans_bytes.to_vec(),
                orig_payload_len: orig_bytes.len(),
            });
        }

        if !work.is_empty() {
            // Resolve every target against the same pre-write image. New entries
            // pin their exact prefix offset; legacy entries are safe only when
            // their complete length+payload needle occurs exactly once.
            let patterns: Vec<&[u8]> = work
                .iter()
                .flat_map(|w| [w.needle.as_slice(), w.alt_needle.as_deref().unwrap_or(&[])])
                .collect();
            let matches = crate::binary_search::find_all_matches(bytes, &patterns);
            let resolved: Vec<HeuristicTarget> = work
                .iter()
                .enumerate()
                .map(|(i, w)| {
                    if let Some(pos) = w.explicit_offset {
                        return if pos
                            .checked_add(w.needle.len())
                            .and_then(|end| bytes.get(pos..end))
                            == Some(w.needle.as_slice())
                        {
                            HeuristicTarget::Found(pos, w.endian)
                        } else {
                            HeuristicTarget::Missing
                        };
                    }
                    let primary = &matches[i * 2];
                    let alternate = &matches[i * 2 + 1];
                    // Repeated technical names (e.g. a font's base name and
                    // family alias) are still invalid, even for offset-less
                    // rows. Mixed display/technical matches remain ambiguous.
                    if !primary.is_empty() || !alternate.is_empty() {
                        let forbidden = |positions: &[usize], len: usize| {
                            positions.iter().all(|&pos| {
                                pos.checked_add(len)
                                    .is_some_and(|end| range_overlaps(&technical_ranges, pos, end))
                            })
                        };
                        if forbidden(primary, w.needle.len())
                            && forbidden(alternate, w.alt_needle.as_ref().map_or(0, Vec::len))
                        {
                            return HeuristicTarget::Invalid;
                        }
                    }
                    match primary.len() + alternate.len() {
                        0 => HeuristicTarget::Missing,
                        1 if primary.len() == 1 => HeuristicTarget::Found(primary[0], w.endian),
                        1 => HeuristicTarget::Found(alternate[0], w.alt_endian.unwrap()),
                        _ => HeuristicTarget::Ambiguous,
                    }
                })
                .collect();

            for (w, target) in work.iter().zip(resolved) {
                if let HeuristicTarget::Found(pos, used_endian) = target {
                    let expected = if used_endian == w.endian {
                        &w.needle
                    } else {
                        w.alt_needle.as_ref().unwrap()
                    };
                    let Some(end) = pos.checked_add(expected.len()) else {
                        report.skip("invalid_target", 1);
                        continue;
                    };
                    if bytes.get(pos..end) != Some(expected.as_slice()) {
                        report.skip("source_changed", 1);
                        continue;
                    }
                    // Older project databases may contain candidates emitted
                    // before technical classes were excluded during extraction.
                    // Their matching bytes still do not make a control binding,
                    // shader or managed type name a valid translation target.
                    if range_overlaps(&technical_ranges, pos, end) {
                        report.skip("invalid_target", 1);
                        report.warnings.push(format!(
                            "Unity entry '{}' points into a technical object; re-extract the project before translating",
                            w.id
                        ));
                        continue;
                    }
                    let new_len = w.trans_bytes.len() as u32;
                    bytes[pos..pos + 4].copy_from_slice(&used_endian.encode_u32(new_len));
                    bytes[pos + 4..pos + 4 + w.trans_bytes.len()].copy_from_slice(&w.trans_bytes);
                    for b in &mut bytes[pos + 4 + w.trans_bytes.len()..pos + 4 + w.orig_payload_len]
                    {
                        *b = 0;
                    }
                    report.strings_written += 1;
                    modified = true;
                } else if target == HeuristicTarget::Invalid {
                    report.skip("invalid_target", 1);
                } else if target == HeuristicTarget::Ambiguous {
                    report.skip("ambiguous_target", 1);
                } else {
                    if w.explicit_offset.is_some() {
                        tracing::warn!(entry_id = %w.id, "Unity heuristic target offset is invalid or stale");
                    }
                    report.skip("source_changed", 1);
                }
            }
        }

        // Fixed-offset edits above refer to the original image. Relayout is
        // deliberately last, including when multiple TextAssets change size.
        if !textasset_plan.is_empty() {
            match SerializedFile::parse(bytes.clone(), label)
                .and_then(|sf| {
                    for id in textasset_plan.keys() {
                        if sf.read_text_asset(*id)?.script != rewriteable[id].script {
                            return Err(crate::unity_serialized::SerializedError {
                                file: label.into(),
                                message: format!("TextAsset {id} changed during fixed-offset edits; overlapping writes refused"),
                            });
                        }
                    }
                    sf.rewrite_text_assets(&textasset_plan)
                })
            {
                Ok(rebuilt) => {
                    *bytes = rebuilt;
                    report.strings_written += planned_writes;
                    modified = true;
                }
                Err(error) => {
                    report
                        .warnings
                        .push(format!("TextAsset structural rebuild: {error}"));
                    report.skip("error", planned_writes);
                }
            }
        }
        modified
    }

    /// Structural TextAsset + heuristic scan (skipping TextAsset ranges).
    fn extract_strings_from_assets(
        bytes: &[u8],
        filename: &str,
        file_path: &Path,
    ) -> Vec<StringEntry> {
        let mut entries = Vec::new();
        // Keep newly enabled name rows out of the legacy heuristic ID counter.
        let mut display_name_entries = Vec::new();
        let mut skip_ranges: Vec<(usize, usize)> = Vec::new();

        match SerializedFile::parse(bytes.to_vec(), file_path) {
            Ok(sf) => {
                let rewriteable = sf.rewriteable_text_assets().unwrap_or_default();
                // Structural ranges + MonoScript/Shader (type names / HLSL noise).
                skip_ranges = sf.heuristic_skip_byte_ranges();
                for obj in sf.text_asset_objects() {
                    match sf.read_text_asset_object(obj) {
                        Ok(ta) => {
                            // The object's range is already excluded from the
                            // heuristic scan, even when its script is rejected.
                            if is_performance_test_config(&ta.name, &ta.script) {
                                continue;
                            }
                            let character_names = parse_character_names_lines(&ta.name, &ta.script);
                            let is_character_names = character_names.is_some();
                            if character_names.is_none()
                                && !is_unity_textasset_script_worth_extracting(&ta.script)
                            {
                                continue;
                            }
                            // Non-player assets by m_Name (TMP linebreak tables, SFX tech
                            // packs, or unstructured proper-name glossaries).
                            if is_non_player_textasset_name(&ta.name) && character_names.is_none() {
                                continue;
                            }
                            // Markdown / internal delivery notes mis-stored as TextAsset.
                            if looks_like_internal_tech_doc(&ta.script) {
                                continue;
                            }
                            // Naninovel / ICU locale name catalogs (`af: Afrikaans`, …) —
                            // not game UI; skip whole asset (BOXMAN ~233 rows).
                            if is_locale_catalog_script(&ta.script) {
                                continue;
                            }
                            // Simple CSV tables (header + data): text columns as cells.
                            if let Some(csv) = character_names
                                .is_none()
                                .then(|| parse_textasset_csv(&ta.script))
                                .flatten()
                            {
                                let newline = if ta.script.contains("\r\n") {
                                    "\r\n"
                                } else {
                                    "\n"
                                };
                                let mut group_entries = Vec::new();
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
                                    entry.tags =
                                        vec!["textasset".to_string(), "textasset_csv".to_string()];
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
                                        serde_json::Value::String("textasset_csv_cell".to_string()),
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
                                    entry
                                        .metadata
                                        .insert("csv_row".to_string(), serde_json::json!(cell.row));
                                    entry
                                        .metadata
                                        .insert("csv_col".to_string(), serde_json::json!(cell.col));
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
                                    // Shared m_Script blob: no per-cell byte budget.
                                    group_entries.push(entry);
                                }
                                textasset_group::attach_to_entries(
                                    &mut group_entries,
                                    GroupKind::Csv,
                                    &ta.script,
                                    ta.script_byte_len,
                                    format!(
                                        "unity-textasset:{}:{}:csv",
                                        file_path.to_string_lossy(),
                                        ta.path_id
                                    ),
                                );
                                if rewriteable.contains_key(&ta.path_id) {
                                    for entry in &mut group_entries {
                                        mark_textasset_rewrite(entry, sf.header.version);
                                    }
                                }
                                entries.extend(group_entries);
                                continue;
                            }
                            // Naninovel ManagedText / locale docs: split Key: Value lines
                            // so each UI string is a translateable row (inject rebuilds blob).
                            if let Some(lines) =
                                character_names.or_else(|| parse_textasset_loc_lines(&ta.script))
                            {
                                let newline = if ta.script.contains("\r\n") {
                                    "\r\n"
                                } else {
                                    "\n"
                                };
                                let mut group_entries = Vec::new();
                                for (line_index, loc) in lines.into_iter().enumerate() {
                                    let id =
                                        format!("textasset/{}/line/{}", ta.path_id, line_index);
                                    let mut entry = StringEntry::new(
                                        id,
                                        loc.value.clone(),
                                        file_path.to_path_buf(),
                                    );
                                    entry.tags =
                                        vec!["textasset".to_string(), "textasset_loc".to_string()];
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
                                        serde_json::Value::String("textasset_loc_line".to_string()),
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
                                    // Inject still pads the whole m_Script blob; do not set
                                    // char_limit — the group budget cannot be attributed per line.
                                    entry.metadata.insert(
                                        "binary_slot".to_string(),
                                        serde_json::Value::String("utf8".to_string()),
                                    );
                                    entry.metadata.insert(
                                        "line_value_byte_len".to_string(),
                                        serde_json::json!(loc.value.len()),
                                    );
                                    group_entries.push(entry);
                                }
                                textasset_group::attach_to_entries(
                                    &mut group_entries,
                                    GroupKind::LocLine,
                                    &ta.script,
                                    ta.script_byte_len,
                                    format!(
                                        "unity-textasset:{}:{}:loc_line",
                                        file_path.to_string_lossy(),
                                        ta.path_id
                                    ),
                                );
                                if rewriteable.contains_key(&ta.path_id) {
                                    for entry in &mut group_entries {
                                        mark_textasset_rewrite(entry, sf.header.version);
                                    }
                                }
                                if is_character_names {
                                    display_name_entries.extend(group_entries);
                                } else {
                                    entries.extend(group_entries);
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
                            entry
                                .metadata
                                .insert("path_id".to_string(), serde_json::json!(ta.path_id));
                            entry
                                .metadata
                                .insert("name".to_string(), serde_json::Value::String(ta.name));
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
                            entry.char_limit = Some(entry.source.len());
                            if rewriteable.contains_key(&ta.path_id) {
                                mark_textasset_rewrite(&mut entry, sf.header.version);
                            }
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
                    match sf.read_mono_strings_object(obj) {
                        Ok(fields) => {
                            for field in fields {
                                if !is_structural_unity_text(&field.text) {
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
                                entry.char_limit = Some(entry.source.len());
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
                    match sf.read_text_mesh_object(obj) {
                        Ok(tm) => {
                            if !is_structural_unity_text(&tm.text) {
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
                            entry
                                .metadata
                                .insert("path_id".to_string(), serde_json::json!(tm.path_id));
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
                            entry.char_limit = Some(entry.source.len());
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
                    match sf.read_gui_text_object(obj) {
                        Ok(gt) => {
                            if !is_structural_unity_text(&gt.text) {
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
                            entry
                                .metadata
                                .insert("path_id".to_string(), serde_json::json!(gt.path_id));
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
                            entry.char_limit = Some(entry.source.len());
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
            entries.extend(display_name_entries);
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
                    entry.char_limit = Some(entry.source.len());
                    entry.metadata.insert(
                        "extraction_method".to_string(),
                        serde_json::Value::String("heuristic".to_string()),
                    );
                    entry.metadata.insert(
                        "length_endian".to_string(),
                        serde_json::Value::String(endian.as_meta().to_string()),
                    );
                    entry
                        .metadata
                        .insert("binary_offset".to_string(), serde_json::json!(i));
                    entries.push(entry);
                }
            }
            let aligned = (str_len + 3) & !3;
            i += 4 + aligned;
        }
        entries.extend(display_name_entries);
        entries
    }
}

/// Endianness of a Unity length-prefixed UTF-8 string field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LengthEndian {
    Little,
    Big,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum HeuristicTarget {
    Missing,
    Invalid,
    Ambiguous,
    Found(usize, LengthEndian),
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
            lower.starts_with("level") || lower == "globalgamemanagers" || lower == "resources"
        })
        .unwrap_or(false)
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

/// Only the known CharacterNames TextAsset's keyed display-value schema opts
/// out of glossary filtering. A loose list or a different identifier map does
/// not gain this capability. Short character IDs are not culture codes here.
fn parse_character_names_lines(name: &str, script: &str) -> Option<Vec<TextAssetLocLine>> {
    if !name.trim().eq_ignore_ascii_case("CharacterNames") {
        return None;
    }
    let mut out = Vec::new();
    for line in script.lines().filter(|line| !line.trim().is_empty()) {
        let (key, sep, value) = split_loc_kv(line)?;
        if key.is_empty()
            || !key.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_')
            || value.trim().is_empty()
            || value.chars().any(char::is_control)
            || looks_like_lorem_ipsum(value)
        {
            return None;
        }
        out.push(TextAssetLocLine {
            key: Some(key.into()),
            sep: sep.into(),
            value: value.into(),
            line_count: 0,
        });
    }
    let line_count = out.len();
    if line_count == 0 {
        return None;
    }
    for line in &mut out {
        line.line_count = line_count;
    }
    Some(out)
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
        if value.trim().is_empty()
            || !looks_like_loc_key(key)
            || looks_like_bcp47_locale_id(key)
            || looks_like_lorem_ipsum(value)
        {
            return None;
        }
        return Some(vec![TextAssetLocLine {
            key: Some(key.to_string()),
            sep: sep.to_string(),
            value: value.to_string(),
            line_count: 1,
        }]);
    }
    let kv_hits = non_empty
        .iter()
        .filter(|l| split_loc_kv(l).is_some())
        .count();
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
            // Designer placeholder preview copy.
            if looks_like_lorem_ipsum(value) {
                continue;
            }
            out.push(TextAssetLocLine {
                key: Some(key.to_string()),
                sep: sep.to_string(),
                value: value.to_string(),
                line_count,
            });
        } else {
            // Rare non-kv line in a loc doc — keep whole line for round-trip
            // unless it's pure placeholder filler.
            if looks_like_lorem_ipsum(line) {
                continue;
            }
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

/// Unity Performance Testing package assets are technical only when both the
/// owner name and the JSON schema identify settings or a captured test run.
fn is_performance_test_config(name: &str, script: &str) -> bool {
    if !matches!(
        name.trim().to_ascii_lowercase().as_str(),
        "performancetestrun"
            | "performancetestrunsettings"
            | "performancetestruninfo"
            | "performancetestconfig"
    ) {
        return false;
    }
    let Ok(serde_json::Value::Object(config)) = serde_json::from_str(script) else {
        return false;
    };
    config
        .get("MeasurementCount")
        .is_some_and(serde_json::Value::is_i64)
        || (config
            .get("TestSuite")
            .is_some_and(serde_json::Value::is_string)
            && config.get("Date").is_some_and(serde_json::Value::is_number)
            && ["Player", "Hardware", "Editor"]
                .iter()
                .all(|key| config.get(*key).is_some_and(serde_json::Value::is_object))
            && ["Dependencies", "Results"]
                .iter()
                .all(|key| config.get(*key).is_some_and(serde_json::Value::is_array)))
}

/// TextAsset `m_Name` that is never player-facing copy.
fn is_non_player_textasset_name(name: &str) -> bool {
    let n = name.trim();
    if n.is_empty() {
        return false;
    }
    let lower = n.to_ascii_lowercase();
    // TMP / ICU linebreak character-class tables (BOXMAN path_id 830/831).
    if lower.starts_with("linebreaking")
        || lower.contains("line breaking")
        || lower.ends_with("leading characters")
        || lower.ends_with("following characters")
    {
        return true;
    }
    // Internal SFX / pipeline delivery notes.
    if lower.contains("technical specifications") {
        return true;
    }
    // Unstructured proper-name lists remain glossary-only. The structurally
    // validated CharacterNames value table gets a narrow exception at extraction.
    if lower == "characternames" || lower.ends_with("character names") {
        return true;
    }
    false
}

/// Markdown-ish internal tech / delivery notes stored as TextAsset.
fn looks_like_internal_tech_doc(script: &str) -> bool {
    let t = script.trim();
    if t.len() < 40 {
        return false;
    }
    let upper = t.to_ascii_uppercase();
    if upper.contains("TECHNICAL SPECIFICATIONS") {
        return true;
    }
    // `**Date:**` / `**Format**` style packages.
    let bold_markers = t.matches("**").count();
    if bold_markers >= 4
        && (t.contains("**Date:**") || t.contains("**Format**") || t.contains("**Format:**"))
    {
        return true;
    }
    false
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
    !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_digit() || c == '-' || c == '+')
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

fn empty_injection_report() -> InjectionReport {
    InjectionReport {
        skip_reasons: Default::default(),
        files_modified: 0,
        strings_written: 0,
        strings_skipped: 0,
        warnings: Vec::new(),
        files_written: Vec::new(),
    }
}

fn entry_needs_write(entry: &StringEntry, report: &mut InjectionReport) -> bool {
    match entry.translation.as_deref() {
        None | Some("") => {
            report.skip("untranslated", 1);
            false
        }
        Some(t) if t == entry.source => {
            report.skip("unchanged", 1);
            false
        }
        Some(_) => true,
    }
}

fn has_textasset_rewrite_capability(entry: &StringEntry) -> bool {
    matches!(
        entry
            .metadata
            .get("extraction_method")
            .and_then(|v| v.as_str()),
        Some("textasset" | "textasset_loc_line" | "textasset_csv_cell")
    ) && entry
        .metadata
        .get("textasset_rewrite")
        .and_then(|v| v.as_str())
        == Some("serialized-v1")
        && entry
            .metadata
            .get("unity_serialized_version")
            .and_then(|v| v.as_u64())
            .is_some_and(|v| (17..=22).contains(&v))
}

fn mark_textasset_rewrite(entry: &mut StringEntry, version: u32) {
    entry.metadata.insert(
        "textasset_rewrite".into(),
        serde_json::json!("serialized-v1"),
    );
    entry.metadata.insert(
        "unity_serialized_version".into(),
        serde_json::json!(version),
    );
    entry.char_limit = None;
}

fn metadata_usize(entry: &StringEntry, key: &str) -> Option<usize> {
    entry.metadata.get(key)?.as_u64()?.try_into().ok()
}

fn slot_payload(bytes: &[u8], offset: usize, len: usize) -> Option<&[u8]> {
    let start = offset.checked_add(4)?;
    bytes.get(start..start.checked_add(len)?)
}

fn line_body(line: &str) -> &str {
    line.trim_end_matches(['\r', '\n'])
}

/// Edit only validated destinations, preserving untouched CSV/loc rows and
/// newline bytes. Accounting is per entry; shared-slot overflow is applied to
/// the returned valid count by the caller, not to already rejected entries.
fn apply_table_translations(
    original: &str,
    entries: &[&StringEntry],
    csv: bool,
    report: &mut InjectionReport,
) -> (String, usize) {
    let mut lines: Vec<String> = original.split_inclusive('\n').map(str::to_string).collect();
    let rows: Vec<usize> = lines
        .iter()
        .enumerate()
        .filter(|(_, line)| !line.trim().is_empty())
        .map(|(i, _)| i)
        .collect();
    let headers: Vec<String> = rows
        .first()
        .map(|i| {
            line_body(&lines[*i])
                .split(',')
                .map(|h| h.trim().to_string())
                .collect()
        })
        .unwrap_or_default();
    // Extraction indexes only included locale entries, not blank/excluded lines.
    // Resolve that index against the original before any translations are made.
    let loc = if csv {
        Vec::new()
    } else {
        parse_character_names_lines(
            entries[0]
                .metadata
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or_default(),
            original,
        )
        .or_else(|| parse_textasset_loc_lines(original))
        .unwrap_or_default()
    };
    let mut loc_rows = Vec::new();
    let mut next = 0;
    for item in &loc {
        let found = (next..lines.len()).find(|i| {
            let body = line_body(&lines[*i]);
            match (&item.key, split_loc_kv(body)) {
                (Some(key), Some((actual, sep, value))) => {
                    actual == key && sep == item.sep && value == item.value
                }
                (None, None) => body == item.value,
                _ => false,
            }
        });
        if let Some(i) = found {
            next = i + 1;
        }
        loc_rows.push(found);
    }
    let mut changed = 0;
    for entry in entries {
        if metadata_usize(entry, "textasset_script_offset")
            != metadata_usize(entries[0], "textasset_script_offset")
            || metadata_usize(entry, "textasset_script_byte_len")
                != metadata_usize(entries[0], "textasset_script_byte_len")
        {
            report.skip("error", 1);
            continue;
        }
        let result: std::result::Result<(usize, String), &'static str> = (|| {
            let translation = entry.translation.as_deref().ok_or("untranslated")?;
            if translation.contains(['\r', '\n']) {
                return Err("error");
            }
            let physical = entry.injection_source().map_err(|_| "error")?;
            if csv {
                if translation.contains([',', '"']) {
                    return Err("error");
                }
                let row = metadata_usize(entry, "csv_row").ok_or("error")?;
                let col = metadata_usize(entry, "csv_col").ok_or("error")?;
                if row == 0 {
                    return Err("error");
                }
                let i = *rows.get(row).ok_or("target_missing")?;
                let header = headers.get(col).ok_or("target_missing")?;
                if entry
                    .metadata
                    .get("csv_header")
                    .and_then(|v| v.as_str())
                    .is_some_and(|expected| expected != header)
                {
                    return Err("source_changed");
                }
                let mut cells: Vec<String> = line_body(&lines[i])
                    .split(',')
                    .map(str::to_string)
                    .collect();
                if cells.len() != headers.len() {
                    return Err("source_changed");
                }
                let cell = cells.get_mut(col).ok_or("target_missing")?;
                if cell.trim() != physical {
                    return Err("source_changed");
                }
                let leading = &cell[..cell.len() - cell.trim_start().len()];
                let trailing = &cell[cell.trim_end().len()..];
                *cell = format!("{leading}{translation}{trailing}");
                Ok((i, cells.join(",")))
            } else {
                let index = metadata_usize(entry, "line_index").ok_or("error")?;
                let i = loc_rows
                    .get(index)
                    .copied()
                    .flatten()
                    .ok_or("target_missing")?;
                let body = line_body(&lines[i]);
                let expected_key = entry.metadata.get("loc_key").and_then(|v| v.as_str());
                match (expected_key, split_loc_kv(body)) {
                    (Some(key), Some((actual, _sep, value))) if actual == key => {
                        if value != physical {
                            return Err("source_changed");
                        }
                        Ok((
                            i,
                            format!("{}{translation}", &body[..body.len() - value.len()]),
                        ))
                    }
                    (None, None) => {
                        if body != physical {
                            return Err("source_changed");
                        }
                        Ok((i, translation.to_string()))
                    }
                    _ => Err("source_changed"),
                }
            }
        })();
        match result {
            Ok((i, body)) => {
                let ending = &lines[i][line_body(&lines[i]).len()..];
                lines[i] = format!("{body}{ending}");
                changed += 1;
            }
            Err(reason) => report.skip(reason, 1),
        }
    }
    (lines.concat(), changed)
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
    if text.is_empty() {
        None
    } else {
        Some(text)
    }
}

struct CharacterDisplaySlot<'a> {
    character: &'a str,
    value: &'a str,
    start: usize,
    end: usize,
}

fn is_character_directive(line: &str) -> bool {
    line.trim_start()
        .strip_prefix("character")
        .is_some_and(|rest| rest.is_empty() || rest.starts_with(char::is_whitespace))
}

/// Locate the first quoted field after the ID and optional unquoted attributes.
/// Keep escapes verbatim; a backslash protects exactly the following character.
fn character_display_slot(line: &str) -> Option<CharacterDisplaySlot<'_>> {
    if !is_character_directive(line) {
        return None;
    }
    let rest = line.trim_start().strip_prefix("character")?.trim_start();
    let id_end = rest.find(char::is_whitespace)?;
    let character = &rest[..id_end];
    if character.is_empty()
        || !character
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'_')
    {
        return None;
    }
    let fields = &rest[id_end..];
    let quote = fields.find('"')?;
    // Quotes in comments and malformed unquoted escapes are not display fields.
    if fields[..quote].contains(['#', '\\']) {
        return None;
    }
    let start = line.len() - fields.len() + quote + 1;
    let mut escaped = false;
    for (offset, ch) in line[start..].char_indices() {
        if escaped {
            escaped = false;
        } else if ch == '\\' {
            escaped = true;
        } else if ch == '"' {
            let end = start + offset;
            let value = &line[start..end];
            return (!value.is_empty() && safe_quoted_value(value)).then_some(
                CharacterDisplaySlot {
                    character,
                    value,
                    start,
                    end,
                },
            );
        }
    }
    None
}

fn safe_quoted_value(value: &str) -> bool {
    let mut escaped = false;
    for ch in value.chars() {
        if ch.is_control() {
            return false;
        }
        if escaped {
            escaped = false;
        } else if ch == '\\' {
            escaped = true;
        } else if ch == '"' {
            return false;
        }
    }
    !escaped
}

struct VnDialogue<'a> {
    character: &'a str,
    // Exact spacing after the speaker, including any sprite/emotion modifiers.
    engine_prefix: &'a str,
    text: &'a str,
}

/// Parse `CharID [+Sprite ... -Sprite ...] Dialogue text here` once for both
/// extraction and injection. Only whitespace-delimited identifiers are engine
/// modifiers; attached punctuation is ambiguous and remains unsupported.
fn extract_vn_dialogue(line: &str) -> Option<VnDialogue<'_>> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return None;
    }

    let space_pos = trimmed.find(char::is_whitespace)?;
    let char_id = &trimmed[..space_pos];
    let mut text = trimmed[space_pos..].trim_start();

    if char_id.is_empty() || char_id.len() > 8 {
        return None;
    }
    if !char_id.chars().next()?.is_ascii_uppercase() {
        return None;
    }
    if !char_id
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_')
    {
        return None;
    }
    if text.starts_with('+') {
        while text.starts_with(['+', '-']) {
            let end = text.find(char::is_whitespace).unwrap_or(text.len());
            let modifier = &text[1..end];
            if modifier.is_empty()
                || !modifier
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || c == b'_')
            {
                return None;
            }
            text = text[end..].trim_start();
        }
    }
    if text.is_empty() || text.starts_with('{') {
        return None;
    }

    Some(VnDialogue {
        character: char_id,
        engine_prefix: &trimmed[space_pos..trimmed.len() - text.len()],
        text,
    })
}

/// Byte spans of protected VN escapes and Unity rich-text tags. Unknown escapes
/// are protected too: interpreting engine commands is not a translator's job.
fn vn_control_spans(text: &str) -> Vec<std::ops::Range<usize>> {
    let mut spans = Vec::new();
    let mut chars = text.char_indices().peekable();
    while let Some((start, ch)) = chars.next() {
        if ch == '\\' {
            let end = chars
                .next()
                .map_or(start + 1, |(pos, code)| pos + code.len_utf8());
            spans.push(start..end);
        } else if ch == '<'
            && chars
                .peek()
                .is_some_and(|(_, next)| next.is_ascii_alphabetic() || matches!(next, '/' | '#'))
        {
            // Link attributes may contain '>' inside quotes; protect the whole
            // tag, including its target, rather than just its tag name.
            let mut quote = None;
            let mut end = text.len();
            for (pos, next) in chars.by_ref() {
                if let Some(q) = quote {
                    if next == q {
                        quote = None;
                    }
                } else if matches!(next, '\'' | '"') {
                    quote = Some(next);
                } else if next == '>' {
                    end = pos + 1;
                    break;
                }
            }
            spans.push(start..end);
        }
    }
    spans
}

/// Preserve the source's exact control order/count (including style pairing).
/// Some shipped lines have open style toggles; retain those as authored too.
fn vn_control_tokens(text: &str) -> Vec<&str> {
    vn_control_spans(text)
        .into_iter()
        .map(|span| &text[span])
        .collect()
}

fn vn_visible_text(text: &str) -> String {
    let mut visible = String::new();
    let mut start = 0;
    for span in vn_control_spans(text) {
        visible.push_str(&text[start..span.start]);
        start = span.end;
    }
    visible.push_str(&text[start..]);
    visible.trim().to_string()
}

/// Safety floor for fields whose Unity class identifies them as text.
///
/// Structural readers already supply valid UTF-8 and bounded string framing, so
/// these fields may contain a single CJK character. Still reject malformed-looking
/// controls, replacement characters, binary payloads, and symbol-only artifacts.
fn is_structural_unity_text(text: &str) -> bool {
    let s = text.trim();
    !s.is_empty()
        && !s.contains('\u{FFFD}')
        && !s
            .chars()
            .any(|c| c.is_control() && !matches!(c, '\t' | '\n' | '\r'))
        && !is_binary_looking_script(s)
        && s.chars().any(char::is_alphanumeric)
}

/// The shared TextAsset filter deliberately rejects no-space CJK character-class
/// tables. That shape also describes real Japanese/Chinese prose, so recover only
/// strongly text-like CJK bodies after the shared noise checks reject them.
fn is_unity_textasset_script_worth_extracting(script: &str) -> bool {
    if !is_structural_unity_text(script) {
        return false;
    }
    is_textasset_script_worth_extracting(script) || looks_like_natural_cjk_text(script, 2)
}

/// Conservative evidence that a string is natural East-Asian text rather than a
/// random symbol table. This is intentionally script/range based: Japanese and
/// Chinese do not have reliable whitespace-delimited words.
fn looks_like_natural_cjk_text(text: &str, min_cjk: usize) -> bool {
    let s = text.trim();
    let total = s.chars().count();
    if total == 0 {
        return false;
    }
    let cjk = s.chars().filter(|&c| is_cjk_script_char(c)).count();
    cjk >= min_cjk && cjk * 100 >= total * 60
}

pub(crate) fn is_cjk_script_char(c: char) -> bool {
    matches!(
        c as u32,
        0x3041..=0x3096 // Hiragana letters
            | 0x30A1..=0x30FA // Katakana letters
            | 0x3105..=0x312F // Bopomofo
            | 0x31A0..=0x31BF // Bopomofo extended
            | 0x31F0..=0x31FF // Katakana phonetic extensions
            | 0x3400..=0x4DBF // CJK Extension A
            | 0x4E00..=0x9FFF // CJK unified ideographs
            | 0xAC00..=0xD7AF // Hangul syllables
            | 0xF900..=0xFAFF // CJK compatibility ideographs
            | 0x20000..=0x2FA1F // supplementary CJK ideographs
    )
}

fn is_unity_translatable(text: &str) -> bool {
    let s = text.trim();
    if s.is_empty() || s.contains('\u{FFFD}') {
        return false;
    }
    // Binary soup / mis-framed length prefixes leave controls in the payload.
    // Allow tab/CR/LF for multi-line dialogue; reject other controls.
    if s.chars()
        .any(|c| c.is_control() && !matches!(c, '\t' | '\n' | '\r'))
    {
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
    let letters = s.chars().filter(|c| c.is_alphabetic()).count();
    let cjk = s.chars().filter(|&c| is_cjk_script_char(c)).count();
    if cjk == 0 && s.len() < 5 {
        return false;
    }
    // Unicode letters are text evidence; unrelated symbols are not. Retain
    // the old noise threshold without restricting natural text to ASCII.
    let textual = s
        .chars()
        .filter(|c| {
            c.is_alphanumeric()
                || c.is_whitespace()
                || c.is_ascii_punctuation()
                || matches!(*c as u32, 0x0300..=0x036F | 0x2000..=0x206F | 0x3000..=0x303F | 0xFF01..=0xFF65)
                || matches!(*c, '¡' | '¿' | '«' | '»')
        })
        .count();
    if textual * 100 < total * 85 {
        return false;
    }
    // Byte heuristics need more evidence than structurally known text fields:
    // two CJK letters are enough for real UI (`設定`, `保存`), while one is too
    // easy to find by chance. Other scripts retain the existing three-letter bar.
    if (cjk > 0 && (cjk < 2 || cjk * 100 < total * 60)) || (cjk == 0 && letters < 3) {
        return false;
    }
    let has_space = s.contains(' ');
    // Long no-space Japanese/Chinese sentences are normal. Keep the legacy
    // single-token guard only for scripts where it is meaningful.
    if cjk == 0 && !has_space && total > 20 {
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
    // Shared asset-root paths (Tilemap/, Shaders/, Day/1…, UI/btn…).
    if looks_like_unity_asset_path(s) {
        return false;
    }
    // Shader #define soup: `BLENDMODES_MODE_MULTIPLY ETC1_EXTERNAL_ALPHA`
    if s.contains("BLENDMODES_") || s.contains("ETC1_EXTERNAL_ALPHA") {
        return false;
    }
    if s.contains('\\') && s.contains('.') {
        return false;
    }
    if s.chars()
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

fn unityfs_to_locust(err: crate::unity_fs::UnityFsError) -> LocustError {
    LocustError::ParseError {
        file: err.file,
        message: err.message,
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
        &[".assets", ".unity3d"]
    }

    fn supported_modes(&self) -> Vec<OutputMode> {
        vec![OutputMode::Replace]
    }

    fn detect(&self, path: &Path) -> bool {
        if path.is_file() {
            return path
                .extension()
                .is_some_and(|e| e == "assets" || e == "unity3d")
                || crate::unity_fs::is_unity_fs_file(path);
        }
        Self::has_unity_structure(path)
    }

    fn extract(&self, path: &Path) -> Result<Vec<StringEntry>> {
        let mut all = Vec::new();
        if let Some(scripts_dir) = Self::find_scripts_dir(path) {
            all.extend(Self::extract_text_scripts(&scripts_dir)?);
        }

        // Scripts and binary files can contain different physical text slots.
        let assets = Self::find_assets_files(path);
        let bundles = Self::find_unityfs_files(path);
        if all.is_empty() && assets.is_empty() && bundles.is_empty() {
            return Err(LocustError::ParseError {
                file: path.display().to_string(),
                message: "no script files, .assets files, or UnityFS bundles found".to_string(),
            });
        }

        for asset_file in &assets {
            let bytes = std::fs::read(asset_file)?;
            let filename = asset_file
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            all.extend(Self::extract_strings_from_assets(
                &bytes, &filename, asset_file,
            ));
        }
        for bundle in &bundles {
            let mut archive =
                crate::unity_fs::UnityFsArchive::parse_path(bundle).map_err(unityfs_to_locust)?;
            archive.discard_storage_cache();
            let supports_relayout = archive.supports_relayout();
            for node in &archive.nodes {
                if !crate::unity_fs::is_serialized_bundle_node(&node.path) {
                    continue;
                }
                let bytes = archive.node_bytes(node).map_err(unityfs_to_locust)?;
                let virtual_path = bundle.join(&node.path);
                let filename = {
                    let norm = node.path.replace("\\", "/");
                    norm.rsplit('/').next().unwrap_or("cab").to_string()
                };
                let mut node_entries =
                    Self::extract_strings_from_assets(bytes, &filename, &virtual_path);
                if !supports_relayout {
                    for entry in &mut node_entries {
                        entry.metadata.remove("textasset_rewrite");
                        entry.metadata.remove("unity_serialized_version");
                        if is_textasset_entry(entry) {
                            entry.char_limit = Some(entry.source.len());
                        }
                    }
                }
                all.extend(node_entries);
            }
        }
        // The database keys rows by ID alone. Preserve legacy IDs when unique,
        // but qualify every colliding local ID by its physical (or virtual) path.
        // Never merge identical sources from distinct slots.
        let mut counts = HashMap::new();
        for entry in &all {
            *counts.entry(entry.id.clone()).or_insert(0usize) += 1;
        }
        let mut occurrences = HashMap::new();
        for entry in &mut all {
            if counts[&entry.id] > 1 {
                let local_id = entry.id.clone();
                let relative = entry
                    .file_path
                    .strip_prefix(path)
                    .unwrap_or(&entry.file_path);
                let physical = relative.to_string_lossy().replace('\\', "/");
                let occurrence = occurrences
                    .entry((physical.clone(), local_id.clone()))
                    .or_insert(0usize);
                // JSON escaping makes the tuple unambiguous even for paths
                // containing separators used by local IDs.
                entry.id = format!(
                    "unity:{}:{local_id}",
                    serde_json::json!([physical, *occurrence])
                );
                *occurrence += 1;
                entry.metadata.insert(
                    "unity_local_id".to_string(),
                    serde_json::Value::String(local_id),
                );
            }
        }
        Ok(all)
    }

    fn inject(&self, path: &Path, entries: &[StringEntry]) -> Result<InjectionReport> {
        // Route per entry: a single external .txt must not hide binary entries.
        let (text, binary): (Vec<&StringEntry>, Vec<&StringEntry>) = entries
            .iter()
            .partition(|e| e.file_path.extension().is_some_and(|ext| ext == "txt"));
        let mut report = Self::inject_text_scripts(path, &text)?;
        let entries = binary.as_slice();

        let mut by_file: HashMap<PathBuf, Vec<&StringEntry>> = HashMap::new();
        for entry in entries {
            if entry_needs_write(entry, &mut report) {
                by_file
                    .entry(entry.file_path.clone())
                    .or_default()
                    .push(entry);
            }
        }

        let mut unityfs_jobs: HashMap<PathBuf, HashMap<String, Vec<&StringEntry>>> = HashMap::new();

        for (file_path, file_entries) in &by_file {
            if file_path.is_file() {
                let mut bytes = std::fs::read(file_path)?;
                let label = file_path.display().to_string();
                let modified =
                    Self::inject_serialized_bytes(&mut bytes, file_entries, &label, &mut report);
                if modified {
                    std::fs::write(file_path, &bytes)?;
                    report.files_modified += 1;
                    report.files_written.push(file_path.clone());
                }
                continue;
            }
            if let Some((bundle, node)) = Self::resolve_unityfs_virtual_path(file_path) {
                unityfs_jobs
                    .entry(bundle)
                    .or_default()
                    .entry(node)
                    .or_default()
                    .extend(file_entries.iter().copied());
            } else {
                for entry in file_entries {
                    if entry_needs_write(entry, &mut report) {
                        report.skip("target_missing", 1);
                    }
                }
            }
        }

        for (bundle_path, nodes) in unityfs_jobs {
            let mut archive = crate::unity_fs::UnityFsArchive::parse_path(&bundle_path)
                .map_err(unityfs_to_locust)?;
            let mut bundle_modified = false;
            for (node_path, node_entries) in nodes {
                let Some(node) = archive.node(&node_path).cloned() else {
                    report.warnings.push(format!(
                        "UnityFS node '{}' missing in {}",
                        node_path,
                        bundle_path.display()
                    ));
                    for entry in node_entries {
                        if entry_needs_write(entry, &mut report) {
                            report.skip("target_missing", 1);
                        }
                    }
                    continue;
                };
                let mut bytes = archive
                    .node_bytes(&node)
                    .map_err(unityfs_to_locust)?
                    .to_vec();
                let label = format!("{} / {node_path}", bundle_path.display());
                let modified =
                    Self::inject_serialized_bytes(&mut bytes, &node_entries, &label, &mut report);
                if modified {
                    if bytes.len() == node.size as usize {
                        archive
                            .replace_node(&node_path, &bytes)
                            .map_err(unityfs_to_locust)?;
                    } else {
                        archive
                            .resize_node(&node_path, &bytes)
                            .map_err(unityfs_to_locust)?;
                    }
                    bundle_modified = true;
                }
            }
            if bundle_modified {
                let out = archive.write_bytes().map_err(unityfs_to_locust)?;
                std::fs::write(&bundle_path, out)?;
                report.files_modified += 1;
                report.files_written.push(bundle_path);
            }
        }

        let length_skipped = report.skip_reasons.get("too_long").copied().unwrap_or(0);
        if length_skipped > 0 {
            report.warnings.push(format!(
                "{length_skipped} translation(s) skipped because the rebuilt Unity slot exceeds its UTF-8 byte budget."
            ));
        }
        report.classify_remaining_skips();
        Ok(report)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn vn_controls_fixture(script: &str) -> (tempfile::TempDir, PathBuf) {
        let fixture = tempfile::tempdir().unwrap();
        let scripts = fixture.path().join("Controls_Data/SCRIPTS~");
        fs::create_dir_all(&scripts).unwrap();
        let file = scripts.join("Dialogue.txt");
        fs::write(&file, script).unwrap();
        fs::write(
            scripts.join("Definitions.txt"),
            "# untouched\r\nversion 1.0  \r\n",
        )
        .unwrap();
        (fixture, file)
    }

    fn mixed_textasset_fixture() -> Vec<u8> {
        crate::unity_serialized::write_v17_fixture_ex("UI", "Welcome traveler!", None)
    }

    fn expected_textasset_slot(before: &[u8], entry: &StringEntry) -> Vec<u8> {
        let translation = entry.translation.as_ref().unwrap();
        assert_eq!(translation.len(), entry.source.len());
        let start = entry.metadata["textasset_script_offset"].as_u64().unwrap() as usize + 4;
        let mut expected = before.to_vec();
        expected[start..start + translation.len()].copy_from_slice(translation.as_bytes());
        expected
    }

    fn database_roundtrip(entries: &[StringEntry]) -> Vec<StringEntry> {
        use locust_core::database::{Database, EntryFilter};
        let db = Database::open_in_memory().unwrap();
        db.save_entries(entries).unwrap();
        let stored = db.get_entries(&EntryFilter::default()).unwrap();
        assert_eq!(
            stored.len(),
            entries.len(),
            "physical rows must survive the DB"
        );
        stored
    }

    #[test]
    fn scripts_and_binary_both_extract() {
        let before = "# untouched\r\n  CJ Hello \\bfriend\\b.  \r\nscene garden\r\n";
        let (fixture, script) = vn_controls_fixture(before);
        let definitions = script.parent().unwrap().join("Definitions.txt");
        let definitions_before = fs::read(&definitions).unwrap();
        let asset = fixture.path().join("Controls_Data/resources.assets");
        let binary_before = mixed_textasset_fixture();
        fs::write(&asset, &binary_before).unwrap();
        let plugin = UnityPlugin::new();
        let mut entries = plugin.extract(fixture.path()).unwrap();
        assert_eq!(entries.len(), 2, "{entries:?}");
        assert!(entries.iter().any(|e| e.file_path == script));
        assert!(entries.iter().any(|e| e.source == "Welcome traveler!"));
        for entry in &mut entries {
            entry.translation = Some(if entry.file_path == script {
                "Hola \\bamigo\\b.".into()
            } else {
                "Bienvenido amigo!".into()
            });
        }
        let entries = database_roundtrip(&entries);
        let binary = entries.iter().find(|e| e.file_path == asset).unwrap();
        let expected = expected_textasset_slot(&binary_before, binary);
        let report = plugin.inject(fixture.path(), &entries).unwrap();
        assert_diagnostic_totals(&report, 2);
        assert_eq!((report.strings_written, report.files_modified), (2, 2));
        assert_eq!(report.strings_skipped, 0);
        assert_eq!(
            fs::read(script).unwrap(),
            before
                .replace("Hello \\bfriend\\b.", "Hola \\bamigo\\b.")
                .as_bytes()
        );
        // Includes all headers, alignment padding, and the untouched dummy object.
        assert_eq!(fs::read(asset).unwrap(), expected);
        assert_eq!(fs::read(definitions).unwrap(), definitions_before);
    }

    #[test]
    fn scripts_and_binary_scripts_only_still_succeeds() {
        let (fixture, script) = vn_controls_fixture("CJ Welcome traveler!\n");
        let entries = UnityPlugin::new().extract(fixture.path()).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].file_path, script);
        assert_eq!(entries[0].id, "Dialogue.txt#1");
    }

    #[test]
    fn scripts_and_binary_binary_only_still_succeeds() {
        let fixture = tempfile::tempdir().unwrap();
        let data = fixture.path().join("Binary_Data");
        fs::create_dir(&data).unwrap();
        let asset = data.join("resources.assets");
        fs::write(&asset, mixed_textasset_fixture()).unwrap();
        let entries = UnityPlugin::new().extract(fixture.path()).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].file_path, asset);
        assert_eq!(entries[0].id, "textasset/1");
    }

    #[test]
    fn scripts_and_binary_repeated_physical_rows_survive_database_and_inject() {
        let before = "# untouched\r\nCJ Welcome traveler!\r\nscene garden\r\n";
        let (fixture, script) = vn_controls_fixture(before);
        let other_script = script.parent().unwrap().join("Vol2/Dialogue.txt");
        fs::create_dir(other_script.parent().unwrap()).unwrap();
        fs::write(&other_script, before).unwrap();
        let binary_before = mixed_textasset_fixture();
        let assets = ["resources.assets", "subdir/resources.assets"].map(|name| {
            let asset = fixture.path().join("Controls_Data").join(name);
            fs::create_dir_all(asset.parent().unwrap()).unwrap();
            fs::write(&asset, &binary_before).unwrap();
            asset
        });
        let plugin = UnityPlugin::new();
        let entries = plugin.extract(fixture.path()).unwrap();
        assert_eq!(entries.len(), 4, "same text must retain four physical rows");
        assert!(entries.iter().all(|e| e.source == "Welcome traveler!"));
        let first_ids: HashMap<_, _> = entries
            .iter()
            .map(|e| (e.file_path.clone(), e.id.clone()))
            .collect();
        let second_ids: HashMap<_, _> = plugin
            .extract(fixture.path())
            .unwrap()
            .into_iter()
            .map(|e| (e.file_path, e.id))
            .collect();
        assert_eq!(first_ids, second_ids, "IDs must be deterministic");
        let mut entries = database_roundtrip(&entries);
        for entry in &mut entries {
            entry.translation = Some(if entry.file_path == script {
                "Hola visitante!".into()
            } else if entry.file_path == other_script {
                "Saludos viajero!".into()
            } else {
                "Bienvenido amigo!".into()
            });
        }
        let report = plugin.inject(fixture.path(), &entries).unwrap();
        assert_diagnostic_totals(&report, 4);
        assert_eq!((report.strings_written, report.files_modified), (4, 4));
        for path in [script, other_script] {
            let entry = entries.iter().find(|e| e.file_path == path).unwrap();
            assert_eq!(
                fs::read(path).unwrap(),
                before
                    .replace(&entry.source, entry.translation.as_ref().unwrap())
                    .as_bytes()
            );
        }
        for path in assets {
            let entry = entries.iter().find(|e| e.file_path == path).unwrap();
            assert_eq!(
                fs::read(path).unwrap(),
                expected_textasset_slot(&binary_before, entry)
            );
        }
    }

    #[test]
    fn scripts_and_binary_unityfs_virtual_rows_survive_database_and_inject() {
        let (fixture, script) = vn_controls_fixture("CJ Welcome traveler!\r\n");
        let binary_before = mixed_textasset_fixture();
        let asset = fixture.path().join("Controls_Data/resources.assets");
        fs::write(&asset, &binary_before).unwrap();
        let bundle = fixture.path().join("Controls_Data/data.unity3d");
        let nodes = ["CAB-one", "nested/CAB-two"];
        let untouched = b"untouched resource payload";
        fs::write(
            &bundle,
            crate::unity_fs::build_test_bundle(
                &[
                    (nodes[0], &binary_before),
                    (nodes[1], &binary_before),
                    ("CAB-one.resS", untouched),
                ],
                true,
                8,
                true,
                false,
            ),
        )
        .unwrap();
        let plugin = UnityPlugin::new();
        let mut entries = database_roundtrip(&plugin.extract(fixture.path()).unwrap());
        assert_eq!(entries.len(), 4);
        for node in nodes {
            assert!(entries.iter().any(|e| e.file_path == bundle.join(node)));
        }
        for entry in &mut entries {
            entry.translation = Some("Bienvenido amigo!".into());
        }
        let report = plugin.inject(fixture.path(), &entries).unwrap();
        assert_diagnostic_totals(&report, 4);
        assert_eq!((report.strings_written, report.files_modified), (4, 3));
        assert_eq!(fs::read(script).unwrap(), b"CJ Bienvenido amigo!\r\n");
        let archive = crate::unity_fs::UnityFsArchive::parse_path(&bundle).unwrap();
        for node in nodes {
            let entry = entries
                .iter()
                .find(|e| e.file_path == bundle.join(node))
                .unwrap();
            assert_eq!(
                archive.node_bytes(archive.node(node).unwrap()).unwrap(),
                expected_textasset_slot(&binary_before, entry)
            );
        }
        assert_eq!(
            archive
                .node_bytes(archive.node("CAB-one.resS").unwrap())
                .unwrap(),
            untouched
        );
        let entry = entries.iter().find(|e| e.file_path == asset).unwrap();
        assert_eq!(
            fs::read(asset).unwrap(),
            expected_textasset_slot(&binary_before, entry)
        );
    }

    #[test]
    fn scripts_and_binary_does_not_hide_bundle_parse_errors() {
        let (fixture, _) = vn_controls_fixture("CJ Welcome traveler!\n");
        fs::write(
            fixture.path().join("Controls_Data/data.unity3d"),
            b"UnityFS\0broken",
        )
        .unwrap();
        assert!(matches!(
            UnityPlugin::new().extract(fixture.path()),
            Err(LocustError::ParseError { .. })
        ));
    }

    #[test]
    fn scripts_and_binary_keeps_serialized_heuristic_fallback() {
        let (fixture, _) = vn_controls_fixture("CJ Existing dialogue.\n");
        // A non-SerializedFile still follows the existing logged heuristic fallback.
        let mut bytes = vec![0; 32];
        bytes.extend_from_slice(&17u32.to_le_bytes());
        bytes.extend_from_slice(b"Welcome traveler!");
        fs::write(fixture.path().join("Controls_Data/resources.assets"), bytes).unwrap();
        let entries = UnityPlugin::new().extract(fixture.path()).unwrap();
        assert_eq!(entries.len(), 2);
        assert!(entries
            .iter()
            .any(|e| e.source == "Welcome traveler!"
                && e.metadata["extraction_method"] == "heuristic"));
    }

    #[test]
    fn inline_sprite_dialogue() {
        let before = "CJ Existing dialogue.\r\nCJ +CJ_Lgr Hello friend.\r\n";
        let (fixture, file) = vn_controls_fixture(before);
        let plugin = UnityPlugin::new();
        let mut entries = plugin.extract(fixture.path()).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[1].id, "Dialogue.txt#2");
        assert_eq!(entries[1].source, "Hello friend.");
        assert_eq!(entries[1].context.as_deref(), Some("CJ"));
        assert_eq!(entries[1].metadata["vn_engine_prefix"], " +CJ_Lgr ");
        entries[1].translation = Some("Hola amigo.".into());
        let report = plugin.inject(fixture.path(), &entries[1..]).unwrap();
        assert_diagnostic_totals(&report, 1);
        assert_eq!((report.strings_written, report.files_modified), (1, 1));
        assert_eq!(
            fs::read(file).unwrap(),
            b"CJ Existing dialogue.\r\nCJ +CJ_Lgr Hola amigo.\r\n"
        );
    }

    #[test]
    fn inline_sprite_single_visible_character_is_dialogue() {
        let (fixture, file) = vn_controls_fixture(
            "CJ +CJ_sur ?\nCJ +CJ_sur !\nCJ +CJ_sur \\bI\\b\nCJ +CJ_sur \\b\\b\n",
        );
        let plugin = UnityPlugin::new();
        let mut entries = plugin.extract(fixture.path()).unwrap();
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].source, "?");
        assert_eq!(entries[1].source, "!");
        assert_eq!(entries[2].source, r"\bI\b");
        entries[0].translation = Some("¿?".into());
        let report = plugin.inject(fixture.path(), &entries[..1]).unwrap();
        assert_eq!((report.strings_written, report.files_modified), (1, 1));
        assert_eq!(
            fs::read_to_string(file).unwrap(),
            "CJ +CJ_sur ¿?\nCJ +CJ_sur !\nCJ +CJ_sur \\bI\\b\nCJ +CJ_sur \\b\\b\n"
        );
    }

    #[test]
    fn inline_sprite_roundtrip_multiple_modifiers_unicode_quotes_and_controls() {
        for (prefix, source, translation) in [
            (" +CJ_Lgr +J_Usur -R ", "W-what?!", "¿Q-qué?!"),
            (
                "\t+CJ_Lgr  +J_Usur\t-R\u{2003}",
                r#"\p<link="journal>entry">Él dijo "\b你好\b".</link>\n\>続き。"#,
                r#"\p<link="journal>entry">Ella dijo "\bこんにちは\b".</link>\n\>Fin."#,
            ),
        ] {
            for ending in ["\r\n", "\n", ""] {
                let before = format!("# untouched  \r\n  CJ{prefix}{source}  \t{ending}");
                let (fixture, file) = vn_controls_fixture(&before);
                let definitions = file.with_file_name("Definitions.txt");
                let untouched = fs::read(&definitions).unwrap();
                let plugin = UnityPlugin::new();
                let mut entries = plugin.extract(fixture.path()).unwrap();
                assert_eq!(entries.len(), 1);
                assert_eq!(entries[0].source, source);
                assert_eq!(entries[0].metadata["original_with_codes"], source);
                assert_eq!(entries[0].metadata["vn_engine_prefix"], prefix);
                entries[0].translation = Some(translation.into());
                let report = plugin.inject(fixture.path(), &entries).unwrap();
                assert_diagnostic_totals(&report, 1);
                assert_eq!((report.strings_written, report.files_modified), (1, 1));
                assert_eq!(
                    fs::read(file).unwrap(),
                    format!("# untouched  \r\n  CJ{prefix}{translation}  \t{ending}").as_bytes()
                );
                assert_eq!(fs::read(definitions).unwrap(), untouched);
                assert_eq!(vn_control_tokens(source), vn_control_tokens(translation));
            }
        }
    }

    #[test]
    fn inline_sprite_commands_and_modifier_only_lines_are_not_dialogue() {
        let script = concat!(
            "CJ +CJ_Lgr\n",
            "CJ +CJ_Lgr +J_Usur -R  \t\n",
            "+CJ_Lgr +J_Usur -R\n",
            "+CJ_Lgr This remains an engine command.\n",
            "CJ +CJ_Lgr +J_Usur -R {\n",
            "CJ +CJ_Lgr \\b\\b\n",
            "CJ +CJ_Lgr +bad! Ambiguous modifier.\n",
            "CJ +A_4dumbo...So?\n",
            "CJ +CA_scr...Wow, what a starter.\n",
            "CJ +CA_guh......Oh.\n",
            "CJ -Wait, this is ordinary dialogue.\n",
        );
        let (fixture, _) = vn_controls_fixture(script);
        let entries = UnityPlugin::new().extract(fixture.path()).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].source, "-Wait, this is ordinary dialogue.");
    }

    #[test]
    fn inline_sprite_changed_prefix_or_source_is_rejected_without_writes() {
        let before = "  CJ  +CJ_Lgr +J_Usur -R Hello \\bfriend\\b.  \r\n";
        for changed in [
            "  CJ  +CJ_ha +J_Usur -R Hello \\bfriend\\b.  \r\n",
            "  CJ  +cj_Lgr +J_Usur -R Hello \\bfriend\\b.  \r\n",
            "  CJ  +J_Usur +CJ_Lgr -R Hello \\bfriend\\b.  \r\n",
            "  CJ  +CJ_Lgr  +J_Usur -R Hello \\bfriend\\b.  \r\n",
            "  CJ  +CJ_Lgr +J_Usur -S Hello \\bfriend\\b.  \r\n",
            "  CJ  Hello \\bfriend\\b.  \r\n",
            "  J  +CJ_Lgr +J_Usur -R Hello \\bfriend\\b.  \r\n",
            "  CJ  +CJ_Lgr +J_Usur -R Hello \\bstranger\\b.  \r\n",
            "  CJ  +CJ_Lgr +J_Usur -R Hello friend.  \r\n",
        ] {
            let (fixture, file) = vn_controls_fixture(before);
            let plugin = UnityPlugin::new();
            let mut entry = plugin.extract(fixture.path()).unwrap().remove(0);
            entry.translation = Some(r"Hola \bamigo\b.".into());
            fs::write(&file, changed).unwrap();
            let report = plugin.inject(fixture.path(), &[entry]).unwrap();
            assert_diagnostic_totals(&report, 1);
            assert_eq!((report.strings_written, report.files_modified), (0, 0));
            assert_eq!(report.skip_reasons.get("source_changed"), Some(&1));
            assert_eq!(fs::read(file).unwrap(), changed.as_bytes());
        }
    }

    #[test]
    fn inline_sprite_rejects_unguarded_prefix_and_unsafe_controls() {
        let source = r"Hello \bfriend\b and \ineighbor\i.";
        let before = format!("CJ +CJ_Lgr {source}\r\n");
        for (translation, remove_prefix, reason) in [
            (r"Hola \bamigo\b y \ivecino\i.", true, "source_changed"),
            ("Hola amigo y vecino.", false, "unsafe_controls"),
            (r"Hola \iamigo\i y \bvecino\b.", false, "unsafe_controls"),
            (r"Hola \bamigo\b y \ivecino\i.\p", false, "unsafe_controls"),
        ] {
            let (fixture, file) = vn_controls_fixture(&before);
            let plugin = UnityPlugin::new();
            let mut entry = plugin.extract(fixture.path()).unwrap().remove(0);
            if remove_prefix {
                entry.metadata.remove("vn_engine_prefix");
            }
            entry.translation = Some(translation.into());
            let report = plugin.inject(fixture.path(), &[entry]).unwrap();
            assert_diagnostic_totals(&report, 1);
            assert_eq!((report.strings_written, report.files_modified), (0, 0));
            assert_eq!(report.skip_reasons.get(reason), Some(&1));
            assert_eq!(fs::read(file).unwrap(), before.as_bytes());
        }
    }

    #[test]
    fn vn_controls_roundtrip_rich_dialogue_and_preserve_untouched_bytes() {
        for (source, translation) in [
            (r"Hello \bworld\b.", r"AUDIT Hello \bworld\b."),
            (r"Hello \iworld\i.", r"Hola \imundo\i."),
            (
                r"\p<link=journal>Hello \bworld\b</link>\n\>続き。",
                r"\p<link=journal>Hola \bmundo\b</link>\n\>Continuación.",
            ),
            (r"\-Él dijo \i你好\i.", r"\-Ella dijo \iこんにちは\i."),
            (r"That...\ishouting?!", r"Eso...\i¿gritando?!"),
            ("Plain dialogue.", "Diálogo sencillo."),
        ] {
            for ending in ["\r\n", "\n", ""] {
                let before = format!("# untouched  \r\n  CJ   {source}  \t{ending}");
                let (fixture, file) = vn_controls_fixture(&before);
                let definitions = file.with_file_name("Definitions.txt");
                let untouched = fs::read(&definitions).unwrap();
                let plugin = UnityPlugin::new();
                let mut entries = plugin.extract(fixture.path()).unwrap();
                assert_eq!(entries.len(), 1, "{source}");
                assert_eq!(
                    entries[0].source, source,
                    "engine controls belong in the source"
                );
                entries[0].translation = Some(translation.into());
                let report = plugin.inject(fixture.path(), &entries).unwrap();
                assert_diagnostic_totals(&report, 1);
                assert_eq!(
                    (report.strings_written, report.files_modified),
                    (1, 1),
                    "{report:?}"
                );
                assert_eq!(
                    fs::read(&file).unwrap(),
                    format!("# untouched  \r\n  CJ   {translation}  \t{ending}").as_bytes()
                );
                assert_eq!(fs::read(definitions).unwrap(), untouched);
            }
        }
    }

    #[test]
    fn vn_controls_reject_missing_added_reordered_or_changed_controls_without_writes() {
        let source = r#"\p<link="journal>entry">Hello \b世界\b and \ifriends\i.</link>\n\>End."#;
        for translation in [
            "Hola mundo.",
            r#"\p<link="journal>entry">Hola \b世界 and \iamigos\i.</link>\n\>Fin."#,
            r#"\p<link="journal>entry">Hola \i世界\i y \bamigos\b.</link>\n\>Fin."#,
            r#"\p<link="other">Hola \b世界\b y \iamigos\i.</link>\n\>Fin."#,
            r#"\p<link="journal>entry">Hola \b世界\b y \iamigos\i.\n\>Fin."#,
            r#"\p<link="journal>entry">Hola \b世界\b y \iamigos\i.</link>\>Fin."#,
            r#"\p<link="journal>entry">Hola \b世界\b y \iamigos\i.</link>\nFin."#,
            r#"\p<link="journal>entry">Hola \b世界\b y \iamigos\i.</link>\n\>Fin.\b"#,
        ] {
            let before = format!("CJ {source}  \r\nCJ Untouched.\r\n");
            let (fixture, file) = vn_controls_fixture(&before);
            let mut entry = UnityPlugin::new()
                .extract(fixture.path())
                .unwrap()
                .into_iter()
                .find(|e| e.id == "Dialogue.txt#1")
                .unwrap();
            entry.translation = Some(translation.into());
            let report = UnityPlugin::new().inject(fixture.path(), &[entry]).unwrap();
            assert_diagnostic_totals(&report, 1);
            assert_eq!(
                (report.strings_written, report.files_modified),
                (0, 0),
                "{report:?}"
            );
            assert_eq!(report.skip_reasons.get("unsafe_controls"), Some(&1));
            assert_eq!(fs::read(file).unwrap(), before.as_bytes());
        }
    }

    #[test]
    fn vn_controls_legacy_stripped_entries_cannot_overwrite_rich_sources() {
        for source in [r"Hello \bworld\b.", r"\iHello world.\i", r"Hello\nworld."] {
            let before = format!("CJ {source}\r\n");
            let (fixture, file) = vn_controls_fixture(&before);
            let mut entry = StringEntry::new("Dialogue.txt#1", "Hello world.", file.clone());
            entry.translation = Some("Hola mundo.".into());
            entry
                .metadata
                .insert("original_with_codes".into(), serde_json::json!(source));
            let report = UnityPlugin::new().inject(fixture.path(), &[entry]).unwrap();
            assert_diagnostic_totals(&report, 1);
            assert_eq!((report.strings_written, report.files_modified), (0, 0));
            assert_eq!(report.skip_reasons.get("source_changed"), Some(&1));
            assert_eq!(fs::read(file).unwrap(), before.as_bytes());
        }
    }

    #[test]
    fn vn_controls_menu_labels_preserve_tags_and_reject_removal() {
        let before = "  button 0 \"<b>Enter</b>\" +link jump 5  \r\n";
        let (fixture, file) = vn_controls_fixture(before);
        let mut entry = UnityPlugin::new()
            .extract(fixture.path())
            .unwrap()
            .remove(0);
        entry.translation = Some("Entrar".into());
        let report = UnityPlugin::new()
            .inject(fixture.path(), &[entry.clone()])
            .unwrap();
        assert_eq!((report.strings_written, report.files_modified), (0, 0));
        assert_eq!(report.skip_reasons.get("unsafe_controls"), Some(&1));
        assert_eq!(fs::read(&file).unwrap(), before.as_bytes());
        entry.translation = Some("<b>Entrar</b>".into());
        let report = UnityPlugin::new().inject(fixture.path(), &[entry]).unwrap();
        assert_eq!((report.strings_written, report.files_modified), (1, 1));
        assert_eq!(
            fs::read(file).unwrap(),
            "  button 0 \"<b>Entrar</b>\" +link jump 5  \r\n".as_bytes()
        );
    }

    #[test]
    fn empty_translation_is_untranslated_and_never_erases_a_unity_slot() {
        let fixture = tempfile::tempdir().unwrap();
        create_unity_fixture(fixture.path());
        let asset = fixture.path().join("TestGame_Data/resources.assets");
        let original = fs::read(&asset).unwrap();
        let mut entry = StringEntry::new("empty", "Hello World", asset.clone());
        entry.translation = Some(String::new());
        let report = UnityPlugin::new().inject(fixture.path(), &[entry]).unwrap();
        assert_eq!((report.strings_written, report.files_modified), (0, 0));
        assert_eq!(report.strings_skipped, 1);
        assert_eq!(report.skip_reasons.get("untranslated"), Some(&1));
        assert_eq!(fs::read(&asset).unwrap(), original);
    }

    fn assert_diagnostic_totals(report: &InjectionReport, entries: usize) {
        assert_eq!(
            report.strings_written + report.strings_skipped,
            entries,
            "{report:?}"
        );
        assert_eq!(
            report.skip_reasons.values().sum::<usize>(),
            report.strings_skipped,
            "{report:?}"
        );
    }

    #[test]
    fn diagnostics_binary_skips_are_exhaustive_and_missing_files_count() {
        let fixture = tempfile::tempdir().unwrap();
        create_unity_fixture(fixture.path());
        let asset = fixture.path().join("TestGame_Data/resources.assets");
        let make = |id: &str, source: &str, translation: Option<&str>| {
            let mut entry = StringEntry::new(id, source, asset.clone());
            entry.translation = translation.map(str::to_string);
            entry
        };
        let mut entries = vec![
            make("write", "Hello World", Some("Hola Mundo")),
            make("pending", "Pending source", None),
            make("same", "Identity", Some("Identity")),
            make("long", "Short", Some("Translation exceeds this slot")),
            make("stale", "No longer present", Some("Absent")),
        ];
        let mut missing = make("missing", "Missing source", Some("Absent"));
        missing.file_path = fixture.path().join("missing.assets");
        entries.push(missing);
        let mut invalid = make("invalid", "Structural source", Some("Cambio"));
        invalid
            .metadata
            .insert("extraction_method".into(), serde_json::json!("textasset"));
        entries.push(invalid);
        let report = UnityPlugin::new().inject(fixture.path(), &entries).unwrap();
        assert_diagnostic_totals(&report, entries.len());
        assert_eq!(report.strings_written, 1);
        for reason in [
            "untranslated",
            "unchanged",
            "too_long",
            "source_changed",
            "target_missing",
            "error",
        ] {
            assert_eq!(report.skip_reasons.get(reason), Some(&1), "{report:?}");
        }
    }

    #[test]
    fn diagnostics_unityfs_missing_node_is_not_silently_lost() {
        let fixture = tempfile::tempdir().unwrap();
        let bundle_path = fixture.path().join("data.unity3d");
        let bundle = crate::unity_fs::build_test_bundle(
            &[("CAB-present", b"payload")],
            true,
            8,
            true,
            false,
        );
        fs::write(&bundle_path, &bundle).unwrap();
        let mut entry = StringEntry::new(
            "missing-node",
            "Missing source",
            bundle_path.join("CAB-absent"),
        );
        entry.translation = Some("Cambio".into());
        let report = UnityPlugin::new().inject(fixture.path(), &[entry]).unwrap();
        assert_diagnostic_totals(&report, 1);
        assert_eq!(report.skip_reasons.get("target_missing"), Some(&1));
        assert_eq!(fs::read(bundle_path).unwrap(), bundle);
    }

    #[test]
    fn diagnostics_csv_counts_only_cells_actually_written() {
        let fixture = tempfile::tempdir().unwrap();
        let asset = fixture.path().join("resources.assets");
        let script = "ITEM_ID,ITEM_NAME\r\n1,apple\r\n2,banana\r\n3,orange\r\n4,grape\r\n";
        fs::write(
            &asset,
            crate::unity_serialized::write_v17_fixture("Items", script),
        )
        .unwrap();
        let plugin = UnityPlugin::new();
        let mut entries: Vec<_> = plugin
            .extract(&asset)
            .unwrap()
            .into_iter()
            .filter(is_textasset_csv_cell_entry)
            .collect();
        assert_eq!(entries.len(), 4);
        for entry in &mut entries {
            entry.translation = match entry.source.as_str() {
                "apple" => None,
                "banana" => Some("banana".into()),
                "orange" => Some("bad,value".into()),
                "grape" => Some("uva".into()),
                other => panic!("Unexpected cell {other}"),
            };
        }
        let report = plugin.inject(fixture.path(), &entries).unwrap();
        assert_diagnostic_totals(&report, 4);
        assert_eq!(report.strings_written, 1);
        for reason in ["untranslated", "unchanged", "error"] {
            assert_eq!(report.skip_reasons.get(reason), Some(&1));
        }
        let after = plugin.extract(&asset).unwrap();
        for source in ["apple", "banana", "orange", "uva"] {
            assert!(after.iter().any(|e| e.source == source), "Missing {source}");
        }
    }

    #[test]
    fn diagnostics_tables_without_changes_do_not_rewrite_or_claim_one_write() {
        for script in [
            "ITEM_ID,ITEM_NAME\n1,apple\n2,banana\n   ",
            "TitleMenu.START: NEW GAME\nTitleMenu.CREDITS: CREDITS\n   ",
        ] {
            let fixture = tempfile::tempdir().unwrap();
            let asset = fixture.path().join("resources.assets");
            let original = crate::unity_serialized::write_v17_fixture("Table", script);
            fs::write(&asset, &original).unwrap();
            let plugin = UnityPlugin::new();
            let mut entries: Vec<_> = plugin
                .extract(&asset)
                .unwrap()
                .into_iter()
                .filter(|e| is_textasset_csv_cell_entry(e) || is_textasset_loc_line_entry(e))
                .collect();
            assert!(!entries.is_empty());
            for entry in &mut entries {
                entry.translation = Some(entry.source.clone());
            }
            let report = plugin.inject(fixture.path(), &entries).unwrap();
            assert_diagnostic_totals(&report, entries.len());
            assert_eq!(report.strings_written, 0);
            assert_eq!(report.files_modified, 0);
            assert_eq!(report.skip_reasons.get("unchanged"), Some(&entries.len()));
            assert_eq!(fs::read(&asset).unwrap(), original);
        }
    }

    #[test]
    fn diagnostics_partial_loc_preserves_rows_not_in_the_request() {
        let fixture = tempfile::tempdir().unwrap();
        let asset = fixture.path().join("resources.assets");
        let script = "TitleMenu.START: NEW GAME\r\n\r\nTitleMenu.CREDITS: CREDITS\r\nConfirmation.Yes: YES\r\n";
        fs::write(
            &asset,
            crate::unity_serialized::write_v17_fixture("ManagedText", script),
        )
        .unwrap();
        let plugin = UnityPlugin::new();
        let mut entry = plugin
            .extract(&asset)
            .unwrap()
            .into_iter()
            .find(|e| e.source == "NEW GAME")
            .unwrap();
        entry.translation = Some("NUEVO".into());
        let report = plugin.inject(fixture.path(), &[entry]).unwrap();
        assert_diagnostic_totals(&report, 1);
        assert_eq!(report.strings_written, 1);
        let bytes = fs::read(&asset).unwrap();
        let parsed = SerializedFile::parse(bytes, &asset).unwrap();
        let object = parsed.text_asset_objects().next().unwrap();
        let text = parsed.read_text_asset(object.path_id).unwrap().script;
        assert!(text.contains(
            "TitleMenu.START: NUEVO\r\n\r\nTitleMenu.CREDITS: CREDITS\r\nConfirmation.Yes: YES\r\n"
        ));
    }

    #[test]
    fn diagnostics_loc_inject_uses_physical_injection_source_after_pivot() {
        let fixture = tempfile::tempdir().unwrap();
        let asset = fixture.path().join("resources.assets");
        let script = "TitleMenu.START: NEW GAME\r\nTitleMenu.CREDITS: CREDITS\r\n";
        fs::write(
            &asset,
            crate::unity_serialized::write_v17_fixture("ManagedText", script),
        )
        .unwrap();
        let plugin = UnityPlugin::new();
        let mut entry = plugin
            .extract(&asset)
            .unwrap()
            .into_iter()
            .find(|e| e.source == "NEW GAME")
            .unwrap();
        entry.metadata.insert(
            locust_core::models::INJECTION_SOURCE_METADATA_KEY.into(),
            serde_json::json!("NEW GAME"),
        );
        entry.source = "Start".into();
        entry.translation = Some("Inicio".into());
        let report = plugin.inject(fixture.path(), &[entry]).unwrap();
        assert_eq!(report.strings_written, 1);
        let bytes = fs::read(&asset).unwrap();
        let parsed = SerializedFile::parse(bytes, &asset).unwrap();
        let object = parsed.text_asset_objects().next().unwrap();
        let text = parsed.read_text_asset(object.path_id).unwrap().script;
        assert!(text.contains("TitleMenu.START: Inicio"), "{text}");
        assert!(text.contains("TitleMenu.CREDITS: CREDITS"), "{text}");
    }

    #[test]
    fn diagnostics_loc_group_overflow_counts_changed_entries_not_pending_rows() {
        let fixture = tempfile::tempdir().unwrap();
        let asset = fixture.path().join("resources.assets");
        let script =
            "TitleMenu.START: NEW GAME\nTitleMenu.CREDITS: CREDITS\nConfirmation.Yes: YES\n";
        let original = crate::unity_serialized::write_v17_fixture("ManagedText", script);
        fs::write(&asset, &original).unwrap();
        let plugin = UnityPlugin::new();
        let mut entries: Vec<_> = plugin
            .extract(&asset)
            .unwrap()
            .into_iter()
            .filter(is_textasset_loc_line_entry)
            .collect();
        assert_eq!(entries.len(), 3);
        for entry in entries.iter_mut().take(2) {
            entry.metadata.remove("textasset_rewrite"); // Legacy fixed-slot diagnostic.
            entry.translation = Some("X".repeat(script.len()));
        }
        let report = plugin.inject(fixture.path(), &entries).unwrap();
        assert_diagnostic_totals(&report, 3);
        assert_eq!(report.skip_reasons.get("too_long"), Some(&2));
        assert_eq!(report.skip_reasons.get("untranslated"), Some(&1));
        assert_eq!(report.strings_written, 0);
        assert_eq!(fs::read(&asset).unwrap(), original);
    }

    #[test]
    fn diagnostics_script_lines_and_binary_entries_can_share_a_batch() {
        let fixture = tempfile::tempdir().unwrap();
        create_unity_fixture(fixture.path());
        let script = fixture.path().join("Chapter.txt");
        fs::write(
            &script,
            "  J Hello there.\r\n  J Changed since extraction.\r\n",
        )
        .unwrap();
        let mut text = StringEntry::new("Chapter.txt#1", "Hello there.", script.clone());
        text.translation = Some("Hola amigo.".into());
        let mut stale = StringEntry::new("Chapter.txt#2", "Original source.", script.clone());
        stale.translation = Some("Antiguo.".into());
        let mut missing = StringEntry::new("Chapter.txt#99", "Missing line.", script.clone());
        missing.translation = Some("Ausente.".into());
        let mut binary = StringEntry::new(
            "binary",
            "Hello World",
            fixture.path().join("TestGame_Data/resources.assets"),
        );
        binary.translation = Some("Hola Mundo".into());
        let report = UnityPlugin::new()
            .inject(fixture.path(), &[text, stale, missing, binary])
            .unwrap();
        assert_diagnostic_totals(&report, 4);
        assert_eq!(report.strings_written, 2);
        assert_eq!(report.files_modified, 2);
        assert_eq!(report.skip_reasons.get("source_changed"), Some(&1));
        assert_eq!(report.skip_reasons.get("target_missing"), Some(&1));
        assert_eq!(
            fs::read_to_string(&script).unwrap(),
            "  J Hola amigo.\r\n  J Changed since extraction.\r\n"
        );
    }

    fn tempdir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("locust_unity_{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn assert_inplace_char_limit(entry: &StringEntry) {
        if has_textasset_rewrite_capability(entry) {
            assert_eq!(entry.char_limit, None);
            return;
        }
        assert_eq!(
            entry.char_limit,
            Some(entry.source.len()),
            "in-place slot {} char_limit must be UTF-8 byte length of source ({:?})",
            entry.id,
            entry.source
        );
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

        fs::write(
            scripts_dir.join("Chapter_1.txt"),
            r#"version 1.0

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
"#,
        )
        .unwrap();

        fs::write(
            scripts_dir.join("Menus.txt"),
            r#"version 1.0

  menu MainMenu {
    button 0 "Talk" jump 10
    button 1 "Examine" jump 20
    button 2 "Leave" +main jump 30
  }
"#,
        )
        .unwrap();

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
        let p = Path::new("Games")
            .join("Boxman_Data")
            .join("globalgamemanagers");
        assert!(is_unity_serialized_candidate(&p));
        #[cfg(windows)]
        assert!(is_unity_serialized_candidate(Path::new(
            r"C:\Games\Boxman_Data\globalgamemanagers"
        )));
    }

    #[test]
    fn test_extract_finds_extensionless_globalgamemanagers() {
        let dir = tempdir();
        let data_dir = dir.join("TestGame_Data");
        fs::create_dir_all(&data_dir).unwrap();
        fs::write(dir.join("UnityPlayer.dll"), b"fake").unwrap();

        // Real SerializedFile (v17 TextAsset) stored under the classic
        // extensionless managers name — must be discovered and extracted.
        let bytes =
            crate::unity_serialized::write_v17_fixture("Sys", "Extensionless managers dialogue");
        fs::write(data_dir.join("globalgamemanagers"), &bytes).unwrap();

        let plugin = UnityPlugin::new();
        let found = UnityPlugin::find_assets_files(&dir);
        assert!(
            found
                .iter()
                .any(|p| p.file_name().is_some_and(|n| n == "globalgamemanagers")),
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
    fn test_unityfs_data_unity3d_extract_and_inject() {
        let dir = tempdir();
        let data_dir = dir.join("TestGame_Data");
        fs::create_dir_all(&data_dir).unwrap();
        fs::write(dir.join("UnityPlayer.dll"), b"fake").unwrap();

        let cab = crate::unity_serialized::write_v17_fixture("Speaker", "Hello from the bundle.");
        let ress = b"SHOULD_NOT_EXTRACT_RESS_PAYLOAD!!!!";
        let bundle = crate::unity_fs::build_test_bundle(
            &[
                ("CAB-test", cab.as_slice()),
                ("CAB-test.resS", ress.as_slice()),
            ],
            true,
            8,
            true,
            false,
        );
        let bundle_path = data_dir.join("data.unity3d");
        fs::write(&bundle_path, &bundle).unwrap();

        let plugin = UnityPlugin::new();
        assert!(plugin.detect(&dir));
        assert!(plugin.detect(&bundle_path));
        assert!(plugin.supported_extensions().contains(&".unity3d"));

        let found = UnityPlugin::find_unityfs_files(&dir);
        assert_eq!(found, vec![bundle_path.clone()]);

        let entries = plugin.extract(&dir).unwrap();
        assert!(
            entries.iter().any(|e| e.source == "Hello from the bundle."),
            "got: {:?}",
            entries.iter().map(|e| &e.source).collect::<Vec<_>>()
        );
        assert!(
            entries
                .iter()
                .all(|e| e.source != "SHOULD_NOT_EXTRACT_RESS_PAYLOAD!!!!"),
            "must skip .resS nodes: {:?}",
            entries.iter().map(|e| &e.source).collect::<Vec<_>>()
        );
        let hit = entries
            .iter()
            .find(|e| e.source == "Hello from the bundle.")
            .unwrap();
        assert_inplace_char_limit(hit);
        let fp = hit.file_path.to_string_lossy();
        assert!(
            fp.contains("data.unity3d") && fp.contains("CAB-test"),
            "virtual path should be data.unity3d/CAB-test, got {fp}"
        );

        let mut to_inject = entries.clone();
        for e in &mut to_inject {
            if e.source == "Hello from the bundle." {
                e.translation = Some("Hola desde el bundle.".to_string());
            }
        }
        let report = plugin.inject(&dir, &to_inject).unwrap();
        assert!(report.strings_written >= 1);
        assert!(report.files_modified >= 1);
        assert!(
            report
                .files_written
                .iter()
                .any(|p| p.file_name().is_some_and(|n| n == "data.unity3d")),
            "files_written: {:?}",
            report.files_written
        );

        let rewritten = fs::read(&bundle_path).unwrap();
        let again =
            crate::unity_fs::UnityFsArchive::parse(rewritten.clone(), "data.unity3d").unwrap();
        assert_eq!(again.header.size as usize, rewritten.len());
        assert_eq!(again.write_bytes().unwrap().len(), rewritten.len());

        let after = plugin.extract(&dir).unwrap();
        assert!(
            after
                .iter()
                .any(|e| e.source.trim_end() == "Hola desde el bundle."),
            "got: {:?}",
            after.iter().map(|e| &e.source).collect::<Vec<_>>()
        );
    }

    #[test]
    fn test_find_assets_nested_depth3_level() {
        let dir = tempdir();
        let data_dir = dir.join("TestGame_Data");
        let nested = data_dir.join("Scenes").join("Act1");
        fs::create_dir_all(&nested).unwrap();
        fs::write(dir.join("UnityPlayer.dll"), b"fake").unwrap();
        let bytes = crate::unity_serialized::write_v17_fixture("N", "Nested level dialogue here");
        // Depth from *_Data: Scenes (1) / Act1 (2) / level0 (3)
        fs::write(nested.join("level0"), &bytes).unwrap();

        let found = UnityPlugin::find_assets_files(&dir);
        assert!(
            found
                .iter()
                .any(|p| p.file_name().is_some_and(|n| n == "level0")),
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
        assert!(
            sources.contains(&"Press any key to continue"),
            "got: {:?}",
            sources
        );
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
            if entry
                .metadata
                .get("extraction_method")
                .and_then(|v| v.as_str())
                == Some("heuristic")
            {
                assert!(
                    entry
                        .metadata
                        .get("binary_offset")
                        .and_then(|v| v.as_u64())
                        .is_some(),
                    "heuristic entry {} missing pinned binary_offset",
                    entry.id
                );
            }
            assert_inplace_char_limit(entry);
        }
    }

    #[test]
    fn test_vn_single_visible_character_extract_inject() {
        let dir = tempdir();
        let scripts = dir.join("TestGame_Data/SCRIPTS~");
        fs::create_dir_all(&scripts).unwrap();
        let path = scripts.join("Short.txt");
        let original = concat!(
            "CJ Existing dialogue.\r\n",
            "  CJ !\n",
            "CJ ?\r\n",
            "CJ I\n",
            "\tCJ  \\bI\r\n",
            "CJ 界\n",
            "CJ ★\r\n",
            "CJ \\bI\\b\n",
            "CJ <i>I</i>\r\n",
            "CJ +CJ_smile  ?\n",
            "CJ \\b\\i\\i\\b\r\n",
            "CJ <i></i>\n",
            "CJ    \r\n",
            "wait 1\n",
            "CJ {\r\n",
            "# CJ !\n",
            "CJ Untargeted ending."
        );
        fs::write(&path, original).unwrap();
        let plugin = UnityPlugin::new();
        let rows = plugin.extract(&dir).unwrap();
        assert_eq!(
            rows.iter()
                .map(|row| row.source.as_str())
                .collect::<Vec<_>>(),
            [
                "Existing dialogue.",
                "!",
                "?",
                "I",
                "\\bI",
                "界",
                "★",
                "\\bI\\b",
                "<i>I</i>",
                "?",
                "Untargeted ending."
            ]
        );
        let translations = [
            (2, "?"),
            (3, "!"),
            (4, "J"),
            (5, "\\bJ"),
            (6, "字"),
            (7, "☆"),
            (8, "\\bJ\\b"),
            (9, "<i>J</i>"),
            (10, "!"),
        ];
        let mut targets = Vec::new();
        let mut expected = original
            .split_inclusive('\n')
            .map(str::to_string)
            .collect::<Vec<_>>();
        for (line, translation) in translations {
            let mut row = rows
                .iter()
                .find(|row| row.id == format!("Short.txt#{line}"))
                .unwrap()
                .clone();
            // Compute the expected file independently, replacing only the body.
            expected[line - 1] = expected[line - 1].replacen(&row.source, translation, 1);
            row.translation = Some(translation.into());
            targets.push(row);
        }
        let report = plugin.inject(&dir, &targets).unwrap();
        assert_eq!(report.strings_written, 9);
        assert_eq!(report.strings_skipped, 0);
        assert_eq!(fs::read(&path).unwrap(), expected.concat().as_bytes());

        fs::write(&path, original).unwrap();
        let mut unsafe_rows = Vec::new();
        for source in ["\\bI", "\\bI\\b", "<i>I</i>"] {
            let mut row = rows
                .iter()
                .find(|row| row.source == source)
                .unwrap()
                .clone();
            row.translation = Some("J".into());
            unsafe_rows.push(row);
        }
        let report = plugin.inject(&dir, &unsafe_rows).unwrap();
        assert_eq!(report.strings_written, 0);
        assert_eq!(report.skip_reasons.get("unsafe_controls"), Some(&3));
        assert_eq!(fs::read(&path).unwrap(), original.as_bytes());
    }

    #[test]
    fn test_extract_vn_scripts() {
        let dir = tempdir();
        create_vn_script_fixture(&dir);
        let plugin = UnityPlugin::new();
        let entries = plugin.extract(&dir).unwrap();

        let sources: Vec<&str> = entries.iter().map(|e| e.source.as_str()).collect();
        assert!(
            sources.contains(&"This is the beginning of our story."),
            "got: {:?}",
            sources
        );
        assert!(sources.contains(&"My name is Jamie."), "got: {:?}", sources);
        assert!(
            sources.contains(&"I'm waiting for my best friend!"),
            "got: {:?}",
            sources
        );
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

        let jamie = entries
            .iter()
            .find(|e| e.source == "My name is Jamie.")
            .unwrap();
        assert_eq!(jamie.context, Some("J".to_string()));
        assert!(jamie.tags.contains(&"dialogue".to_string()));

        let nar = entries
            .iter()
            .find(|e| e.source.contains("beginning"))
            .unwrap();
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
            dir.join("TestGame_Data")
                .join("SCRIPTS~")
                .join("Chapter_1.txt"),
        )
        .unwrap();
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

    /// Exercise the complete heuristic path because translated strings must remain
    /// discoverable on the next extract, for both Unity byte orders.
    #[test]
    fn test_heuristic_multilingual_extract_inject_reextract_le_and_be() {
        let cases = [
            ("保存", "存档"),
            (
                "物語を続けますか？次の章を始めますか？",
                "¿Continuación? ¡Sí, próximo capítulo!",
            ),
            ("Continue", "继续"),
            ("Settings", "设置"),
            ("Main Menu", "主菜单"),
        ];

        for endian in [LengthEndian::Little, LengthEndian::Big] {
            let dir = tempdir();
            let data_dir = dir.join("TestGame_Data");
            fs::create_dir_all(&data_dir).unwrap();
            fs::write(dir.join("UnityPlayer.dll"), b"fake").unwrap();

            // Invalid SerializedFile header forces the heuristic scanner.
            let mut data = vec![0xFF; 64];
            for (source, _) in cases {
                let source = source.as_bytes();
                data.extend_from_slice(&endian.encode_u32(source.len() as u32));
                data.extend_from_slice(source);
                data.resize((data.len() + 3) & !3, 0);
            }
            let assets = data_dir.join("resources.assets");
            fs::write(&assets, data).unwrap();

            let plugin = UnityPlugin::new();
            let mut entries = plugin.extract(&dir).unwrap();
            let expected_endian = endian.as_meta();
            for (source, _) in cases {
                let hit = entries
                    .iter()
                    .find(|e| e.source == source)
                    .unwrap_or_else(|| {
                        panic!(
                            "{expected_endian} must extract {source:?}; got {:?}",
                            entries.iter().map(|e| &e.source).collect::<Vec<_>>()
                        )
                    });
                assert_eq!(
                    hit.metadata.get("length_endian").and_then(|v| v.as_str()),
                    Some(expected_endian)
                );
                assert!(
                    hit.metadata
                        .get("binary_offset")
                        .and_then(|v| v.as_u64())
                        .is_some(),
                    "{source:?} must preserve its pinned binary_offset"
                );
            }

            for entry in &mut entries {
                if let Some((_, target)) = cases.iter().find(|(source, _)| *source == entry.source)
                {
                    assert!(
                        target.len() <= entry.source.len(),
                        "fixture target must fit the in-place byte slot"
                    );
                    entry.translation = Some((*target).to_string());
                }
            }
            let report = plugin.inject(&dir, &entries).unwrap();
            assert_eq!(
                report.strings_written,
                cases.len(),
                "{expected_endian} multilingual inject: skipped={} reasons={:?} warnings={:?}",
                report.strings_skipped,
                report.skip_reasons,
                report.warnings
            );

            let again = plugin.extract(&dir).unwrap();
            for (_, target) in cases {
                let hit = again
                    .iter()
                    .find(|e| e.source == target)
                    .unwrap_or_else(|| {
                        panic!(
                            "{expected_endian} must re-extract {target:?}; got {:?}",
                            again.iter().map(|e| &e.source).collect::<Vec<_>>()
                        )
                    });
                assert_eq!(
                    hit.metadata.get("length_endian").and_then(|v| v.as_str()),
                    Some(expected_endian)
                );
                assert!(
                    hit.metadata
                        .get("binary_offset")
                        .and_then(|v| v.as_u64())
                        .is_some(),
                    "re-extracted {target:?} must retain binary_offset metadata"
                );
            }
        }
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
            hello.metadata.get("length_endian").and_then(|v| v.as_str()),
            Some("be"),
            "must record big-endian length: {:?}",
            hello.metadata
        );
        assert_eq!(
            hello.metadata.get("binary_offset").and_then(|v| v.as_u64()),
            Some(64)
        );
        assert_inplace_char_limit(hello);

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
        let mut offsets = Vec::new();
        for s in [
            s_dup.as_slice(),
            s_id.as_slice(),
            s_dup.as_slice(),
            s_long.as_slice(),
            s_ok.as_slice(),
        ] {
            offsets.push(data.len());
            data.extend_from_slice(&(s.len() as u32).to_le_bytes());
            data.extend_from_slice(s);
            data.push(0);
        }
        let assets = data_dir.join("sharedassets0.assets");
        fs::write(&assets, &data).unwrap();

        let mk = |id: &str, source: &str, translation: Option<&str>, offset: usize| {
            let mut e = StringEntry::new(id, source, assets.clone());
            e.translation = translation.map(|s| s.to_string());
            e.metadata
                .insert("binary_offset".into(), serde_json::json!(offset));
            e.metadata
                .insert("length_endian".into(), serde_json::json!("le"));
            e
        };
        let inject_list = vec![
            mk("dup1", "DupStr!!", Some("DupOne!!"), offsets[0]),
            mk("id", "SameSame", Some("SameSame"), offsets[1]),
            mk("dup2", "DupStr!!", Some("DupTwo!!"), offsets[2]),
            mk("over", "SlotTxt!", Some("WAYTOOLONG"), offsets[3]),
            mk("ok", "OkText!!", Some("OkTxt!!!"), offsets[4]),
        ];

        let plugin = UnityPlugin::new();
        let report = plugin.inject(&dir, &inject_list).unwrap();
        assert_eq!(
            report.strings_written, 3,
            "two dups + ok; got written={} skipped={} warnings={:?}",
            report.strings_written, report.strings_skipped, report.warnings
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
        assert_eq!(out.windows(8).filter(|w| *w == b"DupOne!!").count(), 1);
        assert_eq!(out.windows(8).filter(|w| *w == b"DupTwo!!").count(), 1);
        assert!(out.windows(8).any(|w| w == b"OkTxt!!!"));
        assert!(
            out.windows(8).any(|w| w == b"SameSame"),
            "identity unchanged"
        );
        assert!(
            out.windows(8).any(|w| w == b"SlotTxt!"),
            "oversize target unchanged"
        );
    }

    #[test]
    fn test_heuristic_offsets_and_legacy_targets_are_safe() {
        let dir = tempdir();
        let data_dir = dir.join("TestGame_Data");
        fs::create_dir_all(&data_dir).unwrap();
        fs::write(dir.join("UnityPlayer.dll"), b"fake").unwrap();
        let mut data = vec![0xFF; 16];
        let mut offsets = Vec::new();
        for text in [b"DupStr!!", b"DupStr!!", b"Unique!!", b"Other!!!"] {
            offsets.push(data.len());
            data.extend_from_slice(&(text.len() as u32).to_le_bytes());
            data.extend_from_slice(text);
        }
        let assets = data_dir.join("resources.assets");
        fs::write(&assets, &data).unwrap();

        let mut pinned_second = StringEntry::new("second", "DupStr!!", assets.clone());
        pinned_second.translation = Some("Only2nd!".into());
        pinned_second
            .metadata
            .insert("binary_offset".into(), serde_json::json!(offsets[1]));
        pinned_second
            .metadata
            .insert("length_endian".into(), serde_json::json!("le"));

        let mut legacy_ambiguous = StringEntry::new("legacy-dup", "DupStr!!", assets.clone());
        legacy_ambiguous.translation = Some("Legacy!!".into());
        let mut legacy_unique = StringEntry::new("legacy-one", "Unique!!", assets.clone());
        legacy_unique.translation = Some("Changed!".into());

        let mut invalid_offset = StringEntry::new("invalid", "Other!!!", assets.clone());
        invalid_offset.translation = Some("Invalid!".into());
        invalid_offset
            .metadata
            .insert("binary_offset".into(), serde_json::json!(u64::MAX));
        invalid_offset
            .metadata
            .insert("length_endian".into(), serde_json::json!("le"));

        let mut stale_offset = StringEntry::new("stale", "Unique!!", assets.clone());
        stale_offset.translation = Some("Stale!!!".into());
        stale_offset
            .metadata
            .insert("binary_offset".into(), serde_json::json!(offsets[3]));
        stale_offset
            .metadata
            .insert("length_endian".into(), serde_json::json!("le"));

        let report = UnityPlugin::new()
            .inject(
                &dir,
                &[
                    pinned_second,
                    legacy_ambiguous,
                    legacy_unique,
                    invalid_offset,
                    stale_offset,
                ],
            )
            .unwrap();
        assert_eq!(report.strings_written, 2);
        assert_eq!(report.skip_reasons.get("ambiguous_target"), Some(&1));
        assert_eq!(report.skip_reasons.get("source_changed"), Some(&2));

        let out = fs::read(&assets).unwrap();
        assert_eq!(&out[offsets[0] + 4..offsets[0] + 12], b"DupStr!!");
        assert_eq!(&out[offsets[1] + 4..offsets[1] + 12], b"Only2nd!");
        assert_eq!(&out[offsets[2] + 4..offsets[2] + 12], b"Changed!");
        assert_eq!(&out[offsets[3] + 4..offsets[3] + 12], b"Other!!!");
    }

    #[test]
    fn test_performance_textassets_do_not_extract_or_leak_to_heuristics() {
        // Complete shipped run-info payloads from CCTV/USSR and Sunkissed.
        let scripts = [
            r#"{"MeasurementCount":-1}"#,
            r#"{"TestSuite":"","Date":0,"Player":{"Development":false,"ScreenWidth":0,"ScreenHeight":0,"ScreenRefreshRate":0,"Fullscreen":false,"Vsync":0,"AntiAliasing":0,"Batchmode":false,"RenderThreadingMode":"GraphicsJobs","GpuSkinning":true,"Platform":"","ColorSpace":"","AnisotropicFiltering":"","BlendWeights":"","GraphicsApi":"","ScriptingBackend":"Mono2x","AndroidTargetSdkVersion":"AndroidApiLevelAuto","AndroidBuildSystem":"Gradle","BuildTarget":"StandaloneWindows64","StereoRenderingPath":"MultiPass"},"Hardware":{"OperatingSystem":"","DeviceModel":"","DeviceName":"","ProcessorType":"","ProcessorCount":0,"GraphicsDeviceName":"","SystemMemorySizeMB":0},"Editor":{"Version":"6000.0.24f1","Branch":"6000.0/staging","Changeset":"11fa355cd605","Date":1729086108},"Dependencies":["com.unity.2d.sprite@1.0.0","com.unity.ai.navigation@2.0.4","com.unity.collab-proxy@2.5.2","com.unity.ide.rider@3.0.31","com.unity.ide.visualstudio@2.0.22","com.unity.inputsystem@1.11.1","com.unity.package-validation-suite@0.22.0-preview","com.unity.render-pipelines.universal@17.0.3","com.unity.test-framework@1.4.5","com.unity.timeline@1.8.7","com.unity.ugui@2.0.0","com.unity.visualscripting@1.9.4","com.unity.modules.accessibility@1.0.0","com.unity.modules.ai@1.0.0","com.unity.modules.androidjni@1.0.0","com.unity.modules.animation@1.0.0","com.unity.modules.assetbundle@1.0.0","com.unity.modules.audio@1.0.0","com.unity.modules.cloth@1.0.0","com.unity.modules.director@1.0.0","com.unity.modules.imageconversion@1.0.0","com.unity.modules.imgui@1.0.0","com.unity.modules.jsonserialize@1.0.0","com.unity.modules.particlesystem@1.0.0","com.unity.modules.physics@1.0.0","com.unity.modules.physics2d@1.0.0","com.unity.modules.screencapture@1.0.0","com.unity.modules.terrain@1.0.0","com.unity.modules.terrainphysics@1.0.0","com.unity.modules.tilemap@1.0.0","com.unity.modules.ui@1.0.0","com.unity.modules.uielements@1.0.0","com.unity.modules.umbra@1.0.0","com.unity.modules.unityanalytics@1.0.0","com.unity.modules.unitywebrequest@1.0.0","com.unity.modules.unitywebrequestassetbundle@1.0.0","com.unity.modules.unitywebrequestaudio@1.0.0","com.unity.modules.unitywebrequesttexture@1.0.0","com.unity.modules.unitywebrequestwww@1.0.0","com.unity.modules.vehicles@1.0.0","com.unity.modules.video@1.0.0","com.unity.modules.vr@1.0.0","com.unity.modules.wind@1.0.0","com.unity.modules.xr@1.0.0","com.unity.modules.subsystems@1.0.0","com.unity.modules.hierarchycore@1.0.0","com.unity.ext.nunit@2.0.5","com.unity.render-pipelines.core@17.0.3","com.unity.shadergraph@17.0.3","com.unity.render-pipelines.universal-config@17.0.3","com.unity.nuget.mono-cecil@1.11.4","com.unity.searcher@4.9.2","com.unity.burst@1.8.18","com.unity.mathematics@1.3.2","com.unity.collections@2.5.1","com.unity.rendering.light-transport@1.0.1","com.unity.test-framework.performance@3.0.3"],"Results":[]}"#,
            r#"{"TestSuite":"","Date":0,"Player":{"Development":false,"ScreenWidth":0,"ScreenHeight":0,"ScreenRefreshRate":0,"Fullscreen":false,"Vsync":0,"AntiAliasing":0,"Batchmode":false,"RenderThreadingMode":"MultiThreaded","GpuSkinning":false,"Platform":"","ColorSpace":"","AnisotropicFiltering":"","BlendWeights":"","GraphicsApi":"","ScriptingBackend":"Mono2x","AndroidTargetSdkVersion":"AndroidApiLevelAuto","AndroidBuildSystem":"Gradle","BuildTarget":"StandaloneWindows64","StereoRenderingPath":"MultiPass"},"Hardware":{"OperatingSystem":"","DeviceModel":"","DeviceName":"","ProcessorType":"","ProcessorCount":0,"GraphicsDeviceName":"","SystemMemorySizeMB":0},"Editor":{"Version":"6000.0.0f1","Branch":"6000.0/release","Changeset":"4ff56b3ea44c","Date":1713989104},"Dependencies":["com.unity.2d.animation@10.1.1","com.unity.2d.pixel-perfect@5.0.3","com.unity.2d.psdimporter@9.0.3","com.unity.2d.sprite@1.0.0","com.unity.2d.spriteshape@10.0.4","com.unity.2d.tilemap@1.0.0","com.unity.ai.navigation@2.0.0","com.unity.collab-proxy@2.3.1","com.unity.ide.rider@3.0.28","com.unity.ide.visualstudio@2.0.22","com.unity.memoryprofiler@1.1.0","com.unity.mobile.android-logcat@1.4.2","com.unity.render-pipelines.universal@17.0.3","com.unity.test-framework@1.4.3","com.unity.timeline@1.8.6","com.unity.toolchain.win-x86_64-linux-x86_64@2.0.6","com.unity.ugui@2.0.0","com.unity.modules.accessibility@1.0.0","com.unity.modules.ai@1.0.0","com.unity.modules.androidjni@1.0.0","com.unity.modules.animation@1.0.0","com.unity.modules.assetbundle@1.0.0","com.unity.modules.audio@1.0.0","com.unity.modules.cloth@1.0.0","com.unity.modules.director@1.0.0","com.unity.modules.imageconversion@1.0.0","com.unity.modules.imgui@1.0.0","com.unity.modules.jsonserialize@1.0.0","com.unity.modules.particlesystem@1.0.0","com.unity.modules.physics@1.0.0","com.unity.modules.physics2d@1.0.0","com.unity.modules.screencapture@1.0.0","com.unity.modules.terrain@1.0.0","com.unity.modules.terrainphysics@1.0.0","com.unity.modules.tilemap@1.0.0","com.unity.modules.ui@1.0.0","com.unity.modules.uielements@1.0.0","com.unity.modules.umbra@1.0.0","com.unity.modules.unityanalytics@1.0.0","com.unity.modules.unitywebrequest@1.0.0","com.unity.modules.unitywebrequestassetbundle@1.0.0","com.unity.modules.unitywebrequestaudio@1.0.0","com.unity.modules.unitywebrequesttexture@1.0.0","com.unity.modules.unitywebrequestwww@1.0.0","com.unity.modules.vehicles@1.0.0","com.unity.modules.video@1.0.0","com.unity.modules.vr@1.0.0","com.unity.modules.wind@1.0.0","com.unity.modules.xr@1.0.0","com.unity.modules.subsystems@1.0.0","com.unity.modules.hierarchycore@1.0.0","com.unity.sysroot@2.0.7","com.unity.sysroot.linux-x86_64@2.0.6","com.unity.ext.nunit@2.0.5","com.unity.mathematics@1.3.1","com.unity.burst@1.8.13","com.unity.render-pipelines.core@17.0.3","com.unity.shadergraph@17.0.3","com.unity.render-pipelines.universal-config@17.0.3","com.unity.editorcoroutines@1.0.0","com.unity.2d.common@9.0.4","com.unity.collections@2.4.0","com.unity.searcher@4.9.2","com.unity.rendering.light-transport@1.0.1","com.unity.nuget.mono-cecil@1.11.4","com.unity.test-framework.performance@3.0.3"],"Results":[]}"#,
        ];
        for name in [
            "PerformanceTestRun",
            "PerformanceTestRunSettings",
            "PerformanceTestRunInfo",
            "PerformanceTestConfig",
        ] {
            for script in scripts {
                let bytes = crate::unity_serialized::write_v17_fixture_ex(name, script, None);
                let rows = UnityPlugin::extract_strings_from_assets(
                    &bytes,
                    "resources.assets",
                    Path::new("resources.assets"),
                );
                assert!(
                    rows.is_empty(),
                    "{name} leaked {} rows: {rows:?}",
                    rows.len()
                );
            }
        }
    }

    #[test]
    fn test_performance_filter_keeps_player_json_and_braces_injectable() {
        for (name, source, translated) in [
            (
                "UI",
                r#"{"MENU":"Continue","DIALOGUE":"Wait {player}!"}"#,
                r#"{"MENU":"Adelante","DIALOGUE":"Hola {player}!"}"#,
            ),
            (
                "PerformanceTestRun",
                r#"{"MENU":"Continue","DIALOGUE":"Wait {player}!"}"#,
                r#"{"MENU":"Adelante","DIALOGUE":"Hola {player}!"}"#,
            ),
            (
                "PerformanceTestConfig",
                r#"{"MeasurementCount":"One more try!"}"#,
                r#"{"MeasurementCount":"Otro intento!"}"#,
            ),
            (
                "Dialogue",
                "Hello {player}, welcome back!",
                "Salud {player}, welcome back!",
            ),
            (
                "PerformanceTestRunInfo",
                "Hello {player}, welcome back!",
                "Salud {player}, welcome back!",
            ),
            (
                "Dialogue",
                r#"{"MeasurementCount":-1}"#,
                r#"{"MeasurementCount":12}"#,
            ),
        ] {
            let dir = tempdir();
            let path = dir.join("resources.assets");
            let before = crate::unity_serialized::write_v17_fixture_ex(name, source, None);
            fs::write(&path, &before).unwrap();
            let plugin = UnityPlugin::new();
            let mut rows = plugin.extract(&path).unwrap();
            assert_eq!(rows.len(), 1, "{name}: {rows:?}");
            assert_eq!(rows[0].source, source);
            rows[0].translation = Some(translated.into());
            let report = plugin.inject(&dir, &rows).unwrap();
            assert_eq!(report.strings_written, 1, "{name}: {report:?}");
            assert_eq!(report.strings_skipped, 0);
            let expected = crate::unity_serialized::write_v17_fixture_ex(name, translated, None);
            assert_eq!(fs::read(&path).unwrap(), expected, "{name}");
            assert_eq!(plugin.extract(&path).unwrap()[0].source, translated);
        }
    }

    #[test]
    fn test_runtime_configuration_is_not_extracted() {
        for class_id in [13, 78, 94] {
            let bytes = crate::unity_serialized::write_v17_technical_noise_fixture(
                class_id,
                &["left ctrl", "Debug Persistent", "joystick button 2"],
            );
            let entries = UnityPlugin::extract_strings_from_assets(
                &bytes,
                "globalgamemanagers",
                Path::new("globalgamemanagers"),
            );
            assert_eq!(
                entries.len(),
                1,
                "only the TextAsset should remain: {entries:?}"
            );
            assert_eq!(entries[0].source, "Hello traveler welcome!");
        }
    }

    #[test]
    fn test_runtime_configuration_old_heuristic_translations_cannot_write() {
        for class_id in [13, 78, 94] {
            for pinned in [false, true] {
                let dir = tempdir();
                let data_dir = dir.join("TestGame_Data");
                fs::create_dir_all(&data_dir).unwrap();
                fs::write(dir.join("UnityPlayer.dll"), b"fake").unwrap();
                let bytes = crate::unity_serialized::write_v17_technical_noise_fixture(
                    class_id,
                    &["left ctrl", "Debug Persistent"],
                );
                let assets = data_dir.join("globalgamemanagers");
                fs::write(&assets, &bytes).unwrap();
                let sf = SerializedFile::parse(bytes.clone(), &assets).unwrap();
                let offset = sf
                    .objects
                    .iter()
                    .find(|o| o.class_id == class_id)
                    .unwrap()
                    .data_abs;
                let mut entry = StringEntry::new("old-control", "left ctrl", assets.clone());
                entry.translation = Some("tecla".into());
                if pinned {
                    entry
                        .metadata
                        .insert("binary_offset".into(), serde_json::json!(offset));
                    entry
                        .metadata
                        .insert("length_endian".into(), serde_json::json!("le"));
                }
                let report = UnityPlugin::new().inject(&dir, &[entry]).unwrap();
                assert_eq!(report.strings_written, 0, "pinned={pinned}");
                assert_eq!(report.skip_reasons.get("invalid_target"), Some(&1));
                assert_eq!(fs::read(&assets).unwrap(), bytes);
            }
        }
    }

    #[test]
    fn test_is_translatable() {
        assert!(is_unity_translatable("Hello World"));
        assert!(is_unity_translatable("Press any key to continue"));
        assert!(is_unity_translatable("Hello")); // plain word, not code id
        assert!(is_unity_translatable("Save game"));
        assert!(is_unity_translatable("設定"));
        assert!(is_unity_translatable("保存"));
        assert!(is_unity_translatable("继续"));
        assert!(is_unity_translatable(
            "物語を続けますか？次の章を始めますか？"
        ));
        assert!(is_unity_translatable("Configuración"));
        assert!(is_unity_translatable("Überprüfung"));
        assert!(is_unity_translatable("Настройки"));
        assert!(!is_unity_translatable("Save◆◇※☆"));
        assert!(!is_unity_translatable("abc"));
        assert!(!is_unity_translatable("中")); // byte heuristics require two CJK letters
        assert!(!is_unity_translatable("◆◇※☆"));
        assert!(!is_unity_translatable("保存\u{1}"));
        assert!(!is_unity_translatable("\u{FFFD}設定"));
        assert!(!is_unity_translatable("SOME_CONSTANT_NAME"));
        assert!(!is_unity_translatable("Assets/Textures/player.png"));
        assert!(!is_unity_translatable("素材/画像/背景.png"));
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
        // Asset paths with spaces (mono + heuristic)
        assert!(!is_unity_translatable("Naninovel/Audio/BGM/Erotic 01"));
        assert!(!is_unity_translatable("Tilemap/Pillar Sprite_11"));
        assert!(!is_unity_translatable("Day/1 Centered"));
        // Real UI / dialogue with slash must still pass when clearly sentence-like.
        assert!(is_unity_translatable("Press Start"));
        assert!(is_unity_translatable("Save game now"));
        assert!(is_unity_translatable("Delete")); // short UI verb
        assert!(is_unity_translatable("Progress")); // short UI label
        assert!(is_unity_translatable("Clothing")); // inventory category
        assert!(is_unity_translatable("START / LOAD"));
        assert!(is_unity_translatable("Fridge / Microwave"));
    }

    #[test]
    fn test_structural_textasset_and_mono_extract_cjk_fields() {
        let dir = tempdir();
        let data_dir = dir.join("TestGame_Data");
        fs::create_dir_all(&data_dir).unwrap();
        fs::write(dir.join("UnityPlayer.dll"), b"fake").unwrap();

        let dialogue = "今日は静かな町を歩いて、古い友人に会いに行きます。";
        fs::write(
            data_dir.join("text.assets"),
            crate::unity_serialized::write_v17_fixture("会話", dialogue),
        )
        .unwrap();
        fs::write(
            data_dir.join("mono.assets"),
            crate::unity_serialized::write_v17_mono_fixture("メニュー", &["設定", "保存"]),
        )
        .unwrap();
        // A structurally identified m_Text can safely retain a single CJK glyph.
        fs::write(
            data_dir.join("textmesh.assets"),
            crate::unity_serialized::write_v17_textmesh_fixture("戻"),
        )
        .unwrap();
        // Structural framing alone does not make random symbol payloads text.
        fs::write(
            data_dir.join("symbols.assets"),
            crate::unity_serialized::write_v17_textmesh_fixture("◆◇※☆"),
        )
        .unwrap();

        let entries = UnityPlugin::new().extract(&dir).unwrap();
        assert!(entries.iter().any(|e| {
            e.source == dialogue
                && e.metadata.get("extraction_method").and_then(|v| v.as_str()) == Some("textasset")
        }));
        for source in ["メニュー", "設定", "保存"] {
            assert!(
                entries.iter().any(|e| {
                    e.source == source
                        && e.metadata.get("extraction_method").and_then(|v| v.as_str())
                            == Some("monobehaviour")
                }),
                "missing structural MonoBehaviour {source:?}: {:?}",
                entries.iter().map(|e| &e.source).collect::<Vec<_>>()
            );
        }
        assert!(entries.iter().any(|e| {
            e.source == "戻"
                && e.metadata.get("extraction_method").and_then(|v| v.as_str()) == Some("textmesh")
        }));
        assert!(
            !entries.iter().any(|e| e.source == "◆◇※☆"),
            "symbol-only structural payload must stay excluded"
        );
    }

    #[test]
    fn test_textasset_skips_linebreak_charset_tables() {
        let dir = tempdir();
        let data_dir = dir.join("TestGame_Data");
        fs::create_dir_all(&data_dir).unwrap();
        fs::write(dir.join("UnityPlayer.dll"), b"fake").unwrap();
        // Near-zero alphabetic content — TMP line-break character class table.
        let charset = "([｛〔〈《「『【〘〖〝‘“｟«$—…‥〳〴〵\\［（{£¥\"々〇〉》」＄｠￥￦ #)]｝〕〉》」』】〙〗〟’”｠»";
        let kana_table = ")]｝〕〉》」』】〙〗〟’”｠»ヽゴミ袋ァィゥェォッャュョヮヵヶぁぃぅぇぉっゃゅょゎゕゖㇰㇱㇲㇳㇴㇵㇶㇷㇸㇹㇺㇻㇼㇽㇾㇿ々〻‐゠–〜?!‼⁇⁈⁉・、%,.:;。！？］）：；＝}¢°\"†‡℃〆％，．";
        assert!(
            !crate::unity_serialized::is_textasset_script_worth_extracting(charset),
            "charset table must be rejected"
        );
        assert!(
            !is_unity_textasset_script_worth_extracting(kana_table),
            "multilingual recovery must not reopen small-kana charset tables"
        );
        assert!(
            crate::unity_serialized::is_textasset_script_worth_extracting(
                "TitleMenu.START: NEW GAME\r\nTitleMenu.CREDITS: CREDITS"
            )
        );
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

    #[test]
    fn test_textasset_skips_tech_docs_and_lorem_loc() {
        assert!(is_non_player_textasset_name(
            "LineBreaking Leading Characters"
        ));
        assert!(is_non_player_textasset_name(
            "TECHNICAL SPECIFICATIONS — SFX DELIVERY"
        ));
        assert!(is_non_player_textasset_name("CharacterNames"));
        assert!(!is_non_player_textasset_name("DefaultUI"));
        assert!(!is_non_player_textasset_name("START"));

        let tech = "\r\n\r\n**TECHNICAL SPECIFICATIONS — SFX PACKAGE**  \r\n**Date:** 2025-12-03\r\n\r\n**Format**\r\n\r\nWAV 48kHz";
        assert!(looks_like_internal_tech_doc(tech));
        assert!(!looks_like_internal_tech_doc(
            "TitleMenu.START: NEW GAME\r\nTitleMenu.CREDITS: CREDITS"
        ));

        let dir = tempdir();
        let data_dir = dir.join("TestGame_Data");
        fs::create_dir_all(&data_dir).unwrap();
        fs::write(dir.join("UnityPlayer.dll"), b"fake").unwrap();

        // A recognized CharacterNames value table keeps only display values.
        let names = "Carter: Carter\r\nEmily: Emily\r\nJake: Jake\r\n";
        let bytes = crate::unity_serialized::write_v17_fixture_ex("CharacterNames", names, None);
        fs::write(data_dir.join("sharedassets0.assets"), &bytes).unwrap();
        let entries = UnityPlugin::new().extract(&dir).unwrap();
        assert_eq!(
            entries
                .iter()
                .map(|e| e.source.as_str())
                .collect::<Vec<_>>(),
            ["Carter", "Emily", "Jake"]
        );

        // Tech doc by body content (generic m_Name)
        let bytes = crate::unity_serialized::write_v17_fixture("Notes", tech);
        fs::write(data_dir.join("sharedassets0.assets"), &bytes).unwrap();
        let entries = UnityPlugin::new().extract(&dir).unwrap();
        assert!(
            !entries
                .iter()
                .any(|e| e.source.contains("TECHNICAL SPECIFICATIONS")),
            "tech doc must not extract: {:?}",
            entries.iter().map(|e| &e.source).collect::<Vec<_>>()
        );

        // Lorem preview line dropped from ManagedText; real UI kept.
        let ui = "\
TitleMenu.START: NEW GAME\r\n\
SettingsMenu.PreviewText: Lorem ipsum dolor sit amet, consectetur adipiscing elit\r\n\
Confirmation.Yes: YES\r\n";
        let bytes = crate::unity_serialized::write_v17_fixture("DefaultUI", ui);
        fs::write(data_dir.join("sharedassets0.assets"), &bytes).unwrap();
        let entries = UnityPlugin::new().extract(&dir).unwrap();
        let sources: Vec<&str> = entries.iter().map(|e| e.source.as_str()).collect();
        assert!(sources.contains(&"NEW GAME"), "keep UI: {sources:?}");
        assert!(sources.contains(&"YES"), "keep YES: {sources:?}");
        assert!(
            !sources
                .iter()
                .any(|s| s.to_ascii_lowercase().contains("lorem")),
            "drop lorem loc value: {sources:?}"
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
            !sources
                .iter()
                .any(|s| *s == "Naninovel" || *s == "QuaternionTween"),
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
        for e in &ta {
            assert_inplace_char_limit(e);
        }
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
            parsed
                .cells
                .iter()
                .any(|c| c.value == "mp3 player" && c.header == "ITEM_NAME"),
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
                e.metadata.get("extraction_method").and_then(|v| v.as_str())
                    == Some("textasset_csv_cell")
            })
            .collect();
        assert!(
            cells.len() >= 4,
            "expected csv cells, got {:?}",
            entries
                .iter()
                .map(|e| (&e.id, &e.source))
                .collect::<Vec<_>>()
        );
        assert!(cells.iter().any(|e| e.source == "towel"));
        assert!(
            cells.iter().all(|e| e.char_limit.is_none()),
            "csv cells share one TextAsset blob; char_limit must stay unset: {:?}",
            cells
                .iter()
                .map(|e| (&e.id, e.char_limit))
                .collect::<Vec<_>>()
        );
        assert!(
            cells.iter().all(|e| {
                match locust_core::textasset_group::parse_group_meta(e) {
                    Ok(meta) => {
                        let original: &str = meta.original.as_ref();
                        original == script
                            && meta.capacity >= script.len()
                            && Some(script.len() as u64)
                                == e.metadata
                                    .get("textasset_script_byte_len")
                                    .and_then(|v| v.as_u64())
                    }
                    Err(_) => false,
                }
            }),
            "csv cells must record whole-blob original and script_byte_len capacity"
        );

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
                e.metadata.get("extraction_method").and_then(|v| v.as_str())
                    == Some("textasset_csv_cell")
            })
            .map(|e| e.source.as_str())
            .collect();
        assert!(values.contains(&"toalla"), "{values:?}");
        assert!(values.contains(&"mp3"), "{values:?}");
        assert!(values.contains(&"video games"), "{values:?}");
    }

    #[test]
    fn test_parse_textasset_loc_lines_key_value() {
        let doc =
            "TitleMenu.START: NEW GAME\r\nTitleMenu.CREDITS: CREDITS\r\nConfirmation.Yes: YES\r\n";
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
                e.metadata.get("extraction_method").and_then(|v| v.as_str())
                    == Some("textasset_loc_line")
            })
            .collect();
        assert!(
            loc.len() >= 3,
            "expected loc lines, got {:?}",
            entries
                .iter()
                .map(|e| (&e.id, &e.source))
                .collect::<Vec<_>>()
        );
        assert!(loc.iter().any(|e| e.source == "NEW GAME"));
        assert!(loc.iter().any(|e| e.source == "CREDITS"));
        assert!(loc.iter().any(|e| {
            e.metadata.get("loc_key").and_then(|v| v.as_str()) == Some("TitleMenu.START")
        }));
        assert!(
            loc.iter().all(|e| e.char_limit.is_none()),
            "loc lines share one TextAsset blob; char_limit must stay unset: {:?}",
            loc.iter()
                .map(|e| (&e.id, e.char_limit))
                .collect::<Vec<_>>()
        );
        assert!(
            loc.iter().all(|e| {
                match locust_core::textasset_group::parse_group_meta(e) {
                    Ok(meta) => {
                        let original: &str = meta.original.as_ref();
                        original == script
                            && meta.capacity >= script.len()
                            && Some(script.len() as u64)
                                == e.metadata
                                    .get("textasset_script_byte_len")
                                    .and_then(|v| v.as_u64())
                    }
                    Err(_) => false,
                }
            }),
            "loc lines must record whole-blob original and script_byte_len capacity"
        );

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
                e.metadata.get("extraction_method").and_then(|v| v.as_str())
                    == Some("textasset_loc_line")
            })
            .map(|e| e.source.as_str())
            .collect();
        assert!(values.contains(&"NUEVA"), "re-extract values: {values:?}");
        assert!(values.contains(&"SI"), "re-extract values: {values:?}");
        assert!(
            values.contains(&"CREDITS"),
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
    fn structural_extract_avoids_path_id_table_searches() {
        use crate::unity_serialized::{write_v17_mixed_objects_fixture, PATH_ID_LOOKUP_STEPS};

        let bytes = write_v17_mixed_objects_fixture(200);
        let path = Path::new("mixed.assets");
        let sf = SerializedFile::parse(bytes.clone(), path).unwrap();
        assert_eq!(sf.objects.len(), 2_000);
        assert_eq!(sf.mono_behaviour_objects().count(), 1_000);
        assert_eq!(sf.text_mesh_objects().count(), 200);
        assert_eq!(sf.gui_text_objects().count(), 200);

        // Reference the public path_id readers in the same class/table order.
        // Do not sort or dedupe: repeated labels still need distinct entries.
        PATH_ID_LOOKUP_STEPS.with(|steps| steps.set(0));
        let mut expected = Vec::new();
        for obj in sf.mono_behaviour_objects() {
            for field in sf.read_mono_strings(obj.path_id).unwrap() {
                assert!(is_structural_unity_text(&field.text));
                expected.push((
                    format!("monobehaviour/{}/{}", field.path_id, field.field_index),
                    field.text,
                ));
            }
        }
        for obj in sf.text_mesh_objects() {
            let field = sf.read_text_mesh(obj.path_id).unwrap();
            assert!(is_structural_unity_text(&field.text));
            expected.push((format!("textmesh/{}", field.path_id), field.text));
        }
        for obj in sf.gui_text_objects() {
            let field = sf.read_gui_text(obj.path_id).unwrap();
            assert!(is_structural_unity_text(&field.text));
            expected.push((format!("guitext/{}", field.path_id), field.text));
        }
        assert_eq!(expected.len(), 3_000);
        assert_eq!(
            PATH_ID_LOOKUP_STEPS.with(|steps| steps.replace(0)),
            1_401_400,
            "the reference must count every object examined, including other classes"
        );

        let entries = UnityPlugin::extract_strings_from_assets(&bytes, "mixed.assets", path);
        assert_eq!(
            PATH_ID_LOOKUP_STEPS.with(|steps| steps.replace(0)),
            0,
            "structural extraction must use the objects it already iterates"
        );
        assert_eq!(
            entries
                .into_iter()
                .map(|entry| (entry.id, entry.source))
                .collect::<Vec<_>>(),
            expected,
            "ids, sources and order must match the public path_id readers"
        );
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
            entries
                .iter()
                .map(|e| (&e.id, &e.source))
                .collect::<Vec<_>>()
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
        for e in &mono {
            assert_inplace_char_limit(e);
        }
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
                .any(|e| e.tags.iter().any(|t| t == "monobehaviour")
                    && e.source.starts_with("Hola!")),
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
            entries
                .iter()
                .map(|e| (&e.id, &e.source))
                .collect::<Vec<_>>()
        );
        assert!(hits.iter().all(|e| {
            e.metadata.get("extraction_method").and_then(|v| v.as_str()) == Some("heuristic")
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
        let count = out.windows(6).filter(|w| w == b"Pulsa!").count();
        assert!(count >= 2, "expected ≥2 rewritten payloads, found {count}");
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
            entries
                .iter()
                .map(|e| (&e.id, &e.source))
                .collect::<Vec<_>>()
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
            entries
                .iter()
                .map(|e| (&e.id, &e.source))
                .collect::<Vec<_>>()
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
        for e in &tm {
            assert_inplace_char_limit(e);
        }
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
            entries
                .iter()
                .map(|e| (&e.id, &e.source))
                .collect::<Vec<_>>()
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
        for e in &gt {
            assert_inplace_char_limit(e);
        }
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
    fn test_legacy_textasset_inject_oversize_skips() {
        let dir = tempdir();
        create_textasset_assets_fixture(&dir);
        let plugin = UnityPlugin::new();
        let mut entries = plugin.extract(&dir).unwrap();
        for e in &mut entries {
            e.metadata.remove("textasset_rewrite");
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
    #[test]
    fn display_names_declaration_is_one_quoted_slot() {
        let fixture = tempfile::tempdir().unwrap();
        let path = fixture.path().join("Characters.txt");
        let before = "character CJ \"CJ\"\r\nCJ Ordinary dialogue\r\ncharacter Nar\r\ncharacter Bad # \"comment\"\r\n";
        fs::write(&path, before).unwrap();
        let mut entries = UnityPlugin::extract_text_scripts(fixture.path()).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].source, "CJ");
        assert_eq!(entries[0].metadata["record_kind"], "character_display_name");
        assert_eq!(entries[0].metadata["character_id"], "CJ");
        assert_eq!(entries[1].source, "Ordinary dialogue");
        entries[0].translation = Some("Carlos".into());
        let report = UnityPlugin::inject_text_scripts(fixture.path(), &[&entries[0]]).unwrap();
        assert_eq!(report.strings_written, 1);
        assert_eq!(
            fs::read(&path).unwrap(),
            before.replacen("\"CJ\"", "\"Carlos\"", 1).as_bytes()
        );
    }

    #[test]
    fn display_names_declaration_escapes_and_exact_bytes() {
        let fixture = tempfile::tempdir().unwrap();
        let path = fixture.path().join("Characters.txt");
        let before = concat!(
            "\tcharacter\tTs  000000 \"\\p<link=\\\"s:whiteblack\\\">\\pTsukiko\\p</link>\\p\"  # \"Tsukiko\"\r\n",
            "  character Q FFFFFF \"A \\\"quoted\\\" name\\\\\" # unchanged\n",
            "  character CJ \"CJ\""
        );
        fs::write(&path, before).unwrap();
        let mut entries = UnityPlugin::extract_text_scripts(fixture.path()).unwrap();
        assert_eq!(entries.len(), 3);
        entries[0].translation = Some(entries[0].source.replace("Tsukiko", "Luna"));
        entries[1].translation = Some(entries[1].source.replace("name", "nombre"));
        entries[2].translation = Some("Carlos".into());
        let refs: Vec<_> = entries.iter().collect();
        let report = UnityPlugin::inject_text_scripts(fixture.path(), &refs).unwrap();
        assert_eq!(report.strings_written, 3);
        let expected = before
            .replacen("Tsukiko", "Luna", 1)
            .replacen("name", "nombre", 1)
            .replacen("\"CJ\"", "\"Carlos\"", 1);
        assert_eq!(fs::read(&path).unwrap(), expected.as_bytes());
    }

    #[test]
    fn display_names_declaration_rejects_stale_or_unsafe_writes() {
        let fixture = tempfile::tempdir().unwrap();
        let path = fixture.path().join("Characters.txt");
        let before = "  character CJ FF0253 \"CJ\" # comment\r\n";
        fs::write(&path, before).unwrap();
        let mut entry = UnityPlugin::extract_text_scripts(fixture.path())
            .unwrap()
            .remove(0);
        for changed in [
            before.replace("CJ FF", "XX FF"),
            before.replace("FF0253", "FFFFFF"),
            before.replace("\"CJ\"", "\"XX\""),
            before.replace("# comment", "# changed"),
            before.replace("character", "CJ       "),
        ] {
            fs::write(&path, &changed).unwrap();
            entry.translation = Some("Carlos".into());
            let report = UnityPlugin::inject_text_scripts(fixture.path(), &[&entry]).unwrap();
            assert_eq!(
                report.skip_reasons.get("source_changed"),
                Some(&1),
                "{report:?}"
            );
            assert_eq!(report.strings_written, 0);
            assert_eq!(fs::read(&path).unwrap(), changed.as_bytes());
        }
        fs::write(&path, before).unwrap();
        for (translation, reason) in [
            ("Carl\"os", "error"),
            ("Carlos\\", "error"),
            ("Carl\nos", "error"),
            ("Carl\ros", "error"),
            ("Carl\0os", "error"),
            ("Carl\tos", "error"),
            ("\\bCarlos\\b", "unsafe_controls"),
            ("Carlos<link=x>", "unsafe_controls"),
            ("Carlos\\\"", "unsafe_controls"),
        ] {
            entry.translation = Some(translation.into());
            let report = UnityPlugin::inject_text_scripts(fixture.path(), &[&entry]).unwrap();
            assert_eq!(
                report.skip_reasons.get(reason),
                Some(&1),
                "{translation:?}: {report:?}"
            );
            assert_eq!(report.files_modified, 0);
            assert_eq!(fs::read(&path).unwrap(), before.as_bytes());
        }
        entry.translation = Some(entry.source.clone());
        let report = UnityPlugin::inject_text_scripts(fixture.path(), &[&entry]).unwrap();
        assert_eq!(report.skip_reasons.get("unchanged"), Some(&1));
        entry.metadata.remove("record_kind");
        entry.translation = Some("Carlos".into());
        let report = UnityPlugin::inject_text_scripts(fixture.path(), &[&entry]).unwrap();
        assert_eq!(report.skip_reasons.get("invalid_target"), Some(&1));
        assert_eq!(fs::read(&path).unwrap(), before.as_bytes());
    }

    #[test]
    fn display_names_declaration_uses_physical_source_after_glossary_pivot() {
        let fixture = tempfile::tempdir().unwrap();
        let path = fixture.path().join("Characters.txt");
        fs::write(&path, "character CJ \"CJ\"\n").unwrap();
        let mut entry = UnityPlugin::extract_text_scripts(fixture.path())
            .unwrap()
            .remove(0);
        entry.metadata.insert(
            locust_core::models::INJECTION_SOURCE_METADATA_KEY.into(),
            serde_json::json!("CJ"),
        );
        entry.source = "Charles".into();
        entry.translation = Some("Carlos".into());
        let report = UnityPlugin::inject_text_scripts(fixture.path(), &[&entry]).unwrap();
        assert_eq!(report.strings_written, 1);
        assert_eq!(fs::read(&path).unwrap(), b"character CJ \"Carlos\"\n");
    }

    #[test]
    fn display_names_character_names_table_round_trip() {
        let fixture = tempfile::tempdir().unwrap();
        let path = fixture.path().join("resources.assets");
        let script = "Carter: Carter\r\n\r\nEmily: Emily\r\nJake: Jake\r\n";
        let before = crate::unity_serialized::write_v17_fixture_ex("CharacterNames", script, None);
        fs::write(&path, &before).unwrap();
        let plugin = UnityPlugin::new();
        let mut entries = plugin.extract(&path).unwrap();
        assert_eq!(entries.len(), 3);
        assert_eq!(
            entries
                .iter()
                .map(|e| e.source.as_str())
                .collect::<Vec<_>>(),
            ["Carter", "Emily", "Jake"]
        );
        assert_eq!(
            entries
                .iter()
                .map(|e| &e.id)
                .collect::<std::collections::HashSet<_>>()
                .len(),
            3
        );
        for entry in &mut entries {
            entry.translation = Some(entry.source.clone());
        }
        let report = plugin.inject(fixture.path(), &entries).unwrap();
        assert_eq!(report.skip_reasons.get("unchanged"), Some(&3));
        assert_eq!(fs::read(&path).unwrap(), before);
        entries[0].translation = Some("Bad\nName".into());
        let report = plugin.inject(fixture.path(), &entries).unwrap();
        assert_eq!(report.strings_written, 0, "{report:?}");
        assert_eq!(report.skip_reasons.get("error"), Some(&1));
        assert_eq!(fs::read(&path).unwrap(), before);
        let stale = crate::unity_serialized::write_v17_fixture_ex(
            "CharacterNames",
            &script.replacen("Carter:", "Karter:", 1),
            None,
        );
        fs::write(&path, &stale).unwrap();
        entries[0].translation = Some("Carlos".into());
        let report = plugin.inject(fixture.path(), &entries).unwrap();
        assert_eq!(report.strings_written, 0, "{report:?}");
        assert_eq!(fs::read(&path).unwrap(), stale);
        fs::write(&path, &before).unwrap();
        for (entry, translation) in entries.iter_mut().zip(["Carlos", "Emili", "Juan"]) {
            entry.translation = Some(translation.into());
        }
        entries[0].metadata.insert(
            locust_core::models::INJECTION_SOURCE_METADATA_KEY.into(),
            serde_json::json!("Carter"),
        );
        entries[0].source = "Glossary Carter".into();
        let report = plugin.inject(fixture.path(), &entries).unwrap();
        assert_eq!(report.strings_written, 3, "{report:?}");
        let expected = crate::unity_serialized::write_v17_fixture_ex(
            "CharacterNames",
            "Carter: Carlos\r\n\r\nEmily: Emili\r\nJake: Juan\r\n",
            None,
        );
        assert_eq!(fs::read(&path).unwrap(), expected);
        let changed = fs::read(&path).unwrap();
        let report = plugin.inject(fixture.path(), &entries).unwrap();
        assert_eq!(report.strings_written, 0, "{report:?}");
        assert!(report.strings_skipped >= 3, "{report:?}");
        assert_eq!(fs::read(&path).unwrap(), changed);
    }

    #[test]
    fn display_names_character_names_schema_and_technical_ids() {
        let fixture = tempfile::tempdir().unwrap();
        let path = fixture.path().join("resources.assets");
        let plugin = UnityPlugin::new();
        for (name, script) in [
            ("CharacterNames", "Carter\nEmily\nJake\n"),
            ("CharacterNames", "Carter: Carter\nnot a table\n"),
            ("CharacterNames", "A B: Display name\n"),
            ("Character Names", "Carter: Carter\nEmily: Emily\n"),
            ("LineBreaking Leading Characters", "Carter: Carter\nEmily: Emily\n"),
            ("TECHNICAL SPECIFICATIONS", "Carter: Carter\nEmily: Emily\n"),
            ("CharacterNames", "af: Afrikaans\nes: Spanish\nen: English\nfr: French\nde: German\nit: Italian\npt: Portuguese\nja: Japanese\n"),
        ] {
            fs::write(&path, crate::unity_serialized::write_v17_fixture_ex(name, script, None)).unwrap();
            assert!(plugin.extract(&path).unwrap().is_empty(), "{name}: {script}");
        }
        let script = "CJ: CJ\r\nTMP_FontAsset: TMP_FontAsset\r\nLiftGammaGain: LiftGammaGain\r\n";
        fs::write(
            &path,
            crate::unity_serialized::write_v17_fixture_ex("CharacterNames", script, None),
        )
        .unwrap();
        let mut entries = plugin.extract(&path).unwrap();
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].source, "CJ");
        entries[1].translation = Some("Nombre".into());
        let report = plugin.inject(fixture.path(), &entries).unwrap();
        assert_eq!(report.strings_written, 1, "{report:?}");
        let sf = SerializedFile::parse(fs::read(&path).unwrap(), &path).unwrap();
        assert_eq!(
            sf.read_text_asset(1).unwrap().script,
            script.replace("TMP_FontAsset: TMP_FontAsset", "TMP_FontAsset: Nombre")
        );
    }

    #[test]
    fn display_names_preserve_legacy_heuristic_ids() {
        let path = Path::new("resources.assets");
        let mut bytes = crate::unity_serialized::write_v17_fixture_ex(
            "CharacterNames",
            "Carter: Carter\nEmily: Emily\nJake: Jake\n",
            None,
        );
        let dialogue = "Additional visible dialogue!";
        bytes.extend_from_slice(&(dialogue.len() as u32).to_le_bytes());
        bytes.extend_from_slice(dialogue.as_bytes());
        let file_size = bytes.len() as u32;
        bytes[4..8].copy_from_slice(&file_size.to_be_bytes());
        let mut glossary = bytes.clone();
        let colon = glossary
            .windows(b"Carter: Carter".len())
            .position(|v| v == b"Carter: Carter")
            .unwrap()
            + "Carter".len();
        glossary[colon] = b';'; // same offsets, glossary-only fallback
        let before = UnityPlugin::extract_strings_from_assets(&glossary, "resources.assets", path);
        let after = UnityPlugin::extract_strings_from_assets(&bytes, "resources.assets", path);
        assert_eq!(before.len(), 1);
        assert_eq!(after.len(), 4);
        let old = &before[0];
        let new = after.iter().find(|e| e.source == dialogue).unwrap();
        assert_eq!(new.id, old.id);
        assert_eq!(new.metadata, old.metadata);
    }
}
