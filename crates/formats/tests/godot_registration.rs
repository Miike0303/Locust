use std::io::Cursor;
use std::path::Path;

use locust_core::extraction::{resolve_game_root, FormatPlugin, FormatStability};
use locust_core::models::OutputMode;
use locust_formats::{default_registry, godot_dialogic::GodotDialogicPlugin, godot_pck};

fn pck(path: &Path) {
    let mut header = vec![0; 100];
    header[..4].copy_from_slice(b"GDPC");
    header[4..8].copy_from_slice(&2u32.to_le_bytes());
    header[8..12].copy_from_slice(&4u32.to_le_bytes());
    let mut source = Cursor::new(header);
    let index = godot_pck::read_index(&mut source).unwrap();
    let mut bytes = Vec::new();
    godot_pck::rebuild(
        &mut source,
        &index,
        &[],
        &[("res://main.dtl", b"Hello\n")],
        &mut bytes,
    )
    .unwrap();
    std::fs::write(path, bytes).unwrap();
}

#[test]
fn godot_registry_lists_experimental_replace_and_retains_selected_file() {
    let temp = tempfile::tempdir().unwrap();
    let file = temp.path().join("game.pck");
    pck(&file);
    let registry = default_registry();
    let info = registry
        .list()
        .into_iter()
        .find(|p| p.id == "godot-dialogic")
        .unwrap();
    assert_eq!(info.stability, FormatStability::Experimental);
    assert_eq!(info.supported_modes, [OutputMode::Replace]);
    assert_eq!(info.extensions, ["pck"]);
    assert_eq!(registry.detect(&file).unwrap().id(), "godot-dialogic");
    assert_eq!(resolve_game_root(&file, &registry), file);
    assert!(!GodotDialogicPlugin.detect(temp.path()));
    assert!(registry.detect(temp.path()).is_none());
    assert_eq!(resolve_game_root(temp.path(), &registry), temp.path());
}

#[test]
fn godot_registration_preserves_existing_detection_and_wins_over_parent_adoption() {
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let registry = default_registry();
    for (fixture, id) in [
        ("renpy", "renpy"),
        ("renpy/game/script.rpy", "renpy"),
        ("wolf_rpg", "wolf-rpg"),
    ] {
        assert_eq!(
            registry.detect(&fixtures.join(fixture)).unwrap().id(),
            id,
            "{fixture}"
        );
    }
    for (name, bytes, id) in [
        ("index.html", b"<p>Hello</p>".as_slice(), "html-game"),
        (
            "story.html",
            b"<tw-storydata name=\"Story\"><tw-passagedata>Hi</tw-passagedata></tw-storydata>",
            "sugarcube",
        ),
        ("System.json", b"{}", "rpgmaker-mv"),
        ("game.locres", b"", "unreal"),
        ("game.assets", b"", "unity"),
    ] {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join(name);
        std::fs::write(&file, bytes).unwrap();
        assert_eq!(registry.detect(&file).unwrap().id(), id, "{name}");
        // A valid PCK must retain file scope even beside a recognized engine.
        let archive = temp.path().join("game.pck");
        pck(&archive);
        assert_eq!(
            registry.detect(&archive).unwrap().id(),
            "godot-dialogic",
            "beside {name}"
        );
        assert_eq!(resolve_game_root(&archive, &registry), archive);
        assert!(!GodotDialogicPlugin.detect(temp.path()));
    }
}
