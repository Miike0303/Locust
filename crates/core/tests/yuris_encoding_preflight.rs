//! Synthetic representability/preflight evidence, not engine/runtime readiness.
use locust_core::{
    backup::BackupManager,
    database::Database,
    error::{LocustError, Result},
    extraction::{inject_direct, FormatPlugin, FormatRegistry, InjectionReport, MultiLangInjector},
    models::{
        OutputMode, StringEntry, StringStatus, YURIS_LITERAL_METADATA_KEY, YURIS_QUOTED_LITERAL_V1,
    },
    validation::{binary_slot_budget, encoded_byte_len, Validator},
};
use serde_json::{json, Value};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
};
use tokio::sync::mpsc;

fn entry(target: &str) -> StringEntry {
    let mut entry = StringEntry::new("story.ybn#attr1", "[カナ]", PathBuf::from("story.ybn"))
        .with_format_id("yuris");
    entry.metadata.insert(
        YURIS_LITERAL_METADATA_KEY.into(),
        json!(YURIS_QUOTED_LITERAL_V1),
    );
    entry.translation = Some(target.into());
    entry.status = StringStatus::Translated;
    entry
}

fn encoding_issues(entry: &StringEntry) -> Vec<Value> {
    Validator::validate_entry(entry)
        .iter()
        .map(|issue| serde_json::to_value(issue).unwrap())
        .filter(|issue| issue["kind"].get("UnencodableTranslation").is_some())
        .collect()
}

#[test]
fn literal_shift_jis_reports_unsupported_accents_without_fitting_or_excerpts() {
    for target in ["Español", "café", "¿Cómo estás?", "😀"] {
        let entry = entry(target);
        let before = serde_json::to_value(&entry).unwrap();
        assert_eq!(encoded_byte_len("shift_jis", target), None);
        let issues = encoding_issues(&entry);
        assert_eq!(issues.len(), 1, "{target}: missing encoding diagnosis");
        assert_eq!(
            issues[0]["kind"],
            json!({"UnencodableTranslation": {"encoding": "Shift-JIS"}})
        );
        assert!(issues[0]["source"].is_null());
        assert!(!issues[0]["message"].as_str().unwrap().contains(target));
        assert_eq!(binary_slot_budget(&entry).unwrap(), None);
        assert_eq!(serde_json::to_value(&entry).unwrap(), before);
    }
}

#[test]
fn encodable_literals_controls_and_long_text_have_no_invented_byte_budget() {
    for target in [
        "Latin [Name]",
        "日本語 [カナ]",
        "line\n\t\\tail\r",
        "[Name] ",
    ] {
        let entry = entry(target);
        assert!(encoded_byte_len("shift_jis", target).is_some());
        assert!(encoding_issues(&entry).is_empty());
        assert_eq!(binary_slot_budget(&entry).unwrap(), None);
    }
    let entry = entry(&"Long literal ".repeat(100));
    assert!(Validator::validate_entry(&entry).is_empty());
    let mut pending = entry.clone();
    pending.translation = None;
    pending.status = StringStatus::Pending;
    assert!(encoding_issues(&pending).is_empty());
}

#[test]
fn encoding_authority_requires_exact_format_and_literal_marker() {
    for (format, marker) in [
        (None, Some(json!(YURIS_QUOTED_LITERAL_V1))),
        (Some(json!("yuris")), None),
        (Some(json!("YU-RIS")), Some(json!(YURIS_QUOTED_LITERAL_V1))),
        (Some(json!("renpy")), Some(json!(YURIS_QUOTED_LITERAL_V1))),
        (Some(json!("yuris")), Some(json!("unknown"))),
        (Some(json!("yuris")), Some(json!([YURIS_QUOTED_LITERAL_V1]))),
        (Some(Value::Null), Some(json!(YURIS_QUOTED_LITERAL_V1))),
    ] {
        let mut entry = entry("Español");
        entry.metadata.remove("format_id");
        entry.metadata.remove(YURIS_LITERAL_METADATA_KEY);
        if let Some(format) = format {
            entry.metadata.insert("format_id".into(), format);
        }
        if let Some(marker) = marker {
            entry
                .metadata
                .insert(YURIS_LITERAL_METADATA_KEY.into(), marker);
        }
        entry.context = Some(YURIS_QUOTED_LITERAL_V1.into());
        entry.tags = vec!["dialogue".into()];
        assert!(encoding_issues(&entry).is_empty());
    }
    let mut unmarked = entry("[Name]");
    unmarked.metadata.remove(YURIS_LITERAL_METADATA_KEY);
    assert!(unmarked.require_preserved_translation_controls().is_err());
}

#[tokio::test]
async fn saved_report_counts_encoding_issue_without_mutating_semantic_pivot() {
    let dir = tempfile::tempdir().unwrap();
    let source_path = dir.path().join("source.db");
    let pivot_path = dir.path().join("pivot.db");
    let original = entry("[Kana]");
    let source = Database::open(&source_path).unwrap();
    source
        .save_entries(std::slice::from_ref(&original))
        .unwrap();
    source.pivot_to_complete(&pivot_path).unwrap();
    let pivot = Database::open(&pivot_path).unwrap();
    let mut row = pivot.get_entry(&original.id).unwrap().unwrap();
    row.translation = Some("[Español]".into());
    row.status = StringStatus::Translated;
    pivot.save_entries(std::slice::from_ref(&row)).unwrap();
    let before = serde_json::to_value(&row).unwrap();
    let report = Validator::validate_and_save(std::slice::from_ref(&row), &pivot)
        .await
        .unwrap();
    assert_eq!(report.by_kind.get("UnencodableTranslation"), Some(&1));
    assert_eq!(report.total_checked, 1);
    assert_eq!(report.issues_found, 1);
    assert_eq!(report.entries_with_issues, 1);
    assert!(report.issues[0].source.is_none());
    assert_eq!(
        serde_json::to_value(pivot.get_entry(&row.id).unwrap().unwrap()).unwrap(),
        before
    );
    assert_eq!(row.source, "[Kana]");
    assert_eq!(row.injection_source().unwrap(), "[カナ]");
    assert_eq!(row.metadata["locust_injection_source"], "[カナ]");
    assert!(!row.metadata.contains_key("locust_injection_capacity"));
    assert_eq!(
        serde_json::to_value(source.get_entry(&original.id).unwrap().unwrap()).unwrap(),
        serde_json::to_value(original).unwrap()
    );
}

// A dispatch sentinel, deliberately not a YU-RIS serializer or format roundtrip.
struct DispatchSentinel(Arc<AtomicUsize>);
impl FormatPlugin for DispatchSentinel {
    fn id(&self) -> &str {
        "yuris"
    }
    fn name(&self) -> &str {
        "Encoding preflight sentinel"
    }
    fn supported_extensions(&self) -> &[&str] {
        &["ybn"]
    }
    fn extract(&self, _: &Path) -> Result<Vec<StringEntry>> {
        unreachable!()
    }
    fn inject(&self, _: &Path, _: &[StringEntry]) -> Result<InjectionReport> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Err(LocustError::InjectionError("sentinel dispatched".into()))
    }
    fn inject_add(&self, path: &Path, _: &str, entries: &[StringEntry]) -> Result<InjectionReport> {
        self.inject(path, entries)
    }
}

async fn exercise_all_modes(row: StringEntry, expected: &str, refused: bool) {
    for mode in ["direct", "replace", "add"] {
        let dir = tempfile::tempdir().unwrap();
        let game = dir.path().join("game");
        fs::create_dir(&game).unwrap();
        let bytes = b"synthetic original bytes";
        fs::write(game.join("story.ybn"), bytes).unwrap();
        let db_path = dir.path().join("source.db");
        let db = Arc::new(Database::open(&db_path).unwrap());
        db.save_entries(std::slice::from_ref(&row)).unwrap();
        let db_bytes = fs::read(&db_path).unwrap();
        let wal_path = dir.path().join("source.db-wal");
        let wal_bytes = fs::read(&wal_path).unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let mut registry = FormatRegistry::new();
        registry.register(Box::new(DispatchSentinel(calls.clone())));
        let backup_path = dir.path().join("backups");
        let backup = Arc::new(BackupManager::new(backup_path.clone()));
        let output = dir.path().join("output");
        let (tx, _rx) = mpsc::channel(100);
        let message = if mode == "direct" {
            inject_direct(&registry, &db, &backup, &game, "yuris", &["es".into()])
                .unwrap_err()
                .to_string()
        } else {
            let result = MultiLangInjector::new(Arc::new(registry), db.clone(), backup)
                .inject(
                    &game,
                    "yuris",
                    if mode == "add" {
                        OutputMode::Add
                    } else {
                        OutputMode::Replace
                    },
                    vec!["es".into()],
                    Some(output.clone()),
                    tx,
                )
                .await;
            if refused {
                result.unwrap_err().to_string()
            } else {
                let report = result.unwrap();
                assert!(report.languages_processed.is_empty());
                assert_eq!(report.languages_failed.len(), 1);
                report.languages_failed[0].1.clone()
            }
        };
        assert!(message.contains(expected), "{mode}: {message}");
        assert_eq!(
            calls.load(Ordering::SeqCst),
            usize::from(!refused),
            "{mode}"
        );
        if refused {
            assert!(!output.exists(), "{mode}: output created");
            assert!(!backup_path.exists(), "{mode}: backup created");
            assert_eq!(
                fs::read_dir(&game).unwrap().count(),
                1,
                "{mode}: transaction artifacts created"
            );
            assert_eq!(fs::read(&db_path).unwrap(), db_bytes);
            assert_eq!(fs::read(&wal_path).unwrap(), wal_bytes);
        }
        assert_eq!(fs::read(game.join("story.ybn")).unwrap(), bytes);
        assert!(db.get_injection(Some("es")).unwrap().is_none());
        assert_eq!(
            serde_json::to_value(db.get_entry(&row.id).unwrap().unwrap()).unwrap(),
            serde_json::to_value(&row).unwrap()
        );
    }
}

#[tokio::test]
async fn incompatible_selected_translation_refused_before_any_side_effect_all_modes() {
    exercise_all_modes(entry("Español"), "cannot be encoded as Shift-JIS", true).await;
    // The injection gate must not replace a pivot's semantic English source or
    // its Spanish target, or change the original Japanese physical provenance.
    let mut pivot = entry("[Español]");
    pivot.source = "[Kana]".into();
    pivot
        .metadata
        .insert("locust_injection_source".into(), json!("[カナ]"));
    exercise_all_modes(pivot, "cannot be encoded as Shift-JIS", true).await;
}

#[tokio::test]
async fn stale_and_invalid_physical_provenance_remain_fatal_all_modes() {
    let mut stale = entry("Latin");
    stale.metadata.insert(
        "locust_stale_translation".into(),
        json!({
            "version": 1, "translated_source_sha256": "0".repeat(64),
            "current_source_sha256": stale.source_hash(),
        }),
    );
    assert!(Validator::validate_entry(&stale)
        .iter()
        .any(|issue| serde_json::to_value(&issue.kind).unwrap() == "StaleTranslation"));
    exercise_all_modes(stale, "stale", true).await;
    let mut invalid = entry("Latin");
    invalid
        .metadata
        .insert("locust_injection_source".into(), json!(42));
    assert!(Validator::validate_entry(&invalid)
        .iter()
        .any(|issue| serde_json::to_value(&issue.kind).unwrap() == "InvalidInjectionProvenance"));
    exercise_all_modes(invalid, "locust_injection_source", true).await;
}

#[tokio::test]
async fn physical_controls_still_refuse_even_if_semantic_source_lost_them() {
    let mut row = entry("Latin");
    row.metadata
        .insert("locust_injection_source".into(), json!(r"日本語 \V[1]"));
    exercise_all_modes(row, "protected controls", true).await;
}

#[tokio::test]
async fn identical_diagnosis_is_nonfatal_in_all_modes() {
    let mut row = entry("日本語");
    row.source = "日本語".into();
    assert!(encoding_issues(&row).is_empty());
    assert!(Validator::validate_entry(&row)
        .iter()
        .any(|issue| serde_json::to_value(&issue.kind).unwrap() == "IdenticalToSource"));
    exercise_all_modes(row, "sentinel dispatched", false).await;
}

#[tokio::test]
async fn unmarked_legacy_encoding_behavior_is_not_extended_by_ybn_path() {
    let mut row = entry("Español");
    row.metadata.remove(YURIS_LITERAL_METADATA_KEY);
    row.metadata.remove("format_id");
    exercise_all_modes(row, "sentinel dispatched", false).await;
}
