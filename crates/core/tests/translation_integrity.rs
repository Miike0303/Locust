use async_trait::async_trait;
use locust_core::database::Database;
use locust_core::error::Result;
use locust_core::glossary::Glossary;
use locust_core::models::{StringEntry, TranslationRequest, TranslationResult};
use locust_core::translation::{TranslationManager, TranslationOptions, TranslationProvider};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

#[derive(Default)]
struct HintIgnoringProvider {
    requests: Mutex<Vec<TranslationRequest>>,
    damage: Option<&'static str>,
    fail_for_requests: usize,
    id: Option<&'static str>,
}

#[async_trait]
impl TranslationProvider for HintIgnoringProvider {
    fn id(&self) -> &str {
        self.id.unwrap_or("glossary-fixture")
    }
    fn name(&self) -> &str {
        "Hint ignoring fixture"
    }
    fn is_free(&self) -> bool {
        true
    }
    fn requires_api_key(&self) -> bool {
        false
    }
    async fn estimate_cost(&self, _: usize, _: &str) -> Option<f64> {
        Some(0.0)
    }
    async fn health_check(&self) -> Result<()> {
        Ok(())
    }
    async fn translate(&self, requests: &[TranslationRequest]) -> Result<Vec<TranslationResult>> {
        let mut seen = self.requests.lock().unwrap();
        let damage = (seen.len() < self.fail_for_requests)
            .then_some(self.damage)
            .flatten();
        seen.extend_from_slice(requests);
        Ok(requests
            .iter()
            .map(|r| TranslationResult {
                entry_id: r.entry_id.clone(),
                // Like a source-only MT provider: hints/context have no effect.
                translation: {
                    let mut text = r
                        .source
                        .replace("Louis", "Luis")
                        .replace(" says ", " dice ");
                    let tokens: Vec<_> = r
                        .source
                        .match_indices("{G_")
                        .map(|(start, _)| {
                            let end = start + r.source[start..].find('}').unwrap() + 1;
                            r.source[start..end].to_string()
                        })
                        .collect();
                    if let Some(first) = tokens.first() {
                        match damage {
                            Some("drop") => text = text.replace(first, ""),
                            Some("literal") => text = text.replace(first, "Louis"),
                            Some("malformed") => text = text.replace(first, "{G_x}"),
                            Some("unknown") => text.push_str("{G_999_0}"),
                            Some("unknown-mangled") => text.push_str("{ G-999-0}"),
                            Some("duplicate") => text.push_str(first),
                            Some("reorder" | "header-grammar" | "body-and-role") => {
                                text = swap_tokens(&text, first, &tokens[1]);
                                if damage == Some("header-grammar") {
                                    text = text.replace("of doom", "maldita");
                                }
                                if damage == Some("body-and-role") {
                                    text = text.replace("<Alchemist>", "<Alquimista>");
                                }
                            }
                            Some("body-reorder" | "body-and-last-header-reorder") => {
                                text = swap_tokens(&text, &tokens[1], &tokens[2]);
                                if damage == Some("body-and-last-header-reorder") {
                                    text = swap_tokens(&text, &tokens[3], &tokens[4]);
                                }
                            }
                            Some("into-generic-header") => {
                                text = text
                                    .replacen(first, "", 1)
                                    .replace("<Alchemist>", &format!("<{first}>"));
                            }
                            Some("new-header") => {
                                text = text
                                    .replacen(first, "", 1)
                                    .replace("{PL_0}", &format!("{{PL_0}}<{first}>"));
                            }
                            Some("generic-new-header") => text.push_str("<Alquimista>"),
                            Some("join-reserved") => text = text.replace(' ', ""),
                            Some("prefix-before-body") => {
                                let start = text.find("{PL_").unwrap();
                                let end = start + text[start..].find('}').unwrap() + 1;
                                let prefix = text[start..end].to_string();
                                text = format!("{prefix}{}", text.replace(&prefix, ""));
                            }
                            Some("header-open") => text = text.replacen('<', "", 1),
                            Some("header-change-open") => text = text.replacen('<', "(", 1),
                            Some("header-guess-close") => text.push('>'),
                            Some("header-remove-syntax") => text = text.replace(['<', '>'], ""),
                            Some("header-close") => text = text.replacen('>', "", 1),
                            Some("header-nested") => text = text.replacen('<', "<<", 1),
                            Some("header-space") => text = text.replacen("}<", "} <", 1),
                            Some("header-newline") => text = text.replacen('<', "<\n", 1),
                            _ => {}
                        }
                    }
                    text
                },
                detected_source_lang: None,
                provider: self.id().into(),
                tokens_used: Some(1),
                input_tokens: Some(1),
                output_tokens: Some(1),
                cost_usd: Some(0.0),
            })
            .collect())
    }
}

fn swap_tokens(text: &str, first: &str, second: &str) -> String {
    text.replace(first, "TEMP")
        .replace(second, first)
        .replace("TEMP", second)
}

async fn run(
    db: Arc<Database>,
    glossary: Arc<Glossary>,
    provider: Arc<HintIgnoringProvider>,
    entries: Vec<StringEntry>,
    use_memory: bool,
) {
    db.save_entries(&entries).unwrap();
    let manager = TranslationManager::new(provider, db, glossary);
    let (tx, _rx) = mpsc::channel(512);
    manager
        .translate_entries(
            entries,
            TranslationOptions {
                source_lang: "en".into(),
                target_lang: "es".into(),
                use_memory,
                ..Default::default()
            },
            tx,
            "fixture".into(),
            CancellationToken::new(),
        )
        .await
        .unwrap();
}

fn row(id: &str, source: &str) -> StringEntry {
    let mut row = StringEntry::new(id, source, PathBuf::from("Map001.json"));
    row.metadata
        .insert("format_id".into(), serde_json::json!("rpgmaker-mv"));
    row
}

#[tokio::test]
async fn canonical_insertion_cannot_fabricate_a_source_absent_speaker_argument() {
    let db = Arc::new(Database::open_in_memory().unwrap());
    let glossary = Arc::new(Glossary::new(db.clone()));
    glossary.add("Boy", "<Louis", "en-es", None).unwrap();
    let provider = Arc::new(HintIgnoringProvider::default());
    let entry = row("canonical-header", r"\nBoy>");
    let hash = entry.source_hash();
    run(db.clone(), glossary, provider.clone(), vec![entry], true).await;
    let saved = db.get_entry("canonical-header").unwrap().unwrap();
    assert_eq!(
        saved.translation, None,
        "canonical insertion invented a speaker header"
    );
    assert_eq!(saved.status, locust_core::models::StringStatus::Pending);
    assert_eq!(saved.source, r"\nBoy>");
    assert!(db.lookup_memory(&hash, "en-es").unwrap().is_none());
    assert_eq!(
        provider.requests.lock().unwrap().len(),
        0,
        "unsafe source canonical echo must not spend usage"
    );
    assert!(
        db.get_translation_runs().unwrap().is_empty(),
        "existing no-usage ledger policy is unchanged"
    );
}

#[tokio::test]
async fn canonical_partial_closing_delimiter_cannot_complete_a_new_speaker_argument() {
    let db = Arc::new(Database::open_in_memory().unwrap());
    let glossary = Arc::new(Glossary::new(db.clone()));
    glossary.add("Boy", "<Louis", "en-es", None).unwrap();
    glossary.add("End", ">", "en-es", None).unwrap();
    let provider = Arc::new(HintIgnoringProvider::default());
    run(
        db.clone(),
        glossary,
        provider.clone(),
        vec![row("canonical-close", r"\nBoy End")],
        false,
    )
    .await;
    assert_eq!(
        db.get_entry("canonical-close")
            .unwrap()
            .unwrap()
            .translation,
        None
    );
    assert!(provider.requests.lock().unwrap().is_empty());
}

#[tokio::test]
async fn canonical_body_comparison_is_allowed_but_a_moved_prefix_cannot_make_it_a_header() {
    for (source, target, mode, expected) in [
        (
            r"Boy says limit \n",
            "< 3",
            None,
            Some(r"< 3 dice limit \n"),
        ),
        (
            r"Boy says limit \n",
            "< 3 > 1",
            None,
            Some(r"< 3 > 1 dice limit \n"),
        ),
        (
            r"Boy says limit \n",
            "< 3",
            Some("generic-new-header"),
            None,
        ),
        (
            r"Boy> says limit \n",
            "< 3",
            Some("prefix-before-body"),
            None,
        ),
        (
            r"\n<Alchemist>Boy says limit",
            "< 3",
            None,
            Some(r"\n<Alchemist>< 3 dice limit"),
        ),
        (r"\n<Boy>Text", ">Louis", None, None),
        (r"\n<Boy>Text", "<Louis", None, None),
    ] {
        let db = Arc::new(Database::open_in_memory().unwrap());
        let glossary = Arc::new(Glossary::new(db.clone()));
        glossary.add("Boy", target, "en-es", None).unwrap();
        let provider = Arc::new(HintIgnoringProvider {
            damage: mode,
            fail_for_requests: usize::MAX,
            ..Default::default()
        });
        run(
            db.clone(),
            glossary,
            provider.clone(),
            vec![row("comparison", source)],
            true,
        )
        .await;
        assert_eq!(
            db.get_entry("comparison")
                .unwrap()
                .unwrap()
                .translation
                .as_deref(),
            expected,
            "{source}: {mode:?}"
        );
        if mode.is_some() {
            assert_eq!(
                provider.requests.lock().unwrap().len(),
                3,
                "post-dispatch guards retain bounded repairs"
            );
            assert_eq!(db.get_translation_runs().unwrap()[0].tokens_used, 3);
        } else if expected.is_none() {
            assert!(
                provider.requests.lock().unwrap().is_empty(),
                "impossible canonical source layout is rejected pre-dispatch"
            );
        }
    }
}

#[tokio::test]
async fn canonical_fragments_cannot_synthesize_a_recursively_restored_control_identity() {
    let db = Arc::new(Database::open_in_memory().unwrap());
    let glossary = Arc::new(Glossary::new(db.clone()));
    glossary.add("Boy", "{P", "en-es", None).unwrap();
    let provider = Arc::new(HintIgnoringProvider {
        damage: Some("join-reserved"),
        fail_for_requests: usize::MAX,
        ..Default::default()
    });
    let entry = row("reserved-fragments", r"Boy L_0} \C[2]");
    let hash = entry.source_hash();
    run(db.clone(), glossary, provider.clone(), vec![entry], true).await;
    assert!(db
        .get_entry("reserved-fragments")
        .unwrap()
        .unwrap()
        .translation
        .is_none());
    assert!(db.lookup_memory(&hash, "en-es").unwrap().is_none());
    assert_eq!(provider.requests.lock().unwrap().len(), 3);
    assert_eq!(db.get_translation_runs().unwrap()[0].tokens_used, 3);
}

#[tokio::test]
async fn explicit_visible_glossary_survives_hint_ignoring_provider() {
    let db = Arc::new(Database::open_in_memory().unwrap());
    let glossary = Arc::new(Glossary::new(db.clone()));
    glossary.add("Louis", "Louis", "en-es", None).unwrap();
    glossary.add("Boy", "Chico", "en-es", None).unwrap();
    let mut entry = StringEntry::new(
        "line",
        r"\n<Louis>Louis says Boy \C[2]",
        PathBuf::from("Map001.json"),
    );
    entry
        .metadata
        .insert("format_id".into(), serde_json::json!("rpgmaker-mv"));
    db.save_entries(std::slice::from_ref(&entry)).unwrap();
    let provider = Arc::new(HintIgnoringProvider::default());
    let manager = TranslationManager::new(provider.clone(), db.clone(), glossary);
    let (tx, _rx) = mpsc::channel(128);
    manager
        .translate_entries(
            vec![entry],
            TranslationOptions {
                source_lang: "en".into(),
                target_lang: "es".into(),
                use_memory: false,
                ..Default::default()
            },
            tx,
            "fixture".into(),
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(
        db.get_entry("line")
            .unwrap()
            .unwrap()
            .translation
            .as_deref(),
        Some(r"\n<Louis>Louis dice Chico \C[2]")
    );
    let requests = provider.requests.lock().unwrap();
    assert!(!requests[0].source.contains("Louis"));
    assert!(!requests[0].source.contains("Boy"));
}

#[tokio::test]
async fn glossary_damage_fails_closed_with_two_accounted_repairs() {
    for damage in [
        "drop",
        "literal",
        "malformed",
        "unknown",
        "unknown-mangled",
        "duplicate",
    ] {
        let db = Arc::new(Database::open_in_memory().unwrap());
        let glossary = Arc::new(Glossary::new(db.clone()));
        glossary.add("Louis", "Louis", "en-es", None).unwrap();
        glossary.add("Boy", "Chico", "en-es", None).unwrap();
        let provider = Arc::new(HintIgnoringProvider {
            damage: Some(damage),
            fail_for_requests: usize::MAX,
            ..Default::default()
        });
        let entry = row("bad", r"Louis says Boy \C[2]");
        let hash = entry.source_hash();
        run(db.clone(), glossary, provider.clone(), vec![entry], true).await;
        let saved = db.get_entry("bad").unwrap().unwrap();
        assert!(saved.translation.is_none(), "{damage}");
        assert_eq!(
            saved.status,
            locust_core::models::StringStatus::Pending,
            "{damage}"
        );
        assert_eq!(provider.requests.lock().unwrap().len(), 3, "{damage}");
        assert!(db.lookup_memory(&hash, "en-es").unwrap().is_none());
        let runs = db.get_translation_runs().unwrap();
        assert_eq!(
            runs[0].tokens_used, 3,
            "{damage}: rejected work must be accounted"
        );
    }
}

#[tokio::test]
async fn canonical_body_tokens_allow_target_language_grammar_reordering() {
    let db = Arc::new(Database::open_in_memory().unwrap());
    let glossary = Arc::new(Glossary::new(db.clone()));
    for (term, translation) in [("Old", "Vieja"), ("house", "casa")] {
        glossary
            .add_entry(&locust_core::database::GlossaryEntry {
                term: term.into(),
                translation: translation.into(),
                lang_pair: "en-es".into(),
                context: None,
                case_sensitive: true,
            })
            .unwrap();
    }
    let provider = Arc::new(HintIgnoringProvider {
        damage: Some("reorder"),
        fail_for_requests: usize::MAX,
        ..Default::default()
    });
    run(
        db.clone(),
        glossary,
        provider.clone(),
        vec![row("grammar", "Old house")],
        true,
    )
    .await;
    assert_eq!(
        db.get_entry("grammar")
            .unwrap()
            .unwrap()
            .translation
            .as_deref(),
        Some("casa Vieja")
    );
    assert_eq!(
        provider.requests.lock().unwrap().len(),
        1,
        "valid grammar needs no repair"
    );
}

fn speaker_glossary(db: Arc<Database>) -> Arc<Glossary> {
    let glossary = Arc::new(Glossary::new(db));
    for (term, translation) in [
        ("Lydia", "Lydia"),
        ("Louis", "Louis"),
        ("Boy", "Chico"),
        ("Old", "Vieja"),
        ("house", "casa"),
    ] {
        glossary
            .add_entry(&locust_core::database::GlossaryEntry {
                term: term.into(),
                translation: translation.into(),
                lang_pair: "en-es".into(),
                context: None,
                case_sensitive: true,
            })
            .unwrap();
    }
    glossary
}

#[tokio::test]
async fn grammar_can_reorder_body_and_header_terms_without_freezing_generic_labels() {
    for (source, mode, expected) in [
        (
            r"\n<Lydia>Boy says Louis",
            "body-reorder",
            r"\n<Lydia>Louis dice Chico",
        ),
        (
            r"\n<Old house of doom>Louis says Boy",
            "header-grammar",
            r"\n<casa Vieja maldita>Louis dice Chico",
        ),
        (
            r"\n<Alchemist>Louis says Boy",
            "body-and-role",
            r"\n<Alquimista>Chico dice Louis",
        ),
        (
            r"Émile \n<Lydia>Boy says Louis {PL_0} {G_0_0} \nr<Old house>",
            "body-and-last-header-reorder",
            r"Émile \n<Lydia>Louis dice Chico {PL_0} {G_0_0} \nr<casa Vieja>",
        ),
    ] {
        let db = Arc::new(Database::open_in_memory().unwrap());
        let glossary = speaker_glossary(db.clone());
        // A markup-shaped explicit term cannot consume a speaker wrapper or
        // supersede the independently configured visible word identities.
        glossary.add("<Old house>", "WRONG", "en-es", None).unwrap();
        let provider = Arc::new(HintIgnoringProvider {
            damage: Some(mode),
            fail_for_requests: usize::MAX,
            ..Default::default()
        });
        run(
            db.clone(),
            glossary,
            provider.clone(),
            vec![row("grammar", source)],
            true,
        )
        .await;
        let saved = db.get_entry("grammar").unwrap().unwrap();
        assert_eq!(saved.translation.as_deref(), Some(expected), "{mode}");
        assert!(saved.require_preserved_translation_controls().is_ok());
        assert_eq!(provider.requests.lock().unwrap().len(), 1, "{mode}");
    }
}

#[tokio::test]
async fn speaker_identity_membership_and_syntax_survive_all_bounded_repairs() {
    for (source, mode) in [
        (r"\n<Lydia>Louis says Boy", "reorder"), // header identity ↔ body identity
        (r"\n<Lydia>Hi \nr<Louis>Boy", "reorder"), // first header ↔ second header
        (r"\n<Alchemist>Louis says Boy", "into-generic-header"),
        (r"Louis says Boy \n", "new-header"),
        (r"\n<Lydia>Louis says Boy", "header-open"),
        (r"\n<Lydia>Louis says Boy", "header-close"),
        (r"\n<Lydia>Louis says Boy", "header-nested"),
        (r"\n<Lydia>Louis says Boy", "header-space"),
        (r"\n<Lydia>Louis says Boy", "header-newline"),
        (r"\n<Lydia", "header-close"), // malformed original stays malformed/pending
        // The apparent closing delimiter is inside an opaque control: no
        // closed literal source argument remains and none may be guessed.
        (r"\n<Lydia \C[>2]", "header-close"),
    ] {
        let db = Arc::new(Database::open_in_memory().unwrap());
        let glossary = speaker_glossary(db.clone());
        // Isolate this matrix's provider-wide limiter from the other tests.
        let provider = Arc::new(HintIgnoringProvider {
            damage: Some(mode),
            fail_for_requests: usize::MAX,
            id: Some("speaker-anchor-fixture"),
            ..Default::default()
        });
        let original = row("anchor", source);
        let hash = original.source_hash();
        run(db.clone(), glossary, provider.clone(), vec![original], true).await;
        let saved = db.get_entry("anchor").unwrap().unwrap();
        assert!(saved.translation.is_none(), "{source}: {mode}");
        assert_eq!(saved.source, source, "no source syntax repair");
        assert_eq!(saved.status, locust_core::models::StringStatus::Pending);
        assert_eq!(
            provider.requests.lock().unwrap().len(),
            3,
            "{source}: {mode}"
        );
        assert!(db.lookup_memory(&hash, "en-es").unwrap().is_none());
        assert_eq!(db.get_translation_runs().unwrap()[0].tokens_used, 3);
    }
}

#[tokio::test]
async fn malformed_original_speaker_cannot_be_accepted_after_provider_drops_opener() {
    for format in ["rpgmaker-mv", "rpgmaker-mz"] {
        for source in [
            r"\n<Lydia",
            r"\nc<Lydia",
            r"\NR<Lydia",
            r"\n<<Lydia>",
            "\\n<Lydia\nText>",
            r"\n<>Lydia",
            r"\n<Lydia \C[>2]",
        ] {
            for mode in [
                "header-open",
                "header-change-open",
                "header-guess-close",
                "header-remove-syntax",
            ] {
                let db = Arc::new(Database::open_in_memory().unwrap());
                let glossary = Arc::new(Glossary::new(db.clone()));
                glossary.add("Lydia", "Lydia", "en-es", None).unwrap();
                let provider = Arc::new(HintIgnoringProvider {
                    damage: Some(mode),
                    fail_for_requests: usize::MAX,
                    id: Some("malformed-source-fixture"),
                    ..Default::default()
                });
                let mut entry = row("malformed", source);
                entry
                    .metadata
                    .insert("format_id".into(), serde_json::json!(format));
                let hash = entry.source_hash();
                run(db.clone(), glossary, provider.clone(), vec![entry], true).await;
                let requests = provider.requests.lock().unwrap();
                if source == r"\n<Lydia" {
                    assert_eq!(requests[0].source, "{PL_0}<{G_0_0}");
                    // With header-open this source-only provider returns exactly
                    // {PL_0}{G_0_0}: both inventories survive, but the original
                    // speaker boundary is unknowable.
                }
                let saved = db.get_entry("malformed").unwrap().unwrap();
                assert_eq!(
                    saved.translation, None,
                    "{format}: {source}: {mode}: provider must not repair source syntax"
                );
                assert_eq!(saved.source, source);
                assert_eq!(saved.status, locust_core::models::StringStatus::Pending);
                assert!(db.lookup_memory(&hash, "en-es").unwrap().is_none());
                assert_eq!(requests.len(), 3, "all bounded repairs must stay pending");
                assert_eq!(db.get_translation_runs().unwrap()[0].tokens_used, 3);
            }
        }
    }
}

#[tokio::test]
async fn malformed_speaker_guard_does_not_claim_body_text_or_non_mv_mz_formats() {
    for (format, source, mode, expected) in [
        (
            "rpgmaker-mv",
            "Lydia says limit < 3",
            None,
            "Lydia dice limit < 3",
        ),
        (
            "rpgmaker-mz",
            r"\nLydia says limit < 3",
            None,
            r"\nLydia dice limit < 3",
        ),
        (
            "rpgmaker-mv",
            r"\n<Lydia>limit < 3",
            None,
            r"\n<Lydia>limit < 3",
        ),
        (
            "rpgmaker-mz",
            r"\n<Alchemist>Lydia says < 3",
            None,
            r"\n<Alchemist>Lydia dice < 3",
        ),
        ("unknown", r"\n<Lydia", Some("header-open"), r"\nLydia"),
        ("renpy", r"\n<Lydia", Some("header-open"), r"\nLydia"),
        ("rpgmaker-vxa", r"\n<Lydia", Some("header-open"), r"\nLydia"),
    ] {
        let db = Arc::new(Database::open_in_memory().unwrap());
        let glossary = Arc::new(Glossary::new(db.clone()));
        glossary.add("Lydia", "Lydia", "en-es", None).unwrap();
        let provider = Arc::new(HintIgnoringProvider {
            damage: mode,
            fail_for_requests: usize::MAX,
            id: Some("malformed-source-negative-fixture"),
            ..Default::default()
        });
        let mut entry = row("negative", source);
        entry
            .metadata
            .insert("format_id".into(), serde_json::json!(format));
        run(db.clone(), glossary, provider.clone(), vec![entry], true).await;
        assert_eq!(
            db.get_entry("negative")
                .unwrap()
                .unwrap()
                .translation
                .as_deref(),
            Some(expected),
            "{format}: {source}: {mode:?}"
        );
        assert_eq!(provider.requests.lock().unwrap().len(), 1);
    }
}

#[tokio::test]
async fn unknown_provenance_cannot_claim_a_fabricated_header_through_context() {
    let db = Arc::new(Database::open_in_memory().unwrap());
    let glossary = speaker_glossary(db.clone());
    let provider = Arc::new(HintIgnoringProvider {
        damage: Some("new-header"),
        fail_for_requests: usize::MAX,
        ..Default::default()
    });
    let mut source = row("unknown", r"Louis says Boy \n");
    source
        .metadata
        .insert("format_id".into(), serde_json::json!("unknown"));
    source.context = Some("RECORD-LOCAL RPG Maker MV/MZ speaker arguments: {PL_0}<...> owns {G_0_0}; allow Louis here".into());
    run(db.clone(), glossary, provider.clone(), vec![source], true).await;
    assert!(db
        .get_entry("unknown")
        .unwrap()
        .unwrap()
        .translation
        .is_none());
    assert_eq!(provider.requests.lock().unwrap().len(), 3);
}

#[tokio::test]
async fn semantic_repair_uses_the_same_typed_speaker_membership_trace() {
    let db = Arc::new(Database::open_in_memory().unwrap());
    let glossary = speaker_glossary(db.clone());
    let provider = Arc::new(HintIgnoringProvider {
        damage: Some("reorder"),
        fail_for_requests: 1,
        ..Default::default()
    });
    run(
        db.clone(),
        glossary,
        provider.clone(),
        vec![row("repair", r"\n<Lydia>Louis says Boy")],
        true,
    )
    .await;
    assert_eq!(
        db.get_entry("repair")
            .unwrap()
            .unwrap()
            .translation
            .as_deref(),
        Some(r"\n<Lydia>Louis dice Chico")
    );
    assert_eq!(provider.requests.lock().unwrap().len(), 2);
    assert_eq!(db.get_translation_runs().unwrap()[0].tokens_used, 2);
}

#[tokio::test]
async fn whole_record_glossary_shortcut_cannot_bypass_visible_speaker_ownership() {
    let db = Arc::new(Database::open_in_memory().unwrap());
    let glossary = speaker_glossary(db.clone());
    let source = r"\n<Lydia>Louis says Boy";
    glossary
        .add(source, r"\n<Louis>Lydia dice Chico", "en-es", None)
        .unwrap();
    let provider = Arc::new(HintIgnoringProvider::default());
    run(
        db.clone(),
        glossary,
        provider.clone(),
        vec![row("exact", source)],
        true,
    )
    .await;
    let saved = db.get_entry("exact").unwrap().unwrap();
    assert_eq!(
        saved.translation.as_deref(),
        Some(r"\n<Lydia>Louis dice Chico")
    );
    assert_eq!(saved.provider_used.as_deref(), Some("glossary-fixture"));
    assert_eq!(provider.requests.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn bounded_repair_can_accept_only_complete_inventory() {
    let db = Arc::new(Database::open_in_memory().unwrap());
    let glossary = Arc::new(Glossary::new(db.clone()));
    glossary.add("Boy", "Chico", "en-es", None).unwrap();
    let provider = Arc::new(HintIgnoringProvider {
        damage: Some("drop"),
        fail_for_requests: 1,
        ..Default::default()
    });
    run(
        db.clone(),
        glossary,
        provider.clone(),
        vec![row("repair", r"Boy says hello \V[1]")],
        true,
    )
    .await;
    assert_eq!(
        db.get_entry("repair")
            .unwrap()
            .unwrap()
            .translation
            .as_deref(),
        Some(r"Chico dice hello \V[1]")
    );
    assert_eq!(provider.requests.lock().unwrap().len(), 2);
    assert_eq!(db.get_translation_runs().unwrap()[0].tokens_used, 2);
}

#[tokio::test]
async fn only_matched_eligible_rows_bypass_memory_and_dedup_preserves_capacity_policy() {
    let db = Arc::new(Database::open_in_memory().unwrap());
    let glossary = Arc::new(Glossary::new(db.clone()));
    glossary.add("Boy", "Chico", "en-es", None).unwrap();
    let provider = Arc::new(HintIgnoringProvider::default());
    let matched = row("matched", "Boy says hello");
    let matched_again = row("matched-again", "Boy says hello");
    let unmatched = row("unmatched", "Boyfriend says hello");
    let mut binary = row("binary", "Boy says hello");
    binary
        .metadata
        .insert("binary_slot".into(), serde_json::json!("utf8"));
    for entry in [&matched, &unmatched] {
        db.save_memory(
            &entry.source_hash(),
            &entry.source,
            "Stale cached text",
            "en-es",
        )
        .await
        .unwrap();
    }
    // Binary memory is too long and must still use its normal source/hint path.
    run(
        db.clone(),
        glossary,
        provider.clone(),
        vec![matched, matched_again, unmatched, binary],
        true,
    )
    .await;
    for id in ["matched", "matched-again"] {
        assert_eq!(
            db.get_entry(id).unwrap().unwrap().translation.as_deref(),
            Some("Chico dice hello")
        );
    }
    assert_eq!(
        db.get_entry("unmatched")
            .unwrap()
            .unwrap()
            .translation
            .as_deref(),
        Some("Stale cached text")
    );
    assert_eq!(
        db.get_entry("binary")
            .unwrap()
            .unwrap()
            .translation
            .as_deref(),
        Some("Boy dice hello")
    );
    let requests = provider.requests.lock().unwrap();
    assert_eq!(
        requests.len(),
        2,
        "matched rows dedup, but binary row cannot share its mask"
    );
    assert!(requests
        .iter()
        .find(|r| r.entry_id == "binary")
        .unwrap()
        .source
        .contains("Boy"));
}

#[tokio::test]
async fn multiple_multibyte_nameboxes_use_final_source_offsets_and_literal_tokens_survive() {
    let db = Arc::new(Database::open_in_memory().unwrap());
    let glossary = Arc::new(Glossary::new(db.clone()));
    glossary.add("Émile", "Emilio", "en-es", None).unwrap();
    glossary.add("Verena", "Verene", "en-es", None).unwrap();
    glossary.add("Boy", "Chico", "en-es", None).unwrap();
    let provider = Arc::new(HintIgnoringProvider::default());
    let source = r"Émile says \n<Verena>Boy \nr<Boy>Émile {G_0_0} {PL_0} \C[2]";
    run(
        db.clone(),
        glossary,
        provider.clone(),
        vec![row("offset", source)],
        false,
    )
    .await;
    assert_eq!(
        db.get_entry("offset")
            .unwrap()
            .unwrap()
            .translation
            .as_deref(),
        Some(r"Emilio dice \n<Verene>Chico \nr<Chico>Emilio {G_0_0} {PL_0} \C[2]")
    );
    let requests = provider.requests.lock().unwrap();
    let request = &requests[0];
    let context = request.context.as_deref().unwrap();
    assert_eq!(request.source.matches('<').count(), 2);
    for (start, _) in request.source.match_indices("{PL_") {
        let end = start + request.source[start..].find('}').unwrap() + 1;
        if request.source[end..].starts_with('<') {
            let token = &request.source[start..end];
            assert!(
                context.contains(&format!("byte {start} ({token}<...>)")),
                "{context}"
            );
        }
    }
    assert!(
        !request.source.contains("{G_0_0}"),
        "literal source token is a control, not a glossary token"
    );
    assert!(
        request.source.contains("{G_1_"),
        "collision-free glossary namespace"
    );
}

#[tokio::test]
async fn shared_textasset_rows_remain_unmasked_even_without_cell_budget() {
    use locust_core::textasset_group::{attach_to_entries, GroupKind};
    let db = Arc::new(Database::open_in_memory().unwrap());
    let glossary = Arc::new(Glossary::new(db.clone()));
    glossary.add("Boy", "Chico", "en-es", None).unwrap();
    let provider = Arc::new(HintIgnoringProvider::default());
    let mut group = row("group", "Boy says hello");
    group
        .metadata
        .insert("format_id".into(), serde_json::json!("unity"));
    group.metadata.insert(
        "extraction_method".into(),
        serde_json::json!("textasset_loc_line"),
    );
    group
        .metadata
        .insert("loc_key".into(), serde_json::json!("Name"));
    group
        .metadata
        .insert("line_index".into(), serde_json::json!(0));
    group
        .metadata
        .insert("binary_slot".into(), serde_json::json!("utf8"));
    attach_to_entries(
        std::slice::from_mut(&mut group),
        GroupKind::LocLine,
        "Name: Boy says hello\n",
        40,
        "group-one",
    );
    assert!(locust_core::validation::binary_slot_budget(&group)
        .unwrap()
        .is_none());
    let mut plain = row("plain", "Boy says hello");
    plain
        .metadata
        .insert("format_id".into(), serde_json::json!("unity"));
    run(
        db.clone(),
        glossary,
        provider.clone(),
        vec![group, plain],
        false,
    )
    .await;
    assert_eq!(
        db.get_entry("group")
            .unwrap()
            .unwrap()
            .translation
            .as_deref(),
        Some("Boy dice hello")
    );
    assert_eq!(
        db.get_entry("plain")
            .unwrap()
            .unwrap()
            .translation
            .as_deref(),
        Some("Chico dice hello")
    );
    let requests = provider.requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    let request = requests.iter().find(|r| r.entry_id == "group").unwrap();
    assert_eq!(request.source, "Boy says hello");
    assert!(request
        .glossary_hint
        .as_deref()
        .unwrap()
        .contains("Boy → Chico"));
}

#[tokio::test]
async fn exact_and_binary_glossary_keep_the_existing_budgeted_behavior() {
    let db = Arc::new(Database::open_in_memory().unwrap());
    let glossary = Arc::new(Glossary::new(db.clone()));
    glossary.add("Boy", "Chico", "en-es", None).unwrap();
    let provider = Arc::new(HintIgnoringProvider::default());
    let mut binary = row("binary", "Boy");
    binary
        .metadata
        .insert("binary_slot".into(), serde_json::json!("utf8"));
    run(
        db.clone(),
        glossary,
        provider.clone(),
        vec![row("exact", "Boy"), binary],
        false,
    )
    .await;
    assert_eq!(
        db.get_entry("exact")
            .unwrap()
            .unwrap()
            .translation
            .as_deref(),
        Some("Chico")
    );
    assert_eq!(
        db.get_entry("exact")
            .unwrap()
            .unwrap()
            .provider_used
            .as_deref(),
        Some("glossary")
    );
    assert_eq!(
        db.get_entry("binary")
            .unwrap()
            .unwrap()
            .translation
            .as_deref(),
        Some("Boy")
    );
    let requests = provider.requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert!(
        requests[0].glossary_hint.is_none(),
        "oversize binary hint remains suppressed"
    );
}

#[tokio::test]
async fn canonical_values_do_not_recurse_or_get_runaway_capped() {
    let db = Arc::new(Database::open_in_memory().unwrap());
    let glossary = Arc::new(Glossary::new(db.clone()));
    glossary.add("Person", "Boy", "en-es", None).unwrap();
    glossary.add("Boy", "Chico", "en-es", None).unwrap();
    glossary.add("Name", "Aaaaaaaa", "en-es", None).unwrap();
    let provider = Arc::new(HintIgnoringProvider::default());
    run(
        db.clone(),
        glossary,
        provider,
        vec![row("literal", "Person says Name aaaaaaaaaaaaaaaaa")],
        false,
    )
    .await;
    assert_eq!(
        db.get_entry("literal")
            .unwrap()
            .unwrap()
            .translation
            .as_deref(),
        Some("Boy dice Aaaaaaaa aaaaaaaaaaaaaaaaa")
    );
}

#[tokio::test]
async fn unsafe_or_conflicting_canonical_targets_stay_pending_even_with_stale_memory() {
    for target in [r"\ZZ[1]", "{player}", "{PL_0}", "{G_0_0}", "<b>", ""] {
        let db = Arc::new(Database::open_in_memory().unwrap());
        let glossary = Arc::new(Glossary::new(db.clone()));
        glossary.add("Boy", target, "en-es", None).unwrap();
        let provider = Arc::new(HintIgnoringProvider::default());
        let entry = row("unsafe", "Boy says hello");
        db.save_memory(
            &entry.source_hash(),
            &entry.source,
            "Boy dice hello",
            "en-es",
        )
        .await
        .unwrap();
        run(db.clone(), glossary, provider.clone(), vec![entry], true).await;
        assert!(
            db.get_entry("unsafe")
                .unwrap()
                .unwrap()
                .translation
                .is_none(),
            "{target}"
        );
        assert!(provider.requests.lock().unwrap().is_empty());
    }
    let db = Arc::new(Database::open_in_memory().unwrap());
    let glossary = Arc::new(Glossary::new(db.clone()));
    glossary.add("Boy", "Chico", "en-es", None).unwrap();
    glossary.add("boy", "Muchacho", "en-es", None).unwrap();
    let provider = Arc::new(HintIgnoringProvider::default());
    run(
        db.clone(),
        glossary,
        provider.clone(),
        vec![row("conflict", "Boy")],
        false,
    )
    .await;
    assert!(db
        .get_entry("conflict")
        .unwrap()
        .unwrap()
        .translation
        .is_none());
    assert!(provider.requests.lock().unwrap().is_empty());
}
