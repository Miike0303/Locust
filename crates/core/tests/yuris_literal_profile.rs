//! Synthetic evidence only: no private game data or live provider calls.
use async_trait::async_trait;
use locust_core::{
    database::Database,
    error::Result,
    glossary::Glossary,
    models::{StringEntry, StringStatus, TranslationRequest, TranslationResult},
    placeholder::{PlaceholderProcessor, PlaceholderProfile},
    translation::{TranslationManager, TranslationOptions, TranslationProvider},
    validation::Validator,
};
use std::{path::PathBuf, sync::Arc};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

// This exact extractor representation is intentionally not inferred from a path,
// role, context, or format-global mapping. Unknown versions remain conservative.
const KEY: &str = "yuris_literal_representation";
const LITERAL: &str = "ystb-0x22b-attr3-push0x4d-quoted-sjis-v1";

fn literal_entry() -> StringEntry {
    let mut entry = StringEntry::new("story.ybn#attr1", "[カナ]", PathBuf::from("story.ybn"))
        .with_format_id("yuris");
    entry
        .metadata
        .insert(KEY.into(), serde_json::json!(LITERAL));
    entry
}

struct ScriptedProvider;
#[async_trait]
impl TranslationProvider for ScriptedProvider {
    fn id(&self) -> &str {
        "yuris-literal-fixture"
    }
    fn name(&self) -> &str {
        "YU-RIS literal fixture"
    }
    fn is_free(&self) -> bool {
        true
    }
    fn requires_api_key(&self) -> bool {
        false
    }
    async fn health_check(&self) -> Result<()> {
        Ok(())
    }
    async fn estimate_cost(&self, _: usize, _: &str) -> Option<f64> {
        Some(0.0)
    }
    async fn translate(&self, requests: &[TranslationRequest]) -> Result<Vec<TranslationResult>> {
        Ok(requests
            .iter()
            .map(|r| TranslationResult {
                entry_id: r.entry_id.clone(),
                translation: r.source.replace("[カナ]", "[Kana]"),
                detected_source_lang: None,
                provider: self.id().into(),
                tokens_used: None,
                input_tokens: None,
                output_tokens: None,
                cost_usd: Some(0.0),
            })
            .collect())
    }
}

#[tokio::test]
async fn scripted_kana_brackets_require_both_exact_provenances() {
    for case in [
        "literal",
        "unmarked",
        "renpy",
        "unknown",
        "path-only",
        "malformed",
    ] {
        let mut entry = literal_entry();
        match case {
            "unmarked" => {
                entry.metadata.remove(KEY);
            }
            "renpy" | "unknown" => {
                entry = entry.with_format_id(case);
            }
            "path-only" => {
                entry.metadata.remove("format_id");
            }
            "malformed" => {
                entry
                    .metadata
                    .insert(KEY.into(), serde_json::json!({"version": LITERAL}));
            }
            _ => {}
        }
        // Untrusted descriptive text must never grant literal authority.
        entry.context = Some(LITERAL.into());
        entry.tags = vec!["dialogue".into()];
        let db = Arc::new(Database::open_in_memory().unwrap());
        db.save_entries(&[entry.clone()]).unwrap();
        let (tx, mut rx) = mpsc::channel(100);
        let consume = tokio::spawn(async move { while rx.recv().await.is_some() {} });
        TranslationManager::new(
            Arc::new(ScriptedProvider),
            db.clone(),
            Arc::new(Glossary::new(db.clone())),
        )
        .translate_entries(
            vec![entry],
            TranslationOptions {
                use_memory: false,
                ..Default::default()
            },
            tx,
            case.into(),
            CancellationToken::new(),
        )
        .await
        .unwrap();
        consume.await.unwrap();
        let saved = db.get_entry("story.ybn#attr1").unwrap().unwrap();
        if case == "literal" {
            assert_eq!(saved.translation.as_deref(), Some("[Kana]"));
            assert_eq!(saved.status, StringStatus::Translated);
        } else {
            assert!(saved.translation.is_none(), "{case}");
            assert_eq!(saved.status, StringStatus::Pending, "{case}");
        }
    }
}

#[test]
fn malformed_unknown_and_legacy_metadata_remain_full() {
    assert_eq!(
        PlaceholderProfile::for_format("yuris"),
        PlaceholderProfile::Full
    );
    for marker in [
        serde_json::Value::Null,
        serde_json::json!(true),
        serde_json::json!(1),
        serde_json::json!([LITERAL]),
        serde_json::json!(format!("{LITERAL} ")),
        serde_json::json!("ystb-0x22b-attr3-push0x4d-quoted-sjis-v2"),
    ] {
        let mut entry = literal_entry();
        entry.metadata.insert(KEY.into(), marker);
        assert_eq!(entry.placeholder_profile(), PlaceholderProfile::Full);
    }
    for format in [
        serde_json::Value::Null,
        serde_json::json!(7),
        serde_json::json!("YU-RIS"),
        serde_json::json!("renpy"),
    ] {
        let mut entry = literal_entry();
        entry.metadata.insert("format_id".into(), format);
        assert_eq!(entry.placeholder_profile(), PlaceholderProfile::Full);
    }
    let mut old = literal_entry();
    old.metadata.remove(KEY);
    assert_eq!(old.placeholder_profile(), PlaceholderProfile::Full);
}

#[test]
fn literal_profile_changes_only_bracket_scanning() {
    let mut entry = literal_entry();
    entry.source = r"[カナ] \V[1] <b>%s {name} {PL_9}</b>".into();
    let valid = r"[Kana] \V[1] <b>%s {name} {PL_9}</b>";
    entry.translation = Some(valid.into());
    assert!(entry.require_preserved_translation_controls().is_ok());
    for control in [r"\V[1]", "<b>", "%s", "{name}", "{PL_9}"] {
        for changed in [
            valid.replacen(control, "", 1),
            format!("{valid} {control}"),
            valid.replace(control, &format!("{control}{control}")),
        ] {
            entry.translation = Some(changed);
            assert!(
                entry.require_preserved_translation_controls().is_err(),
                "{control}"
            );
        }
    }
    let (safe, controls) =
        PlaceholderProcessor::extract_with_profile(&entry.source, entry.placeholder_profile());
    assert!(PlaceholderProcessor::restore(&format!("{safe} {{PL_999}}"), &controls).is_err());
    entry
        .metadata
        .insert("binary_slot".into(), serde_json::json!("utf8"));
    entry.translation = Some(format!("{valid} {}", "long ".repeat(30)));
    assert!(!Validator::validate_entry(&entry).is_empty());
}

#[test]
fn strict_pivot_reopen_and_source_copy_preserve_literal_and_physical_controls() {
    let dir = tempfile::tempdir().unwrap();
    let source_path = dir.path().join("source.db");
    let pivot_path = dir.path().join("pivot.db");
    let mut entry = literal_entry();
    entry.source = r"[カナ] \V[1]".into();
    entry.translation = Some(r"[Kana] \V[1]".into());
    entry.status = StringStatus::Translated;
    {
        let db = Database::open(&source_path).unwrap();
        db.save_entries(&[entry.clone()]).unwrap();
        db.pivot_to_complete(&pivot_path).unwrap();
    }
    let db = Database::open(&pivot_path).unwrap();
    let mut pivot = db.get_entry(&entry.id).unwrap().unwrap();
    assert_eq!(pivot.metadata[KEY], LITERAL);
    assert_eq!(pivot.metadata["format_id"], "yuris");
    assert_eq!(pivot.source, r"[Kana] \V[1]");
    assert_eq!(pivot.injection_source().unwrap(), entry.source);
    pivot.translation = Some(r"[Nombre] \V[1]".into());
    pivot.status = StringStatus::Translated;
    db.save_entries(&[pivot.clone()]).unwrap();
    db.pivot_to_complete(&dir.path().join("second.db")).unwrap();
    // Even if an intermediate semantic source loses a control, the physical
    // extraction baseline still refuses a later translation that omits it.
    pivot.source = "[Kana]".into();
    pivot.translation = Some("[Nombre]".into());
    assert!(pivot.require_preserved_translation_controls().is_err());
    let source_copy = Database::open(&dir.path().join("copy.db")).unwrap();
    source_copy.save_entries(&[entry.clone()]).unwrap();
    assert_eq!(
        source_copy.get_entry(&entry.id).unwrap().unwrap().metadata,
        entry.metadata
    );
    let original = Database::open(&source_path)
        .unwrap()
        .get_entry(&entry.id)
        .unwrap()
        .unwrap();
    assert_eq!(original.source, entry.source);
    assert_eq!(original.metadata, entry.metadata);
}
