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
        /// Number of batches sent to the provider in parallel
        #[arg(long)]
        concurrency: Option<usize>,
        /// Providers to fall back to (in order) when the primary stops making
        /// progress — e.g. --fallback deepseek,lmstudio
        #[arg(long, value_delimiter = ',')]
        fallback: Vec<String>,
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
        /// Inject directly into game files without copying (fast, modifies originals)
        #[arg(long)]
        direct: bool,
    },
    /// Validate translations
    Validate { project: PathBuf },
    /// Show translation stats: tokens, time, and cost per run
    Stats { project: PathBuf },
    /// Pivot: seed a new project whose SOURCE is another project's translations,
    /// so you can translate e.g. JA→EN once, then EN→ES / EN→FR / EN→PT from it.
    Pivot {
        /// Existing project whose translations become the new source (e.g. the EN one)
        source: PathBuf,
        #[arg(short, long)]
        output: PathBuf,
    },
    /// Package translated files into a distributable patch zip (patch-only:
    /// just the translated game files, not the whole game)
    Patch {
        /// The INJECTED game folder (run `locust inject` first)
        game_path: PathBuf,
        #[arg(short = 'P', long)]
        project: PathBuf,
        /// Target language, used only for naming and the Astro stub
        #[arg(short, long)]
        lang: Option<String>,
        /// Output zip path (default: <game>-<lang>-patch.zip)
        #[arg(short, long)]
        output: Option<PathBuf>,
        /// Also write an Astro content stub (.md) for rule95 to this path
        #[arg(long)]
        astro: Option<PathBuf>,
    },
    /// Authenticate with a provider via OAuth (currently: grok)
    Auth {
        /// Provider to authenticate: grok
        provider: String,
    },
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

    let config = load_config(&cli.config);

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
            concurrency,
            fallback,
            cost_limit,
            context,
        } => {
            cmd_translate(
                config, project, provider, source, target, batch_size, concurrency, fallback,
                cost_limit, context,
            )
            .await?
        }
        Commands::Inject {
            game_path,
            project,
            mode,
            languages,
            output_dir,
            direct,
        } => {
            if direct {
                cmd_inject_direct(game_path, project, languages).await?
            } else {
                cmd_inject(game_path, project, mode, languages, output_dir).await?
            }
        }
        Commands::Validate { project } => cmd_validate(project)?,
        Commands::Stats { project } => cmd_stats(project)?,
        Commands::Pivot { source, output } => cmd_pivot(source, output)?,
        Commands::Patch {
            game_path,
            project,
            lang,
            output,
            astro,
        } => cmd_patch(game_path, project, lang, output, astro)?,
        Commands::Auth { provider } => cmd_auth(provider).await?,
        Commands::Providers => cmd_providers(&config)?,
        Commands::Formats => cmd_formats()?,
        Commands::Glossary { action } => cmd_glossary(action)?,
        Commands::Export {
            project,
            format,
            lang,
            output,
        } => cmd_export(&config, project, format, lang, output)?,
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

/// Path of a game file relative to the game root, so the patch zip mirrors the
/// game's own layout and extracts cleanly over any copy of the game.
///
/// Resolution is `game_path`-relative first. That is correct for EVERY engine
/// with no per-format knowledge at all — `game/script.rpy` for Ren'Py,
/// `www/data/Map001.json` for RPG Maker, `Data/x.wolf` for Wolf RPG, and a
/// bare `game.html` for the engines that keep their content at the game root.
///
/// `content_roots` is only a fallback, for entries whose stored path is not
/// under `game_path` (a moved game, or a differently-spelled path). Anchoring
/// on a component is second choice because it searches the whole absolute path:
/// a game living at `D:\game\MyVN\game\script.rpy` anchors on the OUTER `game`
/// and yields a path that resolves nowhere.
///
/// Returns `None` when neither resolves. Callers MUST treat that as a hard
/// error naming the unresolved path. There is deliberately no bare-filename
/// fallback: flattening a path discards the directory structure that made it
/// unique, which is what made patch archives pack nothing — or silently drop
/// colliding files — for every engine without a recognized content root.
fn rel_in_game(p: &std::path::Path, game_path: &std::path::Path, content_roots: &[&str]) -> Option<PathBuf> {
    if let Ok(rel) = p.strip_prefix(game_path) {
        if rel.as_os_str().is_empty() {
            return None;
        }
        return Some(rel.to_path_buf());
    }

    let comps: Vec<_> = p.components().collect();
    comps
        .iter()
        .position(|c| {
            c.as_os_str()
                .to_str()
                .is_some_and(|s| content_roots.contains(&s))
        })
        .map(|i| comps[i..].iter().collect())
}

/// Containers whose entries hold the SOURCE archive path rather than a file that
/// injection writes. Packing one ships the game's own original content in a patch
/// that claims to carry translated text only, and carries no translation at all,
/// because the output went somewhere else entirely.
///
/// Ren'Py `.rpa` is the verified case: `renpy.rs` rewrites `file_path` to the
/// archive for archive-sourced entries, while injection writes loose `.rpy` into
/// `game/` or generates `zzz_locust_translate.rpy` — neither of which is a
/// database entry.
///
/// Only add an extension here after confirming in that plugin's `inject` that it
/// does NOT write back to `entry.file_path`. Wolf RPG `.wolf`, Unity `.assets`
/// and Unreal `.pak` are byte-scan injectors that DO edit their file in place,
/// so excluding them would break packaging for those engines.
const SOURCE_ARCHIVE_EXTENSIONS: &[&str] = &["rpa"];

fn is_source_archive(p: &std::path::Path) -> bool {
    p.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| SOURCE_ARCHIVE_EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
}

fn cmd_patch(
    game_path: PathBuf,
    project: PathBuf,
    lang: Option<String>,
    output: Option<PathBuf>,
    astro: Option<PathBuf>,
) -> anyhow::Result<()> {
    use std::io::Write as _;

    let db = Database::open(&project)?;
    let entries = db.get_entries(&EntryFilter::default())?;

    // The content root is derived from the format plugin that recognizes
    // this game — never guessed from a hardcoded, format-agnostic list.
    let registry = locust_formats::default_registry();
    let content_roots: &[&str] = registry
        .detect(&game_path)
        .map(|p| p.content_roots())
        .unwrap_or(&[]);

    // Distinct game files that actually received a translation, keyed by
    // their game-root-relative archive path. Two different source files
    // that would collide on that path are a hard error, never a silent
    // drop.
    let mut files: Vec<PathBuf> = Vec::new();
    let mut seen: std::collections::HashMap<PathBuf, PathBuf> = std::collections::HashMap::new();
    let mut translated = 0usize;
    let mut source_archives: Vec<PathBuf> = Vec::new();
    for e in &entries {
        let has_text = e.translation.as_deref().is_some_and(|t| !t.trim().is_empty());
        if has_text && e.status == StringStatus::Translated {
            translated += 1;
            // Entries sourced from a container name the archive they were READ
            // from, not a file injection wrote. Packing it would put the game's
            // own original content in the patch and no translation at all.
            if is_source_archive(&e.file_path) {
                if !source_archives.contains(&e.file_path) {
                    source_archives.push(e.file_path.clone());
                }
                continue;
            }
            let rel = rel_in_game(&e.file_path, &game_path, content_roots).ok_or_else(|| {
                anyhow::anyhow!(
                    "cannot determine the game-relative path for \"{}\": it is not under \
                     the game directory \"{}\", and none of its components match a known \
                     content root {:?} for this format. Packaging an unresolved layout \
                     would silently discard the file, so this is refused instead.",
                    e.file_path.display(),
                    game_path.display(),
                    content_roots
                )
            })?;
            match seen.get(&rel) {
                Some(existing) if existing != &e.file_path => {
                    anyhow::bail!(
                        "two different source files map to the same patch archive path \
                         \"{}\": \"{}\" and \"{}\"",
                        rel.display(),
                        existing.display(),
                        e.file_path.display()
                    );
                }
                Some(_) => {}
                None => {
                    seen.insert(rel.clone(), e.file_path.clone());
                    files.push(rel);
                }
            }
        }
    }
    if files.is_empty() {
        anyhow::bail!(
            "no translated files found. Run `locust inject \"{}\" -P <db> --direct` first.",
            game_path.display()
        );
    }

    let out = output.unwrap_or_else(|| {
        let base = game_path.file_name().unwrap_or_default().to_string_lossy();
        let suffix = lang.as_deref().map(|l| format!("-{}", l)).unwrap_or_default();
        PathBuf::from(format!("{}{}-patch.zip", base, suffix))
    });

    let zip_file = std::fs::File::create(&out)?;
    let mut zip = zip::ZipWriter::new(zip_file);
    let opts = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);

    let mut added = 0usize;
    let mut missing_paths: Vec<PathBuf> = Vec::new();
    let mut skipped_unsafe = 0usize;
    for rel in &files {
        // Defense-in-depth: never read or pack a path that escapes the game
        // root. Guards against a corrupt/untrusted DB producing a zip-slip
        // entry, since this archive is redistributed to end users.
        if rel
            .components()
            .any(|c| matches!(c, std::path::Component::ParentDir))
        {
            skipped_unsafe += 1;
            continue;
        }
        let src = game_path.join(rel);
        let bytes = match std::fs::read(&src) {
            Ok(b) => b,
            Err(_) => {
                missing_paths.push(src);
                continue;
            }
        };
        // Zip entry names always use forward slashes.
        let name = rel.to_string_lossy().replace('\\', "/");
        zip.start_file(name, opts)?;
        zip.write_all(&bytes)?;
        added += 1;
    }
    let missing = missing_paths.len();

    // A patch holding nothing but the README is worse than a failure: it looks
    // like a success and ships to end users. This guard is what makes any
    // remaining path-resolution problem loud instead of silent.
    if added == 0 {
        drop(zip);
        let _ = std::fs::remove_file(&out);
        let shown: Vec<String> = missing_paths
            .iter()
            .take(5)
            .map(|p| p.display().to_string())
            .collect();
        anyhow::bail!(
            "packed 0 files, so no patch was written. {} translated file(s) were expected \
             but not found on disk{}{}. Run `locust inject \"{}\" -P <db> --direct` first, \
             or check that this is the same game copy the strings were extracted from.",
            missing,
            if shown.is_empty() {
                String::new()
            } else {
                format!(":\n  {}", shown.join("\n  "))
            },
            if missing > shown.len() {
                format!("\n  ... and {} more", missing - shown.len())
            } else {
                String::new()
            },
            game_path.display()
        );
    }

    let readme = "rule95 translation patch\n\n\
        Apply: extract this archive over your game folder, replacing the files\n\
        when asked. Back up your game folder first.\n\n\
        This patch contains translated text only. Get the game itself from the\n\
        original creator.\n";
    zip.start_file("README.txt", opts)?;
    zip.write_all(readme.as_bytes())?;
    zip.finish()?;

    if let Some(astro_path) = astro {
        write_astro_stub(&astro_path, &game_path, lang.as_deref())?;
        println!("Astro stub written to {}", astro_path.display());
    }

    let size = std::fs::metadata(&out).map(|m| m.len()).unwrap_or(0);
    let mut table = Table::new();
    table.set_header(vec!["Metric", "Value"]);
    table.add_row(vec!["Patch file", &out.display().to_string()]);
    table.add_row(vec!["Files packed", &added.to_string()]);
    if missing > 0 {
        table.add_row(vec!["Files not found on disk", &missing.to_string()]);
    }
    if skipped_unsafe > 0 {
        table.add_row(vec!["Skipped unsafe paths", &skipped_unsafe.to_string()]);
    }
    table.add_row(vec!["Translated strings", &translated.to_string()]);
    table.add_row(vec!["Size", &format!("{:.1} KB", size as f64 / 1024.0)]);
    println!("{table}");

    // Report what was actually observed (a file expected at path X was not
    // there) rather than asserting a cause — a prior version of this
    // message guessed "inject first?", which sent users toward a fix that
    // could not help when the real problem was an unresolved content root.
    if !missing_paths.is_empty() {
        println!("\nExpected file(s) not found on disk:");
        for p in missing_paths.iter().take(5) {
            println!("  {}", p.display());
        }
        if missing_paths.len() > 5 {
            println!("  ... and {} more", missing_paths.len() - 5);
        }
    }

    Ok(())
}

/// Write a starter rule95 content file. Fields we can infer are filled;
/// the rest (creator, versions, mirrors after upload) are left as TODO.
fn write_astro_stub(
    path: &std::path::Path,
    game_path: &std::path::Path,
    lang: Option<&str>,
) -> anyhow::Result<()> {
    let title = game_path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();
    let engine_hint = locust_formats::default_registry()
        .detect(game_path)
        .map(|p| p.id().to_string())
        .unwrap_or_else(|| "other".to_string());
    let target = lang.unwrap_or("es");

    let md = format!(
        "---\n\
         gameTitle: \"{title}\"\n\
         sourceLang: \"en\"        # TODO: ja | en | zh | ko | other\n\
         engine: \"{engine_hint}\"  # TODO: pick from the schema enum (rpgmaker-mv/mz/xp/vxace, ...)\n\
         tags: []\n\
         platforms: []            # F95 | DLsite | Ryuugames | Steam | Itch | Other\n\
         originalCreator:\n\
         \x20 name: \"TODO\"\n\
         \x20 links: []            # [{{ label: \"Patreon\", url: \"https://...\" }}]\n\
         storePage: \"\"           # TODO original game page\n\
         gameVersion: \"TODO\"\n\
         translationVersion: \"1.0\"\n\
         translationStatus: \"complete\"   # complete | in-progress\n\
         gameStatus: \"ongoing\"           # completed | ongoing\n\
         cover: \"\"               # TODO R2 URL\n\
         screenshots: []          # TODO R2 URLs\n\
         mirrors: []              # fill after uploading the patch zip to R2\n\
         dateAdded: TODO-YYYY-MM-DD\n\
         ---\n\n\
         Translation into {target} of *{title}*, made with Locust.\n\n\
         **How to apply:** extract the patch over the game folder, replacing files.\n"
    );

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, md)?;
    Ok(())
}

fn cmd_pivot(source: PathBuf, output: PathBuf) -> anyhow::Result<()> {
    use locust_core::models::StringEntry;

    let src_db = Database::open(&source)?;
    let entries = src_db.get_entries(&EntryFilter::default())?;

    // Each translated entry becomes a pending entry in the new project whose
    // SOURCE is the old translation. Ids, file paths, tags and speaker context
    // carry over unchanged, so inject/patch still target the same game files
    // and the next language keeps the same context.
    let mut pivoted: Vec<StringEntry> = Vec::new();
    for e in entries {
        let Some(translation) = e.translation.filter(|t| !t.trim().is_empty()) else {
            continue;
        };
        let mut ne = StringEntry::new(e.id, translation, e.file_path);
        ne.context = e.context;
        ne.tags = e.tags;
        ne.char_limit = e.char_limit;
        ne.metadata = e.metadata;
        pivoted.push(ne);
    }

    if pivoted.is_empty() {
        anyhow::bail!("no translated entries in {} to pivot from", source.display());
    }

    let out_db = Database::open(&output)?;
    let count = out_db.save_entries(&pivoted)?;

    let mut table = Table::new();
    table.set_header(vec!["Metric", "Value"]);
    table.add_row(vec!["Pivoted project", &output.display().to_string()]);
    table.add_row(vec!["Source entries used", &count.to_string()]);
    println!("{table}");
    println!(
        "\nNow translate the new project into any language, e.g.:\n  \
         locust translate \"{}\" -p grok-sub -s en -t fr",
        output.display()
    );
    Ok(())
}

fn cmd_stats(project: PathBuf) -> anyhow::Result<()> {
    let db = Database::open(&project)?;
    let runs = db.get_translation_runs()?;

    if runs.is_empty() {
        println!("No translation runs recorded yet for this project.");
        return Ok(());
    }

    let mut table = Table::new();
    table.set_header(vec![
        "Date", "Provider", "Langs", "Strings", "Tokens", "In", "Out", "Cost ($)", "Time",
    ]);
    let (mut t_strings, mut t_tokens, mut t_in, mut t_out, mut t_cost, mut t_secs) =
        (0usize, 0u64, 0u64, 0u64, 0f64, 0f64);
    for run in &runs {
        table.add_row(vec![
            run.started_at.chars().take(16).collect::<String>(),
            run.provider.clone(),
            format!("{}→{}", run.source_lang, run.target_lang),
            run.strings_translated.to_string(),
            run.tokens_used.to_string(),
            run.input_tokens.to_string(),
            run.output_tokens.to_string(),
            format!("{:.4}", run.cost_usd),
            format_duration(run.duration_secs),
        ]);
        t_strings += run.strings_translated;
        t_tokens += run.tokens_used;
        t_in += run.input_tokens;
        t_out += run.output_tokens;
        t_cost += run.cost_usd;
        t_secs += run.duration_secs;
    }
    table.add_row(vec![
        "TOTAL".to_string(),
        String::new(),
        String::new(),
        t_strings.to_string(),
        t_tokens.to_string(),
        t_in.to_string(),
        t_out.to_string(),
        format!("{:.4}", t_cost),
        format_duration(t_secs),
    ]);
    println!("{table}");
    Ok(())
}

fn format_duration(secs: f64) -> String {
    let s = secs as u64;
    if s >= 3600 {
        format!("{}h {}m", s / 3600, (s % 3600) / 60)
    } else if s >= 60 {
        format!("{}m {}s", s / 60, s % 60)
    } else {
        format!("{}s", s)
    }
}

async fn cmd_auth(provider: String) -> anyhow::Result<()> {
    match provider.as_str() {
        "grok" | "grok-sub" | "xai" => {
            locust_providers::xai_oauth::device_login().await?;
            println!("\nLogged in to xAI.");
            println!("Translate with your subscription: locust translate <db> -p grok-sub");
        }
        other => anyhow::bail!("unknown auth provider: {}. Available: grok", other),
    }
    Ok(())
}

fn load_config(path: &Option<PathBuf>) -> AppConfig {
    let path = path.clone().unwrap_or_else(AppConfig::default_path);
    AppConfig::load(&path).unwrap_or_else(|e| {
        eprintln!("warning: could not load config {}: {}", path.display(), e);
        AppConfig::default()
    })
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

#[allow(clippy::too_many_arguments)]
async fn cmd_translate(
    config: AppConfig,
    project: PathBuf,
    provider_id: String,
    source: String,
    target: String,
    batch_size: Option<usize>,
    concurrency: Option<usize>,
    fallback: Vec<String>,
    cost_limit: Option<f64>,
    context: Option<String>,
) -> anyhow::Result<()> {
    let db = Arc::new(Database::open(&project)?);
    let provider_reg = locust_providers::default_registry(&config);
    let glossary = Arc::new(Glossary::new(db.clone()));

    let opts = TranslationOptions {
        source_lang: source,
        target_lang: target,
        batch_size: batch_size.unwrap_or(40),
        max_concurrent: concurrency.unwrap_or(TranslationOptions::default().max_concurrent),
        cost_limit_usd: cost_limit,
        game_context: context,
        ..Default::default()
    };

    let pending_count = || -> anyhow::Result<usize> {
        Ok(db
            .get_entries(&EntryFilter::default())?
            .iter()
            .filter(|e| e.status == StringStatus::Pending)
            .count())
    };

    // Try the primary provider, then each fallback in order. A provider is
    // abandoned once it stops reducing the pending count (out of credits,
    // rate-limited, refusing every batch); the next one picks up the rest.
    let chain: Vec<String> = std::iter::once(provider_id).chain(fallback).collect();

    for (i, id) in chain.iter().enumerate() {
        let remaining = pending_count()?;
        if remaining == 0 {
            break;
        }
        let provider = match provider_reg.get(id) {
            Some(p) => p,
            None => {
                eprintln!("provider '{}' not found, skipping", id);
                continue;
            }
        };
        if i > 0 {
            println!("\nFalling back to provider: {}", provider.name());
        }
        let before = remaining;
        run_provider_pass(provider, db.clone(), glossary.clone(), opts.clone()).await?;
        let after = pending_count()?;
        if after == 0 {
            break;
        }
        if after >= before {
            eprintln!(
                "Provider '{}' made no progress ({} still pending).",
                id, after
            );
        }
    }

    let left = pending_count()?;
    if left > 0 {
        println!(
            "\n{} strings still pending. Re-run to continue, or add --fallback <provider>.",
            left
        );
    } else {
        println!("\nAll strings translated.");
    }
    Ok(())
}

/// Run one provider over the project's pending strings until it finishes or
/// stops making progress. Reads pending fresh so repeated calls resume.
async fn run_provider_pass(
    provider: Arc<dyn locust_core::translation::TranslationProvider>,
    db: Arc<Database>,
    glossary: Arc<Glossary>,
    opts: TranslationOptions,
) -> anyhow::Result<()> {
    // Only translate PENDING entries. Fetching all statuses would let a
    // fallback provider re-translate (and possibly overwrite/re-bill) strings
    // an earlier provider already finished, and would clobber human-reviewed
    // work. Restricting to pending also makes re-runs a clean resume.
    let entries: Vec<_> = db
        .get_entries(&EntryFilter::default())?
        .into_iter()
        .filter(|e| e.status == StringStatus::Pending)
        .collect();
    let pending = entries.len();
    if pending == 0 {
        return Ok(());
    }

    println!(
        "Provider: {}, {} → {}, {} pending strings",
        provider.name(),
        opts.source_lang,
        opts.target_lang,
        pending
    );

    let manager = TranslationManager::new(provider, db.clone(), glossary);
    let (tx, mut rx) = mpsc::channel(1000);
    let cancel = tokio_util::sync::CancellationToken::new();

    let bar = ProgressBar::new(pending as u64);
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
    let mut errors = 0u64;

    while let Some(event) = rx.recv().await {
        match event {
            ProgressEvent::BatchCompleted { completed, cost_so_far, .. } => {
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
                errors += 1;
                if errors <= 3 {
                    bar.println(format!("Error: {}", error));
                }
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
    table.add_row(vec!["Time elapsed", &format_duration(elapsed)]);
    table.add_row(vec!["Total cost", &format!("${:.4}", total_cost)]);
    table.add_row(vec!["Batch errors", &errors.to_string()]);
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

    // Auto-rotate: keep only the 3 most recent backups to prevent disk bloat
    if let Ok(deleted) = backup_mgr.delete_old_backups(3) {
        if deleted > 0 {
            println!("Cleaned {} old backup(s)", deleted);
        }
    }

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

async fn cmd_inject_direct(
    game_path: PathBuf,
    project: PathBuf,
    _languages: Vec<String>,
) -> anyhow::Result<()> {
    let db = Database::open(&project)?;
    let registry = locust_formats::default_registry();

    let plugin = registry
        .detect(&game_path)
        .ok_or_else(|| anyhow::anyhow!("format not detected for: {}", game_path.display()))?;

    let entries = db.get_entries(&EntryFilter::default())?;
    let translated: Vec<_> = entries
        .into_iter()
        .filter(|e| e.translation.is_some())
        .collect();

    println!(
        "Direct inject: {} translated strings into {} ({})",
        translated.len(),
        game_path.display(),
        plugin.name()
    );

    let report = plugin.inject(&game_path, &translated)?;

    let mut table = Table::new();
    table.set_header(vec!["Metric", "Value"]);
    table.add_row(vec!["Files modified", &report.files_modified.to_string()]);
    table.add_row(vec!["Strings written", &report.strings_written.to_string()]);
    table.add_row(vec!["Strings skipped", &report.strings_skipped.to_string()]);
    if !report.warnings.is_empty() {
        table.add_row(vec!["Warnings", &report.warnings.len().to_string()]);
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

fn cmd_providers(config: &AppConfig) -> anyhow::Result<()> {
    let reg = locust_providers::default_registry(config);
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
    config: &AppConfig,
    project: PathBuf,
    format: String,
    lang: String,
    output: PathBuf,
) -> anyhow::Result<()> {
    let db = Database::open(&project)?;
    let entries = db.get_entries(&EntryFilter::default())?;

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
    let state = locust_server::create_app_state();
    println!("Starting Project Locust server on http://localhost:{}", port);
    println!("Press Ctrl+C to stop");
    locust_server::start_server(state, port).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_cli_parses() {
        // Verify the CLI struct parses without panicking
        use clap::CommandFactory;
        Cli::command().debug_assert();
    }

    #[test]
    fn test_rel_in_game_is_game_path_relative() {
        use std::path::Path;
        let game = Path::new("/games/Game");

        // The game-relative path is correct for every engine WITHOUT consulting
        // content roots at all — that is the point of resolving against the game
        // directory first.
        for (abs, want) in [
            ("/games/Game/www/data/Map001.json", "www/data/Map001.json"), // RPG Maker MV
            ("/games/Game/data/System.json", "data/System.json"),         // RPG Maker MZ
            ("/games/Game/Data/Map084.rxdata", "Data/Map084.rxdata"),     // VX Ace / XP
            ("/games/Game/game/script.rpy", "game/script.rpy"),           // Ren'Py
            ("/games/Game/Data/BasicData.wolf", "Data/BasicData.wolf"),   // Wolf RPG
            ("/games/Game/story.html", "story.html"),                     // SugarCube: game root
            ("/games/Game/scene01.json", "scene01.json"),                 // VNTextPatch: game root
        ] {
            assert_eq!(
                rel_in_game(Path::new(abs), game, &[]),
                Some(PathBuf::from(want)),
                "{abs} should resolve relative to the game directory"
            );
        }
    }

    #[test]
    fn test_rel_in_game_falls_back_to_content_root_when_outside_game_dir() {
        use std::path::Path;
        // A stored path that is not under the game directory (moved game, or a
        // differently spelled path) falls back to anchoring on a content root.
        assert_eq!(
            rel_in_game(
                Path::new("/elsewhere/OldCopy/www/data/Map001.json"),
                Path::new("/games/Game"),
                &["www", "data"]
            ),
            Some(PathBuf::from("www/data/Map001.json"))
        );
    }

    #[test]
    fn test_rel_in_game_unresolvable_is_none_never_a_bare_filename() {
        use std::path::Path;
        // Neither under the game dir nor anchorable: the caller must hard-error.
        // A bare-filename fallback here is what silently discarded directory
        // structure and produced README-only patch archives.
        assert_eq!(
            rel_in_game(
                Path::new("/weird/place/strings.txt"),
                Path::new("/games/Game"),
                &["www", "data"]
            ),
            None
        );
    }

    #[test]
    fn test_rel_in_game_prefers_game_dir_over_ancestor_named_like_a_root() {
        use std::path::Path;
        // A game living under an ancestor directory literally named `game`.
        // Anchoring on the first matching component would pick the OUTER one and
        // yield `game/MyVN/game/script.rpy`, which resolves nowhere.
        assert_eq!(
            rel_in_game(
                Path::new("/game/MyVN/game/script.rpy"),
                Path::new("/game/MyVN"),
                &["game"]
            ),
            Some(PathBuf::from("game/script.rpy"))
        );
    }

    #[test]
    fn test_source_archive_detection() {
        use std::path::Path;
        // Ren'Py rewrites file_path to the .rpa for archive-sourced entries,
        // while injection writes loose .rpy files instead. Packing the .rpa
        // would ship the game's original content and no translation.
        assert!(is_source_archive(Path::new("/g/game/scripts.rpa")));
        assert!(is_source_archive(Path::new("/g/game/SCRIPTS.RPA")));
        // In-place byte-scan injectors write back to these, so they must remain
        // packable.
        assert!(!is_source_archive(Path::new("/g/Data/BasicData.wolf")));
        assert!(!is_source_archive(Path::new("/g/G_Data/resources.assets")));
        assert!(!is_source_archive(Path::new("/g/game/script.rpy")));
    }

    // ─── cmd_patch packaging tests (bug: content-root fallback flattens
    // paths for every non-RPG-Maker engine, see auto-patch-apply design
    // Decision 7) ────────────────────────────────────────────────────────

    use locust_core::models::StringEntry;

    fn patch_test_tempdir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("locust_patch_test_{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn test_patch_packages_renpy_game_files_under_game_root() {
        let base = patch_test_tempdir();
        let game_dir = base.join("mygame");
        let game_sub = game_dir.join("game");
        fs::create_dir_all(&game_sub).unwrap();
        let script_path = game_sub.join("script.rpy");
        let contents = "label start:\n    \"Hola\"\n";
        fs::write(&script_path, contents).unwrap();

        let db_path = base.join("project.locust.db");
        let db = Database::open(&db_path).unwrap();
        let mut entry = StringEntry::new("renpy#0", "Hello", script_path.clone());
        entry.translation = Some("Hola".to_string());
        entry.status = StringStatus::Translated;
        db.save_entries(&[entry]).unwrap();

        let out_zip = base.join("out-patch.zip");
        cmd_patch(game_dir, db_path, None, Some(out_zip.clone()), None).unwrap();

        let file = fs::File::open(&out_zip).unwrap();
        let mut archive = zip::ZipArchive::new(file).unwrap();
        let mut zf = archive
            .by_name("game/script.rpy")
            .expect("game/script.rpy must be packed in the archive");
        let mut read_back = String::new();
        use std::io::Read as _;
        zf.read_to_string(&mut read_back).unwrap();
        assert_eq!(read_back, contents);
    }

    #[test]
    fn test_patch_errors_instead_of_writing_a_readme_only_archive() {
        let base = patch_test_tempdir();
        let game_dir = base.join("mygame");
        let game_sub = game_dir.join("game");
        fs::create_dir_all(&game_sub).unwrap();

        // The entry resolves to a sane game-relative path, but the file is not on
        // disk (a pre-inject database, or a different copy of the game).
        let db_path = base.join("project.locust.db");
        let db = Database::open(&db_path).unwrap();
        let mut entry = StringEntry::new("renpy#0", "Hello", game_sub.join("script.rpy"));
        entry.translation = Some("Hola".to_string());
        entry.status = StringStatus::Translated;
        db.save_entries(&[entry]).unwrap();

        let out_zip = base.join("out-patch.zip");
        let err = cmd_patch(game_dir, db_path, None, Some(out_zip.clone()), None)
            .expect_err("packing zero files must be an error, not a README-only archive");
        let msg = err.to_string();
        assert!(
            msg.contains("packed 0 files"),
            "error must say nothing was packed, got: {msg}"
        );
        assert!(
            msg.contains("script.rpy"),
            "error must name the file it could not find, got: {msg}"
        );
        assert!(
            !out_zip.exists(),
            "no archive may be left behind when nothing was packed"
        );
    }

    #[test]
    fn test_patch_never_packs_a_source_archive() {
        let base = patch_test_tempdir();
        let game_dir = base.join("renpygame");
        let game_sub = game_dir.join("game");
        fs::create_dir_all(&game_sub).unwrap();

        // Ren'Py rewrites file_path to the .rpa for archive-sourced entries. The
        // archive exists on disk, so nothing stops it from being read — packing it
        // would ship the game's original content and zero translations.
        let rpa_path = game_sub.join("scripts.rpa");
        fs::write(&rpa_path, b"RPA-3.0 fake original archive bytes").unwrap();
        // A real translated file alongside it, so the run has something to pack
        // and does not merely trip the zero-files guard.
        let loose = game_sub.join("zzz_locust_translate.rpy");
        fs::write(&loose, "# translations\n").unwrap();

        let db_path = base.join("project.locust.db");
        let db = Database::open(&db_path).unwrap();
        let mut from_rpa = StringEntry::new("scripts.rpa#0", "Hello", rpa_path.clone());
        from_rpa.translation = Some("Hola".to_string());
        from_rpa.status = StringStatus::Translated;
        let mut from_loose = StringEntry::new("zzz#0", "Hi", loose.clone());
        from_loose.translation = Some("Hola".to_string());
        from_loose.status = StringStatus::Translated;
        db.save_entries(&[from_rpa, from_loose]).unwrap();

        let out_zip = base.join("out-patch.zip");
        cmd_patch(game_dir, db_path, None, Some(out_zip.clone()), None).unwrap();

        let file = fs::File::open(&out_zip).unwrap();
        let mut archive = zip::ZipArchive::new(file).unwrap();
        let names: Vec<String> = (0..archive.len())
            .map(|i| archive.by_index(i).unwrap().name().to_string())
            .collect();
        assert!(
            !names.iter().any(|n| n.ends_with(".rpa")),
            "a source archive must never be packed, got entries: {names:?}"
        );
        assert!(
            names.iter().any(|n| n == "game/zzz_locust_translate.rpy"),
            "the actual translated file must still be packed, got entries: {names:?}"
        );
    }

    #[test]
    fn test_patch_packages_rpgmaker_www_data_unchanged() {
        let base = patch_test_tempdir();
        let game_dir = base.join("rpggame");
        let data_dir = game_dir.join("www").join("data");
        fs::create_dir_all(&data_dir).unwrap();
        let map_path = data_dir.join("Map001.json");
        let contents = r#"{"displayName":"Hola"}"#;
        fs::write(&map_path, contents).unwrap();

        let db_path = base.join("project.locust.db");
        let db = Database::open(&db_path).unwrap();
        let mut entry = StringEntry::new("rpg#0", "Hello", map_path.clone());
        entry.translation = Some("Hola".to_string());
        entry.status = StringStatus::Translated;
        db.save_entries(&[entry]).unwrap();

        let out_zip = base.join("out-patch.zip");
        cmd_patch(game_dir, db_path, None, Some(out_zip.clone()), None).unwrap();

        let file = fs::File::open(&out_zip).unwrap();
        let mut archive = zip::ZipArchive::new(file).unwrap();
        let mut zf = archive
            .by_name("www/data/Map001.json")
            .expect("www/data/Map001.json must be packed in the archive");
        let mut read_back = String::new();
        use std::io::Read as _;
        zf.read_to_string(&mut read_back).unwrap();
        assert_eq!(read_back, contents);
    }

    #[test]
    fn test_patch_errors_on_archive_path_collision_between_distinct_files() {
        let base = patch_test_tempdir();
        // Only used so the Ren'Py plugin is detected (content root "game");
        // the colliding entries below point at unrelated fabricated paths.
        let game_dir = base.join("realgame");
        let game_sub = game_dir.join("game");
        fs::create_dir_all(&game_sub).unwrap();
        fs::write(game_sub.join("dummy.rpy"), "").unwrap();

        let db_path = base.join("project.locust.db");
        let db = Database::open(&db_path).unwrap();

        // Two DISTINCT source files that both reduce to the same archive
        // path "game/script.rpy" once the "game" content-root anchor is
        // applied — e.g. two different copies of the game contributing a
        // same-named script.
        let path_a = base.join("copyA").join("game").join("script.rpy");
        let path_b = base.join("copyB").join("game").join("script.rpy");

        let mut entry_a = StringEntry::new("a#0", "Hello", path_a.clone());
        entry_a.translation = Some("Hola".to_string());
        entry_a.status = StringStatus::Translated;
        let mut entry_b = StringEntry::new("b#0", "World", path_b.clone());
        entry_b.translation = Some("Mundo".to_string());
        entry_b.status = StringStatus::Translated;
        db.save_entries(&[entry_a, entry_b]).unwrap();

        let out_zip = base.join("out-patch.zip");
        let err = cmd_patch(game_dir, db_path, None, Some(out_zip), None).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains(&path_a.display().to_string()),
            "error should name {}: {}",
            path_a.display(),
            msg
        );
        assert!(
            msg.contains(&path_b.display().to_string()),
            "error should name {}: {}",
            path_b.display(),
            msg
        );
    }

    #[test]
    fn test_patch_errors_on_unresolvable_layout_instead_of_flattening() {
        let base = patch_test_tempdir();
        // A directory with no plugin-recognized layout at all.
        let game_dir = base.join("unknown_engine_game");
        fs::create_dir_all(&game_dir).unwrap();

        let db_path = base.join("project.locust.db");
        let db = Database::open(&db_path).unwrap();
        let orphan_path = base.join("somewhere").join("deep").join("strings.txt");
        let mut entry = StringEntry::new("x#0", "Hello", orphan_path.clone());
        entry.translation = Some("Hola".to_string());
        entry.status = StringStatus::Translated;
        db.save_entries(&[entry]).unwrap();

        let out_zip = base.join("out-patch.zip");
        let err = cmd_patch(game_dir, db_path, None, Some(out_zip), None).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains(&orphan_path.display().to_string()),
            "error should name the unresolved path: {}",
            msg
        );
    }
}
