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
            if rel.starts_with(".locust") {
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

#[test]
fn an_add_patch_applies_to_a_pristine_copy_and_rolls_back() {
    let dir = tempfile::tempdir().unwrap();
    let game = dir.path().join("Game");
    let fresh = dir.path().join("Fresh");
    write_renpy_game(&game);
    write_renpy_game(&fresh);
    let pristine = listing(&fresh);
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

    // Every file of an Add patch is new: their absence is expected, not a
    // sign of the wrong game.
    let output = locust()
        .arg("apply")
        .arg(&fresh)
        .arg(&zip)
        .arg("--confirm-legacy")
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{stderr}");
    assert!(fresh.join("game/tl/es/script.rpy").is_file());
    let patched = listing(&fresh);
    assert_eq!(
        patched
            .iter()
            .filter(|(rel, _)| rel == "game/script.rpy")
            .count(),
        1
    );

    locust()
        .arg("patch-rollback")
        .arg(&fresh)
        .assert()
        .success();
    assert_eq!(listing(&fresh), pristine);
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
