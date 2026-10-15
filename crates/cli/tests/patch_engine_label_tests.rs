use std::fs;
use std::io::Cursor;
use std::path::{Path, PathBuf};

use locust_core::database::Database;
use locust_core::extraction::FormatPlugin;
use locust_core::models::{StringEntry, StringStatus, FORMAT_ID_METADATA_KEY};
use locust_formats::{godot_dialogic::GodotDialogicPlugin, godot_pck};

mod common;
use common::locust;

fn dialogic_pack(text: &[u8]) -> Vec<u8> {
    let mut header = vec![0; 100];
    header[..4].copy_from_slice(b"GDPC");
    header[4..8].copy_from_slice(&2u32.to_le_bytes());
    header[8..12].copy_from_slice(&4u32.to_le_bytes());
    let mut source = Cursor::new(header);
    let index = godot_pck::read_index(&mut source).unwrap();
    let mut output = Vec::new();
    godot_pck::rebuild(
        &mut source,
        &index,
        &[],
        &[("res://main.dtl", text)],
        &mut output,
    )
    .unwrap();
    output
}

fn recorded_project(base: &Path, root: &Path, file: &Path, rows: &[StringEntry]) -> PathBuf {
    let project = base.join("project.locust.db");
    let db = Database::open(&project).unwrap();
    let translated: Vec<_> = rows
        .iter()
        .cloned()
        .map(|mut row| {
            row.translation = Some("Hola".into());
            row.status = StringStatus::Translated;
            row
        })
        .collect();
    db.save_entries(&translated).unwrap();
    db.record_injection(Some("es"), root, &[file.to_path_buf()])
        .unwrap();
    project
}

fn assert_patch_labels(base: &Path, root: &Path, project: &Path, engine: &str, astro_engine: &str) {
    let zip_path = base.join("patch.zip");
    let stub = base.join("release.md");
    locust()
        .arg("patch")
        .arg(root)
        .arg("-P")
        .arg(project)
        .args(["-l", "es", "-o"])
        .arg(&zip_path)
        .arg("--astro")
        .arg(&stub)
        .assert()
        .success();

    let mut zip = zip::ZipArchive::new(fs::File::open(zip_path).unwrap()).unwrap();
    let manifest: serde_json::Value =
        serde_json::from_reader(zip.by_name("locust-patch.json").unwrap()).unwrap();
    assert_eq!(manifest["engine"], engine);
    let markdown = fs::read_to_string(stub).unwrap();
    assert!(
        markdown.contains(&format!("\nengine: \"{astro_engine}\" ")),
        "{markdown}"
    );
}

#[test]
fn patch_godot_dialogic_replace_output_labels_manifest_and_astro() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("game.pck");
    fs::write(&source, dialogic_pack(b"Hello\n")).unwrap();
    let mut rows = GodotDialogicPlugin.extract(&source).unwrap();
    assert!(!rows.is_empty());
    for row in &mut rows {
        assert_eq!(row.metadata[FORMAT_ID_METADATA_KEY], "godot-dialogic");
        row.translation = Some("Hola".into());
        row.status = StringStatus::Translated;
    }
    // Reproduce the archive-only Replace output and its injection recording.
    let root = temp.path().join("replace-es");
    fs::create_dir(&root).unwrap();
    let output = root.join("game.pck");
    fs::write(&output, dialogic_pack(b"Hola\n")).unwrap();
    assert!(GodotDialogicPlugin.detect(&output));
    assert!(locust_formats::default_registry().detect(&root).is_none());
    let project = recorded_project(temp.path(), &root, &output, &rows);

    assert_patch_labels(
        temp.path(),
        &root,
        &project,
        "godot-dialogic",
        "godot-dialogic",
    );
}

fn renpy_recording(base: &Path, formats: &[Option<&str>]) -> (PathBuf, PathBuf) {
    let root = base.join("renpy");
    fs::create_dir_all(root.join("game")).unwrap();
    let script = root.join("game/script.rpy");
    fs::write(&script, "label start:\n    \"Hola\"\n").unwrap();
    let rows: Vec<_> = formats
        .iter()
        .enumerate()
        .map(|(index, format)| {
            let row = StringEntry::new(format!("line-{index}"), "Hello", script.clone());
            match format {
                Some(format) => row.with_format_id(format),
                None => row,
            }
        })
        .collect();
    let project = recorded_project(base, &root, &script, &rows);
    (root, project)
}

#[test]
fn patch_existing_engine_labels_are_unchanged() {
    let temp = tempfile::tempdir().unwrap();
    let (root, project) = renpy_recording(temp.path(), &[Some("renpy"), Some("renpy")]);
    assert_patch_labels(temp.path(), &root, &project, "renpy", "renpy");
}

#[test]
fn patch_ambiguous_project_formats_fall_back_to_detection() {
    let temp = tempfile::tempdir().unwrap();
    let (root, project) =
        renpy_recording(temp.path(), &[Some("godot-dialogic"), Some("html-game")]);
    assert_patch_labels(temp.path(), &root, &project, "renpy", "renpy");
}

#[test]
fn patch_missing_project_format_falls_back_to_detection() {
    let temp = tempfile::tempdir().unwrap();
    let (root, project) = renpy_recording(temp.path(), &[None]);
    assert_patch_labels(temp.path(), &root, &project, "renpy", "renpy");
}

#[test]
fn patch_missing_format_preserves_unknown_and_other_defaults() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("output");
    fs::create_dir(&root).unwrap();
    let file = root.join("game.pck");
    fs::write(&file, dialogic_pack(b"Hola\n")).unwrap();
    let row = StringEntry::new("legacy", "Hello", file.clone());
    let project = recorded_project(temp.path(), &root, &file, &[row]);
    assert_patch_labels(temp.path(), &root, &project, "unknown", "other");
}
