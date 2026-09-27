use locust_core::database::sha256_hex;
use serde_json::json;

mod common;

#[test]
fn patch_status_verify_detects_changed_and_missing_files() {
    let game = tempfile::tempdir().unwrap();
    let store = game.path().join(".locust");
    std::fs::create_dir(&store).unwrap();
    std::fs::write(game.path().join("replaced.txt"), b"patched").unwrap();
    std::fs::write(game.path().join("added.txt"), b"added").unwrap();
    std::fs::write(
        store.join("receipt.json"),
        serde_json::to_vec(&json!({
            "schema_version": 1, "patch_id": "fixture", "patch_version": "1",
            "generator_version": "test", "language": "es", "engine": "html",
            "applied_at": "fixture", "verification": "strict", "forced": false,
            "baseline": "pristine", "created_dirs": [],
            "replaced": [{"path": "replaced.txt", "patched_sha256": sha256_hex(b"patched")}],
            "added": [{"path": "added.txt", "patched_sha256": sha256_hex(b"added")}]
        }))
        .unwrap(),
    )
    .unwrap();

    common::locust()
        .arg("patch-status")
        .arg(game.path())
        .arg("--verify")
        .assert()
        .success()
        .stdout(predicates::str::contains("verified 2 patch file(s)"));

    std::fs::write(game.path().join("replaced.txt"), b"user edit").unwrap();
    std::fs::remove_file(game.path().join("added.txt")).unwrap();
    common::locust()
        .arg("patch-status")
        .arg(game.path())
        .arg("--verify")
        .assert()
        .failure()
        .stdout(predicates::str::contains("changed: replaced.txt"))
        .stdout(predicates::str::contains("missing: added.txt"))
        .stderr(predicates::str::contains("2 patch file(s) differ"));
}
