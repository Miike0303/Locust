/// Integration tests against real game directories.
/// Run with: cargo test -p locust-formats --test real_games -- --ignored --nocapture
use std::path::Path;

#[test]
#[ignore]
fn test_rpgmaker_xp_detect_and_extract() {
    let game_dir = Path::new(r"D:\juegos\rpgm\en\LoQOO\Legend of Queen Opala - Origin");
    if !game_dir.exists() {
        eprintln!("Skipping: game dir not found");
        return;
    }

    let registry = locust_formats::default_registry();

    let plugin = registry.detect(game_dir).expect("Should detect RPG Maker XP");
    assert_eq!(plugin.id(), "rpgmaker-vxa");
    println!("Detected as: {}", plugin.name());

    // Detect from exe via resolve
    let exe = game_dir.join("Game.exe");
    let resolved = locust_core::extraction::resolve_game_root(&exe, &registry);
    println!("Resolved exe to: {}", resolved.display());
    assert!(registry.detect(&resolved).is_some());

    let entries = plugin.extract(game_dir).expect("Should extract strings");
    println!("RPG Maker XP: {} strings extracted", entries.len());
    assert!(!entries.is_empty(), "Should have extracted strings");

    for e in entries.iter().take(10) {
        println!("  [{}] {}", e.id, &e.source[..e.source.len().min(80)]);
    }
}

#[test]
#[ignore]
fn test_sugarcube_html_detect_and_extract() {
    let game_dir = Path::new(r"D:\juegos\html\The SUP v1.0 backer version");
    if !game_dir.exists() {
        eprintln!("Skipping: game dir not found");
        return;
    }

    let registry = locust_formats::default_registry();

    let plugin = registry.detect(game_dir).expect("Should detect SugarCube");
    assert_eq!(plugin.id(), "sugarcube");
    println!("Detected as: {}", plugin.name());

    let html_file = game_dir.join("The SUP.html");
    let resolved = locust_core::extraction::resolve_game_root(&html_file, &registry);
    println!("Resolved html to: {}", resolved.display());
    assert!(registry.detect(&resolved).is_some());

    let entries = plugin.extract(game_dir).expect("Should extract strings");
    println!("SugarCube: {} strings extracted", entries.len());
    assert!(!entries.is_empty(), "Should have extracted strings");

    for e in entries.iter().take(10) {
        let preview = &e.source[..e.source.len().min(80)];
        println!("  [{}] {}", &e.id[..e.id.len().min(50)], preview);
    }
}

#[test]
#[ignore]
fn test_renpy_rpa_detect_and_extract() {
    let game_dir = Path::new(r"D:\juegos\renpy\FindingCloud9-0.9.2-pc");
    if !game_dir.exists() {
        eprintln!("Skipping: game dir not found");
        return;
    }

    let registry = locust_formats::default_registry();

    let plugin = registry.detect(game_dir).expect("Should detect Ren'Py");
    assert_eq!(plugin.id(), "renpy");
    println!("Detected as: {}", plugin.name());

    let exe = game_dir.join("FindingCloud9.exe");
    let resolved = locust_core::extraction::resolve_game_root(&exe, &registry);
    println!("Resolved exe to: {}", resolved.display());
    assert!(registry.detect(&resolved).is_some());

    let entries = plugin.extract(game_dir).expect("Should extract strings");
    println!("Ren'Py: {} strings extracted", entries.len());
    assert!(!entries.is_empty(), "Should have extracted strings from RPA");

    for e in entries.iter().take(10) {
        let preview = &e.source[..e.source.len().min(80)];
        println!("  [{}] {}", &e.id[..e.id.len().min(50)], preview);
    }
}
