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
