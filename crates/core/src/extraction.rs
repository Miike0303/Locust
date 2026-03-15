use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::{LocustError, Result};
use crate::models::{OutputMode, StringEntry};

pub trait FormatPlugin: Send + Sync {
    fn id(&self) -> &str;
    fn name(&self) -> &str;
    fn description(&self) -> &str {
        ""
    }
    fn supported_extensions(&self) -> &[&str];
    fn supported_modes(&self) -> Vec<OutputMode> {
        vec![OutputMode::Replace]
    }

    fn detect(&self, path: &Path) -> bool {
        if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
            let ext_lower = ext.to_lowercase();
            self.supported_extensions()
                .iter()
                .any(|supported| {
                    let s = supported.strip_prefix('.').unwrap_or(supported);
                    s.to_lowercase() == ext_lower
                })
        } else {
            false
        }
    }

    fn extract(&self, path: &Path) -> Result<Vec<StringEntry>>;

    fn inject(&self, path: &Path, entries: &[StringEntry]) -> Result<InjectionReport>;

    fn inject_add(
        &self,
        _path: &Path,
        _lang: &str,
        _entries: &[StringEntry],
    ) -> Result<InjectionReport> {
        Err(LocustError::UnsupportedFormat(format!(
            "{} does not support Add mode",
            self.name()
        )))
    }
}

#[derive(Debug, Serialize)]
pub struct InjectionReport {
    pub files_modified: usize,
    pub strings_written: usize,
    pub strings_skipped: usize,
    pub warnings: Vec<String>,
}

pub struct FormatRegistry {
    plugins: Vec<Box<dyn FormatPlugin>>,
}

impl FormatRegistry {
    pub fn new() -> Self {
        Self {
            plugins: Vec::new(),
        }
    }

    pub fn register(&mut self, plugin: Box<dyn FormatPlugin>) {
        self.plugins.push(plugin);
    }

    pub fn detect(&self, path: &Path) -> Option<&dyn FormatPlugin> {
        self.plugins.iter().find(|p| p.detect(path)).map(|p| p.as_ref())
    }

    pub fn get(&self, id: &str) -> Option<&dyn FormatPlugin> {
        self.plugins
            .iter()
            .find(|p| p.id() == id)
            .map(|p| p.as_ref())
    }

    pub fn list(&self) -> Vec<PluginInfo> {
        self.plugins
            .iter()
            .map(|p| PluginInfo {
                id: p.id().to_string(),
                name: p.name().to_string(),
                description: p.description().to_string(),
                extensions: p
                    .supported_extensions()
                    .iter()
                    .map(|e| e.to_string())
                    .collect(),
                supported_modes: p.supported_modes(),
            })
            .collect()
    }
}

impl Default for FormatRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct PluginInfo {
    pub id: String,
    pub name: String,
    pub description: String,
    pub extensions: Vec<String>,
    pub supported_modes: Vec<OutputMode>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    struct MockFormatPlugin;

    impl FormatPlugin for MockFormatPlugin {
        fn id(&self) -> &str {
            "mock"
        }
        fn name(&self) -> &str {
            "Mock Format"
        }
        fn supported_extensions(&self) -> &[&str] {
            &[".mock"]
        }
        fn supported_modes(&self) -> Vec<OutputMode> {
            vec![OutputMode::Replace, OutputMode::Add]
        }

        fn extract(&self, _path: &Path) -> Result<Vec<StringEntry>> {
            let entries = vec![
                StringEntry::new("mock#0", "Hello", PathBuf::from("game.mock")),
                StringEntry::new("mock#1", "World", PathBuf::from("game.mock")),
                StringEntry::new("mock#2", "Test", PathBuf::from("game.mock")),
            ];
            Ok(entries)
        }

        fn inject(&self, path: &Path, entries: &[StringEntry]) -> Result<InjectionReport> {
            let out_path = path.with_extension("injected");
            let mut lines = Vec::new();
            let mut written = 0;
            let mut skipped = 0;
            for entry in entries {
                if let Some(ref t) = entry.translation {
                    lines.push(format!("{}={}", entry.id, t));
                    written += 1;
                } else {
                    skipped += 1;
                }
            }
            fs::write(&out_path, lines.join("\n"))?;
            Ok(InjectionReport {
                files_modified: 1,
                strings_written: written,
                strings_skipped: skipped,
                warnings: Vec::new(),
            })
        }

        fn inject_add(
            &self,
            path: &Path,
            lang: &str,
            entries: &[StringEntry],
        ) -> Result<InjectionReport> {
            let lang_dir = path.join("tl").join(lang);
            fs::create_dir_all(&lang_dir)?;
            let out_path = lang_dir.join("mock.txt");
            let mut lines = Vec::new();
            let mut written = 0;
            let mut skipped = 0;
            for entry in entries {
                if let Some(ref t) = entry.translation {
                    lines.push(format!("{}={}", entry.id, t));
                    written += 1;
                } else {
                    skipped += 1;
                }
            }
            fs::write(&out_path, lines.join("\n"))?;
            Ok(InjectionReport {
                files_modified: 1,
                strings_written: written,
                strings_skipped: skipped,
                warnings: Vec::new(),
            })
        }
    }

    struct MockFormatPlugin2;

    impl FormatPlugin for MockFormatPlugin2 {
        fn id(&self) -> &str {
            "mock2"
        }
        fn name(&self) -> &str {
            "Mock Format 2"
        }
        fn supported_extensions(&self) -> &[&str] {
            &[".mock"]
        }
        fn extract(&self, _path: &Path) -> Result<Vec<StringEntry>> {
            Ok(vec![])
        }
        fn inject(&self, _path: &Path, _entries: &[StringEntry]) -> Result<InjectionReport> {
            Ok(InjectionReport {
                files_modified: 0,
                strings_written: 0,
                strings_skipped: 0,
                warnings: Vec::new(),
            })
        }
    }

    fn make_registry() -> FormatRegistry {
        let mut reg = FormatRegistry::new();
        reg.register(Box::new(MockFormatPlugin));
        reg
    }

    #[test]
    fn test_registry_detect_by_extension() {
        let reg = make_registry();
        assert!(reg.detect(Path::new("game.mock")).is_some());
    }

    #[test]
    fn test_registry_detect_case_insensitive() {
        let reg = make_registry();
        assert!(reg.detect(Path::new("game.MOCK")).is_some());
    }

    #[test]
    fn test_registry_unknown_extension() {
        let reg = make_registry();
        assert!(reg.detect(Path::new("game.xyz")).is_none());
    }

    #[test]
    fn test_registry_get_by_id() {
        let reg = make_registry();
        assert!(reg.get("mock").is_some());
        assert_eq!(reg.get("mock").unwrap().id(), "mock");
    }

    #[test]
    fn test_registry_list() {
        let reg = make_registry();
        let list = reg.list();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, "mock");
    }

    #[test]
    fn test_mock_extract_returns_3_entries() {
        let tmp = tempdir();
        let file = tmp.join("game.mock");
        fs::write(&file, "").unwrap();
        let plugin = MockFormatPlugin;
        let entries = plugin.extract(&file).unwrap();
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].id, "mock#0");
        assert_eq!(entries[1].source, "World");
    }

    #[test]
    fn test_inject_replace_roundtrip() {
        let tmp = tempdir();
        let file = tmp.join("game.mock");
        fs::write(&file, "").unwrap();
        let plugin = MockFormatPlugin;
        let mut entries = plugin.extract(&file).unwrap();
        entries[0].translation = Some("Hola".to_string());
        entries[1].translation = Some("Mundo".to_string());
        entries[2].translation = Some("Prueba".to_string());
        plugin.inject(&file, &entries).unwrap();
        let injected = fs::read_to_string(file.with_extension("injected")).unwrap();
        assert!(injected.contains("mock#0=Hola"));
        assert!(injected.contains("mock#1=Mundo"));
        assert!(injected.contains("mock#2=Prueba"));
    }

    #[test]
    fn test_inject_add_creates_lang_dir() {
        let tmp = tempdir();
        let plugin = MockFormatPlugin;
        let mut entries = plugin.extract(&tmp).unwrap();
        entries[0].translation = Some("Hola".to_string());
        plugin.inject_add(&tmp, "es", &entries).unwrap();
        let lang_file = tmp.join("tl").join("es").join("mock.txt");
        assert!(lang_file.exists());
    }

    #[test]
    fn test_inject_report_counts() {
        let tmp = tempdir();
        let file = tmp.join("game.mock");
        fs::write(&file, "").unwrap();
        let plugin = MockFormatPlugin;
        let mut entries = plugin.extract(&file).unwrap();
        entries[0].translation = Some("Hola".to_string());
        entries[1].translation = Some("Mundo".to_string());
        // entries[2] has no translation
        let report = plugin.inject(&file, &entries).unwrap();
        assert_eq!(report.strings_written, 2);
        assert_eq!(report.strings_skipped, 1);
    }

    #[test]
    fn test_detect_prefers_first_registered() {
        let mut reg = FormatRegistry::new();
        reg.register(Box::new(MockFormatPlugin));
        reg.register(Box::new(MockFormatPlugin2));
        let detected = reg.detect(Path::new("game.mock")).unwrap();
        assert_eq!(detected.id(), "mock");
    }

    fn tempdir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("locust_test_{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }
}
