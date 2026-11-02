//! Reviewer imports must be previewed without writes and applied against that snapshot.
use std::sync::Arc;

use locust_core::database::EntryFilter;
use locust_core::export;
use locust_core::models::{StringEntry, StringStatus};
use locust_server::{create_test_state, start_test_server, AppState, ProjectInfo};
use reqwest::{Client, Response};
use serde_json::{json, Value};

const FORMATS: [&str; 3] = ["po", "xliff", "tmx"];
const FILE: &str = "scenarios/Scene 1_日本語.rpy";

fn row(id: &str, source: &str, text: Option<&str>, status: StringStatus) -> StringEntry {
    let mut row = StringEntry::new(id, source, FILE.into());
    row.translation = text.map(str::to_owned);
    row.status = status;
    row
}

async fn seeded_state() -> Arc<AppState> {
    let state = create_test_state();
    {
        let mut config = state.config.write().await;
        config.default_source_lang = "en".into();
        config.default_target_lang = "es".into();
    }
    *state.current_project.write().await = Some(ProjectInfo {
        path: "preview-project".into(),
        ..ProjectInfo::default()
    });
    state
        .db
        .save_entries(&[
            row("fill", "Empty", None, StringStatus::Pending),
            row(
                "replace",
                "Existing",
                Some("Revisada"),
                StringStatus::Reviewed,
            ),
            row("same", "Identical", Some("Igual"), StringStatus::Approved),
            row(
                "stale",
                "Current source",
                Some("Actual"),
                StringStatus::Reviewed,
            ),
            row("skip", "Skipped", None, StringStatus::Pending),
            row("other", "Unrelated", Some("Otra"), StringStatus::Approved),
        ])
        .unwrap();
    state
}

fn catalog(format: &str) -> String {
    catalog_rows(
        format,
        &[
            row(
                "fill",
                "Empty",
                Some("Nueva vacía"),
                StringStatus::Translated,
            ),
            row(
                "replace",
                "Existing",
                Some("Nueva revisada"),
                StringStatus::Translated,
            ),
            row("same", "Identical", Some("Igual"), StringStatus::Translated),
            row(
                "stale",
                "Obsolete source",
                Some("Obsoleta"),
                StringStatus::Translated,
            ),
            row(
                "unknown",
                "Missing",
                Some("Desconocida"),
                StringStatus::Translated,
            ),
            row("skip", "Skipped", Some(""), StringStatus::Pending),
        ],
    )
}

fn catalog_rows(format: &str, rows: &[StringEntry]) -> String {
    match format {
        "po" => export::export_po(rows, "en", "es"),
        "xliff" => export::export_xliff(rows, "en", "es"),
        "tmx" => export::export_tmx(rows, "en", "es").unwrap(),
        _ => unreachable!(),
    }
}

fn language_catalog(format: &str, source: &str, target: &str) -> String {
    let entries = [row(
        "fill",
        "Empty",
        Some("Imported"),
        StringStatus::Translated,
    )];
    match format {
        "po" => export::export_po(&entries, source, target),
        "xliff" => export::export_xliff(&entries, source, target),
        "tmx" => export::export_tmx(&entries, source, target).unwrap(),
        _ => unreachable!(),
    }
}

fn undeclared_catalog(format: &str) -> String {
    language_catalog(format, "en", "es")
        .replace("\"Language: es\\n\"", "")
        .replace("source-language=\"en\" target-language=\"es\"", "")
        .replace("srclang=\"en\"", "")
}

fn client() -> Client {
    Client::builder().no_proxy().build().unwrap()
}

fn snapshot(state: &AppState) -> Value {
    let history = ["fill", "replace", "same", "stale", "skip", "other"]
        .map(|id| state.db.get_string_history(id, 50).unwrap());
    json!({
        "rows": state.db.get_entries(&EntryFilter::default()).unwrap(),
        "history": history,
        "issues": state.db.get_validation_issues(None).unwrap(),
        "stats": state.db.get_stats().unwrap(),
        "memory": state.db.memory_count().unwrap(),
    })
}

async fn preview(client: &Client, base: &str, format: &str, content: &str, keep: bool) -> Value {
    let response = client
        .post(format!("{base}/api/import/{format}/preview"))
        .json(&json!({"content": content, "keep_existing": keep}))
        .send()
        .await
        .unwrap();
    assert_eq!(
        response.status(),
        200,
        "{format}: preview route must accept catalogs"
    );
    let body: Value = response.json().await.unwrap();
    assert!(body["plan_token"].as_str().is_some_and(|s| !s.is_empty()));
    body
}

async fn apply(
    client: &Client,
    base: &str,
    format: &str,
    content: &str,
    keep: bool,
    token: &str,
) -> Response {
    client
        .post(format!("{base}/api/import/{format}/apply"))
        .json(&json!({"content": content, "keep_existing": keep, "plan_token": token}))
        .send()
        .await
        .unwrap()
}

async fn assert_stale(response: Response, state: &AppState, before: Value) {
    assert_eq!(response.status(), 409);
    let message = response.text().await.unwrap().to_lowercase();
    assert!(
        message.contains("stale") && message.contains("preview"),
        "{message}"
    );
    assert_eq!(snapshot(state), before, "stale plans must write nothing");
}

#[tokio::test]
async fn preview_is_read_only_and_reports_core_counts_for_every_format_and_policy() {
    let state = seeded_state().await;
    let before = snapshot(&state);
    let (base, server) = start_test_server(state.clone()).await;
    let client = client();
    for format in FORMATS {
        for keep in [true, false] {
            let content = catalog(format);
            let plan = preview(&client, &base, format, &content, keep).await;
            for (name, expected) in [
                ("fill", 1),
                ("replace", usize::from(!keep)),
                ("unchanged", 1),
                ("unknown_ids", 1),
                ("stale_sources", 1),
                ("kept_existing", usize::from(keep)),
                ("imported", if keep { 2 } else { 3 }),
                ("skipped", if keep { 4 } else { 3 }),
            ] {
                assert_eq!(plan[name], expected, "{format}, keep={keep}, {name}");
            }
            assert_eq!(plan["truncated"], false);
            let replacements = plan["replacements"].as_array().unwrap();
            if keep {
                assert!(replacements.is_empty());
            } else {
                assert_eq!(
                    replacements,
                    &vec![json!({
                        "id": "replace", "file": FILE,
                        "previous_text": "Revisada", "previous_status": "reviewed",
                        "new_text": "Nueva revisada"
                    })]
                );
            }
            assert_eq!(
                snapshot(&state),
                before,
                "{format}: previews must write nothing"
            );
            assert_eq!(
                preview(&client, &base, format, &content, keep).await["plan_token"],
                plan["plan_token"],
                "unchanged previews produce the same fingerprint"
            );
        }
    }
    server.abort();
}

#[tokio::test]
async fn apply_preserves_reviewed_work_by_policy_and_matches_preview_counts() {
    for format in FORMATS {
        for keep in [true, false] {
            let state = seeded_state().await;
            let original = state.db.get_entry("replace").unwrap().unwrap();
            let identical = state.db.get_entry("same").unwrap().unwrap();
            let (base, server) = start_test_server(state.clone()).await;
            let client = client();
            let content = catalog(format);
            let plan = preview(&client, &base, format, &content, keep).await;
            let response = apply(
                &client,
                &base,
                format,
                &content,
                keep,
                plan["plan_token"].as_str().unwrap(),
            )
            .await;
            assert_eq!(response.status(), 200, "{format}, keep={keep}");
            let report: Value = response.json().await.unwrap();
            for name in [
                "imported",
                "skipped",
                "unchanged",
                "kept_existing",
                "stale_sources",
                "unknown_ids",
            ] {
                assert_eq!(report[name], plan[name], "{format}: {name}");
            }
            assert_eq!(
                state
                    .db
                    .get_entry("fill")
                    .unwrap()
                    .unwrap()
                    .translation
                    .as_deref(),
                Some("Nueva vacía")
            );
            let existing = state.db.get_entry("replace").unwrap().unwrap();
            if keep {
                assert_eq!(json!(existing), json!(original));
            } else {
                assert_eq!(existing.translation.as_deref(), Some("Nueva revisada"));
                assert_eq!(existing.status, StringStatus::Translated);
            }
            assert_eq!(
                json!(state.db.get_entry("same").unwrap().unwrap()),
                json!(identical)
            );
            assert_eq!(
                state
                    .db
                    .get_entry("stale")
                    .unwrap()
                    .unwrap()
                    .translation
                    .as_deref(),
                Some("Actual")
            );
            assert!(state.db.get_entry("unknown").unwrap().is_none());
            server.abort();
        }
    }
}

#[tokio::test]
async fn replacement_details_are_capped_without_capping_counts_or_apply() {
    let state = seeded_state().await;
    let existing: Vec<_> = (0..205)
        .map(|i| {
            row(
                &format!("bulk-{i:03}"),
                "Bulk",
                Some("Before"),
                StringStatus::Approved,
            )
        })
        .collect();
    state.db.save_entries(&existing).unwrap();
    let updates: Vec<_> = existing
        .into_iter()
        .map(|mut row| {
            row.translation = Some("After".into());
            row
        })
        .collect();
    let (base, server) = start_test_server(state.clone()).await;
    let client = client();
    for format in FORMATS {
        let content = catalog_rows(format, &updates);
        let plan = preview(&client, &base, format, &content, false).await;
        assert_eq!(plan["replace"], 205);
        assert_eq!(plan["replacements"].as_array().unwrap().len(), 200);
        assert_eq!(plan["truncated"], true);
        if format == "tmx" {
            let response = apply(
                &client,
                &base,
                format,
                &content,
                false,
                plan["plan_token"].as_str().unwrap(),
            )
            .await;
            assert_eq!(response.status(), 200);
            assert_eq!(response.json::<Value>().await.unwrap()["imported"], 205);
            assert_eq!(
                state
                    .db
                    .get_entry("bulk-204")
                    .unwrap()
                    .unwrap()
                    .translation
                    .as_deref(),
                Some("After")
            );
        }
    }
    server.abort();
}

#[tokio::test]
async fn plan_is_bound_to_exact_catalog_bytes_policy_format_and_project_identity() {
    let state = seeded_state().await;
    let (base, server) = start_test_server(state.clone()).await;
    let client = client();
    let content = catalog("po");
    let plan = preview(&client, &base, "po", &content, false).await;
    let token = plan["plan_token"].as_str().unwrap();
    for (format, bytes, keep) in [
        ("po", format!("{content}\n"), false),
        ("po", content.clone(), true),
        ("xliff", catalog("xliff"), false),
    ] {
        let before = snapshot(&state);
        assert_stale(
            apply(&client, &base, format, &bytes, keep, token).await,
            &state,
            before,
        )
        .await;
    }
    state.current_project.write().await.as_mut().unwrap().path = "different-project".into();
    let before = snapshot(&state);
    assert_stale(
        apply(&client, &base, "po", &content, false, token).await,
        &state,
        before,
    )
    .await;
    server.abort();
}

#[tokio::test]
async fn changed_source_status_file_or_unknown_row_invalidates_the_whole_plan() {
    for change in ["source", "status", "file", "unknown"] {
        let state = seeded_state().await;
        let (base, server) = start_test_server(state.clone()).await;
        let client = client();
        let content = catalog("po");
        let plan = preview(&client, &base, "po", &content, false).await;
        let mut changed = state.db.get_entry("replace").unwrap().unwrap();
        match change {
            "source" => changed.source = "Edited source".into(),
            "status" => changed.status = StringStatus::Approved,
            "file" => changed.file_path = "moved.rpy".into(),
            "unknown" => changed = row("unknown", "Missing", None, StringStatus::Pending),
            _ => unreachable!(),
        }
        state.db.save_entries(&[changed]).unwrap();
        let before = snapshot(&state);
        assert_stale(
            apply(
                &client,
                &base,
                "po",
                &content,
                false,
                plan["plan_token"].as_str().unwrap(),
            )
            .await,
            &state,
            before,
        )
        .await;
        server.abort();
    }
}

#[tokio::test]
async fn concurrent_http_edit_between_preview_and_apply_refuses_the_plan_without_partial_writes() {
    let state = seeded_state().await;
    let (base, server) = start_test_server(state.clone()).await;
    let client = client();
    let content = catalog("xliff");
    let plan = preview(&client, &base, "xliff", &content, false).await;
    let edit = {
        let client = client.clone();
        let base = base.clone();
        tokio::spawn(async move {
            client
                .patch(format!("{base}/api/strings/replace"))
                .json(&json!({"translation": "Concurrent reviewer edit", "status": "approved"}))
                .send()
                .await
                .unwrap()
        })
    };
    assert_eq!(edit.await.unwrap().status(), 200);
    let before = snapshot(&state);
    assert_stale(
        apply(
            &client,
            &base,
            "xliff",
            &content,
            false,
            plan["plan_token"].as_str().unwrap(),
        )
        .await,
        &state,
        before,
    )
    .await;
    assert!(state
        .db
        .get_entry("fill")
        .unwrap()
        .unwrap()
        .translation
        .is_none());
    assert_eq!(
        state
            .db
            .get_entry("replace")
            .unwrap()
            .unwrap()
            .translation
            .as_deref(),
        Some("Concurrent reviewer edit")
    );
    server.abort();
}

#[tokio::test]
async fn unrelated_row_changes_do_not_invalidate_an_affected_row_fingerprint() {
    let state = seeded_state().await;
    let (base, server) = start_test_server(state.clone()).await;
    let client = client();
    let content = catalog("po");
    let plan = preview(&client, &base, "po", &content, false).await;
    state
        .db
        .update_entry_status("other", StringStatus::Reviewed)
        .await
        .unwrap();
    assert_eq!(
        apply(
            &client,
            &base,
            "po",
            &content,
            false,
            plan["plan_token"].as_str().unwrap()
        )
        .await
        .status(),
        200
    );
    assert_eq!(
        state.db.get_entry("other").unwrap().unwrap().status,
        StringStatus::Reviewed
    );
    server.abort();
}

#[tokio::test]
async fn preview_and_apply_require_project_admission_and_an_open_project() {
    let state = seeded_state().await;
    let (base, server) = start_test_server(state.clone()).await;
    let client = client();
    let content = catalog("po");
    let plan = preview(&client, &base, "po", &content, true).await;
    let held = locust_server::try_project_operation(&state).unwrap();
    let before = snapshot(&state);
    for format in FORMATS {
        for stage in ["preview", "apply"] {
            let response = client.post(format!("{base}/api/import/{format}/{stage}"))
                .json(&json!({"content": catalog(format), "keep_existing": true, "plan_token": plan["plan_token"]}))
                .send().await.unwrap();
            assert_eq!(response.status(), 409, "{format}/{stage}");
            assert_eq!(
                response.text().await.unwrap(),
                locust_server::PROJECT_BUSY_MESSAGE
            );
        }
    }
    assert_eq!(snapshot(&state), before);
    drop(held);
    *state.current_project.write().await = None;
    for format in FORMATS {
        for stage in ["preview", "apply"] {
            let response = client.post(format!("{base}/api/import/{format}/{stage}"))
                .json(&json!({"content": catalog(format), "keep_existing": true, "plan_token": plan["plan_token"]}))
                .send().await.unwrap();
            if stage == "preview" {
                assert_eq!(response.status(), 400);
                assert_eq!(response.text().await.unwrap(), "no project open");
            } else {
                assert_stale(response, &state, before.clone()).await;
            }
        }
    }
    assert_eq!(snapshot(&state), before);
    server.abort();
}

#[tokio::test]
async fn apply_requires_a_token_and_invalid_catalog_errors_match_legacy_import() {
    let state = seeded_state().await;
    let before = snapshot(&state);
    let (base, server) = start_test_server(state.clone()).await;
    let client = client();
    for format in FORMATS {
        let response = client
            .post(format!("{base}/api/import/{format}/apply"))
            .json(&json!({"content": catalog(format), "keep_existing": true}))
            .send()
            .await
            .unwrap();
        assert!(response.status().is_client_error());
        let invalid = match format {
            "po" => "msgid \"unterminated",
            "xliff" => "<xliff><broken>",
            "tmx" => "<tmx><broken>",
            _ => unreachable!(),
        };
        let legacy = client
            .post(format!("{base}/api/import/{format}"))
            .body(invalid)
            .send()
            .await
            .unwrap();
        assert_eq!(legacy.status(), 400, "{format}: legacy import exists");
        let expected = legacy.text().await.unwrap();
        let response = client
            .post(format!("{base}/api/import/{format}/preview"))
            .json(&json!({"content": invalid, "keep_existing": true}))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 400, "{format}/preview");
        assert_eq!(
            response.text().await.unwrap(),
            expected,
            "same parser and validation"
        );
        let response = apply(&client, &base, format, invalid, true, "invalid").await;
        assert_stale(response, &state, before.clone()).await;
    }
    assert_eq!(snapshot(&state), before);
    server.abort();
}

#[tokio::test]
async fn legacy_raw_import_still_applies_without_preview_and_confirmations_keep_core_semantics() {
    for format in FORMATS {
        let state = seeded_state().await;
        let confirm = row(
            "confirm",
            "Confirm",
            Some("Igual pendiente"),
            StringStatus::Pending,
        );
        state
            .db
            .save_entries(std::slice::from_ref(&confirm))
            .unwrap();
        let (base, server) = start_test_server(state.clone()).await;
        let client = client();
        let content = catalog_rows(format, &[confirm]);
        let plan = preview(&client, &base, format, &content, true).await;
        assert_eq!(plan["imported"], 1);
        assert_eq!(plan["unchanged"], 0);
        assert_eq!(
            apply(
                &client,
                &base,
                format,
                &content,
                true,
                plan["plan_token"].as_str().unwrap()
            )
            .await
            .status(),
            200
        );
        assert_eq!(
            state.db.get_entry("confirm").unwrap().unwrap().status,
            StringStatus::Translated
        );
        let response = client
            .post(format!("{base}/api/import/{format}"))
            .body(catalog(format))
            .send()
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            200,
            "{format}: raw legacy import remains compatible"
        );
        assert_eq!(
            state
                .db
                .get_entry("replace")
                .unwrap()
                .unwrap()
                .translation
                .as_deref(),
            Some("Nueva revisada")
        );
        server.abort();
    }
}

#[tokio::test]
async fn language_mismatches_refuse_preview_direct_and_changed_apply_with_zero_writes() {
    for format in FORMATS {
        let state = seeded_state().await;
        let (base, server) = start_test_server(state.clone()).await;
        let client = client();
        let accepted = language_catalog(format, "en", "es");
        let plan = preview(&client, &base, format, &accepted, false).await;
        let before = snapshot(&state);
        let mut pairs = vec![("en", "fr")];
        if format != "po" {
            pairs.push(("es", "en"));
        }
        for (source, target) in pairs {
            let content = language_catalog(format, source, target);
            for allow in [false, true] {
                let response = client
                    .post(format!("{base}/api/import/{format}/preview"))
                    .json(&json!({"content": content, "allow_undeclared_language": allow}))
                    .send()
                    .await
                    .unwrap();
                assert_eq!(response.status(), 400);
                let message = response.text().await.unwrap();
                assert!(
                    message.contains("does not match project"),
                    "{format}: {message}"
                );
                let response = client
                    .post(format!(
                        "{base}/api/import/{format}?allow_undeclared_language={allow}"
                    ))
                    .body(content.clone())
                    .send()
                    .await
                    .unwrap();
                assert_eq!(response.status(), 400);
                assert_eq!(response.text().await.unwrap(), message);
                let response = client.post(format!("{base}/api/import/{format}/apply"))
                    .json(&json!({"content": content, "keep_existing": false, "plan_token": plan["plan_token"], "allow_undeclared_language": allow})).send().await.unwrap();
                assert_stale(response, &state, before.clone()).await;
            }
        }
        assert_eq!(snapshot(&state), before);
        server.abort();
    }
}

#[tokio::test]
async fn undeclared_languages_warn_in_preview_and_require_explicit_apply_and_direct_confirmation() {
    for format in FORMATS {
        let state = seeded_state().await;
        let (base, server) = start_test_server(state.clone()).await;
        let client = client();
        let content = undeclared_catalog(format);
        let before = snapshot(&state);
        let plan = preview(&client, &base, format, &content, true).await;
        assert_eq!(plan["fill"], 1);
        assert_eq!(plan["languages"]["requires_confirmation"], true);
        assert!(!plan["languages"]["warnings"].as_array().unwrap().is_empty());
        assert_eq!(snapshot(&state), before);
        for allow in [None, Some(false)] {
            let mut body = json!({"content": content, "keep_existing": true, "plan_token": plan["plan_token"]});
            if let Some(allow) = allow {
                body["allow_undeclared_language"] = json!(allow);
            }
            let response = client
                .post(format!("{base}/api/import/{format}/apply"))
                .json(&body)
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), 400, "{format}");
            assert!(response
                .text()
                .await
                .unwrap()
                .contains("language not declared"));
            let response = client
                .post(format!("{base}/api/import/{format}"))
                .body(content.clone())
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), 400);
            assert_eq!(snapshot(&state), before);
        }
        let response = client.post(format!("{base}/api/import/{format}/apply"))
            .json(&json!({"content": content, "keep_existing": true, "plan_token": plan["plan_token"], "allow_undeclared_language": true})).send().await.unwrap();
        assert_eq!(
            response.status(),
            200,
            "{format}: {}",
            response.text().await.unwrap()
        );
        assert_eq!(
            state
                .db
                .get_entry("fill")
                .unwrap()
                .unwrap()
                .translation
                .as_deref(),
            Some("Imported")
        );
        let response = client
            .post(format!(
                "{base}/api/import/{format}?allow_undeclared_language=true"
            ))
            .body(content)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        server.abort();
    }
}

#[tokio::test]
async fn regional_tags_match_and_conflicting_regions_only_warn() {
    for format in FORMATS {
        for target in ["es", "ES-mx", "es-ES", "es_ES"] {
            let state = seeded_state().await;
            state.config.write().await.default_target_lang = "es-MX".into();
            let (base, server) = start_test_server(state.clone()).await;
            let client = client();
            let content = language_catalog(
                format,
                "en",
                if target == "es_ES" { "es-ES" } else { target },
            )
            .replace("es-ES", target);
            let plan = preview(&client, &base, format, &content, true).await;
            assert_eq!(plan["languages"]["requires_confirmation"], false);
            let warnings = plan["languages"]["warnings"].as_array().unwrap();
            assert_eq!(
                warnings.len(),
                usize::from(["es-ES", "es_ES"].contains(&target))
            );
            let response = apply(
                &client,
                &base,
                format,
                &content,
                true,
                plan["plan_token"].as_str().unwrap(),
            )
            .await;
            assert_eq!(response.status(), 200);
            server.abort();
        }
    }
}

#[tokio::test]
async fn project_language_changes_in_config_or_ledger_stale_accepted_pairs() {
    use locust_core::database::TranslationRun;
    for format in FORMATS {
        for change in ["source", "target", "run"] {
            let state = seeded_state().await;
            let (base, server) = start_test_server(state.clone()).await;
            let client = client();
            let content = language_catalog(format, "en", "es");
            let plan = preview(&client, &base, format, &content, true).await;
            match change {
                "source" => state.config.write().await.default_source_lang = "en-GB".into(),
                "target" => state.config.write().await.default_target_lang = "es-MX".into(),
                "run" => state
                    .db
                    .record_translation_run(&TranslationRun {
                        source_lang: "en".into(),
                        target_lang: "fr".into(),
                        ..Default::default()
                    })
                    .await
                    .unwrap(),
                _ => unreachable!(),
            }
            let before = snapshot(&state);
            let response = apply(
                &client,
                &base,
                format,
                &content,
                true,
                plan["plan_token"].as_str().unwrap(),
            )
            .await;
            assert_stale(response, &state, before).await;
            server.abort();
        }
    }
}

#[tokio::test]
async fn pivot_imports_use_semantic_pair_after_reopen_and_ignore_physical_source() {
    use locust_core::database::{Database, TranslationRun};
    let state = seeded_state().await;
    let mut original = row("fill", "日本語", Some("Empty"), StringStatus::Translated);
    original.metadata.insert("format_id".into(), json!("renpy"));
    state.db.save_entries(&[original]).unwrap();
    state
        .db
        .set_project_metadata("source_lang", &json!("ja"))
        .unwrap();
    let directory = tempfile::tempdir().unwrap();
    let pivot = directory.path().join("pivot.db");
    state.db.pivot_to(&pivot).unwrap();
    let pivot_db = Database::open(&pivot).unwrap();
    pivot_db
        .record_translation_run(&TranslationRun {
            source_lang: "en".into(),
            target_lang: "es".into(),
            ..Default::default()
        })
        .await
        .unwrap();
    drop(pivot_db);
    state.config.write().await.default_source_lang = "ja".into();
    state.config.write().await.default_target_lang = "fr".into();
    let (base, server) = start_test_server(state.clone()).await;
    let client = client();
    let response = client
        .post(format!("{base}/api/project/open-db"))
        .json(&json!({
            "database_path": pivot,
            "game_path": directory.path().join("game"),
            "format_id": "renpy"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200, "{}", response.text().await.unwrap());
    assert_eq!(state.db.path(), pivot);
    let before = snapshot(&state);
    for format in ["xliff", "tmx"] {
        let response = client
            .post(format!("{base}/api/import/{format}/preview"))
            .json(&json!({"content": language_catalog(format, "ja", "es")}))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 400);
        assert_eq!(snapshot(&state), before);
    }
    let content = language_catalog("tmx", "en", "es");
    let plan = preview(&client, &base, "tmx", &content, true).await;
    assert_eq!(
        plan["languages"]["project"],
        json!({"source": "en", "target": "es"})
    );
    let response = apply(
        &client,
        &base,
        "tmx",
        &content,
        true,
        plan["plan_token"].as_str().unwrap(),
    )
    .await;
    assert_eq!(response.status(), 200);
    let saved = state.db.get_entry("fill").unwrap().unwrap();
    assert_eq!(saved.translation.as_deref(), Some("Imported"));
    assert_eq!(saved.injection_source().unwrap(), "日本語");
    server.abort();
}

// Exercise the server functions called by Tauri as well as their HTTP wrappers.
async fn w4_08_refused(state: Arc<AppState>, format: &str, content: &str) {
    let before = snapshot(&state);
    let accepted = {
        let config = state.config.read().await;
        let pair = export::project_import_languages(
            &state.db,
            &config.default_source_lang,
            &config.default_target_lang,
        )
        .unwrap();
        language_catalog(
            format,
            pair.source.as_deref().unwrap_or("en"),
            pair.target.as_deref().unwrap_or("es"),
        )
    };
    let previous = locust_server::preview_catalog(state.clone(), format.into(), accepted, true)
        .await
        .unwrap();
    let native = locust_server::preview_catalog(state.clone(), format.into(), content.into(), true)
        .await
        .expect_err("native preview must refuse");
    assert_eq!(native.0.as_u16(), 400);
    let (base, server) = start_test_server(state.clone()).await;
    let client = client();
    for allow in [false, true] {
        let response = client
            .post(format!("{base}/api/import/{format}/preview"))
            .json(&json!({"content": content, "allow_undeclared_language": allow}))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 400);
        assert_eq!(response.text().await.unwrap(), native.1);
        let response = client
            .post(format!(
                "{base}/api/import/{format}?allow_undeclared_language={allow}"
            ))
            .body(content.to_owned())
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 400);
        assert_eq!(response.text().await.unwrap(), native.1);
        let error = locust_server::apply_catalog_preview(
            state.clone(),
            format.into(),
            content.into(),
            true,
            previous.plan_token.clone(),
            allow,
        )
        .await
        .unwrap_err();
        assert_eq!(error.0.as_u16(), 409);
        let response = client
            .post(format!("{base}/api/import/{format}/apply"))
            .json(&json!({"content": content, "keep_existing": true,
                "plan_token": previous.plan_token, "allow_undeclared_language": allow}))
            .send()
            .await
            .unwrap();
        assert_stale(response, &state, before.clone()).await;
        assert_eq!(snapshot(&state), before);
    }
    server.abort();
}

async fn w4_08_confirmation(state: Arc<AppState>, format: &str, content: &str) {
    let before = snapshot(&state);
    let (base, server) = start_test_server(state.clone()).await;
    let client = client();
    let plan = preview(&client, &base, format, content, true).await;
    assert_eq!(plan["languages"]["requires_confirmation"], true);
    let native = locust_server::preview_catalog(state.clone(), format.into(), content.into(), true)
        .await
        .unwrap();
    assert_eq!(serde_json::to_value(&native).unwrap(), plan);
    assert_eq!(snapshot(&state), before);
    let error = locust_server::apply_catalog_preview(
        state.clone(),
        format.into(),
        content.into(),
        true,
        native.plan_token.clone(),
        false,
    )
    .await
    .unwrap_err();
    assert_eq!(error.0.as_u16(), 400);
    let response = apply(&client, &base, format, content, true, &native.plan_token).await;
    assert_eq!(response.status(), 400);
    assert_eq!(response.text().await.unwrap(), error.1);
    let response = client
        .post(format!("{base}/api/import/{format}"))
        .body(content.to_owned())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 400);
    assert_eq!(snapshot(&state), before);
    // Even an innocuous byte change invalidates an ambiguous catalog's plan.
    let changed = format!("{content}\n");
    let error = locust_server::apply_catalog_preview(
        state.clone(),
        format.into(),
        changed.clone(),
        true,
        native.plan_token.clone(),
        true,
    )
    .await
    .unwrap_err();
    assert_eq!(error.0.as_u16(), 409);
    assert_stale(
        apply(&client, &base, format, &changed, true, &native.plan_token).await,
        &state,
        before.clone(),
    )
    .await;
    let response = client
        .post(format!("{base}/api/import/{format}/apply"))
        .json(&json!({"content": content, "keep_existing": true,
            "plan_token": native.plan_token, "allow_undeclared_language": true}))
        .send()
        .await
        .unwrap();
    assert_eq!(
        response.status(),
        200,
        "{format}: {}",
        response.text().await.unwrap()
    );
    let native = locust_server::preview_catalog(state.clone(), format.into(), content.into(), true)
        .await
        .unwrap();
    locust_server::apply_catalog_preview(
        state.clone(),
        format.into(),
        content.into(),
        true,
        native.plan_token,
        true,
    )
    .await
    .unwrap();
    server.abort();
}

#[tokio::test]
async fn w4_08_script_conflicts_refuse_http_and_native_with_zero_writes() {
    for (expected, conflicting) in [("zh-Hans", "zh-Hant"), ("sr-Latn", "sr-Cyrl")] {
        for format in FORMATS {
            let state = seeded_state().await;
            state.config.write().await.default_target_lang = expected.into();
            w4_08_refused(state, format, &language_catalog(format, "en", conflicting)).await;
            if format != "po" {
                let state = seeded_state().await;
                state.config.write().await.default_source_lang = expected.into();
                w4_08_refused(state, format, &language_catalog(format, conflicting, "es")).await;
            }
        }
    }
}

#[tokio::test]
async fn w4_08_script_omission_requires_confirmation_http_and_native() {
    for format in FORMATS {
        for (expected, declared) in [("zh-Hans", "zh"), ("sr", "sr-Latn")] {
            let state = seeded_state().await;
            state.config.write().await.default_target_lang = expected.into();
            w4_08_confirmation(state, format, &language_catalog(format, "en", declared)).await;
        }
    }
}

#[tokio::test]
async fn w4_08_partial_and_disagreeing_xliff_files_keep_all_evidence() {
    let complete = language_catalog("xliff", "en", "es");
    for content in [
        complete.replace(" target-language=\"es\"", ""),
        complete.replace(
            "</xliff>",
            "<file source-language=\"en\"><body/></file></xliff>",
        ),
        complete.replace(
            "</xliff>",
            "<file source-language=\"en\" target-language=\"es-MX\"><body/></file></xliff>",
        ),
    ] {
        w4_08_confirmation(seeded_state().await, "xliff", &content).await;
    }
    for attrs in [
        "source-language=\"fr\"",
        "source-language=\"en\" target-language=\"fr\"",
    ] {
        let content =
            complete.replace("</xliff>", &format!("<file {attrs}><body/></file></xliff>"));
        w4_08_refused(seeded_state().await, "xliff", &content).await;
    }
}

#[tokio::test]
async fn w4_08_po_duplicate_and_empty_headers_do_not_hide_conflicts() {
    let complete = language_catalog("po", "en", "es");
    for headers in [
        "Language: \\n",
        "Language: es\\nLanguage: \\n",
        "Language: es\\nLanguage: es-MX\\n",
    ] {
        let content = complete.replace("Language: es\\n", headers);
        w4_08_confirmation(seeded_state().await, "po", &content).await;
    }
    let conflicting = complete.replace("Language: es\\n", "Language: es\\nLanguage: fr\\n");
    w4_08_refused(seeded_state().await, "po", &conflicting).await;
    let equivalent = complete.replace("Language: es\\n", "Language: es\\nLanguage: ES\\n");
    let state = seeded_state().await;
    let native = locust_server::preview_catalog(state, "po".into(), equivalent, true)
        .await
        .unwrap();
    assert!(!native.languages.requires_confirmation);
}

#[tokio::test]
async fn w4_08_tmx_undeclared_orientation_and_strict_structure_agree() {
    let complete = language_catalog("tmx", "en", "es");
    for source in ["", "*all*"] {
        let content = complete.replace("srclang=\"en\"", &format!("srclang=\"{source}\""));
        w4_08_confirmation(seeded_state().await, "tmx", &content).await;
    }
    for content in [
        export::export_tmx(&[], "en", "es").unwrap(),
        export::export_tmx(&[], "en", "es")
            .unwrap()
            .replace("srclang=\"en\"", "srclang=\"*all*\""),
    ] {
        w4_08_confirmation(seeded_state().await, "tmx", &content).await;
    }
    for content in [
        complete.replace(" xml:lang=\"es\"", ""),
        complete.replace("</tu>", "<tuv xml:lang=\"fr\"><seg>Third</seg></tuv></tu>"),
    ] {
        w4_08_refused(seeded_state().await, "tmx", &content).await;
    }
}

#[tokio::test]
async fn w4_08_unknown_project_pair_requires_confirmation_and_known_sides_constrain() {
    for format in FORMATS {
        let state = seeded_state().await;
        state.config.write().await.default_source_lang.clear();
        state.config.write().await.default_target_lang.clear();
        w4_08_confirmation(state, format, &language_catalog(format, "en", "es")).await;
        let state = seeded_state().await;
        state.config.write().await.default_source_lang.clear();
        state.config.write().await.default_target_lang = "fr".into();
        w4_08_refused(state, format, &language_catalog(format, "en", "es")).await;
    }
}

#[tokio::test]
async fn w4_08_script_and_unknown_pair_changes_stale_http_and_native_plans() {
    for format in FORMATS {
        for unknown in [false, true] {
            let state = seeded_state().await;
            state.config.write().await.default_target_lang =
                if unknown { "" } else { "zh-Hans" }.into();
            if unknown {
                state.config.write().await.default_source_lang.clear();
            }
            let (base, server) = start_test_server(state.clone()).await;
            let client = client();
            let content = language_catalog(format, "en", "zh-Hans");
            let plan = preview(&client, &base, format, &content, true).await;
            state.config.write().await.default_target_lang =
                if unknown { "zh-Hans" } else { "zh-Hant" }.into();
            if unknown {
                state.config.write().await.default_source_lang = "en".into();
            }
            let before = snapshot(&state);
            let error = locust_server::apply_catalog_preview(
                state.clone(),
                format.into(),
                content.clone(),
                true,
                plan["plan_token"].as_str().unwrap().into(),
                true,
            )
            .await
            .unwrap_err();
            assert_eq!(error.0.as_u16(), 409);
            assert_stale(
                apply(
                    &client,
                    &base,
                    format,
                    &content,
                    true,
                    plan["plan_token"].as_str().unwrap(),
                )
                .await,
                &state,
                before,
            )
            .await;
            server.abort();
        }
    }
}

#[tokio::test]
async fn w4_08_fresh_pivot_uses_config_before_first_run_despite_copied_metadata() {
    use locust_core::database::Database;
    let state = seeded_state().await;
    let original = row("fill", "日本語", Some("Empty"), StringStatus::Translated);
    state.db.save_entries(&[original]).unwrap();
    state
        .db
        .set_project_metadata("source_lang", &json!("ja"))
        .unwrap();
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("pivot.db");
    state.db.pivot_to(&path).unwrap();
    assert!(Database::open(&path)
        .unwrap()
        .get_translation_runs()
        .unwrap()
        .is_empty());
    let (base, server) = start_test_server(state.clone()).await;
    let client = client();
    let response = client.post(format!("{base}/api/project/open-db"))
        .json(&json!({"database_path": path, "game_path": directory.path().join("game"), "format_id": "renpy"}))
        .send().await.unwrap();
    assert_eq!(response.status(), 200, "{}", response.text().await.unwrap());
    for format in FORMATS {
        let content = language_catalog(format, "en", "es");
        let plan = preview(&client, &base, format, &content, true).await;
        let native =
            locust_server::preview_catalog(state.clone(), format.into(), content.clone(), true)
                .await
                .unwrap();
        assert_eq!(serde_json::to_value(&native).unwrap(), plan);
        assert_eq!(
            plan["languages"]["project"],
            json!({"source": "en", "target": "es"})
        );
        assert_eq!(plan["languages"]["requires_confirmation"], false);
        assert_eq!(
            apply(
                &client,
                &base,
                format,
                &content,
                true,
                plan["plan_token"].as_str().unwrap()
            )
            .await
            .status(),
            200
        );
    }
    assert_eq!(
        state
            .db
            .get_entry("fill")
            .unwrap()
            .unwrap()
            .injection_source()
            .unwrap(),
        "日本語"
    );
    server.abort();
}

#[tokio::test]
async fn w4_08_tmx_orientation_uses_known_target_and_scripts_before_variant_order() {
    for scripts in [false, true] {
        let (source, target, configured_source, configured_target) = if scripts {
            ("zh-Hans-CN", "zh-Hant-TW", "zh-Hans", "zh-Hant")
        } else {
            ("en", "es", "", "es")
        };
        let state = seeded_state().await;
        state.config.write().await.default_source_lang = configured_source.into();
        state.config.write().await.default_target_lang = configured_target.into();
        let content = language_catalog("tmx", source, target)
            .replace(&format!("srclang=\"{source}\""), "srclang=\"*all*\"")
            .replace(
                &format!("<tuv xml:lang=\"{source}\"><seg>Empty</seg></tuv><tuv xml:lang=\"{target}\"><seg>Imported</seg></tuv>"),
                &format!("<tuv xml:lang=\"{target}\"><seg>Imported</seg></tuv><tuv xml:lang=\"{source}\"><seg>Empty</seg></tuv>"),
            );
        let native =
            locust_server::preview_catalog(state.clone(), "tmx".into(), content.clone(), true)
                .await
                .unwrap();
        assert_eq!(native.fill, 1);
        w4_08_confirmation(state.clone(), "tmx", &content).await;
        assert_eq!(
            state
                .db
                .get_entry("fill")
                .unwrap()
                .unwrap()
                .translation
                .as_deref(),
            Some("Imported")
        );
    }
}
