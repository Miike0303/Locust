use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use walkdir::WalkDir;

use crate::error::{LocustError, Result};

pub struct BackupManager {
    backup_root: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackupEntry {
    pub id: String,
    pub path: PathBuf,
    pub created_at: DateTime<Utc>,
    pub source_path: PathBuf,
    pub file_count: usize,
    pub size_bytes: u64,
}

#[derive(Serialize, Deserialize)]
pub struct BackupManifest {
    pub source_path: PathBuf,
    pub created_at: DateTime<Utc>,
    pub file_count: usize,
    pub size_bytes: u64,
}

impl BackupManager {
    pub fn new(backup_root: PathBuf) -> Self {
        Self { backup_root }
    }

    /// Reserve a fresh backup directory, returning its id.
    ///
    /// Ids stay human-readable second-resolution timestamps, so two backups
    /// started in the same second collide — which used to merge both game
    /// trees into one directory and leave whichever manifest was written last.
    /// `create_dir` is atomic and fails on an existing directory, so losing the
    /// race just means trying the next suffix.
    fn claim_backup_dir(&self, now: DateTime<Utc>) -> Result<(String, PathBuf)> {
        std::fs::create_dir_all(&self.backup_root)?;
        let base = now.format("%Y%m%d_%H%M%S").to_string();
        for attempt in 0..1000 {
            let id = if attempt == 0 {
                base.clone()
            } else {
                format!("{base}_{attempt}")
            };
            let dir = self.backup_root.join(&id);
            match std::fs::create_dir(&dir) {
                Ok(()) => return Ok((id, dir)),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e.into()),
            }
        }
        Err(LocustError::BackupError(format!(
            "could not reserve a backup directory under {} for {base}",
            self.backup_root.display()
        )))
    }

    pub fn create_backup(&self, game_path: &Path) -> Result<BackupEntry> {
        let now = Utc::now();
        let (timestamp, backup_dir) = self.claim_backup_dir(now)?;

        let mut file_count = 0usize;
        let mut size_bytes = 0u64;

        for entry in WalkDir::new(game_path).follow_links(false) {
            let entry = entry.map_err(|e| LocustError::BackupError(e.to_string()))?;
            let rel = entry
                .path()
                .strip_prefix(game_path)
                .map_err(|e| LocustError::BackupError(e.to_string()))?;
            let dest = backup_dir.join(rel);

            if entry.file_type().is_dir() {
                std::fs::create_dir_all(&dest)?;
            } else if entry.file_type().is_file() {
                if let Some(parent) = dest.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                let metadata = entry
                    .metadata()
                    .map_err(|e| LocustError::BackupError(e.to_string()))?;
                size_bytes += metadata.len();
                std::fs::copy(entry.path(), &dest)?;
                file_count += 1;
            }
        }

        let manifest = BackupManifest {
            source_path: game_path.to_path_buf(),
            created_at: now,
            file_count,
            size_bytes,
        };
        let manifest_json = serde_json::to_string_pretty(&manifest)?;
        std::fs::write(backup_dir.join("manifest.json"), manifest_json)?;

        Ok(BackupEntry {
            id: timestamp,
            path: backup_dir,
            created_at: now,
            source_path: game_path.to_path_buf(),
            file_count,
            size_bytes,
        })
    }

    /// Reject anything that is not a bare directory name.
    ///
    /// Ids reach here from untrusted input — `POST /api/backups/:id/restore`
    /// and its delete sibling — and are joined onto the backup root, so a
    /// traversal would let a caller restore from, or delete, any directory.
    fn resolve_backup_dir(&self, backup_id: &str) -> Result<PathBuf> {
        let mut components = Path::new(backup_id).components();
        let single_plain_name = matches!(components.next(), Some(std::path::Component::Normal(_)))
            && components.next().is_none();
        if !single_plain_name {
            return Err(LocustError::BackupError(format!(
                "invalid backup id: {backup_id}"
            )));
        }
        Ok(self.backup_root.join(backup_id))
    }

    pub fn restore(&self, backup_id: &str, target_path: &Path) -> Result<()> {
        let backup_dir = self.resolve_backup_dir(backup_id)?;
        if !backup_dir.exists() {
            return Err(LocustError::BackupError(format!(
                "backup not found: {}",
                backup_id
            )));
        }

        for entry in WalkDir::new(&backup_dir).follow_links(false) {
            let entry = entry.map_err(|e| LocustError::BackupError(e.to_string()))?;
            let rel = entry
                .path()
                .strip_prefix(&backup_dir)
                .map_err(|e| LocustError::BackupError(e.to_string()))?;

            // Skip manifest.json
            if rel == Path::new("manifest.json") {
                continue;
            }

            let dest = target_path.join(rel);

            if entry.file_type().is_dir() {
                std::fs::create_dir_all(&dest)?;
            } else if entry.file_type().is_file() {
                if let Some(parent) = dest.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::copy(entry.path(), &dest)?;
            }
        }

        Ok(())
    }

    pub fn list_backups(&self) -> Result<Vec<BackupEntry>> {
        if !self.backup_root.exists() {
            return Ok(Vec::new());
        }

        let mut entries = Vec::new();
        for dir_entry in std::fs::read_dir(&self.backup_root)? {
            let dir_entry = dir_entry?;
            if !dir_entry.file_type()?.is_dir() {
                continue;
            }
            let manifest_path = dir_entry.path().join("manifest.json");
            if !manifest_path.exists() {
                continue;
            }
            let manifest_str = std::fs::read_to_string(&manifest_path)?;
            let manifest: BackupManifest = serde_json::from_str(&manifest_str)?;
            let id = dir_entry.file_name().to_string_lossy().to_string();
            entries.push(BackupEntry {
                id,
                path: dir_entry.path(),
                created_at: manifest.created_at,
                source_path: manifest.source_path,
                file_count: manifest.file_count,
                size_bytes: manifest.size_bytes,
            });
        }

        entries.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        Ok(entries)
    }

    pub fn delete_backup(&self, backup_id: &str) -> Result<()> {
        let backup_dir = self.resolve_backup_dir(backup_id)?;
        if backup_dir.exists() {
            std::fs::remove_dir_all(&backup_dir)?;
        }
        Ok(())
    }

    pub fn delete_old_backups(&self, keep_last: usize) -> Result<usize> {
        let backups = self.list_backups()?;
        let mut deleted = 0;
        if backups.len() > keep_last {
            for backup in &backups[keep_last..] {
                self.delete_backup(&backup.id)?;
                deleted += 1;
            }
        }
        Ok(deleted)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn tempdir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("locust_bak_{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn create_game_dir() -> PathBuf {
        let dir = tempdir();
        fs::write(dir.join("data.json"), r#"{"hp": 100}"#).unwrap();
        fs::write(dir.join("strings.txt"), "Hello\nWorld").unwrap();
        fs::create_dir_all(dir.join("sub")).unwrap();
        fs::write(dir.join("sub").join("nested.txt"), "nested content").unwrap();
        dir
    }

    #[test]
    fn test_create_backup_copies_files() {
        let game_dir = create_game_dir();
        let backup_root = tempdir();
        let mgr = BackupManager::new(backup_root);
        let entry = mgr.create_backup(&game_dir).unwrap();

        assert!(entry.path.join("data.json").exists());
        assert!(entry.path.join("strings.txt").exists());
        assert!(entry.path.join("sub").join("nested.txt").exists());
        assert_eq!(entry.file_count, 3);
    }

    #[test]
    fn test_create_backup_writes_manifest() {
        let game_dir = create_game_dir();
        let backup_root = tempdir();
        let mgr = BackupManager::new(backup_root);
        let entry = mgr.create_backup(&game_dir).unwrap();

        let manifest_path = entry.path.join("manifest.json");
        assert!(manifest_path.exists());
        let manifest_str = fs::read_to_string(&manifest_path).unwrap();
        let manifest: BackupManifest = serde_json::from_str(&manifest_str).unwrap();
        assert_eq!(manifest.file_count, 3);
    }

    #[test]
    fn test_list_backups_sorted() {
        let game_dir = create_game_dir();
        let backup_root = tempdir();
        let mgr = BackupManager::new(backup_root);
        let _b1 = mgr.create_backup(&game_dir).unwrap();
        let _b2 = mgr.create_backup(&game_dir).unwrap();

        let list = mgr.list_backups().unwrap();
        assert_eq!(list.len(), 2);
        assert!(list[0].created_at >= list[1].created_at);
    }

    #[test]
    fn test_backup_id_traversal_is_rejected() {
        // Ids arrive from HTTP (`POST /api/backups/:id/restore` and delete),
        // so a traversal must not reach outside the backup root — delete in
        // particular would remove the directory tree it lands on.
        let backup_root = tempdir();
        let outsider = backup_root
            .parent()
            .unwrap()
            .join(format!("locust_outsider_{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&outsider).unwrap();
        fs::write(outsider.join("keep.txt"), "important").unwrap();

        let mgr = BackupManager::new(backup_root);
        let escape = format!("../{}", outsider.file_name().unwrap().to_string_lossy());
        let restore_target = tempdir();

        for bad in [
            escape.as_str(),
            "..",
            ".",
            "",
            "sub/dir",
            "sub\\dir",
            "/etc",
        ] {
            assert!(
                mgr.restore(bad, &restore_target).is_err(),
                "restore accepted {bad:?}"
            );
            assert!(mgr.delete_backup(bad).is_err(), "delete accepted {bad:?}");
        }

        assert!(
            outsider.join("keep.txt").exists(),
            "traversal deleted a directory outside the backup root"
        );

        // A real id still works.
        let game_dir = create_game_dir();
        let entry = mgr.create_backup(&game_dir).unwrap();
        mgr.restore(&entry.id, &restore_target).unwrap();
        mgr.delete_backup(&entry.id).unwrap();
    }

    #[test]
    fn test_same_second_backups_do_not_share_a_directory() {
        // Backup ids are second-resolution timestamps. Two injects within the
        // same second must not land in one directory, or each overwrites the
        // other's manifest and restore hands back a mix of both games.
        let backup_root = tempdir();
        let mgr = BackupManager::new(backup_root);

        let game_a = create_game_dir();
        let game_b = tempdir();
        fs::write(game_b.join("only_in_b.txt"), "b").unwrap();

        let a = mgr.create_backup(&game_a).unwrap();
        let b = mgr.create_backup(&game_b).unwrap();

        assert_ne!(a.id, b.id, "same-second backups must get distinct ids");
        assert_ne!(a.path, b.path);

        // Each backup holds its own tree, not the other's.
        assert!(a.path.join("data.json").exists());
        assert!(!a.path.join("only_in_b.txt").exists());
        assert!(b.path.join("only_in_b.txt").exists());
        assert!(!b.path.join("data.json").exists());
        assert_eq!(a.file_count, 3);
        assert_eq!(b.file_count, 1);

        // Both remain independently listable and restorable.
        assert_eq!(mgr.list_backups().unwrap().len(), 2);
        let restore_target = tempdir();
        mgr.restore(&b.id, &restore_target).unwrap();
        assert!(restore_target.join("only_in_b.txt").exists());
        assert!(!restore_target.join("data.json").exists());
    }

    #[test]
    fn test_restore_overwrites_target() {
        let game_dir = create_game_dir();
        let backup_root = tempdir();
        let mgr = BackupManager::new(backup_root);
        let entry = mgr.create_backup(&game_dir).unwrap();

        // Modify original file
        fs::write(game_dir.join("data.json"), "MODIFIED").unwrap();
        assert_eq!(
            fs::read_to_string(game_dir.join("data.json")).unwrap(),
            "MODIFIED"
        );

        // Restore
        mgr.restore(&entry.id, &game_dir).unwrap();
        assert_eq!(
            fs::read_to_string(game_dir.join("data.json")).unwrap(),
            r#"{"hp": 100}"#
        );
    }

    #[test]
    fn test_delete_backup() {
        let game_dir = create_game_dir();
        let backup_root = tempdir();
        let mgr = BackupManager::new(backup_root);
        let entry = mgr.create_backup(&game_dir).unwrap();
        mgr.delete_backup(&entry.id).unwrap();
        let list = mgr.list_backups().unwrap();
        assert!(list.is_empty());
    }

    #[test]
    fn test_delete_old_keeps_recent() {
        let game_dir = create_game_dir();
        let backup_root = tempdir();
        let mgr = BackupManager::new(backup_root);
        for _ in 0..5 {
            mgr.create_backup(&game_dir).unwrap();
            std::thread::sleep(std::time::Duration::from_millis(1100));
        }
        let deleted = mgr.delete_old_backups(2).unwrap();
        assert_eq!(deleted, 3);
        let remaining = mgr.list_backups().unwrap();
        assert_eq!(remaining.len(), 2);
    }
}
