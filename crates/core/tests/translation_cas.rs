use async_trait::async_trait;
use locust_core::{
    database::{Database, EntryFilter},
    error::Result,
    glossary::Glossary,
    models::{
        ProgressEvent, StringEntry, TranslationRequest, TranslationResult,
        STALE_TRANSLATION_METADATA_KEY,
    },
    translation::{TranslationManager, TranslationOptions, TranslationProvider},
};
use std::{path::PathBuf, sync::Arc};
use tokio::sync::{mpsc, Notify};
use tokio_util::sync::CancellationToken;

struct GatedProvider {
    entered: Arc<Notify>,
    resume: Arc<Notify>,
}
#[async_trait]
impl TranslationProvider for GatedProvider {
    fn id(&self) -> &str {
        "cas-provider"
    }
    fn name(&self) -> &str {
        "Neutral gated provider"
    }
    fn is_free(&self) -> bool {
        false
    }
    fn requires_api_key(&self) -> bool {
        false
    }
    async fn translate(&self, requests: &[TranslationRequest]) -> Result<Vec<TranslationResult>> {
        self.entered.notify_one();
        self.resume.notified().await;
        Ok(requests
            .iter()
            .map(|r| TranslationResult {
                entry_id: r.entry_id.clone(),
                translation: format!("Translated {}", r.entry_id),
                detected_source_lang: None,
                provider: self.id().into(),
                tokens_used: Some(7),
                input_tokens: Some(3),
                output_tokens: Some(4),
                cost_usd: Some(0.125),
            })
            .collect())
    }
    async fn estimate_cost(&self, _: usize, _: &str) -> Option<f64> {
        Some(0.0)
    }
    async fn health_check(&self) -> Result<()> {
        Ok(())
    }
}

fn entries() -> Vec<StringEntry> {
    vec![
        StringEntry::new("a", "Original one", PathBuf::from("game.txt")),
        StringEntry::new("b", "Original two", PathBuf::from("game.txt")),
    ]
}

fn grouped() -> Vec<StringEntry> {
    let mut entries = entries();
    for (i, entry) in entries.iter_mut().enumerate() {
        entry.metadata.insert(
            "extraction_method".into(),
            serde_json::json!("textasset_loc_line"),
        );
        entry.metadata.insert(
            "loc_key".into(),
            serde_json::json!(if i == 0 { "Menu.A" } else { "Menu.B" }),
        );
        entry
            .metadata
            .insert("line_index".into(), serde_json::json!(i));
        entry
            .metadata
            .insert("binary_slot".into(), serde_json::json!("utf8"));
    }
    let original = "Menu.A: Original one\nMenu.B: Original two\n";
    locust_core::textasset_group::attach_to_entries(
        &mut entries,
        locust_core::textasset_group::GroupKind::LocLine,
        original,
        original.len(),
        "cas-group",
    );
    entries
}

async fn additional_conflict(kind: &str) {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("project.db");
    let db = Arc::new(Database::open(&path).unwrap());
    let second = Database::open(&path).unwrap();
    let original = if kind == "group" {
        grouped()
    } else {
        entries()
    };
    db.save_entries(&original).unwrap();
    let requested = db.get_entries(&EntryFilter::default()).unwrap();
    let entered = Arc::new(Notify::new());
    let resume = Arc::new(Notify::new());
    let provider = Arc::new(GatedProvider {
        entered: entered.clone(),
        resume: resume.clone(),
    });
    let manager =
        TranslationManager::new(provider, db.clone(), Arc::new(Glossary::new(db.clone())));
    let (tx, mut rx) = mpsc::channel(100);
    let task = tokio::spawn(async move {
        manager
            .translate_entries(
                requested,
                TranslationOptions {
                    use_glossary: false,
                    ..Default::default()
                },
                tx,
                "cas".into(),
                CancellationToken::new(),
            )
            .await
    });
    tokio::time::timeout(std::time::Duration::from_secs(5), entered.notified())
        .await
        .unwrap();
    match kind {
        "manual" => {
            second
                .save_translation("b", "Human revision", "manual")
                .await
                .unwrap();
        }
        "review" => {
            second
                .update_entry_status("b", locust_core::models::StringStatus::Reviewed)
                .await
                .unwrap();
        }
        "missing" => {
            second.merge_entries(&original[..1]).unwrap();
        }
        _ => {
            let mut changed = original;
            changed[1].source = "Changed source".into();
            second.merge_entries(&changed).unwrap();
        }
    }
    let before = second.get_entry("b").unwrap();
    resume.notify_one();
    let error = tokio::time::timeout(std::time::Duration::from_secs(5), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap_err();
    assert!(
        error.to_string().contains("translation_conflict"),
        "{kind}: {error}"
    );
    assert!(
        second
            .get_entry("a")
            .unwrap()
            .unwrap()
            .translation
            .is_none(),
        "{kind}: atomic first row"
    );
    let after = second.get_entry("b").unwrap();
    assert_eq!(
        serde_json::to_value(after).unwrap(),
        serde_json::to_value(before).unwrap(),
        "{kind}: changed row preserved"
    );
    while let Some(event) = rx.recv().await {
        assert!(
            !matches!(
                event,
                ProgressEvent::StringTranslated { .. } | ProgressEvent::Completed { .. }
            ),
            "{kind}: {event:?}"
        );
    }
    let runs = db.get_translation_runs().unwrap();
    assert_eq!(
        runs.len(),
        1,
        "{kind}: observed cost must survive group save errors"
    );
    assert_eq!(runs[0].strings_translated, 0);
    assert_eq!(runs[0].tokens_used, 14);
    assert_eq!(runs[0].cost_usd, 0.25);
}

#[tokio::test]
async fn grouped_batch_conflict_preserves_usage_and_publishes_no_success() {
    additional_conflict("group").await;
}
#[tokio::test]
async fn concurrent_manual_translation_is_not_overwritten() {
    additional_conflict("manual").await;
}
#[tokio::test]
async fn concurrent_review_is_not_overwritten() {
    additional_conflict("review").await;
}
#[tokio::test]
async fn removed_entry_rejects_whole_batch() {
    additional_conflict("missing").await;
}

#[tokio::test]
async fn memory_and_glossary_shortcuts_reject_obsolete_input_snapshots() {
    for kind in ["memory", "glossary"] {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("project.db");
        let db = Arc::new(Database::open(&path).unwrap());
        let second = Database::open(&path).unwrap();
        let original = vec![entries().remove(0)];
        db.save_entries(&original).unwrap();
        db.save_translation("a", "Previous text", "manual")
            .await
            .unwrap();
        let requested = db.get_entries(&EntryFilter::default()).unwrap();
        let glossary = Arc::new(Glossary::new(db.clone()));
        if kind == "memory" {
            db.save_memory(
                &original[0].source_hash(),
                &original[0].source,
                "Cached text",
                "ja-en",
            )
            .await
            .unwrap();
        } else {
            glossary
                .add(&original[0].source, "Glossary text", "ja-en", None)
                .unwrap();
        }
        let mut changed = original;
        changed[0].source = "Current different source".into();
        second.merge_entries(&changed).unwrap();
        let before = second.get_entry("a").unwrap().unwrap();
        let provider = Arc::new(GatedProvider {
            entered: Arc::new(Notify::new()),
            resume: Arc::new(Notify::new()),
        });
        let manager = TranslationManager::new(provider, db.clone(), glossary);
        let (tx, mut rx) = mpsc::channel(100);
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(3),
            manager.translate_entries(
                requested,
                TranslationOptions {
                    use_memory: kind == "memory",
                    use_glossary: kind == "glossary",
                    ..Default::default()
                },
                tx,
                "cached-cas".into(),
                CancellationToken::new(),
            ),
        )
        .await
        .unwrap();
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("translation_conflict"),
            "{kind}"
        );
        assert_eq!(
            serde_json::to_value(second.get_entry("a").unwrap().unwrap()).unwrap(),
            serde_json::to_value(before).unwrap()
        );
        assert!(second
            .get_entry("a")
            .unwrap()
            .unwrap()
            .metadata
            .contains_key(STALE_TRANSLATION_METADATA_KEY));
        while let Some(event) = rx.recv().await {
            assert!(!matches!(
                event,
                ProgressEvent::StringTranslated { .. } | ProgressEvent::Completed { .. }
            ));
        }
        assert!(db.get_translation_runs().unwrap().is_empty());
    }
}

#[tokio::test]
async fn pivot_guard_compares_semantic_request_source_and_preserves_physical_original() {
    let root = tempfile::tempdir().unwrap();
    let source = Database::open(&root.path().join("source.db")).unwrap();
    source
        .save_entries(&[StringEntry::new("a", "日本語", PathBuf::from("game.txt"))])
        .unwrap();
    source
        .save_translation("a", "English semantic source", "manual")
        .await
        .unwrap();
    let pivot_path = root.path().join("pivot.db");
    source.pivot_to(&pivot_path).unwrap();
    let db = Arc::new(Database::open(&pivot_path).unwrap());
    let requested = db.get_entries(&EntryFilter::default()).unwrap();
    assert_eq!(requested[0].source, "English semantic source");
    let original = requested[0].metadata.clone();
    let resume = Arc::new(Notify::new());
    resume.notify_one();
    let provider = Arc::new(GatedProvider {
        entered: Arc::new(Notify::new()),
        resume,
    });
    let manager =
        TranslationManager::new(provider, db.clone(), Arc::new(Glossary::new(db.clone())));
    let (tx, _rx) = mpsc::channel(100);
    manager
        .translate_entries(
            requested,
            TranslationOptions {
                source_lang: "en".into(),
                target_lang: "es".into(),
                use_memory: false,
                use_glossary: false,
                ..Default::default()
            },
            tx,
            "pivot-cas".into(),
            CancellationToken::new(),
        )
        .await
        .unwrap();
    let saved = db.get_entry("a").unwrap().unwrap();
    assert_eq!(saved.source, "English semantic source");
    assert_eq!(saved.translation.as_deref(), Some("Translated a"));
    assert_eq!(saved.metadata, original);
}

#[tokio::test]
async fn conflict_is_terminal_for_fallback_chain_and_does_not_count_external_edit_as_success() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("project.db");
    let db = Arc::new(Database::open(&path).unwrap());
    let second = Database::open(&path).unwrap();
    db.save_entries(&entries()).unwrap();
    let entered = Arc::new(Notify::new());
    let resume = Arc::new(Notify::new());
    let provider: Arc<dyn TranslationProvider> = Arc::new(GatedProvider {
        entered: entered.clone(),
        resume: resume.clone(),
    });
    let resolutions = Arc::new(AtomicUsize::new(0));
    let count = resolutions.clone();
    let manager_db = db.clone();
    let (tx, mut rx) = mpsc::channel(100);
    let task = tokio::spawn(async move {
        let resolve = move |_: &str| {
            count.fetch_add(1, Ordering::SeqCst);
            Some(provider.clone())
        };
        locust_core::translation::run_fallback_chain(
            &["first".into(), "second".into()],
            &resolve,
            manager_db.clone(),
            Arc::new(Glossary::new(manager_db)),
            TranslationOptions {
                use_memory: false,
                use_glossary: false,
                ..Default::default()
            },
            tx,
            "fallback-cas".into(),
            CancellationToken::new(),
        )
        .await
    });
    tokio::time::timeout(std::time::Duration::from_secs(5), entered.notified())
        .await
        .unwrap();
    second
        .save_translation("b", "Manual external success", "manual")
        .await
        .unwrap();
    resume.notify_one();
    let error = tokio::time::timeout(std::time::Duration::from_secs(5), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap_err();
    assert!(error.to_string().contains("translation_conflict"));
    assert_eq!(resolutions.load(Ordering::SeqCst), 1);
    assert!(db.get_entry("a").unwrap().unwrap().translation.is_none());
    assert_eq!(
        db.get_entry("b").unwrap().unwrap().translation.as_deref(),
        Some("Manual external success")
    );
    while let Some(event) = rx.recv().await {
        assert!(!matches!(
            event,
            ProgressEvent::StringTranslated { .. }
                | ProgressEvent::Completed { .. }
                | ProgressEvent::ProviderSwitched { .. }
        ));
    }
    let runs = db.get_translation_runs().unwrap();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].strings_translated, 0);
    assert_eq!(runs[0].cost_usd, 0.25);
}

#[tokio::test]
async fn fresh_automatic_regeneration_clears_stale_after_successful_guarded_commit() {
    let root = tempfile::tempdir().unwrap();
    let db = Arc::new(Database::open(&root.path().join("project.db")).unwrap());
    let mut original = vec![entries().remove(0)];
    db.save_entries(&original).unwrap();
    db.save_translation("a", "Prior translation", "manual")
        .await
        .unwrap();
    original[0].source = "Current semantic source".into();
    db.merge_entries(&original).unwrap();
    let requested = db.get_entries(&EntryFilter::default()).unwrap();
    assert!(requested[0]
        .metadata
        .contains_key(STALE_TRANSLATION_METADATA_KEY));
    let resume = Arc::new(Notify::new());
    resume.notify_one();
    let provider = Arc::new(GatedProvider {
        entered: Arc::new(Notify::new()),
        resume,
    });
    let manager =
        TranslationManager::new(provider, db.clone(), Arc::new(Glossary::new(db.clone())));
    let (tx, mut rx) = mpsc::channel(100);
    manager
        .translate_entries(
            requested,
            TranslationOptions {
                use_glossary: false,
                ..Default::default()
            },
            tx,
            "fresh-cas".into(),
            CancellationToken::new(),
        )
        .await
        .unwrap();
    let row = db.get_entry("a").unwrap().unwrap();
    assert_eq!(row.translation.as_deref(), Some("Translated a"));
    assert!(!row.metadata.contains_key(STALE_TRANSLATION_METADATA_KEY));
    assert_eq!(
        db.lookup_memory(&row.source_hash(), "ja-en")
            .unwrap()
            .as_deref(),
        Some("Translated a")
    );
    let mut published = 0;
    while let Some(event) = rx.recv().await {
        if matches!(event, ProgressEvent::StringTranslated { .. }) {
            published += 1;
        }
    }
    assert_eq!(published, 1);
}

#[tokio::test]
async fn second_connection_source_change_rejects_whole_provider_batch_and_preserves_stale_and_usage(
) {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("project.db");
    let db = Arc::new(Database::open(&path).unwrap());
    let second = Database::open(&path).unwrap();
    let original = entries();
    db.save_entries(&original).unwrap();
    db.save_translation("b", "Prior translation", "manual")
        .await
        .unwrap();
    let requested = db.get_entries(&EntryFilter::default()).unwrap();
    let entered = Arc::new(Notify::new());
    let resume = Arc::new(Notify::new());
    let provider = Arc::new(GatedProvider {
        entered: entered.clone(),
        resume: resume.clone(),
    });
    let manager =
        TranslationManager::new(provider, db.clone(), Arc::new(Glossary::new(db.clone())));
    let (tx, mut rx) = mpsc::channel(100);
    let task = tokio::spawn(async move {
        manager
            .translate_entries(
                requested,
                TranslationOptions {
                    use_glossary: false,
                    ..Default::default()
                },
                tx,
                "cas".into(),
                CancellationToken::new(),
            )
            .await
    });
    tokio::time::timeout(std::time::Duration::from_secs(5), entered.notified())
        .await
        .unwrap();
    let mut changed = original.clone();
    changed[1].source = "Changed while provider was running".into();
    second.merge_entries(&changed).unwrap();
    assert!(second
        .get_entry("b")
        .unwrap()
        .unwrap()
        .metadata
        .contains_key(STALE_TRANSLATION_METADATA_KEY));
    resume.notify_one();
    let outcome = tokio::time::timeout(std::time::Duration::from_secs(5), task)
        .await
        .unwrap()
        .unwrap();
    assert!(
        outcome.is_err(),
        "obsolete provider result must be rejected"
    );
    assert!(outcome
        .unwrap_err()
        .to_string()
        .contains("translation_conflict"));
    let a = second.get_entry("a").unwrap().unwrap();
    let b = second.get_entry("b").unwrap().unwrap();
    assert!(
        a.translation.is_none(),
        "first row must roll back with conflicted second row"
    );
    assert_eq!(b.translation.as_deref(), Some("Prior translation"));
    assert_eq!(b.source, "Changed while provider was running");
    assert!(b.metadata.contains_key(STALE_TRANSLATION_METADATA_KEY));
    while let Some(event) = rx.recv().await {
        assert!(
            !matches!(
                event,
                ProgressEvent::StringTranslated { .. } | ProgressEvent::Completed { .. }
            ),
            "{event:?}"
        );
    }
    for entry in original {
        assert!(db
            .lookup_memory(&entry.source_hash(), "ja-en")
            .unwrap()
            .is_none());
    }
    let runs = db.get_translation_runs().unwrap();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].strings_translated, 0);
    assert_eq!(runs[0].tokens_used, 14);
    assert_eq!(runs[0].input_tokens, 6);
    assert_eq!(runs[0].output_tokens, 8);
    assert_eq!(runs[0].cost_usd, 0.25);
    assert!(runs[0].cost_is_complete);
}
