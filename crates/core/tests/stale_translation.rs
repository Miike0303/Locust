use locust_core::database::Database;
use locust_core::models::{
    StringEntry, StringStatus, TranslationResult, ValidationKind, STALE_TRANSLATION_METADATA_KEY,
};
use locust_core::validation::Validator;
use std::sync::Arc;

struct LocalAnswer(&'static str);

#[async_trait::async_trait]
impl locust_core::translation::TranslationProvider for LocalAnswer {
    fn id(&self) -> &str {
        "local-stale-fixture"
    }
    fn name(&self) -> &str {
        "Local stale fixture"
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
    async fn health_check(&self) -> locust_core::error::Result<()> {
        Ok(())
    }
    async fn translate(
        &self,
        requests: &[locust_core::models::TranslationRequest],
    ) -> locust_core::error::Result<Vec<TranslationResult>> {
        Ok(requests
            .iter()
            .map(|request| {
                assert_eq!(
                    request.source, "海へ行く",
                    "provider must receive current source"
                );
                saved_result(&request.entry_id, self.0)
            })
            .collect())
    }
}

fn entry(source: &str) -> StringEntry {
    let mut row = StringEntry::new("row", source, "story.html".into());
    row.metadata
        .insert("unrelated".into(), serde_json::json!(["preserve", 7]));
    row
}

async fn stale(db: &Database) {
    db.save_entries(&[entry("村へ行く")]).unwrap();
    db.save_translation("row", "Go to village", "manual")
        .await
        .unwrap();
    db.merge_entries(&[entry("海へ行く")]).unwrap();
}

fn saved_result(id: &str, text: &str) -> TranslationResult {
    TranslationResult {
        entry_id: id.into(),
        translation: text.into(),
        detected_source_lang: None,
        provider: "local".into(),
        tokens_used: None,
        input_tokens: None,
        output_tokens: None,
        cost_usd: None,
    }
}

#[tokio::test]
async fn stale_marker_survives_repeat_merge_reopen_further_edits_and_source_reversion() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("project.db");
    let db = Database::open(&path).unwrap();
    stale(&db).await;
    let first_hash = entry("村へ行く").source_hash();
    for source in ["海へ行く", "山へ行く", "村へ行く"] {
        db.merge_entries_preserving_missing(&[entry(source)])
            .unwrap();
        db.reopen(&path).unwrap();
        let row = db.get_entry("row").unwrap().unwrap();
        assert_eq!(row.translation.as_deref(), Some("Go to village"));
        assert_eq!(
            row.metadata[STALE_TRANSLATION_METADATA_KEY]["translated_source_sha256"],
            first_hash
        );
        assert_eq!(
            row.metadata[STALE_TRANSLATION_METADATA_KEY]["current_source_sha256"],
            row.source_hash()
        );
        assert!(row
            .require_current_translation()
            .unwrap_err()
            .contains("stale translation"));
        assert_eq!(
            row.metadata["unrelated"],
            serde_json::json!(["preserve", 7])
        );
        let issues = Validator::validate_entry(&row);
        assert!(issues
            .iter()
            .any(|issue| matches!(issue.kind, ValidationKind::StaleTranslation)));
    }
    let output = temp.path().join("pivot.db");
    assert!(db
        .pivot_to(&output)
        .unwrap_err()
        .to_string()
        .contains("stale translation"));
    assert!(!output.exists(), "pivot must reject before output creation");
    let report = Validator::validate_and_save(&[db.get_entry("row").unwrap().unwrap()], &db)
        .await
        .unwrap();
    assert_eq!(report.by_kind["StaleTranslation"], 1);
    assert!(db
        .get_validation_issues(None)
        .unwrap()
        .iter()
        .any(|issue| matches!(issue.kind, ValidationKind::StaleTranslation)));
}

#[tokio::test]
async fn successful_save_paths_and_explicit_review_clear_only_stale_metadata() {
    for mode in ["manual", "batch", "provider", "reviewed", "approved"] {
        let db = Database::open_in_memory().unwrap();
        stale(&db).await;
        match mode {
            "manual" => {
                db.save_translation("row", "Go to sea", "manual")
                    .await
                    .unwrap();
            }
            "batch" => {
                db.save_translations_batch(vec![("row".into(), "Go to sea".into())], "import")
                    .await
                    .unwrap();
            }
            "provider" => {
                db.save_translation_results(&[saved_result("row", "Go to sea")])
                    .await
                    .unwrap();
            }
            "reviewed" => {
                db.update_entry_status("row", StringStatus::Reviewed)
                    .await
                    .unwrap();
            }
            _ => {
                db.update_entry_status("row", StringStatus::Approved)
                    .await
                    .unwrap();
            }
        }
        db.merge_entries(&[entry("海へ行く")]).unwrap();
        let row = db.get_entry("row").unwrap().unwrap();
        assert!(row.require_current_translation().is_ok(), "{mode}");
        assert!(!row.metadata.contains_key(STALE_TRANSLATION_METADATA_KEY));
        assert_eq!(
            row.metadata["unrelated"],
            serde_json::json!(["preserve", 7])
        );
    }
}

#[tokio::test]
async fn failed_empty_and_status_only_updates_do_not_accept_stale_translation() {
    for status in [
        StringStatus::Pending,
        StringStatus::Error,
        StringStatus::Translated,
    ] {
        let db = Database::open_in_memory().unwrap();
        stale(&db).await;
        db.update_entry_status("row", status).await.unwrap();
        assert!(db
            .get_entry("row")
            .unwrap()
            .unwrap()
            .require_current_translation()
            .is_err());
    }
    for mode in ["manual", "batch", "provider"] {
        let db = Database::open_in_memory().unwrap();
        stale(&db).await;
        let empty = " \t\n\u{3000}";
        match mode {
            "manual" => {
                db.save_translation("row", empty, "manual").await.unwrap();
            }
            "batch" => {
                db.save_translations_batch(vec![("row".into(), empty.into())], "import")
                    .await
                    .unwrap();
            }
            _ => {
                db.save_translation_results(&[saved_result("row", empty)])
                    .await
                    .unwrap();
            }
        }
        assert!(
            db.get_entry("row")
                .unwrap()
                .unwrap()
                .require_current_translation()
                .is_err(),
            "{mode}"
        );
    }
    let db = Database::open_in_memory().unwrap();
    stale(&db).await;
    // An invalid provider batch rolls back its earlier valid row and its marker removal.
    assert!(db
        .save_translation_results(&[
            saved_result("row", "Sea"),
            saved_result("missing", "Missing")
        ])
        .await
        .is_err());
    let row = db.get_entry("row").unwrap().unwrap();
    assert!(row.require_current_translation().is_err());
    assert_eq!(row.translation.as_deref(), Some("Go to village"));
}

#[tokio::test]
async fn malformed_markers_fail_closed_until_explicit_acceptance_and_legacy_rows_remain_valid() {
    for marker in [
        serde_json::Value::Null,
        serde_json::json!(false),
        serde_json::json!({"version":9}),
        serde_json::json!({"version":1,"translated_source_sha256":"bad","current_source_sha256":"bad"}),
    ] {
        let db = Database::open_in_memory().unwrap();
        let mut row = entry("Original");
        row.translation = Some("Translated".into());
        row.metadata
            .insert(STALE_TRANSLATION_METADATA_KEY.into(), marker);
        db.save_entries(&[row]).unwrap();
        db.merge_entries(&[entry("Original")]).unwrap();
        let row = db.get_entry("row").unwrap().unwrap();
        assert!(row
            .require_current_translation()
            .unwrap_err()
            .contains("malformed"));
        assert!(Validator::validate_entry(&row)
            .iter()
            .any(|issue| matches!(issue.kind, ValidationKind::StaleTranslation)));
        db.save_translation("row", "Accepted current text", "manual")
            .await
            .unwrap();
        assert!(db
            .get_entry("row")
            .unwrap()
            .unwrap()
            .require_current_translation()
            .is_ok());
    }
    for status in [
        StringStatus::Pending,
        StringStatus::Error,
        StringStatus::Translated,
    ] {
        let mut legacy = entry("Legacy source");
        legacy.translation = Some("Manual legacy translation".into());
        legacy.status = status;
        assert!(legacy.require_current_translation().is_ok());
    }
}

#[tokio::test]
async fn rejected_provider_answer_keeps_marker_then_valid_retranslation_clears_it() {
    use locust_core::translation::{load_pending_entries, TranslationManager, TranslationOptions};
    let db = Arc::new(Database::open_in_memory().unwrap());
    stale(&db).await;
    let original_marker =
        db.get_entry("row").unwrap().unwrap().metadata[STALE_TRANSLATION_METADATA_KEY].clone();
    let glossary = Arc::new(locust_core::glossary::Glossary::new(db.clone()));
    for answer in ["", "Go to sea"] {
        let manager =
            TranslationManager::new(Arc::new(LocalAnswer(answer)), db.clone(), glossary.clone());
        let (tx, mut rx) = tokio::sync::mpsc::channel(64);
        manager
            .translate_entries(
                load_pending_entries(&db).unwrap(),
                TranslationOptions {
                    use_memory: false,
                    use_glossary: false,
                    ..Default::default()
                },
                tx,
                "stale-retranslation".into(),
                tokio_util::sync::CancellationToken::new(),
            )
            .await
            .unwrap();
        let row = db.get_entry("row").unwrap().unwrap();
        if answer.is_empty() {
            assert_eq!(
                row.metadata[STALE_TRANSLATION_METADATA_KEY],
                original_marker
            );
            assert_eq!(row.translation.as_deref(), Some("Go to village"));
            let mut rejected = false;
            while let Ok(event) = rx.try_recv() {
                rejected |= matches!(
                    event,
                    locust_core::models::ProgressEvent::BatchFailed { .. }
                );
            }
            assert!(
                rejected,
                "empty provider response must be reported as invalid"
            );
        } else {
            assert!(row.require_current_translation().is_ok());
            assert_eq!(row.translation.as_deref(), Some(answer));
        }
    }
}
