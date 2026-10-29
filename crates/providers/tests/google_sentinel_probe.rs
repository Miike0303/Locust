//! Opt-in synthetic sentinel measurement; no game data, at most five minutes.
use std::sync::Arc;
use std::time::Duration;

use locust_core::models::TranslationRequest;
use locust_core::translation::{RateLimiter, TranslationProvider};
use locust_providers::google::GoogleTranslateProvider;
use tokio::time::Instant;

fn token(encoding: &str, index: usize) -> String {
    match encoding {
        "braces" => format!("{{PL_{index}}}"),
        "underscores" => format!("__LOCUST_0_{index}__"),
        "ascii" => format!("ZXQ0X{index}QXZ"),
        "unicode" => format!("⟦L0X{index}⟧"),
        "pua" => format!("\u{e000}{index}\u{e001}"),
        _ => unreachable!(),
    }
}

#[tokio::test]
#[ignore = "bounded live Google sentinel probe (at most five minutes)"]
async fn google_sentinel_probe() {
    let started = Instant::now();
    let end = started + Duration::from_secs(300);
    // One shared cooldown/circuit for the entire probe, never reset per encoding.
    let limiter = Arc::new(RateLimiter::google(Some(60), Some(3)));
    let provider = GoogleTranslateProvider::with_tuning(limiter.clone(), 20, 9000, 1);
    println!("encoding,pair,rows,exact_rows,exact_tokens,total_tokens,http_requests,http_429");
    for encoding in ["braces", "underscores", "ascii", "unicode", "pua"] {
        for (source_lang, target_lang) in [("ja", "en"), ("en", "es"), ("ja", "es")] {
            let t: Vec<_> = (0..3).map(|i| token(encoding, i)).collect();
            let sources = if source_lang == "ja" {
                vec![
                    format!("こんにちは {} {}旅人{}！", t[0], t[1], t[2]),
                    format!("{}<錬金術師>こんにちは {}、{}！", t[0], t[1], t[2]),
                    format!("{} ゴールドが必要です。", t[0]),
                    format!("{} と {} は鍵を見つけた。", t[0], t[1]),
                    format!("{}こんにちは{}", t[0], t[1]),
                ]
            } else {
                vec![
                    format!("Hello {} {}traveler{}!", t[0], t[1], t[2]),
                    format!("{}<Alchemist>Hello {}, {}!", t[0], t[1], t[2]),
                    format!("You need {} gold.", t[0]),
                    format!("{} and {} found the key.", t[0], t[1]),
                    format!("{}Hello{}", t[0], t[1]),
                ]
            };
            let requests: Vec<_> = sources
                .into_iter()
                .enumerate()
                .map(|(i, source)| TranslationRequest {
                    entry_id: i.to_string(),
                    source,
                    source_lang: source_lang.into(),
                    target_lang: target_lang.into(),
                    context: None,
                    glossary_hint: None,
                })
                .collect();
            let results = tokio::select! {
                biased;
                _ = limiter.probe_stopped() => { println!("STOP: three consecutive blocked/429 responses"); return; },
                _ = tokio::time::sleep_until(end) => { println!("STOP: five-minute budget"); return; },
                result = provider.translate(&requests) => match result {
                    Ok(results) => results,
                    Err(error) => { println!("STOP: live endpoint unavailable: {error}"); return; }
                }
            };
            let mut exact_rows = 0;
            let mut exact_tokens = 0;
            let mut total_tokens = 0;
            for (request, result) in requests.iter().zip(&results) {
                let mut exact = true;
                for token in &t {
                    let expected = request.source.matches(token).count();
                    let actual = result.translation.matches(token).count();
                    exact &= expected == actual;
                    total_tokens += expected;
                    if expected == actual {
                        exact_tokens += expected;
                    }
                }
                exact_rows += usize::from(exact);
                println!(
                    "RAW {}",
                    serde_json::json!({"encoding": encoding, "pair": format!("{source_lang}->{target_lang}"), "source": request.source, "translation": result.translation})
                );
            }
            let stats = limiter.google_stats();
            println!("{encoding},{source_lang}->{target_lang},{},{exact_rows},{exact_tokens},{total_tokens},{},{}", requests.len(), stats.requests, stats.http_429);
        }
    }
    println!("DONE seconds={:.3}", started.elapsed().as_secs_f64());
}
