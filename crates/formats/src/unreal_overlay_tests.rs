use super::*;
use std::fs;

#[test]
fn partial_stage_write_failure_preserves_original_and_unrelated_sentinels() {
    let root = tempfile::tempdir().unwrap();
    let lock = GameLock::acquire(root.path()).unwrap();
    let output = root.path().join("Game_LOCUST_P.pak");
    fs::write(&output, b"exact old overlay").unwrap();
    let sibling = output.with_extension("pak.locust-old");
    fs::write(&sibling, b"unrelated sibling").unwrap();
    let unowned = root.path().join(".locust-stage-unrelated");
    fs::create_dir(&unowned).unwrap();
    fs::write(unowned.join("sentinel"), b"recursive sentinel").unwrap();
    let mut warnings = Vec::new();
    let result = write_overlay_with(lock.root(), &output, &mut warnings, |file| {
        file.write_all(b"partial new bytes")?;
        Err(std::io::Error::other("injected late write failure").into())
    });
    assert!(result.is_err());
    assert_eq!(fs::read(output).unwrap(), b"exact old overlay");
    assert_eq!(fs::read(sibling).unwrap(), b"unrelated sibling");
    assert_eq!(
        fs::read(unowned.join("sentinel")).unwrap(),
        b"recursive sentinel"
    );
    assert_eq!(fs::read_dir(root.path()).unwrap().count(), 3);
    assert!(warnings.is_empty());
}

#[test]
fn installation_failure_restores_original_without_claiming_foreign_files() {
    let root = tempfile::tempdir().unwrap();
    let lock = GameLock::acquire(root.path()).unwrap();
    let output = root.path().join("Game_LOCUST_P.pak");
    fs::write(&output, b"original exact overlay").unwrap();
    let stage = StagingDir::create_prepared(root.path()).unwrap();
    // Missing staged input makes the shared installation fail after the old
    // destination was moved aside. It must restore its exact bytes.
    guard_unreal_target(lock.root(), &output).unwrap();
    assert!(PatchStore::replace_file(&stage.child("missing"), &output).is_err());
    assert_eq!(fs::read(output).unwrap(), b"original exact overlay");
    drop(stage);
    assert_eq!(fs::read_dir(root.path()).unwrap().count(), 1);
}

#[test]
fn directory_destination_rejection_preserves_recursive_content() {
    let root = tempfile::tempdir().unwrap();
    let lock = GameLock::acquire(root.path()).unwrap();
    let output = root.path().join("Game_LOCUST_P.pak");
    fs::create_dir(&output).unwrap();
    fs::write(output.join("sentinel"), b"user directory content").unwrap();
    assert!(write_overlay(lock.root(), &output, b"replacement", &mut Vec::new()).is_err());
    assert_eq!(
        fs::read(output.join("sentinel")).unwrap(),
        b"user directory content"
    );
    assert_eq!(fs::read_dir(root.path()).unwrap().count(), 1);
}

#[cfg(any(windows, unix))]
#[test]
fn output_symlink_is_rejected_without_touching_its_target() {
    let root = tempfile::tempdir().unwrap();
    let lock = GameLock::acquire(root.path()).unwrap();
    let target = root.path().join("unrelated");
    fs::write(&target, b"symlink target bytes").unwrap();
    let output = root.path().join("Game_LOCUST_P.pak");
    #[cfg(windows)]
    let result = std::os::windows::fs::symlink_file(&target, &output);
    #[cfg(unix)]
    let result = std::os::unix::fs::symlink(&target, &output);
    if let Err(error) = result {
        assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied);
        eprintln!("symlink creation unavailable on this host: {error}");
        return;
    }
    assert!(write_overlay(lock.root(), &output, b"replacement", &mut Vec::new()).is_err());
    assert!(fs::symlink_metadata(output)
        .unwrap()
        .file_type()
        .is_symlink());
    assert_eq!(fs::read(target).unwrap(), b"symlink target bytes");
}
