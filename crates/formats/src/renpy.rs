use std::collections::HashMap;
use std::path::{Path, PathBuf};

use locust_core::error::Result;
use locust_core::extraction::{FormatPlugin, InjectionReport};
use locust_core::models::{OutputMode, StringEntry};

pub struct RenPyPlugin;

impl RenPyPlugin {
    pub fn new() -> Self {
        Self
    }

    fn find_game_dir(path: &Path) -> Option<PathBuf> {
        if path.is_dir() {
            let game = path.join("game");
            if game.is_dir() {
                return Some(game);
            }
        }
        None
    }

    fn has_rpy_files(dir: &Path) -> bool {
        std::fs::read_dir(dir)
            .map(|entries| {
                entries.filter_map(|e| e.ok()).any(|e| {
                    e.path().extension().map_or(false, |ext| ext == "rpy")
                })
            })
            .unwrap_or(false)
    }

    fn extract_file(file_path: &Path) -> Result<Vec<StringEntry>> {
        let content = std::fs::read_to_string(file_path)?;
        let filename = file_path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();

        let mut entries = Vec::new();
        let mut in_menu = false;

        for (line_idx, line) in content.lines().enumerate() {
            let line_num = line_idx + 1;
            let trimmed = line.trim();

            // Track menu blocks
            if trimmed == "menu:" {
                in_menu = true;
                continue;
            }

            // Menu choice: "Choice text":
            if in_menu {
                if let Some(text) = extract_menu_choice(trimmed) {
                    let id = format!("{}#{}", filename, line_num);
                    let mut entry = StringEntry::new(id, text, file_path.to_path_buf());
                    entry.tags = vec!["menu".to_string()];
                    entries.push(entry);
                    continue;
                }
                // If line is not indented more or is a non-string line, check if still in menu
                if !trimmed.is_empty()
                    && !trimmed.starts_with('"')
                    && !trimmed.starts_with("jump")
                    && !trimmed.starts_with("pass")
                    && !trimmed.starts_with('#')
                {
                    // Could be a say statement after menu — exit menu
                    if !line.starts_with("        ") && !line.starts_with("\t\t") {
                        in_menu = false;
                    }
                }
            }

            // _("text") pattern
            if let Some(text) = extract_underscore_call(trimmed) {
                let id = format!("{}#{}", filename, line_num);
                let mut entry = StringEntry::new(id, text, file_path.to_path_buf());
                entry.tags = vec!["ui_label".to_string()];
                entries.push(entry);
                continue;
            }

            // define gui.xxx = "text"
            if let Some(text) = extract_define_string(trimmed) {
                let id = format!("{}#{}", filename, line_num);
                let mut entry = StringEntry::new(id, text, file_path.to_path_buf());
                entry.tags = vec!["ui_label".to_string()];
                entries.push(entry);
                continue;
            }

            // say statement: character "text" or just "text"
            if !in_menu {
                if let Some((character, text)) = extract_say_statement(trimmed) {
                    let id = format!("{}#{}", filename, line_num);
                    let mut entry = StringEntry::new(id, text, file_path.to_path_buf());
                    entry.tags = vec!["dialogue".to_string()];
                    if let Some(ch) = character {
                        entry.context = Some(ch.to_string());
                    }
                    entries.push(entry);
                }
            }
        }

        Ok(entries)
    }
}

fn extract_quoted_string(s: &str) -> Option<(&str, usize)> {
    let s = s.trim();
    if !s.starts_with('"') {
        return None;
    }
    let inner = &s[1..];
    let mut end = 0;
    let mut escaped = false;
    for (i, ch) in inner.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if ch == '\\' {
            escaped = true;
            continue;
        }
        if ch == '"' {
            end = i;
            return Some((&inner[..end], 1 + end + 1));
        }
    }
    None
}

fn extract_say_statement(line: &str) -> Option<(Option<&str>, &str)> {
    let trimmed = line.trim();

    // Skip non-say lines
    if trimmed.is_empty()
        || trimmed.starts_with('#')
        || trimmed.starts_with("label ")
        || trimmed.starts_with("jump ")
        || trimmed.starts_with("return")
        || trimmed.starts_with("define ")
        || trimmed.starts_with("default ")
        || trimmed.starts_with("menu:")
        || trimmed.starts_with("if ")
        || trimmed.starts_with("elif ")
        || trimmed.starts_with("else:")
        || trimmed.starts_with("while ")
        || trimmed.starts_with("for ")
        || trimmed.starts_with("python:")
        || trimmed.starts_with("init ")
        || trimmed.starts_with("$")
        || trimmed.starts_with("scene ")
        || trimmed.starts_with("show ")
        || trimmed.starts_with("hide ")
        || trimmed.starts_with("with ")
        || trimmed.starts_with("play ")
        || trimmed.starts_with("stop ")
        || trimmed.starts_with("pause")
        || trimmed.starts_with("call ")
        || trimmed.starts_with("pass")
        || trimmed.starts_with("translate ")
        || trimmed.starts_with("_")
    {
        return None;
    }

    // Narrator: just "text"
    if trimmed.starts_with('"') {
        let (text, _) = extract_quoted_string(trimmed)?;
        if !text.is_empty() {
            return Some((None, text));
        }
        return None;
    }

    // Character say: `identifier "text"`
    let parts: Vec<&str> = trimmed.splitn(2, ' ').collect();
    if parts.len() == 2 {
        let character = parts[0];
        // Character must be a simple identifier
        if character
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_')
        {
            let rest = parts[1].trim();
            if rest.starts_with('"') {
                let (text, _) = extract_quoted_string(rest)?;
                if !text.is_empty() {
                    return Some((Some(character), text));
                }
            }
        }
    }

    None
}

fn extract_menu_choice(line: &str) -> Option<&str> {
    let trimmed = line.trim();
    if !trimmed.starts_with('"') {
        return None;
    }
    // "Choice text": or "Choice text"
    let (text, end) = extract_quoted_string(trimmed)?;
    if text.is_empty() {
        return None;
    }
    let after = trimmed[end..].trim();
    if after.is_empty() || after == ":" || after.starts_with(':') {
        return Some(text);
    }
    None
}

fn extract_define_string(line: &str) -> Option<&str> {
    let trimmed = line.trim();
    if !trimmed.starts_with("define ") {
        return None;
    }
    // Skip _() calls — handled separately
    if trimmed.contains("_(") {
        return None;
    }
    let eq_pos = trimmed.find('=')?;
    let after_eq = trimmed[eq_pos + 1..].trim();
    let (text, _) = extract_quoted_string(after_eq)?;
    if !text.is_empty() {
        Some(text)
    } else {
        None
    }
}

fn extract_underscore_call(line: &str) -> Option<&str> {
    let trimmed = line.trim();
    let start = trimmed.find("_(\"")?;
    let inner = &trimmed[start + 2..]; // after `_(`
    let (text, _) = extract_quoted_string(inner)?;
    if !text.is_empty() {
        Some(text)
    } else {
        None
    }
}

impl Default for RenPyPlugin {
    fn default() -> Self {
        Self::new()
    }
}

impl FormatPlugin for RenPyPlugin {
    fn id(&self) -> &str {
        "renpy"
    }

    fn name(&self) -> &str {
        "Ren'Py"
    }

    fn description(&self) -> &str {
        "Ren'Py visual novel .rpy script files"
    }

    fn supported_extensions(&self) -> &[&str] {
        &[".rpy"]
    }

    fn supported_modes(&self) -> Vec<OutputMode> {
        vec![OutputMode::Replace, OutputMode::Add]
    }

    fn detect(&self, path: &Path) -> bool {
        if path.is_file() {
            return path.extension().map_or(false, |ext| ext == "rpy");
        }
        if path.is_dir() {
            if let Some(game_dir) = Self::find_game_dir(path) {
                return Self::has_rpy_files(&game_dir);
            }
        }
        false
    }

    fn extract(&self, path: &Path) -> Result<Vec<StringEntry>> {
        if path.is_file() {
            return Self::extract_file(path);
        }

        let game_dir = Self::find_game_dir(path).ok_or_else(|| {
            locust_core::error::LocustError::ParseError {
                file: path.display().to_string(),
                message: "could not find game/ directory".to_string(),
            }
        })?;

        let mut all = Vec::new();
        for entry in walkdir::WalkDir::new(&game_dir)
            .follow_links(false)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            let fpath = entry.path();
            if fpath.extension().map_or(false, |e| e == "rpy") {
                // Skip tl/ directory
                if let Ok(rel) = fpath.strip_prefix(&game_dir) {
                    if rel.starts_with("tl") {
                        continue;
                    }
                }
                match Self::extract_file(fpath) {
                    Ok(entries) => all.extend(entries),
                    Err(e) => {
                        tracing::warn!("Failed to extract {}: {}", fpath.display(), e);
                    }
                }
            }
        }
        Ok(all)
    }

    fn inject(&self, path: &Path, entries: &[StringEntry]) -> Result<InjectionReport> {
        let mut files_modified = 0;
        let mut strings_written = 0;
        let mut strings_skipped = 0;

        // Group by file
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
            let content = std::fs::read_to_string(file_path)?;
            let filename = file_path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();

            // Build lookup: line_num -> translation
            let mut line_translations: HashMap<usize, &str> = HashMap::new();
            let mut source_lookup: HashMap<usize, &str> = HashMap::new();
            for entry in file_entries {
                let id_suffix = entry.id.strip_prefix(&format!("{}#", filename));
                if let Some(num_str) = id_suffix {
                    if let Ok(line_num) = num_str.parse::<usize>() {
                        if let Some(ref t) = entry.translation {
                            line_translations.insert(line_num, t.as_str());
                            source_lookup.insert(line_num, entry.source.as_str());
                            strings_written += 1;
                        } else {
                            strings_skipped += 1;
                        }
                    }
                }
            }

            let mut new_lines = Vec::new();
            for (line_idx, line) in content.lines().enumerate() {
                let line_num = line_idx + 1;
                if let Some(&translation) = line_translations.get(&line_num) {
                    if let Some(&source) = source_lookup.get(&line_num) {
                        // Replace the source string with translation in-place
                        let new_line = line.replacen(
                            &format!("\"{}\"", source),
                            &format!("\"{}\"", translation),
                            1,
                        );
                        new_lines.push(new_line);
                        continue;
                    }
                }
                new_lines.push(line.to_string());
            }

            std::fs::write(file_path, new_lines.join("\n"))?;
            files_modified += 1;
        }

        Ok(InjectionReport {
            files_modified,
            strings_written,
            strings_skipped,
            warnings: Vec::new(),
        })
    }

    fn inject_add(
        &self,
        path: &Path,
        lang: &str,
        entries: &[StringEntry],
    ) -> Result<InjectionReport> {
        let game_dir = if path.is_dir() {
            Self::find_game_dir(path).unwrap_or_else(|| path.join("game"))
        } else {
            path.parent()
                .unwrap_or(path)
                .to_path_buf()
        };

        let tl_dir = game_dir.join("tl").join(lang);
        std::fs::create_dir_all(&tl_dir)?;

        let mut strings_written = 0;
        let mut strings_skipped = 0;

        // Group by source file
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

        for (filename, file_entries) in &by_file {
            let tl_file = tl_dir.join(filename);
            let mut lines = Vec::new();

            for (idx, entry) in file_entries.iter().enumerate() {
                if let Some(ref translation) = entry.translation {
                    let block_id = format!(
                        "{}_{}",
                        filename.replace('.', "_"),
                        entry.id.replace('#', "_")
                    );
                    let speaker = entry.context.as_deref().unwrap_or("");

                    lines.push(format!("translate {} {}:", lang, block_id));
                    lines.push(format!("    # {}", entry.source));
                    if speaker.is_empty() {
                        lines.push(format!("    \"{}\"", translation));
                    } else {
                        lines.push(format!("    {} \"{}\"", speaker, translation));
                    }
                    lines.push(String::new());
                    strings_written += 1;
                } else {
                    strings_skipped += 1;
                }
            }

            if !lines.is_empty() {
                std::fs::write(&tl_file, lines.join("\n"))?;
            }
        }

        Ok(InjectionReport {
            files_modified: by_file.len(),
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
            .join("renpy")
    }

    fn temp_renpy_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("locust_renpy_{}", uuid::Uuid::new_v4()));
        copy_dir(&fixture_dir(), &dir);
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
    fn test_detect_renpy_dir() {
        let dir = fixture_dir();
        let plugin = RenPyPlugin::new();
        assert!(plugin.detect(&dir));
    }

    #[test]
    fn test_detect_renpy_file() {
        let file = fixture_dir().join("game").join("script.rpy");
        let plugin = RenPyPlugin::new();
        assert!(plugin.detect(&file));
    }

    #[test]
    fn test_detect_non_renpy() {
        let dir = std::env::temp_dir().join(format!("locust_notrenpy_{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        let plugin = RenPyPlugin::new();
        assert!(!plugin.detect(&dir));
    }

    #[test]
    fn test_extract_say_statements() {
        let plugin = RenPyPlugin::new();
        let entries = plugin.extract(&fixture_dir()).unwrap();
        let hello = entries.iter().find(|e| e.source == "Hello, world!");
        assert!(hello.is_some(), "entries: {:?}", entries.iter().map(|e| (&e.id, &e.source)).collect::<Vec<_>>());
        assert_eq!(hello.unwrap().context, Some("e".to_string()));

        let narrator = entries
            .iter()
            .find(|e| e.source == "This is the narrator speaking.");
        assert!(narrator.is_some());
        assert!(narrator.unwrap().context.is_none());
    }

    #[test]
    fn test_extract_menu_choices() {
        let plugin = RenPyPlugin::new();
        let entries = plugin.extract(&fixture_dir()).unwrap();
        let left = entries.iter().find(|e| e.source == "Go left");
        assert!(left.is_some());
        assert!(left.unwrap().tags.contains(&"menu".to_string()));

        let right = entries.iter().find(|e| e.source == "Go right");
        assert!(right.is_some());
        assert!(right.unwrap().tags.contains(&"menu".to_string()));
    }

    #[test]
    fn test_extract_define_strings() {
        let plugin = RenPyPlugin::new();
        let entries = plugin.extract(&fixture_dir()).unwrap();
        let title = entries.iter().find(|e| e.source == "My Visual Novel");
        assert!(title.is_some());
        assert!(title.unwrap().tags.contains(&"ui_label".to_string()));
    }

    #[test]
    fn test_extract_python_i18n() {
        let plugin = RenPyPlugin::new();
        let entries = plugin.extract(&fixture_dir()).unwrap();
        let version = entries.iter().find(|e| e.source == "Version 1.0");
        assert!(version.is_some());
        assert!(version.unwrap().tags.contains(&"ui_label".to_string()));
    }

    #[test]
    fn test_inject_replace_roundtrip() {
        let dir = temp_renpy_dir();
        let plugin = RenPyPlugin::new();
        let mut entries = plugin.extract(&dir).unwrap();

        for entry in &mut entries {
            if entry.source == "Hello, world!" {
                entry.translation = Some("Hola, mundo!".to_string());
            }
        }

        plugin.inject(&dir, &entries).unwrap();

        let content = fs::read_to_string(dir.join("game").join("script.rpy")).unwrap();
        assert!(content.contains("\"Hola, mundo!\""));
        assert!(!content.contains("\"Hello, world!\""));
    }

    #[test]
    fn test_inject_add_creates_tl_dir() {
        let dir = temp_renpy_dir();
        let plugin = RenPyPlugin::new();
        let mut entries = plugin.extract(&dir).unwrap();
        for entry in &mut entries {
            entry.translation = Some(format!("[es] {}", entry.source));
        }

        plugin.inject_add(&dir, "es", &entries).unwrap();

        let tl_dir = dir.join("game").join("tl").join("es");
        assert!(tl_dir.exists());
    }

    #[test]
    fn test_inject_add_format() {
        let dir = temp_renpy_dir();
        let plugin = RenPyPlugin::new();
        let mut entries = plugin.extract(&dir).unwrap();
        for entry in &mut entries {
            entry.translation = Some(format!("[es] {}", entry.source));
        }

        plugin.inject_add(&dir, "es", &entries).unwrap();

        let tl_dir = dir.join("game").join("tl").join("es");
        // Check that at least one translation file was created
        let tl_files: Vec<_> = fs::read_dir(&tl_dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .collect();
        assert!(!tl_files.is_empty());

        // Read and check format
        let content = fs::read_to_string(tl_files[0].path()).unwrap();
        assert!(content.contains("translate es"));
    }

    #[test]
    fn test_entry_ids_include_line_numbers() {
        let plugin = RenPyPlugin::new();
        let entries = plugin.extract(&fixture_dir()).unwrap();
        for entry in &entries {
            let parts: Vec<&str> = entry.id.split('#').collect();
            assert_eq!(parts.len(), 2, "id should be filename#line: {}", entry.id);
            assert!(
                parts[1].parse::<usize>().is_ok(),
                "second part should be a number: {}",
                entry.id
            );
        }
    }
}
