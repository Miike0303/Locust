use std::collections::HashMap;
use std::io::{Read as IoRead, Seek, SeekFrom};
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

    fn extract_rpa_archive(&self, rpa_path: &Path) -> Result<Vec<StringEntry>> {
        let temp_dir = std::env::temp_dir().join(format!(
            "locust_rpa_{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&temp_dir)?;

        let extracted_files = Self::extract_rpa(rpa_path, &temp_dir)?;

        let mut all = Vec::new();
        for file in &extracted_files {
            // Skip tl/ directory files
            if let Ok(rel) = file.strip_prefix(&temp_dir) {
                let rel_str = rel.to_string_lossy();
                if rel_str.starts_with("tl/") || rel_str.starts_with("tl\\") {
                    continue;
                }
            }
            match Self::extract_file(file) {
                Ok(mut entries) => {
                    // Rewrite file_path to reference the original RPA
                    for entry in &mut entries {
                        entry.file_path = rpa_path.to_path_buf();
                    }
                    all.extend(entries);
                }
                Err(e) => {
                    tracing::warn!("Failed to extract {}: {}", file.display(), e);
                }
            }
        }

        // Cleanup temp dir
        let _ = std::fs::remove_dir_all(&temp_dir);

        Ok(all)
    }

    /// For RPA-sourced entries: extract .rpy from the archive, apply translations in-place,
    /// and write the translated .rpy files into game/ directory.
    /// Ren'Py loads loose .rpy files with priority over .rpa archives.
    fn inject_replace_rpa(&self, path: &Path, entries: &[StringEntry]) -> locust_core::error::Result<InjectionReport> {
        let game_dir = if path.is_dir() {
            Self::find_game_dir(path).unwrap_or_else(|| path.join("game"))
        } else {
            path.parent().unwrap_or(path).to_path_buf()
        };

        // Find unique RPA files referenced by entries
        let mut rpa_files: std::collections::HashSet<PathBuf> = std::collections::HashSet::new();
        for entry in entries {
            if entry.file_path.extension().map_or(false, |ext| ext == "rpa") {
                rpa_files.insert(entry.file_path.clone());
            }
        }

        // Extract .rpy files from each RPA to a temp dir
        let temp_dir = std::env::temp_dir().join(format!("locust_rpa_inject_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&temp_dir)?;

        // Run the actual injection logic, ensuring temp_dir is always cleaned up
        let result = Self::inject_rpa_inner(&game_dir, &temp_dir, &rpa_files, entries);

        // Always cleanup temp dir, even on error
        let _ = std::fs::remove_dir_all(&temp_dir);

        result
    }

    fn inject_rpa_inner(
        game_dir: &Path,
        temp_dir: &Path,
        rpa_files: &std::collections::HashSet<PathBuf>,
        entries: &[StringEntry],
    ) -> locust_core::error::Result<InjectionReport> {
        for rpa_path in rpa_files {
            let _ = Self::extract_rpa(rpa_path, temp_dir);
        }

        // Build a lookup: (filename, line_number) -> (source, translation)
        let mut line_translations: HashMap<(String, usize), (String, String)> = HashMap::new();
        for entry in entries {
            if let Some(ref t) = entry.translation {
                if t != &entry.source {
                    // Entry IDs are "filename.rpy#linenumber" or "archive.rpa#filename.rpy#linenumber"
                    let parts: Vec<&str> = entry.id.split('#').collect();
                    if parts.len() >= 2 {
                        let filename = if parts.len() == 3 {
                            parts[1].to_string() // archive.rpa#filename.rpy#line
                        } else {
                            parts[0].to_string() // filename.rpy#line
                        };
                        let line_str = parts.last().unwrap_or(&"0");
                        if let Ok(line_num) = line_str.parse::<usize>() {
                            line_translations.insert(
                                (filename, line_num),
                                (entry.source.clone(), t.clone()),
                            );
                        }
                    }
                }
            }
        }

        let mut files_modified = 0;
        let mut strings_written = 0;

        // Walk all extracted .rpy files and apply translations by line number
        for dir_entry in walkdir::WalkDir::new(temp_dir)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            let fpath = dir_entry.path();
            if !fpath.extension().map_or(false, |e| e == "rpy") {
                continue;
            }
            // Skip tl/ directory
            if let Ok(rel) = fpath.strip_prefix(temp_dir) {
                let rel_str = rel.to_string_lossy();
                if rel_str.starts_with("tl/") || rel_str.starts_with("tl\\") {
                    continue;
                }
            }

            let content = match std::fs::read_to_string(fpath) {
                Ok(c) => c,
                Err(_) => continue,
            };

            // Get the filename (with subdirectory path from temp_dir)
            let rel_path = fpath.strip_prefix(temp_dir).unwrap_or(fpath);
            let filename = rel_path.file_name().unwrap_or_default().to_string_lossy().to_string();

            let mut modified = false;
            let mut new_lines: Vec<String> = Vec::new();

            for (line_idx, line) in content.lines().enumerate() {
                let line_num = line_idx + 1;
                let key = (filename.clone(), line_num);

                if let Some((source, translation)) = line_translations.get(&key) {
                    let trimmed = line.trim();
                    // Only translate dialogue lines, not code
                    if is_dialogue_line(trimmed) {
                        let search = format!("\"{}\"", source);
                        if line.contains(&search) {
                            let safe_trans = escape_inner_quotes(translation);
                            let replace = format!("\"{}\"", safe_trans);
                            let new_line = line.replace(&search, &replace);
                            new_lines.push(new_line);
                            modified = true;
                            strings_written += 1;
                            continue;
                        }
                    }
                }
                new_lines.push(line.to_string());
            }
            let new_content = new_lines.join("\n");

            if modified {
                // Write translated .rpy to game/ dir (preserving subdirectory structure)
                let rel = fpath.strip_prefix(temp_dir).unwrap_or(fpath);
                let dest = game_dir.join(rel);
                if let Some(parent) = dest.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::write(&dest, &new_content)?;
                files_modified += 1;

                // Delete corresponding .rpyc so Ren'Py recompiles from the modified .rpy
                let rpyc_path = dest.with_extension("rpyc");
                if rpyc_path.exists() {
                    let _ = std::fs::remove_file(&rpyc_path);
                }
            }
        }

        Ok(InjectionReport {
            files_modified,
            strings_written,
            strings_skipped: entries.len().saturating_sub(strings_written),
            warnings: Vec::new(),
        })
    }

    fn has_rpa_files(dir: &Path) -> bool {
        std::fs::read_dir(dir)
            .map(|entries| {
                entries.filter_map(|e| e.ok()).any(|e| {
                    e.path().extension().map_or(false, |ext| ext == "rpa")
                })
            })
            .unwrap_or(false)
    }

    /// Extract .rpy files from a .rpa archive (Ren'Py Archive format).
    /// RPA-3.0 header: `RPA-3.0 <hex_offset> <hex_key>\n`
    /// At offset: zlib-compressed pickle with a dict of filename -> [(offset, length, prefix)]
    fn extract_rpa(rpa_path: &Path, temp_dir: &Path) -> Result<Vec<PathBuf>> {
        let mut file = std::fs::File::open(rpa_path)?;
        let mut header_buf = [0u8; 256];
        let n = file.read(&mut header_buf)?;
        let header = String::from_utf8_lossy(&header_buf[..n]);

        let first_line = header.lines().next().unwrap_or("");
        let parts: Vec<&str> = first_line.split_whitespace().collect();

        if parts.len() < 3 || !parts[0].starts_with("RPA-") {
            return Err(locust_core::error::LocustError::ParseError {
                file: rpa_path.display().to_string(),
                message: "not a valid RPA archive".to_string(),
            });
        }

        let index_offset = u64::from_str_radix(parts[1], 16).map_err(|_| {
            locust_core::error::LocustError::ParseError {
                file: rpa_path.display().to_string(),
                message: "invalid RPA index offset".to_string(),
            }
        })?;

        let key = i64::from_str_radix(parts[2], 16).unwrap_or(0);

        // Read the index (zlib-compressed pickle)
        file.seek(SeekFrom::Start(index_offset))?;
        let mut compressed = Vec::new();
        file.read_to_end(&mut compressed)?;

        // Decompress with zlib (raw deflate with zlib wrapper)
        let decompressed = miniz_oxide::inflate::decompress_to_vec_zlib(&compressed).map_err(|e| {
            locust_core::error::LocustError::ParseError {
                file: rpa_path.display().to_string(),
                message: format!("failed to decompress RPA index: {:?}", e),
            }
        })?;

        // Parse the Python pickle to extract file entries
        // We use a simplified pickle parser that handles the common RPA format
        let index = parse_rpa_pickle(&decompressed, key)?;


        let mut extracted_files = Vec::new();
        for (name, offset, length) in &index {
            // Only extract .rpy files (skip .rpyc compiled files unless no .rpy available)
            if !name.ends_with(".rpy") && !name.ends_with(".rpyc") {
                continue;
            }
            // Prefer .rpy over .rpyc — if both exist, the .rpy will be used
            if name.ends_with(".rpyc") {
                let rpy_name = name.strip_suffix("c").unwrap();
                if index.iter().any(|(n, _, _)| n == rpy_name) {
                    continue;
                }
                // Skip compiled files - can't extract text from binary rpyc
                continue;
            }

            file.seek(SeekFrom::Start(*offset))?;
            let mut data = vec![0u8; *length];
            file.read_exact(&mut data)?;

            let rel_path = Path::new(name);
            let out_path = temp_dir.join(rel_path);
            if let Some(parent) = out_path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(&out_path, &data)?;
            extracted_files.push(out_path);
        }

        Ok(extracted_files)
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
        let mut in_python = false;
        let mut python_indent = 0usize;
        // Track multi-line define blocks (dicts, lists, parenthesized values).
        // These contain internal identifiers/config values, not translatable text.
        let mut define_bracket_depth: i32 = 0;

        for (line_idx, line) in content.lines().enumerate() {
            let line_num = line_idx + 1;
            let trimmed = line.trim();

            // Track multi-line define blocks: `define x = { ... }`, `define x = [ ... ]`, `define x = ( ... )`
            // When opened, skip all content until closed.
            if define_bracket_depth == 0 && trimmed.starts_with("define ") {
                // Count opening vs closing brackets on this line
                let opens = trimmed.matches(|c| c == '{' || c == '[' || c == '(').count() as i32;
                let closes = trimmed.matches(|c| c == '}' || c == ']' || c == ')').count() as i32;
                if opens > closes {
                    define_bracket_depth = opens - closes;
                    // Still process this line (the `define x = {` might have extract logic)
                    // But don't skip — the first line is the define itself
                }
            } else if define_bracket_depth > 0 {
                let opens = trimmed.matches(|c| c == '{' || c == '[' || c == '(').count() as i32;
                let closes = trimmed.matches(|c| c == '}' || c == ']' || c == ')').count() as i32;
                define_bracket_depth += opens - closes;
                if define_bracket_depth < 0 {
                    define_bracket_depth = 0;
                }
                // Skip all content inside the multi-line define block
                continue;
            }

            // Track python blocks (skip most content inside them)
            if trimmed.starts_with("python:") || trimmed.starts_with("init python:")
                || trimmed.starts_with("init -") && trimmed.contains("python:")
            {
                in_python = true;
                python_indent = line.len() - line.trim_start().len();
                // But still check for translatable calls inside python
            }
            if in_python && !trimmed.is_empty() {
                let cur_indent = line.len() - line.trim_start().len();
                if cur_indent <= python_indent && !trimmed.starts_with("python:")
                    && !trimmed.starts_with("init ")
                    && !trimmed.starts_with('#')
                {
                    in_python = false;
                }
            }

            // Skip comments
            if trimmed.starts_with('#') {
                continue;
            }

            // Track menu blocks
            if trimmed == "menu:" || trimmed.starts_with("menu ") && trimmed.ends_with(':') {
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

            // _("text") and __("text") patterns — always translatable
            if let Some(text) = extract_underscore_call(trimmed) {
                let id = format!("{}#{}", filename, line_num);
                let mut entry = StringEntry::new(id, text, file_path.to_path_buf());
                entry.tags = vec!["ui_label".to_string()];
                entries.push(entry);
                continue;
            }

            // _p("""text""") — multi-paragraph translatable text
            if let Some(text) = extract_p_call(trimmed) {
                let id = format!("{}#{}", filename, line_num);
                let mut entry = StringEntry::new(id, text, file_path.to_path_buf());
                entry.tags = vec!["ui_label".to_string()];
                entries.push(entry);
                continue;
            }

            // Character("Name") in define or $ — extract the character name
            if let Some(name) = extract_character_name(trimmed) {
                let id = format!("{}#{}", filename, line_num);
                let mut entry = StringEntry::new(id, name, file_path.to_path_buf());
                entry.tags = vec!["actor_name".to_string()];
                entries.push(entry);
                continue;
            }

            // renpy.notify("text") — player-visible notification
            if let Some(text) = extract_renpy_call(trimmed, "renpy.notify(") {
                let id = format!("{}#{}", filename, line_num);
                let mut entry = StringEntry::new(id, text, file_path.to_path_buf());
                entry.tags = vec!["ui_label".to_string()];
                entries.push(entry);
                continue;
            }

            // renpy.input("prompt") — input prompt text
            if let Some(text) = extract_renpy_call(trimmed, "renpy.input(") {
                let id = format!("{}#{}", filename, line_num);
                let mut entry = StringEntry::new(id, text, file_path.to_path_buf());
                entry.tags = vec!["ui_label".to_string()];
                entries.push(entry);
                continue;
            }

            // Inside python blocks, skip everything else
            if in_python {
                continue;
            }

            // define gui.xxx = "text" (but not file paths, colors, etc.)
            if let Some(text) = extract_define_string(trimmed) {
                let id = format!("{}#{}", filename, line_num);
                let mut entry = StringEntry::new(id, text, file_path.to_path_buf());
                entry.tags = vec!["ui_label".to_string()];
                entries.push(entry);
                continue;
            }

            // Screen UI text: text "string", textbutton "string", tooltip "string"
            if let Some(text) = extract_screen_text(trimmed) {
                let id = format!("{}#{}", filename, line_num);
                let mut entry = StringEntry::new(id, text, file_path.to_path_buf());
                entry.tags = vec!["ui_label".to_string()];
                entries.push(entry);
                continue;
            }

            // centered "text" — always translatable
            if trimmed.starts_with("centered ") {
                let rest = trimmed["centered ".len()..].trim();
                if let Some((text, _)) = extract_quoted_string(rest) {
                    if !text.is_empty() && !is_file_reference(text) {
                        let id = format!("{}#{}", filename, line_num);
                        let mut entry = StringEntry::new(id, text, file_path.to_path_buf());
                        entry.tags = vec!["dialogue".to_string()];
                        entries.push(entry);
                        continue;
                    }
                }
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
        // Image/UI property keywords
        || trimmed.starts_with("idle ")
        || trimmed.starts_with("hover ")
        || trimmed.starts_with("insensitive ")
        || trimmed.starts_with("selected_idle ")
        || trimmed.starts_with("selected_hover ")
        || trimmed.starts_with("ground ")
        || trimmed.starts_with("image ")
        || trimmed.starts_with("add ")
        || trimmed.starts_with("use ")
        || trimmed.starts_with("screen ")
        || trimmed.starts_with("style ")
        || trimmed.starts_with("transform ")
        || trimmed.starts_with("at ")
        || trimmed.starts_with("xpos ")
        || trimmed.starts_with("ypos ")
        || trimmed.starts_with("xalign ")
        || trimmed.starts_with("yalign ")
        || trimmed.starts_with("xsize ")
        || trimmed.starts_with("ysize ")
        || trimmed.starts_with("text_align ")
        || trimmed.starts_with("action ")
        || trimmed.starts_with("hovered ")
        || trimmed.starts_with("unhovered ")
        || trimmed.starts_with("background ")
        // Screen/style property keywords (common false positive sources)
        || trimmed.starts_with("style_prefix ")
        || trimmed.starts_with("variant ")
        || trimmed.starts_with("scrollbars ")
        || trimmed.starts_with("layout ")
        || trimmed.starts_with("size_group ")
        || trimmed.starts_with("tag ")
        || trimmed.starts_with("key ")
        || trimmed.starts_with("id ")
        || trimmed.starts_with("foreground ")
        || trimmed.starts_with("side ")
        || trimmed.starts_with("child ")
        || trimmed.starts_with("has ")
        || trimmed.starts_with("focus_mask ")
        || trimmed.starts_with("alt ")
        || trimmed.starts_with("group ")
        || trimmed.starts_with("prefix ")
        || trimmed.starts_with("suffix ")
        || trimmed.starts_with("clicked ")
        || trimmed.starts_with("released ")
        || trimmed.starts_with("activate_sound ")
        || trimmed.starts_with("hover_sound ")
        || trimmed.starts_with("sensitive ")
        || trimmed.starts_with("selected ")
        || trimmed.starts_with("tooltip ")
        // Handled by dedicated extractors
        || trimmed.starts_with("text ")
        || trimmed.starts_with("textbutton ")
        || trimmed.starts_with("centered ")
    {
        return None;
    }

    // Narrator: just "text"
    if trimmed.starts_with('"') {
        let (text, _) = extract_quoted_string(trimmed)?;
        if !text.is_empty() && !is_file_reference(text) {
            return Some((None, text));
        }
        return None;
    }

    // Character say: `identifier "text"`, `identifier expression "text"`,
    // `identifier expression_num "text"`, or `identifier"text"` (no space)
    // Find the first quote to locate where dialogue text begins
    if let Some(quote_pos) = trimmed.find('"') {
        if quote_pos > 0 {
            let before_quote = trimmed[..quote_pos].trim_end();
            // Split the part before the quote into words
            let words: Vec<&str> = before_quote.split_whitespace().collect();
            if !words.is_empty() {
                let character = words[0];
                // Character must be a valid identifier and not a keyword
                if character.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
                    && !is_renpy_keyword(character)
                {
                    // All words between character and quote must be identifiers/numbers (expression tags)
                    let valid_middle = words[1..].iter().all(|w| {
                        w.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
                    });
                    if valid_middle {
                        let rest = &trimmed[quote_pos..];
                        if let Some((text, _)) = extract_quoted_string(rest) {
                            if !text.is_empty() && !is_file_reference(text) {
                                return Some((Some(character), text));
                            }
                        }
                    }
                }
            }
        }
    }

    None
}

/// Check if a line is a dialogue line (say statement or menu choice) that should be translated.
/// Returns true ONLY for lines like:
///   - `character "dialogue text"` (say statement)
///   - `"narrator text"` (narrator say)
///   - `"menu choice":` (menu choice)
/// Returns false for everything else (code, screens, defines, labels, etc.)
fn is_dialogue_line(trimmed: &str) -> bool {
    if trimmed.is_empty() || trimmed.starts_with('#') {
        return false;
    }

    // Menu choice: starts with " and ends with ":  or just ":"
    if trimmed.starts_with('"') && (trimmed.ends_with("\":") || trimmed.ends_with("\":")){
        return true;
    }

    // Narrator say: line is just "text" (possibly with line continuation)
    if trimmed.starts_with('"') && !trimmed.contains('(') && !trimmed.contains("action") {
        // But not if it's a textbutton, text, or other UI element
        return true;
    }

    // Character say: `identifier "text"`, `identifier expression "text"`, or `identifier"text"`
    if let Some(quote_pos) = trimmed.find('"') {
        if quote_pos > 0 {
            let before_quote = trimmed[..quote_pos].trim_end();
            let words: Vec<&str> = before_quote.split_whitespace().collect();
            if !words.is_empty() {
                let first = words[0];
                if first.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
                    && !is_renpy_keyword(first)
                {
                    let valid_middle = words[1..].iter().all(|w| {
                        w.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
                    });
                    if valid_middle {
                        return true;
                    }
                }
            }
        }
    }

    // centered "text"
    if trimmed.starts_with("centered ") && trimmed.contains('"') {
        return true;
    }

    false
}

fn is_renpy_keyword(word: &str) -> bool {
    matches!(word,
        "screen" | "style" | "transform" | "define" | "default" | "init" | "label" |
        "image" | "python" | "if" | "elif" | "else" | "while" | "for" | "return" |
        "jump" | "call" | "pass" | "menu" | "scene" | "show" | "hide" | "with" |
        "play" | "stop" | "pause" | "use" | "has" | "at" | "frame" | "vbox" | "hbox" |
        "grid" | "text" | "textbutton" | "add" | "window" | "null" | "timer" |
        "input" | "key" | "on" | "action" | "bar" | "viewport" | "imagemap" |
        "hotspot" | "hotbar" | "button" | "fixed" | "side" | "drag" | "draggroup" |
        "translate" | "class" | "import" | "from" | "as" | "in" | "not" | "and" | "or" |
        "id" | "layout" | "xalign" | "yalign" | "xpos" | "ypos" | "xsize" | "ysize" |
        "xoffset" | "yoffset" | "xanchor" | "yanchor" | "pos" | "anchor" | "align" |
        "area" | "size" | "xysize" | "idle" | "hover" | "insensitive" | "selected_idle" |
        "selected_hover" | "ground" | "background" | "foreground" | "child" |
        "font" | "color" | "outlines" | "kerning" | "spacing" | "first_indent" |
        "rest_indent" | "prefix" | "suffix" | "alt" | "tooltip" | "focus" |
        "selected" | "sensitive" | "keysym" | "alternate" | "hovered" | "unhovered" |
        "clicked" | "released" | "activate_sound" | "hover_sound"
    )
}

/// Escape unescaped double quotes inside a translation string.
/// Turns `"word"` into `\"word\"` but leaves already-escaped `\"` alone.
fn escape_inner_quotes(s: &str) -> String {
    let mut result = String::with_capacity(s.len() + 8);
    let mut chars = s.chars().peekable();
    let mut prev_was_backslash = false;
    while let Some(ch) = chars.next() {
        if ch == '"' {
            if prev_was_backslash {
                // Already escaped, just push the quote
                result.push('"');
            } else {
                result.push('\\');
                result.push('"');
            }
            prev_was_backslash = false;
        } else {
            prev_was_backslash = ch == '\\';
            result.push(ch);
        }
    }
    result
}

/// Check if a string looks like a file path/reference (not translatable text)
fn is_file_reference(text: &str) -> bool {
    let t = text.trim();
    // File extensions
    if t.ends_with(".png") || t.ends_with(".jpg") || t.ends_with(".jpeg") || t.ends_with(".webp") ||
       t.ends_with(".gif") || t.ends_with(".svg") || t.ends_with(".bmp") ||
       t.ends_with(".mp3") || t.ends_with(".ogg") || t.ends_with(".wav") || t.ends_with(".flac") ||
       t.ends_with(".mp4") || t.ends_with(".webm") || t.ends_with(".avi") || t.ends_with(".ogv") ||
       t.ends_with(".ttf") || t.ends_with(".otf") || t.ends_with(".woff") ||
       t.ends_with(".rpy") || t.ends_with(".rpyc") || t.ends_with(".rpa") ||
       t.ends_with(".json") || t.ends_with(".txt") || t.ends_with(".xml") || t.ends_with(".csv") {
        return true;
    }
    // Path-like patterns
    if (t.contains('/') || t.contains('\\')) && !t.contains(' ') {
        return true;
    }
    // Color hex codes
    if t.starts_with('#') && t.len() <= 9 && t[1..].chars().all(|c| c.is_ascii_hexdigit()) {
        return true;
    }
    false
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
    // Skip Character() definitions — handled separately
    if trimmed.contains("Character(") {
        return None;
    }
    // Skip non-translatable define patterns
    let before_eq = &trimmed[7..trimmed.find('=')?];
    let var_name = before_eq.trim();
    if var_name.starts_with("config.version")
        || var_name.starts_with("config.save_directory")
        || var_name.starts_with("config.window_title")
        || var_name.starts_with("config.window")
        || var_name.starts_with("config.screen_width")
        || var_name.starts_with("config.screen_height")
        || var_name.starts_with("config.name") && !var_name.contains("_(")
        || var_name.starts_with("config.language")
        || var_name.starts_with("config.layer")
        || var_name.starts_with("build.")
        || var_name.starts_with("bubble.")
        || is_gui_non_translatable(var_name)
    {
        return None;
    }
    let eq_pos = trimmed.find('=')?;
    let after_eq = trimmed[eq_pos + 1..].trim();
    let (text, _) = extract_quoted_string(after_eq)?;
    if !text.is_empty() && !is_file_reference(text) {
        // Skip pure numeric/version strings
        if text.chars().all(|c| c.is_ascii_digit() || c == '.') {
            return None;
        }
        Some(text)
    } else {
        None
    }
}

fn extract_underscore_call(line: &str) -> Option<&str> {
    let trimmed = line.trim();
    // Match _("text") or __("text") but not _p("text")
    let start = trimmed.find("_(\"").or_else(|| trimmed.find("__(\""))?;
    let paren_pos = trimmed[start..].find("(\"")? + start;
    let inner = &trimmed[paren_pos + 1..]; // after `(`
    let (text, _) = extract_quoted_string(inner)?;
    if !text.is_empty() {
        Some(text)
    } else {
        None
    }
}

/// Extract _p("""multi-line text""") — Ren'Py multi-paragraph translatable text
fn extract_p_call(line: &str) -> Option<&str> {
    let trimmed = line.trim();
    let start = trimmed.find("_p(\"\"\"")?;
    let inner = &trimmed[start + 6..]; // after `_p("""`
    let end = inner.find("\"\"\")")?;
    let text = &inner[..end];
    if !text.trim().is_empty() {
        Some(text)
    } else {
        None
    }
}

/// Extract character name from Character("Name", ...) definitions
fn extract_character_name(line: &str) -> Option<&str> {
    let trimmed = line.trim();
    // Must be a define or $ assignment with Character(...)
    if !trimmed.starts_with("define ") && !trimmed.starts_with("$ ") {
        return None;
    }
    // Find Character( call
    let char_pos = trimmed.find("Character(")?;
    let after = &trimmed[char_pos + 10..]; // after `Character(`
    let after_trimmed = after.trim();
    // Skip Character(None, ...) and Character(_("..."), ...) (already handled by _() extractor)
    if after_trimmed.starts_with("None") || after_trimmed.starts_with("_(") {
        return None;
    }
    // Extract the quoted name
    if let Some((name, _)) = extract_quoted_string(after_trimmed) {
        // Skip empty names and pure variable references like "[name]"
        if name.is_empty() {
            return None;
        }
        // Pure variable reference: skip (e.g., "[name]" or "[l]")
        if name.starts_with('[') && name.ends_with(']') && !name.contains(' ') {
            return None;
        }
        return Some(name);
    }
    None
}

/// Extract text from renpy.notify("text") or renpy.input("text") calls
fn extract_renpy_call<'a>(line: &'a str, prefix: &str) -> Option<&'a str> {
    let trimmed = line.trim();
    let start = trimmed.find(prefix)?;
    let after = &trimmed[start + prefix.len()..];
    // The argument might start with _(" for translated calls — skip those (handled by _() extractor)
    let after_trimmed = after.trim();
    if after_trimmed.starts_with("_(") {
        return None;
    }
    let (text, _) = extract_quoted_string(after_trimmed)?;
    if !text.is_empty() && !is_file_reference(text) {
        Some(text)
    } else {
        None
    }
}

/// Extract translatable text from screen UI elements:
/// text "string", textbutton "string", tooltip "string"
fn extract_screen_text(line: &str) -> Option<&str> {
    let trimmed = line.trim();

    // Match: text "string", textbutton "string", tooltip "string", tooltip ("string")
    let prefixes = &[
        "text ", "textbutton ", "tooltip ",
    ];

    for &prefix in prefixes {
        if !trimmed.starts_with(prefix) {
            continue;
        }
        let rest = trimmed[prefix.len()..].trim();

        // Skip if already uses _() — handled by underscore call extractor
        if rest.starts_with("_(") || rest.starts_with("__(") {
            return None;
        }
        // Skip variable references (no quote)
        if !rest.starts_with('"') && !rest.starts_with("(\"") {
            return None;
        }
        // Handle tooltip ("string") with parens
        let rest = if rest.starts_with("(\"") {
            &rest[1..]
        } else {
            rest
        };
        let (text, _) = extract_quoted_string(rest)?;
        if text.is_empty() || is_file_reference(text) {
            return None;
        }
        // Skip very short non-word strings that are likely identifiers
        // e.g., text "window" as a style reference
        if text.len() <= 2 && !text.contains(|c: char| c.is_whitespace()) {
            return None;
        }
        return Some(text);
    }
    None
}

/// Check if a gui.xxx variable is non-translatable (colors, sizes, fonts, layout values).
fn is_gui_non_translatable(var: &str) -> bool {
    if !var.starts_with("gui.") {
        return false;
    }
    let prop = &var[4..];
    // Explicit non-translatable system values
    if prop == "language" || prop == "unscrollable" || prop == "rollback_side"
        || prop == "history_allow_tags"
    {
        return true;
    }
    // Skip color, size, font, border, padding, spacing, position properties
    prop.contains("color") || prop.contains("size") || prop.contains("font")
        || prop.contains("border") || prop.contains("padding") || prop.contains("spacing")
        || prop.contains("height") || prop.contains("width") || prop.contains("align")
        || prop.contains("offset") || prop.contains("xpos") || prop.contains("ypos")
        || prop.contains("tile") || prop.contains("opacity") || prop.contains("outlines")
        || prop.contains("background") || prop.contains("icon")
        || prop.ends_with("_idle") || prop.ends_with("_hover") || prop.ends_with("_insensitive")
        || prop.starts_with("show_") || prop.starts_with("button_")
        || prop.starts_with("choice_") || prop.starts_with("navigation_")
        || prop.starts_with("slot_") || prop.starts_with("namebox_")
}

/// Simplified Python pickle parser for RPA index data.
/// The pickle contains a dict mapping filenames (str) to lists of (offset, length, prefix) tuples.
/// We only need to extract the filename, offset, and length.
fn parse_rpa_pickle(data: &[u8], key: i64) -> Result<Vec<(String, u64, usize)>> {
    let mut result = Vec::new();
    let mut pos = 0;
    let len = data.len();

    // Python 2 pickle protocol 2 tokens we care about:
    // \x80\x02 = proto 2
    // } = EMPTY_DICT
    // q/r = SHORT_BINPUT/LONG_BINPUT (memo)
    // X = SHORT_BINUNICODE (4-byte len + utf8)
    // ] = EMPTY_LIST
    // ( = MARK
    // J = BININT (4 bytes little-endian signed)
    // K = BININT1 (1 byte unsigned)
    // M = BININT2 (2 bytes unsigned)
    // \x8a = LONG1 (1-byte length + n bytes little-endian)
    // t = TUPLE
    // a = APPEND
    // e = APPENDS
    // u = SETITEMS
    // s = SETITEM
    // . = STOP

    let mut stack: Vec<PickleVal> = Vec::new();
    let mut mark_stack: Vec<usize> = Vec::new();
    let mut memo: Vec<PickleVal> = Vec::new();
    let mut current_key: Option<String> = None;

    while pos < len {
        let op = data[pos];
        pos += 1;
        match op {
            0x80 => { pos += 1; } // PROTO
            0x95 => { pos += 8; } // FRAME (protocol 4+) — skip 8-byte frame length
            0x94 => { // MEMOIZE (protocol 4+) — store stack top in memo
                if let Some(top) = stack.last() {
                    memo.push(top.clone());
                }
            }
            0x7d => stack.push(PickleVal::Dict), // EMPTY_DICT
            0x5d => stack.push(PickleVal::List(Vec::new())), // EMPTY_LIST
            0x28 => mark_stack.push(stack.len()), // MARK
            0x71 => { // SHORT_BINPUT (memo)
                if pos >= len { break; }
                let idx = data[pos] as usize;
                pos += 1;
                if let Some(top) = stack.last() {
                    while memo.len() <= idx { memo.push(PickleVal::None); }
                    memo[idx] = top.clone();
                }
            }
            0x72 => { // LONG_BINPUT
                if pos + 4 > len { break; }
                let idx = u32::from_le_bytes([data[pos], data[pos+1], data[pos+2], data[pos+3]]) as usize;
                pos += 4;
                if let Some(top) = stack.last() {
                    while memo.len() <= idx { memo.push(PickleVal::None); }
                    memo[idx] = top.clone();
                }
            }
            0x68 => { // SHORT_BINGET
                if pos >= len { break; }
                let idx = data[pos] as usize;
                pos += 1;
                let val = memo.get(idx).cloned().unwrap_or(PickleVal::None);
                stack.push(val);
            }
            0x6a => { // LONG_BINGET
                if pos + 4 > len { break; }
                let idx = u32::from_le_bytes([data[pos], data[pos+1], data[pos+2], data[pos+3]]) as usize;
                pos += 4;
                let val = memo.get(idx).cloned().unwrap_or(PickleVal::None);
                stack.push(val);
            }
            0x43 => { // SHORT_BINBYTES
                if pos >= len { break; }
                let slen = data[pos] as usize;
                pos += 1;
                if pos + slen > len { break; }
                let s = String::from_utf8_lossy(&data[pos..pos+slen]).to_string();
                pos += slen;
                stack.push(PickleVal::Str(s));
            }
            0x44 => { // BINBYTES (4-byte len)
                if pos + 4 > len { break; }
                let slen = u32::from_le_bytes([data[pos], data[pos+1], data[pos+2], data[pos+3]]) as usize;
                pos += 4;
                if pos + slen > len { break; }
                let s = String::from_utf8_lossy(&data[pos..pos+slen]).to_string();
                pos += slen;
                stack.push(PickleVal::Str(s));
            }
            0x8e => { // BINBYTES8 (8-byte len, protocol 4+)
                if pos + 8 > len { break; }
                let slen = u64::from_le_bytes([data[pos], data[pos+1], data[pos+2], data[pos+3], data[pos+4], data[pos+5], data[pos+6], data[pos+7]]) as usize;
                pos += 8;
                if pos + slen > len { break; }
                let s = String::from_utf8_lossy(&data[pos..pos+slen]).to_string();
                pos += slen;
                stack.push(PickleVal::Str(s));
            }
            0x8c => { // SHORT_BINUNICODE (protocol 4+) — 1-byte length
                if pos >= len { break; }
                let slen = data[pos] as usize;
                pos += 1;
                if pos + slen > len { break; }
                let s = String::from_utf8_lossy(&data[pos..pos+slen]).to_string();
                pos += slen;
                stack.push(PickleVal::Str(s));
            }
            0x55 => { // SHORT_BINSTRING
                if pos >= len { break; }
                let slen = data[pos] as usize;
                pos += 1;
                if pos + slen > len { break; }
                let s = String::from_utf8_lossy(&data[pos..pos+slen]).to_string();
                pos += slen;
                stack.push(PickleVal::Str(s));
            }
            0x54 => { // BINSTRING (4-byte len)
                if pos + 4 > len { break; }
                let slen = u32::from_le_bytes([data[pos], data[pos+1], data[pos+2], data[pos+3]]) as usize;
                pos += 4;
                if pos + slen > len { break; }
                let s = String::from_utf8_lossy(&data[pos..pos+slen]).to_string();
                pos += slen;
                stack.push(PickleVal::Str(s));
            }
            0x4a => { // BININT
                if pos + 4 > len { break; }
                let v = i32::from_le_bytes([data[pos], data[pos+1], data[pos+2], data[pos+3]]) as i64;
                pos += 4;
                stack.push(PickleVal::Int(v));
            }
            0x4b => { // BININT1
                if pos >= len { break; }
                stack.push(PickleVal::Int(data[pos] as i64));
                pos += 1;
            }
            0x4d => { // BININT2
                if pos + 2 > len { break; }
                let v = u16::from_le_bytes([data[pos], data[pos+1]]) as i64;
                pos += 2;
                stack.push(PickleVal::Int(v));
            }
            0x8a => { // LONG1
                if pos >= len { break; }
                let nbytes = data[pos] as usize;
                pos += 1;
                if pos + nbytes > len { break; }
                let mut v: i64 = 0;
                for i in 0..nbytes.min(8) {
                    v |= (data[pos + i] as i64) << (i * 8);
                }
                pos += nbytes;
                stack.push(PickleVal::Int(v));
            }
            0x74 => { // TUPLE
                let mark = mark_stack.pop().unwrap_or(0).min(stack.len());
                let items: Vec<PickleVal> = stack.drain(mark..).collect();
                stack.push(PickleVal::Tuple(items));
            }
            0x85 => { // TUPLE1
                let v = stack.pop().unwrap_or(PickleVal::None);
                stack.push(PickleVal::Tuple(vec![v]));
            }
            0x86 => { // TUPLE2
                let b = stack.pop().unwrap_or(PickleVal::None);
                let a = stack.pop().unwrap_or(PickleVal::None);
                stack.push(PickleVal::Tuple(vec![a, b]));
            }
            0x87 => { // TUPLE3
                let c = stack.pop().unwrap_or(PickleVal::None);
                let b = stack.pop().unwrap_or(PickleVal::None);
                let a = stack.pop().unwrap_or(PickleVal::None);
                stack.push(PickleVal::Tuple(vec![a, b, c]));
            }
            0x61 => { // APPEND
                let val = stack.pop().unwrap_or(PickleVal::None);
                if let Some(PickleVal::List(ref mut list)) = stack.last_mut() {
                    list.push(val);
                }
            }
            0x65 => { // APPENDS
                let mark = mark_stack.pop().unwrap_or(stack.len()).min(stack.len());
                let items: Vec<PickleVal> = stack.drain(mark..).collect();
                if let Some(PickleVal::List(ref mut list)) = stack.last_mut() {
                    list.extend(items);
                }
            }
            0x73 => { // SETITEM
                let val = stack.pop().unwrap_or(PickleVal::None);
                let k = stack.pop().unwrap_or(PickleVal::None);
                if let PickleVal::Str(ref name) = k {
                    current_key = Some(name.clone());
                }
                // Process: key should be a string (filename), val should be a list of tuples
                if let (Some(ref filename), PickleVal::List(ref items)) = (&current_key, &val) {
                    for item in items {
                        if let PickleVal::Tuple(ref t) = item {
                            if t.len() >= 2 {
                                let offset = t[0].as_int().unwrap_or(0) ^ key;
                                let length = t[1].as_int().unwrap_or(0) ^ key;
                                let prefix_len = if t.len() >= 3 {
                                    if let PickleVal::Str(ref s) = t[2] { s.len() } else { 0 }
                                } else { 0 };
                                result.push((
                                    filename.clone(),
                                    (offset as u64) + prefix_len as u64,
                                    (length as usize).saturating_sub(prefix_len),
                                ));
                            }
                        }
                    }
                    current_key = None;
                }
            }
            0x75 => { // SETITEMS
                let mark = mark_stack.pop().unwrap_or(0).min(stack.len());
                let items: Vec<PickleVal> = stack.drain(mark..).collect();
                // Items come in pairs: key, val, key, val, ...
                let mut i = 0;
                while i + 1 < items.len() {
                    let k = &items[i];
                    let v = &items[i + 1];
                    if let PickleVal::Str(ref filename) = k {
                        if let PickleVal::List(ref entries) = v {
                            for entry in entries {
                                if let PickleVal::Tuple(ref t) = entry {
                                    if t.len() >= 2 {
                                        let offset = t[0].as_int().unwrap_or(0) ^ key;
                                        let length = t[1].as_int().unwrap_or(0) ^ key;
                                        let prefix_len = if t.len() >= 3 {
                                            if let PickleVal::Str(ref s) = t[2] { s.len() } else { 0 }
                                        } else { 0 };
                                        result.push((
                                            filename.clone(),
                                            (offset as u64) + prefix_len as u64,
                                            (length as usize).saturating_sub(prefix_len),
                                        ));
                                    }
                                }
                            }
                        }
                    }
                    i += 2;
                }
            }
            0x4e => stack.push(PickleVal::None), // NONE
            0x88 => stack.push(PickleVal::Int(1)), // NEWTRUE
            0x89 => stack.push(PickleVal::Int(0)), // NEWFALSE
            0x2e => break, // STOP
            _ => {} // Skip unknown opcodes
        }
    }

    Ok(result)
}

#[derive(Debug, Clone)]
enum PickleVal {
    None,
    Int(i64),
    Str(String),
    List(Vec<PickleVal>),
    Tuple(Vec<PickleVal>),
    Dict,
}

impl PickleVal {
    fn as_int(&self) -> Option<i64> {
        match self {
            PickleVal::Int(v) => Some(*v),
            _ => None,
        }
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
            let ext = path.extension().unwrap_or_default();
            return ext == "rpy" || ext == "rpa";
        }
        if path.is_dir() {
            if let Some(game_dir) = Self::find_game_dir(path) {
                return Self::has_rpy_files(&game_dir) || Self::has_rpa_files(&game_dir);
            }
        }
        false
    }

    fn extract(&self, path: &Path) -> Result<Vec<StringEntry>> {
        if path.is_file() {
            if path.extension().map_or(false, |e| e == "rpa") {
                return self.extract_rpa_archive(path);
            }
            return Self::extract_file(path);
        }

        let game_dir = Self::find_game_dir(path).ok_or_else(|| {
            locust_core::error::LocustError::ParseError {
                file: path.display().to_string(),
                message: "could not find game/ directory".to_string(),
            }
        })?;

        // First try .rpy files directly
        let mut all = Vec::new();
        let mut found_rpy = false;
        for entry in walkdir::WalkDir::new(&game_dir)
            .follow_links(false)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            let fpath = entry.path();
            if fpath.extension().map_or(false, |e| e == "rpy") {
                // Skip tl/ directory and renpy/ engine dir
                if let Ok(rel) = fpath.strip_prefix(&game_dir) {
                    if rel.starts_with("tl") {
                        continue;
                    }
                }
                // Skip the renpy engine directory
                if let Some(parent_root) = game_dir.parent() {
                    if fpath.starts_with(parent_root.join("renpy")) {
                        continue;
                    }
                }
                found_rpy = true;
                match Self::extract_file(fpath) {
                    Ok(entries) => all.extend(entries),
                    Err(e) => {
                        tracing::warn!("Failed to extract {}: {}", fpath.display(), e);
                    }
                }
            }
        }

        // If no .rpy files found, try extracting from .rpa archives
        if !found_rpy {
            for entry in std::fs::read_dir(&game_dir)?.filter_map(|e| e.ok()) {
                let fpath = entry.path();
                if fpath.extension().map_or(false, |e| e == "rpa") {
                    if fpath.file_name().map_or(false, |n| {
                        let name = n.to_string_lossy();
                        name.contains("script") || name == "archive.rpa"
                    }) {
                        match self.extract_rpa_archive(&fpath) {
                            Ok(entries) => all.extend(entries),
                            Err(e) => {
                                tracing::warn!("Failed to extract RPA {}: {}", fpath.display(), e);
                            }
                        }
                    }
                }
            }

            // If still nothing found from named archives, try all .rpa files
            if all.is_empty() {
                for entry in std::fs::read_dir(&game_dir)?.filter_map(|e| e.ok()) {
                    let fpath = entry.path();
                    if fpath.extension().map_or(false, |e| e == "rpa") {
                        match self.extract_rpa_archive(&fpath) {
                            Ok(entries) => all.extend(entries),
                            Err(e) => {
                                tracing::warn!("Failed to extract RPA {}: {}", fpath.display(), e);
                            }
                        }
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

        // Check if entries come from an RPA archive
        let from_rpa = entries.iter().any(|e| {
            e.file_path.extension().map_or(false, |ext| ext == "rpa")
        });

        if from_rpa {
            // For RPA-sourced entries: extract .rpy files from archive, apply translations,
            // then place translated .rpy files in game/ dir where Ren'Py loads them with priority.
            return self.inject_replace_rpa(path, entries);
        }

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
                        let safe_trans = escape_inner_quotes(translation);
                        let new_line = line.replacen(
                            &format!("\"{}\"", source),
                            &format!("\"{}\"", safe_trans),
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

            // Delete corresponding .rpyc so Ren'Py recompiles from the modified .rpy
            let rpyc_path = file_path.with_extension("rpyc");
            if rpyc_path.exists() {
                let _ = std::fs::remove_file(&rpyc_path);
            }
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

        // Also do a direct inject on .rpy files — this is the most reliable way to get
        // dialogue translations to actually appear in the game. The `translate strings:`
        // block works for UI/menu text but isn't always reliable for character dialogue.
        let _ = self.inject(path, entries);

        let tl_dir = game_dir.join("tl").join(lang);
        std::fs::create_dir_all(&tl_dir)?;

        let mut strings_written = 0;
        let mut strings_skipped = 0;

        // Ren'Py translate strings block: works for all string types (say, menu, define, etc.)
        // This is the most reliable translation method as it doesn't depend on internal hash IDs.
        // Format:
        //   translate <lang> strings:
        //       old "source text"
        //       new "translated text"
        //
        // IMPORTANT: Ren'Py throws an exception on duplicate `old` entries,
        // so we deduplicate by source text (first translation wins).
        use std::collections::HashSet;
        let mut seen_sources: HashSet<String> = HashSet::new();
        let mut string_pairs: Vec<(String, String)> = Vec::new();

        for entry in entries {
            if let Some(ref translation) = entry.translation {
                if translation != &entry.source {
                    if seen_sources.insert(entry.source.clone()) {
                        string_pairs.push((entry.source.clone(), translation.clone()));
                        strings_written += 1;
                    } else {
                        strings_skipped += 1;
                    }
                } else {
                    strings_skipped += 1;
                }
            } else {
                strings_skipped += 1;
            }
        }

        if !string_pairs.is_empty() {
            let mut lines = Vec::new();
            lines.push(format!("translate {} strings:", lang));
            lines.push(String::new());

            for (source, translation) in &string_pairs {
                // Escape quotes in strings
                let escaped_source = source.replace('\\', "\\\\").replace('"', "\\\"");
                let escaped_translation = translation.replace('\\', "\\\\").replace('"', "\\\"");
                lines.push(format!("    old \"{}\"", escaped_source));
                lines.push(format!("    new \"{}\"", escaped_translation));
                lines.push(String::new());
            }

            let tl_file = tl_dir.join("locust_strings.rpy");
            std::fs::write(&tl_file, lines.join("\n"))?;

            // Delete the .rpyc if it exists so Ren'Py recompiles with the new translations
            let tl_rpyc = tl_dir.join("locust_strings.rpyc");
            if tl_rpyc.exists() {
                let _ = std::fs::remove_file(&tl_rpyc);
            }

            // Create locust_languages.rpy with an in-game language picker.
            // Scans the tl/ folder for available languages and adds a selector button
            // to the main menu and game menu (preferences).
            let langs_file_content = build_language_picker_script(&game_dir, lang);
            let langs_file = game_dir.join("locust_languages.rpy");
            std::fs::write(&langs_file, langs_file_content)?;
            // Delete .rpyc so Ren'Py picks up the new .rpy
            let langs_rpyc = game_dir.join("locust_languages.rpyc");
            if langs_rpyc.exists() {
                let _ = std::fs::remove_file(&langs_rpyc);
            }

            // Remove old locust_language.rpy from previous versions if it exists
            let old_lang_file = game_dir.join("locust_language.rpy");
            if old_lang_file.exists() {
                let _ = std::fs::remove_file(&old_lang_file);
            }
            let old_lang_rpyc = game_dir.join("locust_language.rpyc");
            if old_lang_rpyc.exists() {
                let _ = std::fs::remove_file(&old_lang_rpyc);
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

/// Build a Ren'Py script that adds an in-game language picker.
/// Scans game/tl/ for available language folders and creates a picker screen
/// accessible from the main menu and game menu (preferences).
fn build_language_picker_script(game_dir: &Path, just_added_lang: &str) -> String {
    // Scan tl/ for available languages
    let tl_dir = game_dir.join("tl");
    let mut langs: Vec<String> = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&tl_dir) {
        for entry in entries.flatten() {
            if let Ok(meta) = entry.metadata() {
                if meta.is_dir() {
                    if let Some(name) = entry.file_name().to_str() {
                        // Skip "None" pseudo-directory and empty entries
                        if !name.is_empty() && name != "None" {
                            langs.push(name.to_string());
                        }
                    }
                }
            }
        }
    }
    // Ensure the just-added language is included (tl/<lang>/ might not be created yet)
    if !langs.iter().any(|l| l == just_added_lang) {
        langs.push(just_added_lang.to_string());
    }
    langs.sort();
    langs.dedup();

    // Human-readable language names
    fn lang_name(code: &str) -> &str {
        match code {
            "es" => "Español",
            "en" => "English",
            "ja" => "日本語",
            "zh-CN" | "zh_CN" | "zhCN" => "简体中文",
            "zh-TW" | "zh_TW" | "zhTW" => "繁體中文",
            "ko" => "한국어",
            "fr" => "Français",
            "de" => "Deutsch",
            "it" => "Italiano",
            "pt" => "Português",
            "pt-BR" | "pt_BR" | "ptBR" => "Português BR",
            "ru" => "Русский",
            "nl" => "Nederlands",
            "pl" => "Polski",
            "tr" => "Türkçe",
            "ar" => "العربية",
            "vi" => "Tiếng Việt",
            "th" => "ไทย",
            "id" => "Bahasa Indonesia",
            other => other,
        }
    }

    let mut buttons = String::new();
    // Original language button (None = use original game language)
    buttons.push_str("                textbutton \"Original\" action Language(None) xalign 0.5 text_size 22\n");
    for code in &langs {
        let name = lang_name(code);
        buttons.push_str(&format!(
            "                textbutton \"{}\" action Language(\"{}\") xalign 0.5 text_size 22\n",
            name.replace('"', "\\\""),
            code.replace('"', "\\\"")
        ));
    }

    format!(
        r##"# Auto-generated by Locust — adds an in-game language picker.
# Players can change language via the floating button on the main menu,
# or from the preferences screen.

screen locust_language_picker():
    modal True
    zorder 200
    frame:
        align (0.5, 0.5)
        background "#000000dd"
        padding (40, 30)
        xmaximum 500
        vbox:
            spacing 10
            text "Language / Idioma" xalign 0.5 size 28 color "#ffffff"
            null height 15
{}            null height 15
            textbutton "Close / Cerrar" action Hide("locust_language_picker") xalign 0.5 text_size 20

screen locust_language_button():
    zorder 150
    textbutton "🌐 Language" action Show("locust_language_picker"):
        xalign 1.0
        yalign 0.0
        xoffset -20
        yoffset 20
        text_size 18
        background "#00000088"
        padding (12, 6)

# Show the language button on the main menu
init python:
    config.after_load_callbacks = getattr(config, "after_load_callbacks", [])

    def _locust_show_lang_button():
        try:
            current = renpy.current_screen()
            name = current.screen_name[0] if current else ""
            if name in ("main_menu", "navigation"):
                if not renpy.get_screen("locust_language_button"):
                    renpy.show_screen("locust_language_button")
            else:
                if renpy.get_screen("locust_language_button"):
                    renpy.hide_screen("locust_language_button")
        except Exception:
            pass

    config.interact_callbacks.append(_locust_show_lang_button)
"##,
        buttons
    )
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
