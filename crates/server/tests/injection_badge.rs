use serde_json::{json, Value};

#[tokio::test]
async fn c139_patch_status_exposes_injections_and_preserves_patch_fields() {
    let temp = tempfile::tempdir().unwrap();
    let game = temp.path().join("game");
    std::fs::create_dir(&game).unwrap();
    std::fs::write(game.join("story.html"), "<p>Original</p>").unwrap();
    let state = locust_server::create_test_state();
    let (url, server) = locust_server::start_test_server(state.clone()).await;
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    let get_status = || async {
        let response = client
            .post(format!("{url}/api/patch/status"))
            .json(&json!({"game_path": game}))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        response.json::<Value>().await.unwrap()
    };
    let clean = get_status().await;
    assert_eq!(clean["status"], "not_patched");
    assert_eq!(clean["injections"], json!([]));
    assert_eq!(clean["injection_pending"], false);
    let response = client
        .post(format!("{url}/api/project/open"))
        .json(&json!({"path":game,"format_id":"html-game"}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let mut entries = state.db.get_entries(&Default::default()).unwrap();
    entries[0].translation = Some("Traducción".into());
    state.db.save_entries(&entries).unwrap();
    let response = client
        .post(format!("{url}/api/inject"))
        .json(
            &json!({"project_path":game,"format_id":"html-game","direct":true,"languages":["es"]}),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let report: Value = response.json().await.unwrap();
    let applied = get_status().await;
    assert_eq!(applied["status"], "not_patched");
    assert_eq!(applied["injections"][0]["mode"], "direct");
    assert_eq!(applied["injections"][0]["language"], "es");
    assert!(applied["injections"][0]["applied_at"].is_string());
    let receipt = locust_core::patch::Receipt {
        schema_version: 1,
        patch_id: "zip".into(),
        patch_version: "1".into(),
        generator_version: "test".into(),
        language: "de".into(),
        engine: "fixture".into(),
        applied_at: "2026-10-06T00:00:00Z".into(),
        verification: locust_core::patch::manifest::VerificationTier::Strict,
        forced: false,
        baseline: locust_core::patch::BackupBaseline::Pristine,
        created_dirs: vec![],
        replaced: vec![],
        added: vec![],
    };
    let store = locust_core::patch::PatchStore::new(&game);
    store.write_receipt(&receipt).unwrap();
    // Read-only status works even while another operation holds the lock.
    {
        let _lock = locust_core::patch::GameLock::acquire(&game).unwrap();
        let both = get_status().await;
        assert_eq!(both["status"], "patched");
        assert_eq!(both["patch_id"], "zip");
        assert_eq!(both["patch_version"], "1");
        assert_eq!(both["language"], "de");
        assert_eq!(both["replaced"], 0);
        assert_eq!(both["added"], 0);
        assert_eq!(both["forced"], false);
        assert_eq!(both["baseline"], "Pristine");
        assert_eq!(both["engine"], "fixture");
        assert_eq!(both["applied_at"], receipt.applied_at);
        assert_eq!(both["injections"], applied["injections"]);
    }
    let id = report["backup_id"].as_str().unwrap();
    let response = client
        .post(format!("{url}/api/backups/{id}/restore"))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(get_status().await["injections"], json!([]));
    server.abort();
}
