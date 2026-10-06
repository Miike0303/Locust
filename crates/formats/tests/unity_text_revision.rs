use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

use locust_core::backup::{BackupManager, RevisionOriginal};
use locust_core::database::Database;
use locust_core::error::Result;
use locust_core::extraction::{inject_direct, DirectInjectReport, FormatPlugin, FormatRegistry};
use locust_core::models::StringEntry;
use locust_formats::unity::UnityPlugin;
use locust_formats::unity_serialized::CLASS_ID_TEXT_ASSET;

// The same SerializedFile builder used by cycle 133's revision fixtures.
#[path = "support/unity_fixture.rs"]
mod fixture;

struct Fixture {
    _temp: tempfile::TempDir,
    game: PathBuf,
    files: Vec<(PathBuf, Vec<u8>)>,
    db: Database,
    store: BackupManager,
    registry: FormatRegistry,
    rows: Vec<StringEntry>,
}

impl Fixture {
    fn new(mixed: bool) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let game = temp.path().join("game");
        let data = game.join("Revision_Data");
        let scripts = data.join("SCRIPTS~");
        fs::create_dir_all(scripts.join("Vol1")).unwrap();
        let mut files = vec![
            (
                scripts.join("Characters.txt"),
                concat!(
                    "\tcharacter CJ FF0253 \"CJ\" # keep comment\r\n",
                    "  character Q FFFFFF \"A \\\"quoted\\\" name\\\\\" # keep suffix\n",
                    "character Nar\n",
                )
                .as_bytes()
                .to_vec(),
            ),
            (
                scripts.join("Chapter.txt"),
                concat!(
                    "# keep comment\r\nversion 1\nscript Chapter {\r\nscene Garden\r\n",
                    "  CJ\tWelcome traveler!  \t\r\n",
                    "CJ \\bHello\\b \\p<link=\"journal>entry\">world</link>\\nNext.\n",
                    " CJ +CJ_Lgr  +J_Usur -R ¿Qué pasó?  \r\n",
                    "  button 1 \"Go inside\" jump Inside\r\n",
                    "CJ ?\n}\n",
                )
                .as_bytes()
                .to_vec(),
            ),
            (
                scripts.join("Vol1/Chapter.txt"),
                // Duplicate basename/local IDs, LF, and no final newline.
                b"# nested\n#\n#\n#\nCJ Nested hello.\nCJ Nested goodbye.".to_vec(),
            ),
        ];
        if mixed {
            files.push((
                data.join("resources.assets"),
                fixture::write_v17_fixture("Story", "Welcome traveler!"),
            ));
        }
        for (path, bytes) in &files {
            fs::write(path, bytes).unwrap();
        }
        let plugin = UnityPlugin::new();
        let mut rows = plugin.extract(&game).unwrap();
        // Translate the TextAsset; keep its dummy neighboring object untouched.
        rows.retain(|row| {
            row.file_path.extension().unwrap() == "txt"
                || row
                    .metadata
                    .get("extraction_method")
                    .and_then(|v| v.as_str())
                    == Some("textasset")
        });
        assert_eq!(
            rows.iter()
                .filter(|r| r.file_path.extension().unwrap() == "txt")
                .count(),
            9,
            "fixture extraction changed: {rows:?}"
        );
        assert!(rows
            .iter()
            .any(|r| r.metadata.contains_key("unity_local_id")));
        if mixed {
            assert!(rows.iter().any(|r| r
                .metadata
                .get("extraction_method")
                .and_then(|v| v.as_str())
                == Some("textasset")));
        }
        let mut registry = FormatRegistry::new();
        registry.register(Box::new(plugin));
        Self {
            game,
            files,
            rows,
            registry,
            db: Database::open_in_memory().unwrap(),
            store: BackupManager::new(temp.path().join("backups")),
            _temp: temp,
        }
    }

    fn translate(&mut self, pass: usize) {
        for row in &mut self.rows {
            row.translation = Some(format!("Revisión {pass} ampliada: {}", row.source));
        }
        self.save();
    }

    fn save(&self) {
        self.db.save_entries(&self.rows).unwrap();
    }

    fn inject(&self) -> Result<DirectInjectReport> {
        inject_direct(
            &self.registry,
            &self.db,
            &self.store,
            &self.game,
            "unity",
            &["es".into()],
        )
    }

    fn bytes(&self) -> Vec<Vec<u8>> {
        self.files
            .iter()
            .map(|(p, _)| fs::read(p).unwrap())
            .collect()
    }

    fn assert_current_translations(&self) {
        let extracted = UnityPlugin::new().extract(&self.game).unwrap();
        for row in &self.rows {
            assert!(
                extracted.iter().any(|new| new.file_path == row.file_path
                    && Some(&new.source) == row.translation.as_ref()),
                "missing {}: {:?}",
                row.id,
                row.translation
            );
        }
    }

    fn assert_one_shot_oracle(&self) {
        let revised = self.bytes();
        for (path, original) in &self.files {
            fs::write(path, original).unwrap();
        }
        UnityPlugin::new().inject(&self.game, &self.rows).unwrap();
        assert_eq!(
            self.bytes(),
            revised,
            "revision must equal pristine one-shot bytes"
        );
    }
}

fn repeated_direct(mixed: bool) {
    let mut f = Fixture::new(mixed);
    f.translate(1);
    let first = f.inject().unwrap();
    assert_eq!(first.strings_written, f.rows.len(), "pass 1: {first:?}");
    f.translate(2);
    let second = f.inject().unwrap();
    assert_eq!(second.strings_written, f.rows.len(), "pass 2: {second:?}");
    assert!(second.reports["es"].skip_reasons.is_empty());
    f.assert_current_translations();
    f.assert_one_shot_oracle();
}

#[test]
fn scripts_two_passes_equal_one_shot() {
    repeated_direct(false);
}

#[test]
fn mixed_scripts_and_serialized_textasset_revision() {
    repeated_direct(true);
}

#[test]
fn partial_edit_replays_other_translations_and_restores_one_source() {
    let mut f = Fixture::new(false);
    f.translate(1);
    f.inject().unwrap();
    // Restore just the display name; edit just one nested dialogue.
    let restored = f.rows.iter_mut().find(|r| r.source == "CJ").unwrap();
    restored.translation = Some(restored.source.clone());
    let edited = f
        .rows
        .iter_mut()
        .find(|r| r.source == "Nested hello.")
        .unwrap();
    edited.translation = Some("Solo esta línea cambia.".into());
    f.save();
    let report = f.inject().unwrap();
    assert_eq!(report.strings_written, f.rows.len(), "{report:?}");
    f.assert_current_translations();
    f.assert_one_shot_oracle();
}

#[test]
fn restoring_all_sources_preserves_exact_pristine_bytes() {
    let mut f = Fixture::new(false);
    f.translate(1);
    f.inject().unwrap();
    for row in &mut f.rows {
        row.translation = Some(row.source.clone());
    }
    f.save();
    let restored = f.inject().unwrap();
    assert_eq!(restored.strings_written, f.rows.len(), "{restored:?}");
    assert_eq!(
        f.bytes(),
        f.files.iter().map(|(_, b)| b.clone()).collect::<Vec<_>>()
    );
}

#[test]
fn unchanged_third_pass_installs_no_files() {
    let mut f = Fixture::new(false);
    f.translate(1);
    f.inject().unwrap();
    f.translate(2);
    f.inject().unwrap();
    f.assert_current_translations();
    let before = f.bytes();
    let modified: Vec<_> = f
        .files
        .iter()
        .map(|(p, _)| fs::metadata(p).unwrap().modified().unwrap())
        .collect();
    let third = f.inject().unwrap();
    assert_eq!(third.files_modified, 0, "{third:?}");
    assert!(third.files_written.is_empty());
    assert_eq!(f.bytes(), before);
    for ((path, _), time) in f.files.iter().zip(modified) {
        assert_eq!(fs::metadata(path).unwrap().modified().unwrap(), time);
    }
}

#[test]
fn replaced_script_is_refused_with_source_changed() {
    let mut f = Fixture::new(false);
    f.translate(1);
    f.inject().unwrap();
    // Same-size change to a comment, outside any translated span.
    let path = &f.files[0].0;
    let changed = fs::read_to_string(path)
        .unwrap()
        .replace("keep comment", "game updated");
    fs::write(path, changed).unwrap();
    let live = f.bytes();
    f.translate(2);
    let error = f.inject().unwrap_err().to_string();
    assert!(
        error.contains("source_changed") && error.contains("Characters.txt"),
        "{error}"
    );
    assert_eq!(f.bytes(), live);
}

#[test]
fn corrupt_pristine_backup_is_refused() {
    let mut f = Fixture::new(false);
    f.translate(1);
    let first = f.inject().unwrap();
    let live = f.bytes();
    let original = PathBuf::from(first.backup_path.unwrap())
        .join("payload")
        .join(f.files[0].0.strip_prefix(&f.game).unwrap());
    let mut bytes = fs::read(&original).unwrap();
    bytes[0] ^= 1;
    fs::write(original, bytes).unwrap();
    f.translate(2);
    let error = f.inject().unwrap_err().to_string();
    assert!(
        error.contains("pristine") || error.contains("hash") || error.contains("digest"),
        "{error}"
    );
    assert_eq!(f.bytes(), live);
}

#[test]
fn revision_reader_verifies_bytes_when_consumed() {
    let mut f = Fixture::new(false);
    f.translate(1);
    f.rows.retain(|r| r.file_path == f.files[0].0);
    let path = f.files[0].0.clone();
    let original = RevisionOriginal::capture(&path).unwrap();
    // Remains valid UTF-8 with identical dialogue, but its digest has changed.
    let changed = String::from_utf8(f.files[0].1.clone())
        .unwrap()
        .replace("keep comment", "game updated");
    fs::write(&path, changed).unwrap();
    let live = f.bytes();
    let error = UnityPlugin::new()
        .inject_revision(&f.game, &mut f.rows, &HashMap::from([(path, original)]))
        .unwrap_err()
        .to_string();
    assert!(error.contains("revision original"), "{error}");
    assert_eq!(f.bytes(), live);
}

#[test]
fn stale_database_sources_are_checked_against_pristine() {
    let mut f = Fixture::new(false);
    f.translate(1);
    f.inject().unwrap();
    // Use the actual pass-1 translations as stale sources: checking live files
    // instead of pristine would incorrectly accept these rows.
    for row in &mut f.rows {
        row.source = row.translation.clone().unwrap();
        row.translation = Some(format!("Otra revisión: {}", row.source));
    }
    f.save();
    let before = f.bytes();
    let report = f.inject().unwrap();
    assert_eq!(report.strings_written, 0, "{report:?}");
    assert_eq!(
        report.reports["es"].skip_reasons.get("source_changed"),
        Some(&f.rows.len())
    );
    assert_eq!(f.bytes(), before);
}
