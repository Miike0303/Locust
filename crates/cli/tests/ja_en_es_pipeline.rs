//! Scripted transport characterization, not a linguistic or game-runtime claim.
//! Every translation goes through the real CLI/provider/placeholder machinery.
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Duration;

use assert_cmd::Command;
use locust_core::database::{sha256_hex, Database, EntryFilter};
use locust_core::models::{StringEntry, StringStatus, INJECTION_SOURCE_METADATA_KEY};
use locust_core::patch::manifest::{
    BackupBaseline, BackupManifest, PatchManifest, Receipt, VerificationTier,
};
use serde_json::{json, Value};

#[path = "support/scripted_openai.rs"]
mod scripted_openai;
use scripted_openai::{Answer, RequestKey, ScriptedOpenAi, DUMMY_KEY, MODEL, PROVIDER};

type Files = BTreeMap<String, Vec<u8>>;
type Slot = (String, String);

#[derive(Clone, Copy, Debug)]
enum Engine {
    RenPy,
    RpgMakerMv,
    KiriKiri,
}

impl Engine {
    fn id(self) -> &'static str {
        match self {
            Self::RenPy => "renpy",
            Self::RpgMakerMv => "rpgmaker-mv",
            Self::KiriKiri => "kirikiri",
        }
    }
}

#[derive(Clone, Copy)]
struct Line {
    text: [&'static str; 3],
    masked: [&'static str; 3],
}

impl Line {
    fn plain(text: [&'static str; 3]) -> Self {
        Self { text, masked: text }
    }
}

struct Fixture {
    engine: Engine,
    files: Files,
    slots: BTreeMap<Slot, Line>,
    control: Line,
}

fn fixture(engine: Engine) -> Fixture {
    let title = Line::plain(["錬金術師", "Alchemist", "Alquimista"]);
    let duplicate = Line::plain(["こんにちは", "Hello", "Hola"]);
    let next = Line::plain(["進む", "Continue", "Continuar"]);
    let back = Line::plain(["戻る", "Return", "Volver"]);
    let mut files = Files::new();
    let mut slots = BTreeMap::new();
    let mut slot = |path: &str, id: &str, line: Line| {
        assert!(slots.insert((path.into(), id.into()), line).is_none());
    };
    let control = match engine {
        Engine::RenPy => {
            let control = Line {
                text: [
                    "こんにちは [player] {b}旅人{/b}！",
                    "Hello [player] {b}traveler{/b}!",
                    "Hola [player] {b}viajero{/b}!",
                ],
                masked: [
                    "こんにちは {PL_0} {PL_1}旅人{PL_2}！",
                    "Hello {PL_0} {PL_1}traveler{PL_2}!",
                    "Hola {PL_0} {PL_1}viajero{PL_2}!",
                ],
            };
            let script = format!(
                r#"define e = Character("{}")
label start:
    $ asset = "NON_TEXT_SENTINEL.png"
    e "{}"
    e "{}"
    e "{}"
    menu:
        "{}":
            jump next
        "{}":
            return
label next:
    return
"#,
                title.text[0],
                control.text[0],
                duplicate.text[0],
                duplicate.text[0],
                next.text[0],
                back.text[0]
            );
            files.insert("game/script.rpy".into(), script.into_bytes());
            for (number, line) in [
                (1, title),
                (4, control),
                (5, duplicate),
                (6, duplicate),
                (8, next),
                (10, back),
            ] {
                slot("game/script.rpy", &format!("script.rpy#{number}"), line);
            }
            control
        }
        Engine::RpgMakerMv => {
            let control = Line {
                text: [
                    r"\n<錬金術師>こんにちは \V[1]！",
                    r"\n<Alchemist>Hello \V[1]!",
                    r"\n<Alquimista>Hola \V[1]!",
                ],
                masked: [
                    "{PL_0}<錬金術師>こんにちは {PL_1}！",
                    "{PL_0}<Alchemist>Hello {PL_1}!",
                    "{PL_0}<Alquimista>Hola {PL_1}!",
                ],
            };
            files.insert(
                "data/System.json".into(),
                serde_json::to_vec_pretty(&json!({
                    "gameTitle": title.text[0], "terms": {"basic": [], "commands": []},
                    "currencyUnit": "G", "switches": [null, "NON_TEXT_SENTINEL"]
                }))
                .unwrap(),
            );
            let map = json!({
                "displayName": "", "note": "NON_TEXT_SENTINEL", "width": 1,
                "height": 1, "data": [17, 23, 42],
                "events": [null, {"id": 1, "name": "NON_TEXT_SENTINEL", "pages": [{
                    "conditions": {"switch1Id": 7, "switch1Valid": true},
                    "image": {"characterName": "NON_TEXT_SENTINEL.png", "direction": 2},
                    "list": [
                        {"code": 101, "indent": 0, "parameters": ["", 0, 0, 2]},
                        {"code": 401, "indent": 0, "parameters": [control.text[0]]},
                        {"code": 101, "indent": 0, "parameters": ["", 0, 0, 2]},
                        {"code": 401, "indent": 0, "parameters": [duplicate.text[0]]},
                        {"code": 101, "indent": 0, "parameters": ["", 0, 0, 2]},
                        {"code": 401, "indent": 0, "parameters": [duplicate.text[0]]},
                        {"code": 102, "indent": 0, "parameters": [[next.text[0], back.text[0]], 1, 0, 2, 0]},
                        {"code": 402, "indent": 0, "parameters": [0, next.text[0]]},
                        {"code": 121, "indent": 1, "parameters": [7, 7, 0]},
                        {"code": 402, "indent": 0, "parameters": [1, back.text[0]]},
                        {"code": 404, "indent": 0, "parameters": []},
                        {"code": 0, "indent": 0, "parameters": []}
                    ]
                }]}]
            });
            files.insert(
                "data/Map001.json".into(),
                serde_json::to_vec_pretty(&map).unwrap(),
            );
            slot("data/System.json", "System.json#gameTitle", title);
            let prefix = "Map001.json#0#event_1#page_0";
            for (locator, line) in [
                ("cmd_1#msg", control),
                ("cmd_3#msg", duplicate),
                ("cmd_5#msg", duplicate),
                ("cmd_6#choice_0", next),
                ("cmd_6#choice_1", back),
            ] {
                slot("data/Map001.json", &format!("{prefix}#{locator}"), line);
            }
            control
        }
        Engine::KiriKiri => {
            let control = Line {
                text: [
                    "こんにちは [emb exp=\"f.player\"][l]",
                    "Hello [emb exp=\"f.player\"][l]",
                    "Hola [emb exp=\"f.player\"][l]",
                ],
                // The bounded bracket scanner masks [l], not arbitrary KAG
                // expressions. Keep the argument-bearing tag literal and prove
                // its preservation in the applied slot instead.
                masked: [
                    "こんにちは [emb exp=\"f.player\"]{PL_0}",
                    "Hello [emb exp=\"f.player\"]{PL_0}",
                    "Hola [emb exp=\"f.player\"]{PL_0}",
                ],
            };
            let next = Line {
                text: [
                    "[link target=*next]進む[endlink]",
                    "[link target=*next]Continue[endlink]",
                    "[link target=*next]Continuar[endlink]",
                ],
                masked: [
                    "[link target=*next]進む{PL_0}",
                    "[link target=*next]Continue{PL_0}",
                    "[link target=*next]Continuar{PL_0}",
                ],
            };
            let back = Line {
                text: [
                    "[link target=*back]戻る[endlink]",
                    "[link target=*back]Return[endlink]",
                    "[link target=*back]Volver[endlink]",
                ],
                masked: [
                    "[link target=*back]戻る{PL_0}",
                    "[link target=*back]Return{PL_0}",
                    "[link target=*back]Volver{PL_0}",
                ],
            };
            let script = format!(
                r#"; NON_TEXT_SENTINEL
*start
@name text="{}"
{}
{}
[p]
{}
{}
{}
[iscript]
f.asset = "NON_TEXT_SENTINEL.png";
[endscript]
*next
[jump target=*back]
*back
"#,
                title.text[0],
                control.text[0],
                duplicate.text[0],
                duplicate.text[0],
                next.text[0],
                back.text[0]
            );
            files.insert("scenario/story.ks".into(), script.into_bytes());
            for (locator, line) in [
                ("kag:3:attr:0:text", title),
                ("kag:4", control),
                ("kag:5", duplicate),
                ("kag:7", duplicate),
                ("kag:8", next),
                ("kag:9", back),
            ] {
                slot(
                    "scenario/story.ks",
                    &format!("scenario/story.ks#{locator}"),
                    line,
                );
            }
            control
        }
    };
    // This prefix catches accidental blanket exclusions of `.locust*` files.
    files.insert(
        ".locust-not-metadata.bin".into(),
        b"\0KEEP\xff\x10".to_vec(),
    );
    files.insert(
        "assets/NON_TEXT_SENTINEL.png".into(),
        b"\x89PNG\r\n\x1a\nfixture\0".to_vec(),
    );
    Fixture {
        engine,
        files,
        slots,
        control,
    }
}

#[derive(Clone, Copy, Debug)]
enum Fault {
    MissingJapaneseAnswer,
    CorruptSpanishPlaceholder,
}

struct Scenario {
    root: tempfile::TempDir,
    fixture: Fixture,
    server: ScriptedOpenAi,
}

impl Scenario {
    fn new(engine: Engine, fault: Option<Fault>) -> Self {
        let fixture = fixture(engine);
        let mut answers = BTreeMap::new();
        for line in fixture.slots.values() {
            for (index, (source, target)) in [("ja", "en"), ("en", "es")].into_iter().enumerate() {
                let key = RequestKey::new(source, target, line.masked[index]);
                let answer = Answer::Text(line.masked[index + 1].into());
                answers.insert(key, answer);
            }
        }
        match fault {
            Some(Fault::MissingJapaneseAnswer) => {
                answers.insert(
                    RequestKey::new("ja", "en", fixture.control.masked[0]),
                    Answer::Missing,
                );
            }
            Some(Fault::CorruptSpanishPlaceholder) => {
                let broken = fixture.control.masked[2].replace("{PL_0}", "{PL_99}");
                answers.insert(
                    RequestKey::new("en", "es", fixture.control.masked[1]),
                    Answer::Text(broken),
                );
            }
            None => {}
        }
        let server = ScriptedOpenAi::start(answers);
        let root = tempfile::Builder::new()
            .prefix("locust_pipeline_")
            .tempdir()
            .unwrap();
        for copy in ["original", "work", "install"] {
            for (relative, bytes) in &fixture.files {
                let destination = root.path().join(copy).join(relative);
                fs::create_dir_all(destination.parent().unwrap()).unwrap();
                fs::write(destination, bytes).unwrap();
            }
        }
        fs::create_dir(root.path().join("profile")).unwrap();
        fs::write(
            root.path().join("profile/config.json"),
            serde_json::to_vec_pretty(&json!({
                "providers": {(PROVIDER): {
                    "api_key": DUMMY_KEY, "model": MODEL, "base_url": server.base_url(),
                    "extra": {"requests_per_minute": "4294967295"}
                }}
            }))
            .unwrap(),
        )
        .unwrap();
        Self {
            root,
            fixture,
            server,
        }
    }

    fn path(&self, relative: &str) -> PathBuf {
        self.root.path().join(relative)
    }

    fn cli(&self) -> Command {
        let mut command = Command::cargo_bin("locust").unwrap();
        command
            .current_dir(self.root.path())
            .env("LOCUST_DATA_DIR", self.path("profile"))
            .env("LOCUST_BACKUP_ROOT", self.path("backups"))
            .env("LOCUST_CONFIG", self.path("profile/config.json"))
            .env("NO_PROXY", "127.0.0.1,localhost")
            .env_remove("HTTP_PROXY")
            .env_remove("HTTPS_PROXY")
            .env_remove("ALL_PROXY")
            .timeout(Duration::from_secs(60));
        command
    }

    fn extract(&self, copy: &str, database: &str) {
        self.cli()
            .arg("extract")
            .arg(self.path(copy))
            .args(["--format", self.fixture.engine.id(), "-o"])
            .arg(self.path(database))
            .assert()
            .success();
    }

    fn translate(&self, database: &str, source: &str, target: &str) -> assert_cmd::assert::Assert {
        let result = self
            .cli()
            .arg("translate")
            .arg(self.path(database))
            .args([
                "-p",
                PROVIDER,
                "-s",
                source,
                "-t",
                target,
                "--batch-size",
                "1",
                "--concurrency",
                "1",
                "--no-memory",
            ])
            .assert();
        self.server.requests();
        result
    }

    fn pivot(&self, database: &str, output: &str) -> Command {
        let mut command = self.cli();
        command
            .arg("pivot")
            .arg(self.path(database))
            .arg("-o")
            .arg(self.path(output))
            .arg("--require-complete");
        command
    }
}

fn rows(path: &Path) -> Vec<StringEntry> {
    let db = Database::open(path).unwrap();
    assert_eq!(
        db.memory_count().unwrap(),
        0,
        "--no-memory must not populate memory"
    );
    db.get_entries(&EntryFilter::default()).unwrap()
}

fn snapshot(path: &Path) -> Value {
    serde_json::to_value(rows(path)).unwrap()
}

fn inventory<'a>(entries: &'a [StringEntry], root: &Path) -> BTreeMap<Slot, &'a StringEntry> {
    let root = root.canonicalize().unwrap();
    let mut result = BTreeMap::new();
    for row in entries {
        let relative = row
            .file_path
            .canonicalize()
            .unwrap()
            .strip_prefix(&root)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        assert!(
            result.insert((relative, row.id.clone()), row).is_none(),
            "duplicate slot id"
        );
    }
    result
}

fn assert_stage(scenario: &Scenario, database: &str, stage: usize) -> Vec<StringEntry> {
    let entries = rows(&scenario.path(database));
    let actual = inventory(&entries, &scenario.path("work"));
    assert_eq!(
        actual.keys().collect::<Vec<_>>(),
        scenario.fixture.slots.keys().collect::<Vec<_>>()
    );
    for (key, row) in actual {
        let line = scenario.fixture.slots[&key];
        assert_eq!(
            row.source,
            line.text[stage.saturating_sub(1)],
            "source at {key:?}"
        );
        assert_eq!(
            row.injection_source().unwrap(),
            line.text[0],
            "Japanese provenance at {key:?}"
        );
        if stage == 0 {
            assert_eq!(row.status, StringStatus::Pending);
            assert!(row.translation.is_none() && row.provider_used.is_none());
        } else {
            assert_eq!(
                row.translation.as_deref(),
                Some(line.text[stage]),
                "answer at {key:?}"
            );
            assert_eq!(row.status, StringStatus::Translated);
            assert_eq!(row.provider_used.as_deref(), Some(PROVIDER));
            assert!(row.translated_at.is_some());
            row.require_current_translation().unwrap();
            row.require_preserved_translation_controls().unwrap();
        }
    }
    entries
}

fn strict_english_pivot(scenario: &Scenario) {
    let before = snapshot(&scenario.path("ja.db"));
    let english = rows(&scenario.path("ja.db"));
    scenario.pivot("ja.db", "es.db").assert().success();
    assert_eq!(
        snapshot(&scenario.path("ja.db")),
        before,
        "pivot changed the Japanese DB"
    );
    let pivot = rows(&scenario.path("es.db"));
    assert_eq!(pivot.len(), english.len());
    for (row, prior) in pivot.iter().zip(&english) {
        assert_eq!(row.id, prior.id);
        assert_eq!(row.file_path, prior.file_path);
        assert_eq!(row.source.as_str(), prior.translation.as_deref().unwrap());
        assert_eq!(row.injection_source().unwrap(), prior.source);
        assert_eq!(row.metadata[INJECTION_SOURCE_METADATA_KEY], prior.source);
        assert_eq!(row.tags, prior.tags);
        assert_eq!(row.context, prior.context);
        let mut metadata = row.metadata.clone();
        metadata.remove(INJECTION_SOURCE_METADATA_KEY);
        assert_eq!(metadata, prior.metadata);
        assert_eq!(row.status, StringStatus::Pending);
        assert!(row.translation.is_none() && row.provider_used.is_none());
    }
}

fn assert_requests(scenario: &Scenario, stages: usize) {
    let requests = scenario.server.requests();
    let mut expected = BTreeSet::new();
    for line in scenario.fixture.slots.values() {
        for (index, (source, target)) in [("ja", "en"), ("en", "es")]
            .into_iter()
            .take(stages)
            .enumerate()
        {
            expected.insert(RequestKey::new(source, target, line.masked[index]));
        }
    }
    // Repeated text may be coalesced by the engine; evidence still covers every
    // distinct masked source, and the database asserts every physical slot.
    assert_eq!(requests.iter().cloned().collect::<BTreeSet<_>>(), expected);
    let first_spanish = requests.iter().position(|r| r.source_lang == "en");
    if stages == 2 {
        let boundary = first_spanish.expect("no EN -> ES requests");
        assert!(requests[..boundary].iter().all(|r| r.source_lang == "ja"));
        assert!(requests[boundary..].iter().all(|r| r.source_lang == "en"));
    }
}

/// Complete tree, including .locust paths: no metadata-prefix exclusions.
fn files(root: &Path) -> Files {
    let mut result = Files::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(directory) = stack.pop() {
        for item in fs::read_dir(directory).unwrap() {
            let item = item.unwrap();
            let path = item.path();
            let kind = item.file_type().unwrap();
            assert!(!kind.is_symlink(), "unexpected link: {}", path.display());
            if kind.is_dir() {
                stack.push(path);
            } else {
                assert!(kind.is_file());
                let relative = path
                    .strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/");
                assert!(result.insert(relative, fs::read(path).unwrap()).is_none());
            }
        }
    }
    result
}

fn assert_nontext(scenario: &Scenario, copy: &str) {
    for relative in [".locust-not-metadata.bin", "assets/NON_TEXT_SENTINEL.png"] {
        assert_eq!(
            fs::read(scenario.path(copy).join(relative)).unwrap(),
            scenario.fixture.files[relative]
        );
    }
    match scenario.fixture.engine {
        Engine::RenPy => {
            assert!(
                fs::read_to_string(scenario.path(copy).join("game/script.rpy"))
                    .unwrap()
                    .contains("    $ asset = \"NON_TEXT_SENTINEL.png\"\n")
            );
        }
        Engine::KiriKiri => {
            assert!(
                fs::read_to_string(scenario.path(copy).join("scenario/story.ks"))
                    .unwrap()
                    .contains("[iscript]\nf.asset = \"NON_TEXT_SENTINEL.png\";\n[endscript]")
            );
        }
        Engine::RpgMakerMv => {
            let path = "data/Map001.json";
            let mut before: Value = serde_json::from_slice(&scenario.fixture.files[path]).unwrap();
            let mut after: Value =
                serde_json::from_slice(&fs::read(scenario.path(copy).join(path)).unwrap()).unwrap();
            // Only the four message/choice payloads and their two branch labels
            // are editable. Compare the rest of the actual event tree exactly.
            for value in [&mut before, &mut after] {
                let list = value["events"][1]["pages"][0]["list"]
                    .as_array_mut()
                    .unwrap();
                for index in [1, 3, 5, 6] {
                    list[index]["parameters"][0] = Value::Null;
                }
                for index in [7, 9] {
                    list[index]["parameters"][1] = Value::Null;
                }
            }
            assert_eq!(after, before, "nontext RPG Maker fields changed");
            let system: Value = serde_json::from_slice(
                &fs::read(scenario.path(copy).join("data/System.json")).unwrap(),
            )
            .unwrap();
            assert_eq!(system["switches"], json!([null, "NON_TEXT_SENTINEL"]));
        }
    }
}

fn inspect_zip(scenario: &Scenario) -> (PatchManifest, Files) {
    let mut archive =
        zip::ZipArchive::new(fs::File::open(scenario.path("patch.zip")).unwrap()).unwrap();
    let mut members = Files::new();
    for index in 0..archive.len() {
        let mut member = archive.by_index(index).unwrap();
        assert!(!member.is_dir(), "unexpected directory entry");
        let mut bytes = Vec::new();
        member.read_to_end(&mut bytes).unwrap();
        assert!(
            members.insert(member.name().into(), bytes).is_none(),
            "duplicate ZIP member"
        );
    }
    let manifest: PatchManifest =
        serde_json::from_slice(&members.remove(PatchManifest::FILENAME).unwrap()).unwrap();
    let readme = members
        .remove("README.txt")
        .expect("packaging instructions");
    assert_eq!(
        readme,
        br#"rule95 / Locust translation patch

Preferred apply:  locust apply <game> <this.zip>
Manual apply:     extract over your game folder, replacing files.
Back up your game folder first (locust apply does this for you).

This patch contains modified game files, which may include complete
asset bundles. Get the base game from the original creator.
"#
    );
    assert_eq!(manifest.engine, scenario.fixture.engine.id());
    assert_eq!(manifest.language, "es");
    assert_eq!(manifest.schema_version, PatchManifest::SCHEMA_VERSION);
    assert!(manifest.supports_strict_tier());
    let work = files(&scenario.path("work"));
    let changed: BTreeSet<_> = scenario
        .fixture
        .files
        .iter()
        .filter(|(path, bytes)| work.get(*path) != Some(*bytes))
        .map(|(path, _)| path.clone())
        .collect();
    assert!(!changed.is_empty());
    assert_eq!(members.keys().cloned().collect::<BTreeSet<_>>(), changed);
    assert_eq!(
        manifest
            .files
            .iter()
            .map(|entry| entry.path.clone())
            .collect::<BTreeSet<_>>(),
        changed
    );
    assert_eq!(manifest.files.len(), changed.len());
    for entry in &manifest.files {
        assert!(!entry.added);
        assert_eq!(members[&entry.path], work[&entry.path]);
        assert_eq!(entry.size, members[&entry.path].len() as u64);
        assert_eq!(entry.patched_sha256, sha256_hex(&members[&entry.path]));
        assert_eq!(
            entry.original_sha256.as_deref(),
            Some(sha256_hex(&scenario.fixture.files[&entry.path]).as_str())
        );
    }
    (manifest, members)
}

fn assert_installed_tree(scenario: &Scenario, manifest: &PatchManifest, payload: &Files) {
    let installed = files(&scenario.path("install"));
    let receipt: Receipt = serde_json::from_slice(&installed[".locust/receipt.json"]).unwrap();
    assert_eq!(receipt.patch_id, manifest.patch_id);
    assert_eq!(receipt.verification, VerificationTier::Strict);
    assert_eq!(receipt.baseline, BackupBaseline::Pristine);
    assert!(!receipt.forced && receipt.added.is_empty());
    assert_eq!(receipt.language, "es");
    assert_eq!(receipt.engine, scenario.fixture.engine.id());
    let backup: BackupManifest =
        serde_json::from_slice(&installed[".locust/backup/manifest.json"]).unwrap();
    assert_eq!(backup.baseline, BackupBaseline::Pristine);
    assert_eq!(backup.files.len(), payload.len());
    assert_eq!(receipt.replaced.len(), payload.len());
    let mut expected = scenario.fixture.files.clone();
    expected.extend(payload.clone());
    // Enumerate only the actual operation-owned metadata, then validate every
    // backup and receipt member. Any other .locust file fails tree equality.
    for metadata in [".locust/receipt.json", ".locust/backup/manifest.json"] {
        expected.insert(metadata.into(), installed[metadata].clone());
    }
    for entry in &backup.files {
        let original = &scenario.fixture.files[&entry.path];
        assert_eq!(entry.sha256, sha256_hex(original));
        assert_eq!(entry.size, original.len() as u64);
        expected.insert(
            format!(".locust/backup/files/{}", entry.path),
            original.clone(),
        );
    }
    for entry in &receipt.replaced {
        assert_eq!(
            entry.original_sha256.as_deref(),
            Some(sha256_hex(&scenario.fixture.files[&entry.path]).as_str())
        );
        assert_eq!(entry.patched_sha256, sha256_hex(&payload[&entry.path]));
    }
    assert_eq!(installed, expected);
}

fn positive(engine: Engine) {
    let scenario = Scenario::new(engine, None);
    let original = files(&scenario.path("original"));
    let original_hashes: BTreeMap<_, _> = original
        .iter()
        .map(|(path, bytes)| (path.clone(), sha256_hex(bytes)))
        .collect();
    scenario.extract("work", "ja.db");
    assert_stage(&scenario, "ja.db", 0);
    scenario.translate("ja.db", "ja", "en").success();
    assert_stage(&scenario, "ja.db", 1);
    strict_english_pivot(&scenario);
    let japanese_db = snapshot(&scenario.path("ja.db"));
    scenario.translate("es.db", "en", "es").success();
    let spanish = assert_stage(&scenario, "es.db", 2);
    assert_requests(&scenario, 2);
    let validation = scenario
        .cli()
        .arg("validate")
        .arg(scenario.path("es.db"))
        .arg("--json")
        .assert()
        .success();
    let report: Value = serde_json::from_slice(&validation.get_output().stdout).unwrap();
    assert_eq!(report["count"], 0);
    assert_eq!(report["issues"], json!([]));
    scenario
        .cli()
        .arg("inject")
        .arg(scenario.path("work"))
        .arg("-P")
        .arg(scenario.path("es.db"))
        .args(["--direct", "-l", "es"])
        .assert()
        .success();
    assert_nontext(&scenario, "work");
    scenario
        .cli()
        .arg("patch")
        .arg(scenario.path("work"))
        .arg("-P")
        .arg(scenario.path("es.db"))
        .args(["-l", "es", "--pristine"])
        .arg(scenario.path("original"))
        .arg("-o")
        .arg(scenario.path("patch.zip"))
        .assert()
        .success();
    let (manifest, payload) = inspect_zip(&scenario);
    let archive_hash = sha256_hex(&fs::read(scenario.path("patch.zip")).unwrap());
    scenario
        .cli()
        .arg("apply")
        .arg(scenario.path("install"))
        .arg(scenario.path("patch.zip"))
        .assert()
        .success();
    assert_installed_tree(&scenario, &manifest, &payload);
    assert_nontext(&scenario, "install");
    scenario.extract("install", "reextracted.db");
    let reextracted = rows(&scenario.path("reextracted.db"));
    let expected = inventory(&spanish, &scenario.path("work"));
    let actual = inventory(&reextracted, &scenario.path("install"));
    assert_eq!(
        actual.keys().collect::<Vec<_>>(),
        expected.keys().collect::<Vec<_>>()
    );
    for (key, row) in actual {
        assert_eq!(
            Some(row.source.as_str()),
            expected[&key].translation.as_deref(),
            "applied slot {key:?}"
        );
    }
    scenario
        .cli()
        .arg("patch-rollback")
        .arg(scenario.path("install"))
        .assert()
        .success();
    assert_eq!(
        files(&scenario.path("install")),
        original,
        "rollback must restore exact bytes and remove owned metadata"
    );
    assert_eq!(files(&scenario.path("original")), original);
    assert_eq!(
        files(&scenario.path("original"))
            .iter()
            .map(|(path, bytes)| (path.clone(), sha256_hex(bytes)))
            .collect::<BTreeMap<_, _>>(),
        original_hashes
    );
    assert_eq!(
        sha256_hex(&fs::read(scenario.path("patch.zip")).unwrap()),
        archive_hash
    );
    assert_eq!(snapshot(&scenario.path("ja.db")), japanese_db);
    assert_stage(&scenario, "es.db", 2);
}

fn negative(engine: Engine, fault: Fault) {
    let scenario = Scenario::new(engine, Some(fault));
    scenario.extract("work", "ja.db");
    let (database, stage, diagnostic) = match fault {
        Fault::MissingJapaneseAnswer => (
            "ja.db",
            0,
            "translation count mismatch: sent 1 strings, got 0 back",
        ),
        Fault::CorruptSpanishPlaceholder => {
            scenario.translate("ja.db", "ja", "en").success();
            assert_stage(&scenario, "ja.db", 1);
            strict_english_pivot(&scenario);
            ("es.db", 1, "placeholder")
        }
    };
    let (source, target) = if stage == 0 {
        ("ja", "en")
    } else {
        ("en", "es")
    };
    let translation = scenario.translate(database, source, target).failure();
    let stderr = String::from_utf8_lossy(&translation.get_output().stderr);
    assert!(stderr.contains(diagnostic), "{stderr}");
    assert!(stderr.contains("Translation incomplete:"), "{stderr}");
    // A strict completeness preflight must also refuse this inventory.
    let entries = rows(&scenario.path(database));
    let actual = inventory(&entries, &scenario.path("work"));
    assert_eq!(
        actual.keys().collect::<Vec<_>>(),
        scenario.fixture.slots.keys().collect::<Vec<_>>()
    );
    let mut rejected = 0;
    for (key, row) in actual {
        let line = scenario.fixture.slots[&key];
        assert_eq!(row.source, line.text[stage]);
        assert_eq!(row.injection_source().unwrap(), line.text[0]);
        if line.text[0] == scenario.fixture.control.text[0] {
            rejected += 1;
            assert_eq!(row.status, StringStatus::Pending);
            assert!(row.translation.is_none() && row.provider_used.is_none());
        } else {
            assert_eq!(
                row.status,
                StringStatus::Translated,
                "unexpected rejection at {key:?}: {stderr}"
            );
            assert_eq!(row.translation.as_deref(), Some(line.text[stage + 1]));
            assert_eq!(row.provider_used.as_deref(), Some(PROVIDER));
        }
    }
    assert_eq!(rejected, 1);
    let before = snapshot(&scenario.path(database));
    scenario
        .pivot(database, "refused.db")
        .assert()
        .failure()
        .stderr(predicates::str::contains("missing or blank translation"));
    assert_eq!(snapshot(&scenario.path(database)), before);
    for suffix in ["", "-wal", "-shm", "-journal"] {
        assert!(!scenario.path(&format!("refused.db{suffix}")).exists());
    }
    assert!(
        !scenario.path("patch.zip").exists(),
        "no release after failed preflight"
    );
    for copy in ["original", "work", "install"] {
        assert_eq!(files(&scenario.path(copy)), scenario.fixture.files);
    }
    assert!(!scenario.path("backups").exists());
    assert_requests(&scenario, stage + 1);
}

#[test]
fn renpy_ja_en_es_strict_patch_roundtrip() {
    positive(Engine::RenPy);
}

#[test]
fn rpgmaker_mv_ja_en_es_strict_patch_roundtrip() {
    positive(Engine::RpgMakerMv);
}

#[test]
fn loose_kirikiri_ja_en_es_strict_patch_roundtrip() {
    positive(Engine::KiriKiri);
}

#[test]
fn missing_answers_and_corrupt_placeholders_stop_release_for_every_engine() {
    for engine in [Engine::RenPy, Engine::RpgMakerMv, Engine::KiriKiri] {
        for fault in [
            Fault::MissingJapaneseAnswer,
            Fault::CorruptSpanishPlaceholder,
        ] {
            negative(engine, fault);
        }
    }
}

#[test]
fn strict_pivot_carries_warning_metadata_but_is_not_an_extraction_warning_gate() {
    let scenario = Scenario::new(Engine::RenPy, None);
    scenario.extract("work", "ja.db");
    scenario.translate("ja.db", "ja", "en").success();
    assert_stage(&scenario, "ja.db", 1);
    // These three loose engines do not emit persisted extraction warnings.
    // Seed ONLY metadata to characterize carry-forward; never seed translations
    // or pretend that this represents a real incomplete Ren'Py extraction.
    let warnings = json!(["synthetic characterization: an unread container"]);
    {
        let db = Database::open(&scenario.path("ja.db")).unwrap();
        db.set_project_metadata("extraction_warnings", &warnings)
            .unwrap();
    }
    strict_english_pivot(&scenario);
    let pivot = Database::open(&scenario.path("es.db")).unwrap();
    assert_eq!(
        pivot.get_project_metadata("extraction_warnings").unwrap(),
        Some(warnings)
    );
    assert!(!scenario.path("patch.zip").exists());
    assert_requests(&scenario, 1);
}
