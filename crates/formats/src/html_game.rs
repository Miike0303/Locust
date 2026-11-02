use locust_core::backup::RevisionOriginal;
use std::collections::{HashMap, HashSet};

#[path = "html_slots.rs"]
mod slots;
use std::path::{Path, PathBuf};

use locust_core::error::{LocustError, Result};
use locust_core::extraction::{Extraction, FormatPlugin, InjectionReport};
use locust_core::models::{OutputMode, StringEntry};

/// Plugin for generic HTML-based games (non-SugarCube).
/// Handles Twine/Harlowe, plain HTML adventure games, and other HTML game formats.
pub struct HtmlGamePlugin;

impl HtmlGamePlugin {
    pub fn new() -> Self {
        Self
    }

    fn find_html_files(path: &Path) -> Vec<PathBuf> {
        let mut files = Vec::new();
        if path.is_file() && is_html(path) {
            files.push(path.to_path_buf());
        } else if path.is_dir() {
            Self::scan_dir(path, &mut files, 0);
        }
        files
    }

    fn scan_dir(dir: &Path, files: &mut Vec<PathBuf>, depth: usize) {
        if depth > 3
            || dir
                .file_name()
                .is_some_and(crate::discovery::is_internal_directory_name)
        {
            return;
        }
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let p = entry.path();
                if locust_core::patch::zipsec::ensure_no_links(dir, Path::new(&entry.file_name()))
                    .is_err()
                {
                    continue;
                }
                if p.is_file() && is_html(&p) {
                    files.push(p);
                } else if p.is_dir() && !is_skip_dir(&p) {
                    Self::scan_dir(&p, files, depth + 1);
                }
            }
        }
    }

    fn is_sugarcube(content: &str) -> bool {
        content.contains("tw-passagedata") || content.contains("SugarCube")
    }

    fn slot_index(slots: &[slots::Slot]) -> HashMap<usize, usize> {
        let mut index = HashMap::with_capacity(slots.len());
        for (position, slot) in slots.iter().enumerate() {
            index.entry(slot.range.start).or_insert(position);
        }
        index
    }

    fn extract_from_html(content: &str, file_path: &Path) -> Vec<StringEntry> {
        slots::scan(content)
            .into_iter()
            .map(|slot| {
                let mut entry = StringEntry::new(
                    format!("{}#html:{}", file_path.display(), slot.range.start),
                    slot.source,
                    file_path.to_path_buf(),
                );
                entry.context = Some(format!(
                    "HTML {} (preserve variables; adjacent inline markup is separate)",
                    slot.kind
                ));
                entry.tags = vec![if slot.kind.starts_with("attr:") {
                    "html-attr"
                } else {
                    "html-text"
                }
                .into()];
                entry
                    .metadata
                    .insert("html_start".into(), serde_json::json!(slot.range.start));
                entry
                    .metadata
                    .insert("html_end".into(), serde_json::json!(slot.range.end));
                entry
                    .metadata
                    .insert("html_raw".into(), serde_json::json!(&content[slot.range]));
                entry
                    .metadata
                    .insert("html_kind".into(), serde_json::json!(slot.kind));
                entry
            })
            .collect()
    }
}

impl Default for HtmlGamePlugin {
    fn default() -> Self {
        Self::new()
    }
}

impl FormatPlugin for HtmlGamePlugin {
    fn id(&self) -> &str {
        "html-game"
    }

    fn name(&self) -> &str {
        "HTML Game (Generic)"
    }

    fn description(&self) -> &str {
        "Generic HTML-based games: Twine/Harlowe, HTML adventure games, interactive fiction"
    }

    fn stability(&self) -> locust_core::extraction::FormatStability {
        // Phase-2 apply proven on synthetic non-SugarCube HTML fixture.
        locust_core::extraction::FormatStability::Experimental
    }

    fn supported_extensions(&self) -> &[&str] {
        &[".html", ".htm"]
    }

    fn supported_modes(&self) -> Vec<OutputMode> {
        vec![OutputMode::Replace]
    }

    fn detect(&self, path: &Path) -> bool {
        let files = Self::find_html_files(path);
        if files.is_empty() {
            return false;
        }
        for f in &files {
            if let Ok(content) = std::fs::read_to_string(f) {
                if Self::is_sugarcube(&content) {
                    return false;
                }
            }
        }
        for f in &files {
            if let Ok(content) = std::fs::read_to_string(f) {
                let entries = Self::extract_from_html(&content, f);
                if !entries.is_empty() {
                    return true;
                }
            }
        }
        false
    }

    fn extract(&self, path: &Path) -> Result<Vec<StringEntry>> {
        Ok(self.extract_with_diagnostics(path)?.entries)
    }

    fn extract_with_diagnostics(&self, path: &Path) -> Result<Extraction> {
        let files = Self::find_html_files(path);
        if files.is_empty() {
            return Err(LocustError::ParseError {
                file: path.to_string_lossy().to_string(),
                message: "No HTML files found".into(),
            });
        }

        let mut all_entries = Vec::new();
        let mut warnings = Vec::new();
        for file in &files {
            match std::fs::read_to_string(file) {
                Ok(content) => {
                    if Self::is_sugarcube(&content) {
                        continue;
                    }
                    let entries = Self::extract_from_html(&content, file);
                    all_entries.extend(entries);
                }
                Err(e) => {
                    let base = if path.is_dir() {
                        path
                    } else {
                        path.parent().unwrap_or(path)
                    };
                    warnings.push(format!(
                        "skipped unreadable HTML file {}: {e}",
                        file.strip_prefix(base).unwrap_or(file).display()
                    ));
                }
            }
        }

        Ok(Extraction {
            complete: warnings.is_empty(),
            entries: all_entries,
            warnings,
        })
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
            if !is_html(current_path) {
                continue;
            }
            let original = original_path.read_text()?;
            let current = std::fs::read_to_string(current_path)?;
            if original == current {
                continue;
            }
            if Self::is_sugarcube(&original) || Self::is_sugarcube(&current) {
                continue;
            }
            let mut old_slots = slots::scan(&original);
            let mut new_slots = slots::scan(&current);
            old_slots.sort_by_key(|slot| slot.range.start);
            new_slots.sort_by_key(|slot| slot.range.start);
            // Injection only changes slot contents. Matching the entire markup
            // between slots proves their correspondence even after byte offsets
            // shift, and avoids matching duplicate dialogue by text alone.
            if old_slots.len() != new_slots.len() {
                continue;
            }
            let (mut old_end, mut new_end) = (0, 0);
            let aligned = old_slots.iter().zip(&new_slots).all(|(old, new)| {
                let same = old.kind == new.kind
                    && original[old_end..old.range.start] == current[new_end..new.range.start];
                old_end = old.range.end;
                new_end = new.range.end;
                same
            }) && original[old_end..] == current[new_end..];
            if !aligned {
                continue;
            }
            let old_index = Self::slot_index(&old_slots);
            let new_index = Self::slot_index(&new_slots);
            let matches = |entry: &StringEntry, slot: &slots::Slot, content: &str| {
                entry.source == slot.source
                    && entry.metadata.get("html_start").and_then(|v| v.as_u64())
                        == Some(slot.range.start as u64)
                    && entry.metadata.get("html_end").and_then(|v| v.as_u64())
                        == Some(slot.range.end as u64)
                    && entry.metadata.get("html_kind").and_then(|v| v.as_str())
                        == Some(slot.kind.as_str())
                    && entry.metadata.get("html_raw").and_then(|v| v.as_str())
                        == Some(&content[slot.range.clone()])
            };
            for entry in file_entries {
                // Freshly extracted rows already describe current bytes. Their
                // identity translations must never restore the older original.
                let Some(start) = entry
                    .metadata
                    .get("html_start")
                    .and_then(|v| v.as_u64())
                    .and_then(|v| usize::try_from(v).ok())
                else {
                    continue;
                };
                let current_index = new_index
                    .get(&start)
                    .copied()
                    .filter(|&index| matches(entry, &new_slots[index], &current));
                let original_index = old_index
                    .get(&start)
                    .copied()
                    .filter(|&index| matches(entry, &old_slots[index], &original));
                if let (Some(old), Some(new)) = (original_index, current_index) {
                    if old != new {
                        return Err(LocustError::InjectionError(
                            "HTML revision locator matches different original and current slots; extract the current game into a new project before editing this ambiguous row".into(),
                        ));
                    }
                }
                if current_index.is_some() {
                    continue;
                }
                let Some(index) = original_index else {
                    continue;
                };
                let slot = &new_slots[index];
                entry.source = slot.source.clone();
                entry
                    .metadata
                    .insert("html_start".into(), serde_json::json!(slot.range.start));
                entry
                    .metadata
                    .insert("html_end".into(), serde_json::json!(slot.range.end));
                entry
                    .metadata
                    .insert("html_kind".into(), serde_json::json!(slot.kind));
                entry.metadata.insert(
                    "html_raw".into(),
                    serde_json::json!(&current[slot.range.clone()]),
                );
            }
        }
        Ok(())
    }

    fn inject(&self, path: &Path, entries: &[StringEntry]) -> Result<InjectionReport> {
        let files = Self::find_html_files(path);
        if files.is_empty() {
            return Err(LocustError::ParseError {
                file: path.to_string_lossy().to_string(),
                message: "No HTML files found".into(),
            });
        }

        let mut report = InjectionReport {
            skip_reasons: Default::default(),
            files_modified: 0,
            strings_written: 0,
            strings_skipped: 0,
            warnings: Vec::new(),
            files_written: Vec::new(),
        };
        let mut by_file: HashMap<PathBuf, Vec<&StringEntry>> = HashMap::new();
        for entry in entries {
            by_file
                .entry(entry.file_path.clone())
                .or_default()
                .push(entry);
        }
        let root = path.canonicalize()?;
        let available: HashSet<PathBuf> = files
            .iter()
            .filter_map(|p| p.canonicalize().ok())
            .filter(|p| {
                if root.is_file() {
                    p == &root
                } else {
                    p.starts_with(&root)
                }
            })
            .collect();
        for (file, file_entries) in by_file {
            // Never write a path outside the selected HTML tree, even if an
            // old database still points at another copy of the game.
            if !file.canonicalize().is_ok_and(|p| available.contains(&p)) {
                report.skip("target_missing", file_entries.len());
                continue;
            }
            let content = std::fs::read_to_string(&file)?;
            if Self::is_sugarcube(&content) {
                report.skip("unsupported", file_entries.len());
                continue;
            }
            let current = slots::scan(&content);
            let current_index = Self::slot_index(&current);
            let mut patches = Vec::new();
            let mut seen = HashSet::new();
            for entry in file_entries {
                let Some(translation) = entry.translation.as_deref().filter(|t| !t.is_empty())
                else {
                    report.skip("untranslated", 1);
                    continue;
                };
                if translation == entry.source {
                    report.skip("unchanged", 1);
                    continue;
                }
                let start = entry
                    .metadata
                    .get("html_start")
                    .and_then(|v| v.as_u64())
                    .and_then(|n| usize::try_from(n).ok());
                let slot = if let Some(start) = start {
                    current_index
                        .get(&start)
                        .map(|&index| &current[index])
                        .filter(|slot| {
                            entry.metadata.get("html_end").and_then(|v| v.as_u64())
                                == Some(slot.range.end as u64)
                                && entry.metadata.get("html_kind").and_then(|v| v.as_str())
                                    == Some(slot.kind.as_str())
                                && entry.metadata.get("html_raw").and_then(|v| v.as_str())
                                    == Some(&content[slot.range.clone()])
                                && slot.source == entry.source
                        })
                } else {
                    // Legacy databases lack locations. Only a unique complete
                    // value in the correct file is safe; ambiguous duplicates
                    // and old flattened rich text require re-extraction.
                    let matches: Vec<_> = current
                        .iter()
                        .filter(|slot| {
                            slot.source == entry.source
                                && (entry.tags.is_empty()
                                    || entry.tags.iter().any(|tag| {
                                        (tag == "html-attr" && slot.kind.starts_with("attr:"))
                                            || (tag == "html-text"
                                                && slot.kind.starts_with("text:"))
                                    }))
                        })
                        .collect();
                    if matches.len() > 1 {
                        report.skip("ambiguous_target", 1);
                        continue;
                    }
                    matches.first().copied()
                };
                let Some(slot) = slot else {
                    report.skip("source_changed", 1);
                    continue;
                };
                let mut encoded = html_encode_text(translation);
                // Angle-bracket text is literal in RCDATA. Validate the encoded
                // values so it is not mistaken for protected HTML markup.
                let rcdata_source =
                    (slot.kind == "text:textarea").then(|| html_encode_text(&entry.source));
                let source = rcdata_source.as_deref().unwrap_or(&entry.source);
                let target = if rcdata_source.is_some() {
                    encoded.as_str()
                } else {
                    translation
                };
                if !locust_core::placeholder::PlaceholderProcessor::validate_with_profile(
                    source,
                    target,
                    locust_core::placeholder::PlaceholderProfile::for_format(self.id()),
                )
                .is_empty()
                {
                    report.skip("invalid_placeholders", 1);
                    continue;
                }
                if !seen.insert(slot.range.start) {
                    report.skip("duplicate", 1);
                    continue;
                }
                if slot.kind.starts_with("attr:") {
                    encoded = encoded.replace('"', "&quot;").replace('\'', "&#39;");
                }
                patches.push((slot.range.clone(), encoded));
            }
            if !patches.is_empty() {
                patches.sort_by_key(|(range, _)| std::cmp::Reverse(range.start));
                let mut output = content;
                for (range, text) in &patches {
                    output.replace_range(range.clone(), text);
                }
                std::fs::write(&file, output)?;
                report.strings_written += patches.len();
                report.files_modified += 1;
                report.files_written.push(file);
            }
        }
        if report.skip_reasons.contains_key("source_changed")
            || report.skip_reasons.contains_key("ambiguous_target")
        {
            report.warnings.push("HTML locations changed or are ambiguous; re-extract the current game before injecting those entries.".into());
        }

        Ok(report)
    }
}

fn is_html(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|e| e.to_str()),
        Some("html" | "htm")
    )
}

fn is_skip_dir(path: &Path) -> bool {
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    matches!(
        name,
        "node_modules" | ".git" | "dist" | "build" | "__pycache__" | ".svn"
    )
}

fn is_skip_tag(tag: &str) -> bool {
    matches!(
        tag,
        "script" | "style" | "svg" | "noscript" | "template" | "math"
    )
}

#[cfg(test)]
fn strip_inner_tags(html: &str) -> String {
    let mut result = String::new();
    let mut in_tag = false;
    for c in html.chars() {
        if c == '<' {
            in_tag = true;
        } else if c == '>' {
            in_tag = false;
        } else if !in_tag {
            result.push(c);
        }
    }
    slots::decode_entities(&result)
}

fn is_translatable_text(text: &str) -> bool {
    let trimmed = text.trim();
    if trimmed.is_empty() || trimmed.len() < 2 {
        return false;
    }
    if trimmed.starts_with("http://")
        || trimmed.starts_with("https://")
        || trimmed.starts_with("//")
        || trimmed.starts_with("./")
        || trimmed.starts_with("../")
        || trimmed.contains("://")
    {
        return false;
    }
    if trimmed
        .chars()
        .all(|c| c.is_ascii_digit() || c == '.' || c == ',' || c == '-')
    {
        return false;
    }
    if trimmed.starts_with('#')
        && trimmed.len() <= 7
        && trimmed[1..].chars().all(|c| c.is_ascii_hexdigit())
    {
        return false;
    }
    let special_ratio = trimmed
        .chars()
        .filter(|c| matches!(c, '{' | '}' | ';' | '=' | '(' | ')' | '[' | ']'))
        .count() as f64
        / trimmed.len() as f64;
    if special_ratio > 0.15 {
        return false;
    }
    trimmed.chars().any(|c| c.is_alphabetic())
}

fn html_encode_text(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_extract_diagnostics_warns_on_invalid_utf8_html() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("game.html"), "<p>Hello traveler</p>").unwrap();
        let plugin = HtmlGamePlugin::new();
        let clean = plugin.extract_with_diagnostics(dir.path()).unwrap();
        assert!(clean.complete);
        assert!(clean.warnings.is_empty());
        assert!(clean.entries.iter().any(|e| e.source == "Hello traveler"));

        fs::write(dir.path().join("broken.html"), [0xff, 0xfe]).unwrap();
        let partial = plugin.extract_with_diagnostics(dir.path()).unwrap();
        assert_eq!(
            partial
                .entries
                .iter()
                .map(|e| (&e.id, &e.source))
                .collect::<Vec<_>>(),
            clean
                .entries
                .iter()
                .map(|e| (&e.id, &e.source))
                .collect::<Vec<_>>()
        );
        assert!(!partial.complete);
        assert_eq!(partial.warnings.len(), 1);
        assert!(partial.warnings[0].contains("broken.html"));
        assert!(partial.warnings[0].contains("UTF-8"));
    }

    #[test]
    fn revision_refuses_mutated_original_before_retargeting_html() {
        let dir = tempfile::tempdir().unwrap();
        let current = dir.path().join("current.html");
        let original = dir.path().join("original.html");
        let source = "<p>Hello traveler</p>";
        std::fs::write(&current, source).unwrap();
        std::fs::write(&original, source).unwrap();
        let plugin = HtmlGamePlugin::new();
        let mut entries = plugin.extract(&current).unwrap();
        assert_eq!(entries.len(), 1);
        let expected_source = entries[0].source.clone();
        let originals = HashMap::from([(
            current.clone(),
            RevisionOriginal::capture(&original).unwrap(),
        )]);
        std::fs::write(&current, "<p>Primera traducción</p>").unwrap();
        for changed in [
            Some("<p>Other traveler</p>"),
            Some("<p>Longer changed original</p>"),
            Some("<p>X</p>"),
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
                "<p>Primera traducción</p>"
            );
        }
        std::fs::write(&original, source).unwrap();
        plugin
            .prepare_revision_entries(&mut entries, &originals)
            .unwrap();
        assert_eq!(entries[0].source, "Primera traducción");
    }

    #[test]
    fn revision_refuses_a_locator_that_collides_with_another_current_slot() {
        let game = tempfile::tempdir().unwrap();
        let backup = tempfile::tempdir().unwrap();
        let current = game.path().join("story.html");
        let original = backup.path().join("story.html");
        let content = "<p>Alpha original</p><p>Duplicate text</p><p>Duplicate text</p>";
        std::fs::write(&current, content).unwrap();
        std::fs::write(&original, content).unwrap();
        let plugin = HtmlGamePlugin::new();
        let mut entries = plugin.extract(game.path()).unwrap();
        let slots = slots::scan(content);
        entries[0].translation =
            Some("A".repeat(slots[0].range.len() + slots[2].range.start - slots[1].range.start));
        plugin.inject(game.path(), &entries[..1]).unwrap();
        let before = std::fs::read(&current).unwrap();
        let mut revision = vec![entries[2].clone()];
        revision[0].translation = Some("Must not hit the second slot".into());
        let error = plugin
            .prepare_revision_entries(
                &mut revision,
                &HashMap::from([(
                    current.clone(),
                    RevisionOriginal::capture(&original).unwrap(),
                )]),
            )
            .unwrap_err();
        assert!(error
            .to_string()
            .contains("different original and current slots"));
        assert_eq!(std::fs::read(current).unwrap(), before);
    }

    #[test]
    fn direct_revision_preserves_other_slots_and_handles_original_or_current_identity() {
        let game = tempfile::tempdir().unwrap();
        let backup = tempfile::tempdir().unwrap();
        let current = game.path().join("story.html");
        let original = backup.path().join("story.html");
        let content = "<p>Hello traveler</p><p>Hello traveler</p>";
        std::fs::write(&current, content).unwrap();
        std::fs::write(&original, content).unwrap();
        let plugin = HtmlGamePlugin::new();
        let mut entries = plugin.extract(game.path()).unwrap();
        assert_eq!(entries.len(), 2);
        let originals = HashMap::from([(
            current.clone(),
            RevisionOriginal::capture(&original).unwrap(),
        )]);
        entries[0].translation = Some("Primera frase extensa".into());
        entries[1].translation = Some("Segunda frase".into());
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
            "<p>Primera frase extensa</p><p>Frase corregida</p>"
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
        let expected = "<p>Primera frase extensa</p><p>Hello traveler</p>";
        assert_eq!(std::fs::read_to_string(&current).unwrap(), expected);
        let mut fresh = plugin.extract(game.path()).unwrap();
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
        mixed[0]
            .metadata
            .insert("html_end".into(), serde_json::json!(1));
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
            "<p>Primera frase extensa</p><p>Cambio permitido</p>"
        );
    }

    #[test]
    fn revision_does_not_guess_changed_markup_or_invalid_original_locators() {
        let game = tempfile::tempdir().unwrap();
        let backup = tempfile::tempdir().unwrap();
        let current = game.path().join("story.html");
        let original = backup.path().join("story.html");
        let content = "<p>Hello traveler</p>";
        std::fs::write(&current, content).unwrap();
        std::fs::write(&original, content).unwrap();
        let plugin = HtmlGamePlugin::new();
        let mut entries = plugin.extract(game.path()).unwrap();
        entries[0].translation = Some("Texto corregido".into());
        let originals = HashMap::from([(
            current.clone(),
            RevisionOriginal::capture(&original).unwrap(),
        )]);
        for changed in ["<div>Texto previo</div>", "<p>Texto previo</p>"] {
            std::fs::write(&current, changed).unwrap();
            let mut invalid = entries.clone();
            if changed.starts_with("<p>") {
                invalid[0]
                    .metadata
                    .insert("html_end".into(), serde_json::json!(1));
            }
            plugin
                .prepare_revision_entries(&mut invalid, &originals)
                .unwrap();
            assert_eq!(
                plugin
                    .inject(game.path(), &invalid)
                    .unwrap()
                    .strings_written,
                0
            );
            assert_eq!(std::fs::read_to_string(&current).unwrap(), changed);
        }
    }

    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn located_injection_preserves_code_and_distinguishes_duplicate_text() {
        let dir = tempdir().unwrap();
        let file = dir.path().join("game.html");
        let html = r#"<script>const label = "Play";</script><!-- Play --><style>.Play{color:red}</style><p id="Play">Play</p><button onclick="Play()">Play</button>"#;
        fs::write(&file, html).unwrap();
        let plugin = HtmlGamePlugin;
        let mut entries = plugin.extract(&file).unwrap();
        assert_eq!(entries.len(), 2);
        entries[0].translation = Some("Jugar".into());
        entries[1].translation = Some("Reproducir".into());
        let report = plugin.inject(&file, &entries).unwrap();
        assert_eq!((report.strings_written, report.strings_skipped), (2, 0));
        assert_eq!(
            fs::read_to_string(file).unwrap(),
            r#"<script>const label = "Play";</script><!-- Play --><style>.Play{color:red}</style><p id="Play">Jugar</p><button onclick="Play()">Reproducir</button>"#
        );
    }

    #[test]
    fn rich_text_entities_and_quoted_attributes_keep_their_structure() {
        let dir = tempdir().unwrap();
        let file = dir.path().join("game.html");
        fs::write(
            &file,
            r#"<P TITLE = 'Tom &amp; Jerry > here'>Hello <b>world</b> &#x26; friends &copy;.</P>"#,
        )
        .unwrap();
        let plugin = HtmlGamePlugin;
        let mut entries = plugin.extract(&file).unwrap();
        assert_eq!(entries.len(), 4);
        let expected = ["Tom & Jerry > here", "Hello", "world", "& friends ©."];
        for (entry, expected) in entries.iter().zip(expected) {
            assert_eq!(entry.source, expected);
        }
        for (entry, translation) in
            entries
                .iter_mut()
                .zip(["Tom's \"amigo\" < aquí", "Hola", "mundo", "& amigos ©."])
        {
            entry.translation = Some(translation.into());
        }
        let report = plugin.inject(&file, &entries).unwrap();
        assert_eq!(report.strings_written, 4);
        assert_eq!(
            fs::read_to_string(file).unwrap(),
            r#"<P TITLE = 'Tom&#39;s &quot;amigo&quot; &lt; aquí'>Hola <b>mundo</b> &amp; amigos ©.</P>"#
        );
    }

    #[test]
    fn textarea_extracts_prefilled_text() {
        let dir = tempdir().unwrap();
        let file = dir.path().join("game.html");
        fs::write(&file, "<textarea>Hello traveler</textarea>").unwrap();
        let plugin = HtmlGamePlugin;
        let entries = plugin.extract(&file).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].source, "Hello traveler");
        assert_eq!(entries[0].metadata["html_kind"], "text:textarea");
        assert_eq!(entries[0].tags, ["html-text"]);
        assert!(plugin.detect(&file));
    }

    #[test]
    fn textarea_attributes_follow_normal_quoted_attribute_rules() {
        let dir = tempdir().unwrap();
        let file = dir.path().join("game.html");
        let plugin = HtmlGamePlugin;
        for tag in ["input", "textarea"] {
            let html = format!(
                r#"<{tag} placeholder="Type here" title='Tom &amp; Jerry' aria-label="Your message" id="Internal name" onclick="Ignore this" data-label="Ignore data" label=Unquoted></{tag}>"#
            );
            fs::write(&file, html).unwrap();
            let mut entries = plugin.extract(&file).unwrap();
            assert_eq!(
                entries
                    .iter()
                    .map(|e| e.source.as_str())
                    .collect::<Vec<_>>(),
                ["Type here", "Tom & Jerry", "Your message"],
                "{tag}"
            );
            for (entry, translation) in
                entries
                    .iter_mut()
                    .zip(["Escribe aquí", "Tom's \"amigo\" & compañía", "Tu mensaje"])
            {
                assert_eq!(entry.tags, ["html-attr"]);
                entry.translation = Some(translation.into());
            }
            let report = plugin.inject(&file, &entries).unwrap();
            assert_eq!((report.strings_written, report.strings_skipped), (3, 0));
            assert_eq!(
                fs::read_to_string(&file).unwrap(),
                format!(
                    r#"<{tag} placeholder="Escribe aquí" title='Tom&#39;s &quot;amigo&quot; &amp; compañía' aria-label="Tu mensaje" id="Internal name" onclick="Ignore this" data-label="Ignore data" label=Unquoted></{tag}>"#
                )
            );
        }
    }

    #[test]
    fn textarea_rcdata_entities_and_literal_tags_round_trip() {
        let dir = tempdir().unwrap();
        let file = dir.path().join("game.html");
        let html = "<textarea>\r\n  Hello &amp; <b>brave traveler</b> &ampere!  \r\n</textarea>";
        fs::write(&file, html).unwrap();
        let plugin = HtmlGamePlugin;
        let mut entries = plugin.extract(&file).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].source, "Hello & <b>brave traveler</b> &ere!");
        assert_eq!(
            entries[0].metadata["html_raw"],
            "Hello &amp; <b>brave traveler</b> &ampere!"
        );
        entries[0].translation = Some(entries[0].source.clone());
        let unchanged = plugin.inject(&file, &entries).unwrap();
        assert_eq!(unchanged.strings_written, 0);
        assert_eq!(unchanged.skip_reasons.get("unchanged"), Some(&1));
        assert_eq!(fs::read_to_string(&file).unwrap(), html);

        let translation = "Hola & <b>viajero valiente</b> &ere!";
        entries[0].translation = Some(translation.into());
        let report = plugin.inject(&file, &entries).unwrap();
        assert_eq!((report.strings_written, report.strings_skipped), (1, 0));
        assert_eq!(
            fs::read_to_string(&file).unwrap(),
            "<textarea>\r\n  Hola &amp; &lt;b&gt;viajero valiente&lt;/b&gt; &amp;ere!  \r\n</textarea>"
        );
        let extracted = plugin.extract(&file).unwrap();
        assert_eq!(extracted.len(), 1);
        assert_eq!(extracted[0].source, translation);
    }

    #[test]
    fn textarea_injection_encodes_literal_closing_tags_and_ampersands() {
        let dir = tempdir().unwrap();
        let file = dir.path().join("game.html");
        fs::write(
            &file,
            "<textarea>Hello {player}</textarea><p>Text after the control</p>",
        )
        .unwrap();
        let plugin = HtmlGamePlugin;
        let mut entries = plugin.extract(&file).unwrap();
        assert_eq!(entries.len(), 2);
        let translation = "Hola {player} </textarea><b>viajero</b> & bienvenido";
        entries[0].translation = Some(translation.into());
        let report = plugin.inject(&file, &entries[..1]).unwrap();
        assert_eq!((report.strings_written, report.strings_skipped), (1, 0));
        assert_eq!(
            fs::read_to_string(&file).unwrap(),
            "<textarea>Hola {player} &lt;/textarea&gt;&lt;b&gt;viajero&lt;/b&gt; &amp; bienvenido</textarea><p>Text after the control</p>"
        );
        let mut extracted = plugin.extract(&file).unwrap();
        assert_eq!(extracted.len(), 2);
        assert_eq!(extracted[0].source, translation);
        assert_eq!(extracted[1].source, "Text after the control");
        let before = fs::read(&file).unwrap();
        extracted[0].translation = Some("Hola </textarea> & bienvenido".into());
        let rejected = plugin.inject(&file, &extracted[..1]).unwrap();
        assert_eq!((rejected.strings_written, rejected.strings_skipped), (0, 1));
        assert_eq!(rejected.skip_reasons.get("invalid_placeholders"), Some(&1));
        assert_eq!(fs::read(&file).unwrap(), before);
    }

    #[test]
    fn textarea_empty_or_whitespace_content_has_no_row() {
        for content in ["", " \t\r\n ", "&nbsp; &#32; &#x09;"] {
            let html = format!("<textarea>{content}</textarea>");
            assert!(HtmlGamePlugin::extract_from_html(&html, Path::new("game.html")).is_empty());
        }
    }

    #[test]
    fn textarea_preserves_existing_byte_offset_ids() {
        let html = "<p title='Before tip'>Héllo traveler</p><textarea placeholder='Type here'>Draft text</textarea><p title='After tip'>Goodbye traveler</p>";
        let file = Path::new("game.html");
        let entries = HtmlGamePlugin::extract_from_html(html, file);
        let existing = entries
            .iter()
            .filter(|entry| {
                entry.metadata["html_kind"] != "text:textarea" && entry.source != "Type here"
            })
            .map(|entry| (entry.id.as_str(), entry.source.as_str()))
            .collect::<Vec<_>>();
        assert_eq!(
            existing,
            [
                ("game.html#html:10", "Before tip"),
                ("game.html#html:22", "Héllo traveler"),
                ("game.html#html:106", "After tip"),
                ("game.html#html:117", "Goodbye traveler"),
            ]
        );
    }

    #[test]
    fn textarea_change_preserves_other_raw_text_elements() {
        let dir = tempdir().unwrap();
        let file = dir.path().join("game.html");
        let plugin = HtmlGamePlugin;
        for tag in ["script", "style", "xmp", "iframe"] {
            let raw = format!(
                "<{tag} title='Hidden title' placeholder='Hidden hint'>Raw &amp; <b>literal text</b></{tag}extra><textarea title='Hidden control'>Hidden content</textarea></{tag}>"
            );
            fs::write(
                &file,
                format!("<p>Before control</p>{raw}<p>After control</p>"),
            )
            .unwrap();
            let mut entries = plugin.extract(&file).unwrap();
            assert_eq!(
                entries
                    .iter()
                    .map(|e| e.source.as_str())
                    .collect::<Vec<_>>(),
                ["Before control", "After control"],
                "{tag}"
            );
            entries[0].translation = Some("Antes del control".into());
            entries[1].translation = Some("Después del control".into());
            let report = plugin.inject(&file, &entries).unwrap();
            assert_eq!((report.strings_written, report.strings_skipped), (2, 0));
            assert_eq!(
                fs::read_to_string(&file).unwrap(),
                format!("<p>Antes del control</p>{raw}<p>Después del control</p>")
            );
        }
    }

    #[test]
    fn changed_source_and_foreign_file_are_not_written() {
        let dir = tempdir().unwrap();
        let file = dir.path().join("game.html");
        fs::write(&file, "<p>Hello world</p>").unwrap();
        let plugin = HtmlGamePlugin;
        let mut entries = plugin.extract(&file).unwrap();
        entries[0].translation = Some("Hola mundo".into());
        fs::write(&file, "<p>Other words</p>").unwrap();
        let report = plugin.inject(&file, &entries).unwrap();
        assert_eq!(report.skip_reasons["source_changed"], 1);
        let other = tempdir().unwrap();
        fs::write(other.path().join("game.html"), "<p>Hello world</p>").unwrap();
        let report = plugin.inject(other.path(), &entries).unwrap();
        assert_eq!(report.skip_reasons["target_missing"], 1);
        assert_eq!(fs::read_to_string(&file).unwrap(), "<p>Other words</p>");
        assert_eq!(
            fs::read_to_string(other.path().join("game.html")).unwrap(),
            "<p>Hello world</p>"
        );
    }

    #[test]
    fn legacy_duplicate_values_require_reextraction() {
        let dir = tempdir().unwrap();
        let file = dir.path().join("game.html");
        fs::write(&file, "<p>Play</p><p>Play</p>").unwrap();
        let mut entry = StringEntry::new("legacy", "Play", file.clone());
        entry.translation = Some("Jugar".into());
        let report = HtmlGamePlugin.inject(&file, &[entry]).unwrap();
        assert_eq!(report.skip_reasons["ambiguous_target"], 1);
        assert_eq!(fs::read_to_string(file).unwrap(), "<p>Play</p><p>Play</p>");
    }

    #[test]
    fn semicolonless_entities_extract_and_inject_as_visible_text() {
        let dir = tempdir().unwrap();
        let file = dir.path().join("game.html");
        fs::write(&file, "<p title='Tom &amp Jerry'>Price &#128 and &#x80; &copy 2026</p><svg/><p>After the icon</p>").unwrap();
        let plugin = HtmlGamePlugin;
        let mut entries = plugin.extract(&file).unwrap();
        let sources: Vec<_> = entries.iter().map(|e| e.source.as_str()).collect();
        assert_eq!(
            sources,
            ["Tom & Jerry", "Price € and € © 2026", "After the icon"]
        );
        for (entry, text) in
            entries
                .iter_mut()
                .zip(["Tom y Jerry", "Precio € y € © 2026", "Después del icono"])
        {
            entry.translation = Some(text.into());
        }
        let report = plugin.inject(dir.path(), &entries).unwrap();
        assert_eq!((report.strings_written, report.strings_skipped), (3, 0));
        assert_eq!(
            fs::read_to_string(&file).unwrap(),
            "<p title='Tom y Jerry'>Precio € y € © 2026</p><svg/><p>Después del icono</p>"
        );
    }

    #[test]
    #[cfg(any(unix, windows))]
    fn injection_rejects_external_html_and_directory_links() {
        let game = tempdir().unwrap();
        let outside = tempdir().unwrap();
        let local_file = game.path().join("index.html");
        let external_file = outside.path().join("external.html");
        let html = "<p>Original outside text</p>";
        fs::write(&local_file, "<p>Original local text</p>").unwrap();
        fs::write(&external_file, html).unwrap();
        let link = game.path().join("linked");
        #[cfg(unix)]
        std::os::unix::fs::symlink(outside.path(), &link).unwrap();
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            let output = std::process::Command::new("powershell.exe")
                .args(["-NoProfile", "-NonInteractive", "-Command", "New-Item -ItemType Junction -Path $env:LOCUST_HTML_TEST_LINK -Target $env:LOCUST_HTML_TEST_TARGET -ErrorAction Stop | Out-Null"])
                .env("LOCUST_HTML_TEST_LINK", &link)
                .env("LOCUST_HTML_TEST_TARGET", outside.path())
                .creation_flags(0x08000000)
                .output().unwrap();
            assert!(
                output.status.success(),
                "junction: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        let plugin = HtmlGamePlugin;
        let mut entry = plugin.extract(&external_file).unwrap().remove(0);
        entry.translation = Some("Traducción exterior".into());
        let mut linked_entry = entry.clone();
        linked_entry.file_path = link.join("external.html");
        let result = plugin.inject(game.path(), &[entry, linked_entry]);
        // Remove only the link before TempDir cleanup or assertions can unwind.
        #[cfg(windows)]
        fs::remove_dir(&link).unwrap();
        #[cfg(unix)]
        fs::remove_file(&link).unwrap();
        let report = result.unwrap();
        assert_eq!((report.strings_written, report.files_modified), (0, 0));
        assert_eq!(report.skip_reasons.get("target_missing"), Some(&2));
        assert_eq!(fs::read_to_string(&external_file).unwrap(), html);
        assert_eq!(
            fs::read_to_string(&local_file).unwrap(),
            "<p>Original local text</p>"
        );
    }

    fn create_html_game(dir: &Path) -> PathBuf {
        let html = r#"<!DOCTYPE html>
<html>
<head><title>Adventure Game</title></head>
<body>
<script>var x = 1;</script>
<style>.red { color: red; }</style>
<div id="intro">
  <h1>Welcome to the Adventure</h1>
  <p>You find yourself in a dark forest.</p>
  <p>The trees are tall and menacing.</p>
</div>
<div id="choices">
  <button onclick="go(1)">Go north</button>
  <button onclick="go(2)">Go south</button>
</div>
<img src="forest.png" alt="A dark forest path" />
<input placeholder="Enter your name" />
</body>
</html>"#;
        let file = dir.join("game.html");
        fs::write(&file, html).unwrap();
        file
    }

    #[test]
    fn test_detect_html_game() {
        let dir = tempdir().unwrap();
        create_html_game(dir.path());
        let plugin = HtmlGamePlugin::new();
        assert!(plugin.detect(dir.path()));
    }

    #[test]
    fn test_detect_rejects_sugarcube() {
        let dir = tempdir().unwrap();
        let html =
            r#"<html><body><tw-passagedata name="Start">Hello</tw-passagedata></body></html>"#;
        fs::write(dir.path().join("game.html"), html).unwrap();
        let plugin = HtmlGamePlugin::new();
        assert!(!plugin.detect(dir.path()));
    }

    #[test]
    fn test_extract_text_elements() {
        let dir = tempdir().unwrap();
        create_html_game(dir.path());
        let plugin = HtmlGamePlugin::new();
        let entries = plugin.extract(dir.path()).unwrap();

        let sources: Vec<&str> = entries.iter().map(|e| e.source.as_str()).collect();
        assert!(sources.contains(&"Welcome to the Adventure"));
        assert!(sources.contains(&"You find yourself in a dark forest."));
        assert!(sources.contains(&"The trees are tall and menacing."));
        assert!(sources.contains(&"Go north"));
        assert!(sources.contains(&"Go south"));
        assert!(!sources.iter().any(|s| s.contains("var x")));
        assert!(!sources.iter().any(|s| s.contains("color: red")));
    }

    #[test]
    fn test_extract_attributes() {
        let dir = tempdir().unwrap();
        create_html_game(dir.path());
        let plugin = HtmlGamePlugin::new();
        let entries = plugin.extract(dir.path()).unwrap();

        let sources: Vec<&str> = entries.iter().map(|e| e.source.as_str()).collect();
        assert!(sources.contains(&"A dark forest path"));
        assert!(sources.contains(&"Enter your name"));
    }

    #[test]
    fn test_extract_title() {
        let dir = tempdir().unwrap();
        create_html_game(dir.path());
        let plugin = HtmlGamePlugin::new();
        let entries = plugin.extract(dir.path()).unwrap();

        let sources: Vec<&str> = entries.iter().map(|e| e.source.as_str()).collect();
        assert!(sources.contains(&"Adventure Game"));
    }

    #[test]
    fn test_inject_replaces_text() {
        let dir = tempdir().unwrap();
        let file = create_html_game(dir.path());
        let plugin = HtmlGamePlugin::new();
        let mut entries = plugin.extract(dir.path()).unwrap();

        for entry in &mut entries {
            match entry.source.as_str() {
                "Go north" => entry.translation = Some("Ir al norte".to_string()),
                "Go south" => entry.translation = Some("Ir al sur".to_string()),
                "You find yourself in a dark forest." => {
                    entry.translation = Some("Te encuentras en un bosque oscuro.".to_string())
                }
                _ => {}
            }
        }

        let report = plugin.inject(dir.path(), &entries).unwrap();
        assert!(report.files_modified >= 1);
        assert!(report.strings_written >= 3);

        let content = fs::read_to_string(&file).unwrap();
        assert!(content.contains("Ir al norte"));
        assert!(content.contains("Ir al sur"));
        assert!(content.contains("Te encuentras en un bosque oscuro."));
    }

    #[test]
    fn test_skips_code_like_text() {
        assert!(!is_translatable_text("var x = 1;"));
        assert!(!is_translatable_text("#ff0000"));
        assert!(!is_translatable_text("https://example.com"));
        assert!(!is_translatable_text("123.456"));
        assert!(!is_translatable_text(""));
        assert!(is_translatable_text("Hello world"));
        assert!(is_translatable_text("Go north"));
    }

    #[test]
    fn test_strip_inner_tags() {
        assert_eq!(strip_inner_tags("Hello <b>world</b>!"), "Hello world!");
        assert_eq!(strip_inner_tags("Plain text"), "Plain text");
        assert_eq!(strip_inner_tags("&amp; &lt; &gt;"), "& < >");
    }

    #[test]
    fn test_stability_is_experimental_after_phase2() {
        let plugin = HtmlGamePlugin::new();
        assert_eq!(
            plugin.stability(),
            locust_core::extraction::FormatStability::Experimental
        );
    }

    #[test]
    fn test_plugin_metadata() {
        let plugin = HtmlGamePlugin::new();
        assert_eq!(plugin.id(), "html-game");
        assert_eq!(plugin.name(), "HTML Game (Generic)");
        assert_eq!(plugin.supported_extensions(), &[".html", ".htm"]);
        assert_eq!(plugin.supported_modes(), vec![OutputMode::Replace]);
    }

    #[test]
    fn test_multi_file_game() {
        let dir = tempdir().unwrap();
        let html1 = r#"<html><body><p>Page one content</p></body></html>"#;
        let html2 = r#"<html><body><p>Page two content</p></body></html>"#;
        fs::write(dir.path().join("page1.html"), html1).unwrap();
        fs::write(dir.path().join("page2.html"), html2).unwrap();

        let plugin = HtmlGamePlugin::new();
        let entries = plugin.extract(dir.path()).unwrap();
        assert!(entries.len() >= 2);

        let sources: Vec<&str> = entries.iter().map(|e| e.source.as_str()).collect();
        assert!(sources.contains(&"Page one content"));
        assert!(sources.contains(&"Page two content"));
    }
}
