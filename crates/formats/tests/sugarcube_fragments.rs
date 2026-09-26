use locust_core::extraction::FormatPlugin;
use locust_core::models::StringEntry;
use locust_formats::sugarcube::SugarCubePlugin;
use std::fs;

fn write_game(html: &str) -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("game.html");
    fs::write(&path, html).unwrap();
    (dir, path)
}

fn by_id(entries: &[StringEntry]) -> Vec<(&str, &str)> {
    entries
        .iter()
        .map(|entry| (entry.id.as_str(), entry.source.as_str()))
        .collect()
}

fn entry<'a>(entries: &'a [StringEntry], id: &str) -> &'a StringEntry {
    entries
        .iter()
        .find(|entry| entry.id == id)
        .unwrap_or_else(|| panic!("missing {id} in {ids:?}", ids = by_id(entries)))
}

fn translated(entries: &[StringEntry], id: &str, text: &str) -> StringEntry {
    let mut entry = entry(entries, id).clone();
    entry.translation = Some(text.to_string());
    entry
}

#[test]
fn duplicate_link_labels_use_stable_fragment_indices() {
    let original = r#"<tw-passagedata pid="1" name="Chapter#One" tags="">Take [[Go|Alpha]] or [[Go|Beta]] now.</tw-passagedata>"#;
    let (_dir, path) = write_game(original);
    let plugin = SugarCubePlugin::new();
    let entries = plugin.extract(&path).unwrap();
    assert_eq!(
        by_id(&entries),
        vec![
            ("passage_1#Chapter#One#0#f0", "Take "),
            ("passage_1#Chapter#One#0#f1", "Go"),
            ("passage_1#Chapter#One#0#f2", " or "),
            ("passage_1#Chapter#One#0#f3", "Go"),
            ("passage_1#Chapter#One#0#f4", " now."),
        ]
    );

    let report = plugin
        .inject(
            &path,
            &[translated(&entries, "passage_1#Chapter#One#0#f1", "Ir")],
        )
        .unwrap();
    let changed = fs::read_to_string(&path).unwrap();
    assert_eq!(report.strings_written, 1);
    assert!(changed.contains("[[&quot;Ir&quot;|Alpha]]"));
    assert!(changed.contains("[[Go|Beta]]"));
    assert!(!changed.contains("[[Ir|Beta]]"));
    assert!(!changed.contains("[[&quot;Ir&quot;|Beta]]"));
}

#[test]
fn inline_style_link_and_conditional_fragments_stay_in_place() {
    let original = concat!(
        r#"<tw-passagedata pid="8" name="Mix" tags="">"#,
        r#"Hello <span class="red" title="hidden">world</span>!"#,
        r#"&lt;&lt;if $ok &gt; 0&gt;&gt;Yes&lt;&lt;else&gt;&gt;No&lt;&lt;/if&gt;&gt;"#,
        r#"[[Stay|Keep]]"#,
        "</tw-passagedata>",
    );
    let (_dir, path) = write_game(original);
    let plugin = SugarCubePlugin::new();
    let entries = plugin.extract(&path).unwrap();
    assert_eq!(
        by_id(&entries),
        vec![
            ("passage_8#Mix#0#f0", "Hello "),
            ("passage_8#Mix#0#f1", "world"),
            ("passage_8#Mix#0#f2", "Yes"),
            ("passage_8#Mix#0#f3", "No"),
            ("passage_8#Mix#0#f4", "Stay"),
        ]
    );
    assert!(entries.iter().all(|entry| {
        !entry.source.contains("hidden")
            && !entry.source.contains("Keep")
            && !entry.source.contains("$ok")
            && !entry.source.contains("class")
    }));
    let report = plugin
        .inject(
            &path,
            &[
                translated(&entries, "passage_8#Mix#0#f1", "mundo"),
                translated(&entries, "passage_8#Mix#0#f2", "Sí"),
                translated(&entries, "passage_8#Mix#0#f3", "Nunca"),
                translated(&entries, "passage_8#Mix#0#f4", "Quédate"),
            ],
        )
        .unwrap();
    let changed = fs::read_to_string(&path).unwrap();
    assert_eq!(report.strings_written, 4);
    assert!(changed.contains(r#"<span class="red" title="hidden">mundo</span>"#));
    assert!(changed
        .contains("&lt;&lt;if $ok &gt; 0&gt;&gt;Sí&lt;&lt;else&gt;&gt;Nunca&lt;&lt;/if&gt;&gt;"));
    assert!(changed.contains("[[&quot;Quédate&quot;|Keep]]"));
    assert!(changed.contains("Hello "));
}

#[test]
fn left_arrow_link_translates_display_not_destination() {
    let original = r#"<tw-passagedata pid="2" name="Nav" tags="">Go [[Hidden<-Visible]] here.</tw-passagedata>"#;
    let (_dir, path) = write_game(original);
    let plugin = SugarCubePlugin::new();
    let entries = plugin.extract(&path).unwrap();
    assert_eq!(
        by_id(&entries),
        vec![
            ("passage_2#Nav#0#f0", "Go "),
            ("passage_2#Nav#0#f1", "Visible"),
            ("passage_2#Nav#0#f2", " here."),
        ]
    );
    let report = plugin
        .inject(
            &path,
            &[translated(&entries, "passage_2#Nav#0#f1", "Visible")],
        )
        .unwrap();
    assert_eq!(report.strings_written, 0);
    assert_eq!(report.skip_reasons.get("unchanged"), Some(&1));
    assert_eq!(fs::read_to_string(&path).unwrap(), original);

    let report = plugin
        .inject(
            &path,
            &[translated(&entries, "passage_2#Nav#0#f1", "Mostrar")],
        )
        .unwrap();
    let changed = fs::read_to_string(&path).unwrap();
    assert_eq!(report.strings_written, 1);
    assert!(changed.contains("[[Hidden<-&quot;Mostrar&quot;]]"));
    assert!(!changed.contains("[[Mostrar<-"));
}

#[test]
fn bare_link_destination_is_not_extracted() {
    let original =
        r#"<tw-passagedata pid="3" name="Bare" tags="">See [[Next]] later.</tw-passagedata>"#;
    let (_dir, path) = write_game(original);
    let plugin = SugarCubePlugin::new();
    let entries = plugin.extract(&path).unwrap();
    assert_eq!(
        by_id(&entries),
        vec![
            ("passage_3#Bare#0#f0", "See "),
            ("passage_3#Bare#0#f1", " later."),
        ]
    );
    assert!(entries.iter().all(|entry| entry.source != "Next"));
}

#[test]
fn punctuation_duplicates_keep_distinct_fragment_slots() {
    let original =
        r#"<tw-passagedata pid="5" name="Echo" tags="">Yes. [[Yes|A]] Yes.</tw-passagedata>"#;
    let (_dir, path) = write_game(original);
    let plugin = SugarCubePlugin::new();
    let entries = plugin.extract(&path).unwrap();
    let yes = entries
        .iter()
        .filter(|entry| entry.source.contains("Yes"))
        .map(|entry| entry.id.as_str())
        .collect::<Vec<_>>();
    assert_eq!(yes.len(), 3);
    let report = plugin
        .inject(&path, &[translated(&entries, yes[1], "Sí")])
        .unwrap();
    let changed = fs::read_to_string(&path).unwrap();
    assert_eq!(report.strings_written, 1);
    assert!(changed.contains("Yes. [[&quot;Sí&quot;|A]] Yes."));
}

#[test]
fn stale_source_and_structure_skip_without_rewrite() {
    let original = r#"<tw-passagedata pid="1" name="Start" tags="">Please [[continue|Next]] now.</tw-passagedata>"#;
    let (_dir, path) = write_game(original);
    let plugin = SugarCubePlugin::new();
    let entries = plugin.extract(&path).unwrap();
    let label = translated(&entries, "passage_1#Start#0#f1", "continúa");

    fs::write(
        &path,
        r#"<tw-passagedata pid="1" name="Start" tags="">Please [[changed|Next]] now.</tw-passagedata>"#,
    )
    .unwrap();
    let before = fs::read(&path).unwrap();
    let report = plugin.inject(&path, std::slice::from_ref(&label)).unwrap();
    assert_eq!(report.strings_written, 0);
    assert_eq!(report.skip_reasons.get("source_changed"), Some(&1));
    assert_eq!(fs::read(&path).unwrap(), before);

    fs::write(
        &path,
        r#"<tw-passagedata pid="1" name="Start" tags="">Please [[continue|Next]] [[extra|X]] now.</tw-passagedata>"#,
    )
    .unwrap();
    let before = fs::read(&path).unwrap();
    let report = plugin.inject(&path, &[label]).unwrap();
    assert_eq!(report.strings_written, 0);
    assert_eq!(report.skip_reasons.get("source_changed"), Some(&1));
    assert_eq!(fs::read(&path).unwrap(), before);
}

#[test]
fn legacy_joined_row_is_rejected_and_single_span_ids_are_kept() {
    let original = concat!(
        r#"<tw-passagedata pid="1" name="Start" tags="">Please [[continue|Next]] now."#,
        "\n",
        "Hello there.",
        "</tw-passagedata>",
    );
    let (_dir, path) = write_game(original);
    let plugin = SugarCubePlugin::new();
    let extracted = plugin.extract(&path).unwrap();
    assert!(extracted
        .iter()
        .any(|entry| entry.id == "passage_1#Start#1"));
    assert_eq!(
        entry(&extracted, "passage_1#Start#1").source,
        "Hello there."
    );

    let mut legacy = StringEntry::new("passage_1#Start#0", "Please continue now.", path.clone());
    legacy.translation = Some("No aplastar".into());
    let before = fs::read(&path).unwrap();
    let report = plugin.inject(&path, &[legacy]).unwrap();
    assert_eq!(report.strings_written, 0);
    assert_eq!(report.skip_reasons.get("unsupported_markup"), Some(&1));
    assert_eq!(fs::read(&path).unwrap(), before);

    let mut single = entry(&extracted, "passage_1#Start#1").clone();
    single.translation = Some("Hola allí.".into());
    let report = plugin.inject(&path, &[single]).unwrap();
    assert_eq!(report.strings_written, 1);
    assert!(fs::read_to_string(&path)
        .unwrap()
        .contains("Please [[continue|Next]] now.\nHola allí."));
}

#[test]
fn fragment_syntax_chars_are_encoded_and_do_not_create_markup() {
    let original = r#"<tw-passagedata pid="9" name="Safe" tags="">Please [[continue|Next]] now.</tw-passagedata>"#;
    let (_dir, path) = write_game(original);
    let plugin = SugarCubePlugin::new();
    let entries = plugin.extract(&path).unwrap();
    let report = plugin
        .inject(
            &path,
            &[translated(
                &entries,
                "passage_9#Safe#0#f1",
                r#"see [[Nope|Trap]] a < b & "c""#,
            )],
        )
        .unwrap();
    let changed = fs::read_to_string(&path).unwrap();
    assert_eq!(report.strings_written, 1);
    assert!(changed.contains(
        r#"Please [[&quot;see [[Nope|Trap]] a &lt; b &amp; \&quot;c\&quot;&quot;|Next]] now."#
    ));
    assert!(changed.contains("|Next]]"));
    assert!(!changed.contains("[[Nope|Trap]] now"));
    assert!(!changed.contains("&amp;#91;"));

    let reextracted = plugin.extract(&path).unwrap();
    assert_eq!(
        entry(&reextracted, "passage_9#Safe#0#f1").source,
        r#"see [[Nope|Trap]] a < b & "c""#
    );
    assert_eq!(
        by_id(&reextracted)
            .iter()
            .map(|(id, _)| *id)
            .collect::<Vec<_>>(),
        vec![
            "passage_9#Safe#0#f0",
            "passage_9#Safe#0#f1",
            "passage_9#Safe#0#f2",
        ]
    );
}

#[test]
fn whitespace_entities_and_untranslated_bytes_are_preserved() {
    let original = concat!(
        r#"<tw-passagedata pid="6" name="Keep" tags="">"#,
        "Tom &amp; Jerry\n",
        "Please [[continue|Next]] now.",
        "</tw-passagedata>",
    );
    let (_dir, path) = write_game(original);
    let plugin = SugarCubePlugin::new();
    let before = fs::read(&path).unwrap();
    let entries = plugin.extract(&path).unwrap();
    assert_eq!(entry(&entries, "passage_6#Keep#0").source, "Tom & Jerry");

    let report = plugin.inject(&path, &[]).unwrap();
    assert_eq!(report.strings_written, 0);
    assert_eq!(fs::read(&path).unwrap(), before);

    let report = plugin
        .inject(
            &path,
            &[translated(&entries, "passage_6#Keep#1#f1", "continúa")],
        )
        .unwrap();
    let changed = fs::read_to_string(&path).unwrap();
    assert_eq!(report.strings_written, 1);
    assert!(changed.contains("Tom &amp; Jerry\nPlease [[&quot;continúa&quot;|Next]] now."));
    assert_eq!(
        plugin
            .extract(&path)
            .unwrap()
            .iter()
            .find(|entry| entry.id == "passage_6#Keep#1#f1")
            .unwrap()
            .source,
        "continúa"
    );
}

#[test]
fn quoted_gt_in_outer_passage_header_roundtrips() {
    let original = r#"<tw-passagedata pid="4" name="A > B#Hash" tags="x > y">Please [[Go|T]] now.</tw-passagedata>"#;
    let (_dir, path) = write_game(original);
    let plugin = SugarCubePlugin::new();
    let entries = plugin.extract(&path).unwrap();
    assert_eq!(
        by_id(&entries),
        vec![
            ("passage_4#A > B#Hash#0#f0", "Please "),
            ("passage_4#A > B#Hash#0#f1", "Go"),
            ("passage_4#A > B#Hash#0#f2", " now."),
        ]
    );
    let report = plugin
        .inject(
            &path,
            &[translated(&entries, "passage_4#A > B#Hash#0#f1", "Ir")],
        )
        .unwrap();
    let changed = fs::read_to_string(&path).unwrap();
    assert_eq!(report.strings_written, 1);
    assert!(changed.contains(r#"name="A > B#Hash" tags="x > y">Please [[&quot;Ir&quot;|T]] now."#));
}

#[test]
fn html_attributes_macros_and_exact_noop_survive_fragment_inject() {
    let original = r#"<tw-passagedata pid="7" name="Start" tags="">&lt;span data-label=&quot;Open&quot;&gt;&lt;/span&gt;&lt;&lt;set $next = &quot;Secret &gt;&gt; word&quot;&gt;&gt;Open [[Open|Secret]] extra</tw-passagedata>"#;
    let (_dir, path) = write_game(original);
    let plugin = SugarCubePlugin::new();
    let entries = plugin.extract(&path).unwrap();
    assert!(entries.iter().any(|entry| entry.source == "Open"));
    assert!(entries
        .iter()
        .all(|entry| { !entry.source.contains("Secret") && !entry.source.contains("data-label") }));
    let open = entries
        .iter()
        .find(|entry| entry.source == "Open" && entry.id.contains("#f"))
        .unwrap()
        .clone();
    let mut same = open.clone();
    same.translation = Some("Open".into());
    let before = fs::read(&path).unwrap();
    let report = plugin.inject(&path, &[same]).unwrap();
    assert_eq!(report.strings_written, 0);
    assert_eq!(report.skip_reasons.get("unchanged"), Some(&1));
    assert_eq!(fs::read(&path).unwrap(), before);

    let report = plugin
        .inject(&path, &[translated(&entries, &open.id, "Abrir")])
        .unwrap();
    let changed = fs::read_to_string(&path).unwrap();
    assert_eq!(report.strings_written, 1);
    assert!(changed.contains(r#"data-label=&quot;Open&quot;"#));
    assert!(changed.contains(r#"&lt;&lt;set $next = &quot;Secret &gt;&gt; word&quot;&gt;&gt;"#));
    assert!(changed.contains("[[&quot;Abrir&quot;|Secret]]"));
    assert!(changed.contains("Open [[&quot;Abrir&quot;|Secret]] extra"));
}

#[test]
fn arrow_link_and_variable_fragments_roundtrip() {
    let original = r#"<tw-passagedata pid="10" name="Vars" tags="">Hi $name, [[Leave->Door]] now.</tw-passagedata>"#;
    let (_dir, path) = write_game(original);
    let plugin = SugarCubePlugin::new();
    let entries = plugin.extract(&path).unwrap();
    assert_eq!(
        by_id(&entries),
        vec![
            ("passage_10#Vars#0#f0", "Hi {0}, "),
            ("passage_10#Vars#0#f1", "Leave"),
            ("passage_10#Vars#0#f2", " now."),
        ]
    );
    let vars = entry(&entries, "passage_10#Vars#0#f0")
        .metadata
        .get("sugarcube_vars")
        .unwrap();
    assert!(vars.to_string().contains("$name"));

    let report = plugin
        .inject(
            &path,
            &[
                translated(&entries, "passage_10#Vars#0#f0", "Hola {0}, "),
                translated(&entries, "passage_10#Vars#0#f1", "Salir"),
            ],
        )
        .unwrap();
    let changed = fs::read_to_string(&path).unwrap();
    assert_eq!(report.strings_written, 2);
    assert!(changed.contains(r#"Hola $name, [[&quot;Salir&quot;->Door]] now."#));
    let again = plugin.extract(&path).unwrap();
    assert_eq!(entry(&again, "passage_10#Vars#0#f0").source, "Hola {0}, ");
    assert_eq!(entry(&again, "passage_10#Vars#0#f1").source, "Salir");
}

#[test]
fn same_labels_with_changed_link_target_are_rejected() {
    let original = r#"<tw-passagedata pid="1" name="Start" tags="">Please [[continue|Next]] now.</tw-passagedata>"#;
    let (_dir, path) = write_game(original);
    let plugin = SugarCubePlugin::new();
    let entries = plugin.extract(&path).unwrap();
    let label = translated(&entries, "passage_1#Start#0#f1", "continúa");
    fs::write(
        &path,
        r#"<tw-passagedata pid="1" name="Start" tags="">Please [[continue|Else]] now.</tw-passagedata>"#,
    )
    .unwrap();
    let before = fs::read(&path).unwrap();
    let report = plugin.inject(&path, &[label]).unwrap();
    assert_eq!(report.strings_written, 0);
    assert_eq!(report.skip_reasons.get("source_changed"), Some(&1));
    assert_eq!(fs::read(&path).unwrap(), before);
}

#[test]
fn same_labels_with_changed_condition_are_rejected() {
    let original = concat!(
        r#"<tw-passagedata pid="8" name="Mix" tags="">"#,
        r#"Hello &lt;&lt;if $ok &gt; 0&gt;&gt;Yes&lt;&lt;else&gt;&gt;No&lt;&lt;/if&gt;&gt;"#,
        "</tw-passagedata>",
    );
    let (_dir, path) = write_game(original);
    let plugin = SugarCubePlugin::new();
    let entries = plugin.extract(&path).unwrap();
    assert_eq!(
        by_id(&entries),
        vec![
            ("passage_8#Mix#0#f0", "Hello "),
            ("passage_8#Mix#0#f1", "Yes"),
            ("passage_8#Mix#0#f2", "No"),
        ]
    );
    let yes = translated(&entries, "passage_8#Mix#0#f1", "Sí");
    fs::write(
        &path,
        concat!(
            r#"<tw-passagedata pid="8" name="Mix" tags="">"#,
            r#"Hello &lt;&lt;if $ok &gt; 1&gt;&gt;Yes&lt;&lt;else&gt;&gt;No&lt;&lt;/if&gt;&gt;"#,
            "</tw-passagedata>",
        ),
    )
    .unwrap();
    let before = fs::read(&path).unwrap();
    let report = plugin.inject(&path, &[yes]).unwrap();
    assert_eq!(report.strings_written, 0);
    assert_eq!(report.skip_reasons.get("source_changed"), Some(&1));
    assert_eq!(fs::read(&path).unwrap(), before);
}

#[test]
fn missing_fragment_structure_metadata_is_rejected() {
    let original = r#"<tw-passagedata pid="1" name="Start" tags="">Please [[continue|Next]] now.</tw-passagedata>"#;
    let (_dir, path) = write_game(original);
    let plugin = SugarCubePlugin::new();
    let entries = plugin.extract(&path).unwrap();
    let mut label = translated(&entries, "passage_1#Start#0#f1", "continúa");
    label.metadata.remove("sugarcube_structure");
    let before = fs::read(&path).unwrap();
    let report = plugin.inject(&path, &[label]).unwrap();
    assert_eq!(report.strings_written, 0);
    assert_eq!(report.skip_reasons.get("source_changed"), Some(&1));
    assert_eq!(fs::read(&path).unwrap(), before);
}

#[test]
fn sibling_fragment_inject_survives_after_unrelated_translation() {
    let original = r#"<tw-passagedata pid="1" name="Start" tags="">Please [[continue|Next]] now.</tw-passagedata>"#;
    let (_dir, path) = write_game(original);
    let plugin = SugarCubePlugin::new();
    let entries = plugin.extract(&path).unwrap();
    let report = plugin
        .inject(
            &path,
            &[translated(&entries, "passage_1#Start#0#f1", "continúa")],
        )
        .unwrap();
    assert_eq!(report.strings_written, 1);
    let report = plugin
        .inject(
            &path,
            &[translated(&entries, "passage_1#Start#0#f0", "Por favor ")],
        )
        .unwrap();
    let changed = fs::read_to_string(&path).unwrap();
    assert_eq!(report.strings_written, 1);
    assert!(changed.contains("Por favor [[&quot;continúa&quot;|Next]] now."));
}

#[test]
fn dangerous_syntax_stays_literal_and_reextracts() {
    let original = r#"<tw-passagedata pid="9" name="Safe" tags="">Please [[continue|Next]] now.</tw-passagedata>"#;
    let (_dir, path) = write_game(original);
    let plugin = SugarCubePlugin::new();
    let entries = plugin.extract(&path).unwrap();
    let payload = "it's <b>x</b> <<set $coins=99>> [[text|Else]] [ok] \u{2018}said\u{2019}";
    let report = plugin
        .inject(
            &path,
            &[translated(&entries, "passage_9#Safe#0#f1", payload)],
        )
        .unwrap();
    let changed = fs::read_to_string(&path).unwrap();
    assert_eq!(report.strings_written, 1);
    assert!(changed.contains("&lt;b&gt;x&lt;/b&gt;"));
    assert!(changed.contains("&lt;&lt;set $coins=99&gt;&gt;"));
    assert!(changed.contains("[[text|Else]]"));
    assert!(changed.contains("&quot;it's "));
    assert!(!changed.contains("&amp;#39;"));
    assert!(!changed.contains("&amp;#60;"));
    assert!(changed.contains("|Next]]"));
    assert!(!changed.contains("<b>x</b>"));
    assert!(!changed.contains("<<set $coins=99>>"));
    assert!(!changed.contains("[[it's"));

    let reextracted = plugin.extract(&path).unwrap();
    assert_eq!(entry(&reextracted, "passage_9#Safe#0#f1").source, payload);
    assert_eq!(
        by_id(&reextracted)
            .iter()
            .map(|(id, _)| *id)
            .collect::<Vec<_>>(),
        vec![
            "passage_9#Safe#0#f0",
            "passage_9#Safe#0#f1",
            "passage_9#Safe#0#f2",
        ]
    );
}

#[test]
fn json_quoted_link_labels_decode_and_skip_dynamic_expressions() {
    let original = concat!(
        r#"<tw-passagedata pid="11" name="Quoted" tags="">"#,
        r#"Say [[&quot;Hello&quot;|Hall]] now."#,
        "\n",
        r#"Skip [[$name|Hall]] here."#,
        "\n",
        r#"Go [[Leave->Door][$coins to 3]]"#,
        "</tw-passagedata>",
    );
    let (_dir, path) = write_game(original);
    let plugin = SugarCubePlugin::new();
    let entries = plugin.extract(&path).unwrap();
    assert_eq!(
        by_id(&entries),
        vec![
            ("passage_11#Quoted#0#f0", "Say "),
            ("passage_11#Quoted#0#f1", "Hello"),
            ("passage_11#Quoted#0#f2", " now."),
            ("passage_11#Quoted#1#f0", "Skip "),
            ("passage_11#Quoted#1#f1", " here."),
            ("passage_11#Quoted#2#f0", "Go "),
            ("passage_11#Quoted#2#f1", "Leave"),
        ]
    );
    assert!(entries.iter().all(|entry| !entry.source.contains("$name")));
    assert!(entries.iter().all(|entry| !entry.source.contains("$coins")));

    let report = plugin
        .inject(
            &path,
            &[
                translated(&entries, "passage_11#Quoted#0#f1", "it's"),
                translated(&entries, "passage_11#Quoted#2#f1", "Salir"),
            ],
        )
        .unwrap();
    let changed = fs::read_to_string(&path).unwrap();
    assert_eq!(report.strings_written, 2);
    assert!(changed.contains(r#"[[&quot;it's&quot;|Hall]]"#));
    assert!(changed.contains("[[$name|Hall]]"));
    assert!(changed.contains(r#"[[&quot;Salir&quot;->Door][$coins to 3]]"#));

    let again = plugin.extract(&path).unwrap();
    assert_eq!(entry(&again, "passage_11#Quoted#0#f1").source, "it's");
    assert_eq!(
        entry(&again, "passage_11#Quoted#0#f1").id,
        "passage_11#Quoted#0#f1"
    );
    assert_eq!(entry(&again, "passage_11#Quoted#2#f1").source, "Salir");
}

#[test]
fn link_label_escapes_quotes_backslashes_newlines_and_separators() {
    let original = r#"<tw-passagedata pid="12" name="Sep" tags="">X [[Stay|Start]] Y [[Go->Hall]] Z [[Hall<-Back]]</tw-passagedata>"#;
    let (_dir, path) = write_game(original);
    let plugin = SugarCubePlugin::new();
    let entries = plugin.extract(&path).unwrap();
    let payload = "say \"hi\"\\\npipe|here [[x]] -> y <- z";
    let report = plugin
        .inject(
            &path,
            &[
                translated(&entries, "passage_12#Sep#0#f1", payload),
                translated(&entries, "passage_12#Sep#0#f3", "G"),
                translated(&entries, "passage_12#Sep#0#f5", "B"),
            ],
        )
        .unwrap();
    let changed = fs::read_to_string(&path).unwrap();
    assert_eq!(report.strings_written, 3);
    assert!(changed.contains("|Start]]"));
    assert!(changed.contains("->Hall]]"));
    assert!(changed.contains("[[Hall<-"));
    assert!(changed.contains(r#"\&quot;hi\&quot;"#));
    assert!(changed.contains("pipe|here"));
    let again = plugin.extract(&path).unwrap();
    assert_eq!(entry(&again, "passage_12#Sep#0#f1").source, payload);
    assert_eq!(entry(&again, "passage_12#Sep#0#f3").source, "G");
    assert_eq!(entry(&again, "passage_12#Sep#0#f5").source, "B");
}

#[test]
fn prose_apostrophe_stays_two_layer_while_link_uses_json() {
    let original = r#"<tw-passagedata pid="13" name="Both" tags="">Please [[continue|Next]] now.</tw-passagedata>"#;
    let (_dir, path) = write_game(original);
    let plugin = SugarCubePlugin::new();
    let entries = plugin.extract(&path).unwrap();
    let report = plugin
        .inject(
            &path,
            &[
                translated(&entries, "passage_13#Both#0#f0", "it's "),
                translated(&entries, "passage_13#Both#0#f1", "it's"),
            ],
        )
        .unwrap();
    let changed = fs::read_to_string(&path).unwrap();
    assert_eq!(report.strings_written, 2);
    assert!(changed.contains("it&amp;#39;s "));
    assert!(changed.contains(r#"[[&quot;it's&quot;|Next]]"#));
    let again = plugin.extract(&path).unwrap();
    assert_eq!(entry(&again, "passage_13#Both#0#f0").source, "it's ");
    assert_eq!(entry(&again, "passage_13#Both#0#f1").source, "it's");
}
