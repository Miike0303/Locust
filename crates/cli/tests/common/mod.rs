use assert_cmd::Command;

pub fn locust() -> Command {
    let root = std::env::temp_dir().join(format!("locust_cli_test_{}", uuid::Uuid::new_v4()));
    let mut command = Command::cargo_bin("locust").unwrap();
    command
        .env("LOCUST_DATA_DIR", root.join("profile"))
        .env("LOCUST_BACKUP_ROOT", root.join("backups"));
    command
}
