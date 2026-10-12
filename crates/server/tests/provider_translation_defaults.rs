use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use locust_core::error::Result;
use locust_core::models::{StringEntry, TranslationRequest, TranslationResult};
use locust_core::translation::{ProviderRegistry, TranslationProvider};
use locust_server::{create_test_state, start_test_server, ProjectInfo};
use serde_json::json;

struct RecordingProvider {
    id: &'static str,
    batches: Mutex<Vec<usize>>,
    // Every wave must reach the expected parallelism before returning results.
    wave: tokio::sync::Barrier,
}

#[async_trait]
impl TranslationProvider for RecordingProvider {
    fn id(&self) -> &str {
        self.id
    }

    fn name(&self) -> &str {
        "Recording provider"
    }

    fn is_free(&self) -> bool {
        true
    }

    fn requires_api_key(&self) -> bool {
        false
    }

    async fn translate(&self, requests: &[TranslationRequest]) -> Result<Vec<TranslationResult>> {
        self.batches.lock().unwrap().push(requests.len());
        self.wave.wait().await;
        locust_providers::mock::MockProvider
            .translate(requests)
            .await
    }

    async fn estimate_cost(&self, _: usize, _: &str) -> Option<f64> {
        None
    }

    async fn health_check(&self) -> Result<()> {
        Ok(())
    }
}

#[tokio::test]
async fn provider_translation_defaults_reach_http_jobs_and_settings_yield_to_requests() {
    // Stay within the unchanged per-provider rate limits across all cases.
    for (provider_id, overrides, configured_batch, batch, parallel) in [
        ("google", json!({}), None, 200, 4),
        ("antigravity", json!({}), Some(50), 50, 4),
        (
            "grok",
            json!({"batch_size": 100, "max_concurrent": 2, "max_batch_tokens": 20_000}),
            Some(40),
            100,
            2,
        ),
        // A tiny explicit token budget forces single-entry batches.
        ("google", json!({"max_batch_tokens": 1}), None, 1, 4),
    ] {
        let state = create_test_state();
        state.config.write().await.default_batch_size = configured_batch;
        *state.current_project.write().await = Some(ProjectInfo {
            path: "test-game".into(),
            ..Default::default()
        });
        let provider = Arc::new(RecordingProvider {
            id: provider_id,
            batches: Mutex::new(Vec::new()),
            wave: tokio::sync::Barrier::new(parallel),
        });
        let mut registry = ProviderRegistry::new();
        registry.register(provider.clone());
        *state.provider_registry.write().await = registry;
        let entries: Vec<_> = (0..batch * 4)
            .map(|index| {
                StringEntry::new(
                    format!("row-{index}"),
                    format!("Source text {index}"),
                    "story.json".into(),
                )
            })
            .collect();
        state.db.save_entries(&entries).unwrap();
        let mut options = json!({
            "source_lang": "ja", "target_lang": "en",
            "use_glossary": false, "use_memory": false, "skip_approved": true
        });
        options
            .as_object_mut()
            .unwrap()
            .extend(overrides.as_object().unwrap().clone());
        let (base, server) = start_test_server(state.clone()).await;
        let response = reqwest::Client::new()
            .post(format!("{base}/api/translate/start"))
            .json(&json!({"provider_id": provider_id, "options": options}))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200, "{}", response.text().await.unwrap());
        tokio::time::timeout(Duration::from_secs(10), async {
            while state
                .project_exclusive
                .load(std::sync::atomic::Ordering::Acquire)
                != 0
            {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("translation should finish with the requested parallelism");
        assert_eq!(
            *provider.batches.lock().unwrap(),
            vec![batch; 4],
            "{overrides}"
        );
        let runs = state.db.get_translation_runs().unwrap();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].strings_translated, entries.len());
        server.abort();
    }
}
