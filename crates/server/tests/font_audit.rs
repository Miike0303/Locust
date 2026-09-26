use locust_core::models::{StringEntry, INJECTION_SOURCE_METADATA_KEY};
use locust_server::audit_project_fonts;

const FONT: &[u8] = include_bytes!("../../cli/tests/fixtures/synthetic-ascii.ttf");

#[tokio::test]
async fn font_audit_does_not_certify_a_stale_translation() {
    let temp = tempfile::tempdir().unwrap();
    std::fs::write(temp.path().join("active.ttf"), FONT).unwrap();
    let mut entry = StringEntry::new("stale-entry", "New source", "story.html".into());
    entry.translation = Some("Old translation".into());
    entry.metadata.insert(locust_core::models::STALE_TRANSLATION_METADATA_KEY.into(), serde_json::json!({
        "version": 1, "translated_source_sha256": "0".repeat(64), "current_source_sha256": entry.source_hash()
    }));
    let report = audit_project_fonts(Some(temp.path().to_owned()), vec![entry]).await;
    assert!(report.fonts.is_empty());
    assert_eq!(report.issues.len(), 1);
    assert!(report.issues[0].message.contains("stale translation"));
}

#[tokio::test]
async fn font_audit_excludes_recovery_for_directory_and_single_file_games() {
    let temp = tempfile::tempdir().unwrap();
    let game = temp.path().join("story.html");
    std::fs::write(&game, "<p>Hello</p>").unwrap();
    std::fs::write(temp.path().join("active.ttf"), FONT).unwrap();
    let recovery = temp
        .path()
        .join(".locust-injections/operations/old/work/fonts");
    std::fs::create_dir_all(&recovery).unwrap();
    std::fs::write(recovery.join("copy.ttf"), FONT).unwrap();
    std::fs::write(recovery.join("bad.ttf"), b"invalid").unwrap();
    for selection in [temp.path().to_owned(), game] {
        let report = audit_project_fonts(Some(selection), vec![]).await;
        assert_eq!(report.fonts.len(), 1);
        assert!(report.issues.is_empty());
    }
}

#[tokio::test]
async fn font_audit_includes_untranslated_physical_text_and_pivot_fallback() {
    let temp = tempfile::tempdir().unwrap();
    std::fs::write(temp.path().join("active.ttf"), FONT).unwrap();
    let mut translated = StringEntry::new("translated", "日", "story.html".into());
    translated.translation = Some("Hello".into());
    for pivot in [false, true] {
        for translation in [None, Some(""), Some(" \n ")] {
            let mut pending = StringEntry::new(
                "pending",
                if pivot { "English pivot" } else { "日" },
                "story.html".into(),
            );
            pending.translation = translation.map(str::to_owned);
            if pivot {
                pending.metadata.insert(
                    INJECTION_SOURCE_METADATA_KEY.into(),
                    serde_json::json!("日"),
                );
            }
            let report = audit_project_fonts(
                Some(temp.path().to_owned()),
                vec![translated.clone(), pending],
            )
            .await;
            assert!(report.issues.is_empty());
            assert_eq!(report.fonts.len(), 1);
            assert_eq!(report.fonts[0].missing_chars, vec!['日']);
            assert!(!report.fonts[0].has_full_coverage);
        }
    }
    let complete = audit_project_fonts(Some(temp.path().to_owned()), vec![translated]).await;
    assert!(
        complete.fonts[0].has_full_coverage,
        "replaced Japanese does not require unused glyphs"
    );
}

#[tokio::test]
async fn font_audit_surfaces_invalid_physical_provenance() {
    let temp = tempfile::tempdir().unwrap();
    std::fs::write(temp.path().join("active.ttf"), FONT).unwrap();
    let mut entry = StringEntry::new("bad-physical", "English pivot", "story.html".into());
    entry
        .metadata
        .insert(INJECTION_SOURCE_METADATA_KEY.into(), serde_json::json!(7));
    let report = audit_project_fonts(Some(temp.path().to_owned()), vec![entry]).await;
    assert!(report.fonts.is_empty());
    assert_eq!(report.issues.len(), 1);
    assert!(report.issues[0].message.contains("bad-physical"));
}

#[tokio::test]
async fn single_file_game_surfaces_unreadable_adjacent_fonts() {
    let temp = tempfile::tempdir().unwrap();
    let game = temp.path().join("story.html");
    std::fs::write(&game, "<html>Test</html>").unwrap();
    std::fs::create_dir(temp.path().join("fonts")).unwrap();
    let font = temp.path().join("fonts/body.woff2");
    std::fs::write(&font, b"wOF2unsupported-test-font").unwrap();
    let report = audit_project_fonts(Some(game), Vec::new()).await;
    assert!(report.fonts.is_empty());
    assert_eq!(report.issues.len(), 1);
    assert_eq!(report.issues[0].font_path, font);
    assert!(report.issues[0].message.contains("WOFF"));
}

#[tokio::test]
async fn missing_game_reports_audit_failure_instead_of_empty_success() {
    let temp = tempfile::tempdir().unwrap();
    let missing = temp.path().join("missing-game");
    let report = audit_project_fonts(Some(missing.clone()), Vec::new()).await;
    assert!(report.fonts.is_empty());
    assert_eq!(report.issues.len(), 1);
    assert_eq!(report.issues[0].font_path, missing);
}

#[tokio::test]
async fn unopened_project_has_no_fabricated_font_errors() {
    let report = audit_project_fonts(None, Vec::new()).await;
    assert!(report.fonts.is_empty());
    assert!(report.issues.is_empty());
}
