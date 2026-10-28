use axum::extract::{Path as AxumPath, Query, State};
use axum::http::StatusCode;
use axum::Json;
use locust_core::database::EntryFilter;
use locust_core::export;
use locust_core::font_validation::{
    suggestions_for_font_reports, FontAuditIssue, FontAuditReport, FontValidator,
    FONT_COVERAGE_LIMITATION,
};
use locust_core::models::StringEntry;
use locust_core::validation::{
    ValidationIssueFilter, ValidationIssueUpdateError, ValidationIssuesPage, ValidationSummary,
    Validator,
};
use serde::Deserialize;
use std::path::PathBuf;
use std::sync::Arc;

use crate::{err, run_owned_project_operation, try_project_operation, ApiError, AppState};

pub(crate) async fn list_validation_issues(
    State(state): State<Arc<AppState>>,
    Query(filter): Query<ValidationIssueFilter>,
) -> Result<Json<ValidationIssuesPage>, ApiError> {
    let _guard = try_project_operation(&state).map_err(|m| err(StatusCode::CONFLICT, m))?;
    if state.current_project.read().await.is_none() {
        return Err(err(StatusCode::BAD_REQUEST, "no project open"));
    }
    state
        .db
        .list_validation_issues(&filter)
        .map(Json)
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e))
}

pub(crate) async fn validation_summary(
    State(state): State<Arc<AppState>>,
) -> Result<Json<ValidationSummary>, ApiError> {
    let _guard = try_project_operation(&state).map_err(|m| err(StatusCode::CONFLICT, m))?;
    if state.current_project.read().await.is_none() {
        return Err(err(StatusCode::BAD_REQUEST, "no project open"));
    }
    state
        .db
        .validation_summary()
        .map(Json)
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e))
}

#[derive(Deserialize)]
pub(crate) struct IgnoreValidationIssueRequest {
    ignored: bool,
}

pub(crate) async fn set_validation_issue_ignored(
    State(state): State<Arc<AppState>>,
    AxumPath(id): AxumPath<i64>,
    Json(request): Json<IgnoreValidationIssueRequest>,
) -> Result<StatusCode, ApiError> {
    // Synchronous DB update: keep ownership until it returns.
    let _guard = try_project_operation(&state).map_err(|m| err(StatusCode::CONFLICT, m))?;
    if state.current_project.read().await.is_none() {
        return Err(err(StatusCode::BAD_REQUEST, "no project open"));
    }
    state
        .db
        .set_validation_issue_ignored(id, request.ignored)
        .map_err(|e| match e {
            ValidationIssueUpdateError::NotFound => err(StatusCode::NOT_FOUND, e),
            ValidationIssueUpdateError::Database(_) => err(StatusCode::INTERNAL_SERVER_ERROR, e),
        })?;
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn validate(
    State(state): State<Arc<AppState>>,
) -> Result<Json<serde_json::Value>, ApiError> {
    // validate_and_save writes issue rows into the live DB — open must not reopen under it.
    let guard = try_project_operation(&state).map_err(|m| err(StatusCode::CONFLICT, m))?;
    run_owned_project_operation(guard, async move { validate_owned(state).await })
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e))?
}

async fn validate_owned(state: Arc<AppState>) -> Result<Json<serde_json::Value>, ApiError> {
    if state.current_project.read().await.is_none() {
        return Err(err(StatusCode::BAD_REQUEST, "no project open"));
    }
    let entries = state
        .db
        .get_entries(&EntryFilter::default())
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e))?;
    let validation = Validator::validate_and_save(&entries, &state.db)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e))?;

    let game_path = state
        .current_project
        .read()
        .await
        .as_ref()
        .map(|p| p.path.clone());
    let font_audit = audit_project_fonts(game_path, entries).await;
    let font_suggestions = suggestions_for_font_reports(&font_audit.fonts);

    Ok(Json(serde_json::json!({
        "validation": validation,
        "fonts": font_audit.fonts,
        "font_issues": font_audit.issues,
        "font_limitations": FONT_COVERAGE_LIMITATION,
        "font_suggestions": font_suggestions,
    })))
}

/// Shared by HTTP and Tauri. Font traversal/parsing must not block the async
/// server, and unreadable fonts must remain visible in the validation result.
pub async fn audit_project_fonts(
    game_path: Option<PathBuf>,
    entries: Vec<StringEntry>,
) -> FontAuditReport {
    let Some(game_path) = game_path else {
        return FontAuditReport::default();
    };
    let error_path = game_path.clone();
    let result = tokio::task::spawn_blocking(move || {
        // Match font-patch's projected visible corpus, including physical
        // Japanese left untranslated in a database pivoted through English.
        let visible_text: Vec<&str> = entries
            .iter()
            .map(|entry| {
                let error = |message| locust_core::error::LocustError::ValidationError {
                    entry_id: entry.id.clone(),
                    message,
                };
                entry.require_current_translation().map_err(error)?;
                match entry
                    .translation
                    .as_deref()
                    .filter(|text| !text.trim().is_empty())
                {
                    Some(text) => Ok(text),
                    None => entry.injection_source().map_err(error),
                }
            })
            .collect::<locust_core::error::Result<_>>()?;
        // Single-file games (HTML, PAK, assets) can reference adjacent fonts.
        let root = if game_path.is_file() {
            game_path.parent().unwrap_or(&game_path)
        } else {
            &game_path
        };
        FontValidator::audit_game_fonts(root, &visible_text)
    })
    .await;
    match result {
        Ok(Ok(report)) => report,
        other => FontAuditReport {
            fonts: Vec::new(),
            issues: vec![FontAuditIssue {
                font_path: error_path,
                message: match other {
                    Ok(Err(error)) => error.to_string(),
                    Err(error) => format!("font audit task failed: {error}"),
                    Ok(Ok(_)) => unreachable!(),
                },
            }],
        },
    }
}

struct ExportQuery {
    lang: String,
    scope: export::ExportScope,
}

impl ExportQuery {
    fn parse(params: Vec<(String, String)>) -> Result<Self, ApiError> {
        let value = |key| {
            params
                .iter()
                .find(|(name, _)| name == key)
                .map(|(_, value)| value.clone())
        };
        let lang =
            value("lang").ok_or_else(|| err(StatusCode::BAD_REQUEST, "missing field `lang`"))?;
        let mut statuses: Vec<_> = params
            .iter()
            .filter(|(key, _)| key == "status")
            .map(|(_, value)| value.clone())
            .collect();
        let has_status = !statuses.is_empty();
        // A present but empty list is distinct from an omitted scope.
        if statuses.len() == 1 && statuses[0].is_empty() {
            statuses.clear();
        }
        let scope =
            export::ExportScope::parse(value("file"), has_status.then_some(statuses.as_slice()))
                .map_err(|message| err(StatusCode::BAD_REQUEST, message))?;
        Ok(Self { lang, scope })
    }
}

pub(crate) async fn export_po(
    State(state): State<Arc<AppState>>,
    Query(params): Query<Vec<(String, String)>>,
) -> Result<(StatusCode, [(String, String); 2], String), ApiError> {
    // Export combines rows with translation-run language metadata. Both must
    // belong to the same project, even across the config-lock await below.
    let _guard = try_project_operation(&state).map_err(|m| err(StatusCode::CONFLICT, m))?;
    if state.current_project.read().await.is_none() {
        return Err(err(StatusCode::BAD_REQUEST, "no project open"));
    }
    let q = ExportQuery::parse(params)?;
    let entries = q
        .scope
        .entries(&state.db)
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e))?;
    if entries.is_empty() {
        return Err(err(StatusCode::BAD_REQUEST, "no entries to export"));
    }
    let config = state.config.read().await;
    let source = state
        .db
        .resolve_export_source_lang(&q.lang, &config.default_source_lang)
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e))?;
    let po = export::export_po(&entries, &source, &q.lang);
    Ok((
        StatusCode::OK,
        [
            (
                "Content-Type".to_string(),
                "text/plain; charset=utf-8".to_string(),
            ),
            (
                "Content-Disposition".to_string(),
                format!("attachment; filename=\"translation_{}.po\"", q.lang),
            ),
        ],
        po,
    ))
}

pub(crate) async fn import_po(
    State(state): State<Arc<AppState>>,
    body: String,
) -> Result<Json<serde_json::Value>, ApiError> {
    let guard = try_project_operation(&state).map_err(|m| err(StatusCode::CONFLICT, m))?;
    run_owned_project_operation(guard, async move { import_po_owned(state, body).await })
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e))?
}

async fn import_po_owned(
    state: Arc<AppState>,
    body: String,
) -> Result<Json<serde_json::Value>, ApiError> {
    if state.current_project.read().await.is_none() {
        return Err(err(StatusCode::BAD_REQUEST, "no project open"));
    }
    let po_entries = export::import_po(&body).map_err(|e| err(StatusCode::BAD_REQUEST, e))?;
    let (updates, pre_skipped) = export::po_entries_for_batch(&po_entries);
    let attempted = updates.len();
    let report = state
        .db
        .save_imported_translations_batch(updates)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e))?;
    let (imported, skipped) =
        export::import_counts_after_batch(pre_skipped, attempted, report.imported);
    Ok(Json(
        serde_json::json!({"imported": imported, "skipped": skipped, "stale_sources": report.stale_sources, "unknown_ids": report.unknown_ids}),
    ))
}

pub(crate) async fn export_xliff(
    State(state): State<Arc<AppState>>,
    Query(params): Query<Vec<(String, String)>>,
) -> Result<(StatusCode, [(String, String); 2], String), ApiError> {
    let _guard = try_project_operation(&state).map_err(|m| err(StatusCode::CONFLICT, m))?;
    if state.current_project.read().await.is_none() {
        return Err(err(StatusCode::BAD_REQUEST, "no project open"));
    }
    let q = ExportQuery::parse(params)?;
    let entries = q
        .scope
        .entries(&state.db)
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e))?;
    if entries.is_empty() {
        return Err(err(StatusCode::BAD_REQUEST, "no entries to export"));
    }
    let config = state.config.read().await;
    let source = state
        .db
        .resolve_export_source_lang(&q.lang, &config.default_source_lang)
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e))?;
    let xliff = export::export_xliff(&entries, &source, &q.lang);
    Ok((
        StatusCode::OK,
        [
            (
                "Content-Type".to_string(),
                "application/xml; charset=utf-8".to_string(),
            ),
            (
                "Content-Disposition".to_string(),
                format!("attachment; filename=\"translation_{}.xliff\"", q.lang),
            ),
        ],
        xliff,
    ))
}

pub(crate) async fn import_xliff(
    State(state): State<Arc<AppState>>,
    body: String,
) -> Result<Json<serde_json::Value>, ApiError> {
    let guard = try_project_operation(&state).map_err(|m| err(StatusCode::CONFLICT, m))?;
    run_owned_project_operation(guard, async move { import_xliff_owned(state, body).await })
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e))?
}

async fn import_xliff_owned(
    state: Arc<AppState>,
    body: String,
) -> Result<Json<serde_json::Value>, ApiError> {
    if state.current_project.read().await.is_none() {
        return Err(err(StatusCode::BAD_REQUEST, "no project open"));
    }
    let units = export::import_xliff(&body).map_err(|e| err(StatusCode::BAD_REQUEST, e))?;
    let (updates, pre_skipped) = export::xliff_units_for_batch(&units);
    let attempted = updates.len();
    let report = state
        .db
        .save_imported_translations_batch(updates)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e))?;
    let (imported, skipped) =
        export::import_counts_after_batch(pre_skipped, attempted, report.imported);
    Ok(Json(
        serde_json::json!({"imported": imported, "skipped": skipped, "stale_sources": report.stale_sources, "unknown_ids": report.unknown_ids}),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::routes::test_support::*;
    use locust_core::models::{ValidationIssue, ValidationKind};
    use std::sync::atomic::Ordering;

    fn qa_issue(entry_id: &str, kind: ValidationKind) -> ValidationIssue {
        ValidationIssue {
            entry_id: entry_id.into(),
            kind,
            message: "validation message".into(),
            source: None,
        }
    }

    #[tokio::test]
    async fn validation_dashboard_requires_an_open_project() {
        let (url, _h, state) = setup_with_state().await;
        state
            .db
            .save_validation_issues(&[qa_issue("leftover", ValidationKind::EmptyTranslation)])
            .await
            .unwrap();
        for (method, path) in [
            (reqwest::Method::GET, "/api/validation/issues"),
            (reqwest::Method::GET, "/api/validation/summary"),
            (reqwest::Method::PATCH, "/api/validation/issues/1"),
        ] {
            let response = client()
                .request(method, format!("{url}{path}"))
                .json(&serde_json::json!({ "ignored": true }))
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), 400, "{path}");
            assert_eq!(response.text().await.unwrap(), "no project open");
        }
        let page = state
            .db
            .list_validation_issues(&Default::default())
            .unwrap();
        assert_eq!(page.total, 1);
        assert!(!page.issues[0].ignored);
    }

    #[tokio::test]
    async fn validation_dashboard_lists_filters_pages_and_joins_entries() {
        let (url, _h, state) = setup_with_state().await;
        mark_project_open(&state).await;
        state
            .db
            .save_entries(&[translated("a", "Hello {0}", "data/a.json", "Hola")])
            .unwrap();
        state
            .db
            .save_validation_issues(&[
                qa_issue(
                    "a",
                    ValidationKind::MissingPlaceholder {
                        placeholder: "{0}".into(),
                    },
                ),
                qa_issue(
                    "orphan",
                    ValidationKind::MissingPlaceholder {
                        placeholder: "{1}".into(),
                    },
                ),
                qa_issue("b", ValidationKind::EmptyTranslation),
            ])
            .await
            .unwrap();
        let response = client()
            .get(format!(
                "{url}/api/validation/issues?kind=MissingPlaceholder&limit=1"
            ))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        let first: serde_json::Value = response.json().await.unwrap();
        assert_eq!(first["total"], 2);
        assert_eq!(first["limit"], 1);
        assert_eq!(first["offset"], 0);
        assert_eq!(first["issues"].as_array().unwrap().len(), 1);
        assert_eq!(first["issues"][0]["kind"], "MissingPlaceholder");
        assert_eq!(first["issues"][0]["source"], "Hello {0}");
        assert_eq!(first["issues"][0]["translation"], "Hola");
        assert_eq!(first["issues"][0]["file_path"], "data/a.json");
        assert_eq!(first["issues"][0]["ignored"], false);
        let second: serde_json::Value = client()
            .get(format!(
                "{url}/api/validation/issues?kind=MissingPlaceholder&limit=1&offset=1"
            ))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(second["total"], 2);
        assert_eq!(second["offset"], 1);
        assert_eq!(second["issues"][0]["entry_id"], "orphan");
        assert!(second["issues"][0]["source"].is_null());
        assert!(second["issues"][0]["translation"].is_null());
        assert!(second["issues"][0]["file_path"].is_null());
        let summary: serde_json::Value = client()
            .get(format!("{url}/api/validation/summary"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(
            summary,
            serde_json::json!({
                "MissingPlaceholder": { "open": 2, "ignored": 0 },
                "EmptyTranslation": { "open": 1, "ignored": 0 },
            })
        );
        let unknown: ValidationIssuesPage = client()
            .get(format!("{url}/api/validation/issues?kind=Unknown"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(unknown.total, 0);
        assert!(unknown.issues.is_empty());
    }

    #[tokio::test]
    async fn validation_dashboard_caps_pages_and_rejects_invalid_queries() {
        let (url, _h, state) = setup_with_state().await;
        mark_project_open(&state).await;
        let issues: Vec<_> = (0..503)
            .map(|i| qa_issue(&format!("e{i}"), ValidationKind::EmptyTranslation))
            .collect();
        state.db.save_validation_issues(&issues).await.unwrap();
        let page: ValidationIssuesPage = client()
            .get(format!("{url}/api/validation/issues?limit=9999"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(page.limit, 500);
        assert_eq!(page.issues.len(), 500);
        assert_eq!(page.total, 503);
        let past: ValidationIssuesPage = client()
            .get(format!("{url}/api/validation/issues?offset=900"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(past.total, 503);
        assert!(past.issues.is_empty());
        for query in ["offset=-1", "limit=oops", "include_ignored=oops"] {
            assert_eq!(
                client()
                    .get(format!("{url}/api/validation/issues?{query}"))
                    .send()
                    .await
                    .unwrap()
                    .status(),
                400
            );
        }
    }

    #[tokio::test]
    async fn validation_dashboard_ignore_restore_survives_runs_and_removes_fixed_issues() {
        let (url, _h, state) = setup_with_state().await;
        mark_project_open(&state).await;
        state
            .db
            .save_entries(&[translated("a", "Hello", "data/a.json", "Hello")])
            .unwrap();
        for _ in 0..2 {
            assert_eq!(
                client()
                    .post(format!("{url}/api/validate"))
                    .send()
                    .await
                    .unwrap()
                    .status(),
                200
            );
        }
        let page: ValidationIssuesPage = client()
            .get(format!("{url}/api/validation/issues"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(page.total, 1);
        let id = page.issues[0].id;
        for ignored in [true, true, false, true] {
            assert_eq!(
                client()
                    .patch(format!("{url}/api/validation/issues/{id}"))
                    .json(&serde_json::json!({ "ignored": ignored }))
                    .send()
                    .await
                    .unwrap()
                    .status(),
                204
            );
            let open: ValidationIssuesPage = client()
                .get(format!("{url}/api/validation/issues"))
                .send()
                .await
                .unwrap()
                .json()
                .await
                .unwrap();
            assert_eq!(open.total, usize::from(!ignored));
            let all: ValidationIssuesPage = client()
                .get(format!(
                    "{url}/api/validation/issues?include_ignored=true&kind=IdenticalToSource"
                ))
                .send()
                .await
                .unwrap()
                .json()
                .await
                .unwrap();
            assert_eq!(all.total, 1);
            assert_eq!(all.issues[0].ignored, ignored);
            let summary: serde_json::Value = client()
                .get(format!("{url}/api/validation/summary"))
                .send()
                .await
                .unwrap()
                .json()
                .await
                .unwrap();
            assert_eq!(
                summary["IdenticalToSource"],
                serde_json::json!({ "open": usize::from(!ignored), "ignored": usize::from(ignored) })
            );
        }
        assert_eq!(
            client()
                .post(format!("{url}/api/validate"))
                .send()
                .await
                .unwrap()
                .status(),
            200
        );
        let all: ValidationIssuesPage = client()
            .get(format!("{url}/api/validation/issues?include_ignored=true"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(all.total, 1);
        assert!(all.issues[0].ignored);
        state
            .db
            .save_entries(&[translated("a", "Hello", "data/a.json", "Hola")])
            .unwrap();
        assert_eq!(
            client()
                .post(format!("{url}/api/validate"))
                .send()
                .await
                .unwrap()
                .status(),
            200
        );
        let cleared: ValidationIssuesPage = client()
            .get(format!("{url}/api/validation/issues?include_ignored=true"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(cleared.total, 0);
        assert!(cleared.issues.is_empty());
    }

    #[tokio::test]
    async fn validation_dashboard_unknown_issue_returns_404_and_requires_boolean() {
        let (url, _h, state) = setup_with_state().await;
        mark_project_open(&state).await;
        let response = client()
            .patch(format!("{url}/api/validation/issues/999999"))
            .json(&serde_json::json!({ "ignored": true }))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 404);
        assert_eq!(response.text().await.unwrap(), "validation issue not found");
        for body in [
            serde_json::json!({}),
            serde_json::json!({ "ignored": "true" }),
        ] {
            assert_eq!(
                client()
                    .patch(format!("{url}/api/validation/issues/1"))
                    .json(&body)
                    .send()
                    .await
                    .unwrap()
                    .status(),
                422
            );
        }
    }

    #[tokio::test]
    async fn validation_dashboard_refuses_project_conflicts_and_releases_guard() {
        let (url, _h, state) = setup_with_state().await;
        mark_project_open(&state).await;
        for translation in [false, true] {
            let guard = if translation {
                park_job(&state, "qa-job", crate::JobKind::Translate, None);
                None
            } else {
                Some(crate::ProjectExclusiveGuard::enter(
                    &state.project_exclusive,
                ))
            };
            for (method, path) in [
                (reqwest::Method::GET, "/api/validation/issues"),
                (reqwest::Method::GET, "/api/validation/summary"),
                (reqwest::Method::PATCH, "/api/validation/issues/1"),
            ] {
                assert_eq!(
                    client()
                        .request(method, format!("{url}{path}"))
                        .json(&serde_json::json!({ "ignored": true }))
                        .send()
                        .await
                        .unwrap()
                        .status(),
                    409
                );
            }
            drop(guard);
            state.active_jobs.remove("qa-job");
            assert_eq!(state.project_exclusive.load(Ordering::SeqCst), 0);
            assert_eq!(
                client()
                    .get(format!("{url}/api/validation/issues"))
                    .send()
                    .await
                    .unwrap()
                    .status(),
                200
            );
        }
    }

    #[tokio::test]
    async fn validation_requires_an_open_project() {
        let (url, _h, state) = setup_with_state().await;
        state
            .db
            .save_entries(&[translated("leftover", "Hello", "story.html", "Hola")])
            .unwrap();
        let before = state.db.get_entry("leftover").unwrap().unwrap();
        let issues_before = state.db.get_validation_issues(None).unwrap();

        let response = client()
            .post(format!("{url}/api/validate"))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 400);
        assert_eq!(response.text().await.unwrap(), "no project open");
        assert_eq!(
            serde_json::to_value(state.db.get_entry("leftover").unwrap().unwrap()).unwrap(),
            serde_json::to_value(before).unwrap()
        );
        assert_eq!(
            serde_json::to_value(state.db.get_validation_issues(None).unwrap()).unwrap(),
            serde_json::to_value(issues_before).unwrap()
        );
    }

    #[tokio::test]
    async fn exports_require_an_open_project() {
        let (url, _h, state) = setup_with_state().await;
        state
            .db
            .save_entries(&[translated("leftover", "Hello", "story.html", "Hola")])
            .unwrap();
        let before = state.db.get_entry("leftover").unwrap().unwrap();

        for format in ["po", "xliff"] {
            let response = client()
                .get(format!("{url}/api/export/{format}?lang=es"))
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), 400, "{format}");
            assert_eq!(response.text().await.unwrap(), "no project open");
        }
        assert_eq!(
            serde_json::to_value(state.db.get_entry("leftover").unwrap().unwrap()).unwrap(),
            serde_json::to_value(before).unwrap()
        );
    }

    #[tokio::test]
    async fn imports_require_an_open_project() {
        let (url, _h, state) = setup_with_state().await;
        state
            .db
            .save_entries(&[translated("leftover", "Hello", "story.html", "Hola")])
            .unwrap();
        let before = state.db.get_entry("leftover").unwrap().unwrap();
        let po = "msgctxt \"leftover\"\nmsgid \"Hello\"\nmsgstr \"Wrong\"\n";
        let xliff = r#"<xliff version="1.2"><file><body><trans-unit id="leftover"><source>Hello</source><target>Wrong</target></trans-unit></body></file></xliff>"#;

        for (format, body) in [("po", po), ("xliff", xliff)] {
            let response = client()
                .post(format!("{url}/api/import/{format}"))
                .body(body)
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), 400, "{format}");
            assert_eq!(response.text().await.unwrap(), "no project open");
            assert_eq!(
                serde_json::to_value(state.db.get_entry("leftover").unwrap().unwrap()).unwrap(),
                serde_json::to_value(&before).unwrap()
            );
        }
    }

    /// Validate writes issue rows into `state.db`. After it finishes the
    /// exclusive counter must be zero so open is not stuck refusing forever
    /// (negative: a leaked enter would leave open at 409).
    #[tokio::test]
    async fn validate_releases_project_exclusive_so_open_can_proceed() {
        let (url, _h, state) = setup_with_state().await;
        mark_project_open(&state).await;
        state
            .db
            .save_entries(&[translated("a", "Hello {0}", "f.json", "Hola {0}")])
            .unwrap();

        let resp = client()
            .post(format!("{}/api/validate", url))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
        assert_eq!(
            state.project_exclusive.load(Ordering::SeqCst),
            0,
            "validate must drop ProjectExclusiveGuard on completion"
        );

        let dir_guard = tempfile::Builder::new()
            .prefix("locust_val_excl_")
            .tempdir()
            .unwrap();
        let dir = dir_guard.path().to_path_buf();
        state.test_temp_dirs.lock().unwrap().push(dir_guard);
        let game = dir.join("OtherGame");
        write_rpgmaker_game(&game);

        let open = client()
            .post(format!("{}/api/project/open", url))
            .json(&serde_json::json!({"path": game.to_string_lossy()}))
            .send()
            .await
            .unwrap();
        assert_eq!(
            open.status(),
            200,
            "open after validate must not see a leaked exclusive: {}",
            open.text().await.unwrap_or_default()
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn test_export_po_returns_text() {
        let (url, _h, state) = setup_with_state().await;
        mark_project_open(&state).await;
        let mut entry = StringEntry::new("e1", "Hello", PathBuf::from("f.json"));
        entry.translation = Some("Hola".to_string());
        state.db.save_entries(&[entry]).unwrap();

        let resp = client()
            .get(format!("{}/api/export/po?lang=es", url))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
        let text = resp.text().await.unwrap();
        assert!(text.contains("msgid"));
        assert!(text.contains("msgstr"));
    }
}
