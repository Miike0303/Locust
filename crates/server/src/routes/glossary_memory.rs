use axum::extract::{Path as AxumPath, Query, State};
use axum::http::StatusCode;
use axum::Json;
use locust_core::database::GlossaryEntry;
use serde::Deserialize;
use std::sync::Arc;

use crate::{err, run_owned_project_operation, try_project_operation, ApiError, AppState};

#[derive(Deserialize)]
pub(crate) struct GlossaryQuery {
    lang_pair: String,
}

pub(crate) async fn get_glossary(
    State(state): State<Arc<AppState>>,
    Query(q): Query<GlossaryQuery>,
) -> Result<Json<Vec<GlossaryEntry>>, ApiError> {
    if state.current_project.read().await.is_none() {
        return Err(err(StatusCode::BAD_REQUEST, "no project open"));
    }
    state
        .glossary
        .get_all(&q.lang_pair)
        .map(Json)
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e))
}

#[derive(Deserialize)]
pub(crate) struct AddGlossaryRequest {
    term: String,
    translation: String,
    lang_pair: String,
    context: Option<String>,
    #[serde(default)]
    case_sensitive: bool,
}

pub(crate) async fn add_glossary(
    State(state): State<Arc<AppState>>,
    Json(entry): Json<AddGlossaryRequest>,
) -> Result<StatusCode, ApiError> {
    if state.current_project.read().await.is_none() {
        return Err(err(StatusCode::BAD_REQUEST, "no project open"));
    }
    state
        .glossary
        .add_entry(&GlossaryEntry {
            term: entry.term,
            translation: entry.translation,
            lang_pair: entry.lang_pair,
            context: entry.context,
            case_sensitive: entry.case_sensitive,
        })
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e))?;
    Ok(StatusCode::CREATED)
}

pub(crate) async fn delete_glossary(
    State(state): State<Arc<AppState>>,
    AxumPath(term): AxumPath<String>,
    Query(q): Query<GlossaryQuery>,
) -> Result<StatusCode, ApiError> {
    if state.current_project.read().await.is_none() {
        return Err(err(StatusCode::BAD_REQUEST, "no project open"));
    }
    state
        .glossary
        .delete(&term, &q.lang_pair)
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e))?;
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn memory_stats(
    State(state): State<Arc<AppState>>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let project = state
        .db
        .memory_count()
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e))?;
    let global = state
        .global_memory
        .memory_count()
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e))?;
    Ok(Json(serde_json::json!({
        "project_entries": project,
        "global_entries": global,
    })))
}

pub(crate) async fn list_memory(
    State(state): State<Arc<AppState>>,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let _guard = try_project_operation(&state).map_err(|m| err(StatusCode::CONFLICT, m))?;
    if state.current_project.read().await.is_none() {
        return Err(err(StatusCode::BAD_REQUEST, "no project open"));
    }
    let search = params.get("search").map(|s| s.as_str());
    let lang_pair = params.get("lang_pair").map(|s| s.as_str());
    let limit: usize = params
        .get("limit")
        .and_then(|s| s.parse().ok())
        .unwrap_or(50);
    let offset: usize = params
        .get("offset")
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);

    let (entries, total) = state
        .db
        .list_memory(search, lang_pair, limit, offset)
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e))?;

    Ok(Json(serde_json::json!({
        "entries": entries,
        "total": total,
        "limit": limit,
        "offset": offset,
    })))
}

pub(crate) async fn delete_memory_entry(
    State(state): State<Arc<AppState>>,
    AxumPath((hash, lang_pair)): AxumPath<(String, String)>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let _guard = try_project_operation(&state).map_err(|m| err(StatusCode::CONFLICT, m))?;
    if state.current_project.read().await.is_none() {
        return Err(err(StatusCode::BAD_REQUEST, "no project open"));
    }
    state
        .db
        .delete_memory(&hash, &lang_pair)
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e))?;
    Ok(Json(serde_json::json!({"ok": true})))
}

pub(crate) async fn clear_memory(
    State(state): State<Arc<AppState>>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let guard = try_project_operation(&state).map_err(|m| err(StatusCode::CONFLICT, m))?;
    run_owned_project_operation(guard, async move { clear_memory_owned(state).await })
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e))?
}

async fn clear_memory_owned(state: Arc<AppState>) -> Result<Json<serde_json::Value>, ApiError> {
    if state.current_project.read().await.is_none() {
        return Err(err(StatusCode::BAD_REQUEST, "no project open"));
    }
    state
        .db
        .clear_memory()
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e))?;
    // Legacy global entries are cleared too, as before; nothing writes them now.
    state
        .global_memory
        .clear_memory()
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e))?;
    Ok(Json(serde_json::json!({"ok": true})))
}

pub(crate) async fn memory_lang_pairs(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<String>>, ApiError> {
    let _guard = try_project_operation(&state).map_err(|m| err(StatusCode::CONFLICT, m))?;
    if state.current_project.read().await.is_none() {
        return Err(err(StatusCode::BAD_REQUEST, "no project open"));
    }
    state
        .db
        .memory_lang_pairs()
        .map(Json)
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e))
}

#[cfg(test)]
mod tests {
    use crate::routes::test_support::*;

    #[tokio::test]
    async fn glossary_requires_an_open_project() {
        let (url, _h, state) = setup_with_state().await;
        state
            .db
            .save_entries(&[translated("leftover", "Hello", "story.html", "Hola")])
            .unwrap();
        state.glossary.add("HP", "PV", "en-es", None).unwrap();
        let before = serde_json::to_value(state.glossary.get_all("en-es").unwrap()).unwrap();
        let row_before = serde_json::to_value(state.db.get_entry("leftover").unwrap()).unwrap();

        let get = client()
            .get(format!("{url}/api/glossary?lang_pair=en-es"))
            .send()
            .await
            .unwrap();
        assert_eq!(get.status(), 400);
        assert_eq!(get.text().await.unwrap(), "no project open");

        let add = client()
            .post(format!("{url}/api/glossary"))
            .json(&serde_json::json!({
                "term": "MP", "translation": "PM", "lang_pair": "en-es",
                "context": null, "case_sensitive": false
            }))
            .send()
            .await
            .unwrap();
        assert_eq!(add.status(), 400);
        assert_eq!(add.text().await.unwrap(), "no project open");

        let delete = client()
            .delete(format!("{url}/api/glossary/HP?lang_pair=en-es"))
            .send()
            .await
            .unwrap();
        assert_eq!(delete.status(), 400);
        assert_eq!(delete.text().await.unwrap(), "no project open");
        assert_eq!(
            serde_json::to_value(state.glossary.get_all("en-es").unwrap()).unwrap(),
            before
        );
        assert_eq!(
            serde_json::to_value(state.db.get_entry("leftover").unwrap()).unwrap(),
            row_before
        );
    }

    #[tokio::test]
    async fn test_glossary_add_and_get() {
        let (url, _h, state) = setup_with_state().await;
        mark_project_open(&state).await;
        let resp = client()
            .post(format!("{}/api/glossary", url))
            .json(&serde_json::json!({
                "term": "HP",
                "translation": "PV",
                "lang_pair": "en-es",
                "context": null,
                "case_sensitive": false
            }))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 201);

        let resp = client()
            .get(format!("{}/api/glossary?lang_pair=en-es", url))
            .send()
            .await
            .unwrap();
        let body: Vec<serde_json::Value> = resp.json().await.unwrap();
        assert_eq!(body.len(), 1);
        assert_eq!(body[0]["term"], "HP");
        assert_eq!(body[0]["case_sensitive"], false);
        assert_eq!(
            state.glossary.lookup_exact("hp", "en", "es").as_deref(),
            Some("PV")
        );
    }

    #[tokio::test]
    async fn glossary_post_preserves_case_sensitive() {
        let (url, _h, state) = setup_with_state().await;
        mark_project_open(&state).await;
        let resp = client()
            .post(format!("{url}/api/glossary"))
            .json(&serde_json::json!({
                "term": "US",
                "translation": "EEUU",
                "lang_pair": "en-es",
                "context": null,
                "case_sensitive": true
            }))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 201);

        let resp = client()
            .get(format!("{url}/api/glossary?lang_pair=en-es"))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
        let body: Vec<serde_json::Value> = resp.json().await.unwrap();
        assert_eq!(body.len(), 1);
        assert_eq!(body[0]["term"], "US");
        assert_eq!(body[0]["case_sensitive"], true);
        assert!(state.glossary.lookup_exact("us", "en", "es").is_none());
        assert_eq!(
            state.glossary.lookup_exact("US", "en", "es").as_deref(),
            Some("EEUU")
        );
    }

    #[tokio::test]
    async fn glossary_post_defaults_to_case_insensitive() {
        let (url, _h, state) = setup_with_state().await;
        mark_project_open(&state).await;
        let resp = client()
            .post(format!("{url}/api/glossary"))
            .json(&serde_json::json!({
                "term": "US",
                "translation": "EEUU",
                "lang_pair": "en-es",
                "context": null
            }))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 201);

        let resp = client()
            .get(format!("{url}/api/glossary?lang_pair=en-es"))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
        let body: Vec<serde_json::Value> = resp.json().await.unwrap();
        assert_eq!(body.len(), 1);
        assert_eq!(body[0]["case_sensitive"], false);
        assert_eq!(
            state.glossary.lookup_exact("us", "en", "es").as_deref(),
            Some("EEUU")
        );
    }

    #[tokio::test]
    async fn test_glossary_delete() {
        let (url, _h, state) = setup_with_state().await;
        mark_project_open(&state).await;
        client()
            .post(format!("{}/api/glossary", url))
            .json(&serde_json::json!({
                "term": "MP",
                "translation": "PM",
                "lang_pair": "en-es",
                "context": null,
                "case_sensitive": false
            }))
            .send()
            .await
            .unwrap();

        let resp = client()
            .delete(format!("{}/api/glossary/MP?lang_pair=en-es", url))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 204);
    }
}
