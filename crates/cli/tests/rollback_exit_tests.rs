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
