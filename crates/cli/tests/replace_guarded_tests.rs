use locust_core::database::Database;
use locust_core::models::StringEntry;
use rusqlite::Connection;

mod common;
use common::locust;

fn fixture() -> (tempfile::TempDir, std::path::PathBuf, Database, Connection) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("project.db");
    let db = Database::open(&path).unwrap();
    let rows: Vec<_> = ["a", "b", "c"]
        .into_iter()
        .map(|id| {
            let mut row = StringEntry::new(id, "Source", "story.html".into());
            row.translation = Some("old text".into());
            row
        })
        .collect();
    db.save_entries(&rows).unwrap();
    let conn = Connection::open(&path).unwrap();
    (dir, path, db, conn)
}

#[test]
fn replace_preserves_an_edit_after_read_and_reports_conflicts() {
    let (_dir, path, db, conn) = fixture();
    // Deterministic interleaving: after the CLI reads all rows, its first write
    // simulates a desktop edit to the next row. No timing-dependent sleeps.
    conn.execute_batch("CREATE TRIGGER intervening_edit AFTER UPDATE OF translation ON strings
        WHEN NEW.id = 'a' BEGIN
        UPDATE strings SET translation = 'desktop edit', status = 'approved', provider_used = 'manual' WHERE id = 'b';
        END;").unwrap();
    let assertion = locust()
        .arg("replace")
        .arg(&path)
        .args(["--find", "old", "--replace", "new"])
        .assert()
        .success();
    let output = String::from_utf8_lossy(&assertion.get_output().stdout);
    assert_eq!(
        db.get_entry("b").unwrap().unwrap().translation.as_deref(),
        Some("desktop edit")
    );
    assert_eq!(
        db.get_entry("b").unwrap().unwrap().provider_used.as_deref(),
        Some("manual")
    );
    for id in ["a", "c"] {
        assert_eq!(
            db.get_entry(id).unwrap().unwrap().translation.as_deref(),
            Some("new text")
        );
    }
    assert!(output.contains("Updated 2 string(s)"), "{output}");
    assert!(output.contains("1 conflict(s)"), "{output}");
    assert!(
        output.contains("translation changed since it was loaded"),
        "{output}"
    );
    assert!(
        !output.contains("3 occurrence(s)"),
        "must not count skipped occurrences: {output}"
    );
}

#[test]
fn replace_rolls_back_the_whole_batch_on_database_error() {
    let (_dir, path, db, conn) = fixture();
    conn.execute_batch(
        "CREATE TRIGGER fail_second BEFORE UPDATE OF translation ON strings
        WHEN NEW.id = 'b' BEGIN SELECT RAISE(ABORT, 'fixture failure'); END;",
    )
    .unwrap();
    locust()
        .arg("replace")
        .arg(&path)
        .args(["--find", "old", "--replace", "new"])
        .assert()
        .failure();
    for id in ["a", "b", "c"] {
        assert_eq!(
            db.get_entry(id).unwrap().unwrap().translation.as_deref(),
            Some("old text")
        );
    }
}
