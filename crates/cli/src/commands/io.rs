use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use comfy_table::Table;
use locust_core::config::AppConfig;
use locust_core::database::{EntryFilter, GlossaryEntry};
use locust_core::export;
use locust_core::glossary::Glossary;

use crate::util::open_existing_project;
use crate::GlossaryCommands;

pub(crate) fn cmd_glossary(action: GlossaryCommands) -> anyhow::Result<()> {
    match action {
        GlossaryCommands::Import {
            project,
            input,
            overwrite,
        } => {
            // Read and validate the entire payload before even opening the
            // destination. A malformed tail cannot leave a partial import.
            let entries = read_glossary_import(&input)?;
            let db = open_existing_project(&project)?;
            let report = db.merge_glossary_entries(&entries, overwrite)?;
            println!(
                "Glossary import: added={} overwritten={} kept_existing={} unchanged={}",
                report.added, report.overwritten, report.kept_existing, report.unchanged
            );
        }
        GlossaryCommands::Add {
            project,
            term,
            translation,
            lang_pair,
            case_sensitive,
            context,
        } => {
            let db = Arc::new(open_existing_project(&project)?);
            let glossary = Glossary::new(db);
            let entry = GlossaryEntry {
                term,
                translation,
                lang_pair,
                context,
                case_sensitive,
            };
            glossary.add_entry(&entry)?;
            println!(
                "Added: {} → {} ({})",
                entry.term, entry.translation, entry.lang_pair
            );
        }
        GlossaryCommands::List { project, lang_pair } => {
            let db = Arc::new(open_existing_project(&project)?);
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
            let db = Arc::new(open_existing_project(&project)?);
            let glossary = Glossary::new(db);
            glossary.delete(&term, &lang_pair)?;
            println!("Deleted: {} ({})", term, lang_pair);
        }
        GlossaryCommands::Copy {
            source,
            dest,
            lang_pair,
            overwrite,
        } => {
            if locust_core::database::paths_identical(&source, &dest) {
                anyhow::bail!("source and destination refer to the same database");
            }
            let source_db = open_existing_project(&source)?;
            let dest_db = open_existing_project(&dest)?;
            let entries = source_db.get_glossary(&lang_pair)?;
            let report = dest_db.merge_glossary_entries(&entries, overwrite)?;
            println!(
                "Glossary copy: added={} overwritten={} kept_existing={} unchanged={}",
                report.added, report.overwritten, report.kept_existing, report.unchanged
            );
        }
    }
    Ok(())
}

fn read_glossary_import(input: &Path) -> anyhow::Result<Vec<GlossaryEntry>> {
    const MAX_GLOSSARY_BYTES: u64 = 32 * 1024 * 1024;
    let mut bytes = Vec::new();
    std::fs::File::open(input)?
        .take(MAX_GLOSSARY_BYTES + 1)
        .read_to_end(&mut bytes)?;
    anyhow::ensure!(
        bytes.len() as u64 <= MAX_GLOSSARY_BYTES,
        "glossary input exceeds 32 MiB"
    );
    let value: serde_json::Value = serde_json::from_slice(&bytes)?;
    let rows = value
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("glossary input must be a JSON array"))?;
    for (index, row) in rows.iter().enumerate() {
        let object = row
            .as_object()
            .ok_or_else(|| anyhow::anyhow!("glossary entry {} must be an object", index + 1))?;
        for field in object.keys() {
            anyhow::ensure!(
                matches!(
                    field.as_str(),
                    "term" | "translation" | "lang_pair" | "context" | "case_sensitive"
                ),
                "unknown field '{field}' in glossary entry {}",
                index + 1
            );
        }
    }
    let entries: Vec<GlossaryEntry> = serde_json::from_value(value)?;
    for (index, entry) in entries.iter().enumerate() {
        for (field, value) in [
            ("term", entry.term.as_str()),
            ("translation", entry.translation.as_str()),
            ("lang_pair", entry.lang_pair.as_str()),
        ] {
            anyhow::ensure!(
                !value.trim().is_empty(),
                "glossary entry {} has an empty {field}",
                index + 1
            );
        }
    }
    Ok(entries)
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn cmd_export(
    config: &AppConfig,
    project: PathBuf,
    format: String,
    lang: String,
    output: PathBuf,
    source_lang: Option<String>,
    overwrite: bool,
    status: Vec<String>,
    file: Option<String>,
) -> anyhow::Result<()> {
    let db = open_existing_project(&project)?;
    export::check_export_destination(&output, &project)?;
    if output.try_exists()? && !overwrite {
        anyhow::bail!(
            "{} already exists; pass --overwrite to replace it",
            output.display()
        );
    }
    let scope = export::ExportScope::parse(
        file.clone(),
        (!status.is_empty()).then_some(status.as_slice()),
    )
    .map_err(anyhow::Error::msg)?;
    let entries = scope.entries(&db)?;
    if entries.is_empty() {
        if let Some(file) = file {
            if db.count_entries(&EntryFilter {
                file_path: Some(file.clone()),
                ..EntryFilter::default()
            })? == 0
            {
                let known_files = db
                    .get_string_facets()?
                    .file_paths
                    .into_iter()
                    .take(5)
                    .collect::<Vec<_>>()
                    .join(", ");
                anyhow::bail!("no entries to export for file '{file}'; known files: {known_files}");
            }
        }
        anyhow::bail!("no entries to export");
    }

    // Prefer the source language of the latest translation run for this
    // target — config.default_source_lang is a user preference default, not
    // what was actually used for the strings in this project.
    let source_lang = if format == "tmx" {
        source_lang.ok_or_else(|| anyhow::anyhow!("--source-lang is required for TMX"))?
    } else {
        db.resolve_export_source_lang(&lang, &config.default_source_lang)?
    };

    let content = match format.as_str() {
        "po" => export::export_po(&entries, &source_lang, &lang),
        "xliff" => export::export_xliff(&entries, &source_lang, &lang),
        "tmx" => export::export_tmx(&entries, &source_lang, &lang)?,
        _ => anyhow::bail!(
            "unsupported export format: {}. Use 'po', 'xliff' or 'tmx'",
            format
        ),
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

#[allow(clippy::too_many_arguments)]
pub(crate) async fn cmd_import(
    config: &AppConfig,
    project: PathBuf,
    format: String,
    lang: String,
    input: PathBuf,
    source_lang: Option<String>,
    keep_existing: bool,
    dry_run: bool,
    allow_undeclared_language: bool,
) -> anyhow::Result<()> {
    let db = open_existing_project(&project)?;
    let content = if format == "tmx" {
        // Cap the read itself, not just metadata or an already allocated string.
        const MAX_TMX_BYTES: u64 = 32 * 1024 * 1024;
        let mut bytes = Vec::new();
        std::fs::File::open(&input)?
            .take(MAX_TMX_BYTES + 1)
            .read_to_end(&mut bytes)?;
        anyhow::ensure!(
            bytes.len() as u64 <= MAX_TMX_BYTES,
            "TMX input exceeds 32 MiB"
        );
        String::from_utf8(bytes)?
    } else {
        std::fs::read_to_string(&input)?
    };

    anyhow::ensure!(
        ["po", "xliff", "tmx"].contains(&format.as_str()),
        "unsupported import format: {format}. Use 'po', 'xliff' or 'tmx'"
    );
    anyhow::ensure!(
        format != "tmx" || source_lang.is_some(),
        "--source-lang is required for TMX"
    );
    let requested_target = export::normalize_import_language(&lang);
    let requested_source = source_lang
        .as_deref()
        .and_then(export::normalize_import_language);
    anyhow::ensure!(
        requested_target.is_some() && (source_lang.is_none() || requested_source.is_some()),
        "expected a concrete ASCII language tag"
    );
    anyhow::ensure!(
        source_lang.is_none() || requested_source != requested_target,
        "source and target languages must differ"
    );
    let project_languages = export::project_import_languages(
        &db,
        source_lang
            .as_deref()
            .unwrap_or(&config.default_source_lang),
        &lang,
    )?;
    // Explicit CLI languages are assertions, never a way to override the
    // project's recorded pair or a conflicting catalog declaration.
    export::validate_import_languages(
        &export::CatalogLanguages {
            source: source_lang.into_iter().collect(),
            target: vec![lang],
            source_required: false,
        },
        &project_languages,
    )?;
    let catalog = export::parse_import_catalog(&format, &content, &project_languages)?;
    let languages = export::validate_import_languages(&catalog.languages, &project_languages)?;
    for warning in &languages.warnings {
        eprintln!(
            "Warning: {} {} language: declared {:?}, project {:?}",
            warning.code, warning.side, warning.declared, warning.expected
        );
    }
    if !dry_run {
        languages.require_confirmation(allow_undeclared_language)?;
    } else if languages.requires_confirmation {
        eprintln!("Warning: language not declared — import anyway requires --allow-undeclared-language when applying");
    }
    let (updates, pre_skipped) = (catalog.updates, catalog.skipped);
    let attempted = updates.len();
    let report = if dry_run {
        let preview = db.preview_imported_translations(&updates, keep_existing)?;
        println!(
            "Would replace {} translation(s):",
            preview.replacements.len() + preview.confirmations.len()
        );
        for change in preview.replacements.iter().chain(&preview.confirmations) {
            let truncate = |text: &str| {
                let mut chars = text.chars();
                let mut short: String = chars.by_ref().take(79).collect();
                if chars.next().is_some() {
                    short.push('…');
                }
                short
            };
            println!(
                "  {}: {:?} ({}) -> {:?}",
                change.id,
                truncate(&change.previous),
                change.previous_status,
                truncate(&change.new_text)
            );
        }
        println!("Would fill {} empty row(s)", preview.fill_ids.len());
        preview.report
    } else {
        db.save_imported_translations_batch_with(updates, keep_existing)
            .await?
    };
    let (imported, skipped) = export::import_counts_after_batch(
        pre_skipped,
        attempted - report.kept_existing,
        report.imported,
    );

    if !dry_run {
        println!(
            "Imported {} translations from {}",
            imported,
            input.display()
        );
    }
    if report.unchanged > 0 {
        println!("Unchanged {} (already identical)", report.unchanged);
    }
    if report.kept_existing > 0 {
        if dry_run {
            println!("Kept {} (--keep-existing)", report.kept_existing);
        } else {
            println!(
                "Kept {} existing translation(s) (--keep-existing)",
                report.kept_existing
            );
        }
    }
    if skipped > 0 {
        println!("Skipped {skipped}: {} outdated source(s), {} unknown id(s), {pre_skipped} empty/missing/fuzzy entries", report.stale_sources, report.unknown_ids);
    }
    if dry_run {
        println!("Dry run: nothing was saved.");
    }
    Ok(())
}

#[cfg(test)]
mod glossary_import_tests {
    use super::*;
    use locust_core::database::{sha256_path, Database};

    fn entry(term: &str, translation: &str, pair: &str) -> GlossaryEntry {
        GlossaryEntry {
            term: term.into(),
            translation: translation.into(),
            lang_pair: pair.into(),
            context: None,
            case_sensitive: false,
        }
    }

    fn import(project: &Path, input: &Path, overwrite: bool) -> anyhow::Result<()> {
        cmd_glossary(GlossaryCommands::Import {
            project: project.into(),
            input: input.into(),
            overwrite,
        })
    }

    fn write_entries(path: &Path, entries: &[GlossaryEntry]) {
        std::fs::write(path, serde_json::to_vec(entries).unwrap()).unwrap();
    }

    fn assert_entries(project: &Path, pair: &str, expected: &[GlossaryEntry]) {
        let db = Database::open(project).unwrap();
        let mut actual = db.get_glossary(pair).unwrap();
        actual.sort_by(|a, b| a.term.cmp(&b.term));
        let mut expected = expected.to_vec();
        expected.sort_by(|a, b| a.term.cmp(&b.term));
        assert_eq!(
            serde_json::to_value(actual).unwrap(),
            serde_json::to_value(expected).unwrap()
        );
    }

    #[test]
    fn bulk_import_preserves_unicode_case_context_and_language_pairs() {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("project.db");
        drop(Database::open(&project).unwrap());
        let input = dir.path().join("glossary.json");
        let entries = [
            GlossaryEntry {
                context: Some("王国の名前 — organización".into()),
                case_sensitive: true,
                ..entry("王国", "Kingdom", "ja-en")
            },
            entry("王国", "Reino", "ja-es"),
            entry("İstanbul", "Estambul", "en-es"),
            entry("Guild", "Gremio", "en-es"),
            entry("guild", "cofradía", "en-es"),
        ];
        write_entries(&input, &entries);

        import(&project, &input, false).unwrap();

        assert_entries(&project, "ja-en", &entries[..1]);
        assert_entries(&project, "ja-es", &entries[1..2]);
        assert_entries(&project, "en-es", &entries[2..]);
    }

    #[test]
    fn bulk_import_preserves_conflicts_unless_overwrite_is_explicit() {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("project.db");
        let input = dir.path().join("glossary.json");
        let original = entry("Guild", "Local edit", "en-es");
        let db = Database::open(&project).unwrap();
        db.save_glossary_entry(&original).unwrap();
        drop(db);
        let incoming = [
            GlossaryEntry {
                context: Some("Faction name".into()),
                case_sensitive: true,
                ..entry("Guild", "Gremio", "en-es")
            },
            entry("HP", "PV", "en-es"),
        ];
        write_entries(&input, &incoming);

        import(&project, &input, false).unwrap();
        assert_entries(&project, "en-es", &[original, incoming[1].clone()]);
        import(&project, &input, true).unwrap();
        assert_entries(&project, "en-es", &incoming);

        let connection = rusqlite::Connection::open(&project).unwrap();
        connection
            .execute_batch(
                "CREATE TRIGGER no_glossary_insert BEFORE INSERT ON glossary
                 BEGIN SELECT RAISE(ABORT, 'unexpected glossary insert'); END;
                 CREATE TRIGGER no_glossary_update BEFORE UPDATE ON glossary
                 BEGIN SELECT RAISE(ABORT, 'unexpected glossary update'); END;",
            )
            .unwrap();
        drop(connection);
        let before = sha256_path(&project).unwrap();
        for overwrite in [false, true] {
            import(&project, &input, overwrite).unwrap();
            assert_eq!(sha256_path(&project).unwrap(), before);
        }
    }

    #[test]
    fn bulk_import_rejects_invalid_tail_before_any_destination_write() {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("project.db");
        let input = dir.path().join("glossary.json");
        drop(Database::open(&project).unwrap());
        let before = sha256_path(&project).unwrap();
        let first = serde_json::to_value(entry("Guild", "Gremio", "en-es")).unwrap();
        let invalid_rows = [
            serde_json::json!({"term": "HP", "translation": "PV", "lang_pair": "en-es", "case_sensitive": "true"}),
            serde_json::json!({"term": "HP", "translation": "PV", "lang_pair": "en-es", "case_sensitive": false, "notes": "typo"}),
            serde_json::json!({"term": " ", "translation": "PV", "lang_pair": "en-es", "case_sensitive": false}),
            serde_json::json!({"term": "HP", "translation": "", "lang_pair": "en-es", "case_sensitive": false}),
            serde_json::json!({"term": "HP", "translation": "PV", "lang_pair": "", "case_sensitive": false}),
            serde_json::json!(null),
        ];
        for invalid in invalid_rows {
            std::fs::write(
                &input,
                serde_json::to_vec(&serde_json::json!([first, invalid])).unwrap(),
            )
            .unwrap();
            assert!(import(&project, &input, true).is_err());
            assert_eq!(sha256_path(&project).unwrap(), before);
            assert_entries(&project, "en-es", &[]);
        }
        for malformed in ["{}", "[", "[{\"term\":\"HP\"},"] {
            std::fs::write(&input, malformed).unwrap();
            assert!(import(&project, &input, true).is_err());
            assert_eq!(sha256_path(&project).unwrap(), before);
        }
    }

    #[test]
    fn bulk_import_rolls_back_the_entire_merge_on_database_error() {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("project.db");
        let input = dir.path().join("glossary.json");
        let original = entry("Guild", "Local edit", "en-es");
        let db = Database::open(&project).unwrap();
        db.save_glossary_entry(&original).unwrap();
        drop(db);
        let connection = rusqlite::Connection::open(&project).unwrap();
        connection
            .execute_batch(
                "CREATE TRIGGER reject_blocked_term BEFORE INSERT ON glossary
                 WHEN NEW.term = 'blocked'
                 BEGIN SELECT RAISE(ABORT, 'blocked glossary term'); END;",
            )
            .unwrap();
        drop(connection);
        write_entries(
            &input,
            &[
                entry("Guild", "Gremio", "en-es"),
                entry("HP", "PV", "en-es"),
                entry("blocked", "fallo", "en-es"),
            ],
        );

        assert!(import(&project, &input, true).is_err());
        assert_entries(&project, "en-es", &[original]);
    }

    #[test]
    fn bulk_import_duplicate_keys_follow_merge_order_and_policy() {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("project.db");
        let input = dir.path().join("glossary.json");
        drop(Database::open(&project).unwrap());
        let entries = [
            entry("HP", "First", "en-es"),
            entry("HP", "Other pair", "en-fr"),
            entry("HP", "Last", "en-es"),
        ];
        write_entries(&input, &entries);

        import(&project, &input, false).unwrap();
        assert_entries(&project, "en-es", &entries[..1]);
        assert_entries(&project, "en-fr", &entries[1..2]);
        import(&project, &input, true).unwrap();
        assert_entries(&project, "en-es", &entries[2..]);
    }

    #[test]
    fn bulk_import_empty_array_is_noop_and_missing_project_is_not_created() {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("project.db");
        let input = dir.path().join("glossary.json");
        drop(Database::open(&project).unwrap());
        std::fs::write(&input, "[]").unwrap();
        let before = sha256_path(&project).unwrap();
        import(&project, &input, false).unwrap();
        assert_eq!(sha256_path(&project).unwrap(), before);

        let missing = dir.path().join("missing.db");
        assert!(import(&missing, &input, false)
            .unwrap_err()
            .to_string()
            .contains("project database not found"));
        for name in [
            "missing.db",
            "missing.db-wal",
            "missing.db-shm",
            "missing.db-journal",
        ] {
            assert!(!dir.path().join(name).exists());
        }
    }
}
