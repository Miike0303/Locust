use std::path::{Path, PathBuf};

use locust_core::error::{LocustError, Result};
use locust_core::extraction::{FormatPlugin, FormatStability, InjectionReport};
use locust_core::models::{OutputMode, StringEntry};

/// Visual-novel text via VNTextPatch (arcusmaximus/VNTranslationTools).
///
/// VNTextPatch extracts the scripts of many VN engines — KiriKiri, YU-RIS,
/// CatSystem2, Artemis, Majiro, and more — into per-script JSON files, each a
/// flat array of objects like `{"name": "...", "message": "..."}`. Rather than
/// re-implement every binary archive/script format, Locust translates those
/// JSON files with its full LLM pipeline and writes them back in place; the
/// user then re-runs VNTextPatch to re-inject. One plugin, every engine
/// VNTextPatch supports.
pub struct VnTextPatchPlugin;

impl VnTextPatchPlugin {
    pub fn new() -> Self {
        Self
    }

    /// Translatable string keys in a VNTextPatch entry, in a stable order.
    const KEYS: [&'static str; 2] = ["name", "message"];

    fn json_files(dir: &Path) -> Vec<PathBuf> {
        let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|e| e == "json"))
            .collect();
        files.sort();
        files
    }

    /// A file is VNTextPatch output if it parses as an array whose objects
    /// carry a "message" string field. Keying on "message" (not "name")
    /// avoids matching RPG Maker data files, whose array elements also have a
    /// "name" field.
    fn looks_like_vntp(path: &Path) -> bool {
        let Ok(text) = std::fs::read_to_string(path) else {
            return false;
        };
        let Ok(serde_json::Value::Array(arr)) = serde_json::from_str::<serde_json::Value>(&text)
        else {
            return false;
        };
        arr.iter()
            .any(|v| v.get("message").and_then(|m| m.as_str()).is_some())
    }
}

impl Default for VnTextPatchPlugin {
    fn default() -> Self {
        Self::new()
    }
}

impl FormatPlugin for VnTextPatchPlugin {
    fn id(&self) -> &str {
        "vntextpatch"
    }

    fn name(&self) -> &str {
        "Visual Novel (VNTextPatch JSON)"
    }

    fn description(&self) -> &str {
        "VN scripts extracted by VNTextPatch (KiriKiri, YU-RIS, CatSystem2, Artemis, Majiro, …)"
    }

    fn stability(&self) -> FormatStability {
        FormatStability::Experimental
    }

    fn supported_extensions(&self) -> &[&str] {
        &[".json"]
    }

    fn supported_modes(&self) -> Vec<OutputMode> {
        vec![OutputMode::Replace]
    }

    fn detect(&self, path: &Path) -> bool {
        if path.is_file() {
            return path.extension().is_some_and(|e| e == "json")
                && Self::looks_like_vntp(path);
        }
        if path.is_dir() {
            return Self::json_files(path).iter().any(|f| Self::looks_like_vntp(f));
        }
        false
    }

    fn extract(&self, path: &Path) -> Result<Vec<StringEntry>> {
        let files = if path.is_file() {
            vec![path.to_path_buf()]
        } else {
            Self::json_files(path)
        };

        let mut entries = Vec::new();
        for file in &files {
            if !Self::looks_like_vntp(file) {
                continue;
            }
            let text = std::fs::read_to_string(file)?;
            let arr: Vec<serde_json::Value> =
                serde_json::from_str(&text).map_err(|e| LocustError::ParseError {
                    file: file.display().to_string(),
                    message: e.to_string(),
                })?;
            let fname = file.file_name().unwrap_or_default().to_string_lossy();

            for (idx, obj) in arr.iter().enumerate() {
                // Speaker name (when present) is the message's context.
                let speaker = obj.get("name").and_then(|n| n.as_str());
                for key in Self::KEYS {
                    let Some(val) = obj.get(key).and_then(|v| v.as_str()) else {
                        continue;
                    };
                    if val.trim().is_empty() {
                        continue;
                    }
                    let mut entry = StringEntry::new(
                        format!("{}#{}#{}", fname, idx, key),
                        val,
                        file.clone(),
                    );
                    entry.tags = vec![if key == "name" { "name" } else { "dialogue" }.to_string()];
                    if key == "message" {
                        entry.context = speaker.map(|s| s.to_string());
                    }
                    entries.push(entry);
                }
            }
        }
        Ok(entries)
    }

    fn inject(&self, path: &Path, entries: &[StringEntry]) -> Result<InjectionReport> {
        use std::collections::HashMap;

        // Group translations by (filename, index, key)
        let mut by_file: HashMap<String, Vec<&StringEntry>> = HashMap::new();
        for e in entries {
            let fname = e
                .id
                .split('#')
                .next()
                .unwrap_or_default()
                .to_string();
            by_file.entry(fname).or_default().push(e);
        }

        let dir = if path.is_file() {
            path.parent().unwrap_or(path).to_path_buf()
        } else {
            path.to_path_buf()
        };

        let mut files_modified = 0;
        let mut strings_written = 0;
        let mut strings_skipped = 0;

        for (fname, file_entries) in &by_file {
            let file_path = dir.join(fname);
            if !file_path.exists() {
                continue;
            }
            let text = std::fs::read_to_string(&file_path)?;
            let mut arr: Vec<serde_json::Value> = serde_json::from_str(&text)?;

            let mut modified = false;
            for e in file_entries {
                let Some(translation) = e.translation.as_deref().filter(|t| !t.is_empty()) else {
                    strings_skipped += 1;
                    continue;
                };
                // id = "<file>#<idx>#<key>"
                let parts: Vec<&str> = e.id.splitn(3, '#').collect();
                if parts.len() != 3 {
                    continue;
                }
                let (Ok(idx), key) = (parts[1].parse::<usize>(), parts[2]) else {
                    continue;
                };
                if let Some(obj) = arr.get_mut(idx).and_then(|v| v.as_object_mut()) {
                    if obj.contains_key(key) {
                        obj.insert(
                            key.to_string(),
                            serde_json::Value::String(translation.to_string()),
                        );
                        strings_written += 1;
                        modified = true;
                    }
                }
            }

            if modified {
                // Pretty-print with 2-space indent, matching VNTextPatch output.
                let out = serde_json::to_string_pretty(&arr)?;
                std::fs::write(&file_path, out)?;
                files_modified += 1;
            }
        }

        Ok(InjectionReport {
            files_modified,
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

    fn tmp() -> PathBuf {
        let d = std::env::temp_dir().join(format!("locust_vntp_{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn test_extract_and_inject_roundtrip() {
        let dir = tmp();
        fs::write(
            dir.join("yst00001.json"),
            r#"[{"name":"太郎","message":"こんにちは。"},{"message":"…"},{"message":"元気ですか？"}]"#,
        )
        .unwrap();

        let plugin = VnTextPatchPlugin::new();
        assert!(plugin.detect(&dir));

        let mut entries = plugin.extract(&dir).unwrap();
        // name + message, message ("…" is a real line), message = 4 translatable
        let sources: Vec<&str> = entries.iter().map(|e| e.source.as_str()).collect();
        assert!(sources.contains(&"こんにちは。"));
        assert!(sources.contains(&"太郎"));
        assert!(sources.contains(&"元気ですか？"));
        assert_eq!(entries.len(), 4, "got {:?}", sources);

        // Speaker is carried as context on the message
        let msg = entries.iter().find(|e| e.source == "こんにちは。").unwrap();
        assert_eq!(msg.context.as_deref(), Some("太郎"));

        for e in &mut entries {
            e.translation = Some(match e.source.as_str() {
                "太郎" => "Taro",
                "こんにちは。" => "Hello.",
                "元気ですか？" => "How are you?",
                "…" => "...",
                _ => "",
            }.to_string());
        }
        let report = plugin.inject(&dir, &entries).unwrap();
        assert_eq!(report.strings_written, 4);

        let out: Vec<serde_json::Value> =
            serde_json::from_str(&fs::read_to_string(dir.join("yst00001.json")).unwrap()).unwrap();
        assert_eq!(out[0]["name"], "Taro");
        assert_eq!(out[0]["message"], "Hello.");
        assert_eq!(out[2]["message"], "How are you?");
        assert_eq!(out[1]["message"], "...");
    }

    #[test]
    fn test_detect_rejects_plain_json() {
        let dir = tmp();
        fs::write(dir.join("config.json"), r#"{"width":800,"height":600}"#).unwrap();
        assert!(!VnTextPatchPlugin::new().detect(&dir));
    }
}
