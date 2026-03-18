/// Direct inject test for Ren'Py RPA games.
/// Run: cargo test -p locust-formats --test inject_renpy_real -- --ignored --nocapture
use std::path::Path;
use locust_core::extraction::FormatPlugin;

#[test]
#[ignore]
fn test_inject_renpy_rpa_replace() {
    let game_dir = Path::new(r"D:\juegos\renpy\FindingCloud9-0.9.2-pc");
    if !game_dir.exists() {
        eprintln!("Skipping: game dir not found");
        return;
    }

    let db_path = Path::new(r"C:\Projects\Locust\FindingCloud9-0.9.2-pc.locust.db");
    if !db_path.exists() {
        eprintln!("Skipping: DB not found at {}", db_path.display());
        return;
    }

    let db = locust_core::database::Database::open(&db_path).expect("Failed to open DB");
    let entries = db.get_entries(&locust_core::database::EntryFilter::default()).expect("Failed to get entries");
    println!("Loaded {} entries from DB", entries.len());

    let translated = entries.iter().filter(|e| e.translation.is_some()).count();
    println!("  {} have translations", translated);

    let plugin = locust_formats::renpy::RenPyPlugin::new();

    println!("Injecting via Replace mode...");
    let report = plugin.inject(game_dir, &entries).expect("Injection failed");

    println!("Files modified: {}", report.files_modified);
    println!("Strings written: {}", report.strings_written);
    println!("Strings skipped: {}", report.strings_skipped);
    assert!(report.files_modified > 0, "Should have modified files");
}
