use std::path::PathBuf;

use comfy_table::{Cell, Table};
use locust_core::database::{EntryFilter, TranslationBatchItem, TRANSLATION_CONFLICT_MESSAGE};
use locust_core::validation::Validator;

use crate::util::{format_duration, format_observed_cost, open_existing_project};

pub(crate) fn cmd_validate(project: PathBuf, json: bool) -> anyhow::Result<()> {
    let db = open_existing_project(&project)?;
    let entries = db.get_entries(&EntryFilter::default())?;
    let issues = Validator::validate_all(&entries);

    if json {
        println!(
            "{}",
            serde_json::json!({ "count": issues.len(), "issues": issues })
        );
        if issues.is_empty() {
            return Ok(());
        }
        std::process::exit(1);
    }

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

pub(crate) async fn cmd_replace(
    project: PathBuf,
    find: String,
    replace: String,
    case_sensitive: bool,
    dry_run: bool,
) -> anyhow::Result<()> {
    if find.is_empty() {
        anyhow::bail!("--find must not be empty");
    }
    let db = open_existing_project(&project)?;
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

    let mut updates = Vec::new();
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
        updates.push(TranslationBatchItem {
            id: e.id.clone(),
            translation: next,
            expected_translation: Some(e.translation.clone()),
        });
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
        for update in updates.iter().take(10) {
            println!(
                "  {} → {}",
                update.id,
                truncate_preview(&update.translation, 80)
            );
        }
        if updates.len() > 10 {
            println!("  … and {} more", updates.len() - 10);
        }
        return Ok(());
    }

    let report = db
        .save_translations_batch_if_unchanged(updates, "cli-replace")
        .await?;
    println!(
        "Updated {} string(s) in {}. {} skipped; {} conflict(s).",
        report.applied,
        project.display(),
        report.skipped,
        report.conflicts.len()
    );
    if report.skipped == 0 {
        println!("Replaced {occurrences} occurrence(s).");
    }
    if !report.conflicts.is_empty() {
        println!("Conflicting rows left untouched ({TRANSLATION_CONFLICT_MESSAGE}). Run replace --dry-run again before retrying.");
    }
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

pub(crate) fn cmd_stats(project: PathBuf, by_file: bool) -> anyhow::Result<()> {
    let db = open_existing_project(&project)?;
    if by_file {
        let files = db.get_file_stats()?;
        if files.is_empty() {
            println!("No strings in this project yet.");
            return Ok(());
        }
        let mut table = Table::new();
        table.set_header(vec![
            "File",
            "Total",
            "Pending",
            "Translated",
            "Reviewed",
            "Approved",
            "Error",
        ]);
        let mut totals = [0usize; 6];
        for file in files {
            let counts = [
                file.total,
                file.pending,
                file.translated,
                file.reviewed,
                file.approved,
                file.error,
            ];
            for (total, count) in totals.iter_mut().zip(counts) {
                *total += count;
            }
            let mut row = vec![file.file_path];
            row.extend(counts.map(|count| count.to_string()));
            table.add_row(row);
        }
        let mut total_row = vec!["TOTAL".to_string()];
        total_row.extend(totals.map(|count| count.to_string()));
        table.add_row(total_row);
        println!("{table}");
        return Ok(());
    }
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
    let mut cost_is_complete = true;
    for run in &runs {
        table.add_row(vec![
            run.started_at.chars().take(16).collect::<String>(),
            run.provider.clone(),
            format!("{}→{}", run.source_lang, run.target_lang),
            run.strings_translated.to_string(),
            run.tokens_used.to_string(),
            run.input_tokens.to_string(),
            run.output_tokens.to_string(),
            format_observed_cost(run.cost_usd, run.cost_is_complete),
            format_duration(run.duration_secs),
        ]);
        t_strings += run.strings_translated;
        t_tokens += run.tokens_used;
        t_in += run.input_tokens;
        t_out += run.output_tokens;
        t_cost += run.cost_usd;
        cost_is_complete &= run.cost_is_complete;
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
        format_observed_cost(t_cost, cost_is_complete),
        format_duration(t_secs),
    ]);
    println!("{table}");
    Ok(())
}

pub(crate) fn cmd_font_check(path: PathBuf, text_file: PathBuf) -> anyhow::Result<()> {
    let text = std::fs::read_to_string(text_file)?;
    let audit = if path.is_dir() {
        locust_core::font_validation::FontValidator::audit_game_fonts(&path, &[&text])?
    } else {
        locust_core::font_validation::FontAuditReport {
            fonts: locust_core::font_validation::FontValidator::check_all_faces(&path, &[&text])?,
            issues: Vec::new(),
        }
    };
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "fonts": audit.fonts, "issues": audit.issues,
            "limitations": locust_core::font_validation::FONT_COVERAGE_LIMITATION,
        }))?
    );
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn cmd_font_patch(
    game_path: PathBuf,
    font: PathBuf,
    target: String,
    engine: String,
    lang: String,
    text_file: PathBuf,
    base_patch: Option<PathBuf>,
    output: PathBuf,
) -> anyhow::Result<()> {
    let text = std::fs::read_to_string(text_file)?;
    let (mut archive, mut report) = locust_core::font_patch::build_font_patch(
        &game_path,
        &font,
        &target,
        &engine,
        &lang,
        &[&text],
    )?;
    if let Some(base) = base_patch {
        archive =
            locust_core::font_patch::combine_font_patch(&game_path, &base, &archive, &mut report)?;
    } else {
        eprintln!("Standalone font patch: to include translated text, use --base-patch <translation.zip>. Locust tracks one applied patch per game; do not force a standalone font patch over an installed translation patch.");
    }
    locust_core::font_patch::write_font_patch(&output, &archive)?;
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
