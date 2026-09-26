use locust_core::database::sha256_hex;
use locust_core::patch::{
    apply, rollback, verify, ApplyOptions, PatchFileEntry, PatchManifest, PatchStore,
    RollbackOptions, VerificationOutcome,
};
use std::fs;
use std::io::Write;
use std::path::Path;

fn manifest() -> PatchManifest {
    PatchManifest {
        schema_version: 1,
        patch_id: "font-safe".into(),
        game_name: "fixture".into(),
        engine: "html".into(),
        language: "es".into(),
        patch_version: "1.0.0".into(),
        generator_version: "0.1.0".into(),
        created_at: "now".into(),
        files: vec![PatchFileEntry {
            path: "fonts/game.ttf".into(),
            size: 4,
            patched_sha256: sha256_hex(b"FONT"),
            original_sha256: Some(sha256_hex(b"orig")),
        }],
    }
}

fn game() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir(dir.path().join("fonts")).unwrap();
    fs::write(dir.path().join("fonts/game.ttf"), b"orig").unwrap();
    dir
}
fn assert_no_mutation(root: &Path, zip: &Path) {
    assert!(verify(root, zip).is_err());
    for force in [false, true] {
        assert!(apply(
            root,
            zip,
            ApplyOptions {
                force,
                ..Default::default()
            },
            |_| {}
        )
        .is_err());
        assert_eq!(fs::read(root.join("fonts/game.ttf")).unwrap(), b"orig");
        assert!(!root.join(".locust").exists());
        assert!(fs::read_dir(root).unwrap().all(|e| !e
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".locust-stage-")));
    }
}

#[test]
fn manifest_and_readme_aliases_rejected_in_verify_and_force_apply() {
    for alias in [
        "locust-patch.json.",
        "locust-patch.json ",
        "LOCUST-PATCH.JSON",
        "./locust-patch.json",
        r".\locust-patch.json",
    ] {
        let dir = game();
        let zip = dir.path().join("bad.zip");
        let json = serde_json::to_vec(&manifest()).unwrap();
        write_zip(
            &zip,
            &[
                ("locust-patch.json", json.clone()),
                ("fonts/game.ttf", b"FONT".to_vec()),
                (alias, json),
            ],
        );
        assert_no_mutation(dir.path(), &zip);
    }
    for alias in ["README.TXT", "readme.txt.", "./readme.txt", r".\README.txt"] {
        let dir = game();
        let zip = dir.path().join("bad.zip");
        write_zip(
            &zip,
            &[
                (
                    "locust-patch.json",
                    serde_json::to_vec(&manifest()).unwrap(),
                ),
                ("fonts/game.ttf", b"FONT".to_vec()),
                ("readme.txt", b"one".to_vec()),
                (alias, b"two".to_vec()),
            ],
        );
        assert_no_mutation(dir.path(), &zip);
    }
}

#[test]
fn exact_duplicate_central_names_not_hidden_by_zip_indexmap() {
    for name in ["locust-patch.json", "README.txt", "fonts/game.ttf"] {
        let dir = game();
        let zip = dir.path().join("bad.zip");
        let alternate = format!("{}X", &name[..name.len() - 1]);
        let mut entries = vec![
            (
                "locust-patch.json",
                serde_json::to_vec(&manifest()).unwrap(),
            ),
            ("fonts/game.ttf", b"FONT".to_vec()),
        ];
        if name == "README.txt" {
            entries.push((name, b"readme".to_vec()));
        }
        let bytes = entries
            .iter()
            .find(|(path, _)| *path == name)
            .unwrap()
            .1
            .clone();
        entries.push((&alternate, bytes));
        write_zip(&zip, &entries);
        // Rewrite both local and central names, preserving all offsets/lengths.
        let mut raw = fs::read(&zip).unwrap();
        let mut patched = 0;
        for pos in 0..=raw.len() - alternate.len() {
            if raw[pos..pos + alternate.len()] == *alternate.as_bytes() {
                raw[pos..pos + name.len()].copy_from_slice(name.as_bytes());
                patched += 1;
            }
        }
        assert_eq!(patched, 2);
        fs::write(&zip, raw).unwrap();
        assert_no_mutation(dir.path(), &zip);
    }
}

#[test]
fn extra_payloads_duplicate_manifest_targets_and_wrong_sizes_rejected() {
    for mode in ["unlisted", "duplicate", "wrong-size", "wrong-schema"] {
        let dir = game();
        let zip = dir.path().join("bad.zip");
        let mut m = manifest();
        if mode == "duplicate" {
            let mut file = m.files[0].clone();
            file.path = r"fonts\.\game.ttf".into();
            m.files.push(file);
        }
        if mode == "wrong-size" {
            m.files[0].size += 1;
        }
        if mode == "wrong-schema" {
            m.schema_version = 999;
        }
        let mut entries = vec![
            ("locust-patch.json", serde_json::to_vec(&m).unwrap()),
            ("fonts/game.ttf", b"FONT".to_vec()),
        ];
        if mode == "unlisted" {
            entries.push(("other-user-data.txt", b"malicious".to_vec()));
        }
        write_zip(&zip, &entries);
        assert_no_mutation(dir.path(), &zip);
    }
}

#[test]
fn corrupted_late_member_rejected_before_backup_or_asset_mutation() {
    let dir = game();
    let zip = dir.path().join("bad.zip");
    let mut m = manifest();
    m.files.push(PatchFileEntry {
        path: "later.txt".into(),
        size: 12,
        patched_sha256: sha256_hex(b"LATERCONTENT"),
        original_sha256: None,
    });
    write_zip(
        &zip,
        &[
            ("locust-patch.json", serde_json::to_vec(&m).unwrap()),
            ("fonts/game.ttf", b"FONT".to_vec()),
            ("later.txt", b"LATERCONTENT".to_vec()),
        ],
    );
    let mut raw = fs::read(&zip).unwrap();
    let at = raw.windows(12).position(|w| w == b"LATERCONTENT").unwrap();
    raw[at] ^= 1;
    fs::write(&zip, raw).unwrap();
    assert_no_mutation(dir.path(), &zip);
    assert!(!dir.path().join("later.txt").exists());
}

#[test]
fn invalid_switch_cannot_rollback_the_installed_patch() {
    let dir = game();
    let good = dir.path().join("good.zip");
    let m = manifest();
    write_zip(
        &good,
        &[
            ("locust-patch.json", serde_json::to_vec(&m).unwrap()),
            ("fonts/game.ttf", b"FONT".to_vec()),
        ],
    );
    apply(dir.path(), &good, ApplyOptions::default(), |_| {}).unwrap();
    let receipt = fs::read(dir.path().join(".locust/receipt.json")).unwrap();
    let mut newer = m.clone();
    newer.patch_version = "2.0.0".into();
    let mut altered = newer.clone();
    altered.patch_id = "different".into();
    let bad = dir.path().join("bad.zip");
    write_zip(
        &bad,
        &[
            ("locust-patch.json", serde_json::to_vec(&newer).unwrap()),
            ("fonts/game.ttf", b"FONT".to_vec()),
            ("locust-patch.json.", serde_json::to_vec(&altered).unwrap()),
        ],
    );
    for force in [false, true] {
        assert!(apply(
            dir.path(),
            &bad,
            ApplyOptions {
                force,
                ..Default::default()
            },
            |_| {}
        )
        .is_err());
        assert_eq!(
            fs::read(dir.path().join("fonts/game.ttf")).unwrap(),
            b"FONT"
        );
        assert_eq!(
            fs::read(dir.path().join(".locust/receipt.json")).unwrap(),
            receipt
        );
    }
    rollback(dir.path(), RollbackOptions::default()).unwrap();
    assert_eq!(
        fs::read(dir.path().join("fonts/game.ttf")).unwrap(),
        b"orig"
    );
}

#[test]
fn uppercase_metadata_and_canonical_manifest_paths_share_one_plan() {
    let dir = game();
    let zip = dir.path().join("good.zip");
    let mut m = manifest();
    m.files[0].path = r"fonts\.\game.ttf".into();
    write_zip(
        &zip,
        &[
            (r".\LOCUST-PATCH.JSON", serde_json::to_vec(&m).unwrap()),
            ("fonts/game.ttf", b"FONT".to_vec()),
        ],
    );
    assert_eq!(
        verify(dir.path(), &zip).unwrap().outcome,
        VerificationOutcome::Clean
    );
    apply(dir.path(), &zip, ApplyOptions::default(), |_| {}).unwrap();
    assert_eq!(
        PatchStore::new(dir.path())
            .read_receipt()
            .unwrap()
            .unwrap()
            .replaced[0]
            .path,
        "fonts/game.ttf"
    );
    rollback(dir.path(), RollbackOptions::default()).unwrap();
    assert_eq!(
        fs::read(dir.path().join("fonts/game.ttf")).unwrap(),
        b"orig"
    );
}

#[test]
fn source_zip_replacement_after_validation_cannot_change_applied_bytes() {
    let dir = game();
    let zip = dir.path().join("good.zip");
    write_zip(
        &zip,
        &[
            (
                "locust-patch.json",
                serde_json::to_vec(&manifest()).unwrap(),
            ),
            ("fonts/game.ttf", b"FONT".to_vec()),
        ],
    );
    apply(dir.path(), &zip, ApplyOptions::default(), |_| {
        fs::write(&zip, b"replaced source archive").unwrap();
    })
    .unwrap();
    assert_eq!(
        fs::read(dir.path().join("fonts/game.ttf")).unwrap(),
        b"FONT"
    );
    rollback(dir.path(), RollbackOptions::default()).unwrap();
    assert_eq!(
        fs::read(dir.path().join("fonts/game.ttf")).unwrap(),
        b"orig"
    );
}

#[test]
fn prepared_stage_cleanup_preserves_unowned_directory_and_children() {
    let dir = game();
    let foreign = dir.path().join(".locust-stage-unrelated");
    fs::create_dir(&foreign).unwrap();
    fs::write(foreign.join("keep"), b"unowned").unwrap();
    let zip = dir.path().join("good.zip");
    write_zip(
        &zip,
        &[
            (
                "locust-patch.json",
                serde_json::to_vec(&manifest()).unwrap(),
            ),
            ("fonts/game.ttf", b"FONT".to_vec()),
        ],
    );
    let mut owned = None;
    apply(dir.path(), &zip, ApplyOptions::default(), |_| {
        let path = fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().path())
            .find(|p| {
                p.file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with(".locust-stage-")
                    && p != &foreign
            })
            .unwrap();
        fs::write(path.join("unrelated.keep"), b"concurrent user file").unwrap();
        owned = Some(path);
    })
    .unwrap();
    assert_eq!(fs::read(foreign.join("keep")).unwrap(), b"unowned");
    assert_eq!(
        fs::read(owned.unwrap().join("unrelated.keep")).unwrap(),
        b"concurrent user file"
    );
}

#[test]
fn interrupted_apply_blocks_force_and_keeps_rollback_authority() {
    let dir = game();
    let zip = dir.path().join("good.zip");
    write_zip(
        &zip,
        &[
            (
                "locust-patch.json",
                serde_json::to_vec(&manifest()).unwrap(),
            ),
            ("fonts/game.ttf", b"FONT".to_vec()),
        ],
    );
    let interrupted = std::panic::catch_unwind(|| {
        let _ = apply(dir.path(), &zip, ApplyOptions::default(), |_| {
            panic!("test interruption before write")
        });
    });
    assert!(interrupted.is_err());
    let journal = fs::read(dir.path().join(".locust/journal.json")).unwrap();
    assert!(apply(
        dir.path(),
        &zip,
        ApplyOptions {
            force: true,
            ..Default::default()
        },
        |_| {}
    )
    .is_err());
    assert_eq!(
        fs::read(dir.path().join(".locust/journal.json")).unwrap(),
        journal
    );
    rollback(dir.path(), RollbackOptions::default()).unwrap();
    assert_eq!(
        fs::read(dir.path().join("fonts/game.ttf")).unwrap(),
        b"orig"
    );
}

#[test]
fn valid_zip64_end_records_are_not_mistaken_for_duplicate_counts() {
    let dir = game();
    let zip = dir.path().join("zip64.zip");
    write_zip(
        &zip,
        &[
            (
                "locust-patch.json",
                serde_json::to_vec(&manifest()).unwrap(),
            ),
            ("fonts/game.ttf", b"FONT".to_vec()),
        ],
    );
    let mut raw = fs::read(&zip).unwrap();
    let end = raw.len() - 22;
    let mut eocd = raw.split_off(end);
    let count = u16::from_le_bytes(eocd[10..12].try_into().unwrap()) as u64;
    let size = u32::from_le_bytes(eocd[12..16].try_into().unwrap()) as u64;
    let offset = u32::from_le_bytes(eocd[16..20].try_into().unwrap()) as u64;
    raw.extend(0x06064b50u32.to_le_bytes());
    raw.extend(44u64.to_le_bytes());
    raw.extend(45u16.to_le_bytes());
    raw.extend(45u16.to_le_bytes());
    raw.extend([0u8; 8]);
    for n in [count, count, size, offset] {
        raw.extend(n.to_le_bytes());
    }
    raw.extend(0x07064b50u32.to_le_bytes());
    raw.extend(0u32.to_le_bytes());
    raw.extend((end as u64).to_le_bytes());
    raw.extend(1u32.to_le_bytes());
    eocd[8..12].fill(0xff);
    eocd[12..20].fill(0xff);
    raw.extend(eocd);
    fs::write(&zip, raw).unwrap();
    assert_eq!(
        verify(dir.path(), &zip).unwrap().outcome,
        VerificationOutcome::Clean
    );
    apply(dir.path(), &zip, ApplyOptions::default(), |_| {}).unwrap();
    rollback(dir.path(), RollbackOptions::default()).unwrap();
    assert_eq!(
        fs::read(dir.path().join("fonts/game.ttf")).unwrap(),
        b"orig"
    );
}

#[test]
fn successful_upgrade_and_forced_switch_use_staging_surviving_rollback() {
    for force in [false, true] {
        let dir = game();
        let first = dir.path().join("first.zip");
        let m = manifest();
        write_zip(
            &first,
            &[
                ("locust-patch.json", serde_json::to_vec(&m).unwrap()),
                ("fonts/game.ttf", b"FONT".to_vec()),
            ],
        );
        apply(dir.path(), &first, ApplyOptions::default(), |_| {}).unwrap();
        let mut next = m;
        next.patch_version = "2.0.0".into();
        if force {
            next.patch_id = "different-id".into();
        }
        next.files[0].patched_sha256 = sha256_hex(b"NEW!");
        let second = dir.path().join("second.zip");
        write_zip(
            &second,
            &[
                ("locust-patch.json", serde_json::to_vec(&next).unwrap()),
                ("fonts/game.ttf", b"NEW!".to_vec()),
            ],
        );
        apply(
            dir.path(),
            &second,
            ApplyOptions {
                force,
                ..Default::default()
            },
            |_| {},
        )
        .unwrap();
        assert_eq!(
            fs::read(dir.path().join("fonts/game.ttf")).unwrap(),
            b"NEW!"
        );
        rollback(dir.path(), RollbackOptions::default()).unwrap();
        assert_eq!(
            fs::read(dir.path().join("fonts/game.ttf")).unwrap(),
            b"orig"
        );
        assert!(fs::read_dir(dir.path()).unwrap().all(|e| !e
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".locust-stage-")));
    }
}

#[test]
fn wrong_pristine_baseline_upgrade_preserves_installed_patch_and_receipt() {
    let dir = game();
    let first = dir.path().join("first.zip");
    let m = manifest();
    write_zip(
        &first,
        &[
            ("locust-patch.json", serde_json::to_vec(&m).unwrap()),
            ("fonts/game.ttf", b"FONT".to_vec()),
        ],
    );
    apply(dir.path(), &first, ApplyOptions::default(), |_| {}).unwrap();
    let receipt = fs::read(dir.path().join(".locust/receipt.json")).unwrap();
    let mut next = m;
    next.patch_version = "2.0.0".into();
    next.files[0].original_sha256 = Some(sha256_hex(b"different original font"));
    let second = dir.path().join("wrong.zip");
    write_zip(
        &second,
        &[
            ("locust-patch.json", serde_json::to_vec(&next).unwrap()),
            ("fonts/game.ttf", b"FONT".to_vec()),
        ],
    );
    assert!(apply(dir.path(), &second, ApplyOptions::default(), |_| {}).is_err());
    assert_eq!(
        fs::read(dir.path().join("fonts/game.ttf")).unwrap(),
        b"FONT"
    );
    assert_eq!(
        fs::read(dir.path().join(".locust/receipt.json")).unwrap(),
        receipt
    );
}

#[test]
fn dirty_game_requires_force_and_keeps_unverified_exact_backup() {
    let dir = game();
    fs::write(dir.path().join("fonts/game.ttf"), b"user font").unwrap();
    let zip = dir.path().join("font.zip");
    write_zip(
        &zip,
        &[
            (
                "locust-patch.json",
                serde_json::to_vec(&manifest()).unwrap(),
            ),
            ("fonts/game.ttf", b"FONT".to_vec()),
        ],
    );
    assert!(apply(dir.path(), &zip, ApplyOptions::default(), |_| {}).is_err());
    assert_eq!(
        fs::read(dir.path().join("fonts/game.ttf")).unwrap(),
        b"user font"
    );
    let report = apply(
        dir.path(),
        &zip,
        ApplyOptions {
            force: true,
            ..Default::default()
        },
        |_| {},
    )
    .unwrap();
    assert_eq!(
        report.baseline,
        locust_core::patch::manifest::BackupBaseline::Unverified
    );
    rollback(dir.path(), RollbackOptions::default()).unwrap();
    assert_eq!(
        fs::read(dir.path().join("fonts/game.ttf")).unwrap(),
        b"user font"
    );
}
fn write_zip(path: &Path, entries: &[(&str, Vec<u8>)]) {
    let mut w = zip::ZipWriter::new(fs::File::create(path).unwrap());
    for (name, bytes) in entries {
        w.start_file(
            *name,
            zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Stored),
        )
        .unwrap();
        w.write_all(bytes).unwrap();
    }
    w.finish().unwrap();
}

#[test]
fn duplicate_manifest_cannot_bypass_verified_plan() {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir(dir.path().join("fonts")).unwrap();
    fs::write(dir.path().join("fonts/game.ttf"), b"orig").unwrap();
    fs::write(dir.path().join("user-settings.json"), b"USER EDITS").unwrap();
    let safe = manifest();
    let mut altered = safe.clone();
    altered.patch_id = "unverified-id".into();
    altered.files.push(PatchFileEntry {
        path: "user-settings.json".into(),
        size: 6,
        patched_sha256: sha256_hex(b"ERASED"),
        original_sha256: Some(sha256_hex(b"FACTORY SETTINGS (does not match)")),
    });
    let zip = dir.path().join("hostile.zip");
    write_zip(
        &zip,
        &[
            ("locust-patch.json", serde_json::to_vec(&safe).unwrap()),
            ("fonts/game.ttf", b"FONT".to_vec()),
            ("user-settings.json", b"ERASED".to_vec()),
            ("locust-patch.json.", serde_json::to_vec(&altered).unwrap()),
        ],
    );
    assert!(verify(dir.path(), &zip).is_err());
    assert!(apply(dir.path(), &zip, ApplyOptions::default(), |_| {}).is_err());
    assert_eq!(
        fs::read(dir.path().join("fonts/game.ttf")).unwrap(),
        b"orig"
    );
    assert_eq!(
        fs::read(dir.path().join("user-settings.json")).unwrap(),
        b"USER EDITS"
    );
    assert!(!dir.path().join(".locust").exists());
}
