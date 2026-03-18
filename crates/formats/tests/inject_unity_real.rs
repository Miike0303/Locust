use std::path::Path;
use locust_core::extraction::FormatPlugin;

#[test]
#[ignore]
fn test_inject_unity_direct() {
    let game_dir = Path::new(r"D:\juegos\unity\Out of Touch");
    let db_path = Path::new(r"C:\Projects\Locust\Out of Touch.locust.db");
    if !game_dir.exists() || !db_path.exists() { return; }

    let db = locust_core::database::Database::open(db_path).unwrap();
    let entries = db.get_entries(&locust_core::database::EntryFilter::default()).unwrap();
    let translated = entries.iter().filter(|e| e.translation.is_some()).count();
    println!("Loaded {} entries, {} translated", entries.len(), translated);

    let plugin = locust_formats::unity::UnityPlugin::new();
    let report = plugin.inject(game_dir, &entries).unwrap();

    println!("Files modified: {}", report.files_modified);
    println!("Strings written: {}", report.strings_written);
    println!("Strings skipped: {}", report.strings_skipped);
    assert!(report.files_modified > 0);
}
