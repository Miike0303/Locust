use axum::extract::{Path as AxumPath, Query, State};
use axum::http::StatusCode;
use axum::Json;
use locust_core::backup::BackupEntry;
use serde::Deserialize;
use std::sync::Arc;

use crate::{err, try_project_operation, ApiError, AppState};

#[derive(Default, Deserialize)]
pub(crate) struct BackupListQuery {
    #[serde(default)]
    report: bool,
}

pub(crate) async fn list_backups(
    State(state): State<Arc<AppState>>,
    Query(params): Query<BackupListQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let manager = state.backup_manager.clone();
    tokio::task::spawn_blocking(move || {
        if params.report {
            let listing = manager
                .list_backups_report()
                .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e))?;
            let unreadable: Vec<_> = listing
                .unreadable
                .into_iter()
                .map(|(id, error)| serde_json::json!({"id": id, "error": error}))
                .collect();
            Ok(serde_json::json!({"entries": listing.entries, "unreadable": unreadable}))
        } else {
            manager
                .list_backups()
                .map(|entries: Vec<BackupEntry>| serde_json::json!(entries))
                .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e))
        }
    })
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e))?
    .map(Json)
}

pub(crate) async fn restore_backup(
    State(state): State<Arc<AppState>>,
    AxumPath(id): AxumPath<String>,
) -> Result<Json<locust_core::backup::RestorePreview>, ApiError> {
    let exclusive = try_project_operation(&state).map_err(|e| err(StatusCode::CONFLICT, e))?;
    let manager = state.backup_manager.clone();
    // The blocking task owns exclusion even if its HTTP caller disconnects.
    let report = tokio::task::spawn_blocking(move || {
        let _exclusive = exclusive;
        manager.restore_with_report(&id)
    })
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e))?
    .map_err(|e| {
        let msg = e.to_string();
        // Missing origin or unreadable manifest: the user can re-open the
        // original game folder or discard the backup. Anything else is ours.
        let status = if matches!(&e, locust_core::LocustError::InjectionError(_))
            || msg.starts_with("patch error: game busy:")
            || msg.starts_with("game busy:")
        {
            StatusCode::CONFLICT
        } else if msg.contains("manifest.json") || msg.contains("no longer exists") {
            StatusCode::BAD_REQUEST
        } else {
            StatusCode::INTERNAL_SERVER_ERROR
        };
        err(status, e)
    })?;
    Ok(Json(report))
}

pub(crate) async fn delete_backup(
    State(state): State<Arc<AppState>>,
    AxumPath(id): AxumPath<String>,
) -> Result<StatusCode, ApiError> {
    let exclusive = try_project_operation(&state).map_err(|e| err(StatusCode::CONFLICT, e))?;
    let manager = state.backup_manager.clone();
    tokio::task::spawn_blocking(move || {
        let _exclusive = exclusive;
        // Only the open project's recordings are known here; recordings in
        // other project databases cannot be checked from this handler.
        for (lang, backup) in state
            .db
            .recorded_backup_refs()
            .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e))?
        {
            if backup.id == id
                && backup.storage_root.as_deref().is_none_or(|root| {
                    locust_core::database::paths_identical(root, manager.root())
                })
            {
                return Err(err(
                    StatusCode::CONFLICT,
                    format!(
                        "backup {id} is needed to pack the injection recorded for {}; inject again or open another project before deleting it",
                        lang.as_deref().unwrap_or("default")
                    ),
                ));
            }
        }
        manager
            .delete_backup(&id)
            .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e))
    })
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e))??;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::routes::test_support::*;
    use crate::*;

    #[tokio::test]
    async fn list_backups_returns_readable_backup_despite_damaged_manifest() {
        let state = create_test_state();
        let game = tempfile::tempdir().unwrap();
        std::fs::write(game.path().join("story.txt"), b"original").unwrap();
        let valid = state.backup_manager.create_backup(game.path()).unwrap();
        let damaged = valid.path.parent().unwrap().join("damaged");
        std::fs::create_dir(&damaged).unwrap();
        std::fs::write(damaged.join("manifest.json"), b"{").unwrap();

        let (url, _h) = start_test_server(state).await;
        let resp = client()
            .get(format!("{url}/api/backups"))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 200, "{}", resp.text().await.unwrap());
        let entries: Vec<BackupEntry> = resp.json().await.unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].id, valid.id);
    }

    fn game_temp(prefix: &str) -> tempfile::TempDir {
        tempfile::Builder::new()
            .prefix(&format!("locust_{prefix}_"))
            .tempdir()
            .unwrap()
    }

    #[tokio::test]
    async fn restore_writes_to_backup_origin_not_the_open_project() {
        let state = create_test_state();
        let game_a_guard = game_temp("restore_a");
        let game_a = game_a_guard.path().to_path_buf();
        let game_b_guard = game_temp("restore_b");
        let game_b = game_b_guard.path().to_path_buf();
        std::fs::write(game_a.join("data.json"), r#"{"hp": 100}"#).unwrap();
        std::fs::write(game_b.join("b_only.txt"), "keep-b").unwrap();

        let entry = state.backup_manager.create_backup(&game_a).unwrap();
        std::fs::write(game_a.join("data.json"), "A-MUTATED").unwrap();

        {
            let mut proj = state.current_project.write().await;
            *proj = Some(ProjectInfo {
                path: game_b.clone(),
                format_id: "rpgmaker-mv".into(),
                name: "game-b".into(),
                extraction_warnings: Vec::new(),
                ..Default::default()
            });
        }

        let (url, _h) = start_test_server(state).await;
        let resp = client()
            .post(format!("{}/api/backups/{}/restore", url, entry.id))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 200, "{}", resp.text().await.unwrap());

        assert_eq!(
            std::fs::read_to_string(game_a.join("data.json")).unwrap(),
            r#"{"hp": 100}"#
        );
        assert_eq!(
            std::fs::read_to_string(game_b.join("b_only.txt")).unwrap(),
            "keep-b"
        );
        assert!(
            !game_b.join("data.json").exists(),
            "open game B must not receive game A's restored files"
        );
    }

    #[tokio::test]
    async fn restore_does_not_require_an_open_project() {
        let state = create_test_state();
        let game_guard = game_temp("restore_no_proj");
        let game = game_guard.path().to_path_buf();
        std::fs::write(game.join("data.json"), r#"{"hp": 100}"#).unwrap();
        let entry = state.backup_manager.create_backup(&game).unwrap();
        std::fs::write(game.join("data.json"), "MUTATED").unwrap();

        let (url, _h) = start_test_server(state).await;
        let resp = client()
            .post(format!("{}/api/backups/{}/restore", url, entry.id))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 200, "{}", resp.text().await.unwrap());
        assert_eq!(
            std::fs::read_to_string(game.join("data.json")).unwrap(),
            r#"{"hp": 100}"#
        );
    }

    #[tokio::test]
    async fn restore_missing_source_directory_is_4xx() {
        let state = create_test_state();
        let game_guard = game_temp("restore_gone");
        let game = game_guard.path().to_path_buf();
        std::fs::write(game.join("data.json"), r#"{"hp": 100}"#).unwrap();
        let entry = state.backup_manager.create_backup(&game).unwrap();
        std::fs::remove_dir_all(&game).unwrap();

        let (url, _h) = start_test_server(state).await;
        let resp = client()
            .post(format!("{}/api/backups/{}/restore", url, entry.id))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status().as_u16() / 100, 4, "{}", resp.status());
        let body = resp.text().await.unwrap();
        assert!(body.contains(&entry.id), "{body}");
        assert!(body.contains("no longer exists"), "{body}");
        assert!(
            !game.exists(),
            "must not recreate the deleted game directory"
        );
    }

    #[tokio::test]
    async fn restore_missing_or_corrupt_manifest_is_4xx() {
        let state = create_test_state();
        let game_guard = game_temp("restore_manifest");
        let game = game_guard.path().to_path_buf();
        std::fs::write(game.join("data.json"), r#"{"hp": 100}"#).unwrap();

        let missing = state.backup_manager.create_backup(&game).unwrap();
        std::fs::write(game.join("data.json"), "AFTER-MISSING").unwrap();
        std::fs::remove_file(missing.path.join("manifest.json")).unwrap();

        let corrupt = state.backup_manager.create_backup(&game).unwrap();
        std::fs::write(game.join("data.json"), "AFTER-CORRUPT").unwrap();
        std::fs::write(corrupt.path.join("manifest.json"), "not-json{").unwrap();

        let (url, _h) = start_test_server(state).await;

        let resp = client()
            .post(format!("{}/api/backups/{}/restore", url, missing.id))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status().as_u16() / 100, 4, "{}", resp.status());
        let body = resp.text().await.unwrap();
        assert!(body.contains(&missing.id), "{body}");
        assert!(body.contains("manifest.json"), "{body}");
        assert_eq!(
            std::fs::read_to_string(game.join("data.json")).unwrap(),
            "AFTER-CORRUPT"
        );

        let resp = client()
            .post(format!("{}/api/backups/{}/restore", url, corrupt.id))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status().as_u16() / 100, 4, "{}", resp.status());
        let body = resp.text().await.unwrap();
        assert!(body.contains(&corrupt.id), "{body}");
        assert!(body.contains("manifest.json"), "{body}");
        assert_eq!(
            std::fs::read_to_string(game.join("data.json")).unwrap(),
            "AFTER-CORRUPT"
        );
    }
}

#[cfg(test)]
#[path = "../backup_operation_tests.rs"]
mod backup_operation_tests;
