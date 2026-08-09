use std::collections::HashMap;
use std::path::{Path, PathBuf};

use locust_core::encoding::EncodingDetector;
use locust_core::error::{LocustError, Result};
use locust_core::extraction::{FormatPlugin, InjectionReport};
use locust_core::models::{OutputMode, StringEntry};

const ARRAY_FILES: &[&str] = &[
    "Actors", "Classes", "Skills", "Items", "Weapons", "Armors", "Enemies", "States", "Troops",
];

const EXTRACTABLE_FIELDS: &[(&str, &str)] = &[
    ("name", "actor_name"),
    ("description", "description"),
    ("profile", "description"),
    ("message1", "dialogue"),
    ("message2", "dialogue"),
    ("message3", "dialogue"),
    ("message4", "dialogue"),
];

/// System.json type arrays that are shown to the player in menus.
const SYSTEM_TYPE_ARRAYS: &[(&str, &str)] = &[
    ("armorTypes", "ui_label"),
    ("elements", "ui_label"),
    ("equipTypes", "ui_label"),
    ("skillTypes", "ui_label"),
    ("weaponTypes", "ui_label"),
];

/// Known text-display plugin command prefixes (MV code 356).
const TEXT_DISPLAY_PLUGINS: &[&str] = &[
    "D_TEXT",      // Dynamic Text plugin (shows text on screen)
    "SHOW_TEXT",   // Various show-text plugins
    "T_TEXT",      // Text plugins
    "GN_TEXT",     // Game Note text
];

/// Argument keys in MZ plugin commands (code 357) that contain translatable text.
const MZ_TRANSLATABLE_ARG_KEYS: &[&str] = &[
    "text",        // DTextPicture text
    "destination", // DestinationWindow quest objective
    "label",       // Choice labels (may be nested JSON)
    "message",     // Various message texts
    "description", // Description fields
    "choices",     // Choice arrays (nested JSON with labels)
];

/// Extract translatable text from a plugin command (code 356).
/// Returns Some(full_command) only if it's a known text-display plugin command
/// containing CJK characters. Returns None for technical/audio/system commands.
fn extract_plugin_command_text(command: &str) -> Option<String> {
    let trimmed = command.trim();
    if trimmed.is_empty() {
        return None;
    }
    // Check if the command starts with a known text-display plugin prefix
    for prefix in TEXT_DISPLAY_PLUGINS {
        if trimmed.starts_with(prefix) {
            // Verify it has CJK text content
            if trimmed.chars().any(|c| {
                ('\u{3000}'..='\u{9FFF}').contains(&c)
                    || ('\u{FF00}'..='\u{FFEF}').contains(&c)
            }) {
                return Some(trimmed.to_string());
            }
        }
    }
    None
}

/// Extract translatable strings from an MZ plugin command (code 357).
/// MZ format: [pluginName, commandName, commandDesc, {args}]
/// Returns Vec of (arg_key, text_value) pairs.
fn extract_mz_plugin_command(params: &[serde_json::Value]) -> Vec<(String, String)> {
    let mut results = Vec::new();
    if params.len() < 4 {
        return results;
    }
    let args = match params[3].as_object() {
        Some(a) => a,
        None => return results,
    };

    for &key in MZ_TRANSLATABLE_ARG_KEYS {
        if let Some(val) = args.get(key).and_then(|v| v.as_str()) {
            if val.trim().is_empty() {
                continue;
            }
            // "choices" field contains nested JSON array of objects with "label" keys
            if key == "choices" {
                if let Ok(choices_arr) = serde_json::from_str::<Vec<serde_json::Value>>(val) {
                    for (ci, choice) in choices_arr.iter().enumerate() {
                        // Each choice might be a string (nested JSON) or an object
                        let choice_obj = if let Some(s) = choice.as_str() {
                            serde_json::from_str::<serde_json::Value>(s).ok()
                        } else {
                            Some(choice.clone())
                        };
                        if let Some(obj) = choice_obj {
                            if let Some(label) = obj.get("label").and_then(|v| v.as_str()) {
                                if !label.trim().is_empty() && label.chars().any(|c| {
                                    ('\u{3000}'..='\u{9FFF}').contains(&c)
                                        || ('\u{FF00}'..='\u{FFEF}').contains(&c)
                                }) {
                                    results.push((format!("choices#{}#label", ci), label.to_string()));
                                }
                            }
                        }
                    }
                }
                continue;
            }
            // Regular text field — only if it contains CJK characters
            if val.chars().any(|c| {
                ('\u{3000}'..='\u{9FFF}').contains(&c)
                    || ('\u{FF00}'..='\u{FFEF}').contains(&c)
            }) {
                results.push((key.to_string(), val.to_string()));
            }
        }
    }
    results
}


#[derive(Debug, Clone, PartialEq)]
pub enum MvMzVersion {
    Mv,
    Mz,
    Unknown,
}

pub struct RpgMakerMvPlugin;

impl RpgMakerMvPlugin {
    pub fn new() -> Self {
        Self
    }

    pub(crate) fn find_data_dir(path: &Path) -> Option<PathBuf> {
        if path.is_dir() {
            // Check www/data first (MV typically uses www/data/)
            let www = path.join("www").join("data");
            if www.is_dir() && Self::has_rpgmaker_json(&www) {
                return Some(www);
            }
            let direct = path.join("data");
            if direct.is_dir() && Self::has_rpgmaker_json(&direct) {
                return Some(direct);
            }
            // Fallback: return any existing data dir even without known files
            if www.is_dir() {
                return Some(www);
            }
            if direct.is_dir() {
                return Some(direct);
            }
        }
        None
    }

    fn has_rpgmaker_json(dir: &Path) -> bool {
        // Plain deploy (editor / unencrypted)
        dir.join("Actors.json").exists()
            || dir.join("System.json").exists()
            || dir.join("Map001.json").exists()
            // POR_DatabaseEncoder deploy: same stems with .jsono (LZString base64)
            || dir.join("Actors.jsono").exists()
            || dir.join("System.jsono").exists()
            || dir.join("Map001.jsono").exists()
            // Any MapNNN.jsono is enough (some titles renumber maps)
            || Self::dir_has_map_data(dir)
            // Iavra multi-pack: data/lang_*_*.json{,o}
            || Self::dir_has_iavra_lang_pack(dir)
    }

    fn dir_has_map_data(dir: &Path) -> bool {
        let Ok(rd) = std::fs::read_dir(dir) else {
            return false;
        };
        rd.filter_map(|e| e.ok())
            .any(|e| {
                let name = e.file_name();
                let Some(s) = name.to_str() else {
                    return false;
                };
                Self::is_map_data_name(s)
            })
    }

    fn dir_has_iavra_lang_pack(dir: &Path) -> bool {
        let Ok(rd) = std::fs::read_dir(dir) else {
            return false;
        };
        rd.filter_map(|e| e.ok()).any(|e| {
            e.file_name()
                .to_str()
                .is_some_and(|s| Self::is_iavra_lang_pack_name(s))
        })
    }

    fn is_map_data_name(name: &str) -> bool {
        let stem = Self::strip_data_ext(name);
        let stem_lower = stem.to_ascii_lowercase();
        stem_lower.starts_with("map")
            && stem_lower.len() > 3
            && stem_lower[3..].chars().all(|c| c.is_ascii_digit())
            && (name.ends_with(".json") || name.ends_with(".jsono"))
    }

    /// `lang_{pack}_{lang}.json` / `.jsono` (Iavra multi-file path template).
    fn is_iavra_lang_pack_name(name: &str) -> bool {
        let stem = Self::strip_data_ext(name);
        if !(name.ends_with(".json") || name.ends_with(".jsono")) {
            return false;
        }
        // lang_g_en, lang_h_jp, lang_terms_zh, ...
        let Some(rest) = stem.strip_prefix("lang_") else {
            return false;
        };
        rest.contains('_')
    }

    fn strip_data_ext(name: &str) -> &str {
        name.strip_suffix(".jsono")
            .or_else(|| name.strip_suffix(".json"))
            .unwrap_or(name)
    }

    /// POR / Iavra deploy: `.jsono` is LZString.compressToBase64 of UTF-16 code units.
    fn decode_data_file_text(file_path: &Path, raw: &str) -> Result<String> {
        let is_jsono = file_path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case("jsono"));
        if !is_jsono {
            return Ok(raw.to_string());
        }
        let trimmed = raw.trim();
        let units = lz_str::decompress_from_base64(trimmed).ok_or_else(|| {
            LocustError::ParseError {
                file: file_path.display().to_string(),
                message: "failed to decompress .jsono (LZString base64 / POR_DatabaseEncoder)"
                    .to_string(),
            }
        })?;
        Ok(String::from_utf16_lossy(&units))
    }

    /// True when the game's data files are wrapped by a protection plugin:
    /// System.json is `{"uid", "bid", "data": "<encoded>"}` instead of the
    /// real object. Cheap check: parse System.json and look for the `data`
    /// string wrapper plus the absence of any normal top-level field.
    fn is_encrypted(data_dir: &Path) -> bool {
        for name in ["System.json", "System.jsono"] {
            let path = data_dir.join(name);
            let Ok(raw) = std::fs::read_to_string(&path) else {
                continue;
            };
            let Ok(text) = Self::decode_data_file_text(&path, &raw) else {
                continue;
            };
            let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) else {
                continue;
            };
            let Some(obj) = json.as_object() else {
                continue;
            };
            return obj.get("data").is_some_and(|d| d.is_string())
                && obj.contains_key("uid")
                && !obj.contains_key("gameTitle")
                && !obj.contains_key("terms");
        }
        false
    }

    fn detect_version(game_root: &Path) -> MvMzVersion {
        if game_root.join("js").join("rmmz_core.js").exists() {
            return MvMzVersion::Mz;
        }
        if game_root.join("js").join("rpg_core.js").exists() {
            return MvMzVersion::Mv;
        }
        if let Ok(pkg) = std::fs::read_to_string(game_root.join("package.json")) {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&pkg) {
                if let Some(ver) = v.get("version").and_then(|v| v.as_str()) {
                    if ver.starts_with("1.") {
                        return MvMzVersion::Mz;
                    }
                }
            }
        }
        MvMzVersion::Unknown
    }

    fn is_known_data_file(name: &str) -> bool {
        if !(name.ends_with(".json") || name.ends_with(".jsono")) {
            return false;
        }
        let stem = Self::strip_data_ext(name);
        let stem_lower = stem.to_lowercase();
        for af in ARRAY_FILES {
            if stem_lower == af.to_lowercase() {
                return true;
            }
        }
        if stem_lower == "system" || stem_lower == "commonevents" {
            return true;
        }
        if stem_lower.starts_with("map")
            && stem_lower.len() > 3
            && stem_lower[3..].chars().all(|c| c.is_ascii_digit())
        {
            return true;
        }
        false
    }

    fn extract_file(file_path: &Path) -> Result<Vec<StringEntry>> {
        let (raw, _enc) = EncodingDetector::read_file_auto(file_path)?;
        let content = Self::decode_data_file_text(file_path, &raw)?;
        let filename = file_path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        let stem = Self::strip_data_ext(&filename);
        let stem_lower = stem.to_lowercase();

        let json: serde_json::Value = serde_json::from_str(&content)?;

        if stem_lower == "system" {
            return Self::extract_system(&filename, &json, file_path);
        }
        if stem_lower == "commonevents" {
            return Self::extract_events_file(&filename, &json, file_path);
        }
        if stem_lower.starts_with("map") {
            return Self::extract_map(&filename, &json, file_path);
        }

        // Array-of-objects file
        for af in ARRAY_FILES {
            if stem_lower == af.to_lowercase() {
                return Self::extract_array_file(&filename, &json, file_path);
            }
        }

        Ok(Vec::new())
    }

    fn extract_array_file(
        filename: &str,
        json: &serde_json::Value,
        file_path: &Path,
    ) -> Result<Vec<StringEntry>> {
        let mut entries = Vec::new();
        let arr = json.as_array().ok_or_else(|| {
            LocustError::ParseError {
                file: filename.to_string(),
                message: "expected JSON array".to_string(),
            }
        })?;

        for (idx, item) in arr.iter().enumerate() {
            if item.is_null() {
                continue;
            }
            let obj = match item.as_object() {
                Some(o) => o,
                None => continue,
            };

            for &(field, tag) in EXTRACTABLE_FIELDS {
                if let Some(val) = obj.get(field).and_then(|v| v.as_str()) {
                    if val.trim().is_empty() {
                        continue;
                    }
                    let id = format!("{}#{}#{}", filename, idx, field);
                    let mut entry = StringEntry::new(id, val, file_path.to_path_buf());
                    entry.tags = vec![tag.to_string()];
                    entries.push(entry);
                }
            }

            // Troops (and similar) have pages with event commands (battle dialogue)
            if let Some(pages) = obj.get("pages").and_then(|v| v.as_array()) {
                for (page_idx, page) in pages.iter().enumerate() {
                    let list = match page.get("list").and_then(|v| v.as_array()) {
                        Some(l) => l,
                        None => continue,
                    };
                    Self::extract_event_commands(
                        &mut entries,
                        list,
                        file_path,
                        &format!("{}#{}#page_{}", filename, idx, page_idx),
                    );
                }
            }
        }

        Ok(entries)
    }

    /// Extract translatable strings from a list of RPG Maker event commands.
    /// Used by maps, common events, and troops.
    fn extract_event_commands(
        entries: &mut Vec<StringEntry>,
        list: &[serde_json::Value],
        file_path: &Path,
        id_prefix: &str,
    ) {
        // Speaker name from the last Show Text header (code 101, MZ params[4]);
        // passed as translation context so the model gets tone/gender right.
        let mut speaker: Option<String> = None;
        let mut skip_until = 0usize;

        for (cmd_idx, cmd) in list.iter().enumerate() {
            if cmd_idx < skip_until {
                continue;
            }
            let code = cmd.get("code").and_then(|v| v.as_i64()).unwrap_or(0);
            let params = match cmd.get("parameters").and_then(|v| v.as_array()) {
                Some(p) => p,
                None => continue,
            };

            match code {
                // Show Text header — remember the speaker for the lines that follow
                101 => {
                    speaker = params
                        .get(4)
                        .and_then(|v| v.as_str())
                        .filter(|s| !s.trim().is_empty())
                        .map(|s| s.to_string());
                }
                // Show Text / Scrolling Text content. Consecutive lines form ONE
                // message box, so merge the run into a single entry — the model
                // translates whole sentences and the injector re-wraps (#msg).
                401 | 405 => {
                    let mut lines: Vec<String> = Vec::new();
                    let mut end = cmd_idx;
                    while let Some(next) = list.get(end) {
                        if next.get("code").and_then(|v| v.as_i64()) != Some(code) {
                            break;
                        }
                        let line = next
                            .get("parameters")
                            .and_then(|v| v.as_array())
                            .and_then(|p| p.first())
                            .and_then(|v| v.as_str())
                            .unwrap_or("");
                        lines.push(line.to_string());
                        end += 1;
                    }
                    skip_until = end;

                    let text = lines.join("\n");
                    if !text.trim().is_empty() {
                        let id = format!("{}#cmd_{}#msg", id_prefix, cmd_idx);
                        let tag = if code == 405 { "scroll_text" } else { "dialogue" };
                        let mut entry = StringEntry::new(id, &text, file_path.to_path_buf());
                        entry.tags = vec![tag.to_string()];
                        if code == 401 {
                            entry.context = speaker.clone();
                        }
                        entries.push(entry);
                    }
                }
                // Show Choices
                102 => {
                    if let Some(choices) = params.first().and_then(|v| v.as_array()) {
                        for (ci, choice) in choices.iter().enumerate() {
                            if let Some(text) = choice.as_str() {
                                if !text.trim().is_empty() {
                                    let id = format!(
                                        "{}#cmd_{}#choice_{}",
                                        id_prefix, cmd_idx, ci
                                    );
                                    let mut entry =
                                        StringEntry::new(id, text, file_path.to_path_buf());
                                    entry.tags = vec!["menu".to_string()];
                                    entries.push(entry);
                                }
                            }
                        }
                    }
                }
                // Change Actor Name
                320 => {
                    if let Some(text) = params.get(1).and_then(|v| v.as_str()) {
                        if !text.trim().is_empty() {
                            let id = format!("{}#cmd_{}", id_prefix, cmd_idx);
                            let mut entry =
                                StringEntry::new(id, text, file_path.to_path_buf());
                            entry.tags = vec!["actor_name".to_string()];
                            entries.push(entry);
                        }
                    }
                }
                // Plugin Command (MV: code 356)
                356 => {
                    if let Some(text) = params.first().and_then(|v| v.as_str()) {
                        if let Some(extracted) = extract_plugin_command_text(text) {
                            let id = format!("{}#cmd_{}", id_prefix, cmd_idx);
                            let mut entry =
                                StringEntry::new(id, &extracted, file_path.to_path_buf());
                            entry.tags = vec!["plugin_cmd".to_string()];
                            entries.push(entry);
                        }
                    }
                }
                // MZ Plugin Command (code 357): structured args
                357 => {
                    for (arg_key, text) in extract_mz_plugin_command(params) {
                        let id = format!("{}#cmd_{}#arg_{}", id_prefix, cmd_idx, arg_key);
                        let mut entry =
                            StringEntry::new(id, &text, file_path.to_path_buf());
                        entry.tags = vec!["plugin_cmd".to_string()];
                        entries.push(entry);
                    }
                }
                _ => {}
            }
        }
    }

    fn extract_system(
        filename: &str,
        json: &serde_json::Value,
        file_path: &Path,
    ) -> Result<Vec<StringEntry>> {
        let mut entries = Vec::new();

        // gameTitle
        if let Some(title) = json.get("gameTitle").and_then(|v| v.as_str()) {
            if !title.trim().is_empty() {
                let mut entry =
                    StringEntry::new(format!("{}#gameTitle", filename), title, file_path.to_path_buf());
                entry.tags = vec!["system".to_string()];
                entries.push(entry);
            }
        }

        if let Some(terms) = json.get("terms").and_then(|v| v.as_object()) {
            // terms.basic[]
            if let Some(basic) = terms.get("basic").and_then(|v| v.as_array()) {
                for (i, val) in basic.iter().enumerate() {
                    if let Some(s) = val.as_str() {
                        if !s.trim().is_empty() {
                            let mut entry = StringEntry::new(
                                format!("{}#terms#basic#{}", filename, i),
                                s,
                                file_path.to_path_buf(),
                            );
                            entry.tags = vec!["ui_label".to_string()];
                            entries.push(entry);
                        }
                    }
                }
            }

            // terms.commands[]
            if let Some(cmds) = terms.get("commands").and_then(|v| v.as_array()) {
                for (i, val) in cmds.iter().enumerate() {
                    if let Some(s) = val.as_str() {
                        if !s.trim().is_empty() {
                            let mut entry = StringEntry::new(
                                format!("{}#terms#commands#{}", filename, i),
                                s,
                                file_path.to_path_buf(),
                            );
                            entry.tags = vec!["menu".to_string()];
                            entries.push(entry);
                        }
                    }
                }
            }

            // terms.params[]
            if let Some(params) = terms.get("params").and_then(|v| v.as_array()) {
                for (i, val) in params.iter().enumerate() {
                    if let Some(s) = val.as_str() {
                        if !s.trim().is_empty() {
                            let mut entry = StringEntry::new(
                                format!("{}#terms#params#{}", filename, i),
                                s,
                                file_path.to_path_buf(),
                            );
                            entry.tags = vec!["ui_label".to_string()];
                            entries.push(entry);
                        }
                    }
                }
            }

            // terms.messages {}
            if let Some(msgs) = terms.get("messages").and_then(|v| v.as_object()) {
                for (key, val) in msgs {
                    if let Some(s) = val.as_str() {
                        if !s.trim().is_empty() {
                            let mut entry = StringEntry::new(
                                format!("{}#terms#messages#{}", filename, key),
                                s,
                                file_path.to_path_buf(),
                            );
                            entry.tags = vec!["dialogue".to_string()];
                            entries.push(entry);
                        }
                    }
                }
            }
        }

        // Type arrays (armorTypes, elements, equipTypes, skillTypes, weaponTypes)
        for &(arr_name, tag) in SYSTEM_TYPE_ARRAYS {
            if let Some(arr) = json.get(arr_name).and_then(|v| v.as_array()) {
                for (i, val) in arr.iter().enumerate() {
                    if let Some(s) = val.as_str() {
                        if !s.trim().is_empty() {
                            let mut entry = StringEntry::new(
                                format!("{}#{}#{}", filename, arr_name, i),
                                s,
                                file_path.to_path_buf(),
                            );
                            entry.tags = vec![tag.to_string()];
                            entries.push(entry);
                        }
                    }
                }
            }
        }

        // Plugin parameters — custom menus, buttons, UI text
        if let Some(plugins) = json.get("plugins").and_then(|v| v.as_array()) {
            for (pi, plugin) in plugins.iter().enumerate() {
                let plugin_name = plugin
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown");
                if let Some(params) = plugin.get("parameters").and_then(|v| v.as_object()) {
                    for (key, val) in params {
                        if let Some(s) = val.as_str() {
                            if s.trim().is_empty() {
                                continue;
                            }
                            // Only extract strings that contain CJK characters (actual Japanese text)
                            if !s.chars().any(|c| {
                                ('\u{3000}'..='\u{9FFF}').contains(&c)
                                    || ('\u{F900}'..='\u{FAFF}').contains(&c)
                                    || ('\u{FF00}'..='\u{FFEF}').contains(&c)
                            }) {
                                continue;
                            }
                            let id = format!(
                                "{}#plugins#{}#{}#{}",
                                filename, pi, plugin_name, key
                            );
                            let mut entry =
                                StringEntry::new(id, s, file_path.to_path_buf());
                            entry.tags = vec!["plugin_param".to_string()];
                            entries.push(entry);
                        }
                    }
                }
            }
        }

        Ok(entries)
    }

    fn extract_map(
        filename: &str,
        json: &serde_json::Value,
        file_path: &Path,
    ) -> Result<Vec<StringEntry>> {
        let mut entries = Vec::new();

        // Map display name (shown when player enters the area)
        if let Some(dn) = json.get("displayName").and_then(|v| v.as_str()) {
            if !dn.trim().is_empty() {
                let mut entry = StringEntry::new(
                    format!("{}#displayName", filename),
                    dn,
                    file_path.to_path_buf(),
                );
                entry.tags = vec!["location".to_string()];
                entries.push(entry);
            }
        }

        let events = match json.get("events").and_then(|v| v.as_array()) {
            Some(e) => e,
            None => return Ok(entries),
        };

        for (ev_idx, event) in events.iter().enumerate() {
            if event.is_null() {
                continue;
            }
            let pages = match event.get("pages").and_then(|v| v.as_array()) {
                Some(p) => p,
                None => continue,
            };

            for (page_idx, page) in pages.iter().enumerate() {
                let list = match page.get("list").and_then(|v| v.as_array()) {
                    Some(l) => l,
                    None => continue,
                };
                Self::extract_event_commands(
                    &mut entries,
                    list,
                    file_path,
                    &format!("{}#0#event_{}#page_{}", filename, ev_idx, page_idx),
                );
            }
        }

        Ok(entries)
    }

    fn extract_events_file(
        filename: &str,
        json: &serde_json::Value,
        file_path: &Path,
    ) -> Result<Vec<StringEntry>> {
        let mut entries = Vec::new();
        let arr = match json.as_array() {
            Some(a) => a,
            None => return Ok(entries),
        };

        for (ev_idx, event) in arr.iter().enumerate() {
            if event.is_null() {
                continue;
            }
            let list = match event.get("list").and_then(|v| v.as_array()) {
                Some(l) => l,
                None => continue,
            };
            Self::extract_event_commands(
                &mut entries,
                list,
                file_path,
                &format!("{}#{}", filename, ev_idx),
            );
        }

        Ok(entries)
    }

    fn apply_translation(
        json: &mut serde_json::Value,
        filename: &str,
        entry_id: &str,
        translation: &str,
    ) {
        // Parse entry_id to figure out where to write
        let suffix = match entry_id.strip_prefix(&format!("{}#", filename)) {
            Some(s) => s,
            None => return,
        };

        let parts: Vec<&str> = suffix.split('#').collect();

        // Iavra multi-pack flat object: "yes", "Map003_0", "skill1", …
        if parts.len() == 1 && Self::is_iavra_lang_pack_name(filename) {
            if let Some(obj) = json.as_object_mut() {
                obj.insert(
                    parts[0].to_string(),
                    serde_json::Value::String(translation.to_string()),
                );
            }
            return;
        }

        // Array file: "1#name" (but NOT CommonEvent commands like "201#cmd_97")
        if parts.len() == 2 && !parts[1].starts_with("cmd_") {
            if let Ok(idx) = parts[0].parse::<usize>() {
                let field = parts[1];
                if let Some(arr) = json.as_array_mut() {
                    if let Some(item) = arr.get_mut(idx) {
                        if let Some(obj) = item.as_object_mut() {
                            if obj.contains_key(field) {
                                obj.insert(
                                    field.to_string(),
                                    serde_json::Value::String(translation.to_string()),
                                );
                            }
                        }
                    }
                }
                return;
            }
        }

        // System: "gameTitle"
        if parts.len() == 1 && parts[0] == "gameTitle" {
            if let Some(obj) = json.as_object_mut() {
                obj.insert(
                    "gameTitle".to_string(),
                    serde_json::Value::String(translation.to_string()),
                );
            }
            return;
        }

        // System terms: "terms#basic#0", "terms#commands#0", "terms#params#0", "terms#messages#key"
        if parts.len() >= 3 && parts[0] == "terms" {
            if let Some(terms) = json
                .as_object_mut()
                .and_then(|o| o.get_mut("terms"))
                .and_then(|v| v.as_object_mut())
            {
                let section = parts[1];
                let key = parts[2];

                if section == "messages" {
                    if let Some(msgs) = terms.get_mut("messages").and_then(|v| v.as_object_mut()) {
                        msgs.insert(
                            key.to_string(),
                            serde_json::Value::String(translation.to_string()),
                        );
                    }
                } else if let Ok(idx) = key.parse::<usize>() {
                    if let Some(arr) = terms.get_mut(section).and_then(|v| v.as_array_mut()) {
                        if idx < arr.len() {
                            arr[idx] = serde_json::Value::String(translation.to_string());
                        }
                    }
                }
            }
            return;
        }

        // System type arrays: "armorTypes#1", "elements#2", etc.
        if parts.len() == 2 {
            let arr_name = parts[0];
            let is_type_array = SYSTEM_TYPE_ARRAYS.iter().any(|&(name, _)| name == arr_name);
            if is_type_array {
                if let Ok(idx) = parts[1].parse::<usize>() {
                    if let Some(arr) = json
                        .as_object_mut()
                        .and_then(|o| o.get_mut(arr_name))
                        .and_then(|v| v.as_array_mut())
                    {
                        if idx < arr.len() {
                            arr[idx] = serde_json::Value::String(translation.to_string());
                        }
                    }
                }
                return;
            }
        }

        // Plugin parameters: "plugins#0#PluginName#paramKey"
        if parts.len() >= 4 && parts[0] == "plugins" {
            if let Ok(pi) = parts[1].parse::<usize>() {
                let param_key = parts[3];
                if let Some(plugins) = json
                    .as_object_mut()
                    .and_then(|o| o.get_mut("plugins"))
                    .and_then(|v| v.as_array_mut())
                {
                    if let Some(plugin) = plugins.get_mut(pi) {
                        if let Some(params) =
                            plugin.get_mut("parameters").and_then(|v| v.as_object_mut())
                        {
                            if params.contains_key(param_key) {
                                params.insert(
                                    param_key.to_string(),
                                    serde_json::Value::String(translation.to_string()),
                                );
                            }
                        }
                    }
                }
            }
            return;
        }

        // Map displayName
        if parts.len() == 1 && parts[0] == "displayName" {
            if let Some(obj) = json.as_object_mut() {
                obj.insert(
                    "displayName".to_string(),
                    serde_json::Value::String(translation.to_string()),
                );
            }
            return;
        }

        // Map/CommonEvents/Troops commands
        if suffix.contains("event_") && suffix.contains("cmd_") {
            // Map: "0#event_1#page_0#cmd_5"
            Self::apply_map_translation(json, suffix, translation);
        } else if suffix.contains("page_") && suffix.contains("cmd_") {
            // Troops: "1#page_2#cmd_13" (array item with pages)
            Self::apply_troops_translation(json, suffix, translation);
        } else if suffix.contains("cmd_") {
            // CommonEvents: "1#cmd_3"
            Self::apply_common_event_translation(json, suffix, translation);
        }
    }

    fn apply_map_translation(json: &mut serde_json::Value, suffix: &str, translation: &str) {
        let parts: Vec<&str> = suffix.split('#').collect();
        // Format: "0#event_N#page_N#cmd_N[#choice_N]"
        if parts.len() < 4 {
            return;
        }
        let ev_idx: usize = parts[1]
            .strip_prefix("event_")
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        let page_idx: usize = parts[2]
            .strip_prefix("page_")
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        let cmd_idx: usize = parts[3]
            .strip_prefix("cmd_")
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);

        let events = match json.get_mut("events").and_then(|v| v.as_array_mut()) {
            Some(e) => e,
            None => return,
        };
        let event = match events.get_mut(ev_idx) {
            Some(e) if !e.is_null() => e,
            _ => return,
        };
        let pages = match event.get_mut("pages").and_then(|v| v.as_array_mut()) {
            Some(p) => p,
            None => return,
        };
        let page = match pages.get_mut(page_idx) {
            Some(p) => p,
            None => return,
        };
        let list = match page.get_mut("list").and_then(|v| v.as_array_mut()) {
            Some(l) => l,
            None => return,
        };

        if parts.last() == Some(&"msg") {
            Self::apply_message_block(list, cmd_idx, translation);
            return;
        }

        let cmd = match list.get_mut(cmd_idx) {
            Some(c) => c,
            None => return,
        };

        if parts.len() == 5 && parts[4].starts_with("choice_") {
            let ci: usize = parts[4]
                .strip_prefix("choice_")
                .and_then(|s| s.parse().ok())
                .unwrap_or(0);
            if let Some(params) = cmd.get_mut("parameters").and_then(|v| v.as_array_mut()) {
                if let Some(choices) = params.first_mut().and_then(|v| v.as_array_mut()) {
                    if ci < choices.len() {
                        choices[ci] = serde_json::Value::String(translation.to_string());
                    }
                }
            }
        } else if parts.len() >= 5 && parts[4].starts_with("arg_") {
            // MZ Plugin Command (code 357): inject into structured args
            let arg_suffix = &suffix[suffix.find("#arg_").unwrap_or(suffix.len())..];
            Self::apply_mz_plugin_arg(cmd, arg_suffix, translation);
        } else {
            let code = cmd.get("code").and_then(|v| v.as_i64()).unwrap_or(0);
            if let Some(params) = cmd.get_mut("parameters").and_then(|v| v.as_array_mut()) {
                if code == 320 {
                    // Change Actor Name: text is in params[1]
                    if let Some(val) = params.get_mut(1) {
                        *val = serde_json::Value::String(translation.to_string());
                    }
                } else if let Some(first) = params.first_mut() {
                    *first = serde_json::Value::String(translation.to_string());
                }
            }
        }
    }

    fn apply_mz_plugin_arg(cmd: &mut serde_json::Value, arg_suffix: &str, translation: &str) {
        // arg_suffix is like "#arg_text" or "#arg_choices#0#label" or "#arg_destination"
        let arg_parts: Vec<&str> = arg_suffix.trim_start_matches('#').split('#').collect();
        if arg_parts.is_empty() {
            return;
        }
        let arg_key = arg_parts[0].strip_prefix("arg_").unwrap_or(arg_parts[0]);

        let params = match cmd.get_mut("parameters").and_then(|v| v.as_array_mut()) {
            Some(p) => p,
            None => return,
        };
        let args = match params.get_mut(3).and_then(|v| v.as_object_mut()) {
            Some(a) => a,
            None => return,
        };

        if arg_key == "choices" && arg_parts.len() >= 3 {
            // Handle nested choice labels: "arg_choices#0#label"
            let choice_idx: usize = arg_parts[1].parse().unwrap_or(0);
            if let Some(choices_str) = args.get(arg_key).and_then(|v| v.as_str()) {
                if let Ok(mut choices_arr) = serde_json::from_str::<Vec<serde_json::Value>>(choices_str) {
                    if let Some(choice_val) = choices_arr.get_mut(choice_idx) {
                        // Choice may be a string containing JSON
                        let mut choice_obj = if let Some(s) = choice_val.as_str() {
                            serde_json::from_str::<serde_json::Value>(s).unwrap_or_default()
                        } else {
                            choice_val.clone()
                        };
                        if let Some(obj) = choice_obj.as_object_mut() {
                            obj.insert("label".to_string(), serde_json::Value::String(translation.to_string()));
                        }
                        // Write back
                        if choice_val.is_string() {
                            *choice_val = serde_json::Value::String(choice_obj.to_string());
                        } else {
                            *choice_val = choice_obj;
                        }
                        // Serialize choices array back
                        if let Ok(new_str) = serde_json::to_string(&choices_arr) {
                            args.insert(arg_key.to_string(), serde_json::Value::String(new_str));
                        }
                    }
                }
            }
        } else {
            // Simple arg replacement (text, destination, message, etc.)
            if args.contains_key(arg_key) {
                args.insert(arg_key.to_string(), serde_json::Value::String(translation.to_string()));
            }
        }
    }

    fn apply_common_event_translation(
        json: &mut serde_json::Value,
        suffix: &str,
        translation: &str,
    ) {
        let parts: Vec<&str> = suffix.split('#').collect();
        if parts.len() < 2 {
            return;
        }
        let ev_idx: usize = parts[0].parse().unwrap_or(0);
        let cmd_idx: usize = parts[1]
            .strip_prefix("cmd_")
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);

        let arr = match json.as_array_mut() {
            Some(a) => a,
            None => return,
        };
        let event = match arr.get_mut(ev_idx) {
            Some(e) if !e.is_null() => e,
            _ => return,
        };
        let list = match event.get_mut("list").and_then(|v| v.as_array_mut()) {
            Some(l) => l,
            None => return,
        };

        if parts.last() == Some(&"msg") {
            Self::apply_message_block(list, cmd_idx, translation);
            return;
        }

        let cmd = match list.get_mut(cmd_idx) {
            Some(c) => c,
            None => return,
        };

        if parts.len() == 3 && parts[2].starts_with("choice_") {
            let ci: usize = parts[2]
                .strip_prefix("choice_")
                .and_then(|s| s.parse().ok())
                .unwrap_or(0);
            if let Some(params) = cmd.get_mut("parameters").and_then(|v| v.as_array_mut()) {
                if let Some(choices) = params.first_mut().and_then(|v| v.as_array_mut()) {
                    if ci < choices.len() {
                        choices[ci] = serde_json::Value::String(translation.to_string());
                    }
                }
            }
        } else if parts.len() >= 3 && parts[2].starts_with("arg_") {
            let arg_suffix = &suffix[suffix.find("#arg_").unwrap_or(suffix.len())..];
            Self::apply_mz_plugin_arg(cmd, arg_suffix, translation);
        } else {
            let code = cmd.get("code").and_then(|v| v.as_i64()).unwrap_or(0);
            if let Some(params) = cmd.get_mut("parameters").and_then(|v| v.as_array_mut()) {
                if code == 320 {
                    if let Some(val) = params.get_mut(1) {
                        *val = serde_json::Value::String(translation.to_string());
                    }
                } else if let Some(first) = params.first_mut() {
                    *first = serde_json::Value::String(translation.to_string());
                }
            }
        }
    }

    fn apply_troops_translation(
        json: &mut serde_json::Value,
        suffix: &str,
        translation: &str,
    ) {
        let parts: Vec<&str> = suffix.split('#').collect();
        // Format: "idx#page_N#cmd_N[#choice_N|#arg_X]"
        if parts.len() < 3 {
            return;
        }
        let item_idx: usize = parts[0].parse().unwrap_or(0);
        let page_idx: usize = parts[1]
            .strip_prefix("page_")
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        let cmd_idx: usize = parts[2]
            .strip_prefix("cmd_")
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);

        let arr = match json.as_array_mut() {
            Some(a) => a,
            None => return,
        };
        let item = match arr.get_mut(item_idx) {
            Some(e) if !e.is_null() => e,
            _ => return,
        };
        let pages = match item.get_mut("pages").and_then(|v| v.as_array_mut()) {
            Some(p) => p,
            None => return,
        };
        let page = match pages.get_mut(page_idx) {
            Some(p) => p,
            None => return,
        };
        let list = match page.get_mut("list").and_then(|v| v.as_array_mut()) {
            Some(l) => l,
            None => return,
        };

        if parts.last() == Some(&"msg") {
            Self::apply_message_block(list, cmd_idx, translation);
            return;
        }

        let cmd = match list.get_mut(cmd_idx) {
            Some(c) => c,
            None => return,
        };

        if parts.len() == 4 && parts[3].starts_with("choice_") {
            let ci: usize = parts[3]
                .strip_prefix("choice_")
                .and_then(|s| s.parse().ok())
                .unwrap_or(0);
            if let Some(params) = cmd.get_mut("parameters").and_then(|v| v.as_array_mut()) {
                if let Some(choices) = params.first_mut().and_then(|v| v.as_array_mut()) {
                    if ci < choices.len() {
                        choices[ci] = serde_json::Value::String(translation.to_string());
                    }
                }
            }
        } else if parts.len() >= 4 && parts[3].starts_with("arg_") {
            let arg_suffix = &suffix[suffix.find("#arg_").unwrap_or(suffix.len())..];
            Self::apply_mz_plugin_arg(cmd, arg_suffix, translation);
        } else {
            let code = cmd.get("code").and_then(|v| v.as_i64()).unwrap_or(0);
            if let Some(params) = cmd.get_mut("parameters").and_then(|v| v.as_array_mut()) {
                if code == 320 {
                    if let Some(val) = params.get_mut(1) {
                        *val = serde_json::Value::String(translation.to_string());
                    }
                } else if let Some(first) = params.first_mut() {
                    *first = serde_json::Value::String(translation.to_string());
                }
            }
        }
    }

    /// Replace a run of consecutive message lines (code 401/405) starting at
    /// `cmd_idx` with the translation re-wrapped to the original line width.
    /// The run may grow or shrink; callers must apply entries in descending
    /// cmd_idx order so earlier indices stay valid.
    fn apply_message_block(
        list: &mut Vec<serde_json::Value>,
        cmd_idx: usize,
        translation: &str,
    ) {
        let code = match list
            .get(cmd_idx)
            .and_then(|c| c.get("code"))
            .and_then(|v| v.as_i64())
        {
            Some(c @ (401 | 405)) => c,
            _ => return,
        };

        let mut run_len = 0usize;
        let mut max_width = 0usize;
        while let Some(cmd) = list.get(cmd_idx + run_len) {
            if cmd.get("code").and_then(|v| v.as_i64()) != Some(code) {
                break;
            }
            if let Some(text) = cmd
                .get("parameters")
                .and_then(|v| v.as_array())
                .and_then(|p| p.first())
                .and_then(|v| v.as_str())
            {
                max_width = max_width.max(visible_len(text));
            }
            run_len += 1;
        }
        if run_len == 0 {
            return;
        }

        // Wrap to the width the game's own text was wrapped at; the floor
        // keeps messages with only short lines from wrapping absurdly early.
        let width = max_width.max(40);
        let flat = translation.split_whitespace().collect::<Vec<_>>().join(" ");
        // A leading name tag (\n<Name>) must stay intact at the start of the
        // first line — it may contain spaces, so wrap only the body.
        let (name_tag, body) = split_name_tag(&flat);
        let mut lines = wrap_message(body, width);
        if !name_tag.is_empty() {
            lines[0] = format!("{}{}", name_tag, lines[0]);
        }

        let template = list[cmd_idx].clone();
        let new_cmds: Vec<serde_json::Value> = lines
            .into_iter()
            .map(|line| {
                let mut cmd = template.clone();
                if let Some(params) = cmd.get_mut("parameters").and_then(|v| v.as_array_mut()) {
                    if let Some(first) = params.first_mut() {
                        *first = serde_json::Value::String(line);
                    }
                }
                cmd
            })
            .collect();

        list.splice(cmd_idx..cmd_idx + run_len, new_cmds);
    }

    /// Parse `lang_{pack}_{lang}` stem → (pack, lang).
    fn parse_iavra_pack_stem(stem: &str) -> Option<(&str, &str)> {
        let rest = stem.strip_prefix("lang_")?;
        let (pack, lang) = rest.rsplit_once('_')?;
        if pack.is_empty() || lang.is_empty() {
            return None;
        }
        Some((pack, lang))
    }

    /// Prefer `en`, then `jp`/`ja`, then any other lang code present on disk.
    fn pick_iavra_source_lang(available: &[String]) -> Option<String> {
        if available.is_empty() {
            return None;
        }
        for pref in ["en", "jp", "ja", "zh"] {
            if available.iter().any(|l| l == pref) {
                return Some(pref.to_string());
            }
        }
        let mut sorted = available.to_vec();
        sorted.sort();
        sorted.into_iter().next()
    }

    fn list_iavra_packs(data_dir: &Path) -> Result<Vec<(String, String, PathBuf)>> {
        // (pack, lang, path)
        let mut out = Vec::new();
        for dir_entry in std::fs::read_dir(data_dir)? {
            let dir_entry = dir_entry?;
            let path = dir_entry.path();
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            if !Self::is_iavra_lang_pack_name(name) {
                continue;
            }
            let stem = Self::strip_data_ext(name);
            let Some((pack, lang)) = Self::parse_iavra_pack_stem(stem) else {
                continue;
            };
            out.push((pack.to_string(), lang.to_string(), path));
        }
        out.sort_by(|a, b| (&a.0, &a.1).cmp(&(&b.0, &b.1)));
        Ok(out)
    }

    fn extract_iavra_pack_file(file_path: &Path) -> Result<Vec<StringEntry>> {
        let (raw, _enc) = EncodingDetector::read_file_auto(file_path)?;
        let content = Self::decode_data_file_text(file_path, &raw)?;
        let filename = file_path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        let stem = Self::strip_data_ext(&filename);
        let (pack, lang) = Self::parse_iavra_pack_stem(stem).ok_or_else(|| {
            LocustError::ParseError {
                file: file_path.display().to_string(),
                message: format!("not an Iavra lang pack name: {filename}"),
            }
        })?;

        let json: serde_json::Value = serde_json::from_str(&content)?;
        let obj = json.as_object().ok_or_else(|| LocustError::ParseError {
            file: file_path.display().to_string(),
            message: "Iavra lang pack root must be a JSON object".to_string(),
        })?;

        let mut entries = Vec::new();
        for (key, val) in obj {
            let Some(text) = val.as_str() else {
                continue;
            };
            if text.trim().is_empty() {
                continue;
            }
            let id = format!("{filename}#{key}");
            let mut entry = StringEntry::new(id, text.to_string(), file_path.to_path_buf());
            entry = entry.with_tags(vec![
                "iavra".to_string(),
                format!("pack:{pack}"),
                format!("lang:{lang}"),
            ]);
            entry.context = Some(format!("iavra/{pack}/{lang}/{key}"));
            entries.push(entry);
        }
        Ok(entries)
    }

    /// Extract all Iavra multi-packs for the preferred source language.
    fn extract_iavra_packs(data_dir: &Path) -> Result<Vec<StringEntry>> {
        let packs = Self::list_iavra_packs(data_dir)?;
        if packs.is_empty() {
            return Ok(Vec::new());
        }
        let langs: Vec<String> = {
            let mut u = packs
                .iter()
                .map(|(_, lang, _)| lang.clone())
                .collect::<Vec<_>>();
            u.sort();
            u.dedup();
            u
        };
        let Some(source_lang) = Self::pick_iavra_source_lang(&langs) else {
            return Ok(Vec::new());
        };

        let mut all = Vec::new();
        for (pack, lang, path) in packs {
            if lang != source_lang {
                continue;
            }
            match Self::extract_iavra_pack_file(&path) {
                Ok(entries) => {
                    tracing::info!(
                        "Iavra pack {} ({}): {} strings",
                        pack,
                        lang,
                        entries.len()
                    );
                    all.extend(entries);
                }
                Err(e) => {
                    tracing::warn!("Failed Iavra pack {}: {}", path.display(), e);
                }
            }
        }
        Ok(all)
    }
}

/// Visible length of a message line, skipping RPG Maker control codes such
/// as \C[6], \N[1], \V[2], \G, \{, \}, \$, \., \|, \!, \>, \<, \^.
pub(crate) fn visible_len(s: &str) -> usize {
    let mut chars = s.chars().peekable();
    let mut n = 0usize;
    while let Some(c) = chars.next() {
        if c == '\\' {
            let mut had_letters = false;
            while chars.peek().is_some_and(|c| c.is_ascii_alphabetic()) {
                chars.next();
                had_letters = true;
            }
            if chars.peek() == Some(&'[') {
                for c in chars.by_ref() {
                    if c == ']' {
                        break;
                    }
                }
            } else if had_letters && chars.peek() == Some(&'<') {
                // YEP-style name tag (\n<Name>) renders in its own box
                for c in chars.by_ref() {
                    if c == '>' {
                        break;
                    }
                }
            } else if !had_letters
                && chars.peek().is_some_and(|c| "{}$.|!><^\\".contains(*c))
            {
                chars.next();
            }
        } else {
            n += 1;
        }
    }
    n
}

/// Word-wrap to lines whose visible length fits `width`. A word longer than
/// the width gets its own line rather than being split.
pub(crate) fn wrap_message(text: &str, width: usize) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    let mut current = String::new();
    for word in text.split_whitespace() {
        if current.is_empty() {
            current = word.to_string();
        } else if visible_len(&current) + 1 + visible_len(word) <= width {
            current.push(' ');
            current.push_str(word);
        } else {
            lines.push(std::mem::take(&mut current));
            current = word.to_string();
        }
    }
    if !current.is_empty() {
        lines.push(current);
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

/// Split a leading YEP-style name tag (`\n<Name>`) off a message, returning
/// (tag, rest). Returns an empty tag when the message doesn't start with one.
fn split_name_tag(text: &str) -> (&str, &str) {
    let bytes = text.as_bytes();
    if bytes.first() != Some(&b'\\') {
        return ("", text);
    }
    let mut i = 1;
    while i < bytes.len() && bytes[i].is_ascii_alphabetic() {
        i += 1;
    }
    if i == 1 || bytes.get(i) != Some(&b'<') {
        return ("", text);
    }
    match text[i..].find('>') {
        Some(end) => {
            let split = i + end + 1;
            (&text[..split], text[split..].trim_start())
        }
        None => ("", text),
    }
}

/// Highest cmd_ index embedded in an entry id, used to order injection.
fn last_cmd_index(id: &str) -> usize {
    id.rsplit('#')
        .find_map(|p| p.strip_prefix("cmd_").and_then(|s| s.parse().ok()))
        .unwrap_or(0)
}

impl Default for RpgMakerMvPlugin {
    fn default() -> Self {
        Self::new()
    }
}

impl FormatPlugin for RpgMakerMvPlugin {
    fn id(&self) -> &str {
        "rpgmaker-mv"
    }

    fn name(&self) -> &str {
        "RPG Maker MV/MZ"
    }

    fn description(&self) -> &str {
        "RPG Maker MV and MZ JSON data files"
    }

    fn supported_extensions(&self) -> &[&str] {
        &[".json", ".jsono"]
    }

    fn supported_modes(&self) -> Vec<OutputMode> {
        vec![OutputMode::Replace, OutputMode::Add]
    }

    fn detect(&self, path: &Path) -> bool {
        if path.is_dir() {
            // Strong MZ/MV signals even when only POR .jsono + Iavra packs exist
            // (NW.js deploy has Chromium .pak files that must not steal detect).
            let has_rm_js = path.join("js").join("rmmz_core.js").exists()
                || path.join("js").join("rpg_core.js").exists()
                || path.join("www").join("js").join("rpg_core.js").exists();
            if let Some(data_dir) = Self::find_data_dir(path) {
                if Self::has_rpgmaker_json(&data_dir) {
                    return true;
                }
                if has_rm_js && (data_dir.exists()) {
                    // data/ present with engine JS — still RM even if empty markers
                    return Self::dir_has_iavra_lang_pack(&data_dir)
                        || Self::dir_has_map_data(&data_dir);
                }
            }
            return false;
        }
        if path.is_file() {
            if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                return Self::is_known_data_file(name) || Self::is_iavra_lang_pack_name(name);
            }
        }
        false
    }

    fn extract(&self, path: &Path) -> Result<Vec<StringEntry>> {
        if path.is_file() {
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or_default();
            if Self::is_iavra_lang_pack_name(name) {
                return Self::extract_iavra_pack_file(path);
            }
            return Self::extract_file(path);
        }

        let data_dir = Self::find_data_dir(path).ok_or_else(|| {
            LocustError::ParseError {
                file: path.display().to_string(),
                message: "could not find data directory".to_string(),
            }
        })?;

        // Fail loudly on encrypted games instead of silently extracting 0
        // strings. Protection plugins (common on DLsite) replace each data
        // file's contents with a wrapper: {"uid", "bid", "data": "<encoded>"}.
        if Self::is_encrypted(&data_dir) {
            return Err(LocustError::ParseError {
                file: data_dir.join("System.json").display().to_string(),
                message: "game data is encrypted (uid/bid/data wrapper) — decrypt it first \
                    (e.g. with an RPG Maker MV/MZ decrypter) and then extract the decrypted copy"
                    .to_string(),
            });
        }

        let mut all_entries = Vec::new();

        // Iavra multi-pack first: these hold the real player-facing strings when
        // maps only store {{keys}}. Prefer source lang `en`, else first available.
        match Self::extract_iavra_packs(&data_dir) {
            Ok(entries) => all_entries.extend(entries),
            Err(e) => {
                tracing::warn!("Iavra pack extract skipped: {}", e);
            }
        }

        for dir_entry in std::fs::read_dir(&data_dir)? {
            let dir_entry = dir_entry?;
            let file_path = dir_entry.path();
            if let Some(name) = file_path.file_name().and_then(|n| n.to_str()) {
                if Self::is_known_data_file(name) {
                    match Self::extract_file(&file_path) {
                        Ok(entries) => all_entries.extend(entries),
                        Err(e) => {
                            tracing::warn!("Failed to extract {}: {}", file_path.display(), e);
                        }
                    }
                }
            }
        }

        Ok(all_entries)
    }

    fn inject(&self, path: &Path, entries: &[StringEntry]) -> Result<InjectionReport> {
        let mut files_modified = 0;
        let mut strings_written = 0;
        let mut strings_skipped = 0;
        let warnings = Vec::new();
        let mut files_written: Vec<PathBuf> = Vec::new();

        // Group entries by file
        let mut by_file: HashMap<String, Vec<&StringEntry>> = HashMap::new();
        for entry in entries {
            let filename = entry
                .file_path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            by_file.entry(filename).or_default().push(entry);
        }

        let data_dir = if path.is_dir() {
            Self::find_data_dir(path).unwrap_or_else(|| path.to_path_buf())
        } else {
            path.parent().unwrap_or(path).to_path_buf()
        };

        for (filename, file_entries) in &by_file {
            let file_path = data_dir.join(filename);
            if !file_path.exists() {
                continue;
            }

            let mut json = Self::read_data_json(&file_path)?;

            // Message-block splices change command counts, so apply from the
            // bottom of each event list up: earlier indices stay valid.
            let mut ordered = file_entries.clone();
            ordered.sort_by_key(|e| std::cmp::Reverse(last_cmd_index(&e.id)));

            for entry in ordered {
                if let Some(ref translation) = entry.translation {
                    Self::apply_translation(&mut json, filename, &entry.id, translation);
                    strings_written += 1;
                } else {
                    strings_skipped += 1;
                }
            }

            Self::write_data_file(&file_path, &json)?;
            files_modified += 1;
            files_written.push(file_path.clone());
        }

        Ok(InjectionReport {
            files_modified,
            strings_written,
            strings_skipped,
            warnings,
            files_written,
        })
    }

    fn inject_add(
        &self,
        path: &Path,
        lang: &str,
        entries: &[StringEntry],
    ) -> Result<InjectionReport> {
        let game_root = if path.is_dir() { path } else { path.parent().unwrap_or(path) };
        let data_dir = Self::find_data_dir(game_root)
            .unwrap_or_else(|| game_root.join("data"));

        // Multi-pack Iavra: write data/lang_{pack}_{lang}.jsono (not Languages/{lang}.json)
        let has_iavra = entries.iter().any(|e| {
            e.file_path
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(Self::is_iavra_lang_pack_name)
                || e.tags.iter().any(|t| t == "iavra")
        }) || Self::dir_has_iavra_lang_pack(&data_dir);

        if has_iavra {
            return Self::inject_add_iavra_packs(game_root, &data_dir, lang, entries);
        }

        let version = Self::detect_version(game_root);
        let mut strings_written = 0;
        let mut strings_skipped = 0;
        let mut files_written: Vec<PathBuf> = Vec::new();

        match version {
            MvMzVersion::Mz | MvMzVersion::Unknown => {
                // MZ format: data/Languages/{lang}.json
                let lang_dir = game_root.join("data").join("Languages");
                std::fs::create_dir_all(&lang_dir)?;
                let lang_file = lang_dir.join(format!("{}.json", lang));

                let mut map = serde_json::Map::new();
                for entry in entries {
                    if let Some(ref translation) = entry.translation {
                        map.insert(entry.id.clone(), serde_json::Value::String(translation.clone()));
                        strings_written += 1;
                    } else {
                        strings_skipped += 1;
                    }
                }
                let output = serde_json::to_string_pretty(&serde_json::Value::Object(map))?;
                std::fs::write(&lang_file, output)?;
                files_written.push(lang_file);
            }
            MvMzVersion::Mv => {
                // MV Iavra format: www/data/i18n/{lang}.json
                let i18n_dir = game_root.join("www").join("data").join("i18n");
                std::fs::create_dir_all(&i18n_dir)?;
                let lang_file = i18n_dir.join(format!("{}.json", lang));

                let mut strings_map = serde_json::Map::new();
                for entry in entries {
                    if let Some(ref translation) = entry.translation {
                        strings_map.insert(
                            entry.source.clone(),
                            serde_json::Value::String(translation.clone()),
                        );
                        strings_written += 1;
                    } else {
                        strings_skipped += 1;
                    }
                }
                let mut root = serde_json::Map::new();
                root.insert(
                    "strings".to_string(),
                    serde_json::Value::Object(strings_map),
                );
                let output = serde_json::to_string_pretty(&serde_json::Value::Object(root))?;
                std::fs::write(&lang_file, output)?;
                files_written.push(lang_file);
            }
        }

        Ok(InjectionReport {
            files_modified: 1,
            strings_written,
            strings_skipped,
            warnings: Vec::new(),
            files_written,
        })
    }
}

impl RpgMakerMvPlugin {
    /// Write JSON (pretty for .json, LZString base64 for .jsono).
    fn write_data_file(file_path: &Path, json: &serde_json::Value) -> Result<()> {
        let is_jsono = file_path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case("jsono"));
        let text = if is_jsono {
            serde_json::to_string(json)?
        } else {
            serde_json::to_string_pretty(json)?
        };
        if is_jsono {
            let encoded = lz_str::compress_to_base64(&text);
            std::fs::write(file_path, encoded)?;
        } else {
            std::fs::write(file_path, text)?;
        }
        Ok(())
    }

    fn read_data_json(file_path: &Path) -> Result<serde_json::Value> {
        let (raw, _enc) = EncodingDetector::read_file_auto(file_path)?;
        let content = Self::decode_data_file_text(file_path, &raw)?;
        Ok(serde_json::from_str(&content)?)
    }

    /// Remap `lang_{pack}_{srcLang}.jsono` → `lang_{pack}_{targetLang}.jsono`.
    fn iavra_target_filename(source_filename: &str, target_lang: &str) -> Option<String> {
        if !Self::is_iavra_lang_pack_name(source_filename) {
            return None;
        }
        let stem = Self::strip_data_ext(source_filename);
        let (pack, _src) = Self::parse_iavra_pack_stem(stem)?;
        let ext = if source_filename.ends_with(".jsono") {
            "jsono"
        } else {
            "json"
        };
        Some(format!("lang_{pack}_{target_lang}.{ext}"))
    }

    /// Write/merge Iavra multi-packs for `target_lang` from extracted source-pack entries.
    /// Iavra pack values carry the game's own hand-wrapped line breaks, but a
    /// provider returns one flat line, which overflows the message window.
    /// Restore the source's line width when the translation exceeds it.
    ///
    /// A single-line source is a name/label slot — it renders on one line, so
    /// an overlong translation needs shorter wording, never a line break.
    fn rewrap_iavra_value(source: &str, translation: &str) -> String {
        if !source.contains('\n') {
            return translation.to_string();
        }
        let width = source.lines().map(visible_len).max().unwrap_or(0);
        if width == 0 || translation.lines().all(|l| visible_len(l) <= width) {
            return translation.to_string();
        }
        let flat = translation.split_whitespace().collect::<Vec<_>>().join(" ");
        wrap_message(&flat, width).join("\n")
    }

    fn inject_add_iavra_packs(
        _game_root: &Path,
        data_dir: &Path,
        target_lang: &str,
        entries: &[StringEntry],
    ) -> Result<InjectionReport> {
        let mut by_pack: HashMap<String, HashMap<String, String>> = HashMap::new();
        let mut pack_source_file: HashMap<String, String> = HashMap::new();
        let mut strings_written = 0usize;
        let mut strings_skipped = 0usize;

        for entry in entries {
            let filename = entry
                .file_path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("");
            if !Self::is_iavra_lang_pack_name(filename) {
                strings_skipped += 1;
                continue;
            }
            let stem = Self::strip_data_ext(filename);
            let Some((pack, _src_lang)) = Self::parse_iavra_pack_stem(stem) else {
                strings_skipped += 1;
                continue;
            };
            pack_source_file
                .entry(pack.to_string())
                .or_insert_with(|| filename.to_string());

            let key = entry
                .id
                .strip_prefix(&format!("{filename}#"))
                .unwrap_or(entry.id.as_str());
            if let Some(ref translation) = entry.translation {
                by_pack.entry(pack.to_string()).or_default().insert(
                    key.to_string(),
                    Self::rewrap_iavra_value(&entry.source, translation),
                );
                strings_written += 1;
            } else {
                strings_skipped += 1;
            }
        }

        let mut files_written = Vec::new();
        let mut files_modified = 0usize;

        for (pack, translations) in by_pack {
            let src_name = pack_source_file
                .get(&pack)
                .cloned()
                .unwrap_or_else(|| format!("lang_{pack}_en.jsono"));
            let Some(target_name) = Self::iavra_target_filename(&src_name, target_lang) else {
                continue;
            };
            let target_path = data_dir.join(&target_name);
            let src_path = data_dir.join(&src_name);

            let mut obj: serde_json::Map<String, serde_json::Value> = if target_path.exists() {
                Self::read_data_json(&target_path)?
                    .as_object()
                    .cloned()
                    .unwrap_or_default()
            } else if src_path.exists() {
                Self::read_data_json(&src_path)?
                    .as_object()
                    .cloned()
                    .unwrap_or_default()
            } else {
                serde_json::Map::new()
            };

            for (k, v) in translations {
                obj.insert(k, serde_json::Value::String(v));
            }

            Self::write_data_file(&target_path, &serde_json::Value::Object(obj))?;
            files_modified += 1;
            files_written.push(target_path);
        }

        Ok(InjectionReport {
            files_modified,
            strings_written,
            strings_skipped,
            warnings: Vec::new(),
            files_written,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn fixture_dir() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("fixtures")
            .join("rpgmaker_mv")
    }

    fn temp_game_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("locust_rpg_{}", uuid::Uuid::new_v4()));
        let src = fixture_dir();
        copy_dir(&src, &dir);
        dir
    }

    fn copy_dir(src: &Path, dst: &Path) {
        fs::create_dir_all(dst).unwrap();
        for entry in walkdir::WalkDir::new(src).follow_links(false) {
            let entry = entry.unwrap();
            let rel = entry.path().strip_prefix(src).unwrap();
            let dest = dst.join(rel);
            if entry.file_type().is_dir() {
                fs::create_dir_all(&dest).unwrap();
            } else {
                fs::copy(entry.path(), &dest).unwrap();
            }
        }
    }

    #[test]
    fn test_detect_mv_directory() {
        let dir = fixture_dir();
        let plugin = RpgMakerMvPlugin::new();
        assert!(plugin.detect(&dir));
    }

    #[test]
    fn test_detect_mv_file() {
        let file = fixture_dir().join("data").join("Actors.json");
        let plugin = RpgMakerMvPlugin::new();
        assert!(plugin.detect(&file));
    }

    #[test]
    fn test_detect_non_rpgmaker() {
        let dir = std::env::temp_dir().join(format!("locust_notrpg_{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        let plugin = RpgMakerMvPlugin::new();
        assert!(!plugin.detect(&dir));
    }

    #[test]
    fn test_encrypted_game_errors_clearly() {
        let dir = std::env::temp_dir().join(format!("locust_enc_{}", uuid::Uuid::new_v4()));
        let data = dir.join("data");
        fs::create_dir_all(&data).unwrap();
        // Encryption-plugin wrapper, not a real System.json
        fs::write(
            data.join("System.json"),
            r#"{"uid":"abc","bid":"MV.1.6.2","data":"VQxPSlhP"}"#,
        )
        .unwrap();

        let plugin = RpgMakerMvPlugin::new();
        let err = plugin.extract(&dir).unwrap_err().to_string();
        assert!(err.contains("encrypted"), "got: {}", err);
        assert!(err.contains("decrypt"), "got: {}", err);
    }

    #[test]
    fn test_extract_actors_names() {
        let plugin = RpgMakerMvPlugin::new();
        let entries = plugin.extract(&fixture_dir()).unwrap();
        let hero = entries.iter().find(|e| e.id == "Actors.json#1#name");
        assert!(hero.is_some());
        assert_eq!(hero.unwrap().source, "Hero");
    }

    #[test]
    fn test_extract_actors_description() {
        let plugin = RpgMakerMvPlugin::new();
        let entries = plugin.extract(&fixture_dir()).unwrap();
        let desc = entries.iter().find(|e| e.id == "Actors.json#1#description");
        assert!(desc.is_some());
        assert_eq!(desc.unwrap().source, "The protagonist");
    }

    #[test]
    fn test_extract_system_game_title() {
        let plugin = RpgMakerMvPlugin::new();
        let entries = plugin.extract(&fixture_dir()).unwrap();
        let title = entries.iter().find(|e| e.id == "System.json#gameTitle");
        assert!(title.is_some());
        assert_eq!(title.unwrap().source, "My RPG Game");
    }

    #[test]
    fn test_extract_system_terms() {
        let plugin = RpgMakerMvPlugin::new();
        let entries = plugin.extract(&fixture_dir()).unwrap();
        let fight = entries
            .iter()
            .find(|e| e.id == "System.json#terms#commands#0");
        assert!(fight.is_some());
        assert_eq!(fight.unwrap().source, "Fight");

        let escape = entries
            .iter()
            .find(|e| e.id == "System.json#terms#commands#1");
        assert!(escape.is_some());
        assert_eq!(escape.unwrap().source, "Escape");
    }

    #[test]
    fn test_extract_map_dialogue_merges_message_block() {
        let plugin = RpgMakerMvPlugin::new();
        let entries = plugin.extract(&fixture_dir()).unwrap();
        // Two consecutive 401 lines form one message block
        let hello = entries
            .iter()
            .find(|e| e.source == "Hello, traveler!\nWelcome to our town.");
        assert!(hello.is_some(), "block entry missing: {:?}",
            entries.iter().map(|e| &e.source).collect::<Vec<_>>());
        let hello = hello.unwrap();
        assert!(hello.tags.contains(&"dialogue".to_string()));
        assert!(hello.id.ends_with("#msg"));
    }

    #[test]
    fn test_visible_len_ignores_control_codes() {
        assert_eq!(visible_len("Hello"), 5);
        assert_eq!(visible_len("\\C[6]Hello\\C[0]"), 5);
        assert_eq!(visible_len("\\N[1] says hi"), 8);
        assert_eq!(visible_len("wait\\."), 4);
        // YEP name tags render in their own box — zero visible width
        assert_eq!(visible_len("\\n<Demon Girl>Hello"), 5);
    }

    #[test]
    fn test_split_name_tag() {
        assert_eq!(split_name_tag("\\n<Demon Girl>Hola"), ("\\n<Demon Girl>", "Hola"));
        assert_eq!(split_name_tag("Hola"), ("", "Hola"));
        assert_eq!(split_name_tag("\\C[6]Hola"), ("", "\\C[6]Hola"));
    }

    #[test]
    fn test_inject_block_preserves_name_tag() {
        let game_dir = temp_game_dir();
        let plugin = RpgMakerMvPlugin::new();
        let mut entries = plugin.extract(&game_dir).unwrap();

        for entry in &mut entries {
            if entry.id.ends_with("#msg") {
                entry.translation = Some(
                    "\\n<Chica Demonio>No es de extrañar que tus amigos te miren por encima del hombro todo el tiempo, de verdad."
                        .to_string(),
                );
            }
        }
        plugin.inject(&game_dir, &entries).unwrap();

        let content = fs::read_to_string(game_dir.join("data").join("Map001.json")).unwrap();
        let json: serde_json::Value = serde_json::from_str(&content).unwrap();
        let list = json["events"][1]["pages"][0]["list"].as_array().unwrap();
        let first_line = list[1]["parameters"][0].as_str().unwrap();
        // Tag intact at the start of the first line despite containing a space
        assert!(first_line.starts_with("\\n<Chica Demonio>"), "{}", first_line);
        // No other line contains a fragment of the tag
        let mut i = 2;
        while list[i]["code"].as_i64() == Some(401) {
            let l = list[i]["parameters"][0].as_str().unwrap();
            assert!(!l.contains('<') && !l.contains('>'), "{}", l);
            assert!(visible_len(l) <= 40, "{}", l);
            i += 1;
        }
    }

    #[test]
    fn test_wrap_message_respects_width() {
        let lines = wrap_message("uno dos tres cuatro cinco seis", 12);
        assert!(lines.iter().all(|l| visible_len(l) <= 12), "{:?}", lines);
        assert_eq!(lines.join(" "), "uno dos tres cuatro cinco seis");
    }

    #[test]
    fn test_rewrap_iavra_value_restores_source_line_width() {
        // Provider flattened a hand-wrapped message: re-wrap to the source width.
        let src = "Hey, I heard your little brother\nand sister have no food to eat?";
        let flat = "Oye, escuché que tu hermanito y tu hermanita no tienen nada de comida, ¿verdad?";
        let out = RpgMakerMvPlugin::rewrap_iavra_value(src, flat);
        let width = src.lines().map(visible_len).max().unwrap();
        assert!(out.contains('\n'), "should have been re-wrapped: {out:?}");
        for line in out.lines() {
            assert!(visible_len(line) <= width, "line over budget: {line:?}");
        }
        assert_eq!(
            out.split_whitespace().collect::<Vec<_>>(),
            flat.split_whitespace().collect::<Vec<_>>(),
            "re-wrapping must not change wording"
        );

        // Single-line source is a name/label slot: never gains a break.
        let name = RpgMakerMvPlugin::rewrap_iavra_value("Dragon Flail", "Mayal de dragón");
        assert_eq!(name, "Mayal de dragón");

        // A translation that already fits is left exactly as the translator wrote it.
        let kept = RpgMakerMvPlugin::rewrap_iavra_value(src, "Corto\ny cabe");
        assert_eq!(kept, "Corto\ny cabe");
    }

    #[test]
    fn test_inject_message_block_rewraps_lines() {
        let game_dir = temp_game_dir();
        let plugin = RpgMakerMvPlugin::new();
        let mut entries = plugin.extract(&game_dir).unwrap();

        // Spanish translation longer than the original two lines
        for entry in &mut entries {
            if entry.id.ends_with("#msg") {
                entry.translation = Some(
                    "¡Hola, viajero cansado de tantos caminos! Bienvenido a nuestro humilde pueblo, espero que disfrutes tu estadía aquí."
                        .to_string(),
                );
            }
        }

        plugin.inject(&game_dir, &entries).unwrap();

        let content = fs::read_to_string(game_dir.join("data").join("Map001.json")).unwrap();
        let json: serde_json::Value = serde_json::from_str(&content).unwrap();
        let list = json["events"][1]["pages"][0]["list"].as_array().unwrap();

        // First command is still the 101 header
        assert_eq!(list[0]["code"].as_i64().unwrap(), 101);
        // Message lines follow, all 401, all within wrap width
        let mut msg_lines = Vec::new();
        let mut i = 1;
        while list[i]["code"].as_i64() == Some(401) {
            msg_lines.push(list[i]["parameters"][0].as_str().unwrap().to_string());
            i += 1;
        }
        assert!(msg_lines.len() >= 2, "expected rewrapped lines: {:?}", msg_lines);
        assert!(msg_lines.iter().all(|l| visible_len(l) <= 40), "{:?}", msg_lines);
        assert_eq!(
            msg_lines.join(" "),
            "¡Hola, viajero cansado de tantos caminos! Bienvenido a nuestro humilde pueblo, espero que disfrutes tu estadía aquí."
        );
        // The end-of-list command (code 0) survived the splice
        assert_eq!(list[i]["code"].as_i64().unwrap(), 0);
        assert_eq!(list.len(), i + 1);
    }

    #[test]
    fn test_extract_skips_empty() {
        let plugin = RpgMakerMvPlugin::new();
        let entries = plugin.extract(&fixture_dir()).unwrap();
        // Actor 1 "note" is empty string, should not be extracted
        let empty_note = entries.iter().find(|e| e.id == "Actors.json#1#note");
        assert!(empty_note.is_none());
    }

    #[test]
    fn test_extract_skips_null() {
        let plugin = RpgMakerMvPlugin::new();
        let entries = plugin.extract(&fixture_dir()).unwrap();
        // Index 0 in Actors.json is null, should not generate entries
        let null_entry = entries.iter().find(|e| e.id.starts_with("Actors.json#0#"));
        assert!(null_entry.is_none());
    }

    #[test]
    fn test_inject_replace_roundtrip() {
        let game_dir = temp_game_dir();
        let plugin = RpgMakerMvPlugin::new();
        let mut entries = plugin.extract(&game_dir).unwrap();

        for entry in &mut entries {
            if entry.id == "Actors.json#1#name" {
                entry.translation = Some("Héroe".to_string());
            }
        }

        plugin.inject(&game_dir, &entries).unwrap();

        let content =
            fs::read_to_string(game_dir.join("data").join("Actors.json")).unwrap();
        let json: serde_json::Value = serde_json::from_str(&content).unwrap();
        let name = json[1]["name"].as_str().unwrap();
        assert_eq!(name, "Héroe");
    }

    #[test]
    fn test_inject_preserves_other_fields() {
        let game_dir = temp_game_dir();
        let plugin = RpgMakerMvPlugin::new();
        let mut entries = plugin.extract(&game_dir).unwrap();

        for entry in &mut entries {
            if entry.id == "Actors.json#1#name" {
                entry.translation = Some("Héroe".to_string());
            }
        }

        plugin.inject(&game_dir, &entries).unwrap();

        let content =
            fs::read_to_string(game_dir.join("data").join("Actors.json")).unwrap();
        let json: serde_json::Value = serde_json::from_str(&content).unwrap();
        assert_eq!(json[1]["characterIndex"].as_i64().unwrap(), 0);
        assert_eq!(json[1]["classId"].as_i64().unwrap(), 1);
    }

    #[test]
    fn test_inject_add_mz_creates_file() {
        let game_dir = temp_game_dir();
        let plugin = RpgMakerMvPlugin::new();
        let mut entries = plugin.extract(&game_dir).unwrap();
        for entry in &mut entries {
            entry.translation = Some(format!("[es] {}", entry.source));
        }

        plugin.inject_add(&game_dir, "es", &entries).unwrap();

        let lang_file = game_dir.join("data").join("Languages").join("es.json");
        assert!(lang_file.exists());
        let content = fs::read_to_string(&lang_file).unwrap();
        let json: serde_json::Value = serde_json::from_str(&content).unwrap();
        assert!(!json.as_object().unwrap().is_empty());
    }

    #[test]
    fn test_inject_add_mv_creates_file() {
        let game_dir = temp_game_dir();
        // Create MV marker
        fs::create_dir_all(game_dir.join("js")).unwrap();
        fs::write(game_dir.join("js").join("rpg_core.js"), "").unwrap();
        fs::create_dir_all(game_dir.join("www").join("data")).unwrap();

        let plugin = RpgMakerMvPlugin::new();
        let mut entries = plugin.extract(&game_dir).unwrap();
        for entry in &mut entries {
            entry.translation = Some(format!("[es] {}", entry.source));
        }

        plugin.inject_add(&game_dir, "es", &entries).unwrap();

        let lang_file = game_dir
            .join("www")
            .join("data")
            .join("i18n")
            .join("es.json");
        assert!(lang_file.exists());
        let content = fs::read_to_string(&lang_file).unwrap();
        let json: serde_json::Value = serde_json::from_str(&content).unwrap();
        assert!(json.get("strings").is_some());
    }

    #[test]
    fn test_extract_handles_system_messages() {
        let plugin = RpgMakerMvPlugin::new();
        let entries = plugin.extract(&fixture_dir()).unwrap();
        let damage = entries
            .iter()
            .find(|e| e.id == "System.json#terms#messages#actorDamage");
        assert!(damage.is_some());
        assert_eq!(damage.unwrap().source, "%1 took %2 damage!");
    }

    fn write_jsono(path: &Path, json: &str) {
        let compressed = lz_str::compress_to_base64(json);
        fs::write(path, compressed).unwrap();
    }

    #[test]
    fn test_detect_por_jsono_only_deploy() {
        let dir = std::env::temp_dir().join(format!("locust_por_{}", uuid::Uuid::new_v4()));
        let data = dir.join("data");
        fs::create_dir_all(&data).unwrap();
        fs::create_dir_all(dir.join("js")).unwrap();
        fs::write(dir.join("js").join("rmmz_core.js"), "// mz").unwrap();
        write_jsono(
            &data.join("System.jsono"),
            r#"{"gameTitle":"POR Game","terms":{"commands":["Fight"]}}"#,
        );
        write_jsono(&data.join("Map001.jsono"), r#"{"displayName":"Town","events":[]}"#);

        let plugin = RpgMakerMvPlugin::new();
        assert!(plugin.detect(&dir), "should detect POR-only MZ deploy");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_extract_system_jsono() {
        let dir = std::env::temp_dir().join(format!("locust_por_ex_{}", uuid::Uuid::new_v4()));
        let data = dir.join("data");
        fs::create_dir_all(&data).unwrap();
        write_jsono(
            &data.join("System.jsono"),
            r#"{"gameTitle":"LZ Title","terms":{"basic":["Level"],"commands":["Fight","Escape"],"params":[],"messages":{}}}"#,
        );

        let plugin = RpgMakerMvPlugin::new();
        let entries = plugin.extract(&dir).unwrap();
        let title = entries
            .iter()
            .find(|e| e.id == "System.jsono#gameTitle")
            .expect("gameTitle from jsono");
        assert_eq!(title.source, "LZ Title");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_extract_iavra_packs_prefer_en() {
        let dir = std::env::temp_dir().join(format!("locust_iavra_{}", uuid::Uuid::new_v4()));
        let data = dir.join("data");
        fs::create_dir_all(&data).unwrap();
        fs::create_dir_all(dir.join("js")).unwrap();
        fs::write(dir.join("js").join("rmmz_core.js"), "// mz").unwrap();
        // Minimal system so known-data walk is happy
        write_jsono(
            &data.join("System.jsono"),
            r#"{"gameTitle":"Iavra Title","terms":{"basic":[],"commands":[],"params":[],"messages":{}}}"#,
        );
        write_jsono(
            &data.join("lang_g_jp.jsono"),
            r#"{"yes":"はい","no":"いいえ"}"#,
        );
        write_jsono(
            &data.join("lang_g_en.jsono"),
            r#"{"yes":"Yes","no":"No","title":"United Front"}"#,
        );
        write_jsono(
            &data.join("lang_h_en.jsono"),
            r#"{"Map001_0":"Hello hero.","Map001_1":"{cm1}"}"#,
        );

        let plugin = RpgMakerMvPlugin::new();
        assert!(plugin.detect(&dir));
        let entries = plugin.extract(&dir).unwrap();

        let yes = entries
            .iter()
            .find(|e| e.id == "lang_g_en.jsono#yes")
            .expect("en pack preferred");
        assert_eq!(yes.source, "Yes");
        assert!(yes.tags.iter().any(|t| t == "iavra"));

        // jp pack must NOT be extracted when en exists
        assert!(
            entries.iter().all(|e| !e.id.contains("lang_g_jp")),
            "should not extract non-source lang packs"
        );

        let dialogue = entries
            .iter()
            .find(|e| e.id == "lang_h_en.jsono#Map001_0")
            .expect("h pack");
        assert_eq!(dialogue.source, "Hello hero.");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_pick_iavra_source_lang_order() {
        assert_eq!(
            RpgMakerMvPlugin::pick_iavra_source_lang(&["zh".into(), "jp".into(), "en".into()]),
            Some("en".into())
        );
        assert_eq!(
            RpgMakerMvPlugin::pick_iavra_source_lang(&["zh".into(), "jp".into()]),
            Some("jp".into())
        );
        assert_eq!(
            RpgMakerMvPlugin::pick_iavra_source_lang(&["zh".into(), "ko".into()]),
            Some("zh".into()) // zh is in the preference list before free-form sort
        );
        assert_eq!(
            RpgMakerMvPlugin::pick_iavra_source_lang(&["ko".into(), "de".into()]),
            Some("de".into()) // neither preferred → alphabetical first
        );
    }

    #[test]
    fn test_inject_add_iavra_writes_target_jsono() {
        let dir = std::env::temp_dir().join(format!("locust_iavra_inj_{}", uuid::Uuid::new_v4()));
        let data = dir.join("data");
        fs::create_dir_all(&data).unwrap();
        fs::create_dir_all(dir.join("js")).unwrap();
        fs::write(dir.join("js").join("rmmz_core.js"), "// mz").unwrap();
        write_jsono(
            &data.join("System.jsono"),
            r#"{"gameTitle":"T","terms":{"basic":[],"commands":[],"params":[],"messages":{}}}"#,
        );
        write_jsono(
            &data.join("lang_g_en.jsono"),
            r#"{"yes":"Yes","no":"No","title":"Front"}"#,
        );
        write_jsono(
            &data.join("lang_h_en.jsono"),
            r#"{"Map001_0":"Hello","Map001_1":"World"}"#,
        );

        let plugin = RpgMakerMvPlugin::new();
        let mut entries = plugin.extract(&dir).unwrap();
        for e in &mut entries {
            if e.id.starts_with("lang_") {
                e.translation = Some(format!("[es] {}", e.source));
            }
        }

        let report = plugin.inject_add(&dir, "es", &entries).unwrap();
        assert!(report.files_modified >= 2, "got {:?}", report.files_written);
        assert!(data.join("lang_g_es.jsono").exists());
        assert!(data.join("lang_h_es.jsono").exists());
        // Source EN packs untouched
        let en = RpgMakerMvPlugin::read_data_json(&data.join("lang_g_en.jsono")).unwrap();
        assert_eq!(en["yes"], "Yes");

        let es = RpgMakerMvPlugin::read_data_json(&data.join("lang_g_es.jsono")).unwrap();
        assert_eq!(es["yes"], "[es] Yes");
        assert_eq!(es["title"], "[es] Front");

        let es_h = RpgMakerMvPlugin::read_data_json(&data.join("lang_h_es.jsono")).unwrap();
        assert_eq!(es_h["Map001_0"], "[es] Hello");

        // Round-trip still LZString
        let raw = fs::read_to_string(data.join("lang_g_es.jsono")).unwrap();
        assert!(!raw.trim_start().starts_with('{'), "should stay encoded jsono");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_inject_replace_jsono_in_place() {
        let dir = std::env::temp_dir().join(format!("locust_por_inj_{}", uuid::Uuid::new_v4()));
        let data = dir.join("data");
        fs::create_dir_all(&data).unwrap();
        write_jsono(
            &data.join("System.jsono"),
            r#"{"gameTitle":"Old","terms":{"basic":[],"commands":[],"params":[],"messages":{}}}"#,
        );

        let plugin = RpgMakerMvPlugin::new();
        let mut entries = plugin.extract(&dir).unwrap();
        for e in &mut entries {
            if e.id == "System.jsono#gameTitle" {
                e.translation = Some("Nuevo Título".into());
            }
        }
        plugin.inject(&dir, &entries).unwrap();

        let json = RpgMakerMvPlugin::read_data_json(&data.join("System.jsono")).unwrap();
        assert_eq!(json["gameTitle"], "Nuevo Título");
        let raw = fs::read_to_string(data.join("System.jsono")).unwrap();
        assert!(!raw.trim_start().starts_with('{'));

        let _ = fs::remove_dir_all(&dir);
    }
}
