use std::collections::HashMap;
use std::io::Cursor;
use std::path::Path;
use std::sync::Arc;

use locust_core::backup::BackupManager;
use locust_core::database::Database;
use locust_core::extraction::{
    inject_direct, FormatPlugin, FormatRegistry, FormatStability, MultiLangInjector,
};
use locust_core::models::OutputMode;
use locust_core::patch::GameLock;
use locust_formats::{dialogic, godot_dialogic::GodotDialogicPlugin, godot_pck};

fn pack(members: &[(&str, &[u8])]) -> Vec<u8> {
    let mut header = vec![0; 100];
    header[..4].copy_from_slice(b"GDPC");
    header[4..8].copy_from_slice(&2u32.to_le_bytes());
    header[8..12].copy_from_slice(&4u32.to_le_bytes());
    header[12..16].copy_from_slice(&3u32.to_le_bytes());
    let mut seed = Cursor::new(header);
    let index = godot_pck::read_index(&mut seed).unwrap();
    let mut output = Vec::new();
    godot_pck::rebuild(&mut seed, &index, &[], members, &mut output).unwrap();
    output
}

fn fixture(path: &Path) -> Vec<u8> {
    let bytes = pack(&[
        (
            "res://story/main.dtl",
            b"hero: Hello [b]{player}[/b]! #id:greeting\n- Yes\n",
        ),
        ("res://characters/hero.dch", br#"{"display_name":"Hero"}"#),
        ("res://assets/raw.bin", &[0, 255, 17, 33]),
        ("res://other.dtl", b"Untouched text\n"),
    ]);
    std::fs::write(path, &bytes).unwrap();
    bytes
}

fn members(path: &Path) -> HashMap<String, Vec<u8>> {
    let mut file = std::fs::File::open(path).unwrap();
    let index = godot_pck::read_index(&mut file).unwrap();
    index
        .entries
        .iter()
        .map(|e| {
            (
                e.path.clone(),
                godot_pck::read_member(&mut file, e, 4 * 1024 * 1024).unwrap(),
            )
        })
        .collect()
}

#[test]
fn godot_dialogic_detection_and_source_matrix() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("game.pck");
    let good = fixture(&path);
    let plugin = GodotDialogicPlugin;
    assert_eq!(plugin.id(), "godot-dialogic");
    assert_eq!(plugin.stability(), FormatStability::Experimental);
    assert_eq!(plugin.supported_modes(), vec![OutputMode::Replace]);
    assert_eq!(plugin.supported_extensions(), &["pck"]);
    assert!(plugin.detect(&path));
    plugin.validate_source(&path).unwrap();
    assert!(!plugin.detect(temp.path()));
    assert!(plugin
        .validate_source(temp.path())
        .unwrap_err()
        .to_string()
        .contains("standalone"));
    assert!(plugin
        .validate_source(&temp.path().join("missing.pck"))
        .is_err());
    for (offset, value) in [(4, 1u32), (4, 3), (8, 3), (12, 4), (20, 1), (20, 4)] {
        let mut bad = good.clone();
        bad[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
        std::fs::write(&path, bad).unwrap();
        assert!(!plugin.detect(&path));
        assert!(plugin.validate_source(&path).is_err());
    }
    std::fs::write(&path, pack(&[("res://plain.txt", b"hi")])).unwrap();
    assert!(!plugin.detect(&path));
    assert!(plugin
        .validate_source(&path)
        .unwrap_err()
        .to_string()
        .contains("timeline"));
    std::fs::write(&path, b"invalid").unwrap();
    assert!(!plugin.detect(&path));
    let exe = temp.path().join("game.exe");
    std::fs::write(&exe, &good).unwrap();
    assert!(!plugin.detect(&exe));
    assert!(plugin.validate_source(&exe).is_err());
    assert!(plugin
        .inject_add(&path, "es", &[])
        .unwrap_err()
        .to_string()
        .contains("does not support Add"));
    std::fs::write(&path, [b"MZ".as_slice(), &good].concat()).unwrap();
    assert!(!plugin.detect(&path));
    assert!(plugin.validate_source(&path).is_err());
    assert!(locust_formats::default_registry()
        .get("godot-dialogic")
        .is_some());
}

#[test]
fn godot_dialogic_rows_are_literal_versioned_and_complete() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("game.pck");
    fixture(&path);
    let extraction = GodotDialogicPlugin.extract_with_diagnostics(&path).unwrap();
    assert!(extraction.complete, "{:?}", extraction.warnings);
    assert!(extraction.warnings.is_empty());
    assert_eq!(extraction.entries.len(), 3);
    let row = &extraction.entries[0];
    assert_eq!(row.source, "Hello [b]{player}[/b]!");
    assert!(!row.source.contains(['\u{e000}', '\u{e001}']));
    assert_eq!(row.file_path, Path::new("game.pck/story/main.dtl"));
    assert!(row.id.starts_with("godot-dialogic:v1:"));
    assert_eq!(
        row.id,
        r#"godot-dialogic:v1:["game.pck","res://story/main.dtl",0,"text",0,"greeting"]"#
    );
    for value in ["game.pck", "res://story/main.dtl", "greeting", "text"] {
        assert!(row.id.contains(value));
    }
    let metadata = &row.metadata["godot_dialogic"];
    assert_eq!(metadata["locator_version"], 1);
    assert_eq!(metadata["archive"], "game.pck");
    assert_eq!(metadata["member"], "res://story/main.dtl");
    assert_eq!(metadata["speaker"], "hero");
    assert_eq!(metadata["original_md5"].as_str().unwrap().len(), 32);
    assert!(row.context.as_ref().unwrap().contains("hero"));
    assert!(row.context.as_ref().unwrap().contains("main.dtl"));
}

#[test]
fn godot_dialogic_incomplete_authentication_codec_and_character_authority() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("game.pck");
    let mut bytes = fixture(&path);
    let index = godot_pck::read_index(&mut Cursor::new(&bytes)).unwrap();
    bytes[index.entries[0].offset as usize] ^= 1;
    std::fs::write(&path, bytes).unwrap();
    let result = GodotDialogicPlugin.extract_with_diagnostics(&path).unwrap();
    assert!(!result.complete);
    assert_eq!(result.entries.len(), 1);
    assert!(result
        .warnings
        .iter()
        .any(|w| w.contains("main.dtl") && w.contains("MD5")));
    std::fs::write(&path, pack(&[("res://only.dtl", b"\\if escaped\n")])).unwrap();
    let result = GodotDialogicPlugin.extract_with_diagnostics(&path).unwrap();
    assert!(!result.complete);
    assert!(result.entries.is_empty());
    assert!(result
        .warnings
        .iter()
        .any(|w| w.contains("escaped event prefix")));
    std::fs::write(
        &path,
        pack(&[
            ("res://only.dtl", b"hero: Hello\n"),
            ("res://a/hero.dch", b"{}"),
            ("res://b/hero.dch", b"{}"),
        ]),
    )
    .unwrap();
    let result = GodotDialogicPlugin.extract_with_diagnostics(&path).unwrap();
    assert!(!result.complete);
    assert!(result.entries.is_empty());
    assert!(result
        .warnings
        .iter()
        .any(|w| w.contains("duplicate") && w.contains("hero")));
    let mut bytes = fixture(&path);
    let index = godot_pck::read_index(&mut Cursor::new(&bytes)).unwrap();
    bytes[index.entries[1].offset as usize] ^= 1;
    std::fs::write(&path, bytes).unwrap();
    let result = GodotDialogicPlugin.extract_with_diagnostics(&path).unwrap();
    assert!(!result.complete);
    assert!(!result.entries.iter().any(|e| e.source.starts_with("Hello")));
    assert!(result
        .warnings
        .iter()
        .any(|w| w.contains("hero.dch") && w.contains("MD5")));
}

#[test]
fn godot_dialogic_literal_codec_bridge_protects_order_segments_and_escapes() {
    let characters = dialogic::CharacterSet::default();
    let original = "Hello [b]{player}[/b]\\: yes\n";
    let timeline = dialogic::parse_timeline(original, &characters).unwrap();
    let span = &timeline.spans[0];
    assert_eq!(span.literal_text(), "Hello [b]{player}[/b]\\: yes");
    let literal = "Hola [b]{player}[/b]\\: sí";
    let encoded = span.encode_literal_translation(literal).unwrap();
    let output = dialogic::rebuild(
        original,
        &timeline,
        &HashMap::from([(span.id.clone(), encoded)]),
    )
    .unwrap();
    assert_eq!(output, format!("{literal}\n"));
    for bad in [
        "Hola [i]{player}[/b]\\: sí",
        "Hola {player}[b][/b]\\: sí",
        "Hola [b]{other}[/b]\\: sí",
        "Hola [b]{player}[/b]: sí",
        "Hola [b]{player}[/b]\\: sí {extra}",
    ] {
        assert!(
            span.encode_literal_translation(bad)
                .and_then(|t| dialogic::rebuild(
                    original,
                    &timeline,
                    &HashMap::from([(span.id.clone(), t)])
                ))
                .is_err(),
            "{bad}"
        );
    }
    let original = "First {player}\\\n  Second [b]bold[/b]\n";
    let timeline = dialogic::parse_timeline(original, &characters).unwrap();
    let span = &timeline.spans[0];
    assert_eq!(span.literal_text(), "First {player}\nSecond [b]bold[/b]");
    let encoded = span
        .encode_literal_translation("Uno {player}\nDos [b]fuerte[/b]")
        .unwrap();
    assert_eq!(
        dialogic::rebuild(
            original,
            &timeline,
            &HashMap::from([(span.id.clone(), encoded)])
        )
        .unwrap(),
        "Uno {player}\\\n  Dos [b]fuerte[/b]\n"
    );
    assert!(span
        .encode_literal_translation("Uno [b]fuerte[/b]\nDos {player}")
        .is_err());
}

#[test]
fn godot_dialogic_provider_protection_is_partial_literal_bridge_is_required() {
    use locust_core::placeholder::{PlaceholderProcessor, PlaceholderProfile};
    let profile = PlaceholderProfile::for_format("godot-dialogic");
    assert_eq!(profile, PlaceholderProfile::Full);
    let source = "Hello [b]{player}[/b] [color=red]{Game.score}[/color]";
    let (protected, tokens) = PlaceholderProcessor::extract_with_profile(source, profile);
    assert!(tokens.iter().any(|p| p.original == "[b]"));
    assert!(tokens.iter().any(|p| p.original == "{player}"));
    for literal in ["[/b]", "[color=red]", "{Game.score}", "[/color]"] {
        assert!(
            protected.contains(literal),
            "unexpected generic protection for {literal}"
        );
    }
    assert_eq!(
        PlaceholderProcessor::restore(&protected, &tokens).unwrap(),
        source
    );
}

#[test]
fn godot_dialogic_unreadable_zero_rows_and_bounded_warnings() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("game.pck");
    let mut bytes = pack(&[("res://only.dtl", b"Hello\n")]);
    let index = godot_pck::read_index(&mut Cursor::new(&bytes)).unwrap();
    bytes[index.entries[0].offset as usize] ^= 1;
    std::fs::write(&path, bytes).unwrap();
    let result = GodotDialogicPlugin.extract_with_diagnostics(&path).unwrap();
    assert!(!result.complete);
    assert!(result.entries.is_empty());
    assert!(result.warnings[0].contains("only.dtl"));
    let big = vec![b'x'; dialogic::MAX_INPUT_BYTES + 1];
    std::fs::write(&path, pack(&[("res://only.dtl", &big)])).unwrap();
    let result = GodotDialogicPlugin.extract_with_diagnostics(&path).unwrap();
    assert!(!result.complete);
    assert!(result.entries.is_empty());
    assert!(result.warnings[0].contains("cap"));
    let names = (0..140)
        .map(|i| format!("res://{i}.dtl"))
        .collect::<Vec<_>>();
    let members = names
        .iter()
        .map(|name| (name.as_str(), b"\\if escaped\n".as_slice()))
        .collect::<Vec<_>>();
    std::fs::write(&path, pack(&members)).unwrap();
    let result = GodotDialogicPlugin.extract_with_diagnostics(&path).unwrap();
    assert!(!result.complete);
    assert!(result.entries.is_empty());
    assert_eq!(result.warnings.len(), 128);
    assert!(result.warnings.iter().all(|w| w.len() <= 2048));
    let long_name = format!("res://{}/last.dtl", "x".repeat(3000));
    std::fs::write(&path, pack(&[(&long_name, b"\\if escaped\n")])).unwrap();
    let result = GodotDialogicPlugin.extract_with_diagnostics(&path).unwrap();
    assert!(!result.complete);
    assert!(result.warnings[0].contains("last.dtl"));
    assert!(result.warnings[0].contains("escaped event prefix"));
    assert!(result.warnings[0].chars().count() <= 2048);
}

#[cfg(unix)]
#[test]
fn godot_dialogic_refuses_source_and_destination_links() {
    let temp = tempfile::tempdir().unwrap();
    let real = temp.path().join("real.pck");
    fixture(&real);
    let link = temp.path().join("linked.pck");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    assert!(!GodotDialogicPlugin.detect(&link));
    assert!(GodotDialogicPlugin.validate_source(&link).is_err());
    let mut rows = GodotDialogicPlugin.extract(&real).unwrap();
    rows[0].translation = Some("Hola [b]{player}[/b]!".into());
    std::fs::remove_file(&real).unwrap();
    std::os::unix::fs::symlink(&link, &real).unwrap();
    assert!(GodotDialogicPlugin.inject(temp.path(), &rows).is_err());
}

#[test]
fn godot_dialogic_no_translation_is_byte_identical_without_a_lock() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("game.pck");
    let before = fixture(&path);
    let entries = GodotDialogicPlugin.extract(&path).unwrap();
    let report = GodotDialogicPlugin.inject(&path, &entries).unwrap();
    assert_eq!(report.strings_written, 0);
    assert!(report.files_written.is_empty());
    assert_eq!(std::fs::read(path).unwrap(), before);
}

#[test]
fn godot_dialogic_rewrite_held_lock_preserves_untouched_and_refuses_second_pass() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("game.pck");
    fixture(&path);
    let before = members(&path);
    let mut rows = GodotDialogicPlugin.extract(&path).unwrap();
    rows[0].translation = Some("Un saludo mucho más largo [b]{player}[/b]!".into());
    let lock = GameLock::acquire(temp.path()).unwrap();
    let report = GodotDialogicPlugin
        .inject_under_lock(temp.path(), &rows, &lock)
        .unwrap();
    assert_eq!(report.strings_written, 1);
    assert_eq!(report.files_modified, 1);
    assert_eq!(report.files_written, vec![path.clone()]);
    let after = members(&path);
    for (name, bytes) in before {
        if name != "res://story/main.dtl" {
            assert_eq!(after[&name], bytes);
        }
    }
    assert!(String::from_utf8_lossy(&after["res://story/main.dtl"]).contains("mucho más largo"));
    let bytes = std::fs::read(&path).unwrap();
    let second = GodotDialogicPlugin
        .inject_under_lock(&path, &rows, &lock)
        .unwrap();
    assert_eq!(second.strings_written, 0);
    assert_eq!(second.strings_skipped, 1);
    assert!(second.warnings.iter().any(|w| w.contains("MD5")));
    assert_eq!(std::fs::read(path).unwrap(), bytes);
}

#[test]
fn godot_dialogic_rejects_structural_tampering_before_lock() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("game.pck");
    let before = fixture(&path);
    let mut rows = GodotDialogicPlugin.extract(&path).unwrap();
    rows[0].translation = Some("Hola [b]{player}[/b]!".into());
    let row = rows[0].clone();
    assert!(GodotDialogicPlugin
        .inject(&path, &[row.clone(), row.clone()])
        .is_err());
    for field in [
        "archive",
        "member",
        "locator_version",
        "original_md5",
        "speaker",
        "kind",
    ] {
        let mut bad = row.clone();
        bad.metadata.get_mut("godot_dialogic").unwrap()[field] = serde_json::json!("wrong");
        assert!(
            GodotDialogicPlugin.inject(&path, &[bad]).is_err(),
            "{field}"
        );
    }
    let mut bad = row.clone();
    bad.file_path = "other.pck/story/main.dtl".into();
    assert!(GodotDialogicPlugin.inject(&path, &[bad]).is_err());
    let mut bad = row.clone();
    bad.id.push('x');
    assert!(GodotDialogicPlugin.inject(&path, &[bad]).is_err());
    let mut bad = row;
    bad.file_path = "../game.pck/story/main.dtl".into();
    assert!(GodotDialogicPlugin.inject(temp.path(), &[bad]).is_err());
    assert_eq!(std::fs::read(path).unwrap(), before);
}

#[test]
fn godot_dialogic_stale_unknown_and_altered_tokens_skip_without_writes() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("game.pck");
    let before = fixture(&path);
    let mut rows = GodotDialogicPlugin.extract(&path).unwrap();
    let lock = GameLock::acquire(temp.path()).unwrap();
    rows[0].translation = Some("Hola [i]{player}[/b]!".into());
    let report = GodotDialogicPlugin
        .inject_under_lock(&path, &rows[..1], &lock)
        .unwrap();
    assert_eq!(report.strings_skipped, 1);
    assert!(report.warnings.iter().any(|w| w.contains("protected")));
    rows[0].translation = Some("Hola [b]{player}[/b]!".into());
    rows[0].metadata.get_mut("godot_dialogic").unwrap()["original_md5"] =
        serde_json::json!("00000000000000000000000000000000");
    let report = GodotDialogicPlugin
        .inject_under_lock(&path, &rows[..1], &lock)
        .unwrap();
    assert_eq!(report.strings_skipped, 1);
    assert!(report.warnings.iter().any(|w| w.contains("MD5")));
    assert_eq!(std::fs::read(path).unwrap(), before);
}

#[tokio::test]
async fn godot_dialogic_real_plugin_source_scope_refuses_before_core_writes() {
    let temp = tempfile::tempdir().unwrap();
    let category = temp.path().join("category");
    std::fs::create_dir(&category).unwrap();
    fixture(&category.join("game.pck"));
    let mut registry = FormatRegistry::new();
    registry.register(Box::new(GodotDialogicPlugin));
    let registry = Arc::new(registry);
    let db = Arc::new(Database::open_in_memory().unwrap());
    let backups = temp.path().join("backups");
    let manager = Arc::new(BackupManager::new(backups.clone()));
    let output = temp.path().join("output");
    let error =
        locust_core::project::open_project(&db, &registry, &category, Some("godot-dialogic"))
            .unwrap_err();
    assert!(error.to_string().contains("standalone"), "{error}");
    let saved = temp.path().join("saved.db");
    drop(Database::open(&saved).unwrap());
    let before = std::fs::read(&saved).unwrap();
    let error =
        locust_core::project::open_project_db(&db, &registry, &saved, &category, "godot-dialogic")
            .unwrap_err();
    assert!(error.to_string().contains("standalone"), "{error}");
    assert_eq!(std::fs::read(saved).unwrap(), before);
    let error =
        inject_direct(&registry, &db, &manager, &category, "godot-dialogic", &[]).unwrap_err();
    assert!(error.to_string().contains("standalone"), "{error}");
    let injector = MultiLangInjector::new(registry, db, manager);
    for mode in [OutputMode::Replace, OutputMode::Add] {
        let (tx, _rx) = tokio::sync::mpsc::channel(20);
        let error = injector
            .inject(
                &category,
                "godot-dialogic",
                mode,
                vec!["es".into()],
                Some(output.clone()),
                tx,
            )
            .await
            .unwrap_err();
        assert!(error.to_string().contains("standalone"), "{error}");
    }
    assert!(!backups.exists());
    assert!(!output.exists());
    assert!(!category.join(".locust-injections").exists());
    assert_eq!(std::fs::read_dir(category).unwrap().count(), 1);
}

#[tokio::test]
async fn godot_dialogic_selected_file_replace_and_first_direct_keep_siblings() {
    let temp = tempfile::tempdir().unwrap();
    let game = temp.path().join("category");
    std::fs::create_dir(&game).unwrap();
    let path = game.join("game.pck");
    let before = fixture(&path);
    std::fs::write(game.join("foreign.exe"), "foreign").unwrap();
    std::fs::create_dir(game.join("other-game")).unwrap();
    std::fs::write(game.join("other-game/private"), "private").unwrap();
    let mut registry = FormatRegistry::new();
    registry.register(Box::new(GodotDialogicPlugin));
    let registry = Arc::new(registry);
    let db = Arc::new(Database::open_in_memory().unwrap());
    let mut rows = GodotDialogicPlugin.extract(&path).unwrap();
    rows[0].translation = Some("Hola [b]{player}[/b]!".into());
    db.save_entries(&rows).unwrap();
    let manager = Arc::new(BackupManager::new(temp.path().join("backups")));
    let injector = MultiLangInjector::new(registry.clone(), db.clone(), manager.clone());
    let output = temp.path().join("output");
    let (tx, _rx) = tokio::sync::mpsc::channel(20);
    let report = injector
        .inject(
            &path,
            "godot-dialogic",
            OutputMode::Replace,
            vec!["es".into()],
            Some(output.clone()),
            tx,
        )
        .await
        .unwrap();
    assert!(
        report.languages_failed.is_empty(),
        "{:?}",
        report.languages_failed
    );
    let dest = output.join("game.pck-es");
    assert_eq!(
        std::fs::read_dir(&dest)
            .unwrap()
            .filter_map(|e| {
                let e = e.unwrap();
                e.file_type().unwrap().is_file().then_some(e.file_name())
            })
            .collect::<Vec<_>>(),
        vec!["game.pck"]
    );
    assert_eq!(std::fs::read(&path).unwrap(), before);
    let report = inject_direct(&registry, &db, &manager, &path, "godot-dialogic", &[]).unwrap();
    assert_eq!(report.files_written, vec![path.clone()]);
    assert_eq!(report.strings_written, 1);
    let after = std::fs::read(&path).unwrap();
    let second = inject_direct(&registry, &db, &manager, &path, "godot-dialogic", &[]).unwrap();
    assert_eq!(second.strings_written, 0);
    assert_eq!(std::fs::read(&path).unwrap(), after);
    assert_eq!(std::fs::read(game.join("foreign.exe")).unwrap(), b"foreign");
    assert_eq!(
        std::fs::read(game.join("other-game/private")).unwrap(),
        b"private"
    );
}
