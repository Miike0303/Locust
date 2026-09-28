use std::path::{Path, PathBuf};

use locust_core::database::{Database, EntryFilter};
use locust_core::export::import_po;

mod common;
use common::locust;

async fn project_fixture() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("game")).unwrap();
    std::fs::write(
        dir.path().join("game/script.rpy"),
        "label start:\n    e \"Hello, world!\"\n    e \"Goodbye!\"\n",
    )
    .unwrap();
    let project = dir.path().join("project.locust.db");
    locust()
        .arg("extract")
        .arg(dir.path())
        .arg("-o")
        .arg(&project)
        .assert()
        .success();
    let db = Database::open(&project).unwrap();
    assert!(db
        .save_translation("script.rpy#2", "¡Hola, mundo!", "mock")
        .await
        .unwrap());
    assert_eq!(db.get_entries(&EntryFilter::default()).unwrap().len(), 2);
    drop(db);
    (dir, project)
}

fn rows(project: &Path) -> serde_json::Value {
    let db = Database::open(project).unwrap();
    serde_json::to_value(db.get_entries(&EntryFilter::default()).unwrap()).unwrap()
}

fn assert_valid_po(output: &Path) {
    let content = std::fs::read_to_string(output).unwrap();
    assert!(content.contains("\"Language: es\\n\""));
    let entries = import_po(&content).unwrap();
    assert_eq!(entries.len(), 2);
    let hello = entries
        .iter()
        .find(|entry| entry.id.as_deref() == Some("script.rpy#2"))
        .unwrap();
    assert_eq!(hello.source, "Hello, world!");
    assert_eq!(hello.translation, "¡Hola, mundo!");
}

#[tokio::test]
async fn cli_export_refuses_project_database_as_output() {
    let (dir, project) = project_fixture().await;
    let before_rows = rows(&project);
    let before_bytes = std::fs::read(&project).unwrap();
    let aliases = [project.clone(), dir.path().join("./project.locust.db")];
    #[cfg(windows)]
    let aliases = [
        aliases[0].clone(),
        aliases[1].clone(),
        dir.path().join("PROJECT.LOCUST.DB"),
    ];

    for output in aliases {
        // Start with --overwrite so removing the alias guard exercises the
        // destructive write, rather than stopping at the existing-file guard.
        for overwrite in [true, false] {
            let mut command = locust();
            command
                .arg("export")
                .arg(&project)
                .args(["-f", "po", "-l", "es", "-o"])
                .arg(&output);
            if overwrite {
                command.arg("--overwrite");
            }
            command.assert().failure().stderr(predicates::str::contains(
                "the export destination is the project database itself; choose a different file",
            ));
            assert_eq!(std::fs::read(&project).unwrap(), before_bytes);
            assert_eq!(rows(&project), before_rows);
        }
    }
}

#[tokio::test]
async fn cli_export_preserves_existing_catalog_without_overwrite() {
    let (dir, project) = project_fixture().await;
    let output = dir.path().join("catalog.po");
    let sentinel = b"translator edits\0\xff\n";
    std::fs::write(&output, sentinel).unwrap();
    locust()
        .arg("export")
        .arg(&project)
        .args(["-f", "po", "-l", "es", "-o"])
        .arg(&output)
        .assert()
        .failure()
        .stderr(predicates::str::contains(format!(
            "{} already exists; pass --overwrite to replace it",
            output.display()
        )));
    assert_eq!(std::fs::read(&output).unwrap(), sentinel);

    locust()
        .arg("export")
        .arg(&project)
        .args(["-f", "po", "-l", "es", "--overwrite", "-o"])
        .arg(&output)
        .assert()
        .success();
    assert_valid_po(&output);
}

#[tokio::test]
async fn cli_export_to_new_path_succeeds() {
    let (dir, project) = project_fixture().await;
    let before_rows = rows(&project);
    let before_bytes = std::fs::read(&project).unwrap();
    locust()
        .current_dir(dir.path())
        .arg("export")
        .arg(&project)
        .args(["-f", "po", "-l", "es", "-o", "catalog.po"])
        .assert()
        .success();
    assert_valid_po(&dir.path().join("catalog.po"));
    assert_eq!(std::fs::read(&project).unwrap(), before_bytes);
    assert_eq!(rows(&project), before_rows);
}

#[tokio::test]
async fn cli_export_refuses_sidecars_and_hard_links_even_with_overwrite() {
    let (dir, project) = project_fixture().await;
    let before_rows = rows(&project);
    let before_bytes = std::fs::read(&project).unwrap();
    let alias = dir.path().join("alias.po");
    std::fs::hard_link(&project, &alias).unwrap();
    for output in [
        alias,
        dir.path().join("project.locust.db-wal"),
        dir.path().join("project.locust.db-shm"),
        dir.path().join("project.locust.db-journal"),
    ] {
        locust()
            .arg("export")
            .arg(&project)
            .args(["-f", "po", "-l", "es", "--overwrite", "-o"])
            .arg(&output)
            .assert()
            .failure()
            .stderr(predicates::str::contains("choose a different file"));
        assert_eq!(std::fs::read(&project).unwrap(), before_bytes);
        assert_eq!(rows(&project), before_rows);
    }
}
