use locust_core::models::{StringEntry, StringStatus, ValidationKind};
use locust_core::validation::Validator;

fn assert_empty_translation_counts(status: StringStatus, expected: [usize; 5]) {
    let translations = [None, Some(""), Some("   "), Some("\n\t"), Some("Hola")];
    let actual = translations.map(|translation| {
        let mut entry = StringEntry::new("row", "Hello", "strings.json".into());
        entry.status = status.clone();
        entry.translation = translation.map(str::to_owned);

        Validator::validate_entry(&entry)
            .iter()
            .filter(|issue| matches!(issue.kind, ValidationKind::EmptyTranslation))
            .count()
    });

    assert_eq!(
        actual, expected,
        "{status:?}: EmptyTranslation counts for {translations:?}"
    );
}

#[test]
fn pending_does_not_flag_empty_translations() {
    assert_empty_translation_counts(StringStatus::Pending, [0, 0, 0, 0, 0]);
}

#[test]
fn translated_flags_only_missing_or_blank_translations() {
    assert_empty_translation_counts(StringStatus::Translated, [1, 1, 1, 1, 0]);
}

#[test]
fn reviewed_flags_only_missing_or_blank_translations() {
    assert_empty_translation_counts(StringStatus::Reviewed, [1, 1, 1, 1, 0]);
}

#[test]
fn approved_flags_only_missing_or_blank_translations() {
    assert_empty_translation_counts(StringStatus::Approved, [1, 1, 1, 1, 0]);
}

#[test]
fn error_does_not_flag_empty_translations() {
    assert_empty_translation_counts(StringStatus::Error, [0, 0, 0, 0, 0]);
}
