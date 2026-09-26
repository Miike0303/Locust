//! Neutral, real-format extraction/reopen/injection regression fixtures.
use std::path::{Path, PathBuf};
use std::sync::Arc;

use locust_core::backup::BackupManager;
use locust_core::database::{Database, EntryFilter};
use locust_core::extraction::{inject_direct, MultiLangInjector};
use locust_core::models::{OutputMode, StringStatus};
use locust_core::project::open_project;

fn write_source(game: &Path, format: &str, source: &str) -> PathBuf {
    if format == "html-game" {
        let file = game.join("story.html");
        std::fs::write(&file, format!("<p>{source}</p>")).unwrap();
        file
    } else if format == "unity" {
        std::fs::create_dir_all(game.join("NeutralGame_Data")).unwrap();
        std::fs::write(
            game.join("UnityPlayer.dll"),
            b"neutral test marker, not executable",
        )
        .unwrap();
        let file = game.join("NeutralGame_Data/sharedassets0.assets");
        std::fs::write(&file, textasset(source)).unwrap();
        file
    } else {
        std::fs::create_dir_all(game.join("data")).unwrap();
        std::fs::write(game.join("data/System.json"), "{}").unwrap();
        let file = game.join("data/Actors.json");
        std::fs::write(
            &file,
            serde_json::to_vec(&serde_json::json!([null,{"id":1,"name":source}])).unwrap(),
        )
        .unwrap();
        file
    }
}

// Independently assembled minimal v17 serialized TextAsset, following the same
// public file layout exercised by formats::unity_serialized's fixture writer.
// One class49 object, no type tree, UTF-8 name+script, all offsets recomputed.
fn textasset(script: &str) -> Vec<u8> {
    let mut payload = Vec::new();
    for text in ["Dialog", script] {
        payload.extend_from_slice(&(text.len() as u32).to_le_bytes());
        payload.extend_from_slice(text.as_bytes());
        while !payload.len().is_multiple_of(4) {
            payload.push(0);
        }
    }
    let mut meta = b"2019.4.0f1\0".to_vec();
    meta.extend_from_slice(&1u32.to_le_bytes());
    meta.push(0);
    meta.extend_from_slice(&1i32.to_le_bytes());
    meta.extend_from_slice(&49i32.to_le_bytes());
    meta.push(0);
    meta.extend_from_slice(&(-1i16).to_le_bytes());
    meta.extend_from_slice(&[0; 16]);
    meta.extend_from_slice(&1i32.to_le_bytes());
    while !meta.len().is_multiple_of(4) {
        meta.push(0);
    }
    meta.extend_from_slice(&1i64.to_le_bytes());
    meta.extend_from_slice(&0u32.to_le_bytes());
    meta.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    meta.extend_from_slice(&0i32.to_le_bytes());
    let offset = (20 + meta.len() + 15) & !15;
    let mut file = Vec::new();
    for value in [
        meta.len() as u32,
        (offset + payload.len()) as u32,
        17,
        offset as u32,
    ] {
        file.extend_from_slice(&value.to_be_bytes());
    }
    file.extend_from_slice(&[0; 4]);
    file.extend_from_slice(&meta);
    file.resize(offset, 0);
    file.extend_from_slice(&payload);
    file
}

async fn reject_obsolete_translation(format: &str) {
    let temp = tempfile::tempdir().unwrap();
    let game = temp.path().join("NeutralGame");
    std::fs::create_dir(&game).unwrap();
    let file = write_source(&game, format, "村へ行く");
    let registry = Arc::new(locust_formats::default_registry());
    let db = Arc::new(Database::open_in_memory().unwrap());
    open_project(&db, &registry, &game, Some(format)).unwrap();
    let first = db.get_entries(&EntryFilter::default()).unwrap();
    assert_eq!(first.len(), 1);
    let id = first[0].id.clone();
    db.save_translation(&id, "Go to village", "manual")
        .await
        .unwrap();
    db.update_entry_status(&id, StringStatus::Approved)
        .await
        .unwrap();
    write_source(&game, format, "海へ行く");
    let physical = std::fs::read(&file).unwrap();
    let reopened = open_project(&db, &registry, &game, Some(format)).unwrap();
    assert_eq!(reopened.stale_source_reset, 1);
    let row = db.get_entry(&id).unwrap().unwrap();
    assert_eq!(row.source, "海へ行く");
    assert_eq!(row.translation.as_deref(), Some("Go to village"));
    assert_eq!(row.status, StringStatus::Pending);
    let backups = Arc::new(BackupManager::new(temp.path().join("backups")));
    let result = inject_direct(&registry, &db, &backups, &game, format, &["en".into()]);
    assert!(
        result.is_err(),
        "obsolete village translation was accepted for the new sea source: {}",
        String::from_utf8_lossy(&std::fs::read(&file).unwrap())
    );
    assert_eq!(std::fs::read(&file).unwrap(), physical);
    assert!(
        backups.list_backups().unwrap().is_empty(),
        "stale preflight must precede backup"
    );
    let pivot = temp.path().join("pivot.db");
    assert!(db.pivot_to(&pivot).is_err());
    assert!(!pivot.exists());
    let output = temp.path().join("translated");
    let injector = MultiLangInjector::new(registry.clone(), db.clone(), backups.clone());
    for mode in [OutputMode::Replace, OutputMode::Add] {
        let (tx, _rx) = tokio::sync::mpsc::channel(20);
        let error = injector
            .inject(
                &game,
                format,
                mode,
                vec!["en".into()],
                Some(output.clone()),
                tx,
            )
            .await
            .unwrap_err();
        assert!(error.to_string().contains("stale translation"));
        assert!(!output.exists());
        assert!(backups.list_backups().unwrap().is_empty());
        assert_eq!(std::fs::read(&file).unwrap(), physical);
    }
    db.save_translation(&id, "Go to sea", "manual")
        .await
        .unwrap();
    let report = inject_direct(&registry, &db, &backups, &game, format, &["en".into()]).unwrap();
    assert_eq!(report.strings_written, 1);
    let after = registry.get(format).unwrap().extract(&game).unwrap();
    assert!(after.iter().any(|entry| entry.source == "Go to sea"));
}

#[tokio::test]
async fn html_reopen_never_injects_translation_for_previous_source() {
    reject_obsolete_translation("html-game").await;
}

#[tokio::test]
async fn rpgmaker_reopen_never_injects_translation_for_previous_source() {
    reject_obsolete_translation("rpgmaker-mv").await;
}

#[tokio::test]
async fn unity_textasset_reopen_never_injects_translation_for_previous_source() {
    reject_obsolete_translation("unity").await;
}
