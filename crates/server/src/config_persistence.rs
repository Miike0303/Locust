use super::*;

pub fn load_startup_config(path: &std::path::Path) -> (AppConfig, bool) {
    match AppConfig::load(path) {
        Ok(config) => (config, false),
        Err(_) => {
            // Do not log deserialization input: it may include credentials.
            tracing::warn!("Could not load configuration; protected fallback is active");
            (AppConfig::default(), true)
        }
    }
}

pub async fn public_app_config(state: &AppState) -> serde_json::Value {
    let mut value = state.config.read().await.redacted_json();
    if state.config_load_failed {
        value["load_warning"] = state.config_path.to_string_lossy().into_owned().into();
    }
    value
}

fn startup_failure(state: &AppState) -> Option<String> {
    state.config_load_failed.then(|| {
        "Configuration could not be loaded. The existing file is protected. Repair the configuration and restart Locust before saving settings.".to_owned()
    })
}

/// Project opening has already succeeded. A recent-list write failure must not
/// masquerade as a failed open; keep the live project and return a visible warning.
pub async fn remember_open_project(
    state: &AppState,
    outcome: &ProjectOpenOutcome,
    saved_database: bool,
) -> Option<String> {
    if let Some(warning) = startup_failure(state) {
        return Some(warning);
    }
    let mut config = state.config.write().await;
    config.add_recent_project(
        outcome.project_path.clone(),
        outcome.project_name.clone(),
        outcome.format_id.clone(),
        saved_database.then(|| outcome.database_path.clone()),
    );
    config
        .save(&state.config_path)
        .err()
        .map(|error| error.to_string())
}

/// Shared by HTTP and desktop. Invalid or unpersistable changes never become
/// active in memory, and responses have the same redaction as GET /config.
pub async fn update_app_config(
    state: &AppState,
    partial: serde_json::Value,
) -> Result<serde_json::Value, (StatusCode, String)> {
    if let Some(warning) = startup_failure(state) {
        return Err(err(StatusCode::CONFLICT, warning));
    }
    let mut config = state.config.write().await;
    let next = config
        .patched(partial)
        .map_err(|e| err(StatusCode::BAD_REQUEST, e))?;
    next.save(&state.config_path)
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e))?;
    *state.provider_registry.write().await = locust_providers::default_registry(&next);
    let response = next.redacted_json();
    *config = next;
    Ok(response)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn unreadable_startup_config_stays_protected_after_repair_until_restart() {
        let mut state = create_test_state();
        std::fs::create_dir_all(state.config_path.parent().unwrap()).unwrap();
        let damaged = b"{\"providers\":{\"grok\":{\"api_key\":\"fixture-secret";
        std::fs::write(&state.config_path, damaged).unwrap();
        let (fallback, failed) = load_startup_config(&state.config_path);
        assert!(failed);
        let inner = Arc::get_mut(&mut state).unwrap();
        inner.config_load_failed = failed;
        *inner.config.write().await = fallback;
        let public = public_app_config(&state).await;
        assert_eq!(
            public["load_warning"],
            state.config_path.to_string_lossy().as_ref()
        );
        assert!(!public.to_string().contains("fixture-secret"));
        let update = || update_app_config(&state, serde_json::json!({"default_target_lang": "fr"}));
        assert_eq!(update().await.unwrap_err().0, StatusCode::CONFLICT);
        assert_eq!(std::fs::read(&state.config_path).unwrap(), damaged);
        // Even after external repair, this process must not persist its stale
        // fallback profile over the recovered credentials.
        let repaired = br#"{"providers":{"grok":{"api_key":"fixture-secret","model":"grok-4.6"}}}"#;
        std::fs::write(&state.config_path, repaired).unwrap();
        assert_eq!(update().await.unwrap_err().0, StatusCode::CONFLICT);
        assert_eq!(std::fs::read(&state.config_path).unwrap(), repaired);
        assert!(!load_startup_config(&state.config_path).1);
    }

    #[tokio::test]
    async fn config_save_failure_does_not_activate_values_or_replace_old_bytes() {
        let state = create_test_state();
        update_app_config(&state, serde_json::json!({"default_target_lang": "es"}))
            .await
            .unwrap();
        let bytes = std::fs::read(&state.config_path).unwrap();
        let old_permissions = std::fs::metadata(&state.config_path).unwrap().permissions();
        let mut readonly = old_permissions.clone();
        readonly.set_readonly(true);
        std::fs::set_permissions(&state.config_path, readonly).unwrap();
        let failure = update_app_config(&state, serde_json::json!({"default_target_lang": "fr"}))
            .await
            .unwrap_err();
        assert_eq!(failure.0, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(state.config.read().await.default_target_lang, "es");
        assert_eq!(std::fs::read(&state.config_path).unwrap(), bytes);
        std::fs::set_permissions(&state.config_path, old_permissions).unwrap();
    }

    #[tokio::test]
    async fn config_http_roundtrip_keeps_masked_keys_and_never_echoes_them() {
        let state = create_test_state();
        let (url, handle) = start_test_server(state.clone()).await;
        let client = reqwest::Client::builder().no_proxy().build().unwrap();
        let initial = client
            .patch(format!("{url}/api/config"))
            .json(&serde_json::json!({
                "providers": {"grok": {"api_key": "fixture-secret", "model": "first"}}
            }))
            .send()
            .await
            .unwrap();
        assert_eq!(initial.status(), StatusCode::OK);
        let mut public: serde_json::Value = initial.json().await.unwrap();
        assert!(!public.to_string().contains("fixture-secret"));
        public["providers"]["grok"]["model"] = "second".into();
        let response = client
            .patch(format!("{url}/api/config"))
            .json(&serde_json::json!({"providers": public["providers"]}))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = response.text().await.unwrap();
        assert!(!body.contains("fixture-secret"));
        let saved = AppConfig::load(&state.config_path).unwrap();
        assert_eq!(
            saved.providers["grok"].api_key.as_deref(),
            Some("fixture-secret")
        );
        assert_eq!(saved.providers["grok"].model.as_deref(), Some("second"));
        assert!(state.provider_registry.read().await.get("grok").is_some());
        handle.abort();
    }

    #[tokio::test]
    async fn project_open_persists_recents_and_reports_failed_save_without_failing_open() {
        let game = tempfile::tempdir().unwrap();
        std::fs::write(
            game.path().join("story.html"),
            "<p>Test Japanese 日本語</p>",
        )
        .unwrap();
        let state = create_test_state();
        let first = project_open(
            State(state.clone()),
            Json(OpenProjectRequest {
                path: game.path().to_string_lossy().into_owned(),
                format_id: Some("html-game".into()),
                prefer_saved: false,
            }),
        )
        .await
        .unwrap()
        .0;
        assert!(first.persistence_warning.is_none());
        let saved = AppConfig::load(&state.config_path).unwrap();
        assert_eq!(saved.recent_projects.len(), 1);
        assert_eq!(
            saved.recent_projects[0].database_path.as_deref(),
            Some(std::path::Path::new(&first.database_path)),
            "recent projects must reopen their saved DB, not re-extract an injected game"
        );
        let current = project_current(State(state.clone())).await.unwrap().0;
        assert_eq!(
            current.database_path.as_ref().unwrap().to_string_lossy(),
            first.database_path
        );
        assert_eq!(current.supported_modes, first.supported_modes);
        let permissions = std::fs::metadata(&state.config_path).unwrap().permissions();
        let mut readonly = permissions.clone();
        readonly.set_readonly(true);
        std::fs::set_permissions(&state.config_path, readonly).unwrap();
        let response = project_open(
            State(state.clone()),
            Json(OpenProjectRequest {
                path: game.path().to_string_lossy().into_owned(),
                format_id: Some("html-game".into()),
                prefer_saved: false,
            }),
        )
        .await
        .unwrap()
        .0;
        assert!(response.persistence_warning.is_some());
        let current = project_current(State(state.clone())).await.unwrap().0;
        assert_eq!(current.persistence_warning, response.persistence_warning);
        assert!(current.database_path.is_some());
        std::fs::set_permissions(&state.config_path, permissions).unwrap();
    }
}
