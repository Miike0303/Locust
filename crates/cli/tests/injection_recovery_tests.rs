use serde_json::{json, Value};
use std::{fs, path::Path};

mod common;

fn invoke(command: &str, game: &Path) -> Value {
    let result = common::locust().arg(command).arg(game).assert().success();
    serde_json::from_slice(&result.get_output().stdout).unwrap()
}

#[test]
fn cli_reports_and_recovers_interrupted_preparation_via_selected_file() {
    let temp = tempfile::tempdir().unwrap();
    let game = temp.path();
    let selected = game.join("story.html");
    fs::write(&selected, "original 日本語").unwrap();
    let store = game.join(".locust-injections");
    let id = "dc32730c-7269-4320-8d3a-c27265cb985d";
    let operation = store.join("operations").join(id);
    fs::create_dir_all(&operation).unwrap();
    fs::write(store.join("store.json"), serde_json::to_vec(&json!({
        "schema_version":1,"kind":"locust-injection-store","game_root":game.canonicalize().unwrap()
    })).unwrap()).unwrap();
    fs::write(
        store.join("active.json"),
        serde_json::to_vec(&json!({
            "schema_version":1,"transaction_id":id,"game_root":game.canonicalize().unwrap(),
            "format":"html-game","language":"es"
        }))
        .unwrap(),
    )
    .unwrap();
    fs::write(operation.join("phase.json"), b"\"preparing\"").unwrap();

    let pending = invoke("inject-status", &selected);
    assert_eq!(pending["pending"]["phase"], "preparing");
    assert_eq!(pending["pending"]["transaction_id"], id);
    let recovered = invoke("inject-recover", &selected);
    assert_eq!(recovered["transaction_id"], id);
    assert_eq!(recovered["restored"], 0);
    assert!(invoke("inject-status", game)["pending"].is_null());
    assert!(invoke("inject-recover", game)["transaction_id"].is_null());
    assert_eq!(fs::read_to_string(&selected).unwrap(), "original 日本語");
}

#[test]
fn unknown_recovery_directory_returns_failure_and_preserves_user_files() {
    let temp = tempfile::tempdir().unwrap();
    let store = temp.path().join(".locust-injections");
    fs::create_dir(&store).unwrap();
    fs::write(store.join("mine.txt"), b"user-owned").unwrap();
    for command in ["inject-status", "inject-recover"] {
        common::locust()
            .arg(command)
            .arg(temp.path())
            .assert()
            .failure();
        assert_eq!(fs::read(store.join("mine.txt")).unwrap(), b"user-owned");
        assert!(!store.join("store.json").exists());
    }
}
