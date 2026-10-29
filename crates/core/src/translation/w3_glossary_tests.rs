use super::*;
use async_trait::async_trait;
use std::path::PathBuf;

struct NoProvider;

#[async_trait]
impl TranslationProvider for NoProvider {
    fn id(&self) -> &str {
        "unused"
    }
    fn name(&self) -> &str {
        "unused"
    }
    fn is_free(&self) -> bool {
        true
    }
    fn requires_api_key(&self) -> bool {
        false
    }
    async fn translate(&self, _: &[TranslationRequest]) -> Result<Vec<TranslationResult>> {
        panic!("exact glossary hits must never call the provider")
    }
    async fn estimate_cost(&self, _: usize, _: &str) -> Option<f64> {
        Some(0.0)
    }
    async fn health_check(&self) -> Result<()> {
        Ok(())
    }
}

fn entries(count: usize) -> Vec<StringEntry> {
    (0..count)
        .map(|i| StringEntry::new(format!("row-{i}"), "Options", PathBuf::from("ui.json")))
        .collect()
}

fn options(batch_size: usize) -> TranslationOptions {
    TranslationOptions {
        source_lang: "en".into(),
        target_lang: "es".into(),
        batch_size,
        use_memory: false,
        ..Default::default()
    }
}

#[tokio::test]
async fn exact_hits_commit_once_per_batch_and_publish_in_order() {
    let (db, glossary) = super::super::tests::setup();
    glossary.add("Options", "Opcns", "en-es", None).unwrap();
    let entries = entries(7);
    db.save_entries(&entries).unwrap();
    let (tx, mut rx) = mpsc::channel(30);
    crate::database::GUARDED_TRANSLATION_TRANSACTIONS.with(|count| count.set(Some(0)));
    TranslationManager::new(Arc::new(NoProvider), db.clone(), glossary.clone())
        .translate_entries(
            entries.clone(),
            options(3),
            tx,
            "batch-glossary".into(),
            CancellationToken::new(),
        )
        .await
        .unwrap();
    let transactions =
        crate::database::GUARDED_TRANSLATION_TRANSACTIONS.with(|count| count.replace(None));
    assert_eq!(transactions, Some(3));
    assert_eq!(glossary.get_all_call_count(), 1);
    let mut events = Vec::new();
    while let Some(event) = rx.recv().await {
        match event {
            ProgressEvent::StringTranslated {
                entry_id,
                translation,
            } => {
                assert_eq!(translation, "Opcns");
                let saved = db.get_entry(&entry_id).unwrap().unwrap();
                assert_eq!(saved.translation.as_deref(), Some("Opcns"));
                assert_eq!(saved.provider_used.as_deref(), Some("glossary"));
                events.push(entry_id);
            }
            ProgressEvent::Completed {
                total_translated, ..
            } => assert_eq!(total_translated, 7),
            _ => {}
        }
    }
    assert_eq!(
        events,
        entries.into_iter().map(|e| e.id).collect::<Vec<_>>()
    );
    assert_eq!(db.get_translation_runs().unwrap()[0].strings_translated, 7);
}

#[tokio::test]
async fn exact_hit_guard_conflicts_roll_back_batch_without_success_events() {
    for mutation in ["source", "translation", "status", "provider", "missing"] {
        let (db, glossary) = super::super::tests::setup();
        glossary.add("Options", "Opcns", "en-es", None).unwrap();
        let originals = entries(3);
        let mut edited = originals.clone();
        match mutation {
            "source" => edited[1].source = "Changed".into(),
            "translation" => edited[1].translation = Some("User edit".into()),
            "status" => edited[1].status = StringStatus::Approved,
            "provider" => edited[1].provider_used = Some("user".into()),
            "missing" => {
                edited.remove(1);
            }
            _ => unreachable!(),
        }
        db.save_entries(&edited).unwrap();
        let (tx, mut rx) = mpsc::channel(30);
        let error = TranslationManager::new(Arc::new(NoProvider), db.clone(), glossary)
            .translate_entries(
                originals.clone(),
                options(3),
                tx,
                "glossary-conflict".into(),
                CancellationToken::new(),
            )
            .await
            .unwrap_err();
        assert!(
            error.to_string().contains("translation_conflict"),
            "{mutation}: {error}"
        );
        for entry in &edited {
            let saved = db.get_entry(&entry.id).unwrap().unwrap();
            assert_eq!(saved.source, entry.source);
            assert_eq!(saved.translation, entry.translation);
            assert_eq!(saved.status, entry.status);
            assert_eq!(saved.provider_used, entry.provider_used);
        }
        while let Some(event) = rx.recv().await {
            assert!(!matches!(
                event,
                ProgressEvent::StringTranslated { .. } | ProgressEvent::Completed { .. }
            ));
        }
    }
}

#[tokio::test]
async fn failed_later_glossary_batch_keeps_only_earlier_committed_events() {
    let (db, glossary) = super::super::tests::setup();
    glossary.add("Options", "Opcns", "en-es", None).unwrap();
    let originals = entries(5);
    let mut edited = originals.clone();
    edited[3].source = "Changed".into();
    db.save_entries(&edited).unwrap();
    let (tx, mut rx) = mpsc::channel(30);
    let result = TranslationManager::new(Arc::new(NoProvider), db.clone(), glossary)
        .translate_entries(
            originals,
            options(2),
            tx,
            "later-conflict".into(),
            CancellationToken::new(),
        )
        .await;
    assert!(result.is_err());
    let mut events = Vec::new();
    while let Some(event) = rx.recv().await {
        if let ProgressEvent::StringTranslated { entry_id, .. } = event {
            events.push(entry_id);
        }
    }
    assert_eq!(events, ["row-0", "row-1"]);
    assert!(db
        .get_entry("row-2")
        .unwrap()
        .unwrap()
        .translation
        .is_none());
    assert!(db
        .get_entry("row-4")
        .unwrap()
        .unwrap()
        .translation
        .is_none());
}

#[tokio::test]
async fn provider_passes_share_one_glossary_snapshot() {
    let (db, glossary) = super::super::tests::setup();
    glossary.add("Options", "Original", "en-es", None).unwrap();
    let entries = entries(2);
    db.save_entries(&entries).unwrap();
    let manager = TranslationManager::new(Arc::new(NoProvider), db.clone(), glossary.clone());
    let job_glossary = OnceLock::new();
    let notes = CharacterNotes::load(&db).unwrap();
    let (tx, _rx) = mpsc::channel(30);
    let mut cost = ObservedCost::default();
    for entry in entries {
        manager
            .translate_entries_accounted(
                vec![entry],
                options(3),
                tx.clone(),
                "shared-snapshot".into(),
                CancellationToken::new(),
                false,
                &mut cost,
                &notes,
                &job_glossary,
            )
            .await
            .unwrap();
        glossary
            .add("Options", "Replacement", "en-es", None)
            .unwrap();
    }
    assert_eq!(glossary.get_all_call_count(), 1);
    assert_eq!(
        db.get_entry("row-1")
            .unwrap()
            .unwrap()
            .translation
            .as_deref(),
        Some("Original")
    );
}

#[tokio::test]
#[ignore = "synthetic 100k row / 1k term persistence benchmark"]
async fn benchmark_glossary_transactions_100k() {
    let (db, glossary) = super::super::tests::setup();
    for i in 0..1_000 {
        glossary
            .add(
                &format!("Term{i:04}"),
                &format!("Target{i:04}"),
                "en-es",
                None,
            )
            .unwrap();
    }
    let entries: Vec<_> = (0..100_000)
        .map(|i| {
            StringEntry::new(
                format!("row-{i}"),
                format!("Term{:04}", i % 1_000),
                PathBuf::from("ui.json"),
            )
        })
        .collect();
    db.save_entries(&entries).unwrap();
    crate::database::GUARDED_TRANSLATION_TRANSACTIONS.with(|count| count.set(Some(0)));
    let start = Instant::now();
    for entry in &entries {
        db.save_translation_guarded(entry, &format!("Target{}", &entry.source[4..]), "glossary")
            .await
            .unwrap();
    }
    let before = start.elapsed();
    let before_transactions = crate::database::GUARDED_TRANSLATION_TRANSACTIONS
        .with(|count| count.replace(Some(0)))
        .unwrap();
    db.save_entries(&entries).unwrap();
    let (tx, rx) = mpsc::channel(1);
    drop(rx);
    let mut completed = 0;
    let start = Instant::now();
    for batch in entries.chunks(200) {
        let mut hits = batch
            .iter()
            .map(|e| (e.clone(), format!("Target{}", &e.source[4..])))
            .collect();
        save_exact_glossary_batch(&db, &mut hits, &tx, &mut completed)
            .await
            .unwrap();
    }
    let after = start.elapsed();
    let after_transactions = crate::database::GUARDED_TRANSLATION_TRANSACTIONS
        .with(|count| count.replace(None))
        .unwrap();
    assert_eq!(
        (before_transactions, after_transactions, completed),
        (100_000, 500, 100_000)
    );
    assert_eq!(db.get_stats().unwrap().translated, 100_000);
    println!("W3-01 persistence (in-memory SQLite; 100k rows/1k terms; batch 200): before={before:?} after={after:?}, transactions={before_transactions}->{after_transactions}");
}
