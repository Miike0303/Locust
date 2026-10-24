use axum::extract::{Path as AxumPath, Query, State};
use axum::http::StatusCode;
use axum::Json;
use locust_core::database::{Database, GlobalMemoryDb, GlossaryEntry};
use locust_core::fuzzy::{normalize, similarity_at_threshold};
use locust_core::models::StringEntry;
use serde::{Deserialize, Serialize};
use std::collections::{hash_map::Entry, HashMap};
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
    // This single DB read (including list count + rows) holds the connection
    // mutex, also used by writes and connection replacement. Reserving the
    // whole project would make concurrent Memory page reads reject each other.
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

#[derive(Deserialize)]
pub(crate) struct MemorySuggestionsQuery {
    entry_id: String,
    lang_pair: String,
}

#[derive(Debug, Serialize)]
struct MemorySuggestion {
    source: String,
    translation: String,
    score: f64,
    origin: &'static str,
    uses: i64,
}

/// The entry's saved translation is required to distinguish redundant exact
/// matches from useful alternatives. Reads never increment memory usage.
fn fuzzy_suggestions(
    entry: &StringEntry,
    lang_pair: &str,
    project_db: &Database,
    global_db: &GlobalMemoryDb,
    limit: usize,
    threshold: f64,
) -> locust_core::Result<Vec<MemorySuggestion>> {
    if limit == 0 {
        return Ok(Vec::new());
    }
    let source_len = entry.source.chars().count();
    let source_hash = entry.source_hash();
    let mut unique: HashMap<(String, String), MemorySuggestion> = HashMap::new();
    for (origin, candidates) in [
        (
            "project",
            project_db.memory_candidates(lang_pair, source_len, 3000)?,
        ),
        (
            "global",
            global_db.memory_candidates(lang_pair, source_len, 3000)?,
        ),
    ] {
        for candidate in candidates {
            let exact = candidate.source_hash == source_hash;
            if exact && entry.translation.as_deref() == Some(candidate.translation.as_str()) {
                continue;
            }
            let score = if exact {
                1.0
            } else if let Some(score) =
                similarity_at_threshold(&entry.source, &candidate.source, threshold)
            {
                score
            } else {
                continue;
            };
            let key = (normalize(&candidate.source), candidate.translation.clone());
            let suggestion = MemorySuggestion {
                source: candidate.source,
                translation: candidate.translation,
                score,
                origin,
                uses: candidate.uses,
            };
            match unique.entry(key) {
                Entry::Vacant(slot) => {
                    slot.insert(suggestion);
                }
                Entry::Occupied(mut slot) => {
                    let winner = slot.get();
                    if suggestion.score > winner.score
                        || (suggestion.score == winner.score && suggestion.uses > winner.uses)
                    {
                        slot.insert(suggestion);
                    }
                }
            }
        }
    }
    let mut suggestions: Vec<_> = unique.into_values().collect();
    suggestions.sort_by(|a, b| {
        b.score
            .total_cmp(&a.score)
            .then_with(|| b.uses.cmp(&a.uses))
            .then_with(|| a.source.cmp(&b.source))
            .then_with(|| a.translation.cmp(&b.translation))
    });
    suggestions.truncate(limit);
    Ok(suggestions)
}

pub(crate) async fn memory_suggestions(
    State(state): State<Arc<AppState>>,
    Query(query): Query<MemorySuggestionsQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    // Unlike the list reads, scoring spans multiple DB calls in a blocking
    // worker. Keep project ownership so a destructive open cannot swap the
    // connection between reading the entry and its project candidates.
    let guard = try_project_operation(&state).map_err(|m| err(StatusCode::CONFLICT, m))?;
    if state.current_project.read().await.is_none() {
        return Err(err(StatusCode::BAD_REQUEST, "no project open"));
    }
    if query.lang_pair.trim().is_empty() {
        return Err(err(StatusCode::BAD_REQUEST, "lang_pair is required"));
    }
    tokio::task::spawn_blocking(move || {
        // Keep ownership even when the HTTP caller cancels during scoring.
        let _guard = guard;
        let entry = state
            .db
            .get_entry(&query.entry_id)
            .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e))?
            .ok_or_else(|| err(StatusCode::NOT_FOUND, "entry not found"))?;
        let suggestions = fuzzy_suggestions(
            &entry,
            &query.lang_pair,
            &state.db,
            &state.global_memory,
            5,
            0.6,
        )
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e))?;
        Ok(Json(serde_json::json!({ "suggestions": suggestions })))
    })
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e))?
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
    // The connection mutex serializes this query with writes and replacement;
    // no project reservation is needed alongside the page's entries read.
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
    async fn memory_reads_do_not_contend_with_each_other_or_project_operations() {
        use axum::extract::State;
        use std::sync::atomic::Ordering;

        let (url, _server, state) = setup_with_state().await;
        mark_project_open(&state).await;
        state
            .db
            .save_memory("hello", "Hello", "Hola", "en-es")
            .await
            .unwrap();
        // Reproduce a language-pair read yielding after admission, before the
        // entries request arrives. It must never reserve the live project.
        let project = state.current_project.write().await;
        let pairs = super::memory_lang_pairs(State(state.clone()));
        tokio::pin!(pairs);
        assert!(futures_util::poll!(&mut pairs).is_pending());
        assert_eq!(state.project_exclusive.load(Ordering::SeqCst), 0);
        drop(project);
        assert_eq!(pairs.await.unwrap().0, ["en-es"]);

        // A read is safe even while another operation owns the project: each
        // database method serializes with writes and connection replacement.
        let guard = crate::try_project_operation(&state).unwrap();
        for path in [
            "/api/memory?limit=50",
            "/api/memory/lang-pairs",
            "/api/memory/stats",
        ] {
            let response = client().get(format!("{url}{path}")).send().await.unwrap();
            assert_eq!(response.status(), 200, "{path}");
            let body: serde_json::Value = response.json().await.unwrap();
            match path {
                "/api/memory?limit=50" => {
                    assert_eq!(body["total"], 1);
                    assert_eq!(body["entries"][0]["translation"], "Hola");
                }
                "/api/memory/lang-pairs" => assert_eq!(body, serde_json::json!(["en-es"])),
                _ => assert_eq!(body["project_entries"], 1),
            }
        }
        drop(guard);
        assert!(crate::try_project_operation(&state).is_ok());
    }

    #[tokio::test]
    async fn memory_suggestions_require_open_project_pair_and_known_entry() {
        let (url, _h, state) = setup_with_state().await;
        state
            .db
            .save_entries(&[translated("row", "Hello", "story.html", "Hola")])
            .unwrap();
        let response = client()
            .get(format!(
                "{url}/api/memory/suggestions?entry_id=row&lang_pair=en-es"
            ))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 400);
        assert_eq!(response.text().await.unwrap(), "no project open");
        mark_project_open(&state).await;
        for query in [
            "entry_id=row",
            "entry_id=row&lang_pair=",
            "entry_id=row&lang_pair=%20",
            "lang_pair=en-es",
        ] {
            let response = client()
                .get(format!("{url}/api/memory/suggestions?{query}"))
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), 400, "{query}");
        }
        let response = client()
            .get(format!(
                "{url}/api/memory/suggestions?entry_id=missing&lang_pair=en-es"
            ))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 404);
        assert_eq!(response.text().await.unwrap(), "entry not found");
    }

    #[tokio::test]
    async fn memory_suggestions_merge_dedupe_rank_filter_and_do_not_write() {
        let (url, _h, state) = setup_with_state().await;
        mark_project_open(&state).await;
        let entry = translated("row", "Hello", "story.html", "Current");
        state.db.save_entries(std::slice::from_ref(&entry)).unwrap();
        let hash = entry.source_hash();
        state
            .db
            .save_memory(&hash, "Hello", "Current", "en-es")
            .await
            .unwrap();
        state
            .global_memory
            .save_memory(&hash, "Hello", "Alternative", "en-es")
            .await
            .unwrap();
        state
            .db
            .save_memory("duplicate", "Hallo", "Hola", "en-es")
            .await
            .unwrap();
        for _ in 0..3 {
            state
                .global_memory
                .save_memory("duplicate", "HALLO", "Hola", "en-es")
                .await
                .unwrap();
        }
        state
            .db
            .save_memory("same-source", "Hallo", "Otra", "en-es")
            .await
            .unwrap();
        state
            .db
            .save_memory("threshold", "Hexxo", "At threshold", "en-es")
            .await
            .unwrap();
        state
            .db
            .save_memory("low", "xxxxx", "Below threshold", "en-es")
            .await
            .unwrap();
        state
            .db
            .save_memory("long", "A much longer source", "Too long", "en-es")
            .await
            .unwrap();
        state
            .global_memory
            .save_memory("other-lang", "Hallo", "Bonjour", "en-fr")
            .await
            .unwrap();
        let before_project =
            serde_json::to_value(state.db.list_memory(None, None, 50, 0).unwrap().0).unwrap();
        let before_global = serde_json::to_value(
            state
                .global_memory
                .list_memory(None, None, 50, 0)
                .unwrap()
                .0,
        )
        .unwrap();
        let response = client()
            .get(format!(
                "{url}/api/memory/suggestions?entry_id=row&lang_pair=en-es"
            ))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        let body: serde_json::Value = response.json().await.unwrap();
        let suggestions = body["suggestions"].as_array().unwrap();
        assert_eq!(suggestions.len(), 4);
        assert_eq!(
            suggestions[0],
            serde_json::json!({ "source": "Hello", "translation": "Alternative", "score": 1.0, "origin": "global", "uses": 1 })
        );
        assert_eq!(
            suggestions[1],
            serde_json::json!({ "source": "HALLO", "translation": "Hola", "score": 0.8, "origin": "global", "uses": 3 })
        );
        assert_eq!(suggestions[2]["translation"], "Otra");
        assert_eq!(suggestions[2]["origin"], "project");
        assert_eq!(suggestions[3]["score"], 0.6);
        assert_eq!(
            serde_json::to_value(state.db.list_memory(None, None, 50, 0).unwrap().0).unwrap(),
            before_project
        );
        assert_eq!(
            serde_json::to_value(
                state
                    .global_memory
                    .list_memory(None, None, 50, 0)
                    .unwrap()
                    .0
            )
            .unwrap(),
            before_global
        );
        assert_eq!(
            state
                .db
                .get_entry("row")
                .unwrap()
                .unwrap()
                .translation
                .as_deref(),
            Some("Current")
        );
    }

    #[tokio::test]
    async fn memory_suggestions_keep_exact_hits_for_untranslated_entries_and_other_sources() {
        let (url, _h, state) = setup_with_state().await;
        mark_project_open(&state).await;
        let mut entry = translated("file#row/1", "Hello", "story.html", "");
        entry.translation = None;
        state.db.save_entries(std::slice::from_ref(&entry)).unwrap();
        state
            .db
            .save_memory(&entry.source_hash(), "Hello", "", "en-es")
            .await
            .unwrap();
        state
            .global_memory
            .save_memory("different-hash", "HELLO", "", "en-es")
            .await
            .unwrap();
        let response = client()
            .get(format!("{url}/api/memory/suggestions"))
            .query(&[("entry_id", entry.id.as_str()), ("lang_pair", "en-es")])
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        let body: serde_json::Value = response.json().await.unwrap();
        assert_eq!(body["suggestions"].as_array().unwrap().len(), 1);
        assert_eq!(body["suggestions"][0]["translation"], "");
        assert_eq!(body["suggestions"][0]["origin"], "project");

        entry.translation = Some(String::new());
        state.db.save_entries(std::slice::from_ref(&entry)).unwrap();
        let response = client()
            .get(format!("{url}/api/memory/suggestions"))
            .query(&[("entry_id", entry.id.as_str()), ("lang_pair", "en-es")])
            .send()
            .await
            .unwrap();
        let body: serde_json::Value = response.json().await.unwrap();
        // Only the identical raw hash is excluded, not a normalized exact hit.
        assert_eq!(body["suggestions"][0]["origin"], "global");
        assert_eq!(body["suggestions"][0]["score"], 1.0);
    }

    #[tokio::test]
    async fn memory_suggestions_limit_to_five_and_return_empty_results() {
        let (url, _h, state) = setup_with_state().await;
        mark_project_open(&state).await;
        let entry = translated("row", "Hello", "story.html", "Current");
        state.db.save_entries(&[entry]).unwrap();
        for i in 0..7 {
            for _ in 0..=i {
                state
                    .db
                    .save_memory(
                        &format!("candidate{i}"),
                        "Hallo",
                        &format!("Translation {i}"),
                        "en-es",
                    )
                    .await
                    .unwrap();
            }
        }
        let response = client()
            .get(format!(
                "{url}/api/memory/suggestions?entry_id=row&lang_pair=en-es"
            ))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        let body: serde_json::Value = response.json().await.unwrap();
        let rows = body["suggestions"].as_array().unwrap();
        assert_eq!(rows.len(), 5);
        assert_eq!(
            rows.iter()
                .map(|row| row["uses"].as_i64().unwrap())
                .collect::<Vec<_>>(),
            [7, 6, 5, 4, 3]
        );
        let response = client()
            .get(format!(
                "{url}/api/memory/suggestions?entry_id=row&lang_pair=en-fr"
            ))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        assert_eq!(
            response.json::<serde_json::Value>().await.unwrap(),
            serde_json::json!({"suggestions": []})
        );
    }

    #[tokio::test]
    async fn memory_suggestions_dedupe_keeps_best_score_before_uses() {
        let project = locust_core::database::Database::open_in_memory().unwrap();
        let global = locust_core::database::GlobalMemoryDb::open_in_memory().unwrap();
        let source = "a".repeat(2000);
        let entry = translated("row", &source, "story.html", "Current");
        let padded = format!("{}{source}", " ".repeat(500));
        for _ in 0..2 {
            project
                .save_memory("padded", &padded, "Shared", "en-es")
                .await
                .unwrap();
        }
        global
            .save_memory("plain", &source, "Shared", "en-es")
            .await
            .unwrap();
        let rows = super::fuzzy_suggestions(&entry, "en-es", &project, &global, 5, 0.6).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].score, 1.0);
        assert_eq!(rows[0].origin, "global");
        assert_eq!(rows[0].uses, 1);
        assert!(
            super::fuzzy_suggestions(&entry, "en-es", &project, &global, 0, 0.6)
                .unwrap()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn memory_suggestions_respect_project_operation_and_translation_guards() {
        let (url, _h, state) = setup_with_state().await;
        mark_project_open(&state).await;
        let guard = crate::try_project_operation(&state).unwrap();
        let response = client()
            .get(format!(
                "{url}/api/memory/suggestions?entry_id=row&lang_pair=en-es"
            ))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 409);
        assert_eq!(response.text().await.unwrap(), crate::PROJECT_BUSY_MESSAGE);
        drop(guard);
        park_job(&state, "translation", crate::JobKind::Translate, None);
        let response = client()
            .get(format!(
                "{url}/api/memory/suggestions?entry_id=row&lang_pair=en-es"
            ))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 409);
        assert_eq!(
            response.text().await.unwrap(),
            crate::TRANSLATION_IN_FLIGHT_MESSAGE
        );
    }

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
                "context": "  Organization title\nUSA  ",
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
        assert_eq!(body[0]["context"], "  Organization title\nUSA  ");
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
