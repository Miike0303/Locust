use locust_core::database::Database;
use locust_core::models::{StringEntry, StringStatus, INJECTION_SOURCE_METADATA_KEY};

mod common;
use common::locust;

#[test]
fn translate_errors_are_visible_without_a_terminal_and_grouped() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("errors.db");
    let db = Database::open(&project).unwrap();
    // Mock reverses short free text: n\ becomes \n, adding a protected control.
    let entries: Vec<_> = (0..8)
        .map(|i| StringEntry::new(format!("line-{i}"), r"n\", "Map001.json".into()))
        .collect();
    db.save_entries(&entries).unwrap();
    drop(db);

    let assertion = locust()
        .args([
            "translate",
            project.to_str().unwrap(),
            "--provider",
            "mock",
            "--no-memory",
        ])
        .assert()
        .failure();
    let output = assertion.get_output();
    let stderr = String::from_utf8_lossy(&output.stderr);
    let reason = "translation changed protected placeholders; entry remains pending";
    assert_eq!(
        stderr.matches(&format!("Error: {reason}")).count(),
        3,
        "{stderr}"
    );
    assert!(stderr.contains(&format!("8 x {reason}")), "{stderr}");
    assert!(
        stderr.contains("Translation incomplete: 8 strings still pending after provider errors"),
        "{stderr}"
    );
    assert_eq!(
        locust_core::translation::load_pending_entries(&Database::open(&project).unwrap())
            .unwrap()
            .len(),
        8
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Batch errors"), "{stdout}");
    assert!(stdout.contains("8 strings still pending"), "{stdout}");
}

#[test]
fn validate_accepts_translated_nameboxes_including_pivot_baseline() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("names.db");
    let db = Database::open(&project).unwrap();
    let mut entries = Vec::new();
    for (i, escape) in [r"\n", r"\nc", r"\nr", r"\NC"].iter().enumerate() {
        let mut entry = StringEntry::new(
            format!("name-{i}"),
            format!("{escape}<Lydia>Hello"),
            "Map001.json".into(),
        );
        entry.translation = Some(format!("{escape}<Lidia>Hola"));
        entry.status = StringStatus::Translated;
        entry.metadata.insert(
            INJECTION_SOURCE_METADATA_KEY.into(),
            serde_json::json!(format!("{escape}<リディア>こんにちは")),
        );
        entries.push(entry);
    }
    db.save_entries(&entries).unwrap();
    drop(db);

    let assertion = locust()
        .args(["validate", project.to_str().unwrap(), "--json"])
        .assert()
        .success();
    let report: serde_json::Value = serde_json::from_slice(&assertion.get_output().stdout).unwrap();
    assert_eq!(report["count"], 0);
    assert_eq!(report["issues"], serde_json::json!([]));
}

#[test]
fn translate_all_rows_succeeds() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("success.db");
    let db = Database::open(&project).unwrap();
    db.save_entries(&[StringEntry::new("line", "Hello", "Map001.json".into())])
        .unwrap();
    drop(db);

    let assertion = locust()
        .args([
            "translate",
            project.to_str().unwrap(),
            "--provider",
            "mock",
            "--no-memory",
        ])
        .assert()
        .success();
    assert!(
        String::from_utf8_lossy(&assertion.get_output().stdout).contains("All strings translated.")
    );
    assert!(
        locust_core::translation::load_pending_entries(&Database::open(&project).unwrap())
            .unwrap()
            .is_empty()
    );
}

#[test]
fn translate_provider_error_with_and_without_fallback_reports_exit_status() {
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::TcpListener;
    use std::time::{Duration, Instant};

    for with_fallback in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("provider.db");
        let config = dir.path().join("fixture-config.json");
        let db = Database::open(&project).unwrap();
        db.save_entries(&[StringEntry::new("line", "Hello", "Map001.json".into())])
            .unwrap();
        drop(db);
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        std::fs::write(
            &config,
            serde_json::to_vec(&serde_json::json!({
                "providers": { "openai": {
                    "api_key": "fixture-key",
                    "base_url": format!("http://{}", listener.local_addr().unwrap()),
                    "model": "fixture-model"
                }}
            }))
            .unwrap(),
        )
        .unwrap();
        let server = std::thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(30);
            loop {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        stream.set_nonblocking(false).unwrap();
                        stream
                            .set_read_timeout(Some(Duration::from_secs(5)))
                            .unwrap();
                        let mut reader = BufReader::new(&mut stream);
                        let mut line = String::new();
                        reader.read_line(&mut line).unwrap();
                        assert!(line.starts_with("POST /v1/chat/completions "));
                        let mut content_length = 0;
                        loop {
                            line.clear();
                            assert!(reader.read_line(&mut line).unwrap() > 0);
                            if line == "\r\n" {
                                break;
                            }
                            if let Some((name, value)) = line.split_once(':') {
                                if name.eq_ignore_ascii_case("content-length") {
                                    content_length = value.trim().parse::<usize>().unwrap();
                                }
                            }
                        }
                        reader.read_exact(&mut vec![0; content_length]).unwrap();
                        drop(reader);
                        stream.write_all(b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 7\r\nConnection: close\r\n\r\nrefused").unwrap();
                        return;
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(
                            Instant::now() < deadline,
                            "CLI never contacted fixture provider"
                        );
                        std::thread::sleep(Duration::from_millis(10));
                    }
                    Err(error) => panic!("fixture listener: {error}"),
                }
            }
        });
        let mut command = locust();
        command.args([
            "--config",
            config.to_str().unwrap(),
            "translate",
            project.to_str().unwrap(),
            "--provider",
            "openai",
            "--no-memory",
        ]);
        if with_fallback {
            command.args(["--fallback", "mock"]);
        }
        let output = command.output().unwrap();
        server.join().unwrap();
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("401"), "{stderr}");
        assert_eq!(output.status.success(), with_fallback, "{stderr}");
        let remaining =
            locust_core::translation::load_pending_entries(&Database::open(&project).unwrap())
                .unwrap()
                .len();
        assert_eq!(remaining, usize::from(!with_fallback));
        if with_fallback {
            assert!(!stderr.contains("Translation incomplete"), "{stderr}");
        } else {
            assert!(
                stderr.contains(
                    "Translation incomplete: 1 strings still pending after provider errors"
                ),
                "{stderr}"
            );
        }
    }
}
