use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;
use locust_core::extraction::MultiLangInjector;
use locust_core::models::OutputMode;
use serde::Deserialize;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::mpsc;

use crate::{
    err, run_owned_project_operation, try_project_operation, ApiError, AppState,
    INJECT_EMPTY_LANGUAGES_MESSAGE,
};

#[derive(Deserialize)]
pub(crate) struct InjectRequest {
    project_path: String,
    format_id: String,
    /// Ignored when `direct` is true.
    #[serde(default)]
    mode: Option<OutputMode>,
    languages: Vec<String>,
    output_dir: Option<String>,
    /// When true, inject into the game tree in place and record for pack
    /// (CLI `--direct`). Default false preserves Replace/Add MultiLangInjector.
    #[serde(default)]
    direct: bool,
    /// Opt-in known-capacity preflight, supported only for Replace copies.
    #[serde(default)]
    require_binary_slot_fit: bool,
}

/// Unblocking advice attached to a containment failure when recording an
/// injection made through the HTTP API. The server cannot know the exact
/// paths the user's shell will use, so it names the shape of the working
/// command rather than a copy-pasteable line.
const INJECT_RECORD_REMEDY: &str =
    "Restore the original game files from the backup listed above (or from a \
     clean copy) first — this engine writes translations into the ORIGINAL \
     tree, and a re-run against the mutated tree writes and records nothing. \
     Then record the injection with the CLI's direct mode: locust inject \
     <game folder> -P <project db> --direct -l <lang> — `locust patch` packs \
     from that recording.";

pub(crate) async fn inject(
    State(state): State<Arc<AppState>>,
    Json(req): Json<InjectRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    // Same guard as CLI: empty languages used to return 200 with zero work and
    // zero recording — a silent no-op that becomes an untranslatable patch later.
    if req.languages.is_empty() {
        return Err(err(StatusCode::BAD_REQUEST, INJECT_EMPTY_LANGUAGES_MESSAGE));
    }
    if req.languages.len() > 1 {
        return Err(err(
            StatusCode::BAD_REQUEST,
            "One project database can inject only one target language. Use a separate pivot database for each target language.",
        ));
    }

    if req.require_binary_slot_fit
        && (req.direct
            || req
                .mode
                .as_ref()
                .is_some_and(|mode| *mode != OutputMode::Replace))
    {
        return Err(err(
            StatusCode::BAD_REQUEST,
            "require_binary_slot_fit is supported only for Replace mode",
        ));
    }

    // Hold for the whole request (including awaits) so open cannot reopen mid-inject.
    let guard = try_project_operation(&state).map_err(|m| err(StatusCode::CONFLICT, m))?;
    run_owned_project_operation(guard, async move { inject_owned(state, req).await })
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e))?
}

async fn inject_owned(
    state: Arc<AppState>,
    req: InjectRequest,
) -> Result<Json<serde_json::Value>, ApiError> {
    if state.current_project.read().await.is_none() {
        return Err(err(StatusCode::BAD_REQUEST, "no project open"));
    }
    if req.direct {
        let game_path = PathBuf::from(&req.project_path);
        let format_id = req.format_id.clone();
        let languages = req.languages.clone();
        let registry = state.format_registry.clone();
        let db = state.db.clone();
        let backup = state.backup_manager.clone();
        let report = tokio::task::spawn_blocking(move || {
            locust_core::extraction::inject_direct(
                &registry, &db, &backup, &game_path, &format_id, &languages,
            )
        })
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e))?
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e))?;
        return Ok(Json(serde_json::to_value(report).unwrap_or_default()));
    }

    let mode = req.mode.unwrap_or(OutputMode::Replace);
    let injector = MultiLangInjector::new(
        state.format_registry.clone(),
        state.db.clone(),
        state.backup_manager.clone(),
    );
    let (tx, mut rx) = mpsc::channel(100);
    tokio::spawn(async move { while rx.recv().await.is_some() {} });

    let languages = req.languages.clone();
    let report = injector
        .inject_with_options(
            &PathBuf::from(&req.project_path),
            &req.format_id,
            locust_core::extraction::MultiLangInjectionOptions {
                mode,
                require_binary_slot_fit: req.require_binary_slot_fit,
            },
            req.languages,
            req.output_dir.map(PathBuf::from),
            tx,
        )
        .await
        .map_err(|e| {
            // Only the new count-only core refusal is a 422. Other injection
            // errors retain their existing status and diagnostic contract.
            let capacity_refusal = req.require_binary_slot_fit
                && matches!(&e, locust_core::error::LocustError::InjectionError(message)
                    if message.strip_suffix(" translation(s) exceed known binary slot capacity; Replace is refused before backup or output; run validation and resolve the capacity issues")
                        .is_some_and(|count| !count.is_empty() && count.bytes().all(|b| b.is_ascii_digit())));
            err(
                if capacity_refusal {
                    StatusCode::UNPROCESSABLE_ENTITY
                } else {
                    StatusCode::INTERNAL_SERVER_ERROR
                },
                e,
            )
        })?;

    // Persist what each language's injection wrote — `locust patch` packs
    // exclusively from this recording, so an inject seam that skips it
    // produces projects that can never be packed. Surface outcomes on the
    // JSON body so the desktop never celebrates a zero-write recording.
    let outcomes = locust_core::extraction::record_multilang_injection(
        &state.db,
        &report,
        &languages,
        &|_lang| INJECT_RECORD_REMEDY.to_string(),
    )
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e))?;

    let mut body = serde_json::to_value(&report).unwrap_or_default();
    if let Some(obj) = body.as_object_mut() {
        obj.insert(
            "outcomes".into(),
            serde_json::to_value(outcomes).unwrap_or_default(),
        );
    }
    Ok(Json(body))
}

// ─── Register language (RPG Maker multi-lang UI) ───────────────────────────

#[derive(Deserialize)]
pub(crate) struct RegisterLangRequest {
    /// Game root (folder with `js/plugins.js` and/or `data/`).
    game_path: String,
    /// Language code (e.g. `es`).
    lang: String,
    /// Display label (e.g. `Español`).
    label: String,
}

/// Patch Iavra/VisuMZ language lists + Map boot choices so a new lang is
/// selectable in the game UI. Writes `*.bak-locust` siblings (same as CLI).
pub(crate) async fn register_lang(
    State(state): State<Arc<AppState>>,
    Json(req): Json<RegisterLangRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let game_path = PathBuf::from(req.game_path.trim());
    if req.game_path.trim().is_empty() || !game_path.is_dir() {
        return Err(err(
            StatusCode::BAD_REQUEST,
            format!(
                "game_path must be an existing directory (got {:?})",
                req.game_path
            ),
        ));
    }
    let lang = req.lang;
    let label = req.label;
    let guard =
        try_project_operation(&state).map_err(|message| err(StatusCode::CONFLICT, message))?;
    let db = state
        .current_project
        .read()
        .await
        .as_ref()
        .map(|_| state.db.clone());
    let report = tokio::task::spawn_blocking(move || {
        let _guard = guard;
        locust_formats::rpgmaker_lang::register_language_with_db(
            &game_path,
            &lang,
            &label,
            db.as_deref(),
        )
    })
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e))?
    .map_err(|e| {
        // Invalid lang/label is a client error; other failures stay 500.
        let msg = e.to_string();
        if msg.contains("invalid language") || msg.contains("label must not be empty") {
            err(StatusCode::BAD_REQUEST, msg)
        } else {
            err(StatusCode::INTERNAL_SERVER_ERROR, msg)
        }
    })?;
    Ok(Json(serde_json::to_value(report).unwrap_or_default()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::routes::test_support::*;
    use crate::*;
    use locust_core::backup::BackupManager;
    use locust_core::database::EntryFilter;
    use locust_core::extraction::FormatRegistry;
    use locust_core::models::{StringEntry, StringStatus};
    use std::path::Path;

    /// Real core dispatch/recording, with an intentionally partial plugin.
    struct CapacitySentinel;

    impl locust_core::extraction::FormatPlugin for CapacitySentinel {
        fn id(&self) -> &str {
            "capacity-sentinel"
        }
        fn name(&self) -> &str {
            "Capacity sentinel"
        }
        fn supported_extensions(&self) -> &[&str] {
            &["bin"]
        }
        fn extract(&self, _: &Path) -> locust_core::error::Result<Vec<StringEntry>> {
            unreachable!()
        }
        fn inject(
            &self,
            path: &Path,
            _: &[StringEntry],
        ) -> locust_core::error::Result<locust_core::extraction::InjectionReport> {
            let file = path.join("story.bin");
            std::fs::write(&file, b"partial output")?;
            Ok(locust_core::extraction::InjectionReport {
                files_modified: 1,
                strings_written: 1,
                strings_skipped: 1,
                files_written: vec![file],
                skip_reasons: Default::default(),
                warnings: vec![],
            })
        }
    }

    #[tokio::test]
    async fn binary_slot_fit_http_transport_contract() {
        for (flag, target, direct, mode, status) in [
            (Some(true), "oversize", false, None, 422),
            (Some(true), "fit", false, None, 200),
            (Some(true), "fit", false, Some("replace"), 200),
            (None, "oversize", false, None, 200),
            (Some(false), "oversize", false, None, 200),
            (Some(true), "fit", true, None, 400),
            (Some(true), "fit", false, Some("add"), 400),
            (Some(false), "oversize", true, Some("add"), 200),
            (None, "oversize", true, Some("add"), 200),
        ] {
            let mut state = create_test_state();
            let root = state.config_path.parent().unwrap().to_path_buf();
            let game = root.join("game");
            let output = root.join("output");
            let backups = root.join("backups");
            std::fs::create_dir(&game).unwrap();
            std::fs::write(game.join("story.bin"), b"original").unwrap();
            let mut registry = FormatRegistry::new();
            registry.register(Box::new(CapacitySentinel));
            let mutable = Arc::get_mut(&mut state).unwrap();
            mutable.format_registry = Arc::new(registry);
            mutable.backup_manager = Arc::new(BackupManager::new(backups.clone()));
            mark_project_open(&state).await;
            let mut row = StringEntry::new("slot", "abc", "story.bin".into());
            row.translation = Some(target.into());
            row.status = StringStatus::Translated;
            row.metadata
                .insert("binary_slot".into(), serde_json::json!("utf8"));
            state.db.save_entries(&[row]).unwrap();
            let before =
                serde_json::to_value(state.db.get_entries(&EntryFilter::default()).unwrap())
                    .unwrap();
            let mut request = serde_json::json!({
                "project_path": game, "format_id": "capacity-sentinel",
                "languages": ["es"], "output_dir": output, "direct": direct
            });
            if let Some(flag) = flag {
                request["require_binary_slot_fit"] = flag.into();
            }
            if let Some(mode) = mode {
                request["mode"] = mode.into();
            }
            let (url, server) = start_test_server(state.clone()).await;
            let response = client()
                .post(format!("{url}/api/inject"))
                .json(&request)
                .send()
                .await
                .unwrap();
            let actual_status = response.status();
            let body = response.text().await.unwrap();
            server.abort();
            let _ = server.await;
            assert_eq!(actual_status.as_u16(), status, "{request}: {body}");
            if status != 200 {
                let message = if status == 422 {
                    "injection error: 1 translation(s) exceed known binary slot capacity; Replace is refused before backup or output; run validation and resolve the capacity issues"
                } else {
                    "require_binary_slot_fit is supported only for Replace mode"
                };
                assert_eq!(body, message);
                assert!(!backups.exists());
                assert!(!output.exists());
                assert!(state.db.get_injection(Some("es")).unwrap().is_none());
                assert_eq!(std::fs::read_dir(&game).unwrap().count(), 1);
            } else {
                let body: serde_json::Value = serde_json::from_str(&body).unwrap();
                assert!(backups.exists());
                assert!(state.db.get_injection(Some("es")).unwrap().is_some());
                if !direct {
                    assert_eq!(body["reports"]["es"]["strings_skipped"], 1);
                    assert!(output.exists());
                }
            }
            assert_eq!(
                serde_json::to_value(state.db.get_entries(&EntryFilter::default()).unwrap())
                    .unwrap(),
                before
            );
            assert_eq!(
                std::fs::read(game.join("story.bin")).unwrap(),
                if direct && status == 200 {
                    b"partial output".as_slice()
                } else {
                    b"original".as_slice()
                }
            );
        }
    }

    #[tokio::test]
    async fn binary_slot_fit_http_unity_real_plugin() {
        for (flag, oversize) in [
            (Some(true), true),
            (Some(true), false),
            (None, true),
            (Some(false), true),
        ] {
            let mut state = create_test_state();
            let root = state.config_path.parent().unwrap().to_path_buf();
            let game = root.join("game");
            let output = root.join("output");
            let backups = root.join("backups");
            Arc::get_mut(&mut state).unwrap().backup_manager =
                Arc::new(BackupManager::new(backups.clone()));
            std::fs::create_dir_all(game.join("TestGame_Data")).unwrap();
            std::fs::write(
                game.join("UnityPlayer.dll"),
                b"fixture marker, not executable",
            )
            .unwrap();
            // Same minimal length-prefixed heuristic layout as Unity's
            // create_unity_fixture; exercises the actual extractor and writer.
            let mut bytes = vec![0; 64];
            for text in ["Hello World", "Press any key to continue"] {
                bytes.extend_from_slice(&(text.len() as u32).to_le_bytes());
                bytes.extend_from_slice(text.as_bytes());
                bytes.push(0);
                bytes.extend_from_slice(&[0xFF; 8]);
            }
            bytes.extend_from_slice(&[0; 32]);
            let asset = game.join("TestGame_Data/resources.assets");
            std::fs::write(&asset, &bytes).unwrap();
            let plugin = state.format_registry.get("unity").unwrap();
            let mut rows = plugin.extract(&game).unwrap();
            assert_eq!(rows.len(), 2);
            for row in &mut rows {
                assert_eq!(row.metadata["binary_slot"], "utf8");
                row.translation = Some(
                    if oversize && row.source == "Hello World" {
                        "This translation exceeds the physical slot"
                    } else {
                        "Hola mundo"
                    }
                    .into(),
                );
                row.status = StringStatus::Translated;
            }
            state.db.save_entries(&rows).unwrap();
            mark_project_open(&state).await;
            let mut request = serde_json::json!({
                "project_path": game, "format_id": "unity",
                "languages": ["es"], "output_dir": output
            });
            if let Some(flag) = flag {
                request["require_binary_slot_fit"] = flag.into();
            }
            let (url, server) = start_test_server(state.clone()).await;
            let response = client()
                .post(format!("{url}/api/inject"))
                .json(&request)
                .send()
                .await
                .unwrap();
            let status = response.status();
            let body = response.text().await.unwrap();
            server.abort();
            let _ = server.await;
            if flag == Some(true) && oversize {
                assert_eq!(status, 422, "{body}");
                assert!(!body.contains("Hello World") && !body.contains("This translation"));
                assert!(!backups.exists() && !output.exists());
                assert!(state.db.get_injection(Some("es")).unwrap().is_none());
            } else {
                assert_eq!(status, 200, "{body}");
                let body: serde_json::Value = serde_json::from_str(&body).unwrap();
                assert_eq!(
                    body["reports"]["es"]["strings_skipped"],
                    usize::from(oversize)
                );
                assert_eq!(
                    body["reports"]["es"]["strings_written"],
                    if oversize { 1 } else { 2 }
                );
                assert!(state.db.get_injection(Some("es")).unwrap().is_some());
                let copied = PathBuf::from(body["injected_roots"]["es"].as_str().unwrap());
                let extracted = plugin.extract(&copied).unwrap();
                assert_eq!(
                    extracted
                        .iter()
                        .filter(|r| r.source == "Hola mundo")
                        .count(),
                    if oversize { 1 } else { 2 }
                );
            }
            assert_eq!(std::fs::read(&asset).unwrap(), bytes);
        }
    }

    fn registration_fixture(root: &Path) -> Vec<(&'static str, Vec<u8>)> {
        let map = serde_json::json!({"events": [null, {"pages": [{"list": [
            {"code":102,"indent":0,"parameters":[["日本語","ENGLISH","中文"],-1,0,1,0]},
            {"code":402,"indent":0,"parameters":[1,"ENGLISH"]},
            {"code":355,"indent":1,"parameters":["IAVRA.MasterLocalization.I18N.language = 'en';"]},
            {"code":0,"indent":1,"parameters":[]},
            {"code":404,"indent":0,"parameters":[]}
        ]}]}]});
        let plugins = r#"var $plugins = [{"name":"Iavra_MZ_Localization_byNeomaStudio","status":true,"parameters":{"Languages":"jp, en, zh","Language Labels":"en:English, jp:日本語, zh:中文"}},{"name":"VisuMZ_1_OptionsCore","status":true,"parameters":{"Language":"const langs = ['jp', 'en', 'zh']; IAVRA.MasterLocalization.I18N.language = langs[value];"}}];"#;
        let files = vec![
            ("js/rmmz_core.js", b"// MZ".to_vec()),
            ("js/plugins.js", plugins.as_bytes().to_vec()),
            (
                "data/System.json",
                br#"{"gameTitle":"Registration fixture"}"#.to_vec(),
            ),
            ("data/Map001.json", serde_json::to_vec(&map).unwrap()),
            ("data/lang_g_en.json", br#"{"greeting":"Hello"}"#.to_vec()),
        ];
        for (rel, bytes) in &files {
            let path = root.join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, bytes).unwrap();
        }
        files
    }

    fn registration_entries(state: &Arc<AppState>, game: &Path) -> String {
        use locust_core::extraction::FormatPlugin;
        let plugin = locust_formats::rpgmaker_mv::RpgMakerMvPlugin::new();
        let mut entries = plugin.extract(game).unwrap();
        entries.retain(|entry| entry.file_path.ends_with("lang_g_en.json"));
        assert_eq!(entries.len(), 1);
        entries[0].translation = Some("Hola".into());
        entries[0].status = StringStatus::Translated;
        state.db.save_entries(&entries).unwrap();
        plugin.id().into()
    }

    async fn registration_add(state: &Arc<AppState>, game: &Path) {
        let format = registration_entries(state, game);
        let injector = MultiLangInjector::new(
            state.format_registry.clone(),
            state.db.clone(),
            state.backup_manager.clone(),
        );
        let (tx, _rx) = mpsc::channel(32);
        let report = injector
            .inject(game, &format, OutputMode::Add, vec!["es".into()], None, tx)
            .await
            .unwrap();
        assert!(
            report.languages_failed.is_empty(),
            "{:?}",
            report.languages_failed
        );
        assert_eq!(report.languages_processed, ["es"]);
        assert!(game.join("data/lang_g_es.json").is_file());
    }

    #[tokio::test]
    async fn pack_add_registration_roundtrip() {
        use locust_core::patch::pack::{pack_with_pristine_backup, PackOptions};
        use locust_core::patch::{apply, rollback, ApplyOptions, RollbackOptions};
        let work = tempfile::tempdir().unwrap();
        let author = work.path().join("author");
        let player = work.path().join("player");
        let originals = registration_fixture(&author);
        registration_fixture(&player);
        let (url, server, state) = setup_with_state().await;
        registration_add(&state, &author).await;
        *state.current_project.write().await = Some(ProjectInfo {
            path: author.clone(),
            ..ProjectInfo::default()
        });
        let response = client()
            .post(format!("{url}/api/register-lang"))
            .json(&serde_json::json!({"game_path":author,"lang":"es","label":"Español"}))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200, "{}", response.text().await.unwrap());
        for rel in ["js/plugins.js", "data/Map001.json"] {
            assert_ne!(
                std::fs::read(author.join(rel)).unwrap(),
                std::fs::read(player.join(rel)).unwrap()
            );
        }
        let registered = state.db.get_injection(Some("es")).unwrap();
        let registration_backups = state.db.registration_backups(Some("es")).unwrap();
        let again = locust_formats::rpgmaker_lang::register_language_with_db(
            &author,
            "es",
            "Español",
            Some(&state.db),
        )
        .unwrap();
        assert!(!again.plugins_js);
        assert!(again.maps_patched.is_empty());
        assert_eq!(state.db.get_injection(Some("es")).unwrap(), registered);
        let mut entries = state.db.get_entries(&EntryFilter::default()).unwrap();
        entries[0].translation = Some("Buenos días".into());
        state.db.save_entries(&entries).unwrap();
        let injector = MultiLangInjector::new(
            state.format_registry.clone(),
            state.db.clone(),
            state.backup_manager.clone(),
        );
        let (tx, _rx) = mpsc::channel(32);
        let format = state.format_registry.detect(&author).unwrap().id();
        let revised = injector
            .inject(
                &author,
                format,
                OutputMode::Add,
                vec!["es".into()],
                None,
                tx,
            )
            .await
            .unwrap();
        assert!(
            revised.languages_failed.is_empty(),
            "{:?}",
            revised.languages_failed
        );
        assert_eq!(
            state.db.registration_backups(Some("es")).unwrap(),
            registration_backups
        );
        let zip = work.path().join("spanish.zip");
        let packed = pack_with_pristine_backup(
            &state.db,
            PackOptions {
                game_path: author.clone(),
                lang: Some("es".into()),
                output: zip.clone(),
                pristine: None,
                engine: Some("rpgmaker-mv".into()),
                project: state.db.path(),
                require_pristine: false,
                game: None,
            },
            &state.backup_manager,
            None,
            true,
        )
        .unwrap();
        let applied = apply(&player, &zip, ApplyOptions::default(), |_| {}).unwrap();
        for rel in ["js/plugins.js", "data/Map001.json", "data/lang_g_es.json"] {
            assert_eq!(
                std::fs::read(player.join(rel)).unwrap(),
                std::fs::read(author.join(rel)).unwrap(),
                "distributed {rel} must match the registered author copy"
            );
        }
        assert_eq!(packed.files_packed, 3);
        assert_eq!(packed.tier, "strict");
        assert_eq!((applied.replaced, applied.added), (2, 1));
        let rolled_back = rollback(&player, RollbackOptions::default()).unwrap();
        assert_eq!((rolled_back.restored, rolled_back.deleted), (2, 1));
        for (rel, bytes) in originals {
            assert_eq!(
                std::fs::read(player.join(rel)).unwrap(),
                bytes,
                "rollback {rel}"
            );
        }
        assert!(!player.join("data/lang_g_es.json").exists());
        server.abort();
    }

    #[tokio::test]
    async fn pack_add_registration_without_db_keeps_recording() {
        let game = tempfile::tempdir().unwrap();
        registration_fixture(game.path());
        let state = create_test_state();
        registration_add(&state, game.path()).await;
        let before = state.db.get_injection(Some("es")).unwrap();
        let report =
            locust_formats::rpgmaker_lang::register_language(game.path(), "es", "Español").unwrap();
        assert!(report.plugins_js);
        assert_eq!(report.maps_patched.len(), 1);
        assert_eq!(state.db.get_injection(Some("es")).unwrap(), before);
    }

    #[tokio::test]
    async fn pack_add_registration_without_matching_recording_keeps_database() {
        let author = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        registration_fixture(author.path());
        registration_fixture(other.path());
        let state = create_test_state();
        let report = locust_formats::rpgmaker_lang::register_language_with_db(
            other.path(),
            "es",
            "Español",
            Some(&state.db),
        )
        .unwrap();
        assert!(report.plugins_js);
        assert!(state.db.list_recorded_langs().unwrap().is_empty());
        registration_add(&state, author.path()).await;
        let before = state.db.get_injection(Some("es")).unwrap();
        // A different root, then a different language, must not change this recording.
        registration_fixture(other.path());
        for (game, lang, label) in [
            (other.path(), "es", "Español"),
            (author.path(), "fr", "Français"),
        ] {
            locust_formats::rpgmaker_lang::register_language_with_db(
                game,
                lang,
                label,
                Some(&state.db),
            )
            .unwrap();
        }
        assert_eq!(state.db.get_injection(Some("es")).unwrap(), before);
        assert_eq!(
            state.db.list_recorded_langs().unwrap(),
            vec![Some("es".into())]
        );
    }

    #[tokio::test]
    async fn pack_add_registration_leaves_direct_recording_untouched() {
        let game = tempfile::tempdir().unwrap();
        registration_fixture(game.path());
        let (url, server, state) = setup_with_state().await;
        let format = registration_entries(&state, game.path());
        locust_core::extraction::inject_direct(
            &state.format_registry,
            &state.db,
            &state.backup_manager,
            game.path(),
            &format,
            &["es".into()],
        )
        .unwrap();
        let before = state.db.get_injection(Some("es")).unwrap();
        assert!(before.is_some());
        *state.current_project.write().await = Some(ProjectInfo {
            path: game.path().to_owned(),
            ..ProjectInfo::default()
        });
        let response = client()
            .post(format!("{url}/api/register-lang"))
            .json(&serde_json::json!({"game_path":game.path(),"lang":"es","label":"Español"}))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200, "{}", response.text().await.unwrap());
        assert!(std::fs::read_to_string(game.path().join("js/plugins.js"))
            .unwrap()
            .contains("Español"));
        assert_eq!(state.db.get_injection(Some("es")).unwrap(), before);
        server.abort();
    }

    #[tokio::test]
    async fn injection_requires_an_open_project() {
        let (url, _h, state) = setup_with_state().await;
        let game = tempfile::tempdir().unwrap();
        let file = game.path().join("story.html");
        let original = b"<p>Hello</p>";
        std::fs::write(&file, original).unwrap();
        state
            .db
            .save_entries(&[translated("leftover", "Hello", "story.html", "Hola")])
            .unwrap();
        let before = state.db.get_entry("leftover").unwrap().unwrap();
        let recordings_before = state.db.list_recorded_langs().unwrap();

        for direct in [false, true] {
            let response = client()
                .post(format!("{url}/api/inject"))
                .json(&serde_json::json!({
                    "project_path": game.path(),
                    "format_id": "html-game",
                    "languages": ["es"],
                    "direct": direct
                }))
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), 400);
            assert_eq!(response.text().await.unwrap(), "no project open");
            assert_eq!(
                serde_json::to_value(state.db.get_entry("leftover").unwrap().unwrap()).unwrap(),
                serde_json::to_value(&before).unwrap()
            );
            assert_eq!(state.db.list_recorded_langs().unwrap(), recordings_before);
            assert_eq!(std::fs::read(&file).unwrap(), original);
        }
    }
}
