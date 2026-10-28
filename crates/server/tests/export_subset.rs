//! Review assignments must select the same ordered rows in PO and XLIFF.
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
    client
        .get(format!("{base}/api/export/{format}"))
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
        _ => unreachable!(),
    }
}

#[tokio::test]
async fn no_filters_are_byte_identical_to_the_old_unfiltered_export() {
    let state = seeded_state().await;
    let entries = state.db.get_entries(&EntryFilter::default()).unwrap();
    let (base, server) = start_test_server(state).await;
    let client = client();
    for format in ["po", "xliff"] {
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
        for format in ["po", "xliff"] {
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
    for format in ["po", "xliff"] {
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
        for format in ["po", "xliff"] {
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
        for format in ["po", "xliff"] {
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
    for format in ["po", "xliff"] {
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
    for format in ["po", "xliff"] {
        let response = get(&client, &base, format, &[("file", FILE)]).await;
        assert_eq!(response.status(), 409);
        assert_eq!(
            response.text().await.unwrap(),
            locust_server::PROJECT_BUSY_MESSAGE
        );
    }
    drop(held);
    *state.current_project.write().await = None;
    for format in ["po", "xliff"] {
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
        for format in ["po", "xliff"] {
            let response = get(&client, &base, format, &scope).await;
            if count == 0 {
                assert_eq!(response.status(), 400);
            } else {
                assert_eq!(response.status(), 200);
                let text = response.text().await.unwrap();
                let actual = if format == "po" {
                    export::import_po(&text).unwrap().len()
                } else {
                    export::import_xliff(&text).unwrap().len()
                };
                assert_eq!(actual as u64, count);
            }
        }
    }
    server.abort();
}
