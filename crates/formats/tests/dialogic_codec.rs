use locust_formats::dialogic::{
    parse_character, parse_timeline, rebuild, CharacterSet, SpanKind, MAX_INPUT_BYTES,
    MAX_LINE_BYTES,
};
use std::collections::{BTreeMap, HashMap};

fn characters() -> CharacterSet {
    let mut set = CharacterSet::default();
    set.insert_filename("res://characters/Aster.dch").unwrap();
    set.insert_filename("res://characters/Blue Bird.dch")
        .unwrap();
    set
}

#[test]
fn lossless_synthetic_corpus_shapes() {
    let source = concat!(
        "[signal arg=\"invented\"]\n[style name=\"paper\"]\n",
        "[sound path=\"res://bell.ogg\"]\n[music]\n[background]\n[wait]\n",
        "join Aster left\nleave Aster\nupdate Aster [mirrored=\"true\"]\n",
        "\t  \nAster: The orchard is quiet.\nPlain invented text.\n",
        "\tIndented invented text.\n",
        "[color=red][font_size={15}]A bright apple.[/font_size][/color]\n",
        "[font_size=30]A large apple.\n"
    );
    let timeline = parse_timeline(source, &characters()).unwrap();
    assert_eq!(timeline.spans.len(), 5);
    assert!(timeline.diagnostics.is_empty());
    assert_eq!(rebuild(source, &timeline, &HashMap::new()).unwrap(), source);
    assert_eq!(timeline.spans[0].speaker.as_deref(), Some("Aster"));
    assert_eq!(timeline.spans[0].id.line, 10);
    assert_eq!(
        &source[timeline.spans[0].range.clone()],
        "The orchard is quiet."
    );
}

#[test]
fn continuation_is_one_event_with_exact_layout() {
    let source = "\tAster (happy): First  \\\r\n  second\\\n\tthird #id:apple\r\nTail";
    let timeline = parse_timeline(source, &characters()).unwrap();
    assert_eq!(timeline.spans.len(), 2);
    let span = &timeline.spans[0];
    assert_eq!(span.kind, SpanKind::TextContinued);
    assert_eq!(span.visible_text, "First\nsecond\nthird");
    assert_eq!(span.id.translation_id.as_deref(), Some("apple"));
    assert_eq!(span.id.ordinal, 0);
    let translations = HashMap::from([(span.id.clone(), "Uno\nDos\nTres".into())]);
    assert_eq!(
        rebuild(source, &timeline, &translations).unwrap(),
        "\tAster (happy): Uno  \\\r\n  Dos\\\n\tTres #id:apple\r\nTail"
    );
    assert_eq!(rebuild(source, &timeline, &HashMap::new()).unwrap(), source);
}

#[test]
fn speaker_variants_and_ids_are_not_translated() {
    let source =
        "Aster: Hello\nAster(happy): Hello\n\"Blue Bird\" (calm): Hello #id:42\n_: Hello\n";
    let timeline = parse_timeline(source, &characters()).unwrap();
    assert_eq!(timeline.spans.len(), 4);
    assert_eq!(timeline.spans[2].speaker.as_deref(), Some("Blue Bird"));
    assert_eq!(timeline.spans[3].speaker.as_deref(), Some("_"));
    let translations = timeline
        .spans
        .iter()
        .map(|s| (s.id.clone(), "Hola".into()))
        .collect();
    let output = rebuild(source, &timeline, &translations).unwrap();
    assert_eq!(output, source.replace("Hello", "Hola"));
    let again = parse_timeline(&output, &characters()).unwrap();
    assert_eq!(
        timeline.spans.iter().map(|s| &s.id).collect::<Vec<_>>(),
        again.spans.iter().map(|s| &s.id).collect::<Vec<_>>()
    );
}

#[test]
fn primary_conditions_do_not_require_a_colon() {
    let source = "if {score} > 2:\nif you see a kite, turn west!\nelif {score}\nelsewhere\nif\nelif\nelse\nordinary text\n";
    let timeline = parse_timeline(source, &characters()).unwrap();
    assert_eq!(timeline.spans.len(), 1);
    assert_eq!(timeline.spans[0].visible_text, "ordinary text");
    assert_eq!(timeline.diagnostics.len(), 7);
    assert!(timeline
        .diagnostics
        .iter()
        .all(|d| d.reason == "condition event (not evaluated)"));
}

#[test]
fn choice_suffix_and_escaped_pipe_are_protected() {
    let source = "\t- Take [b]apple[/b] | [if {score} > 2] [else=\"hide\"] #id:pick\n- Pear [if {flag}]\n- Red \\| blue\n";
    let timeline = parse_timeline(source, &characters()).unwrap();
    assert_eq!(timeline.spans.len(), 3);
    assert!(timeline.spans.iter().all(|s| s.kind == SpanKind::Choice));
    let span = &timeline.spans[0];
    assert!(!span.visible_text.contains("[b]"));
    assert!(!span.visible_text.contains("score"));
    let translations =
        HashMap::from([(span.id.clone(), span.visible_text.replace("Take", "Choose"))]);
    assert_eq!(
        rebuild(source, &timeline, &translations).unwrap(),
        source.replacen("Take", "Choose", 1)
    );
}

#[test]
fn tags_variables_and_escapes_use_immutable_placeholders() {
    let source =
        "Aster: [color=red][font_size={15}]Hi {player.name}\\: friend.[/color] [pause=1]\n";
    let timeline = parse_timeline(source, &characters()).unwrap();
    let span = &timeline.spans[0];
    assert!(!span.visible_text.contains("player.name"));
    assert!(!span.visible_text.contains("[color"));
    let translated = span.visible_text.replace("Hi", "Hola");
    assert_eq!(
        rebuild(
            source,
            &timeline,
            &HashMap::from([(span.id.clone(), translated)])
        )
        .unwrap(),
        source.replace("Hi", "Hola")
    );
    assert!(rebuild(
        source,
        &timeline,
        &HashMap::from([(span.id.clone(), "lost tags".into())])
    )
    .is_err());
}

#[test]
fn unsupported_lines_are_diagnosed_and_never_spans() {
    let source = "[custom arg=\"x\"]\nUnknown: Hello\n-bad choice\n- Apple | nonsense\nText {broken\nText [broken\n\\escaped text\nText #id:a #id:b\n[glossary]\n";
    let timeline = parse_timeline(source, &characters()).unwrap();
    assert!(timeline.spans.is_empty());
    assert_eq!(timeline.diagnostics.len(), 9);
    assert_eq!(rebuild(source, &timeline, &HashMap::new()).unwrap(), source);
}

#[test]
fn registered_commands_including_prefix_matches_are_not_text() {
    let source = "joinery\nleaveNow\nupdateMe\ndoSomething()\nlabelled\njumpstart\nsetback\nsetting x\nreturn\n# comment\n<<END BRANCH>>\n[music]\n[sound]\n[background]\n[clear]\n[end_timeline]\n[history]\n[save]\n[signal]\n[style]\n[text_input]\n[voice]\n[wait]\n[wait_input]\n";
    let timeline = parse_timeline(source, &characters()).unwrap();
    assert!(timeline.spans.is_empty());
    assert_eq!(rebuild(source, &timeline, &HashMap::new()).unwrap(), source);
}

#[test]
fn multiline_shortcode_body_is_never_text() {
    let source = "[signal arg=\"\nopaque body\n\"]\nVisible text\n";
    let timeline = parse_timeline(source, &characters()).unwrap();
    assert_eq!(timeline.spans.len(), 1);
    assert_eq!(timeline.spans[0].id.line, 3);
    assert_eq!(rebuild(source, &timeline, &HashMap::new()).unwrap(), source);
}

#[test]
fn unknown_multiline_event_bodies_are_not_guessed_as_text() {
    let source = "[custom arg=\"\nopaque body\n\"]\nVisible text\n";
    let timeline = parse_timeline(source, &characters()).unwrap();
    assert_eq!(timeline.spans.len(), 1);
    assert_eq!(timeline.spans[0].id.line, 3);
    assert_eq!(timeline.diagnostics.len(), 1);
}

#[test]
fn continuation_bodies_do_not_start_new_events() {
    let source = "An invented direction:\\\nif a kite appears, turn west!\\\nAster: still part of the same text\n";
    let timeline = parse_timeline(source, &characters()).unwrap();
    assert_eq!(timeline.spans.len(), 1);
    assert!(timeline.diagnostics.is_empty());
    assert_eq!(timeline.spans[0].kind, SpanKind::TextContinued);
    assert!(timeline.spans[0].visible_text.contains("\nif a kite"));
    assert!(timeline.spans[0].speaker.is_none());
}

#[test]
fn choice_translations_cannot_introduce_delimiters() {
    let source = "- An apple\n";
    let timeline = parse_timeline(source, &characters()).unwrap();
    for value in ["A pear |", "A pear ||"] {
        assert!(rebuild(
            source,
            &timeline,
            &HashMap::from([(timeline.spans[0].id.clone(), value.into())])
        )
        .is_err());
    }
}

#[test]
fn choice_delimiters_inside_tokens_are_ambiguous() {
    for source in ["- [url=a|b]An apple[/url]\n", "- {thing|other}\n"] {
        let timeline = parse_timeline(source, &characters()).unwrap();
        assert!(timeline.spans.is_empty());
        assert_eq!(timeline.diagnostics.len(), 1);
    }
}

#[test]
fn placeholders_cannot_be_changed_duplicated_or_reordered() {
    let source = "Hello {name} [b]friend[/b]";
    let timeline = parse_timeline(source, &characters()).unwrap();
    let span = &timeline.spans[0];
    for value in [
        span.visible_text.replace('\u{e000}', ""),
        format!("{}\u{e000}0\u{e001}", span.visible_text),
        span.visible_text
            .replace("\u{e000}0\u{e001}", "\u{e000}1\u{e001}"),
    ] {
        assert!(rebuild(
            source,
            &timeline,
            &HashMap::from([(span.id.clone(), value)])
        )
        .is_err());
    }
}

#[test]
fn dangling_and_blank_interrupted_continuations_are_not_spans() {
    for source in [
        "Text\\",
        "Text\\\n",
        "Text\\\n\t\nTail",
        "Text #id:x\\\nMore",
    ] {
        let timeline = parse_timeline(source, &characters()).unwrap();
        assert!(!timeline.diagnostics.is_empty(), "{source:?}");
        assert!(timeline.spans.iter().all(|s| s.id.line > 0));
        assert_eq!(rebuild(source, &timeline, &HashMap::new()).unwrap(), source);
    }
}

#[test]
fn hazardous_translations_are_explicit_errors() {
    let source = "Ordinary words\n";
    let timeline = parse_timeline(source, &characters()).unwrap();
    for value in [
        "",
        "two\nlines",
        "two\rlines",
        "join Aster",
        "[signal]",
        "- choice",
        "if anything",
        "elsewhere",
        "Aster: hello",
        "Unknown: hello",
        "doSomething",
        "setback",
        "# comment",
        "text #id:new",
        "text\\",
        "text {new}",
        "text [b]new[/b]",
        "\u{feff}hidden",
        "text\0",
    ] {
        let error = rebuild(
            source,
            &timeline,
            &HashMap::from([(timeline.spans[0].id.clone(), value.into())]),
        )
        .unwrap_err();
        assert!(!error.to_string().is_empty(), "{value:?}");
    }
}

fn assert_speaker_prefix_translation_error(source: &str, kind: SpanKind, continuation: &str) {
    let timeline = parse_timeline(source, &characters()).unwrap();
    assert!(timeline.diagnostics.is_empty());
    assert_eq!(timeline.spans.len(), 1);
    let span = &timeline.spans[0];
    assert_eq!(span.kind, kind);
    assert!(span.speaker.is_none());
    for prefix in [
        "Aster: ",
        "Aster(happy): ",
        "Aster (happy): ",
        "\"Blue Bird\": ",
        "\"Blue Bird\" (calm): ",
    ] {
        let value = format!("{prefix}Hola{continuation}");
        let error = rebuild(
            source,
            &timeline,
            &HashMap::from([(span.id.clone(), value.clone())]),
        )
        .unwrap_err();
        assert_eq!(
            error.to_string(),
            "translation introduces a speaker prefix",
            "{kind:?}: {value:?}"
        );
    }
}

#[test]
fn plain_text_translations_refuse_speaker_prefixes_with_specific_error() {
    assert_speaker_prefix_translation_error("Ordinary words\n", SpanKind::Text, "");
}

#[test]
fn continued_text_translations_refuse_speaker_prefixes_with_specific_error() {
    assert_speaker_prefix_translation_error(
        "First\\\n  Second\n",
        SpanKind::TextContinued,
        "\nSegundo",
    );
}

#[test]
fn translations_reparsed_as_empty_text_sentinels_are_refused() {
    for (source, value) in [
        ("Ordinary words\n", "<Empty Text Event>"),
        ("First\\\n  Second\n", "First\n<Empty Text Event>"),
        ("- An apple\n", "<Empty Text Event>"),
    ] {
        let timeline = parse_timeline(source, &characters()).unwrap();
        assert!(timeline.diagnostics.is_empty());
        assert_eq!(timeline.spans.len(), 1);
        let error = rebuild(
            source,
            &timeline,
            &HashMap::from([(timeline.spans[0].id.clone(), value.into())]),
        )
        .unwrap_err();
        assert_eq!(
            error.to_string(),
            "translation changes timeline event structure",
            "{source:?}: {value:?}"
        );
    }
}

#[test]
fn continuations_refuse_segment_changes_and_placeholder_moves() {
    let source = "First {name}\\\nSecond [b]bold[/b]\n";
    let timeline = parse_timeline(source, &characters()).unwrap();
    let span = &timeline.spans[0];
    for value in [
        "One".into(),
        format!("{}\nextra", span.visible_text),
        span.visible_text
            .split('\n')
            .rev()
            .collect::<Vec<_>>()
            .join("\n"),
    ] {
        assert!(rebuild(
            source,
            &timeline,
            &HashMap::from([(span.id.clone(), value)])
        )
        .is_err());
    }
}

#[test]
fn stale_and_modified_timelines_and_unknown_ids_are_refused() {
    let source = "Hello";
    let mut timeline = parse_timeline(source, &characters()).unwrap();
    assert!(rebuild("Other", &timeline, &HashMap::new()).is_err());
    let mut id = timeline.spans[0].id.clone();
    id.line += 1;
    assert!(rebuild(source, &timeline, &HashMap::from([(id, "Hola".into())])).is_err());
    timeline.spans[0].range = 0..1;
    assert!(rebuild(source, &timeline, &HashMap::new()).is_err());
}

#[test]
fn crlf_and_final_newline_policy_are_byte_exact() {
    for source in [
        "\tAster: Hi  \r\n\r\nTail",
        "Hi\n",
        "Hi\r\n",
        "Hi",
        "",
        "\n",
        "\r\n",
    ] {
        let timeline = parse_timeline(source, &characters()).unwrap();
        assert_eq!(rebuild(source, &timeline, &HashMap::new()).unwrap(), source);
        let translations = timeline
            .spans
            .iter()
            .map(|s| (s.id.clone(), s.visible_text.replace("Hi", "Hola")))
            .collect();
        assert_eq!(
            rebuild(source, &timeline, &translations).unwrap(),
            source.replace("Hi", "Hola")
        );
    }
}

#[test]
fn character_variant_dictionary_is_inert() {
    let source = r#"{
        "@subpath": NodePath("."), "@path": "res://unused.gd",
        "display_name": "Aster \"Star\"\n\u2605", "nicknames": ["Ash", "St\u0061r"],
        "color": Color(1, 0.5, 0, 1), "custom": {"display_name": "Ignore", "calls": [Dangerous("a,b}"), {"x": 1}]}
    }"#;
    let info = parse_character(source).unwrap();
    assert_eq!(info.display_name.as_deref(), Some("Aster \"Star\"\n★"));
    assert_eq!(info.nicknames, ["Ash", "Star"]);
    assert_eq!(info.diagnostics.len(), 1);
    let mut set = CharacterSet::default();
    set.insert_filename("res://cast/FileName.dch").unwrap();
    let timeline = parse_timeline("FileName: Yes\nAsh: No\n", &set).unwrap();
    assert_eq!(timeline.spans.len(), 1);
    assert_eq!(timeline.diagnostics.len(), 1);
}

#[test]
fn malformed_character_fields_and_nesting_are_refused() {
    for text in [
        "[]",
        "{} trailing",
        "{\"display_name\": 123}",
        "{\"nicknames\": [1]}",
        "{\"display_name\":\"a\",\"display_name\":\"b\"}",
        "{\"x\": Color(1,2]}",
        "{\"x\": }",
        "{\"display_name\":\"\\q\"}",
    ] {
        assert!(parse_character(text).is_err(), "{text}");
    }
    assert!(parse_character(&format!("{{\"x\": {}0{}}}", "[".repeat(65), "]".repeat(65))).is_err());
    assert!(CharacterSet::default()
        .insert_filename("invalid.txt")
        .is_err());
}

#[test]
fn explicit_input_line_and_rebuild_bounds() {
    let too_long = "a".repeat(MAX_LINE_BYTES + 1);
    assert!(parse_timeline(&too_long, &characters()).is_err());
    assert!(parse_character(&too_long).is_err());
    let oversized = "a\n".repeat(MAX_INPUT_BYTES / 2 + 1);
    assert!(parse_timeline(&oversized, &characters()).is_err());
    assert!(parse_character(&oversized).is_err());
    let boundary = "a".repeat(MAX_LINE_BYTES);
    assert!(parse_timeline(&boundary, &characters()).is_ok());
    let source = "Hello";
    let timeline = parse_timeline(source, &characters()).unwrap();
    assert!(rebuild(
        source,
        &timeline,
        &HashMap::from([(timeline.spans[0].id.clone(), too_long)])
    )
    .is_err());
}

#[test]
fn exact_total_input_bound_and_rebuilt_output_bound() {
    let command_line = format!("#{}\n", "x".repeat(MAX_LINE_BYTES - 1));
    let mut source = command_line.repeat(MAX_INPUT_BYTES / command_line.len());
    source.push_str(&format!(
        "#{}\nHi",
        "x".repeat(MAX_INPUT_BYTES - source.len() - 4)
    ));
    assert_eq!(source.len(), MAX_INPUT_BYTES);
    let timeline = parse_timeline(&source, &characters()).unwrap();
    assert_eq!(timeline.spans.len(), 1);
    assert_eq!(
        rebuild(&source, &timeline, &HashMap::new()).unwrap(),
        source
    );
    assert!(rebuild(
        &source,
        &timeline,
        &HashMap::from([(timeline.spans[0].id.clone(), "Longer".into())])
    )
    .is_err());
}

#[test]
fn unicode_byte_ranges_and_duplicate_ids_remain_positional() {
    let source = "\tAster: Árbol 星 #id:same\nAster: Árbol 星 #id:same";
    let timeline = parse_timeline(source, &characters()).unwrap();
    assert_eq!(timeline.spans.len(), 2);
    assert_ne!(timeline.spans[0].id, timeline.spans[1].id);
    assert_eq!(&source[timeline.spans[0].range.clone()], "Árbol 星");
    let translations = HashMap::from([(timeline.spans[0].id.clone(), "Jardín 月".into())]);
    assert_eq!(
        rebuild(source, &timeline, &translations).unwrap(),
        source.replacen("Árbol 星", "Jardín 月", 1)
    );
    let mut set = characters();
    assert!(set.insert_filename("res://other/Aster.dch").is_err());
}

#[test]
#[ignore = "read-only private corpus; set LOCUST_DIALOGIC_CORPUS"]
fn installed_corpus_identity_and_marker_reparse() {
    let root =
        std::env::var_os("LOCUST_DIALOGIC_CORPUS").expect("LOCUST_DIALOGIC_CORPUS directory");
    let paths = walkdir::WalkDir::new(root)
        .into_iter()
        .map(|e| e.unwrap().into_path())
        .collect::<Vec<_>>();
    let mut set = CharacterSet::default();
    let mut character_count = 0;
    for path in &paths {
        if path.extension().is_some_and(|s| s == "dch") {
            parse_character(&std::fs::read_to_string(path).unwrap()).unwrap();
            set.insert_filename(path.to_str().unwrap()).unwrap();
            character_count += 1;
        }
    }
    let mut files = 0;
    let mut spans = 0;
    let mut reasons = BTreeMap::new();
    for path in &paths {
        if path.extension().is_none_or(|s| s != "dtl") {
            continue;
        }
        let original = std::fs::read_to_string(path).unwrap();
        let timeline = parse_timeline(&original, &set).unwrap();
        assert_eq!(
            rebuild(&original, &timeline, &HashMap::new()).unwrap(),
            original
        );
        let translations = timeline
            .spans
            .iter()
            .map(|s| (s.id.clone(), format!("LOCUST_MARKER {}", s.visible_text)))
            .collect();
        let output = rebuild(&original, &timeline, &translations).unwrap();
        let again = parse_timeline(&output, &set).unwrap();
        assert_eq!(
            timeline.spans.iter().map(|s| &s.id).collect::<Vec<_>>(),
            again.spans.iter().map(|s| &s.id).collect::<Vec<_>>()
        );
        assert_eq!(timeline.diagnostics, again.diagnostics);
        files += 1;
        spans += timeline.spans.len();
        for diagnostic in &timeline.diagnostics {
            *reasons.entry(diagnostic.reason.clone()).or_insert(0usize) += 1;
        }
    }
    assert!(files > 0);
    println!("Dialogic corpus: files={files}, characters={character_count}, spans={spans}, diagnostics={}", reasons.values().sum::<usize>());
    for (reason, count) in reasons {
        println!("  {reason}: {count}");
    }
}
