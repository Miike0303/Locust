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
            let rel_str = file
                .strip_prefix(&temp_dir)
                .map(|r| r.to_string_lossy().replace('\\', "/"))
                .unwrap_or_default();
            if rel_str.starts_with("tl/") {
                continue;
            }

            // Compiled scripts: mine the pickled AST for display strings.
            // These are injected via a runtime text filter, not file edits.
            if file.extension().is_some_and(|e| e == "rpyc") {
                match std::fs::read(file) {
                    Ok(bytes) => {
                        for (n, text) in harvest_rpyc_strings(&bytes).into_iter().enumerate() {
                            let id = format!(
                                "{}#{}#s{}",
                                rpa_path.file_name().unwrap_or_default().to_string_lossy(),
                                rel_str,
                                n
                            );
                            let mut entry =
                                StringEntry::new(id, &text, rpa_path.to_path_buf());
                            entry.tags = vec!["dialogue".to_string(), "rpyc".to_string()];
                            all.push(entry);
                        }
                    }
                    Err(e) => tracing::warn!("Failed to read {}: {}", file.display(), e),
                }
                continue;
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

    /// Apply translations mined from compiled scripts (.rpyc) by generating a
    /// runtime text-filter file in game/. Ren'Py's say_menu_text_filter hook
    /// swaps each displayed line before variable substitution, so nothing in
    /// the compiled scripts needs to change. Deleting the generated file
    /// restores the original language.
    fn inject_rpyc_filter(path: &Path, entries: &[&StringEntry]) -> Result<InjectionReport> {
        let game_dir = if path.is_dir() {
            Self::find_game_dir(path).unwrap_or_else(|| path.join("game"))
        } else {
            path.parent().unwrap_or(path).to_path_buf()
        };

        let mut strings_written = 0usize;
        let mut strings_skipped = 0usize;
        let mut body = String::new();
        for e in entries {
            let Some(t) = e.translation.as_deref().filter(|t| !t.trim().is_empty()) else {
                strings_skipped += 1;
                continue;
            };
            if t == e.source {
                // Intentionally not written (nothing would change at runtime), but
                // still counted so written + skipped reconciles with total entries.
                strings_skipped += 1;
                continue;
            }
            body.push_str(&format!(
                "        \"{}\": \"{}\",\n",
                python_escape(&e.source),
                python_escape(t)
            ));
            strings_written += 1;
        }

        // Nothing to translate: don't write a no-op filter file, since it would
        // still overwrite the game's own say_menu_text_filter hook for no gain.
        // But if a PREVIOUS run left a filter file in place (e.g. translations
        // were since cleared or reverted to source), it must be removed here —
        // otherwise the game keeps applying a now-stale translation map while
        // this report claims nothing happened.
        if strings_written == 0 {
            let rpy_path = game_dir.join("zzz_locust_translate.rpy");
            let rpyc_path = game_dir.join("zzz_locust_translate.rpyc");
            let mut removed_stale = false;
            let mut warnings = Vec::new();
            // Non-fatal: a read-only or editor-locked stale file (common on
            // Windows) must not abort the whole inject before any translation
            // is applied. Matches the `let _ =` idiom used on the success path
            // below for the same removal.
            if rpy_path.exists() {
                match std::fs::remove_file(&rpy_path) {
                    Ok(()) => removed_stale = true,
                    Err(e) => warnings.push(format!(
                        "could not remove stale translation filter {}: {e}",
                        rpy_path.display()
                    )),
                }
            }
            if rpyc_path.exists() {
                match std::fs::remove_file(&rpyc_path) {
                    Ok(()) => removed_stale = true,
                    Err(e) => warnings.push(format!(
                        "could not remove stale compiled filter {}: {e}",
                        rpyc_path.display()
                    )),
                }
            }
            return Ok(InjectionReport {
                files_modified: if removed_stale { 1 } else { 0 },
                strings_written,
                strings_skipped,
                warnings,
            });
        }

        std::fs::create_dir_all(&game_dir)?;

        // Capture and chain to whatever filter the game already installed (e.g.
        // censorship toggles, name substitution, text styling) instead of
        // clobbering it outright. When there is no prior filter this preserves
        // the original lookup-only behavior.
        let file = format!(
            "# Generated by Locust — runtime translation filter.\n\
             # Delete this file to restore the original language.\n\
             init 999 python:\n\
             \x20   locust_translations = {{\n\
             {body}\
             \x20   }}\n\
             \x20   locust_previous_filter = config.say_menu_text_filter\n\
             \x20   def locust_text_filter(text):\n\
             \x20       if locust_previous_filter is not None:\n\
             \x20           text = locust_previous_filter(text)\n\
             \x20       return locust_translations.get(text, text)\n\
             \x20   config.say_menu_text_filter = locust_text_filter\n"
        );
        std::fs::write(game_dir.join("zzz_locust_translate.rpy"), file)?;
        // Remove a stale compiled twin so Ren'Py recompiles our new file
        let _ = std::fs::remove_file(game_dir.join("zzz_locust_translate.rpyc"));

        Ok(InjectionReport {
            files_modified: 1,
            strings_written,
            strings_skipped,
            warnings: Vec::new(),
        })
    }

    /// For RPA-sourced entries: extract .rpy from the archive, apply translations in-place,
    /// and write the translated .rpy files into game/ directory.
    /// Ren'Py loads loose .rpy files with priority over .rpa archives.
    fn inject_replace_rpa(
        &self,
        path: &Path,
        entries: &[StringEntry],
        loose_dest_paths: &std::collections::HashSet<String>,
    ) -> locust_core::error::Result<InjectionReport> {
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
        let result = Self::inject_rpa_inner(&game_dir, &temp_dir, &rpa_files, entries, loose_dest_paths);

        // Always cleanup temp dir, even on error
        let _ = std::fs::remove_dir_all(&temp_dir);

        result
    }

    fn inject_rpa_inner(
        game_dir: &Path,
        temp_dir: &Path,
        rpa_files: &std::collections::HashSet<PathBuf>,
        entries: &[StringEntry],
        loose_dest_paths: &std::collections::HashSet<String>,
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
        let mut collision_skipped = 0usize;

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
            // Count of lines matched in THIS file. Not added to strings_written
            // until we know the write destination doesn't collide with a loose
            // file below — the collision check happens at the actual write site
            // because only there is the full destination path (with subdirectory)
            // known; the entry id alone only carries the bare basename.
            let mut local_matched = 0usize;

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
                            local_matched += 1;
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

                // Destination-collision guard: Ren'Py always loads a loose
                // game/<rel>.rpy with priority over an archive member written to
                // that same destination — the archive copy would never actually
                // be read at runtime. Compare full, normalized destination paths
                // (not just basenames) so a same-named file in a DIFFERENT
                // subdirectory is never mistaken for a collision.
                // `dest.exists()` matters: a loose file recorded at extraction
                // time may since have been deleted (a removed mod), and then the
                // destination is genuinely free — blocking the write would drop
                // the archive translation for nothing.
                let dest_key = normalize_path_for_compare(&dest);
                if loose_dest_paths.contains(&dest_key) && dest.exists() {
                    collision_skipped += local_matched;
                    continue;
                }

                if let Some(parent) = dest.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::write(&dest, &new_content)?;
                files_modified += 1;
                strings_written += local_matched;

                // Delete corresponding .rpyc so Ren'Py recompiles from the modified .rpy
                let rpyc_path = dest.with_extension("rpyc");
                if rpyc_path.exists() {
                    let _ = std::fs::remove_file(&rpyc_path);
                }
            }
        }

        let mut warnings = Vec::new();
        if collision_skipped > 0 {
            warnings.push(format!(
                "{collision_skipped} archive-sourced translation(s) skipped: their destination \
                 path collides with an existing loose .rpy file, which Ren'Py always loads with \
                 priority. Applying the archive translation there would have overwritten the \
                 loose file; only the loose file's own translations were applied."
            ));
        }

        Ok(InjectionReport {
            files_modified,
            strings_written,
            strings_skipped: entries.len().saturating_sub(strings_written),
            warnings,
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
            // Only extract script files
            if !name.ends_with(".rpy") && !name.ends_with(".rpyc") {
                continue;
            }
            // Prefer .rpy source over .rpyc — if both exist, use the source.
            // Lone .rpyc files (how shipped games are packed) are extracted too
            // and mined for strings via the pickle harvester.
            if name.ends_with(".rpyc") {
                let rpy_name = name.strip_suffix("c").unwrap();
                if index.iter().any(|(n, _, _)| n == rpy_name) {
                    continue;
                }
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
        // Track the current label — needed to generate Ren'Py translation identifiers
        // in the format `<label>_<hash>`.
        let mut current_label: Option<String> = None;

        for (line_idx, line) in content.lines().enumerate() {
            let line_num = line_idx + 1;
            let trimmed = line.trim();

            // Track label definitions: `label start:`, `label foo(arg):`
            if trimmed.starts_with("label ") && trimmed.ends_with(':') {
                let after_label = trimmed[6..trimmed.len() - 1].trim();
                // Strip parameters: `label foo(x):` → `foo`
                let name = after_label.split('(').next().unwrap_or(after_label).trim();
                if !name.is_empty() {
                    current_label = Some(name.to_string());
                }
            }

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
                        // Store label in metadata for proper Ren'Py translation block generation
                        if let Some(ref lbl) = current_label {
                            entry.metadata.insert(
                                "label".to_string(),
                                serde_json::Value::String(lbl.clone()),
                            );
                        }
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
                    // Store label in metadata for proper Ren'Py translation block generation
                    if let Some(ref lbl) = current_label {
                        entry.metadata.insert(
                            "label".to_string(),
                            serde_json::Value::String(lbl.clone()),
                        );
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
/// Mine display strings out of a compiled Ren'Py script (.rpyc).
///
/// Layout: "RENPY RPC2" magic + slot table; slot 1 is a zlib-compressed
/// Python pickle of the script AST. We don't rebuild the AST — we walk the
/// pickle opcode stream and harvest unicode strings that look like dialogue
/// or menu text. Injection happens through a runtime text filter, so any
/// code-expression strings that slip through simply never match on screen.
fn harvest_rpyc_strings(bytes: &[u8]) -> Vec<String> {
    const MAGIC: &[u8] = b"RENPY RPC2";
    if bytes.len() < MAGIC.len() + 12 || &bytes[..MAGIC.len()] != MAGIC {
        return Vec::new();
    }

    // Slot table: (u32 slot, u32 offset, u32 length) LE triplets, 0-terminated
    let mut i = MAGIC.len();
    let mut slot1: Option<(usize, usize)> = None;
    while i + 12 <= bytes.len() {
        let slot = u32::from_le_bytes(bytes[i..i + 4].try_into().unwrap());
        let start = u32::from_le_bytes(bytes[i + 4..i + 8].try_into().unwrap()) as usize;
        let len = u32::from_le_bytes(bytes[i + 8..i + 12].try_into().unwrap()) as usize;
        i += 12;
        if slot == 0 {
            break;
        }
        if slot == 1 {
            slot1 = Some((start, len));
        }
    }
    let Some((start, len)) = slot1 else { return Vec::new() };
    if start + len > bytes.len() {
        return Vec::new();
    }

    // ponytail: bounded single-shot inflate, ceiling 64 MiB of decompressed pickle.
    // A real rpyc's string table never approaches this; an untrusted/downloaded
    // .rpyc claiming a much larger payload is treated as a decompression bomb and
    // skipped rather than allowed to force an unbounded allocation. Upgrade path:
    // stream through `flate2` if legitimate scripts ever need more.
    const MAX_PICKLE_SIZE: usize = 64 * 1024 * 1024;
    let pickle = match miniz_oxide::inflate::decompress_to_vec_zlib_with_limit(
        &bytes[start..start + len],
        MAX_PICKLE_SIZE,
    ) {
        Ok(p) => p,
        Err(e) => {
            if e.status == miniz_oxide::inflate::TINFLStatus::HasMoreOutput {
                tracing::warn!(
                    "rpyc string table exceeds {} bytes decompressed, skipping (possible decompression bomb)",
                    MAX_PICKLE_SIZE
                );
            } else {
                tracing::warn!("failed to decompress rpyc string table: {:?}", e.status);
            }
            return Vec::new();
        }
    };

    let mut seen = std::collections::HashSet::new();
    scan_pickle_strings(&pickle)
        .into_iter()
        .filter(|s| is_renpy_dialogue_like(s))
        .filter(|s| seen.insert(s.clone()))
        .collect()
}

/// Walk a pickle opcode stream and collect every unicode string payload.
/// Skips all other opcodes by their documented argument sizes; bails out on
/// anything unknown rather than misreading data bytes as opcodes.
fn scan_pickle_strings(data: &[u8]) -> Vec<String> {
    let mut out = Vec::new();
    let mut i = 0usize;
    let read_u32 =
        |d: &[u8], p: usize| u32::from_le_bytes(d[p..p + 4].try_into().unwrap()) as usize;

    while i < data.len() {
        let op = data[i];
        i += 1;
        match op {
            // Unicode strings — the payload we're after
            b'X' => {
                // BINUNICODE: u32 length + utf8
                if i + 4 > data.len() {
                    break;
                }
                let n = read_u32(data, i);
                i += 4;
                if i + n > data.len() {
                    break;
                }
                if let Ok(s) = std::str::from_utf8(&data[i..i + n]) {
                    out.push(s.to_string());
                }
                i += n;
            }
            0x8c => {
                // SHORT_BINUNICODE: u8 length + utf8
                if i >= data.len() {
                    break;
                }
                let n = data[i] as usize;
                i += 1;
                if i + n > data.len() {
                    break;
                }
                if let Ok(s) = std::str::from_utf8(&data[i..i + n]) {
                    out.push(s.to_string());
                }
                i += n;
            }

            // Fixed-size arguments
            0x80 | b'K' | b'h' | b'q' | 0x82 => i += 1, // PROTO/BININT1/BINGET/BINPUT/EXT1
            b'M' | 0x83 => i += 2,                      // BININT2/EXT2
            b'J' | b'j' | b'r' | 0x84 => i += 4,        // BININT/LONG_BINGET/LONG_BINPUT/EXT4
            b'G' => i += 8,                             // BINFLOAT

            // Length-prefixed non-unicode payloads
            b'U' | b'C' | 0x8a => {
                // SHORT_BINSTRING / SHORT_BINBYTES / LONG1
                if i >= data.len() {
                    break;
                }
                let n = data[i] as usize;
                i += 1 + n;
            }
            b'T' | b'B' | 0x8b => {
                // BINSTRING / BINBYTES / LONG4
                if i + 4 > data.len() {
                    break;
                }
                let n = read_u32(data, i);
                i += 4 + n;
            }

            // Newline-terminated text arguments
            b'I' | b'L' | b'F' | b'S' | b'V' | b'P' | b'g' | b'p' => {
                while i < data.len() && data[i] != b'\n' {
                    i += 1;
                }
                i += 1;
            }
            b'c' | b'i' => {
                // GLOBAL / INST: two newline-terminated lines
                for _ in 0..2 {
                    while i < data.len() && data[i] != b'\n' {
                        i += 1;
                    }
                    i += 1;
                }
            }

            // No-argument opcodes seen in protocol <= 2 streams
            b'(' | b')' | b'.' | b']' | b'}' | b'a' | b'e' | b's' | b'u' | b't' | b'd'
            | b'l' | b'b' | b'R' | b'N' | b'0' | b'1' | b'2' | b'Q' | b'o' | 0x81 | 0x85
            | 0x86 | 0x87 | 0x88 | 0x89 => {}

            // Unknown opcode: stop rather than misparse
            _ => break,
        }
    }
    out
}

/// Heuristic: keep strings that could be player-visible dialogue/menu text,
/// drop code expressions, identifiers and paths mined from the same pickle.
fn is_renpy_dialogue_like(s: &str) -> bool {
    let t = s.trim();
    if t.len() < 2 || !t.chars().any(|c| c.is_alphabetic()) {
        return false;
    }
    if t.starts_with("renpy") || t.starts_with("store.") || t.starts_with('_') {
        return false;
    }
    // Paths and file references
    if t.contains('/') || t.contains('\\') {
        return false;
    }
    // Code expressions
    for needle in ["==", "!=", ">=", "<=", "+=", "-="] {
        if t.contains(needle) {
            return false;
        }
    }
    for prefix in ["not ", "if ", "elif ", "import ", "def ", "class "] {
        if t.starts_with(prefix) {
            return false;
        }
    }
    // Bare identifiers: no spaces plus dot/underscore access
    if !t.contains(' ') && (t.contains('.') || t.contains('_')) {
        return false;
    }
    true
}

/// Normalize a filesystem path for destination-collision comparison: forward
/// slashes and lowercase, so `game\Script.rpy` and `game/script.rpy` compare
/// equal. This matches NTFS's own case-insensitive semantics (the primary
/// platform for this project), where those two paths refer to the same file.
fn normalize_path_for_compare(p: &Path) -> String {
    let s = p.to_string_lossy().replace('\\', "/");
    // Case folding only where the filesystem itself folds case. NTFS and APFS
    // treat `Script.rpy` and `script.rpy` as one file; ext4 does not, and Ren'Py
    // games run on Linux too — folding there would invent a collision between
    // two genuinely distinct files.
    // ponytail: no Unicode NFC/NFD normalization; canonically-equivalent forms of
    // a Japanese filename would compare unequal. Add it if a real game trips on it.
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    {
        s.to_lowercase()
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        s
    }
}

/// Escape a string as a Python double-quoted literal for the filter file.
fn python_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            _ => out.push(c),
        }
    }
    out
}

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

        // Loose .rpy files (often just patches/mods living next to the packed
        // game) — extract them, but never let them stop the .rpa scan below:
        // the real scripts of a shipped game live inside scripts.rpa.
        let mut all = Vec::new();
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
                // Skip our own generated runtime filter
                if fpath
                    .file_name()
                    .is_some_and(|n| n.to_string_lossy().starts_with("zzz_locust"))
                {
                    continue;
                }
                match Self::extract_file(fpath) {
                    Ok(entries) => all.extend(entries),
                    Err(e) => {
                        tracing::warn!("Failed to extract {}: {}", fpath.display(), e);
                    }
                }
            }
        }

        // Script archives (named ones first, all of them as a fallback)
        let before_rpa = all.len();
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

        // If the named archives yielded nothing, try every .rpa
        if all.len() == before_rpa {
            for entry in std::fs::read_dir(&game_dir)?.filter_map(|e| e.ok()) {
                let fpath = entry.path();
                if fpath.extension().map_or(false, |e| e == "rpa") {
                    if fpath.file_name().map_or(false, |n| {
                        let name = n.to_string_lossy();
                        name.contains("script") || name == "archive.rpa"
                    }) {
                        continue; // already tried above
                    }
                    match self.extract_rpa_archive(&fpath) {
                        Ok(entries) => all.extend(entries),
                        Err(e) => {
                            tracing::warn!("Failed to extract RPA {}: {}", fpath.display(), e);
                        }
                    }
                }
            }
        }

        Ok(all)
    }

    fn inject(&self, path: &Path, entries: &[StringEntry]) -> Result<InjectionReport> {
        // Strings mined from compiled .rpyc scripts can't be written back into
        // source files — they're applied at runtime through Ren'Py's
        // say_menu_text_filter hook, generated as one loose .rpy file.
        let (rpyc_entries, entries): (Vec<&StringEntry>, Vec<&StringEntry>) = entries
            .iter()
            .partition(|e| e.tags.iter().any(|t| t == "rpyc"));
        let entries: Vec<StringEntry> = entries.into_iter().cloned().collect();

        let mut rpyc_report: Option<InjectionReport> = None;
        if !rpyc_entries.is_empty() {
            rpyc_report = Some(Self::inject_rpyc_filter(path, &rpyc_entries)?);
        }
        if entries.is_empty() {
            return Ok(rpyc_report.unwrap_or(InjectionReport {
                files_modified: 0,
                strings_written: 0,
                strings_skipped: 0,
                warnings: Vec::new(),
            }));
        }

        // Route each entry to the handler that can actually write it back: entries
        // sourced from an archive (file_path ends in .rpa) go through the RPA
        // extraction path; entries sourced from loose .rpy files are edited in
        // place. This routing is per-entry, never a batch-wide flag, so a game
        // that ships BOTH an archive and loose .rpy files translates both instead
        // of silently dropping whichever group didn't win the batch-wide check.
        let (rpa_entries, loose_entries): (Vec<StringEntry>, Vec<StringEntry>) = entries
            .into_iter()
            .partition(|e| e.file_path.extension().map_or(false, |ext| ext == "rpa"));

        let mut warnings: Vec<String> = Vec::new();
        let mut strings_skipped = 0usize;

        // Destination collision guard lives at the actual write site
        // (`inject_rpa_inner`), not here: the RPA write destination preserves
        // the archive member's subdirectory (`game_dir.join(rel)`), but entry
        // ids only carry the bare basename, so a basename-only filter here
        // would wrongly drop archive members in a subdirectory whenever ANY
        // unrelated loose file elsewhere under game/ happens to share that
        // basename — no real destination collision exists in that case. Pass
        // the loose destination paths through so the check can compare full,
        // normalized paths against the file actually about to be written.
        let loose_dest_paths: std::collections::HashSet<String> = loose_entries
            .iter()
            .map(|e| normalize_path_for_compare(&e.file_path))
            .collect();

        let mut rpa_report: Option<InjectionReport> = None;
        if !rpa_entries.is_empty() {
            // For RPA-sourced entries: extract .rpy files from archive, apply translations,
            // then place translated .rpy files in game/ dir where Ren'Py loads them with priority.
            rpa_report = Some(self.inject_replace_rpa(path, &rpa_entries, &loose_dest_paths)?);
        }

        let mut files_modified = 0;
        let mut strings_written = 0;

        // Group loose (non-archive) entries by file
        let mut by_file: HashMap<PathBuf, Vec<&StringEntry>> = HashMap::new();
        for entry in &loose_entries {
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

            // Build lookup: line_num -> (translation, source). Entries without a
            // translation are counted as skipped immediately; entries WITH a
            // translation are only counted as written further below, once the
            // expected source text is actually found and replaced — never
            // up-front, since a stale line number or mismatched content means
            // nothing was actually applied.
            let mut line_translations: HashMap<usize, &str> = HashMap::new();
            let mut source_lookup: HashMap<usize, &str> = HashMap::new();
            for entry in file_entries {
                let id_suffix = entry.id.strip_prefix(&format!("{}#", filename));
                if let Some(num_str) = id_suffix {
                    if let Ok(line_num) = num_str.parse::<usize>() {
                        if let Some(ref t) = entry.translation {
                            if t == &entry.source {
                                // Identity translation: nothing would change at
                                // runtime, so don't rewrite the file or force a
                                // needless .rpyc recompile. Matches the guard
                                // already applied to the other two partitions
                                // (inject_rpyc_filter and inject_rpa_inner).
                                strings_skipped += 1;
                            } else {
                                line_translations.insert(line_num, t.as_str());
                                source_lookup.insert(line_num, entry.source.as_str());
                            }
                        } else {
                            strings_skipped += 1;
                        }
                    }
                }
            }

            let mut matched_lines: std::collections::HashSet<usize> = std::collections::HashSet::new();
            let mut new_lines = Vec::new();
            let mut modified = false;
            for (line_idx, line) in content.lines().enumerate() {
                let line_num = line_idx + 1;
                let mut replaced_line = None;
                if let Some(&translation) = line_translations.get(&line_num) {
                    if let Some(&source) = source_lookup.get(&line_num) {
                        let search = format!("\"{}\"", source);
                        if line.contains(&search) {
                            let safe_trans = escape_inner_quotes(translation);
                            let replace = format!("\"{}\"", safe_trans);
                            replaced_line = Some(line.replacen(&search, &replace, 1));
                        }
                    }
                }
                if let Some(new_line) = replaced_line {
                    new_lines.push(new_line);
                    matched_lines.insert(line_num);
                    strings_written += 1;
                    modified = true;
                } else {
                    new_lines.push(line.to_string());
                }
            }

            // Translations that targeted a line number but never actually matched
            // (stale line number, or source text no longer present) are honestly
            // reported as skipped rather than silently dropped while claiming success.
            for line_num in line_translations.keys() {
                if !matched_lines.contains(line_num) {
                    strings_skipped += 1;
                }
            }

            if modified {
                std::fs::write(file_path, new_lines.join("\n"))?;
                files_modified += 1;

                // Delete corresponding .rpyc so Ren'Py recompiles from the modified .rpy
                let rpyc_path = file_path.with_extension("rpyc");
                if rpyc_path.exists() {
                    let _ = std::fs::remove_file(&rpyc_path);
                }
            }
        }

        if let Some(r) = rpa_report {
            files_modified += r.files_modified;
            strings_written += r.strings_written;
            strings_skipped += r.strings_skipped;
            warnings.extend(r.warnings);
        }
        if let Some(r) = rpyc_report {
            files_modified += r.files_modified;
            strings_written += r.strings_written;
            strings_skipped += r.strings_skipped;
            warnings.extend(r.warnings);
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
        use md5::{Digest, Md5};
        use std::collections::HashMap;

        let game_dir = if path.is_dir() {
            Self::find_game_dir(path).unwrap_or_else(|| path.join("game"))
        } else {
            path.parent().unwrap_or(path).to_path_buf()
        };

        let tl_dir = game_dir.join("tl").join(lang);
        std::fs::create_dir_all(&tl_dir)?;

        // Group dialogue entries by source .rpy filename.
        // Each source file gets a corresponding tl/<lang>/<filename>.rpy with
        // `translate <lang> <label>_<hash>:` blocks (Ren'Py's proper translation format).
        let mut by_file: HashMap<String, Vec<&StringEntry>> = HashMap::new();
        let mut string_entries: Vec<&StringEntry> = Vec::new();
        let mut strings_written = 0;
        let mut strings_skipped = 0;

        for entry in entries {
            let translation = match &entry.translation {
                Some(t) if t != &entry.source && !t.trim().is_empty() => t,
                _ => {
                    strings_skipped += 1;
                    continue;
                }
            };
            let _ = translation;

            // Dialogue entries (with known label) go into per-file translate blocks
            let is_dialogue = entry.tags.iter().any(|t| t == "dialogue" || t == "scroll_text" || t == "menu");
            let has_label = entry.metadata.get("label").and_then(|v| v.as_str()).is_some();

            if is_dialogue && has_label {
                let filename = entry
                    .file_path
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string();
                by_file.entry(filename).or_default().push(entry);
            } else {
                // UI labels, ui_label entries, and dialogue without known label
                // go into a strings block
                string_entries.push(entry);
            }
        }

        // Generate one .rpy file per source file with proper translate blocks
        for (filename, file_entries) in &by_file {
            let mut lines = Vec::new();
            lines.push("# Auto-generated by Locust — Ren'Py translation file.".to_string());
            lines.push("# Format: `translate <lang> <label>_<md5hash[:8]>:`".to_string());
            lines.push(String::new());

            let mut seen_ids: std::collections::HashSet<String> = std::collections::HashSet::new();

            for entry in file_entries {
                let translation = entry.translation.as_ref().unwrap();
                let label = entry
                    .metadata
                    .get("label")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown");

                // Compute hash like Ren'Py does: MD5(source_text)[:8]
                let mut hasher = Md5::new();
                hasher.update(entry.source.as_bytes());
                let digest = hasher.finalize();
                let hash: String = digest.iter().take(4).map(|b| format!("{:02x}", b)).collect();

                let translation_id = format!("{}_{}", label, hash);
                if !seen_ids.insert(translation_id.clone()) {
                    // Ren'Py errors on duplicate translate block IDs — skip dupes
                    strings_skipped += 1;
                    continue;
                }

                // Extract line number from entry ID (format: filename.rpy#N)
                let line_num = entry
                    .id
                    .split('#')
                    .last()
                    .unwrap_or("0")
                    .parse::<usize>()
                    .unwrap_or(0);

                // Reconstruct the original "character text" or just "text" line
                let safe_source = escape_inner_quotes(&entry.source);
                let safe_trans = escape_inner_quotes(translation);
                let (orig_line, trans_line) = match &entry.context {
                    Some(ch) => (
                        format!("{} \"{}\"", ch, safe_source),
                        format!("{} \"{}\"", ch, safe_trans),
                    ),
                    None => (format!("\"{}\"", safe_source), format!("\"{}\"", safe_trans)),
                };

                lines.push(format!("# game/{}:{}", filename, line_num));
                lines.push(format!("translate {} {}:", lang, translation_id));
                lines.push(String::new());
                lines.push(format!("    # {}", orig_line));
                lines.push(format!("    {}", trans_line));
                lines.push(String::new());

                strings_written += 1;
            }

            let tl_file = tl_dir.join(filename);
            std::fs::write(&tl_file, lines.join("\n"))?;
            let tl_rpyc = tl_file.with_extension("rpyc");
            if tl_rpyc.exists() {
                let _ = std::fs::remove_file(&tl_rpyc);
            }
        }

        // Generate strings block for UI labels and entries without known labels
        if !string_entries.is_empty() {
            let mut lines = Vec::new();
            lines.push("# Auto-generated by Locust — UI strings translation".to_string());
            lines.push(String::new());
            lines.push(format!("translate {} strings:", lang));
            lines.push(String::new());

            let mut seen_sources: std::collections::HashSet<String> = std::collections::HashSet::new();
            for entry in &string_entries {
                let translation = entry.translation.as_ref().unwrap();
                if !seen_sources.insert(entry.source.clone()) {
                    strings_skipped += 1;
                    continue;
                }
                let escaped_source = escape_inner_quotes(&entry.source);
                let escaped_translation = escape_inner_quotes(translation);
                lines.push(format!("    old \"{}\"", escaped_source));
                lines.push(format!("    new \"{}\"", escaped_translation));
                lines.push(String::new());
                strings_written += 1;
            }

            let tl_file = tl_dir.join("locust_strings.rpy");
            std::fs::write(&tl_file, lines.join("\n"))?;
            let tl_rpyc = tl_file.with_extension("rpyc");
            if tl_rpyc.exists() {
                let _ = std::fs::remove_file(&tl_rpyc);
            }
        }

        // Create locust_languages.rpy with an in-game language picker.
        let langs_file_content = build_language_picker_script(&game_dir, lang);
        let langs_file = game_dir.join("locust_languages.rpy");
        std::fs::write(&langs_file, langs_file_content)?;
        let langs_rpyc = game_dir.join("locust_languages.rpyc");
        if langs_rpyc.exists() {
            let _ = std::fs::remove_file(&langs_rpyc);
        }

        // Remove old locust_language.rpy from previous versions
        for old_name in &["locust_language.rpy", "locust_language.rpyc"] {
            let p = game_dir.join(old_name);
            if p.exists() {
                let _ = std::fs::remove_file(&p);
            }
        }

        Ok(InjectionReport {
            files_modified: by_file.len() + 1,
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
    buttons.push_str("            textbutton \"Original\" action Language(None) xalign 0.5 text_size 22\n");
    for code in &langs {
        let name = lang_name(code);
        buttons.push_str(&format!(
            "            textbutton \"{}\" action Language(\"{}\") xalign 0.5 text_size 22\n",
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

    /// Minimal protocol-2 pickle: PROTO 2, two BINUNICODE payloads, STOP.
    fn tiny_pickle(strings: &[&str]) -> Vec<u8> {
        let mut p = vec![0x80, 0x02];
        for s in strings {
            p.push(b'X');
            p.extend_from_slice(&(s.len() as u32).to_le_bytes());
            p.extend_from_slice(s.as_bytes());
            p.push(b'q'); // BINPUT
            p.push(0);
        }
        p.push(b'.');
        p
    }

    /// Same shape as `tiny_pickle`, but with a large compressible filler string
    /// inserted first so the DECOMPRESSED pickle size can be pushed above or
    /// below `MAX_PICKLE_SIZE` independent of the real payload. The filler is
    /// built from a non-alphabetic byte so `is_renpy_dialogue_like` always
    /// rejects it — it can never show up in `harvest_rpyc_strings` results,
    /// so assertions on the real strings stay exact.
    fn tiny_pickle_padded(filler_len: usize, strings: &[&str]) -> Vec<u8> {
        let mut p = vec![0x80, 0x02];
        let filler = vec![b'0'; filler_len];
        p.push(b'X');
        p.extend_from_slice(&(filler.len() as u32).to_le_bytes());
        p.extend_from_slice(&filler);
        p.push(b'q');
        p.push(0);
        for s in strings {
            p.push(b'X');
            p.extend_from_slice(&(s.len() as u32).to_le_bytes());
            p.extend_from_slice(s.as_bytes());
            p.push(b'q'); // BINPUT
            p.push(0);
        }
        p.push(b'.');
        p
    }

    /// Wrap a raw pickle byte stream into a minimal `RENPY RPC2` container with
    /// a single slot-1 entry, matching what `harvest_rpyc_strings` expects.
    fn wrap_pickle_as_rpyc(pickle: &[u8]) -> Vec<u8> {
        let compressed = miniz_oxide::deflate::compress_to_vec_zlib(pickle, 6);
        let mut blob = b"RENPY RPC2".to_vec();
        let header_end = blob.len() + 24; // one slot entry + terminator
        blob.extend_from_slice(&1u32.to_le_bytes());
        blob.extend_from_slice(&(header_end as u32).to_le_bytes());
        blob.extend_from_slice(&(compressed.len() as u32).to_le_bytes());
        blob.extend_from_slice(&[0u8; 12]); // terminator triplet
        blob.extend_from_slice(&compressed);
        blob
    }

    fn tiny_rpyc(strings: &[&str]) -> Vec<u8> {
        wrap_pickle_as_rpyc(&tiny_pickle(strings))
    }

    #[test]
    fn test_scan_pickle_strings() {
        let p = tiny_pickle(&["Hola, soy [sRocky.name].", "paula.known"]);
        let got = scan_pickle_strings(&p);
        assert_eq!(got, vec!["Hola, soy [sRocky.name].", "paula.known"]);
    }

    #[test]
    fn test_dialogue_heuristic() {
        assert!(is_renpy_dialogue_like("Hola, soy [sRocky.name]."));
        assert!(is_renpy_dialogue_like("Yes"));
        assert!(!is_renpy_dialogue_like("paula.known == False"));
        assert!(!is_renpy_dialogue_like("not paulaChat[1]"));
        assert!(!is_renpy_dialogue_like("store.thing"));
        assert!(!is_renpy_dialogue_like("game/kNPCs/npc_paula.rpy"));
        assert!(!is_renpy_dialogue_like("some_variable_name"));
    }

    #[test]
    fn test_harvest_rpyc_strings() {
        let blob = tiny_rpyc(&["Welcome to Area 69!", "flag_done == True"]);
        let got = harvest_rpyc_strings(&blob);
        assert_eq!(got, vec!["Welcome to Area 69!"]);
    }

    #[test]
    fn test_harvest_rpyc_strings_rejects_decompression_bomb() {
        // The pickle decompresses to just over the 64 MiB cap (MAX_PICKLE_SIZE)
        // AND contains a genuinely harvestable dialogue string. If the size cap
        // did not reject this, `harvest_rpyc_strings` would return that string —
        // so an empty result here can only be explained by the limit kicking in,
        // not by some unrelated parsing failure. Paired with the "under limit"
        // test below, which uses the identical shape and DOES harvest the
        // string, this proves the limit — not something else — is decisive.
        const OVER_LIMIT_FILLER: usize = 65 * 1024 * 1024; // pickle > 64 MiB decompressed
        let pickle = tiny_pickle_padded(OVER_LIMIT_FILLER, &["Welcome to Area 69!"]);
        let blob = wrap_pickle_as_rpyc(&pickle);

        let got = harvest_rpyc_strings(&blob);
        assert!(
            got.is_empty(),
            "bomb-shaped stream must be rejected without panicking, and must not \
             leak the harvestable string it contains"
        );
    }

    #[test]
    fn test_harvest_rpyc_strings_under_limit_is_harvested() {
        // Identical shape to the bomb test above, but padded to stay well UNDER
        // the 64 MiB cap: decompression must succeed and the dialogue string
        // must be harvested. This is the control case proving the empty result
        // in the bomb test is caused specifically by exceeding the size limit.
        const UNDER_LIMIT_FILLER: usize = 1024 * 1024; // 1 MiB, comfortably under the cap
        let pickle = tiny_pickle_padded(UNDER_LIMIT_FILLER, &["Welcome to Area 69!"]);
        let blob = wrap_pickle_as_rpyc(&pickle);

        let got = harvest_rpyc_strings(&blob);
        assert_eq!(got, vec!["Welcome to Area 69!"]);
    }

    #[test]
    fn test_inject_rpyc_filter_writes_file() {
        let dir = std::env::temp_dir().join(format!("locust_rpycf_{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(dir.join("game")).unwrap();

        let mut e = StringEntry::new(
            "scripts.rpa#a.rpyc#s0",
            "Welcome to \"Area 69\"!",
            dir.join("scripts.rpa"),
        );
        e.tags = vec!["dialogue".to_string(), "rpyc".to_string()];
        e.translation = Some("¡Bienvenido a \"Área 69\"!".to_string());

        let plugin = RenPyPlugin::new();
        let report = plugin.inject(&dir, &[e]).unwrap();
        assert_eq!(report.strings_written, 1);

        let content = fs::read_to_string(dir.join("game").join("zzz_locust_translate.rpy")).unwrap();
        assert!(content.contains("say_menu_text_filter"));
        assert!(content.contains(r#""Welcome to \"Area 69\"!": "¡Bienvenido a \"Área 69\"!""#));
    }

    #[test]
    fn test_inject_rpyc_filter_chains_previous_filter() {
        let dir = std::env::temp_dir().join(format!("locust_rpycf_chain_{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(dir.join("game")).unwrap();

        let mut e = StringEntry::new("scripts.rpa#a.rpyc#s0", "Hello!", dir.join("scripts.rpa"));
        e.tags = vec!["dialogue".to_string(), "rpyc".to_string()];
        e.translation = Some("¡Hola!".to_string());

        let plugin = RenPyPlugin::new();
        let report = plugin.inject(&dir, &[e]).unwrap();
        assert_eq!(report.strings_written, 1);
        assert_eq!(report.files_modified, 1);

        let content = fs::read_to_string(dir.join("game").join("zzz_locust_translate.rpy")).unwrap();
        // Must capture whatever filter the game already had installed and call
        // it first, rather than unconditionally replacing config.say_menu_text_filter.
        assert!(content.contains("locust_previous_filter = config.say_menu_text_filter"));
        assert!(content.contains("if locust_previous_filter is not None:"));
        assert!(content.contains("text = locust_previous_filter(text)"));
    }

    #[test]
    fn test_inject_rpyc_filter_zero_translations_does_not_write_file() {
        let dir = std::env::temp_dir().join(format!("locust_rpycf_empty_{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(dir.join("game")).unwrap();

        let mut e = StringEntry::new("scripts.rpa#a.rpyc#s0", "Hello!", dir.join("scripts.rpa"));
        e.tags = vec!["dialogue".to_string(), "rpyc".to_string()];
        e.translation = None; // no qualifying translation

        let plugin = RenPyPlugin::new();
        let report = plugin.inject(&dir, &[e]).unwrap();
        assert_eq!(report.files_modified, 0);
        assert_eq!(report.strings_written, 0);

        assert!(
            !dir.join("game").join("zzz_locust_translate.rpy").exists(),
            "no-op filter file must not be written when there is nothing to translate"
        );
    }

    #[test]
    fn test_inject_rpyc_filter_zero_translations_removes_stale_filter() {
        let dir = std::env::temp_dir().join(format!("locust_rpycf_stale_{}", uuid::Uuid::new_v4()));
        let game_dir = dir.join("game");
        fs::create_dir_all(&game_dir).unwrap();

        // Simulate a previous run's generated filter file (and compiled twin)
        // left over from when translations existed.
        fs::write(
            game_dir.join("zzz_locust_translate.rpy"),
            "# Generated by Locust\ninit 999 python:\n    pass\n",
        )
        .unwrap();
        fs::write(game_dir.join("zzz_locust_translate.rpyc"), b"stale compiled twin").unwrap();

        let mut e = StringEntry::new("scripts.rpa#a.rpyc#s0", "Hello!", dir.join("scripts.rpa"));
        e.tags = vec!["dialogue".to_string(), "rpyc".to_string()];
        e.translation = None; // no qualifying translation this run

        let plugin = RenPyPlugin::new();
        let report = plugin.inject(&dir, &[e]).unwrap();
        assert_eq!(report.strings_written, 0);
        assert_eq!(
            report.files_modified, 1,
            "removing the stale filter is itself a modification and must be reported"
        );

        assert!(
            !game_dir.join("zzz_locust_translate.rpy").exists(),
            "stale filter file must be removed when no translations qualify"
        );
        assert!(
            !game_dir.join("zzz_locust_translate.rpyc").exists(),
            "stale compiled twin must be removed when no translations qualify"
        );
    }

    #[test]
    fn test_inject_rpyc_filter_reconciles_written_and_skipped() {
        let dir = std::env::temp_dir().join(format!("locust_rpycf_reconcile_{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(dir.join("game")).unwrap();

        let mut translated =
            StringEntry::new("scripts.rpa#a.rpyc#s0", "Hello!", dir.join("scripts.rpa"));
        translated.tags = vec!["dialogue".to_string(), "rpyc".to_string()];
        translated.translation = Some("¡Hola!".to_string());

        let mut missing = StringEntry::new("scripts.rpa#a.rpyc#s1", "Bye!", dir.join("scripts.rpa"));
        missing.tags = vec!["dialogue".to_string(), "rpyc".to_string()];
        missing.translation = None;

        let mut same_as_source =
            StringEntry::new("scripts.rpa#a.rpyc#s2", "OK", dir.join("scripts.rpa"));
        same_as_source.tags = vec!["dialogue".to_string(), "rpyc".to_string()];
        same_as_source.translation = Some("OK".to_string());

        let entries = vec![translated, missing, same_as_source];
        let plugin = RenPyPlugin::new();
        let report = plugin.inject(&dir, &entries).unwrap();

        assert_eq!(report.strings_written, 1);
        assert_eq!(report.strings_skipped, 2);
        assert_eq!(
            report.strings_written + report.strings_skipped,
            entries.len(),
            "written + skipped must reconcile with total entries considered"
        );
    }

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
