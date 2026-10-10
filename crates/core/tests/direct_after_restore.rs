use locust_core::{
    backup::BackupManager,
    database::{sha256_file, sha256_hex, Database},
    error::Result,
    extraction::{
        inject_direct, DirectInjectReport, FormatPlugin, FormatRegistry, InjectionReport,
    },
    models::{StringEntry, StringStatus},
    patch::{pack_with_pristine_backup, verify, PackOptions, VerificationOutcome},
};
use std::{fs, path::Path};

struct SourceMatchWriter;

impl FormatPlugin for SourceMatchWriter {
    fn id(&self) -> &str {
        "source-match"
    }
    fn name(&self) -> &str {
        self.id()
    }
    fn supported_extensions(&self) -> &[&str] {
        &["html"]
    }
    fn extract(&self, _: &Path) -> Result<Vec<StringEntry>> {
        Ok(Vec::new())
    }
    fn inject(&self, _: &Path, entries: &[StringEntry]) -> Result<InjectionReport> {
        let mut files = Vec::new();
        for entry in entries {
            if let Some(translation) = &entry.translation {
                if fs::read(&entry.file_path)? == entry.source.as_bytes() {
                    fs::write(&entry.file_path, translation)?;
                    files.push(entry.file_path.clone());
                }
            }
        }
        Ok(InjectionReport {
            files_modified: files.len(),
            strings_written: files.len(),
            strings_skipped: entries.len() - files.len(),
            files_written: files,
            warnings: Vec::new(),
            skip_reasons: Default::default(),
        })
    }
}

struct Fixture {
    db: Database,
    manager: BackupManager,
    root: tempfile::TempDir,
}

impl Fixture {
    fn new() -> Self {
        // Host temp storage; the project database must not be backed up/restored
        // with the game or restoring it would hide the stale-recording defect.
        let root = tempfile::tempdir().unwrap();
        fs::create_dir(root.path().join("game")).unwrap();
        fs::write(root.path().join("game/one.html"), b"ONE").unwrap();
        fs::write(root.path().join("game/two.html"), b"TWO").unwrap();
        let fixture = Self {
            db: Database::open(&root.path().join("project.db")).unwrap(),
            manager: BackupManager::new(root.path().join("backups")),
            root,
        };
        fixture.translate("one", "ONE", "Uno");
        fixture.translate("two", "TWO", "Dos");
        fixture
    }

    fn translate(&self, id: &str, source: &str, translation: &str) {
        let mut entry = StringEntry::new(id, source, format!("{id}.html").into());
        entry.translation = Some(translation.into());
        entry.status = StringStatus::Translated;
        self.db.save_entries(&[entry]).unwrap();
    }

    fn inject(&self) -> Result<DirectInjectReport> {
        let mut registry = FormatRegistry::new();
        registry.register(Box::new(SourceMatchWriter));
        inject_direct(
            &registry,
            &self.db,
            &self.manager,
            &self.root.path().join("game"),
            "source-match",
            &["es".into()],
        )
    }

    fn assert_refused_at(&self, file: &str) {
        let before = self.db.get_injection(Some("es")).unwrap();
        let backups = self.manager.list_backups().unwrap().len();
        let error = self.inject().unwrap_err().to_string();
        assert!(
            error.contains(&format!(
                "previously injected file \"{file}\" no longer matches its recorded hash/size"
            )),
            "{error}"
        );
        assert_eq!(self.db.get_injection(Some("es")).unwrap(), before);
        assert_eq!(self.manager.list_backups().unwrap().len(), backups);
    }
}

#[test]
fn direct_after_verified_restore_records_new_output_and_pristine_baseline() {
    let fixture = Fixture::new();
    let first = fixture.inject().unwrap();
    let before = fixture.db.get_injection(Some("es")).unwrap();
    fixture.manager.restore(&first.backup_id).unwrap();
    assert_eq!(fixture.db.get_injection(Some("es")).unwrap(), before);
    assert_eq!(
        fs::read(fixture.root.path().join("game/one.html")).unwrap(),
        b"ONE"
    );
    fixture.translate("one", "ONE", "Primero");

    let second = fixture
        .inject()
        .expect("Direct must accept verified restored originals");
    assert_eq!(second.files_modified, 2);
    assert_ne!(first.backup_id, second.backup_id);
    assert_eq!(second.pristine_backup_id.as_ref(), Some(&second.backup_id));
    assert_eq!(
        fs::read(fixture.root.path().join("game/one.html")).unwrap(),
        b"Primero"
    );
    let recording = fixture.db.get_injection(Some("es")).unwrap().unwrap();
    assert_eq!(recording.files.len(), 2);
    for file in &recording.files {
        assert_eq!(
            sha256_file(&recording.root.join(&file.rel)).unwrap(),
            (file.hash.clone(), file.size)
        );
    }
}

#[test]
fn direct_after_restore_recording_packs_against_restored_originals() {
    let fixture = Fixture::new();
    let first = fixture.inject().unwrap();
    fixture.manager.restore(&first.backup_id).unwrap();
    fixture.translate("one", "ONE", "Primero");
    fixture
        .inject()
        .expect("second Direct must succeed before packing");
    let output = fixture.root.path().join("patch.zip");
    let report = pack_with_pristine_backup(
        &fixture.db,
        PackOptions {
            game: None,
            game_path: fixture.root.path().join("game"),
            lang: Some("es".into()),
            output: output.clone(),
            pristine: None,
            engine: Some("source-match".into()),
            project: fixture.root.path().join("project.db"),
            require_pristine: true,
        },
        &fixture.manager,
        None,
        true,
    )
    .unwrap();
    assert_eq!(report.tier, "strict");
    let target = fixture.root.path().join("target");
    fs::create_dir(&target).unwrap();
    fs::write(target.join("one.html"), b"ONE").unwrap();
    fs::write(target.join("two.html"), b"TWO").unwrap();
    let verification = verify(&target, &output).unwrap();
    assert_eq!(verification.outcome, VerificationOutcome::Clean);
    let manifest = verification.manifest.unwrap();
    assert_eq!(manifest.files.len(), 2);
    assert!(manifest
        .files
        .iter()
        .any(|file| { file.original_sha256.as_deref() == Some(sha256_hex(b"ONE").as_str()) }));
}

#[test]
fn direct_without_restore_refuses_even_original_bytes_as_drift() {
    let fixture = Fixture::new();
    fixture.inject().unwrap();
    fs::write(fixture.root.path().join("game/one.html"), b"ONE").unwrap();
    fixture.translate("one", "ONE", "Primero");
    fixture.assert_refused_at("one.html");
    assert_eq!(
        fs::read(fixture.root.path().join("game/two.html")).unwrap(),
        b"Dos"
    );
}

#[test]
fn direct_after_partial_restore_still_refuses_drift_on_other_member() {
    let fixture = Fixture::new();
    let partial = fixture
        .manager
        .create_backup(&fixture.root.path().join("game/one.html"))
        .unwrap();
    fixture.inject().unwrap();
    fixture.manager.restore(&partial.id).unwrap();
    fs::write(fixture.root.path().join("game/two.html"), b"drift").unwrap();
    fixture.translate("one", "ONE", "Primero");
    fixture.assert_refused_at("two.html");
    assert_eq!(
        fs::read(fixture.root.path().join("game/one.html")).unwrap(),
        b"ONE"
    );
}

#[test]
fn direct_after_restore_refuses_new_drift_on_restored_member() {
    let fixture = Fixture::new();
    let first = fixture.inject().unwrap();
    fixture.manager.restore(&first.backup_id).unwrap();
    fs::write(fixture.root.path().join("game/one.html"), b"drift").unwrap();
    fixture.assert_refused_at("one.html");
}

#[test]
fn direct_partial_restore_omits_reverted_files_from_recording_union() {
    let fixture = Fixture::new();
    let partial = fixture
        .manager
        .create_backup(&fixture.root.path().join("game/one.html"))
        .unwrap();
    let first = fixture.inject().unwrap();
    fixture.manager.restore(&partial.id).unwrap();
    fixture.db.clear_entries().unwrap();
    fs::write(fixture.root.path().join("game/three.html"), b"THREE").unwrap();
    fixture.translate("three", "THREE", "Tres");
    fixture
        .inject()
        .expect("partial restore must retain only still-applied members");
    let recording = fixture.db.get_injection(Some("es")).unwrap().unwrap();
    let mut rels: Vec<_> = recording
        .files
        .iter()
        .map(|file| file.rel.as_str())
        .collect();
    rels.sort();
    assert_eq!(rels, vec!["three.html", "two.html"]);
    assert_eq!(recording.pristine_backup.unwrap().id, first.backup_id);
    assert_eq!(
        fs::read(fixture.root.path().join("game/one.html")).unwrap(),
        b"ONE"
    );
}
