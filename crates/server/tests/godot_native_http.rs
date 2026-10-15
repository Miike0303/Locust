mod common;

use locust_core::database::paths_identical;
use serde_json::{json, Value};
use std::io::Cursor;

#[tokio::test]
async fn http_godot_list_and_modes_are_experimental_replace() {
    let state = locust_server::create_test_state();
    let (base, server) = common::start_test_server(locust_server::create_router(state)).await;
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    let formats: Value = client
        .get(format!("{base}/api/formats"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let godot = formats
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["id"] == "godot-dialogic")
        .unwrap();
    assert_eq!(godot["stability"], "experimental");
    assert_eq!(godot["extensions"], json!(["pck"]));
    assert_eq!(godot["supported_modes"], json!(["replace"]));
    let modes: Value = client
        .get(format!("{base}/api/formats/godot-dialogic/modes"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(modes["supported_modes"], json!(["replace"]));
    server.shutdown().await;
}

#[tokio::test]
async fn http_godot_open_inject_pack_uses_recorded_root_and_project_label() {
    for direct in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let shared = temp.path().join("shared");
        std::fs::create_dir(&shared).unwrap();
        let game = shared.join("game.pck");
        let mut header = vec![0; 100];
        header[..4].copy_from_slice(b"GDPC");
        header[4..8].copy_from_slice(&2u32.to_le_bytes());
        header[8..12].copy_from_slice(&4u32.to_le_bytes());
        let mut source = Cursor::new(header);
        let index = locust_formats::godot_pck::read_index(&mut source).unwrap();
        let mut original = Vec::new();
        locust_formats::godot_pck::rebuild(
            &mut source,
            &index,
            &[],
            &[
                ("res://main.dtl", b"Hello\n"),
                ("res://unsupported.dtl", b"\\if escaped\n"),
            ],
            &mut original,
        )
        .unwrap();
        std::fs::write(&game, &original).unwrap();
        std::fs::write(shared.join("index.html"), "<p>Unrelated sibling game</p>").unwrap();
        // Parent redetection would incorrectly label Direct as html-game.
        assert_eq!(
            locust_formats::default_registry()
                .detect(&shared)
                .unwrap()
                .id(),
            "html-game"
        );
        let state = locust_server::create_test_state();
        let (base, server) =
            common::start_test_server(locust_server::create_router(state.clone())).await;
        let client = reqwest::Client::builder().no_proxy().build().unwrap();
        let response = client
            .post(format!("{base}/api/project/open"))
            .json(&json!({"path":game}))
            .send()
            .await
            .unwrap();
        let status = response.status();
        let body = response.text().await.unwrap();
        assert_eq!(status, 200, "{body}");
        let opened: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(opened["format_id"], "godot-dialogic");
        assert_eq!(opened["supported_modes"], json!(["replace"]));
        assert!(opened["extraction_warnings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|warning| warning.as_str().unwrap().contains("escaped event prefix")));
        assert!(paths_identical(
            std::path::Path::new(opened["project_path"].as_str().unwrap()),
            &game
        ));
        let mut rows = state.db.get_entries(&Default::default()).unwrap();
        assert_eq!(rows.len(), 1);
        rows[0].translation = Some("Hola, una traduccion mas larga".into());
        rows[0].status = locust_core::models::StringStatus::Approved;
        state.db.save_entries(&rows).unwrap();
        let response = client.post(format!("{base}/api/inject")).json(&json!({"project_path":game,"format_id":"godot-dialogic","direct":direct,"mode":"replace","languages":["es"],"output_dir":temp.path().join("copies")})).send().await.unwrap();
        let status = response.status();
        let body = response.text().await.unwrap();
        assert_eq!(status, 200, "{body}");
        let recording = state.db.get_injection(Some("es")).unwrap().unwrap();
        assert_eq!(recording.files.len(), 1);
        assert_eq!(recording.files[0].rel, "game.pck");
        if !direct {
            assert_eq!(std::fs::read(&game).unwrap(), original);
            assert!(!recording.root.join("index.html").exists());
        }
        let recordings: Value = client
            .get(format!("{base}/api/patch/recordings"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(recordings["roots"]["es"], json!(recording.root));
        assert_eq!(recordings["pristine_languages"], json!(["es"]));
        let output = temp.path().join("patch.zip");
        let response = client.post(format!("{base}/api/patch/pack")).json(&json!({"game_path":recording.root,"output_path":output,"languages":["es"],"pristine":true,"detect_id":false})).send().await.unwrap();
        let status = response.status();
        let body = response.text().await.unwrap();
        assert_eq!(status, 200, "{body}");
        let target = temp.path().join("target");
        std::fs::create_dir(&target).unwrap();
        std::fs::write(target.join("game.pck"), &original).unwrap();
        let verified: Value = client
            .post(format!("{base}/api/patch/verify"))
            .json(&json!({"game_path":target,"zip_path":output}))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(verified["tier"], "Strict", "{verified}");
        assert_eq!(verified["outcome"], "Clean", "{verified}");
        assert_eq!(verified["manifest"]["engine"], "godot-dialogic");
        assert!(verified["manifest"]["files"][0]["original_sha256"].is_string());
        server.shutdown().await;
    }
}
