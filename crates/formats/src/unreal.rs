use std::path::{Path, PathBuf};

use locust_core::error::{LocustError, Result};
use locust_core::extraction::{FormatPlugin, InjectionReport};
use locust_core::models::{OutputMode, StringEntry};

/// Plugin for Unreal Engine games.
/// Scans .pak files and loose localization files for translatable strings.
///
/// Unreal stores localization in:
///   Content/Localization/{target}/{culture}/{target}.locres (binary)
///   Content/Localization/{target}/{culture}/{target}.po (text PO files — if present)
///   .pak files contain packed assets (we scan for embedded UTF-16LE strings)
pub struct UnrealPlugin;

impl UnrealPlugin {
    pub fn new() -> Self {
        Self
    }

    fn find_pak_files(path: &Path) -> Vec<PathBuf> {
        let mut paks = Vec::new();
        if path.is_file() && path.extension().map_or(false, |e| e == "pak") {
            paks.push(path.to_path_buf());
            return paks;
        }
        if path.is_dir() {
            for entry in walkdir::WalkDir::new(path)
                .max_depth(5)
                .follow_links(false)
                .into_iter()
                .filter_map(|e| e.ok())
            {
                if entry.path().extension().map_or(false, |e| e == "pak") {
                    paks.push(entry.path().to_path_buf());
                }
            }
        }
        paks
    }

    fn has_unreal_structure(path: &Path) -> bool {
        if !path.is_dir() {
            return false;
        }
        // Check for typical Unreal folder structure
        let has_engine = path.join("Engine").is_dir();
        let game_name = path
            .read_dir()
            .ok()
            .and_then(|mut d| d.find(|e| {
                e.as_ref().ok().map_or(false, |e| {
                    e.path().is_dir()
                        && e.path().join("Content").is_dir()
                })
            }))
            .is_some();
        let has_content_paks = Self::find_pak_files(path).len() > 0;

        has_engine || game_name || has_content_paks
    }

    fn find_content_dir(path: &Path) -> Option<PathBuf> {
        // Look for */Content/ directory
        for entry in std::fs::read_dir(path).ok()?.flatten() {
            let content = entry.path().join("Content");
            if content.is_dir() {
                return Some(content);
            }
        }
        None
    }

    /// Extract UTF-16LE strings from PAK file using heuristic scanning.
    /// Unreal PAK format: magic 0xE1 12 6F 5A at end of file, entries packed.
    /// We scan for consecutive UTF-16LE character sequences.
    fn extract_strings_from_pak(
        bytes: &[u8],
        filename: &str,
        file_path: &Path,
    ) -> Vec<StringEntry> {
        let mut entries = Vec::new();
        let mut seen = std::collections::HashSet::new();
        let regions = find_utf16le_strings(bytes);

        for (idx, (offset, text)) in regions.into_iter().enumerate() {
            if text.chars().count() < 3 {
                continue;
            }
            if !seen.insert(text.clone()) {
                continue;
            }
            // Filter out paths, code-like strings, and binary artifacts
            if text.contains('/') && text.contains('.') {
                continue; // Likely asset path
            }
            if text.chars().all(|c| c.is_ascii_uppercase() || c == '_') {
                continue; // Likely enum/constant
            }
            if !has_natural_language(&text) {
                continue;
            }

            let id = format!("{}#offset_{}#{}", filename, offset, idx);
            let mut entry = StringEntry::new(id, &text, file_path.to_path_buf());
            entry.tags = vec!["unknown".to_string()];
            entry.metadata.insert(
                "extraction_method".to_string(),
                serde_json::Value::String("heuristic_utf16".to_string()),
            );
            entries.push(entry);
        }

        entries
    }
}

/// Find UTF-16LE string regions in binary data.
/// Returns (byte_offset, decoded_string).
fn find_utf16le_strings(bytes: &[u8]) -> Vec<(usize, String)> {
    let mut results = Vec::new();
    let len = bytes.len();
    if len < 2 {
        return results;
    }

    let mut i = 0;
    while i + 1 < len {
        // Look for start of UTF-16LE text (printable ASCII range or common Unicode)
        let lo = bytes[i];
        let hi = bytes[i + 1];

        if hi == 0 && lo >= 0x20 && lo <= 0x7E {
            // Potential UTF-16LE ASCII start
            let start = i;
            let mut chars = Vec::new();

            while i + 1 < len {
                let lo = bytes[i];
                let hi = bytes[i + 1];

                if hi == 0 && lo >= 0x20 && lo <= 0x7E {
                    chars.push(lo as char);
                    i += 2;
                } else if hi == 0 && lo == 0 {
                    // Null terminator
                    break;
                } else if hi > 0 && hi < 0xD8 {
                    // Higher Unicode (CJK, etc.)
                    let codepoint = (hi as u16) << 8 | lo as u16;
                    if let Some(ch) = char::from_u32(codepoint as u32) {
                        if ch.is_alphanumeric() || ch.is_whitespace() || ".,!?;:'\"()-".contains(ch) {
                            chars.push(ch);
                            i += 2;
                            continue;
                        }
                    }
                    break;
                } else {
                    break;
                }
            }

            if chars.len() >= 3 {
                let text: String = chars.into_iter().collect();
                results.push((start, text));
            }
        } else {
            i += 2;
        }
    }

    results
}

fn has_natural_language(text: &str) -> bool {
    // Must contain at least one space or be a meaningful word
    let has_space = text.contains(' ');
    let has_letters = text.chars().filter(|c| c.is_alphabetic()).count() >= 2;
    let mostly_printable = text.chars().all(|c| c.is_alphanumeric() || c.is_whitespace() || ".,!?;:'\"()-@#$%&*+=".contains(c));

    has_letters && mostly_printable && (has_space || text.len() <= 30)
}

impl Default for UnrealPlugin {
    fn default() -> Self {
        Self::new()
    }
}

impl FormatPlugin for UnrealPlugin {
    fn id(&self) -> &str {
        "unreal"
    }

    fn name(&self) -> &str {
        "Unreal Engine"
    }

    fn description(&self) -> &str {
        "Unreal Engine games (.pak files, heuristic UTF-16LE extraction)"
    }

    fn supported_extensions(&self) -> &[&str] {
        &[".pak"]
    }

    fn supported_modes(&self) -> Vec<OutputMode> {
        vec![OutputMode::Replace]
    }

    fn detect(&self, path: &Path) -> bool {
        if path.is_file() {
            return path.extension().map_or(false, |e| e == "pak");
        }
        Self::has_unreal_structure(path)
    }

    fn extract(&self, path: &Path) -> Result<Vec<StringEntry>> {
        let paks = Self::find_pak_files(path);
        if paks.is_empty() {
            return Err(LocustError::ParseError {
                file: path.display().to_string(),
                message: "no .pak files found".to_string(),
            });
        }

        let mut all = Vec::new();
        for pak in &paks {
            let bytes = std::fs::read(pak)?;
            let filename = pak
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            all.extend(Self::extract_strings_from_pak(&bytes, &filename, pak));
        }

        Ok(all)
    }

    fn inject(&self, path: &Path, entries: &[StringEntry]) -> Result<InjectionReport> {
        // Binary patching: find UTF-16LE original, replace with UTF-16LE translation
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

                let orig_utf16: Vec<u8> = entry.source.encode_utf16().flat_map(|c| c.to_le_bytes()).collect();
                let trans_utf16: Vec<u8> = translation.encode_utf16().flat_map(|c| c.to_le_bytes()).collect();

                if trans_utf16.len() > orig_utf16.len() {
                    return Err(LocustError::InjectionError(format!(
                        "translation for '{}' is longer than original in UTF-16LE ({} > {} bytes)",
                        entry.id, trans_utf16.len(), orig_utf16.len()
                    )));
                }

                if let Some(pos) = find_bytes_in(&bytes, &orig_utf16) {
                    bytes[pos..pos + trans_utf16.len()].copy_from_slice(&trans_utf16);
                    for b in &mut bytes[pos + trans_utf16.len()..pos + orig_utf16.len()] {
                        *b = 0;
                    }
                    strings_written += 1;
                    modified = true;
                    if trans_utf16.len() < orig_utf16.len() {
                        warnings.push(format!("padded {} null bytes for '{}'", orig_utf16.len() - trans_utf16.len(), entry.id));
                    }
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
        let dir = std::env::temp_dir().join(format!("locust_ue_{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn create_pak_fixture(dir: &Path) -> PathBuf {
        let game_dir = dir.join("TestGame").join("Content").join("Paks");
        fs::create_dir_all(&game_dir).unwrap();

        // Create a fake PAK with embedded UTF-16LE strings
        let mut data: Vec<u8> = vec![0; 32]; // padding
        // "Hello World" in UTF-16LE
        for ch in "Hello World".encode_utf16() {
            data.extend_from_slice(&ch.to_le_bytes());
        }
        data.extend_from_slice(&[0, 0]); // null terminator
        data.extend_from_slice(&[0xFF; 16]); // padding
        // "Press Start" in UTF-16LE
        for ch in "Press Start".encode_utf16() {
            data.extend_from_slice(&ch.to_le_bytes());
        }
        data.extend_from_slice(&[0, 0]);
        data.extend_from_slice(&[0; 32]); // trailing

        let pak_path = game_dir.join("TestGame.pak");
        fs::write(&pak_path, &data).unwrap();

        dir.to_path_buf()
    }

    #[test]
    fn test_detect_unreal() {
        let dir = tempdir();
        create_pak_fixture(&dir);
        let plugin = UnrealPlugin::new();
        assert!(plugin.detect(&dir));
    }

    #[test]
    fn test_detect_non_unreal() {
        let dir = tempdir();
        let plugin = UnrealPlugin::new();
        assert!(!plugin.detect(&dir));
    }

    #[test]
    fn test_extract_utf16le_strings() {
        let dir = tempdir();
        create_pak_fixture(&dir);
        let plugin = UnrealPlugin::new();
        let entries = plugin.extract(&dir).unwrap();
        let sources: Vec<&str> = entries.iter().map(|e| e.source.as_str()).collect();
        assert!(sources.contains(&"Hello World"), "got: {:?}", sources);
        assert!(sources.contains(&"Press Start"), "got: {:?}", sources);
    }

    #[test]
    fn test_inject_shorter_succeeds() {
        let dir = tempdir();
        create_pak_fixture(&dir);
        let plugin = UnrealPlugin::new();
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
    fn test_inject_longer_fails() {
        let dir = tempdir();
        create_pak_fixture(&dir);
        let plugin = UnrealPlugin::new();
        let mut entries = plugin.extract(&dir).unwrap();

        for entry in &mut entries {
            if entry.source == "Hello World" {
                entry.translation = Some("This is a much longer translation that exceeds the original".to_string());
            }
        }

        let result = plugin.inject(&dir, &entries);
        assert!(matches!(result, Err(LocustError::InjectionError(_))));
    }
}
