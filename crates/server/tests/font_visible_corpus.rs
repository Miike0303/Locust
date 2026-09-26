use std::path::PathBuf;

use locust_core::models::{StringEntry, INJECTION_SOURCE_METADATA_KEY};
use locust_server::{create_test_state, start_test_server, ProjectInfo};

const FONT: &[u8] = include_bytes!("../../cli/tests/fixtures/synthetic-ascii.ttf");

#[tokio::test]
async fn font_coverage_includes_untranslated_physical_text_after_pivot() {
    let root = tempfile::tempdir().unwrap();
    let game = root.path().join("game");
    std::fs::create_dir_all(game.join("fonts")).unwrap();
    std::fs::write(game.join("fonts/body.ttf"), FONT).unwrap();
    let source = root.path().join("source.ttf");
    let mut variant = FONT.to_vec();
    variant.extend_from_slice(b"neutral independent source variant");
    std::fs::write(&source, &variant).unwrap();
    let state = create_test_state();
    *state.current_project.write().await = Some(ProjectInfo {
        path: game.clone(),
        format_id: "html-game".into(),
        name: "Neutral fixture".into(),
        extraction_warnings: vec![],
        ..Default::default()
    });
    let mut translated = StringEntry::new("translated", "日本語", PathBuf::from("story.html"));
    translated.translation = Some("English text".into());
    let (base, server) = start_test_server(state.clone()).await;
    let request = serde_json::json!({"game_path":game,"source_font":source,"target_path":"fonts/body.ttf","language":"en","output_path":root.path().join("font.zip")});
    for pivot in [false, true] {
        let mut untranslated = StringEntry::new(
            "untranslated",
            if pivot { "Pivot English" } else { "日本語" },
            PathBuf::from("story.html"),
        );
        if pivot {
            untranslated.metadata.insert(
                INJECTION_SOURCE_METADATA_KEY.into(),
                serde_json::json!("日本語"),
            );
            // Empty translations are also physical fallbacks, not invisible rows.
            untranslated.translation = Some("  ".into());
        }
        state
            .db
            .save_entries(&[translated.clone(), untranslated])
            .unwrap();
        let response = reqwest::Client::new()
            .post(format!("{base}/api/patch/font"))
            .json(&request)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 400);
        assert!(response.text().await.unwrap().contains("lacks"));
        assert!(!root.path().join("font.zip").exists());
        assert_eq!(std::fs::read(game.join("fonts/body.ttf")).unwrap(), FONT);
        assert_eq!(std::fs::read(&source).unwrap(), variant);
    }
    // A complete English translation does not unnecessarily require Japanese
    // coverage solely because Japanese is the original physical baseline.
    state
        .db
        .save_translation("untranslated", "More English", "manual")
        .await
        .unwrap();
    let response = reqwest::Client::new()
        .post(format!("{base}/api/patch/font"))
        .json(&request)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200, "{}", response.text().await.unwrap());
    server.abort();
}
