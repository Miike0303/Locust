//! Real NScripter source matching through core's Direct and Replace pipelines.
//! Compile the existing plugin here to avoid a core -> formats dependency cycle.
#[path = "../../formats/src/nscripter.rs"]
mod nscripter;
// Only the helpers nscripter.rs needs are used; the rest of util.rs is dead here.
#[allow(dead_code)]
#[path = "../../formats/src/util.rs"]
mod util;

use locust_core::{
    backup::BackupManager,
    database::{sha256_file, sha256_hex, Database, EntryFilter},
    extraction::{inject_direct, FormatPlugin, FormatRegistry, MultiLangInjector, MultiLangReport},
    models::{OutputMode, StringStatus},
};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};
use tokio::sync::mpsc;

const ORIGINAL: &str = "; untouched\r\n*start\r\ncaption \"Original title\"\r\n`Original dialogue`\r\ngoto *end\r\n*end\r\n";

struct Fixture {
    db: Arc<Database>,
    registry: Arc<FormatRegistry>,
    backups: Arc<BackupManager>,
    game: PathBuf,
    temp: tempfile::TempDir,
}

impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let game = temp.path().join("game");
        fs::create_dir(&game).unwrap();
        fs::write(game.join("0.txt"), ORIGINAL).unwrap();
        let plugin = nscripter::NScripterPlugin::new();
        let entries = plugin.extract(&game).unwrap();
        assert_eq!(entries.len(), 2);
        let db = Arc::new(Database::open_in_memory().unwrap());
        db.save_entries(&entries).unwrap();
        let mut registry = FormatRegistry::new();
        registry.register(Box::new(plugin));
        let fixture = Self {
            db,
            registry: Arc::new(registry),
            backups: Arc::new(BackupManager::new(temp.path().join("backups"))),
            game,
            temp,
        };
        fixture.translate(&fixture.db, "First", true);
        fixture
    }

    fn translate(&self, db: &Database, target: &str, only_title: bool) {
        let mut entries = self.db.get_entries(&EntryFilter::default()).unwrap();
        for entry in &mut entries {
            let title = entry.source == "Original title";
            entry.translation = if title {
                Some(format!("{target} title"))
            } else if !only_title {
                Some(format!("`{target} dialogue`"))
            } else {
                None
            };
            entry.status = if entry.translation.is_some() {
                StringStatus::Translated
            } else {
                StringStatus::Pending
            };
        }
        db.save_entries(&entries).unwrap();
    }

    fn direct(&self) -> String {
        let report = inject_direct(
            &self.registry,
            &self.db,
            &self.backups,
            &self.game,
            "nscripter",
            &["en".into()],
        )
        .unwrap();
        assert_eq!(report.strings_written, 1);
        report.backup_id
    }

    async fn replace(&self, db: Arc<Database>) -> locust_core::error::Result<MultiLangReport> {
        self.replace_at(db, &self.game, "output").await
    }

    async fn replace_at(
        &self,
        db: Arc<Database>,
        source: &Path,
        output: &str,
    ) -> locust_core::error::Result<MultiLangReport> {
        let (tx, _rx) = mpsc::channel(32);
        MultiLangInjector::new(self.registry.clone(), db, self.backups.clone())
            .inject(
                source,
                "nscripter",
                OutputMode::Replace,
                vec!["en".into()],
                Some(self.temp.path().join(output)),
                tx,
            )
            .await
    }
}

#[tokio::test]
async fn replace_refuses_direct_modified_source_for_same_and_other_project() {
    for same_project in [true, false] {
        let fixture = Fixture::new();
        let original_hash = sha256_file(&fixture.game.join("0.txt")).unwrap();
        assert_eq!(original_hash.0, sha256_hex(ORIGINAL.as_bytes()));
        let backup_id = fixture.direct();
        let db = if same_project {
            fixture.db.clone()
        } else {
            Arc::new(Database::open_in_memory().unwrap())
        };
        fixture.translate(&db, "Second", false);
        let before_recording = db.get_injection(Some("en")).unwrap();
        let before_source = sha256_file(&fixture.game.join("0.txt")).unwrap();
        let result = fixture.replace(db.clone()).await;
        // On the unsafe implementation this is Ok: one row is skipped and the
        // other written, publishing a mixed First/Second generation as success.
        if let Ok(report) = &result {
            let output = fixture.temp.path().join("output/game-en/0.txt");
            panic!(
                "Replace accepted Direct-modified source: report={report:?}, output={:?}",
                fs::read_to_string(output)
            );
        }
        let error = result.unwrap_err().to_string();
        assert!(
            error.contains("Replace") && error.contains("restore"),
            "{error}"
        );
        assert!(!fixture.temp.path().join("output").exists());
        assert_eq!(db.get_injection(Some("en")).unwrap(), before_recording);
        assert_eq!(
            sha256_file(&fixture.game.join("0.txt")).unwrap(),
            before_source
        );
        let backups = fixture.backups.list_backups().unwrap();
        assert_eq!(backups.len(), 1);
        assert_eq!(backups[0].id, backup_id);
        fixture
            .backups
            .with_pristine_tree(&backup_id, &fixture.game, |root| {
                assert_eq!(sha256_file(&root.join("0.txt")).unwrap(), original_hash);
                Ok(())
            })
            .unwrap();
    }
}

#[tokio::test]
async fn replace_accepts_pristine_and_restored_sources_including_replays() {
    for restored in [false, true] {
        for same_project in [true, false] {
            let fixture = Fixture::new();
            if restored {
                let backup = fixture.direct();
                fixture.backups.restore(&backup).unwrap();
            }
            let db = if same_project {
                fixture.db.clone()
            } else {
                Arc::new(Database::open_in_memory().unwrap())
            };
            fixture.translate(&db, "Second", false);
            let original_hash = (sha256_hex(ORIGINAL.as_bytes()), ORIGINAL.len() as u64);
            let backup_count = fixture.backups.list_backups().unwrap().len();
            // Replaying from the unchanged source into a fresh destination is
            // legitimate, even though this DB now records a different root.
            for output in ["output", "replay"] {
                let report = fixture
                    .replace_at(db.clone(), &fixture.game, output)
                    .await
                    .unwrap();
                assert_eq!(report.languages_processed, ["en"]);
                assert!(report.languages_failed.is_empty());
                assert_eq!(report.reports["en"].strings_written, 2);
                assert_eq!(report.reports["en"].strings_skipped, 0);
                let dest = fixture.temp.path().join(output).join("game-en");
                assert_eq!(
                    fs::read_to_string(dest.join("0.txt")).unwrap(),
                    ORIGINAL.replace("Original", "Second")
                );
                let extracted = nscripter::NScripterPlugin::new().extract(&dest).unwrap();
                assert_eq!(extracted.len(), 2);
                assert!(extracted.iter().all(|e| e.source.contains("Second")));
                assert_eq!(
                    sha256_file(&fixture.game.join("0.txt")).unwrap(),
                    original_hash
                );
                let recording = db.get_injection(Some("en")).unwrap().unwrap();
                assert!(locust_core::database::paths_identical(
                    &recording.root,
                    &dest
                ));
                let provenance = recording.pristine_backup.unwrap();
                assert_eq!(provenance.id, report.backup_id);
                assert!(locust_core::database::paths_identical(
                    &provenance.source_path,
                    &fixture.game
                ));
                fixture
                    .backups
                    .with_pristine_tree(&provenance.id, &fixture.game, |root| {
                        assert_eq!(sha256_file(&root.join("0.txt")).unwrap(), original_hash);
                        Ok(())
                    })
                    .unwrap();
            }
            assert_eq!(
                fixture.backups.list_backups().unwrap().len(),
                backup_count + 2
            );
        }
    }
}

#[tokio::test]
async fn unchanged_direct_replay_is_refused_for_same_and_other_project_and_file_selection() {
    for same_project in [true, false] {
        let fixture = Fixture::new();
        fixture.direct();
        let db = if same_project {
            fixture.db.clone()
        } else {
            Arc::new(Database::open_in_memory().unwrap())
        };
        fixture.translate(&db, "First", true);
        let before = db.get_injection(Some("en")).unwrap();
        for source in [&fixture.game, &fixture.game.join("0.txt")] {
            let error = fixture
                .replace_at(db.clone(), source, "output")
                .await
                .unwrap_err();
            assert!(error.to_string().contains("Replace copy refused"));
            assert_eq!(db.get_injection(Some("en")).unwrap(), before);
            assert_eq!(fixture.backups.list_backups().unwrap().len(), 1);
            assert!(!fixture.temp.path().join("output").exists());
        }
    }
}

#[tokio::test]
async fn verified_direct_revision_remains_supported_but_does_not_authorize_replace() {
    let fixture = Fixture::new();
    let original_backup = fixture.direct();
    fixture.translate(&fixture.db, "Second", true);
    fixture.direct();
    assert_eq!(
        fs::read_to_string(fixture.game.join("0.txt")).unwrap(),
        ORIGINAL.replace("Original title", "Second title")
    );
    let before = fixture.db.get_injection(Some("en")).unwrap();
    assert_eq!(
        before
            .as_ref()
            .unwrap()
            .pristine_backup
            .as_ref()
            .unwrap()
            .id,
        original_backup
    );
    assert!(fixture.replace(fixture.db.clone()).await.is_err());
    assert_eq!(fixture.db.get_injection(Some("en")).unwrap(), before);
    assert_eq!(fixture.backups.list_backups().unwrap().len(), 2);
    assert!(!fixture.temp.path().join("output").exists());
    // Restoring the first pristine backup retires both generations, rather than
    // treating the revision's pre-write (already translated) bytes as pristine.
    fixture.backups.restore(&original_backup).unwrap();
    let report = fixture.replace(fixture.db.clone()).await.unwrap();
    assert_eq!(report.reports["en"].strings_written, 1);
    assert_eq!(
        fs::read_to_string(fixture.game.join("0.txt")).unwrap(),
        ORIGINAL
    );
}

#[tokio::test]
async fn pristine_file_selection_does_not_inherit_a_siblings_direct_ownership() {
    let fixture = Fixture::new();
    fixture.direct();
    let sibling_hash = sha256_file(&fixture.game.join("0.txt")).unwrap();
    let source = fixture.game.join("00.txt");
    fs::write(&source, ORIGINAL).unwrap();
    let mut entries = nscripter::NScripterPlugin::new().extract(&source).unwrap();
    for entry in &mut entries {
        entry.translation = Some(entry.source.replace("Original", "Second"));
        entry.status = StringStatus::Translated;
    }
    let db = Arc::new(Database::open_in_memory().unwrap());
    db.save_entries(&entries).unwrap();
    let report = fixture.replace_at(db, &source, "output").await.unwrap();
    assert_eq!(report.reports["en"].strings_written, 2);
    assert_eq!(
        fs::read_to_string(fixture.temp.path().join("output/00.txt-en/00.txt")).unwrap(),
        ORIGINAL.replace("Original", "Second")
    );
    assert_eq!(
        sha256_file(&fixture.game.join("0.txt")).unwrap(),
        sibling_hash
    );
    assert_eq!(fs::read_to_string(source).unwrap(), ORIGINAL);
}
