use locust_core::patch::PatchStore;
use std::{
    fs,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread,
};

#[test]
fn updating_a_durable_marker_never_unpublishes_its_previous_generation() {
    let root = tempfile::tempdir().unwrap();
    let store = PatchStore::new(root.path());
    let marker = store.journal_path();
    store
        .write_durable_json(&marker, &serde_json::json!({"generation": 0}))
        .unwrap();
    let stop = Arc::new(AtomicBool::new(false));
    let reader_stop = stop.clone();
    let reader_marker = marker.clone();
    let (ready_tx, ready_rx) = std::sync::mpsc::channel();
    let reader = thread::spawn(move || {
        let mut count = 0;
        ready_tx.send(()).unwrap();
        while !reader_stop.load(Ordering::Acquire) {
            let bytes = fs::read(&reader_marker)
                .expect("an updating journal must never disappear, even between renames");
            let value: serde_json::Value = serde_json::from_slice(&bytes)
                .expect("a published journal must always be a complete JSON generation");
            assert!(value["generation"].as_u64().unwrap() <= 100);
            count += 1;
        }
        count
    });
    ready_rx.recv().unwrap();
    let mut failure = None;
    for generation in 1..=100 {
        if let Err(error) =
            store.write_durable_json(&marker, &serde_json::json!({"generation": generation}))
        {
            failure = Some(error);
            break;
        }
    }
    stop.store(true, Ordering::Release);
    let observations = reader.join().expect("journal observer failed");
    assert!(failure.is_none(), "marker publication failed: {failure:?}");
    assert!(observations > 0);
    let final_value: serde_json::Value =
        serde_json::from_slice(&fs::read(marker).unwrap()).unwrap();
    assert_eq!(final_value["generation"], 100);
}

#[cfg(windows)]
#[test]
fn denied_marker_replacement_preserves_the_committed_generation_and_foreign_files() {
    use std::os::windows::fs::OpenOptionsExt;
    let root = tempfile::tempdir().unwrap();
    let store = PatchStore::new(root.path());
    let marker = store.journal_path();
    store
        .write_durable_json(&marker, &serde_json::json!({"generation": "old"}))
        .unwrap();
    let before = fs::read(&marker).unwrap();
    let foreign = store.locust_dir().join(".locust-stage-unrelated");
    fs::create_dir(&foreign).unwrap();
    fs::write(foreign.join("sentinel"), b"unrelated").unwrap();
    let held = fs::OpenOptions::new()
        .read(true)
        .share_mode(1 | 2)
        .open(&marker)
        .unwrap();
    assert!(store
        .write_durable_json(&marker, &serde_json::json!({"generation": "new"}))
        .is_err());
    assert_eq!(fs::read(&marker).unwrap(), before);
    assert_eq!(fs::read(foreign.join("sentinel")).unwrap(), b"unrelated");
    assert_eq!(fs::read_dir(store.locust_dir()).unwrap().count(), 2);
    drop(held);
    store
        .write_durable_json(&marker, &serde_json::json!({"generation": "new"}))
        .unwrap();
}

#[test]
fn killed_marker_writer_leaves_a_complete_discoverable_generation() {
    use std::{
        process::{Child, Command, Stdio},
        time::{Duration, Instant},
    };
    struct Worker(Child);
    impl Drop for Worker {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    for _ in 0..3 {
        let root = tempfile::tempdir().unwrap();
        let store = PatchStore::new(root.path());
        store
            .write_durable_json(&store.journal_path(), &serde_json::json!({"generation": 0}))
            .unwrap();
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args(["--exact", "marker_update_child", "--ignored"])
            .env("LOCUST_MARKER_CRASH_TEST_ROOT", root.path())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        }
        let mut child = Worker(command.spawn().unwrap());
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            assert!(
                child.0.try_wait().unwrap().is_none(),
                "writer ended before interruption"
            );
            let value: serde_json::Value =
                serde_json::from_slice(&fs::read(store.journal_path()).unwrap()).unwrap();
            if value["generation"].as_u64().unwrap() >= 3 {
                break;
            }
            assert!(Instant::now() < deadline, "writer did not publish progress");
            thread::sleep(Duration::from_millis(5));
        }
        child.0.kill().unwrap();
        assert!(!child.0.wait().unwrap().success());
        let reopened = PatchStore::new(root.path());
        let value: serde_json::Value =
            serde_json::from_slice(&fs::read(reopened.journal_path()).unwrap()).unwrap();
        assert!(value["generation"].as_u64().unwrap() >= 3);
        reopened
            .write_durable_json(
                &reopened.journal_path(),
                &serde_json::json!({"generation": "recovered"}),
            )
            .unwrap();
        let value: serde_json::Value =
            serde_json::from_slice(&fs::read(reopened.journal_path()).unwrap()).unwrap();
        assert_eq!(value["generation"], "recovered");
    }
}

#[test]
#[ignore = "subprocess entrypoint used by killed_marker_writer_leaves_a_complete_discoverable_generation"]
fn marker_update_child() {
    let root = std::path::PathBuf::from(std::env::var_os("LOCUST_MARKER_CRASH_TEST_ROOT").unwrap());
    let store = PatchStore::new(root);
    for generation in 1..10_000 {
        store
            .write_durable_json(
                &store.journal_path(),
                &serde_json::json!({"generation": generation}),
            )
            .unwrap();
    }
}

#[test]
fn directory_marker_target_is_rejected_without_removing_its_children() {
    let root = tempfile::tempdir().unwrap();
    let store = PatchStore::new(root.path());
    fs::create_dir_all(store.journal_path()).unwrap();
    fs::write(store.journal_path().join("sentinel"), b"foreign").unwrap();
    assert!(store
        .write_durable_json(&store.journal_path(), &serde_json::json!({"generation": 1}))
        .is_err());
    assert_eq!(
        fs::read(store.journal_path().join("sentinel")).unwrap(),
        b"foreign"
    );
    assert_eq!(fs::read_dir(store.locust_dir()).unwrap().count(), 1);
}

#[cfg(any(unix, windows))]
#[test]
fn linked_marker_target_is_rejected_without_modifying_its_referent() {
    let root = tempfile::tempdir().unwrap();
    let store = PatchStore::new(root.path());
    fs::create_dir(store.locust_dir()).unwrap();
    let original = root.path().join("foreign.json");
    fs::write(&original, b"foreign bytes").unwrap();
    #[cfg(windows)]
    let result = std::os::windows::fs::symlink_file(&original, store.journal_path());
    #[cfg(unix)]
    let result = std::os::unix::fs::symlink(&original, store.journal_path());
    if let Err(error) = result {
        assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied);
        eprintln!("symlink fixture unavailable on this host: {error}");
        return;
    }
    assert!(store
        .write_durable_json(&store.journal_path(), &serde_json::json!({"generation": 1}))
        .is_err());
    assert_eq!(fs::read(&original).unwrap(), b"foreign bytes");
    assert!(fs::symlink_metadata(store.journal_path())
        .unwrap()
        .file_type()
        .is_symlink());
    assert_eq!(fs::read_dir(store.locust_dir()).unwrap().count(), 1);
}
