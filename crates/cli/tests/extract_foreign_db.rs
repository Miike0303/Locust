use std::fs;
use std::path::{Path, PathBuf};

use locust_core::database::{Database, EntryFilter};
use serde_json::{json, Value};

mod common;
use common::locust;

struct Fixture {
    dir: tempfile::TempDir,
    a: PathBuf,
    b: PathBuf,
    db: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("A/Game");
        let b = dir.path().join("B/Game");
        for (root, script, text) in [
            (&a, "a.rpy", "Hello from the first game!"),
            (&b, "b.rpy", "Welcome to the second game!"),
        ] {
            fs::create_dir_all(root.join("game")).unwrap();
            fs::write(
                root.join("game").join(script),
                format!("label start:\n    e \"{text}\"\n"),
            )
            .unwrap();
        }
        let db = dir.path().join("Game.locust.db");
        let fixture = Self { dir, a, b, db };
        fixture.extract(Path::new("A/Game")).assert().success();
        locust()
            .arg("translate")
            .arg(&fixture.db)
            .args(["-p", "mock", "-s", "en", "-t", "es"])
            .assert()
            .success();
        let entries = fixture.rows();
        assert_eq!(entries.as_array().unwrap().len(), 1);
        assert!(entries[0]["translation"].as_str().is_some());
        fixture
    }

    fn extract(&self, game: &Path) -> assert_cmd::Command {
        let mut command = locust();
        command
            .current_dir(self.dir.path())
            .arg("extract")
            .arg(game);
        command
    }

    fn rows(&self) -> Value {
        let db = Database::open(&self.db).unwrap();
        json!(db.get_entries(&EntryFilter::default()).unwrap())
    }

    fn root(&self) -> Option<Value> {
        Database::open(&self.db)
            .unwrap()
            .get_project_metadata("game_root")
            .unwrap()
    }

    fn assert_translation_preserved(&self, before: &Value) {
        let after = self.rows();
        assert_eq!(after.as_array().unwrap().len(), 1);
        // Re-extraction may update the spelling of file_path to the alias used.
        for field in [
            "id",
            "source",
            "translation",
            "status",
            "provider_used",
            "translated_at",
            "reviewed_at",
            "created_at",
        ] {
            assert_eq!(after[0][field], before[0][field], "{field}");
        }
    }

    fn make_legacy(&self) {
        rusqlite::Connection::open(&self.db)
            .unwrap()
            .execute_batch("DROP TABLE project_metadata")
            .unwrap();
    }

    fn assert_refused(&self, command: &mut assert_cmd::Command) {
        let before = self.rows();
        let assertion = command.assert().failure();
        let output = assertion.get_output();
        let error = String::from_utf8_lossy(&output.stderr);
        for root in [&self.a, &self.b] {
            assert!(
                error.contains(&root.canonicalize().unwrap().display().to_string()),
                "missing root in error: {error}"
            );
        }
        assert!(error.contains("-o <other.db>"), "{error}");
        assert!(error.contains("--force"), "{error}");
        assert!(!String::from_utf8_lossy(&output.stdout).contains("Translations lost"));
        assert_eq!(self.rows(), before, "refusal must preserve translations");
        assert_eq!(self.root(), Some(json!(self.a.canonicalize().unwrap())));
    }
}

#[test]
fn default_output_refuses_another_game_with_the_same_folder_name() {
    let fixture = Fixture::new();
    fixture.assert_refused(&mut fixture.extract(&fixture.b));
}

#[test]
fn explicit_output_refuses_another_games_database() {
    let fixture = Fixture::new();
    fixture.assert_refused(fixture.extract(&fixture.b).arg("-o").arg(&fixture.db));

    // The suggested alternative output must actually unblock extraction.
    let other = fixture.dir.path().join("other.db");
    fixture
        .extract(&fixture.b)
        .arg("-o")
        .arg(&other)
        .assert()
        .success();
    assert_eq!(
        Database::open(&other)
            .unwrap()
            .get_project_metadata("game_root")
            .unwrap(),
        Some(json!(fixture.b.canonicalize().unwrap()))
    );
    assert!(fixture.rows()[0]["translation"].as_str().is_some());
}

#[test]
fn dry_run_refuses_foreign_roots_for_default_and_explicit_outputs() {
    let fixture = Fixture::new();
    let before = fs::read(&fixture.db).unwrap();
    fixture.assert_refused(fixture.extract(&fixture.b).arg("--dry-run"));
    fixture.assert_refused(
        fixture
            .extract(&fixture.b)
            .arg("-o")
            .arg(&fixture.db)
            .arg("--dry-run"),
    );
    assert_eq!(fs::read(&fixture.db).unwrap(), before);
}

#[test]
fn force_merges_foreign_roots_and_records_the_new_root() {
    for explicit in [false, true] {
        let fixture = Fixture::new();
        let mut command = fixture.extract(&fixture.b);
        if explicit {
            command.arg("-o").arg(&fixture.db);
        }
        command.arg("--force").assert().success();
        let db = Database::open(&fixture.db).unwrap();
        assert!(db.get_entry("a.rpy#2").unwrap().is_none());
        assert_eq!(
            db.get_entry("b.rpy#2").unwrap().unwrap().source,
            "Welcome to the second game!"
        );
        drop(db);
        assert_eq!(
            fixture.root(),
            Some(json!(fixture.b.canonicalize().unwrap()))
        );
        fixture.extract(&fixture.b).assert().success();
    }
}

#[test]
fn same_game_reextract_accepts_canonical_alias_and_preserves_translation() {
    let fixture = Fixture::new();
    let before = fixture.rows();
    fixture
        .extract(&fixture.a.join("game").join(".."))
        .arg("-o")
        .arg(&fixture.db)
        .assert()
        .success();
    fixture.assert_translation_preserved(&before);
    assert_eq!(
        fixture.root(),
        Some(json!(fixture.a.canonicalize().unwrap()))
    );
}

#[cfg(windows)]
#[test]
fn same_game_reextract_accepts_windows_case_variants() {
    let fixture = Fixture::new();
    let before = fixture.rows();
    let uppercase = PathBuf::from(fixture.a.to_str().unwrap().to_uppercase());
    fixture
        .extract(&uppercase)
        .arg("-o")
        .arg(&fixture.db)
        .assert()
        .success();
    fixture.assert_translation_preserved(&before);
}

#[test]
fn legacy_database_merges_and_adopts_the_canonical_root() {
    let fixture = Fixture::new();
    let before = fixture.rows();
    fixture.make_legacy();
    fixture.extract(&fixture.a).assert().success();
    fixture.assert_translation_preserved(&before);
    assert_eq!(
        fixture.root(),
        Some(json!(fixture.a.canonicalize().unwrap()))
    );
    fixture.assert_refused(&mut fixture.extract(&fixture.b));
}

#[test]
fn legacy_dry_run_does_not_record_a_root_or_migrate_the_database() {
    let fixture = Fixture::new();
    fixture.make_legacy();
    let before = fs::read(&fixture.db).unwrap();
    fixture
        .extract(&fixture.b)
        .arg("--dry-run")
        .assert()
        .success();
    assert_eq!(fs::read(&fixture.db).unwrap(), before);
    assert_eq!(fixture.root(), None);
}

#[test]
fn forced_dry_run_previews_without_changing_entries_or_root() {
    let fixture = Fixture::new();
    let before = fixture.rows();
    let bytes = fs::read(&fixture.db).unwrap();
    fixture
        .extract(&fixture.b)
        .args(["--force", "--dry-run"])
        .assert()
        .success()
        .stdout(predicates::str::contains("Dry run: nothing was written"));
    assert_eq!(fs::read(&fixture.db).unwrap(), bytes);
    assert_eq!(fixture.rows(), before);
    assert_eq!(
        fixture.root(),
        Some(json!(fixture.a.canonicalize().unwrap()))
    );
}
