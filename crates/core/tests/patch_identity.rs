use locust_core::patch::PatchManifest;
use serde_json::json;
use std::fs;
use std::path::Path;

use locust_core::database::{sha256_hex, Database};
use locust_core::models::{StringEntry, StringStatus};
use locust_core::patch::{
    apply, detect_dlsite_code, normalize_dlsite_code, pack_injection_recording, verify,
    ApplyOptions, GameIdentity, PackOptions, PatchStore, VerificationOutcome,
};

const OLD_MANIFEST: &str = r#"{"schema_version":1,"patch_id":"id","game_name":"game","engine":"renpy","language":"es","patch_version":"1.0.0","generator_version":"0.1.0","created_at":"t","files":[{"path":"game/a.rpy","patched_sha256":"aa","size":1,"original_sha256":"bb"}]}"#;

#[test]
fn manifest_without_game_round_trips_byte_identically() {
    let manifest: PatchManifest = serde_json::from_str(OLD_MANIFEST).unwrap();
    assert_eq!(serde_json::to_string(&manifest).unwrap(), OLD_MANIFEST);
    assert_eq!(PatchManifest::SCHEMA_VERSION, 1);
}

#[test]
fn manifest_with_game_round_trips_and_accepts_unknown_fields() {
    let mut value: serde_json::Value = serde_json::from_str(OLD_MANIFEST).unwrap();
    value["game"] = json!({
        "store_ids": {"dlsite": "RJ01234567", "steam": "123"},
        "game_version": "1.2",
        "fingerprint": [{"path": "game/a.rpy", "size": 99, "sha256": "bb"}]
    });
    let manifest: PatchManifest = serde_json::from_value(value.clone()).unwrap();
    assert_eq!(serde_json::to_value(&manifest).unwrap(), value);
    value["future"] = json!(true);
    value["game"]["future"] = json!(true);
    assert!(serde_json::from_value::<PatchManifest>(value).is_ok());
}

#[test]
fn manifest_empty_identity_omits_optional_fields() {
    let mut value: serde_json::Value = serde_json::from_str(OLD_MANIFEST).unwrap();
    value["game"] = json!({"store_ids": {}, "game_version": null, "fingerprint": []});
    let manifest: PatchManifest = serde_json::from_value(value).unwrap();
    assert_eq!(serde_json::to_value(manifest).unwrap()["game"], json!({}));
}

#[test]
fn dlsite_validation_accepts_all_prefixes_and_normalizes_case() {
    for prefix in ["RJ", "RE", "VJ", "BJ", "RG"] {
        for digits in ["123456", "01234567"] {
            let code = format!("{prefix}{digits}");
            assert_eq!(normalize_dlsite_code(&code).unwrap(), code);
            assert_eq!(normalize_dlsite_code(&code.to_lowercase()).unwrap(), code);
        }
    }
}

#[test]
fn dlsite_validation_rejects_invalid_codes() {
    for code in [
        "RJ12345",
        "RJ1234567",
        "RJ123456789",
        "XX123456",
        "RJ12345X",
        "RJ１２３４５６",
        " RJ123456",
        "RJ123456 ",
        "",
    ] {
        let error = normalize_dlsite_code(code).unwrap_err().to_string();
        assert!(
            error.contains("DLsite") && error.contains("6 or 8 digits"),
            "{error}"
        );
    }
}

#[test]
fn dlsite_path_detection_uses_whole_tokens_nearest_component_and_first_match() {
    for (path, expected) in [
        ("D:/games/[RJ01234567] Title/", Some("RJ01234567")),
        ("D:/games/[re123456] Title/game", Some("RE123456")),
        ("D:/games/ABRJ123456X/", None),
        ("D:/games/RJ123456789/", None),
        ("D:/games/éRJ123456/", None),
        ("D:/RJ123456/[vj01234567] Title/", Some("VJ01234567")),
        ("D:/games/[bj123456] [RG01234567]/", Some("BJ123456")),
        (r"D:\games\[RG01234567] Title\game", Some("RG01234567")),
    ] {
        assert_eq!(
            detect_dlsite_code(Path::new(path)).as_deref(),
            expected,
            "{path}"
        );
    }
}

const ORIGINALS: [(&str, &[u8]); 5] = [
    ("large.txt", b"original is much larger"),
    ("c.txt", b"cc"),
    ("b.txt", b"bb"),
    ("a.txt", b"aa"),
    ("z.txt", b"z"),
];

fn pack_fixture(base: &Path, source: &str, identity: bool) -> (std::path::PathBuf, PatchManifest) {
    let game = base.join("game");
    fs::create_dir_all(&game).unwrap();
    let pristine = if source == "backup" {
        PatchStore::new(&game).backup_files_dir()
    } else {
        base.join("pristine")
    };
    fs::create_dir_all(&pristine).unwrap();
    let db = Database::open_in_memory().unwrap();
    let mut files = Vec::new();
    for (name, original) in ORIGINALS.into_iter().chain([("added.txt", b"".as_slice())]) {
        let file = game.join(name);
        fs::write(&file, b"patched").unwrap();
        if source != "none" && name != "added.txt" {
            fs::write(pristine.join(name), original).unwrap();
        }
        let mut entry = StringEntry::new(name, "source", file.clone());
        entry.translation = Some("translation".into());
        entry.status = StringStatus::Translated;
        db.save_entries(&[entry]).unwrap();
        files.push(file);
    }
    if source == "backup" {
        let files: Vec<_> = ORIGINALS
            .iter()
            .map(|(path, bytes)| {
                json!({
                    "path": path, "size": bytes.len(), "sha256": sha256_hex(bytes)
                })
            })
            .collect();
        fs::write(
            PatchStore::new(&game).backup_manifest_path(),
            serde_json::to_vec(&json!({
                "schema_version": 1, "created_at": "t", "baseline": "pristine", "files": files
            }))
            .unwrap(),
        )
        .unwrap();
    }
    db.record_injection(Some("es"), &game, &files).unwrap();
    let output = base.join("patch.zip");
    pack_injection_recording(
        &db,
        PackOptions {
            game_path: game,
            lang: Some("es".into()),
            output: output.clone(),
            pristine: (source == "explicit").then_some(pristine),
            engine: Some("renpy".into()),
            project: base.join("project.db"),
            require_pristine: false,
            game: identity.then(GameIdentity::default),
        },
    )
    .unwrap();
    let mut zip = zip::ZipArchive::new(fs::File::open(&output).unwrap()).unwrap();
    let manifest = serde_json::from_reader(zip.by_name(PatchManifest::FILENAME).unwrap()).unwrap();
    (output, manifest)
}

#[test]
fn fingerprint_selects_three_smallest_originals_with_path_tiebreak() {
    for source in ["explicit", "backup"] {
        let base = tempfile::tempdir().unwrap();
        let (_, manifest) = pack_fixture(base.path(), source, true);
        let fingerprint = manifest.game.unwrap().fingerprint;
        assert_eq!(fingerprint.len(), 3);
        for (entry, (path, bytes)) in fingerprint.iter().zip([
            ("z.txt", b"z".as_slice()),
            ("a.txt", b"aa"),
            ("b.txt", b"bb"),
        ]) {
            assert_eq!(entry.path, path);
            assert_eq!(entry.size, bytes.len() as u64);
            assert_eq!(entry.sha256, sha256_hex(bytes));
            let file = manifest.files.iter().find(|f| f.path == path).unwrap();
            assert_eq!(file.original_sha256.as_ref(), Some(&entry.sha256));
            assert_ne!(entry.size, file.size);
        }
        assert!(manifest
            .files
            .iter()
            .find(|f| f.path == "added.txt")
            .unwrap()
            .original_sha256
            .is_none());
    }
}

#[test]
fn fingerprint_without_pristine_hashes_is_empty_and_omitted() {
    let base = tempfile::tempdir().unwrap();
    let (_, manifest) = pack_fixture(base.path(), "none", true);
    // An identity with no store ids, version or fingerprint carries nothing,
    // so the whole `game` block is omitted rather than written as `{}`.
    assert!(manifest.game.is_none());
    assert!(serde_json::to_value(manifest)
        .unwrap()
        .get("game")
        .is_none());
}

#[test]
fn apply_and_verify_ignore_game_identity() {
    let mut results = Vec::new();
    for identity in [false, true] {
        let base = tempfile::tempdir().unwrap();
        let (zip, manifest) = pack_fixture(base.path(), "explicit", identity);
        assert_eq!(manifest.game.is_some(), identity);
        let game = base.path().join("player");
        fs::create_dir(&game).unwrap();
        for (name, bytes) in ORIGINALS {
            fs::write(game.join(name), bytes).unwrap();
        }
        let before = verify(&game, &zip).unwrap();
        assert_eq!(before.outcome, VerificationOutcome::Clean);
        let applied = apply(&game, &zip, ApplyOptions::default(), |_| {}).unwrap();
        let after = verify(&game, &zip).unwrap();
        assert_eq!(after.outcome, VerificationOutcome::AlreadyApplied);
        for name in ORIGINALS.iter().map(|(name, _)| *name).chain(["added.txt"]) {
            assert_eq!(fs::read(game.join(name)).unwrap(), b"patched");
        }
        results.push((
            before.outcome,
            before.tier,
            before.replaced,
            before.added,
            before.messages,
            applied.replaced,
            applied.added,
            applied.baseline,
            applied.messages,
            after.outcome,
        ));
    }
    assert_eq!(results[0], results[1]);
}
