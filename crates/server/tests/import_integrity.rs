use locust_core::models::{StringEntry, StringStatus};
use serde_json::Value;

#[tokio::test]
async fn http_import_reports_stale_sources_and_keeps_current_reviewed_translation() {
    let state = locust_server::create_test_state();
    let mut old = StringEntry::new("stale", "Current source", "story.html".into());
    old.translation = Some("Keep reviewed translation".into());
    old.status = StringStatus::Reviewed;
    let fresh = StringEntry::new("fresh", "Fresh source", "story.html".into());
    state.db.save_entries(&[old, fresh]).unwrap();
    *state.current_project.write().await = Some(locust_server::ProjectInfo {
        path: "story.html".into(),
        format_id: "html-game".into(),
        name: "story".into(),
        ..Default::default()
    });
    let (url, handle) = locust_server::start_test_server(state.clone()).await;
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    let po = "msgctxt \"stale\"\nmsgid \"Old source\"\nmsgstr \"Outdated translation\"\n\nmsgctxt \"fresh\"\nmsgid \"Fresh source\"\nmsgstr \"Traducción\"\n";
    let xliff = r#"<x:xliff xmlns:x="urn:oasis:names:tc:xliff:document:1.2" version="1.2"><x:file><x:body>
        <x:trans-unit id="stale"><x:source>Old source</x:source><x:target>Outdated translation</x:target></x:trans-unit>
        <x:trans-unit id="fresh"><x:source>Fresh source</x:source><x:target>Traducción</x:target></x:trans-unit>
    </x:body></x:file></x:xliff>"#;
    for (format, content) in [("po", po), ("xliff", xliff)] {
        let response = client
            .post(format!("{url}/api/import/{format}"))
            .header("content-type", "text/plain")
            .body(content)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        let counts: Value = response.json().await.unwrap();
        assert_eq!(counts["imported"], 1);
        assert_eq!(counts["skipped"], 1);
        assert_eq!(counts["stale_sources"], 1);
        assert_eq!(counts["unknown_ids"], 0);
        let old = state.db.get_entry("stale").unwrap().unwrap();
        assert_eq!(
            old.translation.as_deref(),
            Some("Keep reviewed translation")
        );
        assert_eq!(old.status, StringStatus::Reviewed);
        assert_eq!(
            state
                .db
                .get_entry("fresh")
                .unwrap()
                .unwrap()
                .translation
                .as_deref(),
            Some("Traducción")
        );
    }
    handle.abort();
}

#[tokio::test]
async fn http_import_po_continued_context_updates_only_line10() {
    let state = locust_server::create_test_state();
    let mut line1 = StringEntry::new("line1", "Same source", "game.rpy".into());
    line1.translation = Some("Keep line1".into());
    line1.status = StringStatus::Reviewed;
    line1.provider_used = Some("line1-provider".into());
    let mut line10 = StringEntry::new("line10", "Same source", "game.rpy".into());
    line10.translation = Some("Replace line10".into());
    line10.status = StringStatus::Reviewed;
    line10.provider_used = Some("line10-provider".into());
    state.db.save_entries(&[line1.clone(), line10]).unwrap();
    *state.current_project.write().await = Some(locust_server::ProjectInfo {
        path: "game.rpy".into(),
        format_id: "renpy".into(),
        name: "game".into(),
        ..Default::default()
    });
    let (url, handle) = locust_server::start_test_server(state.clone()).await;
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    let response = client
        .post(format!("{url}/api/import/po"))
        .header("content-type", "text/plain")
        .body("msgctxt \"line1\"\n\"0\"\nmsgid \"Same source\"\nmsgstr \"Nueva traducción\"\n")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let counts: Value = response.json().await.unwrap();
    assert_eq!(counts["imported"], 1);
    assert_eq!(counts["skipped"], 0);
    assert_eq!(counts["stale_sources"], 0);
    assert_eq!(counts["unknown_ids"], 0);
    let unchanged = state.db.get_entry("line1").unwrap().unwrap();
    assert_eq!(
        unchanged.translation, line1.translation,
        "line1 must stay unchanged"
    );
    assert_eq!(unchanged.status, line1.status);
    assert_eq!(unchanged.provider_used, line1.provider_used);
    let updated = state.db.get_entry("line10").unwrap().unwrap();
    assert_eq!(updated.translation.as_deref(), Some("Nueva traducción"));
    assert_eq!(updated.status, StringStatus::Translated);
    assert_eq!(updated.provider_used.as_deref(), Some("import"));
    handle.abort();
}

#[tokio::test]
async fn http_import_po_malformed_string_rejects_entire_catalog() {
    let state = locust_server::create_test_state();
    let mut first = StringEntry::new("line1", "First source", "game.rpy".into());
    first.translation = Some("Keep first".into());
    first.status = StringStatus::Reviewed;
    first.provider_used = Some("first-provider".into());
    let mut second = StringEntry::new("line10", "Second source", "game.rpy".into());
    second.translation = Some("Keep second".into());
    second.status = StringStatus::Reviewed;
    second.provider_used = Some("second-provider".into());
    let originals = [first, second];
    state.db.save_entries(&originals).unwrap();
    *state.current_project.write().await = Some(locust_server::ProjectInfo {
        path: "game.rpy".into(),
        format_id: "renpy".into(),
        name: "game".into(),
        ..Default::default()
    });
    let (url, handle) = locust_server::start_test_server(state.clone()).await;
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    let po = concat!(
        "msgctxt \"line1\"\nmsgid \"First source\"\nmsgstr \"Valid replacement\"\n\n",
        "msgctxt \"line10\"\nmsgid \"Second source\"\nmsgstr garbage\n",
    );
    let response = client
        .post(format!("{url}/api/import/po"))
        .header("content-type", "text/plain")
        .body(po)
        .send()
        .await
        .unwrap();
    let status = response.status();
    let error = response.text().await.unwrap();
    assert_eq!(status, 400, "{error}");
    assert!(error.contains("line 7:"), "{error}");
    assert!(error.contains("malformed PO string"), "{error}");
    for original in originals {
        let after = state.db.get_entry(&original.id).unwrap().unwrap();
        assert_eq!(after.translation, original.translation, "{}", original.id);
        assert_eq!(after.status, original.status, "{}", original.id);
        assert_eq!(
            after.provider_used, original.provider_used,
            "{}",
            original.id
        );
    }
    handle.abort();
}
