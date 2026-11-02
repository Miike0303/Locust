use std::future::Future;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;

use comfy_table::Table;
use indicatif::{ProgressBar, ProgressStyle};
use locust_core::config::AppConfig;
use locust_core::database::Database;
use locust_core::glossary::Glossary;
use locust_core::models::ProgressEvent;
use locust_core::translation::{run_fallback_chain, TranslationOptions, TranslationProvider};

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
    allow_partial: bool,
    allow_lossy_binary_fit: bool,
    fallback: Vec<String>,
    cost_limit: Option<f64>,
    context: Option<String>,
    files: Vec<PathBuf>,
    ids_from: Option<PathBuf>,
    priority: Vec<PathBuf>,
) -> anyhow::Result<()> {
    let db = Arc::new(open_existing_project(&project)?);
    let provider_reg = locust_providers::default_registry(&config);
    let scoped = !files.is_empty() || ids_from.is_some();

    let opts = TranslationOptions {
        source_lang: source,
        target_lang: target,
        use_memory: !no_memory,
        allow_lossy_binary_fit,
        cost_limit_usd: cost_limit,
        game_context: context,
        file_paths: (!files.is_empty()).then_some(files),
        entry_ids: ids_from.as_deref().map(read_scope_ids).transpose()?,
        file_priority: priority,
        ..cli_translation_options(
            &provider_id,
            config.default_batch_size,
            batch_size,
            concurrency,
            max_batch_tokens,
        )
    }
    .capture_scope(&db)?;

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

    run_translation(
        db,
        opts,
        chain,
        resolve_map,
        scoped,
        allow_partial,
        tokio::signal::ctrl_c(),
        &mut std::io::stdout(),
    )
    .await
}

// Keep the interrupt and summary sink injectable: tests exercise the real engine
// without sending process-wide signals to the test harness.
#[allow(clippy::too_many_arguments)]
async fn run_translation(
    db: Arc<Database>,
    opts: TranslationOptions,
    chain: Vec<String>,
    resolve_map: Arc<std::collections::HashMap<String, Arc<dyn TranslationProvider>>>,
    scoped: bool,
    allow_partial: bool,
    interrupt: impl Future<Output = std::io::Result<()>>,
    output: &mut impl Write,
) -> anyhow::Result<()> {
    let glossary = Arc::new(Glossary::new(db.clone()));
    let initial_pending = opts.pending_entries(&db)?.len();
    if initial_pending == 0 {
        if scoped {
            writeln!(output, "No pending strings in the selected scope.")?;
        } else {
            writeln!(output, "No pending strings.")?;
        }
        return Ok(());
    }
    writeln!(
        output,
        "Translation chain: {} · {} → {} · {} pending",
        chain.join(" → "),
        opts.source_lang,
        opts.target_lang,
        initial_pending
    )?;

    // Paused has no counters. Snapshot the ledger boundary so cancellation can
    // report the engine's final receipts without including earlier runs.
    let previous_run_id = db
        .get_translation_runs()?
        .iter()
        .map(|run| run.id)
        .max()
        .unwrap_or(0);
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
    let cancel_job = cancel.clone();
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
            cancel_job,
        )
        .await
    });

    let mut total_cost = 0.0;
    let mut cost_is_complete = false;
    let mut total_translated = 0;
    let mut errors = 0u64;
    let mut batch_failed = false;
    let mut error_counts = std::collections::HashMap::<String, u64>::new();
    let start = std::time::Instant::now();

    tokio::pin!(interrupt);
    let mut interrupted = false;
    let mut interrupt_error = None;
    loop {
        let event = tokio::select! {
            biased;
            signal = &mut interrupt, if !interrupted => {
                interrupted = true;
                interrupt_error = signal.err();
                cancel.cancel();
                // Do not poll a completed signal future again. Keep receiving
                // events and joining the worker so accounting reaches the ledger.
                continue;
            }
            event = rx.recv() => match event {
                Some(event) => event,
                None => break,
            },
        };
        batch_failed |= matches!(event, ProgressEvent::BatchFailed { .. });
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

    bar.finish_with_message(if interrupted { "Interrupted" } else { "Done!" });
    let result = handle.await;

    let mut tokens = (0u64, 0u64, 0u64);
    if interrupted {
        let runs = db.get_translation_runs()?;
        let runs: Vec<_> = runs
            .iter()
            .filter(|run| {
                run.id > previous_run_id
                    && chain.contains(&run.provider)
                    && run.source_lang == opts.source_lang
                    && run.target_lang == opts.target_lang
            })
            .collect();
        // The cancelled request may change completeness or add observed usage
        // after the last BatchCompleted. Use the ledger only once the engine exits.
        if !runs.is_empty() {
            total_translated = runs.iter().map(|run| run.strings_translated).sum();
            total_cost = runs.iter().map(|run| run.cost_usd).sum();
            cost_is_complete = runs.iter().all(|run| run.cost_is_complete);
            for run in runs {
                tokens.0 += run.tokens_used;
                tokens.1 += run.input_tokens;
                tokens.2 += run.output_tokens;
            }
        }
        writeln!(output, "Translation interrupted.")?;
    }

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
    if interrupted {
        table.add_row(vec!["Tokens used", &tokens.0.to_string()]);
        table.add_row(vec!["Input tokens", &tokens.1.to_string()]);
        table.add_row(vec!["Output tokens", &tokens.2.to_string()]);
    }
    writeln!(output, "{table}")?;

    if !error_counts.is_empty() {
        eprintln!("Translation errors (top 5):");
        for line in translation_error_summary(&error_counts) {
            eprintln!("  {line}");
        }
    }
    if interrupted {
        let left = opts.pending_entries(&db)?.len();
        writeln!(
            output,
            "\n{left} strings still pending. Re-run to continue."
        )?;
        if let Some(error) = interrupt_error {
            return Err(anyhow::Error::new(error)
                .context("Translation interrupted: could not listen for Ctrl+C"));
        }
        result??;
        anyhow::bail!("Translation interrupted");
    }
    result??;

    let left = opts.pending_entries(&db)?.len();
    if left > 0 {
        writeln!(
            output,
            "\n{left} strings still pending. Re-run to continue, or add --fallback <provider>."
        )?;
        if batch_failed && !allow_partial {
            anyhow::bail!(
                "Translation incomplete: {left} strings still pending after provider errors"
            );
        }
    } else if scoped {
        writeln!(output, "\nAll selected strings translated.")?;
    } else {
        writeln!(output, "\nAll strings translated.")?;
    }
    Ok(())
}

fn read_scope_ids(path: &std::path::Path) -> anyhow::Result<Vec<String>> {
    use anyhow::Context;
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("reading translation IDs from {}", path.display()))?;
    // Preserve significant whitespace in IDs; skip only truly empty lines.
    Ok(text
        .trim_start_matches('\u{feff}')
        .lines()
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
        .collect())
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
    fn scoped_translation_cli_repeatable_exact_paths_and_id_file() {
        let cli = Cli::try_parse_from([
            "locust",
            "translate",
            "project.db",
            "--provider",
            "google",
            "--file",
            "  menu_日本語.json  ",
            "--file",
            "chapter/one.json",
            "--ids-from",
            "ids.txt",
            "--priority",
            "chapter/one.json",
            "--priority",
            "  menu_日本語.json  ",
        ])
        .unwrap();
        let Commands::Translate {
            files,
            ids_from,
            priority,
            ..
        } = cli.command
        else {
            panic!("translate expected")
        };
        assert_eq!(
            files,
            [
                PathBuf::from("  menu_日本語.json  "),
                "chapter/one.json".into()
            ]
        );
        assert_eq!(ids_from, Some("ids.txt".into()));
        assert_eq!(
            priority,
            [
                PathBuf::from("chapter/one.json"),
                "  menu_日本語.json  ".into()
            ]
        );
    }

    #[test]
    fn scoped_translation_ids_file_preserves_whitespace_and_empty_means_none() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ids.txt");
        std::fs::write(&path, "\u{feff}a\r\n\n b 日本語 \r\na\n").unwrap();
        assert_eq!(read_scope_ids(&path).unwrap(), ["a", " b 日本語 ", "a"]);
        std::fs::write(&path, "\n\r\n").unwrap();
        assert!(read_scope_ids(&path).unwrap().is_empty());
        assert!(read_scope_ids(&dir.path().join("missing")).is_err());
        std::fs::write(&path, [0xff]).unwrap();
        assert!(read_scope_ids(&path).is_err());
    }

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

    mod cancellation {
        use super::*;
        use locust_core::database::{EntryFilter, TranslationRun};
        use locust_core::models::{StringEntry, StringStatus};
        use std::time::Duration;
        use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
        use tokio::sync::{oneshot, watch};

        const CANCEL_LIMIT: Duration = Duration::from_secs(2);

        fn options() -> TranslationOptions {
            TranslationOptions {
                source_lang: "en".into(),
                target_lang: "es".into(),
                batch_size: 1,
                max_concurrent: 1,
                use_memory: false,
                use_glossary: false,
                ..Default::default()
            }
        }

        fn project(path: &std::path::Path) -> Arc<Database> {
            let db = Arc::new(Database::open(path).unwrap());
            db.save_entries(&[
                StringEntry::new("a", "Hello there", "dialogue.json".into()),
                StringEntry::new("b", "Goodbye now", "dialogue.json".into()),
                StringEntry::new("c", "Welcome back", "dialogue.json".into()),
            ])
            .unwrap();
            db
        }

        fn providers(
            provider: Arc<dyn TranslationProvider>,
        ) -> Arc<std::collections::HashMap<String, Arc<dyn TranslationProvider>>> {
            Arc::new([(provider.id().to_owned(), provider)].into_iter().collect())
        }

        // First request reports real usage; the second waits indefinitely for
        // cancellation to close its connection. No external service or timing
        // guess is involved in choosing when to interrupt.
        async fn delayed_provider() -> (
            Arc<dyn TranslationProvider>,
            oneshot::Receiver<()>,
            oneshot::Receiver<()>,
            tokio::task::JoinHandle<()>,
        ) {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let url = format!("http://{}", listener.local_addr().unwrap());
            let (waiting_tx, waiting) = oneshot::channel();
            let (closed_tx, closed) = oneshot::channel();
            let server = tokio::spawn(async move {
                for request in 0..2 {
                    let (stream, _) = listener.accept().await.unwrap();
                    let mut stream = BufReader::new(stream);
                    let mut line = String::new();
                    stream.read_line(&mut line).await.unwrap();
                    assert!(line.starts_with("POST /v1/chat/completions "));
                    let mut length = None;
                    loop {
                        line.clear();
                        assert!(stream.read_line(&mut line).await.unwrap() > 0);
                        if line == "\r\n" {
                            break;
                        }
                        if let Some((name, value)) = line.split_once(':') {
                            if name.eq_ignore_ascii_case("content-length") {
                                length = Some(value.trim().parse::<usize>().unwrap());
                            }
                        }
                    }
                    stream
                        .read_exact(&mut vec![0; length.unwrap()])
                        .await
                        .unwrap();
                    if request == 1 {
                        waiting_tx.send(()).unwrap();
                        let mut byte = [0];
                        assert_eq!(stream.read(&mut byte).await.unwrap(), 0);
                        closed_tx.send(()).unwrap();
                        return;
                    }
                    let body = serde_json::json!({
                        "choices": [{"message": {"content": "[\"Hola\"]"}}],
                        "usage": {
                            "prompt_tokens": 1000,
                            "completion_tokens": 500,
                            "total_tokens": 1500
                        }
                    })
                    .to_string();
                    let response = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    stream.write_all(response.as_bytes()).await.unwrap();
                    stream.shutdown().await.unwrap();
                }
            });
            (
                Arc::new(locust_providers::openai::OpenAiProvider::new(
                    "fixture-key".into(),
                    None,
                    Some(url),
                )),
                waiting,
                closed,
                server,
            )
        }

        async fn interrupted_run(repeat: bool) {
            let dir = tempfile::tempdir().unwrap();
            let db = project(&dir.path().join("cancel.db"));
            // Existing project history must not enter this job's summary.
            db.record_translation_run(&TranslationRun {
                started_at: "2020-01-01T00:00:00Z".into(),
                provider: "openai".into(),
                source_lang: "en".into(),
                target_lang: "es".into(),
                strings_translated: 99,
                tokens_used: 99,
                cost_usd: 99.0,
                cost_is_complete: true,
                ..Default::default()
            })
            .await
            .unwrap();
            let (provider, waiting, closed, server) = delayed_provider().await;
            let (interrupt_tx, mut interrupt_rx) = watch::channel(0);
            let mut output = Vec::new();
            let result = {
                let interrupt = async {
                    interrupt_rx.changed().await.unwrap();
                    Ok(())
                };
                let job = run_translation(
                    db.clone(),
                    options(),
                    vec!["openai".into(), "mock".into()],
                    // A fallback must not run after cancellation.
                    Arc::new(
                        [
                            ("openai".into(), provider),
                            (
                                "mock".into(),
                                Arc::new(locust_providers::mock::MockProvider)
                                    as Arc<dyn TranslationProvider>,
                            ),
                        ]
                        .into_iter()
                        .collect(),
                    ),
                    false,
                    true, // --allow-partial must not turn an interrupt into success.
                    interrupt,
                    &mut output,
                );
                tokio::pin!(job);
                tokio::select! {
                    result = &mut job => panic!("job ended before interruption: {result:?}"),
                    ready = tokio::time::timeout(Duration::from_secs(10), waiting) => {
                        ready.unwrap().unwrap();
                    }
                }
                let committed = db.get_entry("a").unwrap().unwrap();
                assert_eq!(committed.status, StringStatus::Translated);
                assert_eq!(committed.translation.as_deref(), Some("Hola"));
                // Hold the ledger write so a second interrupt definitely arrives
                // while the command is still draining the engine.
                let lock = repeat.then(|| rusqlite::Connection::open(db.path()).unwrap());
                if let Some(lock) = &lock {
                    lock.execute_batch("BEGIN IMMEDIATE").unwrap();
                }
                interrupt_tx.send(1).unwrap();
                let drain_checks = async {
                    tokio::time::timeout(CANCEL_LIMIT, closed)
                        .await
                        .expect("interrupt did not cancel the provider promptly")
                        .unwrap();
                    if let Some(lock) = &lock {
                        interrupt_tx.send(2).unwrap();
                        // Give the draining command a poll with a second signal
                        // ready before allowing the engine's ledger write.
                        tokio::task::yield_now().await;
                        lock.execute_batch("ROLLBACK").unwrap();
                    }
                };
                let (result, ()) =
                    tokio::join!(tokio::time::timeout(CANCEL_LIMIT, &mut job), drain_checks);
                result.expect("cancelled job did not drain promptly")
            };
            server.await.unwrap();
            let error = result.expect_err("interrupted translation must fail");
            assert!(error.to_string().contains("interrupted"), "{error}");
            let rows = db.get_entries(&EntryFilter::default()).unwrap();
            assert_eq!(rows[0].translation.as_deref(), Some("Hola"));
            for row in &rows[1..] {
                assert_eq!(row.status, StringStatus::Pending);
                assert!(row.translation.is_none());
            }
            let runs = db.get_translation_runs().unwrap();
            assert_eq!(
                runs.len(),
                2,
                "worker accounting must be drained before return"
            );
            let run = &runs[1];
            assert_eq!(run.strings_translated, 1);
            assert_eq!(
                (run.tokens_used, run.input_tokens, run.output_tokens),
                (1500, 1000, 500)
            );
            assert!((run.cost_usd - 0.00045).abs() < 1e-10);
            assert!(
                !run.cost_is_complete,
                "cancelled paid request has unknown final charge"
            );
            let output = String::from_utf8(output).unwrap();
            assert!(output.contains("Translation interrupted"), "{output}");
            assert!(output.contains("2 strings still pending"), "{output}");
            for (metric, value) in [
                ("Total translated", "1".to_owned()),
                (
                    "Total cost",
                    format_observed_cost(run.cost_usd, run.cost_is_complete),
                ),
                ("Tokens used", run.tokens_used.to_string()),
                ("Input tokens", run.input_tokens.to_string()),
                ("Output tokens", run.output_tokens.to_string()),
            ] {
                let line = output.lines().find(|line| line.contains(metric)).unwrap();
                assert_eq!(line.split('|').nth(2).unwrap().trim(), value, "{output}");
            }
            assert!(!output.contains("All strings translated"), "{output}");
        }

        #[tokio::test]
        async fn interrupt_cancels_provider_preserves_commits_and_accounting() {
            interrupted_run(false).await;
        }

        #[tokio::test]
        async fn second_interrupt_while_draining_is_safe() {
            interrupted_run(true).await;
        }

        #[tokio::test]
        async fn uninterrupted_run_preserves_summary_output() {
            let dir = tempfile::tempdir().unwrap();
            let db = project(&dir.path().join("normal.db"));
            let mut output = Vec::new();
            run_translation(
                db.clone(),
                options(),
                vec!["mock".into()],
                providers(Arc::new(locust_providers::mock::MockProvider)),
                false,
                false,
                std::future::pending(),
                &mut output,
            )
            .await
            .unwrap();
            let output = String::from_utf8(output).unwrap();
            let mut expected = Table::new();
            expected.set_header(["Metric", "Value"]);
            expected.add_row(["Total translated", "3"]);
            // Runtime is the sole variable in the existing text output.
            let elapsed = output
                .lines()
                .find(|line| line.contains("Time elapsed"))
                .unwrap()
                .split('|')
                .nth(2)
                .unwrap()
                .trim();
            expected.add_row(["Time elapsed", elapsed]);
            expected.add_row(["Total cost", "$0.0000"]);
            expected.add_row(["Batch errors", "0"]);
            assert_eq!(output, format!(
                "Translation chain: mock · en → es · 3 pending\n{expected}\n\nAll strings translated.\n"
            ));
            assert!(options().pending_entries(&db).unwrap().is_empty());
        }
    }
}
