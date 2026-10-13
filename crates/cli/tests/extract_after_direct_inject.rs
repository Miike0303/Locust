use std::fs;
use std::path::PathBuf;

use locust_core::database::{Database, EntryFilter};
use serde_json::{json, Value};

mod common;
use common::locust;

/// A Ren'Py game extracted, translated and Direct-injected in place.
struct InjectedGame {
    dir: tempfile::TempDir,
    game: PathBuf,
    db: PathBuf,
}

impl InjectedGame {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let game = dir.path().join("Game");
        fs::create_dir_all(game.join("game")).unwrap();
        fs::write(
            game.join("game/script.rpy"),
            "label start:\n    e \"Hello there!\"\n    e \"See you soon.\"\n",
        )
        .unwrap();
        let db = dir.path().join("Game.locust.db");
        let fixture = Self { dir, game, db };
        fixture.extract().assert().success();
        locust()
            .arg("translate")
            .arg(&fixture.db)
            .args(["-p", "mock", "-s", "en", "-t", "es"])
            .assert()
            .success();
        locust()
            .arg("inject")
            .arg(&fixture.game)
            .arg("-P")
            .arg(&fixture.db)
            .args(["--direct", "-l", "es"])
            .assert()
            .success();
        fixture
    }

    fn extract(&self) -> assert_cmd::Command {
        let mut command = locust();
        command
            .current_dir(self.dir.path())
            .arg("extract")
            .arg(&self.game)
            .arg("-o")
            .arg(&self.db);
        command
    }

    fn rows(&self) -> Value {
        let db = Database::open(&self.db).unwrap();
        json!(db.get_entries(&EntryFilter::default()).unwrap())
    }
}

#[test]
fn extract_refuses_a_game_with_a_direct_injection_and_keeps_the_project() {
    let fixture = InjectedGame::new();
    let before = fixture.rows();
    assert!(before
        .as_array()
        .unwrap()
        .iter()
        .all(|row| row["translation"].is_string()));

    let output = fixture.extract().output().unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success(), "{stderr}");
    assert!(
        stderr.contains("Direct injection") && stderr.contains("--force"),
        "{stderr}"
    );
    assert_eq!(fixture.rows(), before, "the project must stay untouched");
}

#[test]
fn dry_run_warns_about_a_direct_injection() {
    let fixture = InjectedGame::new();
    let output = fixture.extract().arg("--dry-run").output().unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{stderr}");
    assert!(stderr.contains("Direct injection"), "{stderr}");
}

#[test]
fn force_extracts_the_injected_text_deliberately() {
    let fixture = InjectedGame::new();
    fixture.extract().arg("--force").assert().success();
}
