use crate::database::{Database, EntryFilter};
use crate::error::{LocustError, Result};
use crate::models::{StringEntry, StringStatus, STALE_TRANSLATION_METADATA_KEY};

use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::path::Path;

mod tmx;
pub use tmx::{export_tmx, import_tmx, tmx_units_for_batch, TmxUnit};

/// Exact stored file path intersected with a union of statuses. Omitted statuses
/// select everything; an explicitly empty list selects nothing.
#[derive(Default)]
pub struct ExportScope {
    file: Option<String>,
    statuses: Option<Vec<StringStatus>>,
}

impl ExportScope {
    pub fn parse(
        file: Option<String>,
        status: Option<&[String]>,
    ) -> std::result::Result<Self, String> {
        let statuses = status
            .map(|values| {
                values
                    .iter()
                    .map(|value| {
                        value.parse::<StringStatus>().map_err(|_| {
                            format!("invalid export status '{value}'; valid values: pending, translated, reviewed, approved, error")
                        })
                    })
                    .collect::<std::result::Result<Vec<_>, _>>()
            })
            .transpose()?;
        Ok(Self { file, statuses })
    }

    pub fn entries(&self, db: &Database) -> Result<Vec<StringEntry>> {
        // SQL equality preserves case, separators and dot components verbatim.
        let mut entries = db.get_entries(&EntryFilter {
            file_path: self.file.clone(),
            ..EntryFilter::default()
        })?;
        // Retaining a union keeps the original database order and cannot repeat rows.
        if let Some(statuses) = &self.statuses {
            entries.retain(|entry| statuses.contains(&entry.status));
        }
        Ok(entries)
    }
}

// Normalize physical file line endings before decoding XML references or PO escapes.
// XML 1.0 §2.11 requires this for the entire document, including CDATA.
fn normalize_line_endings(content: &str) -> Cow<'_, str> {
    if content.contains('\r') {
        Cow::Owned(content.replace("\r\n", "\n").replace('\r', "\n"))
    } else {
        Cow::Borrowed(content)
    }
}

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
                crate::file_identity::same_file(output, &std::fs::File::open(&protected)?)
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
        if entry.metadata.contains_key(STALE_TRANSLATION_METADATA_KEY) {
            lines.push("#, fuzzy".to_string());
        }
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
    Ok(import_po_catalog(content)?.0)
}

fn import_po_catalog(content: &str) -> Result<(Vec<PoEntry>, Vec<String>)> {
    let content = normalize_line_endings(content);
    let mut entries = Vec::new();
    let mut headers = Vec::new();
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
                if msgid.is_empty() && current_id.is_none() && current_reference_id.is_none() {
                    headers.push(msgstr);
                } else if !msgid.is_empty() {
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
        if msgid.is_empty() && current_id.is_none() && current_reference_id.is_none() {
            headers.push(msgstr);
        } else if !msgid.is_empty() {
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

    Ok((entries, headers))
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
        .replace('\r', "\\r")
        .replace('\n', "\\n")
}

fn unescape_po(s: &str) -> String {
    let mut result = String::new();
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('n') => result.push('\n'),
                Some('r') => result.push('\r'),
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

// quick-xml 0.41 splits one textual run into Text and GeneralRef events.
// Decode the original UTF-8 run once, including numeric references, with the
// shared unescaper. Keeping the run intact preserves single expansion, error
// byte ranges/precedence and TMX's per-run validation; CDATA is never included.
fn xml_text_run<'a>(
    content: &'a str,
    event: &quick_xml::events::Event<'_>,
    text_end: &mut usize,
) -> Option<&'a str> {
    use quick_xml::events::Event;
    // Reader::from_str and NsReader::from_str borrow these raw event slices
    // directly from content. Unlike buffer_position, this excludes lookahead
    // delimiters and also accounts for a skipped UTF-8 BOM.
    let start = match event {
        Event::Text(text) => text.as_ptr() as usize - content.as_ptr() as usize,
        Event::GeneralRef(reference) => reference.as_ptr() as usize - content.as_ptr() as usize - 1,
        _ => unreachable!("expected textual XML event"),
    };
    if start < *text_end {
        return None;
    }
    *text_end = start + content[start..].find('<').unwrap_or(content.len() - start);
    Some(&content[start..*text_end])
}

fn xml_parse_error(error: quick_xml::Error) -> String {
    use quick_xml::{errors::SyntaxError, Error};
    // The new lexer distinguishes these formerly shared syntax errors. Keep
    // the existing public diagnostics rather than expose dependency wording.
    match error {
        Error::Syntax(SyntaxError::UnclosedPI | SyntaxError::UnclosedXmlDecl) => {
            "syntax error: processing instruction or xml declaration not closed: `?>` not found before end of input".into()
        }
        Error::Syntax(
            SyntaxError::UnclosedSingleQuotedAttributeValue
            | SyntaxError::UnclosedDoubleQuotedAttributeValue,
        ) => "syntax error: tag not closed: `>` not found before end of input".into(),
        _ => error.to_string(),
    }
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
        let target_state = if entry.metadata.contains_key(STALE_TRANSLATION_METADATA_KEY) {
            " state=\"needs-review-translation\""
        } else {
            ""
        };
        xml.push_str(&format!(
            "      <trans-unit id=\"{}\">\n",
            escape_xml(&entry.id)
        ));
        xml.push_str(&format!(
            "        <source>{}</source>\n",
            escape_xml(&entry.source)
        ));
        xml.push_str(&format!(
            "        <target{target_state}>{}</target>\n",
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
    Ok(import_xliff_catalog(content)?.0)
}

fn import_xliff_catalog(content: &str) -> Result<(Vec<XliffUnit>, CatalogLanguages)> {
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
        target_state: Option<String>,
        has_source: bool,
        has_target: bool,
    }
    let content = normalize_line_endings(content);
    let mut reader = NsReader::from_str(&content);
    reader.config_mut().expand_empty_elements = true;
    // Let the shared unescaper retain the old unterminated-reference errors.
    reader.config_mut().allow_dangling_amp = true;
    let mut text_end = 0;
    let mut stack = Vec::new();
    let mut root_seen = false;
    let mut root_namespaced = false;
    let mut current: Option<Unit> = None;
    let mut ids = HashSet::new();
    let mut units = Vec::new();
    let mut languages = CatalogLanguages::default();
    loop {
        let (namespace, event) = reader
            .read_resolved_event()
            .map_err(|error| invalid(xml_parse_error(error)))?;
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
                let mut state = None;
                let mut source_lang = None;
                let mut target_lang = None;
                for attribute in element.attributes() {
                    let attribute = attribute.map_err(invalid)?;
                    // XLIFF historically preserves literal attribute whitespace;
                    // 0.41's normalized_value would change it to spaces.
                    let raw = std::str::from_utf8(&attribute.value).map_err(invalid)?;
                    let value = quick_xml::escape::unescape(raw)
                        .map_err(invalid)?
                        .into_owned();
                    match attribute.key.as_ref() {
                        b"id" => id = Some(value),
                        b"version" => version = Some(value),
                        b"state" => state = Some(value),
                        b"source-language" => source_lang = Some(value),
                        b"target-language" => target_lang = Some(value),
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
                        (b"file", Some(Element::Root)) => {
                            languages.source.push(source_lang.unwrap_or_default());
                            languages.target.push(target_lang.unwrap_or_default());
                            Element::File
                        }
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
                                target_state: None,
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
                            unit.target_state = state;
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
            event @ (Event::Text(_) | Event::GeneralRef(_)) => {
                let Some(raw) = xml_text_run(&content, &event, &mut text_end) else {
                    continue;
                };
                let value = quick_xml::escape::unescape(raw).map_err(invalid)?;
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
                        target_state: unit.target_state,
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
    Ok((units, languages))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct XliffUnit {
    pub id: String,
    pub source: String,
    pub target: String,
    /// XLIFF 1.2 target state; absent in older serialized units.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_state: Option<String>,
}

/// Collect `(id, translation)` pairs for [`crate::database::Database::save_translations_batch`].
/// An import carries the source it was translated from, not only its row ID.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportedTranslation {
    pub id: String,
    pub source: String,
    pub translation: String,
}

/// Concrete tags use ASCII case folding and hyphens. Matching uses the primary
/// subtag; two explicitly different regions warn rather than rejecting a pair.
pub fn normalize_import_language(value: &str) -> Option<String> {
    let tag = value.trim().replace('_', "-").to_ascii_lowercase();
    let mut parts = tag.split('-');
    let primary = parts.next()?;
    ((2..=8).contains(&primary.len())
        && primary.bytes().all(|b| b.is_ascii_alphabetic())
        && !["auto", "und", "mul", "zxx"].contains(&primary)
        && parts
            .all(|p| (1..=8).contains(&p.len()) && p.bytes().all(|b| b.is_ascii_alphanumeric())))
    .then_some(tag)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ImportLanguagePair {
    pub source: Option<String>,
    pub target: Option<String>,
}

/// The latest run records semantic languages, including EN->ES in a pivot
/// whose immutable injection source is still Japanese. Before the first run,
/// callers supply their configured/explicit pair. Pivot copies of project
/// metadata and physical provenance are deliberately not language evidence.
pub fn project_import_languages(
    db: &Database,
    fallback_source: &str,
    fallback_target: &str,
) -> Result<ImportLanguagePair> {
    let runs = db.get_translation_runs()?;
    let (source, target) = runs
        .last()
        .map(|run| (run.source_lang.as_str(), run.target_lang.as_str()))
        .unwrap_or((fallback_source, fallback_target));
    Ok(ImportLanguagePair {
        source: normalize_import_language(source),
        target: normalize_import_language(target),
    })
}

#[derive(Debug, Clone, Serialize)]
pub struct CatalogLanguages {
    pub source: Vec<String>,
    pub target: Vec<String>,
    pub source_required: bool,
}

impl Default for CatalogLanguages {
    fn default() -> Self {
        Self {
            source: Vec::new(),
            target: Vec::new(),
            source_required: true,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ImportLanguageWarning {
    pub code: &'static str,
    pub side: &'static str,
    pub declared: Vec<String>,
    pub expected: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ImportLanguageValidation {
    pub project: ImportLanguagePair,
    pub declared: CatalogLanguages,
    pub warnings: Vec<ImportLanguageWarning>,
    pub requires_confirmation: bool,
}

impl ImportLanguageValidation {
    pub fn require_confirmation(&self, allow_undeclared_language: bool) -> Result<()> {
        if self.requires_confirmation && !allow_undeclared_language {
            return Err(LocustError::Other(anyhow::anyhow!(
                "catalog language not declared or ambiguous (or project language unknown); review the preview and explicitly confirm language not declared — import anyway with allow_undeclared_language=true / --allow-undeclared-language"
            )));
        }
        Ok(())
    }
}

fn language_region(tag: &str) -> Option<&str> {
    tag.split('-').skip(1).find(|part| {
        (part.len() == 2 && part.bytes().all(|b| b.is_ascii_alphabetic()))
            || (part.len() == 3 && part.bytes().all(|b| b.is_ascii_digit()))
    })
}

pub fn validate_import_languages(
    declared: &CatalogLanguages,
    project: &ImportLanguagePair,
) -> Result<ImportLanguageValidation> {
    let mut validation = ImportLanguageValidation {
        project: project.clone(),
        declared: declared.clone(),
        warnings: Vec::new(),
        requires_confirmation: false,
    };
    for (side, values, expected, required) in [
        (
            "source",
            &declared.source,
            &project.source,
            declared.source_required,
        ),
        ("target", &declared.target, &project.target, true),
    ] {
        let mut tags: Vec<_> = values
            .iter()
            .filter_map(|v| normalize_import_language(v))
            .collect();
        tags.sort();
        tags.dedup();
        if let Some(expected) = expected {
            for tag in &tags {
                if tag.split('-').next() != expected.split('-').next() {
                    return Err(LocustError::Other(anyhow::anyhow!(
                        "catalog {side} language '{tag}' does not match project {side} language '{expected}'; import refused"
                    )));
                }
            }
            if tags.iter().any(|tag| {
                matches!((language_region(tag), language_region(expected)), (Some(a), Some(b)) if a != b)
            }) {
                validation.warnings.push(ImportLanguageWarning {
                    code: "region", side, declared: values.clone(), expected: Some(expected.clone()),
                });
            }
        } else if required {
            validation.requires_confirmation = true;
            validation.warnings.push(ImportLanguageWarning {
                code: "project_undeclared",
                side,
                declared: values.clone(),
                expected: None,
            });
        }
        if required
            && (tags.len() != 1
                || values
                    .iter()
                    .any(|v| normalize_import_language(v).is_none()))
        {
            validation.requires_confirmation = true;
            validation.warnings.push(ImportLanguageWarning {
                code: "undeclared",
                side,
                declared: values.clone(),
                expected: expected.clone(),
            });
        }
    }
    Ok(validation)
}

pub struct ParsedImportCatalog {
    pub updates: Vec<ImportedTranslation>,
    pub skipped: usize,
    pub languages: CatalogLanguages,
}

/// All transports retain declarations and use the same skip rules. Language
/// validation must precede row matching and any write, even for empty catalogs.
pub fn parse_import_catalog(
    format: &str,
    content: &str,
    project: &ImportLanguagePair,
) -> Result<ParsedImportCatalog> {
    let ((updates, skipped), languages) = match format {
        "po" => {
            let (entries, headers) = import_po_catalog(content)?;
            let target = headers
                .iter()
                .flat_map(|header| header.lines())
                .filter_map(|line| {
                    let (key, value) = line.split_once(':')?;
                    key.trim()
                        .eq_ignore_ascii_case("Language")
                        .then(|| value.trim().to_owned())
                })
                .collect();
            (
                po_entries_for_batch(&entries),
                CatalogLanguages {
                    source: Vec::new(),
                    target,
                    source_required: false,
                },
            )
        }
        "xliff" => {
            let (units, languages) = import_xliff_catalog(content)?;
            (xliff_units_for_batch(&units), languages)
        }
        "tmx" => return parse_import_tmx(content, project),
        other => return Err(LocustError::UnsupportedFormat(other.into())),
    };
    Ok(ParsedImportCatalog {
        updates,
        skipped,
        languages,
    })
}

fn parse_import_tmx(content: &str, project: &ImportLanguagePair) -> Result<ParsedImportCatalog> {
    let invalid = |message: String| LocustError::ParseError {
        file: "tmx".into(),
        message: format!("TMX profile error: {message}"),
    };
    if content.len() > 32 * 1024 * 1024 {
        return Err(invalid("document exceeds 32 MiB".into()));
    }
    let mut reader = quick_xml::Reader::from_str(content);
    let mut sources = Vec::new();
    let mut variants = Vec::new();
    let mut edits = Vec::new();
    let mut missing_source_at = None;
    let mut source_range = None;
    let mut header_seen = false;
    loop {
        match reader.read_event().map_err(|e| invalid(e.to_string()))? {
            event @ (quick_xml::events::Event::Start(_) | quick_xml::events::Event::Empty(_)) => {
                let empty = matches!(event, quick_xml::events::Event::Empty(_));
                let element = match event {
                    quick_xml::events::Event::Start(element)
                    | quick_xml::events::Event::Empty(element) => element,
                    _ => unreachable!(),
                };
                let key = match element.name().as_ref() {
                    b"header" | b"tu" => b"srclang".as_slice(),
                    b"tuv" => b"xml:lang".as_slice(),
                    _ => continue,
                };
                let is_header = element.name().as_ref() == b"header";
                if is_header {
                    if header_seen {
                        return Err(invalid("multiple headers".into()));
                    }
                    header_seen = true;
                }
                // Slice-backed Reader events and their attributes borrow
                // the original tag. Replace only language value bytes;
                // reserializing tags could repair malformed XML separators.
                let tag_start = reader.buffer_position() as usize
                    - element.as_ref().len()
                    - if empty { 2 } else { 1 };
                let mut found = false;
                for attr in element.attributes() {
                    let attr = attr.map_err(|e| invalid(e.to_string()))?;
                    if attr.key.as_ref() != key {
                        continue;
                    }
                    let raw =
                        std::str::from_utf8(&attr.value).map_err(|e| invalid(e.to_string()))?;
                    let lang = quick_xml::escape::unescape(raw)
                        .map_err(|e| invalid(e.to_string()))?
                        .into_owned();
                    found = true;
                    let start = tag_start + attr.value.as_ptr() as usize
                        - element.as_ref().as_ptr() as usize;
                    let range = start..start + attr.value.len();
                    if key == b"srclang" {
                        sources.push(lang.clone());
                        if is_header {
                            source_range = Some(range.clone());
                        }
                    } else {
                        variants.push(lang.clone());
                    }
                    if let Some(normalized) = normalize_import_language(&lang) {
                        if raw != normalized {
                            edits.push((range, normalized));
                        }
                    }
                }
                if is_header && !found {
                    sources.push(String::new());
                    let position = tag_start + element.name().as_ref().len();
                    missing_source_at = Some(position);
                }
            }
            quick_xml::events::Event::Eof => break,
            _ => {}
        }
    }
    let declared_source = sources.first().and_then(|v| normalize_import_language(v));
    let source = declared_source
        .clone()
        .or_else(|| {
            project.source.as_ref().and_then(|expected| {
                let tags: Vec<_> = variants
                    .iter()
                    .filter_map(|v| normalize_import_language(v))
                    .collect();
                tags.iter()
                    .find(|tag| *tag == expected)
                    .cloned()
                    .or_else(|| {
                        tags.into_iter()
                            .find(|tag| tag.split('-').next() == expected.split('-').next())
                    })
            })
        })
        .or_else(|| variants.first().and_then(|v| normalize_import_language(v)))
        .or_else(|| project.source.clone())
        .unwrap_or_else(|| "en".into());
    if declared_source.is_none() && !variants.is_empty() {
        // Variant tags still constrain the inferred orientation; the
        // absent/ambiguous header continues to require confirmation.
        sources.push(source.clone());
    }
    let mut targets: Vec<_> = variants
        .into_iter()
        .filter(|v| normalize_import_language(v).as_ref() != Some(&source))
        .collect();
    targets.sort();
    targets.dedup();
    // Empty catalogs have no variants. Either distinct concrete target
    // validates the same empty document; incomplete units still fail.
    let target = targets
        .first()
        .and_then(|v| normalize_import_language(v))
        .unwrap_or_else(|| {
            if source.eq_ignore_ascii_case("en") {
                "es"
            } else {
                "en"
            }
            .into()
        });
    if declared_source.is_none() {
        if let Some(range) = source_range {
            edits.push((range, source.clone()));
        } else if let Some(at) = missing_source_at {
            edits.push((at..at, format!(" srclang=\"{source}\"")));
        }
    }
    edits.sort_by_key(|(range, _)| range.start);
    let normalized = if edits.is_empty() {
        Cow::Borrowed(content)
    } else {
        let mut output = String::with_capacity(content.len());
        let mut cursor = 0;
        for (range, value) in edits {
            output.push_str(&content[cursor..range.start]);
            output.push_str(&value);
            cursor = range.end;
        }
        output.push_str(&content[cursor..]);
        Cow::Owned(output)
    };
    let (updates, skipped) = tmx_units_for_batch(&import_tmx(&normalized, &source, &target)?);
    sources.sort();
    sources.dedup();
    Ok(ParsedImportCatalog {
        updates,
        skipped,
        languages: CatalogLanguages {
            source: sources,
            target: targets,
            source_required: true,
        },
    })
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

/// Same as [`po_entries_for_batch`]: empty or needs-review targets are skipped.
pub fn xliff_units_for_batch(units: &[XliffUnit]) -> (Vec<ImportedTranslation>, usize) {
    let mut skipped = 0usize;
    let mut updates = Vec::with_capacity(units.len());
    for unit in units {
        if unit.target.is_empty()
            || matches!(
                unit.target_state.as_deref(),
                Some("needs-review-translation" | "needs-review-adaptation" | "needs-review-l10n")
            )
        {
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
        .replace('\r', "&#13;")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn import_pair(source: &str, target: &str) -> ImportLanguagePair {
        ImportLanguagePair {
            source: normalize_import_language(source),
            target: normalize_import_language(target),
        }
    }

    fn language_catalog(format: &str, source: &str, target: &str) -> String {
        let mut row = StringEntry::new("id", "Source", "story.rpy".into());
        row.translation = Some("Traducción".into());
        match format {
            "po" => export_po(&[row], source, target),
            "xliff" => export_xliff(&[row], source, target),
            "tmx" => export_tmx(&[row], source, target).unwrap(),
            _ => unreachable!(),
        }
    }

    #[test]
    fn imported_languages_match_primary_subtags_and_warn_for_conflicting_regions() {
        let project = import_pair("en_US", "es-MX");
        for format in ["po", "xliff", "tmx"] {
            for target in ["es", "ES-mx", "es-ES"] {
                let content = language_catalog(format, "en-US", target);
                let parsed = parse_import_catalog(format, &content, &project).unwrap();
                let check = validate_import_languages(&parsed.languages, &project).unwrap();
                assert!(!check.requires_confirmation, "{format}: {target}");
                assert_eq!(check.warnings.len(), usize::from(target == "es-ES"));
                check.require_confirmation(false).unwrap();
                assert_eq!(parsed.updates[0].translation, "Traducción");
            }
            let content = language_catalog(format, "en-US", "es-ES").replace("es-ES", "es_ES");
            let parsed = parse_import_catalog(format, &content, &project).unwrap();
            let check = validate_import_languages(&parsed.languages, &project).unwrap();
            assert_eq!(check.warnings[0].code, "region");
        }
    }

    #[test]
    fn declared_wrong_pairs_are_refused_before_any_rows_are_considered() {
        let project = import_pair("en", "fr");
        for format in ["po", "xliff", "tmx"] {
            let parsed =
                parse_import_catalog(format, &language_catalog(format, "en", "es"), &project)
                    .unwrap();
            let message = validate_import_languages(&parsed.languages, &project)
                .unwrap_err()
                .to_string();
            assert!(message.contains("target language 'es'") && message.contains("'fr'"));
        }
        for format in ["xliff", "tmx"] {
            let parsed =
                parse_import_catalog(format, &language_catalog(format, "fr", "en"), &project)
                    .unwrap();
            assert!(validate_import_languages(&parsed.languages, &project).is_err());
        }
    }

    #[test]
    fn missing_declarations_require_confirmation_including_tmx_orientation() {
        let project = import_pair("en", "es");
        for format in ["po", "xliff", "tmx"] {
            let content = language_catalog(format, "en", "es")
                .replace("\"Language: es\\n\"", "")
                .replace("source-language=\"en\" target-language=\"es\"", "")
                .replace("srclang=\"en\"", "");
            let parsed = parse_import_catalog(format, &content, &project).unwrap();
            let check = validate_import_languages(&parsed.languages, &project).unwrap();
            assert!(check.requires_confirmation, "{format}");
            assert!(check.require_confirmation(false).is_err());
            check.require_confirmation(true).unwrap();
            assert_eq!(parsed.updates[0].source, "Source");
            assert_eq!(parsed.updates[0].translation, "Traducción");
        }
        // Variant order never reverses a TMX with an undeclared header.
        let content = language_catalog("tmx", "en", "es").replace("srclang=\"en\"", "srclang=\"*all*\"")
            .replace("<tuv xml:lang=\"en\"><seg>Source</seg></tuv><tuv xml:lang=\"es\"><seg>Traducción</seg></tuv>",
                "<tuv xml:lang=\"es\"><seg>Traducción</seg></tuv><tuv xml:lang=\"en\"><seg>Source</seg></tuv>");
        let parsed = parse_import_catalog("tmx", &content, &project).unwrap();
        assert_eq!(parsed.updates[0].source, "Source");
        assert!(
            validate_import_languages(&parsed.languages, &project)
                .unwrap()
                .requires_confirmation
        );
    }

    #[test]
    fn ambiguous_headers_warn_but_known_conflicts_cannot_be_overridden() {
        let project = import_pair("en", "es-MX");
        for language in ["", "und", "es_ES\\nLanguage: es-MX"] {
            let content = language_catalog("po", "en", language);
            let parsed = parse_import_catalog("po", &content, &project).unwrap();
            assert!(
                validate_import_languages(&parsed.languages, &project)
                    .unwrap()
                    .requires_confirmation
            );
        }
        let content = language_catalog("po", "en", "es\\nLanguage: fr");
        let parsed = parse_import_catalog("po", &content, &project).unwrap();
        assert!(validate_import_languages(&parsed.languages, &project).is_err());
        let content = language_catalog("xliff", "en", "es-MX").replace(
            "</xliff>",
            "<file source-language=\"en\" target-language=\"es-ES\"><body/></file></xliff>",
        );
        let parsed = parse_import_catalog("xliff", &content, &project).unwrap();
        assert!(
            validate_import_languages(&parsed.languages, &project)
                .unwrap()
                .requires_confirmation
        );
    }

    #[test]
    fn tmx_language_normalization_preserves_strict_xml_and_segment_bytes() {
        let project = import_pair("en-US", "es-ES");
        let content = language_catalog("tmx", "en-US", "es-ES")
            .replace("en-US", "en_US")
            .replace("es-ES", "es_ES");
        let parsed = parse_import_catalog("tmx", &content, &project).unwrap();
        assert_eq!(parsed.updates[0].source, "Source");
        assert_eq!(parsed.updates[0].translation, "Traducción");
        for broken in [
            content.replace(" segtype=", "segtype="),
            content.replace("</tmx>", "</tmx>late"),
        ] {
            assert!(parse_import_catalog("tmx", &broken, &project).is_err());
        }
    }

    #[tokio::test]
    async fn project_import_pair_uses_semantic_runs_and_not_pivot_physical_metadata() {
        use crate::database::TranslationRun;
        let db = Database::open_in_memory().unwrap();
        let mut row = StringEntry::new("id", "日本語", "story.rpy".into());
        row.translation = Some("Source".into());
        row.status = StringStatus::Translated;
        db.save_entries(&[row]).unwrap();
        db.set_project_metadata("source_lang", &serde_json::json!("ja"))
            .unwrap();
        db.record_translation_run(&TranslationRun {
            source_lang: "ja".into(),
            target_lang: "en".into(),
            ..Default::default()
        })
        .await
        .unwrap();
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("pivot.db");
        db.pivot_to(&path).unwrap();
        let pivot = Database::open(&path).unwrap();
        assert_eq!(
            pivot
                .get_entry("id")
                .unwrap()
                .unwrap()
                .injection_source()
                .unwrap(),
            "日本語"
        );
        assert_eq!(
            project_import_languages(&pivot, "en", "es").unwrap(),
            import_pair("en", "es")
        );
        pivot
            .record_translation_run(&TranslationRun {
                source_lang: "en".into(),
                target_lang: "es".into(),
                ..Default::default()
            })
            .await
            .unwrap();
        let pair = project_import_languages(&pivot, "ja", "fr").unwrap();
        assert_eq!(pair, import_pair("en", "es"));
        let wrong =
            parse_import_catalog("xliff", &language_catalog("xliff", "ja", "es"), &pair).unwrap();
        assert!(validate_import_languages(&wrong.languages, &pair).is_err());
    }
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

    fn assert_xliff_document_line_endings(ending: &str) {
        let input = concat!(
            "<xliff><file><body><trans-unit id=\" story#0&amp;日本語 \t\">",
            "<source>  First\n<![CDATA[second\nthird]]>\t </source>",
            "<target>  Uno\n<![CDATA[dos\ntres]]>\t </target>",
            "</trans-unit></body></file></xliff>"
        )
        .replace('\n', ending);
        let units = import_xliff(&input).unwrap();
        assert_eq!(units.len(), 1);
        assert_eq!(units[0].id, " story#0&日本語 \t");
        assert_eq!(units[0].source, "  First\nsecond\nthird\t ");
        assert_eq!(units[0].target, "  Uno\ndos\ntres\t ");
    }

    #[test]
    fn xliff_crlf_document_normalizes_text_and_cdata() {
        assert_xliff_document_line_endings("\r\n");
    }

    #[test]
    fn xliff_lone_cr_document_normalizes_text_and_cdata() {
        assert_xliff_document_line_endings("\r");
    }

    #[test]
    fn xliff_line_endings_do_not_normalize_character_references() {
        let units = import_xliff(concat!(
            "<xliff>\r\n<file><body><trans-unit id=\"id&#13;&#xD;\">",
            "<source>A\r\nB\rC&#13;D&#xD;\n&amp;#13;</source>",
            "<target>X&#13;\r\nY&#xD;\rZ</target>",
            "</trans-unit></body></file></xliff>"
        ))
        .unwrap();
        assert_eq!(units[0].id, "id\r\r");
        assert_eq!(units[0].source, "A\nB\nC\rD\r\n&#13;");
        assert_eq!(units[0].target, "X\r\nY\r\nZ");
    }

    #[test]
    fn xliff_roundtrip_preserves_crlf_cr_and_lf_content() {
        for ending in ["\r\n", "\r", "\n"] {
            let mut entry = StringEntry::new(
                format!(" story#0{ending}&日本語 "),
                format!("  First{ending}second\t & <tag> &#13; "),
                "story.json".into(),
            );
            entry.translation = Some(format!("  Uno{ending}dos\t &#xD; "));
            let exported = export_xliff(std::slice::from_ref(&entry), "en", "es");
            assert!(!exported.contains('\r'), "literal CR must be a reference");
            if ending.contains('\r') {
                assert!(exported.contains("&#13;"));
            }
            for file_ending in ["\n", "\r\n", "\r"] {
                let units = import_xliff(&exported.replace('\n', file_ending)).unwrap();
                assert_eq!(units[0].id, entry.id);
                assert_eq!(units[0].source, entry.source);
                assert_eq!(Some(units[0].target.as_str()), entry.translation.as_deref());
            }
        }
    }

    #[test]
    fn po_crlf_document_preserves_multiline_strings() {
        let input = concat!(
            "msgctxt \" story#0&日本語 \"\n",
            "msgid \"  First\\n\"\n\"second\\nthird\t \"\n",
            "msgstr \"  Uno\\n\"\n\"dos\\ntres\t \"\n"
        )
        .replace('\n', "\r\n");
        let entries = import_po(&input).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].id.as_deref(), Some(" story#0&日本語 "));
        assert_eq!(entries[0].source, "  First\nsecond\nthird\t ");
        assert_eq!(entries[0].translation, "  Uno\ndos\ntres\t ");
    }

    #[test]
    fn po_lone_cr_document_preserves_multiline_strings() {
        let input = "msgctxt \"id\"\rmsgid \"First\\nsecond\"\rmsgstr \"Uno\\ndos\"\r";
        let entries = import_po(input).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].id.as_deref(), Some("id"));
        assert_eq!(entries[0].source, "First\nsecond");
        assert_eq!(entries[0].translation, "Uno\ndos");
    }

    #[test]
    fn po_line_endings_do_not_normalize_escaped_cr() {
        let entries = import_po(concat!(
            "msgctxt \"id\\r\"\r\n",
            "msgid \"A\\r\\nB\\rC\\nD\\\\r\"\r\n",
            "msgstr \"X\\r\\nY\\rZ\\nW\\\\r\"\r\n"
        ))
        .unwrap();
        assert_eq!(entries[0].id.as_deref(), Some("id\r"));
        assert_eq!(entries[0].source, "A\r\nB\rC\nD\\r");
        assert_eq!(entries[0].translation, "X\r\nY\rZ\nW\\r");
    }

    #[test]
    fn po_roundtrip_preserves_crlf_cr_and_lf_content() {
        for ending in ["\r\n", "\r", "\n"] {
            let mut entry = StringEntry::new(
                format!(" story#0{ending}&日本語 "),
                format!("  First{ending}second\t \\r "),
                "story.json".into(),
            );
            entry.translation = Some(format!("  Uno{ending}dos\t \\r "));
            let exported = export_po(std::slice::from_ref(&entry), "en", "es");
            assert!(!exported.contains('\r'), "literal CR must be escaped");
            for file_ending in ["\n", "\r\n", "\r"] {
                let entries = import_po(&exported.replace('\n', file_ending)).unwrap();
                assert_eq!(entries[0].id.as_deref(), Some(entry.id.as_str()));
                assert_eq!(entries[0].source, entry.source);
                assert_eq!(
                    Some(entries[0].translation.as_str()),
                    entry.translation.as_deref()
                );
            }
        }
    }

    #[test]
    fn xliff_prefixes_entities_cdata_and_whitespace_preserve_exact_text() {
        let input = r#"<?xml version="1.0"?>
<x:xliff xmlns:x="urn:oasis:names:tc:xliff:document:1.2" version="1.2">
 <x:file><x:body><x:group><x:trans-unit id="entry&amp;日本語">
  <x:source>  A &amp; B <![CDATA[<tag>]]>  </x:source>
  <x:target><![CDATA[ Español <tag> ]]>&amp; &#x65E5; &amp;unknown; &amp;#x1F980; &#x1F980;</x:target>
 </x:trans-unit></x:group></x:body></x:file>
</x:xliff>"#;
        let units = import_xliff(input).unwrap();
        assert_eq!(units.len(), 1);
        assert_eq!(units[0].id, "entry&日本語");
        assert_eq!(units[0].source, "  A & B <tag>  ");
        assert_eq!(
            units[0].target,
            " Español <tag> & 日 &unknown; &#x1F980; 🦀"
        );
    }

    #[test]
    fn xliff_entity_only_fields_decode_once_including_supplementary_references() {
        for (encoded, decoded) in [
            ("&lt;&gt;&amp;&apos;&quot;", "<>&'\""),
            ("&#x1F980;&#129408;", "🦀🦀"),
            (
                "&amp;lt;&amp;#129408;&amp;unknown;",
                "&lt;&#129408;&unknown;",
            ),
        ] {
            let input = format!(
                "<xliff><file><body><trans-unit id=\"{encoded}\"><source>{encoded}</source><target>{encoded}</target></trans-unit></body></file></xliff>"
            );
            let units = import_xliff(&input).unwrap();
            assert_eq!(units.len(), 1);
            assert_eq!(units[0].id, decoded);
            assert_eq!(units[0].source, decoded);
            assert_eq!(units[0].target, decoded);
        }
    }

    #[test]
    fn xliff_preserves_existing_attribute_whitespace_and_character_policy() {
        let units = import_xliff(
            "<xliff><file><body><trans-unit id=\"a\tb\nc&#13;&amp;#13;\"><source>&#1;&#xFFFE;</source><target><![CDATA[&amp;]]>&amp;lt;</target></trans-unit></body></file></xliff>",
        )
        .unwrap();
        assert_eq!(units[0].id, "a\tb\nc\r&#13;");
        // XLIFF has never enforced TMX's stricter XML 1.0 character policy.
        assert_eq!(units[0].source, "\u{1}\u{fffe}");
        assert_eq!(units[0].target, "&amp;&lt;");
    }

    #[test]
    fn xliff_reference_errors_keep_whole_text_run_byte_ranges() {
        for (text, message) in [
            (
                "🦀&amp;&unknown;",
                "at 10..17: unrecognized entity `unknown`",
            ),
            (
                "A&amp;B&unfinished",
                "Error while escaping character at range 7..18: Cannot find ';' after '&'",
            ),
        ] {
            let xml = format!(
                "<xliff><file><body><trans-unit id=\"id\"><source>{text}</source></trans-unit></body></file></xliff>"
            );
            assert_eq!(
                import_xliff(&xml).unwrap_err().to_string(),
                format!("parse error in xliff: XLIFF parse error: {message}")
            );
        }
    }

    #[test]
    fn xliff_foreign_namespace_local_names_do_not_become_translation_fields() {
        let units = import_xliff(
            r#"<xliff xmlns="urn:oasis:names:tc:xliff:document:1.2" xmlns:m="urn:metadata">
            <file><body>
              <m:trans-unit id="ignored"><m:source>Noise</m:source><m:target>Noise</m:target></m:trans-unit>
              <trans-unit id="id"><source>Source</source><m:target>Foreign</m:target><target>Target</target></trans-unit>
            </body></file></xliff>"#,
        )
        .unwrap();
        assert_eq!(units.len(), 1);
        assert_eq!(units[0].id, "id");
        assert_eq!(units[0].source, "Source");
        assert_eq!(units[0].target, "Target");
    }

    #[test]
    fn xliff_rejects_invalid_numeric_references_in_attributes_and_text() {
        // Invalid Unicode scalars and malformed numbers, not a new XML policy.
        for reference in [
            "&#xD800;",
            "&#1114112;",
            "&#x110000;",
            "&#;",
            "&#x;",
            "&#-1;",
        ] {
            for (id, source, target) in [
                (reference, "Source", "Target"),
                ("id", reference, "Target"),
                ("id", "Source", reference),
            ] {
                let input = format!(
                    "<xliff><file><body><trans-unit id=\"{id}\"><source>{source}</source><target>{target}</target></trans-unit></body></file></xliff>"
                );
                assert!(import_xliff(&input).is_err(), "accepted: {input}");
            }
        }
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
                target_state: None,
            },
            XliffUnit {
                id: "b".into(),
                source: "B".into(),
                target: String::new(),
                target_state: None,
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
