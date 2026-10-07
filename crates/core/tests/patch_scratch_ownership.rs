use locust_core::database::sha256_hex;
use locust_core::patch::{
    apply, rollback, ApplyOptions, PatchFileEntry, PatchManifest, PatchStatus, PatchStore,
    RollbackOptions,
};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};

fn game() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir(dir.path().join("fonts")).unwrap();
    fs::write(dir.path().join("fonts/game.ttf"), b"orig").unwrap();
    dir
}

fn patch(root: &Path, entry_name: &str, added: bool) -> PathBuf {
    let mut files = vec![PatchFileEntry {
        path: entry_name.into(),
        size: 4,
        patched_sha256: sha256_hex(b"FONT"),
        original_sha256: Some(sha256_hex(b"orig")),
    }];
    if added {
        files.push(PatchFileEntry {
            path: "fonts/added.ttf".into(),
            size: 3,
            patched_sha256: sha256_hex(b"ADD"),
            original_sha256: None,
        });
    }
    let manifest = PatchManifest {
        game: None,
        schema_version: 1,
        patch_id: "font".into(),
        game_name: "game".into(),
        engine: "html".into(),
        language: "es".into(),
        patch_version: "1.0.0".into(),
        generator_version: "0.1.0".into(),
        created_at: "now".into(),
        files,
    };
    let path = root.join("font.zip");
    let mut zip = zip::ZipWriter::new(fs::File::create(&path).unwrap());
    let options = zip::write::SimpleFileOptions::default();
    zip.start_file("locust-patch.json", options).unwrap();
    zip.write_all(&serde_json::to_vec(&manifest).unwrap())
        .unwrap();
    zip.start_file(entry_name, options).unwrap();
    zip.write_all(b"FONT").unwrap();
    if added {
        zip.start_file("fonts/added.ttf", options).unwrap();
        zip.write_all(b"ADD").unwrap();
    }
    zip.finish().unwrap();
    path
}

fn sentinels(root: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    [
        "fonts/game.ttf.locust-tmp",
        "fonts/game.ttf.locust-old",
        "unrelated.locust-tmp",
        ".locust-probe-user",
        "saves/deep/recovery.locust-tmp",
        "saves/deep/.locust-probe-user",
        "saves/deep/other-recovery.LOCUST-TMP",
        "folder.locust-tmp/keep.bin",
        ".locust-stage-user/keep.bin",
    ]
    .into_iter()
    .enumerate()
    .map(|(i, rel)| {
        let path = root.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let bytes = format!("user-owned sentinel {i}\0\u{fffd}").into_bytes();
        fs::write(&path, &bytes).unwrap();
        (path, bytes)
    })
    .collect()
}

fn assert_preserved(sentinels: &[(PathBuf, Vec<u8>)]) {
    for (path, bytes) in sentinels {
        assert_eq!(&fs::read(path).unwrap(), bytes, "{}", path.display());
    }
}

#[test]
fn success_and_rollback_preserve_unowned_scratch_names_and_recursive_sentinels() {
    for entry_name in ["fonts/game.ttf", r"fonts\.\game.ttf."] {
        let dir = game();
        let root = dir.path();
        let keep = sentinels(root);
        let zip = patch(root, entry_name, true);
        apply(root, &zip, ApplyOptions::default(), |_| {}).unwrap();
        assert_eq!(fs::read(root.join("fonts/game.ttf")).unwrap(), b"FONT");
        assert_preserved(&keep);
        rollback(root, RollbackOptions::default()).unwrap();
        assert_eq!(fs::read(root.join("fonts/game.ttf")).unwrap(), b"orig");
        assert!(!root.join("fonts/added.ttf").exists());
        assert_preserved(&keep);
        assert_eq!(
            fs::read_dir(root)
                .unwrap()
                .filter(|e| e
                    .as_ref()
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".locust-stage-"))
                .count(),
            1
        );
    }
}

#[test]
fn failed_replacement_restores_original_and_never_claims_siblings() {
    let dir = game();
    let root = dir.path();
    let keep = sentinels(root);
    assert!(
        PatchStore::replace_file(&root.join("missing-input"), &root.join("fonts/game.ttf"))
            .is_err()
    );
    assert_eq!(fs::read(root.join("fonts/game.ttf")).unwrap(), b"orig");
    assert_preserved(&keep);
    // A directory is never a file replacement target, even when empty.
    let directory = root.join("directory");
    fs::create_dir(&directory).unwrap();
    let input = root.join("input");
    fs::write(&input, b"input").unwrap();
    assert!(PatchStore::replace_file(&input, &directory).is_err());
    assert!(directory.is_dir());
    assert_eq!(fs::read(input).unwrap(), b"input");
}

#[test]
fn early_write_failure_and_recovery_preserve_scratch_files() {
    let dir = game();
    let root = dir.path();
    let keep = sentinels(root);
    let zip = patch(root, "fonts/game.ttf", false);
    let path = root.join("fonts/game.ttf");
    let original_permissions = fs::metadata(&path).unwrap().permissions();
    assert!(apply(root, &zip, ApplyOptions::default(), |_| {
        let mut readonly = original_permissions.clone();
        readonly.set_readonly(true);
        fs::set_permissions(&path, readonly).unwrap();
    })
    .is_err());
    fs::set_permissions(&path, original_permissions).unwrap();
    assert_eq!(fs::read(&path).unwrap(), b"orig");
    assert_preserved(&keep);
    assert!(matches!(
        PatchStore::new(root).status().unwrap(),
        PatchStatus::Interrupted(_)
    ));
    rollback(root, RollbackOptions::default()).unwrap();
    assert_preserved(&keep);
}

#[test]
fn interrupted_recovery_preserves_unknown_scratch_and_owned_crash_leftovers() {
    let dir = game();
    let root = dir.path();
    let keep = sentinels(root);
    let zip = patch(root, "fonts/game.ttf", true);
    let interrupted = std::panic::catch_unwind(|| {
        apply(root, &zip, ApplyOptions::default(), |progress| {
            if progress.current == 2 {
                panic!("simulated interruption after first write");
            }
        })
        .unwrap();
    });
    assert!(interrupted.is_err());
    assert_eq!(fs::read(root.join("fonts/game.ttf")).unwrap(), b"FONT");
    assert_preserved(&keep);
    // A prior process may have left an aside. Its name alone confers no cleanup authority.
    let abandoned = root.join("fonts/.locust-stage-previous-process/previous");
    fs::create_dir_all(abandoned.parent().unwrap()).unwrap();
    fs::write(&abandoned, b"retained crash copy").unwrap();
    assert!(apply(
        root,
        &zip,
        ApplyOptions {
            force: true,
            ..Default::default()
        },
        |_| {}
    )
    .is_err());
    rollback(root, RollbackOptions::default()).unwrap();
    assert_eq!(fs::read(root.join("fonts/game.ttf")).unwrap(), b"orig");
    assert_eq!(fs::read(abandoned).unwrap(), b"retained crash copy");
    assert_preserved(&keep);
}

#[test]
fn force_rollback_deletes_only_recorded_added_file() {
    let dir = game();
    let root = dir.path();
    let keep = sentinels(root);
    let zip = patch(root, "fonts/game.ttf", true);
    apply(root, &zip, ApplyOptions::default(), |_| {}).unwrap();
    fs::write(
        root.join("fonts/added.ttf"),
        b"user edited recorded addition",
    )
    .unwrap();
    let receipt = fs::read(root.join(".locust/receipt.json")).unwrap();
    assert_eq!(
        rollback(root, RollbackOptions::default())
            .unwrap()
            .aborted_edited,
        vec!["fonts/added.ttf"]
    );
    assert_eq!(
        fs::read(root.join(".locust/receipt.json")).unwrap(),
        receipt
    );
    assert_preserved(&keep);
    rollback(
        root,
        RollbackOptions {
            delete_modified_added: true,
        },
    )
    .unwrap();
    assert!(!root.join("fonts/added.ttf").exists());
    assert_eq!(fs::read(root.join("fonts/game.ttf")).unwrap(), b"orig");
    assert_preserved(&keep);
}

#[test]
fn marker_writes_preserve_predictable_tmp_and_old_names() {
    let dir = game();
    let root = dir.path();
    let store = PatchStore::new(root);
    let marker = root.join("marker.json");
    fs::write(&marker, b"old marker").unwrap();
    fs::write(root.join("marker.tmp"), b"user temporary marker").unwrap();
    fs::write(root.join("marker.json.locust-old"), b"user old marker").unwrap();
    store
        .write_durable_json(&marker, &serde_json::json!({"value": 42}))
        .unwrap();
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&fs::read(marker).unwrap()).unwrap()["value"],
        42
    );
    assert_eq!(
        fs::read(root.join("marker.tmp")).unwrap(),
        b"user temporary marker"
    );
    assert_eq!(
        fs::read(root.join("marker.json.locust-old")).unwrap(),
        b"user old marker"
    );
}

#[cfg(any(unix, windows))]
fn link_directory(target: &Path, link: &Path) {
    #[cfg(unix)]
    std::os::unix::fs::symlink(target, link).unwrap();
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let output = std::process::Command::new("powershell.exe")
            .args(["-NoProfile", "-NonInteractive", "-Command", "New-Item -ItemType Junction -Path $env:LOCUST_TEST_LINK -Target $env:LOCUST_TEST_TARGET -ErrorAction Stop | Out-Null"])
            .env("LOCUST_TEST_LINK", link).env("LOCUST_TEST_TARGET", target)
            .creation_flags(0x08000000).output().unwrap();
        assert!(
            output.status.success(),
            "junction: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[cfg(any(unix, windows))]
fn unlink_directory(path: &Path) {
    #[cfg(unix)]
    fs::remove_file(path).unwrap();
    #[cfg(windows)]
    fs::remove_dir(path).unwrap();
}

#[test]
#[cfg(any(unix, windows))]
fn unrelated_scratch_links_are_not_followed_or_deleted() {
    let dir = game();
    let root = dir.path();
    let outside = tempfile::tempdir().unwrap();
    fs::write(outside.path().join("save.locust-tmp"), b"external save").unwrap();
    let links = [
        root.join("fonts/game.ttf.locust-tmp"),
        root.join("fonts/game.ttf.locust-old"),
        root.join(".locust-probe-user"),
    ];
    for link in &links {
        link_directory(outside.path(), link);
    }
    let zip = patch(root, "fonts/game.ttf", false);
    apply(root, &zip, ApplyOptions::default(), |_| {}).unwrap();
    rollback(root, RollbackOptions::default()).unwrap();
    for link in &links {
        assert_eq!(
            fs::read(link.join("save.locust-tmp")).unwrap(),
            b"external save"
        );
        unlink_directory(link);
    }
    assert_eq!(
        fs::read(outside.path().join("save.locust-tmp")).unwrap(),
        b"external save"
    );
}

#[test]
#[cfg(any(unix, windows))]
fn link_inserted_at_write_boundary_is_rejected_before_asset_mutation() {
    let dir = game();
    let root = dir.path();
    let keep = sentinels(root);
    let outside = tempfile::tempdir().unwrap();
    fs::write(outside.path().join("game.ttf"), b"external orig").unwrap();
    let zip = patch(root, "fonts/game.ttf", false);
    assert!(apply(root, &zip, ApplyOptions::default(), |_| {
        fs::rename(root.join("fonts"), root.join("saved-fonts")).unwrap();
        link_directory(outside.path(), &root.join("fonts"));
    })
    .is_err());
    assert_eq!(
        fs::read(outside.path().join("game.ttf")).unwrap(),
        b"external orig"
    );
    assert!(rollback(
        root,
        RollbackOptions {
            delete_modified_added: true
        }
    )
    .is_err());
    unlink_directory(&root.join("fonts"));
    fs::rename(root.join("saved-fonts"), root.join("fonts")).unwrap();
    rollback(root, RollbackOptions::default()).unwrap();
    assert_preserved(&keep);
}
