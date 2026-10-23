use std::collections::{BTreeMap, HashSet};
use std::ops::Range;
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

    /// Locate literals in an already validated JSON array. Only depth-two
    /// strings belong to row fields; nested metadata is never an address.
    /// Decode keys so escaped spellings and last-key-wins match serde_json.
    fn string_spans(text: &str) -> Result<BTreeMap<(usize, String), Range<usize>>> {
        let bytes = text.as_bytes();
        let (mut offset, mut depth, mut row) = (0, 0, 0);
        let mut key: Option<String> = None;
        let mut spans = BTreeMap::new();
        while offset < bytes.len() {
            match bytes[offset] {
                b'"' => {
                    let start = offset;
                    offset += 1;
                    while bytes[offset] != b'"' {
                        if bytes[offset] == b'\\' {
                            offset += 1;
                        }
                        offset += 1;
                    }
                    offset += 1;
                    if depth == 2 {
                        if let Some(key) = &key {
                            if Self::KEYS.contains(&key.as_str()) {
                                spans.insert((row, key.clone()), start..offset);
                            }
                        } else {
                            key = Some(serde_json::from_str(&text[start..offset])?);
                        }
                    }
                    continue;
                }
                b'[' | b'{' => depth += 1,
                b']' | b'}' => depth -= 1,
                b',' if depth == 1 => {
                    row += 1;
                    key = None;
                }
                b',' if depth == 2 => key = None,
                _ => {}
            }
            offset += 1;
        }
        Ok(spans)
    }

    /// A file is VNTextPatch output if it parses as an array whose objects
    /// carry a "message" string field. Keying on "message" (not "name")
    /// avoids matching RPG Maker data files, whose array elements also have a
    /// "name" field.
    fn looks_like_vntp(path: &Path) -> bool {
        let Ok(text) = std::fs::read_to_string(path) else {
            return false;
        };
        let Ok(serde_json::Value::Array(arr)) = serde_json::from_str::<serde_json::Value>(
            text.strip_prefix('\u{feff}').unwrap_or(&text),
        ) else {
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
            let arr: Vec<serde_json::Value> = serde_json::from_str(
                text.strip_prefix('\u{feff}').unwrap_or(&text),
            )
            .map_err(|e| LocustError::ParseError {
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
            let json = text.strip_prefix('\u{feff}').unwrap_or(&text);
            let arr: Vec<serde_json::Value> = serde_json::from_str(json)?;
            // Span offsets include the original BOM, when present.
            let spans = Self::string_spans(&text)?;
            let mut seen = HashSet::new();
            let mut edits = Vec::new();
            for (index, key, entry) in file_entries {
                if !seen.insert((index, key)) {
                    report.skip("duplicate", 1);
                    continue;
                }
                let Some(translation) = entry.translation.as_deref().filter(|t| !t.is_empty())
                else {
                    report.skip("untranslated", 1);
                    continue;
                };
                let Some(value) = arr.get(index).and_then(|v| v.get(key)) else {
                    report.skip("target_missing", 1);
                    continue;
                };
                let Some(source) = value.as_str() else {
                    report.skip("not_a_string", 1);
                    continue;
                };
                if source != entry.source {
                    report.skip("source_changed", 1);
                    continue;
                }
                if translation == source {
                    report.skip("unchanged", 1);
                    continue;
                }
                let span = spans
                    .get(&(index, key.to_string()))
                    .ok_or_else(|| Self::invalid_entry(&entry.id, "missing JSON string span"))?;
                edits.push((span.clone(), serde_json::to_string(translation)?));
                report.strings_written += 1;
            }
            if !edits.is_empty() {
                // Apply from the end so earlier offsets remain valid. Prepare
                // every file before writing, preserving the path preflight.
                edits.sort_unstable_by_key(|(span, _)| span.start);
                let mut output = text;
                for (span, literal) in edits.into_iter().rev() {
                    output.replace_range(span, &literal);
                }
                prepared.push((file_path, output));
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

    // Deliberately noncanonical: escaped keys/question marks/slashes, CRLF,
    // nested decoys, exponent notation, and a trailing newline.
    fn safety_fixture() -> String {
        concat!(
            "[\r\n\t{ \"message\" : \"old\", \"name\":\"太郎\", ",
            "\"meta\":{\"message\":\"old\",\"n\":1.2300e+02,",
            "\"nested\":[null,true,{\"x\":\"}\\\"[\"}]} },\r\n",
            "  {\"messa\\u0067e\":\"Why\\u003f\\/\\n\",\"name\":\"Other\"}\r\n]\r\n"
        )
        .into()
    }

    #[test]
    fn vntextpatch_identity_preserves_every_byte() {
        let dir = tmp();
        let path = dir.path().join("story.json");
        let original = safety_fixture();
        fs::write(&path, &original).unwrap();
        let plugin = VnTextPatchPlugin::new();
        let mut entries = plugin.extract(&path).unwrap();
        assert_eq!(entries.len(), 4);
        for entry in &mut entries {
            entry.translation = Some(entry.source.clone());
        }
        let report = plugin.inject(&path, &entries).unwrap();
        assert_eq!(fs::read(&path).unwrap(), original.as_bytes());
        assert_eq!((report.strings_written, report.files_modified), (0, 0));
        assert!(report.files_written.is_empty());
        assert_eq!(report.skip_reasons, [("unchanged".into(), 4)].into());
    }

    #[test]
    fn vntextpatch_translation_changes_only_addressed_literal() {
        let dir = tmp();
        let path = dir.path().join("story.json");
        let original = safety_fixture();
        fs::write(&path, &original).unwrap();
        let plugin = VnTextPatchPlugin::new();
        let mut entries = plugin.extract(&path).unwrap();
        entries[1].translation = Some("New \"quote\" \\ path\n\t\r\u{0001} café?".into());
        let literal = r#""New \"quote\" \\ path\n\t\r\u0001 café?""#;
        let start = original.find(r#""old""#).unwrap();
        let end = start + r#""old""#.len();
        let report = plugin.inject(&path, &entries).unwrap();
        let actual = fs::read(&path).unwrap();
        assert_eq!(&actual[..start], &original.as_bytes()[..start]);
        assert_eq!(&actual[start..start + literal.len()], literal.as_bytes());
        assert_eq!(
            &actual[start + literal.len()..],
            &original.as_bytes()[end..]
        );
        assert_eq!((report.strings_written, report.files_modified), (1, 1));
        assert_eq!(report.files_written, [path.canonicalize().unwrap()]);
    }

    #[test]
    fn vntextpatch_bom_preserved_for_identity_and_multiple_edits() {
        let dir = tmp();
        let path = dir.path().join("story.json");
        let original = format!("\u{feff}{}", safety_fixture());
        fs::write(&path, &original).unwrap();
        let plugin = VnTextPatchPlugin::new();
        assert!(plugin.detect(&path));
        let mut entries = plugin.extract(&path).unwrap();
        assert_eq!(entries.len(), 4);
        for entry in &mut entries {
            entry.translation = Some(entry.source.clone());
        }
        assert_eq!(plugin.inject(&path, &entries).unwrap().files_modified, 0);
        assert_eq!(fs::read(&path).unwrap(), original.as_bytes());
        entries[0].translation = Some("Taro".into());
        entries[3].translation = Some("Longer translated message".into());
        let report = plugin.inject(&path, &entries).unwrap();
        let expected = original
            .replace("太郎", "Taro")
            .replace(r#""Why\u003f\/\n""#, r#""Longer translated message""#);
        assert_eq!(fs::read(&path).unwrap(), expected.as_bytes());
        assert_eq!((report.strings_written, report.files_modified), (2, 1));
    }

    #[test]
    fn vntextpatch_stale_source_skipped_without_write() {
        let dir = tmp();
        let path = dir.path().join("story.json");
        fs::write(&path, r#"[{"message":"original"}]"#).unwrap();
        let entry = fixture_entry(&path);
        let changed = b"[ {\"message\" : \"changed\"} ]\r\n";
        fs::write(&path, changed).unwrap();
        let report = VnTextPatchPlugin::new().inject(&path, &[entry]).unwrap();
        assert_eq!(fs::read(&path).unwrap(), changed);
        assert_eq!(report.skip_reasons, [("source_changed".into(), 1)].into());
        assert_eq!((report.strings_written, report.files_modified), (0, 0));
        assert!(report.files_written.is_empty());
    }

    #[test]
    fn vntextpatch_non_string_targets_skipped_without_write() {
        let dir = tmp();
        let path = dir.path().join("story.json");
        for value in ["42", r#"{"message":"original"}"#, "null", "[]", "false"] {
            let original = format!("[ {{\"message\": {value}}} ]\r\n");
            fs::write(&path, &original).unwrap();
            let report = VnTextPatchPlugin::new()
                .inject(&path, &[fixture_entry(&path)])
                .unwrap();
            assert_eq!(fs::read(&path).unwrap(), original.as_bytes(), "{value}");
            assert_eq!(report.skip_reasons, [("not_a_string".into(), 1)].into());
            assert_eq!((report.strings_written, report.files_modified), (0, 0));
        }
    }

    #[test]
    fn vntextpatch_missing_targets_skipped_without_write() {
        let dir = tmp();
        let path = dir.path().join("story.json");
        for original in ["[]", "[{}]", "[null]", "[[]]", "[42]"] {
            fs::write(&path, original).unwrap();
            let report = VnTextPatchPlugin::new()
                .inject(&path, &[fixture_entry(&path)])
                .unwrap();
            assert_eq!(fs::read(&path).unwrap(), original.as_bytes());
            assert_eq!(report.skip_reasons, [("target_missing".into(), 1)].into());
            assert_eq!((report.strings_written, report.files_modified), (0, 0));
        }
    }

    #[test]
    fn vntextpatch_duplicate_address_second_refused() {
        let dir = tmp();
        let path = dir.path().join("story.json");
        fs::write(&path, r#"[{"message":"original"}]"#).unwrap();
        let first = fixture_entry(&path);
        let mut second = first.clone();
        // Different ID spelling still addresses the same field.
        second.id = "story.json#00#message".into();
        second.translation = Some("must not win".into());
        let report = VnTextPatchPlugin::new()
            .inject(&path, &[first, second])
            .unwrap();
        assert_eq!(fs::read(&path).unwrap(), br#"[{"message":"translated"}]"#);
        assert_eq!(report.strings_written, 1);
        assert_eq!(report.skip_reasons, [("duplicate".into(), 1)].into());
    }

    #[test]
    fn vntextpatch_duplicate_after_identity_cannot_write() {
        let dir = tmp();
        let path = dir.path().join("story.json");
        let original = br#"[{"message":"original"}]"#;
        fs::write(&path, original).unwrap();
        let second = fixture_entry(&path);
        let mut first = second.clone();
        first.translation = Some(first.source.clone());
        let report = VnTextPatchPlugin::new()
            .inject(&path, &[first, second])
            .unwrap();
        assert_eq!(fs::read(&path).unwrap(), original);
        assert_eq!((report.strings_written, report.files_modified), (0, 0));
        assert_eq!(
            report.skip_reasons,
            [("duplicate".into(), 1), ("unchanged".into(), 1)].into()
        );
    }

    #[test]
    fn vntextpatch_duplicate_json_keys_edit_only_effective_value() {
        let dir = tmp();
        let path = dir.path().join("story.json");
        let original = r#"[{"message":"decoy","messa\u0067e":"original"}]"#;
        fs::write(&path, original).unwrap();
        let report = VnTextPatchPlugin::new()
            .inject(&path, &[fixture_entry(&path)])
            .unwrap();
        assert_eq!(report.strings_written, 1);
        assert_eq!(
            fs::read(&path).unwrap(),
            original.replace("original", "translated").as_bytes()
        );
    }

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

    fn tmp() -> tempfile::TempDir {
        tempfile::Builder::new()
            .prefix("locust_vntp_")
            .tempdir()
            .unwrap()
    }

    #[test]
    fn test_extract_and_inject_roundtrip() {
        let dir_guard = tmp();
        let dir = dir_guard.path().to_path_buf();
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
        let dir_guard = tmp();
        let dir = dir_guard.path().to_path_buf();
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
