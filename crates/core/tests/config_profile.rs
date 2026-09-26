use locust_core::config::AppConfig;
use locust_core::database::GlobalMemoryDb;

// Exercise environment selection in a child so parallel tests never share
// process-global configuration, including the persistent translation memory.
#[test]
fn profile_isolates_config_and_global_memory() {
    let temp = tempfile::tempdir().unwrap();
    let profile = temp.path().join("profile with spaces");
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "profile_child", "--nocapture"])
        .env("LOCUST_DATA_DIR", &profile)
        .env("LOCUST_PROFILE_TEST_CHILD", "1")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(profile.join("config.json").is_file());
    assert!(profile.join("global_memory.db").is_file());
}

#[test]
fn profile_child() {
    if std::env::var_os("LOCUST_PROFILE_TEST_CHILD").is_none() {
        return;
    }
    let expected = std::path::PathBuf::from(std::env::var_os("LOCUST_DATA_DIR").unwrap());
    assert_eq!(AppConfig::config_dir(), expected);
    AppConfig::default()
        .save(&AppConfig::default_path())
        .unwrap();
    let _memory = GlobalMemoryDb::open_default().unwrap();
}
