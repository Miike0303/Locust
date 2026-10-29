use crate::database::{Database, GlossaryEntry};
use crate::error::Result;
use std::collections::{HashMap, HashSet, VecDeque};
#[cfg(test)]
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

/// Run-local trie: language filtering and Unicode case folding happen once, not
/// once per database row. Original byte offsets remain authoritative for edits.
#[derive(Default)]
struct TermNode {
    next: std::collections::HashMap<char, usize>,
    entries: Vec<usize>,
}

pub(crate) struct VisibleGlossary {
    nodes: Vec<TermNode>,
    entries: Vec<GlossaryEntry>,
}

fn word_char(c: char) -> bool {
    // Unicode letters/numbers plus combining-mark blocks and connectors. Other
    // non-ASCII non-delimiters conservatively count as word continuations: an
    // unfamiliar script/mark must not permit an unsafe partial-word match.
    c.is_alphanumeric()
        || matches!(
            c,
            '_' | '\u{200c}' | '\u{200d}' | '\u{203f}' | '\u{2040}' | '\u{2054}' | '\u{ff3f}'
        )
        || (!c.is_ascii()
            && !c.is_whitespace()
            && !matches!(c, '«' | '»' | '¡' | '¿' | '\u{2000}'..='\u{206f}' | '\u{2e00}'..='\u{2e7f}'
                | '\u{3000}'..='\u{303f}' | '\u{ff01}'..='\u{ff0f}'
                | '\u{ff1a}'..='\u{ff20}' | '\u{ff3b}'..='\u{ff40}'
                | '\u{ff5b}'..='\u{ff65}'))
}

impl VisibleGlossary {
    pub(crate) fn compile(entries: &[GlossaryEntry], pair: &str) -> Self {
        let entries: Vec<_> = entries
            .iter()
            .filter(|e| e.lang_pair == pair && !e.term.is_empty())
            .cloned()
            .collect();
        let mut nodes = vec![TermNode::default()];
        for (index, entry) in entries.iter().enumerate() {
            let mut node = 0;
            for c in entry.term.to_lowercase().chars() {
                let next = if let Some(next) = nodes[node].next.get(&c) {
                    *next
                } else {
                    let next = nodes.len();
                    nodes.push(TermNode::default());
                    nodes[node].next.insert(c, next);
                    next
                };
                node = next;
            }
            nodes[node].entries.push(index);
        }
        Self { nodes, entries }
    }

    /// Protect controls first. Each leftmost-longest visible match gets a unique
    /// opaque token; equivalent matches with contradictory targets fail closed.
    pub(crate) fn mask(
        &self,
        source: &str,
        profile: crate::placeholder::PlaceholderProfile,
    ) -> std::result::Result<(String, Vec<crate::placeholder::Placeholder>), ()> {
        use crate::placeholder::{Placeholder, PlaceholderKind, PlaceholderProcessor};
        let (sanitized, mut phs, nameboxes) =
            PlaceholderProcessor::extract_with_namebox_positions(source, profile);
        // Speaker delimiters are syntax, not visible term spans. An explicit
        // term containing '<...>' must not consume a recognized argument's
        // wrapper (or make the private source trace impossible to construct).
        let speaker_delimiters: std::collections::HashSet<_> = nameboxes
            .iter()
            .flat_map(|(offset, prefix)| {
                let open = offset + prefix.len();
                std::iter::once(open).chain(
                    sanitized[open + 1..]
                        .find('>')
                        .map(|close| open + 1 + close),
                )
            })
            .collect();
        // Extraction emits controls in source order and masks all literal PL
        // lookalikes, so these positions are exactly the minted control spans.
        let opaque: std::collections::HashMap<_, _> = sanitized
            .match_indices("{PL_")
            .zip(phs.iter())
            .map(|((start, _), ph)| {
                debug_assert!(sanitized[start..].starts_with(&ph.token));
                (start, start + ph.token.len())
            })
            .collect();
        let chars: Vec<_> = sanitized.char_indices().collect();
        let mut edits = Vec::new();
        let mut i = 0;
        while i < chars.len() {
            let start = chars[i].0;
            if let Some(end) = opaque.get(&start) {
                while i < chars.len() && chars[i].0 < *end {
                    i += 1;
                }
                continue;
            }
            let mut node = 0;
            let mut best: Option<(usize, &str)> = None;
            let mut conflict = false;
            'walk: for j in i..chars.len() {
                if opaque.contains_key(&chars[j].0) || speaker_delimiters.contains(&chars[j].0) {
                    break;
                }
                for c in chars[j].1.to_lowercase() {
                    let Some(next) = self.nodes[node].next.get(&c) else {
                        break 'walk;
                    };
                    node = *next;
                }
                let end = chars.get(j + 1).map_or(sanitized.len(), |c| c.0);
                let text = &sanitized[start..end];
                for index in &self.nodes[node].entries {
                    let entry = &self.entries[*index];
                    if entry.case_sensitive && text != entry.term {
                        continue;
                    }
                    if word_char(chars[i].1) && i > 0 && word_char(chars[i - 1].1)
                        || word_char(chars[j].1) && chars.get(j + 1).is_some_and(|c| word_char(c.1))
                    {
                        continue;
                    }
                    if let Some((previous_end, target)) = best {
                        if previous_end == end {
                            conflict |= target != entry.translation;
                            continue;
                        }
                    }
                    best = Some((end, &entry.translation));
                    conflict = false;
                }
            }
            if let Some((end, target)) = best {
                if conflict
                    || target.trim().is_empty()
                    || target.contains('\\')
                    || target.contains("{PL_")
                    || target.contains("{G_")
                    || target.contains("⟦L")
                    || !PlaceholderProcessor::extract_with_profile(target, profile)
                        .1
                        .is_empty()
                {
                    return Err(());
                }
                edits.push((start, end, target));
                while i < chars.len() && chars[i].0 < end {
                    i += 1;
                }
            } else {
                i += 1;
            }
        }
        let mut nonce = 0;
        while source.contains(&format!("{{G_{nonce}_")) {
            nonce += 1;
        }
        let mut out = String::new();
        let mut last = 0;
        for (index, (start, end, target)) in edits.into_iter().enumerate() {
            out.push_str(&sanitized[last..start]);
            let token = format!("{{G_{nonce}_{index}}}");
            out.push_str(&token);
            phs.push(Placeholder {
                index: phs.len(),
                token,
                original: target.into(),
                pattern_type: PlaceholderKind::GlossaryTerm,
            });
            last = end;
        }
        out.push_str(&sanitized[last..]);
        Ok((out, phs))
    }
}

#[derive(Default)]
struct HintNode {
    next: HashMap<u8, usize>,
    failure: usize,
    output: usize,
    entries: Vec<usize>,
}

/// Byte-oriented Aho–Corasick matcher. All patterns and haystacks are valid UTF-8,
/// so a successful byte match has the same boundaries as `str::contains`.
struct HintIndex {
    nodes: Vec<HintNode>,
}

impl HintIndex {
    fn compile(patterns: impl Iterator<Item = (usize, String)>) -> Self {
        let mut nodes = vec![HintNode::default()];
        for (index, pattern) in patterns {
            let mut node = 0;
            for byte in pattern.bytes() {
                let next = if let Some(next) = nodes[node].next.get(&byte) {
                    *next
                } else {
                    let next = nodes.len();
                    nodes.push(HintNode::default());
                    nodes[node].next.insert(byte, next);
                    next
                };
                node = next;
            }
            nodes[node].entries.push(index);
        }
        let mut queue: VecDeque<_> = nodes[0].next.values().copied().collect();
        while let Some(node) = queue.pop_front() {
            let edges: Vec<_> = nodes[node].next.iter().map(|(b, n)| (*b, *n)).collect();
            for (byte, child) in edges {
                let mut failure = nodes[node].failure;
                while failure != 0 && !nodes[failure].next.contains_key(&byte) {
                    failure = nodes[failure].failure;
                }
                let failure = nodes[failure].next.get(&byte).copied().unwrap_or(0);
                nodes[child].failure = failure;
                nodes[child].output = if nodes[failure].entries.is_empty() {
                    nodes[failure].output
                } else {
                    failure
                };
                queue.push_back(child);
            }
        }
        Self { nodes }
    }

    fn collect_matches(&self, text: &str, matched: &mut Vec<usize>) {
        if self.nodes.len() == 1 {
            return;
        }
        let mut node = 0;
        let mut seen_outputs = HashSet::new();
        for byte in text.bytes() {
            while node != 0 && !self.nodes[node].next.contains_key(&byte) {
                node = self.nodes[node].failure;
            }
            node = self.nodes[node].next.get(&byte).copied().unwrap_or(0);
            let mut output = if self.nodes[node].entries.is_empty() {
                self.nodes[node].output
            } else {
                node
            };
            while output != 0 {
                // Revisiting a terminal also revisits its already-collected
                // suffix chain. Repeated occurrences cannot duplicate hints or
                // grow an intermediate vector with the length of the text.
                if !seen_outputs.insert(output) {
                    break;
                }
                matched.extend_from_slice(&self.nodes[output].entries);
                output = self.nodes[output].output;
            }
        }
    }
}

/// Reusable job-local exact and substring indexes. Input order remains the tie
/// breaker for conflicting exact entries and equal-length hint terms.
pub(crate) struct PreparedGlossary {
    entries: Vec<GlossaryEntry>,
    exact_sensitive: HashMap<String, usize>,
    exact_ascii: HashMap<String, usize>,
    exact_unicode: HashMap<String, usize>,
    hint_sensitive: HintIndex,
    hint_insensitive: HintIndex,
}

impl PreparedGlossary {
    pub(crate) fn compile(entries: &[GlossaryEntry]) -> Self {
        let mut exact_sensitive = HashMap::new();
        let mut exact_ascii = HashMap::new();
        let mut exact_unicode = HashMap::new();
        let mut sensitive_patterns = Vec::new();
        let mut insensitive_patterns = Vec::new();
        for (index, entry) in entries.iter().enumerate() {
            if entry.case_sensitive {
                exact_sensitive.entry(entry.term.clone()).or_insert(index);
                if !entry.term.is_empty() {
                    sensitive_patterns.push((index, entry.term.clone()));
                }
            } else {
                exact_ascii
                    .entry(entry.term.to_ascii_lowercase())
                    .or_insert(index);
                let lowercase = entry.term.to_lowercase();
                exact_unicode.entry(lowercase.clone()).or_insert(index);
                if !entry.term.is_empty() {
                    insensitive_patterns.push((index, lowercase));
                }
            }
        }
        Self {
            entries: entries.to_vec(),
            exact_sensitive,
            exact_ascii,
            exact_unicode,
            hint_sensitive: HintIndex::compile(sensitive_patterns.into_iter()),
            hint_insensitive: HintIndex::compile(insensitive_patterns.into_iter()),
        }
    }

    pub(crate) fn lookup_exact(&self, source: &str) -> Option<String> {
        let trimmed = source.trim();
        if trimmed.is_empty() {
            return None;
        }
        let mut first = self
            .exact_sensitive
            .get(trimmed)
            .into_iter()
            .chain(self.exact_sensitive.get(source))
            .chain(self.exact_ascii.get(&trimmed.to_ascii_lowercase()))
            .copied()
            .min();
        // The original scan only attempted Unicode lowercase equality when the
        // source was non-ASCII: e.g. "K" may match "k", but "k" must not match "K".
        if !trimmed.is_ascii() {
            if let Some(index) = self.exact_unicode.get(&trimmed.to_lowercase()) {
                first = Some(first.map_or(*index, |previous| previous.min(*index)));
            }
        }
        first.map(|index| self.entries[index].translation.clone())
    }

    pub(crate) fn hint(&self, text: &str, max_bytes: Option<(&str, usize)>) -> Option<String> {
        if self.entries.is_empty() || text.is_empty() {
            return None;
        }
        let mut matched = Vec::new();
        self.hint_sensitive.collect_matches(text, &mut matched);
        // Lowercase the entire string, not each character: contextual Unicode
        // mappings (notably Greek final sigma) must agree with `str::contains`.
        self.hint_insensitive
            .collect_matches(&text.to_lowercase(), &mut matched);
        if let Some((encoding, budget)) = max_bytes {
            matched.retain(|index| {
                matches!(
                    crate::validation::encoded_byte_len(encoding, &self.entries[*index].translation),
                    Some(bytes) if bytes <= budget
                )
            });
        }
        if matched.is_empty() {
            return None;
        }
        matched.sort_unstable_by(|a, b| {
            self.entries[*b]
                .term
                .len()
                .cmp(&self.entries[*a].term.len())
                .then_with(|| a.cmp(b))
        });
        let mut hint = String::from("Glossary (use these translations consistently):\n");
        for index in matched.into_iter().take(20) {
            let entry = &self.entries[index];
            hint.push_str(&format!("  {} → {}\n", entry.term, entry.translation));
        }
        Some(hint)
    }
}

pub struct Glossary {
    db: Arc<Database>,
    #[cfg(test)]
    get_all_calls: AtomicUsize,
}

impl Glossary {
    pub fn new(db: Arc<Database>) -> Self {
        Self {
            db,
            #[cfg(test)]
            get_all_calls: AtomicUsize::new(0),
        }
    }

    /// Add a case-insensitive entry, preserving the supplied context.
    /// Use [`Self::add_entry`] to specify case sensitivity.
    pub fn add(
        &self,
        term: &str,
        translation: &str,
        lang_pair: &str,
        context: Option<&str>,
    ) -> Result<()> {
        self.add_entry(&GlossaryEntry {
            term: term.to_string(),
            translation: translation.to_string(),
            lang_pair: lang_pair.to_string(),
            context: context.map(|s| s.to_string()),
            case_sensitive: false,
        })
    }

    /// Save an entry with its context and case-sensitivity setting intact.
    pub fn add_entry(&self, entry: &GlossaryEntry) -> Result<()> {
        self.db.save_glossary_entry(entry)
    }

    pub fn get_all(&self, lang_pair: &str) -> Result<Vec<GlossaryEntry>> {
        #[cfg(test)]
        self.get_all_calls.fetch_add(1, Ordering::Relaxed);
        self.db.get_glossary(lang_pair)
    }

    #[cfg(test)]
    pub(crate) fn get_all_call_count(&self) -> usize {
        self.get_all_calls.load(Ordering::Relaxed)
    }

    pub fn delete(&self, term: &str, lang_pair: &str) -> Result<()> {
        self.db.delete_glossary_entry(term, lang_pair)
    }

    pub fn build_hint(&self, source_lang: &str, target_lang: &str) -> Option<String> {
        let lang_pair = format!("{}-{}", source_lang, target_lang);
        let entries = self.get_all(&lang_pair).ok()?;
        if entries.is_empty() {
            return None;
        }
        let mut hint = String::from("Glossary (use these translations consistently):\n");
        for entry in entries.iter().take(50) {
            hint.push_str(&format!("  {} → {}\n", entry.term, entry.translation));
        }
        Some(hint)
    }

    /// Glossary lines for terms that actually appear in `text` (reduces noise on
    /// short UI binary slots). Prefers longer terms first. Cap 20 matches.
    pub fn build_hint_for_text(
        &self,
        source_lang: &str,
        target_lang: &str,
        text: &str,
    ) -> Option<String> {
        self.build_hint_for_text_budgeted(source_lang, target_lang, text, None)
    }

    /// Like [`build_hint_for_text`], but when `max_bytes` is `Some((encoding, budget))`
    /// drop glossary translations that would exceed a binary inject slot (e.g.
    /// `Options → Opciones` on a 7-byte utf8 budget). Avoids steering the model
    /// toward known-oversize forms after exact-match short-circuit declines them.
    pub fn build_hint_for_text_budgeted(
        &self,
        source_lang: &str,
        target_lang: &str,
        text: &str,
        max_bytes: Option<(&str, usize)>,
    ) -> Option<String> {
        let lang_pair = format!("{}-{}", source_lang, target_lang);
        let entries = self.get_all(&lang_pair).ok()?;
        Self::hint_from_entries(&entries, text, max_bytes)
    }

    pub(crate) fn hint_from_entries(
        entries: &[GlossaryEntry],
        text: &str,
        max_bytes: Option<(&str, usize)>,
    ) -> Option<String> {
        // This helper serves a single query. Translation jobs reuse their
        // PreparedGlossary instead of rebuilding its indexes for every row.
        if entries.is_empty() || text.is_empty() {
            return None;
        }
        let text_lower = text.to_lowercase();
        let mut matched: Vec<_> = entries
            .iter()
            .filter(|entry| {
                !entry.term.is_empty()
                    && if entry.case_sensitive {
                        text.contains(&entry.term)
                    } else {
                        text_lower.contains(&entry.term.to_lowercase())
                    }
                    && max_bytes.is_none_or(|(encoding, budget)| {
                        matches!(
                            crate::validation::encoded_byte_len(encoding, &entry.translation),
                            Some(bytes) if bytes <= budget
                        )
                    })
            })
            .collect();
        if matched.is_empty() {
            return None;
        }
        matched.sort_by(|a, b| b.term.len().cmp(&a.term.len()));
        let mut hint = String::from("Glossary (use these translations consistently):\n");
        for entry in matched.into_iter().take(20) {
            hint.push_str(&format!("  {} → {}\n", entry.term, entry.translation));
        }
        Some(hint)
    }

    /// Exact full-string glossary hit (trim + case per entry). Used to short-circuit
    /// the provider for known UI labels (Options → Opcns, etc.).
    pub fn lookup_exact(
        &self,
        source: &str,
        source_lang: &str,
        target_lang: &str,
    ) -> Option<String> {
        let lang_pair = format!("{}-{}", source_lang, target_lang);
        let entries = self.get_all(&lang_pair).ok()?;
        Self::lookup_exact_in_entries(&entries, source)
    }

    pub(crate) fn lookup_exact_in_entries(
        entries: &[GlossaryEntry],
        source: &str,
    ) -> Option<String> {
        // Index construction only pays off when the caller can reuse it.
        let trimmed = source.trim();
        if trimmed.is_empty() {
            return None;
        }
        let lowercase = (!trimmed.is_ascii()).then(|| trimmed.to_lowercase());
        entries
            .iter()
            .find(|entry| {
                if entry.case_sensitive {
                    entry.term == trimmed || entry.term == source
                } else {
                    entry.term.eq_ignore_ascii_case(trimmed)
                        || lowercase
                            .as_ref()
                            .is_some_and(|lowercase| entry.term.to_lowercase() == *lowercase)
                }
            })
            .map(|entry| entry.translation.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::Database;

    fn explicit(term: &str, target: &str, sensitive: bool) -> GlossaryEntry {
        GlossaryEntry {
            term: term.into(),
            translation: target.into(),
            lang_pair: "en-es".into(),
            context: None,
            case_sensitive: sensitive,
        }
    }

    // Kept independently of the prepared implementation as compatibility oracles.
    fn scan_exact(entries: &[GlossaryEntry], source: &str) -> Option<String> {
        let trimmed = source.trim();
        if trimmed.is_empty() {
            return None;
        }
        entries
            .iter()
            .find(|entry| {
                if entry.case_sensitive {
                    entry.term == trimmed || entry.term == source
                } else {
                    entry.term.eq_ignore_ascii_case(trimmed)
                        || (!trimmed.is_ascii()
                            && entry.term.to_lowercase() == trimmed.to_lowercase())
                }
            })
            .map(|entry| entry.translation.clone())
    }

    fn scan_hint(
        entries: &[GlossaryEntry],
        text: &str,
        max_bytes: Option<(&str, usize)>,
    ) -> Option<String> {
        if entries.is_empty() || text.is_empty() {
            return None;
        }
        let lowercase = text.to_lowercase();
        let mut matched: Vec<_> = entries
            .iter()
            .filter(|entry| {
                !entry.term.is_empty()
                    && if entry.case_sensitive {
                        text.contains(&entry.term)
                    } else {
                        lowercase.contains(&entry.term.to_lowercase())
                    }
                    && max_bytes.is_none_or(|(encoding, budget)| {
                        matches!(
                            crate::validation::encoded_byte_len(encoding, &entry.translation),
                            Some(bytes) if bytes <= budget
                        )
                    })
            })
            .collect();
        matched.sort_by(|a, b| b.term.len().cmp(&a.term.len()));
        if matched.is_empty() {
            return None;
        }
        let mut hint = String::from("Glossary (use these translations consistently):\n");
        for entry in matched.into_iter().take(20) {
            hint.push_str(&format!("  {} → {}\n", entry.term, entry.translation));
        }
        Some(hint)
    }

    #[test]
    fn prepared_exact_matches_scan_for_unicode_case_trim_and_conflict_order() {
        let mut entries = vec![
            explicit(" raw ", "raw-sensitive", true),
            explicit("raw", "trim-sensitive", true),
            explicit("RAW", "insensitive", false),
            explicit("k", "ascii-k", false),
            explicit("K", "kelvin", false),
            explicit("ÉMILE", "accent", false),
            explicit("éMILE", "accent-sensitive", true),
            explicit("İ", "dotted-i", false),
            explicit("i\u{307}", "combining-i", false),
            explicit("ΟΣ", "sigma-final", false),
            explicit("οσ", "sigma-medial", false),
            explicit("", "empty", false),
            explicit(" ", "blank", true),
        ];
        let sources = [
            "",
            " ",
            " raw ",
            "\u{2003}raw\u{a0}",
            "RAW",
            "k",
            "K",
            "K",
            "éMILE",
            "Émile",
            "İ",
            "i\u{307}",
            "I",
            "ΟΣ",
            "ος",
            "οσ",
            " ΟΣ ",
            "missing",
        ];
        for _ in 0..entries.len() {
            let prepared = PreparedGlossary::compile(&entries);
            for source in sources {
                let expected = scan_exact(&entries, source);
                assert_eq!(
                    prepared.lookup_exact(source),
                    expected,
                    "source={source:?}, first term={:?}",
                    entries[0].term
                );
                assert_eq!(
                    Glossary::lookup_exact_in_entries(&entries, source),
                    expected
                );
            }
            entries.rotate_left(1);
        }
        // Unicode equality is deliberately asymmetric when the source is ASCII.
        let kelvin = PreparedGlossary::compile(&[explicit("K", "kelvin", false)]);
        assert_eq!(kelvin.lookup_exact("k"), None);
        assert_eq!(kelvin.lookup_exact("K").as_deref(), Some("kelvin"));
    }

    #[test]
    fn prepared_hints_match_scan_for_overlap_unicode_case_and_binary_budgets() {
        let mut entries = vec![
            explicit("a", "A", false),
            explicit("ana", "corto", false),
            explicit("banana", "plátano", false),
            explicit("BANANA", "B", true),
            explicit("nana", "ñ", false),
            explicit("Σ", "sigma", false),
            explicit("ΟΣ", "final", false),
            explicit("οσ", "medial", false),
            explicit("İ", "dotted", false),
            explicit("i\u{307}", "I", false),
            explicit("É", "é", true),
            explicit("é", "e", false),
            explicit("猫", "猫", true),
            explicit("", "ignored", false),
        ];
        let texts = [
            "",
            "no match",
            "BANANA banana bananana",
            "ΟΣ",
            "ΟΣΑ",
            "ΟΣ ΟΣΑ Σ",
            "İ I i\u{307}",
            "É é 猫",
            "bananana İ ΟΣ É 猫",
        ];
        let budgets = [
            None,
            Some(("utf8", 0)),
            Some(("utf8", 1)),
            Some(("utf8", 4)),
            Some(("utf16le", 4)),
            Some(("shift_jis", 4)),
            Some(("unknown", 100)),
        ];
        for _ in 0..entries.len() {
            let prepared = PreparedGlossary::compile(&entries);
            for text in texts {
                for budget in budgets {
                    let expected = scan_hint(&entries, text, budget);
                    assert_eq!(
                        prepared.hint(text, budget),
                        expected,
                        "text={text:?}, budget={budget:?}, first term={:?}",
                        entries[0].term
                    );
                    assert_eq!(
                        Glossary::hint_from_entries(&entries, text, budget),
                        expected
                    );
                }
            }
            entries.rotate_left(1);
        }
    }

    #[test]
    fn prepared_hints_preserve_utf8_length_ties_cap_and_duplicate_entries() {
        let mut entries: Vec<_> = (0..25)
            .map(|index| explicit(&format!("t{index:02}"), &format!("v{index}"), false))
            .collect();
        entries.insert(0, explicit("猫", "cat", false)); // Three UTF-8 bytes, like t00.
        entries.insert(1, explicit("t00", "first duplicate", true));
        let text = format!(
            "猫 {} t00 t00",
            (0..25)
                .rev()
                .map(|index| format!("t{index:02}"))
                .collect::<Vec<_>>()
                .join(" ")
        );
        let hint = PreparedGlossary::compile(&entries)
            .hint(&text, None)
            .unwrap();
        assert_eq!(Some(hint.clone()), scan_hint(&entries, &text, None));
        assert_eq!(hint.lines().count(), 21);
        assert_eq!(hint.lines().nth(1), Some("  猫 → cat"));
        assert_eq!(hint.lines().nth(2), Some("  t00 → first duplicate"));
        assert_eq!(hint.lines().nth(3), Some("  t00 → v0"));
    }

    #[test]
    fn prepared_hints_collect_suffix_outputs_once_across_repeated_occurrences() {
        let entries: Vec<_> = (1..=30)
            .map(|length| explicit(&"a".repeat(length), &format!("v{length}"), false))
            .chain([
                explicit("ba", "ba", true),
                explicit("cba", "cba", true),
                explicit("dcba", "dcba", true),
            ])
            .collect();
        let text = format!("ba cba dcba {} cba ba", "a".repeat(10_000));
        assert_eq!(
            PreparedGlossary::compile(&entries).hint(&text, None),
            scan_hint(&entries, &text, None)
        );
    }

    #[test]
    #[ignore = "100k-row/1k-term W3-01 synthetic matching measurement"]
    fn benchmark_prepared_glossary_100k_rows_1k_terms() {
        use std::hint::black_box;
        use std::time::Instant;

        let entries: Vec<_> = (0..1_000)
            .map(|index| {
                explicit(
                    &format!("term{index:04}"),
                    &format!("valor{index:04}"),
                    false,
                )
            })
            .collect();
        let rows: Vec<_> = (0..100_000)
            .map(|index| {
                if index % 2 == 0 {
                    format!("term{:04}", index % 1_000)
                } else {
                    format!("A dialogue containing TERM{:04}.", index % 1_000)
                }
            })
            .collect();
        let start = Instant::now();
        let before_exact = rows
            .iter()
            .filter(|source| black_box(scan_exact(&entries, source)).is_some())
            .count();
        let before_exact_time = start.elapsed();
        let start = Instant::now();
        let before_hints = rows
            .iter()
            .skip(1)
            .step_by(2)
            .filter(|source| black_box(scan_hint(&entries, source, None)).is_some())
            .count();
        let before_hint_time = start.elapsed();
        let start = Instant::now();
        let prepared = PreparedGlossary::compile(&entries);
        let compile_time = start.elapsed();
        let start = Instant::now();
        let after_exact = rows
            .iter()
            .filter(|source| black_box(prepared.lookup_exact(source)).is_some())
            .count();
        let after_exact_time = start.elapsed();
        let start = Instant::now();
        let after_hints = rows
            .iter()
            .skip(1)
            .step_by(2)
            .filter(|source| black_box(prepared.hint(source, None)).is_some())
            .count();
        let after_hint_time = start.elapsed();
        assert_eq!((before_exact, before_hints), (50_000, 50_000));
        assert_eq!((before_exact, before_hints), (after_exact, after_hints));
        println!(
            "W3-01 matching rows=100000 terms=1000 exact_hits={after_exact} hint_rows={after_hints} \
             before_exact_ms={:.3} before_hint_ms={:.3} before_total_ms={:.3} \
             compile_ms={:.3} after_exact_ms={:.3} after_hint_ms={:.3} after_total_ms={:.3} \
             hint_term_lowercases_before=50000000 hint_term_lowercases_after=1000",
            before_exact_time.as_secs_f64() * 1_000.0,
            before_hint_time.as_secs_f64() * 1_000.0,
            (before_exact_time + before_hint_time).as_secs_f64() * 1_000.0,
            compile_time.as_secs_f64() * 1_000.0,
            after_exact_time.as_secs_f64() * 1_000.0,
            after_hint_time.as_secs_f64() * 1_000.0,
            (compile_time + after_exact_time + after_hint_time).as_secs_f64() * 1_000.0,
        );
    }

    fn canonical(entries: &[GlossaryEntry], text: &str) -> std::result::Result<String, ()> {
        let (masked, phs) = VisibleGlossary::compile(entries, "en-es")
            .mask(text, crate::placeholder::PlaceholderProfile::RpgMaker)?;
        crate::placeholder::PlaceholderProcessor::restore(&masked, &phs).map_err(|_| ())
    }

    #[test]
    fn visible_plan_is_leftmost_longest_case_aware_and_unicode_word_bounded() {
        let entries = vec![
            explicit("Boy", "Chico", false),
            explicit("Boy King", "Rey", false),
            explicit("Louis", "Louis", true),
            explicit("Émile", "Emilio", false),
        ];
        assert_eq!(canonical(&entries,
            "Boy King Boy Boyfriend Louis LouisLongIdentifier louis ÉMILE préÉmile Émile2 Émile\u{301} Boy_foo Boy\u{203f}foo").unwrap(),
            "Rey Chico Boyfriend Louis LouisLongIdentifier louis Emilio préÉmile Émile2 Émile\u{301} Boy_foo Boy\u{203f}foo");
        assert_eq!(
            canonical(&entries, "«Boy» — Émile。Boy").unwrap(),
            "«Chico» — Emilio。Chico"
        );
    }

    #[test]
    fn visible_terms_do_not_consume_recognized_speaker_delimiters() {
        let entries = vec![
            explicit("<Old house>", "WRONG", true),
            explicit("Old", "Vieja", true),
            explicit("house", "casa", true),
        ];
        assert_eq!(
            canonical(&entries, r"\n<Old house>").unwrap(),
            r"\n<Vieja casa>"
        );
    }

    #[test]
    fn public_restore_keeps_canonical_identity_without_freezing_grammar() {
        let entries = vec![
            explicit("Old", "Vieja", true),
            explicit("house", "casa", true),
        ];
        let (masked, phs) = VisibleGlossary::compile(&entries, "en-es")
            .mask(
                "Old house",
                crate::placeholder::PlaceholderProfile::RpgMaker,
            )
            .unwrap();
        assert_eq!(masked, "{G_0_0} {G_0_1}");
        assert_eq!(
            crate::placeholder::PlaceholderProcessor::restore("{G_0_1} {G_0_0}", &phs).unwrap(),
            "casa Vieja"
        );
        assert!(
            crate::placeholder::PlaceholderProcessor::restore("{G_0_1} {G_0_1}", &phs).is_err()
        );
    }

    #[test]
    fn duplicate_equivalents_coalesce_but_conflicts_and_wrong_pair_do_not_win() {
        let entries = vec![
            explicit("Boy", "Chico", false),
            explicit("boy", "Chico", false),
        ];
        assert_eq!(canonical(&entries, "BOY").unwrap(), "Chico");
        let mut conflict = entries.clone();
        conflict.push(explicit("Boy", "Muchacho", true));
        assert!(canonical(&conflict, "Boy").is_err());
        assert_eq!(canonical(&conflict, "boy").unwrap(), "Chico");
        let mut wrong_pair = explicit("Boy", "Garçon", false);
        wrong_pair.lang_pair = "en-fr".into();
        assert_eq!(canonical(&[wrong_pair], "Boy").unwrap(), "Boy");
    }

    #[test]
    fn source_controls_and_sentinel_lookalikes_are_opaque_and_canonical_not_recursive() {
        let entries = vec![
            explicit("Boy", "Chico", false),
            explicit("Person", "Boy", false),
            explicit("PL", "WRONG", false),
            explicit("G", "WRONG", false),
        ];
        let source = r"Person {font=Boy.ttf}Boy{/font} \C[Boy] {PL_0} {PL_1} {G_Boy} {G_0_0}";
        let (masked, phs) = VisibleGlossary::compile(&entries, "en-es")
            .mask(source, crate::placeholder::PlaceholderProfile::RpgMaker)
            .unwrap();
        assert_eq!(
            crate::placeholder::PlaceholderProcessor::restore(&masked, &phs).unwrap(),
            r"Boy {font=Boy.ttf}Chico{/font} \C[Boy] {PL_0} {PL_1} {G_Boy} {G_0_0}"
        );
        assert!(!masked.contains("Boy"));
        assert!(masked.contains("{G_1_0}"));
        assert!(masked.contains("{PL_0_0}"));
        assert_eq!(
            canonical(&entries, r"\\n<Boy> Boy").unwrap(),
            r"\\n<Boy> Chico"
        );
        assert_eq!(
            canonical(&entries, r"\n<Boy> Boy").unwrap(),
            r"\n<Chico> Chico"
        );
        assert_eq!(
            canonical(&entries, "{{G_0_0} { G-Boy} Boy").unwrap(),
            "{{G_0_0} { G-Boy} Chico"
        );
    }

    fn setup() -> (Arc<Database>, Glossary) {
        let db = Arc::new(Database::open_in_memory().unwrap());
        let glossary = Glossary::new(db.clone());
        (db, glossary)
    }

    #[test]
    fn test_add_and_get() {
        let (_db, glossary) = setup();
        glossary.add("HP", "Health Points", "ja-en", None).unwrap();
        glossary.add("MP", "Magic Points", "ja-en", None).unwrap();
        let entries = glossary.get_all("ja-en").unwrap();
        assert_eq!(entries.len(), 2);
        assert!(entries.iter().all(|entry| !entry.case_sensitive));
    }

    #[test]
    fn test_add_entry_preserves_case_sensitive_and_context() {
        let (_db, glossary) = setup();
        for context in [Some("  Organization title\nUSA  "), Some(""), None] {
            let entry = GlossaryEntry {
                context: context.map(str::to_owned),
                ..explicit("US", "EEUU", true)
            };
            glossary.add_entry(&entry).unwrap();
            let entries = glossary.get_all("en-es").unwrap();
            assert_eq!(entries.len(), 1);
            assert_eq!(
                serde_json::to_value(&entries[0]).unwrap(),
                serde_json::to_value(&entry).unwrap()
            );
        }
    }

    #[test]
    fn test_add_preserves_context_and_defaults_to_case_insensitive() {
        let (_db, glossary) = setup();
        glossary
            .add("US", "EEUU", "en-es", Some("  Organization title  "))
            .unwrap();
        let entries = glossary.get_all("en-es").unwrap();
        assert_eq!(entries.len(), 1);
        assert!(!entries[0].case_sensitive);
        assert_eq!(
            entries[0].context.as_deref(),
            Some("  Organization title  ")
        );
    }

    #[test]
    fn test_delete_entry() {
        let (_db, glossary) = setup();
        glossary.add("HP", "Health Points", "ja-en", None).unwrap();
        glossary.delete("HP", "ja-en").unwrap();
        let entries = glossary.get_all("ja-en").unwrap();
        assert_eq!(entries.len(), 0);
    }

    #[test]
    fn test_build_hint_empty() {
        let (_db, glossary) = setup();
        assert!(glossary.build_hint("ja", "en").is_none());
    }

    #[test]
    fn test_build_hint_format() {
        let (_db, glossary) = setup();
        glossary.add("HP", "Health Points", "ja-en", None).unwrap();
        glossary.add("MP", "Magic Points", "ja-en", None).unwrap();
        let hint = glossary.build_hint("ja", "en").unwrap();
        assert!(hint.contains("HP → Health Points"));
        assert!(hint.contains("MP → Magic Points"));
        assert!(hint.starts_with("Glossary (use these translations consistently):\n"));
    }

    #[test]
    fn test_build_hint_max_50() {
        let (_db, glossary) = setup();
        for i in 0..60 {
            glossary
                .add(&format!("term{}", i), &format!("trans{}", i), "ja-en", None)
                .unwrap();
        }
        let hint = glossary.build_hint("ja", "en").unwrap();
        let lines: Vec<&str> = hint.lines().collect();
        // 1 header line + 50 entry lines
        assert_eq!(lines.len(), 51);
    }

    #[test]
    fn test_lang_pair_isolation() {
        let (_db, glossary) = setup();
        glossary.add("HP", "Health Points", "ja-en", None).unwrap();
        glossary.add("vida", "life", "es-en", None).unwrap();
        let ja_entries = glossary.get_all("ja-en").unwrap();
        assert_eq!(ja_entries.len(), 1);
        assert_eq!(ja_entries[0].term, "HP");
    }

    #[test]
    fn test_duplicate_term_upserts() {
        let (_db, glossary) = setup();
        glossary.add("HP", "Health Points", "ja-en", None).unwrap();
        glossary.add("HP", "Hit Points", "ja-en", None).unwrap();
        let entries = glossary.get_all("ja-en").unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].translation, "Hit Points");
    }

    #[test]
    fn test_lookup_exact_case_insensitive() {
        let (_db, glossary) = setup();
        glossary.add("Options", "Opcns", "en-es", None).unwrap();
        assert_eq!(
            glossary.lookup_exact("Options", "en", "es").as_deref(),
            Some("Opcns")
        );
        assert_eq!(
            glossary.lookup_exact("  options  ", "en", "es").as_deref(),
            Some("Opcns")
        );
        assert!(glossary.lookup_exact("Option", "en", "es").is_none());
        assert!(glossary.lookup_exact("Options", "en", "fr").is_none());
    }

    #[test]
    fn test_build_hint_for_text_filters_to_source() {
        let (_db, glossary) = setup();
        glossary.add("HP", "PV", "en-es", None).unwrap();
        glossary.add("Options", "Opcns", "en-es", None).unwrap();
        glossary
            .add("Load Game", "Cargar J", "en-es", None)
            .unwrap();

        let only_hp = glossary
            .build_hint_for_text("en", "es", "Current HP: 12")
            .unwrap();
        assert!(only_hp.contains("HP → PV"), "{only_hp}");
        assert!(
            !only_hp.contains("Options"),
            "unrelated term must not appear: {only_hp}"
        );

        let multi = glossary
            .build_hint_for_text("en", "es", "Load Game / Options")
            .unwrap();
        assert!(multi.contains("Load Game → Cargar J"), "{multi}");
        assert!(multi.contains("Options → Opcns"), "{multi}");
        // Longer term first
        let load_pos = multi.find("Load Game").unwrap();
        let opt_pos = multi.find("Options").unwrap();
        assert!(load_pos < opt_pos, "longer terms first: {multi}");

        assert!(glossary
            .build_hint_for_text("en", "es", "Nothing matching")
            .is_none());
    }

    #[test]
    fn test_build_hint_budgeted_drops_oversize_translation() {
        let (_db, glossary) = setup();
        // "Options" is 7 utf8 bytes; "Opciones" is 8 → oversize for that slot.
        glossary.add("Options", "Opciones", "en-es", None).unwrap();
        glossary.add("OK", "Si", "en-es", None).unwrap();

        assert!(
            glossary
                .build_hint_for_text_budgeted("en", "es", "Options", Some(("utf8", 7)))
                .is_none(),
            "oversize glossary form must not appear in binary-slot hint"
        );
        let fitting = glossary
            .build_hint_for_text_budgeted("en", "es", "Options", Some(("utf8", 8)))
            .unwrap();
        assert!(fitting.contains("Options → Opciones"), "{fitting}");

        // Substring match still filters by translation size.
        let mixed = glossary
            .build_hint_for_text_budgeted("en", "es", "Options OK", Some(("utf8", 7)))
            .unwrap();
        assert!(mixed.contains("OK → Si"), "{mixed}");
        assert!(!mixed.contains("Opciones"), "{mixed}");
    }
}
