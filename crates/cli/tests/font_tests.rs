use assert_cmd::Command;
use serde_json::{json, Value};
use std::io::Write;

fn locust() -> Command {
    let mut command = Command::cargo_bin("locust").unwrap();
    command.env("LOCUST_CONFIG", "__font_test_missing_config__.toml");
    command
}

#[test]
fn font_cli_combined_patch_check_apply_and_rollback() {
    let dir = tempfile::tempdir().unwrap();
    let game = dir.path().join("game");
    std::fs::create_dir_all(game.join("fonts")).unwrap();
    let old = include_bytes!("fixtures/synthetic-ascii.ttf");
    let mut new = old.to_vec();
    new.extend_from_slice(b"Locust synthetic variant");
    let font = dir.path().join("replacement.ttf");
    std::fs::write(game.join("fonts/game.ttf"), old).unwrap();
    std::fs::write(&font, &new).unwrap();
    let text = dir.path().join("text.txt");
    std::fs::write(&text, "Hola\n\t").unwrap();
    let check = locust()
        .arg("font-check")
        .arg(&font)
        .arg("--text-file")
        .arg(&text)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let audit: Value = serde_json::from_slice(&check).unwrap();
    assert_eq!(audit["fonts"][0]["has_full_coverage"], true);
    assert_eq!(audit["fonts"][0]["face_index"], 0);
    std::fs::write(game.join("dialogue.txt"), b"Hello").unwrap();
    let base = dir.path().join("base.zip");
    let mut zip = zip::ZipWriter::new(std::fs::File::create(&base).unwrap());
    let options = zip::write::SimpleFileOptions::default();
    zip.start_file("dialogue.txt", options).unwrap();
    zip.write_all(b"Hola").unwrap();
    zip.start_file("locust-patch.json", options).unwrap();
    zip.write_all(&serde_json::to_vec(&json!({"schema_version":1,"patch_id":"base-font-cli-test","game_name":"fixture","engine":"rpgmaker_mz","language":"es","patch_version":"1.0.0","generator_version":"0.1.0","created_at":"2026-09-11T00:00:00Z","files":[{"path":"dialogue.txt","size":4,"patched_sha256":locust_core::database::sha256_hex(b"Hola"),"original_sha256":locust_core::database::sha256_hex(b"Hello")}]})).unwrap()).unwrap();
    zip.finish().unwrap();
    let output = dir.path().join("combined.zip");
    locust()
        .arg("font-patch")
        .arg(&game)
        .arg("--font")
        .arg(&font)
        .args([
            "--target",
            "fonts/game.ttf",
            "--engine",
            "rpgmaker_mz",
            "--lang",
            "es",
            "--text-file",
        ])
        .arg(&text)
        .arg("--base-patch")
        .arg(&base)
        .arg("--output")
        .arg(&output)
        .assert()
        .success();
    assert_eq!(std::fs::read(game.join("fonts/game.ttf")).unwrap(), old);
    locust()
        .arg("apply")
        .arg(&game)
        .arg(&output)
        .assert()
        .success();
    assert_eq!(std::fs::read(game.join("fonts/game.ttf")).unwrap(), new);
    assert_eq!(std::fs::read(game.join("dialogue.txt")).unwrap(), b"Hola");
    let receipt: Value =
        serde_json::from_slice(&std::fs::read(game.join(".locust/receipt.json")).unwrap()).unwrap();
    assert_eq!(receipt["patch_id"], "base-font-cli-test");
    assert_eq!(receipt["replaced"].as_array().unwrap().len(), 2);
    locust().arg("patch-rollback").arg(&game).assert().success();
    assert_eq!(std::fs::read(game.join("fonts/game.ttf")).unwrap(), old);
    assert_eq!(std::fs::read(game.join("dialogue.txt")).unwrap(), b"Hello");
}

#[test]
fn font_cli_extends_real_rpgmaker_extract_inject_patch_recording() {
    use locust_core::database::{Database, EntryFilter};
    use locust_core::models::StringStatus;
    let dir = tempfile::tempdir().unwrap();
    let game = dir.path().join("injected");
    let clean = dir.path().join("clean");
    let actors =
        br#"[null,{"id":1,"name":"Harold","nickname":"Hero","profile":"Hello adventurer"}]"#;
    let old = include_bytes!("fixtures/synthetic-ascii.ttf");
    let mut new = old.to_vec();
    new.extend_from_slice(b"replacement synthetic font");
    for root in [&game, &clean] {
        std::fs::create_dir_all(root.join("data")).unwrap();
        std::fs::create_dir(root.join("fonts")).unwrap();
        std::fs::write(root.join("data/Actors.json"), actors).unwrap();
        std::fs::write(root.join("fonts/game.ttf"), old).unwrap();
    }
    let db_path = dir.path().join("project.db");
    locust()
        .arg("extract")
        .arg(&game)
        .arg("--output")
        .arg(&db_path)
        .assert()
        .success();
    let db = Database::open(&db_path).unwrap();
    let mut entries = db.get_entries(&EntryFilter::default()).unwrap();
    assert!(!entries.is_empty());
    for entry in &mut entries {
        entry.translation = Some("Hola".into());
        entry.status = StringStatus::Translated;
    }
    db.save_entries(&entries).unwrap();
    drop(db);
    locust()
        .arg("inject")
        .arg(&game)
        .arg("--project")
        .arg(&db_path)
        .args(["--direct", "-l", "es"])
        .assert()
        .success();
    let translated_actors = std::fs::read(game.join("data/Actors.json")).unwrap();
    assert_ne!(translated_actors, actors);
    let base = dir.path().join("translation.zip");
    locust()
        .arg("patch")
        .arg(&game)
        .arg("--project")
        .arg(&db_path)
        .args(["--lang", "es", "--pristine"])
        .arg(&clean)
        .arg("--output")
        .arg(&base)
        .assert()
        .success();
    let font = dir.path().join("replacement.ttf");
    std::fs::write(&font, &new).unwrap();
    let text = dir.path().join("target.txt");
    std::fs::write(&text, "Hola").unwrap();
    let combined = dir.path().join("combined.zip");
    let result = locust()
        .arg("font-patch")
        .arg(&clean)
        .arg("--font")
        .arg(&font)
        .args([
            "--target",
            "fonts/game.ttf",
            "--engine",
            "rpgmaker-mz",
            "--lang",
            "es",
            "--text-file",
        ])
        .arg(&text)
        .arg("--base-patch")
        .arg(&base)
        .arg("--output")
        .arg(&combined)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let report: Value = serde_json::from_slice(&result).unwrap();
    assert_eq!(report["manifest"]["engine"], "rpgmaker-mv");
    assert_eq!(report["manifest"]["files"].as_array().unwrap().len(), 2);
    locust()
        .arg("apply")
        .arg(&clean)
        .arg(&combined)
        .assert()
        .success();
    assert_eq!(
        std::fs::read(clean.join("data/Actors.json")).unwrap(),
        translated_actors
    );
    assert_eq!(std::fs::read(clean.join("fonts/game.ttf")).unwrap(), new);
    locust()
        .arg("patch-rollback")
        .arg(&clean)
        .assert()
        .success();
    assert_eq!(
        std::fs::read(clean.join("data/Actors.json")).unwrap(),
        actors
    );
    assert_eq!(std::fs::read(clean.join("fonts/game.ttf")).unwrap(), old);
}

#[test]
fn font_cli_rejects_empty_requirements_bad_languages_and_non_game_paths() {
    let dir = tempfile::tempdir().unwrap();
    let game = dir.path().join("game");
    std::fs::create_dir_all(game.join("fonts")).unwrap();
    let old = include_bytes!("fixtures/synthetic-ascii.ttf");
    let mut new = old.to_vec();
    new.extend_from_slice(b"synthetic alternate");
    std::fs::write(game.join("fonts/game.ttf"), old).unwrap();
    let source = dir.path().join("source.ttf");
    std::fs::write(&source, &new).unwrap();
    let text = dir.path().join("target.txt");
    let output = dir.path().join("font.zip");
    let run = |root: &std::path::Path, language: &str| {
        locust()
            .arg("font-patch")
            .arg(root)
            .arg("--font")
            .arg(&source)
            .args(["--target", "fonts/game.ttf", "--engine", "html"])
            .arg(format!("--lang={language}"))
            .arg("--text-file")
            .arg(&text)
            .arg("--output")
            .arg(&output)
            .assert()
    };
    std::fs::write(&text, "Hola").unwrap();
    for language in [
        "",
        "-",
        "_",
        "123",
        "es--MX",
        "-es",
        "es-",
        "es_@",
        "toolongprimary",
    ] {
        run(&game, language).failure();
        assert!(!output.exists());
    }
    run(&source, "es").failure();
    assert!(!output.exists());
    for empty in ["", " \t\n\0\u{feff}\u{200d}\u{fe0f}"] {
        std::fs::write(&text, empty).unwrap();
        run(&game, "es")
            .failure()
            .stderr(predicates::str::contains("no relevant characters"));
        assert!(!output.exists());
    }
    std::fs::write(&text, "Hola").unwrap();
    run(&game, "es_MX").success();
    assert!(output.exists());
    assert_eq!(std::fs::read(game.join("fonts/game.ttf")).unwrap(), old);
}
