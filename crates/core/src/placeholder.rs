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

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PlaceholderMismatch {
    pub kind: MismatchKind,
    pub placeholder: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum MismatchKind {
    Missing,
    Extra,
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
            if !result.contains(&ph.token) {
                return Err(LocustError::PlaceholderError {
                    entry_id: String::new(),
                    message: format!(
                        "missing placeholder token {} (original: {})",
                        ph.token, ph.original
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

        mismatches
    }

    fn find_rpgmaker(source: &str, matches: &mut Vec<PatternMatch>) {
        // RPG Maker codes: \c[N], \v[N], \n[N], \p[N] and simple ones \n, \g, \$, etc.
        let bytes = source.as_bytes();
        let len = bytes.len();
        let mut i = 0;
        while i < len {
            if bytes[i] == b'\\' && i + 1 < len {
                let next = bytes[i + 1];
                // Codes with brackets: \c[N], \v[N], \n[N], \p[N]
                if (next == b'c' || next == b'v' || next == b'n' || next == b'p')
                    && i + 2 < len
                    && bytes[i + 2] == b'['
                {
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
                            || tag.chars().nth(1).map_or(false, |c| c.is_ascii_alphabetic()))
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
                // %(name)s style
                if next == b'(' {
                    if let Some(close) = source[i + 2..].find(')') {
                        let after_paren = i + 2 + close + 1;
                        if after_paren < len
                            && b"sdifg".contains(&bytes[after_paren])
                        {
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
                // %s, %d, %i, %f
                if b"sdif".contains(&next) {
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
                // Check for PL_ tokens — skip those
                if source[i..].starts_with("{PL_") {
                    i += 1;
                    continue;
                }
                if let Some(close) = source[i..].find('}') {
                    let end = i + close + 1;
                    let inner = &source[i + 1..end - 1];
                    // {} or {0} or {name} or {name_here}
                    if inner.is_empty()
                        || inner.chars().all(|c| c.is_ascii_digit())
                        || inner
                            .chars()
                            .all(|c| c.is_ascii_alphanumeric() || c == '_')
                    {
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
        let len = bytes.len();
        let mut i = 0;
        while i < len {
            if bytes[i] == b'[' {
                if let Some(close) = source[i..].find(']') {
                    let end = i + close + 1;
                    let inner = &source[i + 1..end - 1];
                    // Must be alpha/underscore identifier, not numbers (those are RPG Maker)
                    if !inner.is_empty()
                        && inner
                            .chars()
                            .all(|c| c.is_ascii_alphabetic() || c == '_')
                    {
                        matches.push(PatternMatch {
                            start: i,
                            end,
                            text: source[i..end].to_string(),
                            kind: PlaceholderKind::CustomBracket,
                        });
                        i = end;
                        continue;
                    }
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
}
