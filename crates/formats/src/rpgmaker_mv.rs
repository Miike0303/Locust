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
    ("note", "note"),
    ("profile", "description"),
    ("message1", "dialogue"),
    ("message2", "dialogue"),
    ("message3", "dialogue"),
    ("message4", "dialogue"),
];

#[derive(Debug, Clone, PartialEq)]
pub enum MvMzVersion {
    Mv,
    Mz,
    Unknown,
}

pub struct RpgMakerMvPlugin {
    version: MvMzVersion,
}

impl RpgMakerMvPlugin {
    pub fn new() -> Self {
        Self {
            version: MvMzVersion::Unknown,
        }
    }

    fn find_data_dir(path: &Path) -> Option<PathBuf> {
        if path.is_dir() {
            let direct = path.join("data");
            if direct.is_dir() {
                return Some(direct);
            }
            let www = path.join("www").join("data");
            if www.is_dir() {
                return Some(www);
            }
        }
        None
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
        let stem = name.strip_suffix(".json").unwrap_or(name);
        let stem_lower = stem.to_lowercase();
        for af in ARRAY_FILES {
            if stem_lower == af.to_lowercase() {
                return true;
            }
        }
        if stem_lower == "system" || stem_lower == "commonevents" {
            return true;
        }
        if stem_lower.starts_with("map") && stem_lower[3..].chars().all(|c| c.is_ascii_digit()) {
            return true;
        }
        false
    }

    fn extract_file(file_path: &Path) -> Result<Vec<StringEntry>> {
        let (content, _enc) = EncodingDetector::read_file_auto(file_path)?;
        let filename = file_path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        let stem = filename.strip_suffix(".json").unwrap_or(&filename);
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
        }

        Ok(entries)
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

        Ok(entries)
    }

    fn extract_map(
        filename: &str,
        json: &serde_json::Value,
        file_path: &Path,
    ) -> Result<Vec<StringEntry>> {
        let mut entries = Vec::new();

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

                for (cmd_idx, cmd) in list.iter().enumerate() {
                    let code = cmd.get("code").and_then(|v| v.as_i64()).unwrap_or(0);
                    let params = cmd.get("parameters").and_then(|v| v.as_array());

                    match code {
                        401 => {
                            if let Some(params) = params {
                                if let Some(text) = params.first().and_then(|v| v.as_str()) {
                                    if !text.trim().is_empty() {
                                        let id = format!(
                                            "{}#0#event_{}#page_{}#cmd_{}",
                                            filename, ev_idx, page_idx, cmd_idx
                                        );
                                        let mut entry = StringEntry::new(
                                            id,
                                            text,
                                            file_path.to_path_buf(),
                                        );
                                        entry.tags = vec!["dialogue".to_string()];
                                        entries.push(entry);
                                    }
                                }
                            }
                        }
                        102 => {
                            if let Some(params) = params {
                                if let Some(choices) =
                                    params.first().and_then(|v| v.as_array())
                                {
                                    for (ci, choice) in choices.iter().enumerate() {
                                        if let Some(text) = choice.as_str() {
                                            if !text.trim().is_empty() {
                                                let id = format!(
                                                    "{}#0#event_{}#page_{}#cmd_{}#choice_{}",
                                                    filename, ev_idx, page_idx, cmd_idx, ci
                                                );
                                                let mut entry = StringEntry::new(
                                                    id,
                                                    text,
                                                    file_path.to_path_buf(),
                                                );
                                                entry.tags = vec!["menu".to_string()];
                                                entries.push(entry);
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        _ => {}
                    }
                }
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

            for (cmd_idx, cmd) in list.iter().enumerate() {
                let code = cmd.get("code").and_then(|v| v.as_i64()).unwrap_or(0);
                let params = cmd.get("parameters").and_then(|v| v.as_array());

                match code {
                    401 => {
                        if let Some(params) = params {
                            if let Some(text) = params.first().and_then(|v| v.as_str()) {
                                if !text.trim().is_empty() {
                                    let id = format!(
                                        "{}#{}#cmd_{}",
                                        filename, ev_idx, cmd_idx
                                    );
                                    let mut entry =
                                        StringEntry::new(id, text, file_path.to_path_buf());
                                    entry.tags = vec!["dialogue".to_string()];
                                    entries.push(entry);
                                }
                            }
                        }
                    }
                    102 => {
                        if let Some(params) = params {
                            if let Some(choices) = params.first().and_then(|v| v.as_array()) {
                                for (ci, choice) in choices.iter().enumerate() {
                                    if let Some(text) = choice.as_str() {
                                        if !text.trim().is_empty() {
                                            let id = format!(
                                                "{}#{}#cmd_{}#choice_{}",
                                                filename, ev_idx, cmd_idx, ci
                                            );
                                            let mut entry = StringEntry::new(
                                                id,
                                                text,
                                                file_path.to_path_buf(),
                                            );
                                            entry.tags = vec!["menu".to_string()];
                                            entries.push(entry);
                                        }
                                    }
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
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

        // Array file: "1#name"
        if parts.len() == 2 {
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

        // Map/CommonEvents commands: "0#event_1#page_0#cmd_5" or "1#cmd_3"
        // Find the command by navigating the event structure
        if suffix.contains("event_") && suffix.contains("cmd_") {
            Self::apply_map_translation(json, suffix, translation);
        } else if suffix.contains("cmd_") {
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
        } else if let Some(params) = cmd.get_mut("parameters").and_then(|v| v.as_array_mut()) {
            if let Some(first) = params.first_mut() {
                *first = serde_json::Value::String(translation.to_string());
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
        } else if let Some(params) = cmd.get_mut("parameters").and_then(|v| v.as_array_mut()) {
            if let Some(first) = params.first_mut() {
                *first = serde_json::Value::String(translation.to_string());
            }
        }
    }
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
        &[".json"]
    }

    fn supported_modes(&self) -> Vec<OutputMode> {
        vec![OutputMode::Replace, OutputMode::Add]
    }

    fn detect(&self, path: &Path) -> bool {
        if path.is_dir() {
            if let Some(data_dir) = Self::find_data_dir(path) {
                let has_actors = data_dir.join("Actors.json").exists();
                let has_system = data_dir.join("System.json").exists();
                let has_map = data_dir.join("Map001.json").exists();
                return has_actors || has_system || has_map;
            }
            return false;
        }
        if path.is_file() {
            if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                return Self::is_known_data_file(name);
            }
        }
        false
    }

    fn extract(&self, path: &Path) -> Result<Vec<StringEntry>> {
        if path.is_file() {
            return Self::extract_file(path);
        }

        let data_dir = Self::find_data_dir(path).ok_or_else(|| {
            LocustError::ParseError {
                file: path.display().to_string(),
                message: "could not find data directory".to_string(),
            }
        })?;

        let mut all_entries = Vec::new();
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

            let (content, _enc) = EncodingDetector::read_file_auto(&file_path)?;
            let mut json: serde_json::Value = serde_json::from_str(&content)?;

            for entry in file_entries {
                if let Some(ref translation) = entry.translation {
                    Self::apply_translation(&mut json, filename, &entry.id, translation);
                    strings_written += 1;
                } else {
                    strings_skipped += 1;
                }
            }

            let output = serde_json::to_string_pretty(&json)?;
            std::fs::write(&file_path, output)?;
            files_modified += 1;
        }

        Ok(InjectionReport {
            files_modified,
            strings_written,
            strings_skipped,
            warnings,
        })
    }

    fn inject_add(
        &self,
        path: &Path,
        lang: &str,
        entries: &[StringEntry],
    ) -> Result<InjectionReport> {
        let game_root = if path.is_dir() { path } else { path.parent().unwrap_or(path) };
        let version = Self::detect_version(game_root);
        let mut strings_written = 0;
        let mut strings_skipped = 0;

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
            }
        }

        Ok(InjectionReport {
            files_modified: 1,
            strings_written,
            strings_skipped,
            warnings: Vec::new(),
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
    fn test_extract_map_dialogue() {
        let plugin = RpgMakerMvPlugin::new();
        let entries = plugin.extract(&fixture_dir()).unwrap();
        let hello = entries
            .iter()
            .find(|e| e.source == "Hello, traveler!");
        assert!(hello.is_some());
        assert!(hello.unwrap().tags.contains(&"dialogue".to_string()));
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
        assert!(json.as_object().unwrap().len() > 0);
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
}
