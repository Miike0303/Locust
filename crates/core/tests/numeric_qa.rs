use std::path::PathBuf;

use locust_core::database::Database;
use locust_core::models::{StringEntry, StringStatus, ValidationKind};
use locust_core::validation::{ValidationIssueFilter, Validator};
use serde_json::{json, Value};

fn row(source: &str, translation: &str) -> StringEntry {
    let mut entry = StringEntry::new("numbers", source, PathBuf::from("dialogue.txt"));
    entry.translation = Some(translation.into());
    entry.status = StringStatus::Translated;
    entry
}

fn numeric_issues(source: &str, translation: &str) -> Vec<Value> {
    Validator::validate_entry(&row(source, translation))
        .iter()
        .map(|issue| serde_json::to_value(&issue.kind).unwrap())
        .filter(|kind| kind.get("NumberMismatch").is_some())
        .collect()
}

fn assert_mismatch(source: &str, translation: &str, missing: &[&str], extra: &[&str]) {
    assert_eq!(
        numeric_issues(source, translation),
        vec![json!({
            "NumberMismatch": { "missing": missing, "extra": extra }
        })]
    );
}

#[test]
fn changed_visible_integer_is_advisory_without_rewriting() {
    let entry = row("Collect 12 gems", "Recoge 2 gemas");
    let before = serde_json::to_value(&entry).unwrap();
    assert_mismatch(
        &entry.source,
        entry.translation.as_deref().unwrap(),
        &["12"],
        &["2"],
    );
    assert_eq!(Validator::validate_entry(&entry).len(), 1);
    assert!(entry.require_current_translation().is_ok());
    assert!(entry.require_preserved_translation_controls().is_ok());
    assert_eq!(serde_json::to_value(&entry).unwrap(), before);
}

#[test]
fn fullwidth_digits_match_ascii_digits() {
    assert!(numeric_issues("Collect １２ gems", "Recoge 12 gemas").is_empty());
}

#[test]
fn rpgmaker_control_arguments_are_not_visible() {
    assert!(numeric_issues(r"\V[12] gems", r"\V[12] gemas").is_empty());
}

#[test]
fn translated_level_label_keeps_its_integer() {
    assert!(numeric_issues("Lv 5", "Nv. 5").is_empty());
}

#[test]
fn integer_order_does_not_matter() {
    assert!(numeric_issues("3 of 10", "10 de 3").is_empty());
}

#[test]
fn grouped_integer_is_skipped() {
    assert!(numeric_issues("Collect 1,000 gems", "Recoge gemas").is_empty());
}

#[test]
fn empty_translation_is_skipped() {
    for translation in ["", " \t\n"] {
        assert!(numeric_issues("Collect 12 gems", translation).is_empty());
    }
}

#[test]
fn empty_source_is_skipped() {
    assert!(numeric_issues(" \t", "Recoge 12 gemas").is_empty());
}

#[test]
fn ambiguous_numeric_forms_are_skipped() {
    for source in [
        "1.5",
        "1,5",
        ".5",
        "1,000",
        "1.000",
        "1 000",
        "1\u{a0}000",
        "1\u{202f}000",
        "1 000 000",
        "HP50",
        "2nd",
        "x2",
        "é50",
        "50ñ",
        "stat_50",
        "1-5",
        "1 - 5",
        "1–5",
        "1 — 5",
        "1..5",
        "1…5",
        "1~5",
        "１，０００",
        "１．５",
        "１～５",
    ] {
        assert!(numeric_issues(source, "Sin cifras").is_empty(), "{source}");
    }
}

#[test]
fn duplicate_counts_are_compared_as_a_multiset() {
    assert_mismatch(
        "3 gems, 3 keys, 10 coins",
        "3 gemas, 10 monedas, 10 llaves",
        &["3"],
        &["10"],
    );
}

#[test]
fn integers_can_be_missing_or_extra() {
    assert_mismatch("Collect 12 gems", "Recoge gemas", &["12"], &[]);
    assert_mismatch("Collect gems", "Recoge 2 gemas", &[], &["2"]);
}

#[test]
fn every_recognized_placeholder_family_is_masked() {
    for (source, translation) in [
        (r"\C[2] gems", r"\C[3] gemas"),
        ("{size=24}gems{/size}", "{size=30}gemas{/size}"),
        ("[var] gems", "[other] gemas"),
        ("[scores[12]] gems", "[scores[2]] gemas"),
        ("%2d gems", "%3d gemas"),
        ("{0:12} gems", "{0:2} gemas"),
        ("<span size=24>gems</span>", "<span size=2>gemas</span>"),
        ("{PL_12} gems", "{PL_2} gemas"),
    ] {
        assert!(numeric_issues(source, translation).is_empty(), "{source}");
    }
}

#[test]
fn visible_numbers_survive_control_masking() {
    assert_mismatch(
        r"\V[12] Collect {size=24}12{/size} gems",
        r"\V[2] Recoge {size=30}2{/size} gemas",
        &["12"],
        &["2"],
    );
}

#[test]
fn mismatch_details_have_a_stable_order() {
    assert_mismatch(
        "7 then 12 then 7",
        "4 y 2 y 4",
        &["12", "7", "7"],
        &["2", "4", "4"],
    );
    assert_mismatch(
        "7 then 7 then 12",
        "4 y 4 y 2",
        &["12", "7", "7"],
        &["2", "4", "4"],
    );
}

#[test]
fn numeric_payload_roundtrips_without_changing_legacy_kinds() {
    for value in [
        json!({"NumberMismatch": {"missing": ["12", "12"], "extra": ["2"]}}),
        json!("IdenticalToSource"),
        json!("InvalidInjectionProvenance"),
        json!({"InconsistentTranslation": {"variants": 2}}),
    ] {
        let kind: ValidationKind = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(serde_json::to_value(kind).unwrap(), value);
    }
}

#[tokio::test]
async fn numeric_issues_persist_ignore_revalidate_and_resolve() {
    let db = Database::open_in_memory().unwrap();
    let mut entries = [row("7 then 12 then 7", "4 y 2 y 4")];
    db.save_entries(&entries).unwrap();
    let report = Validator::validate_and_save(&entries, &db).await.unwrap();
    assert_eq!(report.by_kind.get("NumberMismatch"), Some(&1));
    let filter = ValidationIssueFilter {
        kind: Some("NumberMismatch".into()),
        ..Default::default()
    };
    let page = db.list_validation_issues(&filter).unwrap();
    assert_eq!(page.total, 1);
    assert_eq!(
        page.issues[0].kind_data,
        json!({"NumberMismatch": {"missing": ["12", "7", "7"], "extra": ["2", "4", "4"]}})
    );
    db.set_validation_issue_ignored(page.issues[0].id, true)
        .unwrap();
    entries[0].source = "7 then 7 then 12".into();
    entries[0].translation = Some("4 y 4 y 2".into());
    Validator::validate_and_save(&entries, &db).await.unwrap();
    assert_eq!(db.list_validation_issues(&filter).unwrap().total, 0);
    assert_eq!(
        db.validation_summary().unwrap()["NumberMismatch"].ignored,
        1
    );
    let all = db
        .list_validation_issues(&ValidationIssueFilter {
            include_ignored: true,
            ..filter.clone()
        })
        .unwrap();
    db.set_validation_issue_ignored(all.issues[0].id, false)
        .unwrap();
    assert_eq!(db.get_validation_issues(None).unwrap().len(), 1);
    entries[0].translation = Some("7 y 12 y 7".into());
    Validator::validate_and_save(&entries, &db).await.unwrap();
    assert_eq!(db.list_validation_issues(&filter).unwrap().total, 0);
}

#[test]
fn controls_do_not_create_word_boundaries_in_visible_text() {
    assert!(numeric_issues("HP{b}50{/b}", "Sin cifras").is_empty());
    assert!(numeric_issues("1{b}2{/b} gems", "12 gemas").is_empty());
}

#[test]
fn ambiguous_tokens_do_not_become_spurious_extras_on_the_other_side() {
    assert!(numeric_issues("好感度１４以上", "Affection 14 or higher").is_empty());
    assert!(numeric_issues("HP50", "HP 50").is_empty());
    assert!(numeric_issues("HP 50", "HP50").is_empty());
    assert_mismatch(
        "好感度１４以上, collect 12 gems",
        "Affection 14 or higher, recoge 2 gemas",
        &["12"],
        &["2"],
    );
}
