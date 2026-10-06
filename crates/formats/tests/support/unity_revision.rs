use super::*;
use locust_core::backup::BackupManager;
use locust_core::database::Database;
use locust_core::extraction::{inject_direct, DirectInjectReport, FormatRegistry};
use std::fs;

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
    fn new(bundle: bool) -> Self {
        use crate::unity_serialized::*;
        let temp = tempfile::tempdir().unwrap();
        let game = temp.path().join("game");
        let data = game.join("Revision_Data");
        fs::create_dir_all(&data).unwrap();
        let assets = [
            ("resources.assets", write_v17_fixture_ex("Story", "Welcome traveler!", Some(("Fixed object name.", "Fixed dialogue slot.")))),
            ("level0", write_v17_fixture_ex("ManagedText", "Greeting: Welcome traveler!\nFarewell: See you later.\n", None)),
            ("sharedassets0.assets", write_v17_fixture_ex("Items", "ID,Name,Description\n1,Small potion,Restores health.\n2,Large potion,Restores more health.\n", None)),
            ("level1", write_v17_mixed_objects_fixture(1)),
        ];
        let files = if bundle {
            let mut nodes: Vec<_> = assets
                .iter()
                .map(|(name, bytes)| (*name, bytes.as_slice()))
                .collect();
            nodes.push(("opaque.resS", b"opaque bytes must survive"));
            vec![(
                data.join("data.unity3d"),
                crate::unity_fs::build_test_bundle(&nodes, true, 8, true, true),
            )]
        } else {
            assets
                .into_iter()
                .map(|(name, bytes)| (data.join(name), bytes))
                .collect()
        };
        for (path, bytes) in &files {
            fs::write(path, bytes).unwrap();
        }
        let plugin = UnityPlugin::new();
        let rows = plugin.extract(&game).unwrap();
        for kind in [
            "textasset",
            "textasset_loc_line",
            "textasset_csv_cell",
            "monobehaviour",
            "textmesh",
            "guitext",
        ] {
            assert!(
                rows.iter().any(|row| row
                    .metadata
                    .get("extraction_method")
                    .and_then(|v| v.as_str())
                    == Some(kind)),
                "missing {kind}: {rows:?}"
            );
        }
        assert!(
            rows.iter().any(|row| !is_structural_entry(row)),
            "fixture needs a heuristic slot after the growing TextAsset"
        );
        let mut registry = FormatRegistry::new();
        registry.register(Box::new(plugin));
        let db = Database::open_in_memory().unwrap();
        let store = BackupManager::new(temp.path().join("backups"));
        Self {
            _temp: temp,
            game,
            files,
            db,
            store,
            registry,
            rows,
        }
    }

    fn translate(&mut self, pass: usize) {
        for row in &mut self.rows {
            row.translation = Some(if is_textasset_entry(row) {
                format!("Pass {pass} expanded translation: {}", row.source)
            } else if pass == 1 {
                row.source.to_ascii_uppercase()
            } else {
                row.source.to_ascii_lowercase()
            });
            assert_ne!(row.translation.as_deref(), Some(row.source.as_str()));
        }
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
            .map(|(path, _)| fs::read(path).unwrap())
            .collect()
    }
}

fn repeated_direct(bundle: bool) {
    let mut f = Fixture::new(bundle);
    f.translate(1);
    let first = f.inject().unwrap();
    assert_eq!(first.strings_written, f.rows.len(), "pass 1: {first:?}");
    f.translate(2);
    let second = f.inject().unwrap();
    assert_eq!(second.strings_written, f.rows.len(), "pass 2: {second:?}");
    assert_eq!(
        second.reports["es"].skip_reasons.get("source_changed"),
        None
    );
    let extracted = UnityPlugin::new().extract(&f.game).unwrap();
    for row in &f.rows {
        assert!(
            extracted
                .iter()
                .any(|new| Some(&new.source) == row.translation.as_ref()),
            "pass 2 missing {}: {:?}",
            row.id,
            row.translation
        );
    }
    let pass2 = f.bytes();
    let third = f.inject().unwrap();
    assert_eq!(
        f.bytes(),
        pass2,
        "unchanged third pass must be byte-identical: {third:?}"
    );
    // One-shot oracle, with the same extracted ids/locators and translations.
    for (path, original) in &f.files {
        fs::write(path, original).unwrap();
    }
    UnityPlugin::new().inject(&f.game, &f.rows).unwrap();
    assert_eq!(
        f.bytes(),
        pass2,
        "repeated Direct must equal a pristine one-shot injection"
    );
}

#[test]
fn direct_revision_serialized_all_row_kinds() {
    repeated_direct(false);
}

#[test]
fn direct_revision_unityfs_all_row_kinds() {
    repeated_direct(true);
}

#[test]
fn direct_revision_game_replacement_is_source_changed() {
    for bundle in [false, true] {
        let mut f = Fixture::new(bundle);
        f.translate(1);
        f.inject().unwrap();
        let path = &f.files[0].0;
        let mut replacement = fs::read(path).unwrap();
        let last = replacement.len() - 1;
        replacement[last] ^= 1; // Same-size game update, outside a text slot.
        fs::write(path, &replacement).unwrap();
        f.translate(2);
        let error = f.inject().unwrap_err().to_string();
        assert!(
            error.contains("source_changed"),
            "must name stale-source refusal: {error}"
        );
        assert_eq!(
            f.bytes()[0],
            replacement,
            "game update must never be overwritten"
        );
    }
}

#[test]
fn direct_revision_can_restore_source_translations() {
    for bundle in [false, true] {
        let mut f = Fixture::new(bundle);
        f.translate(1);
        f.inject().unwrap();
        for row in &mut f.rows {
            row.translation = Some(row.source.clone());
        }
        f.db.save_entries(&f.rows).unwrap();
        let report = f.inject().unwrap();
        assert_eq!(
            report.strings_written,
            f.rows.len(),
            "restoring original text is an edit: {report:?}"
        );
        let restored = UnityPlugin::new().extract(&f.game).unwrap();
        assert_eq!(
            restored
                .iter()
                .map(|row| (&row.id, &row.source, &row.metadata))
                .collect::<Vec<_>>(),
            f.rows
                .iter()
                .map(|row| (&row.id, &row.source, &row.metadata))
                .collect::<Vec<_>>()
        );
    }
}

#[test]
fn direct_revision_stale_source_still_refused() {
    for bundle in [false, true] {
        let mut f = Fixture::new(bundle);
        f.translate(1);
        f.inject().unwrap();
        f.translate(2);
        let stale = f
            .rows
            .iter_mut()
            .find(|row| {
                row.metadata
                    .get("extraction_method")
                    .and_then(|v| v.as_str())
                    == Some("textasset")
            })
            .unwrap();
        stale.source = "A stale extraction!".into();
        f.db.save_entries(&f.rows).unwrap();
        let report = f.inject().unwrap();
        assert_eq!(
            report.reports["es"].skip_reasons.get("source_changed"),
            Some(&1)
        );
        assert_eq!(report.strings_written, f.rows.len() - 1);
    }
}

#[test]
fn direct_revision_corrupt_pristine_refused() {
    for bundle in [false, true] {
        let mut f = Fixture::new(bundle);
        f.translate(1);
        let first = f.inject().unwrap();
        let live = f.bytes();
        let original = Path::new(first.backup_path.as_ref().unwrap())
            .join("payload")
            .join(f.files[0].0.strip_prefix(&f.game).unwrap());
        let mut bytes = fs::read(&original).unwrap();
        bytes[0] ^= 1;
        fs::write(original, bytes).unwrap();
        f.translate(2);
        let error = f.inject().unwrap_err().to_string();
        assert!(
            error.contains("pristine") || error.contains("digest") || error.contains("hash"),
            "{error}"
        );
        assert_eq!(f.bytes(), live);
    }
}
