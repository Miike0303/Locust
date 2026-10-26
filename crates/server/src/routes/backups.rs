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

#[derive(Default, Deserialize)]
pub(crate) struct BackupDeleteQuery {
    #[serde(default)]
    confirm_unverified: bool,
}

pub(crate) async fn delete_backup(
    State(state): State<Arc<AppState>>,
    AxumPath(id): AxumPath<String>,
    Query(params): Query<BackupDeleteQuery>,
    body: Option<Json<BackupDeleteQuery>>,
) -> Result<StatusCode, ApiError> {
    let confirm_unverified =
        params.confirm_unverified || body.is_some_and(|Json(body)| body.confirm_unverified);
    let exclusive = try_project_operation(&state).map_err(|e| err(StatusCode::CONFLICT, e))?;
    let manager = state.backup_manager.clone();
    tokio::task::spawn_blocking(move || {
        let _exclusive = exclusive;
        // The open project's live references remain verifiable even when the
        // store index is unavailable. Confirmation must never bypass them.
        for (_, backup) in state
            .db
            .recorded_backup_refs()
            .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e))?
        {
            if backup.id == id
                && locust_core::database::paths_identical(&backup.resolved_storage_root(), manager.root())
            {
                return Err(err(
                    StatusCode::CONFLICT,
                    "backup error: backup is referenced by 1 project(s); release their references before deleting it",
                ));
            }
        }
        manager
            .delete_backup_with_confirmation(&id, confirm_unverified)
            .map_err(|e| {
                let status = if matches!(&e, locust_core::LocustError::BackupError(message) if message.starts_with("backup is referenced by ") || message == "Locust cannot confirm that no project still needs this backup") {
                    StatusCode::CONFLICT
                } else {
                    StatusCode::INTERNAL_SERVER_ERROR
                };
                err(status, e)
            })
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
    async fn backups_delete_unverified_requires_explicit_confirmation() {
        let temp = tempfile::tempdir().unwrap();
        let mut state = create_test_state_with_db(&temp.path().join("open.db"));
        Arc::get_mut(&mut state).unwrap().backup_manager = Arc::new(
            locust_core::backup::BackupManager::new(temp.path().join("backups")),
        );
        let game = temp.path().join("game");
        std::fs::create_dir(&game).unwrap();
        let backup = state.backup_manager.create_backup(&game).unwrap();
        std::fs::remove_file(
            state
                .backup_manager
                .root()
                .join(".backup-references.sqlite3"),
        )
        .unwrap();
        let (url, server) = start_test_server(state).await;
        for suffix in ["", "?confirm_unverified=false"] {
            let response = client()
                .delete(format!("{url}/api/backups/{}{suffix}", backup.id))
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::CONFLICT);
            assert!(response
                .text()
                .await
                .unwrap()
                .contains("Locust cannot confirm that no project still needs this backup"));
            assert!(backup.path.exists());
        }
        let response = client()
            .delete(format!(
                "{url}/api/backups/{}?confirm_unverified=true",
                backup.id
            ))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
        assert!(!backup.path.exists());
        server.abort();
    }

    #[tokio::test]
    async fn backups_delete_accepts_json_confirmation() {
        let temp = tempfile::tempdir().unwrap();
        let mut state = create_test_state_with_db(&temp.path().join("open.db"));
        Arc::get_mut(&mut state).unwrap().backup_manager = Arc::new(
            locust_core::backup::BackupManager::new(temp.path().join("backups")),
        );
        let game = temp.path().join("game");
        std::fs::create_dir(&game).unwrap();
        let backup = state.backup_manager.create_backup(&game).unwrap();
        std::fs::write(
            state
                .backup_manager
                .root()
                .join(".backup-references.sqlite3"),
            b"invalid index",
        )
        .unwrap();
        let (url, server) = start_test_server(state).await;
        for confirm in [false, true] {
            let response = client()
                .delete(format!("{url}/api/backups/{}", backup.id))
                .json(&serde_json::json!({"confirm_unverified": confirm}))
                .send()
                .await
                .unwrap();
            assert_eq!(
                response.status(),
                if confirm {
                    StatusCode::NO_CONTENT
                } else {
                    StatusCode::CONFLICT
                }
            );
        }
        assert!(!backup.path.exists());
        server.abort();
    }

    #[tokio::test]
    async fn backups_confirmation_cannot_override_open_project_with_corrupt_index() {
        let temp = tempfile::tempdir().unwrap();
        let mut state = create_test_state_with_db(&temp.path().join("open.db"));
        Arc::get_mut(&mut state).unwrap().backup_manager = Arc::new(
            locust_core::backup::BackupManager::new(temp.path().join("backups")),
        );
        let game = temp.path().join("game");
        std::fs::create_dir(&game).unwrap();
        let file = game.join("story.txt");
        std::fs::write(&file, b"original").unwrap();
        let backup = state.backup_manager.create_backup(&game).unwrap();
        state
            .db
            .record_injection_with_backup(
                Some("es"),
                &game,
                &[file],
                Some(&locust_core::database::RecordedBackup {
                    id: backup.id.clone(),
                    source_path: game.clone(),
                    storage_root: Some(state.backup_manager.root().to_owned()),
                }),
            )
            .unwrap();
        std::fs::write(
            state
                .backup_manager
                .root()
                .join(".backup-references.sqlite3"),
            b"invalid index",
        )
        .unwrap();
        let (url, server) = start_test_server(state).await;
        let response = client()
            .delete(format!(
                "{url}/api/backups/{}?confirm_unverified=true",
                backup.id
            ))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CONFLICT);
        assert!(response
            .text()
            .await
            .unwrap()
            .contains("referenced by 1 project(s)"));
        assert!(backup.path.exists());
        server.abort();
    }

    #[tokio::test]
    async fn backups_delete_refuses_other_project_after_server_restart() {
        let temp = tempfile::tempdir().unwrap();
        let game = temp.path().join("game-a");
        std::fs::create_dir(&game).unwrap();
        let file = game.join("story.txt");
        std::fs::write(&file, b"original").unwrap();
        let root = temp.path().join("backups");
        let entry = {
            let mut state = create_test_state_with_db(&temp.path().join("a.db"));
            Arc::get_mut(&mut state).unwrap().backup_manager =
                Arc::new(locust_core::backup::BackupManager::new(root.clone()));
            let entry = state.backup_manager.create_backup(&game).unwrap();
            state
                .db
                .record_injection_with_backup(
                    Some("es"),
                    &game,
                    &[file],
                    Some(&locust_core::database::RecordedBackup {
                        id: entry.id.clone(),
                        source_path: game.clone(),
                        storage_root: Some(root.clone()),
                    }),
                )
                .unwrap();
            entry
        };
        // Fresh process state uses the same persistent store, with B open or no project.
        for open_b in [true, false] {
            let mut state = create_test_state_with_db(&temp.path().join("b.db"));
            Arc::get_mut(&mut state).unwrap().backup_manager =
                Arc::new(locust_core::backup::BackupManager::new(root.clone()));
            if open_b {
                *state.current_project.write().await = Some(ProjectInfo {
                    path: temp.path().join("game-b"),
                    name: "game-b".into(),
                    ..Default::default()
                });
            }
            let (url, server) = start_test_server(state).await;
            let response = client()
                .delete(format!(
                    "{url}/api/backups/{}?confirm_unverified=true",
                    entry.id
                ))
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::CONFLICT);
            let body = response.text().await.unwrap();
            assert!(body.contains("referenced by 1 project(s)"), "{body}");
            assert!(entry.path.exists());
            server.abort();
        }
        std::fs::remove_file(temp.path().join("a.db")).unwrap();
        let mut state = create_test_state_with_db(&temp.path().join("b.db"));
        Arc::get_mut(&mut state).unwrap().backup_manager =
            Arc::new(locust_core::backup::BackupManager::new(root));
        assert_eq!(
            delete_backup(
                State(state),
                AxumPath(entry.id),
                Query(BackupDeleteQuery::default()),
                None,
            )
            .await
            .unwrap(),
            StatusCode::NO_CONTENT
        );
        assert!(!entry.path.exists());
    }

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
