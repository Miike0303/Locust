use serde_json::{json, Value};

#[tokio::test]
async fn injection_backup_packs_strictly_for_directory_and_single_file_projects() {
    for single_file in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let game = directory.path().join("game");
        std::fs::create_dir(&game).unwrap();
        let source = game.join("story.html");
        let original = "<p>Original 日本語</p>";
        std::fs::write(&source, original).unwrap();
        let selected = if single_file { &source } else { &game };
        let state = locust_server::create_test_state();
        let (url, handle) = locust_server::start_test_server(state.clone()).await;
        let client = reqwest::Client::builder().no_proxy().build().unwrap();
        let response = client
            .post(format!("{url}/api/project/open"))
            .json(&json!({"path":selected,"format_id":"html-game"}))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        let entries = state.db.get_entries(&Default::default()).unwrap();
        assert_eq!(entries.len(), 1);
        let mut entry_url = reqwest::Url::parse(&format!("{url}/api/strings/")).unwrap();
        entry_url
            .path_segments_mut()
            .unwrap()
            .pop_if_empty()
            .push(&entries[0].id);
        let response = client
            .patch(entry_url)
            .json(&json!({"translation":"Traducción 日本語"}))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        let response = client.post(format!("{url}/api/inject"))
            .json(&json!({"project_path":selected,"format_id":"html-game","direct":true,"languages":["es"]})).send().await.unwrap();
        assert_eq!(response.status(), 200);
        let injected: Value = response.json().await.unwrap();
        let id = injected["backup_id"].as_str().unwrap();
        let backup = std::path::Path::new(injected["backup_path"].as_str().unwrap());
        let refused = client
            .delete(format!("{url}/api/backups/{id}"))
            .send()
            .await
            .unwrap();
        assert_eq!(refused.status(), 409);
        assert!(refused.headers()["content-type"]
            .to_str()
            .unwrap()
            .starts_with("text/plain"));
        let message = refused.text().await.unwrap();
        assert!(message.contains(&format!("backup {id} is needed to pack")));
        assert!(message.contains("recorded for es"));
        assert!(backup.is_dir());

        let unrecorded = state.backup_manager.create_backup(selected).unwrap();
        let unrecorded_path = state.backup_manager.root().join(&unrecorded.id);
        assert!(unrecorded_path.is_dir());
        let deleted = client
            .delete(format!("{url}/api/backups/{}", unrecorded.id))
            .send()
            .await
            .unwrap();
        assert_eq!(deleted.status(), 204);
        assert!(!unrecorded_path.exists());
        assert!(backup.is_dir());

        // The refused deletion must leave the recorded original usable by pack.
        let output = directory.path().join("translation.zip");
        let request = json!({"game_path":selected,"output_path":output,"languages":["es"],"pristine":true,"pristine_backup_id":id});
        let response = client
            .post(format!("{url}/api/patch/pack"))
            .json(&request)
            .send()
            .await
            .unwrap();
        let status = response.status();
        let packed: Value = response.json().await.unwrap();
        assert_eq!(status, 200, "{packed}");
        assert_eq!(packed["files_packed"], 1);
        let target = directory.path().join("target");
        std::fs::create_dir(&target).unwrap();
        std::fs::write(target.join("story.html"), original).unwrap();
        let verified = client
            .post(format!("{url}/api/patch/verify"))
            .json(&json!({"game_path":target,"zip_path":output}))
            .send()
            .await
            .unwrap();
        assert_eq!(verified.status(), 200);
        let verified: Value = verified.json().await.unwrap();
        assert_eq!(verified["tier"], "Strict");
        assert_eq!(verified["outcome"], "Clean");
        assert_eq!(verified["conflicts"], json!([]));
        assert_eq!(
            std::fs::read_to_string(target.join("story.html")).unwrap(),
            original
        );
        // Output may not destroy an original, its manifest, or create files in backup storage.
        let manifest = std::fs::read(backup.join("manifest.json")).unwrap();
        for forbidden in [
            backup.join("manifest.json"),
            backup.join("new/patch.zip"),
            source.clone(),
        ] {
            let mut forbidden_request = request.clone();
            forbidden_request["output_path"] = json!(forbidden);
            let refused = client
                .post(format!("{url}/api/patch/pack"))
                .json(&forbidden_request)
                .send()
                .await
                .unwrap();
            assert_eq!(refused.status(), 400);
            assert_eq!(
                std::fs::read(backup.join("manifest.json")).unwrap(),
                manifest
            );
            assert!(!backup.join("new").exists());
        }
        // A modified saved original must fail before replacing an existing zip.
        let old_zip = std::fs::read(&output).unwrap();
        let payload = std::path::Path::new(injected["backup_path"].as_str().unwrap())
            .join("payload")
            .join(if single_file { "file" } else { "story.html" });
        std::fs::write(payload, "corrupted").unwrap();
        let refused = client
            .post(format!("{url}/api/patch/pack"))
            .json(&request)
            .send()
            .await
            .unwrap();
        assert_eq!(refused.status(), 400);
        assert_eq!(std::fs::read(&output).unwrap(), old_zip);
        assert!(std::fs::read_to_string(&source)
            .unwrap()
            .contains("Traducción"));
        handle.abort();
    }
}

#[tokio::test]
async fn backup_deletion_respects_recorded_store_and_default_language() {
    for store in ["same", "legacy", "different"] {
        let directory = tempfile::tempdir().unwrap();
        let game = directory.path().join("game");
        std::fs::create_dir(&game).unwrap();
        let source = game.join("story.html");
        std::fs::write(&source, "<p>Hello player</p>").unwrap();
        let state = locust_server::create_test_state();
        let backup = state.backup_manager.create_backup(&game).unwrap();
        let backup_path = state.backup_manager.root().join(&backup.id);
        let storage_root = match store {
            // Exercise equivalent path spellings (including Windows verbatim paths).
            "same" => Some(std::fs::canonicalize(state.backup_manager.root()).unwrap()),
            "legacy" => None,
            "different" => {
                let other = directory.path().join("other-backups");
                std::fs::create_dir(&other).unwrap();
                Some(other)
            }
            _ => unreachable!(),
        };
        state
            .db
            .record_injection_with_backup(
                None,
                &game,
                &[source],
                Some(&locust_core::database::RecordedBackup {
                    id: backup.id.clone(),
                    source_path: game.clone(),
                    storage_root,
                }),
            )
            .unwrap();
        let (url, handle) = locust_server::start_test_server(state).await;
        let client = reqwest::Client::builder().no_proxy().build().unwrap();
        let response = client
            .delete(format!("{url}/api/backups/{}", backup.id))
            .send()
            .await
            .unwrap();
        if store == "different" {
            assert_eq!(response.status(), 204);
            assert!(!backup_path.exists());
        } else {
            assert_eq!(response.status(), 409, "store: {store}");
            assert!(response
                .text()
                .await
                .unwrap()
                .contains("recorded for default"));
            assert!(backup_path.is_dir());
        }
        handle.abort();
    }
}

#[tokio::test]
async fn repeated_injection_keeps_the_original_pack_baseline() {
    for single_file in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let game = directory.path().join("game");
        std::fs::create_dir(&game).unwrap();
        let source = game.join("story.html");
        let original = "<p>Hello player</p>";
        std::fs::write(&source, original).unwrap();
        let selected = if single_file { &source } else { &game };
        let state = locust_server::create_test_state();
        let (url, handle) = locust_server::start_test_server(state.clone()).await;
        let client = reqwest::Client::builder().no_proxy().build().unwrap();
        let opened = client
            .post(format!("{url}/api/project/open"))
            .json(&json!({"path":selected,"format_id":"html-game"}))
            .send()
            .await
            .unwrap();
        assert_eq!(opened.status(), 200);
        let mut entries = state.db.get_entries(&Default::default()).unwrap();
        entries[0].translation = Some("Hola jugador".into());
        entries[0].status = locust_core::models::StringStatus::Translated;
        state.db.save_entries(&entries).unwrap();
        let request = json!({"project_path":selected,"format_id":"html-game","direct":true,"languages":["es"]});
        let first = client
            .post(format!("{url}/api/inject"))
            .json(&request)
            .send()
            .await
            .unwrap();
        assert_eq!(first.status(), 200);
        let first: Value = first.json().await.unwrap();
        assert_eq!(first["strings_written"], 1);
        let repeated = client
            .post(format!("{url}/api/inject"))
            .json(&request)
            .send()
            .await
            .unwrap();
        assert_eq!(repeated.status(), 200);
        let mut repeated: Value = repeated.json().await.unwrap();
        assert_eq!(repeated["strings_written"], 0);
        for _ in 0..4 {
            let response = client
                .post(format!("{url}/api/inject"))
                .json(&request)
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), 200);
            repeated = response.json().await.unwrap();
            assert_eq!(repeated["strings_written"], 0);
        }
        assert_eq!(repeated["pristine_backup_id"], first["backup_id"]);
        assert_eq!(repeated["backup_id"], first["backup_id"]);
        assert_eq!(repeated["backup_path"], first["backup_path"]);
        assert_eq!(state.backup_manager.list_backups().unwrap().len(), 1);
        assert!(std::path::Path::new(first["backup_path"].as_str().unwrap()).is_dir());
        let pack_backup = repeated
            .get("pristine_backup_id")
            .unwrap_or(&repeated["backup_id"]);
        let output = directory.path().join("patch.zip");
        let packed = client.post(format!("{url}/api/patch/pack"))
            .json(&json!({"game_path":selected,"output_path":output,"languages":["es"],"pristine":true,"pristine_backup_id":pack_backup}))
            .send().await.unwrap();
        assert_eq!(packed.status(), 200, "{}", packed.text().await.unwrap());
        let target = directory.path().join("target");
        std::fs::create_dir(&target).unwrap();
        std::fs::write(target.join("story.html"), original).unwrap();
        let verified: Value = client
            .post(format!("{url}/api/patch/verify"))
            .json(&json!({"game_path":target,"zip_path":output}))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(
            verified["manifest"]["files"][0]["original_sha256"],
            locust_core::database::sha256_hex(original.as_bytes()),
            "must retain factory original hash after a no-op: {verified}"
        );
        assert_eq!(
            verified["outcome"], "Clean",
            "must still apply to the original after a no-op injection: {verified}"
        );
        assert_eq!(
            verified["conflicts"],
            json!([]),
            "must still apply to the original after a no-op injection: {verified}"
        );
        handle.abort();
        let database_path = state.db.path();
        // A different profile/store must recover the original backup from the recording.
        let restarted = locust_server::create_test_state();
        let (restart_url, restart_handle) = locust_server::start_test_server(restarted).await;
        let opened = client.post(format!("{restart_url}/api/project/open-db"))
            .json(&json!({"database_path":database_path,"game_path":selected,"format_id":"html-game"}))
            .send().await.unwrap();
        assert_eq!(opened.status(), 200);
        let cold_request =
            json!({"game_path":selected,"output_path":output,"languages":["es"],"pristine":true});
        let packed = client
            .post(format!("{restart_url}/api/patch/pack"))
            .json(&cold_request)
            .send()
            .await
            .unwrap();
        assert_eq!(packed.status(), 200, "{}", packed.text().await.unwrap());
        let verified: Value = client
            .post(format!("{restart_url}/api/patch/verify"))
            .json(&json!({"game_path":target,"zip_path":output}))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(verified["outcome"], "Clean", "{verified}");
        let saved_zip = std::fs::read(&output).unwrap();
        let mut wrong_backup = cold_request.clone();
        let wrong_generation = state.backup_manager.create_backup(selected).unwrap();
        wrong_backup["pristine_backup_id"] = json!(wrong_generation.id);
        let refused = client
            .post(format!("{restart_url}/api/patch/pack"))
            .json(&wrong_backup)
            .send()
            .await
            .unwrap();
        assert_eq!(refused.status(), 400);
        assert_eq!(std::fs::read(&output).unwrap(), saved_zip);
        // A removed recorded original must fail, never guess one of the newer backups.
        state
            .backup_manager
            .delete_backup(first["backup_id"].as_str().unwrap())
            .unwrap();
        let refused = client
            .post(format!("{restart_url}/api/patch/pack"))
            .json(&cold_request)
            .send()
            .await
            .unwrap();
        assert_eq!(refused.status(), 400);
        assert_eq!(std::fs::read(&output).unwrap(), saved_zip);
        restart_handle.abort();
    }
}

#[tokio::test]
async fn first_identity_injection_reports_no_retained_backup_or_recording() {
    for single_file in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let game = directory.path().join("game");
        std::fs::create_dir(&game).unwrap();
        let source = game.join("story.html");
        let original = "<p>Hello player</p>";
        std::fs::write(&source, original).unwrap();
        let selected = if single_file { &source } else { &game };
        let state = locust_server::create_test_state();
        let (url, handle) = locust_server::start_test_server(state.clone()).await;
        let client = reqwest::Client::builder().no_proxy().build().unwrap();
        let opened = client
            .post(format!("{url}/api/project/open"))
            .json(&json!({"path": selected, "format_id":"html-game"}))
            .send()
            .await
            .unwrap();
        assert_eq!(opened.status(), 200);
        let mut entries = state.db.get_entries(&Default::default()).unwrap();
        entries[0].translation = Some(entries[0].source.clone());
        entries[0].status = locust_core::models::StringStatus::Translated;
        state.db.save_entries(&entries).unwrap();
        let response = client.post(format!("{url}/api/inject"))
            .json(&json!({"project_path": selected, "format_id":"html-game", "direct":true, "languages":["es"]})).send().await.unwrap();
        let status = response.status();
        let report: Value = response.json().await.unwrap();
        assert_eq!(status, 200, "{report}");
        assert_eq!(report["files_modified"], 0);
        assert_eq!(report["backup_id"], "");
        assert!(report["backup_path"].is_null());
        assert!(report["pristine_backup_id"].is_null());
        assert!(state.backup_manager.list_backups().unwrap().is_empty());
        assert!(state.db.get_injection(Some("es")).unwrap().is_none());
        assert_eq!(std::fs::read_to_string(&source).unwrap(), original);
        handle.abort();
    }
}
