use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

use locust_core::backup::{BackupManager, RevisionOriginal};
use locust_core::database::Database;
use locust_core::error::Result;
use locust_core::extraction::{inject_direct, DirectInjectReport, FormatPlugin, FormatRegistry};
use locust_core::models::StringEntry;
use locust_formats::rpgmaker_vxa::{build_test_fixture, MarshalValue as V, RpgMakerVxaPlugin};

fn object(class: &str, fields: &[(&str, V)]) -> V {
    V::Object {
        class: class.into(),
        ivars: fields
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect(),
    }
}

fn command(code: i64, parameters: Vec<V>) -> V {
    object(
        "RPG::EventCommand",
        &[
            ("@code", V::Int(code)),
            ("@indent", V::Int(0)),
            ("@parameters", V::Array(parameters)),
        ],
    )
}

struct Fixture {
    _temp: tempfile::TempDir,
    game: PathBuf,
    files: Vec<(PathBuf, Vec<u8>)>,
    rows: Vec<StringEntry>,
    db: Database,
    store: BackupManager,
    registry: FormatRegistry,
}

impl Fixture {
    fn new(ext: &str) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let game = temp.path().join("game");
        let data = game.join("Data");
        fs::create_dir_all(&data).unwrap();
        let mut actors = V::parse(&build_test_fixture()).unwrap();
        if let V::Array(items) = &mut actors {
            *items[1].get_ivar_mut("@description").unwrap() =
                V::Str("Small description\nSecond line".into());
        }
        // A >40-column original line exposes width drift as well as shifted IDs.
        let first = "This original line establishes a wider message window for wrapping";
        let mut commands = if ext == "rxdata" {
            vec![command(101, vec![V::Str(first.into())])]
        } else {
            vec![
                command(
                    101,
                    vec![V::Str(String::new()), V::Int(0), V::Int(0), V::Int(2)],
                ),
                command(401, vec![V::Str(first.into())]),
            ]
        };
        commands.push(command(401, vec![V::Str("Second line".into())]));
        commands.push(command(0, vec![]));
        commands.push(command(101, vec![V::Str(String::new())]));
        commands.push(command(401, vec![V::Str("Later message".into())]));
        commands.push(command(
            102,
            vec![V::Array(vec![V::Str("Yes".into()), V::Str("No".into())])],
        ));
        commands.push(command(0, vec![]));
        let list = V::Array(commands);
        let page = object("RPG::Event::Page", &[("@list", list.clone())]);
        let map = object(
            "RPG::Map",
            &[(
                "@events",
                V::Hash(vec![(
                    V::Int(1),
                    object("RPG::Event", &[("@pages", V::Array(vec![page.clone()]))]),
                )]),
            )],
        );
        let common = V::Array(vec![V::Nil, object("RPG::CommonEvent", &[("@list", list)])]);
        let mut values = vec![];
        for name in [
            "Actors", "Classes", "Enemies", "States", "Skills", "Items", "Weapons", "Armors",
        ] {
            values.push((name, actors.clone()));
        }
        values.extend([
            ("Map001", map),
            ("CommonEvents", common),
            // HEAD does not extract System or Troops. Direct must preserve these
            // opaque files; adding support would change the first-pass contract.
            (
                "System",
                object(
                    "RPG::System",
                    &[("@game_title", V::Str("Game title".into()))],
                ),
            ),
            (
                "Troops",
                V::Array(vec![
                    V::Nil,
                    object(
                        "RPG::Troop",
                        &[
                            ("@name", V::Str("Enemies".into())),
                            ("@pages", V::Array(vec![page])),
                        ],
                    ),
                ]),
            ),
        ]);
        let files = values
            .into_iter()
            .map(|(name, value)| {
                let path = data.join(format!("{name}.{ext}"));
                let bytes = value.serialize();
                fs::write(&path, &bytes).unwrap();
                (path, bytes)
            })
            .collect();
        let plugin = RpgMakerVxaPlugin::new();
        let rows = plugin.extract(&game).unwrap();
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
        assert!(!self.rows.is_empty(), "fixture must extract translations");
        for row in &mut self.rows {
            row.translation = Some(format!(
                "Pass {pass} {} {}",
                "many extra words ".repeat(pass + 4),
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
            "rpgmaker-vxa",
            &["es".into()],
        )
    }

    fn bytes(&self) -> Vec<Vec<u8>> {
        self.files
            .iter()
            .map(|(p, _)| fs::read(p).unwrap())
            .collect()
    }

    fn assert_oracle(&self, actual: &[Vec<u8>]) {
        for (p, b) in &self.files {
            fs::write(p, b).unwrap();
        }
        let report = RpgMakerVxaPlugin::new()
            .inject(&self.game, &self.rows)
            .unwrap();
        assert_eq!(report.strings_skipped, 0);
        for ((path, _), (expected, actual)) in
            self.files.iter().zip(self.bytes().iter().zip(actual))
        {
            assert!(
                expected == actual,
                "two passes must equal pristine one-shot: {}",
                path.display()
            );
        }
    }
}

fn words(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn repeated_direct(ext: &str) {
    let mut f = Fixture::new(ext);
    assert_eq!(
        f.rows.len(),
        46,
        "all supported database fields and event messages"
    );
    f.translate(1);
    let first = f.inject().unwrap();
    assert_eq!(first.strings_written, f.rows.len());
    f.translate(2);
    let second = f.inject().unwrap();
    assert_eq!(second.strings_written, f.rows.len());
    assert_eq!(second.strings_skipped, 0);
    assert!(second.reports["es"].skip_reasons.is_empty());
    let pass2 = f.bytes();
    // Compare bytes before testing text, to catch randomized Marshal ivar order.
    let mut oracle = Fixture::new(ext);
    oracle.translate(2);
    oracle.inject().unwrap();
    for ((path, _), (actual, expected)) in f.files.iter().zip(pass2.iter().zip(oracle.bytes())) {
        // Unsupported files retain their own original bytes (fixture HashMaps
        // can serialize differently before the deterministic writer is fixed).
        if matches!(
            path.file_stem().unwrap().to_str().unwrap(),
            "System" | "Troops"
        ) {
            continue;
        }
        assert!(
            actual == &expected,
            "pass 2 differs from one-shot: {}",
            path.display()
        );
    }
    let extracted = RpgMakerVxaPlugin::new().extract(&f.game).unwrap();
    assert_eq!(extracted.len(), f.rows.len());
    for row in &f.rows {
        let output = extracted
            .iter()
            .find(|new| {
                new.file_path == row.file_path
                    && words(&new.source) == words(row.translation.as_deref().unwrap())
            })
            .unwrap_or_else(|| panic!("missing {}", row.id));
        if row.source.contains('\n') {
            assert!(output.source.contains('\n'));
        }
    }
    assert!(
        extracted
            .iter()
            .any(|new| f.rows.iter().any(|old| old.file_path == new.file_path
                && old.id != new.id
                && words(&new.source) == words(old.translation.as_deref().unwrap()))),
        "fixture must shift later command IDs"
    );
    for (path, original) in f.files.iter().rev().take(2) {
        assert_eq!(&fs::read(path).unwrap(), original);
    }
    let modified: Vec<_> = f
        .files
        .iter()
        .map(|(p, _)| fs::metadata(p).unwrap().modified().unwrap())
        .collect();
    let third = f.inject().unwrap();
    assert_eq!(third.files_modified, 0, "unchanged third pass");
    assert!(third.files_written.is_empty());
    assert_eq!(f.bytes(), pass2);
    for ((p, _), before) in f.files.iter().zip(modified) {
        assert_eq!(fs::metadata(p).unwrap().modified().unwrap(), before);
    }
    f.assert_oracle(&pass2);
}

#[test]
fn direct_revision_xp_rxdata() {
    repeated_direct("rxdata");
}
#[test]
fn direct_revision_vx_rvdata() {
    repeated_direct("rvdata");
}
#[test]
fn direct_revision_ace_rvdata2() {
    repeated_direct("rvdata2");
}

#[test]
fn direct_revision_game_update_refused() {
    let mut f = Fixture::new("rxdata");
    f.translate(1);
    f.inject().unwrap();
    fs::write(&f.files[0].0, &f.files[0].1).unwrap();
    let live = f.bytes();
    f.translate(2);
    let error = f.inject().unwrap_err().to_string();
    assert!(
        error.contains("source_changed") && error.contains("Actors"),
        "{error}"
    );
    assert_eq!(f.bytes(), live);
}

#[test]
fn direct_revision_stale_source_refused() {
    for revision in [false, true] {
        let mut f = Fixture::new("rvdata2");
        f.translate(1);
        if revision {
            f.inject().unwrap();
        }
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
}

#[test]
fn direct_revision_corrupt_pristine_refused() {
    let mut f = Fixture::new("rxdata");
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
    let mut f = Fixture::new("rxdata");
    f.translate(1);
    let path = f.files[0].0.clone();
    let reader = RevisionOriginal::capture(&path).unwrap();
    // Keep it valid Marshal so an implementation ignoring the reader succeeds.
    fs::write(&path, V::Array(vec![V::Nil]).serialize()).unwrap();
    let error = RpgMakerVxaPlugin::new()
        .inject_revision(&f.game, &mut f.rows, &HashMap::from([(path, reader)]))
        .unwrap_err()
        .to_string();
    assert!(error.contains("revision original"), "{error}");
}

#[test]
fn direct_revision_restores_source_and_replays_unedited_rows() {
    let mut f = Fixture::new("rxdata");
    f.translate(1);
    f.inject().unwrap();
    let row = f
        .rows
        .iter_mut()
        .find(|r| r.id.ends_with("#1#@name"))
        .unwrap();
    row.translation = Some(row.source.clone());
    f.db.save_entries(&f.rows).unwrap();
    let report = f.inject().unwrap();
    assert_eq!(report.strings_written, f.rows.len());
    f.assert_oracle(&f.bytes());
}

#[test]
fn direct_revision_selected_file() {
    let mut f = Fixture::new("rvdata2");
    f.game = f.files[0].0.clone();
    f.rows.retain(|r| r.file_path == f.game);
    f.translate(1);
    f.inject().unwrap();
    f.translate(2);
    f.inject().unwrap();
    f.assert_oracle(&f.bytes());
}

#[test]
fn direct_revision_missing_locator_refused() {
    let mut f = Fixture::new("rxdata");
    f.translate(1);
    for row in &mut f.rows {
        row.id.push_str("#nonexistent");
    }
    // IDs are primary keys: use a fresh DB so the original IDs aren't retained.
    f.db = Database::open_in_memory().unwrap();
    f.db.save_entries(&f.rows).unwrap();
    let before = f.bytes();
    let report = f.inject().unwrap();
    assert_eq!(report.strings_written, 0);
    assert_eq!(
        report.reports["es"].skip_reasons.get("missing_target"),
        Some(&f.rows.len())
    );
    assert_eq!(f.bytes(), before);
}
