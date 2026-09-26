use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use locust_core::extraction::FormatPlugin;
use locust_core::models::StringEntry;
use serde_json::{json, Value};

use crate::{
    html_game::HtmlGamePlugin, renpy::RenPyPlugin, rpgmaker_mv::RpgMakerMvPlugin,
    unity::UnityPlugin, unreal::UnrealPlugin,
};

fn write(path: &Path, bytes: impl AsRef<[u8]>) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, bytes).unwrap();
}
fn tree(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    walkdir::WalkDir::new(root)
        .into_iter()
        .map(|e| e.unwrap())
        .filter(|e| e.file_type().is_file())
        .map(|e| {
            (
                e.path().strip_prefix(root).unwrap().to_path_buf(),
                fs::read(e.path()).unwrap(),
            )
        })
        .collect()
}
pub(crate) fn projection(entries: Vec<StringEntry>) -> Vec<Value> {
    let mut rows: Vec<_> = entries
        .into_iter()
        .map(|e| {
            json!({
                "id":e.id,"source":e.source,"file_path":e.file_path,"metadata":e.metadata,
                "context":e.context,"tags":e.tags,"char_limit":e.char_limit,
            })
        })
        .collect();
    rows.sort_by_key(|row| row.to_string());
    rows
}
pub(crate) fn assert_unchanged(plugin: &dyn FormatPlugin, root: &Path, baseline: Vec<Value>) {
    let bytes = tree(root);
    assert!(plugin.detect(root), "{} detection changed", plugin.id());
    let after = projection(plugin.extract(root).unwrap());
    assert_eq!(
        after,
        baseline,
        "{}: recovery metadata changed identities, text, or metadata",
        plugin.id()
    );
    assert_eq!(
        tree(root),
        bytes,
        "discovery/extraction must not modify game or recovery sentinels"
    );
}
fn metadata(root: &Path) {
    write(
        &root.join(".locust/store.json"),
        br#"{"sentinel":"patch metadata"}"#,
    );
    write(
        &root.join(".locust-injections/store.json"),
        br#"{"sentinel":"transaction metadata"}"#,
    );
    write(
        &root.join(".locust-injections/operations/operation/work/originals/sentinel.bin"),
        b"preserve original bytes",
    );
}

#[test]
fn recovery_discovery_html_preserves_exact_rows_and_similarly_named_user_dirs() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let plugin = HtmlGamePlugin::new();
    write(&root.join("index.html"), "<p>Visible game dialogue</p>");
    write(
        &root.join(".locust-user/index.html"),
        "<p>Visible user dialogue</p>",
    );
    write(
        &root.join(".locust-injections-old/index.html"),
        "<p>Visible similarly named directory</p>",
    );
    let before = projection(plugin.extract(root).unwrap());
    assert_eq!(before.len(), 3);
    metadata(root);
    write(
        &root.join(".locust/backup/files/index.html"),
        "<p>Hidden backup dialogue</p>",
    );
    write(
        &root.join(".locust-injections/results/index.html"),
        "<p>Hidden transaction dialogue</p>",
    );
    assert_unchanged(&plugin, root, before);
}

#[test]
fn recovery_discovery_html_backup_cannot_change_format_detection() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let plugin = HtmlGamePlugin::new();
    write(&root.join("index.html"), "<p>Visible game dialogue</p>");
    let before = projection(plugin.extract(root).unwrap());
    write(
        &root.join(".locust/backup/index.html"),
        "<tw-passagedata>Foreign SugarCube backup</tw-passagedata>",
    );
    assert_unchanged(&plugin, root, before);
}

#[test]
fn recovery_discovery_renpy_preserves_exact_rows_and_user_directories() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let plugin = RenPyPlugin::new();
    write(
        &root.join("game/script.rpy"),
        "label start:\n    e \"Visible main dialogue\"\n",
    );
    write(
        &root.join("game/.locust-user/extra.rpy"),
        "label extra:\n    e \"Visible user dialogue\"\n",
    );
    let before = projection(plugin.extract(root).unwrap());
    assert_eq!(before.len(), 2);
    metadata(root);
    metadata(&root.join("game"));
    write(
        &root.join("game/.locust/backup/files/script.rpy"),
        "label backup:\n    e \"Hidden backup dialogue\"\n",
    );
    write(
        &root.join("game/.locust-injections/operations/op/work/originals/script.rpy"),
        "label interrupted:\n    e \"Hidden transaction dialogue\"\n",
    );
    assert_unchanged(&plugin, root, before);
}

#[test]
fn recovery_discovery_rpgmaker_direct_data_remains_isolated() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let plugin = RpgMakerMvPlugin::new();
    write(
        &root.join("data/System.json"),
        br#"{"gameTitle":"Visible game title"}"#,
    );
    write(
        &root.join("data/Actors.json"),
        br#"[null,{"id":1,"name":"Visible hero"}]"#,
    );
    let before = projection(plugin.extract(root).unwrap());
    assert_eq!(before.len(), 2);
    metadata(root);
    metadata(&root.join("data"));
    for sub in [
        ".locust/backup/files/data",
        ".locust-injections/operations/op/work/originals/data",
        "data/.locust",
        "data/.locust-injections",
    ] {
        write(
            &root.join(sub).join("System.json"),
            br#"{"gameTitle":"Hidden recovery title"}"#,
        );
    }
    assert_unchanged(&plugin, root, before);
}
fn unity_asset(text: &str) -> Vec<u8> {
    let mut bytes = vec![0; 64];
    bytes.extend_from_slice(&(text.len() as u32).to_le_bytes());
    bytes.extend_from_slice(text.as_bytes());
    bytes.extend_from_slice(&[0; 32]);
    bytes
}
#[test]
fn recovery_discovery_unity_assets_preserves_slots_and_user_directories() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let plugin = UnityPlugin::new();
    let data = root.join("Game_Data");
    write(
        &data.join("resources.assets"),
        unity_asset("Visible main dialogue"),
    );
    write(
        &data.join(".locust-user/user.assets"),
        unity_asset("Visible user dialogue"),
    );
    let before = projection(plugin.extract(root).unwrap());
    assert_eq!(before.len(), 2);
    metadata(root);
    metadata(&data);
    write(
        &data.join(".locust/backup.assets"),
        unity_asset("Hidden backup dialogue"),
    );
    write(
        &data.join(".locust-injections/results/ghost.assets"),
        unity_asset("Hidden transaction dialogue"),
    );
    assert_unchanged(&plugin, root, before);
}
#[test]
fn recovery_discovery_unity_scripts_preserves_user_directories() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let plugin = UnityPlugin::new();
    let scripts = root.join("Game_Data/SCRIPTS~");
    write(&scripts.join("Chapter.txt"), "Nar Visible main dialogue.\n");
    write(
        &scripts.join(".locust-user/Extra.txt"),
        "Nar Visible user dialogue.\n",
    );
    let before = projection(plugin.extract(root).unwrap());
    assert_eq!(before.len(), 2);
    metadata(root);
    metadata(&scripts);
    write(
        &scripts.join(".locust/backup/Chapter.txt"),
        "Nar Hidden backup dialogue.\n",
    );
    write(
        &scripts.join(".locust-injections/operations/op/work/results/Chapter.txt"),
        "Nar Hidden transaction dialogue.\n",
    );
    assert_unchanged(&plugin, root, before);
}
fn locres(namespace: &str, text: &str) -> Vec<u8> {
    use crate::unreal_locres::{
        str_crc32_ue, LocresFile, LocresNamespace, LocresString, LocresVersion,
    };
    LocresFile {
        version: LocresVersion::Compact,
        namespaces: vec![LocresNamespace {
            name: namespace.into(),
            name_hash: 0,
            strings: vec![LocresString {
                key: "Line".into(),
                value: text.into(),
                source_string_hash: str_crc32_ue(text),
                key_hash: 0,
            }],
        }],
    }
    .serialize()
    .unwrap()
}
fn pak(text: &str) -> Vec<u8> {
    use crate::unreal_pak::{write_pak, PakWriteFile};
    write_pak(
        "../../../",
        3,
        &[PakWriteFile {
            name: "Game/Content/Localization/Game/en/Archive.locres".into(),
            data: locres("Archive", text),
        }],
        "fixture",
    )
    .unwrap()
}
#[test]
fn recovery_discovery_unreal_loose_and_pak_preserve_exact_resources() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let plugin = UnrealPlugin::new();
    write(
        &root.join("Live.locres"),
        locres("Live", "Visible loose dialogue"),
    );
    write(&root.join("Game.pak"), pak("Visible archive dialogue"));
    write(
        &root.join(".locust-user/User.locres"),
        locres("User", "Visible user dialogue"),
    );
    let before = projection(plugin.extract(root).unwrap());
    assert_eq!(before.len(), 3);
    metadata(root);
    for sub in [".locust/backup/files", ".locust-injections/results"] {
        write(
            &root.join(sub).join("Hidden.locres"),
            locres("Hidden", "Hidden loose recovery dialogue"),
        );
        write(
            &root.join(sub).join("Hidden.pak"),
            pak("Hidden archive recovery dialogue"),
        );
    }
    assert_unchanged(&plugin, root, before);
}
fn toc() -> Vec<u8> {
    let mut bytes = vec![0; 144];
    bytes[..16].copy_from_slice(crate::unreal_iostore::TOC_MAGIC);
    bytes[16] = 5;
    bytes[20..24].copy_from_slice(&144u32.to_le_bytes());
    bytes
}
#[test]
fn recovery_discovery_unreal_iostore_ignores_internal_containers_only() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    for sub in [".locust/backup", ".locust-injections/results"] {
        write(&root.join(sub).join("Hidden.utoc"), toc());
    }
    let before = tree(root);
    assert!(crate::unreal_iostore::find_toc(root).is_none());
    assert!(crate::unreal_iostore_native::find_tocs(root).is_empty());
    assert!(!UnrealPlugin::new().detect(root));
    let visible = root.join(".locust-injections-user/User.utoc");
    write(&visible, toc());
    assert_eq!(crate::unreal_iostore::find_toc(root), Some(visible.clone()));
    assert_eq!(
        crate::unreal_iostore_native::find_tocs(root),
        vec![visible.clone()]
    );
    assert!(UnrealPlugin::new().detect(root));
    for (path, bytes) in before {
        assert_eq!(fs::read(root.join(path)).unwrap(), bytes);
    }
}
#[test]
fn recovery_discovery_backups_alone_cannot_claim_a_game_format() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    for sub in [".locust", ".locust-injections"] {
        let backup = root.join(sub);
        metadata(&backup);
        write(&backup.join("index.html"), "<p>Hidden backup dialogue</p>");
        write(
            &backup.join("game/script.rpy"),
            "label start:\n    e \"Hidden backup dialogue\"\n",
        );
        write(
            &backup.join("data/System.json"),
            br#"{"gameTitle":"Hidden backup title"}"#,
        );
        write(
            &backup.join("Game_Data/resources.assets"),
            unity_asset("Hidden backup dialogue"),
        );
        write(
            &backup.join("Content/Hidden.locres"),
            locres("Hidden", "Hidden backup dialogue"),
        );
    }
    let before = tree(root);
    for plugin in [
        Box::new(HtmlGamePlugin::new()) as Box<dyn FormatPlugin>,
        Box::new(RenPyPlugin::new()),
        Box::new(RpgMakerMvPlugin::new()),
        Box::new(UnityPlugin::new()),
        Box::new(UnrealPlugin::new()),
    ] {
        assert!(
            !plugin.detect(root),
            "{} claimed only recovery metadata",
            plugin.id()
        );
    }
    assert_eq!(tree(root), before);
}

#[test]
fn recovery_discovery_kirikiri_preserves_user_loose_scripts() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let plugin = crate::kirikiri::KirikiriPlugin::new();
    write(&root.join("scenario.ks"), "Visible main dialogue.\n");
    write(
        &root.join(".locust-user/extra.ks"),
        "Visible user dialogue.\n",
    );
    let before = projection(plugin.extract(root).unwrap());
    assert_eq!(before.len(), 2);
    metadata(root);
    write(
        &root.join(".locust/backup/hidden.ks"),
        "Hidden backup dialogue.\n",
    );
    write(
        &root.join(".locust-injections/results/interrupted.ks"),
        "Hidden transaction dialogue.\n",
    );
    assert_unchanged(&plugin, root, before);
}

#[test]
fn recovery_discovery_tyrano_preserves_user_loose_scripts() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let plugin = crate::tyrano::TyranoPlugin::new();
    let scripts = root.join("data/scenario");
    fs::create_dir_all(root.join("tyrano")).unwrap();
    write(&scripts.join("scene.ks"), "Visible main dialogue.\n");
    write(
        &scripts.join(".locust-injections-user/extra.ks"),
        "Visible user dialogue.\n",
    );
    let before = projection(plugin.extract(root).unwrap());
    assert_eq!(before.len(), 2);
    metadata(root);
    metadata(&scripts);
    write(
        &scripts.join(".locust/backup/hidden.ks"),
        "Hidden backup dialogue.\n",
    );
    write(
        &scripts.join(".locust-injections/results/interrupted.ks"),
        "Hidden transaction dialogue.\n",
    );
    assert_unchanged(&plugin, root, before);
}
