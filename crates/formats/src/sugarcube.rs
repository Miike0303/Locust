use std::collections::HashMap;
use std::path::{Path, PathBuf};

use locust_core::error::{LocustError, Result};
use locust_core::extraction::{FormatPlugin, InjectionReport};
use locust_core::models::{OutputMode, StringEntry};

/// Plugin for SugarCube/Twine HTML games.
/// SugarCube stores story passages inside `<tw-passagedata>` tags in a single HTML file.
pub struct SugarCubePlugin;

impl SugarCubePlugin {
    pub fn new() -> Self {
        Self
    }

    fn find_html_file(path: &Path) -> Option<PathBuf> {
        if path.is_file() && is_html(path) && is_sugarcube_html(path) {
            return Some(path.to_path_buf());
        }
        if path.is_dir() {
            if let Ok(entries) = std::fs::read_dir(path) {
                for entry in entries.flatten() {
                    let p = entry.path();
                    if is_html(&p) && is_sugarcube_html(&p) {
                        return Some(p);
                    }
                }
            }
        }
        None
    }

    fn extract_passages(content: &str, file_path: &Path) -> Vec<StringEntry> {
        let mut entries = Vec::new();

        for passage in parse_passages(content) {
            for (line_idx, row) in passage.rows.iter().enumerate() {
                let fragment_count = row.fragments.len();
                for (frag_idx, fragment) in row.fragments.iter().enumerate() {
                    // Protect SugarCube variables ($var, _var) with placeholders.
                    let (protected, var_map) = if fragment_has_wiki_syntax(&fragment.text) {
                        (fragment.text.clone(), Vec::new())
                    } else {
                        protect_variables(&fragment.text)
                    };
                    if protected.trim().is_empty() {
                        continue;
                    }

                    let id = fragment_entry_id(
                        &passage.pid,
                        &passage.name,
                        line_idx,
                        frag_idx,
                        fragment_count,
                    );
                    let mut entry =
                        StringEntry::new(id, protected.as_str(), file_path.to_path_buf());
                    entry.tags = vec!["dialogue".to_string()];
                    entry.context = Some(passage.name.clone());

                    if fragment_count > 1 {
                        entry.metadata.insert(
                            "sugarcube_span_count".to_string(),
                            serde_json::json!(fragment_count),
                        );
                        entry.metadata.insert(
                            "sugarcube_span_index".to_string(),
                            serde_json::json!(frag_idx),
                        );
                        entry.metadata.insert(
                            "sugarcube_structure".to_string(),
                            serde_json::json!(row.structure),
                        );
                    }

                    // Store variable mapping in metadata for restoration during injection.
                    if !var_map.is_empty() {
                        let var_json: Vec<serde_json::Value> = var_map
                            .iter()
                            .map(|(placeholder, original)| {
                                serde_json::json!({"p": placeholder, "v": original})
                            })
                            .collect();
                        entry.metadata.insert(
                            "sugarcube_vars".to_string(),
                            serde_json::Value::Array(var_json),
                        );
                    }

                    entries.push(entry);
                }
            }
        }

        entries
    }
}

#[derive(Clone)]
struct DecodedUnit {
    ch: char,
    raw_start: usize,
    raw_end: usize,
    link_label: bool,
}

#[derive(Clone)]
struct VisibleSpan {
    start: usize,
    end: usize,
}

struct VisibleFragment {
    text: String,
    span: VisibleSpan,
    link_label: bool,
}

struct PassageRow {
    fragments: Vec<VisibleFragment>,
    structure: String,
}

struct ParsedPassage {
    pid: String,
    name: String,
    rows: Vec<PassageRow>,
}

fn parse_passages(content: &str) -> Vec<ParsedPassage> {
    let mut passages = Vec::new();
    let mut search_from = 0;
    let close_tag = "</tw-passagedata>";

    while let Some(tag_start) = content[search_from..].find("<tw-passagedata") {
        let abs_start = search_from + tag_start;
        let tag_header_end = match find_unquoted_byte(content, abs_start, b'>') {
            Some(pos) => pos + 1,
            None => break,
        };
        let tag_end = match content[tag_header_end..].find(close_tag) {
            Some(pos) => tag_header_end + pos,
            None => break,
        };

        let header = &content[abs_start..tag_header_end];
        let name = extract_attr(header, "name").unwrap_or_default();
        let pid = extract_attr(header, "pid").unwrap_or_default();
        let tags = extract_attr(header, "tags").unwrap_or_default();
        let raw = &content[tag_header_end..tag_end];
        let rows = if is_system_passage(&name, &tags) {
            Vec::new()
        } else {
            extract_passage_rows(raw, tag_header_end)
        };

        passages.push(ParsedPassage { pid, name, rows });
        search_from = tag_end + close_tag.len();
    }

    passages
}

fn decode_units(raw: &str, base: usize) -> Vec<DecodedUnit> {
    // Named markup entities only. Numeric apostrophes stay encoded so an
    // injected `&amp;#39;` cannot toggle quote tracking after one `&amp;` decode.
    const ENTITIES: [(&str, char); 5] = [
        ("&amp;", '&'),
        ("&lt;", '<'),
        ("&gt;", '>'),
        ("&quot;", '"'),
        ("&apos;", '\''),
    ];

    let mut units = Vec::new();
    let mut pos = 0;
    while pos < raw.len() {
        let ch = raw[pos..]
            .chars()
            .next()
            .expect("position is a char boundary");
        let end = pos + ch.len_utf8();
        units.push(DecodedUnit {
            ch,
            raw_start: base + pos,
            raw_end: base + end,
            link_label: false,
        });
        pos = end;
    }

    // Keep the extractor's established replacement order while retaining each
    // decoded character's exact raw byte range.
    for (entity, decoded) in ENTITIES {
        let width = entity.chars().count();
        let mut transformed = Vec::with_capacity(units.len());
        let mut i = 0;
        while i < units.len() {
            if units_start_with(&units, i, entity) {
                transformed.push(DecodedUnit {
                    ch: decoded,
                    raw_start: units[i].raw_start,
                    raw_end: units[i + width - 1].raw_end,
                    link_label: false,
                });
                i += width;
            } else {
                transformed.push(units[i].clone());
                i += 1;
            }
        }
        units = transformed;
    }

    units
}

fn units_start_with(units: &[DecodedUnit], pos: usize, needle: &str) -> bool {
    let mut at = pos;
    for expected in needle.chars() {
        match units.get(at) {
            Some(unit) if unit.ch == expected => at += 1,
            _ => return false,
        }
    }
    true
}

fn find_unquoted_byte(content: &str, from: usize, needle: u8) -> Option<usize> {
    let bytes = content.as_bytes();
    let mut quote = None;
    let mut i = from;
    while i < bytes.len() {
        let b = bytes[i];
        match quote {
            Some(q) if b == q => quote = None,
            None if b == b'"' || b == b'\'' => quote = Some(b),
            None if b == needle => return Some(i),
            _ => {}
        }
        i += 1;
    }
    None
}

fn decode_numeric_char_entities(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '&' && chars.get(i + 1) == Some(&'#') {
            let hex = matches!(chars.get(i + 2), Some('x' | 'X'));
            let digit_start = if hex { i + 3 } else { i + 2 };
            let mut j = digit_start;
            while j < chars.len() {
                let ch = chars[j];
                let ok = if hex {
                    ch.is_ascii_hexdigit()
                } else {
                    ch.is_ascii_digit()
                };
                if !ok {
                    break;
                }
                j += 1;
            }
            if j > digit_start && chars.get(j) == Some(&';') {
                let digits: String = chars[digit_start..j].iter().collect();
                let parsed = if hex {
                    u32::from_str_radix(&digits, 16).ok()
                } else {
                    digits.parse().ok()
                };
                if let Some(decoded) = parsed.and_then(char::from_u32) {
                    out.push(decoded);
                    i = j + 1;
                    continue;
                }
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

fn units_text(units: &[DecodedUnit]) -> String {
    // Numeric leftovers (`&#60;` from `&amp;#60;`) become literal chars here,
    // then a remaining `&amp;` from double-escaped `&` becomes `&`.
    decode_numeric_char_entities(&units.iter().map(|unit| unit.ch).collect::<String>())
        .replace("&amp;", "&")
}

const STRUCTURE_SLOT: &str = "\u{1e}";

fn row_structure(
    raw: &str,
    base: usize,
    line_begin: usize,
    line_end: usize,
    fragments: &[VisibleFragment],
) -> String {
    if line_end < line_begin || line_begin < base {
        return String::new();
    }
    let start = line_begin - base;
    let end = line_end - base;
    if end > raw.len() || start > end {
        return String::new();
    }
    let mut skeleton = raw[start..end].to_string();
    let mut ranges: Vec<(usize, usize)> = fragments
        .iter()
        .map(|fragment| (fragment.span.start, fragment.span.end))
        .collect();
    ranges.sort_unstable_by(|left, right| right.0.cmp(&left.0));
    for (abs_start, abs_end) in ranges {
        if abs_start < line_begin || abs_end > line_end {
            continue;
        }
        let from = abs_start - line_begin;
        let to = abs_end - line_begin;
        if to <= skeleton.len() {
            skeleton.replace_range(from..to, STRUCTURE_SLOT);
        }
    }
    skeleton
}

fn fragment_entry_id(
    pid: &str,
    name: &str,
    row: usize,
    frag_idx: usize,
    fragment_count: usize,
) -> String {
    if fragment_count > 1 {
        format!("passage_{pid}#{name}#{row}#f{frag_idx}")
    } else {
        format!("passage_{pid}#{name}#{row}")
    }
}

fn slurp_double_quote(units: &[DecodedUnit], open: usize, end: usize) -> Option<usize> {
    let mut i = open + 1;
    while i < end {
        if units[i].ch == '\\' {
            i += 1;
            if i < end {
                i += 1;
            }
            continue;
        }
        if units[i].ch == '"' {
            return Some(i);
        }
        i += 1;
    }
    None
}

fn find_square_markup_token(
    units: &[DecodedUnit],
    from: usize,
    end: usize,
    needle: &str,
) -> Option<usize> {
    let mut i = from;
    while i < end {
        if units[i].ch == '"' {
            i = slurp_double_quote(units, i, end)? + 1;
            continue;
        }
        if units_start_with(units, i, needle) && i + needle.chars().count() <= end {
            return Some(i);
        }
        i += 1;
    }
    None
}

fn decode_constant_json_string(text: &str) -> Option<String> {
    let trimmed = text.trim();
    if !trimmed.starts_with('"') {
        return None;
    }
    serde_json::from_str(trimmed).ok()
}

fn is_dynamic_link_label(text: &str) -> bool {
    let text = text.trim();
    if text.is_empty() {
        return true;
    }
    if decode_constant_json_string(text).is_some() {
        return false;
    }
    text.starts_with('"')
        || text.starts_with('\'')
        || text.starts_with('$')
        || text.starts_with('_')
        || text.contains('$')
        || text.contains('`')
        || text.contains('(')
}

fn link_label_text(units: &[DecodedUnit]) -> String {
    let raw = units_text(units);
    decode_constant_json_string(raw.trim()).unwrap_or(raw)
}

fn as_link_label_unit(unit: &DecodedUnit) -> DecodedUnit {
    let mut unit = unit.clone();
    unit.link_label = true;
    unit
}

fn link_display_range(
    units: &[DecodedUnit],
    inner_start: usize,
    link_end: usize,
) -> Option<(usize, usize)> {
    if let Some(pipe) = find_square_markup_token(units, inner_start, link_end, "|") {
        return Some((inner_start, pipe));
    }
    if let Some(arrow) = find_square_markup_token(units, inner_start, link_end, "->") {
        return Some((inner_start, arrow));
    }
    if let Some(arrow) = find_square_markup_token(units, inner_start, link_end, "<-") {
        return Some((arrow + 2, link_end));
    }
    None
}

fn find_unquoted_close(units: &[DecodedUnit], from: usize, needle: &str) -> Option<usize> {
    let mut quote = None;
    let mut escaped = false;
    for pos in from..units.len() {
        let ch = units[pos].ch;
        if let Some(delimiter) = quote {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == delimiter {
                quote = None;
            }
        } else if matches!(ch, '\'' | '"' | '`') {
            quote = Some(ch);
        } else if units_start_with(units, pos, needle) {
            return Some(pos);
        }
    }
    None
}

fn find_units_ascii_case(units: &[DecodedUnit], from: usize, needle: &str) -> Option<usize> {
    (from..units.len()).find(|&pos| {
        units[pos..]
            .iter()
            .zip(needle.chars())
            .all(|(unit, expected)| unit.ch.eq_ignore_ascii_case(&expected))
            && units.len() - pos >= needle.chars().count()
    })
}

fn flush_passage_row(
    current: &mut Vec<DecodedUnit>,
    rows: &mut Vec<PassageRow>,
    raw: &str,
    base: usize,
    line_begin: usize,
    line_end: usize,
) {
    let Some(first) = current.iter().position(|unit| !unit.ch.is_whitespace()) else {
        current.clear();
        return;
    };
    let last = current
        .iter()
        .rposition(|unit| !unit.ch.is_whitespace())
        .expect("first non-whitespace unit exists");
    let visible = &current[first..=last];

    let mut spans: Vec<(VisibleSpan, Vec<DecodedUnit>)> = Vec::new();
    for unit in visible {
        if let Some((last_span, units)) = spans.last_mut() {
            if last_span.end == unit.raw_start {
                last_span.end = unit.raw_end;
                units.push(unit.clone());
                continue;
            }
        }
        spans.push((
            VisibleSpan {
                start: unit.raw_start,
                end: unit.raw_end,
            },
            vec![unit.clone()],
        ));
    }

    let fragments: Vec<VisibleFragment> = spans
        .into_iter()
        .filter_map(|(span, units)| {
            let link_label = units.iter().any(|unit| unit.link_label);
            let text = if link_label {
                link_label_text(&units)
            } else {
                units_text(&units)
            };
            if text.trim().is_empty() || !is_extractable_line(&text) || !is_translatable_text(&text)
            {
                return None;
            }
            Some(VisibleFragment {
                text,
                span,
                link_label,
            })
        })
        .collect();

    if !fragments.is_empty() {
        let structure = row_structure(raw, base, line_begin, line_end, &fragments);
        rows.push(PassageRow {
            fragments,
            structure,
        });
    }
    current.clear();
}

fn extract_passage_rows(raw: &str, base: usize) -> Vec<PassageRow> {
    let units = decode_units(raw, base);
    let mut rows = Vec::new();
    let mut current = Vec::new();
    let mut line_begin = base;
    let mut i = 0;

    while i < units.len() {
        // SugarCube macros and script/widget macro bodies are never visible spans.
        if units_start_with(&units, i, "<<") {
            let body_start = i + 2;
            let macro_name: String = units[body_start..]
                .iter()
                .take_while(|unit| unit.ch.is_alphanumeric() || unit.ch == '_')
                .map(|unit| unit.ch.to_ascii_lowercase())
                .collect();
            if macro_name == "script" || macro_name == "widget" {
                let closing = format!("<</{}>>", macro_name);
                i = find_units_ascii_case(&units, body_start, &closing)
                    .map(|at| at + closing.chars().count())
                    .unwrap_or(units.len());
            } else {
                i = find_unquoted_close(&units, body_start, ">>")
                    .map(|at| at + 2)
                    .unwrap_or(units.len());
            }
            continue;
        }

        // HTML tags and HTML script/style bodies are excluded, including attributes.
        // '<' starts a tag only when it looks like one, so decoded translations
        // such as "a < b" stay visible instead of consuming the rest of the passage.
        if units[i].ch == '<' {
            let looks_like_tag = units.get(i + 1).is_some_and(|unit| {
                matches!(unit.ch, '/' | '!' | '?') || unit.ch.is_ascii_alphabetic()
            });
            if looks_like_tag {
                if let Some(tag_end) = find_unquoted_close(&units, i + 1, ">") {
                    let tag: String = units[i + 1..tag_end].iter().map(|unit| unit.ch).collect();
                    let tag_name = tag
                        .trim_start()
                        .trim_start_matches('/')
                        .split_whitespace()
                        .next()
                        .unwrap_or("")
                        .trim_end_matches('/')
                        .to_ascii_lowercase();
                    if !tag.trim_start().starts_with('/')
                        && (tag_name == "script" || tag_name == "style")
                    {
                        let closing = format!("</{}>", tag_name);
                        i = find_units_ascii_case(&units, tag_end + 1, &closing)
                            .map(|at| at + closing.chars().count())
                            .unwrap_or(units.len());
                    } else {
                        i = tag_end + 1;
                    }
                    continue;
                }
            }
        }

        // Only a SugarCube link's display portion is an allowed visible span.
        // Quoted JSON string labels may contain |, arrows and ]] ; skip those
        // delimiters inside quotes. Dynamic TwineScript labels are not extracted.
        if units_start_with(&units, i, "[[") {
            let inner_start = i + 2;
            let Some(link_end) = find_square_markup_token(&units, inner_start, units.len(), "]]")
            else {
                break;
            };
            if let Some((display_start, display_end)) =
                link_display_range(&units, inner_start, link_end)
            {
                let display = &units[display_start..display_end];
                let display_text = units_text(display);
                if decode_constant_json_string(display_text.trim()).is_some()
                    || !is_dynamic_link_label(&display_text)
                {
                    current.extend(display.iter().map(as_link_label_unit));
                }
            }
            i = link_end + 2;
            continue;
        }

        if units[i].ch == '\n' {
            flush_passage_row(
                &mut current,
                &mut rows,
                raw,
                base,
                line_begin,
                units[i].raw_start,
            );
            line_begin = units[i].raw_end;
        } else {
            current.push(units[i].clone());
        }
        i += 1;
    }

    flush_passage_row(
        &mut current,
        &mut rows,
        raw,
        base,
        line_begin,
        base + raw.len(),
    );
    rows
}

/// Check if a passage is a system/code passage that should not be translated.
fn is_system_passage(name: &str, tags: &str) -> bool {
    // Skip known system passages
    let system_names = [
        "StoryInit",
        "StoryCaption",
        "StoryBanner",
        "StoryMenu",
        "StoryInterface",
        "StoryShare",
        "StoryAuthor",
        "StoryTitle",
        "StorySubtitle",
        "StoryDisplayTitle",
        "PassageHeader",
        "PassageFooter",
        "PassageReady",
        "PassageDone",
    ];
    if system_names.iter().any(|s| name.starts_with(s)) {
        return true;
    }

    // Skip passages tagged as widget, script, or stylesheet
    let skip_tags = ["widget", "script", "stylesheet", "init", "nobr-all"];
    let passage_tags: Vec<&str> = tags.split_whitespace().collect();
    if skip_tags.iter().any(|t| passage_tags.contains(t)) {
        return true;
    }

    false
}

fn is_html(path: &Path) -> bool {
    path.extension().is_some_and(|e| e == "html" || e == "htm")
}

fn is_sugarcube_html(path: &Path) -> bool {
    std::fs::read_to_string(path)
        .map(|content| content.contains("tw-passagedata") || content.contains("SugarCube"))
        .unwrap_or(false)
}

fn extract_attr(tag: &str, name: &str) -> Option<String> {
    let pattern = format!("{}=\"", name);
    let start = tag.find(&pattern)? + pattern.len();
    let end = tag[start..].find('"')? + start;
    Some(tag[start..end].to_string())
}

fn fragment_has_wiki_syntax(text: &str) -> bool {
    text.contains('<') || text.contains('[')
}

/// Protect SugarCube variables ($var, _var) by replacing them with numbered placeholders.
/// Returns the protected text and a list of (placeholder, original_variable) pairs.
fn protect_variables(text: &str) -> (String, Vec<(String, String)>) {
    let mut result = String::new();
    let mut vars = Vec::new();
    let chars: Vec<char> = text.chars().collect();
    let len = chars.len();
    let mut i = 0;

    while i < len {
        // Detect SugarCube permanent variable $varname
        if chars[i] == '$' && i + 1 < len && chars[i + 1].is_alphabetic() {
            let start = i;
            i += 1;
            while i < len && (chars[i].is_alphanumeric() || chars[i] == '_') {
                i += 1;
            }
            let var_name: String = chars[start..i].iter().collect();
            let placeholder = format!("{{{}}}", vars.len());
            vars.push((placeholder.clone(), var_name));
            result.push_str(&placeholder);
            continue;
        }

        // Detect SugarCube temporary variable _varname (at word boundary)
        if chars[i] == '_' && i + 1 < len && chars[i + 1].is_alphabetic() {
            // Only treat as variable if at start of text or after whitespace/punctuation
            let is_word_start = i == 0 || !chars[i - 1].is_alphanumeric();
            if is_word_start {
                let start = i;
                i += 1;
                while i < len && (chars[i].is_alphanumeric() || chars[i] == '_') {
                    i += 1;
                }
                let var_name: String = chars[start..i].iter().collect();
                let placeholder = format!("{{{}}}", vars.len());
                vars.push((placeholder.clone(), var_name));
                result.push_str(&placeholder);
                continue;
            }
        }

        result.push(chars[i]);
        i += 1;
    }

    (result, vars)
}

/// Restore variable placeholders in translated text back to original SugarCube variables.
fn restore_variables(text: &str, var_map: &[(String, String)]) -> String {
    let mut result = text.to_string();
    for (placeholder, original) in var_map {
        result = result.replace(placeholder, original);
    }
    result
}

/// Extract variable mapping from entry metadata.
fn get_var_map(entry: &StringEntry) -> Vec<(String, String)> {
    entry
        .metadata
        .get("sugarcube_vars")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|item| {
                    let p = item.get("p")?.as_str()?.to_string();
                    let v = item.get("v")?.as_str()?.to_string();
                    Some((p, v))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Check if a line is extractable (not just a variable, path, or code fragment).
fn is_extractable_line(line: &str) -> bool {
    let s = line.trim();

    // Skip lines starting with / (comments) or $ (pure variable assignments)
    if s.starts_with('/') || s.starts_with('$') {
        return false;
    }

    // Skip lines that are just a bare SugarCube variable (_var or $var)
    if (s.starts_with('_') || s.starts_with('$'))
        && s[1..].chars().all(|c| c.is_alphanumeric() || c == '_')
    {
        return false;
    }

    // Skip lines that look like file paths
    if (s.contains('/') || s.contains('\\')) && !s.contains(' ') {
        return false;
    }

    // Skip lines that are just numbers or punctuation
    if s.chars().all(|c| !c.is_alphabetic()) {
        return false;
    }

    true
}

/// Returns false for lines that look like CSS, JavaScript, or code.
fn is_translatable_text(line: &str) -> bool {
    let s = line.trim();
    if s.is_empty() {
        return false;
    }

    // Visible fragments have already had script/style/widget bodies removed.
    // Reject complete code shapes, not words that also occur in dialogue.
    if looks_like_css(s) || looks_like_javascript(s) {
        return false;
    }

    // HTML/CSS class/id references
    if s.starts_with('#') && !s.contains(' ') {
        return false;
    }

    // Only punctuation/symbols, no real words
    let alpha_count = s.chars().filter(|c| c.is_alphabetic()).count();
    let total = s.chars().count();
    if total > 0 && (alpha_count as f64 / total as f64) < 0.3 {
        return false;
    }

    true
}

// This is deliberately a conservative recognizer, not a CSS parser. Unknown
// properties or values stay available to translators rather than losing prose.
fn looks_like_css(s: &str) -> bool {
    if let Some((selector, body)) = s.split_once('{') {
        let selector = selector.trim();
        let selector_shape = !selector.is_empty()
            && selector.chars().all(|ch| {
                ch.is_ascii_alphanumeric()
                    || ch.is_ascii_whitespace()
                    || "-_.#:,>+~*[]=\"'()".contains(ch)
            });
        if !selector_shape {
            return false;
        }
        let explicit_selector = selector
            .strip_prefix(['.', '#', ':'])
            .is_some_and(|rest| rest.starts_with(|ch: char| ch.is_ascii_alphabetic() || ch == '_'))
            || selector.starts_with(['[', '*'])
            || matches!(selector, "html" | "body" | "div" | "span" | "p" | "a");
        if let Some(body) = body.trim().strip_suffix('}') {
            return explicit_selector || looks_like_css_declarations(body.trim());
        }
        // Also recognize the opening line of a multiline selector block.
        return body.trim().is_empty() && explicit_selector;
    }
    looks_like_css_declarations(s)
}

fn looks_like_css_declarations(s: &str) -> bool {
    let s = s.trim().strip_suffix(';').unwrap_or(s).trim();
    !s.is_empty()
        && s.split(';').all(|declaration| {
            let Some((property, value)) = declaration.split_once(':') else {
                return false;
            };
            // A property is one identifier; speaker labels and phrases before a
            // colon cannot be CSS properties. Sentence punctuation in a value
            // also fails the token checks below.
            let property = property.trim().to_ascii_lowercase();
            if !is_css_property(&property) {
                return false;
            }
            let value = value.trim();
            let value = value.strip_suffix("!important").unwrap_or(value).trim();
            if value.is_empty() {
                return false;
            }
            // Keep function arguments (including spaces) in a single token.
            let mut depth = 0usize;
            let mut quote = None;
            value
                .split(|ch: char| {
                    if let Some(delimiter) = quote {
                        if ch == delimiter {
                            quote = None;
                        }
                    } else if matches!(ch, '\'' | '"') {
                        quote = Some(ch);
                    } else if ch == '(' {
                        depth += 1;
                    } else if ch == ')' {
                        depth = depth.saturating_sub(1);
                    }
                    ch.is_whitespace() && depth == 0 && quote.is_none()
                })
                .filter(|token| !token.is_empty())
                .all(is_css_value_token)
        })
}

fn is_css_property(property: &str) -> bool {
    const PROPERTIES: &[&str] = &[
        "color",
        "background",
        "background-color",
        "background-image",
        "background-position",
        "background-repeat",
        "background-size",
        "margin",
        "margin-top",
        "margin-right",
        "margin-bottom",
        "margin-left",
        "padding",
        "padding-top",
        "padding-right",
        "padding-bottom",
        "padding-left",
        "border",
        "border-top",
        "border-right",
        "border-bottom",
        "border-left",
        "border-color",
        "border-width",
        "border-style",
        "border-radius",
        "font",
        "font-size",
        "font-family",
        "font-weight",
        "font-style",
        "line-height",
        "letter-spacing",
        "text-align",
        "text-decoration",
        "text-transform",
        "text-shadow",
        "white-space",
        "word-break",
        "display",
        "position",
        "top",
        "right",
        "bottom",
        "left",
        "width",
        "height",
        "min-width",
        "max-width",
        "min-height",
        "max-height",
        "box-shadow",
        "box-sizing",
        "opacity",
        "z-index",
        "overflow",
        "overflow-x",
        "overflow-y",
        "transform",
        "transform-origin",
        "transition",
        "transition-property",
        "transition-duration",
        "transition-timing-function",
        "animation",
        "animation-duration",
        "cursor",
        "visibility",
        "float",
        "clear",
        "vertical-align",
        "flex",
        "flex-direction",
        "flex-wrap",
        "flex-grow",
        "flex-shrink",
        "flex-basis",
        "align-items",
        "align-self",
        "align-content",
        "justify-content",
        "gap",
        "row-gap",
        "column-gap",
        "grid",
        "grid-template-columns",
        "grid-template-rows",
        "grid-column",
        "grid-row",
        "content",
        "list-style",
        "outline",
        "outline-color",
        "outline-width",
        "outline-style",
    ];
    PROPERTIES.contains(&property)
}

fn is_css_value_token(token: &str) -> bool {
    let token = token.trim_end_matches(',').to_ascii_lowercase();
    let token = token.as_str();
    if let Some(hex) = token.strip_prefix('#') {
        return matches!(hex.len(), 3 | 4 | 6 | 8) && hex.bytes().all(|b| b.is_ascii_hexdigit());
    }
    if let Some((function, arguments)) = token.split_once('(') {
        return arguments.ends_with(')')
            && matches!(
                function,
                "var"
                    | "rgb"
                    | "rgba"
                    | "hsl"
                    | "hsla"
                    | "hwb"
                    | "color"
                    | "color-mix"
                    | "url"
                    | "calc"
                    | "min"
                    | "max"
                    | "clamp"
                    | "linear-gradient"
                    | "radial-gradient"
                    | "repeating-linear-gradient"
                    | "translate"
                    | "translatex"
                    | "translatey"
                    | "translate3d"
                    | "rotate"
                    | "scale"
                    | "skew"
                    | "matrix"
                    | "cubic-bezier"
                    | "steps"
            )
            && (function != "var" || arguments.starts_with("--"));
    }
    if (token.starts_with('"') && token.ends_with('"'))
        || (token.starts_with('\'') && token.ends_with('\''))
    {
        return token.len() >= 2;
    }
    let number = |s: &str| {
        s.chars().any(|ch| ch.is_ascii_digit()) && s.parse::<f64>().is_ok_and(|n| n.is_finite())
    };
    if number(token)
        || [
            "px", "em", "rem", "vh", "vw", "vmin", "vmax", "dvh", "dvw", "ch", "ex", "cm", "mm",
            "in", "pt", "pc", "%", "ms", "s", "deg", "turn", "fr",
        ]
        .iter()
        .any(|unit| token.strip_suffix(unit).is_some_and(number))
    {
        return true;
    }
    const KEYWORDS: &[&str] = &[
        "auto",
        "none",
        "normal",
        "initial",
        "inherit",
        "unset",
        "revert",
        "transparent",
        "currentcolor",
        "black",
        "white",
        "red",
        "green",
        "blue",
        "yellow",
        "gray",
        "grey",
        "orange",
        "purple",
        "pink",
        "brown",
        "cyan",
        "magenta",
        "silver",
        "navy",
        "teal",
        "lime",
        "maroon",
        "olive",
        "aqua",
        "fuchsia",
        "rebeccapurple",
        "solid",
        "dashed",
        "dotted",
        "double",
        "hidden",
        "visible",
        "block",
        "inline",
        "inline-block",
        "flex",
        "inline-flex",
        "grid",
        "inline-grid",
        "contents",
        "table",
        "table-row",
        "table-cell",
        "list-item",
        "static",
        "relative",
        "absolute",
        "fixed",
        "sticky",
        "left",
        "right",
        "top",
        "bottom",
        "center",
        "justify",
        "bold",
        "bolder",
        "lighter",
        "italic",
        "oblique",
        "serif",
        "sans-serif",
        "monospace",
        "cursive",
        "fantasy",
        "system-ui",
        "underline",
        "overline",
        "line-through",
        "uppercase",
        "lowercase",
        "capitalize",
        "nowrap",
        "pre",
        "pre-wrap",
        "pre-line",
        "break-word",
        "scroll",
        "clip",
        "cover",
        "contain",
        "repeat",
        "no-repeat",
        "repeat-x",
        "repeat-y",
        "border-box",
        "content-box",
        "row",
        "column",
        "row-reverse",
        "column-reverse",
        "wrap",
        "wrap-reverse",
        "start",
        "end",
        "flex-start",
        "flex-end",
        "stretch",
        "baseline",
        "space-between",
        "space-around",
        "space-evenly",
        "pointer",
        "default",
        "move",
        "text",
        "wait",
        "not-allowed",
        "all",
        "ease",
        "linear",
        "ease-in",
        "ease-out",
        "ease-in-out",
        "infinite",
        "inset",
        "thin",
        "medium",
        "thick",
        "/",
    ];
    KEYWORDS.contains(&token) || is_css_property(token)
}

fn is_javascript_reference(s: &str) -> bool {
    !s.is_empty()
        && s.split('.').all(|part| {
            let mut chars = part.chars();
            chars
                .next()
                .is_some_and(|ch| ch.is_ascii_alphabetic() || matches!(ch, '_' | '$'))
                && chars.all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '$'))
        })
}

fn looks_like_javascript(s: &str) -> bool {
    let statement = s.strip_suffix(';').unwrap_or(s).trim();
    for keyword in ["var ", "let ", "const "] {
        if let Some(declaration) = statement.strip_prefix(keyword) {
            let name = declaration
                .split_once('=')
                .map_or(declaration, |(name, _)| name)
                .trim();
            if is_javascript_reference(name) && (declaration.contains('=') || s.ends_with(';')) {
                return true;
            }
        }
    }
    if let Some(rest) = statement.strip_prefix("return ") {
        // A prose phrase such as "return home before dusk." is not an expression.
        return (s.ends_with(';') && is_javascript_reference(rest))
            || matches!(rest, "true" | "false" | "null" | "undefined")
            || rest.parse::<f64>().is_ok()
            || ['\'', '"', '`']
                .iter()
                .any(|&quote| rest.len() >= 2 && rest.starts_with(quote) && rest.ends_with(quote))
            || rest
                .split_once('(')
                .is_some_and(|(callee, _)| is_javascript_reference(callee) && rest.ends_with(')'));
    }
    if let Some(rest) = statement.strip_prefix("function") {
        if let Some((name, _)) = rest.split_once('(') {
            if name.trim().is_empty() || is_javascript_reference(name.trim()) {
                return statement.ends_with(['{', '}']);
            }
        }
    }
    if let Some(rest) = statement.strip_prefix("else ") {
        if rest.starts_with("if ") || rest.starts_with("if(") {
            return looks_like_javascript(rest);
        }
    }
    for keyword in ["if", "for", "while", "else"] {
        if let Some(rest) = statement.strip_prefix(keyword) {
            let rest = rest.trim_start();
            if (rest.starts_with('(') || (keyword == "else" && rest.starts_with('{')))
                && (statement.ends_with(['{', '}', ')']) || s.ends_with(';'))
            {
                return true;
            }
        }
    }
    // A call must start with a callee, rather than merely mention a JS API.
    if let Some((callee, _)) = statement.split_once('(') {
        if is_javascript_reference(callee) && (statement.ends_with(')') || s.ends_with(';')) {
            return true;
        }
    }
    ["===", "!==", "=>"].iter().any(|operator| {
        statement.split_once(operator).is_some_and(|(left, right)| {
            (is_javascript_reference(left.trim()) || left.trim() == "()")
                && (is_javascript_reference(right.trim())
                    || right.trim().parse::<f64>().is_ok()
                    || right.trim().ends_with(')'))
        })
    })
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct PassageLocator {
    pid: String,
    name: String,
    row: usize,
    fragment: Option<usize>,
}

fn parse_passage_locator(id: &str) -> Option<PassageLocator> {
    let rest = id.strip_prefix("passage_")?;
    let (passage, last) = rest.rsplit_once('#')?;
    if let Some(frag) = last.strip_prefix('f') {
        let fragment: usize = frag.parse().ok()?;
        if format!("f{fragment}") != last {
            return None;
        }
        let (passage, row) = passage.rsplit_once('#')?;
        let (pid, name) = passage.split_once('#')?;
        Some(PassageLocator {
            pid: pid.to_string(),
            name: name.to_string(),
            row: row.parse().ok()?,
            fragment: Some(fragment),
        })
    } else {
        let (pid, name) = passage.split_once('#')?;
        Some(PassageLocator {
            pid: pid.to_string(),
            name: name.to_string(),
            row: last.parse().ok()?,
            fragment: None,
        })
    }
}

struct PendingEntry {
    source: String,
    translation: String,
    expected_span_count: Option<usize>,
    expected_span_index: Option<usize>,
    expected_structure: Option<String>,
}

struct TextEdit {
    start: usize,
    end: usize,
    replacement: String,
}

impl Default for SugarCubePlugin {
    fn default() -> Self {
        Self::new()
    }
}

impl FormatPlugin for SugarCubePlugin {
    fn id(&self) -> &str {
        "sugarcube"
    }

    fn name(&self) -> &str {
        "SugarCube/Twine HTML"
    }

    fn description(&self) -> &str {
        "SugarCube/Twine interactive fiction HTML games"
    }

    fn stability(&self) -> locust_core::extraction::FormatStability {
        // Phase-2 apply proven (Zabulon's Archive); html-game is separate Experimental.
        locust_core::extraction::FormatStability::Experimental
    }

    fn supported_extensions(&self) -> &[&str] {
        &[".html", ".htm"]
    }

    fn supported_modes(&self) -> Vec<OutputMode> {
        vec![OutputMode::Replace]
    }

    fn detect(&self, path: &Path) -> bool {
        Self::find_html_file(path).is_some()
    }

    fn extract(&self, path: &Path) -> Result<Vec<StringEntry>> {
        let html_file = Self::find_html_file(path).ok_or_else(|| LocustError::ParseError {
            file: path.display().to_string(),
            message: "no SugarCube HTML file found".to_string(),
        })?;

        let content = std::fs::read_to_string(&html_file)?;
        Ok(Self::extract_passages(&content, &html_file))
    }

    fn inject(&self, path: &Path, entries: &[StringEntry]) -> Result<InjectionReport> {
        let mut report = InjectionReport {
            skip_reasons: Default::default(),
            files_modified: 0,
            strings_written: 0,
            strings_skipped: 0,
            warnings: Vec::new(),
            files_written: Vec::new(),
        };

        let html_file = if let Some(first) = entries.first() {
            // One story per injection. Distinct stored paths are ambiguous even
            // when remapping would collapse them onto the same basename.
            if entries
                .iter()
                .any(|entry| entry.file_path != first.file_path)
            {
                report.skip("ambiguous_target", entries.len());
                report.warnings.push(
                    "SugarCube entries reference multiple story files; select one story's rows."
                        .into(),
                );
                return Ok(report);
            }
            if path.is_dir() {
                let Some(filename) = first.file_path.file_name() else {
                    report.skip("missing_target", entries.len());
                    return Ok(report);
                };
                // Keep the full location if core already remapped this entry
                // into the target. Otherwise extraction's top-level filename
                // identifies the story in a separately selected copy.
                let root = path.canonicalize()?;
                let absolute_root = std::path::absolute(path)?;
                let stored = std::path::absolute(&first.file_path)?;
                let relative = stored
                    .strip_prefix(&root)
                    .or_else(|_| stored.strip_prefix(&absolute_root));
                match relative {
                    Ok(relative) => path.join(relative),
                    Err(_) => path.join(filename),
                }
            } else {
                // Explicit file selection also supports a renamed story copy.
                path.to_path_buf()
            }
        } else {
            Self::find_html_file(path).ok_or_else(|| LocustError::ParseError {
                file: path.display().to_string(),
                message: "no SugarCube HTML file found".to_string(),
            })?
        };
        if !html_file.is_file() {
            report.skip("missing_target", entries.len());
            report.warnings.push(format!(
                "SugarCube target story is missing: {}",
                html_file.display()
            ));
            return Ok(report);
        }
        if !is_html(&html_file) || !is_sugarcube_html(&html_file) {
            return Err(LocustError::ParseError {
                file: html_file.display().to_string(),
                message: "no SugarCube HTML file found".to_string(),
            });
        }
        let content = std::fs::read_to_string(&html_file)?;

        // Index requested rows once. Duplicate locators are ambiguous and fail closed.
        let mut selected: HashMap<PassageLocator, Vec<PendingEntry>> = HashMap::new();
        for entry in entries {
            let Some(translation) = entry.translation.as_deref() else {
                report.skip("untranslated", 1);
                continue;
            };
            if translation.trim().is_empty() {
                report.skip("untranslated", 1);
                continue;
            }
            let var_map = get_var_map(entry);
            let source = restore_variables(&entry.source, &var_map);
            let translation = restore_variables(translation, &var_map);
            if source == translation {
                report.skip("unchanged", 1);
                continue;
            }
            let Some(locator) = parse_passage_locator(&entry.id) else {
                report.skip("missing_target", 1);
                continue;
            };
            selected.entry(locator).or_default().push(PendingEntry {
                source,
                translation,
                expected_span_count: meta_usize(entry, "sugarcube_span_count"),
                expected_span_index: meta_usize(entry, "sugarcube_span_index"),
                expected_structure: entry
                    .metadata
                    .get("sugarcube_structure")
                    .and_then(|value| value.as_str())
                    .filter(|value| !value.is_empty())
                    .map(str::to_string),
            });
        }

        let passages = parse_passages(&content);
        let mut passage_index: HashMap<(String, String), Vec<usize>> = HashMap::new();
        for (index, passage) in passages.iter().enumerate() {
            passage_index
                .entry((passage.pid.clone(), passage.name.clone()))
                .or_default()
                .push(index);
        }

        let mut edits = Vec::new();
        for (locator, pending) in selected {
            if pending.len() != 1 {
                report.skip("source_changed", pending.len());
                continue;
            }
            let passage_key = (locator.pid, locator.name);
            let Some(matches) = passage_index.get(&passage_key) else {
                report.skip("missing_target", 1);
                continue;
            };
            if matches.len() != 1 {
                report.skip("source_changed", 1);
                continue;
            }
            let passage = &passages[matches[0]];
            let Some(row) = passage.rows.get(locator.row) else {
                report.skip("source_changed", 1);
                continue;
            };
            let pending = pending.into_iter().next().expect("one pending entry");
            if row.fragments.is_empty() {
                report.skip("source_changed", 1);
                continue;
            }

            // Legacy joined multi-span IDs are rejected explicitly. Never flatten
            // the joined source into the first span (that emptied link labels and
            // moved conditional branch text).
            let target = match locator.fragment {
                None => {
                    if row.fragments.len() != 1 {
                        report.skip("unsupported_markup", 1);
                        continue;
                    }
                    &row.fragments[0]
                }
                Some(index) => {
                    if row.fragments.len() <= 1 {
                        report.skip("source_changed", 1);
                        continue;
                    }
                    let Some(count) = pending.expected_span_count else {
                        report.skip("source_changed", 1);
                        continue;
                    };
                    let Some(expected_index) = pending.expected_span_index else {
                        report.skip("source_changed", 1);
                        continue;
                    };
                    let Some(structure) = pending.expected_structure.as_deref() else {
                        report.skip("source_changed", 1);
                        continue;
                    };
                    if count != row.fragments.len()
                        || expected_index != index
                        || structure != row.structure
                    {
                        report.skip("source_changed", 1);
                        continue;
                    }
                    match row.fragments.get(index) {
                        Some(fragment) => fragment,
                        None => {
                            report.skip("source_changed", 1);
                            continue;
                        }
                    }
                }
            };
            if target.text != pending.source {
                report.skip("source_changed", 1);
                continue;
            }

            edits.push(TextEdit {
                start: target.span.start,
                end: target.span.end,
                replacement: if target.link_label {
                    encode_link_label(&pending.translation)
                } else {
                    encode_html_entities(&pending.translation)
                },
            });
            report.strings_written += 1;
        }

        if let Some(count) = report.skip_reasons.get("unsupported_markup") {
            report.warnings.push(format!(
                "{count} SugarCube row(s) used a legacy joined locator for inline markup; original text was kept. Re-extract to translate each visible fragment."
            ));
        }
        if report.strings_written == 0 {
            return Ok(report);
        }

        // Reverse byte order keeps every parser-derived offset valid.
        edits.sort_unstable_by(|left, right| right.start.cmp(&left.start));
        let mut result = content;
        for edit in edits {
            result.replace_range(edit.start..edit.end, &edit.replacement);
        }
        std::fs::write(&html_file, result)?;

        report.files_modified = 1;
        report.files_written.push(html_file);
        Ok(report)
    }
}

fn meta_usize(entry: &StringEntry, key: &str) -> Option<usize> {
    entry
        .metadata
        .get(key)
        .and_then(|value| value.as_u64())
        .and_then(|n| usize::try_from(n).ok())
}

fn html_escape_once(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn encode_link_label(s: &str) -> String {
    html_escape_once(&serde_json::to_string(s).expect("string JSON encoding cannot fail"))
}

fn encode_html_entities(s: &str) -> String {
    let mut out = String::new();
    for ch in s.chars() {
        match ch {
            '&' => out.push_str("&amp;amp;"),
            '<' => out.push_str("&amp;#60;"),
            '>' => out.push_str("&amp;#62;"),
            '"' => out.push_str("&amp;#34;"),
            '\'' => out.push_str("&amp;#39;"),
            '`' => out.push_str("&amp;#96;"),
            '[' => out.push_str("&amp;#91;"),
            ']' => out.push_str("&amp;#93;"),
            '|' => out.push_str("&amp;#124;"),
            '\u{2018}' => out.push_str("&amp;#8216;"),
            '\u{2019}' => out.push_str("&amp;#8217;"),
            '\u{201C}' => out.push_str("&amp;#8220;"),
            '\u{201D}' => out.push_str("&amp;#8221;"),
            _ => out.push(ch),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn tempdir() -> tempfile::TempDir {
        tempfile::Builder::new()
            .prefix("locust_sc_")
            .tempdir()
            .unwrap()
    }

    fn create_fixture(dir: &Path) -> PathBuf {
        let html = dir.join("game.html");
        fs::write(
            &html,
            r#"<!DOCTYPE html>
<html>
<head><meta name="application-name" content="SugarCube" /></head>
<body>
<tw-storydata name="Test">
<tw-passagedata pid="1" name="Start" tags="">Hello, welcome to the game!
This is the second line.
&lt;&lt;set $name = "player"&gt;&gt;
[[Continue|next]]</tw-passagedata>
<tw-passagedata pid="2" name="next" tags="">You chose to continue.
&lt;&lt;if $health &gt; 0&gt;&gt;You are alive.&lt;&lt;/if&gt;&gt;
The adventure awaits!</tw-passagedata>
</tw-storydata>
</body></html>"#,
        )
        .unwrap();
        html
    }

    #[test]
    fn prose_with_css_and_code_words_roundtrips() {
        let prose = [
            "Remember: stay together.",
            "You cross the border.",
            "Note: the system is fixed.",
            "Status: relative calm",
            "The background music fades as you enter.",
            "She wrote a warning in the margin.",
            "The padding in his coat keeps him warm.",
            "Her font-size complaint makes the printer laugh.",
            "The text-align setting is hard for him to read.",
            "A box-shadow would help her see the button.",
            "The opacity of the fog worries you.",
            "She mentions the z-index in her design notes.",
            "The river might overflow before dawn.",
            "This discovery will transform the town.",
            "The transition leaves everyone uneasy.",
            "Warning: the next block is closed.",
            "Remember: flex your fingers before climbing.",
            "The grid: a solid wall of light.",
            "Her answer: absolute silence.",
            "A small reminder: leave a generous margin.",
            "color: red with embarrassment.",
            "position: fixed in her memory.",
            "margin: a little room to breathe.",
            "font-size: please make the lettering easier to read.",
            "color: red; she blushes and looks away.",
            "width: a narrow path through the trees.",
            "height: just enough to reach the window.",
            "cursor: move it over the map.",
            "She points to the window.",
            "Check the document.",
            "He calls it a console.",
            "elsewhere, someone laughs.",
            "functionaries line the hall.",
            "let us go before sunrise.",
            "return home before dusk.",
            "She calls it {a border} in her notes.",
            "color: red.",
            "margin notes: none;",
            "let go",
            "return home",
        ];
        let dir_guard = tempdir();
        let html = create_fixture(dir_guard.path());
        let wrap = |body: &str| {
            format!(r#"<tw-passagedata pid="1" name="Start" tags="">{body}</tw-passagedata>"#)
        };
        fs::write(&html, wrap(&prose.join("\n"))).unwrap();
        let plugin = SugarCubePlugin::new();
        let mut entries = plugin.extract(&html).unwrap();
        assert_eq!(
            entries
                .iter()
                .map(|entry| entry.source.as_str())
                .collect::<Vec<_>>(),
            prose
        );
        let ids: Vec<_> = entries.iter().map(|entry| entry.id.clone()).collect();
        let translations: Vec<_> = prose
            .iter()
            .map(|line| format!("She whispers: {line}"))
            .collect();
        for (entry, translation) in entries.iter_mut().zip(&translations) {
            entry.translation = Some(translation.clone());
        }

        let report = plugin.inject(&html, &entries).unwrap();
        assert_eq!(report.strings_written, prose.len());
        assert_eq!(report.strings_skipped, 0);
        assert_eq!(report.files_modified, 1);
        assert_eq!(
            fs::read_to_string(&html).unwrap(),
            wrap(&translations.join("\n"))
        );
        let extracted = plugin.extract(&html).unwrap();
        assert_eq!(
            extracted.iter().map(|entry| &entry.id).collect::<Vec<_>>(),
            ids.iter().collect::<Vec<_>>()
        );
        assert_eq!(
            extracted
                .iter()
                .map(|entry| &entry.source)
                .collect::<Vec<_>>(),
            translations.iter().collect::<Vec<_>>()
        );
    }

    #[test]
    fn css_lines_are_not_extracted() {
        let css = [
            "color: red;",
            "margin: 0 auto;",
            "border: 1px solid #000",
            ".cls { color: red; }",
            ".cls {}",
            ".cls { --accent: #000; }",
            "color: red",
            "POSITION: absolute",
            "#story { display: grid; }",
            "body { color: red; }",
            ".cls {",
            "}",
            "font-size: 1.25rem",
            "width: calc(100vw - 2rem);",
            "display: flex;",
            "position: fixed;",
            "background: var(--panel-bg);",
            "opacity: 0.8;",
            "transition: opacity 200ms ease;",
            "color: #aabbcc",
            "color: rgb(10, 20, 30);",
            "font-family: sans-serif;",
            "margin: 0 auto; padding: 1em;",
        ];
        for line in css {
            let html = format!(
                r#"<tw-passagedata pid="1" name="Start" tags="">{line}
Visible dialogue.</tw-passagedata>"#
            );
            let entries = SugarCubePlugin::extract_passages(&html, Path::new("test.html"));
            assert_eq!(entries.len(), 1, "CSS leaked: {line}");
            assert_eq!(entries[0].source, "Visible dialogue.");
        }
    }

    #[test]
    fn javascript_lines_are_not_extracted() {
        let javascript = [
            "var score = 1;",
            "let ready = true;",
            "const label = \"Hidden\";",
            "function greet() { return \"Hidden\"; }",
            "return \"Hidden\";",
            "if (ready) { run(); }",
            "else { run(); }",
            "for (const item of items) { use(item); }",
            "while (ready) { tick(); }",
            "document.querySelector(\"#story\").textContent = \"Hidden\";",
            "window.alert(\"Hidden\");",
            "console.log(\"Hidden\");",
            "button.addEventListener(\"click\", handleClick);",
            "setTimeout(() => run(), 10);",
            "items.map(item => item.name);",
            "score === 1;",
            "score !== 1;",
            "querySelector(\".choice\");",
            "return document.querySelector(\"#story\");",
            "if (ready) run();",
            "else if (ready) run();",
        ];
        for line in javascript {
            let html = format!(
                r#"<tw-passagedata pid="1" name="Start" tags="">{line}
Visible dialogue.</tw-passagedata>"#
            );
            let entries = SugarCubePlugin::extract_passages(&html, Path::new("test.html"));
            assert_eq!(entries.len(), 1, "JavaScript leaked: {line}");
            assert_eq!(entries[0].source, "Visible dialogue.");
        }
    }

    #[test]
    fn script_style_and_widget_bodies_stay_hidden_during_injection() {
        let hidden = [
            "<script>Hidden ordinary dialogue.</script>",
            "<style>Hidden ordinary dialogue.</style>",
            "&lt;script&gt;Hidden ordinary dialogue.&lt;/script&gt;",
            "&lt;style&gt;Hidden ordinary dialogue.&lt;/style&gt;",
            "<<script>>Hidden ordinary dialogue.<</script>>",
            "<<widget \"test\">>Hidden ordinary dialogue.<</widget>>",
            "&lt;&lt;script&gt;&gt;Hidden ordinary dialogue.&lt;&lt;/script&gt;&gt;",
            "&lt;&lt;widget \"test\"&gt;&gt;Hidden ordinary dialogue.&lt;&lt;/widget&gt;&gt;",
        ];
        let dir_guard = tempdir();
        let html = create_fixture(dir_guard.path());
        let plugin = SugarCubePlugin::new();
        for body in hidden {
            let original = format!(
                r#"<tw-passagedata pid="1" name="Start" tags="">{body}
Visible dialogue.</tw-passagedata>"#
            );
            fs::write(&html, &original).unwrap();
            let mut entries = plugin.extract(&html).unwrap();
            assert_eq!(entries.len(), 1, "hidden body leaked: {body}");
            assert_eq!(entries[0].source, "Visible dialogue.");
            entries[0].translation = Some("Diálogo visible.".into());
            let report = plugin.inject(&html, &entries).unwrap();
            assert_eq!(report.strings_written, 1);
            assert_eq!(
                fs::read_to_string(&html).unwrap(),
                original.replace("Visible dialogue.", "Diálogo visible.")
            );
        }
    }

    #[test]
    fn script_stylesheet_and_widget_passages_are_not_extracted() {
        for tag in ["script", "stylesheet", "widget"] {
            let html = format!(
                r#"<tw-passagedata pid="1" name="Hidden" tags="{tag}">Hidden ordinary dialogue.</tw-passagedata>
<tw-passagedata pid="2" name="Start" tags="">Visible dialogue.</tw-passagedata>"#
            );
            let entries = SugarCubePlugin::extract_passages(&html, Path::new("test.html"));
            assert_eq!(entries.len(), 1, "hidden passage leaked: {tag}");
            assert_eq!(entries[0].source, "Visible dialogue.");
        }
    }

    #[test]
    fn test_detect_sugarcube() {
        let dir_guard = tempdir();
        let dir = dir_guard.path().to_path_buf();
        create_fixture(&dir);
        let plugin = SugarCubePlugin::new();
        assert!(plugin.detect(&dir));
    }

    #[test]
    fn test_detect_non_sugarcube() {
        let dir_guard = tempdir();
        let dir = dir_guard.path().to_path_buf();
        fs::write(dir.join("index.html"), "<html><body>normal</body></html>").unwrap();
        let plugin = SugarCubePlugin::new();
        assert!(!plugin.detect(&dir));
    }

    #[test]
    fn generic_html_file_is_rejected_and_registry_selects_html_game() {
        let dir_guard = tempdir();
        let dir = dir_guard.path().to_path_buf();
        let html = dir.join("story.html");
        fs::write(&html, "<p>Original one</p>").unwrap();

        let plugin = SugarCubePlugin::new();
        assert!(!plugin.detect(&html));
        assert!(plugin.extract(&html).is_err());
        assert!(plugin.inject(&html, &[]).is_err());

        let registry = crate::default_registry();
        let detected = registry
            .detect(&html)
            .expect("generic HTML should be auto-detected");
        assert_eq!(detected.id(), "html-game");
        assert!(!detected.extract(&html).unwrap().is_empty());
    }

    #[test]
    fn sugarcube_file_and_directory_are_detected_by_plugin_and_registry() {
        let dir_guard = tempdir();
        let dir = dir_guard.path().to_path_buf();
        let html = dir.join("story.htm");
        fs::write(
            &html,
            r#"<tw-storydata name="Test">
<tw-passagedata pid="1" name="Start" tags="">Café visible</tw-passagedata>
</tw-storydata>"#,
        )
        .unwrap();

        let plugin = SugarCubePlugin::new();
        assert!(plugin.detect(&html));
        assert!(plugin.detect(&dir));
        let from_file = plugin.extract(&html).unwrap();
        let from_dir = plugin.extract(&dir).unwrap();
        assert!(from_file.iter().any(|entry| entry.source.contains("Café")));
        assert_eq!(from_file.len(), from_dir.len());

        let registry = crate::default_registry();
        assert_eq!(
            registry.detect(&html).expect("file detected").id(),
            "sugarcube"
        );
        assert_eq!(
            registry.detect(&dir).expect("directory detected").id(),
            "sugarcube"
        );
    }

    #[test]
    fn test_extract_passages() {
        let dir_guard = tempdir();
        let dir = dir_guard.path().to_path_buf();
        create_fixture(&dir);
        let plugin = SugarCubePlugin::new();
        let entries = plugin.extract(&dir).unwrap();
        assert!(
            entries.len() >= 4,
            "got {} entries: {:?}",
            entries.len(),
            entries.iter().map(|e| &e.source).collect::<Vec<_>>()
        );

        let sources: Vec<&str> = entries.iter().map(|e| e.source.as_str()).collect();
        assert!(sources.iter().any(|s| s.contains("welcome")));
        assert!(sources.iter().any(|s| s.contains("adventure")));
    }

    #[test]
    fn test_extract_strips_macros() {
        let dir_guard = tempdir();
        let dir = dir_guard.path().to_path_buf();
        create_fixture(&dir);
        let plugin = SugarCubePlugin::new();
        let entries = plugin.extract(&dir).unwrap();
        for e in &entries {
            assert!(
                !e.source.contains("<<"),
                "source should not contain macros: {}",
                e.source
            );
        }
    }

    fn binding_fixture(dir: &Path, name: &str) -> PathBuf {
        let fixture = create_fixture(dir);
        let content = fs::read_to_string(&fixture)
            .unwrap()
            .replace("Hello, welcome to the game!", "Hello traveler")
            .replace("name=\"Test\"", &format!("name=\"{name}\""));
        let story = dir.join(name);
        fs::rename(fixture, &story).unwrap();
        fs::write(&story, content).unwrap();
        story
    }

    fn binding_translation(plugin: &SugarCubePlugin, story: &Path) -> StringEntry {
        let mut entry = plugin
            .extract(story)
            .unwrap()
            .into_iter()
            .find(|entry| entry.source == "Hello traveler")
            .unwrap();
        entry.translation = Some("Hola viajero".into());
        entry
    }

    #[test]
    fn target_binding_missing_story_does_not_redirect_to_another_story() {
        let dir = tempdir();
        let a = binding_fixture(dir.path(), "A.html");
        let plugin = SugarCubePlugin::new();
        let entry = binding_translation(&plugin, &a);
        let b = binding_fixture(dir.path(), "B.html");
        let before = fs::read(&b).unwrap();
        fs::remove_file(&a).unwrap();

        let report = plugin.inject(dir.path(), &[entry]).unwrap();
        assert_eq!((report.strings_written, report.files_modified), (0, 0));
        assert_eq!(report.strings_skipped, 1);
        assert_eq!(report.skip_reasons.get("missing_target"), Some(&1));
        assert!(report.files_written.is_empty());
        assert_eq!(fs::read(&b).unwrap(), before);
        assert!(!a.exists());
    }

    #[test]
    fn target_binding_directory_discovery_order_cannot_redirect_injection() {
        let dir = tempdir();
        let a = binding_fixture(dir.path(), "A.html");
        let b = binding_fixture(dir.path(), "B.html");
        let plugin = SugarCubePlugin::new();
        // Always select the story that discovery would NOT pick, whatever the
        // filesystem's iteration order happens to be.
        let discovered = SugarCubePlugin::find_html_file(dir.path()).unwrap();
        let target = if discovered == a { b } else { a };
        let other_before = fs::read(&discovered).unwrap();
        let target_before = fs::read_to_string(&target).unwrap();
        let entry = binding_translation(&plugin, &target);

        let report = plugin.inject(dir.path(), &[entry]).unwrap();
        assert_eq!((report.strings_written, report.files_modified), (1, 1));
        assert_eq!(report.strings_skipped, 0);
        assert_eq!(report.files_written, vec![target.clone()]);
        assert_eq!(fs::read(&discovered).unwrap(), other_before);
        assert_eq!(
            fs::read_to_string(&target).unwrap(),
            target_before.replace("Hello traveler", "Hola viajero")
        );
    }

    #[test]
    fn target_binding_translated_copy_uses_the_stored_story_name() {
        for (remapped, nested) in [(false, false), (true, false), (true, true)] {
            let source = tempdir();
            let copy = tempdir();
            let original = binding_fixture(source.path(), "A.html");
            let story_dir = if nested {
                copy.path().join("stories")
            } else {
                copy.path().to_path_buf()
            };
            fs::create_dir_all(&story_dir).unwrap();
            let target = binding_fixture(&story_dir, "A.html");
            let other = binding_fixture(copy.path(), if nested { "A.html" } else { "B.html" });
            let plugin = SugarCubePlugin::new();
            let mut entry = binding_translation(&plugin, &original);
            if remapped {
                // Core's translated-copy dispatcher already remaps entry paths.
                entry.file_path = target.clone();
            }
            let original_before = fs::read(&original).unwrap();
            let other_before = fs::read(&other).unwrap();
            let target_before = fs::read_to_string(&target).unwrap();

            let report = plugin.inject(copy.path(), &[entry]).unwrap();
            assert_eq!((report.strings_written, report.files_modified), (1, 1));
            assert_eq!(report.strings_skipped, 0);
            assert_eq!(report.files_written, vec![target.clone()]);
            assert_eq!(fs::read(&original).unwrap(), original_before);
            assert_eq!(fs::read(&other).unwrap(), other_before);
            assert_eq!(
                fs::read_to_string(&target).unwrap(),
                target_before.replace("Hello traveler", "Hola viajero")
            );
        }
    }

    #[test]
    fn target_binding_explicit_single_file_selection_still_works() {
        let dir = tempdir();
        let original = binding_fixture(dir.path(), "A.html");
        let plugin = SugarCubePlugin::new();
        let entry = binding_translation(&plugin, &original);
        let selected = dir.path().join("chosen.htm");
        fs::copy(&original, &selected).unwrap();
        let before = fs::read(&original).unwrap();

        let report = plugin.inject(&selected, &[entry]).unwrap();
        assert_eq!((report.strings_written, report.files_modified), (1, 1));
        assert_eq!(report.strings_skipped, 0);
        assert_eq!(report.files_written, vec![selected.clone()]);
        assert_eq!(fs::read(&original).unwrap(), before);
        assert_eq!(
            fs::read_to_string(&selected).unwrap(),
            String::from_utf8(before)
                .unwrap()
                .replace("Hello traveler", "Hola viajero")
        );
    }

    #[test]
    fn target_binding_refuses_mixed_or_ambiguous_entry_files() {
        for same_basename in [false, true] {
            let dir = tempdir();
            let external = tempdir();
            let a = binding_fixture(dir.path(), "A.html");
            let b = binding_fixture(dir.path(), "B.html");
            let plugin = SugarCubePlugin::new();
            let first = binding_translation(&plugin, &a);
            let mut second = first.clone();
            // A different row would otherwise be independently writable.
            second.id = plugin.extract(&a).unwrap()[1].id.clone();
            second.source = "This is the second line.".into();
            second.translation = Some("Esta es la segunda línea.".into());
            second.file_path = if same_basename {
                external.path().join("A.html")
            } else {
                b.clone()
            };
            let a_before = fs::read(&a).unwrap();
            let b_before = fs::read(&b).unwrap();

            for selection in [dir.path(), a.as_path()] {
                let report = plugin
                    .inject(selection, &[first.clone(), second.clone()])
                    .unwrap();
                assert_eq!((report.strings_written, report.files_modified), (0, 0));
                assert_eq!(report.strings_skipped, 2);
                assert_eq!(report.skip_reasons.get("ambiguous_target"), Some(&2));
                assert!(report.files_written.is_empty());
                assert_eq!(fs::read(&a).unwrap(), a_before);
                assert_eq!(fs::read(&b).unwrap(), b_before);
            }
        }
    }

    #[test]
    fn test_inject_replace() {
        let dir_guard = tempdir();
        let dir = dir_guard.path().to_path_buf();
        create_fixture(&dir);
        let plugin = SugarCubePlugin::new();
        let mut entries = plugin.extract(&dir).unwrap();
        let total = entries.len();

        for entry in &mut entries {
            if entry.source.contains("welcome") {
                entry.translation = Some("Bienvenido al juego!".to_string());
            }
        }

        let report = plugin.inject(&dir, &entries).unwrap();
        assert_eq!(report.strings_written, 1);
        assert_eq!(report.strings_skipped, total - 1);
        assert_eq!(report.skip_reasons.get("untranslated"), Some(&(total - 1)));
    }

    #[test]
    fn test_context_is_passage_name() {
        let dir_guard = tempdir();
        let dir = dir_guard.path().to_path_buf();
        create_fixture(&dir);
        let plugin = SugarCubePlugin::new();
        let entries = plugin.extract(&dir).unwrap();
        assert!(entries
            .iter()
            .any(|e| e.context == Some("Start".to_string())));
        assert!(entries
            .iter()
            .any(|e| e.context == Some("next".to_string())));
    }

    #[test]
    fn test_protect_variables() {
        let (protected, vars) = protect_variables("Hey $name, how are you?");
        assert_eq!(protected, "Hey {0}, how are you?");
        assert_eq!(vars.len(), 1);
        assert_eq!(vars[0].1, "$name");

        let restored = restore_variables("Hola {0}, ¿cómo estás?", &vars);
        assert_eq!(restored, "Hola $name, ¿cómo estás?");
    }

    #[test]
    fn test_protect_temp_variables() {
        let (protected, vars) = protect_variables("Value is _contents here");
        assert!(protected.contains("{0}"));
        assert_eq!(vars[0].1, "_contents");
    }

    #[test]
    fn test_skip_system_passages() {
        assert!(is_system_passage("PassageReady", ""));
        assert!(is_system_passage("PassageDone", ""));
        assert!(is_system_passage("StoryInit", ""));
        assert!(is_system_passage("SomeWidget", "widget"));
        assert!(!is_system_passage("Introduction", ""));
    }

    #[test]
    fn test_inject_changes_only_selected_duplicate_row() {
        let dir_guard = tempdir();
        let dir = dir_guard.path().to_path_buf();
        let html = dir.join("game.html");
        fs::write(
            &html,
            r#"<tw-passagedata pid="1" name="Chapter#One" tags="">Repeat me
Repeat me</tw-passagedata>
<tw-passagedata pid="2" name="Other" tags="">Repeat me</tw-passagedata>"#,
        )
        .unwrap();
        let plugin = SugarCubePlugin::new();
        let mut selected = plugin
            .extract(&html)
            .unwrap()
            .into_iter()
            .find(|entry| entry.id == "passage_1#Chapter#One#1")
            .unwrap();
        selected.translation = Some("Solo esta".to_string());

        let report = plugin.inject(&html, &[selected]).unwrap();
        let changed = fs::read_to_string(&html).unwrap();
        assert_eq!(report.strings_written, 1);
        assert_eq!(changed.matches("Repeat me").count(), 2);
        assert!(changed.contains("Repeat me\nSolo esta"));
        assert!(changed.contains(r#"pid="2" name="Other" tags="">Repeat me"#));
    }

    #[test]
    fn test_stale_source_is_skipped_without_rewrite() {
        let dir_guard = tempdir();
        let dir = dir_guard.path().to_path_buf();
        let html = dir.join("game.html");
        fs::write(
            &html,
            r#"<tw-passagedata pid="1" name="Start" tags="">Change me</tw-passagedata>"#,
        )
        .unwrap();
        let plugin = SugarCubePlugin::new();
        let mut selected = plugin.extract(&html).unwrap().remove(0);
        selected.translation = Some("Cámbiame".to_string());
        let stale = fs::read_to_string(&html)
            .unwrap()
            .replace("Change me", "Changed upstream");
        fs::write(&html, &stale).unwrap();
        let before = fs::read(&html).unwrap();

        let report = plugin.inject(&html, &[selected]).unwrap();
        assert_eq!(report.strings_written, 0);
        assert_eq!(report.skip_reasons.get("source_changed"), Some(&1));
        assert_eq!(fs::read(&html).unwrap(), before);
    }

    #[test]
    fn joined_inline_link_row_is_not_flattened_or_emptied() {
        let dir_guard = tempdir();
        let dir = dir_guard.path().to_path_buf();
        let html = dir.join("game.html");
        let original = r#"<tw-passagedata pid="1" name="Start" tags="">Please [[continue|Next]] now.</tw-passagedata>"#;
        fs::write(&html, original).unwrap();
        let plugin = SugarCubePlugin::new();
        let entries = plugin.extract(&html).unwrap();
        assert_eq!(
            entries
                .iter()
                .map(|entry| (entry.id.as_str(), entry.source.as_str()))
                .collect::<Vec<_>>(),
            vec![
                ("passage_1#Start#0#f0", "Please "),
                ("passage_1#Start#0#f1", "continue"),
                ("passage_1#Start#0#f2", " now."),
            ]
        );

        let mut legacy =
            StringEntry::new("passage_1#Start#0", "Please continue now.", html.clone());
        legacy.translation = Some("Continúa ahora, por favor.".into());
        let report = plugin.inject(&html, &[legacy]).unwrap();
        assert_eq!(report.strings_written, 0);
        assert_eq!(report.skip_reasons.get("unsupported_markup"), Some(&1));
        assert_eq!(fs::read_to_string(&html).unwrap(), original);

        let mut label = entries
            .into_iter()
            .find(|entry| entry.id == "passage_1#Start#0#f1")
            .unwrap();
        label.translation = Some("continúa".into());
        let report = plugin.inject(&html, &[label]).unwrap();
        let changed = fs::read_to_string(&html).unwrap();
        assert_eq!(report.strings_written, 1);
        assert!(changed.contains(r#"Please [[&quot;continúa&quot;|Next]] now."#));
        assert!(!changed.contains("[[continúa|Next]]now"));
        assert!(!changed.contains("Please continúa"));
    }

    #[test]
    fn quoted_gt_in_passage_name_does_not_split_header() {
        let dir_guard = tempdir();
        let dir = dir_guard.path().to_path_buf();
        let html = dir.join("game.html");
        fs::write(
            &html,
            r#"<tw-passagedata pid="4" name="A > B" tags="x > y">Visible line</tw-passagedata>"#,
        )
        .unwrap();
        let plugin = SugarCubePlugin::new();
        let mut entries = plugin.extract(&html).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].id, "passage_4#A > B#0");
        assert_eq!(entries[0].source, "Visible line");
        entries[0].translation = Some("Línea visible".into());
        let report = plugin.inject(&html, &entries).unwrap();
        assert_eq!(report.strings_written, 1);
        let changed = fs::read_to_string(&html).unwrap();
        assert!(changed.contains(r#"name="A > B" tags="x > y">Línea visible"#));
    }

    #[test]
    fn fragment_locators_survive_hash_in_passage_name() {
        let loc = parse_passage_locator("passage_1#Chapter#One#2#f3").unwrap();
        assert_eq!(loc.pid, "1");
        assert_eq!(loc.name, "Chapter#One");
        assert_eq!(loc.row, 2);
        assert_eq!(loc.fragment, Some(3));
        assert_eq!(
            parse_passage_locator("passage_1#Chapter#One#2")
                .unwrap()
                .fragment,
            None
        );
        assert!(parse_passage_locator("passage_1#Chapter#One#2#f01").is_none());
        assert!(parse_passage_locator("passage_1#Start#f0").is_none());
    }

    #[test]
    fn malformed_fragment_metadata_is_rejected() {
        let dir_guard = tempdir();
        let dir = dir_guard.path().to_path_buf();
        let html = dir.join("game.html");
        let original = r#"<tw-passagedata pid="1" name="Start" tags="">Please [[continue|Next]] now.</tw-passagedata>"#;
        fs::write(&html, original).unwrap();
        let plugin = SugarCubePlugin::new();
        let mut selected = plugin
            .extract(&html)
            .unwrap()
            .into_iter()
            .find(|entry| entry.id == "passage_1#Start#0#f1")
            .unwrap();
        selected.translation = Some("continúa".into());
        selected
            .metadata
            .insert("sugarcube_structure".into(), serde_json::json!(1));
        let before = fs::read(&html).unwrap();
        let report = plugin.inject(&html, &[selected]).unwrap();
        assert_eq!(report.strings_written, 0);
        assert_eq!(report.skip_reasons.get("source_changed"), Some(&1));
        assert_eq!(fs::read(&html).unwrap(), before);
    }

    #[test]
    fn two_layer_encoding_keeps_wiki_and_html_literal() {
        assert_eq!(
            encode_html_entities("it's <b>x</b> <<set $coins=99>> [[text|Else]] [ok]"),
            "it&amp;#39;s &amp;#60;b&amp;#62;x&amp;#60;/b&amp;#62; &amp;#60;&amp;#60;set $coins=99&amp;#62;&amp;#62; &amp;#91;&amp;#91;text&amp;#124;Else&amp;#93;&amp;#93; &amp;#91;ok&amp;#93;"
        );
    }

    #[test]
    fn link_label_uses_json_string_and_one_html_escape() {
        assert_eq!(
            encode_link_label("it's <b>x</b> [[text|Else]]"),
            r#"&quot;it's &lt;b&gt;x&lt;/b&gt; [[text|Else]]&quot;"#
        );
        assert_eq!(
            encode_link_label("say \"hi\"\\pipe|here"),
            r#"&quot;say \&quot;hi\&quot;\\pipe|here&quot;"#
        );
    }

    #[test]
    fn quoted_delimiters_do_not_expose_macro_or_attribute_text() {
        let dir_guard = tempdir();
        let dir = dir_guard.path().to_path_buf();
        let html = dir.join("game.html");
        let original = r#"<tw-passagedata pid="1" name="Start" tags="">&lt;&lt;set $value = &quot;Secret &gt;&gt; word&quot;&gt;&gt;&lt;span title=&quot;Secret &gt; attribute&quot;&gt;Visible text&lt;/span&gt;</tw-passagedata>"#;
        fs::write(&html, original).unwrap();
        let plugin = SugarCubePlugin::new();
        let mut entries = plugin.extract(&html).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].source, "Visible text");
        entries[0].translation = Some("Texto visible".into());
        let report = plugin.inject(&html, &entries).unwrap();
        assert_eq!(report.strings_written, 1);
        assert_eq!(
            fs::read_to_string(html).unwrap(),
            original.replace("Visible text", "Texto visible")
        );
    }

    #[test]
    fn test_inject_protects_encoded_markup_macro_and_link_target() {
        let dir_guard = tempdir();
        let dir = dir_guard.path().to_path_buf();
        let html = dir.join("game.html");
        let original = r#"<tw-passagedata pid="7" name="Start" tags="">&lt;span data-label=&quot;Open&quot;&gt;&lt;/span&gt;&lt;&lt;set $next = &quot;Secret&quot;&gt;&gt;[[Open|Secret]]</tw-passagedata>"#;
        fs::write(&html, original).unwrap();
        let plugin = SugarCubePlugin::new();
        let mut selected = plugin.extract(&html).unwrap().remove(0);
        assert_eq!(selected.source, "Open");
        selected.translation = Some("Abrir".to_string());

        let report = plugin.inject(&html, &[selected]).unwrap();
        let changed = fs::read_to_string(&html).unwrap();
        assert_eq!(report.strings_written, 1);
        assert!(changed.contains(r#"data-label=&quot;Open&quot;"#));
        assert!(changed.contains(r#"&lt;&lt;set $next = &quot;Secret&quot;&gt;&gt;"#));
        assert!(changed.contains(r#"[[&quot;Abrir&quot;|Secret]]"#));
    }

    #[test]
    fn test_extract_inject_extract_roundtrip() {
        let dir_guard = tempdir();
        let dir = dir_guard.path().to_path_buf();
        let html = dir.join("game.html");
        fs::write(
            &html,
            r#"<tw-passagedata pid="3" name="Roundtrip" tags="">Hello $name!</tw-passagedata>"#,
        )
        .unwrap();
        let plugin = SugarCubePlugin::new();
        let mut selected = plugin.extract(&html).unwrap().remove(0);
        let id = selected.id.clone();
        assert_eq!(selected.source, "Hello {0}!");
        selected.translation = Some("Hola {0}!".to_string());

        let report = plugin.inject(&html, &[selected]).unwrap();
        let extracted = plugin.extract(&html).unwrap();
        assert_eq!(report.strings_written, 1);
        assert!(extracted
            .iter()
            .any(|entry| entry.id == id && entry.source == "Hola {0}!"));
    }

    #[test]
    fn test_skip_widget_passages() {
        let html = r#"<tw-passagedata pid="1" name="widgets" tags="widget">&lt;&lt;widget "test"&gt;&gt;_contents&lt;&lt;/widget&gt;&gt;</tw-passagedata>"#;
        let entries = SugarCubePlugin::extract_passages(html, Path::new("test.html"));
        assert!(entries.is_empty(), "widget passages should be skipped");
    }

    #[test]
    fn test_bare_variable_filtered() {
        assert!(!is_extractable_line("_contents"));
        assert!(!is_extractable_line("$name"));
        assert!(!is_extractable_line("Images/Clothes/1.webp"));
        assert!(is_extractable_line("Hello world"));
    }
}
