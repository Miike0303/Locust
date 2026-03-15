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

    pub fn create_backup(&self, game_path: &Path) -> Result<BackupEntry> {
        let now = Utc::now();
        let timestamp = now.format("%Y%m%d_%H%M%S").to_string();
        let backup_dir = self.backup_root.join(&timestamp);
        std::fs::create_dir_all(&backup_dir)?;

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
                let metadata = entry.metadata().map_err(|e| LocustError::BackupError(e.to_string()))?;
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

    pub fn restore(&self, backup_id: &str, target_path: &Path) -> Result<()> {
        let backup_dir = self.backup_root.join(backup_id);
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
            let id = dir_entry
                .file_name()
                .to_string_lossy()
                .to_string();
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
        let backup_dir = self.backup_root.join(backup_id);
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
        std::thread::sleep(std::time::Duration::from_millis(1100));
        let _b2 = mgr.create_backup(&game_dir).unwrap();

        let list = mgr.list_backups().unwrap();
        assert_eq!(list.len(), 2);
        assert!(list[0].created_at >= list[1].created_at);
    }

    #[test]
    fn test_restore_overwrites_target() {
        let game_dir = create_game_dir();
        let backup_root = tempdir();
        let mgr = BackupManager::new(backup_root);
        let entry = mgr.create_backup(&game_dir).unwrap();

        // Modify original file
        fs::write(game_dir.join("data.json"), "MODIFIED").unwrap();
        assert_eq!(fs::read_to_string(game_dir.join("data.json")).unwrap(), "MODIFIED");

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
