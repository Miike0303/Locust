use locust_core::backup::RevisionOriginal;
use std::collections::{HashMap, HashSet};
use std::io::{Read as IoRead, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use locust_core::error::Result;
use locust_core::extraction::{FormatPlugin, InjectionReport};
use locust_core::models::{OutputMode, StringEntry};

pub struct RenPyPlugin;

/// Removes the RPA extraction temp directory on EVERY exit path — including
/// each `?` between creating the directory and finishing the harvest, which
/// previously early-returned past a manual cleanup and leaked one
/// `locust_rpa_<uuid>` directory per failing archive per run. Mirrors the
/// `TempFileGuard` precedent in the CLI crate; deliberately duplicated locally
/// rather than adding a cross-crate dependency for a six-line guard.
struct TempDirGuard(PathBuf);

impl Drop for TempDirGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

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
                entries
                    .filter_map(|e| e.ok())
                    .any(|e| e.path().extension().is_some_and(|ext| ext == "rpy"))
            })
            .unwrap_or(false)
    }

    fn extract_rpa_archive(&self, rpa_path: &Path) -> Result<Vec<StringEntry>> {
        let temp_dir = std::env::temp_dir().join(format!("locust_rpa_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&temp_dir)?;
        // Dropped on every exit path below, including the `?` on extract_rpa.
        let _temp_guard = TempDirGuard(temp_dir.clone());

        let extracted_files = Self::extract_rpa(rpa_path, &temp_dir)?;

        let mut all = Vec::new();
        // Members the harvest loop actually considers — tl/ members are
        // extracted from the archive but deliberately skipped below, so they
        // must not arm the "no strings were harvested" warning.
        let mut considered = 0usize;
        for file in &extracted_files {
            // Skip tl/ directory files
            let rel_str = file
                .strip_prefix(&temp_dir)
                .map(|r| r.to_string_lossy().replace('\\', "/"))
                .unwrap_or_default();
            if rel_str.starts_with("tl/") {
                continue;
            }
            considered += 1;

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
                            let mut entry = StringEntry::new(id, &text, rpa_path.to_path_buf());
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

        // Script members went through the harvest loop but not a single string
        // came out of them. Legitimate for pure-code scripts, so no error —
        // but it is also the symptom of a harvester regression, and staying
        // silent here is exactly what let the index-parser bug go unnoticed.
        // Counted over CONSIDERED members only: a translations-only archive
        // (every member under tl/) harvests nothing by design and must not warn.
        if considered > 0 && all.is_empty() {
            tracing::warn!(
                "{}: {} script member(s) considered for harvest but no strings were harvested",
                rpa_path.display(),
                considered
            );
        }

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
                skip_reasons: Default::default(),
                files_modified: if removed_stale { 1 } else { 0 },
                strings_written,
                strings_skipped,
                warnings,
                // Removal only — nothing was written this run.
                files_written: Vec::new(),
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
        let filter_path = game_dir.join("zzz_locust_translate.rpy");
        std::fs::write(&filter_path, file)?;
        // Remove a stale compiled twin so Ren'Py recompiles our new file
        let _ = std::fs::remove_file(game_dir.join("zzz_locust_translate.rpyc"));

        Ok(InjectionReport {
            skip_reasons: Default::default(),
            files_modified: 1,
            strings_written,
            strings_skipped,
            warnings: Vec::new(),
            // Extraction deliberately skips `zzz_locust*`, so this file can
            // only reach a patch through the report — never through entries.
            files_written: vec![filter_path],
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
            if entry.file_path.extension().is_some_and(|ext| ext == "rpa") {
                rpa_files.insert(entry.file_path.clone());
            }
        }

        // Extract .rpy files from each RPA to a temp dir
        let temp_dir =
            std::env::temp_dir().join(format!("locust_rpa_inject_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&temp_dir)?;

        // Run the actual injection logic, ensuring temp_dir is always cleaned up
        let result =
            Self::inject_rpa_inner(&game_dir, &temp_dir, &rpa_files, entries, loose_dest_paths);

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

        // Build a lookup: (filename, line_number) -> (source, translation).
        // The source is checked again against the current extracted script
        // below, so stale database rows can never authorize a code-string edit.
        let mut line_translations: HashMap<(String, usize), (String, String)> = HashMap::new();
        for entry in entries {
            if let Some(ref t) = entry.translation {
                if t != &entry.source {
                    // Entry IDs are "filename.rpy#linenumber" or
                    // "archive.rpa#filename.rpy#linenumber".
                    if let Some((prefix, line_str)) = entry.id.rsplit_once('#') {
                        let filename = prefix.rsplit('#').next().unwrap_or(prefix).to_string();
                        if let Ok(line_num) = line_str.parse::<usize>() {
                            line_translations
                                .insert((filename, line_num), (entry.source.clone(), t.clone()));
                        }
                    }
                }
            }
        }

        let mut files_modified = 0;
        let mut strings_written = 0;
        let mut collision_skipped = 0usize;
        let mut files_written: Vec<PathBuf> = Vec::new();

        // Walk all extracted .rpy files and apply translations by line number
        for dir_entry in walkdir::WalkDir::new(temp_dir)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            let fpath = dir_entry.path();
            if fpath.extension().is_none_or(|e| e != "rpy") {
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
            let filename = rel_path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            let current_sources: std::collections::HashSet<(usize, String)> =
                Self::extract_file(fpath)
                    .unwrap_or_default()
                    .into_iter()
                    .filter_map(|entry| {
                        entry_line_number(&entry.id, &filename).map(|line| (line, entry.source))
                    })
                    .collect();

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
                    // Re-run the same extractor against the current member and
                    // require the same physical source at the same line. This
                    // admits screen text/textbutton/tooltips as well as
                    // dialogue, without admitting arbitrary code literals.
                    if current_sources.contains(&(line_num, source.clone())) {
                        if let Some(new_line) = replace_extracted_literal(line, source, translation)
                        {
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
                files_written.push(dest.clone());

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

        let no_translation = entries.iter().filter(|e| e.translation.is_none()).count();
        let identity = entries
            .iter()
            .filter(|e| e.translation.as_deref() == Some(e.source.as_str()))
            .count();
        let unmatched = entries
            .len()
            .saturating_sub(strings_written + collision_skipped + no_translation + identity);
        let mut skip_reasons = std::collections::BTreeMap::new();
        if no_translation > 0 {
            skip_reasons.insert("untranslated".to_string(), no_translation);
        }
        if identity > 0 {
            skip_reasons.insert("unchanged".to_string(), identity);
        }
        if collision_skipped > 0 {
            skip_reasons.insert("destination_collision".to_string(), collision_skipped);
        }
        if unmatched > 0 {
            skip_reasons.insert("source_changed".to_string(), unmatched);
        }

        Ok(InjectionReport {
            skip_reasons,
            files_modified,
            strings_written,
            strings_skipped: entries.len().saturating_sub(strings_written),
            warnings,
            files_written,
        })
    }

    fn has_rpa_files(dir: &Path) -> bool {
        std::fs::read_dir(dir)
            .map(|entries| {
                entries
                    .filter_map(|e| e.ok())
                    .any(|e| e.path().extension().is_some_and(|ext| ext == "rpa"))
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
        let decompressed =
            miniz_oxide::inflate::decompress_to_vec_zlib(&compressed).map_err(|e| {
                locust_core::error::LocustError::ParseError {
                    file: rpa_path.display().to_string(),
                    message: format!("failed to decompress RPA index: {:?}", e),
                }
            })?;

        // Parse the Python pickle to extract file entries
        // We use a simplified pickle parser that handles the common RPA format
        let index = parse_rpa_pickle(&decompressed, key, &rpa_path.display().to_string())?;

        // Reaching this point with zero members means the pickle parsed
        // CLEANLY to an empty dict (a derailed parse is a hard error naming
        // the offending opcode, above). That is what rpatool or a placeholder
        // archive produces from `{}` — legitimately empty, but a direct
        // `extract <file>.rpa` naming this archive can only ever yield zero
        // strings, so it still fails loudly rather than "succeeding" with
        // nothing. Directory scans degrade this to a warning and move on.
        if index.is_empty() {
            return Err(locust_core::error::LocustError::ParseError {
                file: rpa_path.display().to_string(),
                message: "RPA index parsed cleanly to zero members — the archive is \
                          genuinely empty, nothing to extract"
                    .to_string(),
            });
        }

        let names: HashSet<&str> = index.iter().map(|(name, _, _)| name.as_str()).collect();
        let mut extracted_files = Vec::new();
        for (name, offset, length) in &index {
            // Only extract script files
            if !name.ends_with(".rpy") && !name.ends_with(".rpyc") {
                continue;
            }
            // Prefer .rpy source over .rpyc — if both exist, use the source.
            // Lone .rpyc files (how shipped games are packed) are extracted too
            // and mined for strings via the pickle harvester.
            if Self::has_rpy_twin(&names, name) {
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

    fn has_rpy_twin(names: &HashSet<&str>, rpyc: &str) -> bool {
        rpyc.ends_with(".rpyc") && names.contains(&rpyc[..rpyc.len() - 1])
    }

    fn extract_file(file_path: &Path) -> Result<Vec<StringEntry>> {
        let content = std::fs::read_to_string(file_path)?;
        Ok(Self::extract_content(file_path, &content))
    }

    fn extract_content(file_path: &Path, content: &str) -> Vec<StringEntry> {
        let filename = file_path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();

        let mut entries = Vec::new();
        // (menu header indent, first child/choice indent). A stack preserves the
        // enclosing menu when a nested menu finishes inside one of its branches.
        let mut menus: Vec<(usize, Option<usize>)> = Vec::new();
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
            // Match the Python block tracker: count each leading space/tab as
            // one byte, consistently for headers, choices and branch bodies.
            let indent = line.len() - line.trim_start().len();

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
                let opens = trimmed.matches(['{', '[', '(']).count() as i32;
                let closes = trimmed.matches(['}', ']', ')']).count() as i32;
                if opens > closes {
                    define_bracket_depth = opens - closes;
                    // Still process this line (the `define x = {` might have extract logic)
                    // But don't skip — the first line is the define itself
                }
            } else if define_bracket_depth > 0 {
                let opens = trimmed.matches(['{', '[', '(']).count() as i32;
                let closes = trimmed.matches(['}', ']', ')']).count() as i32;
                define_bracket_depth += opens - closes;
                if define_bracket_depth < 0 {
                    define_bracket_depth = 0;
                }
                // Skip all content inside the multi-line define block
                continue;
            }

            // Track python blocks (skip most content inside them)
            let python_header = is_python_block_header(trimmed);
            if python_header {
                in_python = true;
                python_indent = indent;
                // But still check for translatable calls inside python
            }
            if in_python
                && !trimmed.is_empty()
                && indent <= python_indent
                && !python_header
                && !trimmed.starts_with('#')
            {
                in_python = false;
            }

            // Comments and blank lines do not close or establish a menu scope.
            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }

            // Python strings must not be mistaken for choices or nested menus.
            if !in_python {
                while menus.last().is_some_and(|(header, _)| indent <= *header) {
                    menus.pop();
                }
                if trimmed == "menu:" || trimmed.starts_with("menu ") && trimmed.ends_with(':') {
                    menus.push((indent, None));
                    continue;
                }
                if let Some((_, choice_indent)) = menus.last_mut() {
                    let choice_indent = *choice_indent.get_or_insert(indent);
                    if indent == choice_indent {
                        // Bare captions are translated via strings blocks in
                        // the corpus, so retain their existing menu tag.
                        let text = extract_menu_choice(trimmed).or_else(|| {
                            let (text, end) = extract_quoted_string(trimmed)?;
                            let after = trimmed[end..].trim();
                            (!text.is_empty() && (after.is_empty() || after.starts_with('#')))
                                .then_some(text)
                        });
                        if let Some(text) = text {
                            let id = format!("{}#{}", filename, line_num);
                            let mut entry = StringEntry::new(id, text, file_path.to_path_buf());
                            entry.tags = vec!["menu".to_string()];
                            entries.push(entry);
                            continue;
                        }
                        if trimmed.starts_with("set ") {
                            continue;
                        }
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
            if let Some(rest) = trimmed.strip_prefix("centered ") {
                let rest = rest.trim();
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
            // Branch bodies follow the same extraction path as outside menus.
            if let Some((character, text)) = extract_say_statement(trimmed) {
                let id = format!("{}#{}", filename, line_num);
                let mut entry = StringEntry::new(id, text, file_path.to_path_buf());
                entry.tags = vec!["dialogue".to_string()];
                if let Some(ch) = character {
                    entry.context = Some(ch.to_string());
                }
                // Store label in metadata for proper Ren'Py translation block generation
                if let Some(ref lbl) = current_label {
                    entry
                        .metadata
                        .insert("label".to_string(), serde_json::Value::String(lbl.clone()));
                }
                entries.push(entry);
            }
        }

        entries
    }
}

/// Recognize complete Python headers, including init priorities and named stores.
/// Plain `init:` blocks still contain Ren'Py statements and are not Python blocks.
fn is_python_block_header(line: &str) -> bool {
    let statement = line.split('#').next().unwrap_or(line).trim();
    let Some(header) = statement.strip_suffix(':') else {
        return false;
    };
    let mut words = header.split_whitespace().peekable();
    match words.next() {
        Some("python") => {}
        Some("init") => {
            if words.peek().is_some_and(|word| {
                let digits = word.strip_prefix(['-', '+']).unwrap_or(word);
                !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit())
            }) {
                words.next();
            }
            if words.next() != Some("python") {
                return false;
            }
        }
        Some("translate") => {
            return words.next().is_some_and(is_python_store_name)
                && words.next() == Some("python")
                && words.next().is_none();
        }
        _ => return false,
    }
    for modifier in ["early", "hide"] {
        if words.peek() == Some(&modifier) {
            words.next();
        }
    }
    match words.next() {
        None => true,
        Some("in") => words.next().is_some_and(is_python_store_name) && words.next().is_none(),
        _ => false,
    }
}

fn is_python_store_name(name: &str) -> bool {
    name.split('.').all(|part| {
        let mut chars = part.chars();
        chars.next().is_some_and(|c| c == '_' || c.is_alphabetic())
            && chars.all(|c| c == '_' || c.is_alphanumeric())
    })
}

fn extract_quoted_string(s: &str) -> Option<(&str, usize)> {
    let s = s.trim();
    if !s.starts_with('"') {
        return None;
    }
    let inner = &s[1..];
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
            return Some((&inner[..i], 1 + i + 1));
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
                if character
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_')
                    && !is_renpy_keyword(character)
                {
                    // All words between character and quote must be identifiers/numbers (expression tags)
                    let valid_middle = words[1..]
                        .iter()
                        .all(|w| w.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'));
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

fn is_renpy_keyword(word: &str) -> bool {
    matches!(
        word,
        "screen"
            | "style"
            | "transform"
            | "define"
            | "default"
            | "init"
            | "label"
            | "image"
            | "python"
            | "if"
            | "elif"
            | "else"
            | "while"
            | "for"
            | "return"
            | "jump"
            | "call"
            | "pass"
            | "menu"
            | "scene"
            | "show"
            | "hide"
            | "with"
            | "play"
            | "stop"
            | "pause"
            | "use"
            | "has"
            | "at"
            | "frame"
            | "vbox"
            | "hbox"
            | "grid"
            | "text"
            | "textbutton"
            | "add"
            | "window"
            | "null"
            | "timer"
            | "input"
            | "key"
            | "on"
            | "action"
            | "bar"
            | "viewport"
            | "imagemap"
            | "hotspot"
            | "hotbar"
            | "button"
            | "fixed"
            | "side"
            | "drag"
            | "draggroup"
            | "translate"
            | "class"
            | "import"
            | "from"
            | "as"
            | "in"
            | "not"
            | "and"
            | "or"
            | "id"
            | "layout"
            | "xalign"
            | "yalign"
            | "xpos"
            | "ypos"
            | "xsize"
            | "ysize"
            | "xoffset"
            | "yoffset"
            | "xanchor"
            | "yanchor"
            | "pos"
            | "anchor"
            | "align"
            | "area"
            | "size"
            | "xysize"
            | "idle"
            | "hover"
            | "insensitive"
            | "selected_idle"
            | "selected_hover"
            | "ground"
            | "background"
            | "foreground"
            | "child"
            | "font"
            | "color"
            | "outlines"
            | "kerning"
            | "spacing"
            | "first_indent"
            | "rest_indent"
            | "prefix"
            | "suffix"
            | "alt"
            | "tooltip"
            | "focus"
            | "selected"
            | "sensitive"
            | "keysym"
            | "alternate"
            | "hovered"
            | "unhovered"
            | "clicked"
            | "released"
            | "activate_sound"
            | "hover_sound"
    )
}

/// Escape unescaped double quotes inside a translation string.
/// Turns `"word"` into `\"word\"` but leaves already-escaped `\"` alone.
fn escape_inner_quotes(s: &str) -> String {
    let mut result = String::with_capacity(s.len() + 8);
    let mut prev_was_backslash = false;
    for ch in s.chars() {
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

/// Reconstruct Ren'Py's canonical say literal from the script literal that
/// extraction retains (including escapes). Decode before encoding so an
/// existing `\"`, `\\`, or `\n` is not escaped a second time.
fn canonical_say_string(source: &str) -> String {
    // Ren'Py's lexer collapses literal spaces/newlines before decoding escapes.
    let mut literal = String::with_capacity(source.len());
    let mut in_space = false;
    for ch in source.chars() {
        let is_space = matches!(ch, ' ' | '\n');
        if !is_space || !in_space {
            literal.push(if is_space { ' ' } else { ch });
        }
        in_space = is_space;
    }

    let mut text = String::with_capacity(literal.len());
    let mut chars = literal.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            text.push(ch);
            continue;
        }
        match chars.next() {
            Some('n') => text.push('\n'),
            Some(ch @ ('{' | '[' | '%')) => {
                text.push(ch);
                text.push(ch);
            }
            Some('u') => {
                let mut digits = String::new();
                while digits.len() < 4 && chars.peek().is_some_and(char::is_ascii_hexdigit) {
                    digits.push(chars.next().unwrap());
                }
                if let Some(ch) = u32::from_str_radix(&digits, 16)
                    .ok()
                    .and_then(char::from_u32)
                {
                    text.push(ch);
                } else {
                    text.push_str("\\u");
                    text.push_str(&digits);
                }
            }
            Some(ch) => text.push(ch),
            None => text.push('\\'),
        }
    }

    // Ren'Py's encode_say_string: quote the decoded text, escaping backslashes,
    // newlines, quotes, and spaces after another space.
    let mut encoded = String::with_capacity(text.len() + 2);
    encoded.push('"');
    let mut previous = None;
    for ch in text.chars() {
        match ch {
            '\\' => encoded.push_str("\\\\"),
            '\n' => encoded.push_str("\\n"),
            '"' => encoded.push_str("\\\""),
            ' ' if previous == Some(' ') => encoded.push_str("\\ "),
            _ => encoded.push(ch),
        }
        previous = Some(ch);
    }
    encoded.push('"');
    encoded
}

fn renpy_translation_id(label: &str, code: &str) -> String {
    use md5::{Digest, Md5};

    // Ren'Py hashes canonical statement code followed by CRLF, not text alone.
    let mut hasher = Md5::new();
    hasher.update(code.as_bytes());
    hasher.update(b"\r\n");
    let digest = hasher.finalize();
    let hash: String = digest.iter().take(4).map(|b| format!("{b:02x}")).collect();
    format!("{label}_{hash}")
}

// Real corpus (D:/juegos/renpy): 5,382 TL files, 1,655,580 dialogue blocks,
// 94 game/language scopes. All 132,577 repeated-base suffixes are unpadded
// _N, starting at 1. Game/language + lexical TL path + block encounter order
// gives the best full-ID agreement: 1,646,822/1,655,580 (99.4710%), versus
// per TL file 99.4520%, per source file/line 99.3994%, reverse paths 99.3604%,
// and starting at _0 91.5802%. Per-label scope within a game is equivalent:
// the label is already part of the base. Lexical source paths + encounter
// order agree at 99.4708%; sorting historical source-line comments drops to
// 99.4195% (2,948 backwards line transitions in appended/updated TL files).
// Base hashes agree at 99.7086%; the corpus gate requires full IDs >= 99.4%.
// Fresh Add input is traversed by source path, then script line. Allocate
// across all files, including untranslated occurrences, for each injection.
fn renpy_numbered_translation_id(
    base_id: String,
    allocated: &mut HashMap<String, usize>,
) -> String {
    // Each allocated ID is reserved; values also cache the next suffix for a
    // base. Checking candidates handles collisions with already suffixed IDs.
    let mut suffix = allocated.get(&base_id).copied().unwrap_or(0);
    loop {
        let id = if suffix == 0 {
            base_id.clone()
        } else {
            format!("{base_id}_{suffix}")
        };
        suffix += 1;
        if let std::collections::hash_map::Entry::Vacant(slot) = allocated.entry(id.clone()) {
            slot.insert(1);
            allocated.insert(base_id, suffix);
            return id;
        }
    }
}

fn renpy_add_line_number(entry: &StringEntry) -> usize {
    entry
        .id
        .rsplit('#')
        .next()
        .unwrap_or("0")
        .parse()
        .unwrap_or(0)
}

/// Check if a string looks like a file path/reference (not translatable text)
fn is_file_reference(text: &str) -> bool {
    let t = text.trim();
    // File extensions
    if t.ends_with(".png")
        || t.ends_with(".jpg")
        || t.ends_with(".jpeg")
        || t.ends_with(".webp")
        || t.ends_with(".gif")
        || t.ends_with(".svg")
        || t.ends_with(".bmp")
        || t.ends_with(".mp3")
        || t.ends_with(".ogg")
        || t.ends_with(".wav")
        || t.ends_with(".flac")
        || t.ends_with(".mp4")
        || t.ends_with(".webm")
        || t.ends_with(".avi")
        || t.ends_with(".ogv")
        || t.ends_with(".ttf")
        || t.ends_with(".otf")
        || t.ends_with(".woff")
        || t.ends_with(".rpy")
        || t.ends_with(".rpyc")
        || t.ends_with(".rpa")
        || t.ends_with(".json")
        || t.ends_with(".txt")
        || t.ends_with(".xml")
        || t.ends_with(".csv")
    {
        return true;
    }
    // Text tags (especially closing tags) and string escapes are not path
    // separators. Check only what remains, without changing the extracted text.
    if t.contains(['/', '\\']) {
        let mut path = String::with_capacity(t.len());
        let mut rest = t;
        while !rest.is_empty() {
            if rest.starts_with('{') && !rest.starts_with("{{") {
                if let Some(end) = rest.find('}') {
                    rest = &rest[end + 1..];
                    continue;
                }
            }
            if rest.starts_with('\\') {
                let mut chars = rest.chars();
                chars.next();
                if chars
                    .next()
                    .is_some_and(|ch| matches!(ch, '"' | '\'' | '\\' | 'n' | 'r' | 't' | ' '))
                {
                    rest = chars.as_str();
                    continue;
                }
            }
            let ch = rest.chars().next().unwrap();
            path.push(ch);
            rest = &rest[ch.len_utf8()..];
        }
        // A slash in prose (yes/no, interpolation, percentages) is not enough.
        // Extensionless resource paths still have a root or resource directory.
        let slash_path = path.contains('/')
            && (path.starts_with('/')
                || path.starts_with("./")
                || path.starts_with("../")
                || ["gui/", "images/", "audio/", "video/", "fonts/", "scripts/"]
                    .iter()
                    .any(|prefix| path.starts_with(prefix))
                || path.split('/').any(|segment| {
                    segment.rsplit_once('.').is_some_and(|(stem, extension)| {
                        !stem.is_empty()
                            && !extension.is_empty()
                            && extension.chars().all(|ch| ch.is_ascii_alphanumeric())
                    })
                }));
        if !path.contains(char::is_whitespace)
            && !path.contains(['{', '}', '[', ']', '%'])
            && (path.contains('\\') || slash_path)
        {
            return true;
        }
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
    // Only choice headers: bare captions are handled at the menu's child level.
    let (text, end) = extract_quoted_string(trimmed)?;
    if text.is_empty() {
        return None;
    }
    let after = trimmed[end..].trim();
    // Conditions and arguments are opaque expressions, but their delimiters
    // must balance and the header must end at a top-level colon (plus comment).
    let clause = after
        .strip_prefix("if")
        .filter(|rest| rest.starts_with(char::is_whitespace) && !rest.trim().is_empty());
    if !after.starts_with(':') && !after.starts_with('(') && clause.is_none() {
        return None;
    }
    let mut brackets = Vec::new();
    let mut in_arguments = after.starts_with('(');
    let mut quote = None;
    let mut escaped = false;
    for (pos, ch) in after.char_indices() {
        if let Some(delimiter) = quote {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == delimiter {
                quote = None;
            }
            continue;
        }
        match ch {
            '\'' | '"' => quote = Some(ch),
            '(' => brackets.push(')'),
            '[' => brackets.push(']'),
            '{' => brackets.push('}'),
            ')' | ']' | '}' => {
                if brackets.pop() != Some(ch) {
                    return None;
                }
                // After an argument list only a condition or the final colon
                // can follow; do not accept arbitrary statement suffixes.
                if in_arguments && brackets.is_empty() {
                    in_arguments = false;
                    let tail = after[pos + 1..].trim_start();
                    if !tail.starts_with(':')
                        && !tail.strip_prefix("if").is_some_and(|rest| {
                            rest.starts_with(char::is_whitespace)
                                && !rest.trim_start().starts_with(':')
                                && !rest.trim().is_empty()
                        })
                    {
                        return None;
                    }
                }
            }
            ':' if brackets.is_empty() => {
                if clause.is_some_and(|rest| rest.trim_start().starts_with(':')) {
                    return None;
                }
                let tail = after[pos + 1..].trim();
                return (tail.is_empty() || tail.starts_with('#')).then_some(text);
            }
            '#' => return None,
            _ => {}
        }
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
    let prefixes = &["text ", "textbutton ", "tooltip "];

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

fn entry_line_number(id: &str, filename: &str) -> Option<usize> {
    let (prefix, line) = id.rsplit_once('#')?;
    let id_filename = prefix.rsplit('#').next().unwrap_or(prefix);
    (id_filename == filename)
        .then(|| line.parse::<usize>().ok())
        .flatten()
}

/// Return the exact literal slice selected by the source extractor for a line.
/// The full-file extraction pass remains authoritative for stateful exclusions
/// such as Python and multi-line define blocks; this locates only the literal
/// that pass selected, so a duplicate action string later on the line is safe.
fn extracted_literal_on_line<'a>(line: &'a str, expected: &str) -> Option<&'a str> {
    let trimmed = line.trim();
    let matches_expected = |candidate: Option<&'a str>| candidate.filter(|text| *text == expected);

    matches_expected(extract_underscore_call(trimmed))
        .or_else(|| matches_expected(extract_p_call(trimmed)))
        .or_else(|| matches_expected(extract_character_name(trimmed)))
        .or_else(|| matches_expected(extract_renpy_call(trimmed, "renpy.notify(")))
        .or_else(|| matches_expected(extract_renpy_call(trimmed, "renpy.input(")))
        .or_else(|| matches_expected(extract_define_string(trimmed)))
        .or_else(|| matches_expected(extract_screen_text(trimmed)))
        .or_else(|| {
            matches_expected(
                trimmed
                    .strip_prefix("centered ")
                    .and_then(|rest| extract_quoted_string(rest.trim()).map(|(text, _)| text)),
            )
        })
        .or_else(|| matches_expected(extract_menu_choice(trimmed)))
        .or_else(|| matches_expected(extract_say_statement(trimmed).map(|(_, text)| text)))
}

fn escape_renpy_double_quoted(s: &str) -> String {
    let mut escaped = String::with_capacity(s.len() + 8);
    for ch in s.chars() {
        match ch {
            '\\' => escaped.push_str("\\\\"),
            '"' => escaped.push_str("\\\""),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            _ => escaped.push(ch),
        }
    }
    escaped
}

fn replace_extracted_literal(line: &str, source: &str, translation: &str) -> Option<String> {
    let literal = extracted_literal_on_line(line, source)?;

    let start = literal.as_ptr() as usize - line.as_ptr() as usize;
    let end = start.checked_add(literal.len())?;
    let mut replaced = line.to_string();
    replaced.replace_range(start..end, &escape_renpy_double_quoted(translation));
    Some(replaced)
}

/// Check if a gui.xxx variable is non-translatable (colors, sizes, fonts, layout values).
fn is_gui_non_translatable(var: &str) -> bool {
    if !var.starts_with("gui.") {
        return false;
    }
    let prop = &var[4..];
    // Explicit non-translatable system values
    if prop == "language"
        || prop == "unscrollable"
        || prop == "rollback_side"
        || prop == "history_allow_tags"
    {
        return true;
    }
    // Skip color, size, font, border, padding, spacing, position properties
    prop.contains("color")
        || prop.contains("size")
        || prop.contains("font")
        || prop.contains("border")
        || prop.contains("padding")
        || prop.contains("spacing")
        || prop.contains("height")
        || prop.contains("width")
        || prop.contains("align")
        || prop.contains("offset")
        || prop.contains("xpos")
        || prop.contains("ypos")
        || prop.contains("tile")
        || prop.contains("opacity")
        || prop.contains("outlines")
        || prop.contains("background")
        || prop.contains("icon")
        || prop.ends_with("_idle")
        || prop.ends_with("_hover")
        || prop.ends_with("_insensitive")
        || prop.starts_with("show_")
        || prop.starts_with("button_")
        || prop.starts_with("choice_")
        || prop.starts_with("navigation_")
        || prop.starts_with("slot_")
        || prop.starts_with("namebox_")
}

/// Read typed display strings out of a compiled Ren'Py script (.rpyc).
///
/// Layout: "RENPY RPC2" magic + slot table; slot 1 is a zlib-compressed
/// Python pickle of the script AST. The inert reader selects visible AST
/// fields, preserving their exact runtime strings for the injection filter.
/// Unsupported/malformed pickles use a conservative per-file fallback.
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
    let Some((start, len)) = slot1 else {
        return Vec::new();
    };
    let Some(end) = start.checked_add(len).filter(|&end| end <= bytes.len()) else {
        return Vec::new();
    };

    // ponytail: bounded single-shot inflate, ceiling 64 MiB of decompressed pickle.
    // A real rpyc's string table never approaches this; an untrusted/downloaded
    // .rpyc claiming a much larger payload is treated as a decompression bomb and
    // skipped rather than allowed to force an unbounded allocation. Upgrade path:
    // stream through `flate2` if legitimate scripts ever need more.
    const MAX_PICKLE_SIZE: usize = 64 * 1024 * 1024;
    let pickle = match miniz_oxide::inflate::decompress_to_vec_zlib_with_limit(
        &bytes[start..end],
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
    let strings = match crate::renpy_pickle::visible_text(&pickle) {
        Ok(strings) => strings,
        Err(reason) => {
            tracing::warn!(
                "typed rpyc reader failed ({reason}); using conservative string fallback"
            );
            scan_pickle_strings(&pickle)
                .into_iter()
                .filter(|s| is_renpy_fallback_text(s))
                .collect()
        }
    };
    strings
        .into_iter()
        .filter(|s| seen.insert(s.clone()))
        .collect()
}

/// On an unreadable AST, require prose evidence as well as the old guard.
/// The audit found bare names (sRockyT), image names, and space-containing
/// expressions (ShowMenu('preferences'), Preference("text speed")) passing
/// that guard. This fallback deliberately prefers losing uncertain text to
/// sending code to translation; supported ASTs never use this heuristic.
fn is_renpy_fallback_text(s: &str) -> bool {
    let t = s.trim();
    is_renpy_dialogue_like(s)
        && t.chars().any(char::is_whitespace)
        && !t.contains(['(', ')', '=', '\n', '\r', '\t'])
        && ![
            "return ", "raise ", "assert ", "from ", "for ", "while ", "lambda ", "hide ", "show ",
            "scene ", "call ", "jump ", "play ", "stop ",
        ]
        .iter()
        .any(|prefix| t.starts_with(prefix))
        && !t.split_whitespace().any(|word| {
            word.contains('_')
                || word.contains(".rpy")
                || word.contains(".png")
                || word.contains(".jpg")
                || word.contains(".ogg")
                || matches!(
                    word,
                    "and"
                        | "or"
                        | "in"
                        | "is"
                        | "True"
                        | "False"
                        | "None"
                        | "+"
                        | "-"
                        | "*"
                        | "**"
                        | "%"
                        | "&"
                        | "|"
                )
        })
        && (t.split_whitespace().count() >= 3 || t.contains(['!', '?', ',']))
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
                let Some(end) = i.checked_add(n).filter(|&end| end <= data.len()) else {
                    break;
                };
                if let Ok(s) = std::str::from_utf8(&data[i..end]) {
                    out.push(s.to_string());
                }
                i = end;
            }
            0x8c => {
                // SHORT_BINUNICODE: u8 length + utf8
                if i >= data.len() {
                    break;
                }
                let n = data[i] as usize;
                i += 1;
                let Some(end) = i.checked_add(n).filter(|&end| end <= data.len()) else {
                    break;
                };
                if let Ok(s) = std::str::from_utf8(&data[i..end]) {
                    out.push(s.to_string());
                }
                i = end;
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
                let Some(end) = i
                    .checked_add(4)
                    .and_then(|i| i.checked_add(n))
                    .filter(|&end| end <= data.len())
                else {
                    break;
                };
                i = end;
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
            b'(' | b')' | b'.' | b']' | b'}' | b'a' | b'e' | b's' | b'u' | b't' | b'd' | b'l'
            | b'b' | b'R' | b'N' | b'0' | b'1' | b'2' | b'Q' | b'o' | 0x81 | 0x85 | 0x86 | 0x87
            | 0x88 | 0x89 => {}

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

fn parse_rpa_pickle(data: &[u8], key: i64, file: &str) -> Result<Vec<(String, u64, usize)>> {
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
            0x80 => {
                pos += 1;
            } // PROTO
            0x95 => {
                pos += 8;
            } // FRAME (protocol 4+) — skip 8-byte frame length
            0x94 => {
                // MEMOIZE (protocol 4+) — store stack top in memo
                if let Some(top) = stack.last() {
                    memo.push(top.clone());
                }
            }
            0x7d => stack.push(PickleVal::Dict), // EMPTY_DICT
            0x5d => stack.push(PickleVal::List(Vec::new())), // EMPTY_LIST
            0x28 => mark_stack.push(stack.len()), // MARK
            0x71 => {
                // SHORT_BINPUT (memo)
                if pos >= len {
                    break;
                }
                let idx = data[pos] as usize;
                pos += 1;
                if let Some(top) = stack.last() {
                    while memo.len() <= idx {
                        memo.push(PickleVal::None);
                    }
                    memo[idx] = top.clone();
                }
            }
            0x72 => {
                // LONG_BINPUT
                if pos + 4 > len {
                    break;
                }
                let idx =
                    u32::from_le_bytes([data[pos], data[pos + 1], data[pos + 2], data[pos + 3]])
                        as usize;
                pos += 4;
                if let Some(top) = stack.last() {
                    while memo.len() <= idx {
                        memo.push(PickleVal::None);
                    }
                    memo[idx] = top.clone();
                }
            }
            0x68 => {
                // SHORT_BINGET
                if pos >= len {
                    break;
                }
                let idx = data[pos] as usize;
                pos += 1;
                let val = memo.get(idx).cloned().unwrap_or(PickleVal::None);
                stack.push(val);
            }
            0x6a => {
                // LONG_BINGET
                if pos + 4 > len {
                    break;
                }
                let idx =
                    u32::from_le_bytes([data[pos], data[pos + 1], data[pos + 2], data[pos + 3]])
                        as usize;
                pos += 4;
                let val = memo.get(idx).cloned().unwrap_or(PickleVal::None);
                stack.push(val);
            }
            0x43 => {
                // SHORT_BINBYTES
                if pos >= len {
                    break;
                }
                let slen = data[pos] as usize;
                pos += 1;
                if pos + slen > len {
                    break;
                }
                let s = String::from_utf8_lossy(&data[pos..pos + slen]).to_string();
                pos += slen;
                stack.push(PickleVal::Str(s));
            }
            0x44 => {
                // BINBYTES (4-byte len)
                if pos + 4 > len {
                    break;
                }
                let slen =
                    u32::from_le_bytes([data[pos], data[pos + 1], data[pos + 2], data[pos + 3]])
                        as usize;
                pos += 4;
                if pos + slen > len {
                    break;
                }
                let s = String::from_utf8_lossy(&data[pos..pos + slen]).to_string();
                pos += slen;
                stack.push(PickleVal::Str(s));
            }
            0x8e => {
                // BINBYTES8 (8-byte len, protocol 4+)
                if pos + 8 > len {
                    break;
                }
                let slen = u64::from_le_bytes([
                    data[pos],
                    data[pos + 1],
                    data[pos + 2],
                    data[pos + 3],
                    data[pos + 4],
                    data[pos + 5],
                    data[pos + 6],
                    data[pos + 7],
                ]) as usize;
                pos += 8;
                if pos + slen > len {
                    break;
                }
                let s = String::from_utf8_lossy(&data[pos..pos + slen]).to_string();
                pos += slen;
                stack.push(PickleVal::Str(s));
            }
            0x58 => {
                // BINUNICODE (4-byte length + utf8) — how real RPA indexes encode filenames
                if pos + 4 > len {
                    break;
                }
                let slen =
                    u32::from_le_bytes([data[pos], data[pos + 1], data[pos + 2], data[pos + 3]])
                        as usize;
                pos += 4;
                if pos + slen > len {
                    break;
                }
                let s = String::from_utf8_lossy(&data[pos..pos + slen]).to_string();
                pos += slen;
                stack.push(PickleVal::Str(s));
            }
            0x8c => {
                // SHORT_BINUNICODE (protocol 4+) — 1-byte length
                if pos >= len {
                    break;
                }
                let slen = data[pos] as usize;
                pos += 1;
                if pos + slen > len {
                    break;
                }
                let s = String::from_utf8_lossy(&data[pos..pos + slen]).to_string();
                pos += slen;
                stack.push(PickleVal::Str(s));
            }
            0x55 => {
                // SHORT_BINSTRING
                if pos >= len {
                    break;
                }
                let slen = data[pos] as usize;
                pos += 1;
                if pos + slen > len {
                    break;
                }
                let s = String::from_utf8_lossy(&data[pos..pos + slen]).to_string();
                pos += slen;
                stack.push(PickleVal::Str(s));
            }
            0x54 => {
                // BINSTRING (4-byte len)
                if pos + 4 > len {
                    break;
                }
                let slen =
                    u32::from_le_bytes([data[pos], data[pos + 1], data[pos + 2], data[pos + 3]])
                        as usize;
                pos += 4;
                if pos + slen > len {
                    break;
                }
                let s = String::from_utf8_lossy(&data[pos..pos + slen]).to_string();
                pos += slen;
                stack.push(PickleVal::Str(s));
            }
            0x4a => {
                // BININT
                if pos + 4 > len {
                    break;
                }
                let v = i32::from_le_bytes([data[pos], data[pos + 1], data[pos + 2], data[pos + 3]])
                    as i64;
                pos += 4;
                stack.push(PickleVal::Int(v));
            }
            0x4b => {
                // BININT1
                if pos >= len {
                    break;
                }
                stack.push(PickleVal::Int(data[pos] as i64));
                pos += 1;
            }
            0x4d => {
                // BININT2
                if pos + 2 > len {
                    break;
                }
                let v = u16::from_le_bytes([data[pos], data[pos + 1]]) as i64;
                pos += 2;
                stack.push(PickleVal::Int(v));
            }
            0x8a => {
                // LONG1
                if pos >= len {
                    break;
                }
                let nbytes = data[pos] as usize;
                pos += 1;
                if pos + nbytes > len {
                    break;
                }
                let mut v: i64 = 0;
                for i in 0..nbytes.min(8) {
                    v |= (data[pos + i] as i64) << (i * 8);
                }
                pos += nbytes;
                stack.push(PickleVal::Int(v));
            }
            0x74 => {
                // TUPLE
                let mark = mark_stack.pop().unwrap_or(0).min(stack.len());
                let items: Vec<PickleVal> = stack.drain(mark..).collect();
                stack.push(PickleVal::Tuple(items));
            }
            0x85 => {
                // TUPLE1
                let v = stack.pop().unwrap_or(PickleVal::None);
                stack.push(PickleVal::Tuple(vec![v]));
            }
            0x86 => {
                // TUPLE2
                let b = stack.pop().unwrap_or(PickleVal::None);
                let a = stack.pop().unwrap_or(PickleVal::None);
                stack.push(PickleVal::Tuple(vec![a, b]));
            }
            0x87 => {
                // TUPLE3
                let c = stack.pop().unwrap_or(PickleVal::None);
                let b = stack.pop().unwrap_or(PickleVal::None);
                let a = stack.pop().unwrap_or(PickleVal::None);
                stack.push(PickleVal::Tuple(vec![a, b, c]));
            }
            0x61 => {
                // APPEND
                let val = stack.pop().unwrap_or(PickleVal::None);
                if let Some(PickleVal::List(ref mut list)) = stack.last_mut() {
                    list.push(val);
                }
            }
            0x65 => {
                // APPENDS
                let mark = mark_stack.pop().unwrap_or(stack.len()).min(stack.len());
                let items: Vec<PickleVal> = stack.drain(mark..).collect();
                if let Some(PickleVal::List(ref mut list)) = stack.last_mut() {
                    list.extend(items);
                }
            }
            0x73 => {
                // SETITEM
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
                                    if let PickleVal::Str(ref s) = t[2] {
                                        s.len()
                                    } else {
                                        0
                                    }
                                } else {
                                    0
                                };
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
            0x75 => {
                // SETITEMS
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
                                            if let PickleVal::Str(ref s) = t[2] {
                                                s.len()
                                            } else {
                                                0
                                            }
                                        } else {
                                            0
                                        };
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
            0x4e => stack.push(PickleVal::None),   // NONE
            0x88 => stack.push(PickleVal::Int(1)), // NEWTRUE
            0x89 => stack.push(PickleVal::Int(0)), // NEWFALSE
            0x2e => break,                         // STOP
            op => {
                // Every no-argument opcode a real index emits has an explicit
                // arm above. Anything else carries an argument of unknown
                // width: "skipping" only the opcode byte leaves its argument
                // bytes to be misread as opcodes, silently derailing the
                // stream — exactly how one missing BINUNICODE arm turned a
                // 271-member index into zero members with no diagnostic. And
                // once at least one SETITEMS batch has been harvested, the
                // index is non-empty, so a derail past that point would
                // truncate the member list with no detector at all. Hard-stop
                // and name the byte so the next such report arrives with the
                // answer attached.
                return Err(locust_core::error::LocustError::ParseError {
                    file: file.to_string(),
                    message: format!(
                        "unsupported pickle opcode {op:#04x} at index byte offset {} — \
                         refusing to skip it: its argument width is unknown, so the \
                         stream cannot be re-synchronized",
                        pos - 1
                    ),
                });
            }
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
            if path.extension().is_some_and(|e| e == "rpa") {
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
            .filter_entry(crate::discovery::is_game_entry)
            .filter_map(|e| e.ok())
        {
            let fpath = entry.path();
            if fpath.extension().is_some_and(|e| e == "rpy") {
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
                // Skip files Locust generates itself: the runtime filter and the
                // Add-mode language picker (plus its pre-rename spelling).
                if fpath.file_name().is_some_and(|n| {
                    let n = n.to_string_lossy();
                    n.starts_with("zzz_locust")
                        || n == "locust_languages.rpy"
                        || n == "locust_language.rpy"
                }) {
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
            if fpath.extension().is_some_and(|e| e == "rpa")
                && fpath.file_name().is_some_and(|n| {
                    let name = n.to_string_lossy();
                    name.contains("script") || name == "archive.rpa"
                })
            {
                match self.extract_rpa_archive(&fpath) {
                    Ok(entries) => all.extend(entries),
                    Err(e) => {
                        tracing::warn!("Failed to extract RPA {}: {}", fpath.display(), e);
                    }
                }
            }
        }

        // If the named archives yielded nothing, try every .rpa
        if all.len() == before_rpa {
            for entry in std::fs::read_dir(&game_dir)?.filter_map(|e| e.ok()) {
                let fpath = entry.path();
                if fpath.extension().is_some_and(|e| e == "rpa") {
                    if fpath.file_name().is_some_and(|n| {
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

        // Keep archive rows for collision reporting, but persist active loose
        // overrides last when their stable IDs overlap archived script rows.
        // Database extraction uses upsert-by-ID, just as Ren'Py gives a loose
        // file priority over the archived member of the same path.
        all.sort_by_key(|entry| entry.file_path.extension().is_none_or(|ext| ext != "rpa"));
        Ok(all)
    }

    fn prepare_revision_entries(
        &self,
        entries: &mut [StringEntry],
        originals: &HashMap<PathBuf, RevisionOriginal>,
    ) -> Result<()> {
        let mut by_file: HashMap<PathBuf, Vec<&mut StringEntry>> = HashMap::new();
        for entry in entries {
            by_file
                .entry(entry.file_path.clone())
                .or_default()
                .push(entry);
        }
        for (current_path, original_path) in originals {
            let Some(file_entries) = by_file.get_mut(current_path) else {
                continue;
            };
            // Compiled scripts and archive overlays have different identity
            // rules; only exact loose-script line locators are supported here.
            if current_path.extension().is_none_or(|ext| ext != "rpy") {
                continue;
            }
            let filename = current_path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy();
            let original = original_path.read_text()?;
            let current = std::fs::read_to_string(current_path)?;
            if original == current {
                continue;
            }
            let old_rows = Self::extract_content(current_path, &original);
            let new_rows = Self::extract_content(current_path, &current);
            let old_lines: Vec<_> = original.lines().collect();
            let new_lines: Vec<_> = current.lines().collect();
            if old_lines.len() != new_lines.len() {
                continue;
            }
            let index_lines = |rows: &[StringEntry]| {
                let mut index: HashMap<usize, Option<usize>> = HashMap::new();
                for (position, row) in rows.iter().enumerate() {
                    if let Some(line) = entry_line_number(&row.id, &filename) {
                        // Multiple extractor rows on one line remain ambiguous.
                        index
                            .entry(line)
                            .and_modify(|value| *value = None)
                            .or_insert(Some(position));
                    }
                }
                index
            };
            let old_index = index_lines(&old_rows);
            let new_index = index_lines(&new_rows);
            for entry in file_entries {
                let Some(line) = entry_line_number(&entry.id, &filename)
                    .filter(|line| *line > 0 && *line <= old_lines.len())
                else {
                    continue;
                };
                let (Some(Some(old)), Some(Some(new))) =
                    (old_index.get(&line), new_index.get(&line))
                else {
                    continue;
                };
                let (old, new) = (&old_rows[*old], &new_rows[*new]);
                if entry.source == new.source || entry.source != old.source {
                    continue;
                }
                // Prove this exact extractor-selected literal is the only
                // change on the line; action/code literals are not candidates.
                let old_line = old_lines[line - 1];
                let new_line = new_lines[line - 1];
                let (Some(old_literal), Some(new_literal)) = (
                    extracted_literal_on_line(old_line, &old.source),
                    extracted_literal_on_line(new_line, &new.source),
                ) else {
                    continue;
                };
                let old_start = old_literal.as_ptr() as usize - old_line.as_ptr() as usize;
                let new_start = new_literal.as_ptr() as usize - new_line.as_ptr() as usize;
                if old_line[..old_start] == new_line[..new_start]
                    && old_line[old_start + old_literal.len()..]
                        == new_line[new_start + new_literal.len()..]
                {
                    entry.source = new.source.clone();
                }
            }
        }
        Ok(())
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
                skip_reasons: Default::default(),
                files_modified: 0,
                strings_written: 0,
                strings_skipped: 0,
                warnings: Vec::new(),
                files_written: Vec::new(),
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
            .partition(|e| e.file_path.extension().is_some_and(|ext| ext == "rpa"));

        let mut warnings: Vec<String> = Vec::new();

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
        let mut files_written: Vec<PathBuf> = Vec::new();

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

            // The current extraction pass is the write allow-list. It carries
            // the extractor's stateful exclusions, so a stale row cannot turn
            // an arbitrary code literal at the old line number into a target.
            let current_sources: std::collections::HashSet<(usize, String)> =
                Self::extract_file(file_path)
                    .unwrap_or_default()
                    .into_iter()
                    .filter_map(|entry| {
                        entry_line_number(&entry.id, &filename).map(|line| (line, entry.source))
                    })
                    .collect();

            // Entries are only counted as written after an exact current-source
            // match and a replacement of the extractor-selected literal.
            let mut line_translations: HashMap<usize, &StringEntry> = HashMap::new();
            for entry in file_entries {
                if let (Some(line_num), Some(translation)) = (
                    entry_line_number(&entry.id, &filename),
                    entry.translation.as_deref(),
                ) {
                    if translation != entry.source.as_str() {
                        line_translations.insert(line_num, entry);
                    }
                }
            }

            let mut new_lines = Vec::new();
            let mut modified = false;
            for (line_idx, line) in content.lines().enumerate() {
                let line_num = line_idx + 1;
                let mut replaced_line = None;
                if let Some(entry) = line_translations.get(&line_num) {
                    if current_sources.contains(&(line_num, entry.source.clone())) {
                        if let Some(translation) = entry.translation.as_deref() {
                            replaced_line =
                                replace_extracted_literal(line, &entry.source, translation);
                        }
                    }
                }
                if let Some(new_line) = replaced_line {
                    new_lines.push(new_line);
                    strings_written += 1;
                    modified = true;
                } else {
                    new_lines.push(line.to_string());
                }
            }

            if modified {
                std::fs::write(file_path, new_lines.join("\n"))?;
                files_modified += 1;
                files_written.push(file_path.clone());

                // Delete corresponding .rpyc so Ren'Py recompiles from the modified .rpy
                let rpyc_path = file_path.with_extension("rpyc");
                if rpyc_path.exists() {
                    let _ = std::fs::remove_file(&rpyc_path);
                }
            }
        }

        let loose_written = strings_written;
        let loose_no_translation = loose_entries
            .iter()
            .filter(|entry| entry.translation.is_none())
            .count();
        let loose_identity = loose_entries
            .iter()
            .filter(|entry| entry.translation.as_deref() == Some(entry.source.as_str()))
            .count();
        let loose_unmatched = loose_entries
            .len()
            .saturating_sub(loose_written + loose_no_translation + loose_identity);
        let mut strings_skipped = loose_entries.len().saturating_sub(loose_written);
        let mut skip_reasons = std::collections::BTreeMap::new();
        if loose_no_translation > 0 {
            skip_reasons.insert("untranslated".to_string(), loose_no_translation);
        }
        if loose_identity > 0 {
            skip_reasons.insert("unchanged".to_string(), loose_identity);
        }
        if loose_unmatched > 0 {
            skip_reasons.insert("source_changed".to_string(), loose_unmatched);
        }

        if let Some(r) = rpa_report {
            files_modified += r.files_modified;
            strings_written += r.strings_written;
            strings_skipped += r.strings_skipped;
            warnings.extend(r.warnings);
            files_written.extend(r.files_written);
            for (reason, count) in r.skip_reasons {
                *skip_reasons.entry(reason).or_default() += count;
            }
        }
        if let Some(r) = rpyc_report {
            files_modified += r.files_modified;
            strings_written += r.strings_written;
            strings_skipped += r.strings_skipped;
            warnings.extend(r.warnings);
            files_written.extend(r.files_written);
            for (reason, count) in r.skip_reasons {
                *skip_reasons.entry(reason).or_default() += count;
            }
        }

        Ok(InjectionReport {
            skip_reasons,
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
        let mut by_file: HashMap<String, Vec<(&StringEntry, String)>> = HashMap::new();
        let mut string_entries: Vec<&StringEntry> = Vec::new();
        let mut strings_written = 0;
        let mut strings_skipped = 0;
        let mut files_written: Vec<PathBuf> = Vec::new();

        // Reserve IDs in script order before filtering translations. Otherwise
        // a missing/unchanged translation renumbers all later repeated lines.
        let mut dialogue_entries: Vec<_> = entries
            .iter()
            .enumerate()
            .filter(|(_, entry)| {
                entry
                    .tags
                    .iter()
                    .any(|tag| tag == "dialogue" || tag == "scroll_text" || tag == "menu")
                    && entry
                        .metadata
                        .get("label")
                        .and_then(|v| v.as_str())
                        .is_some()
            })
            .collect();
        dialogue_entries.sort_by_cached_key(|(_, entry)| {
            (
                entry.file_path.to_string_lossy().replace('\\', "/"),
                renpy_add_line_number(entry),
            )
        });
        let mut allocated = HashMap::new();
        let mut dialogue_ids = HashMap::new();
        for (index, entry) in dialogue_entries {
            let label = entry.metadata["label"].as_str().unwrap();
            let safe_source = canonical_say_string(&entry.source);
            let code = match &entry.context {
                Some(ch) => format!("{} {}", ch, safe_source),
                None => safe_source,
            };
            dialogue_ids.insert(
                index,
                renpy_numbered_translation_id(renpy_translation_id(label, &code), &mut allocated),
            );
        }

        for (index, entry) in entries.iter().enumerate() {
            let translation = match &entry.translation {
                Some(t) if t != &entry.source && !t.trim().is_empty() => t,
                _ => {
                    strings_skipped += 1;
                    continue;
                }
            };
            let _ = translation;

            // Dialogue entries (with known label) go into per-file translate blocks.
            if let Some(translation_id) = dialogue_ids.remove(&index) {
                let filename = entry
                    .file_path
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string();
                by_file
                    .entry(filename)
                    .or_default()
                    .push((entry, translation_id));
            } else {
                // UI labels, ui_label entries, and dialogue without known label
                // go into a strings block
                string_entries.push(entry);
            }
        }

        // Generate one .rpy file per source file with proper translate blocks
        let mut file_groups: Vec<_> = by_file.iter_mut().collect();
        file_groups.sort_by(|(a, _), (b, _)| a.cmp(b));
        for (filename, file_entries) in file_groups {
            file_entries.sort_by_cached_key(|(entry, _)| {
                (
                    entry.file_path.to_string_lossy().replace('\\', "/"),
                    renpy_add_line_number(entry),
                )
            });
            let mut lines = Vec::new();
            lines.push("# Auto-generated by Locust — Ren'Py translation file.".to_string());
            lines.push("# Format: `translate <lang> <label>_<md5(code + CRLF)[:8]>:`".to_string());
            lines.push(String::new());

            for (entry, translation_id) in file_entries {
                let translation = entry.translation.as_ref().unwrap();

                // Use the same canonical code for the ID and original comment.
                // Extraction does not retain say attributes or with/id/nointeract
                // clauses (nor other statements in a translation group), so these
                // entries can only use the best available character/text form.
                let safe_source = canonical_say_string(&entry.source);
                let orig_line = match &entry.context {
                    Some(ch) => format!("{} {}", ch, safe_source),
                    None => safe_source,
                };
                // Extract line number from entry ID (format: filename.rpy#N)
                let line_num = renpy_add_line_number(entry);

                // Reconstruct the translated "character text" or just "text" line.
                let safe_trans = escape_inner_quotes(translation);
                let trans_line = match &entry.context {
                    Some(ch) => format!("{} \"{}\"", ch, safe_trans),
                    None => format!("\"{}\"", safe_trans),
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
            files_written.push(tl_file.clone());
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

            let mut seen_sources: std::collections::HashSet<String> =
                std::collections::HashSet::new();
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
            files_written.push(tl_file.clone());
            let tl_rpyc = tl_file.with_extension("rpyc");
            if tl_rpyc.exists() {
                let _ = std::fs::remove_file(&tl_rpyc);
            }
        }

        // Create locust_languages.rpy with an in-game language picker.
        let langs_file_content = build_language_picker_script(&game_dir, lang);
        let langs_file = game_dir.join("locust_languages.rpy");
        std::fs::write(&langs_file, langs_file_content)?;
        files_written.push(langs_file.clone());
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
            skip_reasons: Default::default(),
            files_modified: by_file.len() + 1,
            strings_written,
            strings_skipped,
            warnings: Vec::new(),
            files_written,
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
    buttons.push_str(
        "            textbutton \"Original\" action Language(None) xalign 0.5 text_size 22\n",
    );
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
    #[test]
    fn revision_refuses_mutated_original_before_retargeting_renpy() {
        let dir = tempfile::tempdir().unwrap();
        let current = dir.path().join("script.rpy");
        let original = dir.path().join("original.rpy");
        let source = "label start:\n    \"Hello traveler\"\n";
        std::fs::write(&current, source).unwrap();
        std::fs::write(&original, source).unwrap();
        let plugin = RenPyPlugin::new();
        let mut entries = RenPyPlugin::extract_file(&current).unwrap();
        assert_eq!(entries.len(), 1);
        let expected_source = entries[0].source.clone();
        let originals = HashMap::from([(
            current.clone(),
            RevisionOriginal::capture(&original).unwrap(),
        )]);
        std::fs::write(&current, "label start:\n    \"Primera traducción\"\n").unwrap();
        for changed in [
            Some("label start:\n    \"Other traveler\"\n"),
            Some("label start:\n    \"Longer changed original\"\n"),
            Some("label start:\n    \"X\"\n"),
            None,
        ] {
            match changed {
                Some(text) => std::fs::write(&original, text).unwrap(),
                None => std::fs::remove_file(&original).unwrap(),
            }
            assert!(plugin
                .prepare_revision_entries(&mut entries, &originals)
                .is_err());
            assert_eq!(entries[0].source, expected_source);
            assert_eq!(
                std::fs::read_to_string(&current).unwrap(),
                "label start:\n    \"Primera traducción\"\n"
            );
        }
        std::fs::write(&original, source).unwrap();
        plugin
            .prepare_revision_entries(&mut entries, &originals)
            .unwrap();
        assert_eq!(entries[0].source, "Primera traducción");
    }

    #[test]
    fn direct_revision_preserves_other_lines_and_handles_quoted_text_and_identity() {
        let game = tempfile::tempdir().unwrap();
        let backup = tempfile::tempdir().unwrap();
        let current = game.path().join("script.rpy");
        let original = backup.path().join("script.rpy");
        let content = "label start:\n    \"Hello traveler\"\n    \"Hello traveler\"\n    return\n";
        std::fs::write(&current, content).unwrap();
        std::fs::write(&original, content).unwrap();
        let plugin = RenPyPlugin::new();
        let mut entries = RenPyPlugin::extract_file(&current).unwrap();
        assert_eq!(entries.len(), 2);
        let originals = HashMap::from([(
            current.clone(),
            RevisionOriginal::capture(&original).unwrap(),
        )]);
        entries[0].translation = Some("Primera frase".into());
        entries[1].translation = Some("Dijo \"hola\".".into());
        assert_eq!(
            plugin
                .inject(game.path(), &entries)
                .unwrap()
                .strings_written,
            2
        );
        let mut revised = vec![entries[1].clone()];
        revised[0].translation = Some("Frase corregida".into());
        plugin
            .prepare_revision_entries(&mut revised, &originals)
            .unwrap();
        assert_eq!(
            plugin
                .inject(game.path(), &revised)
                .unwrap()
                .strings_written,
            1
        );
        assert_eq!(
            std::fs::read_to_string(&current).unwrap(),
            "label start:\n    \"Primera frase\"\n    \"Frase corregida\"\n    return"
        );
        let mut revert = vec![entries[1].clone()];
        revert[0].translation = Some(revert[0].source.clone());
        plugin
            .prepare_revision_entries(&mut revert, &originals)
            .unwrap();
        assert_eq!(
            plugin.inject(game.path(), &revert).unwrap().strings_written,
            1
        );
        let expected = "label start:\n    \"Primera frase\"\n    \"Hello traveler\"\n    return";
        assert_eq!(std::fs::read_to_string(&current).unwrap(), expected);
        let mut fresh = RenPyPlugin::extract_file(&current).unwrap();
        for entry in &mut fresh {
            entry.translation = Some(entry.source.clone());
        }
        plugin
            .prepare_revision_entries(&mut fresh, &originals)
            .unwrap();
        assert_eq!(
            plugin.inject(game.path(), &fresh).unwrap().strings_written,
            0
        );
        assert_eq!(std::fs::read_to_string(&current).unwrap(), expected);
        assert_eq!(std::fs::read_to_string(original).unwrap(), content);

        let mut mixed = entries.clone();
        mixed[0].source = "Unknown original".into();
        mixed[0].translation = Some("Debe omitirse".into());
        mixed[1].translation = Some("Cambio permitido".into());
        plugin
            .prepare_revision_entries(&mut mixed, &originals)
            .unwrap();
        let report = plugin.inject(game.path(), &mixed).unwrap();
        assert_eq!(report.strings_written, 1);
        assert_eq!(report.skip_reasons.get("source_changed"), Some(&1));
        assert_eq!(
            std::fs::read_to_string(&current).unwrap(),
            "label start:\n    \"Primera frase\"\n    \"Cambio permitido\"\n    return"
        );
    }

    #[test]
    fn revision_does_not_retarget_a_changed_script_statement() {
        let game = tempfile::tempdir().unwrap();
        let backup = tempfile::tempdir().unwrap();
        let current = game.path().join("script.rpy");
        let original = backup.path().join("script.rpy");
        let content = "label start:\n    \"Hello traveler\"\n";
        std::fs::write(&current, content).unwrap();
        std::fs::write(&original, content).unwrap();
        let plugin = RenPyPlugin::new();
        let mut entries = RenPyPlugin::extract_file(&current).unwrap();
        entries[0].translation = Some("Texto corregido".into());
        let changed = "label start:\n    narrator \"Texto previo\"\n";
        std::fs::write(&current, changed).unwrap();
        plugin
            .prepare_revision_entries(
                &mut entries,
                &HashMap::from([(
                    current.clone(),
                    RevisionOriginal::capture(&original).unwrap(),
                )]),
            )
            .unwrap();
        assert_eq!(
            plugin
                .inject(game.path(), &entries)
                .unwrap()
                .strings_written,
            0
        );
        assert_eq!(std::fs::read_to_string(current).unwrap(), changed);
    }

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

    fn ast_text(text: &str) -> Vec<u8> {
        let mut p = vec![b'X'];
        p.extend_from_slice(&(text.len() as u32).to_le_bytes());
        p.extend_from_slice(text.as_bytes());
        p
    }

    fn ast_seq(items: Vec<Vec<u8>>, tuple: bool) -> Vec<u8> {
        let mut p = vec![b'('];
        for item in items {
            p.extend(item);
        }
        p.push(if tuple { b't' } else { b'l' });
        p
    }

    fn ast_global(module: &str, class: &str) -> Vec<u8> {
        format!("c{module}\n{class}\n").into_bytes()
    }

    fn ast_object(module: &str, class: &str, fields: Vec<(&str, Vec<u8>)>) -> Vec<u8> {
        let mut p = ast_global(module, class);
        p.extend_from_slice(&[b')', 0x81, b'}', b'(']); // NEWOBJ, dict state
        for (key, value) in fields {
            p.extend(ast_text(key));
            p.extend(value);
        }
        p.extend_from_slice(b"ub"); // SETITEMS, BUILD
        p
    }

    fn ast_expr(source: &str) -> Vec<u8> {
        let mut p = ast_global("renpy.ast", "PyExpr");
        p.extend(ast_text(source));
        p.extend_from_slice(&[0x85, 0x81]); // TUPLE1, NEWOBJ (str subclass)
        p
    }

    fn ast_say(text: &str) -> Vec<u8> {
        ast_object(
            "renpy.ast",
            "Say",
            vec![
                ("who", ast_text("screenInventory")),
                ("what", ast_text(text)),
                ("filename", ast_text("game/script.rpy")),
                (
                    "attributes",
                    ast_seq(vec![ast_text("Background sunset")], false),
                ),
            ],
        )
    }

    fn typed_ast_fixture() -> (Vec<u8>, Vec<&'static str>) {
        let screen = |name: &str, displayable: &str, expr: &str, tooltip: &str| {
            ast_object(
                "renpy.sl2.slast",
                "SLDisplayable",
                vec![
                    ("name", ast_text(name)),
                    ("displayable", ast_global("renpy.ui", displayable)),
                    ("positional", ast_seq(vec![ast_expr(expr)], false)),
                    (
                        "keyword",
                        ast_seq(
                            vec![
                                ast_seq(vec![ast_text("tooltip"), ast_expr(tooltip)], true),
                                ast_seq(
                                    vec![ast_text("action"), ast_expr("ShowMenu('non text ui')")],
                                    true,
                                ),
                                ast_seq(
                                    vec![ast_text("idle"), ast_expr("'images/button.png'")],
                                    true,
                                ),
                            ],
                            false,
                        ),
                    ),
                ],
            )
        };
        let exact = "  A\\B \"quoted\"\n\tline  ";
        let mut p = vec![0x80, 2];
        p.extend(ast_seq(
            vec![
                ast_object(
                    "renpy.ast",
                    "Python",
                    vec![(
                        "code",
                        ast_object(
                            "renpy.ast",
                            "PyCode",
                            vec![
                                ("source", ast_text("Preference(\"text speed\")")),
                                ("block", ast_seq(vec![ast_say("Hidden in code")], false)),
                            ],
                        ),
                    )],
                ),
                ast_object(
                    "renpy.ast",
                    "Image",
                    vec![("imgname", ast_text("Background sunset"))],
                ),
                ast_say("{i}Hm.{/i}"),
                ast_say("No."),
                ast_say("Yes/No?"),
                ast_say(exact),
                ast_say("No."),
                ast_object(
                    "renpy.ast",
                    "Menu",
                    vec![
                        ("set", ast_expr("menu_seen")),
                        (
                            "items",
                            ast_seq(
                                vec![
                                    ast_seq(
                                        vec![
                                            ast_text("Choose"),
                                            ast_expr("flag is True"),
                                            vec![b'N'],
                                        ],
                                        true,
                                    ),
                                    ast_seq(
                                        vec![
                                            ast_text("Go"),
                                            ast_expr("flag is False"),
                                            ast_seq(vec![ast_say("Branch")], false),
                                        ],
                                        true,
                                    ),
                                ],
                                false,
                            ),
                        ),
                    ],
                ),
                ast_object(
                    "renpy.ast",
                    "Screen",
                    vec![(
                        "screen",
                        ast_object(
                            "renpy.sl2.slast",
                            "SLScreen",
                            vec![(
                                "children",
                                ast_seq(
                                    vec![
                                        screen(
                                            "textbutton",
                                            "_textbutton",
                                            "_('Back')",
                                            "__('Help\\nnow')",
                                        ),
                                        screen("label", "_label", "'Status'", "variable"),
                                        screen("text", "_textbutton", "player.name", "variable"),
                                        ast_object(
                                            "renpy.sl2.slast",
                                            "SLDisplayable",
                                            vec![
                                                (
                                                    "displayable",
                                                    ast_global("renpy.text.text", "Text"),
                                                ),
                                                (
                                                    "positional",
                                                    ast_seq(
                                                        vec![ast_expr("u'\\u00a1Hello!'")],
                                                        false,
                                                    ),
                                                ),
                                            ],
                                        ),
                                    ],
                                    false,
                                ),
                            )],
                        ),
                    )],
                ),
                ast_object(
                    "renpy.ast",
                    "Translate",
                    vec![("block", ast_seq(vec![ast_say("Translated block")], false))],
                ),
                ast_object(
                    "renpy.ast",
                    "TranslateString",
                    vec![
                        ("old", ast_text("Original UI")),
                        ("new", ast_text("Not source text")),
                    ],
                ),
                ast_object(
                    "renpy.ast",
                    "UserStatement",
                    vec![("line", ast_text("show screen screenInventory"))],
                ),
            ],
            false,
        ));
        p.push(b'.');
        (
            p,
            vec![
                "{i}Hm.{/i}",
                "No.",
                "Yes/No?",
                exact,
                "Choose",
                "Go",
                "Branch",
                "Back",
                "Help\nnow",
                "Status",
                "¡Hello!",
                "Translated block",
                "Original UI",
            ],
        )
    }

    #[test]
    fn test_harvest_rpyc_strings_typed_visible_fields() {
        let (pickle, expected) = typed_ast_fixture();
        let got = harvest_rpyc_strings(&wrap_pickle_as_rpyc(&pickle));
        assert_eq!(got, expected);
        // Injection must see the untouched what, including leading/trailing
        // whitespace, a literal backslash, quotes, a newline and a tab.
        assert_eq!(got[3], "  A\\B \"quoted\"\n\tline  ");
        let mut empty_say = vec![0x80, 2];
        empty_say.extend(ast_seq(vec![ast_say("")], false));
        empty_say.push(b'.');
        assert_eq!(harvest_rpyc_strings(&wrap_pickle_as_rpyc(&empty_say)), [""]);
    }

    #[test]
    fn test_harvest_rpyc_strings_conservative_fallback() {
        let mut p = tiny_pickle(&[
            "Welcome to Area 69!",
            "screenInventory",
            "Background sunset",
            "Preference(\"text speed\")",
            "show screen inventory",
            "game/script.rpy",
            "screen inventory.png",
            "flag is True",
            "player + bonus",
        ]);
        // Force unsupported-opcode failure after all the text was seen.
        *p.last_mut().unwrap() = 0xff;
        assert_eq!(
            harvest_rpyc_strings(&wrap_pickle_as_rpyc(&p)),
            ["Welcome to Area 69!"]
        );
    }

    #[test]
    fn test_harvest_rpyc_strings_malformed_pickle_never_panics() {
        let (pickle, _) = typed_ast_fixture();
        for n in 0..pickle.len() {
            assert!(
                crate::renpy_pickle::visible_text(&pickle[..n]).is_err(),
                "prefix {n}"
            );
            harvest_rpyc_strings(&wrap_pickle_as_rpyc(&pickle[..n]));
        }
        let mut seed = 0x98_1234u32;
        for n in 0..512 {
            let bytes: Vec<_> = (0..n)
                .map(|_| {
                    seed ^= seed << 13;
                    seed ^= seed >> 17;
                    seed ^= seed << 5;
                    seed as u8
                })
                .collect();
            let _ = crate::renpy_pickle::visible_text(&bytes);
            harvest_rpyc_strings(&wrap_pickle_as_rpyc(&bytes));
        }
        for bytes in [
            vec![0x80, 2, b'X', 255, 255, 255, 255], // giant claimed string
            vec![0x80, 2, b'N', b'r', 255, 255, 255, 255], // sparse memo bomb
            vec![0x80, 2, b'j', 255, 255, 255, 255], // invalid memo reference
        ] {
            assert!(crate::renpy_pickle::visible_text(&bytes).is_err());
            assert!(harvest_rpyc_strings(&wrap_pickle_as_rpyc(&bytes)).is_empty());
        }
        let mut deep = vec![0x80, 2, b'N'];
        deep.extend(std::iter::repeat_n(0x85, 300)); // nested tuples
        deep.push(b'.');
        assert_eq!(
            crate::renpy_pickle::visible_text(&deep),
            Err("graph walk limit")
        );
        let mut stack = vec![0x80, 2];
        stack.extend(std::iter::repeat_n(b'N', 32_769));
        assert_eq!(
            crate::renpy_pickle::visible_text(&stack),
            Err("stack limit")
        );
    }

    #[test]
    fn test_harvest_rpyc_strings_memo_identity_and_slot_state() {
        let mut p = vec![0x80, 2, b']', b'q', 0, b'('];
        p.extend(ast_global("renpy.ast", "Say"));
        // Memoize the object BEFORE BUILD, as Python's real pickler does.
        p.extend_from_slice(&[b')', 0x81, b'q', 1, b'N', b'}', b'q', 2, b'(']);
        p.extend(ast_text("what"));
        p.extend(ast_text("{i}Memo{/i}"));
        p.extend_from_slice(&[b'q', 3, b'u', 0x86, b'b']); // (None, slotdict) state
        p.extend_from_slice(&[b'h', 1]);
        p.extend_from_slice(b"g1\n");
        p.push(b'j');
        p.extend_from_slice(&1u32.to_le_bytes());
        p.extend_from_slice(b"e.");
        assert_eq!(
            harvest_rpyc_strings(&wrap_pickle_as_rpyc(&p)),
            ["{i}Memo{/i}"]
        );
        // A cyclic memo-shared container must terminate too.
        assert_eq!(
            crate::renpy_pickle::visible_text(&[0x80, 2, b']', b'q', 0, b'h', 0, b'a', b'.']),
            Ok(Vec::new())
        );
    }

    /// Multi-member version of the protocol-2 RPA fixture in
    /// tests/renpy_ui_injection.rs, preserving the supplied index order.
    fn build_rpa(files: &[(&str, Vec<u8>)]) -> Vec<u8> {
        const KEY: i64 = 0x42424242;
        let placeholder = format!("RPA-3.0 {:016x} {:08x}\n", 0u64, KEY);
        let mut archive = placeholder.into_bytes();
        let mut pickle = vec![0x80, 0x02, b'}', b'q', 0, b'('];
        for (name, content) in files {
            pickle.push(b'X'); // BINUNICODE
            pickle.extend_from_slice(&(name.len() as u32).to_le_bytes());
            pickle.extend_from_slice(name.as_bytes());
            // Memo slots can be reused: no member refers to an earlier one.
            pickle.extend_from_slice(&[b'q', 1, b']', b'q', 2]);
            let offset = archive.len() as i64 ^ KEY;
            let size = ((64 - (offset as u64).leading_zeros()) / 8 + 1) as usize;
            pickle.extend_from_slice(&[0x8a, size as u8]); // LONG1
            pickle.extend_from_slice(&offset.to_le_bytes()[..size]);
            pickle.push(b'J'); // BININT
            pickle.extend_from_slice(&((content.len() as i64 ^ KEY) as i32).to_le_bytes());
            // Empty prefix, BINPUT, TUPLE3, APPEND.
            pickle.extend_from_slice(&[b'U', 0, b'q', 3, 0x87, b'a']);
            archive.extend_from_slice(content);
        }
        pickle.extend_from_slice(b"u."); // SETITEMS, STOP
        let header = format!("RPA-3.0 {:016x} {:08x}\n", archive.len(), KEY);
        archive[..header.len()].copy_from_slice(header.as_bytes());
        archive.extend_from_slice(&miniz_oxide::deflate::compress_to_vec_zlib(&pickle, 6));
        archive
    }

    #[test]
    fn rpa_rpyc_twin_lookup_uses_the_name_set() {
        let mut names = HashSet::from([
            "scripts/exact.rpy",
            "scripts/case.rpy",
            "other/path.rpy",
            "scripts/extension.RPY",
            "scripts/café.rpy",
        ]);
        for (name, expected) in [
            ("scripts/exact.rpyc", true),
            ("scripts/Case.rpyc", false),
            ("scripts/path.rpyc", false),
            ("scripts/extension.rpyc", false),
            ("scripts/lone.rpyc", false),
            ("scripts/café.rpyc", true),
            ("scripts/exact.rpy", false),
            ("", false),
        ] {
            assert_eq!(RenPyPlugin::has_rpy_twin(&names, name), expected, "{name}");
        }
        names.remove("scripts/exact.rpy");
        assert!(!RenPyPlugin::has_rpy_twin(&names, "scripts/exact.rpyc"));
        names.insert("scripts/lone.rpy");
        assert!(RenPyPlugin::has_rpy_twin(&names, "scripts/lone.rpyc"));
    }

    #[test]
    fn rpa_rpyc_twins_require_exact_names_in_either_index_order() {
        let mut files = vec![
            (
                "scripts/exact.rpyc",
                tiny_rpyc(&["Skip this compiled twin!"]),
            ),
            ("images/title.png", b"image bytes".to_vec()),
            ("scripts/Case.rpyc", tiny_rpyc(&["Keep the case mismatch!"])),
            ("scripts/path.rpyc", tiny_rpyc(&["Keep the path mismatch!"])),
            ("scripts/lone.rpyc", tiny_rpyc(&["Keep the lone script!"])),
            ("scripts/case.rpy", b"\"Case source!\"\n".to_vec()),
            ("other/path.rpy", b"\"Other path source!\"\n".to_vec()),
            ("scripts/exact.rpy", b"\"Exact source!\"\n".to_vec()),
            ("audio/theme.ogg", b"audio bytes".to_vec()),
        ];
        let mut expected = vec![
            ("scripts/Case.rpyc", "Keep the case mismatch!"),
            ("scripts/path.rpyc", "Keep the path mismatch!"),
            ("scripts/lone.rpyc", "Keep the lone script!"),
            ("scripts/case.rpy", "Case source!"),
            ("other/path.rpy", "Other path source!"),
            ("scripts/exact.rpy", "Exact source!"),
        ];
        for reverse in [false, true] {
            if reverse {
                files.reverse();
                expected.reverse();
            }
            let dir = tempfile::tempdir().unwrap();
            let archive = dir.path().join("scripts.rpa");
            let output = dir.path().join("extracted");
            fs::write(&archive, build_rpa(&files)).unwrap();

            let extracted = RenPyPlugin::extract_rpa(&archive, &output).unwrap();
            assert_eq!(
                extracted,
                expected
                    .iter()
                    .map(|(name, _)| output.join(name))
                    .collect::<Vec<_>>()
            );
            assert!(!output.join("scripts/exact.rpyc").exists());
            assert!(!output.join("images/title.png").exists());
            assert!(!output.join("audio/theme.ogg").exists());
            for (name, _) in &expected {
                let (_, bytes) = files.iter().find(|(member, _)| member == name).unwrap();
                assert_eq!(fs::read(output.join(name)).unwrap(), *bytes);
            }

            let entries = RenPyPlugin::new().extract(&archive).unwrap();
            assert_eq!(entries.len(), expected.len());
            for (entry, (_, source)) in entries.iter().zip(&expected) {
                assert_eq!(entry.source, *source);
                assert_eq!(entry.file_path, archive);
            }
        }
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

        let content =
            fs::read_to_string(dir.join("game").join("zzz_locust_translate.rpy")).unwrap();
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

        let content =
            fs::read_to_string(dir.join("game").join("zzz_locust_translate.rpy")).unwrap();
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
        fs::write(
            game_dir.join("zzz_locust_translate.rpyc"),
            b"stale compiled twin",
        )
        .unwrap();

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
        let dir =
            std::env::temp_dir().join(format!("locust_rpycf_reconcile_{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(dir.join("game")).unwrap();

        let mut translated =
            StringEntry::new("scripts.rpa#a.rpyc#s0", "Hello!", dir.join("scripts.rpa"));
        translated.tags = vec!["dialogue".to_string(), "rpyc".to_string()];
        translated.translation = Some("¡Hola!".to_string());

        let mut missing =
            StringEntry::new("scripts.rpa#a.rpyc#s1", "Bye!", dir.join("scripts.rpa"));
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
    fn path_heuristic_regression_text_and_resources() {
        for text in [
            "{i}tap-tap-tap!{/i}",
            "{b}Wow!{/b}",
            "{color=#f00}Stop{/color}",
            r#"She said \"no\""#,
            r#"She said \\"no\\""#,
            r"Line one\nLine two",
            "{w}...{/w}",
            r#"\"no\""#,
            r"Line\nTwo",
            r"Back\\slash",
            r"It\'s",
            r"One\ two",
            "{i}either/or{/i}",
            "yes/no",
            "1/2",
            "gui/[theme]/button",
            "gui/100%/button",
            "gui/foo\tbar",
            "gui/foo\u{a0}bar",
            "{i/gui",
            "gui/foo}",
            "{{gui/foo}}",
        ] {
            assert!(
                !is_file_reference(text),
                "Dialogue rejected as path: {text:?}"
            );
        }
        for path in [
            "gui/button.png",
            "images/bg/room.jpg",
            "audio/sfx/click.ogg",
            r"C:\games\x\y.txt",
            "scripts/label.rpy",
            "#ff00ff",
            "gui/button",
            "images/bg/room",
            "audio/sfx/click",
            "video/intro",
            "fonts/body",
            "scripts/label",
            "./cache/item",
            "../cache/item",
            "/cache/item",
            "packs/chapter.data",
            r"C:\games\x\y",
            "{i}images/bg/room{/i}",
        ] {
            assert!(
                is_file_reference(path),
                "Resource accepted as text: {path:?}"
            );
        }
    }

    #[test]
    fn path_heuristic_regression_extract_content() {
        let script = r##"define button = "gui/button.png"
define room = "images/bg/room.jpg"
define click = "audio/sfx/click.ogg"
define save = "C:\games\x\y.txt"
define source = "scripts/label.rpy"
define tint = "#ff00ff"
define button_dir = "gui/button"
image room = "images/bg/room.jpg"
image button = "gui/button.png"
label path_dialogue:
    "{i}tap-tap-tap!{/i}"
    e "{b}Wow!{/b}"
    "{color=#f00}Stop{/color}"
    e "She said \"no\""
    "Line one\nLine two"
    e "{w}...{/w}"
    "\"no\""
    e "Line\nTwo"
    "Back\\slash"
    e "It\'s"
    "One\ two"
    return
"##;
        let entries = RenPyPlugin::extract_content(Path::new("script.rpy"), script);
        let expected = [
            ("{i}tap-tap-tap!{/i}", None),
            ("{b}Wow!{/b}", Some("e")),
            ("{color=#f00}Stop{/color}", None),
            (r#"She said \"no\""#, Some("e")),
            (r"Line one\nLine two", None),
            ("{w}...{/w}", Some("e")),
            (r#"\"no\""#, None),
            (r"Line\nTwo", Some("e")),
            (r"Back\\slash", None),
            (r"It\'s", Some("e")),
            (r"One\ two", None),
        ];
        assert_eq!(
            entries.len(),
            expected.len(),
            "Extracted entries: {entries:?}"
        );
        for (index, (entry, (text, speaker))) in entries.iter().zip(expected).enumerate() {
            assert_eq!(entry.source, text);
            assert_eq!(entry.context.as_deref(), speaker, "{text}");
            assert_eq!(entry.tags, ["dialogue"], "{text}");
            assert_eq!(entry.metadata["label"], "path_dialogue", "{text}");
            assert_eq!(entry.id, format!("script.rpy#{}", index + 11));
        }
    }

    #[test]
    fn test_extract_say_statements() {
        let plugin = RenPyPlugin::new();
        let entries = plugin.extract(&fixture_dir()).unwrap();
        let hello = entries.iter().find(|e| e.source == "Hello, world!");
        assert!(
            hello.is_some(),
            "entries: {:?}",
            entries
                .iter()
                .map(|e| (&e.id, &e.source))
                .collect::<Vec<_>>()
        );
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

    fn assert_menu_dialogue(
        entries: &[StringEntry],
        text: &str,
        speaker: Option<&str>,
        label: &str,
    ) {
        let entry = entries
            .iter()
            .find(|entry| entry.source == text)
            .unwrap_or_else(|| panic!("Missing dialogue: {text}"));
        assert_eq!(entry.context.as_deref(), speaker, "{text}");
        assert_eq!(entry.tags, ["dialogue"], "{text}");
        assert_eq!(entry.metadata["label"], label, "{text}");
    }

    #[test]
    fn menu_regression_branch_say_and_add_blocks() {
        let dir = tempfile::tempdir().unwrap();
        let game = dir.path().join("game");
        fs::create_dir(&game).unwrap();
        let script = game.join("script.rpy");
        fs::write(
            &script,
            r#"define e = Character("Eileen")
label start:
    menu:
        "Choose a route"
        "First":
            e "A visible branch line."
            "A visible narration line."
        "Second":
            e "Another visible branch line."
    e "An outside line."
"#,
        )
        .unwrap();
        let mut entries = RenPyPlugin::extract_file(&script).unwrap();
        assert_eq!(entries.len(), 8);
        for (text, speaker, line) in [
            ("A visible branch line.", Some("e"), 6),
            ("A visible narration line.", None, 7),
            ("Another visible branch line.", Some("e"), 9),
            ("An outside line.", Some("e"), 10),
        ] {
            assert_menu_dialogue(&entries, text, speaker, "start");
            let entry = entries.iter().find(|entry| entry.source == text).unwrap();
            assert_eq!(entry.id, format!("script.rpy#{line}"));
        }
        for text in ["Choose a route", "First", "Second"] {
            let entry = entries.iter().find(|entry| entry.source == text).unwrap();
            assert_eq!(entry.tags, ["menu"]);
            assert!(entry.context.is_none());
            assert!(!entry.metadata.contains_key("label"));
        }

        for entry in &mut entries {
            entry.translation = Some(format!("Translated {}", entry.source));
        }
        RenPyPlugin::new()
            .inject_add(dir.path(), "es", &entries)
            .unwrap();
        let blocks = fs::read_to_string(game.join("tl/es/script.rpy")).unwrap();
        assert_eq!(blocks.matches("translate es start_").count(), 4);
        for code in [
            r#"e "A visible branch line.""#,
            r#""A visible narration line.""#,
            r#"e "Another visible branch line.""#,
            r#"e "An outside line.""#,
        ] {
            assert!(blocks.contains(&format!("translate es start_{}:", expected_say_hash(code))));
            assert!(blocks.contains(&format!("    # {code}\n")));
        }
        let strings = fs::read_to_string(game.join("tl/es/locust_strings.rpy")).unwrap();
        for text in ["Choose a route", "First", "Second"] {
            assert!(strings.contains(&format!("    old \"{text}\"\n")));
        }
    }

    #[test]
    fn menu_regression_nested_and_control_flow_scopes() {
        let content = r#"label start:
    if flag:
        menu:
            "Outer":
                e "Before inner."
                menu:
                    "Inner":
                        e "Inner branch."
                        "Inner narration."
                "After inner."
            "Other":
                e "Other branch."
        "After outer."
    while flag:
        menu:
            "Loop":
                e "Loop branch."
        e "After loop menu."
    menu:
        "Last":
            e "Last branch."
    return
label next:
    "Next label."
"#;
        let entries = RenPyPlugin::extract_content(Path::new("script.rpy"), content);
        for (text, speaker) in [
            ("Before inner.", Some("e")),
            ("Inner branch.", Some("e")),
            ("Inner narration.", None),
            ("After inner.", None),
            ("Other branch.", Some("e")),
            ("After outer.", None),
            ("Loop branch.", Some("e")),
            ("After loop menu.", Some("e")),
            ("Last branch.", Some("e")),
        ] {
            assert_menu_dialogue(&entries, text, speaker, "start");
        }
        assert_menu_dialogue(&entries, "Next label.", None, "next");
        assert_eq!(entries.len(), 15);
    }

    #[test]
    fn menu_regression_tabs_custom_indents_and_adjacent_menus() {
        // Exercise both tabs (counted as one, like Python blocks) and widths
        // other than four spaces; choices establish their actual child level.
        for unit in ["\t", "  ", "   "] {
            let content = [
                "label start:",
                "@menu:",
                "@@\"First\":",
                "@@@e \"First branch.\"",
                "@menu:",
                "@@\"Second\":",
                "@@@e \"Second branch.\"",
                "@@@\"Branch narration.\"",
                "@\"Outside narration.\"",
                "@menu:",
                "@@\"Third\":",
                "@@@jump done",
                "@jump done",
                "label done:",
                "@\"Done.\"",
            ]
            .join("\n")
            .replace('@', unit);
            let entries = RenPyPlugin::extract_content(Path::new("script.rpy"), &content);
            for (text, speaker) in [
                ("First branch.", Some("e")),
                ("Second branch.", Some("e")),
                ("Branch narration.", None),
                ("Outside narration.", None),
            ] {
                assert_menu_dialogue(&entries, text, speaker, "start");
            }
            assert_menu_dialogue(&entries, "Done.", None, "done");
            assert_eq!(entries.len(), 8);
            assert_eq!(entries[2].source, "Second");
            assert_eq!(entries[2].tags, ["menu"]);
        }
    }

    #[test]
    fn menu_regression_conditional_and_argument_choices() {
        for (line, expected) in [
            (r#""Go" if flag:"#, "Go"),
            (r#""Stay" (x=1):"#, "Stay"),
            (r#""Both" if a (x=1):"#, "Both"),
            (r#""Reverse" (x=1) if flag:"#, "Reverse"),
            (
                r#""Nested condition" (x=1) if (flag) and check(x):"#,
                "Nested condition",
            ),
            (
                r#""Complex" if values["a:b"] and {'x': 1}: # comment"#,
                "Complex",
            ),
            (r#""Args" (x=(1, 2), y="a:#b"):"#, "Args"),
            (r#""Escaped \"choice\"" if flag:"#, r#"Escaped \"choice\""#),
        ] {
            assert_eq!(extract_menu_choice(line), Some(expected), "{line}");
        }
        for line in [
            r#""Caption""#,
            r#"e "Text" if x:"#,
            r#""Missing colon" if flag"#,
            r#""Empty condition" if :"#,
            r#""Unknown suffix" nonsense:"#,
            r#""Trailing code": pass"#,
            r#""Unbalanced" (x=1:"#,
            r#""Bad delimiter" (x=1]:"#,
            r#""Bad args suffix" (x=1) nonsense:"#,
            r#""Unclosed quote" if x == "oops:"#,
            r#""Comment colon" if flag # :"#,
            r#""" if flag:"#,
        ] {
            assert_eq!(extract_menu_choice(line), None, "{line}");
        }
        let content = r#"label start:
    menu (screen="x"):
        set seen
        with dissolve
        "Go" if flag:
            e "Go branch."
        "Stay" (x=1):
            e "Stay branch."
        "Both" if a (x=1):
            e "Both branch."
    e "Text" if x:
"#;
        let entries = RenPyPlugin::extract_content(Path::new("script.rpy"), content);
        assert_eq!(entries.len(), 7);
        for text in ["Go", "Stay", "Both"] {
            let entry = entries.iter().find(|entry| entry.source == text).unwrap();
            assert_eq!(entry.tags, ["menu"]);
            assert!(!entry.metadata.contains_key("label"));
            assert_menu_dialogue(&entries, &format!("{text} branch."), Some("e"), "start");
        }
        assert_menu_dialogue(&entries, "Text", Some("e"), "start");
    }

    #[test]
    fn menu_regression_python_comments_and_normal_extractors() {
        let content = r#"label start:
    menu:

# Unindented comments do not end menus.
        # A comment does not establish the choice indent.
        "Choice":
            python:
                "Hidden Python literal."
                value = "Hidden assigned literal."
                translated = _("Python UI")

# A comment inside Python does not end its block either.
                "Still hidden."
            e "Visible after Python."
            "Visible branch narration."
            $ label = _("Branch UI")
        "Sibling":
            pass
    "Outside."
"#;
        let entries = RenPyPlugin::extract_content(Path::new("script.rpy"), content);
        assert_eq!(entries.len(), 7);
        for text in [
            "Hidden Python literal.",
            "Hidden assigned literal.",
            "Still hidden.",
        ] {
            assert!(!entries.iter().any(|entry| entry.source == text), "{text}");
        }
        for text in ["Python UI", "Branch UI"] {
            let entry = entries.iter().find(|entry| entry.source == text).unwrap();
            assert_eq!(entry.tags, ["ui_label"]);
        }
        assert_menu_dialogue(&entries, "Visible after Python.", Some("e"), "start");
        assert_menu_dialogue(&entries, "Visible branch narration.", None, "start");
        assert_menu_dialogue(&entries, "Outside.", None, "start");
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

    fn python_block_headers() -> Vec<String> {
        let mut headers = Vec::new();
        for prefix in [
            "python",
            "init python",
            "init -1 python",
            "init 1000 python",
        ] {
            for modifiers in [
                "",
                " early",
                " hide",
                " in mystore",
                " early hide",
                " early in phone.config",
                " hide in _viewers",
                " early hide in phone.config",
            ] {
                headers.push(format!("{prefix}{modifiers}:"));
            }
        }
        headers.extend(
            [
                "init +1 python:",
                "translate japanese python:",
                "  init\t1000\tpython\thide :  ",
                "python early in phone.config: # Named store",
            ]
            .map(str::to_string),
        );
        headers
    }

    #[test]
    fn python_block_header_variants_skip_literals_keep_i18n_and_end_at_dedent() {
        for header in python_block_headers() {
            let script = format!(
                "{header}\n\
                 \x20   x = \"Hello, this is a long sentence.\"\n\
                 \x20   \"Hidden Python literal.\"\n\
                 \n\
                 # A dedented comment does not close Python.\n\
                 \x20   \"Still inside Python.\"\n\
                 \x20   translated = _(\"Translatable\")\n\
                 e \"Actual dialogue after Python.\"\n"
            );
            let entries = RenPyPlugin::extract_content(Path::new("script.rpy"), &script);
            let rows: Vec<_> = entries
                .iter()
                .map(|entry| {
                    (
                        entry.id.as_str(),
                        entry.source.as_str(),
                        entry.tags.as_slice(),
                    )
                })
                .collect();
            assert_eq!(
                rows,
                [
                    (
                        "script.rpy#7",
                        "Translatable",
                        &["ui_label".to_string()][..]
                    ),
                    (
                        "script.rpy#8",
                        "Actual dialogue after Python.",
                        &["dialogue".to_string()][..],
                    ),
                ],
                "{header}"
            );
            assert_eq!(entries[1].context.as_deref(), Some("e"), "{header}");
        }
    }

    #[test]
    fn python_block_header_variants_allow_following_menu() {
        for header in python_block_headers() {
            let header = header.trim();
            // Exercise spaces and tabs with the header inside a label.
            for unit in ["    ", "\t"] {
                let script = format!(
                    "label start:\n\
                     @{header}\n\
                     @@\"Hidden Python literal.\"\n\
                     @menu:\n\
                     @@\"Choice\":\n\
                     @@@e \"Visible branch.\"\n\
                     @e \"Visible after menu.\"\n"
                )
                .replace('@', unit);
                let entries = RenPyPlugin::extract_content(Path::new("script.rpy"), &script);
                assert_eq!(entries.len(), 3, "{header}, indent {unit:?}: {entries:?}");
                assert_eq!(entries[0].source, "Choice", "{header}");
                assert_eq!(entries[0].tags, ["menu"], "{header}");
                assert_menu_dialogue(&entries, "Visible branch.", Some("e"), "start");
                assert_menu_dialogue(&entries, "Visible after menu.", Some("e"), "start");
            }
        }
    }

    #[test]
    fn python_block_header_requires_complete_grammar() {
        for line in [
            "init:",
            "init 1000:",
            "init -1:",
            "python",
            "python: trailing code",
            "python_early:",
            "python unknown:",
            "python in:",
            "python in 123:",
            "python in store..nested:",
            "python in store extra:",
            "python hide early:",
            "python early early:",
            "init 1.5 python:",
            "init --1 python:",
            "init -1 not_python:",
            "translate python:",
            "translate japanese strings:",
            "# python:",
            "e \"python:\"",
            "init -1 define text = \"python:\"",
        ] {
            assert!(!is_python_block_header(line), "{line}");
        }
    }

    #[test]
    fn python_block_dedent_to_plain_init_preserves_normal_extractors() {
        let script = r#"python hide:
    "Hidden Python literal."
init 10:
    define gui.title = _("Init title")
    default setting = "Hidden default literal."
    $ text = "Hidden one-line literal."
    $ translated = _("One-line UI")
    python in mystore:
        "Hidden nested Python literal."
label start:
    e "Visible dialogue."
"#;
        let entries = RenPyPlugin::extract_content(Path::new("script.rpy"), script);
        assert_eq!(entries.len(), 3, "{entries:?}");
        assert_eq!(entries[0].source, "Init title");
        assert_eq!(entries[0].tags, ["ui_label"]);
        assert_eq!(entries[1].source, "One-line UI");
        assert_eq!(entries[1].tags, ["ui_label"]);
        assert_menu_dialogue(&entries, "Visible dialogue.", Some("e"), "start");
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
    fn test_inject_reports_the_paths_it_wrote() {
        // `locust patch` packs from the paths injection reports, because for
        // archive-shipped games the written files never become database
        // entries: extraction skips `zzz_locust*` and rewrites entry paths to
        // the .rpa. Both the in-place loose edit and the generated runtime
        // filter must therefore be reported.
        let dir = temp_renpy_dir();
        let plugin = RenPyPlugin::new();
        let mut entries = plugin.extract(&dir).unwrap();
        for entry in &mut entries {
            if entry.source == "Hello, world!" {
                entry.translation = Some("Hola, mundo!".to_string());
            }
        }
        let mut rpyc = StringEntry::new(
            "scripts.rpa#a.rpyc#s0",
            "Compiled line",
            dir.join("game").join("scripts.rpa"),
        );
        rpyc.tags = vec!["dialogue".to_string(), "rpyc".to_string()];
        rpyc.translation = Some("Línea compilada".to_string());
        entries.push(rpyc);

        let report = plugin.inject(&dir, &entries).unwrap();

        let script = dir.join("game").join("script.rpy");
        let filter = dir.join("game").join("zzz_locust_translate.rpy");
        assert!(
            report.files_written.contains(&script),
            "the loose .rpy edited in place must be reported, got: {:?}",
            report.files_written
        );
        assert!(
            report.files_written.contains(&filter),
            "the generated runtime filter must be reported, got: {:?}",
            report.files_written
        );
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
    fn test_reextract_after_inject_add_ignores_generated_language_picker() {
        // Add mode writes game/locust_languages.rpy (an in-game language menu).
        // Reopening the project must not import its labels as game strings.
        let dir = temp_renpy_dir();
        let plugin = RenPyPlugin::new();
        let mut entries = plugin.extract(&dir).unwrap();
        let before: Vec<String> = entries.iter().map(|e| e.id.clone()).collect();
        for entry in &mut entries {
            entry.translation = Some(format!("[es] {}", entry.source));
        }
        plugin.inject_add(&dir, "es", &entries).unwrap();
        // A file left by an older Locust version is skipped too.
        fs::write(
            dir.join("game").join("locust_language.rpy"),
            "screen locust_old():\n    text \"Old picker\"\n",
        )
        .unwrap();
        assert!(dir.join("game").join("locust_languages.rpy").exists());

        let after: Vec<String> = plugin
            .extract(&dir)
            .unwrap()
            .into_iter()
            .map(|e| e.id)
            .collect();
        assert_eq!(after, before);
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
        let content = fs::read_to_string(tl_dir.join("script.rpy")).unwrap();
        let mut expected = String::from(
            "# Auto-generated by Locust — Ren'Py translation file.\n\
             # Format: `translate <lang> <label>_<md5(code + CRLF)[:8]>:`\n\n",
        );
        for (line, code, translation) in [
            (2, r#"e "Hello, world!""#, r#"e "[es] Hello, world!""#),
            (
                3,
                r#""This is the narrator speaking.""#,
                r#""[es] This is the narrator speaking.""#,
            ),
            (9, r#"e "Goodbye!""#, r#"e "[es] Goodbye!""#),
        ] {
            expected.push_str(&format!(
                "# game/script.rpy:{line}\ntranslate es start_{}:\n\n    # {code}\n    {translation}\n\n",
                expected_say_hash(code)
            ));
        }
        expected.pop(); // The writer joins a final empty line, leaving one trailing newline.
        assert_eq!(content, expected);
        let strings = fs::read_to_string(tl_dir.join("locust_strings.rpy")).unwrap();
        assert!(strings.contains("    old \"Go left\"\n    new \"[es] Go left\"\n"));
        assert!(strings.contains("    old \"Go right\"\n    new \"[es] Go right\"\n"));
    }

    fn expected_say_hash(code: &str) -> String {
        use md5::{Digest, Md5};

        // Independently hash the complete test statement and CRLF, not the source text.
        format!("{:x}", Md5::digest(format!("{code}\r\n").as_bytes()))[..8].to_string()
    }

    #[test]
    fn add_dialogue_uses_renpy_translation_id() {
        for (statement, hash) in [
            (r#""Hello.""#, "09e39b23".to_string()),
            (r#"e "Hello.""#, expected_say_hash(r#"e "Hello.""#)),
        ] {
            let dir = tempfile::tempdir().unwrap();
            let game = dir.path().join("game");
            fs::create_dir(&game).unwrap();
            fs::write(
                game.join("script.rpy"),
                format!("label start:\n    {statement}\n"),
            )
            .unwrap();
            let plugin = RenPyPlugin::new();
            let mut entries = plugin.extract(dir.path()).unwrap();
            assert_eq!(entries.len(), 1);
            entries[0].translation = Some("Hola.".to_string());

            let report = plugin.inject_add(dir.path(), "es", &entries).unwrap();
            let content = fs::read_to_string(game.join("tl/es/script.rpy")).unwrap();
            assert!(
                content.contains(&format!("translate es start_{hash}:")),
                "{content}"
            );
            assert!(content.contains(&format!("    # {statement}\n")));
            assert_eq!(report.strings_written, 1);
            assert_eq!(report.strings_skipped, 0);
        }
    }

    #[test]
    fn add_dialogue_numbers_repeated_statements_in_script_order() {
        let dir = tempfile::tempdir().unwrap();
        let game = dir.path().join("game");
        fs::create_dir(&game).unwrap();
        fs::write(
            game.join("script.rpy"),
            "label start:\n    \"Yes.\"\n    \"Yes.\"\n    \"Yes.\"\n",
        )
        .unwrap();
        let plugin = RenPyPlugin::new();
        let mut entries = plugin.extract(dir.path()).unwrap();
        assert_eq!(entries.len(), 3);
        for (index, entry) in entries.iter_mut().enumerate() {
            entry.translation = Some(format!("Sí {}.", index + 1));
        }
        entries.reverse(); // Caller/database order must not assign the suffixes.

        let report = plugin.inject_add(dir.path(), "es", &entries).unwrap();
        assert_eq!(report.strings_written, 3);
        assert_eq!(report.strings_skipped, 0);
        let content = fs::read_to_string(game.join("tl/es/script.rpy")).unwrap();
        let base = format!("start_{}", expected_say_hash(r#""Yes.""#));
        let ids: Vec<_> = content
            .lines()
            .filter(|line| line.starts_with("translate "))
            .collect();
        assert_eq!(
            ids,
            [
                format!("translate es {base}:"),
                format!("translate es {base}_1:"),
                format!("translate es {base}_2:")
            ]
        );
        for (index, suffix) in ["", "_1", "_2"].iter().enumerate() {
            assert!(content.contains(&format!("# game/script.rpy:{}\ntranslate es {base}{suffix}:\n\n    # \"Yes.\"\n    \"Sí {}.\"\n", index + 2, index + 1)), "{content}");
        }
    }

    #[test]
    fn add_dialogue_numbering_spans_files_and_keeps_labels_and_languages_independent() {
        let dir = tempfile::tempdir().unwrap();
        let game = dir.path().join("game");
        fs::create_dir_all(game.join("a")).unwrap();
        fs::create_dir_all(game.join("z")).unwrap();
        // Shared label metadata exercises the global ID namespace. The lexical
        // source-path order is opposite the output basenames (layout stays flat).
        fs::write(
            game.join("a/z.rpy"),
            "label start:\n    \"Yes.\"\n    \"Yes.\"\n",
        )
        .unwrap();
        fs::write(
            game.join("z/a.rpy"),
            "label start:\n    \"Yes.\"\nlabel other:\n    \"Yes.\"\n    \"Yes.\"\n",
        )
        .unwrap();
        let plugin = RenPyPlugin::new();
        let mut entries = plugin.extract(dir.path()).unwrap();
        assert_eq!(entries.len(), 5);
        for entry in &mut entries {
            entry.translation = Some(format!("Sí desde {}.", entry.id));
        }
        entries.reverse();
        let hash = expected_say_hash(r#""Yes.""#);
        for language in ["es", "fr"] {
            let report = plugin.inject_add(dir.path(), language, &entries).unwrap();
            assert_eq!(report.strings_written, 5);
            assert_eq!(report.strings_skipped, 0);
            let first = fs::read_to_string(game.join(format!("tl/{language}/z.rpy"))).unwrap();
            let second = fs::read_to_string(game.join(format!("tl/{language}/a.rpy"))).unwrap();
            let ids = |content: &str| {
                content
                    .lines()
                    .filter(|line| line.starts_with("translate "))
                    .map(str::to_owned)
                    .collect::<Vec<_>>()
            };
            assert_eq!(
                ids(&first),
                [
                    format!("translate {language} start_{hash}:"),
                    format!("translate {language} start_{hash}_1:")
                ]
            );
            assert_eq!(
                ids(&second),
                [
                    format!("translate {language} start_{hash}_2:"),
                    format!("translate {language} other_{hash}:"),
                    format!("translate {language} other_{hash}_1:")
                ]
            );
            entries.rotate_left(1);
            plugin.inject_add(dir.path(), language, &entries).unwrap();
            assert_eq!(
                fs::read_to_string(game.join(format!("tl/{language}/z.rpy"))).unwrap(),
                first
            );
            assert_eq!(
                fs::read_to_string(game.join(format!("tl/{language}/a.rpy"))).unwrap(),
                second
            );
        }
    }

    #[test]
    fn add_dialogue_reserves_ids_for_untranslated_occurrences() {
        let dir = tempfile::tempdir().unwrap();
        let game = dir.path().join("game");
        fs::create_dir(&game).unwrap();
        fs::write(
            game.join("script.rpy"),
            "label start:\n    \"Yes.\"\n    \"Yes.\"\n    \"Yes.\"\n    \"Yes.\"\n    \"Yes.\"\n",
        )
        .unwrap();
        let plugin = RenPyPlugin::new();
        let mut entries = plugin.extract(dir.path()).unwrap();
        assert_eq!(entries.len(), 5);
        entries[1].translation = Some("Yes.".to_string());
        entries[2].translation = Some(" ".to_string());
        entries[3].translation = Some("Sí cuatro.".to_string());
        entries[4].translation = Some("Sí cinco.".to_string());
        entries.reverse();
        let report = plugin.inject_add(dir.path(), "es", &entries).unwrap();
        assert_eq!(report.strings_written, 2);
        assert_eq!(report.strings_skipped, 3);
        let content = fs::read_to_string(game.join("tl/es/script.rpy")).unwrap();
        let hash = expected_say_hash(r#""Yes.""#);
        let ids: Vec<_> = content
            .lines()
            .filter(|line| line.starts_with("translate "))
            .collect();
        assert_eq!(
            ids,
            [
                format!("translate es start_{hash}_3:"),
                format!("translate es start_{hash}_4:")
            ]
        );
        assert!(content.contains("    \"Sí cuatro.\"\n"));
        assert!(content.contains("    \"Sí cinco.\"\n"));
    }

    #[test]
    fn dialogue_numbering_advances_past_reserved_suffix_collisions() {
        let mut allocated = HashMap::new();
        let base = "start_09e39b23";
        assert_eq!(
            renpy_numbered_translation_id(base.to_string(), &mut allocated),
            base
        );
        allocated.insert(format!("{base}_1"), 1);
        assert_eq!(
            renpy_numbered_translation_id(base.to_string(), &mut allocated),
            format!("{base}_2")
        );
    }

    #[test]
    fn add_dialogue_uses_canonical_escaped_statement() {
        for (statement, canonical) in [
            (r#""She said \"hi\" \ bye.""#, r#""She said \"hi\" \ bye.""#),
            (
                r#""She said \"hi\" \\ bye.""#,
                r#""She said \"hi\" \\ bye.""#,
            ),
            (
                r#"e "First line\nSecond line.""#,
                r#"e "First line\nSecond line.""#,
            ),
            (r#""She said \u0022hi\u0022.""#, r#""She said \"hi\".""#),
        ] {
            let dir = tempfile::tempdir().unwrap();
            let game = dir.path().join("game");
            fs::create_dir(&game).unwrap();
            fs::write(
                game.join("script.rpy"),
                format!("label start:\n    {statement}\n"),
            )
            .unwrap();
            let plugin = RenPyPlugin::new();
            let mut entries = plugin.extract(dir.path()).unwrap();
            assert_eq!(entries.len(), 1);
            entries[0].translation = Some("Traducción.".to_string());

            plugin.inject_add(dir.path(), "es", &entries).unwrap();
            let content = fs::read_to_string(game.join("tl/es/script.rpy")).unwrap();
            let hash = expected_say_hash(canonical);
            assert!(
                content.contains(&format!("translate es start_{hash}:")),
                "{content}"
            );
            assert!(
                content.contains(&format!("    # {canonical}\n")),
                "{content}"
            );
        }
    }

    #[test]
    fn add_preserves_order_duplicate_numbering_and_strings() {
        let dir = tempfile::tempdir().unwrap();
        let game = dir.path().join("game");
        fs::create_dir(&game).unwrap();
        fs::write(
            game.join("script.rpy"),
            "\"No label.\"\nlabel start:\n    e \"Hello.\"\n    e \"Hello.\"\n    \"Hello.\"\n    \"Bye.\"\n    $ title = _(\"UI label\")\n",
        )
        .unwrap();
        let plugin = RenPyPlugin::new();
        let mut entries = plugin.extract(dir.path()).unwrap();
        assert_eq!(entries.len(), 6);
        for entry in &mut entries {
            entry.translation = Some(format!("[es] {}", entry.source));
        }

        let report = plugin.inject_add(dir.path(), "es", &entries).unwrap();
        assert_eq!(report.strings_written, 6);
        assert_eq!(report.strings_skipped, 0);
        let content = fs::read_to_string(game.join("tl/es/script.rpy")).unwrap();
        let ids: Vec<_> = content
            .lines()
            .filter(|line| line.starts_with("translate "))
            .collect();
        assert_eq!(
            ids,
            [
                format!("translate es start_{}:", expected_say_hash(r#"e "Hello.""#)),
                format!(
                    "translate es start_{}_1:",
                    expected_say_hash(r#"e "Hello.""#)
                ),
                "translate es start_09e39b23:".to_string(),
                format!("translate es start_{}:", expected_say_hash(r#""Bye.""#)),
            ]
        );
        assert!(content.contains("# game/script.rpy:3\n"));
        assert!(content.contains("# game/script.rpy:4\n"));
        assert!(content.contains("# game/script.rpy:5\n"));
        assert!(content.contains("# game/script.rpy:6\n"));
        assert_eq!(
            fs::read_to_string(game.join("tl/es/locust_strings.rpy")).unwrap(),
            "# Auto-generated by Locust — UI strings translation\n\ntranslate es strings:\n\n    old \"No label.\"\n    new \"[es] No label.\"\n\n    old \"UI label\"\n    new \"[es] UI label\"\n"
        );
    }

    #[test]
    #[ignore = "requires LOCUST_RENPY_TL_CORPUS pointing to real Ren'Py games"]
    fn renpy_add_ids_match_real_tl_corpus() {
        let Some(root) = std::env::var_os("LOCUST_RENPY_TL_CORPUS") else {
            eprintln!("Skipping Ren'Py TL corpus: LOCUST_RENPY_TL_CORPUS is unset.");
            return;
        };
        let mut files = 0usize;
        let mut dialogue_blocks = 0usize;
        let mut matches = 0usize;
        let mut exact_matches = 0usize;
        let mut mismatches = Vec::new();
        struct Block {
            id: String,
            base: String,
            computed: String,
            game_language: usize,
            source_file: usize,
            tl_file: usize,
            line: usize,
        }
        let mut blocks = Vec::new();
        let mut game_languages = HashMap::new();
        let mut source_files = HashMap::new();
        let mut source_paths = Vec::new();
        let mut tl_paths = Vec::new();
        // Recovery copies are not independent Ren'Py corpus data and can contain
        // recursively nested legacy backups. Inspect the game trees themselves.
        for entry in walkdir::WalkDir::new(&root)
            .follow_links(false)
            .into_iter()
            .filter_entry(|entry| {
                crate::discovery::is_game_entry(entry)
                    && (!entry.file_type().is_dir() || entry.file_name() != ".locust_backups")
            })
        {
            let entry = entry.unwrap();
            if !entry.file_type().is_file()
                || entry.path().extension().is_none_or(|ext| ext != "rpy")
                || !entry
                    .path()
                    .components()
                    .any(|part| part.as_os_str() == "tl")
            {
                continue;
            }
            let bytes = fs::read(entry.path()).unwrap();
            let content = String::from_utf8_lossy(&bytes);
            if content.starts_with("# Auto-generated by Locust") {
                continue; // This corpus checks IDs generated by Ren'Py itself.
            }
            files += 1;
            let tl_file = tl_paths.len();
            tl_paths.push(entry.path().to_path_buf());
            let game = entry
                .path()
                .ancestors()
                .find(|path| path.file_name().is_some_and(|name| name == "tl"))
                .unwrap()
                .parent()
                .unwrap();
            let mut pending_id = None;
            let mut language = "";
            let mut source_path = entry.path().to_string_lossy().replace('\\', "/");
            let mut source_line = 0;
            for line in content.lines() {
                let trimmed = line.trim();
                if let Some((path, number)) = trimmed
                    .strip_prefix("# ")
                    .and_then(|comment| comment.rsplit_once(':'))
                    .and_then(|(path, number)| number.parse::<usize>().ok().map(|n| (path, n)))
                {
                    if path.ends_with(".rpy") {
                        source_path = path.replace('\\', "/");
                        source_line = number;
                    }
                }
                if let Some(header) = trimmed.strip_prefix("translate ") {
                    let mut parts = header.split_whitespace();
                    language = parts.next().unwrap_or("");
                    pending_id = parts
                        .next()
                        .and_then(|id| id.strip_suffix(':'))
                        .filter(|id| *id != "strings");
                    continue;
                }
                let Some((id, code)) = pending_id.zip(trimmed.strip_prefix("# ")) else {
                    continue;
                };
                // Some TL files indent the original statement after `#` rather
                // than before it; indentation is not part of canonical code.
                let code = code.trim();
                // Identify commented say statements without the extractor's
                // file-path heuristic or restrictions on character attributes.
                let Some(quote_pos) = code.find('"') else {
                    continue;
                };
                let who = code[..quote_pos].split_whitespace().next();
                if who.is_some_and(|who| {
                    // Python/action comments may also contain quoted literals;
                    // only narration or a character token is a say statement.
                    !who.chars().all(|ch| ch.is_alphanumeric() || ch == '_')
                        || (is_renpy_keyword(who) && who != "centered")
                        || who == "voice"
                }) {
                    continue;
                }
                pending_id = None;
                dialogue_blocks += 1;
                // Separate the hash check from the duplicate-numbering hypotheses.
                let base_id = id.rsplit_once('_').map_or(id, |(prefix, tail)| {
                    let has_hash = prefix.rsplit_once('_').is_some_and(|(_, hash)| {
                        hash.len() == 8 && hash.bytes().all(|b| b.is_ascii_hexdigit())
                    });
                    if has_hash && !tail.is_empty() && tail.bytes().all(|b| b.is_ascii_digit()) {
                        prefix
                    } else {
                        id
                    }
                });
                let label = base_id
                    .rsplit_once('_')
                    .map_or("unknown", |(label, _)| label);
                let actual_id = renpy_translation_id(label, code);
                if actual_id == id {
                    exact_matches += 1;
                }
                if actual_id == base_id {
                    matches += 1;
                } else if mismatches.len() < 5 {
                    mismatches.push(format!("{}: {id}", entry.path().display()));
                }
                let next_game_language = game_languages.len();
                let game_language = *game_languages
                    .entry((game.to_path_buf(), language.to_string()))
                    .or_insert(next_game_language);
                let next_source_file = source_paths.len();
                let source_file = *source_files
                    .entry((game_language, source_path.clone()))
                    .or_insert_with(|| {
                        source_paths.push(source_path.clone());
                        next_source_file
                    });
                blocks.push(Block {
                    id: id.to_string(),
                    base: base_id.to_string(),
                    computed: actual_id,
                    game_language,
                    source_file,
                    tl_file,
                    line: source_line,
                });
            }
        }
        assert!(
            dialogue_blocks > 0,
            "No dialogue blocks found in {:?}",
            root
        );
        let rate = matches as f64 * 100.0 / dialogue_blocks as f64;
        let exact_rate = exact_matches as f64 * 100.0 / dialogue_blocks as f64;
        eprintln!("Ren'Py TL corpus: {matches}/{dialogue_blocks} MD5-derived dialogue IDs matched ({rate:.4}%) across {files} TL files; unsuffixed baseline full IDs: {exact_matches}/{dialogue_blocks} ({exact_rate:.4}%).");
        let mut base_counts = HashMap::new();
        for block in &blocks {
            *base_counts
                .entry((block.game_language, block.base.as_str()))
                .or_insert(0usize) += 1;
        }
        let repeated_suffixes = blocks
            .iter()
            .filter(|block| {
                block.id != block.base
                    && base_counts[&(block.game_language, block.base.as_str())] > 1
            })
            .count();
        let canonical_suffixes = blocks
            .iter()
            .filter(|block| {
                block.id != block.base
                    && base_counts[&(block.game_language, block.base.as_str())] > 1
                    && block
                        .id
                        .strip_prefix(&format!("{}_", block.base))
                        .is_some_and(|n| {
                            n.parse::<usize>()
                                .is_ok_and(|number| number > 0 && number.to_string() == n)
                        })
            })
            .count();
        eprintln!("Repeated-base suffix format: {canonical_suffixes}/{repeated_suffixes} are unpadded _N with N >= 1; {} game/language scopes.", game_languages.len());
        let mut physical_order: Vec<_> = (0..blocks.len()).collect();
        physical_order.sort_by(|&a, &b| {
            let a = &blocks[a];
            let b = &blocks[b];
            (a.game_language, &tl_paths[a.tl_file]).cmp(&(b.game_language, &tl_paths[b.tl_file]))
        });
        let mut script_order = physical_order.clone();
        script_order.sort_by(|&a, &b| {
            let a = &blocks[a];
            let b = &blocks[b];
            (a.game_language, &source_paths[a.source_file], a.line).cmp(&(
                b.game_language,
                &source_paths[b.source_file],
                b.line,
            ))
        });
        let mut source_physical_order = physical_order.clone();
        source_physical_order.sort_by(|&a, &b| {
            let a = &blocks[a];
            let b = &blocks[b];
            (a.game_language, &source_paths[a.source_file])
                .cmp(&(b.game_language, &source_paths[b.source_file]))
        });
        let backwards = physical_order
            .windows(2)
            .filter(|pair| {
                let a = &blocks[pair[0]];
                let b = &blocks[pair[1]];
                a.tl_file == b.tl_file && a.source_file == b.source_file && a.line > b.line
            })
            .count();
        eprintln!("TL blocks with decreasing source-line comments: {backwards}");
        let mut reverse_files = physical_order.clone();
        reverse_files.sort_by(|&a, &b| {
            let a = &blocks[a];
            let b = &blocks[b];
            (
                a.game_language,
                std::cmp::Reverse(&source_paths[a.source_file]),
                a.line,
            )
                .cmp(&(
                    b.game_language,
                    std::cmp::Reverse(&source_paths[b.source_file]),
                    b.line,
                ))
        });
        let mut basename_order = physical_order.clone();
        basename_order.sort_by(|&a, &b| {
            let a = &blocks[a];
            let b = &blocks[b];
            (
                a.game_language,
                source_paths[a.source_file].rsplit('/').next(),
                a.line,
            )
                .cmp(&(
                    b.game_language,
                    source_paths[b.source_file].rsplit('/').next(),
                    b.line,
                ))
        });
        for (name, order, scope, start) in [
            ("per TL file / TL order / _1", &physical_order, 0, 1),
            ("per source file / script order / _1", &script_order, 1, 1),
            (
                "game+language / TL path then TL order / _1",
                &physical_order,
                2,
                1,
            ),
            (
                "game+language / source path then TL order / _1",
                &source_physical_order,
                2,
                1,
            ),
            (
                "game+language / source path then line / _1",
                &script_order,
                2,
                1,
            ),
            (
                "game+language / reverse source path then line / _1",
                &reverse_files,
                2,
                1,
            ),
            (
                "game+language / source basename then line / _1",
                &basename_order,
                2,
                1,
            ),
            (
                "game+language / source path then line / _0",
                &script_order,
                2,
                0,
            ),
        ] {
            let mut counts = HashMap::new();
            let mut full_allocated = HashMap::new();
            let mut current_scope = None;
            let mut numbered_matches = 0;
            let mut full_matches = 0;
            let mut suffix_matches = 0;
            for &index in order {
                let block = &blocks[index];
                let scope = match scope {
                    0 => block.tl_file,
                    1 => block.source_file,
                    _ => block.game_language,
                };
                if current_scope != Some(scope) {
                    full_allocated.clear();
                    current_scope = Some(scope);
                }
                let count = counts.entry((scope, block.base.as_str())).or_insert(0usize);
                let suffix = if *count == 0 {
                    String::new()
                } else {
                    format!("_{}", *count - 1 + start)
                };
                *count += 1;
                let expected = format!("{}{suffix}", block.base);
                if expected == block.id {
                    numbered_matches += 1;
                    if block.id != block.base
                        && base_counts[&(block.game_language, block.base.as_str())] > 1
                    {
                        suffix_matches += 1;
                    }
                }
                let expected_full = if start == 1 {
                    renpy_numbered_translation_id(block.computed.clone(), &mut full_allocated)
                } else {
                    let count = full_allocated
                        .entry(block.computed.clone())
                        .or_insert(0usize);
                    let id = if *count == 0 {
                        block.computed.clone()
                    } else {
                        format!("{}_{}", block.computed, *count - 1)
                    };
                    *count += 1;
                    id
                };
                if expected_full == block.id {
                    full_matches += 1;
                }
            }
            eprintln!("Hypothesis {name}: numbering {numbered_matches}/{dialogue_blocks} ({:.4}%); full {full_matches}/{dialogue_blocks} ({:.4}%); repeated suffixes {suffix_matches}/{repeated_suffixes} ({:.4}%).", numbered_matches as f64 * 100.0 / dialogue_blocks as f64, full_matches as f64 * 100.0 / dialogue_blocks as f64, suffix_matches as f64 * 100.0 / repeated_suffixes as f64);
        }
        // A counter scoped per label within a game/language is equivalent to the
        // game/language counter: each complete base ID already contains its label.
        eprintln!("Per-label numbering within each game/language is identical to the game/language hypotheses because the label is included in every base ID.");
        // TL encounter order best preserves the original script traversal in
        // this historical corpus. Ren'Py appends updates, so old source-line
        // comments cannot be used to reconstruct that traversal reliably.
        let mut allocated = HashMap::new();
        let mut current_game_language = None;
        let mut full_matches = 0usize;
        let mut full_mismatches = Vec::new();
        for &index in &physical_order {
            let block = &blocks[index];
            if current_game_language != Some(block.game_language) {
                allocated.clear();
                current_game_language = Some(block.game_language);
            }
            let expected = renpy_numbered_translation_id(block.computed.clone(), &mut allocated);
            if expected == block.id {
                full_matches += 1;
            } else if full_mismatches.len() < 5 {
                full_mismatches.push(format!(
                    "{}: expected {expected}, found {}",
                    tl_paths[block.tl_file].display(),
                    block.id
                ));
            }
        }
        let full_rate = full_matches as f64 * 100.0 / dialogue_blocks as f64;
        eprintln!("Shared Add numbering function: full IDs {full_matches}/{dialogue_blocks} ({full_rate:.4}%).");
        assert!(
            full_matches * 1000 >= dialogue_blocks * 994,
            "Expected at least 99.4% matching full IDs, got {full_rate:.4}%; examples: {full_mismatches:?}"
        );
        assert!(
            matches * 100 >= dialogue_blocks * 95,
            "Expected at least 95% matching IDs, got {rate:.2}%; examples: {mismatches:?}"
        );
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
