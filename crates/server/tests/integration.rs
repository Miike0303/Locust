use std::path::Path;
use std::sync::Arc;

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
    ).unwrap();
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
    total: usize,
}

#[derive(Deserialize)]
struct StatsResponse {
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
    assert!(resp.total_strings >= 6, "got {} strings", resp.total_strings);
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
    assert_eq!(translated_count, total, "expected {} translated, got {}", total, translated_count);
    for e in &strings.entries {
        let t = e["translation"].as_str().unwrap_or("");
        assert!(
            t.contains("[MOCK:es]"),
            "translation should contain [MOCK:es], got: {}",
            t
        );
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
    assert_eq!(report.languages_processed, vec!["es"], "inject report: {:?}", body);

    // 11. Verify output files
    let game_name = tmpdir.path().file_name().unwrap().to_string_lossy().to_string();
    let output_actors = output_dir
        .join(format!("{}-es", game_name))
        .join("data")
        .join("Actors.json");
    assert!(output_actors.exists(), "output Actors.json should exist");
    let content = std::fs::read_to_string(&output_actors).unwrap();
    let json: serde_json::Value = serde_json::from_str(&content).unwrap();
    // characterIndex should be preserved
    assert_eq!(json[1]["characterIndex"], 0);
    // name should be translated
    let name = json[1]["name"].as_str().unwrap();
    assert!(name.contains("[MOCK:es]"), "name should be translated: {}", name);

    // 12. Check backups
    let backups: Vec<serde_json::Value> = client()
        .get(format!("{}/api/backups", base_url))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(!backups.is_empty(), "should have at least 1 backup");
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
        .filter(|e| e.path().extension().map_or(false, |ext| ext == "rpy"))
        .collect();
    assert!(!rpy_files.is_empty(), "should have .rpy files in tl/es/");
    let content = std::fs::read_to_string(rpy_files[0].path()).unwrap();
    assert!(content.contains("translate es"), "should have translate es blocks");
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
    std::fs::write(data.join("System.json"), r#"{"gameTitle":"Test","terms":{"basic":[],"commands":[],"params":[],"messages":{}}}"#).unwrap();

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

    let actor_entry = strings.entries.iter().find(|e| {
        e["source"].as_str().unwrap_or("").contains("Hero")
    });

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

    // Inject (creates backup) — short path for Windows
    let output_dir = std::env::temp_dir().join("locust_bk_out");
    let _ = std::fs::remove_dir_all(&output_dir);
    std::fs::create_dir_all(&output_dir).unwrap();
    client()
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

    // Corrupt a file
    let actors = tmpdir.path().join("data").join("Actors.json");
    let original_content = std::fs::read_to_string(&actors).unwrap();
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
    assert!(restored.contains("Hero"), "original content should be restored");
}
