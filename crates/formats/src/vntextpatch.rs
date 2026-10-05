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

    fn invalid_entry(id: &str, message: &str) -> LocustError {
        LocustError::InjectionError(format!("VNTextPatch entry '{id}': {message}"))
    }

    fn injection_id(id: &str) -> Result<(&str, usize, &str)> {
        // Filenames may legitimately contain '#'; only the final two fields
        // are separators in IDs emitted by extract().
        let mut parts = id.rsplitn(3, '#');
        let key = parts.next().unwrap_or_default();
        let index = parts.next().and_then(|value| value.parse::<usize>().ok());
        let name = parts.next().unwrap_or_default();
        let index = index.ok_or_else(|| Self::invalid_entry(id, "invalid row index"))?;
        if !Self::KEYS.contains(&key) {
            return Err(Self::invalid_entry(id, "invalid translation field"));
        }
        if name.is_empty()
            || name.chars().any(|c| {
                c.is_control() || matches!(c, '/' | '\\' | ':' | '<' | '>' | '"' | '|' | '?' | '*')
            })
            || name.trim_end_matches(['.', ' ']) != name
        {
            return Err(Self::invalid_entry(
                id,
                "expected a portable filename without directories",
            ));
        }
        let safe = locust_core::patch::zipsec::safe_stored_rel(name)?;
        if safe.components().count() != 1 || safe.as_os_str() != name {
            return Err(Self::invalid_entry(id, "expected an exact filename"));
        }
        Ok((name, index, key))
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
            return path.extension().is_some_and(|e| e == "json") && Self::looks_like_vntp(path);
        }
        if path.is_dir() {
            return Self::json_files(path)
                .iter()
                .any(|f| Self::looks_like_vntp(f));
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
                    let mut entry =
                        StringEntry::new(format!("{}#{}#{}", fname, idx, key), val, file.clone());
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
        use locust_core::patch::zipsec::ensure_no_links;
        use std::collections::BTreeMap;

        let single_file = path.is_file();
        let parent = if single_file {
            path.parent()
                .filter(|p| !p.as_os_str().is_empty())
                .unwrap_or(Path::new("."))
        } else {
            path
        };
        // Resolve the explicitly selected root once (including root aliases).
        // Interior destination links/reparse points are still refused below.
        let dir = parent.canonicalize()?;
        let selected = if single_file {
            let name = path
                .file_name()
                .ok_or_else(|| Self::invalid_entry("", "missing selected filename"))?;
            ensure_no_links(&dir, Path::new(name))?;
            Some(dir.join(name).canonicalize()?)
        } else {
            None
        };
        let mut by_file: BTreeMap<PathBuf, Vec<(usize, &str, &StringEntry)>> = BTreeMap::new();

        // Preflight every entry before reading/preparing/writing any output.
        // An id may identify a row, never authorize a new filesystem location.
        for entry in entries {
            let (name, index, key) = Self::injection_id(&entry.id)?;
            if entry.file_path.file_name() != Some(std::ffi::OsStr::new(name)) {
                return Err(Self::invalid_entry(
                    &entry.id,
                    "filename does not match file_path",
                ));
            }
            let absolute_entry = std::path::absolute(&entry.file_path)?;
            let parent_matches = absolute_entry
                .parent()
                .and_then(|p| p.canonicalize().ok())
                .is_some_and(|p| p == dir);
            let basename_only = !entry.file_path.is_absolute()
                && entry
                    .file_path
                    .components()
                    .filter(|c| !matches!(c, std::path::Component::CurDir))
                    .count()
                    == 1;
            if !parent_matches && !basename_only {
                return Err(Self::invalid_entry(
                    &entry.id,
                    "file_path is outside the selected directory",
                ));
            }
            let relative = Path::new(name);
            ensure_no_links(&dir, relative)?;
            let target = dir.join(relative);
            if !target.is_file() {
                return Err(Self::invalid_entry(
                    &entry.id,
                    "target is not an existing regular file",
                ));
            }
            let target = target.canonicalize()?;
            if target.parent() != Some(dir.as_path())
                || selected
                    .as_ref()
                    .is_some_and(|selected| selected != &target)
            {
                return Err(Self::invalid_entry(
                    &entry.id,
                    "target is outside the selected game file or directory",
                ));
            }
            by_file.entry(target).or_default().push((index, key, entry));
        }

        let mut report = InjectionReport {
            skip_reasons: Default::default(),
            files_modified: 0,
            strings_written: 0,
            strings_skipped: 0,
            warnings: Vec::new(),
            files_written: Vec::new(),
        };
        let mut prepared = Vec::new();
        for (file_path, file_entries) in by_file {
            let text = std::fs::read_to_string(&file_path)?;
            let mut arr: Vec<serde_json::Value> = serde_json::from_str(&text)?;
            let mut modified = false;
            for (index, key, entry) in file_entries {
                let Some(translation) = entry.translation.as_deref().filter(|t| !t.is_empty())
                else {
                    report.skip("untranslated", 1);
                    continue;
                };
                if let Some(obj) = arr.get_mut(index).and_then(|v| v.as_object_mut()) {
                    if obj.contains_key(key) {
                        obj.insert(
                            key.to_string(),
                            serde_json::Value::String(translation.to_string()),
                        );
                        report.strings_written += 1;
                        modified = true;
                    }
                }
            }
            if modified {
                // Prepare all JSON before the first write, including later files.
                prepared.push((file_path, serde_json::to_string_pretty(&arr)?));
            }
        }
        for (file, _) in &prepared {
            ensure_no_links(
                &dir,
                file.strip_prefix(&dir).map_err(|_| {
                    Self::invalid_entry("", "target moved outside selected directory")
                })?,
            )?;
        }
        for (file, output) in prepared {
            ensure_no_links(
                &dir,
                file.strip_prefix(&dir).map_err(|_| {
                    Self::invalid_entry("", "target moved outside selected directory")
                })?,
            )?;
            std::fs::write(&file, output)?;
            report.files_written.push(file);
        }
        report.files_modified = report.files_written.len();

        Ok(report)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn c123_named_untranslated_skips_preserve_writes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("story.json");
        fs::write(
            &path,
            r#"[{"message":"First"},{"message":"Second"},{"message":"Third"}]"#,
        )
        .unwrap();
        let plugin = VnTextPatchPlugin::new();
        let mut entries = plugin.extract(&path).unwrap();
        entries[1].translation = Some(String::new());
        entries[2].translation = Some("Translated".into());
        let mut report = plugin.inject(&path, &entries).unwrap();
        assert_eq!((report.strings_written, report.strings_skipped), (1, 2));
        report.classify_remaining_skips();
        assert_eq!(report.skip_reasons, [("untranslated".into(), 2)].into());
        let output: serde_json::Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
        assert_eq!(output[0]["message"], "First");
        assert_eq!(output[1]["message"], "Second");
        assert_eq!(output[2]["message"], "Translated");
    }

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
            e.translation = Some(
                match e.source.as_str() {
                    "太郎" => "Taro",
                    "こんにちは。" => "Hello.",
                    "元気ですか？" => "How are you?",
                    "…" => "...",
                    _ => "",
                }
                .to_string(),
            );
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
    fn assert_unsafe_id_has_no_writes(absolute: bool) {
        let root = tempfile::tempdir().unwrap();
        let game = root.path().join("game");
        fs::create_dir(&game).unwrap();
        let local = game.join("valid.json");
        let outside = root.path().join("outside.json");
        let original = r#"[{"message":"original"}]"#;
        fs::write(&local, original).unwrap();
        fs::write(&outside, original).unwrap();
        let plugin = VnTextPatchPlugin::new();
        let mut good = plugin.extract(&local).unwrap().remove(0);
        good.translation = Some("valid earlier translation".into());
        let mut bad = good.clone();
        bad.id = format!(
            "{}#0#message",
            if absolute {
                outside.to_string_lossy().into_owned()
            } else {
                "../outside.json".into()
            }
        );
        bad.translation = Some("must never reach outside".into());
        let result = plugin.inject(&game, &[good, bad]);
        assert_eq!(
            fs::read_to_string(&outside).unwrap(),
            original,
            "escaped the selected game"
        );
        assert_eq!(
            fs::read_to_string(&local).unwrap(),
            original,
            "wrote a valid entry before rejecting the invalid target"
        );
        assert!(
            result.is_err(),
            "unsafe entry must reject the complete batch"
        );
    }

    #[test]
    fn vntp_preflight_rejects_traversal_before_any_write() {
        assert_unsafe_id_has_no_writes(false);
    }

    #[test]
    fn vntp_preflight_rejects_absolute_before_any_write() {
        assert_unsafe_id_has_no_writes(true);
    }
    fn fixture_entry(path: &Path) -> StringEntry {
        let mut entry = StringEntry::new(
            format!("{}#0#message", path.file_name().unwrap().to_string_lossy()),
            "original",
            path.to_path_buf(),
        );
        entry.translation = Some("translated".into());
        entry
    }

    #[test]
    fn vntp_preflight_rejects_portable_path_aliases_before_any_write() {
        let root = tempfile::tempdir().unwrap();
        let file = root.path().join("valid.json");
        let original = r#"[{"message":"original"}]"#;
        fs::write(&file, original).unwrap();
        for name in [
            "..\\outside.json",
            "./valid.json",
            "sub/file.json",
            "sub\\file.json",
            "/outside.json",
            "C:outside.json",
            "\\\\server\\share\\outside.json",
            "valid.json:stream",
            "NUL.json",
            "CON.json",
            "COM1.json",
            "LPT9.json",
            "valid.json.",
            "valid.json ",
            "invalid?.json",
            "bad\n.json",
            "",
            ".",
            "..",
        ] {
            let good = fixture_entry(&file);
            let mut bad = good.clone();
            bad.id = format!("{name}#0#message");
            assert!(
                VnTextPatchPlugin::new()
                    .inject(root.path(), &[good, bad])
                    .is_err(),
                "accepted {name:?}"
            );
            assert_eq!(fs::read_to_string(&file).unwrap(), original);
        }
    }

    #[test]
    fn vntp_preflight_binds_id_to_file_path_and_selected_directory() {
        let root = tempfile::tempdir().unwrap();
        let game = root.path().join("game");
        let outside = root.path().join("outside");
        fs::create_dir(&game).unwrap();
        fs::create_dir(&outside).unwrap();
        let original = r#"[{"message":"original"}]"#;
        for file in [
            game.join("a.json"),
            game.join("b.json"),
            outside.join("b.json"),
        ] {
            fs::write(file, original).unwrap();
        }
        let good = fixture_entry(&game.join("a.json"));
        for bad_path in [game.join("a.json"), outside.join("b.json")] {
            let mut bad = fixture_entry(&bad_path);
            bad.id = "b.json#0#message".into();
            bad.translation = None; // Untranslated entries still cannot carry unsafe destinations.
            assert!(VnTextPatchPlugin::new()
                .inject(&game, &[good.clone(), bad])
                .is_err());
            for file in [
                game.join("a.json"),
                game.join("b.json"),
                outside.join("b.json"),
            ] {
                assert_eq!(fs::read_to_string(file).unwrap(), original);
            }
        }
    }

    #[test]
    fn vntp_preflight_single_file_cannot_authorize_a_sibling() {
        let root = tempfile::tempdir().unwrap();
        let a = root.path().join("a.json");
        let b = root.path().join("b.json");
        let original = r#"[{"message":"original"}]"#;
        fs::write(&a, original).unwrap();
        fs::write(&b, original).unwrap();
        assert!(VnTextPatchPlugin::new()
            .inject(&a, &[fixture_entry(&a), fixture_entry(&b)])
            .is_err());
        assert_eq!(fs::read_to_string(&a).unwrap(), original);
        assert_eq!(fs::read_to_string(&b).unwrap(), original);
    }

    #[test]
    fn vntp_preflight_keeps_unicode_hash_filenames_and_relative_file_paths() {
        let root = tempfile::tempdir().unwrap();
        let name = "章 #part 2.json";
        let file = root.path().join(name);
        fs::write(&file, r#"[{"name":"太郎","message":"こんにちは"}]"#).unwrap();
        let plugin = VnTextPatchPlugin::new();
        let mut entries = plugin.extract(&file).unwrap();
        assert_eq!(entries[0].id, format!("{name}#0#name"));
        for entry in &mut entries {
            entry.translation = Some(format!("Translated {}", entry.source));
            entry.file_path = PathBuf::from(name);
        }
        let report = plugin.inject(&file, &entries).unwrap();
        assert_eq!(report.strings_written, 2);
        let result: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&file).unwrap()).unwrap();
        assert_eq!(result[0]["name"], "Translated 太郎");
        assert_eq!(result[0]["message"], "Translated こんにちは");
    }

    #[test]
    fn vntp_preflight_missing_target_and_later_invalid_json_cannot_write_earlier_file() {
        let root = tempfile::tempdir().unwrap();
        let a = root.path().join("a.json");
        let z = root.path().join("z.json");
        let original = r#"[{"message":"original"}]"#;
        fs::write(&a, original).unwrap();
        let entries = [fixture_entry(&a), fixture_entry(&z)];
        let plugin = VnTextPatchPlugin::new();
        assert!(plugin.inject(root.path(), &entries).is_err());
        assert_eq!(fs::read_to_string(&a).unwrap(), original);
        assert!(!z.exists());
        fs::write(&z, "invalid JSON sentinel").unwrap();
        assert!(plugin.inject(root.path(), &entries).is_err());
        assert_eq!(fs::read_to_string(&a).unwrap(), original);
        assert_eq!(fs::read_to_string(&z).unwrap(), "invalid JSON sentinel");
    }

    #[test]
    fn vntp_preflight_rejects_malformed_ids_and_nontranslation_fields() {
        let root = tempfile::tempdir().unwrap();
        let file = root.path().join("a.json");
        let original = r#"[{"message":"original","command":"must stay unchanged"}]"#;
        fs::write(&file, original).unwrap();
        for id in [
            "a.json",
            "a.json#bad#message",
            "a.json#0#command",
            "a.json#0#message#extra",
        ] {
            let good = fixture_entry(&file);
            let mut bad = good.clone();
            bad.id = id.into();
            assert!(VnTextPatchPlugin::new()
                .inject(root.path(), &[good, bad])
                .is_err());
            assert_eq!(fs::read_to_string(&file).unwrap(), original);
        }
    }

    #[cfg(any(unix, windows))]
    fn directory_link(target: &Path, link: &Path) {
        #[cfg(unix)]
        std::os::unix::fs::symlink(target, link).unwrap();
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            let output=std::process::Command::new("powershell.exe")
                .args(["-NoProfile","-NonInteractive","-Command","New-Item -ItemType Junction -Path $env:LOCUST_VNTP_TEST_LINK -Target $env:LOCUST_VNTP_TEST_TARGET -ErrorAction Stop | Out-Null"])
                .env("LOCUST_VNTP_TEST_LINK",link).env("LOCUST_VNTP_TEST_TARGET",target)
                .creation_flags(0x08000000).output().unwrap();
            assert!(
                output.status.success(),
                "junction: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
    }

    #[cfg(any(unix, windows))]
    fn remove_directory_link(link: &Path) {
        #[cfg(unix)]
        fs::remove_file(link).unwrap();
        #[cfg(windows)]
        fs::remove_dir(link).unwrap();
    }

    #[test]
    #[cfg(any(unix, windows))]
    fn vntp_preflight_rejects_interior_reparse_targets_and_preserves_sentinels() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let file = root.path().join("a.json");
        let link = root.path().join("b.json");
        let original = r#"[{"message":"original"}]"#;
        fs::write(&file, original).unwrap();
        fs::write(outside.path().join("sentinel"), b"original outside bytes").unwrap();
        directory_link(outside.path(), &link);
        let result = VnTextPatchPlugin::new()
            .inject(root.path(), &[fixture_entry(&file), fixture_entry(&link)]);
        remove_directory_link(&link);
        assert!(result.unwrap_err().to_string().contains("linked game path"));
        assert_eq!(fs::read_to_string(&file).unwrap(), original);
        assert_eq!(
            fs::read(outside.path().join("sentinel")).unwrap(),
            b"original outside bytes"
        );
    }

    #[test]
    #[cfg(any(unix, windows))]
    fn vntp_preflight_allows_explicit_root_alias() {
        let root = tempfile::tempdir().unwrap();
        let game = tempfile::tempdir().unwrap();
        let file = game.path().join("a.json");
        fs::write(&file, r#"[{"message":"original"}]"#).unwrap();
        let link = root.path().join("selected-game");
        directory_link(game.path(), &link);
        let result = VnTextPatchPlugin::new().inject(&link, &[fixture_entry(&file)]);
        remove_directory_link(&link);
        assert_eq!(result.unwrap().strings_written, 1);
        assert!(fs::read_to_string(&file).unwrap().contains("translated"));
    }
}
