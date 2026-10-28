//! Explicit, bounded real-endpoint benchmark. Never runs in ordinary cargo test.
//! cargo test -p locust-providers --test google_fast_probe -- --ignored --nocapture
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;

use locust_core::models::TranslationRequest;
use locust_core::translation::{with_retry, RateLimiter, RetryConfig, TranslationProvider};
use locust_providers::google::GoogleTranslateProvider;
use tokio::time::Instant;

// Synthetic seed lines from tests/integration/main.rs and google.rs's live smoke
// fixture. Indexed variants prevent translation-memory deduplication. No game IO.
fn dialogue(source: &str, target: &str) -> Vec<TranslationRequest> {
    let seeds: &[&str] = if source == "en" {
        &["Hello traveler!", "Welcome to our town.", "Hello, world!"]
    } else {
        &[
            "はい",
            "こんにちは、\\C[1]リディア\\C[0]さん。",
            "宿屋へようこそ！\n今日は泊まっていく？",
        ]
    };
    (0..2000)
        .map(|i| TranslationRequest {
            entry_id: format!("synthetic-{i:04}"),
            source: format!("{} ({i:04})", seeds[i % seeds.len()]),
            source_lang: source.into(),
            target_lang: target.into(),
            context: None,
            glossary_hint: None,
        })
        .collect()
}

#[tokio::test]
#[ignore = "bounded live Google endpoint benchmark (at most 15 minutes total)"]
async fn google_fast_probe() {
    let suite_started = Instant::now();
    let budget = std::env::var("LOCUST_GOOGLE_PROBE_BUDGET_SECONDS")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(900)
        .min(900);
    let suite_end = suite_started + Duration::from_secs(budget);
    let skip = std::env::var("LOCUST_GOOGLE_PROBE_SKIP")
        .ok()
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(0);
    let mut config_index = 0;
    let mut unavailable_configs = 0;
    println!("config,pair,chunk_sources,chunk_chars,concurrency,strings,seconds,strings_per_min,http_requests,http_429,blocked,first_429_seconds,first_block_seconds,final_rpm,outcome");
    // Legacy 8 *engine batches*/minute, then the same workload under per-HTTP
    // AIMD at each requested engine concurrency, plus larger POST form chunks.
    for (source, target) in [("en", "es"), ("ja", "en")] {
        for (legacy, sources, chars, concurrency) in [
            (true, 50, 4500, 4),
            (false, 50, 4500, 4),
            (false, 50, 4500, 8),
            (false, 50, 4500, 16),
            (false, 100, 9000, 4),
            (false, 100, 9000, 8),
            (false, 100, 9000, 16),
            (false, 200, 9000, 4),
        ] {
            config_index += 1;
            if config_index <= skip {
                continue;
            }
            if Instant::now() >= suite_end {
                println!("STOP: total probing budget exhausted");
                return;
            }
            let limiter = Arc::new(RateLimiter::google(legacy.then_some(u32::MAX), Some(3)));
            let legacy_gate = Arc::new(RateLimiter::new(8));
            let provider = Arc::new(GoogleTranslateProvider::with_tuning(
                limiter.clone(),
                sources,
                chars,
                if legacy { 8 } else { 4 },
            ));
            let requests = dialogue(source, target);
            let started = Instant::now();
            let end = (started + Duration::from_secs(180)).min(suite_end);
            let mut batches = requests.chunks(200);
            let mut tasks = tokio::task::JoinSet::new();
            let mut translated = 0;
            let mut outcome = "complete".to_string();
            'run: loop {
                while tasks.len() < concurrency {
                    let Some(batch) = batches.next() else { break };
                    let batch = batch.to_vec();
                    let provider = provider.clone();
                    let limiter = limiter.clone();
                    let legacy_gate = legacy_gate.clone();
                    tasks.spawn(async move {
                        if legacy {
                            legacy_gate.acquire().await
                        } else {
                            limiter.acquire().await
                        }
                        let attempts = AtomicU32::new(0);
                        with_retry(&RetryConfig::default(), || {
                            let provider = provider.clone();
                            let batch = batch.clone();
                            let legacy_gate = legacy_gate.clone();
                            let is_retry = attempts.fetch_add(1, Ordering::Relaxed) > 0;
                            async move {
                                if legacy && is_retry {
                                    legacy_gate.acquire().await;
                                }
                                provider.translate(&batch).await
                            }
                        })
                        .await
                    });
                }
                if tasks.is_empty() {
                    break;
                }
                tokio::select! {
                    biased;
                    _ = limiter.probe_stopped() => { outcome = "safety_stop".into(); break 'run; },
                    _ = tokio::time::sleep_until(end) => { outcome = "time_budget".into(); break 'run; },
                    task = tasks.join_next() => match task.unwrap().unwrap() {
                        Ok(results) => translated += results.len(),
                        Err(error) => {
                            outcome = format!("error: {error}").replace(',', ";");
                            break 'run;
                        }
                    }
                }
            }
            tasks.abort_all();
            while tasks.join_next().await.is_some() {}
            let seconds = started.elapsed().as_secs_f64();
            let stats = limiter.google_stats();
            println!("{},{source}->{target},{sources},{chars},{concurrency},{translated},{seconds:.3},{:.2},{},{},{},{},{},{},{}",
                if legacy { "legacy" } else { "adaptive" },
                translated as f64 * 60.0 / seconds, stats.requests, stats.http_429, stats.blocked,
                stats.first_429.map_or_else(|| "none".into(), |d| format!("{:.3}", d.as_secs_f64())),
                stats.first_block.map_or_else(|| "none".into(), |d| format!("{:.3}", d.as_secs_f64())),
                limiter.requests_per_minute(), outcome);
            if stats.circuit_open {
                println!("STOP: three consecutive 429/blocked responses; whole suite stopped");
                return;
            }
            if stats.blocked > 0 {
                println!(
                    "STOP: blocked response observed; preserving cooldown across configurations"
                );
                return;
            }
            // A bursty legacy run may fail while paced configurations still work.
            // Bound consecutive transport-failing configurations as well.
            if outcome.starts_with("error:") && stats.successes == 0 {
                unavailable_configs += 1;
                if unavailable_configs >= 3 {
                    println!("STOP: endpoint unavailable for three configurations");
                    return;
                }
                tokio::time::sleep_until((Instant::now() + Duration::from_secs(5)).min(suite_end))
                    .await;
            } else {
                unavailable_configs = 0;
            }
        }
    }
}
