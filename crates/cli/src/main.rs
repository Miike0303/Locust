use std::path::PathBuf;

use clap::{Parser, Subcommand};

mod commands;
mod injection_recovery;
#[cfg(test)]
mod test_support;
mod util;

use commands::{auth_info, extract, inject, io, patch, project, server, translate, validate};

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
        /// Preview only — do not write the database
        #[arg(long)]
        dry_run: bool,
        #[arg(
            long,
            help = "Merge into a project database that records a different game"
        )]
        force: bool,
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
        /// Approximate input-token budget per batch; long entries run alone
        #[arg(long)]
        max_batch_tokens: Option<usize>,
        /// Bypass translation memory (useful for provider benchmarks)
        #[arg(long)]
        no_memory: bool,
        /// Allow mechanical truncation/accent folding of oversized binary text
        #[arg(long)]
        allow_lossy_binary_fit: bool,
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
        #[arg(short, long, value_parser = ["replace", "add"])]
        mode: Option<String>,
        /// One target language for this project DB. Use a separate pivot/DB per
        /// target language. Required for Replace/Add; optional with --direct
        /// (the recording is then language-unspecified).
        #[arg(short, long, num_args = 1..)]
        languages: Vec<String>,
        #[arg(short, long)]
        output_dir: Option<PathBuf>,
        /// Install translations in place using a recoverable private work copy
        #[arg(long)]
        direct: bool,
        /// Emit the complete core injection report as one JSON object on stdout
        #[arg(long)]
        json: bool,
    },
    /// Validate translations
    Validate {
        project: PathBuf,
        /// Emit machine-readable issues on stdout
        #[arg(long)]
        json: bool,
    },
    /// Audit TTF/OTF/TTC glyph coverage (WOFF and parse failures are reported)
    FontCheck {
        path: PathBuf,
        #[arg(long)]
        text_file: PathBuf,
    },
    /// Package a user-supplied font for one existing loose-font asset
    FontPatch {
        game_path: PathBuf,
        #[arg(long)]
        font: PathBuf,
        /// Existing game-relative TTF/OTF path already referenced by the game
        #[arg(long)]
        target: String,
        #[arg(long, value_parser = ["rpgmaker-mv", "rpgmaker-mz", "rpgmaker_mv", "rpgmaker_mz", "renpy", "html"])]
        engine: String,
        #[arg(long)]
        lang: String,
        /// UTF-8 file containing actual target-language text
        #[arg(long)]
        text_file: PathBuf,
        /// Combine with a translation zip that verifies against this clean game
        #[arg(long)]
        base_patch: Option<PathBuf>,
        /// A new output zip; existing files are never overwritten
        #[arg(short, long)]
        output: PathBuf,
    },
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
    /// Show translation stats: tokens, time, and cost per run, or progress by file
    Stats {
        project: PathBuf,
        /// Show string counts by status for each file
        #[arg(long)]
        by_file: bool,
    },
    /// Create a standalone checkpoint that opens like any project database
    BackupProject {
        project: PathBuf,
        /// New checkpoint path; existing files are never overwritten
        #[arg(short, long)]
        output: PathBuf,
    },
    /// Pivot: seed a new project whose SOURCE is another project's translations,
    /// so you can translate e.g. JA→EN once, then EN→ES / EN→FR / EN→PT from it.
    Pivot {
        /// Existing project whose translations become the new source (e.g. the EN one)
        source: PathBuf,
        #[arg(short, long)]
        output: PathBuf,
        /// Require every row to have a nonblank, current, control-preserving
        /// translation with translated/reviewed/approved status (no partial pivot)
        #[arg(long)]
        require_complete: bool,
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
        /// When omitted, use the recorded injection backup if available, then
        /// <game>/.locust/backup/ if valid. Missing recorded backups are errors.
        #[arg(long)]
        pristine: Option<PathBuf>,
        #[command(flatten)]
        identity: PatchIdentityArgs,
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
        /// Check the rollback and show its effects without changing game files
        #[arg(long)]
        dry_run: bool,
    },
    /// Show whether a game has a Locust patch applied
    PatchStatus {
        game_path: PathBuf,
        /// Check installed files against the patch receipt
        #[arg(long)]
        verify: bool,
    },
    /// Inspect an interrupted translation insertion and its recovery conflicts
    InjectStatus { game_path: PathBuf },
    /// Restore the original files after an interrupted translation insertion
    InjectRecover {
        game_path: PathBuf,
        /// Preserve conflicting user bytes separately before restoring originals
        #[arg(long)]
        force: bool,
    },
    /// List saved original-game backups from injection
    Backups,
    /// Restore original game files from a saved injection backup
    RestoreBackup {
        id: String,
        /// Check the backup and restore destination without writing game files
        #[arg(long)]
        dry_run: bool,
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
    /// Register an extra UI language on an RPG Maker MV/MZ multi-lang game
    /// (Iavra Languages + VisuMZ Options + boot Map choices). Creates `*.bak-locust` backups.
    RegisterLang {
        /// Deployed game root (folder with js/ and data/)
        game_path: PathBuf,
        /// Project database whose matching Add recording should include registration
        #[arg(short = 'P', long)]
        project: Option<PathBuf>,
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
    /// Export translations to PO, XLIFF or restricted bilingual TMX
    Export {
        project: PathBuf,
        #[arg(short, long)]
        format: String,
        #[arg(short, long)]
        lang: String,
        #[arg(short, long)]
        output: PathBuf,
        /// Explicit source language (required for TMX; ignored for PO/XLIFF)
        #[arg(long)]
        source_lang: Option<String>,
        /// Replace an existing catalog (never the project database or its sidecars)
        #[arg(long)]
        overwrite: bool,
        /// Export only these states (repeat to combine): pending, translated,
        /// reviewed, approved, error
        #[arg(long, value_name = "STATE")]
        status: Vec<String>,
        /// Export only strings from this source file (exact stored path); combine with --status
        /// Paths are stored verbatim; separators must match
        #[arg(long, value_name = "PATH")]
        file: Option<String>,
    },
    /// Import translations from PO, XLIFF or restricted bilingual TMX
    Import {
        project: PathBuf,
        #[arg(short, long)]
        format: String,
        #[arg(short, long)]
        lang: String,
        #[arg(short, long)]
        input: PathBuf,
        /// Explicit source language (required for TMX; ignored for PO/XLIFF)
        #[arg(long)]
        source_lang: Option<String>,
        /// Preserve existing nonblank translations
        #[arg(long)]
        keep_existing: bool,
        /// Preview changes without saving translations
        #[arg(long)]
        dry_run: bool,
    },
    /// Start the web server
    Server {
        #[arg(short, long, default_value = "7842")]
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
    /// Copy a language pair's glossary to an existing project.
    Copy {
        source: PathBuf,
        dest: PathBuf,
        #[arg(short, long)]
        lang_pair: String,
        /// Replace conflicting destination terms with the source entries.
        #[arg(long)]
        overwrite: bool,
    },
}

#[derive(Debug, Default, clap::Args)]
struct PatchIdentityArgs {
    /// DLsite product code (RJ, RE, VJ, BJ, or RG and 6 or 8 digits)
    #[arg(long, value_name = "CODE")]
    rj: Option<String>,
    /// Store product identity; repeat for different stores
    #[arg(long, value_name = "store=id")]
    store_id: Vec<String>,
    /// Original game version (trimmed, non-empty)
    #[arg(long, value_name = "TEXT")]
    game_version: Option<String>,
    /// Disable automatic DLsite code detection from the game path
    #[arg(long)]
    no_detect_id: bool,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    let filter = if cli.verbose { "debug" } else { "info" };
    let logging = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false);
    if matches!(&cli.command, Commands::Inject { json: true, .. }) {
        // Only inject --json reserves stdout for its core report. Keep all
        // other commands' existing logging/output behavior unchanged.
        logging.with_writer(std::io::stderr).init();
    } else {
        logging.init();
    }

    let config = util::load_config(&cli.config);

    match cli.command {
        Commands::Extract {
            path,
            format,
            output,
            dry_run,
            force,
        } => extract::cmd_extract(path, format, output, dry_run, force)?,
        Commands::Translate {
            project,
            provider,
            source,
            target,
            batch_size,
            concurrency,
            max_batch_tokens,
            no_memory,
            allow_lossy_binary_fit,
            fallback,
            cost_limit,
            context,
        } => {
            translate::cmd_translate(
                config,
                project,
                provider,
                source,
                target,
                batch_size,
                concurrency,
                max_batch_tokens,
                no_memory,
                allow_lossy_binary_fit,
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
            json,
        } => {
            if direct {
                inject::cmd_inject_direct(game_path, project, languages, json).await?
            } else {
                inject::cmd_inject(game_path, project, mode, languages, output_dir, json).await?
            }
        }
        Commands::Validate { project, json } => validate::cmd_validate(project, json)?,
        Commands::FontCheck { path, text_file } => validate::cmd_font_check(path, text_file)?,
        Commands::FontPatch {
            game_path,
            font,
            target,
            engine,
            lang,
            text_file,
            base_patch,
            output,
        } => validate::cmd_font_patch(
            game_path, font, target, engine, lang, text_file, base_patch, output,
        )?,
        Commands::Replace {
            project,
            find,
            replace,
            case_sensitive,
            dry_run,
        } => validate::cmd_replace(project, find, replace, case_sensitive, dry_run).await?,
        Commands::Stats { project, by_file } => validate::cmd_stats(project, by_file)?,
        Commands::BackupProject { project, output } => {
            project::cmd_backup_project(project, output)?
        }
        Commands::Pivot {
            source,
            output,
            require_complete,
        } => project::cmd_pivot(source, output, require_complete)?,
        Commands::Patch {
            game_path,
            project,
            lang,
            output,
            astro,
            pristine,
            identity,
        } => patch::cmd_patch(game_path, project, lang, output, astro, pristine, identity)?,
        Commands::Apply {
            game_path,
            zip,
            url,
            force,
            confirm_legacy,
            dry_run,
        } => patch::cmd_apply(game_path, zip, url, force, confirm_legacy, dry_run)?,
        Commands::PatchRollback {
            game_path,
            force,
            dry_run,
        } => patch::cmd_patch_rollback(game_path, force, dry_run)?,
        Commands::PatchStatus { game_path, verify } => patch::cmd_patch_status(game_path, verify)?,
        Commands::InjectStatus { game_path } => inject::status(&game_path)?,
        Commands::InjectRecover { game_path, force } => inject::recover(&game_path, force)?,
        Commands::Backups => project::cmd_backups()?,
        Commands::RestoreBackup { id, dry_run } => project::cmd_restore_backup(&id, dry_run)?,
        Commands::Auth { provider } => auth_info::cmd_auth(provider).await?,
        Commands::Providers => auth_info::cmd_providers(&config)?,
        Commands::Formats => auth_info::cmd_formats()?,
        Commands::RegisterLang {
            game_path,
            project,
            lang,
            label,
        } => inject::cmd_register_lang(game_path, project, lang, label)?,
        Commands::Glossary { action } => io::cmd_glossary(action)?,
        Commands::Export {
            project,
            format,
            lang,
            output,
            source_lang,
            overwrite,
            status,
            file,
        } => io::cmd_export(
            &config,
            project,
            format,
            lang,
            output,
            source_lang,
            overwrite,
            status,
            file,
        )?,
        Commands::Import {
            project,
            format,
            lang,
            input,
            source_lang,
            keep_existing,
            dry_run,
        } => {
            io::cmd_import(
                project,
                format,
                lang,
                input,
                source_lang,
                keep_existing,
                dry_run,
            )
            .await?
        }
        Commands::Server { port } => server::cmd_server(port.unwrap_or(7842)).await?,
    }

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

    #[test]
    fn server_uses_browser_port_by_default() {
        let Commands::Server { port } = Cli::try_parse_from(["locust", "server"]).unwrap().command
        else {
            panic!("expected server command");
        };
        assert_eq!(port, Some(7842));

        let Commands::Server { port } = Cli::try_parse_from(["locust", "server", "--port", "9000"])
            .unwrap()
            .command
        else {
            panic!("expected server command");
        };
        assert_eq!(port, Some(9000));
    }
}
