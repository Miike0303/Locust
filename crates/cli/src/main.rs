use std::path::{Path, PathBuf};
use std::sync::Arc;

use clap::{Parser, Subcommand};
use comfy_table::{Cell, Table};
use indicatif::{ProgressBar, ProgressStyle};
use tokio::sync::mpsc;

use locust_core::backup::BackupManager;
use locust_core::config::AppConfig;
use locust_core::database::{Database, EntryFilter};
use locust_core::export;
use locust_core::glossary::Glossary;
use locust_core::models::{OutputMode, ProgressEvent, StringEntry};
use locust_core::translation::{load_pending_entries, run_fallback_chain, TranslationOptions};
use locust_core::validation::{count_binary_slot_oversize, Validator};

/// Surface binary inject oversize (Unity/Unreal/Wolf) before the engine silently
/// skips those strings. Full detail: `locust validate`. MultiLangInjector also
/// emits a ValidationFailed progress event for server/desktop injects.
fn warn_binary_slot_oversize(entries: &[StringEntry]) {
    let n = count_binary_slot_oversize(entries);
    if n > 0 {
        eprintln!(
            "Warning: {n} translation(s) exceed binary inject slot length \
             (UTF-8 / UTF-16LE / Shift-JIS) and will be skipped by the engine. \
             Run `locust validate` for entry IDs, or shorten those strings."
        );
    }
}

#[derive(Parser)]
#[command(
    name = "locust",
    about = "Project Locust — Universal game translation tool"
)]
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
        /// Target language(s). Selects the recording `locust patch` packs; also
        /// names Replace output folders and Add-mode language files. Required for
        /// Replace/Add; optional with --direct (the recording is then
        /// language-unspecified and packed by `patch` without -l)
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
    /// Find and replace text inside translations in a project DB
    Replace {
        project: PathBuf,
        /// Text to find in translations
        #[arg(long)]
        find: String,
        /// Replacement text (may be empty)
        #[arg(long, default_value = "")]
        replace: String,
        /// Case-sensitive matching (default: case-insensitive)
        #[arg(long)]
        case_sensitive: bool,
        /// Preview only — do not write the database
        #[arg(long)]
        dry_run: bool,
    },
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
        /// Target language. Selects which injection recording to pack; also names
        /// the zip and the Astro stub. Required when more than one language is
        /// recorded
        #[arg(short, long)]
        lang: Option<String>,
        /// Output zip path (default: <game>-<lang>-patch.zip)
        #[arg(short, long)]
        output: Option<PathBuf>,
        /// Also write an Astro content stub (.md) for rule95 to this path
        #[arg(long)]
        astro: Option<PathBuf>,
        /// Optional pristine (pre-inject) game tree used to fill original_sha256
        /// in the patch manifest. Enables strict-tier verification on apply.
        /// Resolution order when omitted: <game>/.locust/backup/ if valid → none.
        #[arg(long)]
        pristine: Option<PathBuf>,
    },
    /// Apply a patch zip to a game folder (verify → backup → write → receipt)
    Apply {
        /// Game root to patch
        game_path: PathBuf,
        /// Local patch zip produced by `locust patch` (omit when using --url)
        zip: Option<PathBuf>,
        /// Download this patch zip URL (http/https) then apply
        #[arg(long)]
        url: Option<String>,
        /// Override verification blocks (mismatch, already-applied, unknown, downgrade)
        #[arg(long)]
        force: bool,
        /// Accept legacy zips without locust-patch.json, and structural-tier
        /// patches that lack original hashes
        #[arg(long)]
        confirm_legacy: bool,
        /// Plan only — no files written
        #[arg(long)]
        dry_run: bool,
    },
    /// Roll a game back to the pre-apply state stored in .locust/backup/
    PatchRollback {
        game_path: PathBuf,
        /// Delete user-edited patch-added files without confirmation
        #[arg(long)]
        force: bool,
    },
    /// Show whether a game has a Locust patch applied
    PatchStatus { game_path: PathBuf },
    /// Authenticate with a provider via OAuth (currently: grok)
    Auth {
        /// Provider to authenticate: grok
        provider: String,
    },
    /// List available translation providers
    Providers,
    /// List supported game formats
    Formats,
    /// Register an extra UI language on an RPG Maker MV/MZ multi-lang game
    /// (Iavra Languages + VisuMZ Options + boot Map choices). Creates `*.bak-locust` backups.
    RegisterLang {
        /// Deployed game root (folder with js/ and data/)
        game_path: PathBuf,
        /// Language code written into packs/options (e.g. es)
        #[arg(short, long)]
        lang: String,
        /// Menu label (e.g. Español)
        #[arg(long, default_value = "Español")]
        label: String,
    },
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

    let filter = if cli.verbose { "debug" } else { "info" };
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
                config,
                project,
                provider,
                source,
                target,
                batch_size,
                concurrency,
                fallback,
                cost_limit,
                context,
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
        Commands::Replace {
            project,
            find,
            replace,
            case_sensitive,
            dry_run,
        } => cmd_replace(project, find, replace, case_sensitive, dry_run).await?,
        Commands::Stats { project } => cmd_stats(project)?,
        Commands::Pivot { source, output } => cmd_pivot(source, output)?,
        Commands::Patch {
            game_path,
            project,
            lang,
            output,
            astro,
            pristine,
        } => cmd_patch(game_path, project, lang, output, astro, pristine)?,
        Commands::Apply {
            game_path,
            zip,
            url,
            force,
            confirm_legacy,
            dry_run,
        } => cmd_apply(game_path, zip, url, force, confirm_legacy, dry_run)?,
        Commands::PatchRollback { game_path, force } => cmd_patch_rollback(game_path, force)?,
        Commands::PatchStatus { game_path } => cmd_patch_status(game_path)?,
        Commands::Auth { provider } => cmd_auth(provider).await?,
        Commands::Providers => cmd_providers(&config)?,
        Commands::Formats => cmd_formats()?,
        Commands::RegisterLang {
            game_path,
            lang,
            label,
        } => cmd_register_lang(game_path, lang, label)?,
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

/// Engines whose `inject` writes into the tree the ENTRIES name instead of the
/// tree it is handed, verified plugin by plugin: Unity and Unreal ignore the
/// path argument entirely; Wolf RPG prefers `entry.file_path` whenever the
/// original file still exists. For these, Replace mode's per-language copy
/// never receives the writes, so only `--direct` (where the entry tree IS the
/// target tree) records correctly today.
///
/// ponytail: a hand-maintained list until plugins declare which tree they
/// write to; ceiling — a NEW entry-tree-writing plugin gets the generic
/// remedy text until it is added here.
fn writes_to_entry_tree(format_id: &str) -> bool {
    matches!(format_id, "unity" | "unreal" | "wolf-rpg")
}

/// Engines whose `inject` mutates the ORIGINAL game tree: the entry-tree
/// writers, plus Ren'Py, whose loose scripts are rewritten in place (writes
/// go to `entry.file_path`) even in Replace mode. Once such an inject has
/// run, the original source text its injector scans for is gone: a bare
/// re-run writes nothing (or, for a mixed Ren'Py game, only the
/// archive-derived files) and the recording silently omits the rest — so
/// EVERY remedy issued from a mutated (or possibly mutated) state must
/// carry the restore step. `replace_containment_remedy` and
/// `maybe_mutated_note` are the two renderers of that step; any new error
/// branch that advises a re-run must go through one of them.
fn mutates_original_tree(format_id: &str) -> bool {
    locust_core::extraction::mutates_original_tree(format_id)
}

/// Central backup root for inject / direct-inject. `LOCUST_BACKUP_ROOT` isolates
/// tests and lets operators put backups on a larger volume; default matches the
/// historical `temp_dir()/locust_bak` path named in recovery messages.
fn locust_backup_root() -> PathBuf {
    std::env::var_os("LOCUST_BACKUP_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("locust_bak"))
}

/// The restore step shared by every remedy issued from a state where the
/// ORIGINAL game tree was provably already mutated. Advice without it is a
/// closed loop: the re-run finds no original text, writes nothing, and
/// records nothing.
const RESTORE_ORIGINAL_FIRST: &str =
    "Your original game was modified — restore it from the backup listed above \
     (or from a clean copy) first.";

/// Appended to `patch`-side advice that names a `--direct` re-run, for
/// engines that mutate the original tree. `patch` cannot know whether a
/// prior inject already ran (a legacy database records nothing), so this
/// note is conditional where the containment remedies are imperative —
/// without it, a legacy database on an already-injected game loops on the
/// identical error forever.
fn maybe_mutated_note(game_path: &std::path::Path) -> &'static str {
    let mutates = locust_formats::default_registry()
        .detect(game_path)
        .is_some_and(|p| mutates_original_tree(p.id()));
    if mutates {
        "\nThis engine writes translations into the ORIGINAL game files: if this \
         game was already injected (for example through an older Locust that kept \
         no recording), that command will report 0 files written and record \
         nothing — restore the original game files from a backup or a clean copy \
         first, then re-run it."
    } else {
        ""
    }
}

/// Remedy for a Replace-mode containment failure, per engine — advice that
/// cannot work for the engine that produced the error is a closed loop, so
/// each branch names only commands proven to unblock that engine class.
fn replace_containment_remedy(
    format_id: &str,
    game_path: &std::path::Path,
    project: &std::path::Path,
    lang: &str,
) -> String {
    let g = game_path.display();
    let p = project.display();
    if writes_to_entry_tree(format_id) {
        // A bare `--direct` re-run from this state writes nothing (the
        // original bytes were already replaced) and records nothing, so the
        // restore step is part of the remedy, not optional advice.
        format!(
            "This engine writes translations into the ORIGINAL game files, so Replace \
             mode cannot produce a translated copy for it yet. {RESTORE_ORIGINAL_FIRST} \
             Then run: locust inject \"{g}\" -P \"{p}\" --direct -l {lang} — \
             direct mode records what it writes, and `locust patch \"{g}\" -P \"{p}\" \
             -l {lang}` packs from that recording."
        )
    } else if format_id == "renpy" {
        // The loose scripts were already rewritten in place when this error
        // fires, so the restore step applies exactly as it does for the
        // entry-tree writers: without it a `--direct` re-run skips every
        // already-translated line — zero writes for a loose-only game, or a
        // recording that silently omits the loose translations for a mixed
        // loose+.rpa game.
        format!(
            "Ren'Py writes loose scripts into the ORIGINAL tree even in Replace mode. \
             {RESTORE_ORIGINAL_FIRST} Then use Add mode: locust inject \"{g}\" -P \
             \"{p}\" -m add -l {lang}, or direct mode: locust inject \"{g}\" -P \
             \"{p}\" --direct -l {lang}."
        )
    } else {
        format!("Run: locust inject \"{g}\" -P \"{p}\" --direct -l {lang}")
    }
}

/// Remedy for a per-language Add-mode failure on an engine without Add
/// support, per engine: Replace works for path-derived engines, but for
/// entry-tree writers it hits the containment hard-error, so advising it
/// there would be a dead end.
fn add_mode_remedy(
    format_id: &str,
    game_path: &std::path::Path,
    project: &std::path::Path,
    languages: &[String],
) -> String {
    let g = game_path.display();
    let p = project.display();
    let langs = languages.join(" ");
    if writes_to_entry_tree(format_id) {
        format!(
            "This engine does not support Add mode, and it writes translations into \
             the original game files, so Replace mode cannot work for it either. Use \
             direct mode: locust inject \"{g}\" -P \"{p}\" --direct -l {langs}"
        )
    } else {
        format!(
            "This engine does not support Add mode. Use Replace mode: locust inject \
             \"{g}\" -P \"{p}\" -l {langs} -o <output_dir>, or direct mode: locust \
             inject \"{g}\" -P \"{p}\" --direct -l {langs}"
        )
    }
}

/// Surface a recording outcome to the user. The zero-write cases MUST be
/// printed: a silent keep was the stale-recording hazard, and a silent
/// nothing-recorded run sends the user into `locust patch` advising the very
/// command that just reported zero writes.
fn print_record_outcome(
    label: &str,
    outcome: &locust_core::extraction::RecordOutcome,
    rep: &locust_core::extraction::InjectionReport,
    format_id: &str,
) {
    use locust_core::extraction::RecordOutcome;
    match outcome {
        RecordOutcome::Recorded { .. } => {}
        RecordOutcome::KeptPrevious { recorded_at } => {
            println!("{label}: 0 files written — previous recording (dated {recorded_at}) kept")
        }
        RecordOutcome::NothingRecorded => {
            // Say WHY nothing was recorded and name a remedy that fits this
            // state — `locust patch` on this project will only ever advise
            // the inject that just wrote zero files.
            let cause = if rep.strings_skipped > 0 {
                format!(
                    " {} string(s) could not be applied (see the warnings below) — \
                     fix those translations and re-run.",
                    rep.strings_skipped
                )
            } else {
                String::new()
            };
            let restore = if mutates_original_tree(format_id) {
                " If this game was ALREADY injected, the original text this engine \
                 scans for is gone and every re-run will keep writing 0 files — \
                 restore the original game files from a backup or a clean copy, \
                 then re-run the inject."
            } else {
                ""
            };
            println!(
                "{label}: 0 files written — nothing was recorded, and `locust patch` \
                 refuses to pack until an inject writes at least one file.{cause}{restore}"
            );
            for w in rep.warnings.iter().take(5) {
                println!("  warning: {w}");
            }
            if rep.warnings.len() > 5 {
                println!("  ... and {} more warning(s)", rep.warnings.len() - 5);
            }
        }
    }
}

fn cmd_patch(
    game_path: PathBuf,
    project: PathBuf,
    lang: Option<String>,
    output: Option<PathBuf>,
    astro: Option<PathBuf>,
    pristine: Option<PathBuf>,
) -> anyhow::Result<()> {
    use locust_core::patch::{pack_injection_recording, PackOptions};

    let db = Database::open(&project)?;
    let out = output.unwrap_or_else(|| {
        let base = game_path.file_name().unwrap_or_default().to_string_lossy();
        let suffix = lang.as_deref().map(|l| format!("-{l}")).unwrap_or_default();
        PathBuf::from(format!("{base}{suffix}-patch.zip"))
    });
    let engine = detect_engine_label(&game_path);

    let report = pack_injection_recording(
        &db,
        PackOptions {
            game_path: game_path.clone(),
            lang: lang.clone(),
            output: out,
            pristine,
            engine: Some(engine),
            project: project.clone(),
            require_pristine: false,
        },
    )
    .map_err(|e| {
        // Preserve CLI remedies that mention inject paths when useful.
        let mut msg = e.to_string();
        if msg.contains("no injection has been recorded") {
            msg = format!("{msg}{}", maybe_mutated_note(&game_path));
        }
        anyhow::anyhow!(msg)
    })?;

    for m in &report.messages {
        println!("note: {m}");
    }

    if let Some(astro_path) = astro {
        write_astro_stub(&astro_path, &game_path, lang.as_deref())?;
        println!("Astro stub written to {}", astro_path.display());
    }

    let mut table = Table::new();
    table.set_header(vec!["Metric", "Value"]);
    table.add_row(vec!["Patch file", &report.output_path]);
    table.add_row(vec![
        "Recording",
        &report
            .recording_lang
            .clone()
            .unwrap_or_else(|| "(unspecified)".into()),
    ]);
    table.add_row(vec!["Recorded root", &report.recorded_root]);
    table.add_row(vec!["Files packed", &report.files_packed.to_string()]);
    table.add_row(vec![
        "Translated strings",
        &report.translated_strings.to_string(),
    ]);
    table.add_row(vec![
        "Size",
        &format!("{:.1} KB", report.size_bytes as f64 / 1024.0),
    ]);
    table.add_row(vec!["Patch id", &report.patch_id]);
    table.add_row(vec!["Version", &report.patch_version]);
    table.add_row(vec!["Tier", &report.tier]);
    println!("{table}");

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
         **How to apply:** `locust apply <game> <patch.zip>` (or `--url https://…/patch.zip`), \
         or the desktop Patch modal. This creates a restorable backup; \
         `locust patch-rollback <game>` undoes it.\n"
    );

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, md)?;
    Ok(())
}

fn detect_engine_label(game_path: &std::path::Path) -> String {
    let registry = locust_formats::default_registry();
    registry
        .detect(game_path)
        .map(|p| p.id().to_string())
        .unwrap_or_else(|| "unknown".into())
}

fn cmd_apply(
    game_path: PathBuf,
    zip: Option<PathBuf>,
    url: Option<String>,
    force: bool,
    confirm_legacy: bool,
    dry_run: bool,
) -> anyhow::Result<()> {
    use locust_core::patch::{apply, ApplyOptions};

    if !game_path.is_dir() {
        anyhow::bail!("game path is not a directory: {}", game_path.display());
    }

    // Keep download tempdir alive for the whole apply.
    let mut _download_guard: Option<tempfile::TempDir> = None;
    let zip_path = match (zip, url) {
        (Some(p), None) => {
            if !p.is_file() {
                anyhow::bail!("patch zip not found: {}", p.display());
            }
            p
        }
        (None, Some(u)) => {
            let dir = tempfile::tempdir()?;
            let dest = dir.path().join("locust-patch.zip");
            println!("downloading {u} …");
            download_patch_zip(&u, &dest)?;
            println!("saved {}", dest.display());
            _download_guard = Some(dir);
            dest
        }
        (Some(_), Some(_)) => {
            anyhow::bail!("pass either a local zip path or --url, not both");
        }
        (None, None) => {
            anyhow::bail!("patch zip path or --url is required");
        }
    };

    let opts = ApplyOptions {
        force,
        confirm_legacy,
        dry_run,
    };
    let report = apply(&game_path, &zip_path, opts, |p| {
        println!("[{}/{}] {} ({})", p.current, p.total, p.path, p.phase);
    })?;

    if !report.user_edits_overwritten.is_empty() {
        println!(
            "warning: overwriting {} user-edited added file(s):",
            report.user_edits_overwritten.len()
        );
        for p in &report.user_edits_overwritten {
            println!("  {p}");
        }
    }
    for m in &report.messages {
        println!("{m}");
    }
    println!(
        "patch {}@{} {} — replaced {}, added {} (baseline: {:?})",
        report.patch_id,
        report.patch_version,
        if report.dry_run { "planned" } else { "applied" },
        report.replaced,
        report.added,
        report.baseline
    );
    Ok(())
}

/// Download a patch zip over http(s) with size and scheme guards.
fn download_patch_zip(url: &str, dest: &Path) -> anyhow::Result<()> {
    let max_bytes = locust_core::patch::zipsec::max_download_bytes();

    let parsed = reqwest::Url::parse(url).map_err(|e| anyhow::anyhow!("invalid URL: {e}"))?;
    match parsed.scheme() {
        "http" | "https" => {}
        other => anyhow::bail!("only http/https URLs are allowed (got {other})"),
    }

    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(30 * 60))
        .connect_timeout(std::time::Duration::from_secs(30))
        .redirect(reqwest::redirect::Policy::limited(5))
        .user_agent(format!("locust/{}", env!("CARGO_PKG_VERSION")))
        .build()?;

    let mut resp = client.get(parsed).send()?.error_for_status()?;
    if let Some(len) = resp.content_length() {
        if len > max_bytes {
            anyhow::bail!("remote zip too large: {len} bytes (max {max_bytes})");
        }
    }

    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut file = std::fs::File::create(dest)?;
    let mut written: u64 = 0;
    let mut buf = [0u8; 1024 * 256];
    loop {
        let n = {
            use std::io::Read;
            resp.read(&mut buf)?
        };
        if n == 0 {
            break;
        }
        written += n as u64;
        if written > max_bytes {
            let _ = std::fs::remove_file(dest);
            anyhow::bail!("download exceeded {max_bytes} bytes — aborted");
        }
        use std::io::Write;
        file.write_all(&buf[..n])?;
    }
    if written == 0 {
        let _ = std::fs::remove_file(dest);
        anyhow::bail!("download produced an empty file");
    }
    println!("downloaded {written} bytes");
    Ok(())
}

fn cmd_patch_rollback(game_path: PathBuf, force: bool) -> anyhow::Result<()> {
    use locust_core::patch::{rollback, RollbackOptions};

    if !game_path.is_dir() {
        anyhow::bail!("game path is not a directory: {}", game_path.display());
    }
    let report = rollback(
        &game_path,
        RollbackOptions {
            delete_modified_added: force,
        },
    )?;
    if !report.aborted_edited.is_empty() {
        println!("rollback aborted — edited added files need --force:");
        for p in &report.aborted_edited {
            println!("  {p}");
        }
        return Ok(());
    }
    for m in &report.messages {
        println!("{m}");
    }
    if !report.torn_deleted.is_empty() {
        println!("torn files deleted (interrupted apply):");
        for p in &report.torn_deleted {
            println!("  {p}");
        }
    }
    println!(
        "rollback complete — restored {}, deleted {}",
        report.restored, report.deleted
    );
    Ok(())
}

fn cmd_patch_status(game_path: PathBuf) -> anyhow::Result<()> {
    use locust_core::patch::{PatchStatus, PatchStore};

    if !game_path.is_dir() {
        anyhow::bail!("game path is not a directory: {}", game_path.display());
    }
    let store = PatchStore::new(&game_path);
    match store.status()? {
        PatchStatus::NotPatched => println!("not patched"),
        PatchStatus::Patched(r) => {
            println!(
                "patched: {}@{} (engine {}, lang {}, baseline {:?}, forced={})",
                r.patch_id, r.patch_version, r.engine, r.language, r.baseline, r.forced
            );
            println!(
                "  replaced {} file(s), added {} file(s), applied_at {}",
                r.replaced.len(),
                r.added.len(),
                r.applied_at
            );
        }
        PatchStatus::Interrupted(j) => {
            println!(
                "INTERRUPTED apply of {} — run `locust patch-rollback \"{}\"`",
                j.patch_id,
                game_path.display()
            );
        }
        PatchStatus::Unknown => {
            println!(
                "unknown — .locust/ present but no usable receipt (run patch-status after apply, \
                 or patch-rollback if a backup exists)"
            );
        }
    }
    Ok(())
}

fn cmd_pivot(source: PathBuf, output: PathBuf) -> anyhow::Result<()> {
    // Shared with HTTP `POST /api/pivot` and the Tauri `run_pivot` command.
    let src_db = Database::open(&source)?;
    let result = src_db.pivot_to(&output)?;

    let mut table = Table::new();
    table.set_header(vec!["Metric", "Value"]);
    table.add_row(vec!["Pivoted project", &result.database_path]);
    table.add_row(vec!["Source entries used", &result.entries.to_string()]);
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
    table.add_row(vec!["Strings extracted", &total.to_string()]);
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
    let mut total_translated = 0;
    let mut errors = 0u64;
    let start = std::time::Instant::now();

    while let Some(event) = rx.recv().await {
        match event {
            ProgressEvent::Started { total, .. } => {
                bar.set_length(total as u64);
            }
            ProgressEvent::BatchCompleted {
                completed,
                cost_so_far,
                ..
            } => {
                bar.set_position(completed as u64);
                bar.set_message(format!("${:.4}", cost_so_far));
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
                ..
            } => {
                total_translated = tt;
                total_cost = tc;
            }
            ProgressEvent::Failed { error, .. } => {
                errors += 1;
                if errors <= 3 {
                    bar.println(format!("Error: {error}"));
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

async fn cmd_inject(
    game_path: PathBuf,
    project: PathBuf,
    mode: Option<String>,
    languages: Vec<String>,
    output_dir: Option<PathBuf>,
) -> anyhow::Result<()> {
    let mode = match mode.as_deref() {
        Some("add") => OutputMode::Add,
        _ => OutputMode::Replace,
    };
    let mode_name = match mode {
        OutputMode::Add => "Add",
        OutputMode::Replace => "Replace",
    };

    // A language-less Replace/Add run used to iterate over zero languages:
    // nothing copied, nothing injected, nothing recorded — and exit 0. That
    // silent no-op became an untranslated "patch" two commands later.
    if languages.is_empty() {
        anyhow::bail!(
            "{mode_name} mode requires at least one language: pass -l <lang>. The \
             language names the output (the Replace copy / the Add language files) \
             and selects the recording `locust patch` packs. Example: locust inject \
             \"{}\" -P \"{}\"{} -l es{}",
            game_path.display(),
            project.display(),
            if matches!(mode, OutputMode::Add) {
                " -m add"
            } else {
                ""
            },
            if matches!(mode, OutputMode::Replace) {
                " -o <output_dir>"
            } else {
                ""
            },
        );
    }

    let db = Arc::new(Database::open(&project)?);
    let registry = Arc::new(locust_formats::default_registry());

    let plugin = registry
        .detect(&game_path)
        .ok_or_else(|| anyhow::anyhow!("format not detected"))?;

    let format_id = plugin.id().to_string();

    // Use short temp path for backups to avoid Windows MAX_PATH issues
    // (overridable with LOCUST_BACKUP_ROOT — same helper as --direct).
    let backup_root = locust_backup_root();
    std::fs::create_dir_all(&backup_root).ok();
    let backup_mgr = Arc::new(BackupManager::new(backup_root));

    // Auto-rotate: keep only the 3 most recent backups to prevent disk bloat
    if let Ok(deleted) = backup_mgr.delete_old_backups(3) {
        if deleted > 0 {
            println!("Cleaned {} old backup(s)", deleted);
        }
    }

    let preflight_entries = db.get_entries(&EntryFilter::default())?;
    warn_binary_slot_oversize(&preflight_entries);

    println!("Creating backup...");
    let injector =
        locust_core::extraction::MultiLangInjector::new(registry, db.clone(), backup_mgr);

    let (tx, mut rx) = mpsc::channel(100);
    let report = injector
        .inject(
            &game_path,
            &format_id,
            mode,
            languages.clone(),
            output_dir,
            tx,
        )
        .await?;

    while rx.recv().await.is_some() {}

    // Persist, for EVERY processed language, the files its injection actually
    // wrote and the root it wrote them under — `locust patch` packs from this
    // recording. The recording itself lives in core (`record_multilang_injection`,
    // shared with the HTTP server and the desktop app so no inject seam can
    // skip it); the CLI supplies the per-engine remedy and surfaces the
    // zero-write outcomes.
    let outcomes =
        locust_core::extraction::record_multilang_injection(&db, &report, &languages, &|lang| {
            replace_containment_remedy(&format_id, &game_path, &project, lang)
        })?;
    for (lang, outcome) in &outcomes {
        if let Some(rep) = report.reports.get(lang) {
            print_record_outcome(lang, outcome, rep, &format_id);
        }
    }

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

    // Surface per-language failures. These used to be swallowed silently:
    // injection could fail for EVERY language while the command printed a
    // success table and exited 0 — and the user then shipped an untranslated
    // "patch" that claimed otherwise.
    if !report.languages_failed.is_empty() {
        let mut msg = format!(
            "injection failed for {} of {} language(s):",
            report.languages_failed.len(),
            languages.len()
        );
        for (lang, err) in &report.languages_failed {
            msg.push_str(&format!("\n  {lang}: {err}"));
        }
        if report
            .languages_failed
            .iter()
            .any(|(_, e)| e.contains("does not support Add mode"))
        {
            msg.push('\n');
            msg.push_str(&add_mode_remedy(
                &format_id, &game_path, &project, &languages,
            ));
        }
        anyhow::bail!("{msg}");
    }

    Ok(())
}

async fn cmd_inject_direct(
    game_path: PathBuf,
    project: PathBuf,
    languages: Vec<String>,
) -> anyhow::Result<()> {
    let db = Database::open(&project)?;
    let registry = locust_formats::default_registry();

    let plugin = registry
        .detect(&game_path)
        .ok_or_else(|| anyhow::anyhow!("format not detected for: {}", game_path.display()))?;

    let format_id = plugin.id().to_string();
    let entries = db.get_entries(&EntryFilter::default())?;
    let translated_count = entries.iter().filter(|e| e.translation.is_some()).count();
    println!(
        "Direct inject: {} translated strings into {} ({})",
        translated_count,
        game_path.display(),
        plugin.name()
    );
    warn_binary_slot_oversize(&entries);

    // Shared core path (HTTP + desktop): backup when the engine mutates
    // originals, inject in place, record for `locust patch`.
    let backup_root = locust_backup_root();
    std::fs::create_dir_all(&backup_root).ok();
    let mgr = BackupManager::new(backup_root);
    if locust_core::extraction::mutates_original_tree(&format_id) {
        println!("Creating backup (engine mutates the original game tree)...");
    }
    let report = locust_core::extraction::inject_direct(
        &registry, &db, &mgr, &game_path, &format_id, &languages,
    )?;

    // Contractual zero-write reporting (same messages as multilang inject):
    // silent keeps / silent nothing-recorded are the stale-recording hazard.
    for (lang, outcome) in &report.outcomes {
        if let Some(rep) = report.reports.get(lang) {
            print_record_outcome(lang, outcome, rep, &format_id);
        }
        if matches!(
            outcome,
            locust_core::extraction::RecordOutcome::Recorded { .. }
        ) {
            println!(
                "Recorded {} file(s) for {lang} (pack root = game folder).",
                report.files_written.len()
            );
        }
    }

    let mut table = Table::new();
    table.set_header(vec!["Metric", "Value"]);
    table.add_row(vec!["Files modified", &report.files_modified.to_string()]);
    table.add_row(vec!["Strings written", &report.strings_written.to_string()]);
    table.add_row(vec!["Strings skipped", &report.strings_skipped.to_string()]);
    if let Some(ref path) = report.backup_path {
        table.add_row(vec!["Backup", path]);
    }
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

/// Byte length of a `haystack` prefix that case-folds to `find_folded`, or `None`.
///
/// Walks whole characters only. Compares `char::to_lowercase()` sequences so
/// length-changing folds (e.g. ẞ→ß) stay aligned with the original string.
fn case_insensitive_prefix_len(haystack: &str, find_folded: &[char]) -> Option<usize> {
    let mut fi = 0;
    let mut bytes = 0;
    for ch in haystack.chars() {
        let folded: Vec<char> = ch.to_lowercase().collect();
        if fi + folded.len() > find_folded.len() {
            return None;
        }
        if find_folded[fi..fi + folded.len()] != folded[..] {
            return None;
        }
        fi += folded.len();
        bytes += ch.len_utf8();
        if fi == find_folded.len() {
            return Some(bytes);
        }
    }
    None
}

/// Replace all occurrences of `find` in `text`. Returns (new_text, occurrence_count).
fn replace_in_translation(
    text: &str,
    find: &str,
    replace: &str,
    case_sensitive: bool,
) -> (String, usize) {
    if find.is_empty() {
        return (text.to_string(), 0);
    }
    if case_sensitive {
        let parts: Vec<&str> = text.split(find).collect();
        let n = parts.len().saturating_sub(1);
        if n == 0 {
            return (text.to_string(), 0);
        }
        return (parts.join(replace), n);
    }
    // Case-insensitive: walk original chars; match by case-folded sequences.
    let find_folded: Vec<char> = find.chars().flat_map(|c| c.to_lowercase()).collect();
    let mut out = String::with_capacity(text.len());
    let mut n = 0usize;
    let mut rest = text;
    while !rest.is_empty() {
        if let Some(matched_bytes) = case_insensitive_prefix_len(rest, &find_folded) {
            out.push_str(replace);
            rest = &rest[matched_bytes..];
            n += 1;
        } else {
            let mut chars = rest.chars();
            let ch = chars.next().expect("rest non-empty");
            out.push(ch);
            rest = chars.as_str();
        }
    }
    (out, n)
}

async fn cmd_replace(
    project: PathBuf,
    find: String,
    replace: String,
    case_sensitive: bool,
    dry_run: bool,
) -> anyhow::Result<()> {
    if find.is_empty() {
        anyhow::bail!("--find must not be empty");
    }
    let db = Database::open(&project)?;
    // SQLite LIKE is case-insensitive only for ASCII. With non-ASCII find in
    // case-insensitive mode, skip the SQL prefilter and match in Rust.
    const ENTRY_LIMIT: usize = 100_000;
    let use_sql_search = case_sensitive || find.is_ascii();
    let entries = db.get_entries(&EntryFilter {
        search: if use_sql_search {
            Some(find.clone())
        } else {
            None
        },
        limit: Some(ENTRY_LIMIT),
        ..Default::default()
    })?;
    if entries.len() == ENTRY_LIMIT {
        eprintln!(
            "warning: result set hit the {ENTRY_LIMIT}-entry cap; results may be truncated. \
             Use a narrower --find."
        );
    }

    let mut updates: Vec<(String, String)> = Vec::new();
    let mut occurrences = 0usize;
    for e in &entries {
        let Some(ref t) = e.translation else {
            continue;
        };
        let (next, n) = replace_in_translation(t, &find, &replace, case_sensitive);
        if n == 0 || next == *t {
            continue;
        }
        occurrences += n;
        updates.push((e.id.clone(), next));
    }

    if updates.is_empty() {
        println!("No matching translations for {find:?}.");
        return Ok(());
    }

    if dry_run {
        println!(
            "Dry run: would update {} string(s), {} occurrence(s).",
            updates.len(),
            occurrences
        );
        for (id, next) in updates.iter().take(10) {
            println!("  {id} → {}", truncate_preview(next, 80));
        }
        if updates.len() > 10 {
            println!("  … and {} more", updates.len() - 10);
        }
        return Ok(());
    }

    let applied = db.save_translations_batch(updates, "cli-replace").await?;
    println!(
        "Updated {applied} string(s), {occurrences} occurrence(s) in {}.",
        project.display()
    );
    Ok(())
}

fn truncate_preview(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max).collect();
    out.push('…');
    out
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
            if p.is_free {
                "Yes".to_string()
            } else {
                "No".to_string()
            },
            if p.requires_api_key {
                "Yes".to_string()
            } else {
                "No".to_string()
            },
        ]);
    }
    println!("{table}");

    Ok(())
}

fn cmd_formats() -> anyhow::Result<()> {
    let registry = locust_formats::default_registry();
    let formats = registry.list();

    let mut table = Table::new();
    table.set_header(vec!["ID", "Name", "Extensions", "Modes", "Stability"]);
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
            f.stability.label(),
        ]);
    }
    println!("{table}");

    Ok(())
}

fn cmd_register_lang(game_path: PathBuf, lang: String, label: String) -> anyhow::Result<()> {
    let report = locust_formats::rpgmaker_lang::register_language(&game_path, &lang, &label)?;
    println!(
        "register-lang {} → {} ({})",
        game_path.display(),
        lang,
        label
    );
    println!(
        "  plugins.js: {} (iavra={}, visumz={})",
        report.plugins_js, report.iavra_languages, report.visumz_options
    );
    println!("  maps patched: {}", report.maps_patched.len());
    for p in &report.maps_patched {
        println!("    {}", p.display());
    }
    println!("  backups: {}", report.backups.len());
    for n in &report.notes {
        println!("  note: {n}");
    }
    if !report.plugins_js && report.maps_patched.is_empty() {
        println!("  (idempotent / no further changes needed)");
    }
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

    // Prefer the source language of the latest translation run for this
    // target — config.default_source_lang is a user preference default, not
    // what was actually used for the strings in this project.
    let source_lang = db.resolve_export_source_lang(&lang, &config.default_source_lang)?;

    let content = match format.as_str() {
        "po" => export::export_po(&entries, &source_lang, &lang),
        "xliff" => export::export_xliff(&entries, &source_lang, &lang),
        _ => anyhow::bail!("unsupported export format: {}. Use 'po' or 'xliff'", format),
    };

    std::fs::write(&output, &content)?;
    println!(
        "Exported {} entries to {} ({}→{})",
        entries.len(),
        output.display(),
        source_lang,
        lang
    );
    Ok(())
}

async fn cmd_import(
    project: PathBuf,
    format: String,
    _lang: String,
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
                        if db.save_translation(id, &pe.translation, "import").await? {
                            imported += 1;
                        }
                    }
                }
            }
        }
        "xliff" => {
            let units = export::import_xliff(&content)?;
            for unit in &units {
                if !unit.target.is_empty()
                    && db
                        .save_translation(&unit.id, &unit.target, "import")
                        .await?
                {
                    imported += 1;
                }
            }
        }
        _ => anyhow::bail!("unsupported import format: {}. Use 'po' or 'xliff'", format),
    }

    println!(
        "Imported {} translations from {}",
        imported,
        input.display()
    );
    Ok(())
}

async fn cmd_server(port: u16) -> anyhow::Result<()> {
    let state = locust_server::create_app_state();
    println!(
        "Starting Project Locust server on http://localhost:{}",
        port
    );
    println!("Press Ctrl+C to stop");
    locust_server::start_server(state, port).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use locust_core::models::StringStatus;
    use std::fs;

    /// Process-global env var — serialize tests that set LOCUST_BACKUP_ROOT.
    static BACKUP_ROOT_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    #[test]
    fn download_patch_zip_rejects_non_http_schemes() {
        // A patch URL is attacker-supplied in the CDN case; only http(s) may
        // ever be fetched, and nothing is written when the scheme is refused.
        let dir = std::env::temp_dir().join(format!("locust_dl_{}", uuid::Uuid::new_v4()));
        let dest = dir.join("p.zip");
        for url in [
            "file:///C:/Windows/win.ini",
            "ftp://example.com/a.zip",
            "data:application/zip;base64,UEsDBA==",
        ] {
            let e = download_patch_zip(url, &dest)
                .expect_err("must refuse non-http scheme")
                .to_string();
            assert!(e.contains("http"), "{url} -> {e}");
            assert!(!dest.exists(), "{url} wrote a file");
        }
    }

    #[test]
    fn test_cli_parses() {
        // Verify the CLI struct parses without panicking
        use clap::CommandFactory;
        Cli::command().debug_assert();
    }

    #[test]
    fn cmd_pivot_skips_pending_and_refuses_overwrite() {
        let dir = std::env::temp_dir().join(format!("locust_cli_pivot_{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        let src_path = dir.join("src.locust.db");
        let src = Database::open(&src_path).unwrap();
        let mut done = StringEntry::new("a", "Hello", PathBuf::from("f.json"));
        done.translation = Some("Hola".into());
        done.status = StringStatus::Translated;
        src.save_entries(&[
            done,
            StringEntry::new("b", "World", PathBuf::from("f.json")),
        ])
        .unwrap();
        drop(src);

        let out = dir.join("out.locust.db");
        cmd_pivot(src_path.clone(), out.clone()).unwrap();
        let new_db = Database::open(&out).unwrap();
        let pivoted = new_db.get_entries(&EntryFilter::default()).unwrap();
        assert_eq!(pivoted.len(), 1);
        assert_eq!(pivoted[0].source, "Hola");
        drop(new_db);

        let src = Database::open(&src_path).unwrap();
        assert_eq!(src.get_entries(&EntryFilter::default()).unwrap().len(), 2);

        let err = cmd_pivot(src_path, out.clone()).expect_err("must refuse overwrite");
        assert!(
            err.to_string().to_lowercase().contains("exist")
                || err.to_string().to_lowercase().contains("overwrite"),
            "{err}"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn astro_stub_tells_users_to_use_locust_apply() {
        let dir = std::env::temp_dir().join(format!("locust_astro_{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        let stub = dir.join("release.md");
        let game = dir.join("MyGame");
        fs::create_dir_all(&game).unwrap();
        write_astro_stub(&stub, &game, Some("es")).unwrap();
        let md = fs::read_to_string(&stub).unwrap();
        let _ = fs::remove_dir_all(&dir);
        assert!(
            !md.contains("extract the patch over the game folder"),
            "unzip-over-game skips verify/backup/receipt:\n{md}"
        );
        assert!(
            md.contains("locust apply <game> <patch.zip>"),
            "must name locust apply:\n{md}"
        );
        assert!(md.contains("--url"), "must mention --url:\n{md}");
        assert!(
            md.contains("locust patch-rollback <game>"),
            "must name rollback:\n{md}"
        );
        assert!(
            md.to_lowercase().contains("backup"),
            "must mention restorable backup:\n{md}"
        );
    }

    #[test]
    fn replace_in_translation_case_sensitive_basic() {
        let (out, n) = replace_in_translation("foo bar foo", "foo", "X", true);
        assert_eq!(out, "X bar X");
        assert_eq!(n, 2);
    }

    #[test]
    fn replace_in_translation_case_sensitive_no_match() {
        let (out, n) = replace_in_translation("Foo bar", "foo", "X", true);
        assert_eq!(out, "Foo bar");
        assert_eq!(n, 0);
    }

    #[test]
    fn replace_in_translation_case_sensitive_multiple() {
        let (out, n) = replace_in_translation("aaa", "a", "b", true);
        assert_eq!(out, "bbb");
        assert_eq!(n, 3);
    }

    #[test]
    fn replace_in_translation_case_insensitive_ascii_mixed() {
        let (out, n) = replace_in_translation("Foo FOO foo", "foo", "x", false);
        assert_eq!(out, "x x x");
        assert_eq!(n, 3);
    }

    #[test]
    fn replace_in_translation_case_insensitive_unicode_length_change() {
        // ẞ lowercases to ß (different UTF-8 byte length) — must not panic.
        let (out, n) = replace_in_translation("STRAẞE fine", "straße", "road", false);
        assert_eq!(out, "road fine");
        assert_eq!(n, 1);
    }

    #[test]
    fn replace_in_translation_case_insensitive_non_ascii_accent() {
        let (out, n) = replace_in_translation("Árbol y Á", "á", "a", false);
        assert_eq!(out, "arbol y a");
        assert_eq!(n, 2);
    }

    #[test]
    fn replace_in_translation_empty_find() {
        let (out, n) = replace_in_translation("hello", "", "x", false);
        assert_eq!(out, "hello");
        assert_eq!(n, 0);
    }

    #[test]
    fn replace_in_translation_shorter_and_longer_replacement() {
        let (short, n1) = replace_in_translation("xxfoo yyfoo", "foo", "z", true);
        assert_eq!(short, "xxz yyz");
        assert_eq!(n1, 2);
        let (long, n2) = replace_in_translation("a-b", "b", "BBB", true);
        assert_eq!(long, "a-BBB");
        assert_eq!(n2, 1);
    }

    // ─── cmd_patch packaging tests: `patch` packs EXCLUSIVELY from the
    // recording injection persisted (root + rel + hash per language key);
    // every mismatch is a loud error naming a remedy that works ─────────────

    use locust_core::database::{paths_identical, sha256_hex};
    use locust_core::models::StringEntry;

    fn patch_test_tempdir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("locust_patch_test_{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// One translated entry, in the exact shape `locust extract` +
    /// `locust translate`/`locust import` leave in the database.
    fn save_translated(db: &Database, id: &str, source: &str, path: &std::path::Path, t: &str) {
        let mut entry = StringEntry::new(id, source, path.to_path_buf());
        entry.translation = Some(t.to_string());
        entry.status = StringStatus::Translated;
        db.save_entries(&[entry]).unwrap();
    }

    /// A Ren'Py-shaped game tree with one loose script (a state extraction
    /// genuinely ingests: loose `.rpy` files become entries with their own
    /// path), plus a project database holding its translated entry.
    fn make_renpy_game(base: &std::path::Path) -> (PathBuf, PathBuf, PathBuf, String) {
        let game_dir = base.join("mygame");
        let game_sub = game_dir.join("game");
        fs::create_dir_all(&game_sub).unwrap();
        let script = game_sub.join("script.rpy");
        let contents = "label start:\n    \"Hola\"\n".to_string();
        fs::write(&script, &contents).unwrap();

        let db_path = base.join("project.locust.db");
        let db = Database::open(&db_path).unwrap();
        save_translated(&db, "script.rpy#2", "Hello", &script, "Hola");
        drop(db);
        (game_dir, script, db_path, contents)
    }

    #[test]
    fn test_patch_packs_the_recorded_files_and_bytes() {
        // A recording exists for one key ("es", from a direct inject on the
        // game root); `patch` without -l resolves the single key and packs
        // exactly the recorded rels, byte-for-byte.
        let base = patch_test_tempdir();
        let (game_dir, script, db_path, contents) = make_renpy_game(&base);
        let db = Database::open(&db_path).unwrap();
        db.record_injection(Some("es"), &game_dir, &[script])
            .unwrap();
        drop(db);

        let out_zip = base.join("out-patch.zip");
        cmd_patch(game_dir, db_path, None, Some(out_zip.clone()), None, None).unwrap();

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

    #[cfg(windows)]
    #[test]
    fn test_patch_resolves_when_game_path_case_differs_from_record_time() {
        // The recorded root and the patch-time game path are the same on-disk
        // directory spelled with different case. The identity check must fold
        // case where the filesystem does, not refuse a spelling difference.
        let base = patch_test_tempdir();
        let (game_dir, script, db_path, _) = make_renpy_game(&base);
        let db = Database::open(&db_path).unwrap();
        db.record_injection(Some("es"), &game_dir, &[script])
            .unwrap();
        drop(db);

        let respelled = base.join("MYGAME");
        let out_zip = base.join("out-patch.zip");
        cmd_patch(respelled, db_path, None, Some(out_zip.clone()), None, None)
            .expect("a case-divergent spelling of the recorded root must still pack");

        let file = fs::File::open(&out_zip).unwrap();
        let mut archive = zip::ZipArchive::new(file).unwrap();
        assert!(archive.by_name("game/script.rpy").is_ok());
    }

    #[test]
    fn test_patch_all_rpa_database_packs_recorded_injection_output() {
        // Archive-shipped Ren'Py game: EVERY database entry names the .rpa it
        // was READ from, while injection wrote loose .rpy files that
        // extraction deliberately never re-ingests (it skips `zzz_locust*`).
        // The patch must come from what injection RECORDED, not from entries.
        let base = patch_test_tempdir();
        let game_dir = base.join("renpygame");
        let game_sub = game_dir.join("game");
        fs::create_dir_all(&game_sub).unwrap();
        let rpa_path = game_sub.join("scripts.rpa");
        fs::write(&rpa_path, b"RPA-3.0 fake original archive bytes").unwrap();
        // What injection actually wrote for the archive content.
        let loose = game_sub.join("script.rpy");
        fs::write(&loose, "label start:\n    \"Hola\"\n").unwrap();
        let filter = game_sub.join("zzz_locust_translate.rpy");
        fs::write(&filter, "# runtime filter\n").unwrap();

        let db_path = base.join("project.locust.db");
        let db = Database::open(&db_path).unwrap();
        save_translated(&db, "scripts.rpa#script.rpy#2", "Hello", &rpa_path, "Hola");
        db.record_injection(Some("es"), &game_dir, &[loose.clone(), filter.clone()])
            .unwrap();
        drop(db);

        let out_zip = base.join("out-patch.zip");
        cmd_patch(game_dir, db_path, None, Some(out_zip.clone()), None, None)
            .expect("an all-archive database with recorded injection output must pack");

        let file = fs::File::open(&out_zip).unwrap();
        let mut archive = zip::ZipArchive::new(file).unwrap();
        let names: Vec<String> = (0..archive.len())
            .map(|i| archive.by_index(i).unwrap().name().to_string())
            .collect();
        assert!(
            names.iter().any(|n| n == "game/script.rpy"),
            "the loose .rpy injection wrote must be packed, got: {names:?}"
        );
        assert!(
            names.iter().any(|n| n == "game/zzz_locust_translate.rpy"),
            "the generated filter file must be packed, got: {names:?}"
        );
        assert!(
            !names.iter().any(|n| n.ends_with(".rpa")),
            "the source archive was never recorded as written, so it must not pack: {names:?}"
        );
    }

    #[test]
    fn test_patch_without_recording_names_the_exact_inject_command() {
        // A legacy database (or one whose old-format recording was dropped by
        // the migration): entries are translated, but nothing was ever
        // recorded. There is deliberately NO entry-derived fallback — the
        // fallback was the mechanism of every silently-wrong patch — so this
        // must hard-error naming the exact command that records an injection.
        let base = patch_test_tempdir();
        let (game_dir, _script, db_path, _) = make_renpy_game(&base);

        let out_zip = base.join("out-patch.zip");
        let err = cmd_patch(
            game_dir.clone(),
            db_path.clone(),
            Some("es".to_string()),
            Some(out_zip.clone()),
            None,
            None,
        )
        .expect_err("no recording must be a hard error, never an entry-derived guess");
        let msg = err.to_string();
        assert!(
            msg.contains("no injection has been recorded"),
            "error must say what is missing: {msg}"
        );
        let advised = format!(
            "locust inject \"{}\" -P \"{}\" --direct -l es",
            game_dir.display(),
            db_path.display()
        );
        assert!(
            msg.contains(&advised),
            "error must name the exact inject command\nwant: {advised}\ngot: {msg}"
        );
        assert!(!out_zip.exists(), "no archive may be written on this path");
    }

    #[test]
    fn test_patch_key_miss_is_a_loud_error_listing_recorded_keys() {
        // A recording exists — for a DIFFERENT language. The old code fell
        // back to the entry-derived list silently; now the mismatch is loud,
        // lists what IS recorded, and offers only remedies that work from
        // this state ( `-l fr` is offered because fr's root IS this tree).
        let base = patch_test_tempdir();
        let (game_dir, script, db_path, _) = make_renpy_game(&base);
        let db = Database::open(&db_path).unwrap();
        db.record_injection(Some("fr"), &game_dir, &[script])
            .unwrap();
        drop(db);

        let out_zip = base.join("out-patch.zip");
        let err = cmd_patch(
            game_dir.clone(),
            db_path.clone(),
            Some("es".to_string()),
            Some(out_zip),
            None,
            None,
        )
        .expect_err("a language with no recording must be a hard error");
        let msg = err.to_string();
        assert!(
            msg.contains("no injection recorded for language \"es\""),
            "error must name the missing key: {msg}"
        );
        assert!(
            msg.contains("\"fr\""),
            "error must list the recorded keys: {msg}"
        );
        assert!(
            msg.contains("--direct -l es"),
            "error must advise recording the requested language: {msg}"
        );
        assert!(
            msg.contains("-l fr"),
            "error must offer the recorded key whose root is this tree: {msg}"
        );
    }

    #[test]
    fn test_patch_null_key_is_packed_without_lang_and_never_by_a_named_lang() {
        // `--direct` without -l records under the reserved language-unspecified
        // key: `patch` without -l packs it, `patch -l es` must NOT silently
        // match it.
        let base = patch_test_tempdir();
        let (game_dir, script, db_path, _) = make_renpy_game(&base);
        let db = Database::open(&db_path).unwrap();
        db.record_injection(None, &game_dir, &[script]).unwrap();
        drop(db);

        let out_zip = base.join("out-patch.zip");
        cmd_patch(
            game_dir.clone(),
            db_path.clone(),
            None,
            Some(out_zip.clone()),
            None,
            None,
        )
        .expect("the language-unspecified recording must pack without -l");
        assert!(out_zip.exists());

        let err = cmd_patch(
            game_dir,
            db_path,
            Some("es".to_string()),
            Some(base.join("other.zip")),
            None,
            None,
        )
        .expect_err("a named language must never silently match the unspecified key");
        let msg = err.to_string();
        assert!(
            msg.contains("(unspecified)"),
            "error must list the unspecified key so the state is visible: {msg}"
        );
        assert!(
            msg.contains("without -l"),
            "the working remedy here is re-running patch without -l: {msg}"
        );
    }

    #[test]
    fn test_patch_without_lang_and_multiple_keys_requires_an_explicit_choice() {
        // Key set {es, (unspecified)} with `patch` invoked without -l is
        // claimed by two rules: "NULL key is matched by patch without -l" and
        // "multiple keys without -l is an error". The error wins — packing a
        // silent guess (or the union) is how mixed-language zips shipped.
        let base = patch_test_tempdir();
        let (game_dir, script, db_path, _) = make_renpy_game(&base);
        let db = Database::open(&db_path).unwrap();
        db.record_injection(Some("es"), &game_dir, std::slice::from_ref(&script))
            .unwrap();
        db.record_injection(None, &game_dir, &[script]).unwrap();
        drop(db);

        let out_zip = base.join("out-patch.zip");
        let err = cmd_patch(
            game_dir.clone(),
            db_path.clone(),
            None,
            Some(out_zip.clone()),
            None,
            None,
        )
        .expect_err("multiple keys without -l must be a hard error, not a guess");
        let msg = err.to_string();
        assert!(msg.contains("es"), "error must list the named key: {msg}");
        assert!(
            msg.contains("(unspecified)"),
            "error must list the unspecified key too: {msg}"
        );
        assert!(
            msg.contains("-l"),
            "error must require an explicit -l: {msg}"
        );
        // "Pass -l <lang>" alone cannot reach the (unspecified) recording —
        // no -l value names the NULL key. The error must say how to reach it:
        // re-record it under a named key.
        assert!(
            msg.contains("re-record") && msg.contains("--direct -l <lang>"),
            "the unspecified recording must be reachable, not a dead entry \
             in a list: {msg}"
        );
        assert!(!out_zip.exists());

        // The advised choice unblocks.
        cmd_patch(
            game_dir,
            db_path,
            Some("es".to_string()),
            Some(out_zip.clone()),
            None,
            None,
        )
        .expect("choosing a key with -l must unblock");
        assert!(out_zip.exists());
    }

    /// A Wolf-RPG-shaped game (engine class E5, an entry-tree writer whose
    /// originals are byte-patched in place) with one translated entry, for
    /// exercising the mutated-originals advice on `patch`'s error paths.
    fn make_wolf_game(base: &std::path::Path) -> (PathBuf, PathBuf, PathBuf) {
        let game_dir = base.join("wolfgame");
        let data = game_dir.join("Data");
        fs::create_dir_all(&data).unwrap();
        let wolf_file = data.join("BasicData.wolf");
        fs::write(&wolf_file, locust_formats::wolf_rpg::build_test_fixture()).unwrap();

        let db_path = base.join("project.locust.db");
        let db = Database::open(&db_path).unwrap();
        save_translated(&db, "BasicData.wolf#0", "テストデータ", &wolf_file, "test");
        drop(db);
        (game_dir, wolf_file, db_path)
    }

    #[test]
    fn test_replace_containment_remedy_says_restore_first_for_every_mutating_engine() {
        // Both engine classes that mutate the ORIGINAL tree in Replace mode
        // must lead with the restore step: a bare re-run against the mutated
        // tree finds no original source text, writes nothing (or, for a mixed
        // Ren'Py game, only the archive-derived files), and records a
        // recording that silently omits the loose translations.
        let game = PathBuf::from("game");
        let project = PathBuf::from("proj.db");

        // Ren'Py: loose scripts were already rewritten in place when the
        // containment error fires (renpy.rs writes to entry.file_path).
        let renpy = replace_containment_remedy("renpy", &game, &project, "es");
        assert!(
            renpy.contains("restore"),
            "the Ren'Py remedy must lead with restoring the original: {renpy}"
        );
        assert!(renpy.contains("-m add"), "{renpy}");
        assert!(renpy.contains("--direct -l es"), "{renpy}");

        // Entry-tree writers already carried the restore step; keep it.
        let wolf = replace_containment_remedy("wolf-rpg", &game, &project, "es");
        assert!(wolf.contains("restore"), "{wolf}");
        assert!(wolf.contains("--direct -l es"), "{wolf}");

        // Path-derived engines never mutate the original: no restore step.
        let html = replace_containment_remedy("html-game", &game, &project, "es");
        assert!(
            !html.contains("restore"),
            "a non-mutating engine must not be told to restore anything: {html}"
        );
    }

    #[test]
    fn test_patch_no_recording_advice_warns_when_the_originals_may_be_mutated() {
        // A legacy database on an already-injected entry-tree game: `patch`
        // advises a bare `--direct` re-run, but for engines that mutate the
        // originals that re-run writes 0 files and records nothing — the
        // identical error forever. The advice must carry the restore-first
        // note. `patch` cannot know whether a prior inject ran, so the note
        // is conditional where the containment remedies are imperative.
        let base = patch_test_tempdir();
        let (game_dir, _wolf_file, db_path) = make_wolf_game(&base);

        let err = cmd_patch(
            game_dir.clone(),
            db_path.clone(),
            Some("es".to_string()),
            None,
            None,
            None,
        )
        .expect_err("no recording must be a hard error");
        let msg = err.to_string();
        assert!(msg.contains("no injection has been recorded"), "{msg}");
        assert!(
            msg.contains("0 files written") && msg.contains("restore the original"),
            "the advice must explain the already-injected dead end and its way \
             out for an engine that mutates originals: {msg}"
        );

        // Triangulate: a path-derived engine's advice stays unconditional.
        let base2 = patch_test_tempdir();
        let (game2, _script, db2, _) = make_renpy_game(&base2);
        // Ren'Py mutates loose scripts, so it gets the note too; an HTML game
        // (pure path-derived writes) must not.
        let err2 = cmd_patch(game2, db2, Some("es".to_string()), None, None, None).unwrap_err();
        assert!(err2.to_string().contains("restore the original"), "{err2}");

        let base3 = patch_test_tempdir();
        let game3 = base3.join("htmlgame");
        fs::create_dir_all(&game3).unwrap();
        fs::write(
            game3.join("index.html"),
            "<html><body><p>Hi there world</p></body></html>",
        )
        .unwrap();
        let db3 = base3.join("project.locust.db");
        let dbh = Database::open(&db3).unwrap();
        save_translated(
            &dbh,
            "index.html#0",
            "Hi there world",
            &game3.join("index.html"),
            "Hola",
        );
        drop(dbh);
        let err3 = cmd_patch(game3, db3, Some("es".to_string()), None, None, None).unwrap_err();
        let msg3 = err3.to_string();
        assert!(msg3.contains("no injection has been recorded"), "{msg3}");
        assert!(
            !msg3.contains("restore the original"),
            "a non-mutating engine's advice must stay unconditional: {msg3}"
        );
    }

    #[test]
    fn test_patch_key_miss_advice_warns_when_the_originals_may_be_mutated() {
        // A recording exists for another language on an entry-tree game: its
        // originals were provably byte-patched by that inject, so the advised
        // `--direct -l <missing>` re-run needs the same restore-first note.
        let base = patch_test_tempdir();
        let (game_dir, wolf_file, db_path) = make_wolf_game(&base);
        let db = Database::open(&db_path).unwrap();
        db.record_injection(Some("fr"), &game_dir, &[wolf_file])
            .unwrap();
        drop(db);

        let err = cmd_patch(game_dir, db_path, Some("es".to_string()), None, None, None)
            .expect_err("a key miss must be a hard error");
        let msg = err.to_string();
        assert!(
            msg.contains("no injection recorded for language \"es\""),
            "{msg}"
        );
        assert!(
            msg.contains("0 files written") && msg.contains("restore the original"),
            "the advised --direct re-run dead-ends on the already-patched \
             originals without the restore note: {msg}"
        );
    }

    #[test]
    fn test_patch_refuses_recorded_rels_that_are_not_plain_relative_paths() {
        // Record time cannot emit an absolute rel, but `patch` trusts DB TEXT
        // it did not derive (a shared .locust.db is a plausible input). An
        // absolute rel survives a ParentDir-only guard, and
        // `recording.root.join(abs)` REPLACES the root — the patch would read
        // a file outside the game tree and ship it, hash-verified from the
        // same row.
        let base = patch_test_tempdir();
        let (game_dir, script, db_path, _) = make_renpy_game(&base);
        let db = Database::open(&db_path).unwrap();
        db.record_injection(Some("es"), &game_dir, &[script])
            .unwrap();
        drop(db);

        let secret = base.join("secret.txt");
        let payload: &[u8] = b"outside the game tree";
        fs::write(&secret, payload).unwrap();
        let abs_rel = secret.display().to_string().replace('\\', "/");
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        conn.execute(
            "UPDATE injected_files SET rel = ?1, hash = ?2, size = ?3",
            rusqlite::params![abs_rel, sha256_hex(payload), payload.len() as i64],
        )
        .unwrap();
        drop(conn);

        let out_zip = base.join("out-patch.zip");
        let err = cmd_patch(
            game_dir.clone(),
            db_path.clone(),
            None,
            Some(out_zip.clone()),
            None,
            None,
        )
        .expect_err("an absolute recorded rel must be refused, never packed");
        assert!(
            err.to_string().contains("escapes the game root"),
            "the refusal must name the escape: {err}"
        );
        assert!(!out_zip.exists(), "no archive may ship an out-of-tree file");

        // Triangulate: a `..` rel is refused by the same guard.
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        conn.execute("UPDATE injected_files SET rel = '../evil.txt'", [])
            .unwrap();
        drop(conn);
        let err = cmd_patch(game_dir, db_path, None, Some(out_zip.clone()), None, None)
            .expect_err("a parent-dir rel must be refused");
        assert!(err.to_string().contains("escapes the game root"), "{err}");
        assert!(!out_zip.exists());
    }

    #[test]
    fn test_patch_counts_reviewed_and_approved_strings_as_translated() {
        // The desktop Review page sets `approved` (Review.tsx); `patch`'s
        // pre-check must not send a fully reviewed/approved project back to
        // `locust translate` — that advice would re-translate finished work.
        for status in [StringStatus::Approved, StringStatus::Reviewed] {
            let base = patch_test_tempdir();
            let game_dir = base.join("mygame");
            let game_sub = game_dir.join("game");
            fs::create_dir_all(&game_sub).unwrap();
            let script = game_sub.join("script.rpy");
            fs::write(&script, "label start:\n    \"Hola\"\n").unwrap();
            let db_path = base.join("project.locust.db");
            let db = Database::open(&db_path).unwrap();
            let mut entry = StringEntry::new("script.rpy#2", "Hello", script.clone());
            entry.translation = Some("Hola".to_string());
            entry.status = status.clone();
            db.save_entries(&[entry]).unwrap();
            drop(db);

            let err = cmd_patch(game_dir, db_path, Some("es".to_string()), None, None, None)
                .expect_err("no recording exists, so patch still errors — but LATER");
            let msg = err.to_string();
            assert!(
                msg.contains("no injection has been recorded"),
                "a {status:?} project must pass the translated-strings gate: {msg}"
            );
            assert!(
                !msg.contains("locust translate"),
                "re-translating finished work must never be the advice: {msg}"
            );
        }

        // Triangulate: a project with nothing translated still gets the gate.
        let base = patch_test_tempdir();
        let game_dir = base.join("mygame");
        fs::create_dir_all(game_dir.join("game")).unwrap();
        let script = game_dir.join("game").join("script.rpy");
        fs::write(&script, "label start:\n    \"Hello\"\n").unwrap();
        let db_path = base.join("project.locust.db");
        let db = Database::open(&db_path).unwrap();
        db.save_entries(&[StringEntry::new("script.rpy#2", "Hello", script)])
            .unwrap();
        drop(db);
        let err = cmd_patch(game_dir, db_path, None, None, None, None).unwrap_err();
        assert!(
            err.to_string().contains("nothing to pack yet"),
            "an untranslated project must still be told to translate: {err}"
        );
    }

    #[test]
    fn test_patch_refuses_the_wrong_tree_and_names_the_recorded_root() {
        // R6: Replace-mode injection recorded under the per-language COPY;
        // the user points patch at the ORIGINAL. The old content-root anchor
        // silently re-read the rels out of the original — an untranslated zip
        // claiming "translated text only". Now: hard error naming the root.
        let base = patch_test_tempdir();
        let (game_dir, _script, db_path, _) = make_renpy_game(&base);
        // The Replace copy, injected and recorded.
        let copy_dir = base.join("out").join("mygame-es");
        let copy_sub = copy_dir.join("game");
        fs::create_dir_all(&copy_sub).unwrap();
        let copy_script = copy_sub.join("script.rpy");
        fs::write(&copy_script, "label start:\n    \"Hola\"\n").unwrap();
        let db = Database::open(&db_path).unwrap();
        db.record_injection(Some("es"), &copy_dir, &[copy_script])
            .unwrap();
        drop(db);

        let out_zip = base.join("out-patch.zip");
        let err = cmd_patch(
            game_dir,
            db_path.clone(),
            Some("es".to_string()),
            Some(out_zip.clone()),
            None,
            None,
        )
        .expect_err("packing recorded rels out of a different tree must be refused");
        let msg = err.to_string();
        assert!(
            msg.contains(&copy_dir.display().to_string()),
            "error must name the recorded root to point patch at: {msg}"
        );
        assert!(!out_zip.exists());

        // The advised command — patch pointed at the recorded root — unblocks.
        cmd_patch(
            copy_dir,
            db_path,
            Some("es".to_string()),
            Some(out_zip.clone()),
            None,
            None,
        )
        .expect("patch pointed at the recorded root must pack");
        assert!(out_zip.exists());
    }

    #[test]
    fn test_patch_refuses_a_recorded_file_that_changed_since_injection() {
        // The recording carries the hash of what injection wrote; a file that
        // no longer matches must fail loudly instead of shipping bytes nobody
        // verified (F8: nothing ever checked a packed file was the file
        // injection reported writing).
        let base = patch_test_tempdir();
        let (game_dir, script, db_path, _) = make_renpy_game(&base);
        let db = Database::open(&db_path).unwrap();
        db.record_injection(Some("es"), &game_dir, std::slice::from_ref(&script))
            .unwrap();
        drop(db);

        fs::write(&script, "label start:\n    \"Overwritten\"\n").unwrap();

        let out_zip = base.join("out-patch.zip");
        let err = cmd_patch(
            game_dir,
            db_path,
            Some("es".to_string()),
            Some(out_zip.clone()),
            None,
            None,
        )
        .expect_err("a changed file must refuse to pack");
        let msg = err.to_string();
        assert!(
            msg.contains("changed on disk since injection"),
            "error must say what happened: {msg}"
        );
        assert!(
            msg.contains("game/script.rpy"),
            "error must name the changed rel: {msg}"
        );
        assert!(
            msg.contains("--direct -l es"),
            "error must advise re-injecting to refresh the recording: {msg}"
        );
        assert!(!out_zip.exists(), "no archive may be written on this path");
    }

    #[test]
    fn test_patch_errors_when_a_recorded_file_is_missing() {
        // A recording names every file the patch promised to carry. A missing
        // one must be a hard error naming the path — the old flow listed it
        // under "Files not found on disk" and shipped the zip anyway.
        let base = patch_test_tempdir();
        let (game_dir, script, db_path, _) = make_renpy_game(&base);
        let db = Database::open(&db_path).unwrap();
        db.record_injection(Some("es"), &game_dir, std::slice::from_ref(&script))
            .unwrap();
        drop(db);

        fs::remove_file(&script).unwrap();

        let out_zip = base.join("out-patch.zip");
        let err = cmd_patch(
            game_dir,
            db_path,
            Some("es".to_string()),
            Some(out_zip.clone()),
            None,
            None,
        )
        .expect_err("a missing recorded file must be an error, not a silent skip");
        let msg = err.to_string();
        assert!(msg.contains("missing from disk"), "{msg}");
        assert!(
            msg.contains(&script.display().to_string()),
            "error must name the missing path: {msg}"
        );
        assert!(
            !out_zip.exists(),
            "no archive may be left behind when the recording cannot be honored"
        );
    }

    #[tokio::test]
    async fn test_inject_direct_records_rel_root_and_hash_per_language() {
        // The bridge that makes the recorded-injection patch path reachable:
        // `locust inject --direct` must persist root + rel + hash for EVERY
        // requested language (recording only the first orphaned `patch -l
        // <other>` — today's F1 bug).
        let base = patch_test_tempdir();
        let bak = base.join("bak");
        let game_dir = base.join("renpygame");
        let game_sub = game_dir.join("game");
        fs::create_dir_all(&game_sub).unwrap();
        let script = game_sub.join("script.rpy");
        fs::write(&script, "label start:\n    \"Hello, world!\"\n").unwrap();

        let db_path = base.join("project.locust.db");
        let db = Database::open(&db_path).unwrap();
        save_translated(
            &db,
            "script.rpy#2",
            "Hello, world!",
            &script,
            "Hola, mundo!",
        );
        drop(db);

        {
            let _guard = BACKUP_ROOT_LOCK.lock().await;
            std::env::set_var("LOCUST_BACKUP_ROOT", &bak);
            cmd_inject_direct(
                game_dir.clone(),
                db_path.clone(),
                vec!["es".to_string(), "fr".to_string()],
            )
            .await
            .unwrap();
            std::env::remove_var("LOCUST_BACKUP_ROOT");
        }

        let db = Database::open(&db_path).unwrap();
        for lang in ["es", "fr"] {
            let rec = db
                .get_injection(Some(lang))
                .unwrap()
                .unwrap_or_else(|| panic!("a recording must exist for {lang}"));
            assert!(
                paths_identical(&rec.root, &game_dir),
                "{lang}: the recorded root must be the injected game dir"
            );
            assert_eq!(rec.files.len(), 1, "{lang}: exactly the written file");
            assert_eq!(rec.files[0].rel, "game/script.rpy");
            let on_disk = fs::read(&script).unwrap();
            assert_eq!(
                rec.files[0].hash,
                sha256_hex(&on_disk),
                "{lang}: the recorded hash must be the hash of the written bytes"
            );
        }
    }

    #[tokio::test]
    async fn test_inject_direct_backs_up_before_mutating_renpy_tree() {
        // Task #14: --direct is the universally recommended recovery path, but
        // for engines that write the original tree it used to take no backup.
        let base = patch_test_tempdir();
        let bak = base.join("bak");
        let game_dir = base.join("renpygame");
        let game_sub = game_dir.join("game");
        fs::create_dir_all(&game_sub).unwrap();
        let script = game_sub.join("script.rpy");
        fs::write(&script, "label start:\n    \"Hello, world!\"\n").unwrap();
        let original = fs::read(&script).unwrap();

        let db_path = base.join("project.locust.db");
        let db = Database::open(&db_path).unwrap();
        save_translated(
            &db,
            "script.rpy#2",
            "Hello, world!",
            &script,
            "Hola, mundo!",
        );
        drop(db);

        {
            let _guard = BACKUP_ROOT_LOCK.lock().await;
            std::env::set_var("LOCUST_BACKUP_ROOT", &bak);
            cmd_inject_direct(game_dir.clone(), db_path, vec!["es".to_string()])
                .await
                .unwrap();
            std::env::remove_var("LOCUST_BACKUP_ROOT");
        }

        // Injection mutated the loose script.
        assert_ne!(fs::read(&script).unwrap(), original);

        // Isolated backup root holds the pre-inject bytes.
        let bak_dirs: Vec<_> = fs::read_dir(&bak)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.is_dir())
            .collect();
        assert_eq!(
            bak_dirs.len(),
            1,
            "direct inject on a mutating engine must create exactly one backup under {}",
            bak.display()
        );
        let backed = bak_dirs[0].join("game").join("script.rpy");
        assert!(
            backed.is_file(),
            "backup must contain the original game/script.rpy"
        );
        assert_eq!(
            fs::read(&backed).unwrap(),
            original,
            "backup must hold pre-inject bytes"
        );
    }

    #[tokio::test]
    async fn test_inject_direct_without_lang_records_the_unspecified_key() {
        let base = patch_test_tempdir();
        let bak = base.join("bak");
        let game_dir = base.join("renpygame");
        let game_sub = game_dir.join("game");
        fs::create_dir_all(&game_sub).unwrap();
        let script = game_sub.join("script.rpy");
        fs::write(&script, "label start:\n    \"Hello, world!\"\n").unwrap();

        let db_path = base.join("project.locust.db");
        let db = Database::open(&db_path).unwrap();
        save_translated(
            &db,
            "script.rpy#2",
            "Hello, world!",
            &script,
            "Hola, mundo!",
        );
        drop(db);

        {
            let _guard = BACKUP_ROOT_LOCK.lock().await;
            std::env::set_var("LOCUST_BACKUP_ROOT", &bak);
            cmd_inject_direct(game_dir, db_path.clone(), Vec::new())
                .await
                .unwrap();
            std::env::remove_var("LOCUST_BACKUP_ROOT");
        }

        let db = Database::open(&db_path).unwrap();
        assert!(
            db.get_injection(None).unwrap().is_some(),
            "no -l must record under the reserved language-unspecified key"
        );
        assert!(
            db.get_injection(Some("es")).unwrap().is_none(),
            "no named key may be invented"
        );
    }

    #[test]
    fn test_patch_failed_run_leaves_existing_output_untouched() {
        // A previously published patch sits at `-o`. A re-run that fails the
        // recording verification (file missing since) must not truncate or
        // delete it: the archive is built in a temp file and renamed over the
        // destination only on success; the drop guard removes the temp.
        let base = patch_test_tempdir();
        let (game_dir, script, db_path, _) = make_renpy_game(&base);
        let db = Database::open(&db_path).unwrap();
        db.record_injection(Some("es"), &game_dir, std::slice::from_ref(&script))
            .unwrap();
        drop(db);
        fs::remove_file(&script).unwrap();

        let out_zip = base.join("published-patch.zip");
        let published = b"previously published good patch bytes";
        fs::write(&out_zip, published).unwrap();

        let err = cmd_patch(
            game_dir,
            db_path,
            Some("es".to_string()),
            Some(out_zip.clone()),
            None,
            None,
        )
        .expect_err("a recording that cannot be honored must remain an error");
        assert!(err.to_string().contains("missing from disk"), "{err}");
        assert_eq!(
            fs::read(&out_zip).unwrap(),
            published,
            "the previously published patch must survive a failed run byte-for-byte"
        );
        // No temp litter either.
        let leftovers: Vec<_> = fs::read_dir(&base)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .filter(|n| n.contains(".tmp"))
            .collect();
        assert!(
            leftovers.is_empty(),
            "temp files left behind: {leftovers:?}"
        );
    }
}
