use std::path::{Path, PathBuf};

use locust_core::config::AppConfig;
use locust_core::database::Database;

pub(crate) fn open_existing_project(path: &Path) -> anyhow::Result<Database> {
    require_existing_project(path)?;
    Database::open(path).map_err(|error| {
        // SQLite's own wording repeats three times through the cause chain.
        if error.to_string().contains("file is not a database") {
            anyhow::anyhow!(
                "not a Locust project database: {}. Select the .locust.db file created by `locust extract`.",
                path.display()
            )
        } else {
            error.into()
        }
    })
}

pub(crate) fn require_existing_project(path: &Path) -> anyhow::Result<()> {
    if !path.try_exists()? {
        anyhow::bail!(
            "project database not found: {}. Create it with `locust extract <game> -o \"{}\"`",
            path.display(),
            path.display()
        );
    }
    if path.metadata()?.is_dir() {
        anyhow::bail!("project database path is a directory: {}", path.display());
    }
    Ok(())
}

/// Persistent backup root for inject / direct-inject. A nonempty
/// `LOCUST_BACKUP_ROOT` isolates tests or selects another volume; otherwise the
/// root follows `AppConfig::config_dir()`, including `LOCUST_DATA_DIR`.
pub(crate) fn locust_backup_root() -> PathBuf {
    #[cfg(test)]
    assert!(
        std::env::var_os("LOCUST_BACKUP_ROOT").is_some_and(|path| !path.is_empty())
            || std::env::var_os("LOCUST_DATA_DIR").is_some_and(|path| !path.is_empty()),
        "tests must set LOCUST_BACKUP_ROOT or LOCUST_DATA_DIR before selecting a backup root"
    );
    std::env::var_os("LOCUST_BACKUP_ROOT")
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| locust_core::config::AppConfig::config_dir().join("backups"))
}

pub(crate) fn format_observed_cost(amount: f64, complete: bool) -> String {
    if complete {
        format!("${amount:.4}")
    } else if amount > 0.0 {
        format!(">= ${amount:.4} (partial; total unknown)")
    } else {
        "Unknown (not reported)".into()
    }
}

pub(crate) fn format_duration(secs: f64) -> String {
    let s = secs as u64;
    if s >= 3600 {
        format!("{}h {}m", s / 3600, (s % 3600) / 60)
    } else if s >= 60 {
        format!("{}m {}s", s / 60, s % 60)
    } else {
        format!("{}s", s)
    }
}

pub(crate) fn load_config(path: &Option<PathBuf>) -> AppConfig {
    let path = path.clone().unwrap_or_else(AppConfig::default_path);
    AppConfig::load(&path).unwrap_or_else(|e| {
        eprintln!("warning: could not load config {}: {}", path.display(), e);
        AppConfig::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{EnvVarRestore, BACKUP_ROOT_LOCK};

    #[tokio::test]
    async fn backup_root_follows_data_dir_not_temp() {
        let _lock = BACKUP_ROOT_LOCK.lock().await;
        let _backup_root_restore = EnvVarRestore::capture("LOCUST_BACKUP_ROOT");
        let _data_dir_restore = EnvVarRestore::capture("LOCUST_DATA_DIR");
        let data_dir =
            std::env::temp_dir().join(format!("locust_cli_data_{}", uuid::Uuid::new_v4()));

        std::env::remove_var("LOCUST_BACKUP_ROOT");
        std::env::set_var("LOCUST_DATA_DIR", &data_dir);

        let expected = data_dir.join("backups");
        assert_eq!(locust_backup_root(), expected);
        assert_ne!(
            locust_backup_root(),
            std::env::temp_dir().join("locust_bak")
        );

        std::env::set_var("LOCUST_BACKUP_ROOT", "");
        assert_eq!(locust_backup_root(), expected);

        let override_root =
            std::env::temp_dir().join(format!("locust_cli_backup_{}", uuid::Uuid::new_v4()));
        std::env::set_var("LOCUST_BACKUP_ROOT", &override_root);
        assert_eq!(locust_backup_root(), override_root);
    }
}
