//! Synthetic standalone-PCK lifecycle tests through the shared public core APIs.
use std::collections::HashMap;
use std::fs;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use locust_core::backup::BackupManager;
use locust_core::database::{
    paths_identical, sha256_hex, Database, InjectionRecording, RecordedBackup,
};
use locust_core::error::{LocustError, Result};
use locust_core::extraction::{
    inject_direct, DirectInjectReport, FormatPlugin, FormatRegistry, MultiLangInjector,
    MultiLangReport,
};
use locust_core::models::{OutputMode, StringEntry, StringStatus};
use locust_core::patch::{self, ApplyOptions, PackOptions, RollbackOptions, VerificationOutcome};
use locust_formats::{godot_dialogic::GodotDialogicPlugin, godot_pck};

fn pack(members: &[(&str, &[u8])]) -> Vec<u8> {
    let mut header = vec![0; 100];
    header[..4].copy_from_slice(b"GDPC");
    header[4..8].copy_from_slice(&2u32.to_le_bytes());
    header[8..12].copy_from_slice(&4u32.to_le_bytes());
    let mut source = Cursor::new(header);
    let index = godot_pck::read_index(&mut source).unwrap();
    let mut output = Vec::new();
    godot_pck::rebuild(&mut source, &index, &[], members, &mut output).unwrap();
    output
}

fn pristine() -> Vec<u8> {
    pack(&[
        (
            "res://main.dtl",
            b"hero: Hello [b]{player}[/b]! #id:greet\n- Yes\n",
        ),
        ("res://hero.dch", b"{}"),
        ("res://raw.bin", &[0, 255, 17]),
        ("res://other.dtl", b"Other words\n"),
    ])
}

fn members(bytes: &[u8]) -> Vec<(String, Vec<u8>)> {
    let mut source = Cursor::new(bytes);
    let index = godot_pck::read_index(&mut source).unwrap();
    index
        .entries
        .iter()
        .map(|e| {
            (
                e.path.clone(),
                godot_pck::read_member(&mut source, e, 4096).unwrap(),
            )
        })
        .collect()
}

struct Fixture {
    temp: tempfile::TempDir,
    game: PathBuf,
    original: Vec<u8>,
    rows: Vec<StringEntry>,
    db: Arc<Database>,
    registry: Arc<FormatRegistry>,
    store: Arc<BackupManager>,
}

impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("shared");
        fs::create_dir(&root).unwrap();
        let game = root.join("game.pck");
        let original = pristine();
        fs::write(&game, &original).unwrap();
        fs::write(root.join("sibling.exe"), b"foreign executable").unwrap();
        fs::write(root.join("sibling.pck"), b"foreign pack").unwrap();
        let game = game.canonicalize().unwrap();
        let rows = GodotDialogicPlugin.extract(&game).unwrap();
        let mut registry = FormatRegistry::new();
        registry.register(Box::new(GodotDialogicPlugin));
        let store = Arc::new(BackupManager::new(temp.path().join("backups")));
        Self {
            temp,
            game,
            original,
            rows,
            db: Arc::new(Database::open_in_memory().unwrap()),
            registry: Arc::new(registry),
            store,
        }
    }

    fn translate(&mut self, values: &[Option<&str>]) {
        assert_eq!(values.len(), self.rows.len());
        for (row, value) in self.rows.iter_mut().zip(values) {
            row.translation = value.map(str::to_owned);
            row.status = if value.is_some() {
                StringStatus::Translated
            } else {
                StringStatus::Pending
            };
        }
        self.db.save_entries(&self.rows).unwrap();
    }

    fn direct(&self) -> Result<DirectInjectReport> {
        inject_direct(
            &self.registry,
            &self.db,
            &self.store,
            &self.game,
            "godot-dialogic",
            &["es".into()],
        )
    }

    fn recording(&self) -> InjectionRecording {
        self.db.get_injection(Some("es")).unwrap().unwrap()
    }

    fn record_with(&self, backup: Option<&RecordedBackup>, files: &[PathBuf]) {
        self.db
            .record_injection_with_backup(Some("es"), self.game.parent().unwrap(), files, backup)
            .unwrap();
    }

    fn options(&self, name: &str) -> PackOptions {
        PackOptions {
            game_path: self.game.clone(),
            lang: Some("es".into()),
            output: self.temp.path().join(name),
            pristine: None,
            engine: Some("godot-dialogic".into()),
            project: self.temp.path().join("project.db"),
            require_pristine: true,
            game: None,
        }
    }

    fn pack(&self, name: &str) -> Result<patch::PackReport> {
        patch::pack_with_pristine_backup(&self.db, self.options(name), &self.store, None, true)
    }

    async fn replace(&self, output: &Path) -> Result<MultiLangReport> {
        let injector =
            MultiLangInjector::new(self.registry.clone(), self.db.clone(), self.store.clone());
        let (tx, _rx) = tokio::sync::mpsc::channel(100);
        injector
            .inject(
                &self.game,
                "godot-dialogic",
                OutputMode::Replace,
                vec!["es".into()],
                Some(output.to_path_buf()),
                tx,
            )
            .await
    }

    fn siblings_unchanged(&self, root: &Path) {
        assert_eq!(
            fs::read(root.join("sibling.exe")).unwrap(),
            b"foreign executable"
        );
        assert_eq!(fs::read(root.join("sibling.pck")).unwrap(), b"foreign pack");
    }
}

#[test]
fn direct_abc_without_reextraction_partial_and_noop_keep_outputs_and_original_backup() {
    let mut f = Fixture::new();
    f.translate(&[
        Some("A [b]{player}[/b]!"),
        Some("Sí"),
        Some("Otros traducidos"),
    ]);
    let first = f.direct().unwrap();
    assert_eq!(first.strings_written, 3);
    let provenance = f.recording().pristine_backup.unwrap();
    let payload = Path::new(first.backup_path.as_ref().unwrap()).join("payload/file");
    assert_eq!(fs::read(&payload).unwrap(), f.original);
    for value in ["B mucho más largo [b]{player}[/b]!", "C [b]{player}[/b]!"] {
        f.translate(&[Some(value), None, None]);
        let report = f.direct().unwrap();
        assert_eq!(report.strings_written, 1, "{report:?}");
        assert_eq!(report.files_written.len(), 1);
        assert!(
            paths_identical(&report.files_written[0], &f.game),
            "{report:?}"
        );
        let output = members(&fs::read(&f.game).unwrap())
            .into_iter()
            .collect::<HashMap<_, _>>();
        assert_eq!(
            output["res://main.dtl"],
            format!("hero: {value} #id:greet\n- Sí\n").as_bytes()
        );
        assert_eq!(output["res://other.dtl"], b"Otros traducidos\n");
        assert_eq!(output["res://raw.bin"], [0, 255, 17]);
        assert_eq!(f.recording().pristine_backup, Some(provenance.clone()));
    }
    let before = fs::read(&f.game).unwrap();
    let recording = f.recording();
    let backups = f.store.list_backups().unwrap().len();
    for no_rows in [false, true] {
        if no_rows {
            f.translate(&[None, None, None]);
        }
        let report = f.direct().unwrap();
        assert_eq!(report.strings_written, 0);
        assert!(report.files_written.is_empty());
        assert_eq!(fs::read(&f.game).unwrap(), before);
        assert_eq!(f.recording(), recording);
        assert_eq!(f.store.list_backups().unwrap().len(), backups);
        assert_eq!(fs::read(&payload).unwrap(), f.original);
    }
    f.siblings_unchanged(f.game.parent().unwrap());
}

#[test]
fn direct_refuses_original_and_current_same_length_grow_shrink_or_missing_drift() {
    for original in [false, true] {
        for change in ["same", "grow", "shrink", "missing"] {
            let mut f = Fixture::new();
            f.translate(&[Some("A [b]{player}[/b]!"), None, None]);
            let first = f.direct().unwrap();
            let recording = f.recording();
            let payload = Path::new(first.backup_path.as_ref().unwrap()).join("payload/file");
            let path = if original { &payload } else { &f.game };
            let mut bytes = fs::read(path).unwrap();
            match change {
                "same" => bytes[32] ^= 1,
                "grow" => bytes.push(1),
                "shrink" => {
                    bytes.pop();
                }
                "missing" => {}
                _ => unreachable!(),
            }
            if change == "missing" {
                fs::remove_file(path).unwrap();
            } else {
                fs::write(path, &bytes).unwrap();
            }
            let current = fs::read(&f.game).ok();
            let backups = f.store.list_backups().unwrap().len();
            f.translate(&[Some("B [b]{player}[/b]!"), None, None]);
            assert!(f.direct().is_err(), "original={original} {change}");
            assert_eq!(fs::read(&f.game).ok(), current);
            assert_eq!(f.recording(), recording);
            assert_eq!(f.store.list_backups().unwrap().len(), backups);
            f.siblings_unchanged(f.game.parent().unwrap());
        }
    }
}

#[test]
fn direct_refuses_authenticated_but_incompatible_archive_or_codec_and_bad_row_md5() {
    for change in [
        "member-set",
        "missing-member",
        "order",
        "comment",
        "control",
        "event",
        "speaker",
        "id",
        "authority",
        "row-md5",
    ] {
        let mut f = Fixture::new();
        f.translate(&[Some("A [b]{player}[/b]!"), None, None]);
        f.direct().unwrap();
        let provenance = f.recording().pristine_backup.unwrap();
        let mut items = members(&fs::read(&f.game).unwrap());
        match change {
            "member-set" => items.push(("res://new.bin".into(), b"added".to_vec())),
            "missing-member" => {
                items.remove(2);
            }
            "order" => items.swap(0, 1),
            "comment" => items[0]
                .1
                .extend_from_slice(b"# different command/comment\n"),
            "control" => items[0].1 = b"hero: A [i]{player}[/i]! #id:greet\n- Yes\n".to_vec(),
            "event" => items[0].1.extend_from_slice(b"New event\n"),
            "speaker" => items[0].1 = b"A [b]{player}[/b]! #id:greet\n- Yes\n".to_vec(),
            "id" => items[0].1 = b"hero: A [b]{player}[/b]! #id:other\n- Yes\n".to_vec(),
            "authority" => items[1].1 = br#"{"display_name":"Different"}"#.to_vec(),
            "row-md5" => {
                f.rows[0].metadata.get_mut("godot_dialogic").unwrap()["original_md5"] =
                    serde_json::json!("00000000000000000000000000000000")
            }
            _ => unreachable!(),
        }
        let refs = items
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_slice()))
            .collect::<Vec<_>>();
        fs::write(&f.game, pack(&refs)).unwrap();
        // Model a verified previous output with incompatible structure. Merely
        // accepting the recording's SHA-256 cannot prove codec compatibility.
        f.record_with(Some(&provenance), &[f.game.clone()]);
        let recording = f.recording();
        let before = fs::read(&f.game).unwrap();
        f.translate(&[Some("B [b]{player}[/b]!"), None, None]);
        assert!(f.direct().is_err(), "{change}");
        assert_eq!(fs::read(&f.game).unwrap(), before);
        assert_eq!(f.recording(), recording);
    }
}

#[test]
fn direct_refuses_missing_provenance_wrong_store_and_wrong_pristine_with_matching_visible_text() {
    for change in ["no-provenance", "missing-store", "wrong-pristine"] {
        let mut f = Fixture::new();
        f.translate(&[Some("A [b]{player}[/b]!"), None, None]);
        f.direct().unwrap();
        let mut provenance = f.recording().pristine_backup.unwrap();
        let before = fs::read(&f.game).unwrap();
        match change {
            "missing-store" => {
                provenance.storage_root = Some(f.temp.path().join("unrelated-store"))
            }
            "wrong-pristine" => {
                // This is a valid pristine store bound to the exact source
                // path, but the member bytes differ although all text matches.
                let mut items = members(&f.original);
                items[0].1.extend_from_slice(b"# other baseline\n");
                let refs = items
                    .iter()
                    .map(|(k, v)| (k.as_str(), v.as_slice()))
                    .collect::<Vec<_>>();
                fs::write(&f.game, pack(&refs)).unwrap();
                let wrong_store = BackupManager::new(f.temp.path().join("other-store"));
                let backup = wrong_store.create_backup(&f.game).unwrap();
                provenance = RecordedBackup {
                    id: backup.id,
                    source_path: f.game.clone(),
                    storage_root: Some(wrong_store.root().to_path_buf()),
                };
                fs::write(&f.game, &before).unwrap();
            }
            _ => {}
        }
        f.record_with(
            (change != "no-provenance").then_some(&provenance),
            &[f.game.clone()],
        );
        let recording = f.recording();
        f.translate(&[Some("B [b]{player}[/b]!"), None, None]);
        assert!(f.direct().is_err(), "{change}");
        assert_eq!(fs::read(&f.game).unwrap(), before);
        assert_eq!(f.recording(), recording);
    }
}

#[test]
fn selected_file_refuses_sibling_recording_before_hashing_a_missing_sibling() {
    let mut f = Fixture::new();
    f.translate(&[Some("A [b]{player}[/b]!"), None, None]);
    f.direct().unwrap();
    let provenance = f.recording().pristine_backup.unwrap();
    let sibling = f.game.parent().unwrap().join("sibling.pck");
    f.record_with(Some(&provenance), &[f.game.clone(), sibling.clone()]);
    fs::remove_file(sibling).unwrap();
    let before = fs::read(&f.game).unwrap();
    let error = f.direct().unwrap_err().to_string();
    assert!(error.contains("before hashing a sibling"), "{error}");
    assert_eq!(fs::read(&f.game).unwrap(), before);
}

#[test]
fn two_selected_pcks_in_shared_parent_have_independent_direct_histories_and_backups() {
    let mut f = Fixture::new();
    let second = f.game.parent().unwrap().join("second.pck");
    let second_original = pack(&[
        (
            "res://main.dtl",
            b"hero: Second game [b]{player}[/b]! #id:greet\n- Yes\n",
        ),
        ("res://hero.dch", b"{}"),
        ("res://raw.bin", b"different game assets"),
    ]);
    fs::write(&second, &second_original).unwrap();
    let db = Database::open_in_memory().unwrap();
    let mut rows = GodotDialogicPlugin.extract(&second).unwrap();
    rows[0].translation = Some("Segundo [b]{player}[/b]!".into());
    db.save_entries(&rows).unwrap();
    f.translate(&[Some("Primero [b]{player}[/b]!"), None, None]);
    let first = f.direct().unwrap();
    let first_bytes = fs::read(&f.game).unwrap();
    let report = inject_direct(
        &f.registry,
        &db,
        &f.store,
        &second,
        "godot-dialogic",
        &["es".into()],
    )
    .unwrap();
    assert_eq!(fs::read(&f.game).unwrap(), first_bytes);
    assert_ne!(first.pristine_backup_id, report.pristine_backup_id);
    assert_eq!(
        fs::read(Path::new(report.backup_path.as_ref().unwrap()).join("payload/file")).unwrap(),
        second_original
    );
    let second_bytes = fs::read(&second).unwrap();
    f.translate(&[Some("Revisión primero [b]{player}[/b]!"), None, None]);
    f.direct().unwrap();
    assert_eq!(fs::read(&second).unwrap(), second_bytes);
    rows[0].translation = Some("Revisión segundo [b]{player}[/b]!".into());
    db.save_entries(&rows).unwrap();
    inject_direct(
        &f.registry,
        &db,
        &f.store,
        &second,
        "godot-dialogic",
        &["es".into()],
    )
    .unwrap();
    assert_eq!(f.recording().files[0].rel, "game.pck");
    assert_eq!(
        db.get_injection(Some("es")).unwrap().unwrap().files[0].rel,
        "second.pck"
    );
    f.siblings_unchanged(f.game.parent().unwrap());
}

#[tokio::test]
async fn replace_before_direct_succeeds_after_direct_refuses_before_copy_and_after_restore_succeeds(
) {
    let mut f = Fixture::new();
    f.translate(&[Some("Traducido [b]{player}[/b]!"), None, None]);
    let output = f.temp.path().join("replace-before");
    let report = f.replace(&output).await.unwrap();
    assert!(report.languages_failed.is_empty(), "{report:?}");
    assert_eq!(fs::read(&f.game).unwrap(), f.original);
    let destination = report.injected_roots["es"].canonicalize().unwrap();
    let recording = f.recording();
    assert!(
        paths_identical(&recording.root, &destination),
        "{recording:?}"
    );
    assert_eq!(recording.files.len(), 1);
    assert_eq!(recording.files[0].rel, "game.pck");
    assert_eq!(
        recording.files[0].hash,
        sha256_hex(&fs::read(destination.join("game.pck")).unwrap())
    );
    assert!(!destination.join("sibling.exe").exists());
    let mut opts = f.options("replace.zip");
    opts.game_path = destination;
    assert_eq!(
        patch::pack_with_pristine_backup(&f.db, opts, &f.store, None, true)
            .unwrap()
            .tier,
        "strict"
    );
    let mut archive =
        zip::ZipArchive::new(fs::File::open(f.temp.path().join("replace.zip")).unwrap()).unwrap();
    let manifest: patch::PatchManifest =
        serde_json::from_reader(archive.by_name("locust-patch.json").unwrap()).unwrap();
    assert_eq!(manifest.files.len(), 1);
    assert_eq!(
        manifest.files[0].original_sha256,
        Some(sha256_hex(&f.original))
    );
    assert!(!manifest.files[0].added);
    drop(archive);
    let direct = f.direct().unwrap();
    let blocked = f.temp.path().join("replace-after");
    let backups = f.store.list_backups().unwrap().len();
    let error = f.replace(&blocked).await.unwrap_err().to_string();
    assert!(
        error.contains("Replace copy refused") && error.contains("still owned by a Direct"),
        "{error}"
    );
    assert!(!blocked.exists());
    assert_eq!(f.store.list_backups().unwrap().len(), backups);
    f.store.restore(&direct.backup_id).unwrap();
    assert_eq!(fs::read(&f.game).unwrap(), f.original);
    let report = f.replace(&blocked).await.unwrap();
    assert!(report.languages_failed.is_empty(), "{report:?}");
    f.siblings_unchanged(f.game.parent().unwrap());
}

#[test]
fn strict_recorded_pck_pack_apply_already_applied_rollback_and_reapply() {
    let mut f = Fixture::new();
    f.translate(&[Some("Traducido [b]{player}[/b]!"), None, None]);
    let direct = f.direct().unwrap();
    let translated = fs::read(&f.game).unwrap();
    let recording = f.recording();
    assert_eq!(direct.files_written.len(), 1);
    assert!(
        paths_identical(&direct.files_written[0], &f.game),
        "{direct:?}"
    );
    assert!(
        paths_identical(&recording.root, f.game.parent().unwrap()),
        "{recording:?}"
    );
    assert_eq!(recording.files.len(), 1);
    assert_eq!(recording.files[0].rel, "game.pck");
    assert_eq!(recording.files[0].hash, sha256_hex(&translated));
    assert_eq!(recording.files[0].size, translated.len() as u64);
    let report = f.pack("strict.zip").unwrap();
    assert_eq!(report.tier, "strict");
    assert_eq!(report.files_packed, 1);
    let zip = f.temp.path().join("strict.zip");
    let mut archive = zip::ZipArchive::new(fs::File::open(&zip).unwrap()).unwrap();
    let mut entry_names: Vec<_> = archive.file_names().collect();
    entry_names.sort_unstable();
    assert_eq!(entry_names, ["README.txt", "game.pck", "locust-patch.json"]);
    let manifest: patch::PatchManifest =
        serde_json::from_reader(archive.by_name("locust-patch.json").unwrap()).unwrap();
    assert_eq!(manifest.files.len(), 1);
    assert_eq!(manifest.files[0].path, "game.pck");
    assert_eq!(
        manifest.files[0].original_sha256,
        Some(sha256_hex(&f.original))
    );
    assert!(!manifest.files[0].added);
    drop(archive);

    // Apply targets the directory holding the PCK, as required by GameLock and
    // the CLI. A file root cannot be used as an implicit parent-directory grant.
    let target = f.temp.path().join("clean-shared-parent");
    fs::create_dir(&target).unwrap();
    fs::write(target.join("game.pck"), &f.original).unwrap();
    fs::write(target.join("sibling.exe"), b"foreign executable").unwrap();
    fs::write(target.join("sibling.pck"), b"foreign pack").unwrap();
    let error = patch::apply(
        &target.join("game.pck"),
        &zip,
        ApplyOptions::default(),
        |_| {},
    )
    .unwrap_err()
    .to_string();
    assert!(error.contains("game root is not a directory"), "{error}");
    let verified = patch::verify(&target, &zip).unwrap();
    assert_eq!(verified.outcome, VerificationOutcome::Clean);
    assert_eq!(
        verified.tier,
        Some(patch::manifest::VerificationTier::Strict)
    );
    assert_eq!(verified.replaced, vec!["game.pck"]);
    assert!(verified.added.is_empty());
    let applied = patch::apply(&target, &zip, ApplyOptions::default(), |_| {}).unwrap();
    assert_eq!((applied.replaced, applied.added), (1, 0));
    assert_eq!(fs::read(target.join("game.pck")).unwrap(), translated);
    assert_eq!(
        patch::verify(&target, &zip).unwrap().outcome,
        VerificationOutcome::AlreadyApplied
    );
    f.siblings_unchanged(&target);
    let rolled = patch::rollback(&target, RollbackOptions::default()).unwrap();
    assert_eq!(rolled.restored, 1);
    assert_eq!(fs::read(target.join("game.pck")).unwrap(), f.original);
    f.siblings_unchanged(&target);
    assert_eq!(
        patch::verify(&target, &zip).unwrap().outcome,
        VerificationOutcome::Clean
    );
    patch::apply(&target, &zip, ApplyOptions::default(), |_| {}).unwrap();
    assert_eq!(fs::read(target.join("game.pck")).unwrap(), translated);
    f.siblings_unchanged(&target);
}

#[test]
fn strict_pack_refuses_payload_drift_missing_baseline_and_wrong_game_apply() {
    let mut f = Fixture::new();
    f.translate(&[Some("Traducido [b]{player}[/b]!"), None, None]);
    assert_eq!(f.db.count_translated_entries().unwrap(), 1);
    f.direct().unwrap();
    f.pack("strict.zip").unwrap();
    let translated = fs::read(&f.game).unwrap();
    // A recorded single-file backup requires its live source to exist. Use an
    // explicit pristine tree for these payload checks so a missing PCK reaches
    // the recorded-file check rather than failing during backup resolution.
    let pristine = f.temp.path().join("pristine");
    fs::create_dir(&pristine).unwrap();
    fs::write(pristine.join("game.pck"), &f.original).unwrap();
    for change in ["same", "grow", "shrink", "missing"] {
        let mut bytes = translated.clone();
        match change {
            "same" => bytes[32] ^= 1,
            "grow" => bytes.push(1),
            "shrink" => {
                bytes.pop();
            }
            "missing" => {}
            _ => unreachable!(),
        }
        if change == "missing" {
            fs::remove_file(&f.game).unwrap();
        } else {
            fs::write(&f.game, bytes).unwrap();
        }
        let output = f.temp.path().join("refused.zip");
        fs::write(&output, b"keep existing output").unwrap();
        let mut opts = f.options("refused.zip");
        // Keep the recorded directory selected even when its PCK is missing,
        // so the refusal checks payload drift rather than a root mismatch.
        opts.game_path = f.game.parent().unwrap().to_path_buf();
        opts.pristine = Some(pristine.clone());
        let error = patch::pack_with_pristine_backup(&f.db, opts, &f.store, None, true)
            .unwrap_err()
            .to_string();
        let reason = if change == "missing" {
            "missing from disk"
        } else {
            "changed on disk since injection recorded them"
        };
        assert!(
            error.contains("1 of 1 recorded file(s) no longer match")
                && error.contains(reason)
                && error.contains("game.pck"),
            "{change}: {error}"
        );
        assert_eq!(fs::read(&output).unwrap(), b"keep existing output");
        fs::write(&f.game, &translated).unwrap();
    }
    let wrong = f.temp.path().join("wrong-game");
    fs::create_dir(&wrong).unwrap();
    let zip = f.temp.path().join("strict.zip");
    for bytes in [Some(b"different game".as_slice()), None] {
        let path = wrong.join("game.pck");
        if let Some(bytes) = bytes {
            fs::write(&path, bytes).unwrap();
        } else {
            fs::remove_file(&path).unwrap();
        }
        let outcome = patch::verify(&wrong, &zip).unwrap().outcome;
        let VerificationOutcome::Mismatch(mismatches) = outcome else {
            panic!("expected wrong-game mismatch, got {outcome:?}");
        };
        assert_eq!(mismatches.len(), 1);
        assert_eq!(mismatches[0].path, "game.pck");
        assert!(mismatches[0].expected.contains(&sha256_hex(&f.original)));
        assert_eq!(
            mismatches[0].found,
            bytes.map(sha256_hex).unwrap_or_else(|| "absent".into())
        );
        let error = patch::apply(&wrong, &zip, ApplyOptions::default(), |_| {}).unwrap_err();
        let LocustError::PatchVerificationFailed(detail) = error else {
            panic!("expected wrong-game verification refusal, got {error:?}");
        };
        assert_eq!(
            detail,
            format!(
                "game.pck: expected {}, found {}",
                mismatches[0].expected, mismatches[0].found
            )
        );
        assert_eq!(fs::read(path).ok().as_deref(), bytes);
    }
    f.record_with(None, &[f.game.clone()]);
    let error = f.pack("no-baseline.zip").unwrap_err().to_string();
    assert!(error.contains("pristine hashes required"), "{error}");
    assert!(!f.temp.path().join("no-baseline.zip").exists());
}
