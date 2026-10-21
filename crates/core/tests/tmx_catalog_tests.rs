use locust_core::{
    database::Database,
    export::{export_tmx, import_tmx, tmx_units_for_batch, ImportedTranslation},
    models::{StringEntry, StringStatus, STALE_TRANSLATION_METADATA_KEY},
};

fn row(id: &str, source: &str, target: &str) -> StringEntry {
    let mut row = StringEntry::new(id, source, "story.json".into());
    row.translation = Some(target.into());
    row
}

fn catalog() -> String {
    export_tmx(&[row("id", "Hello", "Hola")], "en", "es").unwrap()
}

fn rejects(xml: &str) {
    assert!(
        import_tmx(xml, "en", "es").is_err(),
        "accepted: {}",
        xml.chars().take(160).collect::<String>()
    );
}

#[test]
fn exact_public_roundtrip_and_required_header() {
    let entries = vec![
        row(
            " story#0#日本語 \t\r\n<&\"",
            "  日本語 <tag> \\n[wait] & &#13;\r\n\t ",
            "  español 🦀\rfin\n\t ",
        ),
        row("other", "same source", ""),
        row("third", "same source", "Otro"),
    ];
    let xml = export_tmx(&entries, "ja", "es-419").unwrap();
    for attribute in [
        "creationtool=",
        "creationtoolversion=",
        "segtype=",
        "o-tmf=",
        "adminlang=",
        "srclang=",
        "datatype=",
    ] {
        assert!(xml.contains(attribute), "{attribute}");
    }
    assert!(xml.contains("tuid=\"locust-v1-"));
    assert!(!xml.contains('\r'));
    for ending in ["\n", "\r\n", "\r"] {
        let units = import_tmx(&xml.replace('\n', ending), "JA", "ES-419").unwrap();
        assert_eq!(units.len(), entries.len());
        for (unit, entry) in units.iter().zip(&entries) {
            assert_eq!(unit.id, entry.id);
            assert_eq!(unit.source, entry.source);
            assert_eq!(Some(unit.target.as_str()), entry.translation.as_deref());
            assert!(!unit.needs_review);
        }
        let (updates, skipped) = tmx_units_for_batch(&units);
        assert_eq!(skipped, 1);
        assert_eq!(
            updates[0],
            ImportedTranslation {
                id: entries[0].id.clone(),
                source: entries[0].source.clone(),
                translation: entries[0].translation.clone().unwrap()
            }
        );
    }
}

#[test]
fn cdata_references_and_physical_line_endings() {
    let xml = catalog().replace("Hello", " A\r\n<![CDATA[B\rC]]>&#13;&#xD;&amp;#13;\t ");
    let units = import_tmx(&xml, "en", "es").unwrap();
    assert_eq!(units[0].source, " A\nB\nC\r\r&#13;\t ");
    // Variant order does not determine source/target identity.
    let xml = catalog()
        .replace("xml:lang=\"en\"", "xml:lang=\"ES\"")
        .replace("xml:lang=\"es\"", "xml:lang=\"EN\"");
    let units = import_tmx(&xml, "en", "es").unwrap();
    assert_eq!((&*units[0].source, &*units[0].target), ("Hola", "Hello"));
}

#[test]
fn languages_are_explicit_concrete_distinct_and_exact() {
    for (source, target) in [
        ("", "es"),
        ("*all*", "es"),
        ("en", "EN"),
        ("en_US", "es"),
        ("en", "es "),
        ("und", "es"),
        ("mul", "es"),
        ("zxx", "es"),
        ("en\"", "es"),
    ] {
        assert!(export_tmx(&[], source, target).is_err());
        assert!(import_tmx(&catalog(), source, target).is_err());
    }
    for (source, target) in [("en-US", "es"), ("en", "es-ES"), ("ja", "es")] {
        assert!(import_tmx(&catalog(), source, target).is_err());
    }
    for xml in [
        catalog().replace("srclang=\"en\"", "srclang=\"*all*\""),
        catalog().replace("<tu tuid=", "<tu srclang=\"ja\" tuid="),
        catalog().replace("xml:lang=\"es\"", "xml:lang=\"en\""),
        catalog().replace("xml:lang=\"es\"", "xml:lang=\"fr\""),
        catalog().replace("<tuv xml:lang=\"es\"><seg>Hola</seg></tuv>", ""),
        catalog().replace("</tu>", "<tuv xml:lang=\"fr\"><seg>X</seg></tuv></tu>"),
    ] {
        rejects(&xml);
    }
    let xml = catalog().replace("<tu tuid=", "<tu srclang=\"EN\" tuid=");
    assert_eq!(import_tmx(&xml, "en", "es").unwrap().len(), 1);
}

#[test]
fn rejects_ambiguous_identity_and_structure() {
    let xml = catalog();
    for broken in [
        xml.replace("locust-v1-6964", "id"),
        xml.replace("locust-v1-6964", "locust-v2-6964"),
        xml.replace("locust-v1-6964", "locust-v1-ff"),
        xml.replace("locust-v1-6964", "locust-v1-6"),
        xml.replace("locust-v1-6964", "locust-v1-"),
        xml.replace("locust-v1-6964", "locust-v1-00"),
        xml.replace(" tuid=\"locust-v1-6964\"", ""),
        xml.replace("</body>", "<tu tuid=\"locust-v1-6964\"><tuv xml:lang=\"en\"><seg>X</seg></tuv><tuv xml:lang=\"es\"><seg>Y</seg></tuv></tu></body>"),
        xml.replace("</seg>", "</seg><seg>duplicate</seg>"),
        xml.replace("<seg>Hello</seg>", ""),
        xml.replace("<body>", "<body><body/>"),
        xml.replace("<body>", ""),
        xml.replace("</body>", "</body><body/>"),
        xml.replace("Hello", "<b>Hello</b>"),
        xml.replace("<tu tuid=", "<group><tu tuid="),
        xml.replace("</header>", "</header><header/>"),
        xml.replace("version=\"1.4\"", "version=\"2.0\""),
    ] { rejects(&broken); }
    assert!(export_tmx(&[row("id", "X", "Y"), row("id", "Z", "W")], "en", "es").is_err());
    assert!(export_tmx(&[row("", "X", "Y")], "en", "es").is_err());
}

#[test]
fn validates_every_attribute_namespace_entity_and_header() {
    let xml = catalog();
    for broken in [
        xml.replace("<tmx ", "<tmx xmlns=\"urn:other\" "),
        xml.replace("<tmx ", "<tmx xmlns:x=\"urn:other\" "),
        xml.replace("<tu tuid=", "<tu x:other=\"value\" tuid="),
        xml.replace("<tu tuid=", "<tu other=\"&broken;\" tuid="),
        xml.replace("<tu tuid=", "<tu other=\"x<y\" tuid="),
        xml.replace("<tmx ", "<tmx version=\"1.4\" "),
        xml.replace("xml:lang=\"en\"", "lang=\"en\""),
        xml.replace("<tu tuid=", "<tu xml:lang=\"en\" tuid="),
        xml.replace("datatype=\"plaintext\"", "datatype=\"html\""),
        xml.replace("segtype=\"block\"", "segtype=\"unknown\""),
    ] {
        rejects(&broken);
    }
    for name in [
        "creationtool",
        "creationtoolversion",
        "segtype",
        "o-tmf",
        "adminlang",
        "srclang",
        "datatype",
    ] {
        let start = xml.find(&format!(" {name}=\"")).unwrap();
        let end = start + xml[start + 1..].find('"').unwrap() + 2;
        let end = end + xml[end..].find('"').unwrap() + 1;
        rejects(&format!("{}{}", &xml[..start], &xml[end..]));
    }
}

#[test]
fn rejects_missing_separator_after_creationtool() {
    for quote in ['"', '\''] {
        let xml = catalog().replace('"', &quote.to_string());
        let valid = format!("{quote} creationtoolversion=");
        let broken = xml.replace(&valid, &format!("{quote}creationtoolversion="));
        assert_ne!(broken, xml);
        rejects(&broken);
    }
}

#[test]
fn rejects_missing_separator_after_segtype() {
    for quote in ['"', '\''] {
        let xml = catalog().replace('"', &quote.to_string());
        let valid = format!("{quote} o-tmf=");
        let broken = xml.replace(&valid, &format!("{quote}o-tmf="));
        assert_ne!(broken, xml);
        rejects(&broken);
    }
}

#[test]
fn rejects_missing_separator_in_xml_declaration() {
    for quote in ['"', '\''] {
        let xml = catalog().replace('"', &quote.to_string());
        let valid = format!("{quote} encoding=");
        let broken = xml.replace(&valid, &format!("{quote}encoding="));
        assert_ne!(broken, xml);
        rejects(&broken);
    }
}

#[test]
fn accepts_xml_whitespace_between_attributes() {
    let expected = import_tmx(&catalog(), "en", "es").unwrap();
    for quote in ['"', '\''] {
        for separator in [" ", "\t", "\n", "\r", "\r\n", " \t\r\n "] {
            let xml = catalog()
                .replace('"', &quote.to_string())
                .replace(&format!("{quote} "), &format!("{quote}{separator}"))
                .replace("<header ", &format!("<header{separator}"))
                .replace("<?xml ", &format!("<?xml{separator}"));
            // The opposite quote inside a value must not end that value.
            let tool = if quote == '"' {
                "Locust 'editor' build"
            } else {
                "Locust \"editor\" build"
            };
            let xml = xml.replace(
                &format!("creationtool={quote}Locust{quote}"),
                &format!("creationtool={quote}{tool}{quote}"),
            );
            assert_eq!(import_tmx(&xml, "en", "es").unwrap(), expected);
            let empty_header = xml.replace(&format!("{quote}></header>"), &format!("{quote}/>"));
            assert_eq!(import_tmx(&empty_header, "en", "es").unwrap(), expected);
            // XML also permits whitespace around equals and before tag closure.
            let spaced = xml
                .replace('=', " \t=\n ")
                .replace(&format!("{quote}>"), &format!("{quote} \t>"))
                .replace(&format!("{quote}?>"), &format!("{quote} \t?>"));
            assert_eq!(import_tmx(&spaced, "en", "es").unwrap(), expected);
        }
    }
    let mixed = catalog()
        .replace("creationtool=\"Locust\"", "creationtool='Locust'")
        .replace("segtype=\"block\"", "segtype='block'")
        .replace("version=\"1.0\"", "version='1.0'");
    assert_eq!(import_tmx(&mixed, "en", "es").unwrap(), expected);
}

#[test]
fn complete_document_validation_rejects_late_or_malformed_xml() {
    let xml = catalog();
    for broken in [
        String::new(),
        xml[..xml.len() - 8].into(),
        format!("{xml}<extra/>"),
        format!("{xml}garbage"),
        format!("{xml}{xml}"),
        format!("{xml}&#32;"),
        format!("&#32;{xml}"),
        xml.replace("Hola", "&unknown;"),
        xml.replace("Hola", "&#0;"),
        xml.replace("Hola", "&#xD800;"),
        xml.replace("Hola", "&#xFFFE;"),
        xml.replace("Hola", "]]>"),
        xml.replace("</seg>", "</wrong>"),
        xml.replace("<body>", "<!DOCTYPE tmx [<!ENTITY x 'X'>]><body>"),
        xml.replace("</body>", "<!--bad--comment--></body>"),
        xml.replace("</body>", "<?process instruction?></body>"),
        xml.replace("</body>", "<?xml version=\"1.0\"?></body>"),
        xml.replace("version=\"1.0\"", "version=\"1.1\""),
        xml.replace("encoding=\"UTF-8\"", "encoding=\"UTF-16\""),
        xml.replace("<body>", "<body>unexpected"),
        xml.replace("version=\"1.0\"", "version=\"1.&#48;\""),
        xml.replace(
            "version=\"1.0\" encoding=\"UTF-8\"",
            "encoding=\"UTF-8\" version=\"1.0\"",
        ),
        xml.replace(
            "encoding=\"UTF-8\"",
            "standalone=\"yes\" encoding=\"UTF-8\"",
        ),
        xml.replace(
            "encoding=\"UTF-8\"",
            "encoding=\"UTF-8\" standalone=\"maybe\"",
        ),
    ] {
        rejects(&broken);
    }
}

#[test]
fn xml_10_character_contract_is_fallible_both_ways() {
    for c in [
        '\0', '\u{1}', '\u{b}', '\u{c}', '\u{1f}', '\u{fffe}', '\u{ffff}',
    ] {
        let text = format!("bad{c}");
        for entry in [
            row(&text, "X", "Y"),
            row("id", &text, "Y"),
            row("id", "X", &text),
        ] {
            assert!(export_tmx(&[entry], "en", "es").is_err());
        }
        rejects(&catalog().replace("Hola", &text));
    }
    let good = "\t\n\r\u{20}\u{85}\u{d7ff}\u{e000}\u{fffd}\u{10000}\u{10ffff}";
    let xml = export_tmx(&[row(good, good, good)], "en", "es").unwrap();
    assert_eq!(import_tmx(&xml, "en", "es").unwrap()[0].source, good);
}

#[test]
fn decoded_segment_and_id_limits_are_inclusive() {
    let source = "é".repeat(512 * 1024);
    let id = "é".repeat(8 * 1024);
    let xml = export_tmx(&[row(&id, &source, "")], "en", "es").unwrap();
    assert_eq!(import_tmx(&xml, "en", "es").unwrap()[0].id, id);
    assert!(export_tmx(&[row(&(id.clone() + "x"), "", "")], "en", "es").is_err());
    assert!(export_tmx(&[row("id", &(source.clone() + "x"), "")], "en", "es").is_err());
    rejects(&xml.replace("</seg>", "x</seg>"));
    rejects(&xml.replace(
        &format!("locust-v1-{}", hex::encode(&id)),
        &format!("locust-v1-{}78", hex::encode(&id)),
    ));
    rejects(&catalog().replace("Hola", &"x".repeat(1024 * 1024 + 1)));
    let xml = catalog().replace("Hello", &"&#65;".repeat(1024 * 1024));
    assert_eq!(
        import_tmx(&xml, "en", "es").unwrap()[0].source.len(),
        1024 * 1024
    );
    rejects(&xml.replace("</seg>", "<![CDATA[x]]></seg>"));
}

#[test]
fn document_and_unit_caps_apply_on_import_and_export() {
    let empty = export_tmx(&[], "en", "es").unwrap();
    // Even an empty catalog must include its header in the output byte cap.
    let long_language = format!("en{}", "-abcdefgh".repeat(32 * 1024 * 1024 / 9 + 1));
    assert!(export_tmx(&[], &long_language, "es").is_err());
    let mut padded = empty.clone();
    padded.push_str(&" ".repeat(32 * 1024 * 1024 - padded.len()));
    assert!(import_tmx(&padded, "en", "es").unwrap().is_empty());
    padded.push(' ');
    rejects(&padded);
    let entries: Vec<_> = (0..100_000).map(|n| row(&n.to_string(), "", "")).collect();
    let xml = export_tmx(&entries, "en", "es").unwrap();
    assert_eq!(import_tmx(&xml, "en", "es").unwrap().len(), 100_000);
    rejects(&xml.replace("</body>", "<tu tuid=\"locust-v1-6578747261\"><tuv xml:lang=\"en\"><seg/></tuv><tuv xml:lang=\"es\"><seg/></tuv></tu></body>"));
    let mut too_many = entries;
    too_many.push(row("extra", "", ""));
    assert!(export_tmx(&too_many, "en", "es").is_err());
    let expensive = row("id", &"&".repeat(1024 * 1024), &"&".repeat(1024 * 1024));
    let entries: Vec<_> = (0..4)
        .map(|n| {
            let mut r = expensive.clone();
            r.id = n.to_string();
            r
        })
        .collect();
    assert!(export_tmx(&entries, "en", "es").is_err());
}

#[tokio::test]
async fn stale_empty_and_exact_source_guards_reuse_existing_batch_contract() {
    let db = Database::open_in_memory().unwrap();
    let mut stale = row("stale", "Old", "Viejo");
    stale.status = StringStatus::Approved;
    db.save_entries(&[stale]).unwrap();
    db.merge_entries(&[StringEntry::new("stale", "New", "story.json".into())])
        .unwrap();
    let stale = db.get_entry("stale").unwrap().unwrap();
    assert!(stale.metadata.contains_key(STALE_TRANSLATION_METADATA_KEY));
    let xml = export_tmx(
        &[
            stale.clone(),
            row("empty", "X", ""),
            row("changed", "Old", "Antes"),
        ],
        "en",
        "es",
    )
    .unwrap();
    db.save_entries(&[row("changed", "Current", "Saved")])
        .unwrap();
    let before = serde_json::to_value(db.get_entry("stale").unwrap()).unwrap();
    let units = import_tmx(&xml, "en", "es").unwrap();
    assert!(units[0].needs_review);
    assert_eq!(units[0].target, "Viejo");
    let (updates, skipped) = tmx_units_for_batch(&units);
    assert_eq!(skipped, 2);
    let report = db.save_imported_translations_batch(updates).await.unwrap();
    assert_eq!((report.imported, report.stale_sources), (0, 1));
    assert_eq!(
        serde_json::to_value(db.get_entry("stale").unwrap()).unwrap(),
        before
    );
    let reviewed = xml.replace("<prop type=\"x-locust-needs-review\">true</prop>", "");
    let (updates, skipped) = tmx_units_for_batch(&import_tmx(&reviewed, "en", "es").unwrap());
    assert_eq!(skipped, 1);
    let preview = db.preview_imported_translations(&updates, true).unwrap();
    assert_eq!(preview.confirmations.len(), 1);
    assert_eq!(
        serde_json::to_value(db.get_entry("stale").unwrap()).unwrap(),
        before
    );
    let report = db
        .save_imported_translations_batch_with(updates, true)
        .await
        .unwrap();
    assert_eq!(report.imported, 1);
    assert!(!db
        .get_entry("stale")
        .unwrap()
        .unwrap()
        .metadata
        .contains_key(STALE_TRANSLATION_METADATA_KEY));
    rejects(&xml.replace("true</prop>", "false</prop>"));
    rejects(&xml.replace(
        "</prop>",
        "</prop><prop type=\"x-locust-needs-review\">true</prop>",
    ));
    rejects(&xml.replace("x-locust-needs-review", "x-unknown"));
    let before = serde_json::to_value(db.get_entries(&Default::default()).unwrap()).unwrap();
    assert!(import_tmx(&(reviewed + "<broken>"), "en", "es").is_err());
    assert_eq!(
        serde_json::to_value(db.get_entries(&Default::default()).unwrap()).unwrap(),
        before
    );
}
