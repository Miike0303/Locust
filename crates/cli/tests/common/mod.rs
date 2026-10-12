use assert_cmd::Command;
use std::cell::RefCell;

thread_local! {
    // Each call gets a fresh root, but later commands may read its backups.
    // libtest runs each test on its own thread; thread exit cleans up every root.
    static ROOTS: RefCell<Vec<tempfile::TempDir>> = const { RefCell::new(Vec::new()) };
}

pub fn locust() -> Command {
    let root = tempfile::Builder::new()
        .prefix("locust_cli_test_")
        .tempdir()
        .unwrap();
    let mut command = Command::cargo_bin("locust").unwrap();
    command
        .env("LOCUST_DATA_DIR", root.path().join("profile"))
        .env("LOCUST_BACKUP_ROOT", root.path().join("backups"));
    ROOTS.with(|roots| roots.borrow_mut().push(root));
    command
}
