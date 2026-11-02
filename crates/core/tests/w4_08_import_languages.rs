//! Language declarations are evidence, never guesses from text or pivot metadata.
use locust_core::database::Database;
use locust_core::export::{self, ImportLanguagePair, ImportLanguageValidation};
use locust_core::models::{StringEntry, StringStatus};

fn pair(source: &str, target: &str) -> ImportLanguagePair {
    ImportLanguagePair {
        source: export::normalize_import_language(source),
        target: export::normalize_import_language(target),
    }
}

fn catalog(format: &str, source: &str, target: &str) -> String {
    let mut row = StringEntry::new("id", "Source", "story.rpy".into());
    row.translation = Some("Imported".into());
    match format {
        "po" => export::export_po(&[row], source, target),
        "xliff" => export::export_xliff(&[row], source, target),
        "tmx" => export::export_tmx(&[row], source, target).unwrap(),
        _ => unreachable!(),
    }
}

fn check(format: &str, content: &str, project: &ImportLanguagePair) -> ImportLanguageValidation {
    let parsed = export::parse_import_catalog(format, content, project).unwrap();
    export::validate_import_languages(&parsed.languages, project).unwrap()
}

#[test]
fn explicit_script_conflicts_refuse_on_both_sides_in_all_formats() {
    for (expected, conflicting) in [
        ("zh-Hans", "zh-Hant"),
        ("sr-Latn", "sr-Cyrl"),
        ("ZH_cmn_HaNs", "zh-cmn-Hant"),
    ] {
        for format in ["po", "xliff", "tmx"] {
            for side in ["source", "target"] {
                if format == "po" && side == "source" {
                    continue;
                }
                let (project, content) = if side == "source" {
                    (pair(expected, "es"), catalog(format, conflicting, "es"))
                } else {
                    (pair("en", expected), catalog(format, "en", conflicting))
                };
                let parsed = export::parse_import_catalog(format, &content, &project).unwrap();
                let error = export::validate_import_languages(&parsed.languages, &project)
                    .expect_err("a different explicit script must refuse before confirmation");
                assert!(error.to_string().contains("does not match project"));
            }
        }
    }
}

#[test]
fn one_sided_script_declarations_require_confirmation_without_region_inference() {
    for (bare, scripted) in [
        ("zh", "zh-Hans"),
        ("sr", "sr-Latn"),
        ("zh-cmn", "zh-cmn-Hans"),
    ] {
        for (expected, declared) in [(bare, scripted), (scripted, bare)] {
            for format in ["po", "xliff", "tmx"] {
                for side in ["source", "target"] {
                    if format == "po" && side == "source" {
                        continue;
                    }
                    let (project, content) = if side == "source" {
                        (pair(expected, "es"), catalog(format, declared, "es"))
                    } else {
                        (pair("en", expected), catalog(format, "en", declared))
                    };
                    let validation = check(format, &content, &project);
                    assert!(
                        validation.requires_confirmation,
                        "{format}/{side}/{expected}"
                    );
                    assert!(validation.require_confirmation(false).is_err());
                    validation.require_confirmation(true).unwrap();
                    assert!(validation.warnings.iter().any(|w| w.code == "undeclared"));
                    assert!(!validation.warnings.iter().any(|w| w.code == "region"));
                }
            }
        }
    }
    for format in ["po", "xliff", "tmx"] {
        let validation = check(
            format,
            &catalog(format, "en", "ZH-hAnS-TW"),
            &pair("en", "zh-Hans-CN"),
        );
        assert!(!validation.requires_confirmation);
        assert_eq!(validation.warnings[0].code, "region");
        for (expected, declared) in [("en-x-latn", "en-x-cyrl"), ("sr-u-co-latn", "sr-u-co-cyrl")] {
            let validation = check(
                format,
                &catalog(format, "ja", declared),
                &pair("ja", expected),
            );
            assert!(
                !validation.requires_confirmation,
                "extension values are not scripts"
            );
        }
    }
}

#[test]
fn partial_xliff_files_cannot_borrow_another_files_language() {
    let project = pair("en", "es");
    let complete = catalog("xliff", "en", "es");
    for content in [
        complete.replace(" target-language=\"es\"", ""),
        complete.replace(
            "</xliff>",
            "<file source-language=\"en\"><body/></file></xliff>",
        ),
        complete.replace(
            "</xliff>",
            "<file target-language=\"es\"><body/></file></xliff>",
        ),
    ] {
        let validation = check("xliff", &content, &project);
        assert!(validation.require_confirmation(false).is_err());
        validation.require_confirmation(true).unwrap();
    }
    for attrs in [
        "source-language=\"fr\" target-language=\"es\"",
        "source-language=\"en\" target-language=\"fr\"",
    ] {
        let content =
            complete.replace("</xliff>", &format!("<file {attrs}><body/></file></xliff>"));
        let parsed = export::parse_import_catalog("xliff", &content, &project).unwrap();
        assert!(export::validate_import_languages(&parsed.languages, &project).is_err());
    }
}

#[test]
fn equivalent_duplicate_po_headers_are_concrete_but_blank_duplicates_are_ambiguous() {
    let project = pair("en", "es-MX");
    let complete = catalog("po", "en", "es-MX");
    let equivalent = complete.replace("Language: es-MX\\n", "Language: es-MX\\nLanguage: ES_mx\\n");
    assert!(!check("po", &equivalent, &project).requires_confirmation);
    // Separate header entries are evidence too; do not silently take the last one.
    for header in [
        "msgid \"\"\nmsgstr \"Language: \\n\"\n\n",
        "msgid \"\"\nmsgstr \"Language: es-ES\\n\"\n\n",
    ] {
        let validation = check("po", &format!("{header}{complete}"), &project);
        assert!(validation.require_confirmation(false).is_err());
        validation.require_confirmation(true).unwrap();
    }
    let conflicting = format!("msgid \"\"\nmsgstr \"Language: fr\\n\"\n\n{complete}");
    let parsed = export::parse_import_catalog("po", &conflicting, &project).unwrap();
    assert!(export::validate_import_languages(&parsed.languages, &project).is_err());
}

#[test]
fn tmx_empty_orientation_warns_but_missing_or_extra_variants_remain_strict_errors() {
    let project = pair("en", "es");
    let complete = catalog("tmx", "en", "es");
    for source in ["", "*all*"] {
        let content = complete.replace("srclang=\"en\"", &format!("srclang=\"{source}\""));
        let validation = check("tmx", &content, &project);
        assert!(validation.require_confirmation(false).is_err());
        validation.require_confirmation(true).unwrap();
        assert!(export::import_tmx(&content, "en", "es").is_err());
    }
    for content in [
        complete.replace(" xml:lang=\"es\"", ""),
        complete.replace("</tu>", "<tuv xml:lang=\"fr\"><seg>Third</seg></tuv></tu>"),
    ] {
        assert!(export::parse_import_catalog("tmx", &content, &project).is_err());
    }
    let empty = export::export_tmx(&[], "en", "es").unwrap();
    let validation = check("tmx", &empty, &project);
    assert!(validation.require_confirmation(false).is_err());
    validation.require_confirmation(true).unwrap();
}

#[test]
fn unknown_project_pair_requires_confirmation_and_known_sides_still_constrain() {
    let db = Database::open_in_memory().unwrap();
    let unknown = export::project_import_languages(&db, "", "").unwrap();
    assert_eq!(unknown, pair("", ""));
    for format in ["po", "xliff", "tmx"] {
        let content = catalog(format, "en", "es");
        let validation = check(format, &content, &unknown);
        assert!(validation.require_confirmation(false).is_err());
        validation.require_confirmation(true).unwrap();
        let known_target = pair("", "fr");
        let parsed = export::parse_import_catalog(format, &content, &known_target).unwrap();
        assert!(export::validate_import_languages(&parsed.languages, &known_target).is_err());
    }
}

#[test]
fn tmx_unknown_source_orients_from_known_target_instead_of_variant_order() {
    let complete = catalog("tmx", "en", "es")
        .replace("srclang=\"en\"", "srclang=\"*all*\"")
        .replace(
            "<tuv xml:lang=\"en\"><seg>Source</seg></tuv><tuv xml:lang=\"es\"><seg>Imported</seg></tuv>",
            "<tuv xml:lang=\"es\"><seg>Imported</seg></tuv><tuv xml:lang=\"en\"><seg>Source</seg></tuv>",
        );
    let project = pair("", "es");
    let parsed = export::parse_import_catalog("tmx", &complete, &project).unwrap();
    assert_eq!(parsed.updates[0].source, "Source");
    assert_eq!(parsed.updates[0].translation, "Imported");
    let validation = export::validate_import_languages(&parsed.languages, &project).unwrap();
    assert!(validation.require_confirmation(false).is_err());
    validation.require_confirmation(true).unwrap();
}

#[test]
fn tmx_undeclared_orientation_prefers_matching_script_before_matching_primary() {
    let complete = catalog("tmx", "zh-Hans-CN", "zh-Hant-TW")
        .replace("srclang=\"zh-Hans-CN\"", "srclang=\"*all*\"")
        .replace(
            "<tuv xml:lang=\"zh-Hans-CN\"><seg>Source</seg></tuv><tuv xml:lang=\"zh-Hant-TW\"><seg>Imported</seg></tuv>",
            "<tuv xml:lang=\"zh-Hant-TW\"><seg>Imported</seg></tuv><tuv xml:lang=\"zh-Hans-CN\"><seg>Source</seg></tuv>",
        );
    let project = pair("zh-Hans", "zh-Hant");
    let parsed = export::parse_import_catalog("tmx", &complete, &project).unwrap();
    assert_eq!(parsed.updates[0].source, "Source");
    assert_eq!(parsed.updates[0].translation, "Imported");
    let validation = export::validate_import_languages(&parsed.languages, &project).unwrap();
    assert!(validation.require_confirmation(false).is_err());
    validation.require_confirmation(true).unwrap();
}

#[test]
fn fresh_pivot_import_uses_explicit_pair_before_first_run() {
    let db = Database::open_in_memory().unwrap();
    let mut row = StringEntry::new("id", "日本語", "story.rpy".into());
    row.translation = Some("Source".into());
    row.status = StringStatus::Translated;
    db.save_entries(&[row]).unwrap();
    db.set_project_metadata("source_lang", &serde_json::json!("ja"))
        .unwrap();
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("pivot.db");
    db.pivot_to(&path).unwrap();
    let pivot = Database::open(&path).unwrap();
    assert!(pivot.get_translation_runs().unwrap().is_empty());
    let project = export::project_import_languages(&pivot, "en", "es").unwrap();
    assert_eq!(project, pair("en", "es"));
    for format in ["po", "xliff", "tmx"] {
        assert!(!check(format, &catalog(format, "en", "es"), &project).requires_confirmation);
    }
    assert_eq!(
        pivot
            .get_entry("id")
            .unwrap()
            .unwrap()
            .injection_source()
            .unwrap(),
        "日本語"
    );
}
