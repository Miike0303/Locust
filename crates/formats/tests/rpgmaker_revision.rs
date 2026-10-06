use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

use locust_core::backup::{BackupManager, RevisionOriginal};
use locust_core::database::Database;
use locust_core::error::Result;
use locust_core::extraction::{inject_direct, DirectInjectReport, FormatPlugin, FormatRegistry};
use locust_core::models::StringEntry;
use locust_formats::rpgmaker_mv::RpgMakerMvPlugin;
use serde_json::json;

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
    fn new(www: bool, encoded: bool) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let game = temp.path().join("game");
        let data = game.join(if www { "www/data" } else { "data" });
        fs::create_dir_all(&data).unwrap();
        // Growing the first message moves every subsequent command's locator.
        let list = json!([
            {"code":101,"indent":0,"parameters":["",0,0,2,"Alice"]},
            {"code":401,"indent":0,"parameters":["First small line"]},
            {"code":401,"indent":0,"parameters":["Second line"]},
            {"code":101,"indent":0,"parameters":["",0,0,2]},
            {"code":401,"indent":0,"parameters":["Later message"]},
            {"code":102,"indent":0,"parameters":[["Yes","No"],0,0,2,0]},
            {"code":356,"indent":0,"parameters":["D_TEXT Floating words 24"]},
            {"code":357,"indent":0,"parameters":["DTextPicture","dText","",{"text":"Plugin words"}]},
            {"code":0,"indent":0,"parameters":[]}
        ]);
        let fixtures = [
            (
                "Actors",
                json!([null,{"id":1,"name":"Hero","profile":"Small profile\nSecond line","classId":1}]),
            ),
            (
                "System",
                json!({"gameTitle":"Revision game","terms":{"commands":["Fight"]},"elements":["","Fire"]}),
            ),
            (
                "Map001",
                json!({"displayName":"Garden","events":[null,{"pages":[{"list":list}]}]}),
            ),
            ("CommonEvents", json!([null,{"list":list}])),
            (
                "Troops",
                json!([null,{"name":"Enemies","pages":[{"list":list}]}]),
            ),
            (
                "lang_g_en",
                json!({"hello":"Pack greeting","wrapped":"Short pack line\nSecond line"}),
            ),
        ];
        let files = fixtures
            .into_iter()
            .enumerate()
            .map(|(i, (stem, value))| {
                let text = value.to_string();
                let payload = if encoded {
                    lz_str::compress_to_base64(&text)
                } else {
                    text
                };
                // Exercise both BOM and BOM-free files, including compressed packs.
                let mut bytes = if i % 2 == 0 {
                    vec![0xef, 0xbb, 0xbf]
                } else {
                    Vec::new()
                };
                bytes.extend(payload.as_bytes());
                let path = data.join(format!("{stem}.{}", if encoded { "jsono" } else { "json" }));
                fs::write(&path, &bytes).unwrap();
                (path, bytes)
            })
            .collect();
        let plugin = RpgMakerMvPlugin::new();
        let rows = plugin.extract(&game).unwrap();
        assert_eq!(rows.len(), 30, "fixture extraction changed: {rows:?}");
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
            row.translation = Some(format!(
                "Pass {pass} {} translation of {}",
                "many extra words ".repeat(pass + 3),
                row.source.replace('\n', " ")
            ));
        }
        self.db.save_entries(&self.rows).unwrap();
    }

    fn inject(&self) -> Result<DirectInjectReport> {
        inject_direct(
            &self.registry,
            &self.db,
            &self.store,
            &self.game,
            "rpgmaker-mv",
            &["es".into()],
        )
    }

    fn bytes(&self) -> Vec<Vec<u8>> {
        self.files
            .iter()
            .map(|(path, _)| fs::read(path).unwrap())
            .collect()
    }
}

fn words(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn repeated_direct(www: bool, encoded: bool) {
    let mut f = Fixture::new(www, encoded);
    f.translate(1);
    let first = f.inject().unwrap();
    assert_eq!(first.strings_written, f.rows.len(), "pass 1: {first:?}");
    f.translate(2);
    let second = f.inject().unwrap();
    assert_eq!(second.strings_written, f.rows.len(), "pass 2: {second:?}");
    assert!(second.reports["es"].skip_reasons.is_empty());
    let extracted = RpgMakerMvPlugin::new().extract(&f.game).unwrap();
    assert_eq!(extracted.len(), f.rows.len());
    for row in &f.rows {
        // Wrapping intentionally inserts newlines; command IDs can move.
        let text = extracted
            .iter()
            .find(|new| {
                new.file_path == row.file_path
                    && words(&new.source) == words(row.translation.as_deref().unwrap())
            })
            .unwrap_or_else(|| panic!("pass 2 missing {}", row.id));
        if row.source.contains('\n') {
            let width = row.source.lines().map(str::len).max().unwrap();
            // Event messages have an established 40-column minimum.
            let width = if row.id.ends_with("#msg") {
                width.max(40)
            } else {
                width
            };
            assert!(text.source.contains('\n'));
            assert!(text.source.lines().all(|line| line.len() <= width));
        }
    }
    assert!(
        extracted
            .iter()
            .any(|new| f.rows.iter().any(|old| new.file_path == old.file_path
                && new.id != old.id
                && words(&new.source) == words(old.translation.as_deref().unwrap()))),
        "fixture must move later command locators"
    );
    let pass2 = f.bytes();
    let modified: Vec<_> = f
        .files
        .iter()
        .map(|(p, _)| fs::metadata(p).unwrap().modified().unwrap())
        .collect();
    let third = f.inject().unwrap();
    assert_eq!(third.files_modified, 0, "third pass: {third:?}");
    assert!(third.files_written.is_empty());
    assert_eq!(f.bytes(), pass2);
    for ((path, _), before) in f.files.iter().zip(modified) {
        assert_eq!(fs::metadata(path).unwrap().modified().unwrap(), before);
    }
    for (path, original) in &f.files {
        fs::write(path, original).unwrap();
    }
    RpgMakerMvPlugin::new().inject(&f.game, &f.rows).unwrap();
    assert_eq!(
        f.bytes(),
        pass2,
        "two passes must equal a pristine one-shot oracle"
    );
}

#[test]
fn direct_revision_mv_www_json() {
    repeated_direct(true, false);
}
#[test]
fn direct_revision_mz_data_json() {
    repeated_direct(false, false);
}
#[test]
fn direct_revision_por_iavra_www_jsono() {
    repeated_direct(true, true);
}
#[test]
fn direct_revision_por_iavra_data_jsono() {
    repeated_direct(false, true);
}

#[test]
fn direct_revision_game_update_refused() {
    for encoded in [false, true] {
        let mut f = Fixture::new(true, encoded);
        f.translate(1);
        f.inject().unwrap();
        // Even replacement with a valid pristine game file is external drift.
        let replacement = f.files[0].1.clone();
        fs::write(&f.files[0].0, &replacement).unwrap();
        let live = f.bytes();
        f.translate(2);
        let error = f.inject().unwrap_err().to_string();
        assert!(
            error.contains("source_changed") && error.contains("Actors"),
            "{error}"
        );
        assert_eq!(f.bytes(), live, "game update must not be overwritten");
    }
}

#[test]
fn direct_revision_restore_source_and_keep_unedited_rows() {
    let mut f = Fixture::new(false, false);
    f.translate(1);
    f.inject().unwrap();
    // Only one row edited; every other current translation must be replayed.
    let changed = f
        .rows
        .iter_mut()
        .find(|row| row.id.ends_with("#1#name"))
        .unwrap();
    changed.translation = Some(changed.source.clone());
    let id = changed.id.clone();
    let source = changed.source.clone();
    f.db.save_entries(&f.rows).unwrap();
    f.inject().unwrap();
    let extracted = RpgMakerMvPlugin::new().extract(&f.game).unwrap();
    assert_eq!(
        extracted.iter().find(|r| r.id == id).unwrap().source,
        source
    );
    let revised = f.bytes();
    for (path, original) in &f.files {
        fs::write(path, original).unwrap();
    }
    RpgMakerMvPlugin::new().inject(&f.game, &f.rows).unwrap();
    assert_eq!(f.bytes(), revised);
}

#[test]
fn direct_revision_restore_all_sources_matches_pristine_oracle() {
    for encoded in [false, true] {
        let mut f = Fixture::new(false, encoded);
        f.translate(1);
        f.inject().unwrap();
        for row in &mut f.rows {
            row.translation = Some(row.source.clone());
        }
        f.db.save_entries(&f.rows).unwrap();
        let restored = f.inject().unwrap();
        assert_eq!(restored.strings_written, f.rows.len());
        assert_eq!(
            f.bytes(),
            f.files.iter().map(|(_, b)| b.clone()).collect::<Vec<_>>()
        );
    }
}

#[test]
fn direct_revision_stale_database_source_refused() {
    let mut f = Fixture::new(false, false);
    f.translate(1);
    f.inject().unwrap();
    let live = f.bytes();
    for row in &mut f.rows {
        row.source = "Stale database source".into();
    }
    f.db.save_entries(&f.rows).unwrap();
    let report = f.inject().unwrap();
    assert_eq!(report.strings_written, 0);
    assert_eq!(
        report.reports["es"].skip_reasons.get("source_changed"),
        Some(&f.rows.len())
    );
    assert_eq!(f.bytes(), live);
}

#[test]
fn direct_revision_corrupt_original_refused() {
    let mut f = Fixture::new(false, true);
    f.translate(1);
    let first = f.inject().unwrap();
    let live = f.bytes();
    let original = PathBuf::from(first.backup_path.unwrap())
        .join("payload")
        .join(f.files[0].0.strip_prefix(&f.game).unwrap());
    fs::write(original, b"tampered backup").unwrap();
    f.translate(2);
    let error = f.inject().unwrap_err().to_string();
    assert!(
        error.contains("pristine") || error.contains("hash") || error.contains("digest"),
        "{error}"
    );
    assert_eq!(f.bytes(), live);
}

#[test]
fn direct_revision_reader_checks_consumed_original() {
    let mut f = Fixture::new(false, false);
    f.translate(1);
    let path = &f.files[0].0;
    let original = RevisionOriginal::capture(path).unwrap();
    // Core's contract: a supplied reader must verify the bytes actually read.
    fs::write(path, b"changed after capture").unwrap();
    let originals = HashMap::from([(path.clone(), original)]);
    let error = RpgMakerMvPlugin::new()
        .inject_revision(&f.game, &mut f.rows, &originals)
        .unwrap_err()
        .to_string();
    assert!(error.contains("revision original"), "{error}");
}

#[test]
fn direct_revision_selected_file() {
    let mut f = Fixture::new(true, false);
    f.game = f.files[0].0.clone();
    f.rows.retain(|r| r.file_path == f.game);
    f.translate(1);
    f.inject().unwrap();
    f.translate(2);
    let second = f.inject().unwrap();
    assert_eq!(second.strings_written, f.rows.len(), "{second:?}");
    let extracted = RpgMakerMvPlugin::new().extract(&f.game).unwrap();
    for row in &f.rows {
        assert!(extracted
            .iter()
            .any(|new| words(&new.source) == words(row.translation.as_deref().unwrap())));
    }
}
