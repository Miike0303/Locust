use std::fs;
use std::path::Path;

mod common;
use common::locust;

fn write_renpy_game(root: &Path) {
    fs::create_dir_all(root.join("game")).unwrap();
    fs::write(
        root.join("game/script.rpy"),
        "label start:\n    e \"Hello there!\"\n    e \"See you soon.\"\n",
    )
    .unwrap();
}

fn listing(root: &Path) -> Vec<(String, Vec<u8>)> {
    let mut files = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            let rel = path
                .strip_prefix(root)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            // These two exact roots hold operation receipts, not game payload.
            // Refusal checks below include both without filtering any bytes.
            if matches!(rel.as_str(), ".locust" | ".locust-injections") {
                continue;
            }
            if path.is_dir() {
                stack.push(path);
            } else {
                files.push((rel, fs::read(&path).unwrap()));
            }
        }
    }
    files.sort();
    files
}

// Unlike the public payload listing, refusal snapshots include every directory
// and byte: game metadata, profile, backup index/counters, DB and packed ZIP.
fn persisted_tree(root: &Path) -> Vec<(String, Option<Vec<u8>>)> {
    let mut entries = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            let rel = path
                .strip_prefix(root)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            let bytes = if path.is_dir() {
                stack.push(path.clone());
                None
            } else {
                Some(fs::read(path).unwrap())
            };
            entries.push((rel, bytes));
        }
    }
    entries.sort();
    entries
}

fn lifecycle_locust(root: &Path) -> assert_cmd::Command {
    let mut command = locust();
    command
        .env("LOCUST_DATA_DIR", root.join("profile"))
        .env("LOCUST_BACKUP_ROOT", root.join("backups"));
    command
}

fn assert_original_status(root: &Path, game: &Path) {
    let before = persisted_tree(root);
    lifecycle_locust(root)
        .arg("patch-status")
        .arg(game)
        .assert()
        .code(0)
        .stdout("Original game (no recorded Locust changes)\n")
        .stderr("");
    assert_eq!(persisted_tree(root), before, "status must be read-only");
}

fn assert_applied_status(root: &Path, game: &Path, identity: &str) {
    let before = persisted_tree(root);
    let result = lifecycle_locust(root)
        .arg("patch-status")
        .arg(game)
        .arg("--verify")
        .assert()
        .code(0)
        .stderr("");
    let stdout = std::str::from_utf8(&result.get_output().stdout).unwrap();
    let prefix = format!(
        "verified 2 patch file(s) against the receipt\n\
         patched: {identity} (engine renpy, lang es, baseline Unverified, forced=false)\n\
         \x20\x20replaced 0 file(s), added 2 file(s), applied_at "
    );
    // Only the documented operation time varies. Check the entire remaining
    // output as one RFC3339 timestamp, rather than masking receipt metadata.
    let timestamp = stdout
        .strip_prefix(&prefix)
        .unwrap_or_else(|| panic!("unexpected patch status: {stdout:?}"))
        .strip_suffix('\n')
        .unwrap();
    chrono::DateTime::parse_from_rfc3339(timestamp).unwrap();
    assert_eq!(
        persisted_tree(root),
        before,
        "verification must be read-only"
    );
}

#[test]
fn an_add_patch_applies_to_a_pristine_copy_rolls_back_and_reapplies() {
    let dir = tempfile::tempdir().unwrap();
    let game = dir.path().join("Game");
    let fresh = dir.path().join("Fresh");
    write_renpy_game(&game);
    write_renpy_game(&fresh);
    let pristine = listing(&fresh);
    let db = dir.path().join("Game.locust.db");
    let zip = dir.path().join("add.zip");

    lifecycle_locust(dir.path())
        .arg("extract")
        .arg(&game)
        .arg("-o")
        .arg(&db)
        .assert()
        .success();
    lifecycle_locust(dir.path())
        .arg("translate")
        .arg(&db)
        .args(["-p", "mock", "-s", "en", "-t", "es"])
        .assert()
        .success();
    lifecycle_locust(dir.path())
        .arg("inject")
        .arg(&game)
        .arg("-P")
        .arg(&db)
        .args(["-m", "add", "-l", "es"])
        .assert()
        .success();
    lifecycle_locust(dir.path())
        .arg("patch")
        .arg(&game)
        .arg("-P")
        .arg(&db)
        .args(["-l", "es", "-o"])
        .arg(&zip)
        .assert()
        .success();

    let expected = listing(&game);
    assert_eq!(
        expected
            .iter()
            .map(|(rel, _)| rel.as_str())
            .collect::<Vec<_>>(),
        [
            "game/locust_languages.rpy",
            "game/script.rpy",
            "game/tl/es/script.rpy"
        ]
    );
    assert_eq!(
        fs::read(game.join("game/script.rpy")).unwrap(),
        pristine[0].1
    );
    let mut archive = zip::ZipArchive::new(fs::File::open(&zip).unwrap()).unwrap();
    let manifest: serde_json::Value =
        serde_json::from_reader(archive.by_name("locust-patch.json").unwrap()).unwrap();
    let identity = format!(
        "{}@{}",
        manifest["patch_id"].as_str().unwrap(),
        manifest["patch_version"].as_str().unwrap()
    );
    drop(archive);

    assert_original_status(dir.path(), &fresh);
    let before_refusal = persisted_tree(dir.path());
    lifecycle_locust(dir.path())
        .arg("apply")
        .arg(&fresh)
        .arg(&zip)
        .assert()
        .code(1)
        .stdout("")
        .stderr(
            "Error: legacy patch requires explicit confirmation: structural-tier patch \
             (no original hashes) requires --confirm-legacy or --force\n",
        );
    assert_eq!(persisted_tree(dir.path()), before_refusal);
    assert_original_status(dir.path(), &fresh);

    // Every file of an Add patch is new: their absence is expected, not a
    // sign of the wrong game.
    let applied_stdout = format!(
        "[1/2] game/locust_languages.rpy (write)\n\
         [2/2] game/tl/es/script.rpy (write)\n\
         patch {identity} applied — replaced 0, added 2 (baseline: Unverified)\n"
    );
    lifecycle_locust(dir.path())
        .arg("apply")
        .arg(&fresh)
        .arg(&zip)
        .arg("--confirm-legacy")
        .assert()
        .code(0)
        .stdout(applied_stdout.clone())
        .stderr("");
    assert!(fresh.join("game/tl/es/script.rpy").is_file());
    let patched = listing(&fresh);
    assert_eq!(patched, expected, "install the complete injected payload");
    assert_eq!(
        fs::read(fresh.join("game/script.rpy")).unwrap(),
        pristine[0].1
    );
    assert_eq!(
        patched
            .iter()
            .filter(|(rel, _)| rel == "game/script.rpy")
            .count(),
        1
    );

    assert_applied_status(dir.path(), &fresh, &identity);
    let rollback_stdout = "restored to pre-apply state, NOT verified pristine\n\
                           rollback complete — restored 0, deleted 2\n";
    lifecycle_locust(dir.path())
        .arg("patch-rollback")
        .arg(&fresh)
        .assert()
        .code(0)
        .stdout(rollback_stdout)
        .stderr("");
    assert_eq!(listing(&fresh), pristine);
    assert_original_status(dir.path(), &fresh);

    lifecycle_locust(dir.path())
        .arg("apply")
        .arg(&fresh)
        .arg(&zip)
        .arg("--confirm-legacy")
        .assert()
        .code(0)
        .stdout(applied_stdout)
        .stderr("");
    assert_eq!(
        listing(&fresh),
        patched,
        "reapply must reproduce every byte"
    );
    assert_applied_status(dir.path(), &fresh, &identity);
    lifecycle_locust(dir.path())
        .arg("patch-rollback")
        .arg(&fresh)
        .assert()
        .code(0)
        .stdout(rollback_stdout)
        .stderr("");
    assert_eq!(listing(&fresh), pristine);
    assert_original_status(dir.path(), &fresh);
}

#[test]
fn an_add_patch_still_refuses_a_game_that_already_has_its_files() {
    let dir = tempfile::tempdir().unwrap();
    let game = dir.path().join("Game");
    let other = dir.path().join("Other");
    write_renpy_game(&game);
    write_renpy_game(&other);
    let db = dir.path().join("Game.locust.db");
    let zip = dir.path().join("add.zip");
    locust()
        .arg("extract")
        .arg(&game)
        .arg("-o")
        .arg(&db)
        .assert()
        .success();
    locust()
        .arg("translate")
        .arg(&db)
        .args(["-p", "mock", "-s", "en", "-t", "es"])
        .assert()
        .success();
    locust()
        .arg("inject")
        .arg(&game)
        .arg("-P")
        .arg(&db)
        .args(["-m", "add", "-l", "es"])
        .assert()
        .success();
    locust()
        .arg("patch")
        .arg(&game)
        .arg("-P")
        .arg(&db)
        .args(["-l", "es", "-o"])
        .arg(&zip)
        .assert()
        .success();
    // A different translation already occupies one of the added paths.
    fs::create_dir_all(other.join("game/tl/es")).unwrap();
    fs::write(
        other.join("game/tl/es/script.rpy"),
        "# someone else's translation\n",
    )
    .unwrap();
    let output = locust()
        .arg("apply")
        .arg(&other)
        .arg(&zip)
        .arg("--confirm-legacy")
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert_eq!(
        fs::read_to_string(other.join("game/tl/es/script.rpy")).unwrap(),
        "# someone else's translation\n"
    );
}
