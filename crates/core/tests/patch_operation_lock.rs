//! Cross-process patch exclusion. Barriers are outside the game; a child pauses
//! just before its first asset write, so a racing rollback cannot touch its journal.
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use locust_core::database::sha256_hex;
use locust_core::patch::manifest::{PatchFileEntry, PatchManifest};
use locust_core::patch::{apply, rollback, verify, ApplyOptions, RollbackOptions};

fn fixture(base: &Path, version: &str) -> (PathBuf, PathBuf) {
    let game = base.join("game");
    fs::create_dir_all(&game).unwrap();
    fs::write(game.join("asset.txt"), b"ORIGINAL").unwrap();
    let zip_path = base.join(format!("patch-{version}.zip"));
    let manifest = PatchManifest {
        game: None,
        schema_version: 1,
        patch_id: "locking-fixture".into(),
        game_name: "Neutral fixture".into(),
        engine: "test".into(),
        language: "es".into(),
        patch_version: version.into(),
        generator_version: "test".into(),
        created_at: "now".into(),
        files: vec![PatchFileEntry {
            path: "asset.txt".into(),
            patched_sha256: sha256_hex(b"PATCHED"),
            size: 7,
            original_sha256: Some(sha256_hex(b"ORIGINAL")),
        }],
    };
    let mut zip = zip::ZipWriter::new(File::create(&zip_path).unwrap());
    let options =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    zip.start_file("locust-patch.json", options).unwrap();
    zip.write_all(&serde_json::to_vec(&manifest).unwrap())
        .unwrap();
    zip.start_file("asset.txt", options).unwrap();
    zip.write_all(b"PATCHED").unwrap();
    zip.finish().unwrap();
    (game, zip_path)
}

struct Worker(Child);
impl Drop for Worker {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn wait_for(path: &Path) {
    let deadline = Instant::now() + Duration::from_secs(15);
    while !path.exists() {
        assert!(
            Instant::now() < deadline,
            "barrier timeout: {}",
            path.display()
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn worker(base: &Path, game: &Path, zip: &Path) -> Worker {
    let child = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "patch_lock_child", "--nocapture"])
        .env("LOCUST_LOCK_CHILD", "1")
        .env("LOCUST_DATA_DIR", base.join("isolated-child-profile"))
        .env("LOCUST_LOCK_GAME", game)
        .env("LOCUST_LOCK_ZIP", zip)
        .env("LOCUST_LOCK_BARRIER", base)
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .spawn()
        .unwrap();
    let worker = Worker(child);
    wait_for(&base.join("ready"));
    worker
}

#[test]
fn patch_lock_child() {
    if std::env::var_os("LOCUST_LOCK_CHILD").is_none() {
        return;
    }
    let game = PathBuf::from(std::env::var_os("LOCUST_LOCK_GAME").unwrap());
    let zip = PathBuf::from(std::env::var_os("LOCUST_LOCK_ZIP").unwrap());
    let base = PathBuf::from(std::env::var_os("LOCUST_LOCK_BARRIER").unwrap());
    apply(&game, &zip, ApplyOptions::default(), |_| {
        fs::write(
            base.join("ready"),
            b"journal armed before first asset write",
        )
        .unwrap();
        wait_for(&base.join("release"));
    })
    .unwrap();
}

#[test]
fn other_games_are_independent_and_process_death_releases_kernel_lock() {
    let temp = tempfile::tempdir().unwrap();
    let (game, zip) = fixture(temp.path(), "1.0.0");
    let mut child = worker(temp.path(), &game, &zip);
    let other = tempfile::tempdir().unwrap();
    let (other_game, other_zip) = fixture(other.path(), "1.0.0");
    apply(&other_game, &other_zip, ApplyOptions::default(), |_| {}).unwrap();
    rollback(&other_game, RollbackOptions::default()).unwrap();
    assert_eq!(fs::read(other_game.join("asset.txt")).unwrap(), b"ORIGINAL");
    child.0.kill().unwrap();
    child.0.wait().unwrap();
    // No PID inspection, stale lock removal, retry timeout or manual unlock.
    assert!(matches!(
        verify(&game, &zip).unwrap().outcome,
        locust_core::patch::VerificationOutcome::Interrupted
    ));
    rollback(&game, RollbackOptions::default()).unwrap();
    apply(&game, &zip, ApplyOptions::default(), |_| {}).unwrap();
    rollback(&game, RollbackOptions::default()).unwrap();
    assert_eq!(fs::read(game.join("asset.txt")).unwrap(), b"ORIGINAL");
}

#[test]
fn nested_callbacks_fail_fast_and_upgrade_reuses_held_lock() {
    let temp = tempfile::tempdir().unwrap();
    let (game, zip1) = fixture(temp.path(), "1.0.0");
    let (_, zip2) = fixture(temp.path(), "2.0.0");
    let mut callbacks = 0;
    apply(&game, &zip1, ApplyOptions::default(), |_| {
        callbacks += 1;
        assert!(verify(&game, &zip1)
            .unwrap_err()
            .to_string()
            .contains("busy"));
        assert!(rollback(&game, RollbackOptions::default())
            .unwrap_err()
            .to_string()
            .contains("busy"));
    })
    .unwrap();
    assert_eq!(callbacks, 1);
    // This invokes rollback internally: reacquiring the public lock would fail.
    apply(&game, &zip2, ApplyOptions::default(), |_| {}).unwrap();
    rollback(&game, RollbackOptions::default()).unwrap();
    assert_eq!(fs::read(game.join("asset.txt")).unwrap(), b"ORIGINAL");
}

#[test]
fn errors_and_panics_release_lock_without_discarding_recovery_state() {
    let temp = tempfile::tempdir().unwrap();
    let (game, zip) = fixture(temp.path(), "1.0.0");
    assert!(verify(&game, &temp.path().join("missing.zip")).is_err());
    assert!(apply(
        &game,
        &temp.path().join("missing.zip"),
        ApplyOptions::default(),
        |_| {}
    )
    .is_err());
    assert!(!game.join(".locust").exists());
    assert!(verify(&game, &zip).is_ok());
    let panic = std::panic::catch_unwind(|| {
        let _ = apply(&game, &zip, ApplyOptions::default(), |_| {
            panic!("simulated failure")
        });
    });
    assert!(panic.is_err());
    assert!(game.join(".locust/journal.json").exists());
    rollback(&game, RollbackOptions::default()).unwrap();
    assert!(verify(&game, &zip).is_ok());
}

#[cfg(windows)]
#[test]
fn windows_case_verbatim_and_root_link_aliases_share_lock() {
    let temp = tempfile::tempdir().unwrap();
    let (game, zip) = fixture(temp.path(), "1.0.0");
    let mut child = worker(temp.path(), &game, &zip);
    let uppercase = PathBuf::from(game.to_string_lossy().to_uppercase());
    for alias in [&uppercase, &game.canonicalize().unwrap(), &game.join(".")] {
        assert!(verify(alias, &zip)
            .unwrap_err()
            .to_string()
            .contains("busy"));
    }
    let alias = temp.path().join("game-link");
    std::os::windows::fs::symlink_dir(&game, &alias).unwrap();
    assert!(rollback(&alias, RollbackOptions::default())
        .unwrap_err()
        .to_string()
        .contains("busy"));
    fs::write(temp.path().join("release"), b"continue").unwrap();
    assert!(child.0.wait().unwrap().success());
    rollback(&alias, RollbackOptions::default()).unwrap();
    assert_eq!(fs::read(game.join("asset.txt")).unwrap(), b"ORIGINAL");
}

fn tree_snapshot(root: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    let mut result: Vec<_> = walkdir::WalkDir::new(root)
        .into_iter()
        .map(|item| item.unwrap())
        .filter(|item| item.file_type().is_file())
        .map(|item| {
            (
                item.path().strip_prefix(root).unwrap().to_owned(),
                fs::read(item.path()).unwrap(),
            )
        })
        .collect();
    result.sort_by(|a, b| a.0.cmp(&b.0));
    result
}

#[test]
fn racing_rollback_apply_and_verify_are_busy_before_any_game_mutation() {
    let temp = tempfile::tempdir().unwrap();
    let (game, zip) = fixture(temp.path(), "1.0.0");
    let mut child = worker(temp.path(), &game, &zip);
    let before = tree_snapshot(&game);
    let rollback_error = rollback(&game, RollbackOptions::default()).unwrap_err();
    assert!(
        rollback_error.to_string().contains("busy"),
        "{rollback_error}"
    );
    let apply_error = apply(
        &game,
        &zip,
        ApplyOptions {
            force: true,
            ..Default::default()
        },
        |_| panic!("loser callback"),
    )
    .unwrap_err();
    assert!(apply_error.to_string().contains("busy"), "{apply_error}");
    let verify_error = verify(&game, &zip).unwrap_err();
    assert!(verify_error.to_string().contains("busy"), "{verify_error}");
    assert_eq!(
        tree_snapshot(&game),
        before,
        "losing processes must preserve source, backup and journal bytes"
    );
    assert!(
        !temp.path().join("isolated-child-profile").exists(),
        "patch lock scope must ignore Locust profiles"
    );
    fs::write(temp.path().join("release"), b"continue").unwrap();
    assert!(child.0.wait().unwrap().success());
    rollback(&game, RollbackOptions::default()).unwrap();
    assert_eq!(fs::read(game.join("asset.txt")).unwrap(), b"ORIGINAL");
}
