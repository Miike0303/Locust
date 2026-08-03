//! Integration tests for automatic patch apply / rollback (design rev 4).
//! Covers CRITICAL-A/B/C regressions and the basic apply→rollback round-trip.

use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};

use locust_core::database::sha256_hex;
use locust_core::error::LocustError;
use locust_core::patch::manifest::{BackupManifest, PatchFileEntry, PatchManifest};
use locust_core::patch::{
    apply, rollback, verify, ApplyOptions, RollbackOptions, VerificationOutcome,
};
use zip::write::SimpleFileOptions;
use zip::ZipWriter;

fn tmp_game(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "locust_patch_test_{}_{}",
        name,
        uuid::Uuid::new_v4()
    ));
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn write_file(root: &Path, rel: &str, contents: &[u8]) {
    let p = root.join(rel.replace('/', std::path::MAIN_SEPARATOR_STR));
    if let Some(parent) = p.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(p, contents).unwrap();
}

fn build_patch_zip(
    path: &Path,
    files: &[(&str, &[u8], Option<&[u8]>)], // rel, patched bytes, optional original bytes
    version: &str,
    patch_id: &str,
) {
    let manifest = PatchManifest {
        schema_version: 1,
        patch_id: patch_id.into(),
        game_name: "test".into(),
        engine: "test_engine".into(),
        language: "es".into(),
        patch_version: version.into(),
        generator_version: "0.1.0".into(),
        created_at: "now".into(),
        files: files
            .iter()
            .map(|(rel, patched, orig)| PatchFileEntry {
                path: (*rel).into(),
                patched_sha256: sha256_hex(patched),
                size: patched.len() as u64,
                original_sha256: orig.map(sha256_hex),
            })
            .collect(),
    };
    let f = File::create(path).unwrap();
    let mut zip = ZipWriter::new(f);
    let opts = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    zip.start_file("locust-patch.json", opts).unwrap();
    zip.write_all(serde_json::to_string_pretty(&manifest).unwrap().as_bytes())
        .unwrap();
    for (rel, patched, _) in files {
        zip.start_file(*rel, opts).unwrap();
        zip.write_all(patched).unwrap();
    }
    zip.finish().unwrap();
}

#[test]
fn apply_then_rollback_is_byte_identical_including_added_file() {
    let game = tmp_game("roundtrip");
    write_file(&game, "data/Map001.json", b"ORIGINAL_MAP");
    let zip = game.join("patch.zip");
    build_patch_zip(
        &zip,
        &[
            ("data/Map001.json", b"PATCHED_MAP", Some(b"ORIGINAL_MAP")),
            ("data/NewFile.json", b"ADDED", None),
        ],
        "1.0.0",
        "patch-a",
    );

    let report = apply(&game, &zip, ApplyOptions::default(), |_| {}).unwrap();
    assert_eq!(report.replaced, 1);
    assert_eq!(report.added, 1);
    assert_eq!(
        fs::read(game.join("data").join("Map001.json")).unwrap(),
        b"PATCHED_MAP"
    );
    assert_eq!(
        fs::read(game.join("data").join("NewFile.json")).unwrap(),
        b"ADDED"
    );

    let rb = rollback(&game, RollbackOptions::default()).unwrap();
    assert_eq!(rb.restored, 1);
    assert_eq!(rb.deleted, 1);
    assert_eq!(
        fs::read(game.join("data").join("Map001.json")).unwrap(),
        b"ORIGINAL_MAP"
    );
    assert!(!game.join("data").join("NewFile.json").exists());
    assert!(!game.join(".locust").exists());

    let _ = fs::remove_dir_all(&game);
}

#[test]
fn already_applied_blocks_without_force() {
    let game = tmp_game("already");
    write_file(&game, "data/a.json", b"ORIG");
    let zip = game.join("p.zip");
    build_patch_zip(
        &zip,
        &[("data/a.json", b"NEW", Some(b"ORIG"))],
        "1.0.0",
        "id1",
    );
    apply(&game, &zip, ApplyOptions::default(), |_| {}).unwrap();
    let err = apply(&game, &zip, ApplyOptions::default(), |_| {}).unwrap_err();
    assert!(matches!(err, LocustError::PatchAlreadyApplied(_)));
    let _ = fs::remove_dir_all(&game);
}

#[test]
fn critical_a_forced_reapply_after_reclassified_conflict_still_restores_user_original() {
    // User file at an "added" path → force reclassifies to replaced and backs it up.
    // Forced same-version reapply must keep that original in the backup (R1).
    let game = tmp_game("crit_a");
    write_file(&game, "data/base.json", b"BASE_ORIG");
    write_file(&game, "data/extra.json", b"USER_FILE"); // conflict for intended-add
    let zip = game.join("p.zip");
    build_patch_zip(
        &zip,
        &[
            ("data/base.json", b"BASE_PATCH", Some(b"BASE_ORIG")),
            ("data/extra.json", b"EXTRA_PATCH", None), // intended add
        ],
        "1.0.0",
        "id-a",
    );

    // First apply needs force for conflict.
    apply(
        &game,
        &zip,
        ApplyOptions {
            force: true,
            ..Default::default()
        },
        |_| {},
    )
    .unwrap();
    assert_eq!(
        fs::read(game.join("data").join("extra.json")).unwrap(),
        b"EXTRA_PATCH"
    );

    // Forced same-version reapply.
    apply(
        &game,
        &zip,
        ApplyOptions {
            force: true,
            ..Default::default()
        },
        |_| {},
    )
    .unwrap();

    // Rollback must restore USER_FILE (CRITICAL-A).
    rollback(
        &game,
        RollbackOptions {
            delete_modified_added: true,
        },
    )
    .unwrap();
    assert_eq!(
        fs::read(game.join("data").join("base.json")).unwrap(),
        b"BASE_ORIG"
    );
    assert_eq!(
        fs::read(game.join("data").join("extra.json")).unwrap(),
        b"USER_FILE",
        "CRITICAL-A: user original at reclassified path must be restored"
    );
    let _ = fs::remove_dir_all(&game);
}

#[test]
fn critical_b_receipt_present_manifest_less_backup_hard_errors() {
    let game = tmp_game("crit_b");
    write_file(&game, "data/a.json", b"ORIG");
    let zip = game.join("p.zip");
    build_patch_zip(
        &zip,
        &[("data/a.json", b"NEW", Some(b"ORIG"))],
        "1.0.0",
        "id-b",
    );
    apply(&game, &zip, ApplyOptions::default(), |_| {}).unwrap();

    // Destroy the commit marker, leave backup files + receipt.
    fs::remove_file(game.join(".locust").join("backup").join("manifest.json")).unwrap();
    // Leave a decoy so backup/ still exists.
    fs::write(
        game.join(".locust").join("backup").join("files").join("keep"),
        b"x",
    )
    .unwrap();

    let err = apply(
        &game,
        &zip,
        ApplyOptions {
            force: true,
            ..Default::default()
        },
        |_| {},
    )
    .unwrap_err();
    assert!(
        matches!(err, LocustError::PatchBackupIncomplete(_)),
        "got {err:?}"
    );

    let err2 = rollback(
        &game,
        RollbackOptions {
            delete_modified_added: true,
        },
    )
    .unwrap_err();
    assert!(matches!(err2, LocustError::PatchBackupIncomplete(_)));
    // Receipt and decoy must still be there — nothing discarded.
    assert!(game.join(".locust").join("receipt.json").is_file());
    assert!(game
        .join(".locust")
        .join("backup")
        .join("files")
        .join("keep")
        .is_file());
    let _ = fs::remove_dir_all(&game);
}

#[test]
fn critical_c_structural_manifest_less_backup_not_discarded() {
    // Pure-replacement structural patch (no original hashes). Manifest-less
    // backup/ with surviving files and no receipt/journal → R2 hard error.
    let game = tmp_game("crit_c");
    write_file(&game, "www/data/Map001.json", b"ORIG");
    // Fake a leftover backup without commit marker (externally lost).
    let backup_file = game
        .join(".locust")
        .join("backup")
        .join("files")
        .join("www")
        .join("data")
        .join("Map001.json");
    fs::create_dir_all(backup_file.parent().unwrap()).unwrap();
    fs::write(&backup_file, b"PRISTINE_COPY").unwrap();

    let zip = game.join("p.zip");
    // Structural: no original_sha256
    build_patch_zip(
        &zip,
        &[("www/data/Map001.json", b"PATCHED", None)],
        "1.0.0",
        "id-c",
    );

    let err = apply(
        &game,
        &zip,
        ApplyOptions {
            force: true,
            confirm_legacy: true,
            ..Default::default()
        },
        |_| {},
    )
    .unwrap_err();
    assert!(
        matches!(err, LocustError::PatchBackupIncomplete(_)),
        "CRITICAL-C: structural must not discard manifest-less backup, got {err:?}"
    );
    assert_eq!(fs::read(&backup_file).unwrap(), b"PRISTINE_COPY");
    let _ = fs::remove_dir_all(&game);
}

#[test]
fn verify_clean_on_pristine_game() {
    let game = tmp_game("verify_clean");
    write_file(&game, "data/a.json", b"ORIG");
    let zip = game.join("p.zip");
    build_patch_zip(
        &zip,
        &[("data/a.json", b"NEW", Some(b"ORIG"))],
        "1.0.0",
        "id-v",
    );
    let r = verify(&game, &zip).unwrap();
    assert_eq!(r.outcome, VerificationOutcome::Clean);
    let _ = fs::remove_dir_all(&game);
}

#[test]
fn unsafe_zip_entry_rejected() {
    let game = tmp_game("unsafe");
    fs::create_dir_all(&game).unwrap();
    let zip_path = game.join("bad.zip");
    let f = File::create(&zip_path).unwrap();
    let mut zip = ZipWriter::new(f);
    let opts = SimpleFileOptions::default();
    zip.start_file("foo/../../evil.txt", opts).unwrap();
    zip.write_all(b"nope").unwrap();
    zip.finish().unwrap();

    let err = verify(&game, &zip_path).unwrap_err();
    assert!(matches!(err, LocustError::PatchUnsafeEntry(_)), "{err:?}");
    let _ = fs::remove_dir_all(&game);
}

#[test]
fn r1_deletion_set_never_includes_backup_manifest_paths() {
    // Even if a (corrupt) receipt lists a path as added that is also in the
    // backup manifest, rollback restores it and does not delete it.
    let game = tmp_game("r1_veto");
    write_file(&game, "data/a.json", b"ORIG");
    let zip = game.join("p.zip");
    build_patch_zip(
        &zip,
        &[("data/a.json", b"NEW", Some(b"ORIG"))],
        "1.0.0",
        "id-r1",
    );
    apply(&game, &zip, ApplyOptions::default(), |_| {}).unwrap();

    // Corrupt receipt: claim a.json is added (should be replaced).
    let receipt_path = game.join(".locust").join("receipt.json");
    let mut receipt: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&receipt_path).unwrap()).unwrap();
    receipt["added"] = serde_json::json!([{
        "path": "data/a.json",
        "patched_sha256": sha256_hex(b"NEW"),
    }]);
    receipt["replaced"] = serde_json::json!([]);
    fs::write(&receipt_path, serde_json::to_string_pretty(&receipt).unwrap()).unwrap();

    rollback(&game, RollbackOptions::default()).unwrap();
    assert_eq!(
        fs::read(game.join("data").join("a.json")).unwrap(),
        b"ORIG",
        "R1: path in backup manifest must be restored, never deleted"
    );
    let _ = fs::remove_dir_all(&game);
}

// Silence unused import if BackupManifest only used via side effects.
#[allow(dead_code)]
fn _touch_backup_type(_: &BackupManifest) {}


#[test]
fn upgrade_aborts_when_rollback_soft_fails_on_edited_added_file() {
    // CRITICAL: apply must not continue after a soft-abort rollback.
    let game = tmp_game("upgrade_abort");
    write_file(&game, "data/base.json", b"BASE_ORIG");
    let zip_v1 = game.join("v1.zip");
    build_patch_zip(
        &zip_v1,
        &[
            ("data/base.json", b"BASE_V1", Some(b"BASE_ORIG")),
            ("data/extra.json", b"EXTRA_V1", None),
        ],
        "1.0.0",
        "id-up",
    );
    apply(&game, &zip_v1, ApplyOptions::default(), |_| {}).unwrap();
    write_file(&game, "data/extra.json", b"USER_EDIT");

    let zip_v2 = game.join("v2.zip");
    build_patch_zip(
        &zip_v2,
        &[
            ("data/base.json", b"BASE_V2", Some(b"BASE_ORIG")),
            ("data/extra.json", b"EXTRA_V2", None),
        ],
        "1.1.0",
        "id-up",
    );
    let err = apply(&game, &zip_v2, ApplyOptions::default(), |_| {}).unwrap_err();
    assert!(
        matches!(err, LocustError::PatchVerificationFailed(_)),
        "upgrade must hard-fail when rollback aborts, got {err:?}"
    );
    assert_eq!(fs::read(game.join("data").join("base.json")).unwrap(), b"BASE_V1");
    assert_eq!(fs::read(game.join("data").join("extra.json")).unwrap(), b"USER_EDIT");
    let _ = fs::remove_dir_all(&game);
}

