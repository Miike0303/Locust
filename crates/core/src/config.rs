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
                let config: AppConfig = serde_json::from_str(&contents)?;
                Ok(config)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e.into()),
        }
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_string_pretty(self)?;
        std::fs::write(path, json)?;
        Ok(())
    }

    pub fn config_dir() -> PathBuf {
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

    pub fn add_recent_project(&mut self, path: PathBuf, name: String, format_id: String) {
        self.recent_projects.retain(|p| p.path != path);
        self.recent_projects.insert(
            0,
            RecentProject {
                path,
                name,
                format_id,
                last_opened: Utc::now(),
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
    pub path: PathBuf,
    pub name: String,
    pub format_id: String,
    pub last_opened: DateTime<Utc>,
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
        let mut cfg = AppConfig::default();
        cfg.default_source_lang = "ko".to_string();
        cfg.default_batch_size = 20;
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
        cfg.add_recent_project(PathBuf::from("/a"), "A".into(), "json".into());
        cfg.add_recent_project(PathBuf::from("/b"), "B".into(), "json".into());
        cfg.add_recent_project(PathBuf::from("/c"), "C".into(), "json".into());
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
            );
        }
        assert_eq!(cfg.recent_projects.len(), 10);
    }

    #[test]
    fn test_recent_projects_deduplication() {
        let mut cfg = AppConfig::default();
        cfg.add_recent_project(PathBuf::from("/same"), "First".into(), "json".into());
        cfg.add_recent_project(PathBuf::from("/same"), "Second".into(), "json".into());
        assert_eq!(cfg.recent_projects.len(), 1);
        assert_eq!(cfg.recent_projects[0].name, "Second");
    }

    #[test]
    fn test_config_dir_is_absolute() {
        let dir = AppConfig::config_dir();
        assert!(dir.is_absolute());
    }

    #[test]
    fn test_save_creates_parent_dirs() {
        let tmp = tempdir();
        let path = tmp.join("deep").join("nested").join("dir").join("config.json");
        let cfg = AppConfig::default();
        cfg.save(&path).unwrap();
        assert!(path.exists());
    }
}
