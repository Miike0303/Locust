use locust_core::{
    backup::BackupManager,
    database::Database,
    error::Result,
    extraction::{inject_direct, FormatPlugin, FormatRegistry, InjectionReport},
    models::{StringEntry, StringStatus},
    patch::GameLock,
    project::{
        open_project, open_verified_saved_project, preflight_project_open, ProjectOpenPreflight,
    },
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

struct TextPlugin(Arc<AtomicUsize>);
impl FormatPlugin for TextPlugin {
    fn id(&self) -> &str {
        "resume-test"
    }
    fn name(&self) -> &str {
        "Resume Test"
    }
    fn supported_extensions(&self) -> &[&str] {
        &["txt"]
    }
    fn detect(&self, path: &Path) -> bool {
        path.join("story.txt").is_file()
    }
    fn extract(&self, path: &Path) -> Result<Vec<StringEntry>> {
        self.0.fetch_add(1, Ordering::SeqCst);
        let mut entry = StringEntry::new(
            "hero",
            fs::read_to_string(path.join("story.txt"))?,
            path.join("story.txt"),
        );
        entry.metadata.insert(
            "iostore_unread_containers".into(),
            json!(["extra.utoc: encrypted"]),
        );
        Ok(vec![entry])
    }
    fn inject(&self, path: &Path, entries: &[StringEntry]) -> Result<InjectionReport> {
        let mut files = Vec::new();
        for entry in entries {
            let file = path.join(entry.file_path.file_name().unwrap());
            fs::write(&file, entry.translation.as_deref().unwrap())?;
            files.push(file);
        }
        Ok(InjectionReport {
            files_modified: files.len(),
            strings_written: entries.len(),
            strings_skipped: 0,
            warnings: vec![],
            files_written: files,
            skip_reasons: Default::default(),
        })
    }
}

struct Fixture {
    temp: tempfile::TempDir,
    game: PathBuf,
    database: PathBuf,
    registry: FormatRegistry,
    calls: Arc<AtomicUsize>,
}
impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let game = temp.path().join("game");
        fs::create_dir(&game).unwrap();
        fs::write(game.join("story.txt"), "勇者").unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let mut registry = FormatRegistry::new();
        registry.register(Box::new(TextPlugin(calls.clone())));
        let db = Database::open_in_memory().unwrap();
        let opened = open_project(&db, &registry, &game, None).unwrap();
        let mut row = db.get_entry("hero").unwrap().unwrap();
        row.translation = Some("Hero".into());
        row.status = StringStatus::Approved;
        row.provider_used = Some("test-provider".into());
        db.save_entries(&[row]).unwrap();
        let fixture = Self {
            temp,
            game,
            database: opened.database_path,
            registry,
            calls,
        };
        fixture.inject(&db);
        fixture
    }
    fn inject(&self, db: &Database) {
        inject_direct(
            &self.registry,
            db,
            &BackupManager::new(self.temp.path().join("backups")),
            &self.game,
            "resume-test",
            &["en".into()],
        )
        .unwrap();
    }
    fn preflight(&self) -> ProjectOpenPreflight {
        preflight_project_open(&self.registry, &self.game, None).unwrap()
    }
    fn operation(&self) -> PathBuf {
        let store = self.game.join(".locust-injections");
        let active: Value =
            serde_json::from_slice(&fs::read(store.join("active.json")).unwrap()).unwrap();
        store
            .join("operations")
            .join(active["transaction_id"].as_str().unwrap())
    }
    fn sql(&self, sql: &str) {
        rusqlite::Connection::open(&self.database)
            .unwrap()
            .execute_batch(sql)
            .unwrap();
    }
}

fn change_json(path: &Path, change: impl FnOnce(&mut Value)) {
    let mut value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    change(&mut value);
    fs::write(path, serde_json::to_vec(&value).unwrap()).unwrap();
}

#[test]
fn direct_project_resume_preserves_saved_rows_warnings_without_extraction() {
    let fixture = Fixture::new();
    let preflight = fixture.preflight();
    assert!(
        matches!(&preflight, ProjectOpenPreflight::ResumeAvailable {
        database_path, project_path, format_id
    } if database_path == &fixture.database && project_path == &fixture.game
        && format_id == "resume-test"),
        "{preflight:?}"
    );
    let wire = serde_json::to_value(&preflight).unwrap();
    assert_eq!(wire["kind"], "resume_available");
    let live = Database::open_in_memory().unwrap();
    let opened = open_verified_saved_project(
        &live,
        &fixture.registry,
        &fixture.database,
        &fixture.game,
        "resume-test",
    )
    .unwrap();
    let row = live.get_entry("hero").unwrap().unwrap();
    assert_eq!(row.source, "勇者");
    assert_eq!(row.translation.as_deref(), Some("Hero"));
    assert_eq!(row.status, StringStatus::Approved);
    assert_eq!(row.provider_used.as_deref(), Some("test-provider"));
    assert_eq!(opened.extraction_warnings, ["extra.utoc: encrypted"]);
    assert_eq!(
        (
            opened.added,
            opened.updated,
            opened.stale_source_reset,
            opened.removed,
            opened.preserved_translations
        ),
        (0, 0, 0, 0, 0)
    );
    assert_eq!(fixture.calls.load(Ordering::SeqCst), 1);
    let refreshed = open_project(&live, &fixture.registry, &fixture.game, None).unwrap();
    assert_eq!(refreshed.stale_source_reset, 1);
    let row = live.get_entry("hero").unwrap().unwrap();
    assert_eq!(row.source, "Hero");
    assert_eq!(row.status, StringStatus::Pending);
    assert_eq!(fixture.calls.load(Ordering::SeqCst), 2);
}

#[test]
fn direct_project_resume_revalidates_drift_without_switching_database() {
    let fixture = Fixture::new();
    assert!(matches!(
        fixture.preflight(),
        ProjectOpenPreflight::ResumeAvailable { .. }
    ));
    fs::write(fixture.game.join("story.txt"), "Drift").unwrap();
    assert_attention(&fixture, "drift after preflight");
}

fn assert_attention(fixture: &Fixture, label: &str) {
    let saved_bytes = fs::read(&fixture.database).ok();
    let preflight = fixture.preflight();
    assert!(
        matches!(&preflight, ProjectOpenPreflight::NeedsAttention { reason }
        if !reason.is_empty()),
        "{label}: {preflight:?}"
    );
    let live = Database::open_in_memory().unwrap();
    live.save_entries(&[StringEntry::new("keep", "Unrelated", "other.txt".into())])
        .unwrap();
    assert!(
        open_verified_saved_project(
            &live,
            &fixture.registry,
            &fixture.database,
            &fixture.game,
            "resume-test",
        )
        .is_err(),
        "{label}"
    );
    assert_eq!(live.path(), PathBuf::from(":memory:"), "{label}");
    assert_eq!(live.get_entry("keep").unwrap().unwrap().source, "Unrelated");
    assert!(live.get_entry("hero").unwrap().is_none());
    assert_eq!(fixture.calls.load(Ordering::SeqCst), 1);
    assert_eq!(fs::read(&fixture.database).ok(), saved_bytes, "{label}");
}

#[test]
fn direct_project_resume_rejects_unverified_provenance_and_members() {
    for invalid in [
        "add",
        "unknown_mode",
        "format",
        "language",
        "foreign_root",
        "replace_copy",
        "restored",
        "pending",
        "corrupt_plan",
        "duplicate_plan",
        "unsafe_plan",
        "duplicate_recording",
        "unsafe_recording",
        "malformed_provenance",
        "missing",
        "hash",
        "size",
        "no_recording",
        "no_saved_db",
        "corrupt_db",
    ] {
        let fixture = Fixture::new();
        let operation = fixture.operation();
        match invalid {
            "add" | "unknown_mode" | "format" | "language" => {
                change_json(&operation.join("identity.json"), |id| match invalid {
                    "add" => id["mode"] = json!("add"),
                    "unknown_mode" => id["mode"] = Value::Null,
                    "format" => id["format"] = json!("unknown-format"),
                    _ => id["language"] = json!("es"),
                });
            }
            "foreign_root" | "replace_copy" => {
                fixture.sql("UPDATE injected_files SET root='foreign-game-copy'")
            }
            "restored" => fs::write(operation.join("restored.json"), b"[\"story.txt\"]").unwrap(),
            "pending" => {
                fs::write(operation.join("phase.json"), b"\"committed_unrecorded\"").unwrap()
            }
            "corrupt_plan" => fs::write(operation.join("plan.json"), b"{broken").unwrap(),
            "duplicate_plan" => change_json(&operation.join("plan.json"), |plan| {
                let first = plan["files"][0].clone();
                plan["files"].as_array_mut().unwrap().push(first);
            }),
            "unsafe_plan" => change_json(&operation.join("plan.json"), |plan| {
                plan["files"][0]["path"] = json!("../outside.txt");
            }),
            "duplicate_recording" => fixture.sql(
                "INSERT INTO injected_files (lang,root,rel,hash,size,recorded_at,pristine_backup)
                 SELECT lang,root,rel,hash,size,recorded_at,pristine_backup FROM injected_files",
            ),
            "unsafe_recording" => fixture.sql("UPDATE injected_files SET rel='../outside.txt'"),
            "malformed_provenance" => {
                fixture.sql("UPDATE injected_files SET pristine_backup='{broken'")
            }
            "missing" => fs::rename(
                fixture.game.join("story.txt"),
                fixture.temp.path().join("missing.txt"),
            )
            .unwrap(),
            "hash" => fs::write(fixture.game.join("story.txt"), "HERO").unwrap(),
            "size" => fixture.sql("UPDATE injected_files SET size=size+1"),
            "no_recording" => fixture.sql("DELETE FROM injected_files"),
            "no_saved_db" => {
                fs::rename(&fixture.database, fixture.temp.path().join("other.db")).unwrap()
            }
            "corrupt_db" => fs::write(&fixture.database, b"not sqlite").unwrap(),
            _ => unreachable!(),
        }
        assert_attention(&fixture, invalid);
    }
}

#[test]
fn direct_project_resume_preflight_is_advisory_lock_free_and_plain_folders_extract() {
    let fixture = Fixture::new();
    let guard = GameLock::acquire(&fixture.game).unwrap();
    assert!(matches!(
        fixture.preflight(),
        ProjectOpenPreflight::ResumeAvailable { .. }
    ));
    let live = Database::open_in_memory().unwrap();
    assert!(open_verified_saved_project(
        &live,
        &fixture.registry,
        &fixture.database,
        &fixture.game,
        "resume-test",
    )
    .unwrap_err()
    .to_string()
    .contains("game busy"));
    drop(guard);
    assert!(matches!(
        preflight_project_open(&fixture.registry, &fixture.game, Some("wrong-format")).unwrap(),
        ProjectOpenPreflight::NeedsAttention { .. }
    ));
    let plain = fixture.temp.path().join("plain");
    fs::create_dir(&plain).unwrap();
    fs::write(plain.join("story.txt"), "Unrelated source").unwrap();
    let choice = preflight_project_open(&fixture.registry, &plain, None).unwrap();
    assert_eq!(
        serde_json::to_value(choice).unwrap(),
        json!({"kind": "extract"})
    );
    assert!(!plain.join(".locust-injections").exists());
    assert!(!plain.with_extension("locust.db").exists());
    assert_eq!(fixture.calls.load(Ordering::SeqCst), 1);
}

#[test]
fn direct_project_resume_covers_multigeneration_union_and_revisions() {
    let fixture = Fixture::new();
    let first = fixture.operation();
    let saved = Database::open(&fixture.database).unwrap();
    let mut row = StringEntry::new("npc", "村人", fixture.game.join("extra.txt"));
    row.translation = Some("Villager".into());
    saved.save_entries(&[row]).unwrap();
    fixture.inject(&saved);
    assert_ne!(fixture.operation(), first);
    assert_eq!(
        saved
            .get_injection(Some("en"))
            .unwrap()
            .unwrap()
            .files
            .len(),
        2
    );
    assert!(matches!(
        fixture.preflight(),
        ProjectOpenPreflight::ResumeAvailable { .. }
    ));
    let mut hero = saved.get_entry("hero").unwrap().unwrap();
    hero.translation = Some("Brave Hero".into());
    saved.save_entries(&[hero]).unwrap();
    fixture.inject(&saved);
    assert!(matches!(
        fixture.preflight(),
        ProjectOpenPreflight::ResumeAvailable { .. }
    ));
    let live = Database::open_in_memory().unwrap();
    let resumed = open_verified_saved_project(
        &live,
        &fixture.registry,
        &fixture.database,
        &fixture.game,
        "resume-test",
    )
    .unwrap();
    assert_eq!(resumed.total_strings, 2);
    assert_eq!(live.get_entry("npc").unwrap().unwrap().source, "村人");
    fs::write(first.join("phase.json"), b"\"preparing\"").unwrap();
    assert_attention(&fixture, "pending older generation");
    fs::write(first.join("phase.json"), b"\"completed\"").unwrap();
    fixture.sql("DELETE FROM injected_files WHERE rel='extra.txt'");
    assert_attention(&fixture, "incomplete generation union");
    saved
        .record_injection(
            Some("en"),
            &fixture.game,
            &[
                fixture.game.join("story.txt"),
                fixture.game.join("extra.txt"),
            ],
        )
        .unwrap();
    let latest = fixture.operation();
    let latest_plan: Value =
        serde_json::from_slice(&fs::read(latest.join("plan.json")).unwrap()).unwrap();
    change_json(&first.join("plan.json"), |plan| {
        plan["prepared_at"] = latest_plan["prepared_at"].clone();
    });
    assert_attention(&fixture, "simultaneous overlapping ownership");
}

#[test]
fn direct_project_resume_reads_wal_and_does_not_require_pristine_backup_contents() {
    let fixture = Fixture::new();
    let saved = Database::open(&fixture.database).unwrap();
    saved
        .set_project_metadata("extraction_warnings", &json!(["Committed WAL warning"]))
        .unwrap();
    assert!(PathBuf::from(format!("{}-wal", fixture.database.display())).is_file());
    let backups = BackupManager::new(fixture.temp.path().join("backups"));
    let backup = backups.list_backups().unwrap().pop().unwrap();
    fs::write(
        backup.path.join("payload/story.txt"),
        "corrupt pristine bytes",
    )
    .unwrap();
    fs::write(
        fixture.operation().join("originals/0"),
        "corrupt transaction backup",
    )
    .unwrap();
    let live = Database::open_in_memory().unwrap();
    assert!(matches!(
        fixture.preflight(),
        ProjectOpenPreflight::ResumeAvailable { .. }
    ));
    let opened = open_verified_saved_project(
        &live,
        &fixture.registry,
        &fixture.database,
        &fixture.game,
        "resume-test",
    )
    .unwrap();
    assert_eq!(opened.extraction_warnings, ["Committed WAL warning"]);
    assert_eq!(live.get_entry("hero").unwrap().unwrap().source, "勇者");
}
