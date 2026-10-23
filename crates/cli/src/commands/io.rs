use std::io::Read;
use std::path::PathBuf;
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
    use locust_core::models::StringStatus;

    let db = open_existing_project(&project)?;
    export::check_export_destination(&output, &project)?;
    if output.try_exists()? && !overwrite {
        anyhow::bail!(
            "{} already exists; pass --overwrite to replace it",
            output.display()
        );
    }
    let statuses = status
        .iter()
        .map(|state| {
            state.parse::<StringStatus>().map_err(|_| {
                anyhow::anyhow!(
                    "invalid export status '{state}'; valid values: pending, translated, reviewed, approved, error"
                )
            })
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    // Extraction stores paths verbatim; EntryFilter uses SQL equality, keeping
    // separators, case and dot components exactly as stored in strings.file_path.
    let mut entries = db.get_entries(&EntryFilter {
        file_path: file.clone(),
        ..EntryFilter::default()
    })?;
    if entries.is_empty() {
        if let Some(file) = file {
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
    // Retain the database's ID order for any union of states, including repeats.
    if !statuses.is_empty() {
        entries.retain(|entry| statuses.contains(&entry.status));
    }
    if entries.is_empty() {
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

pub(crate) async fn cmd_import(
    project: PathBuf,
    format: String,
    lang: String,
    input: PathBuf,
    source_lang: Option<String>,
    keep_existing: bool,
    dry_run: bool,
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

    let (updates, pre_skipped) = match format.as_str() {
        "po" => {
            let entries = export::import_po(&content)?;
            export::po_entries_for_batch(&entries)
        }
        "xliff" => {
            let units = export::import_xliff(&content)?;
            export::xliff_units_for_batch(&units)
        }
        "tmx" => {
            let source_lang =
                source_lang.ok_or_else(|| anyhow::anyhow!("--source-lang is required for TMX"))?;
            let units = export::import_tmx(&content, &source_lang, &lang)?;
            export::tmx_units_for_batch(&units)
        }
        _ => anyhow::bail!(
            "unsupported import format: {}. Use 'po', 'xliff' or 'tmx'",
            format
        ),
    };
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
