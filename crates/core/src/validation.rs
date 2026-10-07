use std::collections::HashMap;

use serde::Serialize;

use crate::database::Database;
use crate::error::Result;
use crate::models::{StringEntry, StringStatus, ValidationIssue, ValidationKind};

pub struct Validator;

/// UTF-16 byte length of `text`, with no implicit BOM or terminator.
pub fn utf16_byte_len(text: &str) -> usize {
    text.encode_utf16().count() * 2
}

/// Byte length of `text` under a binary inject encoding.
/// Returns `None` if the text cannot be encoded (e.g. unmappable Shift-JIS).
pub fn encoded_byte_len(encoding: &str, text: &str) -> Option<usize> {
    match encoding {
        "utf8" => Some(text.len()),
        "utf16le" => Some(utf16_byte_len(text)),
        "sjis" | "shift_jis" | "shift-jis" => {
            let (bytes, _, had_errors) = encoding_rs::SHIFT_JIS.encode(text);
            if had_errors {
                None
            } else {
                Some(bytes.len())
            }
        }
        _ => None,
    }
}

/// Effective per-entry binary capacity. A pivot records an explicit immutable
/// value; older/non-pivot rows derive it from the physical injection source.
/// Grouped Unity locale/CSV rows intentionally have no per-cell capacity
/// because injection validates and pads their rebuilt blob as one unit.
/// Whole-blob capacity lives in `textasset_group_*` metadata, never here.
pub fn binary_slot_budget(entry: &StringEntry) -> Result<Option<(String, usize)>> {
    let source = entry
        .injection_source()
        .map_err(|message| crate::error::LocustError::Other(anyhow::anyhow!(message)))?;
    let grouped = crate::textasset_group::is_grouped_entry(entry);
    let explicit_capacity = entry
        .injection_capacity()
        .map_err(|message| crate::error::LocustError::Other(anyhow::anyhow!(message)))?;
    if grouped {
        if let Some((encoding, bytes)) = explicit_capacity {
            if bytes == 0 || encoded_byte_len(encoding, "").is_none() {
                return Err(crate::error::LocustError::Other(anyhow::anyhow!(
                    "entry '{}' has invalid immutable binary capacity",
                    entry.id
                )));
            }
            if entry
                .metadata
                .get("binary_slot")
                .and_then(|v| v.as_str())
                .is_some_and(|slot| slot != encoding)
            {
                return Err(crate::error::LocustError::Other(anyhow::anyhow!(
                    "entry '{}' has conflicting binary slot encodings",
                    entry.id
                )));
            }
        }
        return Ok(None);
    }
    if let Some((encoding, bytes)) = explicit_capacity {
        if bytes == 0 || encoded_byte_len(encoding, "").is_none() {
            return Err(crate::error::LocustError::Other(anyhow::anyhow!(
                "entry '{}' has invalid immutable binary capacity",
                entry.id
            )));
        }
        if entry
            .metadata
            .get("binary_slot")
            .and_then(|v| v.as_str())
            .is_some_and(|slot| slot != encoding)
        {
            return Err(crate::error::LocustError::Other(anyhow::anyhow!(
                "entry '{}' has conflicting binary slot encodings",
                entry.id
            )));
        }
        if encoded_byte_len(encoding, source) != Some(bytes) {
            return Err(crate::error::LocustError::Other(anyhow::anyhow!(
                "entry '{}' has binary capacity inconsistent with its physical source",
                entry.id
            )));
        }
        let capacity = if encoding == "utf8" {
            crate::textasset_group::structural_textasset_capacity(entry).unwrap_or(bytes)
        } else {
            bytes
        };
        return Ok(Some((encoding.to_string(), capacity)));
    }
    if let Some(capacity) = crate::textasset_group::structural_textasset_capacity(entry) {
        return Ok(Some(("utf8".to_owned(), capacity)));
    }
    let Some(encoding) = entry.metadata.get("binary_slot").and_then(|v| v.as_str()) else {
        return Ok(None);
    };
    let bytes = encoded_byte_len(encoding, source).ok_or_else(|| {
        crate::error::LocustError::Other(anyhow::anyhow!(
            "entry '{}' binary source cannot be encoded as {encoding}",
            entry.id
        ))
    })?;
    Ok(Some((encoding.to_string(), bytes)))
}

/// Issues where translation exceeds a tagged binary inject slot
/// (`metadata.binary_slot` = utf8 / utf16le / sjis). Shared by CLI inject
/// preflight and [`crate::extraction::MultiLangInjector`] so every inject seam
/// surfaces the same problem.
pub fn binary_slot_oversize_issues(entries: &[StringEntry]) -> Vec<ValidationIssue> {
    Validator::validate_all(entries)
        .into_iter()
        .filter(|i| matches!(i.kind, ValidationKind::ExceedsBinarySlot { .. }))
        .collect()
}

pub fn count_binary_slot_oversize(entries: &[StringEntry]) -> usize {
    binary_slot_oversize_issues(entries).len()
}

/// Deterministic last-resort shrink after provider length-retries still oversize.
/// Tries (in order of preference for content): already-fits, accent-fold, despace,
/// multi-word first/initials, vowel-compress, then encoding-aware truncate.
/// Returns `Some` only when the result fits `budget` and still has at least one
/// alphanumeric character.
///
/// Aimed at tight UI labels (e.g. EN→ES `Opciones` on a 7-byte utf8 slot → fold
/// accents / drop spaces / drop inner vowels / truncate) so inject skips fewer slots.
///
/// Preference: longest **non-truncated** fit first (vowel-compress beats mid-word
/// chop: `Opciones`→`Opcns` over `Opcione`; multi-word `Cargar Juego`@6 → `Cargar`);
/// only then longest truncated fit.
pub fn mechanical_fit_binary_slot(encoding: &str, budget: usize, text: &str) -> Option<String> {
    if budget == 0 || text.is_empty() {
        return None;
    }
    if encoded_byte_len(encoding, text)
        .map(|n| n <= budget)
        .unwrap_or(false)
    {
        return Some(text.to_string());
    }

    let mut soft: Vec<String> = Vec::new();
    let push_unique = |v: &mut Vec<String>, s: String| {
        if !s.is_empty() && !v.iter().any(|x| x == &s) {
            v.push(s);
        }
    };

    let trimmed = text.trim();
    push_unique(&mut soft, trimmed.to_string());
    let folded = fold_latin_accents(trimmed);
    push_unique(&mut soft, folded.clone());
    let despaced: String = trimmed.chars().filter(|c| !c.is_whitespace()).collect();
    push_unique(&mut soft, despaced);
    let fold_despaced: String = folded.chars().filter(|c| !c.is_whitespace()).collect();
    push_unique(&mut soft, fold_despaced);

    // Multi-word UI labels: first word, first+initials, pure initials.
    // e.g. "Cargar Juego"@6 → "Cargar"; "Nueva Partida"@7 → "Nueva P".
    for base in [trimmed, folded.as_str()] {
        for cand in multiword_soft_candidates(base) {
            push_unique(&mut soft, cand);
        }
    }

    // Inner-vowel drop on each soft base (Latin UI abbreviation style).
    let soft_bases: Vec<String> = soft.clone();
    for c in &soft_bases {
        push_unique(&mut soft, drop_inner_vowels(c));
        let d: String = drop_inner_vowels(c)
            .chars()
            .filter(|ch| !ch.is_whitespace())
            .collect();
        push_unique(&mut soft, d);
    }

    if let Some(best) = pick_longest_fit(encoding, budget, &soft) {
        return Some(best);
    }

    // Last resort: truncate soft candidates on encoding boundaries.
    let mut hard: Vec<String> = Vec::new();
    for c in &soft {
        if let Some(t) = truncate_to_encoded_budget(encoding, budget, c) {
            push_unique(&mut hard, t);
        }
    }
    pick_longest_fit(encoding, budget, &hard)
}

/// Soft shrink candidates for multi-word UI (`Cargar Juego`, `Nueva Partida`).
/// Returns first word, `First I…` (rest initials, spaced), `FirstI…` (no space),
/// and pure initials (`NP`).
fn multiword_soft_candidates(s: &str) -> Vec<String> {
    let words: Vec<&str> = s.split_whitespace().filter(|w| !w.is_empty()).collect();
    if words.len() < 2 {
        return Vec::new();
    }
    let mut out = Vec::new();
    out.push(words[0].to_string());

    let mut initials: Vec<char> = Vec::new();
    for w in &words {
        if let Some(c) = w.chars().find(|ch| ch.is_alphabetic()) {
            initials.push(c);
        }
    }
    if initials.len() >= 2 {
        out.push(initials.iter().collect::<String>());
    }

    // First word + first letter of each subsequent word (spaced and joined).
    if words.len() >= 2 {
        let mut spaced = words[0].to_string();
        let mut joined = words[0].to_string();
        for w in &words[1..] {
            if let Some(c) = w.chars().find(|ch| ch.is_alphabetic()) {
                spaced.push(' ');
                spaced.push(c);
                joined.push(c);
            }
        }
        if spaced != words[0] {
            out.push(spaced);
        }
        if joined != words[0] {
            out.push(joined);
        }
    }
    out
}

/// Prefer the longest candidate that encodes within `budget` and keeps alnum.
fn pick_longest_fit(encoding: &str, budget: usize, candidates: &[String]) -> Option<String> {
    let mut best: Option<(usize, String)> = None;
    for c in candidates {
        if !c.chars().any(|ch| ch.is_alphanumeric()) {
            continue;
        }
        let Some(n) = encoded_byte_len(encoding, c) else {
            continue;
        };
        if n > budget {
            continue;
        }
        match &best {
            Some((bn, _)) if n <= *bn => {}
            _ => best = Some((n, c.clone())),
        }
    }
    best.map(|(_, s)| s)
}

/// Keep the first letter of each word; drop subsequent Latin vowels (a e i o u).
/// `Opciones` → `Opcns`, `Nueva Partida` → `Nv Prtd`.
fn drop_inner_vowels(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut at_word_start = true;
    for c in s.chars() {
        if c.is_whitespace() {
            out.push(c);
            at_word_start = true;
            continue;
        }
        if !c.is_alphabetic() {
            out.push(c);
            // Digits / punctuation start a new "word" for the next letter.
            at_word_start = true;
            continue;
        }
        let is_vowel = matches!(c.to_ascii_lowercase(), 'a' | 'e' | 'i' | 'o' | 'u');
        if at_word_start || !is_vowel {
            out.push(c);
        }
        at_word_start = false;
    }
    out
}

/// Map common Latin accented letters to ASCII (Spanish/French/German UI).
fn fold_latin_accents(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        let r = match c {
            'á' | 'à' | 'ä' | 'â' | 'ã' | 'å' | 'ā' => 'a',
            'Á' | 'À' | 'Ä' | 'Â' | 'Ã' | 'Å' | 'Ā' => 'A',
            'é' | 'è' | 'ë' | 'ê' | 'ē' => 'e',
            'É' | 'È' | 'Ë' | 'Ê' | 'Ē' => 'E',
            'í' | 'ì' | 'ï' | 'î' | 'ī' => 'i',
            'Í' | 'Ì' | 'Ï' | 'Î' | 'Ī' => 'I',
            'ó' | 'ò' | 'ö' | 'ô' | 'õ' | 'ø' | 'ō' => 'o',
            'Ó' | 'Ò' | 'Ö' | 'Ô' | 'Õ' | 'Ø' | 'Ō' => 'O',
            'ú' | 'ù' | 'ü' | 'û' | 'ū' => 'u',
            'Ú' | 'Ù' | 'Ü' | 'Û' | 'Ū' => 'U',
            'ý' | 'ÿ' => 'y',
            'Ý' => 'Y',
            'ñ' => 'n',
            'Ñ' => 'N',
            'ç' => 'c',
            'Ç' => 'C',
            'ß' => {
                out.push('s');
                out.push('s');
                continue;
            }
            other => other,
        };
        out.push(r);
    }
    out
}

/// Truncate `text` so encoded length ≤ `budget` (char-boundary for utf8;
/// UTF-16 code-unit pairs for utf16le; byte-greedy re-encode check for sjis).
fn truncate_to_encoded_budget(encoding: &str, budget: usize, text: &str) -> Option<String> {
    if budget == 0 || text.is_empty() {
        return None;
    }
    if encoded_byte_len(encoding, text)
        .map(|n| n <= budget)
        .unwrap_or(false)
    {
        return Some(text.to_string());
    }
    match encoding {
        "utf8" => {
            let mut end = budget.min(text.len());
            while end > 0 && !text.is_char_boundary(end) {
                end -= 1;
            }
            if end == 0 {
                return None;
            }
            let s = text[..end].to_string();
            if s.is_empty() {
                None
            } else {
                Some(s)
            }
        }
        "utf16le" => {
            // budget is in bytes; each code unit is 2 bytes.
            let max_units = budget / 2;
            if max_units == 0 {
                return None;
            }
            let units: Vec<u16> = text.encode_utf16().take(max_units).collect();
            // Drop trailing unpaired high surrogate.
            let mut units = units;
            if let Some(&last) = units.last() {
                if (0xD800..=0xDBFF).contains(&last) {
                    units.pop();
                }
            }
            if units.is_empty() {
                return None;
            }
            String::from_utf16(&units).ok()
        }
        "sjis" | "shift_jis" | "shift-jis" => {
            // Walk chars; keep prefix while SJIS encoding fits.
            let mut out = String::new();
            for ch in text.chars() {
                let mut trial = out.clone();
                trial.push(ch);
                match encoded_byte_len(encoding, &trial) {
                    Some(n) if n <= budget => out = trial,
                    _ => break,
                }
            }
            if out.is_empty() {
                None
            } else {
                Some(out)
            }
        }
        _ => None,
    }
}

impl Validator {
    pub fn validate_entry(entry: &StringEntry) -> Vec<ValidationIssue> {
        let mut issues = Vec::new();
        if let Err(message) = entry.require_current_translation() {
            issues.push(ValidationIssue {
                entry_id: entry.id.clone(),
                kind: ValidationKind::StaleTranslation,
                message,
                source: None,
            });
        }
        let translation = entry.translation.as_deref().unwrap_or("");
        let slot_budget = binary_slot_budget(entry);
        if let Err(error) = &slot_budget {
            issues.push(ValidationIssue {
                entry_id: entry.id.clone(),
                kind: ValidationKind::InvalidInjectionProvenance,
                message: error.to_string(),
                source: None,
            });
        }
        if let Err(error) = crate::textasset_group::require_valid_metadata(entry) {
            issues.push(ValidationIssue {
                entry_id: entry.id.clone(),
                kind: ValidationKind::InvalidInjectionProvenance,
                message: error.to_string(),
                source: None,
            });
        }

        // Check 1 — EmptyTranslation
        if translation.trim().is_empty() && entry.status == StringStatus::Translated {
            issues.push(ValidationIssue {
                entry_id: entry.id.clone(),
                kind: ValidationKind::EmptyTranslation,
                message: "translation is empty".to_string(),
                source: None,
            });
        }

        // Check 2 — IdenticalToSource
        if !translation.is_empty() && translation.trim() == entry.source.trim() {
            issues.push(ValidationIssue {
                entry_id: entry.id.clone(),
                kind: ValidationKind::IdenticalToSource,
                message: "translation is identical to source".to_string(),
                source: None,
            });
        }

        // Check 3 — ExceedsCharLimit
        if let Some(limit) = entry.char_limit {
            let actual = translation.chars().count();
            if actual > limit {
                issues.push(ValidationIssue {
                    entry_id: entry.id.clone(),
                    kind: ValidationKind::ExceedsCharLimit { limit, actual },
                    message: format!("translation exceeds char limit: {} > {}", actual, limit),
                    source: None,
                });
            }
        }

        // Check 4 — Controls must agree with semantic AND physical sources.
        match entry.translation_control_mismatches() {
            Ok(mismatches) => {
                for m in mismatches {
                    let kind = match m.kind {
                        crate::placeholder::MismatchKind::Missing
                        | crate::placeholder::MismatchKind::Unbalanced => {
                            ValidationKind::MissingPlaceholder {
                                placeholder: m.placeholder.clone(),
                            }
                        }
                        crate::placeholder::MismatchKind::Extra => {
                            ValidationKind::ExtraPlaceholder {
                                placeholder: m.placeholder.clone(),
                            }
                        }
                    };
                    let message = if m.kind == crate::placeholder::MismatchKind::Unbalanced {
                        format!("unbalanced control structure: {}", m.placeholder)
                    } else {
                        format!(
                            "placeholder mismatch against semantic or physical source: {}",
                            m.placeholder
                        )
                    };
                    issues.push(ValidationIssue {
                        entry_id: entry.id.clone(),
                        kind,
                        message,
                        source: None,
                    });
                }
            }
            Err(message) => issues.push(ValidationIssue {
                entry_id: entry.id.clone(),
                kind: ValidationKind::InvalidInjectionProvenance,
                message,
                source: None,
            }),
        }

        // Check 5 — Binary inject slot (Unity UTF-8 / Unreal UTF-16LE / Wolf SJIS)
        if !translation.is_empty() {
            if let Ok(Some((enc, src_len))) = slot_budget {
                if let Some(tr_len) = encoded_byte_len(&enc, translation) {
                    if tr_len > src_len {
                        issues.push(ValidationIssue {
                            entry_id: entry.id.clone(),
                            kind: ValidationKind::ExceedsBinarySlot {
                                encoding: enc.clone(),
                                limit: src_len,
                                actual: tr_len,
                            },
                            message: format!(
                                "translation exceeds binary inject slot ({enc}): {tr_len} > {src_len} bytes"
                            ),
                            source: None,
                        });
                    }
                }
            }
        }

        issues
    }

    pub fn validate_all(entries: &[StringEntry]) -> Vec<ValidationIssue> {
        let mut issues: Vec<ValidationIssue> =
            entries.iter().flat_map(Self::validate_entry).collect();
        issues.extend(crate::textasset_group::group_validation_issues(entries));
        issues
    }

    pub async fn validate_and_save(
        entries: &[StringEntry],
        db: &Database,
    ) -> Result<ValidationReport> {
        let mut issues = Self::validate_all(entries);

        db.save_validation_issues(&issues).await?;

        // Attach source snippets for UI (not stored in the validation table).
        let source_by_id: HashMap<&str, &str> = entries
            .iter()
            .map(|e| (e.id.as_str(), e.source.as_str()))
            .collect();
        for issue in &mut issues {
            if let Some(src) = source_by_id.get(issue.entry_id.as_str()) {
                issue.source = Some(truncate_snippet(src, 100));
            }
        }

        let mut entries_with_issues = std::collections::HashSet::new();
        let mut by_kind: HashMap<String, usize> = HashMap::new();
        for issue in &issues {
            entries_with_issues.insert(&issue.entry_id);
            let kind_name = match &issue.kind {
                ValidationKind::MissingPlaceholder { .. } => "MissingPlaceholder",
                ValidationKind::ExtraPlaceholder { .. } => "ExtraPlaceholder",
                ValidationKind::ExceedsCharLimit { .. } => "ExceedsCharLimit",
                ValidationKind::ExceedsBinarySlot { .. } => "ExceedsBinarySlot",
                ValidationKind::EmptyTranslation => "EmptyTranslation",
                ValidationKind::IdenticalToSource => "IdenticalToSource",
                ValidationKind::InvalidInjectionProvenance => "InvalidInjectionProvenance",
                ValidationKind::StaleTranslation => "StaleTranslation",
            };
            *by_kind.entry(kind_name.to_string()).or_insert(0) += 1;
        }

        Ok(ValidationReport {
            total_checked: entries.len(),
            issues_found: issues.len(),
            entries_with_issues: entries_with_issues.len(),
            by_kind,
            issues,
        })
    }
}

fn truncate_snippet(s: &str, max_chars: usize) -> String {
    let count = s.chars().count();
    if count <= max_chars {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(max_chars).collect();
        out.push('…');
        out
    }
}

#[derive(Debug, Serialize)]
pub struct ValidationReport {
    pub total_checked: usize,
    pub issues_found: usize,
    pub entries_with_issues: usize,
    pub by_kind: HashMap<String, usize>,
    /// Per-entry issues with optional source snippets for the desktop UI.
    pub issues: Vec<ValidationIssue>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::Arc;

    fn make_entry(id: &str, source: &str, translation: Option<&str>) -> StringEntry {
        let mut e = StringEntry::new(id, source, PathBuf::from("test.json"));
        if let Some(t) = translation {
            e.translation = Some(t.to_string());
            e.status = StringStatus::Translated;
        }
        e
    }

    #[test]
    fn test_validate_empty_translation() {
        let entry = make_entry("e1", "Hello", Some(""));
        let issues = Validator::validate_entry(&entry);
        assert!(issues
            .iter()
            .any(|i| matches!(i.kind, ValidationKind::EmptyTranslation)));
    }

    #[test]
    fn test_validate_identical_to_source() {
        let entry = make_entry("e1", "Hello", Some("Hello"));
        let issues = Validator::validate_entry(&entry);
        assert!(issues
            .iter()
            .any(|i| matches!(i.kind, ValidationKind::IdenticalToSource)));
    }

    #[test]
    fn test_validate_exceeds_char_limit() {
        let mut entry = make_entry("e1", "Hi", Some("This is a long translation"));
        entry.char_limit = Some(10);
        let issues = Validator::validate_entry(&entry);
        assert!(issues
            .iter()
            .any(|i| matches!(i.kind, ValidationKind::ExceedsCharLimit { .. })));
    }

    #[test]
    fn test_validate_missing_placeholder() {
        let entry = make_entry("e1", r"\c[2]Hello", Some("Hola"));
        let issues = Validator::validate_entry(&entry);
        assert!(issues
            .iter()
            .any(|i| matches!(i.kind, ValidationKind::MissingPlaceholder { .. })));
    }

    #[test]
    fn test_validate_clean_entry() {
        let entry = make_entry("e1", "Hello", Some("Hola"));
        let issues = Validator::validate_entry(&entry);
        assert!(issues.is_empty());
    }

    #[test]
    fn test_validate_exceeds_binary_slot_utf8() {
        let mut entry = make_entry("e1", "Hi", Some("Hola amigos"));
        entry.metadata.insert(
            "binary_slot".to_string(),
            serde_json::Value::String("utf8".to_string()),
        );
        let issues = Validator::validate_entry(&entry);
        assert!(
            issues
                .iter()
                .any(|i| matches!(i.kind, ValidationKind::ExceedsBinarySlot { .. })),
            "expected ExceedsBinarySlot, got {issues:?}"
        );
    }

    #[test]
    fn test_validate_binary_slot_utf16_ok_when_fits() {
        // Same char count → same UTF-16LE length for BMP.
        let mut entry = make_entry("e1", "Hero", Some("oreH"));
        entry.metadata.insert(
            "binary_slot".to_string(),
            serde_json::Value::String("utf16le".to_string()),
        );
        let issues = Validator::validate_entry(&entry);
        assert!(
            !issues
                .iter()
                .any(|i| matches!(i.kind, ValidationKind::ExceedsBinarySlot { .. })),
            "unexpected binary slot issues: {issues:?}"
        );
    }

    #[test]
    fn pivot_capacity_remains_anchored_to_physical_source() {
        let mut entry = make_entry("e1", "English", Some("Ten bytes!"));
        entry.metadata.insert(
            crate::models::INJECTION_SOURCE_METADATA_KEY.into(),
            serde_json::json!("日本語文"),
        );
        entry.metadata.insert(
            crate::models::INJECTION_CAPACITY_METADATA_KEY.into(),
            serde_json::json!({"encoding": "utf8", "bytes": 12}),
        );
        entry
            .metadata
            .insert("binary_slot".into(), serde_json::json!("utf8"));
        assert_eq!(
            binary_slot_budget(&entry).unwrap(),
            Some(("utf8".into(), 12))
        );
        assert!(!Validator::validate_entry(&entry)
            .iter()
            .any(|i| matches!(i.kind, ValidationKind::ExceedsBinarySlot { .. })));
    }

    #[test]
    fn grouped_unity_rows_have_no_phantom_cell_budget() {
        let original = "ID,NAME\n1,A\n2,B\n                    ";
        let mut entry = make_entry("cell", "A", Some("Longer localized cell"));
        entry
            .metadata
            .insert("binary_slot".into(), serde_json::json!("utf8"));
        entry.metadata.insert(
            "extraction_method".into(),
            serde_json::json!("textasset_csv_cell"),
        );
        entry
            .metadata
            .insert("csv_row".into(), serde_json::json!(1));
        entry
            .metadata
            .insert("csv_col".into(), serde_json::json!(1));
        entry
            .metadata
            .insert("csv_header".into(), serde_json::json!("NAME"));
        crate::textasset_group::attach_to_entries(
            std::slice::from_mut(&mut entry),
            crate::textasset_group::GroupKind::Csv,
            original,
            original.len(),
            "g-csv-budget",
        );
        assert_eq!(binary_slot_budget(&entry).unwrap(), None);
        assert!(!Validator::validate_entry(&entry).iter().any(|i| matches!(
            i.kind,
            ValidationKind::ExceedsBinarySlot { .. } | ValidationKind::InvalidInjectionProvenance
        )));
    }

    #[test]
    fn grouped_textasset_validate_all_flags_shared_oversize() {
        let original = "Menu.A: Hi\nMenu.B: Go\n";
        let mut a = make_entry("a", "Hi", Some("Hola!!!!"));
        let mut b = make_entry("b", "Go", Some("Vamos"));
        for (entry, key, index) in [(&mut a, "Menu.A", 0usize), (&mut b, "Menu.B", 1usize)] {
            entry.metadata.insert(
                "extraction_method".into(),
                serde_json::json!("textasset_loc_line"),
            );
            entry
                .metadata
                .insert("loc_key".into(), serde_json::json!(key));
            entry
                .metadata
                .insert("line_index".into(), serde_json::json!(index));
            entry
                .metadata
                .insert("binary_slot".into(), serde_json::json!("utf8"));
        }
        let mut members = vec![a, b];
        crate::textasset_group::attach_to_entries(
            &mut members,
            crate::textasset_group::GroupKind::LocLine,
            original,
            original.len(),
            "g-oversize",
        );
        let issues = Validator::validate_all(&members);
        assert!(
            issues
                .iter()
                .any(|i| matches!(i.kind, ValidationKind::ExceedsBinarySlot { .. })),
            "{issues:?}"
        );
    }

    #[test]
    fn grouped_malformed_metadata_is_provenance_error() {
        let mut entry = make_entry("cell", "A", Some("Bee"));
        entry.metadata.insert(
            "extraction_method".into(),
            serde_json::json!("textasset_loc_line"),
        );
        assert!(binary_slot_budget(&entry).unwrap().is_none());
        assert!(Validator::validate_entry(&entry)
            .iter()
            .any(|i| matches!(i.kind, ValidationKind::InvalidInjectionProvenance)));
    }

    #[test]
    fn test_encoded_byte_len_utf16le_literal_lengths() {
        let cases = [
            ("", 0),
            ("A", 2),
            ("漢", 2),
            ("😀", 4),
            ("e\u{301}", 4),
            ("\0", 2),
            ("\r\n", 4),
            ("\u{feff}", 2),
            ("A漢😀e\u{301}\0\r\n\u{feff}", 20),
        ];
        for (text, expected) in cases {
            assert_eq!(
                encoded_byte_len("utf16le", text),
                Some(expected),
                "{text:?}"
            );
            assert_eq!(utf16_byte_len(text), expected, "{text:?}");
        }
    }

    #[test]
    fn test_encoded_byte_len_sjis() {
        let n = encoded_byte_len("sjis", "テスト").unwrap();
        assert_eq!(n, 6); // 3 CJK × 2 SJIS
        assert!(encoded_byte_len("utf16le", "テスト").unwrap() == 6);
        assert!(encoded_byte_len("utf8", "テスト").unwrap() == 9);
    }

    #[test]
    fn malformed_pivot_provenance_is_reported_even_without_binary_slots() {
        let mut entry = make_entry("html", "English", Some("Español"));
        entry.metadata.insert(
            crate::models::INJECTION_SOURCE_METADATA_KEY.into(),
            serde_json::json!(42),
        );
        assert!(binary_slot_budget(&entry).is_err());
        assert!(Validator::validate_entry(&entry)
            .iter()
            .any(|issue| matches!(issue.kind, ValidationKind::InvalidInjectionProvenance)));

        entry.metadata.insert(
            crate::models::INJECTION_SOURCE_METADATA_KEY.into(),
            serde_json::json!("日本語文"),
        );
        entry.metadata.insert(
            crate::models::INJECTION_CAPACITY_METADATA_KEY.into(),
            serde_json::json!({"encoding":"utf8", "bytes":100}),
        );
        assert!(binary_slot_budget(&entry).is_err());
    }

    #[test]
    fn test_mechanical_fit_folds_accents_for_utf8_ui_slot() {
        // "Opciones" is 8 ASCII bytes; with accent "Opciónes" style:
        // "Nueva" fits; "Canción" (8 utf8: C-a-n-c-i-ó(2)-n = 8) on budget 7 → "Cancion" (7).
        let src = "Canción"; // C a n c i ó n = 1*5 + 2 + 1 = 8
        assert_eq!(encoded_byte_len("utf8", src).unwrap(), 8);
        let fit = mechanical_fit_binary_slot("utf8", 7, src).unwrap();
        assert!(encoded_byte_len("utf8", &fit).unwrap() <= 7, "fit={fit:?}");
        assert_eq!(fit, "Cancion");
    }

    #[test]
    fn test_mechanical_fit_despaces_and_truncates() {
        // "New Game" budget 7: despace "NewGame" is 7 — fits.
        let fit = mechanical_fit_binary_slot("utf8", 7, "New Game").unwrap();
        assert_eq!(fit, "NewGame");
        // Truncate when still over.
        let fit2 = mechanical_fit_binary_slot("utf8", 4, "Options").unwrap();
        assert_eq!(fit2, "Opti");
        assert!(encoded_byte_len("utf8", &fit2).unwrap() <= 4);
    }

    #[test]
    fn test_mechanical_fit_prefers_vowel_compress_over_midword_truncate() {
        // ES UI: "Opciones" (8) on 7-byte utf8 slot.
        // Naive truncate → "Opcione"; inner-vowel drop → "Opcns" (matches real ES E2E style).
        assert_eq!(encoded_byte_len("utf8", "Opciones").unwrap(), 8);
        let fit = mechanical_fit_binary_slot("utf8", 7, "Opciones").unwrap();
        assert!(encoded_byte_len("utf8", &fit).unwrap() <= 7, "fit={fit:?}");
        assert_eq!(fit, "Opcns");
        // Accented: drop_inner_vowels keeps non-ASCII vowels (ó) so "Canción"→"Cncón"
        // (6 utf8 bytes) fits budget 6 without mid-word chop; preferred over shorter
        // fold+compress "Cncn".
        let fit2 = mechanical_fit_binary_slot("utf8", 6, "Canción").unwrap();
        assert!(
            encoded_byte_len("utf8", &fit2).unwrap() <= 6,
            "fit2={fit2:?}"
        );
        assert_eq!(fit2, "Cncón");
    }

    #[test]
    fn test_mechanical_fit_multiword_prefers_first_word_over_truncate() {
        // ES "Cargar Juego" (12) on 6-byte slot: first word "Cargar" fits cleanly.
        // Without multi-word soft candidates, despace+vowel/"CargarJuego" truncates mid-token.
        assert_eq!(encoded_byte_len("utf8", "Cargar Juego").unwrap(), 12);
        let fit = mechanical_fit_binary_slot("utf8", 6, "Cargar Juego").unwrap();
        assert_eq!(fit, "Cargar");
        assert!(encoded_byte_len("utf8", &fit).unwrap() <= 6);

        // "Nueva Partida" (13) budget 7 → "Nueva P" (7) beats bare "Nueva" (5).
        let fit2 = mechanical_fit_binary_slot("utf8", 7, "Nueva Partida").unwrap();
        assert_eq!(fit2, "Nueva P");
        assert!(encoded_byte_len("utf8", &fit2).unwrap() <= 7);

        // Tiny budget: initials only.
        let fit3 = mechanical_fit_binary_slot("utf8", 2, "Nueva Partida").unwrap();
        assert_eq!(fit3, "NP");
    }

    #[test]
    fn test_mechanical_fit_utf16le_truncate() {
        // 4 BMP chars = 8 bytes; budget 6 → 3 code units.
        let fit = mechanical_fit_binary_slot("utf16le", 6, "ABCD").unwrap();
        assert_eq!(fit, "ABC");
        assert_eq!(encoded_byte_len("utf16le", &fit).unwrap(), 6);
    }

    #[test]
    fn test_count_binary_slot_oversize() {
        let mut over = make_entry("e1", "Hi", Some("Hola amigos"));
        over.metadata.insert(
            "binary_slot".to_string(),
            serde_json::Value::String("utf8".to_string()),
        );
        let mut ok = make_entry("e2", "Hello", Some("Hola!"));
        ok.metadata.insert(
            "binary_slot".to_string(),
            serde_json::Value::String("utf8".to_string()),
        );
        assert_eq!(count_binary_slot_oversize(&[over, ok]), 1);
    }

    #[test]
    fn test_validate_all_aggregates() {
        let entries = vec![
            make_entry("e1", "Hello", Some("")),      // EmptyTranslation
            make_entry("e2", "World", Some("World")), // IdenticalToSource
            {
                let mut e = make_entry("e3", "Hi", Some("Very long translation here!"));
                e.char_limit = Some(5);
                e
            }, // ExceedsCharLimit
        ];
        let issues = Validator::validate_all(&entries);
        assert_eq!(issues.len(), 3);
    }

    #[tokio::test]
    async fn test_validation_report_counts() {
        let db = Arc::new(Database::open_in_memory().unwrap());
        let entries = vec![
            make_entry("e1", "Hello", Some("")),
            make_entry("e2", "World", Some("Mundo")),
        ];
        db.save_entries(&entries).unwrap();

        let report = Validator::validate_and_save(&entries, &db).await.unwrap();

        assert_eq!(report.total_checked, 2);
        assert_eq!(report.issues_found, 1);
        assert_eq!(report.entries_with_issues, 1);
    }
    #[test]
    fn control_repro_validation_checks_physical_source_even_when_english_lost_control() {
        let mut entry = make_entry("line", "Hello", Some("Hola"));
        entry.metadata.insert(
            crate::models::INJECTION_SOURCE_METADATA_KEY.into(),
            serde_json::json!("JA {name}"),
        );
        assert!(Validator::validate_entry(&entry)
            .iter()
            .any(|i| matches!(i.kind, ValidationKind::MissingPlaceholder { .. })));
        entry.translation = Some(String::new());
        assert!(Validator::validate_entry(&entry)
            .iter()
            .any(|i| matches!(i.kind, ValidationKind::MissingPlaceholder { .. })));
    }
}
