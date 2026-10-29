use async_trait::async_trait;
use locust_core::{
    database::Database,
    error::{LocustError, Result},
    glossary::Glossary,
    models::{ProgressEvent, StringEntry, StringStatus, TranslationRequest, TranslationResult},
    placeholder::PlaceholderProcessor,
    translation::{
        run_fallback_chain, TranslationManager, TranslationOptions, TranslationProvider,
    },
};
use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

#[derive(Clone, Copy)]
enum Damage {
    None,
    BaseOnly,
    Always,
    Duplicate,
    Unknown,
    Malformed,
    ExtraControl,
    WrongId,
    MoveSpeaker,
}

struct MtFixture {
    id: &'static str,
    instructions: bool,
    damage: Damage,
    partial: bool,
    calls: Mutex<Vec<Vec<TranslationRequest>>>,
}

impl MtFixture {
    fn new(damage: Damage) -> Arc<Self> {
        Arc::new(Self {
            id: "source-only-fixture",
            instructions: false,
            damage,
            partial: false,
            calls: Mutex::new(Vec::new()),
        })
    }

    fn answer(&self, request: &TranslationRequest) -> TranslationResult {
        let mut text = if request.target_lang == "en" {
            request
                .source
                .replace("こんにちは", "Hello")
                .replace("旅人", "traveler")
                .replace("錬金術師", "Alchemist")
        } else {
            request
                .source
                .replace("Hello", "Hola")
                .replace("traveler", "viajero")
                .replace("Alchemist", "Alquimista")
        };
        let alternate = text.contains("⟦L");
        let tokens: Vec<_> = PlaceholderProcessor::extract(&text)
            .1
            .into_iter()
            .map(|ph| ph.original)
            .filter(|s| s.starts_with("{PL_") || s.starts_with("{G_") || s.starts_with("⟦L"))
            .collect();
        if request.entry_id.starts_with("bad") {
            if let Some(first) = tokens.first() {
                match self.damage {
                    Damage::None => {}
                    Damage::BaseOnly if alternate => {}
                    // Force the initial repair for adversarial alternate answers.
                    _ if !alternate => text = text.replacen(first, "", 1),
                    Damage::BaseOnly | Damage::Always => text = text.replacen(first, "", 1),
                    Damage::Duplicate => text.push_str(first),
                    Damage::Unknown => text.push_str("⟦L999X99⟧"),
                    Damage::Malformed => text.push_str("⟦L0Xoops"),
                    Damage::ExtraControl => text.push_str(r"\V[99]"),
                    Damage::MoveSpeaker => {
                        let canonical = tokens.iter().find(|s| **s != *first).unwrap();
                        text = text.replacen(canonical, "", 1);
                        text.push_str(canonical);
                    }
                    Damage::WrongId => {}
                }
            }
        }
        TranslationResult {
            entry_id: if alternate && matches!(self.damage, Damage::WrongId) {
                "foreign".into()
            } else {
                request.entry_id.clone()
            },
            translation: text,
            detected_source_lang: None,
            provider: self.id.into(),
            tokens_used: Some(3),
            input_tokens: Some(2),
            output_tokens: Some(1),
            cost_usd: Some(0.0),
        }
    }
}

#[async_trait]
impl TranslationProvider for MtFixture {
    fn id(&self) -> &str {
        self.id
    }
    fn name(&self) -> &str {
        "Source-only MT fixture"
    }
    fn is_free(&self) -> bool {
        true
    }
    fn requires_api_key(&self) -> bool {
        false
    }
    fn supports_instructions(&self) -> bool {
        self.instructions
    }
    async fn health_check(&self) -> Result<()> {
        Ok(())
    }
    async fn estimate_cost(&self, _: usize, _: &str) -> Option<f64> {
        Some(0.0)
    }
    async fn translate(&self, requests: &[TranslationRequest]) -> Result<Vec<TranslationResult>> {
        self.calls.lock().unwrap().push(requests.to_vec());
        Ok(requests.iter().map(|r| self.answer(r)).collect())
    }
    async fn translate_partial(
        &self,
        requests: &[TranslationRequest],
        successes: &mut Vec<TranslationResult>,
    ) -> Option<Vec<(String, LocustError)>> {
        if !self.partial {
            return None;
        }
        let first = {
            let mut calls = self.calls.lock().unwrap();
            calls.push(requests.to_vec());
            calls.len() == 1
        };
        let mut failed = Vec::new();
        for request in requests {
            if first && request.entry_id == "transport" {
                failed.push((
                    request.entry_id.clone(),
                    LocustError::ProviderError("503 unavailable".into()),
                ));
            } else {
                successes.push(self.answer(request));
            }
        }
        Some(failed)
    }
}

fn row(id: &str, source: &str, format: &str) -> StringEntry {
    StringEntry::new(id, source, PathBuf::from("fixture.txt")).with_format_id(format)
}

fn options(source: &str, target: &str) -> TranslationOptions {
    TranslationOptions {
        source_lang: source.into(),
        target_lang: target.into(),
        use_memory: true,
        max_concurrent: 1,
        max_batch_tokens: None,
        ..Default::default()
    }
}

async fn run(
    db: Arc<Database>,
    glossary: Arc<Glossary>,
    provider: Arc<MtFixture>,
    entries: Vec<StringEntry>,
    opts: TranslationOptions,
) -> Vec<ProgressEvent> {
    db.save_entries(&entries).unwrap();
    let (tx, mut rx) = mpsc::channel(512);
    TranslationManager::new(provider, db, glossary)
        .translate_entries(entries, opts, tx, "mt".into(), CancellationToken::new())
        .await
        .unwrap();
    let mut events = Vec::new();
    while let Some(event) = rx.recv().await {
        events.push(event);
    }
    events
}

#[tokio::test]
async fn ja_en_es_controls_names_and_literal_lookalikes_survive_alternate_repair() {
    let cases = [
        (
            "bad-renpy",
            "こんにちは [player] {b}旅人{/b}!",
            "renpy",
            "Hello [player] {b}traveler{/b}!",
            "Hola [player] {b}viajero{/b}!",
        ),
        (
            "bad-rpg",
            r"\n<錬金術師>こんにちは \V[1]、\N[2]!",
            "rpgmaker-mv",
            r"\n<Alchemist>Hello \V[1]、\N[2]!",
            r"\n<Alquimista>Hola \V[1]、\N[2]!",
        ),
        (
            "bad-names",
            "リディア様、リディア アンナ アン",
            "renpy",
            "Lady Lydia、Lydia Anna Ann",
            "Dama Lidia、Lidia Anna Ana",
        ),
        (
            "bad-literals",
            "こんにちは {PL_0} {G_0_0} ⟦L0X0⟧ ⟦Lbroken [player]",
            "renpy",
            "Hello {PL_0} {G_0_0} ⟦L0X0⟧ ⟦Lbroken [player]",
            "Hola {PL_0} {G_0_0} ⟦L0X0⟧ ⟦Lbroken [player]",
        ),
    ];
    let mut entries: Vec<_> = cases
        .iter()
        .map(|(id, source, format, _, _)| row(id, source, format))
        .collect();
    for (source, target) in [("ja", "en"), ("en", "es")] {
        let db = Arc::new(Database::open_in_memory().unwrap());
        let glossary = Arc::new(Glossary::new(db.clone()));
        let terms = if source == "ja" {
            vec![
                ("リディア", "Lydia"),
                ("リディア様", "Lady Lydia"),
                ("アン", "Ann"),
                ("アンナ", "Anna"),
            ]
        } else {
            vec![
                ("Lydia", "Lidia"),
                ("Lady Lydia", "Dama Lidia"),
                ("Ann", "Ana"),
                ("Anna", "Anna"),
            ]
        };
        for (term, canonical) in terms {
            glossary
                .add(term, canonical, &format!("{source}-{target}"), None)
                .unwrap();
        }
        let provider = MtFixture::new(Damage::BaseOnly);
        run(
            db.clone(),
            glossary,
            provider.clone(),
            entries,
            options(source, target),
        )
        .await;
        entries = Vec::new();
        for (id, _, format, en, es) in cases {
            let saved = db.get_entry(id).unwrap().unwrap();
            assert_eq!(
                saved.translation.as_deref(),
                Some(if target == "en" { en } else { es }),
                "{source}->{target} {id}"
            );
            assert!(saved.require_preserved_translation_controls().is_ok());
            entries.push(row(id, saved.translation.as_deref().unwrap(), format));
        }
        let calls = provider.calls.lock().unwrap();
        assert_eq!(
            calls.len(),
            5,
            "one batch plus one alternate for each damaged row"
        );
        for retry in &calls[1..] {
            assert_eq!(retry.len(), 1);
            let original = calls[0]
                .iter()
                .find(|r| r.entry_id == retry[0].entry_id)
                .unwrap();
            assert_ne!(retry[0].source, original.source);
            assert!(retry[0].source.contains("⟦L"));
            if retry[0].entry_id == "bad-literals" {
                assert!(!retry[0].source.contains("⟦L0X"));
            }
        }
    }
}

#[tokio::test(start_paused = true)]
async fn partial_outcomes_then_control_failure_fallback_only_resend_affected_ids() {
    let db = Arc::new(Database::open_in_memory().unwrap());
    let glossary = Arc::new(Glossary::new(db.clone()));
    let entries = vec![
        row("good", "Hello [player]", "renpy"),
        row("bad", "Hello {0}", "renpy"),
        row("transport", "Hello {1}", "renpy"),
        row("unselected", "Hello [other]", "renpy"),
    ];
    db.save_entries(&entries).unwrap();
    let primary = Arc::new(MtFixture {
        partial: true,
        ..Arc::try_unwrap(MtFixture::new(Damage::Always))
            .ok()
            .unwrap()
    });
    let fallback = Arc::new(MtFixture {
        id: "fallback",
        instructions: true,
        ..Arc::try_unwrap(MtFixture::new(Damage::None)).ok().unwrap()
    });
    let (tx, mut rx) = mpsc::channel(512);
    run_fallback_chain(
        &[primary.id.into(), "fallback".into()],
        &|id| {
            Some(if id == "fallback" {
                fallback.clone()
            } else {
                primary.clone()
            })
        },
        db.clone(),
        glossary,
        TranslationOptions {
            entry_ids: Some(vec!["good".into(), "bad".into(), "transport".into()]),
            ..options("en", "es")
        },
        tx,
        "partial-mt".into(),
        CancellationToken::new(),
    )
    .await
    .unwrap();
    let mut translated = Vec::new();
    while let Some(event) = rx.recv().await {
        if let ProgressEvent::StringTranslated { entry_id, .. } = event {
            translated.push(entry_id);
        }
    }
    translated.sort();
    assert_eq!(translated, ["bad", "good", "transport"]);
    let calls = primary.calls.lock().unwrap();
    assert_eq!(calls.len(), 3);
    assert_eq!(calls[0].len(), 3);
    assert_eq!(
        calls[1]
            .iter()
            .map(|r| r.entry_id.as_str())
            .collect::<Vec<_>>(),
        ["transport"]
    );
    assert_eq!(
        calls[2]
            .iter()
            .map(|r| r.entry_id.as_str())
            .collect::<Vec<_>>(),
        ["bad"]
    );
    assert!(calls[2][0].source.contains("⟦L"));
    let fallback_calls = fallback.calls.lock().unwrap();
    assert_eq!(fallback_calls.len(), 1);
    assert_eq!(
        fallback_calls[0]
            .iter()
            .map(|r| r.entry_id.as_str())
            .collect::<Vec<_>>(),
        ["bad"]
    );
    assert!(fallback_calls[0][0].source.contains("{PL_"));
    assert_eq!(
        db.get_entry("bad")
            .unwrap()
            .unwrap()
            .provider_used
            .as_deref(),
        Some("fallback")
    );
    assert_eq!(
        db.get_entry("unselected").unwrap().unwrap().status,
        StringStatus::Pending
    );
    assert_eq!(
        db.get_translation_runs()
            .unwrap()
            .iter()
            .map(|r| r.tokens_used)
            .sum::<u64>(),
        15
    );
}

#[tokio::test]
async fn alternate_missing_duplicate_unknown_malformed_extra_controls_and_foreign_ids_stay_pending()
{
    for damage in [
        Damage::Always,
        Damage::Duplicate,
        Damage::Unknown,
        Damage::Malformed,
        Damage::ExtraControl,
        Damage::WrongId,
        Damage::MoveSpeaker,
    ] {
        let db = Arc::new(Database::open_in_memory().unwrap());
        let glossary = Arc::new(Glossary::new(db.clone()));
        glossary.add("Lydia", "Lidia", "en-es", None).unwrap();
        let entry = row("bad", r"\n<Lydia>Hello \V[1]", "rpgmaker-mv");
        let provider = MtFixture::new(damage);
        run(
            db.clone(),
            glossary,
            provider.clone(),
            vec![entry.clone()],
            options("en", "es"),
        )
        .await;
        let saved = db.get_entry("bad").unwrap().unwrap();
        assert_eq!(saved.status, StringStatus::Pending);
        assert!(saved.translation.is_none());
        assert!(db
            .lookup_memory(&entry.source_hash(), "en-es")
            .unwrap()
            .is_none());
        assert_eq!(provider.calls.lock().unwrap().len(), 2);
    }
}
