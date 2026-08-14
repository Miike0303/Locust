use std::path::Path;

use serde::Deserialize;
use tempfile::TempDir;

// ─── Fixture builders ──────────────────────────────────────────────────────

fn create_rpgmaker_mv_fixture(dir: &Path) {
    let data = dir.join("data");
    std::fs::create_dir_all(&data).unwrap();

    std::fs::write(
        data.join("Actors.json"),
        r#"[null,
{"id":1,"name":"Hero","description":"The protagonist","profile":"A brave hero.","note":"","battlerName":"Actor1","characterIndex":0,"characterName":"Actor1","classId":1,"equips":[0,0,0,0,0],"faceIndex":0,"faceName":"Actor1","initialLevel":1,"maxLevel":99,"nickname":"The Brave","traits":[]},
{"id":2,"name":"Mage","description":"A powerful mage","profile":"","note":"","battlerName":"Actor2","characterIndex":1,"characterName":"Actor2","classId":2,"equips":[0,0,0,0,0],"faceIndex":1,"faceName":"Actor2","initialLevel":1,"maxLevel":99,"nickname":"The Wise","traits":[]}]"#,
    ).unwrap();

    std::fs::write(
        data.join("System.json"),
        r#"{"gameTitle":"Test RPG","terms":{"basic":["Max HP","Max MP","Attack"],"commands":["Fight","Escape"],"params":["Max HP","Max MP"],"messages":{"actionFailure":"Miss!","actorDamage":"%1 took %2 damage!"}}}"#,
    ).unwrap();

    std::fs::write(
        data.join("Map001.json"),
        r#"{"displayName":"Town","data":[],"events":[null,{"id":1,"name":"NPC","note":"","pages":[{"list":[{"code":101,"indent":0,"parameters":["",0,0,2,""]},{"code":401,"indent":0,"parameters":["Hello traveler!"]},{"code":401,"indent":0,"parameters":["Welcome to our town."]},{"code":0,"indent":0,"parameters":[]}],"moveFrequency":3,"moveRoute":{"list":[{"code":0,"parameters":[]}],"repeat":true,"skippable":false,"wait":false},"moveSpeed":3,"moveType":0,"priorityType":1,"trigger":0}],"x":8,"y":6}]}"#,
    ).unwrap();

    std::fs::write(
        data.join("CommonEvents.json"),
        r#"[null,{"id":1,"name":"TestEvent","list":[{"code":102,"indent":0,"parameters":[["Yes","No"]]},{"code":401,"indent":0,"parameters":["Thank you!"]},{"code":0,"indent":0,"parameters":[]}]}]"#,
    ).unwrap();
}

fn create_renpy_fixture(dir: &Path) {
    let game = dir.join("game");
    std::fs::create_dir_all(&game).unwrap();
    std::fs::write(
        game.join("script.rpy"),
        r#"label start:
    e "Hello, world!"
    "This is the narrator."
    e "How are you?"
    menu:
        "I'm fine":
            jump fine
        "Not great":
            jump bad
"#,
    )
    .unwrap();
}

fn client() -> reqwest::Client {
    reqwest::Client::new()
}

#[derive(Deserialize)]
struct ProjectOpenResponse {
    format_id: String,
    total_strings: usize,
}

#[derive(Deserialize)]
struct StringsResponse {
    entries: Vec<serde_json::Value>,
    // Present in API JSON; tests assert coverage via entries.len() / pending.
    #[allow(dead_code)]
    total: usize,
}

#[derive(Deserialize)]
struct StatsResponse {
    // Present in API JSON; tests assert via pending/translated counts.
    #[allow(dead_code)]
    total: usize,
    pending: usize,
    translated: usize,
}

#[derive(Deserialize)]
struct TranslateStartResponse {
    job_id: String,
}

#[derive(Deserialize)]
struct MultiLangReport {
    languages_processed: Vec<String>,
    backup_id: String,
}

// ─── Full RPG Maker MV flow ────────────────────────────────────────────────

#[tokio::test]
async fn test_full_rpgmaker_mv_flow() {
    let tmpdir = TempDir::new().unwrap();
    create_rpgmaker_mv_fixture(tmpdir.path());

    let state = locust_server::create_test_state();
    let (base_url, _handle) = locust_server::start_test_server(state).await;

    // 1. Open project
    let resp: ProjectOpenResponse = client()
        .post(format!("{}/api/project/open", base_url))
        .json(&serde_json::json!({"path": tmpdir.path().to_string_lossy()}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    assert_eq!(resp.format_id, "rpgmaker-mv");
    assert!(
        resp.total_strings >= 6,
        "got {} strings",
        resp.total_strings
    );
    let total = resp.total_strings;

    // 2. Get strings — all pending
    let strings: StringsResponse = client()
        .get(format!("{}/api/strings?limit=1000", base_url))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(strings.entries.len(), total);
    for e in &strings.entries {
        assert_eq!(e["status"], "pending");
    }

    // 3. Check stats
    let stats: StatsResponse = client()
        .get(format!("{}/api/stats", base_url))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(stats.pending, total);
    assert_eq!(stats.translated, 0);

    // 4. Check glossary empty
    let glossary: Vec<serde_json::Value> = client()
        .get(format!("{}/api/glossary?lang_pair=ja-en", base_url))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(glossary.is_empty());

    // 5. Add glossary entry
    let resp = client()
        .post(format!("{}/api/glossary", base_url))
        .json(&serde_json::json!({
            "term": "Hero", "translation": "Héroe",
            "lang_pair": "en-es", "context": null, "case_sensitive": false
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 201);

    // 6. Start translation
    let start: TranslateStartResponse = client()
        .post(format!("{}/api/translate/start", base_url))
        .json(&serde_json::json!({
            "provider_id": "mock",
            "options": {
                "source_lang": "en", "target_lang": "es",
                "batch_size": 100, "max_concurrent": 1,
                "cost_limit_usd": null, "game_context": null,
                "use_glossary": true, "use_memory": true, "skip_approved": true
            }
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(!start.job_id.is_empty());

    // 7. Wait for translation to complete
    tokio::time::sleep(std::time::Duration::from_secs(3)).await;

    // 8. Verify all translated
    let strings: StringsResponse = client()
        .get(format!("{}/api/strings?limit=1000", base_url))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let translated_count = strings
        .entries
        .iter()
        .filter(|e| e["status"] == "translated")
        .count();
    assert_eq!(
        translated_count, total,
        "expected {} translated, got {}",
        total, translated_count
    );
    for e in &strings.entries {
        let t = e["translation"].as_str().unwrap_or("");
        let src = e["source"].as_str().unwrap_or("");
        // Exact glossary hits short-circuit the provider and are free to grow;
        // only the mock output is length-bound. See step 5.
        if e["provider_used"].as_str() == Some("glossary") {
            continue;
        }
        // Mock is length-safe (≤ source bytes). Long strings keep the tag;
        // short ones may be a same-length reverse without the full marker.
        assert!(
            t.len() <= src.len(),
            "mock must not exceed source bytes: {t:?} vs {src:?}"
        );
        if src.len() >= 12 {
            assert!(
                t.starts_with("[MOCK:es]"),
                "long mock should keep tag, got: {t}"
            );
        }
    }

    // 9. Check stats after translation
    let stats: StatsResponse = client()
        .get(format!("{}/api/stats", base_url))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(stats.translated, total);
    assert_eq!(stats.pending, 0);

    // 10. Inject Replace mode — use short path to avoid Windows path length limit
    let output_dir = std::env::temp_dir().join("locust_out");
    let _ = std::fs::remove_dir_all(&output_dir);
    std::fs::create_dir_all(&output_dir).unwrap();
    let resp = client()
        .post(format!("{}/api/inject", base_url))
        .json(&serde_json::json!({
            "project_path": tmpdir.path().to_string_lossy(),
            "format_id": "rpgmaker-mv",
            "mode": "replace",
            "languages": ["es"],
            "output_dir": output_dir.to_string_lossy()
        }))
        .send()
        .await
        .unwrap();
    let body: serde_json::Value = resp.json().await.unwrap();
    let report: MultiLangReport = serde_json::from_value(body.clone())
        .unwrap_or_else(|_| panic!("failed to parse inject response: {:?}", body));
    assert_eq!(
        report.languages_processed,
        vec!["es"],
        "inject report: {:?}",
        body
    );

    // 11. Verify output files
    let game_name = tmpdir
        .path()
        .file_name()
        .unwrap()
        .to_string_lossy()
        .to_string();
    let output_actors = output_dir
        .join(format!("{}-es", game_name))
        .join("data")
        .join("Actors.json");
    assert!(output_actors.exists(), "output Actors.json should exist");
    let content = std::fs::read_to_string(&output_actors).unwrap();
    let json: serde_json::Value = serde_json::from_str(&content).unwrap();
    // characterIndex should be preserved
    assert_eq!(json[1]["characterIndex"], 0);
    // "Hero" is an exact glossary hit (step 5), so it short-circuits the mock
    // and reaches the injected file as the glossary value.
    let name = json[1]["name"].as_str().unwrap();
    assert_eq!(name, "Héroe", "glossary hit should reach the injected file");

    // 12. Replace mode with output_dir must still take a backup. The original is NOT
    // reliably untouched: Unity, Unreal, Wolf RPG and Ren'Py loose scripts inject via
    // entry.file_path and write straight back to the original game.
    assert_ne!(
        report.backup_id, "skip-replace-mode",
        "Replace mode with an output_dir must not skip the backup"
    );
}

// ─── Injection recording through the HTTP seam ──────────────────────────────

#[tokio::test]
async fn test_inject_records_the_injection_for_patch() {
    // `locust patch` packs EXCLUSIVELY from the recording an injection
    // persisted. The HTTP seam used to call MultiLangInjector::inject with
    // no record_injection at all, so every project injected through the
    // server (or the desktop app, which shares the core seam) hard-errored
    // at patch time with "no injection has been recorded".
    let tmpdir = TempDir::new().unwrap();
    create_rpgmaker_mv_fixture(tmpdir.path());
    let db_dir = TempDir::new().unwrap();
    let db_path = db_dir.path().join("project.locust.db");

    let state = locust_server::create_test_state_with_db(&db_path);
    let (base_url, _handle) = locust_server::start_test_server(state).await;

    let resp: ProjectOpenResponse = client()
        .post(format!("{}/api/project/open", base_url))
        .json(&serde_json::json!({"path": tmpdir.path().to_string_lossy()}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(resp.format_id, "rpgmaker-mv");

    client()
        .post(format!("{}/api/translate/start", base_url))
        .json(&serde_json::json!({
            "provider_id": "mock",
            "options": {
                "source_lang": "en", "target_lang": "es",
                "batch_size": 100, "max_concurrent": 1,
                "cost_limit_usd": null, "game_context": null,
                "use_glossary": false, "use_memory": false, "skip_approved": true
            }
        }))
        .send()
        .await
        .unwrap();
    tokio::time::sleep(std::time::Duration::from_secs(3)).await;

    let output_dir = TempDir::new().unwrap();
    let resp = client()
        .post(format!("{}/api/inject", base_url))
        .json(&serde_json::json!({
            "project_path": tmpdir.path().to_string_lossy(),
            "format_id": "rpgmaker-mv",
            "mode": "replace",
            "languages": ["es"],
            "output_dir": output_dir.path().to_string_lossy()
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    // The load-bearing side effect, read back from the SAME database file
    // the server wrote: a recording for "es" whose root is the per-language
    // copy the injection reported writing into.
    let project_db = locust_core::project::resolve_project_db_path(tmpdir.path());
    let db = locust_core::database::Database::open(&project_db).unwrap();
    let rec = db
        .get_injection(Some("es"))
        .unwrap()
        .expect("the HTTP inject seam must record the injection for `locust patch`");
    let game_name = tmpdir
        .path()
        .file_name()
        .unwrap()
        .to_string_lossy()
        .to_string();
    let expected_root = output_dir.path().join(format!("{}-es", game_name));
    assert!(
        locust_core::database::paths_identical(&rec.root, &expected_root),
        "the recording root must be the per-language copy: got {}, want {}",
        rec.root.display(),
        expected_root.display()
    );
    assert!(
        !rec.files.is_empty(),
        "the recording must list the files injection wrote"
    );
    assert!(
        rec.files.iter().any(|f| f.rel == "data/Actors.json"),
        "the injected data files must be recorded, got: {:?}",
        rec.files.iter().map(|f| &f.rel).collect::<Vec<_>>()
    );
}

// ─── Direct inject records for patch packing ────────────────────────────────

#[tokio::test]
async fn test_inject_direct_records_on_game_root() {
    // `direct: true` must write into the game tree and record under that root
    // (not a per-lang copy), matching CLI --direct — so Patch → Pack can use it.
    let tmpdir = TempDir::new().unwrap();
    create_rpgmaker_mv_fixture(tmpdir.path());
    let db_dir = TempDir::new().unwrap();
    let db_path = db_dir.path().join("project.locust.db");

    let state = locust_server::create_test_state_with_db(&db_path);
    let (base_url, _handle) = locust_server::start_test_server(state).await;

    let _open: ProjectOpenResponse = client()
        .post(format!("{}/api/project/open", base_url))
        .json(&serde_json::json!({"path": tmpdir.path().to_string_lossy()}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    client()
        .post(format!("{}/api/translate/start", base_url))
        .json(&serde_json::json!({
            "provider_id": "mock",
            "options": {
                "source_lang": "en", "target_lang": "es",
                "batch_size": 100, "max_concurrent": 1,
                "cost_limit_usd": null, "game_context": null,
                "use_glossary": false, "use_memory": false, "skip_approved": true
            }
        }))
        .send()
        .await
        .unwrap();
    tokio::time::sleep(std::time::Duration::from_secs(3)).await;

    let resp = client()
        .post(format!("{}/api/inject", base_url))
        .json(&serde_json::json!({
            "project_path": tmpdir.path().to_string_lossy(),
            "format_id": "rpgmaker-mv",
            "languages": ["es"],
            "direct": true
        }))
        .send()
        .await
        .unwrap();
    let status = resp.status();
    let body = resp.text().await.unwrap();
    assert_eq!(status, 200, "direct inject: {body}");

    let v: serde_json::Value =
        serde_json::from_str(&body).unwrap_or_else(|_| panic!("json: {body}"));
    assert_eq!(v["mode"], "direct");
    assert!(
        v["languages_processed"]
            .as_array()
            .map(|a| a.iter().any(|x| x == "es"))
            .unwrap_or(false),
        "expected es processed: {body}"
    );
    // RPG Maker MV replace-style plugin still reports via inject; strings may be written.
    assert!(v.get("backup_id").is_some(), "backup_id present: {body}");

    let project_db = locust_core::project::resolve_project_db_path(tmpdir.path());
    let db = locust_core::database::Database::open(&project_db).unwrap();
    let rec = db
        .get_injection(Some("es"))
        .unwrap()
        .expect("direct inject must record for language es");
    assert!(
        locust_core::database::paths_identical(&rec.root, tmpdir.path()),
        "direct recording root must be the game path: got {}, want {}",
        rec.root.display(),
        tmpdir.path().display()
    );
    // Non-direct default path unchanged: without direct, recording is under *-lang copy
    // (covered by test_inject_records_the_injection_for_patch).
}

// ─── Pack patch zip via HTTP ────────────────────────────────────────────────

#[tokio::test]
async fn test_patch_pack_from_injection_recording() {
    // End-to-end: open → translate → inject (records files) → POST /api/patch/pack
    // packs a zip from that recording. game_path must be the recorded root
    // (per-language copy when inject uses output_dir).
    let tmpdir = TempDir::new().unwrap();
    create_rpgmaker_mv_fixture(tmpdir.path());
    let db_dir = TempDir::new().unwrap();
    let db_path = db_dir.path().join("project.locust.db");

    let state = locust_server::create_test_state_with_db(&db_path);
    let (base_url, _handle) = locust_server::start_test_server(state).await;

    let _open: ProjectOpenResponse = client()
        .post(format!("{}/api/project/open", base_url))
        .json(&serde_json::json!({"path": tmpdir.path().to_string_lossy()}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    client()
        .post(format!("{}/api/translate/start", base_url))
        .json(&serde_json::json!({
            "provider_id": "mock",
            "options": {
                "source_lang": "en", "target_lang": "es",
                "batch_size": 100, "max_concurrent": 1,
                "cost_limit_usd": null, "game_context": null,
                "use_glossary": false, "use_memory": false, "skip_approved": true
            }
        }))
        .send()
        .await
        .unwrap();
    tokio::time::sleep(std::time::Duration::from_secs(3)).await;

    let output_dir = TempDir::new().unwrap();
    let inject_resp = client()
        .post(format!("{}/api/inject", base_url))
        .json(&serde_json::json!({
            "project_path": tmpdir.path().to_string_lossy(),
            "format_id": "rpgmaker-mv",
            "mode": "replace",
            "languages": ["es"],
            "output_dir": output_dir.path().to_string_lossy()
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(inject_resp.status(), 200, "inject must succeed before pack");

    let game_name = tmpdir
        .path()
        .file_name()
        .unwrap()
        .to_string_lossy()
        .to_string();
    let recorded_root = output_dir.path().join(format!("{}-es", game_name));
    let pack_out = TempDir::new().unwrap();
    let zip_path = pack_out.path().join("locust-test-patch.zip");

    let pack_resp = client()
        .post(format!("{}/api/patch/pack", base_url))
        .json(&serde_json::json!({
            "game_path": recorded_root.to_string_lossy(),
            "output_path": zip_path.to_string_lossy(),
            "languages": ["es"],
            "pristine": false
        }))
        .send()
        .await
        .unwrap();
    let status = pack_resp.status();
    let body = pack_resp.text().await.unwrap();
    assert_eq!(status, 200, "pack response: {}", body);

    #[derive(Deserialize)]
    struct PackBody {
        files_packed: usize,
        patch_id: String,
        patch_version: String,
        tier: String,
        engine: String,
        language: String,
        size_bytes: u64,
        output_path: String,
    }
    let report: PackBody =
        serde_json::from_str(&body).unwrap_or_else(|_| panic!("pack JSON: {}", body));
    assert!(report.files_packed > 0, "expected packed files: {:?}", body);
    assert!(!report.patch_id.is_empty());
    assert!(!report.patch_version.is_empty());
    assert_eq!(report.tier, "structural");
    assert_eq!(report.language, "es");
    assert!(report.size_bytes > 0);
    assert!(
        zip_path.is_file(),
        "zip must exist at {}",
        report.output_path
    );
    // Engine comes from format detect on the recorded tree (rpgmaker-mv).
    assert!(
        report.engine.contains("rpgmaker") || report.engine == "unknown",
        "engine: {}",
        report.engine
    );

    // Empty game_path → 400 with a body
    let bad = client()
        .post(format!("{}/api/patch/pack", base_url))
        .json(&serde_json::json!({
            "game_path": "",
            "output_path": zip_path.to_string_lossy(),
            "languages": ["es"]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(bad.status(), 400);
    let bad_body = bad.text().await.unwrap();
    assert!(
        !bad_body.is_empty() && bad_body.to_lowercase().contains("game_path"),
        "expected game_path error body, got: {}",
        bad_body
    );

    // pristine without backup → 400 PatchError
    let strict_zip = pack_out.path().join("strict.zip");
    let strict = client()
        .post(format!("{}/api/patch/pack", base_url))
        .json(&serde_json::json!({
            "game_path": recorded_root.to_string_lossy(),
            "output_path": strict_zip.to_string_lossy(),
            "languages": ["es"],
            "pristine": true
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(strict.status(), 400, "pristine without backup must fail");
    let strict_body = strict.text().await.unwrap();
    assert!(
        strict_body.to_lowercase().contains("pristine"),
        "expected pristine error, got: {}",
        strict_body
    );
}

// ─── Ren'Py Add mode flow ──────────────────────────────────────────────────

#[tokio::test]
async fn test_renpy_add_mode_flow() {
    let tmpdir = TempDir::new().unwrap();
    create_renpy_fixture(tmpdir.path());

    let state = locust_server::create_test_state();
    let (base_url, _handle) = locust_server::start_test_server(state).await;

    // Open project
    let resp: ProjectOpenResponse = client()
        .post(format!("{}/api/project/open", base_url))
        .json(&serde_json::json!({"path": tmpdir.path().to_string_lossy()}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(resp.format_id, "renpy");
    let total = resp.total_strings;
    assert!(total >= 3);

    // Translate
    client()
        .post(format!("{}/api/translate/start", base_url))
        .json(&serde_json::json!({
            "provider_id": "mock",
            "options": {
                "source_lang": "en", "target_lang": "es",
                "batch_size": 100, "max_concurrent": 1,
                "cost_limit_usd": null, "game_context": null,
                "use_glossary": false, "use_memory": false, "skip_approved": true
            }
        }))
        .send()
        .await
        .unwrap();

    tokio::time::sleep(std::time::Duration::from_secs(3)).await;

    // Inject Add mode for es and fr
    let resp = client()
        .post(format!("{}/api/inject", base_url))
        .json(&serde_json::json!({
            "project_path": tmpdir.path().to_string_lossy(),
            "format_id": "renpy",
            "mode": "add",
            "languages": ["es", "fr"]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    // Verify tl dirs created
    let tl_es = tmpdir.path().join("game").join("tl").join("es");
    let tl_fr = tmpdir.path().join("game").join("tl").join("fr");
    assert!(tl_es.exists(), "tl/es/ should exist");
    assert!(tl_fr.exists(), "tl/fr/ should exist");

    // Check tl/es has a .rpy file with translate blocks
    let rpy_files: Vec<_> = std::fs::read_dir(&tl_es)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().is_some_and(|ext| ext == "rpy"))
        .collect();
    assert!(!rpy_files.is_empty(), "should have .rpy files in tl/es/");
    let content = std::fs::read_to_string(rpy_files[0].path()).unwrap();
    assert!(
        content.contains("translate es"),
        "should have translate es blocks"
    );
}

// ─── Validation catches placeholder issues ─────────────────────────────────

#[tokio::test]
async fn test_validation_catches_placeholder_issues() {
    let tmpdir = TempDir::new().unwrap();
    let data = tmpdir.path().join("data");
    std::fs::create_dir_all(&data).unwrap();

    // Actor with placeholder in name
    std::fs::write(
        data.join("Actors.json"),
        r#"[null,{"id":1,"name":"\\c[2]Hero","description":"Desc","profile":"","note":"","battlerName":"","characterIndex":0,"characterName":"","classId":1,"equips":[],"faceIndex":0,"faceName":"","initialLevel":1,"maxLevel":99,"nickname":"","traits":[]}]"#,
    ).unwrap();
    std::fs::write(
        data.join("System.json"),
        r#"{"gameTitle":"Test","terms":{"basic":[],"commands":[],"params":[],"messages":{}}}"#,
    )
    .unwrap();

    let state = locust_server::create_test_state();
    let (base_url, _handle) = locust_server::start_test_server(state).await;

    // Open project
    client()
        .post(format!("{}/api/project/open", base_url))
        .json(&serde_json::json!({"path": tmpdir.path().to_string_lossy()}))
        .send()
        .await
        .unwrap();

    // Patch string with translation missing placeholder
    let strings: StringsResponse = client()
        .get(format!("{}/api/strings?limit=100", base_url))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    let actor_entry = strings
        .entries
        .iter()
        .find(|e| e["source"].as_str().unwrap_or("").contains("Hero"));

    if let Some(entry) = actor_entry {
        let id = entry["id"].as_str().unwrap();
        // Set translation WITHOUT the placeholder
        client()
            .patch(format!("{}/api/strings/{}", base_url, urlencoding(id)))
            .json(&serde_json::json!({"translation": "Héroe", "status": "translated"}))
            .send()
            .await
            .unwrap();

        // Validate
        let resp: serde_json::Value = client()
            .post(format!("{}/api/validate", base_url))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();

        let issues_found = resp["validation"]["issues_found"].as_u64().unwrap_or(0);
        assert!(issues_found > 0, "should have validation issues");
    }
}

// ─── Batch string patch (search-replace) ───────────────────────────────────

#[tokio::test]
async fn test_batch_patch_strings_applies_known_skips_unknown() {
    let tmpdir = TempDir::new().unwrap();
    create_rpgmaker_mv_fixture(tmpdir.path());

    let state = locust_server::create_test_state();
    let (base_url, _handle) = locust_server::start_test_server(state).await;

    client()
        .post(format!("{}/api/project/open", base_url))
        .json(&serde_json::json!({"path": tmpdir.path().to_string_lossy()}))
        .send()
        .await
        .unwrap();

    let strings: StringsResponse = client()
        .get(format!("{}/api/strings?limit=100", base_url))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(!strings.entries.is_empty());

    let id0 = strings.entries[0]["id"].as_str().unwrap().to_string();
    let id1 = strings
        .entries
        .get(1)
        .and_then(|e| e["id"].as_str())
        .unwrap_or(&id0)
        .to_string();

    let resp: serde_json::Value = client()
        .post(format!("{}/api/strings/batch", base_url))
        .json(&serde_json::json!({
            "provider": "search-replace",
            "updates": [
                {"id": id0, "translation": "BATCH_A"},
                {"id": "does-not-exist-id", "translation": "NOPE"},
                {"id": id1, "translation": "BATCH_B"},
            ]
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    assert_eq!(resp["requested"].as_u64(), Some(3));
    assert_eq!(resp["applied"].as_u64(), Some(2));
    assert_eq!(resp["skipped"].as_u64(), Some(1));

    let e0: serde_json::Value = client()
        .get(format!("{}/api/strings/{}", base_url, urlencoding(&id0)))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(e0["translation"].as_str(), Some("BATCH_A"));
}

fn urlencoding(s: &str) -> String {
    s.replace('#', "%23")
        .replace('[', "%5B")
        .replace(']', "%5D")
}

// ─── Backup and restore ────────────────────────────────────────────────────

#[tokio::test]
async fn test_backup_restore() {
    let tmpdir = TempDir::new().unwrap();
    create_rpgmaker_mv_fixture(tmpdir.path());

    let state = locust_server::create_test_state();
    let (base_url, _handle) = locust_server::start_test_server(state).await;

    // Open project
    client()
        .post(format!("{}/api/project/open", base_url))
        .json(&serde_json::json!({"path": tmpdir.path().to_string_lossy()}))
        .send()
        .await
        .unwrap();

    // Translate
    client()
        .post(format!("{}/api/translate/start", base_url))
        .json(&serde_json::json!({
            "provider_id": "mock",
            "options": {
                "source_lang": "en", "target_lang": "es",
                "batch_size": 100, "max_concurrent": 1,
                "cost_limit_usd": null, "game_context": null,
                "use_glossary": false, "use_memory": false, "skip_approved": true
            }
        }))
        .send()
        .await
        .unwrap();
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;

    // Inject with Add mode (creates backup; Replace+output_dir skips backup)
    client()
        .post(format!("{}/api/inject", base_url))
        .json(&serde_json::json!({
            "project_path": tmpdir.path().to_string_lossy(),
            "format_id": "rpgmaker-mv",
            "mode": "add",
            "languages": ["es"]
        }))
        .send()
        .await
        .unwrap();

    // Corrupt a file
    let actors = tmpdir.path().join("data").join("Actors.json");
    std::fs::write(&actors, "CORRUPTED").unwrap();
    assert_eq!(std::fs::read_to_string(&actors).unwrap(), "CORRUPTED");

    // Get backup id
    let backups: Vec<serde_json::Value> = client()
        .get(format!("{}/api/backups", base_url))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(!backups.is_empty());
    let backup_id = backups[0]["id"].as_str().unwrap();

    // Restore
    let resp = client()
        .post(format!("{}/api/backups/{}/restore", base_url, backup_id))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    // Verify restored
    let restored = std::fs::read_to_string(&actors).unwrap();
    assert_ne!(restored, "CORRUPTED");
    assert!(
        restored.contains("Hero"),
        "original content should be restored"
    );
}

// ─── Register language (RM multi-lang UI) ───────────────────────────────────

#[tokio::test]
async fn test_register_lang_endpoint_patches_plugins() {
    let tmpdir = TempDir::new().unwrap();
    let root = tmpdir.path();
    std::fs::create_dir_all(root.join("js")).unwrap();
    std::fs::create_dir_all(root.join("data")).unwrap();
    std::fs::write(root.join("js").join("rmmz_core.js"), "// mz").unwrap();
    std::fs::write(
        root.join("js").join("plugins.js"),
        r#"var $plugins = [{"name":"Iavra_MZ_Localization_byNeomaStudio","status":true,"parameters":{"Languages":"jp, en, zh","Language Labels":"en:English, jp:日本語, zh:中文"}}];
const langs = ['jp', 'en', 'zh'];
"#,
    )
    .unwrap();
    std::fs::write(
        root.join("data").join("System.json"),
        r#"{"gameTitle":"T","terms":{"basic":[],"commands":[],"params":[],"messages":{}}}"#,
    )
    .unwrap();

    let state = locust_server::create_test_state();
    let (base_url, _handle) = locust_server::start_test_server(state).await;

    let resp = client()
        .post(format!("{}/api/register-lang", base_url))
        .json(&serde_json::json!({
            "game_path": root.to_string_lossy(),
            "lang": "es",
            "label": "Español"
        }))
        .send()
        .await
        .unwrap();
    let status = resp.status();
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(status, 200, "body: {body}");
    assert!(
        body.get("plugins_js")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
            || body
                .get("iavra_languages")
                .and_then(|v| v.as_bool())
                .unwrap_or(false),
        "expected plugins patch: {body}"
    );

    let plugins = std::fs::read_to_string(root.join("js").join("plugins.js")).unwrap();
    assert!(plugins.contains("es"), "{plugins}");
    assert!(
        plugins.contains("Español") || plugins.contains("'es'"),
        "{plugins}"
    );
    assert!(root.join("js").join("plugins.js.bak-locust").is_file());
}

#[tokio::test]
async fn test_register_lang_rejects_bad_lang() {
    let tmpdir = TempDir::new().unwrap();
    std::fs::create_dir_all(tmpdir.path()).unwrap();
    let state = locust_server::create_test_state();
    let (base_url, _handle) = locust_server::start_test_server(state).await;
    let resp = client()
        .post(format!("{}/api/register-lang", base_url))
        .json(&serde_json::json!({
            "game_path": tmpdir.path().to_string_lossy(),
            "lang": "bad lang!",
            "label": "X"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);
}

/// Recursively copy `src` into `dst` (created if missing).
fn copy_tree(src: &Path, dst: &Path) {
    std::fs::create_dir_all(dst).unwrap();
    for entry in walkdir::WalkDir::new(src).follow_links(false) {
        let entry = entry.unwrap();
        let rel = entry.path().strip_prefix(src).unwrap();
        let target = dst.join(rel);
        if entry.file_type().is_dir() {
            std::fs::create_dir_all(&target).unwrap();
        } else if entry.file_type().is_file() {
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent).unwrap();
            }
            std::fs::copy(entry.path(), &target).unwrap();
        }
    }
}

/// Snapshot every file under `root` as (relative path, bytes).
fn snapshot_tree(root: &Path) -> std::collections::BTreeMap<String, Vec<u8>> {
    let mut out = std::collections::BTreeMap::new();
    for entry in walkdir::WalkDir::new(root).follow_links(false) {
        let entry = entry.unwrap();
        if !entry.file_type().is_file() {
            continue;
        }
        let rel = entry
            .path()
            .strip_prefix(root)
            .unwrap()
            .to_string_lossy()
            .replace(std::path::MAIN_SEPARATOR, "/");
        // `.locust/` is patch bookkeeping, not game content.
        if rel.starts_with(".locust/") {
            continue;
        }
        out.insert(rel, std::fs::read(entry.path()).unwrap());
    }
    out
}

/// The four patch endpoints that had no coverage, driven in their real order
/// against a real packed zip: verify → apply → status → rollback. The game must
/// come back byte-identical, which is the whole promise of the transaction.
#[tokio::test]
async fn test_patch_lifecycle_over_http_restores_game_byte_identical() {
    let tmpdir = TempDir::new().unwrap();
    create_rpgmaker_mv_fixture(tmpdir.path());
    let db_dir = TempDir::new().unwrap();
    let db_path = db_dir.path().join("project.locust.db");

    let state = locust_server::create_test_state_with_db(&db_path);
    let (base_url, _handle) = locust_server::start_test_server(state).await;

    let _open: ProjectOpenResponse = client()
        .post(format!("{}/api/project/open", base_url))
        .json(&serde_json::json!({"path": tmpdir.path().to_string_lossy()}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    client()
        .post(format!("{}/api/translate/start", base_url))
        .json(&serde_json::json!({
            "provider_id": "mock",
            "options": {
                "source_lang": "en", "target_lang": "es",
                "batch_size": 100, "max_concurrent": 1,
                "cost_limit_usd": null, "game_context": null,
                "use_glossary": false, "use_memory": false, "skip_approved": true
            }
        }))
        .send()
        .await
        .unwrap();
    tokio::time::sleep(std::time::Duration::from_secs(3)).await;

    let output_dir = TempDir::new().unwrap();
    let inject_resp = client()
        .post(format!("{}/api/inject", base_url))
        .json(&serde_json::json!({
            "project_path": tmpdir.path().to_string_lossy(),
            "format_id": "rpgmaker-mv",
            "mode": "replace",
            "languages": ["es"],
            "output_dir": output_dir.path().to_string_lossy()
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(inject_resp.status(), 200, "inject must succeed before pack");

    let game_name = tmpdir
        .path()
        .file_name()
        .unwrap()
        .to_string_lossy()
        .to_string();
    let recorded_root = output_dir.path().join(format!("{}-es", game_name));
    let pack_out = TempDir::new().unwrap();
    let zip_path = pack_out.path().join("lifecycle.zip");

    let pack_resp = client()
        .post(format!("{}/api/patch/pack", base_url))
        .json(&serde_json::json!({
            // Pack against the untranslated tree so the manifest carries
            // original hashes — that is the strict tier apply/rollback need.
            "game_path": recorded_root.to_string_lossy(),
            "output_path": zip_path.to_string_lossy(),
            "languages": ["es"],
            "pristine": true,
            "pristine_path": tmpdir.path().to_string_lossy()
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(
        pack_resp.status(),
        200,
        "pack: {}",
        pack_resp.text().await.unwrap()
    );

    // Target a pristine copy of the game, so rollback has something to prove.
    let target = TempDir::new().unwrap();
    let game = target.path().join("game");
    copy_tree(tmpdir.path(), &game);
    let before = snapshot_tree(&game);
    assert!(!before.is_empty(), "fixture should have files");

    let paths = serde_json::json!({
        "game_path": game.to_string_lossy(),
        "zip_path": zip_path.to_string_lossy(),
    });

    // status — clean game, nothing applied yet
    let st: serde_json::Value = client()
        .post(format!("{}/api/patch/status", base_url))
        .json(&serde_json::json!({"game_path": game.to_string_lossy()}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(st["status"], "not_patched", "status before apply: {st}");

    // verify — read-only pre-flight
    let vr = client()
        .post(format!("{}/api/patch/verify", base_url))
        .json(&paths)
        .send()
        .await
        .unwrap();
    let vs = vr.status();
    let vbody = vr.text().await.unwrap();
    assert_eq!(vs, 200, "verify: {vbody}");
    let vjson: serde_json::Value = serde_json::from_str(&vbody).unwrap();
    assert_eq!(vjson["outcome"], "Clean", "verify outcome: {vbody}");
    assert_eq!(
        vjson["tier"], "Strict",
        "pristine pack must verify strict: {vbody}"
    );
    assert!(
        !vjson["replaced"]
            .as_array()
            .expect("replaced is a path list")
            .is_empty(),
        "verify should plan replacements: {vbody}"
    );
    assert!(
        vjson["conflicts"].as_array().is_some_and(|c| c.is_empty()),
        "clean game must have no conflicts: {vbody}"
    );
    assert_eq!(
        snapshot_tree(&game),
        before,
        "verify must not touch the game"
    );

    // apply
    let ar = client()
        .post(format!("{}/api/patch/apply", base_url))
        .json(&paths)
        .send()
        .await
        .unwrap();
    let as_ = ar.status();
    let abody = ar.text().await.unwrap();
    assert_eq!(as_, 200, "apply: {abody}");
    let ajson: serde_json::Value = serde_json::from_str(&abody).unwrap();
    assert!(
        ajson["replaced"].as_u64().unwrap_or(0) > 0,
        "apply should replace files: {abody}"
    );
    assert_ne!(
        snapshot_tree(&game),
        before,
        "apply must actually change the game"
    );

    // status — now patched, and it echoes the receipt
    let st2: serde_json::Value = client()
        .post(format!("{}/api/patch/status", base_url))
        .json(&serde_json::json!({"game_path": game.to_string_lossy()}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(st2["status"], "patched", "status after apply: {st2}");
    assert_eq!(st2["patch_id"], ajson["patch_id"], "receipt id mismatch");

    // rollback — the whole point of the backup slot
    let rr = client()
        .post(format!("{}/api/patch/rollback", base_url))
        .json(&serde_json::json!({"game_path": game.to_string_lossy()}))
        .send()
        .await
        .unwrap();
    let rs = rr.status();
    let rbody = rr.text().await.unwrap();
    assert_eq!(rs, 200, "rollback: {rbody}");

    assert_eq!(
        snapshot_tree(&game),
        before,
        "rollback must restore the game byte-identically"
    );

    // A patch packed with no pristine baseline carries no original hashes, so
    // every path reads as "added" and collides with the files already there.
    // Applying it unforced must be refused rather than overwrite blindly.
    let loose_zip = pack_out.path().join("no-baseline.zip");
    let loose_pack = client()
        .post(format!("{}/api/patch/pack", base_url))
        .json(&serde_json::json!({
            "game_path": recorded_root.to_string_lossy(),
            "output_path": loose_zip.to_string_lossy(),
            "languages": ["es"],
            "pristine": false
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(loose_pack.status(), 200, "loose pack should still build");

    let loose_paths = serde_json::json!({
        "game_path": game.to_string_lossy(),
        "zip_path": loose_zip.to_string_lossy(),
    });
    let lv: serde_json::Value = client()
        .post(format!("{}/api/patch/verify", base_url))
        .json(&loose_paths)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(lv["tier"], "Structural", "no baseline → structural: {lv}");
    assert!(
        !lv["conflicts"].as_array().unwrap().is_empty(),
        "structural patch over existing files must conflict: {lv}"
    );

    let la = client()
        .post(format!("{}/api/patch/apply", base_url))
        .json(&loose_paths)
        .send()
        .await
        .unwrap();
    assert_ne!(
        la.status(),
        200,
        "unforced structural apply must be refused: {}",
        la.text().await.unwrap()
    );
    assert_eq!(
        snapshot_tree(&game),
        before,
        "a refused apply must leave the game untouched"
    );
}

#[derive(Deserialize)]
struct ProjectOpenMergeResponse {
    format_id: String,
    total_strings: usize,
    added: usize,
    updated: usize,
    stale_source_reset: usize,
    removed: usize,
    preserved_translations: usize,
    database_path: String,
}

#[tokio::test]
async fn test_http_reopen_same_game_preserves_translations() {
    let tmpdir = TempDir::new().unwrap();
    create_rpgmaker_mv_fixture(tmpdir.path());

    let state = locust_server::create_test_state();
    let (base_url, _handle) = locust_server::start_test_server(state.clone()).await;

    let first: ProjectOpenMergeResponse = client()
        .post(format!("{}/api/project/open", base_url))
        .json(&serde_json::json!({"path": tmpdir.path().to_string_lossy()}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(first.format_id, "rpgmaker-mv");
    assert!(first.added >= 6);
    assert_eq!(first.preserved_translations, 0);
    assert!(first.database_path.ends_with(".locust.db"));

    let strings: StringsResponse = client()
        .get(format!("{}/api/strings?limit=1000", base_url))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let id = strings.entries[0]["id"].as_str().unwrap().to_string();
    // Ids carry `#` (and `[`/`]`), which a raw URL would read as a fragment —
    // the PATCH would then miss the row and this test would fail later, at the
    // preserved-translations assert, far from the cause.
    let patched = client()
        .patch(format!("{}/api/strings/{}", base_url, urlencoding(&id)))
        .json(&serde_json::json!({"translation": "Hola", "status": "approved"}))
        .send()
        .await
        .unwrap();
    assert!(
        patched.status().is_success(),
        "PATCH must land before reopen can preserve anything, got {}",
        patched.status()
    );

    let second: ProjectOpenMergeResponse = client()
        .post(format!("{}/api/project/open", base_url))
        .json(&serde_json::json!({"path": tmpdir.path().to_string_lossy()}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(second.preserved_translations > 0);
    assert_eq!(second.added, 0);
    assert!(second.updated >= 1);

    let after: StringsResponse = client()
        .get(format!("{}/api/strings?limit=1000", base_url))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let kept = after
        .entries
        .iter()
        .find(|e| e["id"] == id)
        .expect("translated id still present");
    assert_eq!(kept["translation"], "Hola");
    assert_eq!(kept["status"], "approved");
}

#[tokio::test]
async fn test_http_and_core_open_produce_the_same_merge() {
    let http_game = TempDir::new().unwrap();
    let core_game = TempDir::new().unwrap();
    create_rpgmaker_mv_fixture(http_game.path());
    create_rpgmaker_mv_fixture(core_game.path());

    let state = locust_server::create_test_state();
    let (base_url, _handle) = locust_server::start_test_server(state).await;
    let http: ProjectOpenMergeResponse = client()
        .post(format!("{}/api/project/open", base_url))
        .json(&serde_json::json!({"path": http_game.path().to_string_lossy()}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    // Tauri calls the same `project::open_project` the HTTP handler uses.
    let db = locust_core::database::Database::open_in_memory().unwrap();
    let reg = locust_formats::default_registry();
    let core = locust_core::project::open_project(&db, &reg, core_game.path(), None).unwrap();

    assert_eq!(http.added, core.added);
    assert_eq!(http.updated, core.updated);
    assert_eq!(http.stale_source_reset, core.stale_source_reset);
    assert_eq!(http.removed, core.removed);
    assert_eq!(http.preserved_translations, core.preserved_translations);
    assert_eq!(http.total_strings, core.total_strings);
}

#[tokio::test]
async fn test_spawn_translation_job_falls_back_to_second_provider() {
    use locust_core::models::{StringEntry, StringStatus};
    use locust_core::translation::TranslationOptions;
    use std::path::PathBuf;
    use std::sync::Arc;

    let state = locust_server::create_test_state();
    {
        let mut reg = state.provider_registry.write().await;
        reg.register(Arc::new(locust_providers::mock::AlwaysErrorProvider));
    }
    state
        .db
        .save_entries(&[StringEntry::new(
            "e1",
            "Hello world",
            PathBuf::from("a.json"),
        )])
        .unwrap();

    locust_server::spawn_translation_job(
        &state,
        "always-error".into(),
        Some(vec!["mock".into()]),
        TranslationOptions {
            use_memory: false,
            use_glossary: false,
            ..Default::default()
        },
    )
    .await
    .unwrap();

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let got = state.db.get_entry("e1").unwrap().unwrap();
        if got.status == StringStatus::Translated {
            assert_eq!(got.provider_used.as_deref(), Some("mock"));
            break;
        }
        if std::time::Instant::now() > deadline {
            panic!("fallback did not finish: {:?}", got.provider_used);
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
}
