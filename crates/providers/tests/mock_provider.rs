use locust_core::{
    database::Database,
    glossary::Glossary,
    models::{StringEntry, StringStatus, TranslationRequest},
    placeholder::PlaceholderProcessor,
    translation::{TranslationManager, TranslationOptions, TranslationProvider},
    validation::utf16_byte_len,
};
use locust_providers::mock::MockProvider;
use std::{path::PathBuf, sync::Arc};
use tokio::sync::mpsc;

fn request(id: &str, source: &str) -> TranslationRequest {
    TranslationRequest {
        entry_id: id.into(),
        source: source.into(),
        source_lang: "en".into(),
        target_lang: "es".into(),
        context: None,
        glossary_hint: None,
    }
}

fn assert_inventory(source: &str, output: &str, tokens: &[&str]) {
    assert!(output.len() <= source.len(), "UTF-8: {output:?}");
    assert!(
        utf16_byte_len(output) <= utf16_byte_len(source),
        "UTF-16: {output:?}"
    );
    let mut end = 0;
    for token in tokens {
        assert_eq!(
            output.matches(token).count(),
            source.matches(token).count(),
            "{token}: {output:?}"
        );
        let start = output[end..]
            .find(token)
            .unwrap_or_else(|| panic!("missing/reordered {token}: {output:?}"))
            + end;
        end = start + token.len();
    }
}

#[tokio::test]
async fn mock_single_request_preserves_each_token_family_and_spelling() {
    for token in ["{PL_0}", "{PL_123}", "{PL_1_0}", "{G_0_0}", "{G_01_002}"] {
        for source in [
            token.to_string(),
            format!("Émile 😀 says a long enough line {token} end"),
        ] {
            let result = MockProvider
                .translate(&[request("one", &source)])
                .await
                .unwrap();
            assert_inventory(&source, &result[0].translation, &[token]);
        }
    }
}

#[tokio::test]
async fn mock_batch_keeps_composite_inventory_order_and_record_identity() {
    let sources = [
        (
            "one",
            "{PL_0}Émile 😀 says {G_0_0} and {G_0_1}{PL_1}",
            vec!["{PL_0}", "{G_0_0}", "{G_0_1}", "{PL_1}"],
        ),
        (
            "two",
            "{PL_7_0}{G_1_0}{PL_7_1}",
            vec!["{PL_7_0}", "{G_1_0}", "{PL_7_1}"],
        ),
    ];
    let requests: Vec<_> = sources
        .iter()
        .map(|(id, source, _)| request(id, source))
        .collect();
    let results = MockProvider.translate(&requests).await.unwrap();
    assert_eq!(results.len(), requests.len());
    for ((id, source, tokens), result) in sources.iter().zip(results) {
        assert_eq!(&result.entry_id, id);
        assert_inventory(source, &result.translation, tokens);
    }
}

#[tokio::test]
async fn mock_does_not_repair_malformed_or_unknown_sentinels_into_valid_tokens() {
    for token in ["{G_bad}", "{G_0_}", "{PL_name_0}", "{G_0_0", "{PL_0_}"] {
        let results = MockProvider
            .translate(&[request("malformed", token)])
            .await
            .unwrap();
        assert_eq!(results[0].translation, token);
        assert!(PlaceholderProcessor::restore(&results[0].translation, &[]).is_err());
    }
}

#[tokio::test]
async fn mock_no_token_output_and_dual_budget_truncation_are_unchanged() {
    let cases = [
        ("", ""),
        ("Hero", "oreH"),
        ("日本語", "語本日"),
        ("A😀", "😀A"),
        ("😀😀😀😀😀", "[MOCK:es] "),
        ("Hello, world!", "[MOCK:es] Hel"),
        ("{PL_0}😀😀😀😀😀{PL_1}", "{PL_0}[MOCK:es] {PL_1}"),
    ];
    let requests: Vec<_> = cases
        .iter()
        .enumerate()
        .map(|(i, (source, _))| request(&i.to_string(), source))
        .collect();
    let results = MockProvider.translate(&requests).await.unwrap();
    for ((source, expected), result) in cases.iter().zip(results) {
        assert_eq!(&result.translation, expected, "{source}");
        assert!(result.translation.len() <= source.len());
        assert!(utf16_byte_len(&result.translation) <= utf16_byte_len(source));
    }
}

#[tokio::test]
async fn core_with_stock_mock_retains_canonical_speaker_argument_syntax() {
    let db = Arc::new(Database::open_in_memory().unwrap());
    let glossary = Arc::new(Glossary::new(db.clone()));
    glossary.add("Old", "Vieja", "en-es", None).unwrap();
    glossary.add("house", "casa", "en-es", None).unwrap();
    glossary.add("Hero", "Héroe", "en-es", None).unwrap();
    let entries = vec![
        StringEntry::new("speaker", r"\n<Old house>Body text long enough for the mock marker.",
            PathBuf::from("Map001.json")).with_format_id("rpgmaker-mv"),
        StringEntry::new("multiple", r"Émile \n<Old house>Body text long enough for the mock marker. {G_0_0} \nr<Hero {PL_0}>More body text.",
            PathBuf::from("Map001.json")).with_format_id("rpgmaker-mv"),
    ];
    db.save_entries(&entries).unwrap();
    let manager = TranslationManager::new(Arc::new(MockProvider), db.clone(), glossary);
    let (tx, _rx) = mpsc::channel(128);
    manager
        .translate_entries(
            entries,
            TranslationOptions {
                source_lang: "en".into(),
                target_lang: "es".into(),
                use_memory: false,
                ..Default::default()
            },
            tx,
            "real-mock-speaker".into(),
            Default::default(),
        )
        .await
        .unwrap();
    let saved = db.get_entry("speaker").unwrap().unwrap();
    assert_eq!(saved.status, StringStatus::Translated);
    assert!(saved
        .translation
        .as_deref()
        .unwrap()
        .starts_with(r"\n<Vieja casa>"));
    assert!(saved.translation.as_deref().unwrap().contains("[MOCK:es]"));
    assert!(saved.require_preserved_translation_controls().is_ok());
    let multiple = db.get_entry("multiple").unwrap().unwrap();
    assert_eq!(multiple.status, StringStatus::Translated);
    let text = multiple.translation.as_deref().unwrap();
    for literal in [
        r"\n<Vieja casa>",
        r"\nr<Héroe {PL_0}>",
        "{G_0_0}",
        "[MOCK:es]",
    ] {
        assert!(text.contains(literal), "{text}");
    }
    assert!(multiple.require_preserved_translation_controls().is_ok());
}

#[tokio::test]
async fn core_with_stock_mock_restores_canonical_glossary_and_literal_sentinels() {
    let db = Arc::new(Database::open_in_memory().unwrap());
    let glossary = Arc::new(Glossary::new(db.clone()));
    glossary.add("Hero", "Héroe", "en-es", None).unwrap();
    let source = r"A brave hero. \C[2] {PL_0} {G_0_0} {G_bad}";
    let entry = StringEntry::new("masked", source, PathBuf::from("Actors.json"))
        .with_format_id("rpgmaker-mv");
    db.save_entries(std::slice::from_ref(&entry)).unwrap();
    let manager = TranslationManager::new(Arc::new(MockProvider), db.clone(), glossary);
    let (tx, _rx) = mpsc::channel(128);
    manager
        .translate_entries(
            vec![entry],
            TranslationOptions {
                source_lang: "en".into(),
                target_lang: "es".into(),
                use_memory: false,
                ..Default::default()
            },
            tx,
            "real-mock".into(),
            Default::default(),
        )
        .await
        .unwrap();
    let saved = db.get_entry("masked").unwrap().unwrap();
    assert_eq!(saved.status, StringStatus::Translated);
    let output = saved.translation.as_deref().unwrap();
    assert!(output.contains("Héroe"), "{output}");
    for control in [r"\C[2]", "{PL_0}", "{G_0_0}", "{G_bad}"] {
        assert_eq!(output.matches(control).count(), 1, "{output}");
    }
    assert!(saved.require_preserved_translation_controls().is_ok());
}
