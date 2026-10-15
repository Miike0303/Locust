//! Core policy/dispatch evidence only, not a format serializer or runtime test.
use locust_core::{
    backup::BackupManager,
    database::{Database, EntryFilter},
    error::Result,
    extraction::{
        FormatPlugin, FormatRegistry, InjectionReport, MultiLangInjectionOptions,
        MultiLangInjector, MultiLangReport,
    },
    models::{
        OutputMode, StringEntry, StringStatus, YURIS_LITERAL_METADATA_KEY, YURIS_QUOTED_LITERAL_V1,
    },
    textasset_group::{attach_to_entries, GroupKind},
};
use serde_json::json;
use std::{
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};
use tokio::sync::mpsc;

struct DispatchSentinel(Arc<Mutex<Option<Vec<StringEntry>>>>);
impl FormatPlugin for DispatchSentinel {
    fn id(&self) -> &str {
        "sentinel"
    }
    fn name(&self) -> &str {
        "Capacity preflight sentinel"
    }
    fn supported_extensions(&self) -> &[&str] {
        &["bin"]
    }
    fn extract(&self, _: &Path) -> Result<Vec<StringEntry>> {
        unreachable!()
    }
    fn inject(&self, path: &Path, entries: &[StringEntry]) -> Result<InjectionReport> {
        *self.0.lock().unwrap() = Some(entries.to_vec());
        let file = path.join("story.bin");
        fs::write(&file, b"sentinel output")?;
        Ok(InjectionReport {
            files_modified: 1,
            strings_written: 1,
            // Deliberately partial: capacity policy is not require_complete.
            strings_skipped: 1,
            skip_reasons: Default::default(),
            warnings: vec![],
            files_written: vec![file],
        })
    }
}

fn slot(id: &str, source: &str, target: Option<&str>, encoding: Option<&str>) -> StringEntry {
    let mut row = StringEntry::new(id, source, PathBuf::from("story.bin"));
    row.translation = target.map(str::to_owned);
    row.status = if target.is_some() {
        StringStatus::Translated
    } else {
        StringStatus::Pending
    };
    if let Some(encoding) = encoding {
        row.metadata.insert("binary_slot".into(), json!(encoding));
    }
    row
}

struct Fixture {
    db: Arc<Database>,
    injector: MultiLangInjector,
    dispatched: Arc<Mutex<Option<Vec<StringEntry>>>>,
    game: PathBuf,
    output: PathBuf,
    backups: PathBuf,
    db_before: Vec<u8>,
    wal_before: Vec<u8>,
    rows_before: serde_json::Value,
    temp: tempfile::TempDir,
}
impl Fixture {
    fn new(rows: &[StringEntry]) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let game = temp.path().join("game");
        fs::create_dir(&game).unwrap();
        fs::write(game.join("story.bin"), b"original").unwrap();
        let db = Arc::new(Database::open(&temp.path().join("source.db")).unwrap());
        db.save_entries(rows).unwrap();
        let dispatched = Arc::new(Mutex::new(None));
        let mut registry = FormatRegistry::new();
        registry.register(Box::new(DispatchSentinel(dispatched.clone())));
        let backups = temp.path().join("backups");
        Self {
            injector: MultiLangInjector::new(
                Arc::new(registry),
                db.clone(),
                Arc::new(BackupManager::new(backups.clone())),
            ),
            rows_before: serde_json::to_value(db.get_entries(&EntryFilter::default()).unwrap())
                .unwrap(),
            db_before: fs::read(temp.path().join("source.db")).unwrap(),
            wal_before: fs::read(temp.path().join("source.db-wal")).unwrap(),
            db,
            dispatched,
            game,
            output: temp.path().join("output"),
            backups,
            temp,
        }
    }
    async fn run(&self, options: Option<MultiLangInjectionOptions>) -> Result<MultiLangReport> {
        let (tx, rx) = mpsc::channel(1);
        drop(rx); // The fatal policy cannot depend on a progress consumer.
        if let Some(options) = options {
            self.injector
                .inject_with_options(
                    &self.game,
                    "sentinel",
                    options,
                    vec!["es".into()],
                    Some(self.output.clone()),
                    tx,
                )
                .await
        } else {
            self.injector
                .inject(
                    &self.game,
                    "sentinel",
                    OutputMode::Replace,
                    vec!["es".into()],
                    Some(self.output.clone()),
                    tx,
                )
                .await
        }
    }
    fn assert_accepted(&self, report: MultiLangReport) {
        assert_eq!(report.languages_processed, ["es"]);
        assert!(report.languages_failed.is_empty());
        assert_eq!(report.reports["es"].strings_skipped, 1);
        assert!(self.backups.exists());
        assert!(self.db.get_injection(Some("es")).unwrap().is_some());
        let destination = &report.injected_roots["es"];
        assert_eq!(
            fs::read(destination.join("story.bin")).unwrap(),
            b"sentinel output"
        );
        assert_eq!(fs::read(self.game.join("story.bin")).unwrap(), b"original");
        let mut expected = self.db.get_entries(&EntryFilter::default()).unwrap();
        assert_eq!(serde_json::to_value(&expected).unwrap(), self.rows_before);
        for row in &mut expected {
            row.source = row.injection_source().unwrap().to_owned();
            row.file_path = destination.canonicalize().unwrap().join("story.bin");
        }
        assert_eq!(
            serde_json::to_value(self.dispatched.lock().unwrap().as_ref().unwrap()).unwrap(),
            serde_json::to_value(expected).unwrap()
        );
    }
    fn assert_refused(&self) {
        assert!(self.dispatched.lock().unwrap().is_none());
        assert!(!self.output.exists());
        assert!(!self.backups.exists());
        assert_eq!(fs::read_dir(&self.game).unwrap().count(), 1);
        assert_eq!(fs::read(self.game.join("story.bin")).unwrap(), b"original");
        assert!(self.db.get_injection(Some("es")).unwrap().is_none());
        assert_eq!(
            fs::read(self.temp.path().join("source.db")).unwrap(),
            self.db_before
        );
        assert_eq!(
            fs::read(self.temp.path().join("source.db-wal")).unwrap(),
            self.wal_before
        );
        assert_eq!(
            serde_json::to_value(self.db.get_entries(&EntryFilter::default()).unwrap()).unwrap(),
            self.rows_before
        );
    }
}

fn strict() -> MultiLangInjectionOptions {
    MultiLangInjectionOptions {
        require_binary_slot_fit: true,
        ..Default::default()
    }
}

#[tokio::test]
async fn strict_copy_refuses_utf8_oversize_before_side_effects() {
    let fixture = Fixture::new(&[slot("utf8", "abc", Some("éé"), Some("utf8"))]);
    let error = fixture
        .run(Some(strict()))
        .await
        .expect_err("oversize copy must be refused");
    let message = error.to_string();
    assert!(message.contains("1 translation(s) exceed known binary slot capacity"));
    assert!(!message.contains("abc") && !message.contains("éé"));
    fixture.assert_refused();
}

#[tokio::test]
async fn strict_copy_counts_non_bmp_utf16_and_shift_jis_bytes() {
    for (encoding, source, target) in [("utf16le", "A", "😀"), ("sjis", "ab", "日本")] {
        let fixture = Fixture::new(&[slot("bytes", source, Some(target), Some(encoding))]);
        let error = fixture.run(Some(strict())).await.unwrap_err().to_string();
        assert!(error.contains("1 translation(s) exceed known binary slot capacity"));
        fixture.assert_refused();
    }
}

#[tokio::test]
async fn pivot_capacity_uses_physical_not_semantic_source() {
    for (semantic, physical, bytes, refused) in [
        ("Long semantic source", "あ", 3, true),
        ("a", "日本語", 9, false),
    ] {
        let mut row = slot("pivot", semantic, Some("éé"), Some("utf8"));
        row.metadata
            .insert("locust_injection_source".into(), json!(physical));
        row.metadata.insert(
            "locust_injection_capacity".into(),
            json!({"encoding": "utf8", "bytes": bytes}),
        );
        let fixture = Fixture::new(&[row]);
        let result = fixture.run(Some(strict())).await;
        if refused {
            assert!(result
                .unwrap_err()
                .to_string()
                .contains("binary slot capacity"));
            fixture.assert_refused();
        } else {
            fixture.assert_accepted(result.unwrap());
        }
    }
}

fn group(padded: bool) -> Vec<StringEntry> {
    let mut rows = vec![
        slot("group-a", "Hi", Some("Hello"), Some("utf8")),
        slot("group-b", "Go", None, Some("utf8")),
    ];
    for (index, (row, key)) in rows.iter_mut().zip(["Menu.A", "Menu.B"]).enumerate() {
        row.metadata
            .insert("extraction_method".into(), json!("textasset_loc_line"));
        row.metadata.insert("loc_key".into(), json!(key));
        row.metadata.insert("line_index".into(), json!(index));
    }
    let original = if padded {
        "Menu.A: Hi\nMenu.B: Go\n    "
    } else {
        "Menu.A: Hi\nMenu.B: Go\n"
    };
    attach_to_entries(
        &mut rows,
        GroupKind::LocLine,
        original,
        original.len(),
        "group",
    );
    rows
}

#[tokio::test]
async fn shared_budget_retains_untranslated_members_and_allows_cell_growth() {
    for padded in [false, true] {
        let fixture = Fixture::new(&group(padded));
        let result = fixture.run(Some(strict())).await;
        if padded {
            fixture.assert_accepted(result.unwrap());
        } else {
            assert!(result
                .unwrap_err()
                .to_string()
                .contains("binary slot capacity"));
            fixture.assert_refused();
        }
    }
}

fn semantic_equal_pivot_group(padded: bool) -> Vec<StringEntry> {
    let mut rows = group(padded);
    rows[0]
        .metadata
        .insert("locust_injection_source".into(), json!("Hi"));
    rows[0].source = "Hello".into();
    // Target Hello is unchanged semantically, but grows physical Hi by 3 bytes.
    // Keep the untranslated second member in both preflight and dispatch.
    rows
}

#[tokio::test]
async fn semantic_equal_pivot_group_refuses_physical_blob_growth() {
    let fixture = Fixture::new(&semantic_equal_pivot_group(false));
    let error = match fixture.run(Some(strict())).await {
        Ok(report) => {
            fixture.assert_accepted(report);
            panic!("strict capacity gate dispatched and recorded an oversized semantic-equal pivot group");
        }
        Err(error) => error,
    };
    assert!(error.to_string().contains("binary slot capacity"));
    fixture.assert_refused();
}

#[tokio::test]
async fn semantic_equal_pivot_group_allows_growth_within_physical_budget() {
    let fixture = Fixture::new(&semantic_equal_pivot_group(true));
    fixture.assert_accepted(fixture.run(Some(strict())).await.unwrap());
}

#[tokio::test]
async fn exact_fits_and_rows_without_selected_translations_or_budgets_do_not_false_fail() {
    let fixture = Fixture::new(&[
        slot("utf8-fit", "abcd", Some("éé"), Some("utf8")),
        slot("utf16-fit", "AB", Some("😀"), Some("utf16le")),
        slot("sjis-fit", "abcd", Some("日本"), Some("sjis")),
        slot("untranslated", "a", None, Some("utf8")),
        slot("empty", "a", Some(""), Some("utf8")),
        slot(
            "unknown-budget",
            "a",
            Some("Long unbounded translation"),
            None,
        ),
        slot("unselected-unbounded", "a", None, None),
    ]);
    fixture.assert_accepted(fixture.run(Some(strict())).await.unwrap());
}

#[tokio::test]
async fn legacy_and_explicit_partial_options_keep_oversize_copy_behavior() {
    assert!(!MultiLangInjectionOptions::default().require_binary_slot_fit);
    for options in [None, Some(MultiLangInjectionOptions::default())] {
        let fixture = Fixture::new(&[slot("oversize", "a", Some("éé"), Some("utf8"))]);
        fixture.assert_accepted(fixture.run(options).await.unwrap());
    }
}

#[tokio::test]
async fn strict_add_is_explicitly_unsupported_before_side_effects() {
    let fixture = Fixture::new(&[slot("fits", "abc", Some("xyz"), Some("utf8"))]);
    let error = fixture
        .run(Some(MultiLangInjectionOptions {
            mode: OutputMode::Add,
            ..strict()
        }))
        .await
        .unwrap_err();
    assert!(error
        .to_string()
        .contains("require_binary_slot_fit is supported only for Replace"));
    fixture.assert_refused();
}

#[tokio::test]
async fn provenance_stale_controls_and_encoding_errors_precede_capacity_policy() {
    for (key, value, expected) in [
        (
            "locust_injection_source",
            json!(42),
            "locust_injection_source",
        ),
        (
            "locust_injection_capacity",
            json!(42),
            "locust_injection_capacity",
        ),
        ("locust_stale_translation", json!({}), "stale"),
        (
            "locust_injection_source",
            json!("Physical {name}"),
            "protected controls",
        ),
        (
            YURIS_LITERAL_METADATA_KEY,
            json!(YURIS_QUOTED_LITERAL_V1),
            "cannot be encoded as Shift-JIS",
        ),
    ] {
        let mut invalid = slot("invalid", "a", Some("Español"), None).with_format_id("yuris");
        invalid.metadata.insert(key.into(), value);
        let fixture = Fixture::new(&[slot("oversize", "a", Some("éé"), Some("utf8")), invalid]);
        let error = fixture.run(Some(strict())).await.unwrap_err().to_string();
        assert!(error.contains(expected), "{error}");
        assert!(!error.contains("exceed known binary slot capacity"));
        fixture.assert_refused();
    }
}
