use std::fs;
use std::path::Path;

use locust_core::extraction::FormatPlugin;
use locust_core::models::StringEntry;
use locust_formats::renpy::RenPyPlugin;

const RPA_KEY: i64 = 0x42424242;

fn emit_put(pickle: &mut Vec<u8>, index: u8) {
    pickle.extend_from_slice(&[b'q', index]);
}

fn emit_long1(pickle: &mut Vec<u8>, value: i64) {
    assert!(value >= 0);
    pickle.push(0x8a);
    let bit_length = 64 - (value as u64).leading_zeros();
    let size = (bit_length / 8 + 1) as usize;
    pickle.push(size as u8);
    pickle.extend_from_slice(&value.to_le_bytes()[..size]);
}

/// One-member RPA-3.0 fixture using the protocol-2 member shape emitted by
/// shipped Ren'Py archives: BINUNICODE, LONG1, prefix, TUPLE3, and SETITEMS.
fn build_rpa(name: &str, content: &[u8]) -> Vec<u8> {
    let header_len = format!("RPA-3.0 {:016x} {:08x}\n", 0u64, 0u32).len();
    let offset = header_len as i64;
    let index_offset = header_len + content.len();

    let mut pickle = vec![0x80, 0x02, b'}']; // PROTO 2, EMPTY_DICT
    emit_put(&mut pickle, 0);
    pickle.push(b'('); // MARK
    pickle.push(b'X'); // BINUNICODE
    pickle.extend_from_slice(&(name.len() as u32).to_le_bytes());
    pickle.extend_from_slice(name.as_bytes());
    emit_put(&mut pickle, 1);
    pickle.push(b']'); // EMPTY_LIST
    emit_put(&mut pickle, 2);
    emit_long1(&mut pickle, offset ^ RPA_KEY);
    pickle.push(b'J'); // BININT
    pickle.extend_from_slice(&((content.len() as i64 ^ RPA_KEY) as i32).to_le_bytes());
    pickle.extend_from_slice(&[b'U', 0]); // empty SHORT_BINSTRING prefix
    emit_put(&mut pickle, 3);
    pickle.extend_from_slice(&[0x87, b'a', b'u', b'.']); // TUPLE3, APPEND, SETITEMS, STOP

    let header = format!("RPA-3.0 {:016x} {:08x}\n", index_offset, RPA_KEY as u32);
    let mut archive = header.into_bytes();
    archive.extend_from_slice(content);
    archive.extend_from_slice(&miniz_oxide::deflate::compress_to_vec_zlib(&pickle, 6));
    archive
}

fn game_fixture(kind: &str) -> tempfile::TempDir {
    let root_guard = tempfile::Builder::new()
        .prefix(&format!("locust_renpy_ui_{kind}_"))
        .tempdir()
        .unwrap();
    let root = root_guard.path().to_path_buf();
    fs::create_dir_all(root.join("game")).unwrap();
    root_guard
}

fn select_ui_translations(entries: &mut [StringEntry]) {
    for entry in entries {
        entry.translation = match entry.source.as_str() {
            "Back" => Some("Retour \"principal\" \\ menu\nSuite".to_string()),
            "Options" => Some("选项".to_string()),
            _ => None,
        };
    }
}

fn inject_selected(root: &Path) -> (locust_core::extraction::InjectionReport, String) {
    let plugin = RenPyPlugin::new();
    let mut entries = plugin.extract(root).expect("extract UI fixture");
    assert_eq!(
        entries
            .iter()
            .map(|entry| entry.source.as_str())
            .collect::<Vec<_>>(),
        vec!["Back", "History", "Options", "Save"]
    );
    select_ui_translations(&mut entries);
    let report = plugin.inject(root, &entries).expect("inject UI fixture");
    let output = fs::read_to_string(root.join("game/screens.rpy")).expect("written loose script");
    (report, output)
}

#[test]
fn rpa_ui_injection_matches_loose_and_replaces_only_the_selected_literal() {
    let script = concat!(
        "screen navigation():\n",
        "    textbutton \"Back\" action Return(\"Back\")\n",
        "    text \"History\"\n",
        "    tooltip (\"Options\")\n",
        "    textbutton \"Save\" action FileSave(1)\n",
    );

    let rpa_root_guard = game_fixture("rpa");
    let rpa_root = rpa_root_guard.path().to_path_buf();
    fs::write(
        rpa_root.join("game/scripts.rpa"),
        build_rpa("screens.rpy", script.as_bytes()),
    )
    .unwrap();

    let loose_root_guard = game_fixture("loose");
    let loose_root = loose_root_guard.path().to_path_buf();
    fs::write(loose_root.join("game/screens.rpy"), script).unwrap();

    let (rpa_report, rpa_output) = inject_selected(&rpa_root);
    let (loose_report, loose_output) = inject_selected(&loose_root);

    assert_eq!(rpa_report.strings_written, 2);
    assert_eq!(loose_report.strings_written, 2);
    assert_eq!(rpa_output, loose_output, "RPA and loose writes must agree");
    assert!(rpa_output
        .contains(r#"textbutton "Retour \"principal\" \\ menu\nSuite" action Return("Back")"#));
    assert!(
        rpa_output.contains(r#"action Return("Back")"#),
        "the duplicate action literal must remain untouched"
    );
    assert!(rpa_output.contains(r#"text "History""#));
    assert!(rpa_output.contains(r#"textbutton "Save" action FileSave(1)"#));
    assert!(rpa_output.contains(r#"tooltip ("选项")"#));
}

// The expected script is assembled from literal bytes, not from the injector's
// escaping or newline helpers. Only the selected literal may differ.
fn assert_replace_preserves_script_bytes(endings: &[&str], archived: bool) {
    let selected = "\ttextbutton \"Back\" action Return(\"Back\")  # keep \"Back\"  ";
    let translated = r#"	textbutton "Retour \"principal\" \\ menu\nSuite\r\t选项" action Return("Back")  # keep "Back"  "#;
    let translation = "Retour \"principal\" \\ menu\nSuite\r\t选项";

    // Start with the real audit's shape: a terminating newline plus a blank
    // line. Also cover an unterminated final line and whitespace-only tails.
    for tail in [
        "\n\n",
        "",
        "\n",
        "\n\n\n",
        "\r\n",
        "\r\n\r\n",
        "\n \t\r\n\n",
    ] {
        for selected_last in [false, true] {
            let mut lines = vec![
                "screen navigation():",
                "    # untouched comment: 日本語 ",
                "    text \"History\"  # keep spacing  ",
                "",
                "    tooltip (\"Options\")",
                "    textbutton \"Save\" action FileSave(1)",
            ];
            lines.insert(if selected_last { lines.len() } else { 3 }, selected);
            let mut script = String::new();
            let mut expected = String::new();
            for (index, line) in lines.iter().enumerate() {
                script.push_str(line);
                expected.push_str(if *line == selected { translated } else { line });
                if index + 1 < lines.len() {
                    let ending = endings[index % endings.len()];
                    script.push_str(ending);
                    expected.push_str(ending);
                }
            }
            script.push_str(tail);
            expected.push_str(tail);

            let root = game_fixture("byte_preservation");
            let loose_path = root.path().join("game/screens.rpy");
            let archive_path = root.path().join("game/scripts.rpa");
            let archive = build_rpa("screens.rpy", script.as_bytes());
            if archived {
                fs::write(&archive_path, &archive).unwrap();
            } else {
                fs::write(&loose_path, &script).unwrap();
            }

            let plugin = RenPyPlugin::new();
            let mut entries = plugin.extract(root.path()).unwrap();
            assert_eq!(entries.len(), 4);
            for entry in &mut entries {
                entry.translation = match entry.source.as_str() {
                    "Back" => Some(translation.to_string()),
                    "Options" => Some(entry.source.clone()), // identity control
                    _ => None,                               // untranslated controls
                };
            }
            let report = plugin.inject(root.path(), &entries).unwrap();
            assert_eq!(report.strings_written, 1);
            assert_eq!(report.files_modified, 1);
            assert_eq!(report.strings_skipped, 3);
            assert_eq!(
                fs::read(&loose_path).unwrap(),
                expected.as_bytes(),
                "archived={archived}, endings={endings:?}, tail={tail:?}, selected_last={selected_last}"
            );
            if archived {
                assert_eq!(fs::read(&archive_path).unwrap(), archive);
            }
        }
    }
}

#[test]
fn loose_replace_preserves_lf_script_bytes() {
    assert_replace_preserves_script_bytes(&["\n"], false);
}

#[test]
fn loose_replace_preserves_crlf_script_bytes() {
    assert_replace_preserves_script_bytes(&["\r\n"], false);
}

#[test]
fn loose_replace_preserves_mixed_script_bytes() {
    assert_replace_preserves_script_bytes(&["\n", "\r\n", "\r\n", "\n"], false);
}

#[test]
fn rpa_replace_preserves_lf_script_bytes() {
    assert_replace_preserves_script_bytes(&["\n"], true);
}

#[test]
fn rpa_replace_preserves_crlf_script_bytes() {
    assert_replace_preserves_script_bytes(&["\r\n"], true);
}

#[test]
fn rpa_replace_preserves_mixed_script_bytes() {
    assert_replace_preserves_script_bytes(&["\n", "\r\n", "\r\n", "\n"], true);
}

#[test]
fn replace_without_an_authorized_change_preserves_script_bytes() {
    let script =
        "screen navigation():\r\n\ttextbutton \"Back\" action Return(\"Back\") # keep\n\r\n\n";
    for archived in [false, true] {
        for control in ["untranslated", "identity", "stale"] {
            let root = game_fixture("unchanged_bytes");
            let loose_path = root.path().join("game/screens.rpy");
            let archive_path = root.path().join("game/scripts.rpa");
            let archive = build_rpa("screens.rpy", script.as_bytes());
            if archived {
                fs::write(&archive_path, &archive).unwrap();
            } else {
                fs::write(&loose_path, script).unwrap();
            }
            let plugin = RenPyPlugin::new();
            let mut entries = plugin.extract(root.path()).unwrap();
            assert_eq!(entries.len(), 1);
            match control {
                "identity" => entries[0].translation = Some(entries[0].source.clone()),
                "stale" => {
                    entries[0].source = "Previous Back".to_string();
                    entries[0].translation = Some("Retour".to_string());
                }
                _ => {}
            }
            let report = plugin.inject(root.path(), &entries).unwrap();
            assert_eq!(report.strings_written, 0, "{control}, archived={archived}");
            assert_eq!(report.files_modified, 0);
            assert_eq!(report.strings_skipped, 1);
            if archived {
                assert!(!loose_path.exists());
                assert_eq!(fs::read(&archive_path).unwrap(), archive);
            } else {
                assert_eq!(fs::read(&loose_path).unwrap(), script.as_bytes());
            }
        }
    }
}

#[test]
fn rpa_ui_injection_rejects_a_stale_physical_source() {
    let script = "screen navigation():\n    textbutton \"Back\" action Return(\"Back\")\n";
    let root_guard = game_fixture("stale");
    let root = root_guard.path().to_path_buf();
    fs::write(
        root.join("game/scripts.rpa"),
        build_rpa("screens.rpy", script.as_bytes()),
    )
    .unwrap();

    let plugin = RenPyPlugin::new();
    let mut entries = plugin.extract(&root).expect("extract UI fixture");
    assert_eq!(entries.len(), 1);
    entries[0].source = "Previous Back".to_string();
    entries[0].translation = Some("Retour".to_string());

    let report = plugin.inject(&root, &entries).expect("reject stale row");
    assert_eq!(report.files_modified, 0);
    assert_eq!(report.strings_written, 0);
    assert_eq!(report.strings_skipped, 1);
    assert_eq!(report.skip_reasons.get("source_changed"), Some(&1));
    assert!(
        !root.join("game/screens.rpy").exists(),
        "a stale source must not produce an override file"
    );
}
