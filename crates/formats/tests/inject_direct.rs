use std::path::Path;
use locust_core::extraction::FormatPlugin;

#[test]
#[ignore]
fn test_inject_sugarcube_direct() {
    let game_dir = Path::new(r"D:\juegos\html\The SUP v1.0 backer version");
    let db_path = Path::new(r"C:\Projects\Locust\The SUP v1.0 backer version.locust.db");
    if !game_dir.exists() || !db_path.exists() { return; }

    // Make a copy of just the HTML file to avoid modifying original
    let output_dir = std::env::temp_dir().join("locust_sup_inject");
    let _ = std::fs::remove_dir_all(&output_dir);
    std::fs::create_dir_all(&output_dir).unwrap();
    std::fs::copy(
        game_dir.join("The SUP.html"),
        output_dir.join("The SUP.html"),
    ).unwrap();

    let db = locust_core::database::Database::open(db_path).unwrap();
    let mut entries = db.get_entries(&locust_core::database::EntryFilter::default()).unwrap();
    let translated = entries.iter().filter(|e| e.translation.is_some()).count();
    println!("SugarCube: {} entries, {} translated", entries.len(), translated);

    // Rewrite file_path to the copy
    for entry in &mut entries {
        entry.file_path = output_dir.join("The SUP.html");
    }

    let plugin = locust_formats::sugarcube::SugarCubePlugin::new();
    let report = plugin.inject(&output_dir, &entries).unwrap();

    println!("Files modified: {}", report.files_modified);
    println!("Strings written: {}", report.strings_written);
    println!("Strings skipped: {}", report.strings_skipped);

    // Copy Images too
    let src_images = game_dir.join("Images");
    if src_images.exists() {
        let _ = copy_dir_recursive(&src_images, &output_dir.join("Images"));
    }

    println!("Output at: {}", output_dir.display());
}

#[test]
#[ignore]
fn test_inject_rpgmaker_xp_direct() {
    let game_dir = Path::new(r"D:\juegos\rpgm\en\LoQOO\Legend of Queen Opala - Origin");
    let db_path = Path::new(r"C:\Projects\Locust\Legend of Queen Opala - Origin.locust.db");
    if !game_dir.exists() || !db_path.exists() { return; }

    // Copy only the Data directory
    let output_dir = std::env::temp_dir().join("locust_loqo_inject");
    let _ = std::fs::remove_dir_all(&output_dir);
    let _ = copy_dir_recursive(game_dir, &output_dir);

    let db = locust_core::database::Database::open(db_path).unwrap();
    let mut entries = db.get_entries(&locust_core::database::EntryFilter::default()).unwrap();
    let translated = entries.iter().filter(|e| e.translation.is_some()).count();
    println!("RPG Maker XP: {} entries, {} translated", entries.len(), translated);

    // Rewrite file_paths to the copy
    for entry in &mut entries {
        let fname = entry.file_path.file_name().unwrap_or_default().to_os_string();
        entry.file_path = output_dir.join("Data").join(fname);
    }

    let plugin = locust_formats::rpgmaker_vxa::RpgMakerVxaPlugin::new();
    let report = plugin.inject(&output_dir, &entries).unwrap();

    println!("Files modified: {}", report.files_modified);
    println!("Strings written: {}", report.strings_written);
    println!("Strings skipped: {}", report.strings_skipped);
    println!("Output at: {}", output_dir.display());
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());
        if src_path.is_dir() {
            copy_dir_recursive(&src_path, &dst_path)?;
        } else {
            std::fs::copy(&src_path, &dst_path)?;
        }
    }
    Ok(())
}
