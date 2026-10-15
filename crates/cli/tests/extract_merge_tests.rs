use std::fs;
use std::path::Path;

use locust_core::database::{sha256_hex, Database, EntryFilter};
use locust_core::models::{StringEntry, StringStatus};

mod common;
use common::locust;

fn renpy_fixture() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("game")).unwrap();
    std::fs::write(
        dir.path().join("game/script.rpy"),
        "label start:\n    e \"Hello, world!\"\n    e \"Goodbye!\"\n",
    )
    .unwrap();
    dir
}

fn extract(game: &Path, project: &Path) -> String {
    extract_output(game, project, false).0
}

fn extract_output(game: &Path, project: &Path, dry_run: bool) -> (String, String) {
    let mut cmd = locust();
    cmd.arg("extract").arg(game).arg("-o").arg(project);
    if dry_run {
        cmd.arg("--dry-run");
    }
    let assertion = cmd.assert().success();
    let output = assertion.get_output();
    (
        String::from_utf8(output.stdout.clone()).unwrap(),
        String::from_utf8(output.stderr.clone()).unwrap(),
    )
}

fn sqlite_sidecar(path: &Path, suffix: &str) -> std::path::PathBuf {
    let mut name = path.file_name().unwrap().to_os_string();
    name.push(suffix);
    path.with_file_name(name)
}

/// Content hashes of the database and its SQLite sidecars. Missing files stay
/// missing; file timestamps are not part of the hash.
fn db_fingerprints(path: &Path) -> Vec<(String, Option<String>)> {
    let mut files = vec![path.to_path_buf()];
    for suffix in ["-wal", "-shm", "-journal"] {
        files.push(sqlite_sidecar(path, suffix));
    }
    files
        .into_iter()
        .map(|file| {
            let name = file.file_name().unwrap().to_string_lossy().into_owned();
            let hash = file
                .is_file()
                .then(|| sha256_hex(&fs::read(&file).unwrap()));
            (name, hash)
        })
        .collect()
}

fn dir_listing(path: &Path) -> Vec<String> {
    let mut names: Vec<_> = fs::read_dir(path)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

const EXTRACT_COUNT_ROWS: &[&str] = &[
    "Strings extracted",
    "Added",
    "Updated",
    "Source changed (reset to pending)",
    "Removed",
    "Translations preserved",
    "Translations lost",
];

fn assert_same_extract_counts(dry: &str, real: &str) {
    for label in EXTRACT_COUNT_ROWS {
        assert_eq!(
            summary_count(dry, label),
            summary_count(real, label),
            "{label}"
        );
    }
}

fn summary_count(output: &str, label: &str) -> usize {
    output
        .lines()
        .find_map(|line| {
            let cells: Vec<_> = line.split(['|', '│']).map(str::trim).collect();
            (cells.get(1) == Some(&label))
                .then(|| cells.get(2)?.parse().ok())
                .flatten()
        })
        .unwrap_or_else(|| panic!("missing numeric summary row {label:?}:\n{output}"))
}

async fn approve(db: &Database, id: &str, translation: &str) -> StringEntry {
    assert!(db.save_translation(id, translation, "mock").await.unwrap());
    db.update_entry_status(id, StringStatus::Approved)
        .await
        .unwrap();
    let entry = db.get_entry(id).unwrap().unwrap();
    assert_eq!(entry.status, StringStatus::Approved);
    assert!(entry.translated_at.is_some());
    assert!(entry.reviewed_at.is_some());
    entry
}

#[tokio::test]
async fn cli_reextract_preserves_translations() {
    let dir = renpy_fixture();
    let project = dir.path().join("project.db");
    extract(dir.path(), &project);
    let db = Database::open(&project).unwrap();
    let before = approve(&db, "script.rpy#2", "¡Hola, mundo!").await;
    drop(db);

    let output = extract(dir.path(), &project);

    let db = Database::open(&project).unwrap();
    let after = db.get_entry(&before.id).unwrap().unwrap();
    assert_eq!(after.translation, before.translation);
    assert_eq!(after.status, before.status);
    assert_eq!(after.provider_used, before.provider_used);
    assert_eq!(after.translated_at, before.translated_at);
    assert_eq!(after.reviewed_at, before.reviewed_at);
    assert_eq!(after.created_at, before.created_at);
    assert_eq!(summary_count(&output, "Translations preserved"), 1);
    assert_eq!(summary_count(&output, "Added"), 0);
    assert_eq!(summary_count(&output, "Updated"), 0);
    assert_eq!(
        summary_count(&output, "Source changed (reset to pending)"),
        0
    );
    assert_eq!(summary_count(&output, "Removed"), 0);
}

#[tokio::test]
async fn cli_reextract_resets_changed_source_and_removes_gone_ids() {
    let dir = renpy_fixture();
    let project = dir.path().join("project.db");
    extract(dir.path(), &project);
    let db = Database::open(&project).unwrap();
    let before = approve(&db, "script.rpy#2", "¡Hola, mundo!").await;
    let removed = approve(&db, "script.rpy#3", "¡Adiós!").await;
    drop(db);

    // Ren'Py IDs include the line number: keep line 2 and delete the last line.
    std::fs::write(
        dir.path().join("game/script.rpy"),
        "label start:\n    e \"Hello, everyone!\"\n",
    )
    .unwrap();
    let output = extract(dir.path(), &project);

    let db = Database::open(&project).unwrap();
    let after = db.get_entry(&before.id).unwrap().unwrap();
    assert_eq!(after.source, "Hello, everyone!");
    assert_eq!(after.translation, before.translation);
    assert_eq!(after.status, StringStatus::Pending);
    assert_eq!(after.provider_used, before.provider_used);
    assert_eq!(after.translated_at, before.translated_at);
    assert_eq!(after.reviewed_at, before.reviewed_at);
    assert_eq!(after.created_at, before.created_at);
    assert!(after.require_current_translation().is_err());
    assert!(db.get_entry(&removed.id).unwrap().is_none());
    assert_eq!(db.get_entries(&EntryFilter::default()).unwrap().len(), 1);
    assert_eq!(summary_count(&output, "Added"), 0);
    assert_eq!(summary_count(&output, "Updated"), 1);
    assert_eq!(
        summary_count(&output, "Source changed (reset to pending)"),
        1
    );
    assert_eq!(summary_count(&output, "Removed"), 1);
    assert_eq!(summary_count(&output, "Translations preserved"), 1);
}

#[test]
fn cli_first_extract_matches_save_entries() {
    let dir = renpy_fixture();
    let project = dir.path().join("project.db");
    let registry = locust_formats::default_registry();
    let expected = registry
        .detect(dir.path())
        .unwrap()
        .extract(dir.path())
        .unwrap();
    let started = chrono::Utc::now();
    let output = extract(dir.path(), &project);
    let finished = chrono::Utc::now();
    let db = Database::open(&project).unwrap();
    let actual = db.get_entries(&EntryFilter::default()).unwrap();
    assert_eq!(actual.len(), 2);
    assert_eq!(actual.len(), expected.len());
    for entry in &actual {
        assert_eq!(entry.status, StringStatus::Pending);
        assert!(entry.translation.is_none());
        assert!(entry.provider_used.is_none());
        assert!(entry.translated_at.is_none());
        assert!(entry.reviewed_at.is_none());
        assert!(entry.created_at >= started && entry.created_at <= finished);
        let mut expected_entry = expected.iter().find(|e| e.id == entry.id).unwrap().clone();
        // These are separate extractions; only their creation times may differ.
        expected_entry.created_at = entry.created_at;
        assert_eq!(
            serde_json::to_value(entry).unwrap(),
            serde_json::to_value(expected_entry).unwrap()
        );
    }
    assert_eq!(summary_count(&output, "Strings extracted"), 2);
    assert_eq!(summary_count(&output, "Added"), 2);
    assert_eq!(summary_count(&output, "Updated"), 0);
    assert_eq!(
        summary_count(&output, "Source changed (reset to pending)"),
        0
    );
    assert_eq!(summary_count(&output, "Removed"), 0);
    assert_eq!(summary_count(&output, "Translations preserved"), 0);
    assert_eq!(
        db.get_project_metadata("extraction_warnings").unwrap(),
        Some(serde_json::json!([]))
    );

    // Compare the same extraction's timestamps and all persisted fields against
    // the old writer, including original-blob persistence used by Unity rows.
    let mut entries = expected;
    entries[0].textasset_original = Some(std::sync::Arc::new(
        locust_core::models::TextAssetOriginal::new("Key: Hello, world!\n").unwrap(),
    ));
    let saved_path = dir.path().join("saved.db");
    let merged_path = dir.path().join("merged.db");
    let saved = Database::open(&saved_path).unwrap();
    saved.save_entries(&entries).unwrap();
    let merged = Database::open(&merged_path).unwrap();
    assert_eq!(merged.merge_entries(&entries).unwrap().added, 2);
    for query in [
        "SELECT * FROM strings ORDER BY id",
        "SELECT * FROM textasset_originals ORDER BY sha256",
    ] {
        let snapshot = |path: &Path| {
            let conn = rusqlite::Connection::open(path).unwrap();
            let mut stmt = conn.prepare(query).unwrap();
            let columns = stmt.column_count();
            let rows = stmt
                .query_map([], |row| {
                    (0..columns)
                        .map(|column| {
                            let value: rusqlite::types::Value = row.get(column)?;
                            // Metadata object key order is not significant.
                            Ok(if query.contains("strings") && column == 7 {
                                rusqlite::types::Value::Text(
                                    serde_json::from_str::<serde_json::Value>(
                                        &row.get::<_, String>(column)?,
                                    )
                                    .unwrap()
                                    .to_string(),
                                )
                            } else {
                                value
                            })
                        })
                        .collect::<rusqlite::Result<Vec<_>>>()
                })
                .unwrap();
            rows.collect::<rusqlite::Result<Vec<_>>>().unwrap()
        };
        assert_eq!(snapshot(&merged_path), snapshot(&saved_path), "{query}");
    }
}

#[tokio::test]
async fn cli_partial_reextract_preserves_missing_rows_until_complete() {
    use locust_formats::unreal_locres::{LocresFile, LocresNamespace, LocresString, LocresVersion};

    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("project.db");
    let resource = dir.path().join("Game.locres");
    let mut file = LocresFile {
        version: LocresVersion::Compact,
        namespaces: vec![LocresNamespace {
            name: "Dialogue".into(),
            name_hash: 0,
            strings: ["Hello, world!", "Goodbye!"]
                .into_iter()
                .enumerate()
                .map(|(i, value)| LocresString {
                    key: format!("line{i}"),
                    key_hash: 0,
                    source_string_hash: 0,
                    value: value.into(),
                })
                .collect(),
        }],
    };
    std::fs::write(&resource, file.serialize().unwrap()).unwrap();
    extract(dir.path(), &project);
    let db = Database::open(&project).unwrap();
    let entries = db.get_entries(&EntryFilter::default()).unwrap();
    assert_eq!(entries.len(), 2);
    let id = &entries.iter().find(|e| e.source == "Goodbye!").unwrap().id;
    let missing = approve(&db, id, "¡Adiós!").await;
    let id = &entries
        .iter()
        .find(|e| e.source == "Hello, world!")
        .unwrap()
        .id;
    let present = approve(&db, id, "¡Hola, mundo!").await;
    drop(db);

    file.namespaces[0].strings.pop();
    std::fs::write(&resource, file.serialize().unwrap()).unwrap();
    // An encrypted IoStore header yields an extraction warning while the
    // companion loose LocRes remains readable (no UCAS payload is read).
    let toc = dir.path().join("extra.utoc");
    let mut header = [0u8; 144];
    header[..16].copy_from_slice(b"-==--==--==--==-");
    header[16] = 5;
    header[20..24].copy_from_slice(&144u32.to_le_bytes());
    header[80] = 2;
    std::fs::write(&toc, header).unwrap();
    let output = extract(dir.path(), &project);

    let db = Database::open(&project).unwrap();
    assert_eq!(db.get_entries(&EntryFilter::default()).unwrap().len(), 2);
    assert_eq!(
        serde_json::to_value(db.get_entry(&missing.id).unwrap().unwrap()).unwrap(),
        serde_json::to_value(&missing).unwrap()
    );
    let after = db.get_entry(&present.id).unwrap().unwrap();
    assert_eq!(after.translation, present.translation);
    assert_eq!(after.status, present.status);
    assert_eq!(after.provider_used, present.provider_used);
    assert_eq!(after.translated_at, present.translated_at);
    assert_eq!(after.reviewed_at, present.reviewed_at);
    let warnings = locust_core::project::saved_extraction_warnings(&db).unwrap();
    assert_eq!(warnings.len(), 1);
    assert!(warnings[0].contains("encrypted"));
    assert_eq!(summary_count(&output, "Removed"), 0);
    assert_eq!(summary_count(&output, "Translations preserved"), 1);
    drop(db);

    std::fs::remove_file(toc).unwrap();
    let output = extract(dir.path(), &project);
    let db = Database::open(&project).unwrap();
    assert!(db.get_entry(&missing.id).unwrap().is_none());
    assert!(locust_core::project::saved_extraction_warnings(&db)
        .unwrap()
        .is_empty());
    assert_eq!(summary_count(&output, "Removed"), 1);
}

#[tokio::test]
async fn cli_extract_dry_run_previews_lost_translations_without_writing() {
    let dir = renpy_fixture();
    let project = dir.path().join("project.db");
    extract(dir.path(), &project);
    let db = Database::open(&project).unwrap();
    let changed = approve(&db, "script.rpy#2", "¡Hola, mundo!").await;
    drop(db);

    std::fs::write(
        dir.path().join("game/script.rpy"),
        "label start:\n    e \"Hello, everyone!\"\n",
    )
    .unwrap();
    let before = db_fingerprints(&project);
    let (dry, _) = extract_output(dir.path(), &project, true);
    assert_eq!(
        db_fingerprints(&project),
        before,
        "dry-run changed the project database or a SQLite sidecar"
    );
    assert!(dry.contains("Dry run: nothing was written"), "{dry}");
    assert_eq!(summary_count(&dry, "Source changed (reset to pending)"), 1);
    assert_eq!(summary_count(&dry, "Removed"), 1);
    assert_eq!(summary_count(&dry, "Translations lost"), 1);

    let db = Database::open(&project).unwrap();
    let still = db.get_entry(&changed.id).unwrap().unwrap();
    assert_eq!(still.source, changed.source);
    assert_eq!(still.translation, changed.translation);
    assert_eq!(still.status, changed.status);
    assert!(db.get_entry("script.rpy#3").unwrap().is_some());
    drop(db);

    let real = extract(dir.path(), &project);
    assert_same_extract_counts(&dry, &real);
    assert!(!real.contains("Dry run: nothing was written"), "{real}");
    let db = Database::open(&project).unwrap();
    let after = db.get_entry(&changed.id).unwrap().unwrap();
    assert_eq!(after.source, "Hello, everyone!");
    assert_eq!(after.status, StringStatus::Pending);
    assert!(db.get_entry("script.rpy#3").unwrap().is_none());
}

#[test]
fn cli_extract_dry_run_missing_output_creates_no_file() {
    let dir = renpy_fixture();
    let missing_parent = dir.path().join("missing-parent");
    let nested = missing_parent.join("project.locust.db");
    let (nested_out, _) = extract_output(dir.path(), &nested, true);
    assert!(
        !missing_parent.exists(),
        "dry-run created {missing_parent:?}"
    );
    assert!(!nested.exists());
    assert!(
        nested_out.contains("Dry run: nothing was written"),
        "{nested_out}"
    );
    assert_eq!(summary_count(&nested_out, "Added"), 2);
    assert_eq!(summary_count(&nested_out, "Translations lost"), 0);

    let project = dir.path().join("new.locust.db");
    let before = dir_listing(dir.path());
    let (output, _) = extract_output(dir.path(), &project, true);
    assert_eq!(
        dir_listing(dir.path()),
        before,
        "dry-run left a database or scratch file next to the output path"
    );
    assert!(!project.exists());
    for suffix in ["-wal", "-shm", "-journal"] {
        assert!(!sqlite_sidecar(&project, suffix).exists());
    }
    assert!(output.contains("Dry run: nothing was written"), "{output}");
    assert_eq!(summary_count(&output, "Added"), 2);
    assert_eq!(summary_count(&output, "Removed"), 0);
}

#[tokio::test]
async fn cli_extract_dry_run_partial_preserves_missing_rows() {
    use locust_formats::unreal_locres::{LocresFile, LocresNamespace, LocresString, LocresVersion};

    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("project.db");
    let resource = dir.path().join("Game.locres");
    let mut file = LocresFile {
        version: LocresVersion::Compact,
        namespaces: vec![LocresNamespace {
            name: "Dialogue".into(),
            name_hash: 0,
            strings: ["Hello, world!", "Goodbye!"]
                .into_iter()
                .enumerate()
                .map(|(i, value)| LocresString {
                    key: format!("line{i}"),
                    key_hash: 0,
                    source_string_hash: 0,
                    value: value.into(),
                })
                .collect(),
        }],
    };
    std::fs::write(&resource, file.serialize().unwrap()).unwrap();
    extract(dir.path(), &project);
    let db = Database::open(&project).unwrap();
    let entries = db.get_entries(&EntryFilter::default()).unwrap();
    let id = &entries.iter().find(|e| e.source == "Goodbye!").unwrap().id;
    let missing = approve(&db, id, "¡Adiós!").await;
    drop(db);

    file.namespaces[0].strings.pop();
    std::fs::write(&resource, file.serialize().unwrap()).unwrap();
    let toc = dir.path().join("extra.utoc");
    let mut header = [0u8; 144];
    header[..16].copy_from_slice(b"-==--==--==--==-");
    header[16] = 5;
    header[20..24].copy_from_slice(&144u32.to_le_bytes());
    header[80] = 2;
    std::fs::write(&toc, header).unwrap();

    let before = db_fingerprints(&project);
    let (dry, stderr) = extract_output(dir.path(), &project, true);
    assert!(
        stderr.contains("partial extraction"),
        "dry-run did not take the partial-extraction path:\n{stderr}"
    );
    assert_eq!(db_fingerprints(&project), before);
    assert_eq!(summary_count(&dry, "Removed"), 0);
    assert_eq!(summary_count(&dry, "Translations lost"), 0);
    assert_eq!(summary_count(&dry, "Strings extracted"), 1);
    assert!(dry.contains("Dry run: nothing was written"), "{dry}");

    let db = Database::open(&project).unwrap();
    assert_eq!(
        serde_json::to_value(db.get_entry(&missing.id).unwrap().unwrap()).unwrap(),
        serde_json::to_value(&missing).unwrap()
    );
    assert!(locust_core::project::saved_extraction_warnings(&db)
        .unwrap()
        .is_empty());
    drop(db);

    let (real, _) = extract_output(dir.path(), &project, false);
    assert_same_extract_counts(&dry, &real);
    let db = Database::open(&project).unwrap();
    assert!(db.get_entry(&missing.id).unwrap().is_some());
    assert_eq!(
        locust_core::project::saved_extraction_warnings(&db)
            .unwrap()
            .len(),
        1
    );
    assert_eq!(summary_count(&real, "Removed"), 0);
}

#[test]
fn cli_formats_lists_godot_experimental_replace() {
    let output = locust().arg("formats").assert().success();
    let stdout = String::from_utf8_lossy(&output.get_output().stdout);
    let line = stdout
        .lines()
        .find(|line| line.contains("godot-dialogic"))
        .unwrap();
    assert!(
        line.contains("experimental") && line.contains("Replace"),
        "{line}"
    );
    assert!(!line.contains("Add"), "{line}");
}

#[tokio::test]
async fn cli_godot_extract_replace_direct_and_pack_keep_file_scope() {
    use locust_formats::godot_pck;
    use std::io::{Cursor, Read};
    let temp = tempfile::tempdir().unwrap();
    let shared = temp.path().join("shared");
    fs::create_dir(&shared).unwrap();
    let game = shared.join("game.pck");
    let mut header = vec![0; 100];
    header[..4].copy_from_slice(b"GDPC");
    header[4..8].copy_from_slice(&2u32.to_le_bytes());
    header[8..12].copy_from_slice(&4u32.to_le_bytes());
    let mut source = Cursor::new(header);
    let index = godot_pck::read_index(&mut source).unwrap();
    let mut original = Vec::new();
    godot_pck::rebuild(
        &mut source,
        &index,
        &[],
        &[("res://main.dtl", b"Hello\n"), ("res://raw.bin", &[0, 255])],
        &mut original,
    )
    .unwrap();
    fs::write(&game, &original).unwrap();
    fs::write(shared.join("index.html"), b"<p>Foreign game</p>").unwrap();
    let project = temp.path().join("project.locust.db");
    extract(&game, &project);
    let db = Database::open(&project).unwrap();
    let rows = db.get_entries(&EntryFilter::default()).unwrap();
    assert_eq!(rows.len(), 1);
    approve(&db, &rows[0].id, "Hola, un saludo mucho mas largo").await;
    drop(db);
    let backup_root = temp.path().join("backups");
    for direct in [false, true] {
        let mut command = locust();
        command
            .env("LOCUST_BACKUP_ROOT", &backup_root)
            .arg("inject")
            .arg(&game)
            .arg("-P")
            .arg(&project)
            .args(["-l", "es", "--json"]);
        if direct {
            command.arg("--direct");
        } else {
            command
                .args(["-m", "replace", "-o"])
                .arg(temp.path().join("copies"));
        }
        command.assert().success();
        let db = Database::open(&project).unwrap();
        let recording = db.get_injection(Some("es")).unwrap().unwrap();
        assert_eq!(recording.files.len(), 1);
        assert_eq!(recording.files[0].rel, "game.pck");
        if !direct {
            assert_eq!(fs::read(&game).unwrap(), original);
            assert!(!recording.root.join("index.html").exists());
        } else {
            assert!(locust_core::database::paths_identical(
                &recording.root,
                &shared
            ));
        }
        let zip_path = temp
            .path()
            .join(if direct { "direct.zip" } else { "replace.zip" });
        drop(db);
        locust()
            .env("LOCUST_BACKUP_ROOT", &backup_root)
            .arg("patch")
            .arg(&recording.root)
            .arg("-P")
            .arg(&project)
            .args(["-l", "es", "--no-detect-id", "-o"])
            .arg(&zip_path)
            .assert()
            .success();
        let mut zip = zip::ZipArchive::new(fs::File::open(zip_path).unwrap()).unwrap();
        let mut manifest = String::new();
        zip.by_name("locust-patch.json")
            .unwrap()
            .read_to_string(&mut manifest)
            .unwrap();
        let manifest: serde_json::Value = serde_json::from_str(&manifest).unwrap();
        assert!(
            manifest["files"][0]["original_sha256"].is_string(),
            "{manifest}"
        );
    }
    assert_eq!(
        fs::read(shared.join("index.html")).unwrap(),
        b"<p>Foreign game</p>"
    );
}
