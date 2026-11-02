use assert_cmd::Command;
use predicates::prelude::*;
use std::path::Path;
use std::time::Duration;

fn server(data_dir: &Path) -> Command {
    let mut command = Command::cargo_bin("locust").unwrap();
    command
        .env("LOCUST_DATA_DIR", data_dir)
        .env("LOCUST_BACKUP_ROOT", data_dir.join("backups"))
        .arg("server")
        .timeout(Duration::from_secs(15));
    command
}

#[test]
fn server_reports_unusable_data_directory_without_panicking() {
    let root = tempfile::tempdir().unwrap();
    let data_dir = root.path().join("profile");
    let original = b"keep this existing file";
    std::fs::write(&data_dir, original).unwrap();

    server(&data_dir)
        .assert()
        .code(1)
        .stderr(predicate::str::contains("Cannot create data directory"))
        .stderr(predicate::str::contains(data_dir.display().to_string()))
        .stderr(predicate::str::contains("LOCUST_DATA_DIR"))
        .stderr(predicate::str::contains("panicked").not());

    assert_eq!(std::fs::read(&data_dir).unwrap(), original);
    assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 1);
}

#[test]
fn server_reports_corrupt_database_without_changing_it() {
    let root = tempfile::tempdir().unwrap();
    let db_path = root.path().join("project.db");
    let original = b"this is not a SQLite database";
    std::fs::write(&db_path, original).unwrap();

    server(root.path())
        .assert()
        .code(1)
        .stderr(predicate::str::contains("Cannot open project database"))
        .stderr(predicate::str::contains(db_path.display().to_string()))
        .stderr(predicate::str::contains(
            "Check its permissions and validity",
        ))
        .stderr(predicate::str::contains("panicked").not());

    assert_eq!(std::fs::read(&db_path).unwrap(), original);
    assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 1);
}

#[test]
fn server_reports_occupied_loopback_address_without_panicking() {
    let root = tempfile::tempdir().unwrap();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();

    server(root.path())
        .args(["--port", &address.port().to_string()])
        .assert()
        .code(1)
        .stderr(predicate::str::contains(address.to_string()))
        .stderr(predicate::str::contains("choose another --port"))
        .stderr(predicate::str::contains("panicked").not());
}
