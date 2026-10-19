use std::path::PathBuf;
use std::sync::Arc;

use comfy_table::Table;
use indicatif::{ProgressBar, ProgressStyle};
use locust_core::config::AppConfig;
use locust_core::glossary::Glossary;
use locust_core::models::ProgressEvent;
use locust_core::translation::{load_pending_entries, run_fallback_chain, TranslationOptions};

use crate::util::{format_duration, format_observed_cost, open_existing_project};

fn cli_translation_options(
    provider_id: &str,
    configured_batch_size: Option<usize>,
    batch_size: Option<usize>,
    concurrency: Option<usize>,
    max_batch_tokens: Option<usize>,
) -> TranslationOptions {
    let defaults = TranslationOptions::defaults_for_provider(provider_id);
    TranslationOptions {
        batch_size: batch_size
            .or(configured_batch_size)
            .unwrap_or(defaults.batch_size),
        max_concurrent: concurrency.unwrap_or(defaults.max_concurrent),
        max_batch_tokens: max_batch_tokens.or(defaults.max_batch_tokens),
        ..defaults
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn cmd_translate(
    config: AppConfig,
    project: PathBuf,
    provider_id: String,
    source: String,
    target: String,
    batch_size: Option<usize>,
    concurrency: Option<usize>,
    max_batch_tokens: Option<usize>,
    no_memory: bool,
    allow_lossy_binary_fit: bool,
    fallback: Vec<String>,
    cost_limit: Option<f64>,
    context: Option<String>,
) -> anyhow::Result<()> {
    let db = Arc::new(open_existing_project(&project)?);
    let provider_reg = locust_providers::default_registry(&config);
    let glossary = Arc::new(Glossary::new(db.clone()));

    let opts = TranslationOptions {
        source_lang: source,
        target_lang: target,
        use_memory: !no_memory,
        allow_lossy_binary_fit,
        cost_limit_usd: cost_limit,
        game_context: context,
        ..cli_translation_options(
            &provider_id,
            config.default_batch_size,
            batch_size,
            concurrency,
            max_batch_tokens,
        )
    };

    // Primary then fallbacks — shared chain in locust_core (same as HTTP server).
    let chain: Vec<String> = std::iter::once(provider_id).chain(fallback).collect();
    let mut resolve_map: std::collections::HashMap<
        String,
        Arc<dyn locust_core::translation::TranslationProvider>,
    > = std::collections::HashMap::new();
    for id in &chain {
        match provider_reg.get(id) {
            Some(p) => {
                resolve_map.insert(id.clone(), p);
            }
            None => eprintln!("provider '{id}' not found, will skip"),
        }
    }
    let resolve_map = Arc::new(resolve_map);

    let initial_pending = load_pending_entries(&db)?.len();
    if initial_pending == 0 {
        println!("No pending strings.");
        return Ok(());
    }
    println!(
        "Translation chain: {} · {} → {} · {} pending",
        chain.join(" → "),
        opts.source_lang,
        opts.target_lang,
        initial_pending
    );

    let (tx, mut rx) = tokio::sync::mpsc::channel(1000);
    let cancel = tokio_util::sync::CancellationToken::new();
    let job_id = uuid::Uuid::new_v4().to_string();

    let bar = ProgressBar::new(initial_pending as u64);
    bar.set_style(
        ProgressStyle::default_bar()
            .template("{spinner:.green} [{bar:40.cyan/blue}] {pos}/{len} {msg}")
            .unwrap()
            .progress_chars("█▓░"),
    );

    let map_job = resolve_map.clone();
    let db_job = db.clone();
    let glossary_job = glossary.clone();
    let opts_job = opts.clone();
    let chain_job = chain.clone();
    let handle = tokio::spawn(async move {
        let resolve = |id: &str| map_job.get(id).cloned();
        run_fallback_chain(
            &chain_job,
            &resolve,
            db_job,
            glossary_job,
            opts_job,
            tx,
            job_id,
            cancel,
        )
        .await
    });

    let mut total_cost = 0.0;
    let mut cost_is_complete = false;
    let mut total_translated = 0;
    let mut errors = 0u64;
    let mut error_counts = std::collections::HashMap::<String, u64>::new();
    let start = std::time::Instant::now();

    while let Some(event) = rx.recv().await {
        match event {
            ProgressEvent::Started { total, .. } => {
                bar.set_length(total as u64);
            }
            ProgressEvent::BatchCompleted {
                completed,
                cost_so_far,
                cost_is_complete: complete,
                ..
            } => {
                bar.set_position(completed as u64);
                bar.set_message(format_observed_cost(cost_so_far, complete));
                cost_is_complete = complete;
                total_cost = cost_so_far;
                total_translated = completed;
            }
            ProgressEvent::ProviderSwitched {
                provider_name,
                remaining_pending,
                ..
            } => {
                bar.println(format!(
                    "Falling back to provider: {provider_name} ({remaining_pending} still pending)"
                ));
            }
            ProgressEvent::Completed {
                total_translated: tt,
                total_cost: tc,
                cost_is_complete: complete,
                ..
            } => {
                total_translated = tt;
                total_cost = tc;
                cost_is_complete = complete;
            }
            ProgressEvent::BatchFailed { error, .. } | ProgressEvent::Failed { error, .. } => {
                errors += 1;
                if errors <= 3 {
                    if bar.is_hidden() {
                        eprintln!("Error: {error}");
                    } else {
                        bar.println(format!("Error: {error}"));
                    }
                }
                *error_counts.entry(error).or_default() += 1;
            }
            _ => {}
        }
    }

    bar.finish_with_message("Done!");
    let result = handle.await;

    let elapsed = start.elapsed().as_secs_f64();
    let mut table = Table::new();
    table.set_header(vec!["Metric", "Value"]);
    table.add_row(vec!["Total translated", &total_translated.to_string()]);
    table.add_row(vec!["Time elapsed", &format_duration(elapsed)]);
    table.add_row(vec![
        "Total cost",
        &format_observed_cost(total_cost, cost_is_complete),
    ]);
    table.add_row(vec!["Batch errors", &errors.to_string()]);
    println!("{table}");

    if !error_counts.is_empty() {
        eprintln!("Translation errors (top 5):");
        for line in translation_error_summary(&error_counts) {
            eprintln!("  {line}");
        }
    }
    result??;

    let left = load_pending_entries(&db)?.len();
    if left > 0 {
        println!(
            "\n{left} strings still pending. Re-run to continue, or add --fallback <provider>."
        );
    } else {
        println!("\nAll strings translated.");
    }
    Ok(())
}

fn translation_error_summary(errors: &std::collections::HashMap<String, u64>) -> Vec<String> {
    let mut ranked: Vec<_> = errors.iter().collect();
    ranked.sort_by(|(a, a_count), (b, b_count)| b_count.cmp(a_count).then_with(|| a.cmp(b)));
    ranked
        .into_iter()
        .take(5)
        .map(|(message, count)| {
            // Keep summaries on one line and truncate on Unicode boundaries.
            let message = message.split_whitespace().collect::<Vec<_>>().join(" ");
            let message = if message.chars().count() > 240 {
                format!("{}...", message.chars().take(237).collect::<String>())
            } else {
                message
            };
            format!("{count} x {message}")
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Cli, Commands};
    use clap::Parser;

    #[test]
    fn provider_translation_defaults_cli_settings_and_partial_flags() {
        let configured = cli_translation_options("google", Some(40), None, None, None);
        assert_eq!(
            (
                configured.batch_size,
                configured.max_concurrent,
                configured.max_batch_tokens
            ),
            (40, 4, Some(40_000))
        );
        let explicit = cli_translation_options("google", Some(40), Some(7), None, None);
        assert_eq!(
            (
                explicit.batch_size,
                explicit.max_concurrent,
                explicit.max_batch_tokens
            ),
            (7, 4, Some(40_000))
        );
        let tokens = cli_translation_options("google", None, None, None, Some(900));
        assert_eq!(
            (
                tokens.batch_size,
                tokens.max_concurrent,
                tokens.max_batch_tokens
            ),
            (200, 4, Some(900))
        );
    }

    #[test]
    fn provider_translation_defaults_cli_flags() {
        for (provider, expected) in [
            ("google", (200, 4, Some(40_000))),
            ("antigravity", (100, 4, Some(20_000))),
            ("grok", (10, 10, Some(6000))),
            ("grok-sub", (10, 10, Some(6000))),
            ("ollama", (40, 1, Some(6000))),
        ] {
            for flags in [
                vec![],
                vec![
                    "--batch-size",
                    "7",
                    "--concurrency",
                    "2",
                    "--max-batch-tokens",
                    "900",
                ],
            ] {
                let mut args = vec!["locust", "translate", "project.db", "--provider", provider];
                args.extend(&flags);
                let cli = Cli::try_parse_from(args).unwrap();
                let Commands::Translate {
                    provider,
                    batch_size,
                    concurrency,
                    max_batch_tokens,
                    ..
                } = cli.command
                else {
                    panic!("expected translate")
                };
                let opts = cli_translation_options(
                    &provider,
                    None,
                    batch_size,
                    concurrency,
                    max_batch_tokens,
                );
                assert_eq!(
                    (opts.batch_size, opts.max_concurrent, opts.max_batch_tokens),
                    if flags.is_empty() {
                        expected
                    } else {
                        (7, 2, Some(900))
                    },
                    "{provider}"
                );
            }
        }
    }

    #[test]
    fn translation_error_summary_ranks_counts_and_limits_to_five() {
        let errors = [
            ("zeta", 1),
            ("beta", 4),
            ("alpha", 4),
            ("gamma", 3),
            ("delta", 2),
            ("common failure", 8193),
        ]
        .into_iter()
        .map(|(message, count)| (message.to_string(), count))
        .collect();
        assert_eq!(
            translation_error_summary(&errors),
            vec![
                "8193 x common failure",
                "4 x alpha",
                "4 x beta",
                "3 x gamma",
                "2 x delta",
            ]
        );
        assert!(translation_error_summary(&Default::default()).is_empty());
    }

    #[test]
    fn translation_error_summary_truncates_unicode_and_flattens_lines() {
        let errors = [
            (format!("  {}\r\n tail", "名".repeat(300)), 2),
            ("first\nsecond\tthird".into(), 1),
            ("a".repeat(240), 1),
        ]
        .into_iter()
        .collect();
        let summary = translation_error_summary(&errors);
        assert_eq!(summary[0], format!("2 x {}...", "名".repeat(237)));
        assert_eq!(summary[1], format!("1 x {}", "a".repeat(240)));
        assert_eq!(summary[2], "1 x first second third");
    }
}
