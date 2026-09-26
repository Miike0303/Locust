use std::collections::HashMap;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::error::Result;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AppConfig {
    #[serde(default)]
    pub providers: HashMap<String, ProviderConfig>,
    #[serde(default)]
    pub default_provider: Option<String>,
    #[serde(default = "default_source_lang")]
    pub default_source_lang: String,
    #[serde(default = "default_target_lang")]
    pub default_target_lang: String,
    #[serde(default = "default_batch_size")]
    pub default_batch_size: usize,
    #[serde(default)]
    pub default_cost_limit: Option<f64>,
    #[serde(default)]
    pub ui: UiConfig,
    #[serde(default)]
    pub recent_projects: Vec<RecentProject>,
}

fn default_source_lang() -> String {
    "ja".to_string()
}
fn default_target_lang() -> String {
    "en".to_string()
}
fn default_batch_size() -> usize {
    40
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            providers: HashMap::new(),
            default_provider: None,
            default_source_lang: "ja".to_string(),
            default_target_lang: "en".to_string(),
            default_batch_size: 40,
            default_cost_limit: None,
            ui: UiConfig::default(),
            recent_projects: Vec::new(),
        }
    }
}

impl AppConfig {
    pub fn load(path: &Path) -> Result<Self> {
        match std::fs::read_to_string(path) {
            Ok(contents) => {
                let value: serde_json::Value = serde_json::from_str(&contents)?;
                if !value.is_object() {
                    return Err(anyhow::anyhow!("configuration must be a JSON object").into());
                }
                let config: AppConfig = serde_json::from_value(value)?;
                Ok(config)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e.into()),
        }
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        use std::io::Write;
        let json = serde_json::to_vec_pretty(self)?;
        let parent = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        std::fs::create_dir_all(parent)?;
        let parent = parent.canonicalize()?;
        let name = path.file_name().ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "configuration path has no filename",
            )
        })?;
        crate::patch::zipsec::ensure_no_links(&parent, Path::new(name))?;
        let target = parent.join(name);
        let permissions = match std::fs::symlink_metadata(&target) {
            Ok(meta) if !meta.is_file() || meta.permissions().readonly() => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "configuration destination is not a writable regular file",
                )
                .into());
            }
            Ok(meta) => {
                // Defaults loaded after a read failure must never overwrite a
                // damaged file that may still contain recoverable credentials.
                Self::load(&target).map_err(|_| {
                    std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "existing configuration is unreadable or invalid; repair it before saving",
                    )
                })?;
                Some(meta.permissions())
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(e.into()),
        };
        let stage = crate::patch::stream::StagingDir::create_prepared(&parent)?;
        let mut file = stage.create_file("config.json")?;
        if let Some(permissions) = permissions {
            file.set_permissions(permissions)?;
        } else {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
            }
        }
        file.write_all(&json)?;
        file.sync_all()?;
        drop(file);
        // Keep the old generation readable until the complete new one replaces
        // it. Do not truncate credentials in place or rename the old file away.
        std::fs::rename(stage.child("config.json"), target)?;
        Ok(())
    }

    /// Public UI representation. Never echo stored API keys in save responses.
    pub fn redacted_json(&self) -> serde_json::Value {
        let mut value = serde_json::to_value(self).expect("AppConfig is serializable");
        if let Some(providers) = value.get_mut("providers").and_then(|v| v.as_object_mut()) {
            for provider in providers.values_mut() {
                if provider
                    .get("api_key")
                    .and_then(|v| v.as_str())
                    .is_some_and(|s| !s.is_empty())
                {
                    provider["api_key"] = "***".into();
                }
            }
        }
        value
    }

    /// Merge a UI partial without turning a redacted credential into a new key.
    /// Omitted provider fields stay intact; null/empty API keys explicitly clear.
    pub fn patched(&self, mut partial: serde_json::Value) -> Result<Self> {
        if !partial.is_object() {
            return Err(anyhow::anyhow!("configuration update must be an object").into());
        }
        if let Some(providers) = partial.get_mut("providers").and_then(|v| v.as_object_mut()) {
            for provider in providers.values_mut() {
                if provider.get("api_key").and_then(|v| v.as_str()) == Some("***") {
                    if let Some(fields) = provider.as_object_mut() {
                        fields.remove("api_key");
                    }
                }
            }
        }
        fn merge(current: &mut serde_json::Value, partial: serde_json::Value) {
            if let (Some(current), Some(partial)) = (current.as_object_mut(), partial.as_object()) {
                for (key, value) in partial {
                    merge(
                        current
                            .entry(key.clone())
                            .or_insert(serde_json::Value::Null),
                        value.clone(),
                    );
                }
            } else {
                *current = partial;
            }
        }
        let mut value = serde_json::to_value(self)?;
        merge(&mut value, partial);
        Ok(serde_json::from_value(value)?)
    }

    /// Persistent state root. A nonempty LOCUST_DATA_DIR selects an isolated
    /// profile for portable installs, automated checks, and parallel instances.
    pub fn config_dir() -> PathBuf {
        if let Some(path) = std::env::var_os("LOCUST_DATA_DIR").filter(|p| !p.is_empty()) {
            return PathBuf::from(path);
        }
        #[cfg(target_os = "windows")]
        {
            dirs::data_local_dir()
                .map(|p| p.join("project-locust"))
                .unwrap_or_else(|| PathBuf::from(".project-locust"))
        }
        #[cfg(target_os = "macos")]
        {
            dirs::data_dir()
                .map(|p| p.join("project-locust"))
                .unwrap_or_else(|| PathBuf::from(".project-locust"))
        }
        #[cfg(not(any(target_os = "windows", target_os = "macos")))]
        {
            dirs::config_dir()
                .map(|p| p.join("project-locust"))
                .unwrap_or_else(|| PathBuf::from(".project-locust"))
        }
    }

    pub fn default_path() -> PathBuf {
        Self::config_dir().join("config.json")
    }

    /// Remember a project for Welcome → Recents.
    ///
    /// `path` is always the **game folder** (inject / pack root).
    /// When `database_path` is `Some`, reopening must call open-db with that
    /// file — never extract against the game path alone (pivoted projects).
    /// Dedupes by database path when set, otherwise by game path among
    /// extract-style entries only so a pivot and its source can coexist.
    pub fn add_recent_project(
        &mut self,
        path: PathBuf,
        name: String,
        format_id: String,
        database_path: Option<PathBuf>,
    ) {
        match &database_path {
            Some(db) => {
                self.recent_projects
                    .retain(|p| p.database_path.as_ref() != Some(db));
            }
            None => {
                self.recent_projects
                    .retain(|p| !(p.path == path && p.database_path.is_none()));
            }
        }
        self.recent_projects.insert(
            0,
            RecentProject {
                path,
                name,
                format_id,
                last_opened: Utc::now(),
                database_path,
            },
        );
        self.recent_projects.truncate(10);
    }

    pub fn get_provider_config(&self, provider_id: &str) -> Option<&ProviderConfig> {
        self.providers.get(provider_id)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProviderConfig {
    pub api_key: Option<String>,
    pub base_url: Option<String>,
    pub model: Option<String>,
    #[serde(default)]
    pub free_tier: bool,
    #[serde(default)]
    pub extra: HashMap<String, String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct UiConfig {
    #[serde(default = "default_theme")]
    pub theme: String,
    #[serde(default = "default_font_size")]
    pub font_size: u8,
    #[serde(default = "default_show_source")]
    pub show_source_column: bool,
    #[serde(default = "default_row_height")]
    pub table_row_height: u8,
}

fn default_theme() -> String {
    "system".to_string()
}
fn default_font_size() -> u8 {
    14
}
fn default_show_source() -> bool {
    true
}
fn default_row_height() -> u8 {
    36
}

impl Default for UiConfig {
    fn default() -> Self {
        Self {
            theme: "system".to_string(),
            font_size: 14,
            show_source_column: true,
            table_row_height: 36,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RecentProject {
    /// Game folder (inject / pack root). Never a `.locust.db` alone.
    pub path: PathBuf,
    pub name: String,
    pub format_id: String,
    pub last_opened: DateTime<Utc>,
    /// When set, Welcome reopens via open-db (no extract/merge).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub database_path: Option<PathBuf>,
}

impl PartialEq for AppConfig {
    fn eq(&self, other: &Self) -> bool {
        self.default_source_lang == other.default_source_lang
            && self.default_target_lang == other.default_target_lang
            && self.default_batch_size == other.default_batch_size
            && self.default_cost_limit == other.default_cost_limit
            && self.default_provider == other.default_provider
            && self.ui.theme == other.ui.theme
            && self.ui.font_size == other.ui.font_size
            && self.providers.len() == other.providers.len()
            && self.recent_projects.len() == other.recent_projects.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tempdir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("locust_cfg_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn redacted_provider_updates_preserve_keys_and_other_provider_fields() {
        let config = AppConfig::default().patched(serde_json::json!({
            "providers": {
                "xai": { "api_key": "fixture-key-one", "model": "before", "free_tier": false, "extra": {"setting": "1"} },
                "second": { "api_key": "fixture-key-two", "free_tier": false }
            }
        })).unwrap();
        let mut public = config.redacted_json();
        assert!(!public.to_string().contains("fixture-key"));
        public["providers"]["xai"]["model"] = "after".into();
        let next = config
            .patched(serde_json::json!({ "providers": public["providers"] }))
            .unwrap();
        assert_eq!(
            next.providers["xai"].api_key.as_deref(),
            Some("fixture-key-one")
        );
        assert_eq!(
            next.providers["second"].api_key.as_deref(),
            Some("fixture-key-two")
        );
        let partial = next
            .patched(serde_json::json!({ "providers": {"xai": {"model": "third"}} }))
            .unwrap();
        assert_eq!(partial.providers["xai"].model.as_deref(), Some("third"));
        assert_eq!(partial.providers["xai"].extra["setting"], "1");
        assert!(partial.providers.contains_key("second"));
        let cleared = partial
            .patched(serde_json::json!({ "providers": {"xai": {"api_key": null}} }))
            .unwrap();
        assert!(cleared.providers["xai"].api_key.is_none());
        assert!(partial.patched(serde_json::json!([])).is_err());
    }

    #[test]
    fn failed_config_save_preserves_previous_bytes_and_cleans_owned_staging() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.json");
        AppConfig::default().save(&path).unwrap();
        let original = std::fs::read(&path).unwrap();
        let permissions = std::fs::metadata(&path).unwrap().permissions();
        let mut readonly = permissions.clone();
        readonly.set_readonly(true);
        std::fs::set_permissions(&path, readonly).unwrap();
        let next = AppConfig {
            default_target_lang: "es".into(),
            ..Default::default()
        };
        assert!(next.save(&path).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), original);
        std::fs::set_permissions(&path, permissions).unwrap();
        next.save(&path).unwrap();
        assert_eq!(AppConfig::load(&path).unwrap().default_target_lang, "es");
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
    }

    #[test]
    fn config_readers_always_observe_a_complete_generation() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.json");
        AppConfig::default().save(&path).unwrap();
        std::thread::scope(|scope| {
            let reader = scope.spawn(|| {
                for _ in 0..200 {
                    let bytes = std::fs::read(&path).unwrap();
                    let _: AppConfig = serde_json::from_slice(&bytes).unwrap();
                }
            });
            for count in 1..20 {
                AppConfig {
                    default_batch_size: count,
                    ..Default::default()
                }
                .save(&path)
                .unwrap();
            }
            reader.join().unwrap();
        });
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
    }

    #[test]
    fn test_default_config_fields() {
        let cfg = AppConfig::default();
        assert_eq!(cfg.default_source_lang, "ja");
        assert_eq!(cfg.default_target_lang, "en");
        assert_eq!(cfg.default_batch_size, 40);
        assert!(cfg.default_provider.is_none());
        assert_eq!(cfg.ui.theme, "system");
        assert_eq!(cfg.ui.font_size, 14);
    }

    #[test]
    fn test_save_and_load_roundtrip() {
        let tmp = tempdir();
        let path = tmp.join("config.json");
        let cfg = AppConfig {
            default_source_lang: "ko".to_string(),
            default_batch_size: 20,
            ..Default::default()
        };
        cfg.save(&path).unwrap();
        let loaded = AppConfig::load(&path).unwrap();
        assert_eq!(loaded.default_source_lang, "ko");
        assert_eq!(loaded.default_batch_size, 20);
        assert_eq!(cfg, loaded);
    }

    #[test]
    fn test_load_missing_file_returns_default() {
        let cfg = AppConfig::load(Path::new("/tmp/nonexistent_locust_config.json")).unwrap();
        assert_eq!(cfg.default_source_lang, "ja");
    }

    #[test]
    fn test_load_invalid_json_returns_error() {
        let tmp = tempdir();
        let path = tmp.join("bad.json");
        std::fs::write(&path, "not json at all!!!").unwrap();
        assert!(AppConfig::load(&path).is_err());
    }

    #[test]
    fn config_save_preserves_unreadable_or_invalid_existing_bytes() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("config.json");
        for damaged in [
            b"{\"providers\":{\"grok\":{\"api_key\":\"secret".as_slice(),
            b"\xff\xfe\x80",
            b"[]",
        ] {
            std::fs::write(&path, damaged).unwrap();
            let error = AppConfig::default().save(&path).unwrap_err().to_string();
            assert!(error.contains("repair it before saving"));
            assert!(!error.contains("secret"));
            assert_eq!(std::fs::read(&path).unwrap(), damaged);
            assert_eq!(std::fs::read_dir(tmp.path()).unwrap().count(), 1);
        }
    }

    #[test]
    fn test_provider_config_roundtrip() {
        let tmp = tempdir();
        let path = tmp.join("config.json");
        let mut cfg = AppConfig::default();
        cfg.providers.insert(
            "deepl".to_string(),
            ProviderConfig {
                api_key: Some("sk-test-123".to_string()),
                base_url: None,
                model: None,
                free_tier: true,
                extra: HashMap::new(),
            },
        );
        cfg.save(&path).unwrap();
        let loaded = AppConfig::load(&path).unwrap();
        let pc = loaded.get_provider_config("deepl").unwrap();
        assert_eq!(pc.api_key, Some("sk-test-123".to_string()));
        assert!(pc.free_tier);
    }

    #[test]
    fn test_add_recent_project() {
        let mut cfg = AppConfig::default();
        cfg.add_recent_project(PathBuf::from("/a"), "A".into(), "json".into(), None);
        cfg.add_recent_project(PathBuf::from("/b"), "B".into(), "json".into(), None);
        cfg.add_recent_project(PathBuf::from("/c"), "C".into(), "json".into(), None);
        assert_eq!(cfg.recent_projects.len(), 3);
        assert_eq!(cfg.recent_projects[0].name, "C");
        assert_eq!(cfg.recent_projects[1].name, "B");
        assert_eq!(cfg.recent_projects[2].name, "A");
    }

    #[test]
    fn test_recent_projects_max_10() {
        let mut cfg = AppConfig::default();
        for i in 0..12 {
            cfg.add_recent_project(
                PathBuf::from(format!("/proj{}", i)),
                format!("P{}", i),
                "json".into(),
                None,
            );
        }
        assert_eq!(cfg.recent_projects.len(), 10);
    }

    #[test]
    fn test_recent_projects_deduplication() {
        let mut cfg = AppConfig::default();
        cfg.add_recent_project(PathBuf::from("/same"), "First".into(), "json".into(), None);
        cfg.add_recent_project(PathBuf::from("/same"), "Second".into(), "json".into(), None);
        assert_eq!(cfg.recent_projects.len(), 1);
        assert_eq!(cfg.recent_projects[0].name, "Second");
    }

    #[test]
    fn test_recent_pivot_coexists_with_source_game() {
        let mut cfg = AppConfig::default();
        let game = PathBuf::from("/game");
        cfg.add_recent_project(game.clone(), "Game".into(), "rpgmaker-mv".into(), None);
        cfg.add_recent_project(
            game.clone(),
            "Game-pivot".into(),
            "rpgmaker-mv".into(),
            Some(PathBuf::from("/game-pivot.locust.db")),
        );
        assert_eq!(cfg.recent_projects.len(), 2);
        assert_eq!(
            cfg.recent_projects[0].database_path.as_deref(),
            Some(Path::new("/game-pivot.locust.db"))
        );
        assert!(cfg.recent_projects[1].database_path.is_none());

        // Re-adding the same pivot replaces it, source stays.
        cfg.add_recent_project(
            game,
            "Game-pivot".into(),
            "rpgmaker-mv".into(),
            Some(PathBuf::from("/game-pivot.locust.db")),
        );
        assert_eq!(cfg.recent_projects.len(), 2);
        assert_eq!(cfg.recent_projects[0].name, "Game-pivot");
    }

    #[test]
    fn test_config_dir_is_absolute() {
        let dir = AppConfig::config_dir();
        assert!(dir.is_absolute());
    }

    #[test]
    fn test_save_creates_parent_dirs() {
        let tmp = tempdir();
        let path = tmp
            .join("deep")
            .join("nested")
            .join("dir")
            .join("config.json");
        let cfg = AppConfig::default();
        cfg.save(&path).unwrap();
        assert!(path.exists());
    }
}
