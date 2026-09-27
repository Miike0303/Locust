use locust_core::database::sha256_hex;
use serde_json::json;

mod common;

#[test]
fn interrupted_rollback_reports_failure_until_modified_added_deletion_is_forced() {
    let game = tempfile::tempdir().unwrap();
    let store = game.path().join(".locust");
    std::fs::create_dir_all(store.join("backup/files")).unwrap();
    std::fs::write(game.path().join("added.txt"), b"user edit").unwrap();
    std::fs::write(
        store.join("backup/manifest.json"),
        serde_json::to_vec(&json!({
            "schema_version": 1, "created_at": "fixture", "baseline": "pristine", "files": []
        }))
        .unwrap(),
    )
    .unwrap();
    std::fs::write(
        store.join("journal.json"),
        serde_json::to_vec(&json!({
            "schema_version": 1, "state": "applying", "patch_id": "fixture",
            "plan": { "patch_version": "1.0.0", "language": "es", "engine": "html",
                "generator_version": "test", "verification": "strict", "forced": false,
                "baseline": "pristine", "replaced": [], "created_dirs": [],
                "added": [{"path":"added.txt", "patched_sha256":sha256_hex(b"patch output")}] }
        }))
        .unwrap(),
    )
    .unwrap();
    let journal = std::fs::read(store.join("journal.json")).unwrap();
    common::locust()
        .arg("patch-rollback")
        .arg(game.path())
        .assert()
        .failure()
        .stderr(predicates::str::contains(
            "modified added files were preserved",
        ));
    assert_eq!(
        std::fs::read(game.path().join("added.txt")).unwrap(),
        b"user edit"
    );
    assert_eq!(std::fs::read(store.join("journal.json")).unwrap(), journal);
    common::locust()
        .arg("patch-rollback")
        .arg(game.path())
        .arg("--force")
        .assert()
        .success();
    assert!(!game.path().join("added.txt").exists());
    assert!(!store.exists());
}

#[test]
fn forced_completed_rollback_reports_success_after_deleting_edited_added_file() {
    let game = tempfile::tempdir().unwrap();
    let store = game.path().join(".locust");
    std::fs::create_dir_all(store.join("backup/files")).unwrap();
    std::fs::write(game.path().join("replaced.txt"), b"patched").unwrap();
    std::fs::write(game.path().join("added.txt"), b"user edit").unwrap();
    std::fs::write(store.join("backup/files/replaced.txt"), b"original").unwrap();
    std::fs::write(
        store.join("backup/manifest.json"),
        serde_json::to_vec(&json!({
            "schema_version": 1, "created_at": "fixture", "baseline": "pristine",
            "files": [{"path": "replaced.txt", "sha256": sha256_hex(b"original"), "size": 8}]
        }))
        .unwrap(),
    )
    .unwrap();
    std::fs::write(
        store.join("receipt.json"),
        serde_json::to_vec(&json!({
            "schema_version": 1, "patch_id": "fixture", "patch_version": "1.0.0",
            "generator_version": "test", "language": "es", "engine": "html",
            "applied_at": "fixture", "verification": "strict", "forced": false,
            "baseline": "pristine", "created_dirs": [],
            "replaced": [{"path": "replaced.txt", "patched_sha256": sha256_hex(b"patched")}],
            "added": [{"path": "added.txt", "patched_sha256": sha256_hex(b"patch output")}]
        }))
        .unwrap(),
    )
    .unwrap();

    common::locust()
        .arg("patch-rollback")
        .arg(game.path())
        .arg("--force")
        .assert()
        .success()
        .stdout(predicates::str::contains(
            "rollback complete — restored 1, deleted 1",
        ));
    assert_eq!(
        std::fs::read(game.path().join("replaced.txt")).unwrap(),
        b"original"
    );
    assert!(!game.path().join("added.txt").exists());
    assert!(!store.exists());
}

#[test]
fn rollback_dry_run_previews_forced_deletion_without_changing_game() {
    let game = tempfile::tempdir().unwrap();
    let store = game.path().join(".locust");
    std::fs::create_dir_all(store.join("backup/files")).unwrap();
    std::fs::write(game.path().join("replaced.txt"), b"patched").unwrap();
    std::fs::write(game.path().join("added.txt"), b"user edit").unwrap();
    std::fs::write(store.join("backup/files/replaced.txt"), b"original").unwrap();
    std::fs::write(
        store.join("backup/manifest.json"),
        serde_json::to_vec(&json!({
            "schema_version": 1, "created_at": "fixture", "baseline": "pristine",
            "files": [{"path": "replaced.txt", "sha256": sha256_hex(b"original"), "size": 8}]
        }))
        .unwrap(),
    )
    .unwrap();
    std::fs::write(
        store.join("receipt.json"),
        serde_json::to_vec(&json!({
            "schema_version": 1, "patch_id": "fixture", "patch_version": "1.0.0",
            "generator_version": "test", "language": "es", "engine": "html",
            "applied_at": "fixture", "verification": "strict", "forced": false,
            "baseline": "pristine", "created_dirs": [],
            "replaced": [{"path": "replaced.txt", "patched_sha256": sha256_hex(b"patched")}],
            "added": [{"path": "added.txt", "patched_sha256": sha256_hex(b"patch output")}]
        }))
        .unwrap(),
    )
    .unwrap();

    common::locust()
        .arg("patch-rollback")
        .arg(game.path())
        .arg("--force")
        .arg("--dry-run")
        .assert()
        .success()
        .stdout(predicates::str::contains(
            "rollback planned — restore 1, delete 1",
        ))
        .stdout(predicates::str::contains("added.txt"));
    assert_eq!(
        std::fs::read(game.path().join("replaced.txt")).unwrap(),
        b"patched"
    );
    assert_eq!(
        std::fs::read(game.path().join("added.txt")).unwrap(),
        b"user edit"
    );
    assert!(store.join("receipt.json").is_file());
    assert!(store.join("backup/manifest.json").is_file());
}
