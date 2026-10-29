use std::collections::HashMap;

use crate::models::{StringEntry, TranslationRequest, TranslationResult};
use crate::placeholder::{
    Placeholder, PlaceholderKind, PlaceholderProcessor, PlaceholderProfile, SpeakerArgumentTrace,
};

pub(super) fn translation_fits_entry(entry: &StringEntry, translation: &str) -> bool {
    if translation.trim().is_empty()
        || !PlaceholderProcessor::validate_with_profile(
            &entry.source,
            translation,
            entry.placeholder_profile(),
        )
        .is_empty()
    {
        return false;
    }
    if crate::textasset_group::is_grouped_entry(entry) {
        return false;
    }
    match crate::validation::binary_slot_budget(entry) {
        Ok(Some((slot, budget))) => {
            return crate::validation::encoded_byte_len(&slot, translation)
                .is_some_and(|size| size <= budget);
        }
        Err(_) => return false,
        Ok(None) => {}
    }
    true
}

/// Reuse the existing strict legacy inference, narrowing its broad RPG Maker
/// profile to MV/MZ extensions. Explicit provenance always wins, even when it
/// is unknown or malformed; a .json path alone is never sufficient.
pub(super) fn has_mv_mz_provenance(entry: &StringEntry) -> bool {
    if let Some(id) = entry.metadata.get(crate::models::FORMAT_ID_METADATA_KEY) {
        return matches!(id.as_str(), Some("rpgmaker-mv" | "rpgmaker-mz"));
    }
    entry.placeholder_profile() == PlaceholderProfile::RpgMaker
        && entry
            .file_path
            .extension()
            .and_then(|ext| ext.to_str())
            .is_some_and(|ext| matches!(ext.to_ascii_lowercase().as_str(), "json" | "jsono" | "js"))
}

#[derive(Clone)]
pub(super) struct RecordProtection {
    pub(super) placeholders: Vec<Placeholder>,
    pub(super) speakers: SpeakerArgumentTrace,
}

/// Longest run of one letter a translation may contain unless its source has
/// a longer run of that letter.
const MAX_LETTER_RUN: usize = 5;

/// Machine translators stretch moans and screams (`はぁぁ～～～～`) into
/// hundreds of repeated letters; one unbreakable word then overflows the
/// game's text window. Cap runs of the same letter at `MAX_LETTER_RUN`,
/// keeping any longer run the source itself spells out.
pub fn collapse_runaway_letters(source: &str, translation: &str) -> String {
    fn letter_runs(text: &str) -> Vec<(char, Vec<char>)> {
        let mut runs: Vec<(char, Vec<char>)> = Vec::new();
        for c in text.chars() {
            let key = c.to_lowercase().next().unwrap_or(c);
            match runs.last_mut() {
                Some((last, run)) if c.is_alphabetic() && *last == key => run.push(c),
                _ => runs.push((key, vec![c])),
            }
        }
        runs
    }

    let runs = letter_runs(translation);
    let letters_capped: String = if runs.iter().all(|(_, run)| run.len() <= MAX_LETTER_RUN) {
        translation.to_string()
    } else {
        let mut source_max: HashMap<char, usize> = HashMap::new();
        for (key, run) in letter_runs(source) {
            let longest = source_max.entry(key).or_default();
            *longest = (*longest).max(run.len());
        }
        runs.into_iter()
            .flat_map(|(key, run)| {
                let keep = if run[0].is_alphabetic() {
                    MAX_LETTER_RUN.max(source_max.get(&key).copied().unwrap_or(0))
                } else {
                    run.len()
                };
                run.into_iter().take(keep)
            })
            .collect()
    };
    collapse_repeated_units(source, &letters_capped)
}

/// The same stretch with separators: `N-n-n-n-...`, `ha-ha-ha-...`. Units of
/// two to four characters containing a letter repeat at most
/// `MAX_LETTER_RUN` times in a row, unless the source repeats that unit more.
fn collapse_repeated_units(source: &str, text: &str) -> String {
    fn lower(text: &str) -> Vec<char> {
        text.chars()
            .map(|c| c.to_lowercase().next().unwrap_or(c))
            .collect()
    }
    fn repeats_at(chars: &[char], start: usize, unit: &[char]) -> usize {
        let mut count = 0;
        while chars
            .get(start + count * unit.len()..start + (count + 1) * unit.len())
            .is_some_and(|window| window == unit)
        {
            count += 1;
        }
        count
    }
    fn longest_repeat(chars: &[char], unit: &[char]) -> usize {
        (0..chars.len())
            .map(|start| repeats_at(chars, start, unit))
            .max()
            .unwrap_or(0)
    }

    let original: Vec<char> = text.chars().collect();
    let folded = lower(text);
    let source_folded = lower(source);
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    'scan: while i < original.len() {
        for len in 2..=4 {
            let Some(unit) = folded.get(i..i + len) else {
                break;
            };
            if !unit.iter().any(|c| c.is_alphabetic()) {
                continue;
            }
            let count = repeats_at(&folded, i, unit);
            if count <= MAX_LETTER_RUN {
                continue;
            }
            let allowed = MAX_LETTER_RUN.max(longest_repeat(&source_folded, unit));
            if count > allowed {
                out.extend(&original[i..i + allowed * len]);
                i += count * len;
                continue 'scan;
            }
        }
        out.push(original[i]);
        i += 1;
    }
    out
}

/// Raw sentinel integrity and restored control integrity are separate gates.
/// Never manufacture missing controls or accept literal controls instead of tokens.
pub(super) fn restore_placeholders_in_result(
    result: &mut TranslationResult,
    placeholders_by_id: &HashMap<String, RecordProtection>,
) -> bool {
    let Some(protection) = placeholders_by_id.get(&result.entry_id) else {
        return false;
    };
    let phs = &protection.placeholders;
    // Check unknown/malformed spellings in the raw answer beyond restore's
    // bounded numeric scan. Protected originals can themselves contain this
    // spelling, so testing the restored text would reject legitimate controls.
    if result.translation.match_indices("{PL_").any(|(offset, _)| {
        !phs.iter()
            .any(|ph| result.translation[offset..].starts_with(&ph.token))
    }) {
        return false;
    }
    match PlaceholderProcessor::restore_with_speaker_trace(
        &result.translation,
        phs,
        &protection.speakers,
    ) {
        Ok(restored) => {
            result.translation = restored;
            true
        }
        Err(_) => false,
    }
}

/// Apply the existing body runaway cap without rewriting explicit canonical
/// values. Matching targets are restored after the cap, never target-searched.
pub(super) fn restore_and_collapse_result(
    result: &mut TranslationResult,
    placeholders: &HashMap<String, RecordProtection>,
    source: &str,
) -> bool {
    let canonical = placeholders
        .get(&result.entry_id)
        .is_some_and(|protection| {
            protection
                .placeholders
                .iter()
                .any(|ph| matches!(ph.pattern_type, PlaceholderKind::GlossaryTerm))
        });
    if canonical {
        result.translation = collapse_runaway_letters(source, &result.translation);
    }
    let restored = restore_placeholders_in_result(result, placeholders);
    if !canonical {
        result.translation = collapse_runaway_letters(source, &result.translation);
    }
    restored
}

pub(super) fn protected_control_inventory(phs: &[Placeholder]) -> String {
    let tokens = phs
        .iter()
        .map(|ph| ph.token.as_str())
        .collect::<Vec<_>>()
        .join(" ");
    format!(
        "EXPECTED SENTINELS: {tokens}. Preserve each exactly once; output no additional sentinel or control tokens."
    )
}

pub(super) fn protected_control_repair_request(
    original: &TranslationRequest,
    phs: &[Placeholder],
) -> TranslationRequest {
    let mut request = original.clone();
    let inventory = protected_control_inventory(phs);
    // Keep the original sanitized source and all record-local guidance, with
    // one inventory rather than accumulating copies across logical repairs.
    let context = original.context.as_deref().unwrap_or_default();
    let context = context.strip_suffix(&inventory).unwrap_or(context);
    let correction = "PROTECTED-CONTROL CORRECTION: The previous answer did not preserve this record's protected tokens. Translate the original source again, preserving only its expected sentinels, without adding literal control codes. Return only the requested translation.";
    request.context = Some(format!("{context}{correction} | {inventory}"));
    request
}

/// Semantic repairs are logical calls; transport retries remain call_provider's policy.
pub(super) const MAX_PROTECTED_CONTROL_REPAIRS: usize = 2;

/// One source-changing MT attempt, then the pending-only configured fallback.
/// Measured encodings and the bounded live probe: docs/MT-PROTECTION.md.
pub(super) struct MtControlRepair {
    pub(super) request: TranslationRequest,
    // Alternate -> original opaque identity; never expand controls here. The
    // original speaker trace and two-phase canonical restoration still apply.
    tokens: Vec<(String, String)>,
}

impl MtControlRepair {
    pub(super) fn new(original: &TranslationRequest, phs: &[Placeholder]) -> Option<Self> {
        if phs.is_empty() {
            return None;
        }
        let mut nonce = 0usize;
        loop {
            let prefix = format!("⟦L{nonce}X");
            if !original.source.contains(&prefix)
                && !phs.iter().any(|ph| ph.original.contains(&prefix))
            {
                break;
            }
            nonce += 1;
        }
        let mut spans: Vec<_> = phs
            .iter()
            .enumerate()
            .map(|(index, ph)| {
                original
                    .source
                    .find(&ph.token)
                    .map(|start| (start, ph.token.as_str(), format!("⟦L{nonce}X{index}⟧")))
            })
            .collect::<Option<_>>()?;
        spans.sort_by_key(|(start, _, _)| *start);
        let mut request = original.clone();
        request.source.clear();
        let mut last = 0;
        let mut tokens = Vec::with_capacity(spans.len());
        for (start, base, alternate) in spans {
            request.source.push_str(&original.source[last..start]);
            request.source.push_str(&alternate);
            last = start + base.len();
            tokens.push((alternate, base.to_owned()));
        }
        request.source.push_str(&original.source[last..]);
        Some(Self { request, tokens })
    }

    pub(super) fn decode(&self, translated: &str) -> Option<String> {
        // Unknown/malformed identities and missing/duplicated tokens fail closed.
        // Match the entire alternate family, not just this record's nonce.
        if translated.match_indices("⟦L").any(|(start, _)| {
            !self
                .tokens
                .iter()
                .any(|(token, _)| translated[start..].starts_with(token))
        }) || self
            .tokens
            .iter()
            .any(|(token, _)| translated.matches(token).count() != 1)
        {
            return None;
        }
        let mut spans: Vec<_> = self
            .tokens
            .iter()
            .map(|(token, base)| (translated.find(token).unwrap(), token, base))
            .collect();
        spans.sort_by_key(|(start, _, _)| *start);
        let mut out = String::new();
        let mut last = 0;
        for (start, token, base) in spans {
            out.push_str(&translated[last..start]);
            out.push_str(base);
            last = start + token.len();
        }
        out.push_str(&translated[last..]);
        Some(out)
    }
}

/// Tight UI slots (short source labels) need a harsher first-pass budget hint.
/// Threshold is in **encoded bytes** of the source string.
const TIGHT_BINARY_SLOT_BYTES: usize = 12;

/// Extra provider attempts after an oversize first answer (not counting the
/// original batch call). Two retries help very tight UI labels (e.g. 7–9 byte
/// slots) when the first shortening still misses by 1 byte.
pub(super) const MAX_BINARY_SLOT_LENGTH_RETRIES: usize = 2;

/// First-pass context hint for binary-slot inject budgets.
/// When `source` is set and the budget is tight, quote the source so the model
/// can edit length against the actual label (helps short UI slots).
pub(super) fn binary_slot_length_hint(slot: &str, budget: usize, source: &str) -> String {
    let src_display = if source.chars().count() > 48 {
        let t: String = source.chars().take(48).collect();
        format!("{t}…")
    } else {
        source.to_string()
    };
    let accent_note = if slot == "utf8" && source.is_ascii() && budget <= TIGHT_BINARY_SLOT_BYTES {
        " Prefer ASCII when needed: accented letters cost 2 UTF-8 bytes each (ó/ñ/ü)."
    } else {
        ""
    };
    if budget <= TIGHT_BINARY_SLOT_BYTES {
        format!(
            "LENGTH LIMIT: HARD MAX {budget} bytes ({slot}). Source: «{src_display}». \
             Prefer one short word or heavy abbreviation; spaces and accents count as bytes.\
             {accent_note}"
        )
    } else {
        format!(
            "LENGTH LIMIT: the translation MUST fit in {budget} bytes when encoded as {slot}; \
             abbreviate if needed."
        )
    }
}

/// Length-aware retry correction: include the failed text and exact excess so the
/// model can edit instead of retranslating from scratch.
pub(super) fn binary_slot_retry_correction(
    slot: &str,
    budget: usize,
    first_len: usize,
    previous: &str,
) -> String {
    let excess = first_len.saturating_sub(budget);
    // Cap quoted previous text so a runaway provider answer does not blow context.
    let prev_display = if previous.chars().count() > 80 {
        let truncated: String = previous.chars().take(80).collect();
        format!("{truncated}…")
    } else {
        previous.to_string()
    };
    format!(
        "PREVIOUS ATTEMPT WAS {first_len} BYTES — HARD LIMIT {budget} BYTES ({slot}). \
         Previous text: «{prev_display}». Remove at least {excess} byte(s). \
         Shorten aggressively: drop articles/vowels/spaces, use abbreviations; \
         return ONLY the shortened translation."
    )
}

#[cfg(test)]
mod runaway_letter_tests {
    use super::collapse_runaway_letters;

    #[test]
    fn stretched_moans_are_capped_at_five_letters() {
        let long = format!("¡H{}!", "a".repeat(80));
        assert_eq!(
            collapse_runaway_letters("はぁぁ～～～～！？", &long),
            "¡Haaaaa!"
        );
        assert_eq!(
            collapse_runaway_letters("んぁ～", &format!("\\n<Lydia>M{}...", "m".repeat(300))),
            "\\n<Lydia>Mmmmm..."
        );
    }

    #[test]
    fn ordinary_text_and_short_runs_are_unchanged() {
        for text in [
            "Hola, ¿cómo estás?",
            "Aaaah... ¡Nooooo!",
            "\\C[27]♥\\C[0] Hmmmmm",
            "!!!!!!!!!!!! ??????? ........",
            "――――――――",
            "",
        ] {
            assert_eq!(collapse_runaway_letters("そのまま", text), text);
        }
    }

    #[test]
    fn runs_the_source_already_spells_out_are_kept() {
        let source = "AAAAAAAAAH! The dragon!";
        let translation = "¡AAAAAAAAAH! ¡El dragón!";
        assert_eq!(collapse_runaway_letters(source, translation), translation);
        // Only up to the source's own length, and case-insensitively.
        assert_eq!(
            collapse_runaway_letters("aaaaaaa!", &format!("¡A{}!", "a".repeat(20))),
            "¡Aaaaaaa!"
        );
    }

    #[test]
    fn every_letter_run_is_capped_independently() {
        let translation = format!(
            "U{}hi{}... Aa{}",
            "u".repeat(9),
            "i".repeat(12),
            "a".repeat(40)
        );
        assert_eq!(
            collapse_runaway_letters("うひぃ……あぁ", &translation),
            "Uuuuuhiiiii... Aaaaa"
        );
    }

    #[test]
    fn separated_stutters_are_capped_at_five_repetitions() {
        // Real game: Gemini answered a moan with "N-n-n-..." ninety times.
        let stutter = format!("\\n<Lydia>N{}!", "-n".repeat(90));
        assert_eq!(
            collapse_runaway_letters("\\n<リディア>んゔぃっ！", &stutter),
            "\\n<Lydia>N-n-n-n-n-n!"
        );
        assert_eq!(
            collapse_runaway_letters("あはは", &format!("{}ha!", "ha-".repeat(40))),
            "ha-ha-ha-ha-ha-ha!"
        );
        assert_eq!(
            collapse_runaway_letters("んちゅ", &"chu ".repeat(30)),
            "chu chu chu chu chu "
        );
    }

    #[test]
    fn short_or_source_spelled_patterns_are_kept() {
        for text in [
            "N-n-no, please!",
            "Ha-ha-ha, very funny.",
            "1-2-3-4-5-6-7-8-9",
            "... ... ... ... ... ... ...",
            "Bye bye bye bye",
            "",
        ] {
            assert_eq!(collapse_runaway_letters("そのまま", text), text);
        }
        let source = "la-la-la-la-la-la-la-la song";
        let translation = "canción la-la-la-la-la-la-la-la";
        assert_eq!(collapse_runaway_letters(source, translation), translation);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn format_profile_applies_to_memory_and_glossary_acceptance() {
        for (format, accepted) in [
            ("rpgmaker-mv", true),
            ("wolf-rpg", true),
            ("renpy", false),
            ("unknown", false),
        ] {
            let entry = StringEntry::new("id", r"Hello \V[1]", PathBuf::from("test.json"))
                .with_format_id(format);
            assert_eq!(
                translation_fits_entry(&entry, r"[Noble] Hola \V[1]"),
                accepted,
                "{format}"
            );
            assert!(!translation_fits_entry(&entry, "[Noble] Hola"));
        }
    }

    #[test]
    fn test_binary_slot_length_hint_tight_vs_loose() {
        let tight = binary_slot_length_hint("utf8", 8, "Options");
        assert!(tight.contains("HARD MAX 8 bytes (utf8)"), "{tight}");
        assert!(
            tight.contains("Source: «Options»"),
            "tight hint must quote source: {tight}"
        );
        let loose = binary_slot_length_hint("utf8", 40, "Longer dialogue line here");
        assert!(loose.contains("40 bytes when encoded as utf8"), "{loose}");
        assert!(!loose.contains("HARD MAX"), "{loose}");
        assert!(
            !loose.contains("Source:"),
            "loose budget keeps shorter phrasing: {loose}"
        );
    }

    #[test]
    fn test_binary_slot_retry_correction_quotes_previous() {
        let c = binary_slot_retry_correction("utf8", 7, 8, "Opciones");
        assert!(c.contains("Previous text: «Opciones»"), "{c}");
        assert!(c.contains("Remove at least 1 byte"), "{c}");
        assert!(c.contains("HARD LIMIT 7 BYTES (utf8)"), "{c}");
    }
}
