//! Identity contract matrix (small synthetic fixtures; no external games).
//!
//! | Plugin | Mode / fixture | Documented identity exception |
//! | --- | --- | --- |
//! | godot-dialogic | Replace / generated PCK | None |
//! | rpgmaker-mv | Replace / JSON | None |
//! | rpgmaker-vxa | Replace / generated Marshal | None |
//! | renpy | Replace / loose RPY | None |
//! | wolf-rpg | Replace / generated Shift-JIS blob | None |
//! | sugarcube | Replace / selected story HTML | None |
//! | unreal | Replace / generated loose LocRes | None |
//! | unity | Replace / generated SerializedFile | None |
//! | html-game | Replace / HTML | None |
//! | qsp | Replace / generated UTF-16LE QSPGAME | None |
//! | tyrano | Replace / loose scenario | None |
//! | kirikiri | Replace / UTF-16LE scenario | None |
//! | yuris | Replace / generated XOR-encrypted YSTB | None |
//! | nscripter | Replace / 0.txt | None |
//! | vntextpatch | Replace / JSON | None |
//!
//! PCK replacement retains an owned backup by design: single-edit checks verify
//! that any new backup is an exact copy of the original archive. Unrelated files
//! must remain byte-identical; unrelated PCK members are also checked. Other
//! edited containers are not subject to a general intra-file byte-span oracle.
//! The focused YU-RIS unit regression checks untouched expression/padding,
//! instructions and line data inside its edited synthetic YSTB.
//! SugarCube and NScripter select one container; their second container is an
//! untouched guard. This covers one representative path per plugin, not every
//! supported codec.
//! After each single edit, public extraction must return the same logical row
//! identities (file path and entry ID), the exact fixture translation on the
//! selected row, and the original text on every other observable row. NScripter
//! retains its backticks; KiriKiri/Tyrano retain [p]. Unity's first sorted row is
//! the dummy object's heuristic name, not the structural TextAsset: its five-byte
//! ASCII translation meets the public scanner's five-byte/three-letter minimum.
//! These fixture profiles need no text normalization.
//! Provenance metadata (such as checksums or container-layout context) is not
//! compared. Public extraction does not establish runtime behavior or
//! preservation of arbitrary bytes inside edited containers.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Cursor;
use std::path::{Path, PathBuf};

use locust_core::extraction::{FormatPlugin, InjectionReport};
use locust_core::models::{OutputMode, StringEntry};
use locust_formats::unity_serialized::CLASS_ID_TEXT_ASSET;
use locust_formats::{default_registry, godot_pck, unreal_locres};

#[path = "support/unity_fixture.rs"]
mod unity_fixture;

const PLUGINS: &[&str] = &[
    "godot-dialogic",
    "rpgmaker-mv",
    "rpgmaker-vxa",
    "renpy",
    "wolf-rpg",
    "sugarcube",
    "unreal",
    "unity",
    "html-game",
    "qsp",
    "tyrano",
    "kirikiri",
    "yuris",
    "nscripter",
    "vntextpatch",
];

type Snapshot = BTreeMap<PathBuf, Vec<u8>>;

fn snapshot(root: &Path) -> Snapshot {
    walkdir::WalkDir::new(root)
        .into_iter()
        .map(|entry| entry.unwrap())
        .filter(|entry| entry.file_type().is_file())
        .map(|entry| {
            (
                entry.path().strip_prefix(root).unwrap().to_path_buf(),
                fs::read(entry.path()).unwrap(),
            )
        })
        .collect()
}

fn write(root: &Path, relative: &str, bytes: impl AsRef<[u8]>) {
    let path = root.join(relative);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, bytes).unwrap();
}

fn pck(text: &str) -> Vec<u8> {
    // Same seed/rebuild builder as godot_dialogic_tests.rs.
    let mut header = vec![0; 100];
    header[..4].copy_from_slice(b"GDPC");
    header[4..8].copy_from_slice(&2u32.to_le_bytes());
    header[8..12].copy_from_slice(&4u32.to_le_bytes());
    header[12..16].copy_from_slice(&3u32.to_le_bytes());
    let mut seed = Cursor::new(header);
    let index = godot_pck::read_index(&mut seed).unwrap();
    let mut output = Vec::new();
    godot_pck::rebuild(
        &mut seed,
        &index,
        &[],
        &[
            ("res://story/main.dtl", text.as_bytes()),
            ("res://story/other.dtl", b"Another timeline.\n"),
            ("res://assets/raw.bin", &[0, 255, 17, 33]),
        ],
        &mut output,
    )
    .unwrap();
    output
}

fn locres(text: &str) -> Vec<u8> {
    use unreal_locres::{LocresFile, LocresNamespace, LocresString, LocresVersion};
    LocresFile {
        version: LocresVersion::Compact,
        namespaces: vec![LocresNamespace {
            name: "Story".into(),
            name_hash: 0,
            strings: vec![LocresString {
                key: "Greeting".into(),
                value: text.into(),
                source_string_hash: 123,
                key_hash: 0,
            }],
        }],
    }
    .serialize()
    .unwrap()
}

fn qsp(text: &str) -> Vec<u8> {
    // Inline serialize_game builder from qsp.rs: first two records are plain,
    // subsequent records use CODREMOV; every record ends in UTF-16LE CRLF.
    let records = [
        "QSPGAME",
        "locust-test 1.0",
        "No",
        "1",
        "start",
        text,
        "",
        "0",
    ];
    let mut bytes = Vec::new();
    for (index, record) in records.iter().enumerate() {
        for unit in record.encode_utf16() {
            let encoded = if index < 2 {
                unit
            } else if unit == 5 {
                0u16.wrapping_sub(5)
            } else {
                unit.wrapping_sub(5)
            };
            bytes.extend_from_slice(&encoded.to_le_bytes());
        }
        bytes.extend_from_slice(&[13, 0, 10, 0]);
    }
    bytes
}

fn ystb(text: &str) -> Vec<u8> {
    // build_ystb_with_inst_pad from yuris.rs, with two dialogue expressions.
    let mut values = Vec::new();
    let mut descriptors = Vec::new();
    for text in [text, "Welcome home."] {
        let body = format!("\"{text}\"");
        let mut expression = vec![0x4d]; // PUSH_STRING
        expression.extend_from_slice(&(body.len() as u16).to_le_bytes());
        expression.extend_from_slice(body.as_bytes());
        descriptors.extend_from_slice(&0i16.to_le_bytes());
        descriptors.extend_from_slice(&3i16.to_le_bytes()); // ATTR_EXPRESSION
        descriptors.extend_from_slice(&(expression.len() as u32).to_le_bytes());
        descriptors.extend_from_slice(&(values.len() as u32).to_le_bytes());
        values.extend(expression);
    }
    values.extend_from_slice(&[0; 16]);
    while !values.len().is_multiple_of(4) {
        values.push(0);
    }
    let mut instructions = vec![108, 1, 0, 0, 108, 1, 0, 0];
    instructions.extend_from_slice(&[0; 32]);
    let lines: Vec<u8> = (1u32..=10).flat_map(u32::to_le_bytes).collect();
    let mut bytes = b"YSTB".to_vec();
    for value in [
        0x22b,
        10,
        instructions.len() as u32,
        descriptors.len() as u32,
        values.len() as u32,
        lines.len() as u32,
        0,
    ] {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    let key = 0xb4626ad8u32.to_le_bytes();
    for section in [instructions, descriptors, values, lines] {
        bytes.extend(
            section
                .iter()
                .enumerate()
                .map(|(i, byte)| byte ^ key[i % 4]),
        );
    }
    bytes
}

// Returns the selected source and the physical file whose first row is edited.
fn fixture(id: &str, root: &Path) -> (PathBuf, PathBuf) {
    let (selection, primary, secondary, first, second) = match id {
        "godot-dialogic" => (
            "main.pck",
            "main.pck",
            "other.pck",
            pck("Hello, traveler!\r\n"),
            pck("A second story.\n"),
        ),
        "rpgmaker-mv" => (
            "",
            "data/Actors.json",
            "data/Items.json",
            b"[null, { \"id\":1, \"name\":\"Hello traveler\", \"note\":\"\" }]\r\n".to_vec(),
            b"[null,{\"id\":1,\"name\":\"Other item\",\"note\":\"\"}]\n".to_vec(),
        ),
        "rpgmaker-vxa" => (
            "",
            "Data/Actors.rvdata2",
            "Data/Items.rvdata2",
            locust_formats::rpgmaker_vxa::build_test_fixture(),
            locust_formats::rpgmaker_vxa::build_test_fixture(),
        ),
        "renpy" => (
            "",
            "game/script.rpy",
            "game/other.rpy",
            b"# preserve CRLF\r\nlabel start:\r\n    \"Hello, traveler!\"\r\n".to_vec(),
            b"label other:\n    \"A second story.\"\n".to_vec(),
        ),
        "wolf-rpg" => (
            "",
            "Data/BasicData.wolf",
            "Data/Other.wolf",
            locust_formats::wolf_rpg::build_test_fixture(),
            locust_formats::wolf_rpg::build_test_fixture(),
        ),
        "sugarcube" => (
            "main.html",
            "main.html",
            "other.html",
            story("Hello, traveler!"),
            story("A second story."),
        ),
        "unreal" => (
            "",
            "main.locres",
            "other.locres",
            locres("Hello, traveler!"),
            locres("A second story."),
        ),
        "unity" => (
            "",
            "Game_Data/resources.assets",
            "Game_Data/sharedassets0.assets",
            unity_fixture::write_v17_fixture("Story", "Hello, traveler!"),
            unity_fixture::write_v17_fixture("Other", "A second story."),
        ),
        "html-game" => (
            "",
            "main.html",
            "other.html",
            b"<!doctype html><p>Hello &amp; welcome, traveler!</p>\r\n".to_vec(),
            b"<p>A second story.</p>\n".to_vec(),
        ),
        "qsp" => (
            "",
            "main.qsp",
            "other.qsp",
            qsp("Hello, traveler!"),
            qsp("A second story."),
        ),
        "tyrano" => {
            fs::create_dir_all(root.join("tyrano")).unwrap();
            (
                "",
                "data/scenario/main.ks",
                "data/scenario/other.ks",
                b"\xef\xbb\xbf*start\r\nHello, traveler![p]\r\n".to_vec(),
                b"*other\nA second story.[p]\n".to_vec(),
            )
        }
        "kirikiri" => (
            "",
            "main.ks",
            "other.ks",
            utf16("*start\r\nHello, traveler![p]\r\n"),
            utf16("*other\nA second story.[p]\n"),
        ),
        "yuris" => (
            "",
            "ysbin/yst00001.ybn",
            "ysbin/yst00002.ybn",
            ystb("Hello, traveler!"),
            ystb("A second story."),
        ),
        "nscripter" => (
            "",
            "0.txt",
            "00.txt",
            b"*define\r\ngame\r\n*start\r\n`Hello, traveler!`\r\nend\r\n".to_vec(),
            b"*start\n`A second story.`\n".to_vec(),
        ),
        "vntextpatch" => (
            "",
            "main.json",
            "other.json",
            b"\xef\xbb\xbf[ { \"message\" : \"Hello, traveler!\", \"opaque\":17 } ]\r\n".to_vec(),
            b"[{\"message\":\"A second story.\"}]\n".to_vec(),
        ),
        _ => panic!("registered plugin {id} needs an identity fixture"),
    };
    write(root, primary, first);
    write(root, secondary, second);
    write(root, "assets/opaque.bin", [0, 255, 17, 0, 13, 10]);
    (PathBuf::from(selection), PathBuf::from(primary))
}

fn story(text: &str) -> Vec<u8> {
    format!("<!doctype html><meta name=\"application-name\" content=\"SugarCube\">\r\n<tw-storydata name=\"Test\"><tw-passagedata pid=\"1\" name=\"Start\" tags=\"\">{text}</tw-passagedata></tw-storydata>\r\n").into_bytes()
}

fn utf16(text: &str) -> Vec<u8> {
    [0xff, 0xfe]
        .into_iter()
        .chain(text.encode_utf16().flat_map(u16::to_le_bytes))
        .collect()
}

struct GameCopy {
    _temp: tempfile::TempDir,
    root: PathBuf,
    selection: PathBuf,
    primary: PathBuf,
    before: Snapshot,
}

impl GameCopy {
    fn new(id: &str) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let pristine = temp.path().join("pristine");
        let root = temp.path().join("copy");
        let (selection, primary) = fixture(id, &pristine);
        let before = snapshot(&pristine);
        for (path, bytes) in &before {
            write(&root, path.to_str().unwrap(), bytes);
        }
        Self {
            selection: root.join(selection),
            root,
            primary,
            before,
            _temp: temp,
        }
    }

    fn entries(&self, plugin: &dyn FormatPlugin) -> Vec<StringEntry> {
        let mut entries = plugin.extract(&self.selection).unwrap();
        assert!(
            !entries.is_empty(),
            "{} must extract at least one row",
            plugin.id()
        );
        entries.sort_by(|a, b| a.file_path.cmp(&b.file_path).then(a.id.cmp(&b.id)));
        entries
    }

    fn inject(&self, plugin: &dyn FormatPlugin, entries: &[StringEntry]) -> InjectionReport {
        if plugin.supported_modes().contains(&OutputMode::Replace) {
            plugin.inject(&self.selection, entries).unwrap()
        } else {
            assert!(plugin.supported_modes().contains(&OutputMode::Add));
            plugin.inject_add(&self.selection, "es", entries).unwrap()
        }
    }
}

#[test]
fn registry_has_exactly_the_matrix_plugins() {
    let registered: BTreeSet<_> = default_registry()
        .list()
        .into_iter()
        .map(|p| p.id)
        .collect();
    assert_eq!(
        registered,
        PLUGINS.iter().map(|id| (*id).to_owned()).collect()
    );
}

fn identity(id: &str) {
    let registry = default_registry();
    let plugin = registry.get(id).unwrap();
    let game = GameCopy::new(id);
    let mut entries = game.entries(plugin);
    for entry in &mut entries {
        entry.translation = Some(entry.source.clone());
    }
    let report = game.inject(plugin, &entries);
    let after = snapshot(&game.root);
    // Compare bytes even if the plugin claims zero writes; also detect removals.
    for (path, bytes) in &game.before {
        assert_eq!(
            after.get(path),
            Some(bytes),
            "{id}: identity changed {path:?}; {report:?}"
        );
    }
    if plugin.supported_modes().contains(&OutputMode::Replace) {
        assert_eq!(
            after.keys().collect::<Vec<_>>(),
            game.before.keys().collect::<Vec<_>>(),
            "{id}: identity created files; {report:?}"
        );
    }
    eprintln!(
        "{id} | {} | identity bytes: pass | writes: {}",
        entries.len(),
        report.files_modified
    );
}

fn single_edit(id: &str) {
    let registry = default_registry();
    let plugin = registry.get(id).unwrap();
    let game = GameCopy::new(id);
    let mut entries = game.entries(plugin);
    for entry in &mut entries {
        entry.translation = Some(entry.source.clone());
    }
    let primary = game.root.join(&game.primary);
    let row = entries
        .iter_mut()
        .find(|entry| {
            let physical = plugin.revision_original_path(&entry.file_path);
            physical == primary || physical == game.primary
        })
        .expect("primary fixture must contain an extracted entry");
    // Short ASCII fits Wolf's slot; retain script markers. Unity's heuristic
    // "Dummy" row needs five bytes and three letters to remain publicly visible.
    let translation = match id {
        "nscripter" => "`Hi`",
        "kirikiri" | "tyrano" => "Hi[p]",
        "unity" => "Howdy",
        _ => "Hi",
    };
    assert_ne!(
        row.source, translation,
        "{id}: fixture edit must change text"
    );
    row.translation = Some(translation.into());
    let changed_key = (row.file_path.clone(), row.id.clone());
    let report = game.inject(plugin, &entries);
    assert!(
        report.strings_written >= 1,
        "{id}: one edit must be applied; {report:?}"
    );
    let after = snapshot(&game.root);
    for (path, bytes) in &game.before {
        if path == &game.primary {
            let edited = after.get(path).expect("edited target must still exist");
            assert_ne!(edited, bytes, "{id}: edit did not change target");
        } else {
            assert_eq!(
                after.get(path),
                Some(bytes),
                "{id}: edit changed unrelated {path:?}; {report:?}"
            );
        }
    }
    for (path, bytes) in &after {
        if !game.before.contains_key(path) {
            assert_eq!(id, "godot-dialogic", "{id}: unexpected new file {path:?}");
            assert_eq!(
                Some(bytes),
                game.before.get(&game.primary),
                "backup must preserve the exact original archive"
            );
        }
    }
    if id == "godot-dialogic" {
        let before = pck_members(&game.before[&game.primary]);
        let after = pck_members(&after[&game.primary]);
        assert_eq!(
            before.keys().collect::<Vec<_>>(),
            after.keys().collect::<Vec<_>>()
        );
        for (name, bytes) in before {
            if name != "res://story/main.dtl" {
                assert_eq!(after[&name], bytes, "unrelated PCK member {name}");
            }
        }
    }
    let extracted = game.entries(plugin);
    let keys = |rows: &[StringEntry]| {
        rows.iter()
            .map(|entry| (entry.file_path.clone(), entry.id.clone()))
            .collect::<Vec<_>>()
    };
    assert_eq!(
        keys(&extracted),
        keys(&entries),
        "{id}: public re-extraction changed logical row identities; {report:?}"
    );
    for (before, after) in entries.iter().zip(&extracted) {
        let expected = if (&before.file_path, &before.id) == (&changed_key.0, &changed_key.1) {
            translation
        } else {
            &before.source
        };
        assert_eq!(
            after.source, expected,
            "{id}: public re-extraction has wrong text for {:?} / {}; selected: {changed_key:?}; {report:?}",
            before.file_path, before.id
        );
    }
    eprintln!(
        "{id} | {} | single-edit isolation and exact public re-extraction: pass",
        entries.len()
    );
}

fn pck_members(bytes: &[u8]) -> BTreeMap<String, Vec<u8>> {
    let mut file = Cursor::new(bytes);
    let index = godot_pck::read_index(&mut file).unwrap();
    index
        .entries
        .iter()
        .map(|entry| {
            (
                entry.path.clone(),
                godot_pck::read_member(&mut file, entry, 4096).unwrap(),
            )
        })
        .collect()
}

macro_rules! matrix {
    ($($name:ident => $id:literal),+ $(,)?) => { $(
        mod $name {
            #[test]
            fn identity_bytes() { super::identity($id); }
            #[test]
            fn single_edit_isolation() { super::single_edit($id); }
        }
    )+ };
}

matrix! {
    godot_dialogic => "godot-dialogic",
    rpgmaker_mv => "rpgmaker-mv",
    rpgmaker_vxa => "rpgmaker-vxa",
    renpy => "renpy",
    wolf_rpg => "wolf-rpg",
    sugarcube => "sugarcube",
    unreal => "unreal",
    unity => "unity",
    html_game => "html-game",
    qsp => "qsp",
    tyrano => "tyrano",
    kirikiri => "kirikiri",
    nscripter => "nscripter",
    vntextpatch => "vntextpatch",
}

mod yuris {
    #[test]
    fn identity_bytes() {
        super::identity("yuris");
    }

    #[test]
    fn single_edit_isolation() {
        super::single_edit("yuris");
    }
}
