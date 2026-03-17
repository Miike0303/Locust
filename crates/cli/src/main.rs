use std::path::PathBuf;
use std::sync::Arc;

use clap::{Parser, Subcommand};
use comfy_table::{Cell, Table};
use indicatif::{ProgressBar, ProgressStyle};
use tokio::sync::mpsc;

use locust_core::backup::BackupManager;
use locust_core::config::AppConfig;
use locust_core::database::{Database, EntryFilter};
use locust_core::export;
use locust_core::extraction::FormatRegistry;
use locust_core::glossary::Glossary;
use locust_core::models::{OutputMode, ProgressEvent, StringStatus};
use locust_core::translation::{TranslationManager, TranslationOptions};
use locust_core::validation::Validator;

#[derive(Parser)]
#[command(name = "locust", about = "Project Locust — Universal game translation tool")]
#[command(version, author)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
    #[arg(long, global = true, help = "Enable verbose logging")]
    verbose: bool,
    #[arg(long, global = true, env = "LOCUST_CONFIG")]
    config: Option<PathBuf>,
}

#[derive(Subcommand)]
enum Commands {
    /// Extract translatable strings from a game
    Extract {
        path: PathBuf,
        #[arg(short, long)]
        format: Option<String>,
        #[arg(short, long)]
        output: Option<PathBuf>,
    },
    /// Translate extracted strings using a provider
    Translate {
        project: PathBuf,
        #[arg(short = 'p', long)]
        provider: String,
        #[arg(short, long, default_value = "ja")]
        source: String,
        #[arg(short, long, default_value = "en")]
        target: String,
        #[arg(long)]
        batch_size: Option<usize>,
        #[arg(long)]
        cost_limit: Option<f64>,
        #[arg(long)]
        context: Option<String>,
    },
    /// Inject translations back into the game
    Inject {
        game_path: PathBuf,
        #[arg(short = 'P', long)]
        project: PathBuf,
        #[arg(short, long)]
        mode: Option<String>,
        #[arg(short, long, num_args = 1..)]
        languages: Vec<String>,
        #[arg(short, long)]
        output_dir: Option<PathBuf>,
    },
    /// Validate translations
    Validate { project: PathBuf },
    /// List available translation providers
    Providers,
    /// List supported game formats
    Formats,
    /// Manage glossary terms
    Glossary {
        #[command(subcommand)]
        action: GlossaryCommands,
    },
    /// Export translations to PO or XLIFF
    Export {
        project: PathBuf,
        #[arg(short, long)]
        format: String,
        #[arg(short, long)]
        lang: String,
        #[arg(short, long)]
        output: PathBuf,
    },
    /// Import translations from PO or XLIFF
    Import {
        project: PathBuf,
        #[arg(short, long)]
        format: String,
        #[arg(short, long)]
        lang: String,
        #[arg(short, long)]
        input: PathBuf,
    },
    /// Start the web server
    Server {
        #[arg(short, long, default_value = "3000")]
        port: Option<u16>,
    },
}

#[derive(Subcommand)]
enum GlossaryCommands {
    Add {
        project: PathBuf,
        #[arg(short, long)]
        term: String,
        #[arg(short = 'T', long)]
        translation: String,
        #[arg(short, long)]
        lang_pair: String,
    },
    List {
        project: PathBuf,
        #[arg(short, long)]
        lang_pair: String,
    },
    Delete {
        project: PathBuf,
        #[arg(short, long)]
        term: String,
        #[arg(short, long)]
        lang_pair: String,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    let filter = if cli.verbose {
        "debug"
    } else {
        "info"
    };
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .init();

    match cli.command {
        Commands::Extract {
            path,
            format,
            output,
        } => cmd_extract(path, format, output)?,
        Commands::Translate {
            project,
            provider,
            source,
            target,
            batch_size,
            cost_limit,
            context,
        } => {
            cmd_translate(project, provider, source, target, batch_size, cost_limit, context)
                .await?
        }
        Commands::Inject {
            game_path,
            project,
            mode,
            languages,
            output_dir,
        } => cmd_inject(game_path, project, mode, languages, output_dir).await?,
        Commands::Validate { project } => cmd_validate(project)?,
        Commands::Providers => cmd_providers()?,
        Commands::Formats => cmd_formats()?,
        Commands::Glossary { action } => cmd_glossary(action)?,
        Commands::Export {
            project,
            format,
            lang,
            output,
        } => cmd_export(project, format, lang, output)?,
        Commands::Import {
            project,
            format,
            lang,
            input,
        } => cmd_import(project, format, lang, input).await?,
        Commands::Server { port } => cmd_server(port.unwrap_or(3000)).await?,
    }

    Ok(())
}

fn cmd_extract(
    path: PathBuf,
    format: Option<String>,
    output: Option<PathBuf>,
) -> anyhow::Result<()> {
    let registry = locust_formats::default_registry();

    let plugin = if let Some(ref fmt) = format {
        registry
            .get(fmt)
            .ok_or_else(|| anyhow::anyhow!("format not found: {}", fmt))?
    } else {
        println!("Detecting format...");
        registry
            .detect(&path)
            .ok_or_else(|| anyhow::anyhow!("format not detected for path: {}", path.display()))?
    };

    println!("Format: {} ({})", plugin.name(), plugin.id());

    let entries = plugin.extract(&path)?;
    let total = entries.len();

    let db_path = output.unwrap_or_else(|| {
        let name = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        PathBuf::from(format!("{}.locust.db", name))
    });

    let db = Database::open(&db_path)?;
    db.save_entries(&entries)?;

    let mut table = Table::new();
    table.set_header(vec!["Property", "Value"]);
    table.add_row(vec!["Format", plugin.name()]);
    table.add_row(vec![
        "Strings extracted",
        &total.to_string(),
    ]);
    table.add_row(vec!["Output file", &db_path.display().to_string()]);
    println!("{table}");

    Ok(())
}

async fn cmd_translate(
    project: PathBuf,
    provider_id: String,
    source: String,
    target: String,
    batch_size: Option<usize>,
    cost_limit: Option<f64>,
    context: Option<String>,
) -> anyhow::Result<()> {
    let db = Arc::new(Database::open(&project)?);
    let config = AppConfig::default();
    let provider_reg = locust_providers::default_registry(&config);

    let provider = provider_reg
        .get(&provider_id)
        .ok_or_else(|| anyhow::anyhow!("provider not found: {}", provider_id))?;

    let glossary = Arc::new(Glossary::new(db.clone()));

    let entries = db.get_entries(&EntryFilter::default())?;
    let pending: Vec<_> = entries
        .iter()
        .filter(|e| e.status == StringStatus::Pending)
        .collect();

    println!(
        "Provider: {}, {} → {}, {} pending strings",
        provider.name(),
        source,
        target,
        pending.len()
    );

    let opts = TranslationOptions {
        source_lang: source,
        target_lang: target,
        batch_size: batch_size.unwrap_or(40),
        cost_limit_usd: cost_limit,
        game_context: context,
        ..Default::default()
    };

    let manager = TranslationManager::new(provider, db.clone(), glossary);
    let (tx, mut rx) = mpsc::channel(1000);
    let cancel = tokio_util::sync::CancellationToken::new();

    let bar = ProgressBar::new(pending.len() as u64);
    bar.set_style(
        ProgressStyle::default_bar()
            .template("{spinner:.green} [{bar:40.cyan/blue}] {pos}/{len} {msg}")
            .unwrap()
            .progress_chars("█▓░"),
    );

    let job_id = uuid::Uuid::new_v4().to_string();

    let handle = tokio::spawn(async move {
        manager
            .translate_entries(entries, opts, tx, job_id, cancel)
            .await
    });

    let start = std::time::Instant::now();
    let mut total_cost = 0.0;
    let mut total_translated = 0;

    while let Some(event) = rx.recv().await {
        match event {
            ProgressEvent::BatchCompleted {
                completed, total, cost_so_far, ..
            } => {
                bar.set_position(completed as u64);
                bar.set_message(format!("${:.4}", cost_so_far));
                total_cost = cost_so_far;
                total_translated = completed;
            }
            ProgressEvent::Completed { total_translated: tt, total_cost: tc, .. } => {
                total_translated = tt;
                total_cost = tc;
            }
            ProgressEvent::Failed { error, .. } => {
                bar.println(format!("Error: {}", error));
            }
            _ => {}
        }
    }

    bar.finish_with_message("Done!");
    handle.await??;

    let elapsed = start.elapsed().as_secs_f64();
    let mut table = Table::new();
    table.set_header(vec!["Metric", "Value"]);
    table.add_row(vec!["Total translated", &total_translated.to_string()]);
    table.add_row(vec!["Time elapsed", &format!("{:.1}s", elapsed)]);
    table.add_row(vec!["Total cost", &format!("${:.4}", total_cost)]);
    if elapsed > 0.0 {
        table.add_row(vec![
            "Strings/sec",
            &format!("{:.1}", total_translated as f64 / elapsed),
        ]);
    }
    println!("{table}");

    Ok(())
}

async fn cmd_inject(
    game_path: PathBuf,
    project: PathBuf,
    mode: Option<String>,
    languages: Vec<String>,
    output_dir: Option<PathBuf>,
) -> anyhow::Result<()> {
    let db = Arc::new(Database::open(&project)?);
    let registry = Arc::new(locust_formats::default_registry());

    let plugin = registry
        .detect(&game_path)
        .ok_or_else(|| anyhow::anyhow!("format not detected"))?;

    let format_id = plugin.id().to_string();

    // Use short temp path for backups to avoid Windows MAX_PATH issues
    let backup_root = std::env::temp_dir().join("locust_bak");
    std::fs::create_dir_all(&backup_root).ok();
    let backup_mgr = Arc::new(BackupManager::new(backup_root));

    println!("Creating backup...");
    let injector =
        locust_core::extraction::MultiLangInjector::new(registry, db, backup_mgr);

    let mode = match mode.as_deref() {
        Some("add") => OutputMode::Add,
        _ => OutputMode::Replace,
    };

    let (tx, mut rx) = mpsc::channel(100);
    let report = injector
        .inject(&game_path, &format_id, mode, languages, output_dir, tx)
        .await?;

    while rx.recv().await.is_some() {}

    let mut table = Table::new();
    table.set_header(vec!["Property", "Value"]);
    table.add_row(vec![
        "Languages processed",
        &report.languages_processed.join(", "),
    ]);
    table.add_row(vec!["Backup ID", &report.backup_id]);
    for (lang, rep) in &report.reports {
        table.add_row(vec![
            &format!("{} files modified", lang),
            &rep.files_modified.to_string(),
        ]);
        table.add_row(vec![
            &format!("{} strings written", lang),
            &rep.strings_written.to_string(),
        ]);
    }
    println!("{table}");

    Ok(())
}

fn cmd_validate(project: PathBuf) -> anyhow::Result<()> {
    let db = Database::open(&project)?;
    let entries = db.get_entries(&EntryFilter::default())?;
    let issues = Validator::validate_all(&entries);

    if issues.is_empty() {
        println!("No validation issues found.");
        return Ok(());
    }

    let mut table = Table::new();
    table.set_header(vec!["Entry ID", "Kind", "Message"]);
    for issue in &issues {
        let kind = format!("{:?}", issue.kind);
        table.add_row(vec![
            Cell::new(&issue.entry_id),
            Cell::new(&kind),
            Cell::new(&issue.message),
        ]);
    }
    println!("{table}");
    println!("\n{} issues found.", issues.len());

    std::process::exit(1);
}

fn cmd_providers() -> anyhow::Result<()> {
    let config = AppConfig::default();
    let reg = locust_providers::default_registry(&config);
    let providers = reg.list();

    let mut table = Table::new();
    table.set_header(vec!["ID", "Name", "Free", "Requires API Key"]);
    for p in &providers {
        table.add_row(vec![
            p.id.clone(),
            p.name.clone(),
            if p.is_free { "Yes".to_string() } else { "No".to_string() },
            if p.requires_api_key { "Yes".to_string() } else { "No".to_string() },
        ]);
    }
    println!("{table}");

    Ok(())
}

fn cmd_formats() -> anyhow::Result<()> {
    let registry = locust_formats::default_registry();
    let formats = registry.list();

    let mut table = Table::new();
    table.set_header(vec!["ID", "Name", "Extensions", "Modes"]);
    for f in &formats {
        let modes: Vec<&str> = f
            .supported_modes
            .iter()
            .map(|m| match m {
                OutputMode::Replace => "Replace",
                OutputMode::Add => "Add",
            })
            .collect();
        table.add_row(vec![
            &f.id,
            &f.name,
            &f.extensions.join(", "),
            &modes.join(", "),
        ]);
    }
    println!("{table}");

    Ok(())
}

fn cmd_glossary(action: GlossaryCommands) -> anyhow::Result<()> {
    match action {
        GlossaryCommands::Add {
            project,
            term,
            translation,
            lang_pair,
        } => {
            let db = Arc::new(Database::open(&project)?);
            let glossary = Glossary::new(db);
            glossary.add(&term, &translation, &lang_pair, None)?;
            println!("Added: {} → {} ({})", term, translation, lang_pair);
        }
        GlossaryCommands::List { project, lang_pair } => {
            let db = Arc::new(Database::open(&project)?);
            let glossary = Glossary::new(db);
            let entries = glossary.get_all(&lang_pair)?;
            let mut table = Table::new();
            table.set_header(vec!["Term", "Translation", "Lang Pair"]);
            for e in &entries {
                table.add_row(vec![&e.term, &e.translation, &e.lang_pair]);
            }
            println!("{table}");
        }
        GlossaryCommands::Delete {
            project,
            term,
            lang_pair,
        } => {
            let db = Arc::new(Database::open(&project)?);
            let glossary = Glossary::new(db);
            glossary.delete(&term, &lang_pair)?;
            println!("Deleted: {} ({})", term, lang_pair);
        }
    }
    Ok(())
}

fn cmd_export(
    project: PathBuf,
    format: String,
    lang: String,
    output: PathBuf,
) -> anyhow::Result<()> {
    let db = Database::open(&project)?;
    let entries = db.get_entries(&EntryFilter::default())?;
    let config = AppConfig::default();

    let content = match format.as_str() {
        "po" => export::export_po(&entries, &config.default_source_lang, &lang),
        "xliff" => export::export_xliff(&entries, &config.default_source_lang, &lang),
        _ => anyhow::bail!("unsupported export format: {}. Use 'po' or 'xliff'", format),
    };

    std::fs::write(&output, &content)?;
    println!("Exported {} entries to {}", entries.len(), output.display());
    Ok(())
}

async fn cmd_import(
    project: PathBuf,
    format: String,
    lang: String,
    input: PathBuf,
) -> anyhow::Result<()> {
    let db = Database::open(&project)?;
    let content = std::fs::read_to_string(&input)?;

    let mut imported = 0;
    match format.as_str() {
        "po" => {
            let entries = export::import_po(&content)?;
            for pe in &entries {
                if !pe.translation.is_empty() {
                    if let Some(ref id) = pe.id {
                        db.save_translation(id, &pe.translation, "import").await?;
                        imported += 1;
                    }
                }
            }
        }
        "xliff" => {
            let units = export::import_xliff(&content)?;
            for unit in &units {
                if !unit.target.is_empty() {
                    db.save_translation(&unit.id, &unit.target, "import")
                        .await?;
                    imported += 1;
                }
            }
        }
        _ => anyhow::bail!("unsupported import format: {}. Use 'po' or 'xliff'", format),
    }

    println!("Imported {} translations from {}", imported, input.display());
    Ok(())
}

async fn cmd_server(port: u16) -> anyhow::Result<()> {
    let state = locust_server::create_test_state();
    println!("Starting Project Locust server on http://localhost:{}", port);
    println!("Press Ctrl+C to stop");
    locust_server::start_server(state, port).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cli_parses() {
        // Verify the CLI struct parses without panicking
        use clap::CommandFactory;
        Cli::command().debug_assert();
    }
}
