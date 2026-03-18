use std::path::Path;
use locust_core::extraction::FormatPlugin;

#[test]
#[ignore]
fn test_unity_oot_extract() {
    let game_dir = Path::new(r"D:\juegos\unity\Out of Touch");
    if !game_dir.exists() { return; }

    let plugin = locust_formats::unity::UnityPlugin::new();
    assert!(plugin.detect(game_dir), "Should detect Unity");

    let entries = plugin.extract(game_dir).unwrap();
    println!("Extracted {} entries", entries.len());

    let dialogue = entries.iter().filter(|e| e.tags.contains(&"dialogue".to_string())).count();
    let menu = entries.iter().filter(|e| e.tags.contains(&"menu".to_string())).count();
    println!("  Dialogue: {}, Menu: {}", dialogue, menu);

    for e in entries.iter().take(15) {
        let ctx = e.context.as_deref().unwrap_or("-");
        println!("  [{}] ({}) {}", &e.id[..e.id.len().min(30)], ctx, &e.source[..e.source.len().min(60)]);
    }

    assert!(entries.len() > 1000, "Should have lots of dialogue, got {}", entries.len());
}
