use locust_core::extraction::FormatPlugin;
use std::path::{Path, PathBuf};

#[test]
#[ignore = "requires LOCUST_REAL_SUGARCUBE_COPY pointing to an explicit game copy and LOCUST_REAL_SUGARCUBE_DB pointing to its translated database"]
fn test_inject_sugarcube_direct() {
    let (Some(game_dir), Some(db_path)) = (
        env_path("LOCUST_REAL_SUGARCUBE_COPY"),
        env_path("LOCUST_REAL_SUGARCUBE_DB"),
    ) else {
        return;
    };
    if !game_dir.exists() || !db_path.exists() {
        eprintln!("Skipping: game copy or translated database not found");
        return;
    }

    // Make a copy of just the HTML file to avoid modifying original
    let fixture = tempfile::tempdir().unwrap();
    let output_dir = fixture.path();
    std::fs::copy(
        game_dir.join("The SUP.html"),
        output_dir.join("The SUP.html"),
    )
    .unwrap();

    let db = locust_core::database::Database::open(&db_path).unwrap();
    let mut entries = db
        .get_entries(&locust_core::database::EntryFilter::default())
        .unwrap();
    let translated = entries.iter().filter(|e| e.translation.is_some()).count();
    println!(
        "SugarCube: {} entries, {} translated",
        entries.len(),
        translated
    );

    // Rewrite file_path to the copy
    for entry in &mut entries {
        entry.file_path = output_dir.join("The SUP.html");
    }

    let plugin = locust_formats::sugarcube::SugarCubePlugin::new();
    let report = plugin.inject(output_dir, &entries).unwrap();

    println!("Files modified: {}", report.files_modified);
    println!("Strings written: {}", report.strings_written);
    println!("Strings skipped: {}", report.strings_skipped);

    println!("Output at: {}", output_dir.display());
}

#[test]
#[ignore = "requires LOCUST_REAL_RPGMAKER_XP_COPY pointing to an explicit game copy and LOCUST_REAL_RPGMAKER_XP_DB pointing to its translated database"]
fn test_inject_rpgmaker_xp_direct() {
    let (Some(game_dir), Some(db_path)) = (
        env_path("LOCUST_REAL_RPGMAKER_XP_COPY"),
        env_path("LOCUST_REAL_RPGMAKER_XP_DB"),
    ) else {
        return;
    };
    if !game_dir.exists() || !db_path.exists() {
        eprintln!("Skipping: game copy or translated database not found");
        return;
    }

    // Copy only the Data directory
    let fixture = tempfile::tempdir().unwrap();
    let output_dir = fixture.path();
    copy_dir_recursive(&game_dir.join("Data"), &output_dir.join("Data")).unwrap();

    let db = locust_core::database::Database::open(&db_path).unwrap();
    let mut entries = db
        .get_entries(&locust_core::database::EntryFilter::default())
        .unwrap();
    let translated = entries.iter().filter(|e| e.translation.is_some()).count();
    println!(
        "RPG Maker XP: {} entries, {} translated",
        entries.len(),
        translated
    );

    // Rewrite file_paths to the copy
    for entry in &mut entries {
        let fname = entry
            .file_path
            .file_name()
            .unwrap_or_default()
            .to_os_string();
        entry.file_path = output_dir.join("Data").join(fname);
    }

    let plugin = locust_formats::rpgmaker_vxa::RpgMakerVxaPlugin::new();
    let report = plugin.inject(output_dir, &entries).unwrap();

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

fn env_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os(name);
    if path.is_none() {
        eprintln!("Skipping: {name} is unset");
    }
    path.map(PathBuf::from)
}
