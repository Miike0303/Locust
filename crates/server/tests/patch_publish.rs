use std::path::PathBuf;
use std::sync::Arc;

use locust_core::patch::{identity::normalize_dlsite_code, PatchManifest};
use serde_json::{json, Value};

struct Fixture {
    directory: tempfile::TempDir,
    game: PathBuf,
    pristine: PathBuf,
    state: Arc<locust_server::AppState>,
    url: String,
    handle: tokio::task::JoinHandle<()>,
    client: reqwest::Client,
}

impl Fixture {
    async fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let game = directory.path().join("[RJ01234567] Demo Game");
        let pristine = directory.path().join("pristine");
        std::fs::create_dir(&game).unwrap();
        std::fs::create_dir(&pristine).unwrap();
        let script = game.join("story.html");
        std::fs::write(&script, "<p>Hello</p>").unwrap();
        std::fs::write(pristine.join("story.html"), "<p>Hello</p>").unwrap();
        let state = locust_server::create_test_state_with_db(&directory.path().join("project.db"));
        let (url, handle) = locust_server::start_test_server(state.clone()).await;
        let client = reqwest::Client::builder().no_proxy().build().unwrap();
        let response = client
            .post(format!("{url}/api/project/open"))
            .json(&json!({"path":game,"format_id":"html-game"}))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        let mut entries = state.db.get_entries(&Default::default()).unwrap();
        for entry in &mut entries {
            entry.translation = Some("Hola".into());
            entry.status = locust_core::models::StringStatus::Translated;
        }
        state.db.save_entries(&entries).unwrap();
        std::fs::write(&script, "<p>Hola</p>").unwrap();
        state
            .db
            .record_injection(Some("es"), &game, &[script])
            .unwrap();
        Self {
            directory,
            game,
            pristine,
            state,
            url,
            handle,
            client,
        }
    }

    fn request(&self) -> Value {
        json!({"game_path":self.game,"output_path":self.directory.path().join("patch.zip"),"languages":["es"],"pristine_path":self.pristine})
    }

    async fn pack(&self, request: &Value) -> reqwest::Response {
        self.client
            .post(format!("{}/api/patch/pack", self.url))
            .json(request)
            .send()
            .await
            .unwrap()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

#[tokio::test]
async fn pack_identity_is_written_to_manifest_and_response() {
    let fixture = Fixture::new().await;
    let mut request = fixture.request();
    request["rj_code"] = json!("re123456");
    request["store_ids"] = json!(["Steam=123"]);
    request["game_version"] = json!(" 1.2 ");
    let response = fixture.pack(&request).await;
    let status = response.status();
    let report: Value = response.json().await.unwrap();
    assert_eq!(status, 200, "{report}");
    let verified: Value = fixture
        .client
        .post(format!("{}/api/patch/verify", fixture.url))
        .json(&json!({"game_path":fixture.pristine,"zip_path":request["output_path"]}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let manifest: PatchManifest = serde_json::from_value(verified["manifest"].clone()).unwrap();
    let game = manifest.game.unwrap();
    assert_eq!(game.store_ids["dlsite"], "RE123456");
    assert_eq!(game.store_ids["steam"], "123");
    assert_eq!(game.game_version.as_deref(), Some("1.2"));
    assert_eq!(game.fingerprint.len(), 1);
    assert_eq!(
        report["game"],
        json!({"store_ids":game.store_ids,"game_version":"1.2","fingerprint_count":1})
    );
    assert!(report.get("entry_path").is_none());
}

#[tokio::test]
async fn pack_invalid_identity_returns_400_without_zip() {
    let fixture = Fixture::new().await;
    for invalid in [
        json!({"rj_code":"RJ12345"}),
        json!({"store_ids":["steam"]}),
        json!({"game_version":" "}),
    ] {
        let mut request = fixture.request();
        request
            .as_object_mut()
            .unwrap()
            .extend(invalid.as_object().unwrap().clone());
        let response = fixture.pack(&request).await;
        assert_eq!(response.status(), 400);
        let message = response.text().await.unwrap();
        if request.get("rj_code").is_some() {
            assert_eq!(
                message,
                normalize_dlsite_code("RJ12345").unwrap_err().to_string()
            );
        }
        assert!(!fixture.directory.path().join("patch.zip").exists());
    }
}

#[tokio::test]
async fn pack_entry_contains_detected_rj_and_written_zip_hash() {
    let fixture = Fixture::new().await;
    let mut request = fixture.request();
    let entry = fixture.directory.path().join("release.md");
    request["entry_path"] = json!(entry);
    let response = fixture.pack(&request).await;
    let status = response.status();
    let report: Value = response.json().await.unwrap();
    assert_eq!(status, 200, "{report}");
    assert_eq!(report["entry_path"], json!(entry));
    assert_eq!(report["game"]["store_ids"]["dlsite"], "RJ01234567");
    let md = std::fs::read_to_string(&entry).unwrap();
    let (hash, size) =
        locust_core::database::sha256_file(&fixture.directory.path().join("patch.zip")).unwrap();
    assert!(md.contains("gameTitle: \"Demo Game\"\n"), "{md}");
    assert!(md.contains("rjCode: \"RJ01234567\"\n"));
    assert!(md.contains(&format!("  size: {size}\n  sha256: {hash:?}\n")));
}

#[tokio::test]
async fn pack_rejects_protected_and_existing_entry_paths_before_zip() {
    let fixture = Fixture::new().await;
    let existing = fixture.directory.path().join("existing.md");
    std::fs::write(&existing, "keep").unwrap();
    for entry in [
        fixture.game.join("new/entry.md"),
        fixture.pristine.join("entry.md"),
        fixture.state.backup_manager.root().join("entry.md"),
        fixture.directory.path().join("patch.zip"),
        PathBuf::from(format!("{}-wal", fixture.state.db.path().display())),
        existing.clone(),
        PathBuf::new(),
    ] {
        let mut request = fixture.request();
        request["entry_path"] = json!(entry);
        let response = fixture.pack(&request).await;
        assert_eq!(response.status(), 400, "{}", entry.display());
        assert!(!fixture.directory.path().join("patch.zip").exists());
    }
    assert!(!fixture.game.join("new").exists());
    assert_eq!(std::fs::read_to_string(existing).unwrap(), "keep");
}

#[tokio::test]
async fn pack_detection_can_be_disabled() {
    let fixture = Fixture::new().await;
    let mut request = fixture.request();
    request["detect_id"] = json!(false);
    let response = fixture.pack(&request).await;
    assert_eq!(response.status(), 200);
    let report: Value = response.json().await.unwrap();
    assert_eq!(report["game"]["store_ids"], json!({}));
    assert_eq!(report["game"]["fingerprint_count"], 1);
}

#[tokio::test]
async fn identity_endpoint_detects_paths_without_project_or_filesystem_access() {
    let state = locust_server::create_test_state();
    let (url, handle) = locust_server::start_test_server(state.clone()).await;
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    for (path, expected) in [
        ("Z:/missing/[rj01234567] Game", Some("RJ01234567")),
        (r"Z:\missing\RE123456\plain", Some("RE123456")),
        ("unknown-game", None),
    ] {
        let response = client
            .get(format!("{url}/api/patch/identity"))
            .query(&[("game_path", path)])
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        assert_eq!(
            response.json::<Value>().await.unwrap(),
            json!({"detected_dlsite_code":expected})
        );
    }
    let response = client
        .get(format!("{url}/api/patch/identity"))
        .query(&[("game_path", "  ")])
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 400);
    assert!(state.current_project.read().await.is_none());
    handle.abort();
}

#[tokio::test]
async fn pack_entry_uses_recorded_root_when_game_path_is_the_project_db() {
    // A project opened from its database reports the DB file as its path, so
    // the desktop sends it as game_path. The entry must still describe and
    // protect the recorded game folder, not the DB's parent directory.
    let fixture = Fixture::new().await;
    // Opening the game moved the project to `<parent>/<game>.locust.db`.
    let db_path = fixture.state.db.path();
    assert!(db_path.is_file(), "{}", db_path.display());
    let out = fixture.directory.path().join("out");
    let entry = out.join("demo-es.md");
    let mut request = fixture.request();
    request["game_path"] = json!(db_path);
    request["output_path"] = json!(out.join("demo-es.zip"));
    request["entry_path"] = json!(entry);
    let response = fixture.pack(&request).await;
    let status = response.status();
    let body = response.text().await.unwrap();
    assert_eq!(status, 200, "{body}");
    let markdown = std::fs::read_to_string(&entry).unwrap();
    assert!(
        markdown.contains("gameTitle: \"Demo Game\"\n"),
        "{markdown}"
    );
    assert!(markdown.contains("engine: \"html-game\""), "{markdown}");
    assert!(markdown.contains("rjCode: \"RJ01234567\"\n"), "{markdown}");

    let mut inside_game = fixture.request();
    inside_game["game_path"] = json!(db_path);
    inside_game["output_path"] = json!(out.join("second.zip"));
    inside_game["entry_path"] = json!(fixture.game.join("entry.md"));
    let response = fixture.pack(&inside_game).await;
    assert_eq!(response.status(), 400);
    assert!(!out.join("second.zip").exists());

    // Any other file keeps the different-tree refusal.
    let other = fixture.directory.path().join("other.db");
    std::fs::write(&other, b"").unwrap();
    let mut unrelated = fixture.request();
    unrelated["game_path"] = json!(other);
    unrelated["output_path"] = json!(out.join("third.zip"));
    let response = fixture.pack(&unrelated).await;
    assert_eq!(response.status(), 400);
    assert!(!out.join("third.zip").exists());
}
