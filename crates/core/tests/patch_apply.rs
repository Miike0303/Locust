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

/// Relative path, patched bytes, optional original bytes (for original_sha256).
type PatchZipFile<'a> = (&'a str, &'a [u8], Option<&'a [u8]>);

fn build_patch_zip(
    path: &Path,
    files: &[PatchZipFile<'_>],
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

// ── Streaming / multi-GB budget tests ───────────────────────────────────────

/// Patch the first local-header **uncompressed** size field (offset 22) and
/// the matching central-directory field, leaving compressed size intact.
/// For Stored entries that makes `entry.size()` (declared) smaller than the
/// bytes the reader can still yield — streaming must abort as a zip bomb.
fn understate_uncompressed_sizes(zip_path: &Path, new_declared: u32) {
    let mut bytes = fs::read(zip_path).unwrap();
    // Local file header signature PK\x03\x04
    let local_sig = [0x50u8, 0x4b, 0x03, 0x04];
    let central_sig = [0x50u8, 0x4b, 0x01, 0x02];
    let mut patched = 0usize;
    for i in 0..bytes.len().saturating_sub(26) {
        if bytes[i..i + 4] == local_sig {
            // uncompressed size at +22
            bytes[i + 22..i + 26].copy_from_slice(&new_declared.to_le_bytes());
            patched += 1;
        }
        if bytes[i..i + 4] == central_sig {
            // central: uncompressed at +24
            bytes[i + 24..i + 28].copy_from_slice(&new_declared.to_le_bytes());
            patched += 1;
        }
    }
    assert!(patched >= 2, "expected local+central headers to patch, got {patched}");
    fs::write(zip_path, bytes).unwrap();
}

#[test]
fn streaming_apply_multi_entry_byte_identical() {
    // A few MB across multiple files — streaming path must not depend on size.
    let game = tmp_game("stream_multi");
    let chunk_a: Vec<u8> = (0..500_000u32).map(|i| (i % 251) as u8).collect();
    let chunk_b: Vec<u8> = (0..750_000u32).map(|i| (i % 241) as u8).collect();
    let orig_a: Vec<u8> = vec![0x11; chunk_a.len()];
    let orig_b: Vec<u8> = vec![0x22; chunk_b.len()];
    write_file(&game, "data/big_a.bin", &orig_a);
    write_file(&game, "data/big_b.bin", &orig_b);

    let zip = game.join("p.zip");
    build_patch_zip(
        &zip,
        &[
            ("data/big_a.bin", &chunk_a, Some(orig_a.as_slice())),
            ("data/big_b.bin", &chunk_b, Some(orig_b.as_slice())),
            ("data/small.txt", b"hello-stream", None),
        ],
        "1.0.0",
        "stream-id",
    );

    let report = apply(&game, &zip, ApplyOptions::default(), |_| {}).unwrap();
    assert_eq!(report.replaced, 2);
    assert_eq!(report.added, 1);
    assert_eq!(fs::read(game.join("data").join("big_a.bin")).unwrap(), chunk_a);
    assert_eq!(fs::read(game.join("data").join("big_b.bin")).unwrap(), chunk_b);
    assert_eq!(
        fs::read(game.join("data").join("small.txt")).unwrap(),
        b"hello-stream"
    );
    // Staging dir must not linger after success.
    let locust = game.join(".locust");
    if locust.is_dir() {
        for e in fs::read_dir(&locust).unwrap() {
            let name = e.unwrap().file_name().to_string_lossy().to_string();
            assert!(
                !name.starts_with("staging-"),
                "leftover staging dir: {name}"
            );
        }
    }
    let _ = fs::remove_dir_all(&game);
}

#[test]
fn actual_exceeds_declared_aborts_nothing_replaced() {
    let game = tmp_game("bomb_declared");
    write_file(&game, "data/a.json", b"ORIGINAL_KEEP");
    let zip = game.join("bomb.zip");
    // Build a normal stored zip with a real content file + manifest, then
    // understate uncompressed sizes so streaming hits actual > declared.
    let payload = vec![0x5Au8; 64 * 1024]; // 64 KiB
    build_patch_zip(
        &zip,
        &[("data/a.json", &payload, Some(b"ORIGINAL_KEEP"))],
        "1.0.0",
        "bomb-id",
    );
    understate_uncompressed_sizes(&zip, 1024); // declare 1 KiB, payload 64 KiB

    let v = verify(&game, &zip);
    assert!(
        v.is_err(),
        "verify must abort when stream exceeds declared size, got {v:?}"
    );
    let err = v.unwrap_err().to_string();
    assert!(
        err.contains("declared") || err.contains("bomb") || err.contains("expanded"),
        "loud abort message, got: {err}"
    );

    // Game file untouched (verify is read-only; apply would also refuse).
    assert_eq!(
        fs::read(game.join("data").join("a.json")).unwrap(),
        b"ORIGINAL_KEEP"
    );
    assert!(!game.join(".locust").join("receipt.json").exists());

    let apply_err = apply(&game, &zip, ApplyOptions::default(), |_| {});
    // apply runs verify first — same failure, nothing written.
    assert!(apply_err.is_err());
    assert_eq!(
        fs::read(game.join("data").join("a.json")).unwrap(),
        b"ORIGINAL_KEEP"
    );
    let _ = fs::remove_dir_all(&game);
}

#[test]
fn declared_over_total_ceiling_rejected_before_stream() {
    // Unit-level ceiling is in zipsec; here we assert the public helper used
    // by verify/apply rejects a declared size above an explicit ceiling.
    use locust_core::patch::zipsec::check_entry_budget_with;
    let ceiling = 1024u64;
    let err = check_entry_budget_with("huge.bin", ceiling + 1, 0, ceiling).unwrap_err();
    let s = err.to_string();
    assert!(
        s.contains("limit") || s.contains("expand") || s.contains("declares"),
        "{s}"
    );
    // Sum of declared sizes also trips the ceiling.
    let t = check_entry_budget_with("a", 600, 0, ceiling).unwrap();
    assert!(check_entry_budget_with("b", 600, t, ceiling).is_err());
}

#[test]
fn dry_run_does_not_leave_locust_dir_on_clean_game() {
    let game = tmp_game("dry_run_clean");
    write_file(&game, "data/a.json", b"ORIG");
    let zip = game.join("p.zip");
    build_patch_zip(
        &zip,
        &[("data/a.json", b"NEW", Some(b"ORIG"))],
        "1.0.0",
        "dry-id",
    );
    let report = apply(
        &game,
        &zip,
        ApplyOptions {
            dry_run: true,
            ..Default::default()
        },
        |_| {},
    )
    .unwrap();
    assert!(report.dry_run);
    assert_eq!(
        fs::read(game.join("data").join("a.json")).unwrap(),
        b"ORIG"
    );
    assert!(
        !game.join(".locust").exists(),
        "dry-run must not leave .locust/ on a previously clean game"
    );
    let _ = fs::remove_dir_all(&game);
}


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

#[test]
fn r2_discards_manifest_less_backup_when_strict_clean() {
    // W1: leftover backup/ without receipt made status Unknown and blocked
    // discard even when game content was Clean at strict tier.
    let game = tmp_game("r2_discard");
    write_file(&game, "data/a.json", b"ORIG");
    let junk = game.join(".locust").join("backup").join("files");
    fs::create_dir_all(&junk).unwrap();
    fs::write(junk.join("junk.bin"), b"stale").unwrap();
    // No receipt, no journal, no backup manifest.

    let zip = game.join("p.zip");
    build_patch_zip(
        &zip,
        &[("data/a.json", b"NEW", Some(b"ORIG"))],
        "1.0.0",
        "id-r2",
    );
    let report = apply(&game, &zip, ApplyOptions::default(), |_| {}).unwrap();
    assert_eq!(report.replaced, 1);
    assert_eq!(fs::read(game.join("data").join("a.json")).unwrap(), b"NEW");
    // Junk was discarded and a real backup commit marker exists.
    assert!(game.join(".locust").join("backup").join("manifest.json").is_file());
    assert!(!junk.join("junk.bin").exists());
    let _ = fs::remove_dir_all(&game);
}

#[test]
fn restore_rejects_path_escape_in_backup_manifest() {
    let game = tmp_game("w5_escape");
    write_file(&game, "data/a.json", b"ORIG");
    let zip = game.join("p.zip");
    build_patch_zip(
        &zip,
        &[("data/a.json", b"NEW", Some(b"ORIG"))],
        "1.0.0",
        "id-w5",
    );
    apply(&game, &zip, ApplyOptions::default(), |_| {}).unwrap();

    // Tamper backup manifest with a traversal path.
    let mpath = game.join(".locust").join("backup").join("manifest.json");
    let mut m: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&mpath).unwrap()).unwrap();
    m["files"] = serde_json::json!([{
        "path": "../escape.txt",
        "sha256": "00",
        "size": 0
    }]);
    fs::write(&mpath, serde_json::to_string_pretty(&m).unwrap()).unwrap();

    let err = rollback(&game, RollbackOptions::default()).unwrap_err();
    assert!(
        matches!(err, LocustError::PatchUnsafeEntry(_))
            || matches!(err, LocustError::PatchBackupIncomplete(_))
            || matches!(err, LocustError::PatchError(_)),
        "must refuse path escape, got {err:?}"
    );
    let _ = fs::remove_dir_all(&game);
}

#[test]
fn identity_patch_on_pristine_game_is_clean_not_unknown() {
    // When translation == source, original_sha256 == patched_sha256. A pristine
    // game matching that hash must verify Clean so apply can proceed (Unity
    // equal-length inject case), not Unknown.
    let game = tmp_game("identity_clean");
    write_file(&game, "data/a.json", b"SAME_BYTES");
    let zip = game.join("p.zip");
    build_patch_zip(
        &zip,
        &[("data/a.json", b"SAME_BYTES", Some(b"SAME_BYTES"))],
        "1.0.0",
        "id-id",
    );
    let r = verify(&game, &zip).unwrap();
    assert_eq!(
        r.outcome,
        VerificationOutcome::Clean,
        "identity patch on pristine must be Clean, got {:?}",
        r.outcome
    );
    let report = apply(&game, &zip, ApplyOptions::default(), |_| {}).unwrap();
    assert_eq!(report.replaced, 1);
    let _ = fs::remove_dir_all(&game);
}

