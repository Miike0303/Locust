use crate::database::{Database, GlossaryEntry};
use crate::error::Result;
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
        if entries.is_empty() || text.is_empty() {
            return None;
        }
        let text_lower = text.to_lowercase();
        let mut matched: Vec<&GlossaryEntry> = entries
            .iter()
            .filter(|e| {
                if e.term.is_empty() {
                    return false;
                }
                let in_text = if e.case_sensitive {
                    text.contains(&e.term)
                } else {
                    text_lower.contains(&e.term.to_lowercase())
                };
                if !in_text {
                    return false;
                }
                if let Some((enc, budget)) = max_bytes {
                    matches!(
                        crate::validation::encoded_byte_len(enc, &e.translation),
                        Some(n) if n <= budget
                    )
                } else {
                    true
                }
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
        let trimmed = source.trim();
        if trimmed.is_empty() {
            return None;
        }
        for e in entries {
            let hit = if e.case_sensitive {
                e.term == trimmed || e.term == source
            } else {
                e.term.eq_ignore_ascii_case(trimmed)
                    || (!trimmed.is_ascii() && e.term.to_lowercase() == trimmed.to_lowercase())
            };
            if hit {
                return Some(e.translation.clone());
            }
        }
        None
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
