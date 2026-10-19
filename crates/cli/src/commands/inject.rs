use std::path::PathBuf;
use std::sync::Arc;

use comfy_table::Table;
use locust_core::backup::BackupManager;
use locust_core::database::{Database, EntryFilter};
use locust_core::models::{OutputMode, StringEntry};
use locust_core::validation::count_binary_slot_oversize;
use tokio::sync::mpsc;

pub(crate) use crate::injection_recovery::{recover, status};
use crate::util::{locust_backup_root, open_existing_project};

/// The restore step shared by every remedy issued from a state where the
/// ORIGINAL game tree was provably already mutated. Advice without it is a
/// closed loop: the re-run finds no original text, writes nothing, and
/// records nothing.
const RESTORE_ORIGINAL_FIRST: &str =
    "Your original game was modified — restore it from the backup listed above \
     (or from a clean copy) first.";

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
    // Wolf RPG once wrote into the original tree too, but its Replace mode now
    // writes the per-language copy (wolf_replace_records_the_copy_and_preserves_the_original).
    if writes_to_entry_tree(format_id) && format_id != "wolf-rpg" {
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
            let restore = " If this game was ALREADY injected, the original text this engine \
                 scans for is gone and every re-run will keep writing 0 files — \
                 restore the original game files from a backup or a clean copy, \
                 then re-run the inject.";
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

pub(crate) async fn cmd_inject(
    game_path: PathBuf,
    project: PathBuf,
    mode: Option<String>,
    languages: Vec<String>,
    output_dir: Option<PathBuf>,
    json: bool,
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
    if languages.len() > 1 {
        anyhow::bail!(
            "one project database can inject only one target language per run. \
             Create/use a separate pivot database for each target language, then \
             run inject once per DB."
        );
    }

    let registry = Arc::new(locust_formats::default_registry());

    let plugin = registry
        .detect(&game_path)
        .ok_or_else(|| anyhow::anyhow!("format not detected"))?;
    plugin.validate_source(&game_path)?;
    let db = Arc::new(open_existing_project(&project)?);

    let format_id = plugin.id().to_string();

    // Keep originals in persistent app data; Rust's Windows paths support long paths.
    let backup_root = locust_backup_root();
    std::fs::create_dir_all(&backup_root).ok();
    let backup_mgr = Arc::new(BackupManager::new(backup_root));

    // Recordings in other project databases can still reference older backups.
    // A global age/count limit cannot prove those originals are unreferenced.

    let preflight_entries = db.get_entries(&EntryFilter::default())?;
    warn_binary_slot_oversize(&preflight_entries);

    if !json {
        println!("Creating backup...");
    }
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
    if json {
        println!("{}", serde_json::to_string(&report)?);
    } else {
        for (lang, outcome) in &outcomes {
            if let Some(rep) = report.reports.get(lang) {
                print_record_outcome(lang, outcome, rep);
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
            for (reason, count) in &rep.skip_reasons {
                table.add_row(vec![format!("{lang} skipped: {reason}"), count.to_string()]);
            }
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
    }

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

pub(crate) async fn cmd_inject_direct(
    game_path: PathBuf,
    project: PathBuf,
    languages: Vec<String>,
    json: bool,
) -> anyhow::Result<()> {
    if languages.len() > 1 {
        anyhow::bail!(
            "one project database can inject only one target language per run. \
             Create/use a separate pivot database for each target language, then \
             run inject once per DB."
        );
    }
    let registry = locust_formats::default_registry();

    let plugin = registry
        .detect(&game_path)
        .ok_or_else(|| anyhow::anyhow!("format not detected for: {}", game_path.display()))?;
    plugin.validate_source(&game_path)?;
    let db = open_existing_project(&project)?;

    let format_id = plugin.id().to_string();
    let entries = db.get_entries(&EntryFilter::default())?;
    if !json {
        let translated_count = entries.iter().filter(|e| e.translation.is_some()).count();
        println!(
            "Direct inject: {} translated strings into {} ({})",
            translated_count,
            game_path.display(),
            plugin.name()
        );
    }
    warn_binary_slot_oversize(&entries);

    // Shared core path (HTTP + desktop): backup, inject in place, record
    // for `locust patch`. Direct inject always writes the original tree.
    let backup_root = locust_backup_root();
    std::fs::create_dir_all(&backup_root).ok();
    let mgr = BackupManager::new(backup_root);
    if !json {
        println!("Creating backup before writing the original game tree...");
    }
    let report = locust_core::extraction::inject_direct(
        &registry, &db, &mgr, &game_path, &format_id, &languages,
    )?;
    if json {
        println!("{}", serde_json::to_string(&report)?);
        return Ok(());
    }

    // Contractual zero-write reporting (same messages as multilang inject):
    // silent keeps / silent nothing-recorded are the stale-recording hazard.
    for (lang, outcome) in &report.outcomes {
        if let Some(rep) = report.reports.get(lang) {
            print_record_outcome(lang, outcome, rep);
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
    if let Some(rep) = report.reports.values().next() {
        for (reason, count) in &rep.skip_reasons {
            table.add_row(vec![format!("Skipped: {reason}"), count.to_string()]);
        }
    }
    if let Some(ref path) = report.backup_path {
        table.add_row(vec!["Backup", path]);
    }
    if !report.warnings.is_empty() {
        table.add_row(vec!["Warnings", &report.warnings.len().to_string()]);
    }
    println!("{table}");

    Ok(())
}

pub(crate) fn cmd_register_lang(
    game_path: PathBuf,
    project: Option<PathBuf>,
    lang: String,
    label: String,
) -> anyhow::Result<()> {
    let db = project.as_deref().map(Database::open).transpose()?;
    let report = locust_formats::rpgmaker_lang::register_language_with_db(
        &game_path,
        &lang,
        &label,
        db.as_ref(),
    )?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{patch_test_tempdir, save_translated, TestProfile};
    use locust_core::models::StringStatus;
    use std::fs;

    #[test]
    fn test_add_mode_remedy_offers_replace_where_it_works() {
        let game = PathBuf::from("game");
        let project = PathBuf::from("proj.db");
        let langs = ["es".to_string()];
        // Wolf Replace writes the per-language copy and keeps the original
        // (wolf_replace_records_the_copy_and_preserves_the_original).
        let wolf = add_mode_remedy("wolf-rpg", &game, &project, &langs);
        assert!(wolf.contains("-o <output_dir>"), "{wolf}");
        assert!(!wolf.contains("cannot work"), "{wolf}");
        for engine in ["unity", "unreal"] {
            let remedy = add_mode_remedy(engine, &game, &project, &langs);
            assert!(remedy.contains("--direct"), "{remedy}");
            assert!(!remedy.contains("-o <output_dir>"), "{remedy}");
        }
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

    #[tokio::test]
    async fn test_inject_direct_rejects_multiple_languages_for_one_database() {
        let _profile = TestProfile::new_async().await;
        let base_guard = patch_test_tempdir();
        let base = base_guard.path().to_path_buf();
        let bak = base.join("bak");
        let game_dir = base.join("renpygame");
        let game_sub = game_dir.join("game");
        fs::create_dir_all(&game_sub).unwrap();
        let script = game_sub.join("script.rpy");
        fs::write(&script, "label start:\n    \"Hello, world!\"\n").unwrap();

        let db_path = base.join("project.locust.db");
        let db = Database::open(&db_path).unwrap();
        let mut pivoted =
            StringEntry::new("script.rpy#2", "English semantic source", script.clone());
        pivoted.translation = Some("Hola, mundo!".into());
        pivoted.status = StringStatus::Translated;
        pivoted.metadata.insert(
            "locust_injection_source".into(),
            serde_json::json!("Hello, world!"),
        );
        db.save_entries(&[pivoted]).unwrap();
        drop(db);

        {
            std::env::set_var("LOCUST_BACKUP_ROOT", &bak);
            let error = cmd_inject_direct(
                game_dir.clone(),
                db_path.clone(),
                vec!["es".to_string(), "fr".to_string()],
                false,
            )
            .await
            .unwrap_err();
            assert!(error.to_string().contains("separate pivot database"));
        }

        let db = Database::open(&db_path).unwrap();
        for lang in ["es", "fr"] {
            assert!(db.get_injection(Some(lang)).unwrap().is_none());
        }
        assert_eq!(
            fs::read_to_string(&script).unwrap(),
            "label start:\n    \"Hello, world!\"\n"
        );
        assert!(!bak.exists());
    }

    #[tokio::test]
    async fn test_inject_direct_backs_up_before_mutating_renpy_tree() {
        let _profile = TestProfile::new_async().await;
        // Task #14: --direct is the universally recommended recovery path, but
        // for engines that write the original tree it used to take no backup.
        let base_guard = patch_test_tempdir();
        let base = base_guard.path().to_path_buf();
        let bak = base.join("bak");
        let game_dir = base.join("renpygame");
        let game_sub = game_dir.join("game");
        fs::create_dir_all(&game_sub).unwrap();
        let script = game_sub.join("script.rpy");
        fs::write(&script, "label start:\n    \"こんにちは、世界！\"\n").unwrap();
        let original = fs::read(&script).unwrap();

        let db_path = base.join("project.locust.db");
        let db = Database::open(&db_path).unwrap();
        let mut pivoted =
            StringEntry::new("script.rpy#2", "English semantic source", script.clone());
        pivoted.translation = Some("Hola, mundo!".into());
        pivoted.status = StringStatus::Translated;
        pivoted.metadata.insert(
            "locust_injection_source".into(),
            serde_json::json!("こんにちは、世界！"),
        );
        db.save_entries(&[pivoted]).unwrap();
        drop(db);

        {
            std::env::set_var("LOCUST_BACKUP_ROOT", &bak);
            cmd_inject_direct(game_dir.clone(), db_path, vec!["es".to_string()], false)
                .await
                .unwrap();
        }

        // Injection mutated the loose script.
        let injected = fs::read_to_string(&script).unwrap();
        assert!(injected.contains("Hola, mundo!"), "{injected}");

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
        let backed = bak_dirs[0].join("payload").join("game").join("script.rpy");
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
    async fn pivoted_html_inject_uses_original_slot_and_leaves_duplicate_untouched() {
        let _profile = TestProfile::new_async().await;
        let base_guard = patch_test_tempdir();
        let base = base_guard.path().to_path_buf();
        let bak = base.join("bak");
        let game = base.join("htmlgame");
        fs::create_dir_all(&game).unwrap();
        let html = game.join("index.html");
        fs::write(&html, "<p>日本語</p><p>日本語</p>").unwrap();

        let registry = locust_formats::default_registry();
        let plugin = registry.detect(&game).expect("HTML format");
        let mut extracted = plugin.extract(&game).unwrap();
        assert_eq!(extracted.len(), 2);
        let mut pivoted = extracted.remove(0);
        let physical = pivoted.source.clone();
        pivoted.source = "English semantic source".into();
        pivoted.translation = Some("Español".into());
        pivoted.status = StringStatus::Translated;
        pivoted.metadata.insert(
            "locust_injection_source".into(),
            serde_json::json!(physical),
        );
        let db_path = base.join("html.locust.db");
        Database::open(&db_path)
            .unwrap()
            .save_entries(&[pivoted])
            .unwrap();

        {
            std::env::set_var("LOCUST_BACKUP_ROOT", &bak);
            cmd_inject_direct(game.clone(), db_path, vec!["es".into()], false)
                .await
                .unwrap();
        }

        assert_eq!(
            fs::read_to_string(html).unwrap(),
            "<p>Español</p><p>日本語</p>"
        );
    }

    #[tokio::test]
    async fn test_inject_direct_without_lang_records_the_unspecified_key() {
        let _profile = TestProfile::new_async().await;
        let base_guard = patch_test_tempdir();
        let base = base_guard.path().to_path_buf();
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
            std::env::set_var("LOCUST_BACKUP_ROOT", &bak);
            cmd_inject_direct(game_dir, db_path.clone(), Vec::new(), false)
                .await
                .unwrap();
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
}
