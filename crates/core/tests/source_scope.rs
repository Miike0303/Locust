//! Preflight must run before any lock, database binding, backup or copy.
use std::path::Path;
use std::sync::Arc;

use locust_core::backup::BackupManager;
use locust_core::database::Database;
use locust_core::error::{LocustError, Result};
use locust_core::extraction::{
    inject_direct, FormatPlugin, FormatRegistry, InjectionReport, MultiLangInjector,
};
use locust_core::models::{OutputMode, StringEntry};

struct FileOnly;
impl FormatPlugin for FileOnly {
    fn id(&self) -> &str {
        "file-only"
    }
    fn name(&self) -> &str {
        "File only"
    }
    fn supported_extensions(&self) -> &[&str] {
        &["pck"]
    }
    fn validate_source(&self, _: &Path) -> Result<()> {
        Err(LocustError::UnsupportedFormat(
            "source scope refused".into(),
        ))
    }
    fn extract(&self, _: &Path) -> Result<Vec<StringEntry>> {
        panic!("preflight bypassed")
    }
    fn inject(&self, _: &Path, _: &[StringEntry]) -> Result<InjectionReport> {
        panic!("preflight bypassed")
    }
}

#[tokio::test]
async fn source_scope_precedes_all_core_side_effects() {
    let temp = tempfile::tempdir().unwrap();
    let game = temp.path().join("category");
    std::fs::create_dir(&game).unwrap();
    std::fs::write(game.join("foreign.txt"), "foreign").unwrap();
    let mut registry = FormatRegistry::new();
    registry.register(Box::new(FileOnly));
    let registry = Arc::new(registry);
    let db = Arc::new(Database::open_in_memory().unwrap());
    let backup_path = temp.path().join("backups");
    let backup = Arc::new(BackupManager::new(backup_path.clone()));
    let output = temp.path().join("output");
    let error =
        locust_core::project::open_project(&db, &registry, &game, Some("file-only")).unwrap_err();
    assert!(
        error.to_string().contains("source scope refused"),
        "{error}"
    );
    let error = locust_core::project::open_recent_project(&db, &registry, &game, Some("file-only"))
        .unwrap_err();
    assert!(
        error.to_string().contains("source scope refused"),
        "{error}"
    );
    let saved = temp.path().join("saved.db");
    let saved_db = Database::open(&saved).unwrap();
    drop(saved_db);
    let before = std::fs::read(&saved).unwrap();
    let error = locust_core::project::open_project_db(&db, &registry, &saved, &game, "file-only")
        .unwrap_err();
    assert!(
        error.to_string().contains("source scope refused"),
        "{error}"
    );
    assert_eq!(std::fs::read(saved).unwrap(), before);
    let error = locust_core::project::open_verified_saved_project(
        &db,
        &registry,
        &game.join("absent.db"),
        &game,
        "file-only",
    )
    .unwrap_err();
    assert!(
        error.to_string().contains("source scope refused"),
        "{error}"
    );
    let preflight =
        locust_core::project::preflight_project_open(&registry, &game, Some("file-only")).unwrap();
    assert!(
        matches!(preflight, locust_core::project::ProjectOpenPreflight::NeedsAttention { reason } if reason.contains("source scope refused"))
    );
    let error = inject_direct(&registry, &db, &backup, &game, "file-only", &[]).unwrap_err();
    assert!(
        error.to_string().contains("source scope refused"),
        "{error}"
    );
    let injector = MultiLangInjector::new(registry, db, backup);
    for mode in [OutputMode::Replace, OutputMode::Add] {
        let (tx, _rx) = tokio::sync::mpsc::channel(20);
        let error = injector
            .inject(
                &game,
                "file-only",
                mode,
                vec!["es".into()],
                Some(output.clone()),
                tx,
            )
            .await
            .unwrap_err();
        assert!(
            error.to_string().contains("source scope refused"),
            "{error}"
        );
        assert!(!output.exists());
        assert!(!backup_path.exists());
        assert!(!game.join(".locust-injections").exists());
        assert_eq!(std::fs::read_dir(&game).unwrap().count(), 1);
    }
}
