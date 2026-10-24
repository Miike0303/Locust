//! Exercise the same small NScripter fixture as replace_source_provenance.
#[path = "../../formats/src/nscripter.rs"]
mod nscripter;
// Only the helpers nscripter.rs needs are used; the rest of util.rs is dead here.
#[allow(dead_code)]
#[path = "../../formats/src/util.rs"]
mod util;

use locust_core::{
    backup::BackupManager,
    database::Database,
    extraction::{inject_direct, FormatPlugin, FormatRegistry, MultiLangInjector},
    models::{OutputMode, StringStatus},
};
use std::{fs, path::Path, sync::Arc};
use tokio::sync::mpsc;

const ORIGINAL: &str = "*start\r\ncaption \"Original title\"\r\n`Original dialogue`\r\n";
const INVALID_CODES: &[&str] = &[
    "../x",
    "..\\x",
    "a/b",
    "C:x",
    "",
    ".",
    "es\0",
    "../../evil",
    "../../../evil",
];

fn listing(root: &Path) -> Vec<std::path::PathBuf> {
    let mut paths: Vec<_> = walkdir::WalkDir::new(root)
        .into_iter()
        .map(|entry| entry.unwrap().path().strip_prefix(root).unwrap().to_owned())
        .collect();
    paths.sort();
    paths
}

fn fixture(root: &Path) -> MultiLangInjector {
    let game = root.join("game");
    fs::create_dir(&game).unwrap();
    fs::write(game.join("0.txt"), ORIGINAL).unwrap();
    let plugin = nscripter::NScripterPlugin::new();
    let mut entries = plugin.extract(&game).unwrap();
    assert_eq!(entries.len(), 2);
    for entry in &mut entries {
        if entry.source == "Original title" {
            entry.translation = Some("Translated title".into());
            entry.status = StringStatus::Translated;
        }
    }
    let db = Arc::new(Database::open_in_memory().unwrap());
    db.save_entries(&entries).unwrap();
    let mut registry = FormatRegistry::new();
    registry.register(Box::new(plugin));
    MultiLangInjector::new(
        Arc::new(registry),
        db,
        Arc::new(BackupManager::new(root.join("backups"))),
    )
}

#[tokio::test]
async fn replace_invalid_language_creates_nothing() {
    for lang in INVALID_CODES {
        let temp = tempfile::tempdir().unwrap();
        let injector = fixture(temp.path());
        let before = listing(temp.path());
        let (tx, _rx) = mpsc::channel(32);
        let result = injector
            .inject(
                &temp.path().join("game"),
                "nscripter",
                OutputMode::Replace,
                vec![(*lang).into()],
                Some(temp.path().join("output")),
                tx,
            )
            .await;
        assert_eq!(listing(temp.path()), before, "side effects for {lang:?}");
        let error = result.unwrap_err().to_string();
        assert!(
            error.contains("invalid target language code"),
            "{lang:?}: {error}"
        );
        assert_eq!(
            fs::read_to_string(temp.path().join("game/0.txt")).unwrap(),
            ORIGINAL
        );
    }
}

#[tokio::test]
async fn replace_valid_language_preserves_copy_behavior() {
    for lang in ["es", "pt-BR"] {
        let temp = tempfile::tempdir().unwrap();
        let injector = fixture(temp.path());
        let (tx, _rx) = mpsc::channel(32);
        let report = injector
            .inject(
                &temp.path().join("game"),
                "nscripter",
                OutputMode::Replace,
                vec![lang.into()],
                Some(temp.path().join("output")),
                tx,
            )
            .await
            .unwrap();
        assert!(report.languages_failed.is_empty(), "{report:?}");
        assert_eq!(report.languages_processed, [lang]);
        let dest = temp.path().join("output").join(format!("game-{lang}"));
        assert_eq!(report.injected_roots[lang], dest);
        assert_eq!(
            fs::read_to_string(dest.join("0.txt")).unwrap(),
            ORIGINAL.replace("Original title", "Translated title")
        );
        assert_eq!(
            fs::read_to_string(temp.path().join("game/0.txt")).unwrap(),
            ORIGINAL
        );
    }
}

#[tokio::test]
async fn add_invalid_language_is_rejected_before_plugin_or_backup() {
    for lang in INVALID_CODES {
        let temp = tempfile::tempdir().unwrap();
        let injector = fixture(temp.path());
        let before = listing(temp.path());
        let (tx, _rx) = mpsc::channel(32);
        let error = injector
            .inject(
                &temp.path().join("game"),
                "nscripter",
                OutputMode::Add,
                vec![(*lang).into()],
                None,
                tx,
            )
            .await
            .unwrap_err();
        assert_eq!(listing(temp.path()), before, "side effects for {lang:?}");
        assert!(
            error.to_string().contains("invalid target language code"),
            "{lang:?}: {error}"
        );
    }
}

#[test]
fn direct_invalid_language_creates_nothing() {
    for lang in INVALID_CODES {
        let temp = tempfile::tempdir().unwrap();
        let injector = fixture(temp.path());
        let before = listing(temp.path());
        let result = inject_direct(
            &injector.registry,
            &injector.db,
            &injector.backup_manager,
            &temp.path().join("game"),
            "nscripter",
            &[(*lang).into()],
        );
        assert_eq!(listing(temp.path()), before, "side effects for {lang:?}");
        let error = result.unwrap_err();
        assert!(
            error.to_string().contains("invalid target language code"),
            "{lang:?}: {error}"
        );
    }
}
