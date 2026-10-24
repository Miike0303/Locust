use axum::extract::{Path as AxumPath, Query, State};
use axum::http::StatusCode;
use axum::Json;
use locust_core::database::{
    EntryFilter, FileStats, ProjectStats, StringFacets, StringRevision, TranslationRun,
};
use locust_core::models::{StringEntry, StringStatus};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::{err, run_owned_project_operation, try_project_operation, ApiError, AppState};

#[derive(Deserialize)]
pub(crate) struct StringsQuery {
    status: Option<String>,
    file_path: Option<String>,
    tag: Option<String>,
    search: Option<String>,
    limit: Option<usize>,
    offset: Option<usize>,
}

#[derive(Serialize, Deserialize)]
pub(crate) struct StringsResponse {
    entries: Vec<StringEntry>,
    total: usize,
    offset: usize,
    limit: usize,
}

pub(crate) async fn get_strings(
    State(state): State<Arc<AppState>>,
    Query(q): Query<StringsQuery>,
) -> Result<Json<StringsResponse>, ApiError> {
    // Without an open project, never surface leftover rows from project.db.
    if state.current_project.read().await.is_none() {
        let limit = q.limit.unwrap_or(100);
        let offset = q.offset.unwrap_or(0);
        return Ok(Json(StringsResponse {
            entries: vec![],
            total: 0,
            offset,
            limit,
        }));
    }

    let status = q.status.and_then(|s| s.parse::<StringStatus>().ok());
    let limit = q.limit.unwrap_or(100);
    let offset = q.offset.unwrap_or(0);

    let filter = EntryFilter {
        status,
        file_path: q.file_path,
        tag: q.tag,
        search: q.search,
        limit: Some(limit),
        offset: Some(offset),
    };
    let (entries, total) = state
        .db
        .get_entries_page(&filter)
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e))?;

    Ok(Json(StringsResponse {
        entries,
        total,
        offset,
        limit,
    }))
}

pub(crate) async fn get_string_facets(
    State(state): State<Arc<AppState>>,
) -> Result<Json<StringFacets>, ApiError> {
    // Same convention as GET /api/strings: leftover rows must not leak.
    if state.current_project.read().await.is_none() {
        return Ok(Json(StringFacets::default()));
    }
    state
        .db
        .get_string_facets()
        .map(Json)
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e))
}

pub(crate) async fn get_string(
    State(state): State<Arc<AppState>>,
    AxumPath(id): AxumPath<String>,
) -> Result<Json<StringEntry>, ApiError> {
    if state.current_project.read().await.is_none() {
        return Err(err(StatusCode::NOT_FOUND, "entry not found"));
    }
    state
        .db
        .get_entry(&id)
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e))?
        .map(Json)
        .ok_or_else(|| err(StatusCode::NOT_FOUND, "entry not found"))
}

pub(crate) async fn get_string_history(
    State(state): State<Arc<AppState>>,
    AxumPath(id): AxumPath<String>,
) -> Result<Json<Vec<StringRevision>>, ApiError> {
    if state.current_project.read().await.is_none() {
        return Err(err(StatusCode::NOT_FOUND, "entry not found"));
    }
    state
        .db
        .get_entry(&id)
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e))?
        .ok_or_else(|| err(StatusCode::NOT_FOUND, "entry not found"))?;
    state
        .db
        .get_string_history(&id, 50)
        .map(Json)
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e))
}

#[derive(Deserialize)]
pub(crate) struct PatchStringRequest {
    translation: Option<String>,
    status: Option<StringStatus>,
    #[serde(
        default,
        deserialize_with = "locust_core::database::deserialize_expected_translation"
    )]
    expected_translation: Option<Option<String>>,
}

pub(crate) async fn patch_string(
    State(state): State<Arc<AppState>>,
    AxumPath(id): AxumPath<String>,
    Json(req): Json<PatchStringRequest>,
) -> Result<Json<StringEntry>, ApiError> {
    let guard = try_project_operation(&state).map_err(|m| err(StatusCode::CONFLICT, m))?;
    run_owned_project_operation(
        guard,
        async move { patch_string_owned(state, id, req).await },
    )
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e))?
}

async fn patch_string_owned(
    state: Arc<AppState>,
    id: String,
    req: PatchStringRequest,
) -> Result<Json<StringEntry>, ApiError> {
    if state.current_project.read().await.is_none() {
        return Err(err(StatusCode::BAD_REQUEST, "no project open"));
    }
    if let Some(ref translation) = req.translation {
        let outcome = state
            .db
            .save_translation_and_status_if_unchanged(
                &id,
                translation,
                "manual",
                req.status,
                req.expected_translation,
            )
            .await
            .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e))?;
        if outcome == locust_core::database::TranslationSaveOutcome::Conflict {
            return Err(err(
                StatusCode::CONFLICT,
                locust_core::database::TRANSLATION_CONFLICT_MESSAGE,
            ));
        }
    } else if let Some(ref status) = req.status {
        state
            .db
            .update_entry_status(&id, status.clone())
            .await
            .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e))?;
    }
    state
        .db
        .get_entry(&id)
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e))?
        .map(Json)
        .ok_or_else(|| err(StatusCode::NOT_FOUND, "entry not found"))
}

type BatchPatchItem = locust_core::database::TranslationBatchItem;

#[derive(Deserialize)]
pub(crate) struct BatchPatchRequest {
    updates: Vec<BatchPatchItem>,
    #[serde(default = "default_batch_provider")]
    provider: String,
}

fn default_batch_provider() -> String {
    "manual".into()
}

pub(crate) async fn batch_patch_strings(
    State(state): State<Arc<AppState>>,
    Json(req): Json<BatchPatchRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let guard = try_project_operation(&state).map_err(|m| err(StatusCode::CONFLICT, m))?;
    run_owned_project_operation(
        guard,
        async move { batch_patch_strings_owned(state, req).await },
    )
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e))?
}

async fn batch_patch_strings_owned(
    state: Arc<AppState>,
    req: BatchPatchRequest,
) -> Result<Json<serde_json::Value>, ApiError> {
    if state.current_project.read().await.is_none() {
        return Err(err(StatusCode::BAD_REQUEST, "no project open"));
    }
    if req.updates.len() > 50_000 {
        return Err(err(
            StatusCode::BAD_REQUEST,
            "batch too large (max 50000 updates)",
        ));
    }
    let report = state
        .db
        .save_translations_batch_if_unchanged(req.updates, &req.provider)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e))?;
    Ok(Json(serde_json::json!(report)))
}

pub(crate) async fn get_stats(
    State(state): State<Arc<AppState>>,
) -> Result<Json<ProjectStats>, ApiError> {
    if state.current_project.read().await.is_none() {
        return Ok(Json(ProjectStats::default()));
    }
    state
        .db
        .get_stats()
        .map(Json)
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e))
}

#[derive(Serialize)]
pub(crate) struct FileStatsResponse {
    files: Vec<FileStats>,
}

pub(crate) async fn get_file_stats(
    State(state): State<Arc<AppState>>,
) -> Result<Json<FileStatsResponse>, ApiError> {
    let files = if state.current_project.read().await.is_none() {
        Vec::new()
    } else {
        state
            .db
            .get_file_stats()
            .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e))?
    };
    Ok(Json(FileStatsResponse { files }))
}

/// Translation run ledger (same rows as CLI `locust stats`), newest first.
pub(crate) async fn list_translation_runs(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<TranslationRun>>, ApiError> {
    if state.current_project.read().await.is_none() {
        return Err(err(StatusCode::BAD_REQUEST, "no project open"));
    }
    let mut runs = state
        .db
        .get_translation_runs()
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e))?;
    runs.reverse();
    Ok(Json(runs))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::routes::test_support::*;
    use std::path::PathBuf;

    #[tokio::test]
    async fn string_history_returns_newest_first_and_empty_for_unchanged_entry() {
        let (url, _h, state) = setup_with_state().await;
        mark_project_open(&state).await;
        state
            .db
            .save_entries(&[
                StringEntry::new("history", "Hello", PathBuf::from("f.json")),
                StringEntry::new("empty", "World", PathBuf::from("f.json")),
            ])
            .unwrap();
        for text in ["Hola", "Saludos"] {
            state
                .db
                .save_translation("history", text, "manual")
                .await
                .unwrap();
        }
        let response = client()
            .get(format!("{url}/api/strings/history/history"))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        let body: serde_json::Value = response.json().await.unwrap();
        let rows = body.as_array().unwrap();
        assert_eq!(rows.len(), 2);
        assert!(rows[0]["id"].as_i64().unwrap() > rows[1]["id"].as_i64().unwrap());
        assert_eq!(rows[0]["new_translation"], "Saludos");
        assert_eq!(rows[0]["old_translation"], "Hola");
        assert_eq!(rows[0]["provider"], "manual");
        assert_eq!(rows[1]["old_translation"], serde_json::Value::Null);
        assert_eq!(rows[1]["old_status"], "pending");
        assert_eq!(rows[1]["new_status"], "translated");
        let response = client()
            .get(format!("{url}/api/strings/empty/history"))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        assert_eq!(
            response.json::<serde_json::Value>().await.unwrap(),
            serde_json::json!([])
        );
    }

    #[tokio::test]
    async fn string_history_unknown_entry_is_not_found() {
        let (url, _h, state) = setup_with_state().await;
        mark_project_open(&state).await;
        let response = client()
            .get(format!("{url}/api/strings/missing/history"))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 404);
        assert_eq!(response.text().await.unwrap(), "entry not found");
    }

    #[tokio::test]
    async fn string_history_requires_an_open_project() {
        let (url, _h, state) = setup_with_state().await;
        state
            .db
            .save_entries(&[StringEntry::new(
                "leftover",
                "Hello",
                PathBuf::from("f.json"),
            )])
            .unwrap();
        state
            .db
            .save_translation("leftover", "Hola", "manual")
            .await
            .unwrap();
        let response = client()
            .get(format!("{url}/api/strings/leftover/history"))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 404);
        assert_eq!(response.text().await.unwrap(), "entry not found");
    }

    #[tokio::test]
    async fn test_get_strings_before_project_returns_empty() {
        let (url, _h, state) = setup_with_state().await;
        // Leftover rows in project.db must not leak without an open project.
        let leftover = StringEntry::new("stale", "Old session", PathBuf::from("old.json"));
        state.db.save_entries(&[leftover]).unwrap();

        let resp = client()
            .get(format!("{}/api/strings", url))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
        let body: StringsResponse = resp.json().await.unwrap();
        assert!(body.entries.is_empty());
        assert_eq!(body.total, 0);

        let stats = client()
            .get(format!("{}/api/stats", url))
            .send()
            .await
            .unwrap();
        assert_eq!(stats.status(), 200);
        let stats_body: ProjectStats = stats.json().await.unwrap();
        assert_eq!(stats_body.total, 0);
    }

    #[tokio::test]
    async fn test_get_strings_search_out_of_range_keeps_total() {
        let (url, _h, state) = setup_with_state().await;
        mark_project_open(&state).await;
        let source_match = StringEntry::new("a", "needle in source", PathBuf::from("menu.json"));
        let mut translation_match =
            StringEntry::new("b", "Other source", PathBuf::from("menu.json"));
        translation_match.translation = Some("needle in translation".into());
        let unrelated = StringEntry::new("c", "Unrelated", PathBuf::from("menu.json"));
        state
            .db
            .save_entries(&[source_match, translation_match, unrelated])
            .unwrap();

        for offset in [0, 2, 9] {
            let resp = client()
                .get(format!(
                    "{url}/api/strings?search=needle&limit=1&offset={offset}"
                ))
                .send()
                .await
                .unwrap();
            assert_eq!(resp.status(), 200);
            let body: StringsResponse = resp.json().await.unwrap();
            assert_eq!(body.total, 2);
            assert_eq!(body.offset, offset);
            assert_eq!(body.limit, 1);
            if offset == 0 {
                assert_eq!(body.entries.len(), 1);
                assert_eq!(body.entries[0].id, "a");
            } else {
                assert!(body.entries.is_empty());
            }
        }
    }

    #[tokio::test]
    async fn test_string_facets_before_project_returns_empty() {
        let (url, _h, state) = setup_with_state().await;
        let leftover = StringEntry::new("stale", "Old session", PathBuf::from("old.json"))
            .with_tags(vec!["dialogue".into()]);
        state.db.save_entries(&[leftover]).unwrap();

        let resp = client()
            .get(format!("{}/api/strings/facets", url))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
        let body: serde_json::Value = resp.json().await.unwrap();
        assert_eq!(body["file_paths"], serde_json::json!([]));
        assert_eq!(body["tags"], serde_json::json!([]));
    }

    #[tokio::test]
    async fn test_string_facets_are_project_wide_not_the_current_page() {
        let (url, _h, state) = setup_with_state().await;
        mark_project_open(&state).await;

        let mut entries = Vec::new();
        for i in 0..120 {
            let file = match i % 3 {
                0 => "data/Map001.json",
                1 => "data/Actors.json",
                _ => "data/Map002.json",
            };
            let mut e = StringEntry::new(format!("e{i}"), format!("S{i}"), PathBuf::from(file));
            e.tags = match i % 3 {
                0 => vec!["dialogue".into(), "ui_label".into()],
                1 => vec!["ui_label".into()],
                _ => vec!["dialogue".into()],
            };
            entries.push(e);
        }
        let mut extra = StringEntry::new("zz", "Last", PathBuf::from("data/System.json"));
        extra.tags = vec!["system".into()];
        entries.push(extra);
        state.db.save_entries(&entries).unwrap();

        let resp = client()
            .get(format!("{}/api/strings/facets", url))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 200, "{}", resp.status());
        let body: serde_json::Value = resp.json().await.unwrap();
        assert_eq!(
            body["file_paths"],
            serde_json::json!([
                "data/Actors.json",
                "data/Map001.json",
                "data/Map002.json",
                "data/System.json"
            ])
        );
        assert_eq!(
            body["tags"],
            serde_json::json!(["dialogue", "system", "ui_label"])
        );
    }

    #[tokio::test]
    async fn string_detail_and_edits_require_an_open_project() {
        let (url, _h, state) = setup_with_state().await;
        state
            .db
            .save_entries(&[translated("leftover", "Hello", "f.json", "Hola")])
            .unwrap();

        let detail = client()
            .get(format!("{url}/api/strings/leftover"))
            .send()
            .await
            .unwrap();
        assert_eq!(detail.status(), 404);

        let patch = client()
            .patch(format!("{url}/api/strings/leftover"))
            .json(&serde_json::json!({"translation": "Wrong project"}))
            .send()
            .await
            .unwrap();
        assert_eq!(patch.status(), 400);
        assert_eq!(patch.text().await.unwrap(), "no project open");

        let batch = client()
            .post(format!("{url}/api/strings/batch"))
            .json(&serde_json::json!({"updates": [{"id": "leftover", "translation": "Wrong project"}]}))
            .send()
            .await
            .unwrap();
        assert_eq!(batch.status(), 400);
        assert_eq!(batch.text().await.unwrap(), "no project open");
        assert_eq!(
            state
                .db
                .get_entry("leftover")
                .unwrap()
                .unwrap()
                .translation
                .as_deref(),
            Some("Hola")
        );
    }

    #[tokio::test]
    async fn strings_patch_translation_and_status_records_one_final_revision() {
        let (url, _h, state) = setup_with_state().await;
        mark_project_open(&state).await;
        state
            .db
            .save_entries(&[StringEntry::new("atomic", "Hello", PathBuf::from("f.json"))])
            .unwrap();

        let response = client()
            .patch(format!("{url}/api/strings/atomic"))
            .json(&serde_json::json!({
                "translation": "Hola", "status": "approved", "expected_translation": null
            }))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        let saved: StringEntry = response.json().await.unwrap();
        assert_eq!(saved.translation.as_deref(), Some("Hola"));
        assert_eq!(saved.status, StringStatus::Approved);
        assert_eq!(saved.provider_used.as_deref(), Some("manual"));
        assert!(saved.translated_at.is_some());
        assert!(saved.reviewed_at.is_some());
        let history = state.db.get_string_history("atomic", 50).unwrap();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].old_status.as_deref(), Some("pending"));
        assert_eq!(history[0].new_status.as_deref(), Some("approved"));
        assert_eq!(history[0].new_translation.as_deref(), Some("Hola"));

        let before = serde_json::to_value(&saved).unwrap();
        let response = client()
            .patch(format!("{url}/api/strings/atomic"))
            .json(&serde_json::json!({
                "translation": "Stale", "status": "pending", "expected_translation": null
            }))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 409);
        assert_eq!(
            response.text().await.unwrap(),
            locust_core::database::TRANSLATION_CONFLICT_MESSAGE
        );
        assert_eq!(
            serde_json::to_value(state.db.get_entry("atomic").unwrap().unwrap()).unwrap(),
            before
        );
        assert_eq!(
            serde_json::to_value(state.db.get_string_history("atomic", 50).unwrap()).unwrap(),
            serde_json::to_value(history).unwrap()
        );
    }

    #[tokio::test]
    async fn test_patch_string_updates_translation() {
        let (url, _h, state) = setup_with_state().await;
        mark_project_open(&state).await;
        let entry = StringEntry::new("test1", "Hello", PathBuf::from("f.json"));
        state.db.save_entries(&[entry]).unwrap();

        // Verify entry exists first
        let resp = client()
            .get(format!("{}/api/strings/test1", url))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 200, "entry should exist before patch");

        let resp = client()
            .patch(format!("{}/api/strings/test1", url))
            .json(&serde_json::json!({"translation": "Hola"}))
            .send()
            .await
            .unwrap();
        let status = resp.status().as_u16();
        let body: serde_json::Value = resp.json().await.unwrap();
        assert_eq!(status, 200, "patch failed: {:?}", body);
        assert_eq!(body["translation"], "Hola");
    }

    #[tokio::test]
    async fn test_patch_string_updates_status() {
        let (url, _h, state) = setup_with_state().await;
        mark_project_open(&state).await;
        let entry = StringEntry::new("test2", "Hello", PathBuf::from("f.json"));
        state.db.save_entries(&[entry]).unwrap();

        let resp = client()
            .patch(format!("{}/api/strings/test2", url))
            .json(&serde_json::json!({"status": "approved"}))
            .send()
            .await
            .unwrap();
        let status = resp.status().as_u16();
        let body: serde_json::Value = resp.json().await.unwrap();
        assert_eq!(status, 200, "patch failed: {:?}", body);
        assert_eq!(body["status"], "approved");
    }

    #[tokio::test]
    async fn test_get_stats_shape() {
        let (url, _h) = setup().await;
        let resp = client()
            .get(format!("{}/api/stats", url))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
        let body: serde_json::Value = resp.json().await.unwrap();
        assert!(body.get("total").is_some());
        assert!(body.get("pending").is_some());
        assert!(body.get("translated").is_some());
    }

    #[tokio::test]
    async fn file_stats_counts_statuses_in_file_order() {
        let (url, _h, state) = setup_with_state().await;
        mark_project_open(&state).await;
        let entries: Vec<_> = [
            (
                "b-translated",
                "b.rpy",
                StringStatus::Translated,
                Some("Hola"),
            ),
            ("b-error", "b.rpy", StringStatus::Error, None),
            (
                "a-pending",
                "a.rpy",
                StringStatus::Pending,
                Some("Stale text"),
            ),
            (
                "a-approved",
                "a.rpy",
                StringStatus::Approved,
                Some("Aprobado"),
            ),
        ]
        .into_iter()
        .map(|(id, file, status, translation)| {
            let mut entry = StringEntry::new(id, id, file.into());
            entry.status = status;
            entry.translation = translation.map(str::to_owned);
            entry
        })
        .collect();
        state.db.save_entries(&entries).unwrap();
        assert!(state.db.get_translation_runs().unwrap().is_empty());

        let response = client()
            .get(format!("{url}/api/stats/files"))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        assert_eq!(
            response.json::<serde_json::Value>().await.unwrap(),
            serde_json::json!({"files": [
                {"file_path": "a.rpy", "total": 2, "pending": 1, "translated": 0,
                 "reviewed": 0, "approved": 1, "error": 0},
                {"file_path": "b.rpy", "total": 2, "pending": 0, "translated": 1,
                 "reviewed": 0, "approved": 0, "error": 1}
            ]})
        );
    }

    #[tokio::test]
    async fn file_stats_without_project_matches_project_stats() {
        let (url, _h, state) = setup_with_state().await;
        // Persisted rows must stay hidden until a project is open.
        state
            .db
            .save_entries(&[StringEntry::new("old", "Old session", "old.rpy".into())])
            .unwrap();
        let stats = client()
            .get(format!("{url}/api/stats"))
            .send()
            .await
            .unwrap();
        let files = client()
            .get(format!("{url}/api/stats/files"))
            .send()
            .await
            .unwrap();
        assert_eq!(stats.status(), 200);
        assert_eq!(files.status(), stats.status());
        assert_eq!(stats.json::<ProjectStats>().await.unwrap().total, 0);
        assert_eq!(
            files.json::<serde_json::Value>().await.unwrap(),
            serde_json::json!({"files": []})
        );
    }

    #[tokio::test]
    async fn file_stats_empty_project_returns_empty_files() {
        let (url, _h, state) = setup_with_state().await;
        mark_project_open(&state).await;
        let response = client()
            .get(format!("{url}/api/stats/files"))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        assert_eq!(
            response.json::<serde_json::Value>().await.unwrap(),
            serde_json::json!({"files": []})
        );
    }

    #[tokio::test]
    async fn test_list_translation_runs_seeded() {
        use locust_core::database::TranslationRun;

        let (url, _h, state) = setup_with_state().await;
        state
            .db
            .record_translation_run(&TranslationRun {
                id: 0,
                started_at: "2026-03-15T12:34:56Z".into(),
                duration_secs: 42.5,
                provider: "mock→deepl".into(),
                source_lang: "en".into(),
                target_lang: "es".into(),
                strings_translated: 12,
                tokens_used: 100,
                input_tokens: 60,
                output_tokens: 40,
                cost_usd: 0.0012,
                cost_is_complete: true,
            })
            .await
            .unwrap();
        state
            .db
            .record_translation_run(&TranslationRun {
                id: 0,
                started_at: "2026-03-16T08:00:00Z".into(),
                duration_secs: 10.0,
                provider: "mock".into(),
                source_lang: "en".into(),
                target_lang: "fr".into(),
                strings_translated: 3,
                tokens_used: 20,
                input_tokens: 10,
                output_tokens: 10,
                cost_usd: 0.0001,
                cost_is_complete: true,
            })
            .await
            .unwrap();

        let refused = client()
            .get(format!("{}/api/runs", url))
            .send()
            .await
            .unwrap();
        assert_eq!(refused.status(), 400);
        assert_eq!(refused.text().await.unwrap(), "no project open");
        assert_eq!(state.db.get_translation_runs().unwrap().len(), 2);
        mark_project_open(&state).await;

        let resp = client()
            .get(format!("{}/api/runs", url))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
        let body: Vec<serde_json::Value> = resp.json().await.unwrap();
        assert_eq!(body.len(), 2, "{body:?}");
        // Newest first
        assert_eq!(body[0]["started_at"], "2026-03-16T08:00:00Z");
        assert_eq!(body[0]["provider"], "mock");
        assert_eq!(body[0]["target_lang"], "fr");
        assert_eq!(body[1]["provider"], "mock→deepl");
        assert_eq!(body[1]["strings_translated"], 12);
        assert_eq!(body[1]["input_tokens"], 60);
        assert_eq!(body[1]["output_tokens"], 40);
        assert!((body[1]["cost_usd"].as_f64().unwrap() - 0.0012).abs() < 1e-9);
        assert!((body[1]["duration_secs"].as_f64().unwrap() - 42.5).abs() < 1e-9);
        assert!(body[0]["id"].as_i64().unwrap() >= 1);
        // All ledger columns present
        for key in [
            "id",
            "started_at",
            "duration_secs",
            "provider",
            "source_lang",
            "target_lang",
            "strings_translated",
            "tokens_used",
            "input_tokens",
            "output_tokens",
            "cost_usd",
            "cost_is_complete",
        ] {
            assert!(body[0].get(key).is_some(), "missing {key}");
        }
    }
}
