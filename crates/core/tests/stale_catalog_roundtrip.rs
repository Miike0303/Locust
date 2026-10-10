use locust_core::{
    database::Database,
    export::{
        export_po, export_xliff, import_counts_after_batch, import_po, import_xliff,
        po_entries_for_batch, xliff_units_for_batch, ImportedTranslation,
    },
    models::{StringEntry, StringStatus, STALE_TRANSLATION_METADATA_KEY},
};

#[derive(Clone, Copy)]
enum Catalog {
    Po,
    Xliff,
}

impl Catalog {
    fn export(self, entries: &[StringEntry]) -> String {
        match self {
            Self::Po => export_po(entries, "en", "es"),
            Self::Xliff => export_xliff(entries, "en", "es"),
        }
    }

    fn parse(self, content: &str) -> (Vec<ImportedTranslation>, usize) {
        match self {
            Self::Po => po_entries_for_batch(&import_po(content).unwrap()),
            Self::Xliff => xliff_units_for_batch(&import_xliff(content).unwrap()),
        }
    }

    fn clear_review(self, content: &str) -> String {
        match self {
            Self::Po => content.replace("#, fuzzy\n", ""),
            Self::Xliff => content.replace(" state=\"needs-review-translation\"", ""),
        }
    }
}

fn stale_row() -> (Database, StringEntry) {
    let db = Database::open_in_memory().unwrap();
    let mut entry = StringEntry::new("story#0#message", "Hello", "story.json".into());
    entry.translation = Some("Hola".into());
    entry.status = StringStatus::Approved;
    entry.provider_used = Some("original-provider".into());
    entry.translated_at = Some("2025-01-01T00:00:00Z".parse().unwrap());
    entry.reviewed_at = Some("2025-01-02T00:00:00Z".parse().unwrap());
    entry
        .metadata
        .insert("custom".into(), serde_json::json!(42));
    db.save_entries(&[entry]).unwrap();
    let mut extracted = StringEntry::new("story#0#message", "Hello again", "story.json".into());
    extracted
        .metadata
        .insert("custom".into(), serde_json::json!(42));
    assert_eq!(
        db.merge_entries(&[extracted]).unwrap().stale_source_reset,
        1
    );
    let stale = db.get_entry("story#0#message").unwrap().unwrap();
    assert_eq!(stale.status, StringStatus::Pending);
    assert_eq!(stale.translation.as_deref(), Some("Hola"));
    assert!(stale.metadata.contains_key(STALE_TRANSLATION_METADATA_KEY));
    (db, stale)
}

async fn untouched_roundtrip(format: Catalog) {
    let (db, before) = stale_row();
    let catalog = format.export(std::slice::from_ref(&before));
    let (updates, skipped) = format.parse(&catalog);
    assert!(updates.is_empty());
    let attempted = updates.len();
    let report = db.save_imported_translations_batch(updates).await.unwrap();
    let after = db.get_entry(&before.id).unwrap().unwrap();
    assert_eq!(after.status, StringStatus::Pending);
    assert_eq!(after.translation, before.translation);
    assert_eq!(after.metadata, before.metadata);
    assert_eq!(
        serde_json::to_value(after).unwrap(),
        serde_json::to_value(before).unwrap()
    );
    assert_eq!(skipped, 1);
    assert_eq!(
        import_counts_after_batch(skipped, attempted, report.imported),
        (0, 1)
    );
}

#[tokio::test]
async fn untouched_po_keeps_stale_translation_pending() {
    untouched_roundtrip(Catalog::Po).await;
}

#[tokio::test]
async fn untouched_xliff_keeps_stale_translation_pending() {
    untouched_roundtrip(Catalog::Xliff).await;
}

#[test]
fn stale_exports_retain_text_and_use_standard_review_markers() {
    let (_, stale) = stale_row();
    let po = export_po(std::slice::from_ref(&stale), "en", "es");
    assert!(po.contains("#, fuzzy\n"));
    let entries = import_po(&po).unwrap();
    assert!(entries[0].fuzzy);
    assert_eq!(entries[0].source, "Hello again");
    assert_eq!(entries[0].translation, "Hola");
    let xliff = export_xliff(&[stale], "en", "es");
    assert!(xliff.contains("<target state=\"needs-review-translation\">Hola</target>"));
    let units = import_xliff(&xliff).unwrap();
    assert_eq!(units[0].source, "Hello again");
    assert_eq!(units[0].target, "Hola");
}

async fn edited_roundtrip(format: Catalog) {
    let (db, before) = stale_row();
    let catalog = format
        .export(std::slice::from_ref(&before))
        .replace("Hola", "Hola de nuevo");
    let (updates, skipped) = format.parse(&format.clear_review(&catalog));
    assert_eq!(skipped, 0);
    let preview = db.preview_imported_translations(&updates, false).unwrap();
    assert_eq!(preview.replacements.len(), 1);
    let report = db.save_imported_translations_batch(updates).await.unwrap();
    assert_eq!(report.imported, 1);
    let after = db.get_entry(&before.id).unwrap().unwrap();
    assert_eq!(after.status, StringStatus::Translated);
    assert_eq!(after.translation.as_deref(), Some("Hola de nuevo"));
    assert_eq!(after.provider_used.as_deref(), Some("import"));
    assert!(!after.metadata.contains_key(STALE_TRANSLATION_METADATA_KEY));
    assert_eq!(after.metadata.get("custom"), before.metadata.get("custom"));
    assert!(after.require_current_translation().is_ok());
}

#[tokio::test]
async fn edited_unflagged_po_accepts_current_translation() {
    edited_roundtrip(Catalog::Po).await;
}

#[tokio::test]
async fn edited_unflagged_xliff_accepts_current_translation() {
    edited_roundtrip(Catalog::Xliff).await;
}

async fn reviewed_same_text_roundtrip(format: Catalog) {
    for keep_existing in [false, true] {
        let (db, before) = stale_row();
        let catalog = format.clear_review(&format.export(std::slice::from_ref(&before)));
        let (updates, skipped) = format.parse(&catalog);
        assert_eq!(updates.len(), 1);
        assert_eq!(skipped, 0);
        let preview = db
            .preview_imported_translations(&updates, keep_existing)
            .unwrap();
        assert_eq!(preview.confirmations.len(), 1);
        let confirmation = &preview.confirmations[0];
        assert_eq!(confirmation.id, before.id);
        assert_eq!(confirmation.previous, "Hola");
        assert_eq!(confirmation.previous_status, "pending");
        assert_eq!(confirmation.new_text, "Hola");
        assert!(preview.replacements.is_empty());
        assert!(preview.kept_ids.is_empty());
        let report = db
            .save_imported_translations_batch_with(updates, keep_existing)
            .await
            .unwrap();
        assert_eq!(report.imported, 1);
        assert_eq!(report.unchanged, 0);
        assert_eq!(report.kept_existing, 0);
        assert_eq!(report.stale_sources, 0);
        assert_eq!(report.unknown_ids, 0);
        assert_eq!(
            import_counts_after_batch(skipped, 1, report.imported),
            (1, 0)
        );
        assert_eq!(
            serde_json::to_value(report).unwrap(),
            serde_json::to_value(preview.report).unwrap()
        );
        let after = db.get_entry(&before.id).unwrap().unwrap();
        assert_eq!(after.translation, before.translation);
        assert_eq!(after.status, StringStatus::Translated);
        assert_eq!(after.provider_used.as_deref(), Some("import"));
        assert!(after.translated_at.unwrap() > before.translated_at.unwrap());
        assert!(!after.metadata.contains_key(STALE_TRANSLATION_METADATA_KEY));
        assert_eq!(after.metadata.get("custom"), before.metadata.get("custom"));
        assert!(after.require_current_translation().is_ok());
    }
}

#[tokio::test]
async fn reviewed_same_text_po_confirms_stale_translation() {
    reviewed_same_text_roundtrip(Catalog::Po).await;
}

#[tokio::test]
async fn reviewed_same_text_xliff_confirms_stale_translation() {
    reviewed_same_text_roundtrip(Catalog::Xliff).await;
}

#[test]
fn xliff_needs_review_targets_are_skipped_without_leaking_state() {
    for state in [
        "needs-review-translation",
        "needs-review-adaptation",
        "needs-review-l10n",
    ] {
        for prefix in ["", "x:"] {
            let catalog = format!(
                r#"<{prefix}xliff xmlns:x="urn:oasis:names:tc:xliff:document:1.2" version="1.2"><{prefix}file><{prefix}body>
                <{prefix}trans-unit id="review"><{prefix}source>Hello again</{prefix}source><{prefix}target state="{state}">Hola de nuevo</{prefix}target></{prefix}trans-unit>
                <{prefix}trans-unit id="clear"><{prefix}source>Hello</{prefix}source><{prefix}target>Hola</{prefix}target></{prefix}trans-unit>
                <{prefix}trans-unit id="final"><{prefix}source>Hello</{prefix}source><{prefix}target state="final">Hola</{prefix}target></{prefix}trans-unit>
                <{prefix}trans-unit id="empty"><{prefix}source>Hello</{prefix}source><{prefix}target state="{state}"/></{prefix}trans-unit>
                </{prefix}body></{prefix}file></{prefix}xliff>"#
            );
            let units = import_xliff(&catalog).unwrap();
            assert_eq!(units[0].target, "Hola de nuevo");
            let (updates, skipped) = xliff_units_for_batch(&units);
            assert_eq!(skipped, 2, "state={state}, prefix={prefix}");
            assert_eq!(
                updates
                    .iter()
                    .map(|entry| entry.id.as_str())
                    .collect::<Vec<_>>(),
                ["clear", "final"]
            );
        }
    }
}

fn non_stale_fixture() -> Vec<StringEntry> {
    let mut translated =
        StringEntry::new("story#0#message", "Hello & \"world\"", "story.json".into());
    translated.context = Some("greeting".into());
    translated.translation = Some("Hola <mundo>\n".into());
    translated
        .metadata
        .insert("custom".into(), serde_json::json!(true));
    vec![
        translated,
        StringEntry::new("empty", "Goodbye", "story.json".into()),
    ]
}

// Golden bytes from the serializer before stale catalog handling was added.
#[test]
fn non_stale_po_matches_previous_serializer_bytes() {
    let expected = concat!(
        "# Project Locust export\n# Source: en, Target: es\n\n",
        "msgid \"\"\nmsgstr \"\"\n",
        "\"Content-Type: text/plain; charset=UTF-8\\n\"\n",
        "\"Content-Transfer-Encoding: 8bit\\n\"\n\"Language: es\\n\"\n\n",
        "#. greeting\n#: story.json\nmsgctxt \"story#0#message\"\n",
        "msgid \"Hello & \\\"world\\\"\"\nmsgstr \"Hola <mundo>\\n\"\n\n",
        "#: story.json\nmsgctxt \"empty\"\nmsgid \"Goodbye\"\nmsgstr \"\"\n",
    );
    assert_eq!(
        export_po(&non_stale_fixture(), "en", "es").as_bytes(),
        expected.as_bytes()
    );
}

#[test]
fn non_stale_xliff_matches_previous_serializer_bytes() {
    let expected = concat!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n",
        "<xliff version=\"1.2\" xmlns=\"urn:oasis:names:tc:xliff:document:1.2\">\n",
        "  <file source-language=\"en\" target-language=\"es\" datatype=\"plaintext\">\n    <body>\n",
        "      <trans-unit id=\"story#0#message\">\n",
        "        <source>Hello &amp; &quot;world&quot;</source>\n",
        "        <target>Hola &lt;mundo&gt;\n</target>\n      </trans-unit>\n",
        "      <trans-unit id=\"empty\">\n        <source>Goodbye</source>\n",
        "        <target></target>\n      </trans-unit>\n    </body>\n  </file>\n</xliff>\n",
    );
    assert_eq!(
        export_xliff(&non_stale_fixture(), "en", "es").as_bytes(),
        expected.as_bytes()
    );
}
