use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use chrono::Utc;
use rusqlite::{params, Connection};

use crate::error::{LocustError, Result};

use super::{
    fold_path_case, lock_connection, path_identity_key, rel_under_root, sha256_file, Database,
    InjectionRecording, RecordedBackup, RecordedFile,
};

impl Database {
    pub fn get_project_metadata(&self, key: &str) -> Result<Option<serde_json::Value>> {
        let conn = lock_connection(&self.conn);
        let mut stmt = conn.prepare("SELECT value FROM project_metadata WHERE key = ?1")?;
        let mut rows = stmt.query(params![key])?;
        match rows.next()? {
            Some(row) => Ok(Some(serde_json::from_str(&row.get::<_, String>(0)?)?)),
            None => Ok(None),
        }
    }

    pub fn set_project_metadata(&self, key: &str, value: &serde_json::Value) -> Result<()> {
        lock_connection(&self.conn).execute(
            "INSERT OR REPLACE INTO project_metadata(key, value) VALUES (?1, ?2)",
            params![key, serde_json::to_string(value)?],
        )?;
        Ok(())
    }

    /// Record the files an injection run actually wrote for the `lang` key,
    /// under the root it targeted. `locust patch` packs EXCLUSIVELY from this
    /// recording: entries only name where text was READ, and for archive-based
    /// engines the files injection writes can never become entries.
    ///
    /// - `root` is absolutized here, so the recording survives any later cwd.
    /// - Each file is stored as a forward-slash rel under `root` plus the
    ///   SHA-256 and size of its bytes (read back once, right now).
    /// - CONTAINMENT: any file that does not resolve under `root` — or whose
    ///   rel keeps a `..` component — is a hard error and NOTHING is recorded
    ///   for this key. Recording a cross-tree write is how patches silently
    ///   shipped the wrong tree's files.
    /// - One physical file listed under two spellings is deduplicated; two
    ///   DIFFERENT files whose rels collide under case folding are an error,
    ///   because they cannot both extract from the patch zip on NTFS/APFS.
    /// - A new recording REPLACES the previous one for the SAME key only. An
    ///   EMPTY list is a no-op: a run that wrote nothing must not clobber the
    ///   last good recording, whose files are still on disk.
    pub fn record_injection(
        &self,
        lang: Option<&str>,
        root: &Path,
        written: &[PathBuf],
    ) -> Result<()> {
        self.record_injection_with_backup(lang, root, written, None)
    }

    /// Persist output hashes and their exact backup in one SQLite transaction.
    /// A no-op preserves both; a new legacy recording clears old provenance.
    pub fn record_injection_with_backup(
        &self,
        lang: Option<&str>,
        root: &Path,
        written: &[PathBuf],
        pristine_backup: Option<&RecordedBackup>,
    ) -> Result<()> {
        self.record_injection_generation(lang, root, written, pristine_backup, None)
    }

    /// Extend a checked generation atomically. Newly recorded registration files
    /// retain their pre-registration backup; repeated registration retains the
    /// first original. Ordinary injection recording clears these associations.
    pub fn extend_injection_with_registration_backup(
        &self,
        expected: &InjectionRecording,
        written: &[PathBuf],
        backup: &RecordedBackup,
    ) -> Result<()> {
        self.extend_injection_recording(expected, written, Some(backup))
    }

    /// Merge another Add output into its checked generation without losing
    /// registration originals. The caller holds the game lock throughout.
    pub(crate) fn extend_injection_recording(
        &self,
        expected: &InjectionRecording,
        written: &[PathBuf],
        registration_backup: Option<&RecordedBackup>,
    ) -> Result<()> {
        if written.is_empty() {
            return Ok(());
        }
        let changed: HashSet<_> = written
            .iter()
            .filter_map(|path| rel_under_root(path, &expected.root))
            .map(|rel| fold_path_case(&rel))
            .collect();
        for file in &expected.files {
            if !changed.contains(&fold_path_case(&file.rel)) {
                crate::extraction::verify_recorded_injection_member(&expected.root, file)?;
            }
        }
        let mut files: Vec<_> = expected
            .files
            .iter()
            .map(|f| expected.root.join(&f.rel))
            .collect();
        files.extend_from_slice(written);
        self.record_injection_generation(
            expected.lang.as_deref(),
            &expected.root,
            &files,
            expected.pristine_backup.as_ref(),
            Some((expected, registration_backup)),
        )
    }

    fn record_injection_generation(
        &self,
        lang: Option<&str>,
        root: &Path,
        written: &[PathBuf],
        pristine_backup: Option<&RecordedBackup>,
        extension: Option<(&InjectionRecording, Option<&RecordedBackup>)>,
    ) -> Result<()> {
        if written.is_empty() {
            return Ok(());
        }
        let pristine_backup = pristine_backup.map(serde_json::to_string).transpose()?;
        let root_abs = std::path::absolute(root)?;
        let mut seen: HashMap<String, (String, PathBuf)> = HashMap::new();
        let mut rows: Vec<(String, String, u64)> = Vec::new();
        for p in written {
            let rel = rel_under_root(p, &root_abs).ok_or_else(|| {
                LocustError::InjectionError(format!(
                    "cannot record injection output: \"{}\" is not under the \
                     injection root \"{}\"",
                    p.display(),
                    root_abs.display()
                ))
            })?;
            if Path::new(&rel)
                .components()
                .any(|c| matches!(c, std::path::Component::ParentDir))
            {
                return Err(LocustError::InjectionError(format!(
                    "cannot record injection output: \"{}\" escapes the \
                     injection root \"{}\" via a `..` component",
                    p.display(),
                    root_abs.display()
                )));
            }
            let identity = path_identity_key(p);
            match seen.get(&fold_path_case(&rel)) {
                Some((existing_identity, existing_path)) => {
                    if existing_identity != &identity {
                        return Err(LocustError::InjectionError(format!(
                            "two different written files collide on the same \
                             archive path \"{}\": \"{}\" and \"{}\" — they \
                             cannot both extract from the patch zip on a \
                             case-folding filesystem",
                            rel,
                            existing_path.display(),
                            p.display()
                        )));
                    }
                    continue; // same physical file listed twice
                }
                None => {
                    seen.insert(fold_path_case(&rel), (identity, p.clone()));
                }
            }
            let (hash, size) = sha256_file(p)?;
            rows.push((rel, hash, size));
        }

        let root_str = root_abs.to_string_lossy().to_string();
        let now = Utc::now().to_rfc3339();
        let conn = lock_connection(&self.conn);
        let tx = conn.unchecked_transaction()?;
        let mut registration_backups = HashMap::new();
        if let Some((expected, backup)) = extension {
            if Self::get_injection_from(&tx, lang)?.as_ref() != Some(expected) {
                return Err(LocustError::InjectionError(
                    "injection recording changed during language registration".into(),
                ));
            }
            let mut stmt =
                tx.prepare("SELECT rel, registration_backup FROM injected_files WHERE lang IS ?1")?;
            let saved = stmt.query_map(params![lang], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?))
            })?;
            for row in saved {
                let (rel, backup) = row?;
                registration_backups.insert(fold_path_case(&rel), backup);
            }
            let backup = backup.map(serde_json::to_string).transpose()?;
            for (rel, _, _) in &rows {
                registration_backups
                    .entry(fold_path_case(rel))
                    .or_insert_with(|| backup.clone());
            }
        }
        tx.execute("DELETE FROM injected_files WHERE lang IS ?1", params![lang])?;
        // Same class as save_entries/merge_entries: one plan for N file rows
        // (Unreal/Unity injects can record hundreds of written paths).
        {
            let mut insert = tx.prepare_cached(
                "INSERT INTO injected_files (lang, root, rel, hash, size, recorded_at, pristine_backup, registration_backup)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            )?;
            for (rel, hash, size) in &rows {
                insert.execute(params![
                    lang,
                    root_str,
                    rel,
                    hash,
                    *size as i64,
                    now,
                    pristine_backup,
                    registration_backups
                        .get(&fold_path_case(rel))
                        .and_then(Option::as_ref)
                ])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// The recording persisted by [`Self::record_injection`] for exactly this
    /// key — `None` matches only the language-unspecified recording, never a
    /// named one, and vice versa. `Ok(None)` when nothing is recorded for it.
    pub fn get_injection(&self, lang: Option<&str>) -> Result<Option<InjectionRecording>> {
        let conn = lock_connection(&self.conn);
        Self::get_injection_from(&conn, lang)
    }

    fn get_injection_from(
        conn: &Connection,
        lang: Option<&str>,
    ) -> Result<Option<InjectionRecording>> {
        let mut stmt = conn.prepare(
            "SELECT root, rel, hash, size, recorded_at, pristine_backup FROM injected_files
             WHERE lang IS ?1 ORDER BY id ASC",
        )?;
        let rows = stmt.query_map(params![lang], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, Option<String>>(5)?,
            ))
        })?;
        let mut backup_json: Option<Option<String>> = None;
        let mut root = None;
        let mut recorded_at = String::new();
        let mut files = Vec::new();
        for row in rows {
            let (r, rel, hash, size, at, backup) = row?;
            if backup_json
                .as_ref()
                .is_some_and(|previous| previous != &backup)
                || root.as_ref().is_some_and(|previous| previous != &r)
                || (!recorded_at.is_empty() && recorded_at != at)
            {
                return Err(LocustError::InjectionError(
                    "inconsistent injection recording provenance".into(),
                ));
            }
            backup_json.get_or_insert(backup);
            root.get_or_insert(r);
            recorded_at = at;
            files.push(RecordedFile {
                rel,
                hash,
                size: size as u64,
            });
        }
        match root {
            Some(r) => Ok(Some(InjectionRecording {
                lang: lang.map(str::to_string),
                root: PathBuf::from(r),
                files,
                recorded_at,
                pristine_backup: backup_json
                    .flatten()
                    .map(|json| serde_json::from_str(&json))
                    .transpose()?,
            })),
            None => Ok(None),
        }
    }

    /// Exact verified backup used for each registration-only replaced file.
    pub fn registration_backups(
        &self,
        lang: Option<&str>,
    ) -> Result<Vec<(String, RecordedBackup)>> {
        let conn = lock_connection(&self.conn);
        let mut stmt = conn.prepare("SELECT rel, registration_backup FROM injected_files WHERE lang IS ?1 AND registration_backup IS NOT NULL")?;
        let rows = stmt.query_map(params![lang], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        rows.map(|row| {
            let (rel, backup) = row?;
            Ok((rel, serde_json::from_str(&backup)?))
        })
        .collect()
    }

    #[cfg(test)]
    pub(crate) fn fail_registration_recording(&self) {
        lock_connection(&self.conn)
            .execute_batch(
                "CREATE TRIGGER fail_registration BEFORE INSERT ON injected_files
             BEGIN SELECT RAISE(ABORT, 'recording failure'); END;",
            )
            .unwrap();
    }

    /// The pristine backup for each recorded language that has one.
    /// Decode through `get_injection` so provenance and JSON errors are preserved.
    pub fn recorded_backup_refs(&self) -> Result<Vec<(Option<String>, RecordedBackup)>> {
        let mut refs = Vec::new();
        for lang in self.list_recorded_langs()? {
            if let Some(backup) = self
                .get_injection(lang.as_deref())?
                .and_then(|recording| recording.pristine_backup)
            {
                refs.push((lang, backup));
            }
        }
        for lang in self.list_recorded_langs()? {
            for (_, backup) in self.registration_backups(lang.as_deref())? {
                refs.push((lang.clone(), backup));
            }
        }
        Ok(refs)
    }

    /// Every language key with a recording, named keys first, the reserved
    /// language-unspecified key (`None`) last. Empty when no injection has
    /// ever been recorded — `locust patch` must then hard-error with the
    /// exact inject command, never fall back to guessing from entries.
    pub fn list_recorded_langs(&self) -> Result<Vec<Option<String>>> {
        let conn = lock_connection(&self.conn);
        let mut stmt =
            conn.prepare("SELECT DISTINCT lang FROM injected_files ORDER BY (lang IS NULL), lang")?;
        let rows = stmt.query_map([], |row| row.get::<_, Option<String>>(0))?;
        let mut langs = Vec::new();
        for row in rows {
            langs.push(row?);
        }
        Ok(langs)
    }
}

#[cfg(test)]
mod registration_recording_tests {
    use super::*;

    #[test]
    fn registration_extension_keeps_outputs_and_first_originals() {
        let root = tempfile::tempdir().unwrap();
        let db = Database::open_in_memory().unwrap();
        let pack = root.path().join("lang_es.json");
        let menu = root.path().join("plugins.js");
        std::fs::write(&pack, b"Spanish pack").unwrap();
        std::fs::write(&menu, b"Spanish menu").unwrap();
        db.record_injection(Some("es"), root.path(), std::slice::from_ref(&pack))
            .unwrap();
        let first = db.get_injection(Some("es")).unwrap().unwrap();
        let backup = RecordedBackup {
            id: "first".into(),
            source_path: root.path().to_owned(),
            storage_root: None,
        };
        db.extend_injection_with_registration_backup(&first, std::slice::from_ref(&menu), &backup)
            .unwrap();
        let registered = db.get_injection(Some("es")).unwrap().unwrap();
        assert_eq!(registered.files.len(), 2);
        assert_eq!(registered.files[0], first.files[0]);
        let file = registered
            .files
            .iter()
            .find(|f| f.rel == "plugins.js")
            .unwrap();
        assert_eq!((file.hash.clone(), file.size), sha256_file(&menu).unwrap());
        std::fs::write(&menu, b"Updated Spanish menu").unwrap();
        let later = RecordedBackup {
            id: "later".into(),
            ..backup.clone()
        };
        db.extend_injection_with_registration_backup(
            &registered,
            std::slice::from_ref(&menu),
            &later,
        )
        .unwrap();
        assert_eq!(
            db.registration_backups(Some("es")).unwrap(),
            vec![("plugins.js".into(), backup.clone())]
        );
        let updated = db.get_injection(Some("es")).unwrap().unwrap();
        std::fs::write(&pack, b"Revised Spanish pack").unwrap();
        db.extend_injection_recording(&updated, std::slice::from_ref(&pack), None)
            .unwrap();
        assert_eq!(
            db.registration_backups(Some("es")).unwrap(),
            vec![("plugins.js".into(), backup)]
        );
        assert_eq!(
            db.get_injection(Some("es")).unwrap().unwrap().files.len(),
            2
        );
    }

    #[test]
    fn registration_extension_rejects_stale_generation_and_unrelated_drift() {
        let root = tempfile::tempdir().unwrap();
        let db = Database::open_in_memory().unwrap();
        let pack = root.path().join("lang_es.json");
        let menu = root.path().join("plugins.js");
        std::fs::write(&pack, b"Spanish pack").unwrap();
        std::fs::write(&menu, b"Spanish menu").unwrap();
        db.record_injection(Some("es"), root.path(), std::slice::from_ref(&pack))
            .unwrap();
        let before = db.get_injection(Some("es")).unwrap().unwrap();
        let backup = RecordedBackup {
            id: "first".into(),
            source_path: root.path().to_owned(),
            storage_root: None,
        };
        std::fs::write(&pack, b"external edit").unwrap();
        assert!(db
            .extend_injection_with_registration_backup(
                &before,
                std::slice::from_ref(&menu),
                &backup
            )
            .is_err());
        assert_eq!(db.get_injection(Some("es")).unwrap().unwrap(), before);
        std::fs::write(&pack, b"Spanish pack").unwrap();
        db.record_injection(Some("es"), root.path(), std::slice::from_ref(&menu))
            .unwrap();
        let replacement = db.get_injection(Some("es")).unwrap();
        assert!(db
            .extend_injection_with_registration_backup(&before, &[menu], &backup)
            .unwrap_err()
            .to_string()
            .contains("recording changed"));
        assert_eq!(db.get_injection(Some("es")).unwrap(), replacement);
        assert!(db.registration_backups(Some("es")).unwrap().is_empty());
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::{make_recorded_tree, recording_tempdir};
    use super::super::{paths_identical, sha256_hex};
    use super::*;

    #[test]
    fn project_metadata_roundtrips_reopen_and_update() {
        let dir_guard = recording_tempdir();
        let dir = dir_guard.path().to_path_buf();
        let path = dir.join("metadata.db");
        let db = Database::open(&path).unwrap();
        assert_eq!(db.get_project_metadata("missing").unwrap(), None);
        db.set_project_metadata("key", &serde_json::json!({"日本語": [1, false]}))
            .unwrap();
        drop(db);
        let db = Database::open(&path).unwrap();
        assert_eq!(
            db.get_project_metadata("key").unwrap(),
            Some(serde_json::json!({"日本語": [1, false]}))
        );
        db.set_project_metadata("key", &serde_json::Value::Null)
            .unwrap();
        assert_eq!(
            db.get_project_metadata("key").unwrap(),
            Some(serde_json::Value::Null)
        );
    }

    #[test]
    fn test_recorded_backup_refs_returns_one_per_language_with_a_backup() {
        let base = tempfile::tempdir().unwrap();
        let (root, file, _) = make_recorded_tree(base.path());
        let second = root.join("second.rpy");
        std::fs::write(&second, b"translated").unwrap();
        let files = [file, second];
        let db = Database::open_in_memory().unwrap();
        assert!(db.recorded_backup_refs().unwrap().is_empty());

        db.record_injection(Some("ja"), &root, &files).unwrap();
        assert!(db.recorded_backup_refs().unwrap().is_empty());

        let backup = RecordedBackup {
            id: "shared-backup".into(),
            source_path: root.clone(),
            storage_root: Some(base.path().join("backups")),
        };
        let legacy_backup = RecordedBackup {
            storage_root: None,
            ..backup.clone()
        };
        for lang in [Some("es"), Some("fr")] {
            db.record_injection_with_backup(lang, &root, &files, Some(&backup))
                .unwrap();
        }
        db.record_injection_with_backup(None, &root, &files, Some(&legacy_backup))
            .unwrap();

        assert_eq!(
            db.recorded_backup_refs().unwrap(),
            vec![
                (Some("es".into()), backup.clone()),
                (Some("fr".into()), backup),
                (None, legacy_backup),
            ]
        );
    }

    #[test]
    fn test_recorded_backup_refs_propagates_recording_decode_errors() {
        let base = tempfile::tempdir().unwrap();
        let (root, file, _) = make_recorded_tree(base.path());
        let second = root.join("second.rpy");
        std::fs::write(&second, b"translated").unwrap();
        let files = [file, second];
        for corruption in [
            "UPDATE injected_files SET pristine_backup = 'not json'",
            "UPDATE injected_files SET root = 'different' WHERE rel = 'second.rpy'",
        ] {
            let db = Database::open_in_memory().unwrap();
            db.record_injection(Some("es"), &root, &files).unwrap();
            lock_connection(&db.conn).execute(corruption, []).unwrap();
            let expected = db.get_injection(Some("es")).unwrap_err().to_string();
            assert_eq!(db.recorded_backup_refs().unwrap_err().to_string(), expected);
        }
    }

    #[test]
    fn test_record_injection_roundtrip_stores_rel_hash_and_size() {
        let base_guard = recording_tempdir();
        let base = base_guard.path().to_path_buf();
        let (root, file, bytes) = make_recorded_tree(&base);
        let db = Database::open_in_memory().unwrap();

        db.record_injection(Some("es"), &root, &[file]).unwrap();

        let rec = db
            .get_injection(Some("es"))
            .unwrap()
            .expect("a recording must exist for es");
        assert!(
            paths_identical(&rec.root, &root),
            "recorded root {} must name the injection root {}",
            rec.root.display(),
            root.display()
        );
        assert!(
            rec.root.is_absolute(),
            "the root must be absolutized at record time"
        );
        assert_eq!(rec.files.len(), 1);
        assert_eq!(
            rec.files[0].rel, "game/script.rpy",
            "rels are stored with forward slashes, relative to the root"
        );
        assert_eq!(rec.files[0].hash, sha256_hex(&bytes));
        assert_eq!(rec.files[0].size, bytes.len() as u64);
    }

    #[test]
    fn test_record_injection_many_files_all_persist() {
        // Pins the prepare_cached INSERT path: N rows in one transaction must
        // all round-trip (guards against accidentally emptying the insert loop).
        let base_guard = recording_tempdir();
        let base = base_guard.path().to_path_buf();
        let root = base.join("root");
        let sub = root.join("game");
        std::fs::create_dir_all(&sub).unwrap();
        let mut files = Vec::new();
        for i in 0..40 {
            let p = sub.join(format!("f{i}.rpy"));
            std::fs::write(&p, format!("line {i}")).unwrap();
            files.push(p);
        }
        let db = Database::open_in_memory().unwrap();
        db.record_injection(Some("es"), &root, &files).unwrap();
        let rec = db.get_injection(Some("es")).unwrap().unwrap();
        assert_eq!(rec.files.len(), 40);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn test_record_injection_null_key_is_its_own_recording() {
        // `--direct` without -l records under the reserved NULL key, matched
        // only by `patch` without -l — never silently by a named language.
        let base_guard = recording_tempdir();
        let base = base_guard.path().to_path_buf();
        let (root, file, _) = make_recorded_tree(&base);
        let db = Database::open_in_memory().unwrap();

        db.record_injection(None, &root, &[file]).unwrap();

        assert!(db.get_injection(None).unwrap().is_some());
        assert!(
            db.get_injection(Some("es")).unwrap().is_none(),
            "a named language must never match the language-unspecified recording"
        );
        assert_eq!(db.list_recorded_langs().unwrap(), vec![None]);
    }

    #[test]
    fn test_record_injection_replaces_only_its_own_key() {
        let base_guard = recording_tempdir();
        let base = base_guard.path().to_path_buf();
        let root = base.join("root");
        std::fs::create_dir_all(root.join("game")).unwrap();
        let a = root.join("game").join("a.rpy");
        let b = root.join("game").join("b.rpy");
        let c = root.join("game").join("c.rpy");
        for f in [&a, &b, &c] {
            std::fs::write(f, b"x").unwrap();
        }
        let db = Database::open_in_memory().unwrap();

        db.record_injection(Some("es"), &root, &[a]).unwrap();
        db.record_injection(Some("fr"), &root, &[b]).unwrap();
        db.record_injection(Some("es"), &root, &[c]).unwrap();

        let es = db.get_injection(Some("es")).unwrap().unwrap();
        assert_eq!(
            es.files.iter().map(|f| f.rel.as_str()).collect::<Vec<_>>(),
            vec!["game/c.rpy"],
            "a new recording replaces the previous one for the SAME key only"
        );
        let fr = db.get_injection(Some("fr")).unwrap().unwrap();
        assert_eq!(fr.files[0].rel, "game/b.rpy");
        assert_eq!(
            db.list_recorded_langs().unwrap(),
            vec![Some("es".to_string()), Some("fr".to_string())]
        );
    }

    #[test]
    fn test_record_injection_empty_report_keeps_previous_recording() {
        // An inject run that wrote nothing must not clobber the last good
        // recording — the files it previously wrote are still on disk.
        let base_guard = recording_tempdir();
        let base = base_guard.path().to_path_buf();
        let (root, file, _) = make_recorded_tree(&base);
        let db = Database::open_in_memory().unwrap();
        db.record_injection(Some("es"), &root, &[file]).unwrap();

        db.record_injection(Some("es"), &root, &[]).unwrap();

        let rec = db.get_injection(Some("es")).unwrap().unwrap();
        assert_eq!(rec.files[0].rel, "game/script.rpy");
    }

    #[test]
    fn test_record_injection_refuses_a_file_outside_the_root() {
        // Containment is checked at record time: a plugin that wrote into a
        // different tree (Unity/Unreal/Wolf/Ren'Py-loose in Replace mode) must
        // produce a hard error and record NOTHING — recording the paths would
        // silently re-create the packs-the-wrong-tree corruption.
        let base_guard = recording_tempdir();
        let base = base_guard.path().to_path_buf();
        let (root, file, _) = make_recorded_tree(&base);
        let outside = base.join("elsewhere.rpy");
        std::fs::write(&outside, b"y").unwrap();
        let db = Database::open_in_memory().unwrap();

        let err = db
            .record_injection(Some("es"), &root, &[file, outside.clone()])
            .expect_err("a write outside the root must refuse to record");
        let msg = err.to_string();
        assert!(
            msg.contains(&outside.display().to_string()),
            "error must name the escaping file: {msg}"
        );
        assert!(
            db.get_injection(Some("es")).unwrap().is_none(),
            "nothing may be recorded when any file escapes the root"
        );
    }

    #[test]
    fn test_record_injection_refuses_parent_dir_escape() {
        // Zip-slip guard at record time: a dot-dot path escaping the root
        // must refuse to record. On Windows `std::path::absolute` collapses
        // `..` lexically, so the CONTAINMENT branch catches it; on POSIX the
        // `..` survives resolution and the explicit `..` branch does. Either
        // way the refusal must come from the recording guards (shared
        // "cannot record injection output" contract), never from an
        // incidental read failure later on.
        let base_guard = recording_tempdir();
        let base = base_guard.path().to_path_buf();
        let (root, _file, _) = make_recorded_tree(&base);
        let sneaky = root.join("game").join("..").join("..").join("evil.txt");
        let db = Database::open_in_memory().unwrap();

        let err = db
            .record_injection(Some("es"), &root, &[sneaky])
            .expect_err("a dot-dot path escaping the root must refuse to record");
        assert!(
            err.to_string().contains("cannot record injection output"),
            "the refusal must come from the recording guards: {err}"
        );
        assert!(db.get_injection(Some("es")).unwrap().is_none());
    }

    #[cfg(any(windows, target_os = "macos"))]
    #[test]
    fn test_record_injection_dedupes_two_case_spellings_of_one_file() {
        // ONE physical file reached under two case spellings is a duplicate,
        // not a collision — NTFS/APFS fold case, so both spellings name the
        // same bytes and the file must be recorded exactly once.
        let base_guard = recording_tempdir();
        let base = base_guard.path().to_path_buf();
        let (root, file, _) = make_recorded_tree(&base);
        let respelled = root.join("game").join("SCRIPT.RPY");
        let db = Database::open_in_memory().unwrap();

        db.record_injection(Some("es"), &root, &[file, respelled])
            .unwrap();

        let rec = db.get_injection(Some("es")).unwrap().unwrap();
        assert_eq!(
            rec.files.len(),
            1,
            "one physical file must be recorded once"
        );
    }
}
