//! Restricted bilingual TMX 1.4b Level-1 project catalogs; see docs/TMX-CATALOGS.md.
use super::{
    escape_xml, normalize_line_endings, xml_parse_error, xml_text_run, ImportedTranslation,
};
use crate::{
    error::{LocustError, Result},
    models::{StringEntry, STALE_TRANSLATION_METADATA_KEY},
};
use quick_xml::{events::BytesStart, events::Event, Reader};
use std::collections::{BTreeMap, HashSet};

const DOCUMENT_BYTES: usize = 32 * 1024 * 1024;
const UNITS: usize = 100_000;
const SEGMENT_BYTES: usize = 1024 * 1024;
const ID_BYTES: usize = 16 * 1024;
const ID_PREFIX: &str = "locust-v1-";
const REVIEW: &str = "x-locust-needs-review";

fn invalid(message: impl std::fmt::Display) -> LocustError {
    LocustError::ParseError {
        file: "tmx".into(),
        message: format!("TMX profile error: {message}"),
    }
}

fn require(condition: bool, message: &str) -> Result<()> {
    if condition {
        Ok(())
    } else {
        Err(invalid(message))
    }
}

fn legal_xml(value: &str) -> Result<()> {
    require(value.chars().all(|c| matches!(c,
        '\t' | '\n' | '\r' | '\u{20}'..='\u{d7ff}' | '\u{e000}'..='\u{fffd}' | '\u{10000}'..='\u{10ffff}'
    )), "illegal XML 1.0 character")
}

fn language(tag: &str) -> Result<()> {
    let mut parts = tag.split('-');
    let base = parts.next().unwrap_or_default();
    require(
        (2..=8).contains(&base.len())
            && base.bytes().all(|c| c.is_ascii_alphabetic())
            && !["und", "mul", "zxx"]
                .iter()
                .any(|v| base.eq_ignore_ascii_case(v))
            && parts.all(|p| {
                (1..=8).contains(&p.len()) && p.bytes().all(|c| c.is_ascii_alphanumeric())
            }),
        "expected a concrete ASCII language tag",
    )
}

fn languages(source: &str, target: &str) -> Result<()> {
    language(source)?;
    language(target)?;
    require(
        !source.eq_ignore_ascii_case(target),
        "source and target languages must differ",
    )
}

fn bounded(value: &str, limit: usize) -> Result<()> {
    require(value.len() <= limit, "decoded field exceeds byte limit")?;
    legal_xml(value)
}

/// Decoded project identity and exact text, returned only after complete validation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TmxUnit {
    pub id: String,
    pub source: String,
    pub target: String,
    pub needs_review: bool,
}

/// Export a fallible, bounded plaintext catalog, never a global translation memory.
pub fn export_tmx(entries: &[StringEntry], source_lang: &str, target_lang: &str) -> Result<String> {
    languages(source_lang, target_lang)?;
    require(entries.len() <= UNITS, "too many translation units")?;
    let mut xml = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<tmx version=\"1.4\">\n<header creationtool=\"Locust\" creationtoolversion=\"{}\" segtype=\"block\" o-tmf=\"Locust\" adminlang=\"en\" srclang=\"{}\" datatype=\"plaintext\"></header>\n<body>\n",
        env!("CARGO_PKG_VERSION"), source_lang
    );
    let mut ids = HashSet::new();
    for entry in entries {
        bounded(&entry.id, ID_BYTES)?;
        require(
            !entry.id.is_empty() && ids.insert(&entry.id),
            "empty or duplicate project id",
        )?;
        bounded(&entry.source, SEGMENT_BYTES)?;
        let target = entry.translation.as_deref().unwrap_or_default();
        bounded(target, SEGMENT_BYTES)?;
        let review = if entry.metadata.contains_key(STALE_TRANSLATION_METADATA_KEY) {
            "<prop type=\"x-locust-needs-review\">true</prop>"
        } else {
            ""
        };
        let unit = format!(
            "<tu tuid=\"{ID_PREFIX}{}\">{review}<tuv xml:lang=\"{source_lang}\"><seg>{}</seg></tuv><tuv xml:lang=\"{target_lang}\"><seg>{}</seg></tuv></tu>\n",
            hex::encode(&entry.id), escape_xml(&entry.source), escape_xml(target)
        );
        require(
            xml.len() + unit.len() + "</body>\n</tmx>\n".len() <= DOCUMENT_BYTES,
            "document exceeds 32 MiB",
        )?;
        xml.push_str(&unit);
    }
    xml.push_str("</body>\n</tmx>\n");
    require(xml.len() <= DOCUMENT_BYTES, "document exceeds 32 MiB")?;
    Ok(xml)
}

// quick-xml accepts adjacent quoted attributes without XML's required separator.
// Scan raw bytes before decoding references; the opposite quote inside a value
// is content, not a delimiter. The existing lexer still validates attribute syntax.
fn check_attribute_separators(raw: &[u8]) -> Result<()> {
    let xml_space = |byte: &u8| matches!(byte, b' ' | b'\t' | b'\n' | b'\r');
    require(
        raw.first().is_none_or(xml_space),
        "missing XML whitespace before attribute",
    )?;
    let mut quote = None;
    for (index, &byte) in raw.iter().enumerate() {
        if quote == Some(byte) {
            require(
                raw.get(index + 1).is_none_or(xml_space),
                "missing XML whitespace between attributes",
            )?;
            quote = None;
        } else if quote.is_none() && matches!(byte, b'\'' | b'"') {
            quote = Some(byte);
        }
    }
    Ok(())
}

// A closed grammar rejects unknown attributes (including namespace declarations),
// so undeclared/aliased prefixes cannot hide attributes with conflicting meanings.
fn attributes(element: &BytesStart<'_>, allowed: &[&str]) -> Result<BTreeMap<String, String>> {
    check_attribute_separators(element.attributes_raw())?;
    let mut values = BTreeMap::new();
    for attribute in element.attributes() {
        let attribute = attribute.map_err(invalid)?;
        let key = std::str::from_utf8(attribute.key.as_ref()).map_err(invalid)?;
        require(allowed.contains(&key), "unsupported attribute or namespace")?;
        let raw = std::str::from_utf8(&attribute.value).map_err(invalid)?;
        require(!raw.contains('<'), "literal less-than in XML attribute")?;
        // XML attribute whitespace normalization precedes reference expansion.
        let normalized = raw.replace(['\t', '\n', '\r'], " ");
        let value = quick_xml::escape::unescape(&normalized)
            .map_err(invalid)?
            .into_owned();
        legal_xml(&value)?;
        require(
            values.insert(key.to_owned(), value).is_none(),
            "duplicate attribute",
        )?;
    }
    Ok(values)
}

fn required<'a>(attrs: &'a BTreeMap<String, String>, key: &str) -> Result<&'a str> {
    attrs
        .get(key)
        .map(String::as_str)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| invalid(format!("missing required {key} attribute")))
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Element {
    Root,
    Header,
    Body,
    Unit,
    Variant,
    Segment,
    Prop,
}

/// Parse only the documented bilingual profile, with exact caller-selected languages.
/// No rows or partial unit list escape on malformed, oversized or trailing input.
pub fn import_tmx(content: &str, source_lang: &str, target_lang: &str) -> Result<Vec<TmxUnit>> {
    languages(source_lang, target_lang)?;
    require(content.len() <= DOCUMENT_BYTES, "document exceeds 32 MiB")?;
    legal_xml(content)?;
    let content = normalize_line_endings(content);
    let mut reader = Reader::from_str(&content);
    reader.config_mut().expand_empty_elements = true;
    reader.config_mut().check_comments = true;
    reader.config_mut().check_end_names = true;
    reader.config_mut().allow_dangling_amp = true;
    let mut text_end = 0;
    let mut stack = Vec::new();
    let (mut root_seen, mut header_seen, mut body_seen) = (false, false, false);
    let mut first_event = true;
    let mut ids = HashSet::new();
    let mut units = Vec::new();
    let mut current: Option<TmxUnit> = None;
    let (mut source_seen, mut target_seen, mut segment_seen, mut review_seen) =
        (false, false, false, false);
    let mut is_source = false;
    let mut prop = String::new();
    loop {
        match reader
            .read_event()
            .map_err(|error| invalid(xml_parse_error(error)))?
        {
            Event::Start(element) => {
                let parent = stack.last().copied();
                let name = element.name();
                let kind = match (parent, name.as_ref()) {
                    (None, b"tmx") => {
                        require(!root_seen, "multiple document roots")?;
                        let attrs = attributes(&element, &["version"])?;
                        require(
                            required(&attrs, "version")? == "1.4",
                            "expected TMX 1.4b (version 1.4)",
                        )?;
                        root_seen = true;
                        Element::Root
                    }
                    (Some(Element::Root), b"header") => {
                        require(!header_seen && !body_seen, "duplicate or misplaced header")?;
                        let attrs = attributes(
                            &element,
                            &[
                                "creationtool",
                                "creationtoolversion",
                                "segtype",
                                "o-tmf",
                                "adminlang",
                                "srclang",
                                "datatype",
                            ],
                        )?;
                        for key in ["creationtool", "creationtoolversion", "o-tmf"] {
                            required(&attrs, key)?;
                        }
                        language(required(&attrs, "adminlang")?)?;
                        require(
                            ["block", "sentence", "paragraph", "phrase"]
                                .contains(&required(&attrs, "segtype")?),
                            "unsupported segmentation type",
                        )?;
                        require(
                            required(&attrs, "datatype")? == "plaintext",
                            "expected plaintext datatype",
                        )?;
                        require(
                            required(&attrs, "srclang")?.eq_ignore_ascii_case(source_lang),
                            "header source language mismatch",
                        )?;
                        header_seen = true;
                        Element::Header
                    }
                    (Some(Element::Root), b"body") => {
                        require(
                            header_seen && !body_seen,
                            "missing header or duplicate body",
                        )?;
                        attributes(&element, &[])?;
                        body_seen = true;
                        Element::Body
                    }
                    (Some(Element::Body), b"tu") => {
                        require(units.len() < UNITS, "too many translation units")?;
                        let attrs = attributes(&element, &["tuid", "srclang"])?;
                        if let Some(source) = attrs.get("srclang") {
                            require(
                                source.eq_ignore_ascii_case(source_lang),
                                "TU source language mismatch",
                            )?;
                        }
                        let encoded = required(&attrs, "tuid")?
                            .strip_prefix(ID_PREFIX)
                            .ok_or_else(|| invalid("unsupported project id encoding"))?;
                        require(encoded.len() <= ID_BYTES * 2, "encoded id exceeds limit")?;
                        let id = String::from_utf8(hex::decode(encoded).map_err(invalid)?)
                            .map_err(invalid)?;
                        bounded(&id, ID_BYTES)?;
                        require(
                            !id.is_empty() && ids.insert(id.clone()),
                            "empty or duplicate decoded project id",
                        )?;
                        current = Some(TmxUnit {
                            id,
                            source: String::new(),
                            target: String::new(),
                            needs_review: false,
                        });
                        (source_seen, target_seen, review_seen) = (false, false, false);
                        Element::Unit
                    }
                    (Some(Element::Unit), b"prop") => {
                        require(
                            !review_seen && !source_seen && !target_seen,
                            "duplicate or misplaced review property",
                        )?;
                        let attrs = attributes(&element, &["type"])?;
                        require(
                            required(&attrs, "type")? == REVIEW,
                            "unsupported TU property",
                        )?;
                        review_seen = true;
                        prop.clear();
                        Element::Prop
                    }
                    (Some(Element::Unit), b"tuv") => {
                        let attrs = attributes(&element, &["xml:lang"])?;
                        let lang = required(&attrs, "xml:lang")?;
                        is_source = lang.eq_ignore_ascii_case(source_lang);
                        require(
                            is_source || lang.eq_ignore_ascii_case(target_lang),
                            "unexpected variant language",
                        )?;
                        let seen = if is_source {
                            &mut source_seen
                        } else {
                            &mut target_seen
                        };
                        require(!*seen, "duplicate language variant")?;
                        *seen = true;
                        segment_seen = false;
                        Element::Variant
                    }
                    (Some(Element::Variant), b"seg") => {
                        attributes(&element, &[])?;
                        require(!segment_seen, "duplicate segment")?;
                        segment_seen = true;
                        Element::Segment
                    }
                    _ => {
                        return Err(invalid(
                            "unsupported element, namespace, inline markup or nesting",
                        ))
                    }
                };
                stack.push(kind);
                require(stack.len() <= 5, "nesting limit exceeded")?;
            }
            event @ (Event::Text(_) | Event::GeneralRef(_)) => {
                let Some(raw) = xml_text_run(&content, &event, &mut text_end) else {
                    continue;
                };
                require(!raw.contains("]]>"), "CDATA terminator in text")?;
                if stack.is_empty() {
                    require(
                        raw.bytes().all(|c| matches!(c, b' ' | b'\t' | b'\n')),
                        "only literal whitespace is allowed outside the root",
                    )?;
                }
                let value = quick_xml::escape::unescape(raw).map_err(invalid)?;
                append_text(&stack, current.as_mut(), is_source, &mut prop, &value)?;
            }
            Event::CData(text) => {
                require(
                    matches!(stack.last(), Some(Element::Segment | Element::Prop)),
                    "CDATA outside text field",
                )?;
                let value = std::str::from_utf8(text.as_ref()).map_err(invalid)?;
                append_text(&stack, current.as_mut(), is_source, &mut prop, value)?;
            }
            Event::End(_) => {
                match stack
                    .pop()
                    .ok_or_else(|| invalid("unmatched closing element"))?
                {
                    Element::Variant => require(segment_seen, "variant has no segment")?,
                    Element::Prop => {
                        require(prop == "true", "review property must be exactly true")?;
                        current.as_mut().unwrap().needs_review = true;
                    }
                    Element::Unit => {
                        require(
                            source_seen && target_seen,
                            "unit must contain both language variants",
                        )?;
                        units.push(current.take().unwrap());
                    }
                    Element::Root => require(header_seen && body_seen, "missing header or body")?,
                    _ => {}
                }
            }
            Event::Decl(decl) => {
                require(first_event, "misplaced XML declaration")?;
                let raw = std::str::from_utf8(decl.as_ref()).map_err(invalid)?;
                // Declarations are not ordinary XML attributes: references and
                // reordered pseudo-attributes are not permitted by XML 1.0.
                require(!raw.contains('&'), "reference in XML declaration")?;
                require(
                    decl.version().map_err(invalid)?.as_ref() == b"1.0",
                    "expected XML 1.0",
                )?;
                if let (Some(encoding), Some(standalone)) =
                    (raw.find("encoding"), raw.find("standalone"))
                {
                    require(encoding < standalone, "reordered XML declaration")?;
                }
                let attrs = attributes(
                    &BytesStart::from_content(raw, 3),
                    &["version", "encoding", "standalone"],
                )?;
                require(required(&attrs, "version")? == "1.0", "expected XML 1.0")?;
                require(
                    attrs
                        .get("encoding")
                        .is_none_or(|v| v.eq_ignore_ascii_case("UTF-8")),
                    "expected UTF-8 encoding",
                )?;
                require(
                    attrs
                        .get("standalone")
                        .is_none_or(|v| matches!(v.as_str(), "yes" | "no")),
                    "invalid standalone declaration",
                )?;
            }
            Event::Comment(_) => {}
            Event::Eof => {
                require(
                    root_seen && header_seen && body_seen && stack.is_empty() && current.is_none(),
                    "empty or incomplete document",
                )?;
                return Ok(units);
            }
            _ => return Err(invalid("DTD and processing instructions are unsupported")),
        }
        first_event = false;
    }
}

fn append_text(
    stack: &[Element],
    current: Option<&mut TmxUnit>,
    is_source: bool,
    prop: &mut String,
    value: &str,
) -> Result<()> {
    legal_xml(value)?;
    match stack.last() {
        Some(Element::Segment) => {
            let unit = current.unwrap();
            let segment = if is_source {
                &mut unit.source
            } else {
                &mut unit.target
            };
            require(
                segment.len() + value.len() <= SEGMENT_BYTES,
                "decoded segment exceeds 1 MiB",
            )?;
            segment.push_str(value);
        }
        Some(Element::Prop) => {
            require(prop.len() + value.len() <= 4, "invalid review property")?;
            prop.push_str(value);
        }
        _ => require(
            value.chars().all(|c| matches!(c, ' ' | '\t' | '\n' | '\r')),
            "unexpected text outside segment",
        )?,
    }
    Ok(())
}

/// Adapt validated units to the existing transactional, exact-source importer.
/// Stale and empty targets count as skips, never translation confirmations.
pub fn tmx_units_for_batch(units: &[TmxUnit]) -> (Vec<ImportedTranslation>, usize) {
    let updates: Vec<_> = units
        .iter()
        .filter(|u| !u.needs_review && !u.target.is_empty())
        .map(|u| ImportedTranslation {
            id: u.id.clone(),
            source: u.source.clone(),
            translation: u.target.clone(),
        })
        .collect();
    let skipped = units.len() - updates.len();
    (updates, skipped)
}
