use std::fs;
use std::path::{Path, PathBuf};

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

fn game_fixture(kind: &str) -> PathBuf {
    let root =
        std::env::temp_dir().join(format!("locust_renpy_ui_{kind}_{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(root.join("game")).unwrap();
    root
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

    let rpa_root = game_fixture("rpa");
    fs::write(
        rpa_root.join("game/scripts.rpa"),
        build_rpa("screens.rpy", script.as_bytes()),
    )
    .unwrap();

    let loose_root = game_fixture("loose");
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

#[test]
fn rpa_ui_injection_rejects_a_stale_physical_source() {
    let script = "screen navigation():\n    textbutton \"Back\" action Return(\"Back\")\n";
    let root = game_fixture("stale");
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
