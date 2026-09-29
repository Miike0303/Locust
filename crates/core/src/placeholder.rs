use serde::{Deserialize, Serialize};

use crate::error::{LocustError, Result};

pub struct PlaceholderProcessor;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Placeholder {
    pub index: usize,
    pub token: String,
    pub original: String,
    pub pattern_type: PlaceholderKind,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum PlaceholderKind {
    RpgMakerCode,
    HtmlTag,
    PythonFormat,
    RustFormat,
    CFormat,
    CustomBracket,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PlaceholderMismatch {
    pub kind: MismatchKind,
    pub placeholder: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum MismatchKind {
    Missing,
    Extra,
    Unbalanced,
}

struct PatternMatch {
    start: usize,
    end: usize,
    text: String,
    kind: PlaceholderKind,
}

impl PlaceholderProcessor {
    pub fn extract(source: &str) -> (String, Vec<Placeholder>) {
        let mut matches = Vec::new();

        // Collect all pattern matches
        Self::find_rpgmaker(source, &mut matches);
        Self::find_html_tags(source, &mut matches);
        Self::find_python_format(source, &mut matches);
        Self::find_rust_format(source, &mut matches);
        Self::find_c_format(source, &mut matches);
        Self::find_custom_brackets(source, &mut matches);

        // Sort by start position and remove overlaps
        matches.sort_by_key(|m| m.start);
        let matches = Self::remove_overlaps(matches);

        if matches.is_empty() {
            return (source.to_string(), Vec::new());
        }

        // Build sanitized string and placeholders
        let mut sanitized = String::new();
        let mut placeholders = Vec::new();
        let mut last_end = 0;

        for (idx, m) in matches.into_iter().enumerate() {
            sanitized.push_str(&source[last_end..m.start]);
            let token = format!("{{PL_{}}}", idx);
            sanitized.push_str(&token);
            placeholders.push(Placeholder {
                index: idx,
                token,
                original: m.text,
                pattern_type: m.kind,
            });
            last_end = m.end;
        }
        sanitized.push_str(&source[last_end..]);

        (sanitized, placeholders)
    }

    pub fn restore(translated: &str, placeholders: &[Placeholder]) -> Result<String> {
        let mut result = translated.to_string();

        // Check for extra tokens
        for i in 0..100 {
            let token = format!("{{PL_{}}}", i);
            if result.contains(&token) && !placeholders.iter().any(|p| p.index == i) {
                return Err(LocustError::PlaceholderError {
                    entry_id: String::new(),
                    message: format!("extra placeholder token {} in translation", token),
                });
            }
        }

        // Replace tokens with originals
        for ph in placeholders {
            let count = translated.matches(ph.token.as_str()).count();
            if count == 0 {
                return Err(LocustError::PlaceholderError {
                    entry_id: String::new(),
                    message: format!(
                        "missing placeholder token {} (original: {})",
                        ph.token, ph.original
                    ),
                });
            }
            if count > 1 {
                return Err(LocustError::PlaceholderError {
                    entry_id: String::new(),
                    message: format!(
                        "duplicate placeholder token {} appears {} times in translation",
                        ph.token, count
                    ),
                });
            }
            result = result.replacen(&ph.token, &ph.original, 1);
        }

        Ok(result)
    }

    pub fn validate(original: &str, translated: &str) -> Vec<PlaceholderMismatch> {
        let (_, orig_phs) = Self::extract(original);
        let (_, trans_phs) = Self::extract(translated);

        let orig_texts: Vec<&str> = orig_phs.iter().map(|p| p.original.as_str()).collect();
        let trans_texts: Vec<&str> = trans_phs.iter().map(|p| p.original.as_str()).collect();

        let mut mismatches = Vec::new();

        // Find missing placeholders (in original but not in translated)
        let mut trans_remaining: Vec<&str> = trans_texts.clone();
        for orig in &orig_texts {
            if let Some(pos) = trans_remaining.iter().position(|t| t == orig) {
                trans_remaining.remove(pos);
            } else {
                mismatches.push(PlaceholderMismatch {
                    kind: MismatchKind::Missing,
                    placeholder: orig.to_string(),
                });
            }
        }

        // Find extra placeholders (in translated but not in original)
        let mut orig_remaining: Vec<&str> = orig_texts;
        for trans in &trans_texts {
            if let Some(pos) = orig_remaining.iter().position(|t| t == trans) {
                orig_remaining.remove(pos);
            } else {
                mismatches.push(PlaceholderMismatch {
                    kind: MismatchKind::Extra,
                    placeholder: trans.to_string(),
                });
            }
        }

        // Only impose nesting when the original contains balanced, recognized
        // paired Ren'Py tags. Partial strings and literal {i} format fields do
        // not acquire a new balancing requirement from this generic processor.
        if renpy_tags_balanced(&orig_phs) == Some(true)
            && renpy_tags_balanced(&trans_phs) == Some(false)
        {
            mismatches.push(PlaceholderMismatch {
                kind: MismatchKind::Unbalanced,
                placeholder: "Ren'Py formatting tags must retain valid nesting".into(),
            });
        }
        mismatches
    }

    fn find_rpgmaker(source: &str, matches: &mut Vec<PatternMatch>) {
        // RPG Maker codes: \c[N], \v[N], \n[N], \p[N] and simple ones \n, \g, \$, etc.
        let bytes = source.as_bytes();
        let len = bytes.len();
        let mut i = 0;
        while i < len {
            if bytes[i] == b'\\' && i + 1 < len {
                let next = bytes[i + 1].to_ascii_lowercase();
                if next == b'\\' {
                    i += 2;
                    continue;
                }
                // Codes with brackets: \c[N], \v[N], \n[N], \p[N]
                if b"cvnpi".contains(&next) && i + 2 < len && bytes[i + 2] == b'[' {
                    if let Some(close) = source[i + 3..].find(']') {
                        let end = i + 3 + close + 1;
                        matches.push(PatternMatch {
                            start: i,
                            end,
                            text: source[i..end].to_string(),
                            kind: PlaceholderKind::RpgMakerCode,
                        });
                        i = end;
                        continue;
                    }
                }
                // Simple codes: \g, \$, \., \|, \!, \>, \<, \^
                if b"g$.!><^|".contains(&next) {
                    matches.push(PatternMatch {
                        start: i,
                        end: i + 2,
                        text: source[i..i + 2].to_string(),
                        kind: PlaceholderKind::RpgMakerCode,
                    });
                    i += 2;
                    continue;
                }
                // \n without bracket (newline in RPG Maker context) - skip, handled by CFormat
            }
            i += 1;
        }
    }

    fn find_html_tags(source: &str, matches: &mut Vec<PatternMatch>) {
        let mut i = 0;
        let bytes = source.as_bytes();
        let len = bytes.len();
        while i < len {
            if bytes[i] == b'<' {
                if let Some(close) = source[i..].find('>') {
                    let end = i + close + 1;
                    let tag = &source[i..end];
                    // Basic validation: must look like a tag
                    if tag.len() >= 3
                        && (tag.starts_with("</")
                            || tag.chars().nth(1).is_some_and(|c| c.is_ascii_alphabetic()))
                    {
                        matches.push(PatternMatch {
                            start: i,
                            end,
                            text: tag.to_string(),
                            kind: PlaceholderKind::HtmlTag,
                        });
                        i = end;
                        continue;
                    }
                }
            }
            i += 1;
        }
    }

    fn find_python_format(source: &str, matches: &mut Vec<PatternMatch>) {
        let bytes = source.as_bytes();
        let len = bytes.len();
        let mut i = 0;
        while i < len {
            if bytes[i] == b'%' && i + 1 < len {
                let next = bytes[i + 1];
                // %% is an escaped literal percent, including in %%1.
                if next == b'%' {
                    i += 2;
                    continue;
                }
                // %(name)s style
                if next == b'(' {
                    if let Some(close) = source[i + 2..].find(')') {
                        let after_paren = i + 2 + close + 1;
                        if after_paren < len && b"sdifg".contains(&bytes[after_paren]) {
                            let end = after_paren + 1;
                            matches.push(PatternMatch {
                                start: i,
                                end,
                                text: source[i..end].to_string(),
                                kind: PlaceholderKind::PythonFormat,
                            });
                            i = end;
                            continue;
                        }
                    }
                }
                // %s, %d, %i, %f and RPG Maker's single-digit %1..%9.
                if b"sdif".contains(&next)
                    || ((b'1'..=b'9').contains(&next)
                        && !bytes.get(i + 2).is_some_and(u8::is_ascii_digit))
                {
                    matches.push(PatternMatch {
                        start: i,
                        end: i + 2,
                        text: source[i..i + 2].to_string(),
                        kind: PlaceholderKind::PythonFormat,
                    });
                    i += 2;
                    continue;
                }
            }
            i += 1;
        }
    }

    fn find_rust_format(source: &str, matches: &mut Vec<PatternMatch>) {
        let bytes = source.as_bytes();
        let len = bytes.len();
        let mut i = 0;
        while i < len {
            if bytes[i] == b'{' {
                // Double opening braces escape a literal brace in Ren'Py and format strings.
                if bytes.get(i + 1) == Some(&b'{') {
                    i += 2;
                    continue;
                }
                // Check for PL_ tokens — skip those
                if source[i..].starts_with("{PL_") {
                    i += 1;
                    continue;
                }
                if let Some(close) = source[i..].find('}') {
                    let end = i + close + 1;
                    let inner = &source[i + 1..end - 1];
                    // {} / {0} / {name} / Ren'Py-style {i} — not binary-noise {P}
                    if is_rust_format_inner(inner) {
                        matches.push(PatternMatch {
                            start: i,
                            end,
                            text: source[i..end].to_string(),
                            kind: PlaceholderKind::RustFormat,
                        });
                        i = end;
                        continue;
                    }
                }
            }
            i += 1;
        }
    }

    fn find_c_format(source: &str, matches: &mut Vec<PatternMatch>) {
        let bytes = source.as_bytes();
        let len = bytes.len();
        let mut i = 0;
        while i < len {
            if bytes[i] == b'\\' && i + 1 < len {
                let next = bytes[i + 1];
                if next == b'\\' {
                    i += 2;
                    continue;
                }
                if next == b'n' || next == b't' {
                    // Don't match if already captured by RPG Maker (e.g. \n[1])
                    if next == b'n' && i + 2 < len && bytes[i + 2] == b'[' {
                        i += 1;
                        continue;
                    }
                    matches.push(PatternMatch {
                        start: i,
                        end: i + 2,
                        text: source[i..i + 2].to_string(),
                        kind: PlaceholderKind::CFormat,
                    });
                    i += 2;
                    continue;
                }
            }
            i += 1;
        }
    }

    fn find_custom_brackets(source: &str, matches: &mut Vec<PatternMatch>) {
        let bytes = source.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i] == b'[' {
                // Ren'Py [[ spells a literal opening bracket.
                if bytes.get(i + 1) == Some(&b'[') {
                    i += 2;
                    continue;
                }
                if let Some(end) = bracket_end(source, i) {
                    let inner = &source[i + 1..end - 1];
                    if is_renpy_interpolation(inner) {
                        matches.push(PatternMatch {
                            start: i,
                            end,
                            text: source[i..end].to_string(),
                            kind: PlaceholderKind::CustomBracket,
                        });
                        i = end;
                        continue;
                    }
                    // Do not reinterpret an inner bracket of an unsupported
                    // expression as an independent variable.
                    i = end;
                    continue;
                }
            }
            i += 1;
        }
    }

    fn remove_overlaps(sorted: Vec<PatternMatch>) -> Vec<PatternMatch> {
        let mut result: Vec<PatternMatch> = Vec::new();
        for m in sorted {
            if let Some(last) = result.last() {
                if m.start < last.end {
                    continue; // overlaps, skip
                }
            }
            result.push(m);
        }
        result
    }
}

/// Whether `{inner}` looks like a real format/text tag, not binary-scan noise.
///
/// Accepts `{}`, `{0}`, `{name}`, Ren'Py open tags (`{i}`, `{b}`, …), and Ren'Py
/// **closing** tags (`{/i}`, `{/b}`, `{/color}`). Rejects single **uppercase**
/// letters (`{P}`, `{F}`, `{G}`) that dominate Unreal/Unity heuristic dumps and
/// only produce restore-fail noise under mock/length-safe translate.
fn is_rust_format_inner(inner: &str) -> bool {
    if let Some((name, argument)) = inner.split_once('=') {
        return renpy_parameter_tag(name)
            && !argument.is_empty()
            && !argument.contains(['{', '}', '\n', '\r']);
    }
    // Ren'Py closing tag: same rules on the name after the leading '/'.
    if let Some(rest) = inner.strip_prefix('/') {
        return !rest.is_empty() && is_rust_format_name(rest);
    }
    if inner.is_empty() {
        return true;
    }
    if inner.chars().all(|c| c.is_ascii_digit()) {
        return true;
    }
    is_rust_format_name(inner)
}

/// Identifier body for open tags / names: `{name}`, `{i}`, not `{P}`.
fn is_rust_format_name(name: &str) -> bool {
    if !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return false;
    }
    let n = name.chars().count();
    if n >= 2 {
        return true;
    }
    // n == 1: lowercase Ren'Py-style only
    name.chars().next().is_some_and(|c| c.is_ascii_lowercase())
}

// Accommodate known parameterized Ren'Py tags without treating arbitrary
// prose assignments/JSON inside braces as engine controls.
fn renpy_parameter_tag(name: &str) -> bool {
    matches!(
        name,
        "a" | "alpha"
            | "color"
            | "cps"
            | "font"
            | "size"
            | "outlinecolor"
            | "plain"
            | "k"
            | "image"
            | "space"
            | "vspace"
            | "w"
            | "p"
    )
}

fn renpy_paired_tag(name: &str) -> bool {
    matches!(
        name,
        "a" | "alpha"
            | "b"
            | "i"
            | "u"
            | "s"
            | "color"
            | "cps"
            | "font"
            | "size"
            | "outlinecolor"
            | "plain"
            | "k"
            | "rb"
            | "rt"
    )
}

fn renpy_tags_balanced(placeholders: &[Placeholder]) -> Option<bool> {
    let mut stack = Vec::new();
    let mut saw_tag = false;
    for ph in placeholders {
        let Some(inner) = ph
            .original
            .strip_prefix('{')
            .and_then(|s| s.strip_suffix('}'))
        else {
            continue;
        };
        let (closing, body) = inner
            .strip_prefix('/')
            .map_or((false, inner), |body| (true, body));
        let name = body.split('=').next().unwrap_or(body);
        if !renpy_paired_tag(name) {
            continue;
        }
        saw_tag = true;
        if closing {
            if stack.pop() != Some(name) {
                return Some(false);
            }
        } else {
            stack.push(name);
        }
    }
    saw_tag.then_some(stack.is_empty())
}

fn bracket_end(source: &str, start: usize) -> Option<usize> {
    let bytes = source.as_bytes();
    let mut depth = 0usize;
    let mut quote = None;
    let mut i = start;
    while i < bytes.len() {
        let b = bytes[i];
        if let Some(q) = quote {
            if b == b'\\' {
                i += 2;
                continue;
            }
            if b == q {
                quote = None;
            }
        } else {
            match b {
                b'\'' | b'"' => quote = Some(b),
                b'[' => {
                    depth += 1;
                    if depth > 16 {
                        return None;
                    }
                }
                b']' => {
                    depth = depth.checked_sub(1)?;
                    if depth == 0 {
                        return Some(i + 1);
                    }
                }
                b'\n' | b'\r' => return None,
                _ => {}
            }
        }
        i += 1;
    }
    None
}

fn quoted_subscript_key(key: &str) -> bool {
    let bytes = key.as_bytes();
    let Some(&quote) = bytes.first() else {
        return false;
    };
    if !matches!(quote, b'\'' | b'"') {
        return false;
    }
    let mut i = 1;
    while i < bytes.len() {
        if bytes[i] == b'\\' {
            i += 2;
            continue;
        }
        if bytes[i] == quote {
            return i + 1 == bytes.len();
        }
        i += 1;
    }
    false
}

/// Deliberately bounded interpolation grammar: identifier, dotted attributes,
/// numeric/quoted-key subscripts and optional Ren'Py conversion flags. Function
/// calls, arithmetic and arbitrary Python expressions are not guessed here.
fn is_renpy_interpolation(inner: &str) -> bool {
    fn ident(bytes: &[u8], cursor: &mut usize) -> bool {
        if !bytes
            .get(*cursor)
            .is_some_and(|b| b.is_ascii_alphabetic() || *b == b'_')
        {
            return false;
        }
        *cursor += 1;
        while bytes
            .get(*cursor)
            .is_some_and(|b| b.is_ascii_alphanumeric() || *b == b'_')
        {
            *cursor += 1;
        }
        true
    }
    let bytes = inner.as_bytes();
    let mut i = 0;
    if !ident(bytes, &mut i) {
        return false;
    }
    while i < bytes.len() {
        match bytes[i] {
            b'.' => {
                i += 1;
                if !ident(bytes, &mut i) {
                    return false;
                }
            }
            b'[' => {
                let Some(end) = bracket_end(inner, i) else {
                    return false;
                };
                let key = &inner[i + 1..end - 1];
                let number = key.strip_prefix('-').unwrap_or(key);
                let numeric = !number.is_empty() && number.bytes().all(|b| b.is_ascii_digit());
                let quoted = quoted_subscript_key(key);
                if !(numeric || quoted) {
                    return false;
                }
                i = end;
            }
            b'!' => {
                let flags = &bytes[i + 1..];
                return !flags.is_empty() && flags.iter().all(|b| b"sraqtulc".contains(b));
            }
            _ => return false,
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_rpgmaker_codes() {
        let source = r"\c[2]Hero\n[1] defeated \v[10] enemies";
        let (sanitized, placeholders) = PlaceholderProcessor::extract(source);
        assert_eq!(sanitized, "{PL_0}Hero{PL_1} defeated {PL_2} enemies");
        assert_eq!(placeholders.len(), 3);
        assert_eq!(placeholders[0].original, r"\c[2]");
        assert_eq!(placeholders[1].original, r"\n[1]");
        assert_eq!(placeholders[2].original, r"\v[10]");
    }

    #[test]
    fn test_validate_rpgmaker_positional_missing() {
        assert_eq!(
            PlaceholderProcessor::validate("%1 took %2 damage!", "%1 took"),
            vec![PlaceholderMismatch {
                kind: MismatchKind::Missing,
                placeholder: "%2".into(),
            }]
        );
    }

    #[test]
    fn test_validate_rpgmaker_positional_reordered() {
        assert!(
            PlaceholderProcessor::validate("%1 took %2 damage!", "%2 recibe %1 de daño").is_empty()
        );
    }

    #[test]
    fn test_validate_rpgmaker_positional_extra() {
        assert_eq!(
            PlaceholderProcessor::validate("%1 took %2 damage!", "%1 took %2 %3"),
            vec![PlaceholderMismatch {
                kind: MismatchKind::Extra,
                placeholder: "%3".into(),
            }]
        );
    }

    #[test]
    fn test_extract_rpgmaker_positional_round_trip() {
        let source = "%1 took %2 damage!";
        let (sanitized, placeholders) = PlaceholderProcessor::extract(source);
        assert_eq!(placeholders.len(), 2);
        assert_eq!(sanitized, "{PL_0} took {PL_1} damage!");
        assert_eq!(placeholders[0].original, "%1");
        assert_eq!(placeholders[1].original, "%2");
        assert_eq!(
            PlaceholderProcessor::restore(&sanitized, &placeholders).unwrap(),
            source
        );
        assert!(PlaceholderProcessor::restore("{PL_0} took damage!", &placeholders).is_err());
    }

    #[test]
    fn test_extract_rpgmaker_positional_literals() {
        for source in ["100% sure", "%12 items", "%0", "%%1", "%90", "%%", "%"] {
            let (sanitized, placeholders) = PlaceholderProcessor::extract(source);
            assert!(placeholders.is_empty(), "{source}");
            assert_eq!(sanitized, source);
        }
    }

    #[test]
    fn test_extract_rpgmaker_positional_boundaries() {
        for digit in 1..=9 {
            let original = format!("%{digit}");
            for source in [
                original.clone(),
                format!("é{original} daño"),
                format!("{original}猫"),
            ] {
                let (sanitized, placeholders) = PlaceholderProcessor::extract(&source);
                assert_eq!(placeholders.len(), 1, "{source}");
                assert_eq!(placeholders[0].original, original);
                assert_eq!(
                    PlaceholderProcessor::restore(&sanitized, &placeholders).unwrap(),
                    source
                );
            }
        }
    }

    #[test]
    fn test_extract_python_format_with_rpgmaker_positional() {
        let source = "%s %d %i %f %(name)s %%1 %%%9";
        let (sanitized, placeholders) = PlaceholderProcessor::extract(source);
        assert_eq!(sanitized, "{PL_0} {PL_1} {PL_2} {PL_3} {PL_4} %%1 %%{PL_5}");
        assert_eq!(placeholders.len(), 6);
        assert!(placeholders
            .iter()
            .all(|ph| matches!(ph.pattern_type, PlaceholderKind::PythonFormat)));
        assert_eq!(
            PlaceholderProcessor::restore(&sanitized, &placeholders).unwrap(),
            source
        );
    }

    #[test]
    fn test_extract_html_tags() {
        let source = "<b>Hello</b> <i>world</i>";
        let (sanitized, placeholders) = PlaceholderProcessor::extract(source);
        assert_eq!(sanitized, "{PL_0}Hello{PL_1} {PL_2}world{PL_3}");
        assert_eq!(placeholders.len(), 4);
        assert_eq!(placeholders[0].original, "<b>");
        assert_eq!(placeholders[1].original, "</b>");
    }

    #[test]
    fn test_extract_rust_format() {
        let source = "Hello {name}, you have {count} items";
        let (sanitized, placeholders) = PlaceholderProcessor::extract(source);
        assert_eq!(sanitized, "Hello {PL_0}, you have {PL_1} items");
        assert_eq!(placeholders.len(), 2);
        assert_eq!(placeholders[0].original, "{name}");
        assert_eq!(placeholders[1].original, "{count}");
    }

    #[test]
    fn test_extract_renpy_single_letter_tags() {
        let source = "{i}italic{/i} and {b}bold{/b}";
        let (sanitized, placeholders) = PlaceholderProcessor::extract(source);
        let originals: Vec<&str> = placeholders.iter().map(|p| p.original.as_str()).collect();
        for expected in ["{i}", "{/i}", "{b}", "{/b}"] {
            assert!(
                originals.contains(&expected),
                "expected {expected} protected, got {originals:?} sanitized={sanitized}"
            );
        }
        assert_eq!(
            sanitized, "{PL_0}italic{PL_1} and {PL_2}bold{PL_3}",
            "open+close tags must both become tokens"
        );
    }

    #[test]
    fn test_extract_renpy_closing_rejects_uppercase_noise() {
        // {/P} is not a real Ren'Py close tag we care about; still name-rule based.
        let source = "noise {/P} end";
        let (sanitized, placeholders) = PlaceholderProcessor::extract(source);
        assert_eq!(sanitized, source);
        assert!(placeholders.is_empty());
    }

    #[test]
    fn test_extract_ignores_uppercase_single_letter_noise() {
        // Unreal/Unity heuristic scans often emit lone {P}/{F}/{G} fragments.
        let source = "Score {P} and flag {F} then {G} done";
        let (sanitized, placeholders) = PlaceholderProcessor::extract(source);
        assert_eq!(sanitized, source, "noise braces must stay as plain text");
        assert!(
            placeholders.is_empty(),
            "expected no placeholders, got {placeholders:?}"
        );
    }

    #[test]
    fn test_extract_still_keeps_digit_and_empty_braces() {
        let source = "Hi {} player {0}";
        let (sanitized, placeholders) = PlaceholderProcessor::extract(source);
        assert_eq!(sanitized, "Hi {PL_0} player {PL_1}");
        assert_eq!(placeholders.len(), 2);
        assert_eq!(placeholders[0].original, "{}");
        assert_eq!(placeholders[1].original, "{0}");
    }

    #[test]
    fn test_extract_no_placeholders() {
        let source = "Hello world";
        let (sanitized, placeholders) = PlaceholderProcessor::extract(source);
        assert_eq!(sanitized, "Hello world");
        assert!(placeholders.is_empty());
    }

    #[test]
    fn test_restore_success() {
        let source = r"\c[2]Hero\n[1] defeated \v[10] enemies";
        let (sanitized, placeholders) = PlaceholderProcessor::extract(source);
        let restored = PlaceholderProcessor::restore(&sanitized, &placeholders).unwrap();
        assert_eq!(restored, source);
    }

    #[test]
    fn test_restore_duplicate_token() {
        let (_, placeholders) = PlaceholderProcessor::extract("Hello %1");
        for (translated, count) in [("Hola {PL_0} {PL_0}", 2), ("Hola{PL_0}{PL_0}{PL_0}", 3)] {
            let err = PlaceholderProcessor::restore(translated, &placeholders).unwrap_err();
            let LocustError::PlaceholderError { message, .. } = err else {
                panic!("expected placeholder error, got {err}");
            };
            assert!(message.contains("{PL_0}"), "{message}");
            assert!(message.contains(&count.to_string()), "{message}");
        }
    }

    #[test]
    fn test_restore_tokens_once_including_adjacent_text() {
        let (_, one) = PlaceholderProcessor::extract("Hello %1");
        assert_eq!(
            PlaceholderProcessor::restore("Hola {PL_0}", &one).unwrap(),
            "Hola %1"
        );
        assert_eq!(
            PlaceholderProcessor::restore("Hola{PL_0}!", &one).unwrap(),
            "Hola%1!"
        );

        let (_, two) = PlaceholderProcessor::extract("%1 has %2");
        assert_eq!(
            PlaceholderProcessor::restore("{PL_1} para {PL_0}", &two).unwrap(),
            "%2 para %1"
        );
    }

    #[test]
    fn test_restore_missing_token() {
        let placeholders = vec![
            Placeholder {
                index: 0,
                token: "{PL_0}".to_string(),
                original: "\\c[2]".to_string(),
                pattern_type: PlaceholderKind::RpgMakerCode,
            },
            Placeholder {
                index: 1,
                token: "{PL_1}".to_string(),
                original: "\\n[1]".to_string(),
                pattern_type: PlaceholderKind::RpgMakerCode,
            },
        ];
        let translated = "{PL_0}Hero defeated enemies"; // missing {PL_1}
        let result = PlaceholderProcessor::restore(translated, &placeholders);
        assert!(result.is_err());
    }

    #[test]
    fn test_restore_extra_token() {
        let placeholders = vec![Placeholder {
            index: 0,
            token: "{PL_0}".to_string(),
            original: "\\c[2]".to_string(),
            pattern_type: PlaceholderKind::RpgMakerCode,
        }];
        let translated = "{PL_0}Hero{PL_5}"; // extra {PL_5}
        let result = PlaceholderProcessor::restore(translated, &placeholders);
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_mismatch() {
        let original = r"\c[2]Hello world";
        let translated = r"\c[3]Hello world";
        let mismatches = PlaceholderProcessor::validate(original, translated);
        assert!(!mismatches.is_empty());
        assert!(mismatches
            .iter()
            .any(|m| m.kind == MismatchKind::Missing && m.placeholder == r"\c[2]"));
        assert!(mismatches
            .iter()
            .any(|m| m.kind == MismatchKind::Extra && m.placeholder == r"\c[3]"));
    }

    #[test]
    fn test_validate_no_issues() {
        let original = r"\c[2]Hello\n[1]";
        let translated = r"\c[2]Hola\n[1]";
        let mismatches = PlaceholderProcessor::validate(original, translated);
        assert!(mismatches.is_empty());
    }

    #[test]
    fn test_extract_preserves_order() {
        let source = r"<b>\c[1]Hello {name}</b>";
        let (sanitized, placeholders) = PlaceholderProcessor::extract(source);
        assert_eq!(placeholders.len(), 4);
        // Verify order: <b>, \c[1], {name}, </b>
        assert_eq!(placeholders[0].original, "<b>");
        assert_eq!(placeholders[1].original, r"\c[1]");
        assert_eq!(placeholders[2].original, "{name}");
        assert_eq!(placeholders[3].original, "</b>");
        // Restore should give back original
        let restored = PlaceholderProcessor::restore(&sanitized, &placeholders).unwrap();
        assert_eq!(restored, source);
    }
    #[test]
    fn control_repro_rpg_uppercase_and_icon_currency_are_protected() {
        let source = r"\C[2]\V[1]\N[3]\P[1]\I[2]\G";
        let (safe, controls) = PlaceholderProcessor::extract(source);
        assert_eq!(controls.len(), 6);
        assert_eq!(
            PlaceholderProcessor::restore(&safe, &controls).unwrap(),
            source
        );
        assert!(!PlaceholderProcessor::validate(source, "Oro").is_empty());
    }

    #[test]
    fn control_repro_renpy_expressions_and_parameter_tags_are_protected() {
        let source = "{color=#fff}Hello [player.name] [values[0]!q]{/color}";
        let (safe, controls) = PlaceholderProcessor::extract(source);
        assert_eq!(controls.len(), 4);
        assert_eq!(
            PlaceholderProcessor::restore(&safe, &controls).unwrap(),
            source
        );
        assert!(!PlaceholderProcessor::validate(source, "Hola{/color}").is_empty());
    }

    #[test]
    fn control_repro_changed_nesting_is_rejected_but_words_can_move() {
        let source = "{color=#fff}{i}Hello{/i}{/color}";
        assert!(
            !PlaceholderProcessor::validate(source, "{color=#fff}{i}Hola{/color}{/i}").is_empty()
        );
        assert!(
            !PlaceholderProcessor::validate(source, "{/color}{i}Hola{/i}{color=#fff}").is_empty()
        );
        assert!(
            PlaceholderProcessor::validate(source, "Hola {color=#fff}{i}amigo{/i}{/color}")
                .is_empty()
        );
    }

    #[test]
    fn control_repro_escaped_and_literal_syntax_is_not_overprotected() {
        for source in [
            r"literal \\V[1]",
            "literal [[player.name]",
            "literal {{color=#fff}",
            "[not an expression]",
            "[1, 2]",
            "{answer=42}",
            "{P}",
        ] {
            assert!(
                PlaceholderProcessor::extract(source).1.is_empty(),
                "overprotected {source}"
            );
        }
    }
    #[test]
    fn control_renpy_partial_tags_and_supported_expression_boundaries() {
        // Strings can be fragments of a larger styled message: preserve their
        // tokens without demanding a closer absent from the original itself.
        assert!(PlaceholderProcessor::validate("{color=#fff}Hello", "{color=#fff}Hola").is_empty());
        for expression in [
            "[player_name2]",
            "[player.name]",
            "[items[0]]",
            "[items['name']]",
            "[items[\"name\"]!q]",
            "[player.name!rq]",
        ] {
            let (_, controls) = PlaceholderProcessor::extract(expression);
            assert_eq!(controls.len(), 1, "{expression}");
            assert_eq!(controls[0].original, expression);
        }
        for literal in [
            "[[player]",
            "{{name}",
            r"\\n",
            "[call()]",
            "[a + b]",
            "[items['a' + name + 'b']]",
        ] {
            assert!(
                PlaceholderProcessor::extract(literal).1.is_empty(),
                "{literal}"
            );
        }
    }
}
