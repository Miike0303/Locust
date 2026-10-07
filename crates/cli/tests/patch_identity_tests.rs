use predicates::prelude::*;

mod common;
use common::locust;

#[test]
fn invalid_rj_flag_reports_validation_error_without_writing_zip() {
    let dir = tempfile::tempdir().unwrap();
    let output = dir.path().join("patch.zip");
    locust()
        .arg("patch")
        .arg(dir.path().join("game"))
        .arg("-P")
        .arg(dir.path().join("missing.db"))
        .arg("-o")
        .arg(&output)
        .args(["--rj", "RJ12345"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("DLsite").and(predicate::str::contains("6 or 8 digits")));
    assert!(!output.exists());
}

#[test]
fn detected_dlsite_id_is_printed_and_no_detect_id_suppresses_it() {
    let dir = tempfile::tempdir().unwrap();
    for disabled in [false, true] {
        let mut command = locust();
        command
            .arg("patch")
            .arg(dir.path().join("[rj01234567] Game"))
            .arg("-P")
            .arg(dir.path().join("missing.db"));
        if disabled {
            command.arg("--no-detect-id");
        }
        let result = command.assert().failure();
        let detected = predicate::str::contains("Detected DLsite id RJ01234567 from path");
        if disabled {
            result.stdout(detected.not());
        } else {
            result.stdout(detected);
        }
    }
}
