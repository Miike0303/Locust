//! HTTP entry points for recovery after interrupted translation insertion.
use super::*;
use locust_core::injection_transaction::{self, InjectionRecoveryOptions};

fn map_recovery_error(error: locust_core::LocustError) -> ApiError {
    use locust_core::LocustError;
    match error {
        LocustError::InjectionError(message) => err(StatusCode::CONFLICT, message),
        LocustError::PatchError(message) if message.starts_with("game busy:") => {
            err(StatusCode::CONFLICT, message)
        }
        LocustError::IoError(error) if error.kind() == std::io::ErrorKind::NotFound => {
            err(StatusCode::BAD_REQUEST, error)
        }
        other => map_patch_err(other),
    }
}

#[derive(Deserialize)]
pub(super) struct Request {
    game_path: PathBuf,
    #[serde(default)]
    force: bool,
    #[serde(default)]
    expected_transaction_id: Option<String>,
}

pub(super) async fn status(
    Json(req): Json<Request>,
) -> Result<Json<injection_transaction::InjectionStatus>, ApiError> {
    let report = tokio::task::spawn_blocking(move || injection_transaction::status(&req.game_path))
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e))?
        .map_err(map_recovery_error)?;
    Ok(Json(report))
}

pub(super) async fn recover(
    State(state): State<Arc<AppState>>,
    Json(req): Json<Request>,
) -> Result<
    (
        StatusCode,
        Json<injection_transaction::InjectionRecoveryReport>,
    ),
    ApiError,
> {
    let exclusive = try_project_operation(&state).map_err(|m| err(StatusCode::CONFLICT, m))?;
    let force = req.force;
    let report = tokio::task::spawn_blocking(move || {
        // A disconnected HTTP caller cannot release the reservation while the
        // blocking operation is still restoring files.
        let _exclusive = exclusive;
        injection_transaction::recover(
            &req.game_path,
            InjectionRecoveryOptions {
                force,
                expected_transaction_id: req.expected_transaction_id,
            },
        )
    })
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e))?
    .map_err(map_recovery_error)?;
    let status = if !force && !report.preserved_conflicts.is_empty() {
        StatusCode::CONFLICT
    } else {
        StatusCode::OK
    };
    Ok((status, Json(report)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn recovery_refuses_project_conflict_before_accessing_path() {
        let state = create_test_state();
        let _exclusive = try_project_operation(&state).unwrap();
        let result = recover(
            State(state),
            Json(Request {
                game_path: "missing-game".into(),
                force: false,
                expected_transaction_id: None,
            }),
        )
        .await;
        assert_eq!(result.unwrap_err().0, StatusCode::CONFLICT);
    }

    #[tokio::test]
    async fn clean_game_recovery_is_noop_and_releases_project() {
        let game = tempfile::tempdir().unwrap();
        let sentinel = game.path().join("sentinel");
        std::fs::write(&sentinel, "unchanged").unwrap();
        let state = create_test_state();
        let report = status(Json(Request {
            game_path: game.path().to_owned(),
            force: false,
            expected_transaction_id: None,
        }))
        .await
        .unwrap();
        assert!(report.pending.is_none());
        let (code, report) = recover(
            State(state.clone()),
            Json(Request {
                game_path: game.path().to_owned(),
                force: false,
                expected_transaction_id: None,
            }),
        )
        .await
        .unwrap();
        assert_eq!(code, StatusCode::OK);
        assert_eq!(report.restored, 0);
        assert_eq!(report.removed, 0);
        assert_eq!(std::fs::read_to_string(sentinel).unwrap(), "unchanged");
        assert!(try_project_operation(&state).is_ok());
    }

    // Exercise the production router over loopback, including extractors and
    // IntoResponse. Every test owns its server, game and project reservation.
    struct HttpServer {
        url: String,
        handle: tokio::task::JoinHandle<()>,
        client: reqwest::Client,
    }

    impl HttpServer {
        async fn new(state: Arc<AppState>) -> Self {
            let (url, handle) = start_test_server(state).await;
            Self {
                url,
                handle,
                client: reqwest::Client::builder()
                    .no_proxy()
                    .timeout(Duration::from_secs(15))
                    .build()
                    .unwrap(),
            }
        }

        async fn post(&self, route: &str, body: serde_json::Value) -> reqwest::Response {
            self.client
                .post(format!("{}{route}", self.url))
                .json(&body)
                .send()
                .await
                .unwrap()
        }
    }

    impl Drop for HttpServer {
        fn drop(&mut self) {
            self.handle.abort();
        }
    }

    async fn json_success(response: reqwest::Response) -> serde_json::Value {
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()["content-type"], "application/json");
        response.json().await.unwrap()
    }

    async fn text_error(response: reqwest::Response, expected: StatusCode) -> String {
        assert_eq!(response.status(), expected);
        assert_eq!(
            response.headers()["content-type"],
            "text/plain; charset=utf-8"
        );
        let message = response.text().await.unwrap();
        assert!(!message.trim().is_empty());
        message
    }

    // Same interrupted-preparation disk schema used by core's
    // tests/injection_operation_exclusion.rs; recovery itself stays in core.
    fn interrupted_preparation(game: &Path) -> String {
        let root = game.canonicalize().unwrap();
        let store = game.join(injection_transaction::STORE_DIR);
        std::fs::create_dir(&store).unwrap();
        std::fs::write(
            store.join("store.json"),
            serde_json::to_vec(&serde_json::json!({
                "schema_version": 1, "kind": "locust-injection-store", "game_root": root
            }))
            .unwrap(),
        )
        .unwrap();
        let id = uuid::Uuid::new_v4().to_string();
        let operation = store.join("operations").join(&id);
        std::fs::create_dir_all(&operation).unwrap();
        std::fs::write(operation.join("phase.json"), b"\"preparing\"").unwrap();
        std::fs::write(
            store.join("active.json"),
            serde_json::to_vec(&serde_json::json!({
                "schema_version": 1, "transaction_id": id, "game_root": root,
                "format": "html-game", "language": "es"
            }))
            .unwrap(),
        )
        .unwrap();
        id
    }

    #[tokio::test]
    async fn http_pending_preparation_recovers_and_status_becomes_clean() {
        let game = tempfile::tempdir().unwrap();
        let sentinel = game.path().join("story.html");
        std::fs::write(&sentinel, b"user content").unwrap();
        let id = interrupted_preparation(game.path());
        let state = create_test_state();
        let server = HttpServer::new(state.clone()).await;
        let request = serde_json::json!({"game_path": game.path()});
        let status = json_success(server.post("/api/inject/status", request.clone()).await).await;
        assert_eq!(
            status["game_root"],
            serde_json::json!(game.path().canonicalize().unwrap())
        );
        let pending = &status["pending"];
        assert_eq!(pending["transaction_id"], id);
        assert_eq!(pending["phase"], "preparing");
        assert_eq!(pending["format"], "html-game");
        assert_eq!(pending["language"], "es");
        assert_eq!(pending["changed_files"], 0);
        assert_eq!(pending["conflicts"], serde_json::json!([]));
        let report = json_success(server.post("/api/inject/recover", request.clone()).await).await;
        assert_eq!(report["transaction_id"], id);
        assert_eq!(report["restored"], 0);
        assert_eq!(report["removed"], 0);
        assert_eq!(report["preserved_conflicts"], serde_json::json!([]));
        assert!(report["messages"].is_array());
        let status = json_success(server.post("/api/inject/status", request.clone()).await).await;
        assert!(status.as_object().unwrap().contains_key("pending"));
        assert!(status["pending"].is_null());
        let retry = json_success(server.post("/api/inject/recover", request).await).await;
        assert!(retry["transaction_id"].is_null());
        assert_eq!(retry["restored"], 0);
        assert_eq!(retry["removed"], 0);
        assert_eq!(std::fs::read(sentinel).unwrap(), b"user content");
        assert!(try_project_operation(&state).is_ok());
    }

    #[tokio::test]
    async fn http_reviewed_recovery_rejects_stale_operation_and_accepts_current() {
        let game = tempfile::tempdir().unwrap();
        let id = interrupted_preparation(game.path());
        let marker = game
            .path()
            .join(injection_transaction::STORE_DIR)
            .join("active.json");
        let before = std::fs::read(&marker).unwrap();
        let server = HttpServer::new(create_test_state()).await;
        for force in [false, true] {
            let response = server
                .post(
                    "/api/inject/recover",
                    serde_json::json!({
                        "game_path": game.path(), "force": force,
                        "expected_transaction_id": "stale-reviewed-operation"
                    }),
                )
                .await;
            assert!(text_error(response, StatusCode::CONFLICT)
                .await
                .contains("operation changed"));
            assert_eq!(std::fs::read(&marker).unwrap(), before);
        }
        let report = json_success(
            server
                .post(
                    "/api/inject/recover",
                    serde_json::json!({
                        "game_path": game.path(), "expected_transaction_id": id
                    }),
                )
                .await,
        )
        .await;
        assert_eq!(report["transaction_id"], id);
    }

    #[tokio::test]
    async fn http_unknown_reserved_store_is_not_adopted_even_with_force() {
        let game = tempfile::tempdir().unwrap();
        let store = game.path().join(injection_transaction::STORE_DIR);
        std::fs::create_dir(&store).unwrap();
        let marker = store.join("store.json");
        std::fs::write(&marker, b"unknown user bytes").unwrap();
        let sentinel = game.path().join("story.html");
        std::fs::write(&sentinel, b"user content").unwrap();
        let state = create_test_state();
        let server = HttpServer::new(state.clone()).await;
        for route in ["/api/inject/status", "/api/inject/recover"] {
            for force in [false, true] {
                let response = server
                    .post(
                        route,
                        serde_json::json!({
                            "game_path": game.path(), "force": force
                        }),
                    )
                    .await;
                let message = text_error(response, StatusCode::CONFLICT).await;
                assert!(message.contains("invalid recovery metadata"), "{message}");
                assert_eq!(std::fs::read(&marker).unwrap(), b"unknown user bytes");
                assert_eq!(std::fs::read(&sentinel).unwrap(), b"user content");
                assert_eq!(std::fs::read_dir(&store).unwrap().count(), 1);
                assert_eq!(std::fs::read_dir(game.path()).unwrap().count(), 2);
                assert!(try_project_operation(&state).is_ok());
            }
        }
    }

    #[tokio::test]
    async fn http_held_game_lock_returns_409_and_releases_project_reservation() {
        let game = tempfile::tempdir().unwrap();
        interrupted_preparation(game.path());
        let state = create_test_state();
        let server = HttpServer::new(state.clone()).await;
        let lock = locust_core::patch::GameLock::acquire(game.path()).unwrap();
        let request = serde_json::json!({"game_path": game.path()});
        for route in ["/api/inject/status", "/api/inject/recover"] {
            let message = text_error(
                server.post(route, request.clone()).await,
                StatusCode::CONFLICT,
            )
            .await;
            assert!(message.starts_with("game busy:"), "{message}");
            assert!(try_project_operation(&state).is_ok());
        }
        drop(lock);
        let status = json_success(server.post("/api/inject/status", request.clone()).await).await;
        assert_eq!(status["pending"]["phase"], "preparing");
        json_success(server.post("/api/inject/recover", request).await).await;
    }

    #[tokio::test]
    async fn http_project_conflict_and_missing_path_failures_allow_later_recovery() {
        let game = tempfile::tempdir().unwrap();
        let state = create_test_state();
        let server = HttpServer::new(state.clone()).await;
        let missing = serde_json::json!({"game_path": game.path().join("missing")});
        let exclusive = try_project_operation(&state).unwrap();
        text_error(
            server.post("/api/inject/recover", missing.clone()).await,
            StatusCode::CONFLICT,
        )
        .await;
        // A rejected HTTP request must not release another operation's guard.
        assert!(try_project_operation(&state).is_err());
        drop(exclusive);
        text_error(
            server.post("/api/inject/recover", missing).await,
            StatusCode::BAD_REQUEST,
        )
        .await;
        assert!(try_project_operation(&state).is_ok());
        let report = json_success(
            server
                .post(
                    "/api/inject/recover",
                    serde_json::json!({
                        "game_path": game.path()
                    }),
                )
                .await,
        )
        .await;
        assert!(report["transaction_id"].is_null());
        assert_eq!(report["restored"], 0);
        assert!(try_project_operation(&state).is_ok());
    }
}
