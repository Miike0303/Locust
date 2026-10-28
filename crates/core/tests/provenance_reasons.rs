use std::path::PathBuf;

use locust_core::database::Database;
use locust_core::models::{
    StringEntry, StringStatus, ValidationIssue, ValidationKind, INJECTION_CAPACITY_METADATA_KEY,
    INJECTION_SOURCE_METADATA_KEY,
};
use locust_core::textasset_group::{self as group, GroupKind};
use locust_core::validation::{binary_slot_budget, ValidationIssueFilter, Validator};
use serde_json::{json, Value};

fn entry() -> StringEntry {
    let mut entry = StringEntry::new("row", "Hello", PathBuf::from("strings.assets"));
    entry.translation = Some("Hola".into());
    entry.status = StringStatus::Translated;
    entry
}

fn grouped_entry() -> StringEntry {
    let mut entry = entry();
    entry.source = "Hi".into();
    entry.translation = Some("Yo".into());
    entry
        .metadata
        .insert("extraction_method".into(), json!("textasset_loc_line"));
    entry.metadata.insert("loc_key".into(), json!("Menu.A"));
    entry.metadata.insert("line_index".into(), json!(0));
    let original = "Menu.A: Hi\n";
    group::attach_to_entries(
        std::slice::from_mut(&mut entry),
        GroupKind::LocLine,
        original,
        original.len(),
        "group",
    );
    entry
}

fn assert_reason(entry: &StringEntry, reason: Value, message: &str) {
    let issues = Validator::validate_entry(entry);
    let issues: Vec<_> = issues
        .iter()
        .filter(|issue| {
            matches!(
                issue.kind,
                ValidationKind::InvalidInjectionProvenance { .. }
            )
        })
        .collect();
    assert!(!issues.is_empty(), "missing provenance issue: {entry:?}");
    for issue in issues {
        assert_eq!(
            serde_json::to_value(&issue.kind).unwrap(),
            json!({"InvalidInjectionProvenance": {"reason": reason}})
        );
        assert_eq!(issue.message, message);
    }
}

#[test]
fn malformed_physical_source_and_capacity_have_typed_reasons() {
    for (key, value, field) in [
        (
            INJECTION_SOURCE_METADATA_KEY,
            Value::Null,
            INJECTION_SOURCE_METADATA_KEY.to_string(),
        ),
        (
            INJECTION_SOURCE_METADATA_KEY,
            json!(""),
            INJECTION_SOURCE_METADATA_KEY.to_string(),
        ),
        (
            INJECTION_CAPACITY_METADATA_KEY,
            json!([]),
            INJECTION_CAPACITY_METADATA_KEY.to_string(),
        ),
        (
            INJECTION_CAPACITY_METADATA_KEY,
            json!({}),
            format!("{INJECTION_CAPACITY_METADATA_KEY}.encoding"),
        ),
        (
            INJECTION_CAPACITY_METADATA_KEY,
            json!({"encoding": "utf8"}),
            format!("{INJECTION_CAPACITY_METADATA_KEY}.bytes"),
        ),
        (
            INJECTION_CAPACITY_METADATA_KEY,
            json!({"encoding": "utf8", "bytes": -1}),
            format!("{INJECTION_CAPACITY_METADATA_KEY}.bytes"),
        ),
    ] {
        let mut entry = entry();
        entry.metadata.insert(key.into(), value);
        assert_reason(
            &entry,
            json!({"MalformedMetadata": {"field": field}}),
            &format!("entry 'row' has malformed {field} metadata"),
        );
    }
    assert!(Validator::validate_entry(&entry()).is_empty());
}

#[test]
fn binary_capacity_errors_keep_parameters_and_english_diagnostics() {
    for (encoding, capacity, reason, message) in [
        (
            "utf8",
            0,
            json!({"InvalidCapacity": {"encoding": "utf8", "capacity": 0}}),
            "entry 'row' has invalid immutable binary capacity",
        ),
        (
            "future",
            5,
            json!({"InvalidCapacity": {"encoding": "future", "capacity": 5}}),
            "entry 'row' has invalid immutable binary capacity",
        ),
        (
            "utf16le",
            10,
            json!({"ConflictingEncodings": {"slot_encoding": "utf8", "capacity_encoding": "utf16le"}}),
            "entry 'row' has conflicting binary slot encodings",
        ),
        (
            "utf8",
            100,
            json!({"CapacitySourceMismatch": {"encoding": "utf8", "capacity": 100}}),
            "entry 'row' has binary capacity inconsistent with its physical source",
        ),
    ] {
        let mut entry = entry();
        entry.metadata.insert("binary_slot".into(), json!("utf8"));
        entry.metadata.insert(
            INJECTION_CAPACITY_METADATA_KEY.into(),
            json!({"encoding": encoding, "bytes": capacity}),
        );
        assert_eq!(binary_slot_budget(&entry).unwrap_err().to_string(), message);
        assert_reason(&entry, reason, message);
    }
    let mut entry = entry();
    entry.source = "😀".into();
    entry.metadata.insert("binary_slot".into(), json!("sjis"));
    assert_reason(
        &entry,
        json!({"UnencodableSource": {"encoding": "sjis"}}),
        "entry 'row' binary source cannot be encoded as sjis",
    );
}

#[test]
fn grouped_binary_metadata_keeps_its_checks_without_inventing_cell_budgets() {
    for (encoding, bytes, reason) in [
        (
            "utf8",
            0,
            json!({"InvalidCapacity": {"encoding": "utf8", "capacity": 0}}),
        ),
        (
            "utf16le",
            4,
            json!({"ConflictingEncodings": {"slot_encoding": "utf8", "capacity_encoding": "utf16le"}}),
        ),
    ] {
        let mut entry = grouped_entry();
        entry.metadata.insert("binary_slot".into(), json!("utf8"));
        entry.metadata.insert(
            INJECTION_CAPACITY_METADATA_KEY.into(),
            json!({"encoding": encoding, "bytes": bytes}),
        );
        assert_reason(
            &entry,
            reason,
            if bytes == 0 {
                "entry 'row' has invalid immutable binary capacity"
            } else {
                "entry 'row' has conflicting binary slot encodings"
            },
        );
    }
    let mut entry = grouped_entry();
    entry.metadata.insert(
        INJECTION_CAPACITY_METADATA_KEY.into(),
        json!({"encoding": "utf8", "bytes": 100}),
    );
    assert!(binary_slot_budget(&entry).unwrap().is_none());
}

#[test]
fn textasset_metadata_failures_are_structured_at_the_parser() {
    for (key, value, reason, message) in [
        (group::GROUP_KIND_KEY, None, json!({"MissingMetadata": {"field": group::GROUP_KIND_KEY}}),
            "entry 'row' is missing textasset_group_kind; cannot verify shared TextAsset capacity"),
        (group::GROUP_KIND_KEY, Some(json!("future")), json!({"UnsupportedTextAssetLayout": {"layout": "future"}}),
            "entry 'row' has unsupported textasset_group_kind 'future'"),
        (group::GROUP_KIND_KEY, Some(json!("csv")), json!("ConflictingTextAssetMetadata"),
            "entry 'row' has conflicting TextAsset group kinds"),
        (group::GROUP_ID_KEY, None, json!({"MissingMetadata": {"field": group::GROUP_ID_KEY}}),
            "entry 'row' is missing textasset_group_id; cannot verify shared TextAsset capacity"),
        (group::GROUP_CAPACITY_KEY, None, json!({"MissingMetadata": {"field": group::GROUP_CAPACITY_KEY}}),
            "entry 'row' is missing textasset_group_capacity; cannot verify shared TextAsset capacity"),
        (group::GROUP_CAPACITY_KEY, Some(json!(0)), json!({"InvalidCapacity": {"encoding": "utf8", "capacity": 0}}),
            "entry 'row' has invalid shared TextAsset capacity"),
        (group::GROUP_MEMBER_COUNT_KEY, Some(json!(0)), json!({"MalformedMetadata": {"field": group::GROUP_MEMBER_COUNT_KEY}}),
            "entry 'row' has invalid textasset_group_members"),
        (group::GROUP_OVERHEAD_KEY, None, json!({"MissingMetadata": {"field": group::GROUP_OVERHEAD_KEY}}),
            "entry 'row' is missing textasset_group_overhead; cannot compute reconstruction overhead"),
        (group::GROUP_OVERHEAD_KEY, Some(json!(12)), json!({"InvalidTextAssetOverhead": {"overhead": 12, "original_bytes": 11}}),
            "entry 'row' has reconstruction overhead larger than the original blob"),
        (group::GROUP_CAPACITY_KEY, Some(json!(1)), json!({"TextAssetOriginalExceedsCapacity": {"actual": 11, "capacity": 1}}),
            "entry 'row' shared TextAsset original length 11 exceeds capacity 1"),
    ] {
        let mut entry = grouped_entry();
        if let Some(value) = value { entry.metadata.insert(key.into(), value); }
        else { entry.metadata.remove(key); }
        assert_reason(&entry, reason, message);
        assert_eq!(group::parse_group_meta(&entry).unwrap_err(), message);
    }
}

#[test]
fn textasset_original_record_failures_have_reasons() {
    let mut missing = grouped_entry();
    missing.textasset_original = None;
    assert_reason(
        &missing,
        json!("MissingTextAssetOriginal"),
        "missing shared TextAsset original; reload from its project database",
    );
    missing.metadata.remove(group::GROUP_ORIGINAL_REF_KEY);
    missing.metadata.remove(group::GROUP_ORIGINAL_BYTES_KEY);
    assert_reason(
        &missing,
        json!("MissingTextAssetOriginal"),
        "entry 'row' is missing textasset_group_original; cannot reconstruct the shared blob",
    );
    missing
        .metadata
        .insert(group::GROUP_ORIGINAL_KEY.into(), json!(42));
    assert_reason(
        &missing,
        json!({"MalformedMetadata": {"field": group::GROUP_ORIGINAL_KEY}}),
        "malformed TextAsset original",
    );
    for (key, value, reason, message) in [
        (
            group::GROUP_ORIGINAL_REF_KEY,
            Some(json!(42)),
            json!({"MalformedMetadata": {"field": group::GROUP_ORIGINAL_REF_KEY}}),
            "malformed TextAsset original reference",
        ),
        (
            group::GROUP_ORIGINAL_REF_KEY,
            Some(json!("bad-digest")),
            json!("TextAssetOriginalMismatch"),
            "shared TextAsset original digest or length does not match metadata",
        ),
        (
            group::GROUP_ORIGINAL_BYTES_KEY,
            Some(json!(1)),
            json!("TextAssetOriginalMismatch"),
            "shared TextAsset original digest or length does not match metadata",
        ),
        (
            group::GROUP_ORIGINAL_REF_KEY,
            None,
            json!({"MissingMetadata": {"field": group::GROUP_ORIGINAL_REF_KEY}}),
            "TextAsset original length has no digest reference",
        ),
        (
            group::GROUP_ORIGINAL_KEY,
            Some(json!("conflicting")),
            json!("ConflictingTextAssetMetadata"),
            "conflicting inline and shared TextAsset originals",
        ),
    ] {
        let mut entry = grouped_entry();
        if let Some(value) = value {
            entry.metadata.insert(key.into(), value);
        } else {
            entry.metadata.remove(key);
        }
        assert_reason(&entry, reason, message);
        assert_eq!(group::shared_original(&entry).unwrap_err(), message);
    }
}

#[test]
fn automatic_group_bounds_report_the_actual_limits() {
    for (key, value) in [
        (
            group::GROUP_MEMBER_COUNT_KEY,
            json!(group::MAX_GROUP_MEMBERS + 1),
        ),
        (group::GROUP_ERROR_KEY, json!("unsupported")),
        (
            group::GROUP_ORIGINAL_KEY,
            json!("A".repeat(group::MAX_GROUP_ORIGINAL_BYTES + 1)),
        ),
    ] {
        let mut entry = grouped_entry();
        if key == group::GROUP_ORIGINAL_KEY {
            entry.textasset_original = None;
        }
        entry.metadata.insert(key.into(), value);
        let issues = Validator::validate_entry(&entry);
        assert_eq!(issues.len(), 1);
        assert_eq!(
            serde_json::to_value(&issues[0].kind).unwrap(),
            json!({
                "InvalidInjectionProvenance": {"reason": {"TextAssetGroupTooLarge": {
                    "max_members": group::MAX_GROUP_MEMBERS,
                    "max_original_bytes": group::MAX_GROUP_ORIGINAL_BYTES,
                }}}
            })
        );
        assert!(issues[0].message.contains("automatic group too large"));
        assert!(issues[0].message.contains("not a per-cell budget"));
    }
}

#[test]
fn group_producers_cover_conflicts_patch_provenance_and_reconstruction() {
    let a = grouped_entry();
    let mut b = a.clone();
    b.id = "sibling".into();
    b.metadata
        .insert(group::GROUP_OVERHEAD_KEY.into(), json!(0));
    let issues = group::group_validation_issues(&[a, b]);
    assert_eq!(issues.len(), 2);
    for issue in issues {
        assert_eq!(
            serde_json::to_value(&issue.kind).unwrap(),
            json!({
                "InvalidInjectionProvenance": {"reason": "ConflictingTextAssetMetadata"}
            })
        );
        assert_eq!(
            issue.message,
            format!(
                "entry '{}' has conflicting shared TextAsset group metadata",
                issue.entry_id
            )
        );
    }
    let mut entry = grouped_entry();
    entry
        .metadata
        .insert(INJECTION_SOURCE_METADATA_KEY.into(), json!(false));
    let issues = group::group_validation_issues(&[entry]);
    assert_eq!(issues.len(), 1);
    assert_eq!(
        serde_json::to_value(&issues[0].kind).unwrap(),
        json!({
            "InvalidInjectionProvenance": {"reason": {"MalformedMetadata": {"field": INJECTION_SOURCE_METADATA_KEY}}}
        })
    );
    assert_eq!(
        issues[0].message,
        "entry 'row' has malformed locust_injection_source metadata"
    );
    for kind in [GroupKind::LocLine, GroupKind::Csv] {
        let mut entry = grouped_entry();
        if kind == GroupKind::Csv {
            entry
                .metadata
                .insert("extraction_method".into(), json!("textasset_csv_cell"));
            entry.metadata.insert("csv_row".into(), json!(1));
            entry.metadata.insert("csv_col".into(), json!(1));
            let original = "ID,NAME\n1,Hi\n";
            group::attach_to_entries(
                std::slice::from_mut(&mut entry),
                kind,
                original,
                original.len(),
                "csv",
            );
        }
        entry.translation = Some("No\nvale".into());
        let issues = group::group_validation_issues(&[entry]);
        assert_eq!(issues.len(), 1);
        assert_eq!(
            serde_json::to_value(&issues[0].kind).unwrap(),
            json!({
                "InvalidInjectionProvenance": {"reason": {"UnsupportedTextAssetReconstruction": {"layout": kind.as_str()}}}
            })
        );
        assert_eq!(
            issues[0].message,
            "entry 'row' cannot be reconstructed into the supported TextAsset layout"
        );
    }
}

#[tokio::test]
async fn provenance_report_and_stored_api_preserve_reasons() {
    let db = Database::open_in_memory().unwrap();
    let mut entry = entry();
    entry.metadata.insert(
        INJECTION_CAPACITY_METADATA_KEY.into(),
        json!({"encoding": "utf8", "bytes": 100}),
    );
    db.save_entries(std::slice::from_ref(&entry)).unwrap();
    let report = Validator::validate_and_save(&[entry], &db).await.unwrap();
    assert_eq!(report.by_kind["InvalidInjectionProvenance"], 1);
    let expected = json!({"InvalidInjectionProvenance": {"reason": {
        "CapacitySourceMismatch": {"encoding": "utf8", "capacity": 100}
    }}});
    let live = serde_json::to_value(&report).unwrap();
    assert_eq!(live["issues"][0]["kind"], expected);
    let page = db
        .list_validation_issues(&ValidationIssueFilter {
            kind: Some("InvalidInjectionProvenance".into()),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(page.total, 1);
    let stored = serde_json::to_value(&page).unwrap();
    assert_eq!(stored["issues"][0]["kind"], "InvalidInjectionProvenance");
    assert_eq!(stored["issues"][0]["kind_data"], expected);
    assert_eq!(stored["issues"][0]["message"], live["issues"][0]["message"]);
    assert_eq!(
        serde_json::to_value(&db.get_validation_issues(None).unwrap()[0].kind).unwrap(),
        expected
    );
}

#[tokio::test]
async fn every_reason_round_trips_through_storage() {
    let reasons = [
        json!({"MalformedMetadata": {"field": "field"}}),
        json!({"MissingMetadata": {"field": "field"}}),
        json!({"InvalidCapacity": {"encoding": "utf8", "capacity": 0}}),
        json!({"ConflictingEncodings": {"slot_encoding": "utf8", "capacity_encoding": "sjis"}}),
        json!({"CapacitySourceMismatch": {"encoding": "utf8", "capacity": 100}}),
        json!({"UnencodableSource": {"encoding": "sjis"}}),
        json!({"UnsupportedTextAssetLayout": {"layout": "future"}}),
        json!("ConflictingTextAssetMetadata"),
        json!({"TextAssetGroupTooLarge": {"max_members": 16384, "max_original_bytes": 1048576}}),
        json!("MissingTextAssetOriginal"),
        json!("TextAssetOriginalMismatch"),
        json!({"TextAssetOriginalExceedsCapacity": {"actual": 11, "capacity": 1}}),
        json!({"InvalidTextAssetOverhead": {"overhead": 12, "original_bytes": 11}}),
        json!({"UnsupportedTextAssetReconstruction": {"layout": "csv"}}),
    ];
    let issues: Vec<_> = reasons
        .iter()
        .enumerate()
        .map(|(i, reason)| ValidationIssue {
            entry_id: i.to_string(),
            kind: serde_json::from_value(json!({"InvalidInjectionProvenance": {"reason": reason}}))
                .unwrap(),
            message: format!("readable diagnostic {i}"),
            source: None,
        })
        .collect();
    let db = Database::open_in_memory().unwrap();
    db.save_validation_issues(&issues).await.unwrap();
    let page = db.list_validation_issues(&Default::default()).unwrap();
    let legacy = db.get_validation_issues(None).unwrap();
    assert_eq!(page.total, reasons.len());
    for ((stored, read), original) in page.issues.iter().zip(&legacy).zip(&issues) {
        assert_eq!(stored.kind, "InvalidInjectionProvenance");
        assert_eq!(
            stored.kind_data,
            serde_json::to_value(&original.kind).unwrap()
        );
        assert_eq!(serde_json::to_value(&read.kind).unwrap(), stored.kind_data);
        assert_eq!(stored.message, original.message);
    }
}
