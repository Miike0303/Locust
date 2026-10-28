use std::path::PathBuf;

use locust_core::database::Database;
use locust_core::models::{
    StringEntry, StringStatus, ValidationIssue, ValidationKind, STALE_TRANSLATION_METADATA_KEY,
};
use locust_core::validation::{ValidationIssueFilter, Validator};
use serde_json::json;

fn row(id: &str, source: &str, translation: &str) -> StringEntry {
    let mut entry = StringEntry::new(id, source, PathBuf::from(format!("{id}.txt")));
    entry.translation = Some(translation.into());
    entry.status = StringStatus::Translated;
    entry
}

fn consistency_issues(entries: &[StringEntry]) -> Vec<ValidationIssue> {
    Validator::validate_all(entries)
        .into_iter()
        .filter(|issue| {
            serde_json::to_value(&issue.kind)
                .unwrap()
                .get("InconsistentTranslation")
                .is_some()
        })
        .collect()
}

fn assert_variants(issues: &[ValidationIssue], variants: usize, common: &str) {
    for issue in issues {
        assert_eq!(
            serde_json::to_value(&issue.kind).unwrap(),
            json!({"InconsistentTranslation": {"variants": variants}})
        );
        assert!(issue.message.contains(&variants.to_string()));
        assert!(issue.message.contains(common), "{}", issue.message);
    }
}

#[test]
fn reports_every_member_project_wide_without_rewriting_translations() {
    let mut entries = Vec::new();
    for index in 0..42 {
        entries.push(row(
            &format!("save-{index}"),
            "Save",
            if index < 40 { "Guardar" } else { "Salvar" },
        ));
    }
    let before = serde_json::to_value(&entries).unwrap();
    let issues = consistency_issues(&entries);
    assert_eq!(issues.len(), 42);
    assert_variants(&issues, 2, "Guardar");
    assert_eq!(
        issues
            .iter()
            .map(|issue| &issue.entry_id)
            .collect::<Vec<_>>(),
        entries.iter().map(|entry| &entry.id).collect::<Vec<_>>()
    );
    assert_eq!(serde_json::to_value(&entries).unwrap(), before);
}

#[test]
fn trims_only_outer_whitespace_of_sources_and_translations() {
    let entries = [
        row("a", " \tSave\n", " Guardar "),
        row("b", "Save", "Guardar\t"),
        row("c", "Save ", "Salvar"),
        row("case", "save", "Otro"),
        row("inside", "Sa ve", "Otro"),
    ];
    let issues = consistency_issues(&entries);
    assert_eq!(issues.len(), 3);
    assert_variants(&issues, 2, "Guardar");
    let entries = [
        row("a", "Same", "A B"),
        row("b", "Same", "A  B"),
        row("c", "Same", "a b"),
    ];
    let issues = consistency_issues(&entries);
    assert_eq!(issues.len(), 3);
    assert_variants(&issues, 3, "A  B");
}

#[test]
fn only_current_completed_nonblank_rows_participate() {
    let mut entries = vec![row("translated", "Save", "Guardar")];
    for (id, status, translation) in [
        ("reviewed", StringStatus::Reviewed, "Guardar"),
        ("approved", StringStatus::Approved, "Salvar"),
        ("pending", StringStatus::Pending, "Otro"),
        ("error", StringStatus::Error, "Otro"),
        ("blank", StringStatus::Translated, " \t\n"),
    ] {
        let mut entry = row(id, "Save", translation);
        entry.status = status;
        entries.push(entry);
    }
    let mut absent = row("absent", "Save", "");
    absent.translation = None;
    entries.push(absent);
    let mut stale = row("stale", "Save", "Otro");
    stale.metadata.insert(
        STALE_TRANSLATION_METADATA_KEY.into(),
        json!({"version": 1, "translated_source_sha256": "a".repeat(64),
            "current_source_sha256": stale.source_hash()}),
    );
    entries.push(stale);
    let mut malformed = row("malformed", "Save", "Otro");
    malformed
        .metadata
        .insert(STALE_TRANSLATION_METADATA_KEY.into(), json!(true));
    entries.push(malformed);
    let issues = consistency_issues(&entries);
    assert_eq!(
        issues
            .iter()
            .map(|issue| issue.entry_id.as_str())
            .collect::<Vec<_>>(),
        ["translated", "reviewed", "approved"]
    );
    assert_variants(&issues, 2, "Guardar");
}

#[test]
fn consistent_unique_and_ineligible_variants_are_not_reported() {
    let mut pending = row("pending", "Save", "Salvar");
    pending.status = StringStatus::Pending;
    assert!(consistency_issues(&[
        row("a", "Save", " Guardar"),
        row("b", "Save ", "Guardar "),
        row("unique", "Load", "Cargar"),
        row("blank", "Save", "  "),
        pending,
    ])
    .is_empty());
    assert!(consistency_issues(&[]).is_empty());
}

#[test]
fn explicit_context_separates_rows_but_same_context_still_checks() {
    let entries = [
        row("menu-a", "Save", "Guardar").with_context("menu"),
        row("menu-b", "Save", "Salvar").with_context(" menu "),
        row("dialogue", "Save", "Rescatar").with_context("dialogue"),
        row("unknown-a", "Save", "Conservar"),
        row("unknown-b", "Save", "Conservar").with_context(" \t"),
    ];
    let issues = consistency_issues(&entries);
    assert_eq!(issues.len(), 2);
    assert_variants(&issues, 2, "Guardar");
}

#[test]
fn semantic_metadata_separates_rows_without_using_file_or_locator_metadata() {
    for key in ["context", "speaker", "speaker_name", "character"] {
        let mut entries = [
            row("a", "Hello", "Hola"),
            row("b", "Hello", "Buenas"),
            row("c", "Hello", "Saludos"),
        ];
        for (entry, value) in entries.iter_mut().zip(["Alice", "Bob", " Alice "]) {
            entry.metadata.insert(key.into(), json!(value));
            entry.metadata.insert("line_index".into(), json!(entry.id));
        }
        let issues = consistency_issues(&entries);
        assert_eq!(issues.len(), 2, "{key}");
        assert_eq!(issues[0].entry_id, "a");
        assert_eq!(issues[1].entry_id, "c");
    }
    let mut a = row("a", "Hello", "Hola");
    let mut b = row("b", "Hello", "Buenas");
    a.metadata.insert("line_index".into(), json!(1));
    b.metadata.insert("line_index".into(), json!(2));
    b.metadata.insert("speaker".into(), json!(" \t"));
    assert_eq!(consistency_issues(&[a, b]).len(), 2);
}

#[test]
fn dialogic_speaker_metadata_separates_context_dependent_dialogue() {
    let mut entries = [row("a", "Hello", "Hola"), row("b", "Hello", "Buenas")];
    for (entry, speaker) in entries.iter_mut().zip(["Alice", "Bob"]) {
        entry.metadata.insert(
            "godot_dialogic".into(),
            json!({"locator_version": 1, "kind": "text", "speaker": speaker}),
        );
    }
    assert!(consistency_issues(&entries).is_empty());
    entries[1].metadata.get_mut("godot_dialogic").unwrap()["speaker"] = json!("Alice");
    assert_eq!(consistency_issues(&entries).len(), 2);
}

#[test]
fn context_and_speaker_are_independent_grouping_axes() {
    let mut entries = [
        row("a", "Hello", "Hola").with_context("menu"),
        row("b", "Hello", "Buenas").with_context("menu"),
        row("c", "Hello", "Saludos").with_context("dialogue"),
        row("d", "Hello", "Qué tal").with_context("menu"),
    ];
    for (entry, speaker) in entries.iter_mut().zip(["Alice", "Bob", "Alice", "Alice"]) {
        entry.metadata.insert("speaker".into(), json!(speaker));
    }
    let issues = consistency_issues(&entries);
    assert_eq!(issues.len(), 2);
    assert_eq!(issues[0].entry_id, "a");
    assert_eq!(issues[1].entry_id, "d");
}

#[test]
fn most_common_ties_use_lexical_order_and_output_follows_input_order() {
    let mut entries = vec![
        row("z", "Save", "Salvar"),
        row("b", "Load", "Leer"),
        row("a", "Save", "Guardar"),
        row("c", "Load", "Cargar"),
    ];
    for _ in 0..4 {
        let issues = consistency_issues(&entries);
        assert_eq!(issues.len(), 4);
        for (entry, issue) in entries.iter().zip(&issues) {
            assert_eq!(issue.entry_id, entry.id);
            assert_variants(
                std::slice::from_ref(issue),
                2,
                if entry.source == "Save" {
                    "Guardar"
                } else {
                    "Cargar"
                },
            );
        }
        entries.reverse();
    }
}

#[test]
fn new_kind_uses_default_serde_without_changing_existing_kinds() {
    let value = json!({"InconsistentTranslation": {"variants": 2}});
    let kind: ValidationKind = serde_json::from_value(value.clone()).unwrap();
    assert_eq!(serde_json::to_value(kind).unwrap(), value);
    assert_eq!(
        serde_json::to_value(ValidationKind::IdenticalToSource).unwrap(),
        json!("IdenticalToSource")
    );
}

#[tokio::test]
async fn advisory_issues_flow_through_summary_filter_ignore_and_rerun() {
    let db = Database::open_in_memory().unwrap();
    let mut entries = [row("a", "Save", "Guardar"), row("b", "Save", "Salvar")];
    db.save_entries(&entries).unwrap();
    let report = Validator::validate_and_save(&entries, &db).await.unwrap();
    assert_eq!(report.issues_found, 2);
    assert_eq!(report.entries_with_issues, 2);
    assert_eq!(report.by_kind["InconsistentTranslation"], 2);
    assert!(report
        .issues
        .iter()
        .all(|issue| issue.source.as_deref() == Some("Save")));
    let filter = ValidationIssueFilter {
        kind: Some("InconsistentTranslation".into()),
        ..Default::default()
    };
    let page = db.list_validation_issues(&filter).unwrap();
    assert_eq!(page.total, 2);
    assert_eq!(page.issues[0].translation.as_deref(), Some("Guardar"));
    db.set_validation_issue_ignored(page.issues[0].id, true)
        .unwrap();
    entries[0].translation = Some("Conservar".into());
    Validator::validate_and_save(&entries, &db).await.unwrap();
    assert_eq!(db.list_validation_issues(&filter).unwrap().total, 1);
    let summary = db.validation_summary().unwrap();
    assert_eq!(summary["InconsistentTranslation"].open, 1);
    assert_eq!(summary["InconsistentTranslation"].ignored, 1);
    let all = db
        .list_validation_issues(&ValidationIssueFilter {
            include_ignored: true,
            ..filter
        })
        .unwrap();
    assert!(all.issues[0].ignored);
    db.set_validation_issue_ignored(all.issues[0].id, false)
        .unwrap();
    assert_eq!(
        db.validation_summary().unwrap()["InconsistentTranslation"].open,
        2
    );
    assert_eq!(db.get_validation_issues(None).unwrap().len(), 2);
}

#[test]
fn completes_for_100k_rows_with_many_groups_and_variants() {
    let entries: Vec<_> = (0..100_000)
        .map(|index| {
            row(
                &format!("entry-{index}"),
                &format!("Source {}", index % 1_000),
                if index < 50_000 { "Uno" } else { "Dos" },
            )
        })
        .collect();
    let issues = consistency_issues(&entries);
    assert_eq!(issues.len(), 100_000);
    assert_variants(&issues, 2, "Dos");
    assert_eq!(issues.first().unwrap().entry_id, "entry-0");
    assert_eq!(issues.last().unwrap().entry_id, "entry-99999");
}

#[test]
fn completes_for_100k_distinct_variants_in_one_group() {
    let entries: Vec<_> = (0..100_000)
        .map(|index| {
            row(
                &format!("entry-{index}"),
                "Save",
                &format!("Variant {index:06}"),
            )
        })
        .collect();
    let issues = consistency_issues(&entries);
    assert_eq!(issues.len(), 100_000);
    assert_variants(&issues, 100_000, "Variant 000000");
}
