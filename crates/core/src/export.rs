use crate::error::{LocustError, Result};
use crate::models::StringEntry;

use serde::{Deserialize, Serialize};
use std::path::Path;

/// Refuse destinations that would replace the project or its SQLite sidecars.
pub fn check_export_destination(output: &Path, project_db: &Path) -> Result<()> {
    let project = project_db.canonicalize()?;
    let destination = match output.canonicalize() {
        Ok(path) => path,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let name = output.file_name().ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "invalid export destination",
                )
            })?;
            let parent = output
                .parent()
                .filter(|path| !path.as_os_str().is_empty())
                .unwrap_or_else(|| Path::new("."));
            parent.canonicalize()?.join(name)
        }
        Err(error) => return Err(error.into()),
    };

    let output_handle = if output.try_exists()? {
        Some(std::fs::File::open(output)?)
    } else {
        None
    };
    for suffix in ["", "-wal", "-shm", "-journal"] {
        let mut name = project.as_os_str().to_os_string();
        name.push(suffix);
        let protected = std::path::PathBuf::from(name);
        let same_path = crate::database::paths_identical(&destination, &protected);
        let same_file = match &output_handle {
            Some(output) if protected.try_exists()? => {
                crate::project::saved_db_handles_match(output, &std::fs::File::open(&protected)?)
            }
            _ => false,
        };
        if same_path || same_file {
            let target = if suffix.is_empty() {
                "the project database itself".to_string()
            } else {
                format!("the project database's SQLite {suffix} sidecar")
            };
            return Err(LocustError::Other(anyhow::anyhow!(
                "the export destination is {target}; choose a different file"
            )));
        }
    }
    Ok(())
}

// ─── PO format ─────────────────────────────────────────────────────────────

pub fn export_po(entries: &[StringEntry], source_lang: &str, target_lang: &str) -> String {
    let mut lines = Vec::new();

    // Header
    lines.push("# Project Locust export".to_string());
    lines.push(format!(
        "# Source: {}, Target: {}",
        source_lang, target_lang
    ));
    lines.push(String::new());
    lines.push("msgid \"\"".to_string());
    lines.push("msgstr \"\"".to_string());
    lines.push("\"Content-Type: text/plain; charset=UTF-8\\n\"".to_string());
    lines.push("\"Content-Transfer-Encoding: 8bit\\n\"".to_string());
    lines.push(format!("\"Language: {}\\n\"", target_lang));
    lines.push(String::new());

    // Entries
    for entry in entries {
        if let Some(ref ctx) = entry.context {
            lines.push(format!("#. {}", ctx));
        }
        // File location only — Locust ids often contain `#` (e.g. file#idx#key).
        // Putting the id after path# and re-importing with rfind broke multi-# ids.
        lines.push(format!("#: {}", entry.file_path.display()));
        // Full entry id for lossless import (preferred over legacy path#id).
        lines.push(format!("msgctxt \"{}\"", escape_po(&entry.id)));
        lines.push(format!("msgid \"{}\"", escape_po(&entry.source)));
        let translation = entry.translation.as_deref().unwrap_or("");
        lines.push(format!("msgstr \"{}\"", escape_po(translation)));
        lines.push(String::new());
    }

    lines.join("\n")
}

pub fn import_po(content: &str) -> Result<Vec<PoEntry>> {
    let mut entries = Vec::new();
    let mut current_id: Option<String> = None;
    let mut current_reference_id: Option<String> = None;
    let mut current_msgid: Option<String> = None;
    let mut current_msgstr: Option<String> = None;
    let mut current_fuzzy = false;
    let mut entry_complete = false;
    let mut reading = ReadingState::None;

    for (index, line) in content.lines().enumerate() {
        let trimmed = line.trim();
        let line_number = index + 1;
        let (keyword, rest) = trimmed
            .split_once(char::is_whitespace)
            .unwrap_or((trimmed, ""));
        let starts_entry = matches!(keyword, "msgctxt" | "msgid")
            || trimmed.starts_with("#,")
            || trimmed.starts_with("#: ");

        if trimmed.is_empty() || (entry_complete && starts_entry) {
            // Flush current entry
            if let (Some(msgid), Some(msgstr)) = (current_msgid.take(), current_msgstr.take()) {
                if !msgid.is_empty() {
                    entries.push(PoEntry {
                        id: current_id
                            .take()
                            .filter(|id| !id.is_empty())
                            .or(current_reference_id.take()),
                        source: msgid,
                        translation: msgstr,
                        fuzzy: current_fuzzy,
                    });
                }
            }
            current_id = None;
            current_reference_id = None;
            current_fuzzy = false;
            entry_complete = false;
            reading = ReadingState::None;
            if trimmed.is_empty() {
                continue;
            }
        }

        if let Some(flags) = trimmed.strip_prefix("#,") {
            current_fuzzy |= flags.split(',').any(|flag| flag.trim() == "fuzzy");
            continue;
        }

        if let Some(reference) = trimmed.strip_prefix("#: ") {
            // Keep the legacy `#: path#id` fallback separate until msgctxt and
            // all its continuations have been read; an empty context uses it.
            if current_reference_id.is_none() {
                // First `#` separates path from id so multi-# ids stay intact.
                if let Some(hash_pos) = reference.find('#') {
                    current_reference_id = Some(reference[hash_pos + 1..].to_string());
                }
            }
            continue;
        }

        if trimmed.starts_with("#") {
            continue;
        }

        if keyword == "msgctxt" {
            let val = parse_po_string(rest, line_number)?;
            current_id = Some(unescape_po(&val));
            reading = ReadingState::Msgctxt;
            continue;
        }

        if keyword == "msgid" {
            let val = parse_po_string(rest, line_number)?;
            current_msgid = Some(unescape_po(&val));
            reading = ReadingState::Msgid;
            continue;
        }

        if keyword == "msgstr" {
            let val = parse_po_string(rest, line_number)?;
            current_msgstr = Some(unescape_po(&val));
            entry_complete = true;
            reading = ReadingState::Msgstr;
            continue;
        }

        if keyword == "msgid_plural"
            || keyword
                .strip_prefix("msgstr[")
                .and_then(|index| index.strip_suffix(']'))
                .is_some_and(|index| !index.is_empty() && index.bytes().all(|b| b.is_ascii_digit()))
        {
            parse_po_string(rest, line_number)?;
            // Plural entries are not imported; validate and discard their continuations.
            current_msgid = None;
            current_msgstr = None;
            entry_complete = true;
            reading = ReadingState::None;
            continue;
        }

        // Every non-comment, non-directive line must be a quoted continuation.
        let val = parse_po_string(trimmed, line_number)?;
        let unescaped = unescape_po(&val);
        match reading {
            ReadingState::Msgctxt => {
                if let Some(ref mut s) = current_id {
                    s.push_str(&unescaped);
                }
            }
            ReadingState::Msgid => {
                if let Some(ref mut s) = current_msgid {
                    s.push_str(&unescaped);
                }
            }
            ReadingState::Msgstr => {
                if let Some(ref mut s) = current_msgstr {
                    s.push_str(&unescaped);
                }
            }
            ReadingState::None => {}
        }
    }

    // Flush last entry
    if let (Some(msgid), Some(msgstr)) = (current_msgid, current_msgstr) {
        if !msgid.is_empty() {
            entries.push(PoEntry {
                id: current_id
                    .filter(|id| !id.is_empty())
                    .or(current_reference_id),
                source: msgid,
                translation: msgstr,
                fuzzy: current_fuzzy,
            });
        }
    }

    Ok(entries)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PoEntry {
    pub id: Option<String>,
    pub source: String,
    pub translation: String,
    #[serde(default)]
    pub fuzzy: bool,
}

enum ReadingState {
    None,
    Msgctxt,
    Msgid,
    Msgstr,
}

fn escape_po(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
}

fn unescape_po(s: &str) -> String {
    let mut result = String::new();
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('n') => result.push('\n'),
                Some('"') => result.push('"'),
                Some('\\') => result.push('\\'),
                Some(other) => {
                    result.push('\\');
                    result.push(other);
                }
                None => result.push('\\'),
            }
        } else {
            result.push(c);
        }
    }
    result
}

fn parse_po_string(s: &str, line: usize) -> Result<String> {
    let trimmed = s.trim();
    if trimmed.starts_with('"') {
        let mut backslashes = 0;
        for (index, byte) in trimmed.bytes().enumerate().skip(1) {
            // The first unescaped quote closes the string. Nothing but
            // whitespace may follow it, even another quoted string.
            if byte == b'"' && backslashes % 2 == 0 {
                if index == trimmed.len() - 1 {
                    return Ok(trimmed[1..index].to_string());
                }
                break;
            }
            backslashes = if byte == b'\\' { backslashes + 1 } else { 0 };
        }
    }
    let excerpt: String = trimmed.chars().take(80).collect();
    Err(LocustError::ParseError {
        file: "po".into(),
        message: format!("line {line}: malformed PO string: {excerpt}"),
    })
}

// ─── XLIFF format ──────────────────────────────────────────────────────────

pub fn export_xliff(entries: &[StringEntry], source_lang: &str, target_lang: &str) -> String {
    let mut xml = String::new();
    xml.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    xml.push_str("<xliff version=\"1.2\" xmlns=\"urn:oasis:names:tc:xliff:document:1.2\">\n");
    xml.push_str(&format!(
        "  <file source-language=\"{}\" target-language=\"{}\" datatype=\"plaintext\">\n",
        escape_xml(source_lang),
        escape_xml(target_lang)
    ));
    xml.push_str("    <body>\n");

    for entry in entries {
        let translation = entry.translation.as_deref().unwrap_or("");
        xml.push_str(&format!(
            "      <trans-unit id=\"{}\">\n",
            escape_xml(&entry.id)
        ));
        xml.push_str(&format!(
            "        <source>{}</source>\n",
            escape_xml(&entry.source)
        ));
        xml.push_str(&format!(
            "        <target>{}</target>\n",
            escape_xml(translation)
        ));
        xml.push_str("      </trans-unit>\n");
    }

    xml.push_str("    </body>\n");
    xml.push_str("  </file>\n");
    xml.push_str("</xliff>\n");
    xml
}

/// Import the plain-text XLIFF 1.2 emitted by Locust, with optional XML
/// namespace prefixes and CDATA. Unsupported inline codes fail explicitly:
/// silently flattening them would drop protected game controls.
pub fn import_xliff(content: &str) -> Result<Vec<XliffUnit>> {
    use quick_xml::{events::Event, name::ResolveResult, NsReader};
    use std::collections::HashSet;

    fn invalid(message: impl std::fmt::Display) -> LocustError {
        LocustError::ParseError {
            file: "xliff".into(),
            message: format!("XLIFF parse error: {message}"),
        }
    }
    #[derive(Clone, Copy, PartialEq)]
    enum Element {
        Root,
        File,
        Body,
        Group,
        Unit,
        Source,
        Target,
        Other,
    }
    struct Unit {
        id: String,
        source: String,
        target: String,
        has_source: bool,
        has_target: bool,
    }
    let mut reader = NsReader::from_str(content);
    reader.config_mut().expand_empty_elements = true;
    let mut stack = Vec::new();
    let mut root_seen = false;
    let mut root_namespaced = false;
    let mut current: Option<Unit> = None;
    let mut ids = HashSet::new();
    let mut units = Vec::new();
    loop {
        let (namespace, event) = reader.read_resolved_event().map_err(invalid)?;
        let namespaced = match namespace {
            ResolveResult::Bound(ns) => ns.as_ref() == b"urn:oasis:names:tc:xliff:document:1.2",
            ResolveResult::Unbound => false,
            ResolveResult::Unknown(_) => return Err(invalid("undeclared XML namespace prefix")),
        };
        let is_xliff =
            namespaced || (!root_namespaced && matches!(namespace, ResolveResult::Unbound));
        match event {
            Event::Start(element) => {
                let name = element.local_name();
                let parent = stack.last().copied();
                if matches!(parent, Some(Element::Source | Element::Target)) {
                    return Err(invalid("inline XLIFF elements are unsupported; export source and target as plain text with escaped game controls"));
                }
                // Check all attributes, including metadata, rather than silently
                // ignoring duplicate attributes or broken entity references.
                let mut id = None;
                let mut version = None;
                for attribute in element.attributes() {
                    let attribute = attribute.map_err(invalid)?;
                    let value = attribute.unescape_value().map_err(invalid)?.into_owned();
                    match attribute.key.as_ref() {
                        b"id" => id = Some(value),
                        b"version" => version = Some(value),
                        _ => {}
                    }
                }
                let kind = if parent.is_none() {
                    if root_seen || name.as_ref() != b"xliff" || !is_xliff {
                        return Err(invalid("expected one XLIFF 1.2 document"));
                    }
                    if version.as_deref().is_some_and(|v| v != "1.2") {
                        return Err(invalid("only XLIFF 1.2 is supported"));
                    }
                    root_seen = true;
                    root_namespaced = namespaced;
                    Element::Root
                } else if is_xliff {
                    match (name.as_ref(), parent) {
                        (b"file", Some(Element::Root)) => Element::File,
                        (b"body", Some(Element::File)) => Element::Body,
                        (b"group", Some(Element::Body | Element::Group)) => Element::Group,
                        (b"trans-unit", Some(Element::Body | Element::Group)) => {
                            let id = id
                                .filter(|id| !id.is_empty())
                                .ok_or_else(|| invalid("translation unit has no id"))?;
                            if !ids.insert(id.clone()) {
                                return Err(invalid("duplicate translation unit id"));
                            }
                            current = Some(Unit {
                                id,
                                source: String::new(),
                                target: String::new(),
                                has_source: false,
                                has_target: false,
                            });
                            Element::Unit
                        }
                        (b"source", Some(Element::Unit)) => {
                            let unit = current
                                .as_mut()
                                .ok_or_else(|| invalid("source outside unit"))?;
                            if unit.has_source {
                                return Err(invalid("duplicate source in translation unit"));
                            }
                            unit.has_source = true;
                            Element::Source
                        }
                        (b"target", Some(Element::Unit)) => {
                            let unit = current
                                .as_mut()
                                .ok_or_else(|| invalid("target outside unit"))?;
                            if unit.has_target {
                                return Err(invalid("duplicate target in translation unit"));
                            }
                            unit.has_target = true;
                            Element::Target
                        }
                        // Translation elements in a wrong position are not
                        // metadata; refusing them avoids a false zero-row import.
                        (b"trans-unit", _) => {
                            return Err(invalid("translation unit outside a file body or group"))
                        }
                        _ => Element::Other,
                    }
                } else {
                    Element::Other
                };
                stack.push(kind);
            }
            Event::Text(text) => {
                let value = text.unescape().map_err(invalid)?;
                match stack.last() {
                    Some(Element::Source) => current.as_mut().unwrap().source.push_str(&value),
                    Some(Element::Target) => current.as_mut().unwrap().target.push_str(&value),
                    None if !value.trim().is_empty() => {
                        return Err(invalid("text outside the document root"))
                    }
                    _ => {}
                }
            }
            Event::CData(text) => {
                let value = std::str::from_utf8(text.as_ref()).map_err(invalid)?;
                match stack.last() {
                    Some(Element::Source) => current.as_mut().unwrap().source.push_str(value),
                    Some(Element::Target) => current.as_mut().unwrap().target.push_str(value),
                    None => return Err(invalid("CDATA outside the document root")),
                    _ => {}
                }
            }
            Event::End(_) => {
                let kind = stack
                    .pop()
                    .ok_or_else(|| invalid("unmatched closing element"))?;
                if kind == Element::Unit {
                    let unit = current
                        .take()
                        .ok_or_else(|| invalid("missing translation unit"))?;
                    if !unit.has_source {
                        return Err(invalid("translation unit has no source"));
                    }
                    units.push(XliffUnit {
                        id: unit.id,
                        source: unit.source,
                        target: unit.target,
                    });
                }
            }
            Event::DocType(_) => return Err(invalid("DOCTYPE declarations are unsupported")),
            Event::Eof => {
                if !root_seen || !stack.is_empty() || current.is_some() {
                    return Err(invalid("empty or incomplete XLIFF document"));
                }
                break;
            }
            _ => {}
        }
    }
    Ok(units)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct XliffUnit {
    pub id: String,
    pub source: String,
    pub target: String,
}

/// Collect `(id, translation)` pairs for [`crate::database::Database::save_translations_batch`].
/// An import carries the source it was translated from, not only its row ID.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportedTranslation {
    pub id: String,
    pub source: String,
    pub translation: String,
}

/// Empty msgstr / missing id / fuzzy count toward `skipped`; the database checks source identity.
pub fn po_entries_for_batch(entries: &[PoEntry]) -> (Vec<ImportedTranslation>, usize) {
    let mut skipped = 0usize;
    let mut updates = Vec::with_capacity(entries.len());
    for pe in entries {
        if pe.fuzzy || pe.translation.is_empty() {
            skipped += 1;
            continue;
        }
        let Some(ref id) = pe.id else {
            skipped += 1;
            continue;
        };
        updates.push(ImportedTranslation {
            id: id.clone(),
            source: pe.source.clone(),
            translation: pe.translation.clone(),
        });
    }
    (updates, skipped)
}

/// Same as [`po_entries_for_batch`] for XLIFF units (empty target → skipped).
pub fn xliff_units_for_batch(units: &[XliffUnit]) -> (Vec<ImportedTranslation>, usize) {
    let mut skipped = 0usize;
    let mut updates = Vec::with_capacity(units.len());
    for unit in units {
        if unit.target.is_empty() {
            skipped += 1;
            continue;
        }
        updates.push(ImportedTranslation {
            id: unit.id.clone(),
            source: unit.source.clone(),
            translation: unit.target.clone(),
        });
    }
    (updates, skipped)
}

/// After a batch apply: unknown ids become extra skips.
pub fn import_counts_after_batch(
    pre_skipped: usize,
    attempted: usize,
    applied: usize,
) -> (usize /* imported */, usize /* skipped */) {
    let unknown = attempted.saturating_sub(applied);
    (applied, pre_skipped + unknown)
}

fn escape_xml(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn export_destination_refuses_database() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("project.locust.db");
        std::fs::write(&db, b"project").unwrap();
        for output in [&db, &dir.path().join("./project.locust.db")] {
            let err = check_export_destination(output, &db).unwrap_err();
            assert!(err.to_string().contains("project database itself"), "{err}");
        }
        assert_eq!(std::fs::read(&db).unwrap(), b"project");
    }

    #[test]
    fn export_destination_refuses_existing_and_missing_sidecars() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("project.locust.db");
        std::fs::write(&db, b"project").unwrap();
        for suffix in ["-wal", "-shm", "-journal"] {
            let sidecar = dir.path().join(format!("project.locust.db{suffix}"));
            for exists in [false, true] {
                if exists {
                    std::fs::write(&sidecar, b"sqlite sidecar").unwrap();
                }
                let err = check_export_destination(&sidecar, &db).unwrap_err();
                assert!(err.to_string().contains("SQLite"), "{err}");
                assert!(err.to_string().contains(suffix), "{err}");
                assert_eq!(sidecar.exists(), exists);
                if exists {
                    assert_eq!(std::fs::read(&sidecar).unwrap(), b"sqlite sidecar");
                }
            }
        }
    }

    #[test]
    fn export_destination_allows_unrelated_file() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("project.locust.db");
        let catalog = dir.path().join("catalog.po");
        std::fs::write(&db, b"project").unwrap();
        std::fs::write(&catalog, b"translator edits").unwrap();
        check_export_destination(&catalog, &db).unwrap();
        assert_eq!(std::fs::read(&catalog).unwrap(), b"translator edits");
    }

    #[test]
    fn export_destination_allows_missing_output() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("project.locust.db");
        let catalog = dir.path().join("catalog.po");
        std::fs::write(&db, b"project").unwrap();
        check_export_destination(&catalog, &db).unwrap();
        assert!(!catalog.exists());
    }

    #[test]
    fn export_destination_refuses_hard_links() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("project.locust.db");
        std::fs::write(&db, b"project").unwrap();
        for suffix in ["", "-wal", "-shm", "-journal"] {
            let protected = dir.path().join(format!("project.locust.db{suffix}"));
            if !suffix.is_empty() {
                std::fs::write(&protected, b"sqlite sidecar").unwrap();
            }
            let alias = dir.path().join(format!("catalog{suffix}.po"));
            std::fs::hard_link(&protected, &alias).unwrap();
            let err = check_export_destination(&alias, &db).unwrap_err();
            assert!(err.to_string().contains("choose a different file"), "{err}");
        }
    }

    #[cfg(windows)]
    #[test]
    fn export_destination_refuses_windows_case_aliases() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("Project.locust.db");
        std::fs::write(&db, b"project").unwrap();
        for suffix in ["", "-WAL", "-SHM", "-JOURNAL"] {
            let output = dir.path().join(format!("PROJECT.LOCUST.DB{suffix}"));
            assert!(check_export_destination(&output, &db).is_err());
        }
    }

    #[cfg(unix)]
    #[test]
    fn export_destination_refuses_symlink_aliases() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("project.locust.db");
        let alias = dir.path().join("catalog.po");
        std::fs::write(&db, b"project").unwrap();
        std::os::unix::fs::symlink(&db, &alias).unwrap();
        assert!(check_export_destination(&alias, &db).is_err());
        let parent_alias = dir.path().join("linked");
        std::os::unix::fs::symlink(dir.path(), &parent_alias).unwrap();
        assert!(
            check_export_destination(&parent_alias.join("project.locust.db-wal"), &db).is_err()
        );
    }

    fn make_entries() -> Vec<StringEntry> {
        let mut e1 = StringEntry::new("e1", "Hello", PathBuf::from("test.json"));
        e1.translation = Some("Hola".to_string());
        e1.context = Some("greeting".to_string());

        let mut e2 = StringEntry::new("e2", "World", PathBuf::from("test.json"));
        e2.translation = Some("Mundo".to_string());

        let e3 = StringEntry::new("e3", "Untranslated", PathBuf::from("test.json"));

        vec![e1, e2, e3]
    }

    #[test]
    fn test_export_po_header() {
        let entries = make_entries();
        let po = export_po(&entries, "ja", "en");
        assert!(po.starts_with("# Project Locust export"));
        assert!(po.contains("\"Language: en\\n\""));
    }

    #[test]
    fn test_export_po_entries() {
        let entries = make_entries();
        let po = export_po(&entries, "ja", "en");
        let msgid_count = po.matches("msgid \"").count();
        // 1 header msgid + 3 entry msgids
        assert_eq!(msgid_count, 4);
    }

    #[test]
    fn test_export_po_empty_translation() {
        let entries = make_entries();
        let po = export_po(&entries, "ja", "en");
        // e3 has no translation
        assert!(po.contains("msgid \"Untranslated\"\nmsgstr \"\""));
    }

    #[test]
    fn test_import_po_roundtrip() {
        let entries = make_entries();
        let po = export_po(&entries, "ja", "en");
        let imported = import_po(&po).unwrap();
        assert_eq!(imported.len(), 3);
        assert_eq!(imported[0].source, "Hello");
        assert_eq!(imported[0].translation, "Hola");
        assert_eq!(imported[0].id.as_deref(), Some("e1"));
        assert_eq!(imported[1].source, "World");
        assert_eq!(imported[1].translation, "Mundo");
        assert_eq!(imported[2].source, "Untranslated");
        assert_eq!(imported[2].translation, "");
        assert!(
            po.contains("msgctxt \"e1\""),
            "export must write msgctxt ids"
        );
    }

    #[test]
    fn test_import_po_fuzzy_entries_are_skipped() {
        for context in ["msgctxt \"entry\"\n", "#: game.rpy#entry\n"] {
            for translation in ["Hola", ""] {
                for ending in ["", "\n\n"] {
                    let po = format!(
                        "#, fuzzy\n{context}msgid \"Hello\"\nmsgstr \"{translation}\"{ending}"
                    );
                    let imported = import_po(&po).unwrap();
                    assert_eq!(imported.len(), 1);
                    assert!(imported[0].fuzzy, "{po}");
                    assert_eq!(imported[0].id.as_deref(), Some("entry"));
                    let (updates, skipped) = po_entries_for_batch(&imported);
                    assert!(updates.is_empty(), "{po}");
                    assert_eq!(skipped, 1, "{po}");
                }
            }
        }
    }

    #[test]
    fn test_import_po_fuzzy_flags_match_exactly() {
        for (comments, fuzzy) in [
            ("#, fuzzy, python-format", true),
            ("#, c-format, fuzzy", true),
            ("#,c-format,  fuzzy  , python-format", true),
            ("#, fuzzy\n#, python-format", true),
            ("#, python-format", false),
            ("#, Fuzzy, fuzzy-format, not-fuzzy", false),
            ("# fuzzy", false),
            ("#| msgid \"old\"", false),
            ("#| msgid \"#, fuzzy\"", false),
            ("#~ msgid \"#, fuzzy\"\n#~ msgstr \"old\"", false),
            ("#~ #, fuzzy", false),
        ] {
            let po = format!("{comments}\nmsgctxt \"entry\"\nmsgid \"Hello\"\nmsgstr \"Hola\"");
            let imported = import_po(&po).unwrap();
            assert_eq!(imported.len(), 1, "{po}");
            assert_eq!(imported[0].fuzzy, fuzzy, "{po}");
            let (updates, skipped) = po_entries_for_batch(&imported);
            assert_eq!(updates.len(), usize::from(!fuzzy), "{po}");
            assert_eq!(skipped, usize::from(fuzzy), "{po}");
        }
    }

    #[test]
    fn test_import_po_fuzzy_does_not_leak_between_entries() {
        for separator in ["\n", "\n\n"] {
            for fuzzy_first in [true, false] {
                for use_context in [true, false] {
                    let entry = |id, fuzzy| {
                        let flags = if fuzzy { "#, fuzzy\n" } else { "" };
                        let context = if use_context {
                            format!("msgctxt \"{id}\"\n")
                        } else {
                            format!("#: game.rpy#{id}\n")
                        };
                        format!("{flags}{context}msgid \"Hello\"\nmsgstr \"Hola\"")
                    };
                    let po = format!(
                        "{}{separator}{}",
                        entry("first", fuzzy_first),
                        entry("second", !fuzzy_first)
                    );
                    let imported = import_po(&po).unwrap();
                    assert_eq!(imported.len(), 2, "{po}");
                    assert_eq!(imported[0].fuzzy, fuzzy_first, "{po}");
                    assert_eq!(imported[1].fuzzy, !fuzzy_first, "{po}");
                    let (updates, skipped) = po_entries_for_batch(&imported);
                    assert_eq!(skipped, 1, "{po}");
                    assert_eq!(updates.len(), 1, "{po}");
                    assert_eq!(updates[0].id, if fuzzy_first { "second" } else { "first" });
                }
            }
        }
    }

    #[test]
    fn test_import_po_fuzzy_resets_after_discarded_entries() {
        for prefix in [
            "#, fuzzy\n\n",
            "#, fuzzy\nmsgid \"\"\nmsgstr \"\"\n\"Language: es\\n\"\n",
            "#, fuzzy\nmsgid \"One\"\nmsgid_plural \"Many\"\nmsgstr[0] \"Uno\"\n",
        ] {
            let po = format!("{prefix}msgctxt \"entry\"\nmsgid \"Hello\"\nmsgstr \"Hola\"");
            let imported = import_po(&po).unwrap();
            assert_eq!(imported.len(), 1, "{po}");
            assert!(!imported[0].fuzzy, "{po}");
            let (updates, skipped) = po_entries_for_batch(&imported);
            assert_eq!(updates.len(), 1, "{po}");
            assert_eq!(skipped, 0, "{po}");
        }
    }

    #[test]
    fn test_import_po_fuzzy_preserves_wrapped_strings() {
        let po = concat!(
            "#, fuzzy\n#: game.rpy#legacy\nmsgctxt \"\"\n\"game.rpy\"\n\"#42\"\n",
            "msgid \"Hello \"\n\"world\\n\"\n",
            "msgstr \"Hola \"\n# Translator comment\n\"mundo\\n\"",
        );
        let imported = import_po(po).unwrap();
        assert_eq!(imported.len(), 1);
        assert!(imported[0].fuzzy);
        assert_eq!(imported[0].id.as_deref(), Some("game.rpy#42"));
        assert_eq!(imported[0].source, "Hello world\n");
        assert_eq!(imported[0].translation, "Hola mundo\n");
        let (updates, skipped) = po_entries_for_batch(&imported);
        assert!(updates.is_empty());
        assert_eq!(skipped, 1);
    }

    #[test]
    fn test_po_entry_fuzzy_defaults_false_and_export_is_not_fuzzy() {
        let entry: PoEntry = serde_json::from_value(serde_json::json!({
            "id": "entry", "source": "Hello", "translation": "Hola"
        }))
        .unwrap();
        assert!(!entry.fuzzy);
        let po = export_po(&make_entries(), "en", "es");
        let imported = import_po(&po).unwrap();
        assert!(imported.iter().all(|entry| !entry.fuzzy));
        let (updates, skipped) = po_entries_for_batch(&imported);
        assert_eq!(updates.len(), 2);
        assert_eq!(skipped, 1);
    }

    #[test]
    fn test_import_po_preserves_multi_hash_ids() {
        // VNTP / Ren'Py style ids: file#index#key — must survive export→import.
        let mut e = StringEntry::new(
            "S004b.ks.json#0#message",
            "Hello there",
            PathBuf::from(r"C:\work\S004b.ks.json"),
        );
        e.translation = Some("Hola".to_string());
        let po = export_po(std::slice::from_ref(&e), "en", "es");
        assert!(
            po.contains("msgctxt \"S004b.ks.json#0#message\""),
            "msgctxt missing full id: {po}"
        );
        assert!(
            !po.contains("#: C:\\work\\S004b.ks.json#S004b"),
            "must not glue path#id (legacy broken form): {po}"
        );
        let imported = import_po(&po).unwrap();
        assert_eq!(imported.len(), 1);
        assert_eq!(
            imported[0].id.as_deref(),
            Some("S004b.ks.json#0#message"),
            "id truncated on import"
        );
        assert_eq!(imported[0].translation, "Hola");
    }

    #[test]
    fn test_import_po_legacy_path_hash_id_uses_first_hash() {
        // Pre-msgctxt Locust export: `#: path#id` with multi-# id.
        let legacy = r#"
msgid ""
msgstr ""
"Language: es\n"

#: C:\work\S004b.ks.json#S004b.ks.json#0#message
msgid "Hello"
msgstr "Hola"
"#;
        let imported = import_po(legacy).unwrap();
        assert_eq!(imported.len(), 1);
        assert_eq!(
            imported[0].id.as_deref(),
            Some("S004b.ks.json#0#message"),
            "legacy import must keep full id after first #, got {:?}",
            imported[0].id
        );
    }

    #[test]
    fn test_import_po_continued_empty_context() {
        let po = "msgctxt \"\"\n\"game.rpy\"\n\"#42\"\nmsgid \"Hello\"\nmsgstr \"Hola\"\n\n";
        let imported = import_po(po).unwrap();
        assert_eq!(imported.len(), 1);
        assert_eq!(imported[0].id.as_deref(), Some("game.rpy#42"));
    }

    #[test]
    fn test_import_po_continued_context_does_not_use_prefix_id() {
        let po = "msgctxt \"line1\"\n\"0\"\nmsgid \"Hello\"\nmsgstr \"Hola\"";
        let imported = import_po(po).unwrap();
        assert_eq!(imported.len(), 1);
        assert_eq!(imported[0].id.as_deref(), Some("line10"));
    }

    #[test]
    fn test_import_po_empty_context_falls_back_to_reference() {
        for context in ["msgctxt \"\"\n", "msgctxt \"\"\n\"\"\n"] {
            for prefix in [
                format!("#: game.rpy#legacy#42\n{context}"),
                format!("{context}#: game.rpy#legacy#42\n"),
            ] {
                for ending in ["", "\n\n"] {
                    let po = format!("{prefix}msgid \"Hello\"\nmsgstr \"Hola\"{ending}");
                    let imported = import_po(&po).unwrap();
                    assert_eq!(imported.len(), 1);
                    assert_eq!(imported[0].id.as_deref(), Some("legacy#42"), "{po}");
                }
            }
        }
    }

    #[test]
    fn test_import_po_continued_context_overrides_reference_and_resets() {
        let po = concat!(
            "#: game.rpy#legacy\nmsgctxt \"\"\n",
            "# A comment does not interrupt the context.\n\"line1\"\n\"0\"\n",
            "#: game.rpy#other\nmsgid \"Hello\"\nmsgstr \"Hola\"\n\n",
            "msgid \"Next\"\nmsgstr \"Siguiente\"\n\n",
            "msgctxt \"\"\nmsgid \"Last\"\nmsgstr \"Último\"",
        );
        let imported = import_po(po).unwrap();
        assert_eq!(imported.len(), 3);
        assert_eq!(imported[0].id.as_deref(), Some("line10"));
        assert_eq!(imported[1].id, None);
        assert_eq!(imported[2].id, None);
    }

    #[test]
    fn test_import_po_skips_plural_entries_between_singular_entries() {
        let po = concat!(
            "msgctxt \"first\"\nmsgid \"Hello\"\nmsgstr \"Hola\"\n\n",
            "msgctxt \"plural\"\nmsgid \"One \"\n\"item\"\n",
            "msgid_plural \"Many \"\n\"items\"\n",
            "msgstr[0] \"Un \"\n\"elemento\"\n",
            "msgstr[1] \"Varios \"\n\"elementos\"\n",
            "msgstr[10] \"Otros \"\n\"elementos\"\n\n",
            "msgctxt \"last\"\nmsgid \"Good\"\n\"bye\"\nmsgstr \"Adi\"\n\"ós\"",
        );
        for ending in ["", "\n\n"] {
            let imported = import_po(&format!("{po}{ending}")).unwrap();
            assert_eq!(imported.len(), 2);
            assert_eq!(imported[0].id.as_deref(), Some("first"));
            assert_eq!(imported[0].source, "Hello");
            assert_eq!(imported[0].translation, "Hola");
            assert_eq!(imported[1].id.as_deref(), Some("last"));
            assert_eq!(imported[1].source, "Goodbye");
            assert_eq!(imported[1].translation, "Adiós");
        }
    }

    #[test]
    fn test_import_po_rejects_unknown_keywords() {
        for keyword in [
            "msgfoo",
            "msgid_plurals",
            "msgstr[]",
            "msgstr[-1]",
            "msgstr[1x]",
            "msgstr[١]",
            "msgstr[1]suffix",
        ] {
            let po = format!("msgid \"Hello\"\n{keyword} \"x\"\n");
            let LocustError::ParseError { file, message } = import_po(&po).expect_err(&po) else {
                panic!("expected ParseError for {po}");
            };
            assert_eq!(file, "po");
            assert!(
                message.starts_with("line 2: malformed PO string:"),
                "{message}"
            );
            assert!(message.contains(keyword), "{message}");
        }
    }

    #[test]
    fn test_import_po_rejects_malformed_strings_with_line_numbers() {
        for malformed in [
            "garbage",
            "\"Hola",
            "\"Hola\" trailing junk",
            "\"abc\\\"",
            "\"abc\\\\\\\"",
            "\"Hola\" \"extra\"",
            "\"Hola\" junk\"",
            "\"",
            "",
        ] {
            // A complete preceding entry must not hide a later parse failure.
            let prefix = "# Comment\nmsgctxt \"valid\"\nmsgid \"First\"\nmsgstr \"Uno\"\n\n";
            for (directive, preceding) in [
                ("msgctxt", ""),
                ("msgid", "msgctxt \"second\"\n"),
                ("msgstr", "msgctxt \"second\"\nmsgid \"Second\"\n"),
                ("msgid_plural", "msgid \"One item\"\n"),
                (
                    "msgstr[0]",
                    "msgid \"One item\"\nmsgid_plural \"Many items\"\n",
                ),
                (
                    "msgstr[1]",
                    "msgid \"One item\"\nmsgid_plural \"Many items\"\nmsgstr[0] \"Un elemento\"\n",
                ),
            ] {
                for continuation in [false, true] {
                    // A blank continuation is a separator, not a malformed string.
                    if continuation && malformed.is_empty() {
                        continue;
                    }
                    let input = if continuation {
                        format!("{prefix}{preceding}{directive} \"\"\n{malformed}\n")
                    } else {
                        format!("{prefix}{preceding}{directive} {malformed}\n")
                    };
                    let line = prefix.lines().count()
                        + preceding.lines().count()
                        + 1
                        + usize::from(continuation);
                    let err = import_po(&input).expect_err(&input);
                    let LocustError::ParseError { file, message } = err else {
                        panic!("expected ParseError for {input}");
                    };
                    assert_eq!(file, "po");
                    assert!(
                        message.starts_with(&format!("line {line}: malformed PO string:")),
                        "{message}: {input}"
                    );
                    assert!(message.contains(malformed), "{message}: {input}");
                }
            }
        }
    }

    #[test]
    fn test_import_po_continuations_unescape_and_preserve_whitespace() {
        let po = concat!(
            "msgctxt \"\"\n\"game\\\\path\\\"\"\n\"#42\\n日本語\"  \t\n",
            "msgid \"Hello \"\n\"\\\"world\\\"\\n\"\n\"\\\\\"\n",
            "msgstr \"Hola \"\n\"\\\"mundo\\\"\\n\"\n\"\\\\\\\\\"\n",
        );
        let imported = import_po(po).unwrap();
        assert_eq!(imported.len(), 1);
        assert_eq!(imported[0].id.as_deref(), Some("game\\path\"#42\n日本語"));
        assert_eq!(imported[0].source, "Hello \"world\"\n\\");
        assert_eq!(imported[0].translation, "Hola \"mundo\"\n\\\\");
    }

    #[test]
    fn test_import_po_roundtrip_tricky_strings() {
        let values = [
            "game.rpy#42#日本語 \"quote\" \\path\nnext\ttab",
            "ends in a quote\"",
            "ends in a backslash\\",
            "ends in two backslashes\\\\",
            "backslash and quote\\\"",
            r"literal escapes: \n \t \q",
            "  espacios, español, 日本語\t ",
        ];
        let entries: Vec<_> = values
            .iter()
            .map(|value| {
                let mut entry = StringEntry::new(*value, *value, PathBuf::from("game.rpy"));
                entry.translation = Some((*value).into());
                entry
            })
            .collect();
        let po = export_po(&entries, "en", "es");
        let imported = import_po(&po).unwrap();
        assert_eq!(imported.len(), entries.len());
        for (actual, expected) in imported.iter().zip(&entries) {
            assert_eq!(actual.id.as_deref(), Some(expected.id.as_str()));
            assert_eq!(actual.source, expected.source);
            assert_eq!(
                Some(actual.translation.as_str()),
                expected.translation.as_deref()
            );
        }
    }

    #[test]
    fn test_export_xliff_structure() {
        let entries = make_entries();
        let xliff = export_xliff(&entries, "ja", "en");
        assert!(xliff.contains("<xliff version=\"1.2\""));
        assert!(xliff.contains("source-language=\"ja\""));
        assert!(xliff.contains("target-language=\"en\""));
        assert!(xliff.contains("<trans-unit id=\"e1\">"));
        assert!(xliff.contains("<source>Hello</source>"));
        assert!(xliff.contains("<target>Hola</target>"));
    }

    #[test]
    fn xliff_prefixes_entities_cdata_and_whitespace_preserve_exact_text() {
        let input = r#"<?xml version="1.0"?>
<x:xliff xmlns:x="urn:oasis:names:tc:xliff:document:1.2" version="1.2">
 <x:file><x:body><x:group><x:trans-unit id="entry&amp;日本語">
  <x:source>  A &amp; B <![CDATA[<tag>]]>  </x:source>
  <x:target><![CDATA[ Español <tag> ]]>&amp; &#x65E5;</x:target>
 </x:trans-unit></x:group></x:body></x:file>
</x:xliff>"#;
        let units = import_xliff(input).unwrap();
        assert_eq!(units.len(), 1);
        assert_eq!(units[0].id, "entry&日本語");
        assert_eq!(units[0].source, "  A & B <tag>  ");
        assert_eq!(units[0].target, " Español <tag> & 日");
    }

    #[test]
    fn xliff_rejects_incomplete_ambiguous_or_lossy_inputs_before_returning_any_units() {
        let wrap =
            |body: &str| format!("<xliff version=\"1.2\"><file><body>{body}</body></file></xliff>");
        for input in [
            String::new(),
            "<other/>".into(),
            "<xliff version=\"2.0\"/>".into(),
            "<q:xliff/>".into(),
            "<xliff xmlns=\"urn:not-xliff\"/>".into(),
            "<xliff><file><body><trans-unit id=\"a\"><source>S</source><target>T</target>".into(),
            "<xliff/><xliff/>".into(),
            "<!DOCTYPE xliff><xliff/>".into(),
            wrap("<trans-unit><source>S</source></trans-unit>"),
            wrap("<trans-unit id=\"\"><source>S</source></trans-unit>"),
            wrap("<trans-unit id=\"a\" id=\"b\"><source>S</source></trans-unit>"),
            wrap("<trans-unit id=\"a\"><target>T</target></trans-unit>"),
            wrap("<trans-unit id=\"a\"><source>S</source><target>A</target><target>B</target></trans-unit>"),
            wrap("<trans-unit id=\"a\"><source>S</source><target>&unknown;</target></trans-unit>"),
            wrap("<trans-unit id=\"a\"><source>S</source><target>Text <ph id=\"control\"/> lost</target></trans-unit>"),
            wrap("<trans-unit id=\"a\"><source>S</source></trans-unit><trans-unit id=\"a\"><source>S</source></trans-unit>"),
            "<xliff><trans-unit id=\"a\"><source>S</source></trans-unit></xliff>".into(),
        ] {
            assert!(import_xliff(&input).is_err(), "accepted lossy or invalid input: {input}");
        }
    }

    #[test]
    fn xliff_empty_targets_do_not_borrow_another_units_text() {
        let units = import_xliff(
            r#"<xliff><file><body>
            <trans-unit id="a"><source/><target>Target</target></trans-unit>
            <trans-unit id="b"><source>Source</source><target/></trans-unit>
            <trans-unit id="c"><source>Third</source></trans-unit>
        </body></file></xliff>"#,
        )
        .unwrap();
        assert_eq!(units.len(), 3);
        assert_eq!(units[0].source, "");
        assert_eq!(units[0].target, "Target");
        assert_eq!(units[1].target, "");
        assert_eq!(units[2].target, "");
    }

    #[test]
    fn test_import_xliff_roundtrip() {
        let entries = make_entries();
        let xliff = export_xliff(&entries, "ja", "en");
        let imported = import_xliff(&xliff).unwrap();
        assert_eq!(imported.len(), 3);
        assert_eq!(imported[0].id, "e1");
        assert_eq!(imported[0].source, "Hello");
        assert_eq!(imported[0].target, "Hola");
        assert_eq!(imported[2].source, "Untranslated");
        assert_eq!(imported[2].target, "");
    }

    /// Pin: empty/missing rows stay out of the batch; unknown ids inflate skipped
    /// after apply (negative: treating unknown as imported would under-count skips).
    #[test]
    fn test_po_entries_for_batch_skips_empty_and_missing_id() {
        let entries = [
            PoEntry {
                id: Some("a".into()),
                source: "A".into(),
                translation: "Á".into(),
                fuzzy: false,
            },
            PoEntry {
                id: Some("b".into()),
                source: "B".into(),
                translation: String::new(),
                fuzzy: false,
            },
            PoEntry {
                id: None,
                source: "C".into(),
                translation: "Cé".into(),
                fuzzy: false,
            },
        ];
        let (updates, pre_skipped) = po_entries_for_batch(&entries);
        assert_eq!(updates.len(), 1);
        assert_eq!(updates[0].id, "a");
        assert_eq!(updates[0].source, "A");
        assert_eq!(pre_skipped, 2);
        let (imported, skipped) = import_counts_after_batch(pre_skipped, updates.len(), 0);
        assert_eq!(imported, 0);
        assert_eq!(skipped, 3, "unknown id must count as skipped, not imported");
    }

    #[test]
    fn test_xliff_units_for_batch_skips_empty_target() {
        let units = [
            XliffUnit {
                id: "a".into(),
                source: "A".into(),
                target: "Á".into(),
            },
            XliffUnit {
                id: "b".into(),
                source: "B".into(),
                target: String::new(),
            },
        ];
        let (updates, pre_skipped) = xliff_units_for_batch(&units);
        assert_eq!(
            updates,
            vec![ImportedTranslation {
                id: "a".into(),
                source: "A".into(),
                translation: "Á".into()
            }]
        );
        assert_eq!(pre_skipped, 1);
        let (imported, skipped) = import_counts_after_batch(pre_skipped, 1, 1);
        assert_eq!((imported, skipped), (1, 1));
    }
}
