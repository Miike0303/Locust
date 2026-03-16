use std::path::{Path, PathBuf};

use locust_core::error::{LocustError, Result};
use locust_core::extraction::{FormatPlugin, InjectionReport};
use locust_core::models::{OutputMode, StringEntry};

/// Plugin for Unity Engine games.
/// Scans .assets files for length-prefixed UTF-8 strings.
///
/// Unity serialization format uses 4-byte LE length prefix + UTF-8 data + padding to 4-byte alignment.
/// We scan for strings that look like natural language dialogue/UI text.
pub struct UnityPlugin;

impl UnityPlugin {
    pub fn new() -> Self {
        Self
    }

    fn has_unity_structure(path: &Path) -> bool {
        if !path.is_dir() {
            return false;
        }
        // Unity games have: GameName_Data/ with .assets files, UnityPlayer.dll, etc.
        let has_unity_dll = path.join("UnityPlayer.dll").exists()
            || path.join("UnityPlayer.so").exists()
            || path.join("UnityPlayer.dylib").exists();

        if has_unity_dll {
            return true;
        }

        // Check for *_Data directory with .assets files
        Self::find_data_dir(path).is_some()
    }

    fn find_data_dir(path: &Path) -> Option<PathBuf> {
        for entry in std::fs::read_dir(path).ok()?.flatten() {
            let p = entry.path();
            if p.is_dir() {
                let name = p.file_name()?.to_string_lossy().to_string();
                if name.ends_with("_Data") {
                    // Verify it has .assets files
                    if std::fs::read_dir(&p).ok()?.any(|e| {
                        e.ok().map_or(false, |e| {
                            e.path().extension().map_or(false, |ext| ext == "assets")
                        })
                    }) {
                        return Some(p);
                    }
                }
            }
        }
        None
    }

    fn find_assets_files(path: &Path) -> Vec<PathBuf> {
        let mut assets = Vec::new();

        // If path points to a specific .assets file
        if path.is_file() && path.extension().map_or(false, |e| e == "assets") {
            assets.push(path.to_path_buf());
            return assets;
        }

        // Find the _Data directory
        let data_dir = if path.is_dir() {
            if let Some(d) = Self::find_data_dir(path) {
                d
            } else if path.file_name().map_or(false, |n| n.to_string_lossy().ends_with("_Data")) {
                path.to_path_buf()
            } else {
                return assets;
            }
        } else {
            return assets;
        };

        // Collect .assets files (skip very small ones that are just headers)
        for entry in walkdir::WalkDir::new(&data_dir)
            .max_depth(2)
            .follow_links(false)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            let p = entry.path();
            if p.extension().map_or(false, |e| e == "assets") {
                // Skip files smaller than 100 bytes (likely just metadata)
                if let Ok(meta) = std::fs::metadata(p) {
                    if meta.len() > 100 {
                        assets.push(p.to_path_buf());
                    }
                }
            }
        }

        assets
    }

    /// Extract strings from Unity .assets files.
    /// Unity serializes strings as: [4-byte LE length] [UTF-8 data] [padding to 4-byte align]
    fn extract_strings_from_assets(
        bytes: &[u8],
        filename: &str,
        file_path: &Path,
    ) -> Vec<StringEntry> {
        let mut entries = Vec::new();
        let mut seen = std::collections::HashSet::new();
        let len = bytes.len();

        if len < 8 {
            return entries;
        }

        let mut i = 0;
        while i + 4 < len {
            // Read potential string length (4 bytes LE)
            let str_len = u32::from_le_bytes([bytes[i], bytes[i + 1], bytes[i + 2], bytes[i + 3]]) as usize;

            // Plausible string length: 5-2000 characters
            if str_len >= 5 && str_len <= 2000 && i + 4 + str_len <= len {
                // Try to decode as UTF-8
                if let Ok(text) = std::str::from_utf8(&bytes[i + 4..i + 4 + str_len]) {
                    if is_unity_translatable(text) && seen.insert(text.to_string()) {
                        let id = format!("{}#offset_{}#{}", filename, i, entries.len());
                        let mut entry = StringEntry::new(id, text, file_path.to_path_buf());
                        entry.tags = vec!["unknown".to_string()];
                        entry.metadata.insert(
                            "extraction_method".to_string(),
                            serde_json::Value::String("unity_assets".to_string()),
                        );
                        entries.push(entry);
                    }
                }
                // Skip past string + alignment padding
                let aligned = (str_len + 3) & !3;
                i += 4 + aligned;
            } else {
                i += 1;
            }
        }

        entries
    }
}

fn is_unity_translatable(text: &str) -> bool {
    let s = text.trim();
    if s.is_empty() || s.len() < 5 {
        return false;
    }

    // Must be mostly ASCII printable
    let ascii_printable = s.chars().filter(|c| c.is_ascii_graphic() || c.is_ascii_whitespace()).count();
    let total = s.chars().count();
    if (ascii_printable as f64 / total as f64) < 0.85 {
        return false;
    }

    // Must have meaningful letter content
    let letters = s.chars().filter(|c| c.is_alphabetic()).count();
    if letters < 3 {
        return false;
    }

    // Must have spaces (multi-word) for strings > 20 chars
    let has_space = s.contains(' ');
    if !has_space && s.len() > 20 {
        return false;
    }

    // Filter out paths
    if s.contains('/') && s.contains('.') && !s.contains(' ') {
        return false;
    }
    if s.contains('\\') && s.contains('.') {
        return false;
    }

    // Filter out code/identifiers
    if s.chars().all(|c| c.is_ascii_uppercase() || c == '_') {
        return false;
    }

    // Filter camelCase/PascalCase identifiers
    if !has_space {
        let transitions = s.as_bytes().windows(2)
            .filter(|w| w[0].is_ascii_lowercase() && w[1].is_ascii_uppercase())
            .count();
        if transitions >= 2 {
            return false;
        }
    }

    // Filter out URLs
    if s.starts_with("http") || s.starts_with("www.") {
        return false;
    }

    // Filter out class/namespace-like patterns
    if s.contains("::") || (s.contains('.') && !s.contains(' ')) {
        return false;
    }

    // Filter lines that look like code
    if s.contains("(){") || s.contains("};") || s.starts_with("using ") ||
       s.starts_with("import ") || s.starts_with("public ") || s.starts_with("private ") ||
       s.starts_with("static ") || s.contains(" = ") && !has_space {
        return false;
    }

    // Must have reasonable punctuation
    let punct_ratio = s.chars().filter(|c| !c.is_alphanumeric() && !c.is_whitespace()).count() as f64 / total as f64;
    if punct_ratio > 0.4 {
        return false;
    }

    true
}

impl Default for UnityPlugin {
    fn default() -> Self {
        Self::new()
    }
}

impl FormatPlugin for UnityPlugin {
    fn id(&self) -> &str {
        "unity"
    }

    fn name(&self) -> &str {
        "Unity Engine"
    }

    fn description(&self) -> &str {
        "Unity Engine games (.assets files, heuristic UTF-8 extraction)"
    }

    fn supported_extensions(&self) -> &[&str] {
        &[".assets"]
    }

    fn supported_modes(&self) -> Vec<OutputMode> {
        vec![OutputMode::Replace]
    }

    fn detect(&self, path: &Path) -> bool {
        if path.is_file() {
            return path.extension().map_or(false, |e| e == "assets");
        }
        Self::has_unity_structure(path)
    }

    fn extract(&self, path: &Path) -> Result<Vec<StringEntry>> {
        let assets = Self::find_assets_files(path);
        if assets.is_empty() {
            return Err(LocustError::ParseError {
                file: path.display().to_string(),
                message: "no .assets files found".to_string(),
            });
        }

        let mut all = Vec::new();
        for asset_file in &assets {
            let bytes = std::fs::read(asset_file)?;
            let filename = asset_file
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            all.extend(Self::extract_strings_from_assets(&bytes, &filename, asset_file));
        }

        Ok(all)
    }

    fn inject(&self, _path: &Path, entries: &[StringEntry]) -> Result<InjectionReport> {
        // Binary patching: find length-prefixed UTF-8 original, replace with translation
        let mut files_modified = 0;
        let mut strings_written = 0;
        let mut strings_skipped = 0;
        let mut warnings = Vec::new();

        let mut by_file: std::collections::HashMap<PathBuf, Vec<&StringEntry>> = std::collections::HashMap::new();
        for entry in entries {
            by_file.entry(entry.file_path.clone()).or_default().push(entry);
        }

        for (file_path, file_entries) in &by_file {
            if !file_path.exists() {
                continue;
            }
            let mut bytes = std::fs::read(file_path)?;
            let mut modified = false;

            for entry in file_entries {
                let translation = match &entry.translation {
                    Some(t) => t,
                    None => { strings_skipped += 1; continue; }
                };

                let orig_bytes = entry.source.as_bytes();
                let trans_bytes = translation.as_bytes();

                if trans_bytes.len() > orig_bytes.len() {
                    warnings.push(format!(
                        "translation for '{}' is longer than original ({} > {} bytes), skipping",
                        entry.id, trans_bytes.len(), orig_bytes.len()
                    ));
                    strings_skipped += 1;
                    continue;
                }

                // Find the length-prefixed string in the binary
                let orig_len_bytes = (orig_bytes.len() as u32).to_le_bytes();
                let mut needle = Vec::with_capacity(4 + orig_bytes.len());
                needle.extend_from_slice(&orig_len_bytes);
                needle.extend_from_slice(orig_bytes);

                if let Some(pos) = find_bytes_in(&bytes, &needle) {
                    // Write new length
                    let new_len = trans_bytes.len() as u32;
                    bytes[pos..pos + 4].copy_from_slice(&new_len.to_le_bytes());
                    // Write translation
                    bytes[pos + 4..pos + 4 + trans_bytes.len()].copy_from_slice(trans_bytes);
                    // Null-pad remainder
                    for b in &mut bytes[pos + 4 + trans_bytes.len()..pos + 4 + orig_bytes.len()] {
                        *b = 0;
                    }
                    strings_written += 1;
                    modified = true;
                } else {
                    strings_skipped += 1;
                }
            }

            if modified {
                std::fs::write(file_path, &bytes)?;
                files_modified += 1;
            }
        }

        Ok(InjectionReport {
            files_modified,
            strings_written,
            strings_skipped,
            warnings,
        })
    }
}

fn find_bytes_in(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || needle.len() > haystack.len() {
        return None;
    }
    haystack.windows(needle.len()).position(|w| w == needle)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn tempdir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("locust_unity_{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn create_unity_fixture(dir: &Path) -> PathBuf {
        let data_dir = dir.join("TestGame_Data");
        fs::create_dir_all(&data_dir).unwrap();

        // Create UnityPlayer.dll marker
        fs::write(dir.join("UnityPlayer.dll"), b"fake").unwrap();

        // Create a fake .assets file with length-prefixed strings
        let mut data: Vec<u8> = vec![0; 64]; // header padding

        // "Hello World" - 11 bytes
        let s1 = b"Hello World";
        data.extend_from_slice(&(s1.len() as u32).to_le_bytes());
        data.extend_from_slice(s1);
        data.push(0); // padding to align to 4 bytes (11 + 1 = 12)
        data.extend_from_slice(&[0xFF; 8]); // gap

        // "Press any key to continue" - 25 bytes
        let s2 = b"Press any key to continue";
        data.extend_from_slice(&(s2.len() as u32).to_le_bytes());
        data.extend_from_slice(s2);
        data.extend_from_slice(&[0, 0, 0]); // padding to 28
        data.extend_from_slice(&[0; 32]); // trailing

        let assets_path = data_dir.join("resources.assets");
        fs::write(&assets_path, &data).unwrap();

        dir.to_path_buf()
    }

    #[test]
    fn test_detect_unity() {
        let dir = tempdir();
        create_unity_fixture(&dir);
        let plugin = UnityPlugin::new();
        assert!(plugin.detect(&dir));
    }

    #[test]
    fn test_detect_non_unity() {
        let dir = tempdir();
        let plugin = UnityPlugin::new();
        assert!(!plugin.detect(&dir));
    }

    #[test]
    fn test_extract_strings() {
        let dir = tempdir();
        create_unity_fixture(&dir);
        let plugin = UnityPlugin::new();
        let entries = plugin.extract(&dir).unwrap();
        let sources: Vec<&str> = entries.iter().map(|e| e.source.as_str()).collect();
        assert!(sources.contains(&"Hello World"), "got: {:?}", sources);
        assert!(sources.contains(&"Press any key to continue"), "got: {:?}", sources);
    }

    #[test]
    fn test_inject_shorter_succeeds() {
        let dir = tempdir();
        create_unity_fixture(&dir);
        let plugin = UnityPlugin::new();
        let mut entries = plugin.extract(&dir).unwrap();

        for entry in &mut entries {
            if entry.source == "Hello World" {
                entry.translation = Some("Hola Mundo".to_string());
            }
        }

        let report = plugin.inject(&dir, &entries).unwrap();
        assert!(report.strings_written >= 1);
    }

    #[test]
    fn test_is_translatable() {
        assert!(is_unity_translatable("Hello World"));
        assert!(is_unity_translatable("Press any key to continue"));
        assert!(!is_unity_translatable("abc")); // too short
        assert!(!is_unity_translatable("SOME_CONSTANT_NAME"));
        assert!(!is_unity_translatable("Assets/Textures/player.png"));
        assert!(!is_unity_translatable("UnityEngine.CoreModule"));
        assert!(!is_unity_translatable("getSomeValueFromThing"));
    }
}
