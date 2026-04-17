use std::collections::HashMap;
use std::path::{Path, PathBuf};

use locust_core::error::{LocustError, Result};
use locust_core::extraction::{FormatPlugin, InjectionReport};
use locust_core::models::{OutputMode, StringEntry};

/// Plugin for Unity Engine games.
/// Supports two modes:
/// 1. Text-based VN scripts (SCRIPTS~/ directory with .txt dialogue files)
/// 2. Binary .assets files with length-prefixed UTF-8 strings (heuristic)
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
            if !fpath.extension().map_or(false, |e| e == "txt") {
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

    fn inject_text_scripts(path: &Path, entries: &[StringEntry]) -> Result<InjectionReport> {
        let mut files_modified = 0;
        let mut strings_written = 0;
        let mut strings_skipped = 0;

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
            }
        }

        Ok(InjectionReport {
            files_modified,
            strings_written,
            strings_skipped,
            warnings: Vec::new(),
        })
    }

    // ─── Binary .assets Extraction (fallback) ───────────────────────────────

    fn find_assets_files(path: &Path) -> Vec<PathBuf> {
        let mut assets = Vec::new();
        if path.is_file() && path.extension().map_or(false, |e| e == "assets") {
            assets.push(path.to_path_buf());
            return assets;
        }
        let data_dir = if path.is_dir() {
            if let Some(d) = Self::find_data_dir(path) { d }
            else if path.file_name().map_or(false, |n| n.to_string_lossy().ends_with("_Data")) {
                path.to_path_buf()
            } else { return assets; }
        } else { return assets; };

        for entry in walkdir::WalkDir::new(&data_dir)
            .max_depth(2)
            .follow_links(false)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            let p = entry.path();
            if p.extension().map_or(false, |e| e == "assets") {
                if let Ok(meta) = std::fs::metadata(p) {
                    if meta.len() > 100 {
                        assets.push(p.to_path_buf());
                    }
                }
            }
        }
        assets
    }

    fn extract_strings_from_assets(
        bytes: &[u8],
        filename: &str,
        file_path: &Path,
    ) -> Vec<StringEntry> {
        let mut entries = Vec::new();
        let mut seen = std::collections::HashSet::new();
        let len = bytes.len();
        if len < 8 { return entries; }

        let mut i = 0;
        while i + 4 < len {
            let str_len = u32::from_le_bytes([bytes[i], bytes[i + 1], bytes[i + 2], bytes[i + 3]]) as usize;
            if str_len >= 5 && str_len <= 2000 && i + 4 + str_len <= len {
                if let Ok(text) = std::str::from_utf8(&bytes[i + 4..i + 4 + str_len]) {
                    if is_unity_translatable(text) && seen.insert(text.to_string()) {
                        let id = format!("{}#offset_{}#{}", filename, i, entries.len());
                        let mut entry = StringEntry::new(id, text, file_path.to_path_buf());
                        entry.tags = vec!["unknown".to_string()];
                        entries.push(entry);
                    }
                }
                let aligned = (str_len + 3) & !3;
                i += 4 + aligned;
            } else {
                i += 1;
            }
        }
        entries
    }
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
    if s.is_empty() || s.len() < 5 { return false; }
    let total = s.chars().count();
    let ascii_printable = s.chars().filter(|c| c.is_ascii_graphic() || c.is_ascii_whitespace()).count();
    if (ascii_printable as f64 / total as f64) < 0.85 { return false; }
    let letters = s.chars().filter(|c| c.is_alphabetic()).count();
    if letters < 3 { return false; }
    let has_space = s.contains(' ');
    if !has_space && s.len() > 20 { return false; }
    if s.contains('/') && s.contains('.') && !s.contains(' ') { return false; }
    if s.contains('\\') && s.contains('.') { return false; }
    if s.chars().all(|c| c.is_ascii_uppercase() || c == '_') { return false; }
    if !has_space {
        let transitions = s.as_bytes().windows(2)
            .filter(|w| w[0].is_ascii_lowercase() && w[1].is_ascii_uppercase())
            .count();
        if transitions >= 2 { return false; }
    }
    if s.starts_with("http") || s.starts_with("www.") { return false; }
    if s.contains("::") || (s.contains('.') && !s.contains(' ')) { return false; }
    if s.contains("(){") || s.contains("};") || s.starts_with("using ") ||
       s.starts_with("import ") || s.starts_with("public ") || s.starts_with("private ") {
        return false;
    }
    let punct_ratio = s.chars().filter(|c| !c.is_alphanumeric() && !c.is_whitespace()).count() as f64 / total as f64;
    if punct_ratio > 0.4 { return false; }
    true
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
        "Unity Engine games (text scripts or .assets heuristic extraction)"
    }

    fn supported_extensions(&self) -> &[&str] {
        &[".assets"]
    }

    fn supported_modes(&self) -> Vec<OutputMode> {
        vec![OutputMode::Replace]
    }

    fn detect(&self, path: &Path) -> bool {
        if path.is_file() {
            return path.extension().map_or(false, |e| e == "assets");
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
            e.file_path.extension().map_or(false, |ext| ext == "txt")
        });

        if from_text {
            return Self::inject_text_scripts(path, entries);
        }

        // Binary .assets injection
        let mut files_modified = 0;
        let mut strings_written = 0;
        let mut strings_skipped = 0;
        let mut warnings = Vec::new();

        let mut by_file: HashMap<PathBuf, Vec<&StringEntry>> = HashMap::new();
        for entry in entries {
            by_file.entry(entry.file_path.clone()).or_default().push(entry);
        }

        for (file_path, file_entries) in &by_file {
            if !file_path.exists() { continue; }
            let mut bytes = std::fs::read(file_path)?;
            let mut modified = false;

            for entry in file_entries {
                let translation = match &entry.translation {
                    Some(t) => t,
                    None => { strings_skipped += 1; continue; }
                };
                let orig_bytes = entry.source.as_bytes();
                let trans_bytes = translation.as_bytes();
                if trans_bytes.len() > orig_bytes.len() {
                    warnings.push(format!(
                        "translation for '{}' longer than original, skipping", entry.id
                    ));
                    strings_skipped += 1;
                    continue;
                }
                let orig_len_bytes = (orig_bytes.len() as u32).to_le_bytes();
                let mut needle = Vec::with_capacity(4 + orig_bytes.len());
                needle.extend_from_slice(&orig_len_bytes);
                needle.extend_from_slice(orig_bytes);

                if let Some(pos) = find_bytes_in(&bytes, &needle) {
                    let new_len = trans_bytes.len() as u32;
                    bytes[pos..pos + 4].copy_from_slice(&new_len.to_le_bytes());
                    bytes[pos + 4..pos + 4 + trans_bytes.len()].copy_from_slice(trans_bytes);
                    for b in &mut bytes[pos + 4 + trans_bytes.len()..pos + 4 + orig_bytes.len()] {
                        *b = 0;
                    }
                    strings_written += 1;
                    modified = true;
                } else {
                    strings_skipped += 1;
                }
            }

            if modified {
                std::fs::write(file_path, &bytes)?;
                files_modified += 1;
            }
        }

        Ok(InjectionReport { files_modified, strings_written, strings_skipped, warnings })
    }
}

fn find_bytes_in(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || needle.len() > haystack.len() { return None; }
    haystack.windows(needle.len()).position(|w| w == needle)
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

    #[test]
    fn test_is_translatable() {
        assert!(is_unity_translatable("Hello World"));
        assert!(is_unity_translatable("Press any key to continue"));
        assert!(!is_unity_translatable("abc"));
        assert!(!is_unity_translatable("SOME_CONSTANT_NAME"));
        assert!(!is_unity_translatable("Assets/Textures/player.png"));
        assert!(!is_unity_translatable("UnityEngine.CoreModule"));
    }
}
