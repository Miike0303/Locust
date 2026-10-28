//! Review assignments must select the same ordered rows in PO, XLIFF and TMX.
use std::sync::Arc;

use locust_core::database::EntryFilter;
use locust_core::export;
use locust_core::models::{StringEntry, StringStatus};
use locust_server::{create_test_state, start_test_server, AppState, ProjectInfo};
use reqwest::{Client, Response};

const FILE: &str = "scenarios/Scene 1_日本語.rpy";

async fn seeded_state() -> Arc<AppState> {
    let state = create_test_state();
    *state.current_project.write().await = Some(ProjectInfo::default());
    state.config.write().await.default_source_lang = "ja".into();
    // Deliberately insert out of ID order. Near-matching paths must stay distinct.
    let rows = [
        ("z", FILE, StringStatus::Translated),
        ("a", FILE, StringStatus::Pending),
        ("c", FILE, StringStatus::Reviewed),
        ("b", FILE, StringStatus::Approved),
        ("d", FILE, StringStatus::Error),
        ("e", "scenarios/Scene 2.rpy", StringStatus::Translated),
        (
            "f",
            "scenarios/scene 1_日本語.rpy",
            StringStatus::Translated,
        ),
        (
            "g",
            "scenarios\\Scene 1_日本語.rpy",
            StringStatus::Translated,
        ),
        (
            "h",
            "./scenarios/Scene 1_日本語.rpy",
            StringStatus::Translated,
        ),
    ]
    .into_iter()
    .map(|(id, file, status)| {
        let mut row = StringEntry::new(id, format!("Source {id}"), file.into());
        row.translation = Some(format!("Traducción {id}"));
        row.status = status;
        row
    })
    .collect::<Vec<_>>();
    state.db.save_entries(&rows).unwrap();
    state
}

fn client() -> Client {
    Client::builder().no_proxy().build().unwrap()
}

async fn get(client: &Client, base: &str, format: &str, scope: &[(&str, &str)]) -> Response {
    let source = if format == "tmx" {
        vec![("source_lang", "ja")]
    } else {
        vec![]
    };
    client
        .get(format!("{base}/api/export/{format}"))
        .query(&source)
        .query(&[("lang", "es")])
        .query(scope)
        .send()
        .await
        .unwrap()
}

fn old_output(format: &str, entries: &[StringEntry]) -> String {
    match format {
        "po" => export::export_po(entries, "ja", "es"),
        "xliff" => export::export_xliff(entries, "ja", "es"),
        "tmx" => export::export_tmx(entries, "ja", "es").unwrap(),
        _ => unreachable!(),
    }
}

#[tokio::test]
async fn no_filters_are_byte_identical_to_the_old_unfiltered_export() {
    let state = seeded_state().await;
    let entries = state.db.get_entries(&EntryFilter::default()).unwrap();
    let (base, server) = start_test_server(state).await;
    let client = client();
    for format in ["po", "xliff", "tmx"] {
        let response = get(&client, &base, format, &[]).await;
        assert_eq!(response.status(), 200);
        assert_eq!(
            response.headers()["content-disposition"],
            format!("attachment; filename=\"translation_es.{format}\"")
        );
        assert_eq!(
            response.bytes().await.unwrap().as_ref(),
            old_output(format, &entries).as_bytes(),
            "{format}: legacy bytes and row order must be unchanged"
        );
    }
    server.abort();
}

#[tokio::test]
async fn exact_file_intersects_the_status_union_without_reordering_or_duplicates() {
    let state = seeded_state().await;
    let all = state.db.get_entries(&EntryFilter::default()).unwrap();
    let (base, server) = start_test_server(state).await;
    let client = client();
    for (scope, ids) in [
        (vec![("file", FILE)], vec!["a", "b", "c", "d", "z"]),
        (
            vec![
                ("file", FILE),
                ("status", "translated"),
                ("status", "reviewed"),
                ("status", "translated"),
            ],
            vec!["c", "z"],
        ),
        (
            vec![("status", "translated"), ("status", "reviewed")],
            vec!["c", "e", "f", "g", "h", "z"],
        ),
        (
            vec![
                ("status", "pending"),
                ("status", "approved"),
                ("status", "error"),
            ],
            vec!["a", "b", "d"],
        ),
        (vec![("file", "scenarios\\Scene 1_日本語.rpy")], vec!["g"]),
        (vec![("file", "./scenarios/Scene 1_日本語.rpy")], vec!["h"]),
    ] {
        let expected: Vec<_> = all
            .iter()
            .filter(|row| ids.contains(&row.id.as_str()))
            .cloned()
            .collect();
        for format in ["po", "xliff", "tmx"] {
            let response = get(&client, &base, format, &scope).await;
            assert_eq!(response.status(), 200, "{format}: {scope:?}");
            assert_eq!(
                response.text().await.unwrap(),
                old_output(format, &expected),
                "{format}: {scope:?}"
            );
        }
    }
    server.abort();
}

#[tokio::test]
async fn all_explicit_statuses_are_also_byte_identical_to_the_old_output() {
    let state = seeded_state().await;
    let entries = state.db.get_entries(&EntryFilter::default()).unwrap();
    let (base, server) = start_test_server(state).await;
    let client = client();
    let scope =
        ["pending", "translated", "reviewed", "approved", "error"].map(|value| ("status", value));
    for format in ["po", "xliff", "tmx"] {
        let response = get(&client, &base, format, &scope).await;
        assert_eq!(response.status(), 200);
        assert_eq!(
            response.bytes().await.unwrap().as_ref(),
            old_output(format, &entries).as_bytes()
        );
    }
    server.abort();
}

#[tokio::test]
async fn unknown_status_returns_the_cli_valid_values_message() {
    let state = seeded_state().await;
    let (base, server) = start_test_server(state).await;
    let client = client();
    for status in ["bogus", "Translated", "translated,reviewed"] {
        for format in ["po", "xliff", "tmx"] {
            let response = get(&client, &base, format, &[("status", status)]).await;
            assert_eq!(response.status(), 400);
            assert_eq!(response.text().await.unwrap(), format!("invalid export status '{status}'; valid values: pending, translated, reviewed, approved, error"));
        }
    }
    server.abort();
}

#[tokio::test]
async fn unknown_files_and_empty_intersections_return_no_entries_to_export() {
    let state = seeded_state().await;
    let before =
        serde_json::to_value(state.db.get_entries(&EntryFilter::default()).unwrap()).unwrap();
    let (base, server) = start_test_server(state.clone()).await;
    let client = client();
    for scope in [
        vec![("file", "missing.rpy")],
        vec![("file", "scenarios/SCENE 1_日本語.rpy")],
        vec![("file", "scenarios/Scene 2.rpy"), ("status", "reviewed")],
        vec![("status", "")],
    ] {
        for format in ["po", "xliff", "tmx"] {
            let response = get(&client, &base, format, &scope).await;
            assert_eq!(response.status(), 400, "{scope:?}");
            assert_eq!(response.text().await.unwrap(), "no entries to export");
        }
    }
    assert_eq!(
        serde_json::to_value(state.db.get_entries(&EntryFilter::default()).unwrap()).unwrap(),
        before
    );
    server.abort();
}

#[tokio::test]
async fn an_empty_open_project_has_no_entries_to_export() {
    let state = create_test_state();
    *state.current_project.write().await = Some(ProjectInfo::default());
    let (base, server) = start_test_server(state).await;
    let client = client();
    for format in ["po", "xliff", "tmx"] {
        let response = get(&client, &base, format, &[]).await;
        assert_eq!(response.status(), 400);
        assert_eq!(response.text().await.unwrap(), "no entries to export");
    }
    server.abort();
}

#[tokio::test]
async fn filtered_exports_keep_project_admission_and_open_project_checks() {
    let state = seeded_state().await;
    let (base, server) = start_test_server(state.clone()).await;
    let client = client();
    let held = locust_server::try_project_operation(&state).unwrap();
    for format in ["po", "xliff", "tmx"] {
        let response = get(&client, &base, format, &[("file", FILE)]).await;
        assert_eq!(response.status(), 409);
        assert_eq!(
            response.text().await.unwrap(),
            locust_server::PROJECT_BUSY_MESSAGE
        );
    }
    drop(held);
    *state.current_project.write().await = None;
    for format in ["po", "xliff", "tmx"] {
        let response = get(&client, &base, format, &[("file", FILE)]).await;
        assert_eq!(response.status(), 400);
        assert_eq!(response.text().await.unwrap(), "no project open");
    }
    server.abort();
}

#[tokio::test]
async fn existing_file_status_counts_preview_the_same_scope_without_loading_strings() {
    let state = seeded_state().await;
    let (base, server) = start_test_server(state).await;
    let client = client();
    let stats: serde_json::Value = client
        .get(format!("{base}/api/stats/files"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    for (file, statuses) in [
        (
            None,
            vec!["pending", "translated", "reviewed", "approved", "error"],
        ),
        (Some(FILE), vec!["translated", "reviewed"]),
        (Some("scenarios/Scene 2.rpy"), vec!["reviewed"]),
    ] {
        let count: u64 = stats["files"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|row| file.is_none_or(|file| row["file_path"] == file))
            .map(|row| {
                statuses
                    .iter()
                    .map(|status| row[status].as_u64().unwrap())
                    .sum::<u64>()
            })
            .sum();
        let mut scope: Vec<_> = statuses.iter().map(|status| ("status", *status)).collect();
        if let Some(file) = file {
            scope.push(("file", file));
        }
        for format in ["po", "xliff", "tmx"] {
            let response = get(&client, &base, format, &scope).await;
            if count == 0 {
                assert_eq!(response.status(), 400);
            } else {
                assert_eq!(response.status(), 200);
                let text = response.text().await.unwrap();
                let actual = if format == "po" {
                    export::import_po(&text).unwrap().len()
                } else if format == "tmx" {
                    export::import_tmx(&text, "ja", "es").unwrap().len()
                } else {
                    export::import_xliff(&text).unwrap().len()
                };
                assert_eq!(actual as u64, count);
            }
        }
    }
    server.abort();
}

#[tokio::test]
async fn tmx_scoped_export_round_trips_exact_identities_languages_and_review_markers() {
    use locust_core::models::STALE_TRANSLATION_METADATA_KEY;
    use serde_json::json;

    let state = create_test_state();
    *state.current_project.write().await = Some(ProjectInfo::default());
    let exact_source = " \t日本語 & <same>\r\n\\V[7] e\u{301}  ";
    let mut rows = [
        (
            "id:\t日本語\r\n\"|",
            exact_source,
            " ¡Hola!\r\n<&> ",
            FILE,
            StringStatus::Translated,
        ),
        (
            "duplicate-source",
            exact_source,
            "Otro destino",
            FILE,
            StringStatus::Reviewed,
        ),
        (
            "stale",
            "Changed source",
            "Old target",
            FILE,
            StringStatus::Reviewed,
        ),
        (
            "excluded-status",
            "Approved",
            "Aprobada",
            FILE,
            StringStatus::Approved,
        ),
        (
            "excluded-file",
            "Other file",
            "Otra",
            "other.rpy",
            StringStatus::Translated,
        ),
    ]
    .into_iter()
    .map(|(id, source, target, file, status)| {
        let mut row = StringEntry::new(id, source, file.into());
        row.translation = Some(target.into());
        row.status = status;
        row
    })
    .collect::<Vec<_>>();
    rows[2]
        .metadata
        .insert(STALE_TRANSLATION_METADATA_KEY.into(), json!(false));
    state.db.save_entries(&rows).unwrap();
    let before =
        serde_json::to_value(state.db.get_entries(&EntryFilter::default()).unwrap()).unwrap();
    let (base, server) = start_test_server(state.clone()).await;
    let client = client();
    let response = client
        .get(format!("{base}/api/export/tmx"))
        .query(&[
            ("lang", "es-MX"),
            ("source_lang", "en-US"),
            ("file", FILE),
            ("status", "reviewed"),
            ("status", "translated"),
            ("status", "reviewed"),
        ])
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(
        response.headers()["content-type"],
        "application/xml; charset=utf-8"
    );
    assert_eq!(
        response.headers()["content-disposition"],
        "attachment; filename=\"translation_es-MX.tmx\""
    );
    let content = response.text().await.unwrap();
    assert!(content.contains("srclang=\"en-US\""));
    assert!(content.contains("xml:lang=\"es-MX\""));
    assert!(export::import_tmx(&content, "en", "es-MX").is_err());
    assert!(export::import_tmx(&content, "en-US", "es").is_err());
    let units = export::import_tmx(&content, "en-US", "es-MX").unwrap();
    assert_eq!(units.len(), 3);
    for row in &rows[..3] {
        let unit = units.iter().find(|unit| unit.id == row.id).unwrap();
        assert_eq!(unit.source, row.source);
        assert_eq!(unit.target, row.translation.as_deref().unwrap());
        assert_eq!(unit.needs_review, row.id == "stale");
    }
    assert_eq!(
        serde_json::to_value(state.db.get_entries(&EntryFilter::default()).unwrap()).unwrap(),
        before
    );

    // Submit this actual exported catalog through the existing preview/apply API.
    let destination = create_test_state();
    *destination.current_project.write().await = Some(ProjectInfo::default());
    let originals = rows.clone();
    for row in &mut rows[..2] {
        row.translation = None;
        row.status = StringStatus::Pending;
    }
    destination.db.save_entries(&rows).unwrap();
    let before =
        serde_json::to_value(destination.db.get_entries(&EntryFilter::default()).unwrap()).unwrap();
    let (import_base, import_server) = start_test_server(destination.clone()).await;
    let response = client
        .post(format!("{import_base}/api/import/tmx/preview"))
        .json(&json!({ "content": content, "keep_existing": true }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let preview: serde_json::Value = response.json().await.unwrap();
    assert_eq!(preview["fill"], 2);
    assert_eq!(preview["skipped"], 1);
    assert_eq!(
        serde_json::to_value(destination.db.get_entries(&EntryFilter::default()).unwrap()).unwrap(),
        before
    );
    let response = client.post(format!("{import_base}/api/import/tmx/apply"))
        .json(&json!({ "content": content, "keep_existing": true, "plan_token": preview["plan_token"] }))
        .send().await.unwrap();
    assert_eq!(response.status(), 200);
    let applied: serde_json::Value = response.json().await.unwrap();
    assert_eq!(applied["imported"], 2);
    assert_eq!(applied["skipped"], 1);
    for original in &originals[..2] {
        let saved = destination.db.get_entry(&original.id).unwrap().unwrap();
        assert_eq!(saved.id, original.id);
        assert_eq!(saved.source, original.source);
        assert_eq!(saved.translation, original.translation);
        assert_eq!(saved.status, StringStatus::Translated);
        assert_eq!(saved.provider_used.as_deref(), Some("import"));
    }
    for original in &originals[2..] {
        assert_eq!(
            serde_json::to_value(destination.db.get_entry(&original.id).unwrap().unwrap()).unwrap(),
            serde_json::to_value(original).unwrap()
        );
    }
    server.abort();
    import_server.abort();
}

#[tokio::test]
async fn tmx_requires_both_explicit_languages_and_rejects_invalid_pairs() {
    let state = seeded_state().await;
    let before =
        serde_json::to_value(state.db.get_entries(&EntryFilter::default()).unwrap()).unwrap();
    let (base, server) = start_test_server(state.clone()).await;
    let client = client();
    for (params, message) in [
        (vec![("lang", "es")], "missing field `source_lang`"),
        (vec![("source_lang", "ja")], "missing field `lang`"),
        (
            vec![("lang", "es"), ("source_lang", "")],
            "concrete ASCII language tag",
        ),
        (
            vec![("lang", "es"), ("source_lang", "und")],
            "concrete ASCII language tag",
        ),
        (
            vec![("lang", "es"), ("source_lang", "ES")],
            "languages must differ",
        ),
        (
            vec![("lang", "bad_tag"), ("source_lang", "en")],
            "concrete ASCII language tag",
        ),
    ] {
        let response = client
            .get(format!("{base}/api/export/tmx"))
            .query(&params)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 400);
        assert!(response.headers().get("content-disposition").is_none());
        assert!(response.text().await.unwrap().contains(message));
    }
    assert_eq!(
        serde_json::to_value(state.db.get_entries(&EntryFilter::default()).unwrap()).unwrap(),
        before
    );
    server.abort();
}

#[tokio::test]
async fn tmx_invalid_and_oversized_output_is_refused_before_a_download() {
    let state = create_test_state();
    *state.current_project.write().await = Some(ProjectInfo::default());
    let (base, server) = start_test_server(state.clone()).await;
    let client = client();
    // Each field is legal and within 1 MiB, but XML escaping exceeds 32 MiB.
    let big: Vec<_> = (0..7)
        .map(|id| StringEntry::new(id.to_string(), "&".repeat(1024 * 1024), FILE.into()))
        .collect();
    let mut invalid = StringEntry::new("invalid", "illegal \u{1} XML", "invalid.rpy".into());
    state.db.save_entries(&big).unwrap();
    state.db.save_entries(&[invalid.clone()]).unwrap();
    for file in [FILE, "invalid.rpy"] {
        let response = get(&client, &base, "tmx", &[("file", file)]).await;
        assert_eq!(response.status(), 400);
        assert!(response.headers().get("content-disposition").is_none());
        let message = response.text().await.unwrap();
        assert!(
            message.contains(if file == FILE {
                "document exceeds 32 MiB"
            } else {
                "illegal XML 1.0 character"
            }),
            "{message}"
        );
    }
    invalid.source = "x".repeat(1024 * 1024 + 1);
    state.db.save_entries(&[invalid]).unwrap();
    let response = get(&client, &base, "tmx", &[("file", "invalid.rpy")]).await;
    assert_eq!(response.status(), 400);
    assert!(response
        .text()
        .await
        .unwrap()
        .contains("decoded field exceeds byte limit"));
    server.abort();
}

#[tokio::test]
async fn source_language_suggestion_uses_project_history_then_configuration_and_keeps_admission() {
    use locust_core::database::TranslationRun;
    use serde_json::json;
    let state = seeded_state().await;
    let (base, server) = start_test_server(state.clone()).await;
    let client = client();
    let url = format!("{base}/api/export/source-language");
    let response = client
        .get(&url)
        .query(&[("lang", "es")])
        .send()
        .await
        .unwrap();
    assert_eq!(
        response.json::<serde_json::Value>().await.unwrap(),
        json!({ "source_lang": "ja" })
    );
    let run: TranslationRun = serde_json::from_value(json!({
        "started_at": "2026-10-28T10:00:00Z", "duration_secs": 0, "provider": "import",
        "source_lang": "en-US", "target_lang": "es", "strings_translated": 1,
        "tokens_used": 0, "input_tokens": 0, "output_tokens": 0, "cost_usd": 0
    }))
    .unwrap();
    state.db.record_translation_run(&run).await.unwrap();
    state
        .db
        .record_translation_run(&TranslationRun {
            source_lang: "ko".into(),
            target_lang: "fr".into(),
            started_at: "2026-10-28T11:00:00Z".into(),
            ..run
        })
        .await
        .unwrap();
    for (target, source) in [("ES", "en-US"), ("de", "ko")] {
        let response = client
            .get(&url)
            .query(&[("lang", target)])
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        assert_eq!(
            response.json::<serde_json::Value>().await.unwrap(),
            json!({ "source_lang": source })
        );
    }
    let held = locust_server::try_project_operation(&state).unwrap();
    assert_eq!(
        client
            .get(&url)
            .query(&[("lang", "es")])
            .send()
            .await
            .unwrap()
            .status(),
        409
    );
    drop(held);
    *state.current_project.write().await = None;
    assert_eq!(
        client
            .get(&url)
            .query(&[("lang", "es")])
            .send()
            .await
            .unwrap()
            .status(),
        400
    );
    server.abort();
}
