//! User-supplied loose-font patch generation. The selected project supplies
//! engine and translated text; generation never edits the selected game tree.
use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;
use serde::{Deserialize, Serialize};

use locust_core::database::EntryFilter;
use locust_core::font_patch::{
    build_font_patch, combine_font_patch, write_font_patch, FontPatchReport,
};
use locust_core::patch::{PatchStatus, PatchStore};

use crate::{err, try_project_operation, ApiError, AppState};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct FontPatchRequest {
    game_path: String,
    source_font: String,
    target_path: String,
    language: String,
    output_path: String,
    #[serde(default)]
    base_patch: Option<String>,
}

#[derive(Serialize)]
pub(super) struct FontPatchResponse {
    output_path: String,
    report: FontPatchReport,
}

pub(super) async fn generate(
    State(state): State<Arc<AppState>>,
    Json(request): Json<FontPatchRequest>,
) -> Result<Json<FontPatchResponse>, ApiError> {
    for (name, value) in [
        ("game_path", request.game_path.as_str()),
        ("source_font", request.source_font.as_str()),
        ("target_path", request.target_path.as_str()),
        ("language", request.language.as_str()),
        ("output_path", request.output_path.as_str()),
    ]
    .into_iter()
    .chain(
        request
            .base_patch
            .as_deref()
            .map(|value| ("base_patch", value)),
    ) {
        if value.trim().is_empty() {
            return Err(err(
                StatusCode::BAD_REQUEST,
                format!("{name} must not be empty"),
            ));
        }
    }
    let exclusive = try_project_operation(&state).map_err(|m| err(StatusCode::CONFLICT, m))?;
    let project = state.current_project.read().await.clone().ok_or_else(|| {
        err(
            StatusCode::BAD_REQUEST,
            "Open a project before generating a font patch.",
        )
    })?;
    let db = state.db.clone();
    // The guard moves into the blocking task so a disconnected HTTP client
    // cannot release DB protection while generation is still running.
    let response = tokio::task::spawn_blocking(move || -> Result<FontPatchResponse, ApiError> {
        let _exclusive = exclusive;
        let selected = Path::new(&request.game_path).canonicalize().map_err(io_error)?;
        let game_root = if selected.is_file() {
            selected.parent().ok_or_else(|| err(StatusCode::BAD_REQUEST, "selected game file has no parent directory"))?.to_owned()
        } else if selected.is_dir() {
            selected
        } else {
            return Err(err(StatusCode::BAD_REQUEST, "game_path must be an existing game directory or file"));
        };
        locust_core::patch::zipsec::ensure_safe_store(&game_root).map_err(core_error)?;
        if !matches!(PatchStore::new(&game_root).status().map_err(core_error)?, PatchStatus::NotPatched) {
            return Err(err(StatusCode::CONFLICT, "Select a clean game copy or roll back its installed patch before generating a font patch."));
        }
        let output = normalized_output(&request.output_path)?;
        if output.starts_with(&game_root) {
            return Err(err(StatusCode::BAD_REQUEST, "output_path must be outside the selected game directory"));
        }
        if std::fs::symlink_metadata(&output).is_ok() {
            return Err(err(StatusCode::CONFLICT, "output_path already exists; choose a new file name"));
        }
        let entries = db.get_entries(&EntryFilter::default()).map_err(core_error)?;
        for entry in &entries {
            entry.require_current_translation().map_err(|message| err(StatusCode::BAD_REQUEST, message))?;
        }
        if !entries.iter().any(|entry| entry.translation.as_deref().is_some_and(|text| !text.trim().is_empty())) {
            return Err(err(StatusCode::BAD_REQUEST, "The current project has no translated text. Translate entries before generating a font patch."));
        }
        // An untranslated row remains visible in the untouched game's language,
        // including after a pivot replaced its editable source with English.
        let visible_text: Vec<&str> = entries.iter().map(|entry| {
            match entry.translation.as_deref().filter(|text| !text.trim().is_empty()) {
                Some(text) => Ok(text),
                None => entry.injection_source().map_err(|m| err(StatusCode::BAD_REQUEST, m)),
            }
        }).collect::<Result<_, ApiError>>()?;
        let (mut bytes, mut report) = build_font_patch(
            &game_root, Path::new(&request.source_font), &request.target_path,
            &project.format_id, &request.language, &visible_text,
        ).map_err(core_error)?;
        if let Some(base) = &request.base_patch {
            bytes = combine_font_patch(&game_root, Path::new(base), &bytes, &mut report).map_err(core_error)?;
        }
        // All coverage, baseline and archive checks completed before the first
        // output write. create_new remains the authoritative collision check.
        let output = write_font_patch(&output, &bytes).map_err(core_error)?;
        Ok(FontPatchResponse { output_path: output.to_string_lossy().into_owned(), report })
    }).await.map_err(|error| err(StatusCode::INTERNAL_SERVER_ERROR, error))??;
    Ok(Json(response))
}

fn normalized_output(value: &str) -> Result<PathBuf, ApiError> {
    let path = Path::new(value);
    let name = path
        .file_name()
        .ok_or_else(|| err(StatusCode::BAD_REQUEST, "output_path must name a file"))?;
    let parent = path
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let parent = parent.canonicalize().map_err(io_error)?;
    if !parent.is_dir() {
        return Err(err(
            StatusCode::BAD_REQUEST,
            "output_path parent must be an existing directory",
        ));
    }
    Ok(parent.join(name))
}

fn io_error(error: std::io::Error) -> ApiError {
    let status = if error.kind() == std::io::ErrorKind::AlreadyExists {
        StatusCode::CONFLICT
    } else {
        StatusCode::BAD_REQUEST
    };
    err(status, error)
}

fn core_error(error: locust_core::error::LocustError) -> ApiError {
    match error {
        locust_core::error::LocustError::IoError(error) => io_error(error),
        locust_core::error::LocustError::DatabaseError(error) => {
            err(StatusCode::INTERNAL_SERVER_ERROR, error)
        }
        other => err(StatusCode::BAD_REQUEST, other),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{create_test_state, start_test_server, ProjectInfo};
    use locust_core::models::{StringEntry, StringStatus};
    use locust_core::patch::{
        apply, rollback, verify, ApplyOptions, RollbackOptions, VerificationOutcome,
    };

    const FONT: &[u8] = include_bytes!("../../cli/tests/fixtures/synthetic-ascii.ttf");

    struct Fixture {
        temp: tempfile::TempDir,
        game: PathBuf,
        source: PathBuf,
        state: Arc<AppState>,
    }

    impl Fixture {
        async fn new(translation: Option<&str>) -> Self {
            let temp = tempfile::tempdir().unwrap();
            let game = temp.path().join("clean game");
            std::fs::create_dir_all(game.join("fonts")).unwrap();
            std::fs::write(game.join("fonts/body.ttf"), FONT).unwrap();
            std::fs::write(game.join("story.html"), b"<p>Original text</p>").unwrap();
            let source = temp.path().join("replacement.ttf");
            let mut replacement = FONT.to_vec();
            replacement.extend_from_slice(b"neutral test variant");
            std::fs::write(&source, replacement).unwrap();
            let state = create_test_state();
            *state.current_project.write().await = Some(ProjectInfo {
                path: game.join("story.html"),
                format_id: "html".into(),
                name: "Neutral test".into(),
                extraction_warnings: vec![],
                ..Default::default()
            });
            let mut entry = StringEntry::new(
                "row",
                "Japanese physical source",
                PathBuf::from("story.html"),
            );
            entry.translation = translation.map(str::to_owned);
            entry.status = StringStatus::Translated;
            state.db.save_entries(&[entry]).unwrap();
            Self {
                temp,
                game,
                source,
                state,
            }
        }

        fn request(&self, name: &str) -> serde_json::Value {
            serde_json::json!({
                "game_path": self.game.join("story.html"), "source_font": self.source,
                "target_path": "fonts/body.ttf", "language": "es",
                "output_path": self.temp.path().join(name)
            })
        }
    }

    async fn post(base: &str, request: &serde_json::Value) -> reqwest::Response {
        reqwest::Client::new()
            .post(format!("{base}/api/patch/font"))
            .json(request)
            .send()
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn http_font_patch_rejects_stale_text_before_output_and_accepts_explicit_review() {
        let f = Fixture::new(Some("Hello test")).await;
        let mut entries = f.state.db.get_entries(&EntryFilter::default()).unwrap();
        let current_source_sha256 = entries[0].source_hash();
        entries[0].metadata.insert(
            locust_core::models::STALE_TRANSLATION_METADATA_KEY.into(),
            serde_json::json!({"version": 1, "translated_source_sha256": "a".repeat(64), "current_source_sha256": current_source_sha256}),
        );
        f.state.db.save_entries(&entries).unwrap();
        let (base, server) = start_test_server(f.state.clone()).await;
        let response = post(&base, &f.request("stale.zip")).await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert!(response.text().await.unwrap().contains("stale translation"));
        assert!(!f.temp.path().join("stale.zip").exists());
        assert_eq!(std::fs::read(f.game.join("fonts/body.ttf")).unwrap(), FONT);
        f.state
            .db
            .update_entry_status("row", StringStatus::Reviewed)
            .await
            .unwrap();
        assert_eq!(
            post(&base, &f.request("reviewed.zip")).await.status(),
            StatusCode::OK
        );
        server.abort();
    }

    #[tokio::test]
    async fn http_font_patch_generates_verifies_applies_and_rolls_back_without_generation_mutation()
    {
        let f = Fixture::new(Some("Hello test")).await;
        let original_source = std::fs::read(&f.source).unwrap();
        let (base, server) = start_test_server(f.state.clone()).await;
        let response = post(&base, &f.request("font.zip")).await;
        assert_eq!(
            response.status(),
            StatusCode::OK,
            "{}",
            response.text().await.unwrap()
        );
        let body: serde_json::Value = response.json().await.unwrap();
        assert_eq!(body["report"]["manifest"]["engine"], "html-game");
        assert_eq!(body["report"]["coverage"]["has_full_coverage"], true);
        assert!(body["report"]["limitations"]
            .as_str()
            .is_some_and(|s| !s.is_empty()));
        let output = PathBuf::from(body["output_path"].as_str().unwrap());
        assert_eq!(std::fs::read(f.game.join("fonts/body.ttf")).unwrap(), FONT);
        assert_eq!(std::fs::read(&f.source).unwrap(), original_source);
        assert_eq!(
            std::fs::read(f.game.join("story.html")).unwrap(),
            b"<p>Original text</p>"
        );
        assert!(!f.game.join(".locust").exists());
        assert_eq!(
            verify(&f.game, &output).unwrap().outcome,
            VerificationOutcome::Clean
        );
        apply(&f.game, &output, ApplyOptions::default(), |_| {}).unwrap();
        assert_eq!(
            std::fs::read(f.game.join("fonts/body.ttf")).unwrap(),
            original_source
        );
        rollback(&f.game, RollbackOptions::default()).unwrap();
        assert_eq!(std::fs::read(f.game.join("fonts/body.ttf")).unwrap(), FONT);
        assert_eq!(
            f.state
                .project_exclusive
                .load(std::sync::atomic::Ordering::SeqCst),
            0
        );
        server.abort();
    }

    #[tokio::test]
    async fn http_font_patch_rejects_output_collision_escape_empty_and_missing_coverage() {
        let f = Fixture::new(Some("Hello")).await;
        let (base, server) = start_test_server(f.state.clone()).await;
        let output = f.temp.path().join("existing.zip");
        std::fs::write(&output, b"do not overwrite").unwrap();
        assert_eq!(
            post(&base, &f.request("existing.zip")).await.status(),
            StatusCode::CONFLICT
        );
        assert_eq!(std::fs::read(&output).unwrap(), b"do not overwrite");
        for target in ["../body.ttf", "/body.ttf", "fonts/../../body.ttf"] {
            let mut request = f.request("invalid.zip");
            request["target_path"] = serde_json::json!(target);
            assert_eq!(
                post(&base, &request).await.status(),
                StatusCode::BAD_REQUEST
            );
            assert!(!f.temp.path().join("invalid.zip").exists());
        }
        for field in [
            "game_path",
            "source_font",
            "target_path",
            "language",
            "output_path",
            "base_patch",
        ] {
            let mut request = f.request("empty.zip");
            request[field] = serde_json::json!("  ");
            assert_eq!(
                post(&base, &request).await.status(),
                StatusCode::BAD_REQUEST
            );
        }
        let mut inside = f.request("outside.zip");
        inside["output_path"] = serde_json::json!(f.game.join("created.zip"));
        assert_eq!(post(&base, &inside).await.status(), StatusCode::BAD_REQUEST);
        assert!(!f.game.join("created.zip").exists());
        f.state
            .db
            .save_translation("row", "日本語", "manual")
            .await
            .unwrap();
        let response = post(&base, &f.request("coverage.zip")).await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert!(response.text().await.unwrap().contains("lacks"));
        assert!(!f.temp.path().join("coverage.zip").exists());
        // The request cannot replace real DB text with an invented sample.
        let mut spoof = f.request("spoof.zip");
        spoof["translations"] = serde_json::json!(["Hello"]);
        assert_eq!(
            post(&base, &spoof).await.status(),
            StatusCode::UNPROCESSABLE_ENTITY
        );
        assert_eq!(std::fs::read(f.game.join("fonts/body.ttf")).unwrap(), FONT);
        server.abort();
    }

    #[tokio::test]
    async fn http_font_patch_rejects_absent_project_zero_text_and_active_translation() {
        let f = Fixture::new(None).await;
        let (base, server) = start_test_server(f.state.clone()).await;
        let response = post(&base, &f.request("empty.zip")).await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert!(response
            .text()
            .await
            .unwrap()
            .contains("no translated text"));
        *f.state.current_project.write().await = None;
        assert_eq!(
            post(&base, &f.request("no-project.zip")).await.status(),
            StatusCode::BAD_REQUEST
        );
        let (tx, _) = tokio::sync::broadcast::channel(8);
        let task = tokio::spawn(std::future::pending::<()>());
        f.state.active_jobs.insert(
            "active".into(),
            crate::JobState {
                abort_handle: task.abort_handle(),
                progress_tx: tx,
                replay: Arc::new(std::sync::Mutex::new(Vec::new())),
                cancel: tokio_util::sync::CancellationToken::new(),
                kind: crate::JobKind::Translate,
                patch_game_key: None,
            },
        );
        assert_eq!(
            post(&base, &f.request("active.zip")).await.status(),
            StatusCode::CONFLICT
        );
        assert!(!f.temp.path().join("active.zip").exists());
        assert_eq!(
            f.state
                .project_exclusive
                .load(std::sync::atomic::Ordering::SeqCst),
            0
        );
        task.abort();
        server.abort();
    }

    #[tokio::test]
    async fn http_font_patch_holds_project_guard_across_await() {
        let f = Fixture::new(Some("Hello")).await;
        let (base, server) = start_test_server(f.state.clone()).await;
        let project_write = f.state.current_project.write().await;
        let url = base.clone();
        let request = f.request("guard.zip");
        let pending = tokio::spawn(async move { post(&url, &request).await });
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while f
                .state
                .project_exclusive
                .load(std::sync::atomic::Ordering::SeqCst)
                == 0
            {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        let open = reqwest::Client::new()
            .post(format!("{base}/api/project/open"))
            .json(&serde_json::json!({"path": f.game, "format_id": "html"}))
            .send()
            .await
            .unwrap();
        assert_eq!(open.status(), StatusCode::CONFLICT);
        assert!(open.text().await.unwrap().contains("project operation"));
        drop(project_write);
        assert_eq!(pending.await.unwrap().status(), StatusCode::OK);
        assert_eq!(
            f.state
                .project_exclusive
                .load(std::sync::atomic::Ordering::SeqCst),
            0
        );
        server.abort();
    }

    fn base_translation_patch(f: &Fixture, engine: &str, language: &str) -> PathBuf {
        let injected = f.temp.path().join(format!("injected-{engine}-{language}"));
        std::fs::create_dir(&injected).unwrap();
        std::fs::write(injected.join("story.html"), b"<p>Translated text</p>").unwrap();
        f.state
            .db
            .record_injection(Some(language), &injected, &[injected.join("story.html")])
            .unwrap();
        let output = f.temp.path().join(format!("base-{engine}-{language}.zip"));
        locust_core::patch::pack_injection_recording(
            &f.state.db,
            locust_core::patch::PackOptions {
                game: None,
                game_path: injected,
                lang: Some(language.into()),
                output: output.clone(),
                pristine: Some(f.game.clone()),
                engine: Some(engine.into()),
                project: f.temp.path().join("fixture.db"),
                require_pristine: true,
            },
        )
        .unwrap();
        output
    }

    #[tokio::test]
    async fn http_font_patch_combines_text_patch_and_rejects_mismatched_base() {
        let f = Fixture::new(Some("Hello")).await;
        let (base, server) = start_test_server(f.state.clone()).await;
        let translation = base_translation_patch(&f, "html", "es");
        let base_bytes = std::fs::read(&translation).unwrap();
        let mut request = f.request("combined.zip");
        request["base_patch"] = serde_json::json!(translation);
        let response = post(&base, &request).await;
        assert_eq!(
            response.status(),
            StatusCode::OK,
            "{}",
            response.text().await.unwrap()
        );
        let body: serde_json::Value = response.json().await.unwrap();
        assert_eq!(
            body["report"]["manifest"]["files"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        assert_eq!(std::fs::read(&translation).unwrap(), base_bytes);
        let combined = PathBuf::from(body["output_path"].as_str().unwrap());
        assert_eq!(
            verify(&f.game, &combined).unwrap().outcome,
            VerificationOutcome::Clean
        );
        apply(&f.game, &combined, ApplyOptions::default(), |_| {}).unwrap();
        assert_eq!(
            std::fs::read(f.game.join("story.html")).unwrap(),
            b"<p>Translated text</p>"
        );
        assert_eq!(
            std::fs::read(f.game.join("fonts/body.ttf")).unwrap(),
            std::fs::read(&f.source).unwrap()
        );
        assert_eq!(
            post(&base, &f.request("already-patched.zip"))
                .await
                .status(),
            StatusCode::CONFLICT
        );
        rollback(&f.game, RollbackOptions::default()).unwrap();
        assert_eq!(
            std::fs::read(f.game.join("story.html")).unwrap(),
            b"<p>Original text</p>"
        );
        assert_eq!(std::fs::read(f.game.join("fonts/body.ttf")).unwrap(), FONT);
        for (engine, language) in [("renpy", "es"), ("html", "en")] {
            let mismatched = base_translation_patch(&f, engine, language);
            let mut request = f.request("mismatch.zip");
            request["base_patch"] = serde_json::json!(mismatched);
            let response = post(&base, &request).await;
            assert_eq!(response.status(), StatusCode::BAD_REQUEST);
            assert!(response.text().await.unwrap().contains("engine/language"));
            assert!(!f.temp.path().join("mismatch.zip").exists());
        }
        server.abort();
    }
}
