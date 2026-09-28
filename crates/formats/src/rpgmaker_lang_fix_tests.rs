use super::*;
use std::fs;
use std::path::Path;

const REGISTRATION_PLUGINS: &str = r#"var $plugins = [{"name":"Iavra_MZ_Localization_byNeomaStudio","status":true,"parameters":{"Languages":"jp, en, zh","Language Labels":"en:English, jp:日本語, zh:中文"}}];
const langs = ['jp', 'en', 'zh'];
const length = TextManager.optionsCoreFonts.length - 1;
"#;

fn game_snapshot(root: &Path) -> std::collections::BTreeMap<PathBuf, Vec<u8>> {
    walkdir::WalkDir::new(root)
        .into_iter()
        .map(|entry| entry.unwrap())
        .filter(|entry| entry.file_type().is_file())
        .map(|entry| {
            (
                entry.path().strip_prefix(root).unwrap().to_owned(),
                fs::read(entry.path()).unwrap(),
            )
        })
        .collect()
}

fn assert_failed_map_leaves_game_unchanged(bad_name: &str, bad_bytes: &[u8]) {
    for existing_backup in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        write_game_with_map(
            dir.path(),
            "Map001.json",
            &map_json_from_list(language_choice_list()),
            Some(REGISTRATION_PLUGINS),
        );
        fs::write(dir.path().join("data").join(bad_name), bad_bytes).unwrap();
        if existing_backup {
            fs::write(
                dir.path().join("js/plugins.js.bak-locust"),
                b"backup from an earlier registration",
            )
            .unwrap();
        }
        let before = game_snapshot(dir.path());
        assert!(register_language(dir.path(), "es", "Español").is_err());
        assert_eq!(game_snapshot(dir.path()), before);

        fs::remove_file(dir.path().join("data").join(bad_name)).unwrap();
        let report = register_language(dir.path(), "es", "Español").unwrap();
        assert!(report.plugins_js && report.iavra_languages && report.visumz_options);
        assert_eq!(report.maps_patched, [dir.path().join("data/Map001.json")]);
        assert_eq!(
            report.backups,
            [
                dir.path().join("js/plugins.js.bak-locust"),
                dir.path().join("data/Map001.json.bak-locust")
            ]
        );
        let registered = game_snapshot(dir.path());
        let report = register_language(dir.path(), "es", "Español").unwrap();
        assert!(!report.plugins_js && !report.iavra_languages && !report.visumz_options);
        assert!(report.maps_patched.is_empty());
        assert_eq!(
            report.backups,
            [dir.path().join("js/plugins.js.bak-locust")]
        );
        assert_eq!(
            report.notes,
            [
                "plugins.js present but no Iavra/VisuMZ language patterns matched",
                "nothing changed — already registered, or game has no Iavra/VisuMZ/Map language hooks"
            ]
        );
        assert_eq!(game_snapshot(dir.path()), registered);
    }
}

#[test]
fn test_registration_malformed_later_map_preserves_every_file() {
    assert_failed_map_leaves_game_unchanged("Map999.json", b"{malformed");
}

#[test]
fn test_registration_undecodable_later_jsono_preserves_every_file() {
    assert!(lz_str::decompress_from_base64("A").is_none());
    assert_failed_map_leaves_game_unchanged("Map999.jsono", b"A");
}

#[test]
fn test_registration_refuses_held_game_lock() {
    let dir = tempfile::tempdir().unwrap();
    write_game_with_map(
        dir.path(),
        "Map001.json",
        &map_json_from_list(language_choice_list()),
        Some(REGISTRATION_PLUGINS),
    );
    let before = game_snapshot(dir.path());
    let _lock = locust_core::patch::GameLock::acquire(dir.path()).unwrap();
    let error = register_language(dir.path(), "es", "Español").unwrap_err();
    assert!(error.to_string().contains("game busy"), "{error}");
    assert_eq!(game_snapshot(dir.path()), before);
}

#[test]
fn test_registration_refuses_pending_injection() {
    for phase in [
        "preparing",
        "applying",
        "committed_unrecorded",
        "rolling_back",
    ] {
        let dir = tempfile::tempdir().unwrap();
        write_game_with_map(
            dir.path(),
            "Map001.json",
            &map_json_from_list(language_choice_list()),
            Some(REGISTRATION_PLUGINS),
        );
        let root = dir.path().canonicalize().unwrap();
        let store = root.join(locust_core::injection_transaction::STORE_DIR);
        let id = uuid::Uuid::new_v4().to_string();
        let operation = store.join("operations").join(&id);
        fs::create_dir_all(&operation).unwrap();
        fs::write(
            store.join("store.json"),
            serde_json::json!({
                "schema_version": 1, "kind": "locust-injection-store", "game_root": root
            })
            .to_string(),
        )
        .unwrap();
        fs::write(
            store.join("active.json"),
            serde_json::json!({
                "schema_version": 1, "transaction_id": id, "game_root": root,
                "format": "rpgmaker_mv", "language": "es"
            })
            .to_string(),
        )
        .unwrap();
        fs::write(operation.join("phase.json"), format!("\"{phase}\"")).unwrap();
        let before = game_snapshot(dir.path());
        let error = register_language(dir.path(), "es", "Español").unwrap_err();
        assert!(error.to_string().contains("is unfinished"), "{error}");
        assert_eq!(game_snapshot(dir.path()), before);
    }
}

#[cfg(windows)]
#[test]
fn test_registration_write_failure_restores_every_file() {
    use std::os::windows::fs::OpenOptionsExt;

    for existing_backup in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let map = map_json_from_list(language_choice_list());
        write_game_with_map(dir.path(), "Map001.json", &map, Some(REGISTRATION_PLUGINS));
        write_game_with_map(dir.path(), "Map999.jsono", &map, None);
        if existing_backup {
            fs::write(
                dir.path().join("js/plugins.js.bak-locust"),
                b"backup from an earlier registration",
            )
            .unwrap();
        }
        let blocked = fs::read_dir(dir.path().join("data"))
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| {
                path.file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with("Map")
            })
            .last()
            .unwrap();
        // Permit reads/backups, but make the later map's actual write fail.
        let _reader = fs::OpenOptions::new()
            .read(true)
            .share_mode(1)
            .open(&blocked)
            .unwrap();
        let expected_error = fs::write(&blocked, b"probe").unwrap_err();
        let before = game_snapshot(dir.path());
        let error = register_language(dir.path(), "es", "Español").unwrap_err();
        assert!(
            error.to_string().contains(&expected_error.to_string()),
            "{error}"
        );
        assert_eq!(game_snapshot(dir.path()), before);
    }
}

#[test]
fn test_registration_utf8_length_clamp_window() {
    let needle = "TextManager.optionsCoreFonts.length - 1";
    let raw = format!(
        "var $plugins = []; /*日{} langs */ {needle};",
        " ".repeat(588)
    );
    let start = raw.find(needle).unwrap() - 600;
    assert!(!raw.is_char_boundary(start));
    for fixture in [raw.replace('日', "abc"), raw] {
        let out = rewrite_lang_length_clamps(&fixture);
        assert_eq!(out, fixture.replace(needle, "(langs.length - 1)"));
        assert!(std::str::from_utf8(out.as_bytes()).is_ok());
    }
}

#[test]
fn test_registration_utf8_fontfaces_window() {
    let mut raw =
        r#"var $plugins = [{"parameters":{"FontFaces:arraystr":"['jp','en']"}}]; /*"#.to_string();
    let end = raw.find("FontFaces:arraystr").unwrap() + 200;
    raw.push_str(&" ".repeat(end - 1 - raw.len()));
    raw.push_str("日本語*/");
    assert!(!raw.is_char_boundary(end));
    for fixture in [raw.replace("日本語", "abcdefghi"), raw] {
        let out = extend_fontfaces_array(&fixture, "es").unwrap();
        assert_eq!(out, fixture.replace("['jp','en']", "['jp','en','es']"));
        assert!(std::str::from_utf8(out.as_bytes()).is_ok());
    }
}

#[test]
fn test_registration_utf8_draw_label_window() {
    let draw = r#"this.drawText('中文', fx3, rect.y, segment, "center")"#;
    let raw = format!("var $plugins = []; /*日{}*/ {draw};", " ".repeat(31));
    let start = raw.find("drawText(").unwrap() - 40;
    assert!(!raw.is_char_boundary(start));
    let insert = r#"\nthis.changePaintOpacity((value==3));\nconst fx4 = rect.x + halfWidth + (segment * 3);\nthis.drawText('Spanish', fx4, rect.y, segment, "center")"#;
    for fixture in [raw.replace('日', "abc"), raw] {
        let out = append_language_draw_label(&fixture, "es", "Spanish").unwrap();
        assert_eq!(out, fixture.replace(draw, &format!("{draw}{insert}")));
        assert!(std::str::from_utf8(out.as_bytes()).is_ok());
    }
}

fn cmd(code: u64, indent: i64, parameters: serde_json::Value) -> serde_json::Value {
    serde_json::json!({ "code": code, "indent": indent, "parameters": parameters })
}

fn rewritten(script: &str, lang: &str, index: i32) -> String {
    match rewrite_lang_script(script, lang, index) {
        LangScriptRewrite::Rewritten(s) => s,
        other => panic!("expected Rewritten, got {other:?} for {script:?}"),
    }
}

fn language_choice_list() -> Vec<serde_json::Value> {
    vec![
        cmd(
            102,
            0,
            serde_json::json!([["日本語", "ENGLISH", "中文"], -1, 0, 1, 0]),
        ),
        cmd(402, 0, serde_json::json!([0, "日本語"])),
        cmd(
            355,
            1,
            serde_json::json!(["IAVRA.MasterLocalization.I18N.language = \"jp\";"]),
        ),
        cmd(655, 1, serde_json::json!(["ConfigManager.lang = 0;"])),
        cmd(0, 1, serde_json::json!([])),
        cmd(402, 0, serde_json::json!([1, "ENGLISH"])),
        cmd(
            355,
            1,
            serde_json::json!(["IAVRA.MasterLocalization.I18N.language = \"en\";"]),
        ),
        cmd(655, 1, serde_json::json!(["ConfigManager.lang = 1;"])),
        cmd(0, 1, serde_json::json!([])),
        cmd(402, 0, serde_json::json!([2, "中文"])),
        cmd(
            355,
            1,
            serde_json::json!(["IAVRA.MasterLocalization.I18N.language = \"zh\";"]),
        ),
        cmd(655, 1, serde_json::json!(["ConfigManager.lang = 2;"])),
        cmd(0, 1, serde_json::json!([])),
        cmd(404, 0, serde_json::json!([])),
        cmd(
            355,
            0,
            serde_json::json!(["ConfigManager.language = IAVRA.MasterLocalization.I18N.language;"]),
        ),
        cmd(0, 0, serde_json::json!([])),
    ]
}

fn nested_language_choice_list() -> Vec<serde_json::Value> {
    vec![
        cmd(
            102,
            0,
            serde_json::json!([["日本語", "ENGLISH", "中文"], -1, 0, 1, 0]),
        ),
        cmd(402, 0, serde_json::json!([0, "日本語"])),
        cmd(
            355,
            1,
            serde_json::json!(["IAVRA.MasterLocalization.I18N.language = \"jp\";"]),
        ),
        cmd(102, 1, serde_json::json!([["Yes", "No"], 0, 0, 1, 0])),
        cmd(402, 1, serde_json::json!([0, "Yes"])),
        cmd(0, 2, serde_json::json!([])),
        cmd(402, 1, serde_json::json!([1, "No"])),
        cmd(0, 2, serde_json::json!([])),
        cmd(404, 1, serde_json::json!([])),
        cmd(0, 1, serde_json::json!([])),
        cmd(402, 0, serde_json::json!([1, "ENGLISH"])),
        cmd(
            355,
            1,
            serde_json::json!(["IAVRA.MasterLocalization.I18N.language = \"en\";"]),
        ),
        cmd(655, 1, serde_json::json!(["ConfigManager.lang = 1;"])),
        cmd(102, 1, serde_json::json!([["Yes", "No"], 0, 0, 1, 0])),
        cmd(402, 1, serde_json::json!([0, "Yes"])),
        cmd(0, 2, serde_json::json!([])),
        cmd(402, 1, serde_json::json!([1, "No"])),
        cmd(0, 2, serde_json::json!([])),
        cmd(404, 1, serde_json::json!([])),
        cmd(0, 1, serde_json::json!([])),
        cmd(402, 0, serde_json::json!([2, "中文"])),
        cmd(
            355,
            1,
            serde_json::json!(["IAVRA.MasterLocalization.I18N.language = \"zh\";"]),
        ),
        cmd(0, 1, serde_json::json!([])),
        cmd(404, 0, serde_json::json!([])),
        cmd(0, 0, serde_json::json!([])),
    ]
}

fn write_game_with_map(root: &Path, map_name: &str, map_json: &str, plugins: Option<&str>) {
    let data = root.join("data");
    fs::create_dir_all(&data).unwrap();
    fs::write(data.join("System.json"), r#"{"gameTitle":"T"}"#).unwrap();
    if map_name.ends_with(".jsono") {
        fs::write(data.join(map_name), lz_str::compress_to_base64(map_json)).unwrap();
    } else {
        fs::write(data.join(map_name), map_json).unwrap();
    }
    if let Some(plugins) = plugins {
        let js = root.join("js");
        fs::create_dir_all(&js).unwrap();
        fs::write(js.join("plugins.js"), plugins).unwrap();
    }
}

fn map_json_from_list(list: Vec<serde_json::Value>) -> String {
    serde_json::json!({
        "events": [
            null,
            { "pages": [{ "list": list }] }
        ]
    })
    .to_string()
}

#[test]
fn test_rewrite_preserves_unrelated_assignments() {
    let out = rewritten(
        "ConfigManager.lang = 1; ConfigManager.save(); SoundManager.playOk();",
        "es",
        3,
    );
    assert_eq!(
        out,
        "ConfigManager.lang = 3; ConfigManager.save(); SoundManager.playOk();"
    );

    let out = rewritten(
        "IAVRA.MasterLocalization.I18N.language = \"en\"; ConfigManager.language = IAVRA.MasterLocalization.I18N.language;",
        "es",
        3,
    );
    assert!(out.contains("I18N.language = \"es\""), "{out}");
    assert!(
        out.contains("ConfigManager.language = IAVRA.MasterLocalization.I18N.language;"),
        "{out}"
    );
    assert!(
        !out.contains("ConfigManager.lang ="),
        "must not invent a numeric lang assign from ConfigManager.language: {out}"
    );

    let commenty = rewritten(
        "  ConfigManager.lang = 1; // keep ConfigManager.lang = 9 in comment",
        "es",
        3,
    );
    assert_eq!(
        commenty,
        "  ConfigManager.lang = 3; // keep ConfigManager.lang = 9 in comment"
    );

    let quoted = rewritten(
        "ConfigManager.lang = 1; const note = \"ConfigManager.lang = 9\";",
        "es",
        3,
    );
    assert_eq!(
        quoted,
        "ConfigManager.lang = 3; const note = \"ConfigManager.lang = 9\";"
    );
}

#[test]
fn test_rewrite_two_assignments_same_line() {
    let out = rewritten(
        "IAVRA.MasterLocalization.I18N.language = \"en\"; ConfigManager.lang = 1;",
        "es",
        3,
    );
    assert_eq!(
        out,
        "IAVRA.MasterLocalization.I18N.language = \"es\"; ConfigManager.lang = 3;"
    );

    let spaced = rewritten("I18N.language =  'en';  ConfigManager.lang =  1;", "es", 3);
    assert_eq!(spaced, "I18N.language =  'es';  ConfigManager.lang =  3;");
}

#[test]
fn test_rewrite_config_manager_language_is_not_numeric_lang() {
    assert_eq!(
        rewrite_lang_script(
            "ConfigManager.language = IAVRA.MasterLocalization.I18N.language;",
            "es",
            3
        ),
        LangScriptRewrite::Unrelated(
            "ConfigManager.language = IAVRA.MasterLocalization.I18N.language;".into()
        )
    );
    assert_eq!(
        rewrite_lang_script("ConfigManager.language = 1;", "es", 3),
        LangScriptRewrite::Unrelated("ConfigManager.language = 1;".into())
    );
    assert_eq!(
        rewrite_lang_script("I18N.language = langs[value];", "es", 3),
        LangScriptRewrite::Refuse
    );
    assert_eq!(
        rewrite_lang_script("ConfigManager.lang = langs[value];", "es", 3),
        LangScriptRewrite::Refuse
    );
}

#[test]
fn test_patch_event_list_nested_choices() {
    let mut list = nested_language_choice_list();
    let original_inner = list
        .iter()
        .filter(|c| event_code(c) == 102 && event_indent(c) == 1)
        .count();
    assert_eq!(original_inner, 2);

    assert!(patch_event_list(&mut list, "es", "Español"));

    let outer = list
        .iter()
        .find(|c| event_code(c) == 102 && event_indent(c) == 0)
        .unwrap();
    let outer_choices: Vec<&str> = outer["parameters"][0]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|v| v.as_str())
        .collect();
    assert_eq!(outer_choices, ["日本語", "ENGLISH", "中文", "Español"]);

    for inner in list
        .iter()
        .filter(|c| event_code(c) == 102 && event_indent(c) == 1)
    {
        let choices: Vec<&str> = inner["parameters"][0]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|v| v.as_str())
            .collect();
        assert_eq!(choices, ["Yes", "No"], "nested choices must stay Yes/No");
    }

    let indent0_402: Vec<&str> = list
        .iter()
        .filter(|c| event_code(c) == 402 && event_indent(c) == 0)
        .filter_map(|c| c["parameters"][1].as_str())
        .collect();
    assert_eq!(
        indent0_402,
        ["日本語", "ENGLISH", "中文", "Español"],
        "new When must be a sibling of the language branches, not nested"
    );

    let es_402 = list
        .iter()
        .position(|c| {
            event_code(c) == 402
                && event_indent(c) == 0
                && c["parameters"][1].as_str() == Some("Español")
        })
        .unwrap();
    let end404 = list
        .iter()
        .position(|c| event_code(c) == 404 && event_indent(c) == 0)
        .unwrap();
    assert!(
        es_402 < end404,
        "cloned language branch must sit before the matching indent-0 404"
    );

    let es_scripts: Vec<String> = list[es_402..end404]
        .iter()
        .filter(|c| event_code(c) == 355 || event_code(c) == 655)
        .filter_map(|c| c["parameters"][0].as_str().map(|s| s.to_string()))
        .collect();
    assert!(
        es_scripts
            .iter()
            .any(|s| s.contains("I18N.language = \"es\"")),
        "{es_scripts:?}"
    );
    assert!(
        es_scripts
            .iter()
            .any(|s| s.contains("ConfigManager.lang = 3")),
        "{es_scripts:?}"
    );
}

#[test]
fn test_patch_event_list_no_usable_branch_no_changes() {
    let mut list = vec![
        cmd(
            102,
            0,
            serde_json::json!([["日本語", "ENGLISH", "中文"], -1, 0, 1, 0]),
        ),
        cmd(402, 0, serde_json::json!([0, "日本語"])),
        cmd(355, 1, serde_json::json!(["$gameMessage.add(\"jp\");"])),
        cmd(0, 1, serde_json::json!([])),
        cmd(402, 0, serde_json::json!([1, "ENGLISH"])),
        cmd(355, 1, serde_json::json!(["I18N.language = langs[1];"])),
        cmd(
            355,
            1,
            serde_json::json!(["ConfigManager.language = \"en\";"]),
        ),
        cmd(0, 1, serde_json::json!([])),
        cmd(402, 0, serde_json::json!([2, "中文"])),
        cmd(0, 1, serde_json::json!([])),
        cmd(404, 0, serde_json::json!([])),
    ];
    let before = list.clone();
    assert!(!patch_event_list(&mut list, "es", "Español"));
    assert_eq!(list, before);
}

#[test]
fn test_quoted_labels_escaped_in_plugins_js() {
    let raw = r#"{"parameters":{"Languages":"jp, en, zh","Language Labels":"en:English, jp:日本語, zh:中文"}}"#;
    let out = patch_iavra_labels_param(raw, "es", r#"Español "ES""#).unwrap();
    assert!(
        out.contains(r#"es:Español \"ES\""#),
        "label quotes must be escaped inside the JSON string: {out}"
    );
    let labels_start = out.find("\"Language Labels\":\"").unwrap() + "\"Language Labels\":\"".len();
    let labels_end = out[labels_start..].find('"').unwrap() + labels_start;
    // Naive quote scan would stop at the escaped quote; escaped form keeps one JSON string.
    assert!(
        out[labels_start..labels_end].contains("es:Español \\"),
        "{out}"
    );
    assert!(
        serde_json::from_str::<serde_json::Value>(&out).is_ok(),
        "{out}"
    );

    let dir = tempfile::tempdir().unwrap();
    write_game_with_map(
        dir.path(),
        "Map001.json",
        &map_json_from_list(language_choice_list()),
        Some(
            r#"var $plugins = [{"name":"Iavra_MZ_Localization_byNeomaStudio","status":true,"parameters":{"Languages":"jp, en, zh","Language Labels":"en:English, jp:日本語, zh:中文"}}];
"#,
        ),
    );
    let report = register_language(dir.path(), "es", r#"Español "ES""#).unwrap();
    assert!(report.iavra_languages || report.plugins_js);
    let plugins = fs::read_to_string(dir.path().join("js").join("plugins.js")).unwrap();
    assert!(
        plugins.contains(r#"es:Español \"ES\""#),
        "plugins.js must keep a valid quoted Language Labels string: {plugins}"
    );
    assert!(
        !plugins.contains(r#"es:Español "ES""#),
        "unescaped label quote would terminate the JSON string: {plugins}"
    );
}

#[test]
fn test_backup_original_bytes_before_write_and_multiple_languages() {
    let dir = tempfile::tempdir().unwrap();
    let map_json = map_json_from_list(language_choice_list());
    write_game_with_map(dir.path(), "Map001.json", &map_json, None);
    let map_path = dir.path().join("data").join("Map001.json");
    let original = fs::read(&map_path).unwrap();

    let report_es = register_language(dir.path(), "es", "Español").unwrap();
    assert!(!report_es.maps_patched.is_empty());
    let bak_path = dir.path().join("data").join("Map001.json.bak-locust");
    let bak_after_es = fs::read(&bak_path).unwrap();
    assert_eq!(
        bak_after_es, original,
        "backup must be exact original bytes, not the already-patched map"
    );
    let after_es = fs::read(&map_path).unwrap();
    assert_ne!(after_es, original);
    assert!(String::from_utf8_lossy(&after_es).contains("Español"));

    let report_fr = register_language(dir.path(), "fr", "Français").unwrap();
    assert!(!report_fr.maps_patched.is_empty());
    let bak_after_fr = fs::read(&bak_path).unwrap();
    assert_eq!(
        bak_after_fr, original,
        "existing initial backup must be preserved across adding multiple langs"
    );
    let after_fr = fs::read(&map_path).unwrap();
    assert_ne!(after_fr, after_es);
    assert!(String::from_utf8_lossy(&after_fr).contains("Français"));

    let dir_jsono = tempfile::tempdir().unwrap();
    write_game_with_map(dir_jsono.path(), "Map012.jsono", &map_json, None);
    let jsono_path = dir_jsono.path().join("data").join("Map012.jsono");
    let original_jsono = fs::read(&jsono_path).unwrap();
    register_language(dir_jsono.path(), "es", "Español").unwrap();
    let jsono_bak = fs::read(
        dir_jsono
            .path()
            .join("data")
            .join("Map012.jsono.bak-locust"),
    )
    .unwrap();
    assert_eq!(jsono_bak, original_jsono);
}

#[test]
fn test_json_jsono_unchanged_on_noop() {
    let noop_list = vec![
        cmd(
            102,
            0,
            serde_json::json!([["日本語", "ENGLISH", "中文"], -1, 0, 1, 0]),
        ),
        cmd(402, 0, serde_json::json!([0, "日本語"])),
        cmd(355, 1, serde_json::json!(["I18N.language = langs[0];"])),
        cmd(0, 1, serde_json::json!([])),
        cmd(402, 0, serde_json::json!([1, "ENGLISH"])),
        cmd(
            355,
            1,
            serde_json::json!(["ConfigManager.language = \"en\";"]),
        ),
        cmd(0, 1, serde_json::json!([])),
        cmd(402, 0, serde_json::json!([2, "中文"])),
        cmd(0, 1, serde_json::json!([])),
        cmd(404, 0, serde_json::json!([])),
    ];
    let map_json = map_json_from_list(noop_list);

    let dir = tempfile::tempdir().unwrap();
    write_game_with_map(dir.path(), "Map001.json", &map_json, None);
    let json_path = dir.path().join("data").join("Map001.json");
    let before_json = fs::read(&json_path).unwrap();
    let report = register_language(dir.path(), "es", "Español").unwrap();
    assert!(report.maps_patched.is_empty());
    assert_eq!(fs::read(&json_path).unwrap(), before_json);
    assert!(!dir
        .path()
        .join("data")
        .join("Map001.json.bak-locust")
        .exists());

    let dir_jsono = tempfile::tempdir().unwrap();
    write_game_with_map(dir_jsono.path(), "Map012.jsono", &map_json, None);
    let jsono_path = dir_jsono.path().join("data").join("Map012.jsono");
    let before_jsono = fs::read(&jsono_path).unwrap();
    let report = register_language(dir_jsono.path(), "es", "Español").unwrap();
    assert!(report.maps_patched.is_empty());
    assert_eq!(fs::read(&jsono_path).unwrap(), before_jsono);
    assert!(!dir_jsono
        .path()
        .join("data")
        .join("Map012.jsono.bak-locust")
        .exists());
}

#[test]
fn test_usable_language_list_still_appends_choice() {
    let mut list = language_choice_list();
    assert!(patch_event_list(&mut list, "es", "Español"));
    let choices: Vec<&str> = list[0]["parameters"][0]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|v| v.as_str())
        .collect();
    assert_eq!(choices, ["日本語", "ENGLISH", "中文", "Español"]);
    let after_404 = list
        .iter()
        .rev()
        .find(|c| event_code(c) == 355)
        .and_then(|c| c["parameters"][0].as_str())
        .unwrap();
    assert_eq!(
        after_404,
        "ConfigManager.language = IAVRA.MasterLocalization.I18N.language;"
    );
}
