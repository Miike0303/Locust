use std::{collections::BTreeMap, fs, path::Path};

use locust_core::{
    backup::BackupManager,
    database::{Database, EntryFilter},
    error::Result,
    extraction::{inject_direct, DirectInjectReport, FormatPlugin, FormatRegistry},
};
use locust_formats::rpgmaker_mv::RpgMakerMvPlugin;
use serde_json::json;

struct Fixture {
    temp: tempfile::TempDir,
    source: Database,
    registry: FormatRegistry,
    store: BackupManager,
}

impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let game = temp.path().join("game");
        fs::create_dir_all(game.join("www/data")).unwrap();
        fs::write(
            game.join("www/data/Actors.json"),
            json!([null, {"id":1,"name":"勇者","profile":"まだ訳していない紹介"}]).to_string(),
        )
        .unwrap();
        fs::write(
            game.join("www/data/CommonEvents.json"),
            json!([null, {"list":[
                {"code":101,"indent":0,"parameters":["",0,0,2]},
                {"code":401,"indent":0,"parameters":["最初の言葉"]},
                {"code":101,"indent":0,"parameters":["",0,0,2]},
                {"code":401,"indent":0,"parameters":["次の言葉"]},
                {"code":102,"indent":0,"parameters":[["はい","いいえ"],0,0,2,0]},
                {"code":356,"indent":0,"parameters":["D_TEXT 浮かぶ言葉 24"]},
                {"code":0,"indent":0,"parameters":[]}
            ]}])
            .to_string(),
        )
        .unwrap();
        let plugin = RpgMakerMvPlugin::new();
        let mut rows = plugin.extract(&game).unwrap();
        assert_eq!(rows.len(), 7);
        let source = Database::open_in_memory().unwrap();
        for row in &mut rows {
            // Portable rows also let the same project target a pristine oracle.
            row.file_path = row.file_path.strip_prefix(&game).unwrap().to_owned();
            row.translation = Some(format!("English source for {}", row.id));
        }
        source.save_entries(&rows).unwrap();
        let mut registry = FormatRegistry::new();
        registry.register(Box::new(plugin));
        let store = BackupManager::new(temp.path().join("backups"));
        Self {
            temp,
            source,
            registry,
            store,
        }
    }

    fn pivot(&self, name: &str, pass: usize) -> Database {
        let path = self.temp.path().join(name);
        self.source.pivot_to(&path).unwrap();
        let db = Database::open(&path).unwrap();
        Self::translate(&db, pass);
        assert!(db.get_injection(Some("es")).unwrap().is_none());
        db
    }

    fn translate(db: &Database, pass: usize) {
        let mut rows = db.get_entries(&EntryFilter::default()).unwrap();
        for row in &mut rows {
            assert_ne!(row.source, row.injection_source().unwrap());
            row.translation = if pass == 1 && row.id.ends_with("#profile") {
                None
            } else {
                // The first message grows and shifts subsequent command IDs.
                Some(format!(
                    "Pass {pass} {} {}",
                    "long text ".repeat(12),
                    row.id
                ))
            };
        }
        db.save_entries(&rows).unwrap();
    }

    fn inject(&self, db: &Database, game: &str) -> Result<DirectInjectReport> {
        inject_direct(
            &self.registry,
            db,
            &self.store,
            &self.temp.path().join(game),
            "rpgmaker-mv",
            &["es".into()],
        )
    }

    fn copy_pristine(&self) {
        for (relative, bytes) in snapshot(&self.temp.path().join("game")) {
            let path = self.temp.path().join("oracle").join(relative);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, bytes).unwrap();
        }
    }

    fn assert_refused_without_writes(&self, db: &Database) {
        let game = self.temp.path().join("game");
        let before = snapshot(&game);
        let backups = snapshot(self.store.root());
        let recording = db.get_injection(Some("es")).unwrap();
        let error = self
            .inject(db, "game")
            .expect_err("must not install a partial mix");
        let message = error.to_string();
        assert!(
            message.contains("Direct") && message.contains("recording"),
            "{message}"
        );
        assert!(
            message.contains("restore") && message.contains("pristine"),
            "{message}"
        );
        assert_eq!(snapshot(&game), before, "no game or journal writes");
        assert_eq!(snapshot(self.store.root()), backups, "no new backup");
        assert_eq!(db.get_injection(Some("es")).unwrap(), recording);
    }
}

fn snapshot(root: &Path) -> BTreeMap<std::path::PathBuf, Vec<u8>> {
    walkdir::WalkDir::new(root)
        .into_iter()
        .map(|entry| entry.unwrap())
        .filter(|entry| entry.file_type().is_file())
        .map(|entry| {
            (
                entry.path().strip_prefix(root).unwrap().to_owned(),
                fs::read(entry.path()).unwrap(),
            )
        })
        .collect()
}

#[test]
fn mv_live_source_checks_explain_partial_mix() {
    let f = Fixture::new();
    let first = f.pivot("a.db", 1);
    assert_eq!(f.inject(&first, "game").unwrap().strings_written, 6);
    let second = f.pivot("b.db", 2);
    let game = f.temp.path().join("game");
    let before = snapshot(&game.join("www/data"));
    let mut rows = second.get_entries(&EntryFilter::default()).unwrap();
    for row in &mut rows {
        row.source = row.injection_source().unwrap().to_owned();
        row.file_path = game.join(&row.file_path);
    }
    // The unsafe old core dispatch used this ordinary live-file path. Keep the
    // plugin's source checks: core must supply revision proof or refuse first.
    let report = RpgMakerMvPlugin::new().inject(&game, &rows).unwrap();
    assert_eq!(report.strings_written, 1, "{report:?}");
    assert_eq!(
        report.skip_reasons.get("source_changed"),
        Some(&2),
        "{report:?}"
    );
    assert_eq!(
        report.skip_reasons.get("missing_target"),
        Some(&4),
        "{report:?}"
    );
    let after = snapshot(&game.join("www/data"));
    assert_eq!(
        before[Path::new("CommonEvents.json")],
        after[Path::new("CommonEvents.json")]
    );
    assert_ne!(
        before[Path::new("Actors.json")],
        after[Path::new("Actors.json")]
    );
    let actors: serde_json::Value =
        serde_json::from_slice(&after[Path::new("Actors.json")]).unwrap();
    assert!(actors[1]["name"].as_str().unwrap().starts_with("Pass 1"));
    assert!(actors[1]["profile"].as_str().unwrap().starts_with("Pass 2"));
}

#[test]
fn new_pivot_at_same_db_path_refused_before_writes() {
    let f = Fixture::new();
    let first = f.pivot("project.db", 1);
    f.inject(&first, "game").unwrap();
    drop(first);
    fs::rename(
        f.temp.path().join("project.db"),
        f.temp.path().join("old.db"),
    )
    .unwrap();
    let second = f.pivot("project.db", 2);
    f.assert_refused_without_writes(&second);
}

#[test]
fn new_pivot_at_different_db_path_refused_before_writes() {
    let f = Fixture::new();
    let first = f.pivot("a.db", 1);
    f.inject(&first, "game").unwrap();
    let second = f.pivot("b.db", 2);
    f.assert_refused_without_writes(&second);
}

#[test]
fn moved_db_with_recording_replays_like_pristine_one_shot() {
    let f = Fixture::new();
    f.copy_pristine();
    let first = f.pivot("a.db", 1);
    let original_backup = f.inject(&first, "game").unwrap().pristine_backup_id;
    drop(first);
    fs::rename(f.temp.path().join("a.db"), f.temp.path().join("moved.db")).unwrap();
    let moved = Database::open(&f.temp.path().join("moved.db")).unwrap();
    Fixture::translate(&moved, 2);
    let replay = f.inject(&moved, "game").unwrap();
    assert_eq!(replay.strings_written, 7);
    assert_eq!(replay.strings_skipped, 0);
    assert_eq!(replay.pristine_backup_id, original_backup);
    let second = f.pivot("b.db", 2);
    assert_eq!(f.inject(&second, "oracle").unwrap().strings_written, 7);
    assert_eq!(
        snapshot(&f.temp.path().join("game/www/data")),
        snapshot(&f.temp.path().join("oracle/www/data")),
    );
}

#[test]
fn new_pivot_after_verified_restore_matches_pristine_one_shot() {
    let f = Fixture::new();
    f.copy_pristine();
    let first = f.pivot("a.db", 1);
    let backup = f.inject(&first, "game").unwrap().backup_id;
    f.store.restore(&backup).unwrap();
    let second = f.pivot("b.db", 2);
    let result = f.inject(&second, "game").unwrap();
    assert_eq!(result.strings_written, 7);
    assert_eq!(result.strings_skipped, 0);
    let oracle = f.pivot("oracle.db", 2);
    f.inject(&oracle, "oracle").unwrap();
    assert_eq!(
        snapshot(&f.temp.path().join("game/www/data")),
        snapshot(&f.temp.path().join("oracle/www/data")),
    );
}

#[test]
fn new_pivot_with_other_root_recording_refused_before_writes() {
    let f = Fixture::new();
    f.copy_pristine();
    let first = f.pivot("a.db", 1);
    f.inject(&first, "game").unwrap();
    let second = f.pivot("b.db", 2);
    f.inject(&second, "oracle").unwrap();
    f.assert_refused_without_writes(&second);
}

#[test]
fn new_pivot_with_incomplete_recording_refused_before_writes() {
    let f = Fixture::new();
    let first = f.pivot("a.db", 1);
    f.inject(&first, "game").unwrap();
    let record = first.get_injection(Some("es")).unwrap().unwrap();
    let second = f.pivot("b.db", 2);
    second
        .record_injection_with_backup(
            Some("es"),
            &record.root,
            &[record.root.join("www/data/Actors.json")],
            record.pristine_backup.as_ref(),
        )
        .unwrap();
    f.assert_refused_without_writes(&second);
}

#[test]
fn new_pivot_with_recording_without_pristine_provenance_refused() {
    let f = Fixture::new();
    let first = f.pivot("a.db", 1);
    f.inject(&first, "game").unwrap();
    let record = first.get_injection(Some("es")).unwrap().unwrap();
    let second = f.pivot("b.db", 2);
    let files: Vec<_> = record
        .files
        .iter()
        .map(|file| record.root.join(&file.rel))
        .collect();
    second
        .record_injection(Some("es"), &record.root, &files)
        .unwrap();
    f.assert_refused_without_writes(&second);
}

#[test]
fn new_pivot_after_partial_restore_refused_before_writes() {
    let f = Fixture::new();
    let partial = f
        .store
        .create_backup(&f.temp.path().join("game/www/data/Actors.json"))
        .unwrap();
    let first = f.pivot("a.db", 1);
    f.inject(&first, "game").unwrap();
    f.store.restore(&partial.id).unwrap();
    let second = f.pivot("b.db", 2);
    f.assert_refused_without_writes(&second);
}

#[test]
fn new_pivot_after_noop_still_refused_before_writes() {
    let f = Fixture::new();
    let first = f.pivot("a.db", 1);
    f.inject(&first, "game").unwrap();
    assert_eq!(f.inject(&first, "game").unwrap().files_modified, 0);
    let second = f.pivot("b.db", 2);
    f.assert_refused_without_writes(&second);
}

#[test]
fn new_pivot_over_drifted_game_refused_before_writes() {
    let f = Fixture::new();
    let first = f.pivot("a.db", 1);
    f.inject(&first, "game").unwrap();
    fs::write(
        f.temp.path().join("game/www/data/CommonEvents.json"),
        b"drift",
    )
    .unwrap();
    let second = f.pivot("b.db", 2);
    f.assert_refused_without_writes(&second);
}
