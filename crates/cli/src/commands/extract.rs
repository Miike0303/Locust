use std::path::{Path, PathBuf};

use comfy_table::Table;
use locust_core::database::{Database, MergeStats};
use locust_core::models::StringEntry;

fn merge_extracted(
    db: &Database,
    game_root: &Path,
    entries: &[StringEntry],
    preserve_missing: bool,
    force: bool,
) -> anyhow::Result<MergeStats> {
    // Both real extracts and scratch previews must check before merging: a
    // complete merge removes entries absent from the selected game's sources.
    if !force {
        if let Some(value) = db.get_project_metadata("game_root")? {
            let recorded_root: PathBuf = serde_json::from_value(value)?;
            if !locust_core::database::paths_identical(&recorded_root, game_root) {
                anyhow::bail!(
                    "The project database belongs to the game at \"{}\", but \"{}\" is being extracted. \
                     Pass `-o <other.db>` to use another database, or `--force` to merge anyway.",
                    recorded_root.display(),
                    game_root.display()
                );
            }
        }
    }
    let merge = if preserve_missing {
        db.merge_entries_preserving_missing(entries)?
    } else {
        db.merge_entries(entries)?
    };
    // The caller supplies the canonical selected path. For dry runs this only
    // updates the disposable snapshot, including when adopting a legacy DB.
    db.set_project_metadata("game_root", &serde_json::json!(game_root))?;
    Ok(merge)
}

fn sqlite_sidecar(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(suffix);
    path.with_file_name(name)
}

fn remove_sqlite_files(path: &Path) {
    let _ = std::fs::remove_file(path);
    for suffix in ["-wal", "-shm", "-journal"] {
        let _ = std::fs::remove_file(sqlite_sidecar(path, suffix));
    }
}

fn preview_extract_merge_in(
    scratch_path: &Path,
    db_path: &Path,
    game_root: &Path,
    entries: &[StringEntry],
    preserve_missing: bool,
    force: bool,
) -> anyhow::Result<MergeStats> {
    if db_path.try_exists()? {
        Database::snapshot_existing(db_path, scratch_path)?;
    }
    let scratch = Database::open(scratch_path)?;
    merge_extracted(&scratch, game_root, entries, preserve_missing, force)
}

/// Run the same merge a real extract would, against a scratch database.
/// The output path, its parent, and any existing database sidecars are not created or modified.
fn preview_extract_merge(
    db_path: &Path,
    game_root: &Path,
    entries: &[StringEntry],
    preserve_missing: bool,
    force: bool,
) -> anyhow::Result<MergeStats> {
    let scratch_dir = tempfile::tempdir()?;
    let scratch_path = scratch_dir.path().join("extract-preview.locust.db");
    let result = preview_extract_merge_in(
        &scratch_path,
        db_path,
        game_root,
        entries,
        preserve_missing,
        force,
    );
    remove_sqlite_files(&scratch_path);
    result
}

/// A Direct (or legacy, mode-unknown) injection replaced the game's own text
/// with translations. Extracting it reads that text as the source, and the
/// merge drops translated rows whose ids its own wrapping shifted (a real
/// 2489-row game lost 965 translations). Add injections leave originals alone.
fn warn_or_refuse_direct_injection(
    game_root: &Path,
    dry_run: bool,
    force: bool,
) -> anyhow::Result<()> {
    use locust_core::injection_transaction::{game_status, InjectionMode};
    let status = game_status(game_root)?;
    let in_place = status
        .injections
        .iter()
        .any(|injection| injection.mode != Some(InjectionMode::Add));
    if !in_place && !status.injection_pending {
        return Ok(());
    }
    let reason = format!(
        "{} contains a Locust Direct injection: its files hold translated text, so extracting would \
         read translations as the source and remove translated rows whose ids moved.",
        game_root.display()
    );
    if dry_run {
        eprintln!("warning: {reason}");
        return Ok(());
    }
    if force {
        eprintln!("warning: {reason} Continuing because --force was given.");
        return Ok(());
    }
    anyhow::bail!(
        "{reason} Keep using the project database that injected it, restore the original \
         first (`locust backups`, then `locust restore-backup <id>`), or pass --force to \
         extract the translated files anyway."
    )
}

pub(crate) fn cmd_extract(
    path: PathBuf,
    format: Option<String>,
    output: Option<PathBuf>,
    dry_run: bool,
    force: bool,
) -> anyhow::Result<()> {
    cmd_extract_with_registry(
        path,
        format,
        output,
        dry_run,
        force,
        &locust_formats::default_registry(),
    )
}

fn cmd_extract_with_registry(
    path: PathBuf,
    format: Option<String>,
    output: Option<PathBuf>,
    dry_run: bool,
    force: bool,
    registry: &locust_core::extraction::FormatRegistry,
) -> anyhow::Result<()> {
    if !path.try_exists()? {
        anyhow::bail!(
            "game folder not found: {}. Check the path and try again.",
            path.display()
        );
    }
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
    plugin.validate_source(&path)?;
    let game_root = path.canonicalize()?;
    let _source_lock = locust_core::project::lock_game_source(&path)?;
    warn_or_refuse_direct_injection(&game_root, dry_run, force)?;

    println!("Format: {} ({})", plugin.name(), plugin.id());

    let extraction = plugin.extract_with_diagnostics(&path)?;
    let extraction_warnings = extraction.bounded_warnings();
    let entries = extraction.entries;
    let total = entries.len();
    for warning in &extraction_warnings {
        eprintln!("warning: partial extraction: {warning}");
    }

    let db_path = output.unwrap_or_else(|| {
        let name = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        PathBuf::from(format!("{}.locust.db", name))
    });

    let preserve_missing = !extraction.complete;
    let merge = if dry_run {
        preview_extract_merge(&db_path, &game_root, &entries, preserve_missing, force)?
    } else {
        let db = Database::open(&db_path)?;
        let merge = merge_extracted(&db, &game_root, &entries, preserve_missing, force)?;
        db.set_project_metadata(
            "extraction_warnings",
            &serde_json::json!(extraction_warnings),
        )?;
        merge
    };

    let mut table = Table::new();
    table.set_header(vec!["Property", "Value"]);
    table.add_row(vec!["Format", plugin.name()]);
    table.add_row(vec!["Strings extracted", &total.to_string()]);
    table.add_row(vec!["Output file", &db_path.display().to_string()]);
    table.add_row(vec!["Added", &merge.added.to_string()]);
    table.add_row(vec!["Updated", &merge.updated.to_string()]);
    table.add_row(vec![
        "Source changed (reset to pending)",
        &merge.stale_source_reset.to_string(),
    ]);
    table.add_row(vec!["Removed", &merge.removed.to_string()]);
    table.add_row(vec![
        "Translations preserved",
        &merge.preserved_translations.to_string(),
    ]);
    table.add_row(vec![
        "Translations lost",
        &merge.lost_translations.to_string(),
    ]);
    println!("{table}");
    if dry_run {
        println!("Dry run: nothing was written");
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use locust_core::database::EntryFilter;
    use locust_core::models::StringStatus;
    use std::fs;

    struct DiagnosticExtractPlugin;

    impl locust_core::extraction::FormatPlugin for DiagnosticExtractPlugin {
        fn id(&self) -> &str {
            "diagnostics-test"
        }
        fn name(&self) -> &str {
            "Diagnostics test"
        }
        fn supported_extensions(&self) -> &[&str] {
            &[]
        }
        fn extract(&self, _: &Path) -> locust_core::error::Result<Vec<StringEntry>> {
            panic!("CLI must use the diagnostics channel")
        }
        fn extract_with_diagnostics(
            &self,
            path: &Path,
        ) -> locust_core::error::Result<locust_core::extraction::Extraction> {
            let complete = !path.join("partial").exists();
            let entries = fs::read_to_string(path.join("story.txt"))?
                .lines()
                .map(|id| StringEntry::new(id, id, path.join("story.txt")))
                .collect();
            Ok(locust_core::extraction::Extraction {
                entries,
                warnings: if complete {
                    vec![]
                } else {
                    vec!["story.txt: unreadable member".into()]
                },
                complete,
            })
        }
        fn inject(
            &self,
            _: &Path,
            _: &[StringEntry],
        ) -> locust_core::error::Result<locust_core::extraction::InjectionReport> {
            unreachable!("extraction-only fixture")
        }
    }

    struct SourceScopePlugin;

    impl locust_core::extraction::FormatPlugin for SourceScopePlugin {
        fn id(&self) -> &str {
            "source-scope"
        }
        fn name(&self) -> &str {
            "Source scope"
        }
        fn supported_extensions(&self) -> &[&str] {
            &[]
        }
        fn validate_source(&self, _: &Path) -> locust_core::error::Result<()> {
            Err(locust_core::error::LocustError::UnsupportedFormat(
                "source scope refused".into(),
            ))
        }
        fn extract(&self, _: &Path) -> locust_core::error::Result<Vec<StringEntry>> {
            panic!("preflight bypassed")
        }
        fn inject(
            &self,
            _: &Path,
            _: &[StringEntry],
        ) -> locust_core::error::Result<locust_core::extraction::InjectionReport> {
            panic!("preflight bypassed")
        }
    }

    #[test]
    fn cli_source_scope_refuses_manual_directory_before_output_or_lock() {
        let temp = tempfile::tempdir().unwrap();
        let output = temp.path().join("must-not-exist.db");
        let mut registry = locust_core::extraction::FormatRegistry::new();
        registry.register(Box::new(SourceScopePlugin));
        let error = cmd_extract_with_registry(
            temp.path().to_owned(),
            Some("source-scope".into()),
            Some(output.clone()),
            false,
            false,
            &registry,
        )
        .unwrap_err();
        assert!(
            error.to_string().contains("source scope refused"),
            "{error}"
        );
        assert!(!output.exists());
        assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 0);
    }

    #[test]
    fn cli_plugin_diagnostics_preserve_zero_rows_and_dry_run_then_clear() {
        let temp = tempfile::tempdir().unwrap();
        let game = temp.path().join("game");
        fs::create_dir(&game).unwrap();
        fs::write(game.join("story.txt"), "hero\nnpc").unwrap();
        let project = temp.path().join("project.db");
        let mut registry = locust_core::extraction::FormatRegistry::new();
        registry.register(Box::new(DiagnosticExtractPlugin));
        let extract = |dry_run| {
            cmd_extract_with_registry(
                game.clone(),
                Some("diagnostics-test".into()),
                Some(project.clone()),
                dry_run,
                false,
                &registry,
            )
            .unwrap()
        };
        extract(false);
        let db = Database::open(&project).unwrap();
        let mut rows = db.get_entries(&EntryFilter::default()).unwrap();
        for row in &mut rows {
            row.translation = Some("Traducción".into());
            row.status = StringStatus::Approved;
        }
        db.save_entries(&rows).unwrap();
        let before = serde_json::to_value(&rows).unwrap();
        drop(db);
        fs::write(game.join("story.txt"), "").unwrap();
        fs::write(game.join("partial"), "").unwrap();
        extract(true);
        let db = Database::open(&project).unwrap();
        assert!(locust_core::project::saved_extraction_warnings(&db)
            .unwrap()
            .is_empty());
        assert_eq!(
            serde_json::to_value(db.get_entries(&EntryFilter::default()).unwrap()).unwrap(),
            before
        );
        drop(db);
        extract(false);
        let db = Database::open(&project).unwrap();
        assert_eq!(
            serde_json::to_value(db.get_entries(&EntryFilter::default()).unwrap()).unwrap(),
            before
        );
        assert_eq!(
            locust_core::project::saved_extraction_warnings(&db).unwrap(),
            ["story.txt: unreadable member"]
        );
        drop(db);
        fs::remove_file(game.join("partial")).unwrap();
        extract(false);
        let db = Database::open(&project).unwrap();
        assert_eq!(db.get_stats().unwrap().total, 0);
        assert!(locust_core::project::saved_extraction_warnings(&db)
            .unwrap()
            .is_empty());
    }

    fn extract_root_fixture() -> (tempfile::TempDir, PathBuf, PathBuf, Database) {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("A");
        let b = dir.path().join("B");
        fs::create_dir(&a).unwrap();
        fs::create_dir(&b).unwrap();
        let db = Database::open(&dir.path().join("project.db")).unwrap();
        let mut entry = StringEntry::new("a", "Hello!", "a.rpy".into());
        entry.translation = Some("¡Hola!".into());
        entry.status = StringStatus::Translated;
        db.save_entries(&[entry]).unwrap();
        (dir, a, b, db)
    }

    #[test]
    fn extract_merge_refuses_foreign_root_before_changing_translations() {
        for preserve_missing in [false, true] {
            let (_dir, a, b, db) = extract_root_fixture();
            let a = a.canonicalize().unwrap();
            let b = b.canonicalize().unwrap();
            db.set_project_metadata("game_root", &serde_json::json!(a))
                .unwrap();
            let before = serde_json::to_value(db.get_entry("a").unwrap()).unwrap();
            let entries = [StringEntry::new("b", "Welcome!", "b.rpy".into())];
            let result = merge_extracted(&db, &b, &entries, preserve_missing, false);
            assert!(result.is_err(), "foreign extraction unexpectedly merged");
            let error = result.unwrap_err().to_string();
            for root in [&a, &b] {
                assert!(error.contains(&root.display().to_string()), "{error}");
            }
            assert!(error.contains("-o <other.db>"), "{error}");
            assert!(error.contains("--force"), "{error}");
            assert_eq!(
                serde_json::to_value(db.get_entry("a").unwrap()).unwrap(),
                before
            );
            assert!(db.get_entry("b").unwrap().is_none());
        }
    }

    #[test]
    fn extract_preview_checks_foreign_root_and_force_without_writing() {
        let (_dir, a, b, db) = extract_root_fixture();
        db.set_project_metadata("game_root", &serde_json::json!(a.canonicalize().unwrap()))
            .unwrap();
        let saved = db.path();
        drop(db);
        let before = fs::read(&saved).unwrap();
        let entries = [StringEntry::new("b", "Welcome!", "b.rpy".into())];
        let root = b.canonicalize().unwrap();
        assert!(preview_extract_merge(&saved, &root, &entries, false, false).is_err());
        assert_eq!(fs::read(&saved).unwrap(), before);
        let preview = preview_extract_merge(&saved, &root, &entries, false, true).unwrap();
        assert_eq!(preview.added, 1);
        assert_eq!(preview.removed, 1);
        assert_eq!(preview.lost_translations, 1);
        assert_eq!(fs::read(&saved).unwrap(), before);
    }

    #[test]
    fn extract_merge_records_root_for_legacy_same_root_and_forced_merges() {
        for kind in ["legacy", "same", "force", "case"] {
            let (_dir, a, b, db) = extract_root_fixture();
            let saved_root = match kind {
                "legacy" => None,
                "force" => Some(b),
                "case" if cfg!(windows) => Some(PathBuf::from(a.to_str().unwrap().to_uppercase())),
                _ => Some(a.clone()),
            };
            if let Some(root) = saved_root {
                db.set_project_metadata("game_root", &serde_json::json!(root))
                    .unwrap();
            }
            let before = db.get_entries(&EntryFilter::default()).unwrap();
            let root = a.canonicalize().unwrap();
            let merge = merge_extracted(&db, &root, &before, false, kind == "force").unwrap();
            assert_eq!(merge.preserved_translations, 1, "{kind}");
            assert_eq!(
                serde_json::to_value(db.get_entries(&EntryFilter::default()).unwrap()).unwrap(),
                serde_json::to_value(before).unwrap(),
                "{kind}"
            );
            assert_eq!(
                db.get_project_metadata("game_root").unwrap(),
                Some(serde_json::json!(root)),
                "{kind}"
            );
        }
    }
}
