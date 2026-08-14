use std::collections::HashMap;
use std::path::{Path, PathBuf};

use locust_core::error::{LocustError, Result};
use locust_core::extraction::{FormatPlugin, InjectionReport};
use locust_core::models::{OutputMode, StringEntry};

/// Minimum string length (in chars) to extract via heuristic scan
const MIN_STRING_LEN: usize = 2;

pub struct WolfRpgPlugin;

impl WolfRpgPlugin {
    pub fn new() -> Self {
        Self
    }

    fn find_data_dir(path: &Path) -> Option<PathBuf> {
        if path.is_dir() {
            let data = path.join("Data");
            if data.is_dir() {
                return Some(data);
            }
        }
        None
    }

    /// Heuristic: scan bytes for Shift-JIS encoded string regions.
    /// Shift-JIS strings in Wolf RPG are typically preceded by a 4-byte LE length
    /// or are null-terminated. We scan for contiguous runs of valid Shift-JIS bytes
    /// that decode to printable text.
    fn extract_strings_heuristic(
        bytes: &[u8],
        filename: &str,
        file_path: &Path,
    ) -> Vec<StringEntry> {
        let mut entries = Vec::new();
        let mut seen = std::collections::HashSet::new();
        let regions = find_sjis_string_regions(bytes);

        for (idx, (offset, text)) in regions.into_iter().enumerate() {
            if text.chars().count() < MIN_STRING_LEN {
                continue;
            }
            // Skip duplicates
            if !seen.insert(text.clone()) {
                continue;
            }
            // Skip strings that look like binary artifacts
            if text
                .chars()
                .all(|c| c.is_ascii_punctuation() || c.is_ascii_digit())
            {
                continue;
            }

            let id = format!("{}#offset_{}#{}", filename, offset, idx);
            let mut entry = StringEntry::new(id, &text, file_path.to_path_buf());
            entry.tags = vec!["unknown".to_string()];
            entry.metadata.insert(
                "extraction_method".to_string(),
                serde_json::Value::String("heuristic".to_string()),
            );
            // Inject pads/truncates in Shift-JIS; validate before inject.
            entry.metadata.insert(
                "binary_slot".to_string(),
                serde_json::Value::String("sjis".to_string()),
            );
            entry.metadata.insert(
                "byte_offset".to_string(),
                serde_json::Value::Number(serde_json::Number::from(offset as u64)),
            );
            entries.push(entry);
        }

        entries
    }
}

/// Find contiguous regions of valid Shift-JIS text in raw bytes.
/// Returns (byte_offset, decoded_string) pairs.
fn find_sjis_string_regions(bytes: &[u8]) -> Vec<(usize, String)> {
    let mut results = Vec::new();
    let len = bytes.len();
    let mut i = 0;

    while i < len {
        // Look for start of a Shift-JIS string region
        if is_sjis_printable_start(bytes, i) {
            let start = i;
            let mut end = i;

            // Consume valid Shift-JIS bytes
            while end < len {
                if bytes[end] == 0 {
                    break;
                }
                if is_sjis_lead_byte(bytes[end]) && end + 1 < len {
                    // Double-byte character
                    end += 2;
                } else if bytes[end] >= 0x20 && bytes[end] <= 0x7E {
                    // ASCII printable
                    end += 1;
                } else if bytes[end] >= 0xA1 && bytes[end] <= 0xDF {
                    // Half-width katakana
                    end += 1;
                } else {
                    break;
                }
            }

            if end > start {
                let region = &bytes[start..end];
                let encoding = encoding_rs::SHIFT_JIS;
                let (decoded, _, had_errors) = encoding.decode(region);
                if !had_errors {
                    let text = decoded.to_string();
                    let char_count = text.chars().count();
                    // Only keep strings with Japanese characters or meaningful ASCII
                    if char_count >= MIN_STRING_LEN && has_meaningful_content(&text) {
                        results.push((start, text));
                    }
                }
            }
            i = end.max(i + 1);
        } else {
            i += 1;
        }
    }

    results
}

fn is_sjis_lead_byte(b: u8) -> bool {
    (0x81..=0x9F).contains(&b) || (0xE0..=0xFC).contains(&b)
}

fn is_sjis_printable_start(bytes: &[u8], pos: usize) -> bool {
    let b = bytes[pos];
    // Start with a Japanese double-byte char or ASCII letter
    if is_sjis_lead_byte(b) && pos + 1 < bytes.len() {
        return true;
    }
    if b.is_ascii_alphabetic() || b == b'"' {
        return true;
    }
    false
}

fn has_meaningful_content(text: &str) -> bool {
    // Must contain at least one letter (CJK or ASCII alphabetic)
    text.chars().any(|c| {
        c.is_alphabetic()
            || ('\u{3040}'..='\u{309F}').contains(&c) // Hiragana
            || ('\u{30A0}'..='\u{30FF}').contains(&c) // Katakana
            || ('\u{4E00}'..='\u{9FFF}').contains(&c) // CJK
    })
}

impl Default for WolfRpgPlugin {
    fn default() -> Self {
        Self::new()
    }
}

impl FormatPlugin for WolfRpgPlugin {
    fn id(&self) -> &str {
        "wolf-rpg"
    }

    fn name(&self) -> &str {
        "Wolf RPG Editor"
    }

    fn description(&self) -> &str {
        "Wolf RPG Editor binary data files (.wolf)"
    }

    fn stability(&self) -> locust_core::extraction::FormatStability {
        // Phase-2 apply proven on synthetic Data/*.wolf fixture; no commercial title yet.
        locust_core::extraction::FormatStability::Experimental
    }

    fn supported_extensions(&self) -> &[&str] {
        &[".wolf"]
    }

    fn supported_modes(&self) -> Vec<OutputMode> {
        vec![OutputMode::Replace]
    }

    fn detect(&self, path: &Path) -> bool {
        if path.is_file() {
            return path.extension().is_some_and(|ext| ext == "wolf");
        }
        if path.is_dir() {
            if let Some(data_dir) = Self::find_data_dir(path) {
                return std::fs::read_dir(&data_dir)
                    .map(|entries| {
                        entries
                            .filter_map(|e| e.ok())
                            .any(|e| e.path().extension().is_some_and(|ext| ext == "wolf"))
                    })
                    .unwrap_or(false);
            }
        }
        false
    }

    fn extract(&self, path: &Path) -> Result<Vec<StringEntry>> {
        if path.is_file() {
            let bytes = std::fs::read(path)?;
            let filename = path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            return Ok(WolfRpgPlugin::extract_strings_heuristic(
                &bytes, &filename, path,
            ));
        }

        let data_dir = Self::find_data_dir(path).ok_or_else(|| LocustError::ParseError {
            file: path.display().to_string(),
            message: "could not find Data directory".to_string(),
        })?;

        let mut all = Vec::new();
        for entry in std::fs::read_dir(&data_dir)? {
            let entry = entry?;
            let fpath = entry.path();
            if fpath.extension().is_some_and(|e| e == "wolf") {
                let bytes = std::fs::read(&fpath)?;
                let fname = fpath
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string();
                all.extend(WolfRpgPlugin::extract_strings_heuristic(
                    &bytes, &fname, &fpath,
                ));
            }
        }
        Ok(all)
    }

    fn inject(&self, path: &Path, entries: &[StringEntry]) -> Result<InjectionReport> {
        // Same resilience/perf posture as Unity/Unreal: identity skip, oversize
        // skip (not hard fail), first-byte scan, capped pad/length noise.
        let mut files_modified = 0;
        let mut strings_written = 0;
        let mut strings_skipped = 0;
        let mut length_skipped = 0usize;
        let mut pad_noted = 0usize;
        let mut find_missed = 0usize;
        let mut warnings = Vec::new();
        let mut files_written: Vec<PathBuf> = Vec::new();

        // Group by file
        let mut by_file: HashMap<PathBuf, Vec<&StringEntry>> = HashMap::new();
        for entry in entries {
            by_file
                .entry(entry.file_path.clone())
                .or_default()
                .push(entry);
        }

        let data_dir = if path.is_dir() {
            Self::find_data_dir(path).unwrap_or_else(|| path.to_path_buf())
        } else {
            path.parent().unwrap_or(path).to_path_buf()
        };

        for (file_path, file_entries) in &by_file {
            let actual_path = if file_path.exists() {
                file_path.clone()
            } else {
                let fname = file_path
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string();
                data_dir.join(&fname)
            };
            if !actual_path.exists() {
                continue;
            }

            let mut bytes = std::fs::read(&actual_path)?;
            let mut modified = false;

            for entry in file_entries {
                let translation = match &entry.translation {
                    Some(t) => t,
                    None => {
                        strings_skipped += 1;
                        continue;
                    }
                };

                // Encode original and translation to Shift-JIS
                let encoding = encoding_rs::SHIFT_JIS;
                let (orig_bytes, _, orig_err) = encoding.encode(&entry.source);
                if orig_err {
                    warnings.push(format!(
                        "could not encode original '{}' to Shift-JIS",
                        entry.id
                    ));
                    strings_skipped += 1;
                    continue;
                }

                let (trans_bytes, _, trans_err) = encoding.encode(translation);
                if trans_err {
                    warnings.push(format!(
                        "could not encode translation for '{}' to Shift-JIS",
                        entry.id
                    ));
                    strings_skipped += 1;
                    continue;
                }

                // Identity: nothing to rewrite; skip the multi-MB scan.
                if trans_bytes.as_ref() == orig_bytes.as_ref() {
                    strings_skipped += 1;
                    continue;
                }

                if trans_bytes.len() > orig_bytes.len() {
                    if length_skipped < 5 {
                        warnings.push(format!(
                            "translation for '{}' longer than original in Shift-JIS ({} > {} bytes), skipping",
                            entry.id,
                            trans_bytes.len(),
                            orig_bytes.len()
                        ));
                    }
                    length_skipped += 1;
                    strings_skipped += 1;
                    continue;
                }

                // Find original bytes in file and replace
                if let Some(pos) = find_bytes(&bytes, &orig_bytes) {
                    // Write translation bytes
                    bytes[pos..pos + trans_bytes.len()].copy_from_slice(&trans_bytes);
                    // Pad remaining with null bytes
                    if trans_bytes.len() < orig_bytes.len() {
                        for b in &mut bytes[pos + trans_bytes.len()..pos + orig_bytes.len()] {
                            *b = 0;
                        }
                        if pad_noted < 5 {
                            warnings.push(format!(
                                "padded {} null bytes for '{}'",
                                orig_bytes.len() - trans_bytes.len(),
                                entry.id
                            ));
                        }
                        pad_noted += 1;
                    }
                    strings_written += 1;
                    modified = true;
                } else {
                    if find_missed < 5 {
                        warnings.push(format!(
                            "could not find original bytes for '{}' in file",
                            entry.id
                        ));
                    }
                    find_missed += 1;
                    strings_skipped += 1;
                }
            }

            if modified {
                std::fs::write(&actual_path, &bytes)?;
                files_modified += 1;
                files_written.push(actual_path);
            }
        }

        if length_skipped > 0 {
            warnings.push(format!(
                "{length_skipped} translation(s) skipped because they are longer than the \
                 original Wolf string (Shift-JIS byte length must be ≤ source). Shorten them or \
                 use a length-aware model; equal-length translations inject cleanly."
            ));
        }

        Ok(InjectionReport {
            files_modified,
            strings_written,
            strings_skipped,
            warnings,
            files_written,
        })
    }
}

/// Locate `needle` in `haystack`. Prefer scanning for the first byte before
/// full compares — Wolf data files can be multi‑MB; naive windows() over every
/// offset dominates inject the same way it did for Unity/Unreal.
fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || needle.len() > haystack.len() {
        return None;
    }
    let first = needle[0];
    let nlen = needle.len();
    let mut i = 0;
    let end = haystack.len() - nlen + 1;
    while i < end {
        if haystack[i] != first {
            i += 1;
            continue;
        }
        if &haystack[i..i + nlen] == needle {
            return Some(i);
        }
        i += 1;
    }
    None
}

/// Build a minimal test fixture: a binary blob with embedded Shift-JIS strings
pub fn build_test_fixture() -> Vec<u8> {
    let encoding = encoding_rs::SHIFT_JIS;
    let mut buf = Vec::new();

    // Some padding header bytes
    buf.extend_from_slice(&[0x00; 16]);

    // String 1: "テストデータ" (test data)
    let (s1, _, _) = encoding.encode("テストデータ");
    buf.extend_from_slice(&s1);
    buf.push(0x00); // null terminator

    // Some binary padding
    buf.extend_from_slice(&[0xFF; 8]);

    // String 2: "勇者" (hero)
    let (s2, _, _) = encoding.encode("勇者");
    buf.extend_from_slice(&s2);
    buf.push(0x00);

    // More padding
    buf.extend_from_slice(&[0x00; 8]);

    // String 3: "魔法使い" (mage)
    let (s3, _, _) = encoding.encode("魔法使い");
    buf.extend_from_slice(&s3);
    buf.push(0x00);

    // Trailing bytes
    buf.extend_from_slice(&[0x00; 16]);

    buf
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// Every test gets its OWN fixture directory. This used to build into a
    /// fixed, git-tracked path under `tests/fixtures/wolf_rpg`, which four tests
    /// then wrote and read concurrently — cargo runs tests in a binary in
    /// parallel, so one test could read the `.wolf` file while another was
    /// mid-write, intermittently failing on missing strings. It also meant the
    /// suite overwrote a tracked repo file on every run.
    fn setup_fixture() -> PathBuf {
        temp_wolf_dir()
    }

    fn temp_wolf_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("locust_wolf_{}", uuid::Uuid::new_v4()));
        let data_dir = dir.join("Data");
        fs::create_dir_all(&data_dir).unwrap();
        let bytes = build_test_fixture();
        fs::write(data_dir.join("BasicData.wolf"), &bytes).unwrap();
        dir
    }

    #[test]
    fn test_stability_is_experimental_after_phase2() {
        let plugin = WolfRpgPlugin::new();
        assert_eq!(
            plugin.stability(),
            locust_core::extraction::FormatStability::Experimental
        );
    }

    #[test]
    fn test_detect_wolf_dir() {
        let dir = setup_fixture();
        let plugin = WolfRpgPlugin::new();
        assert!(plugin.detect(&dir));
    }

    #[test]
    fn test_detect_wolf_file() {
        let dir = setup_fixture();
        let file = dir.join("Data").join("BasicData.wolf");
        let plugin = WolfRpgPlugin::new();
        assert!(plugin.detect(&file));
    }

    #[test]
    fn test_detect_non_wolf() {
        let dir = std::env::temp_dir().join(format!("locust_notwolf_{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        let plugin = WolfRpgPlugin::new();
        assert!(!plugin.detect(&dir));
    }

    #[test]
    fn test_extract_heuristic_finds_strings() {
        let dir = setup_fixture();
        let plugin = WolfRpgPlugin::new();
        let entries = plugin.extract(&dir).unwrap();

        let sources: Vec<&str> = entries.iter().map(|e| e.source.as_str()).collect();
        assert!(
            sources.contains(&"テストデータ"),
            "expected テストデータ in {:?}",
            sources
        );
        assert!(sources.contains(&"勇者"), "expected 勇者 in {:?}", sources);
        assert!(
            sources.contains(&"魔法使い"),
            "expected 魔法使い in {:?}",
            sources
        );
    }

    #[test]
    fn test_inject_shorter_string() {
        let dir = temp_wolf_dir();
        let file_path = dir.join("Data").join("BasicData.wolf");
        let original_len = fs::metadata(&file_path).unwrap().len();

        let plugin = WolfRpgPlugin::new();
        let mut entries = plugin.extract(&dir).unwrap();

        // Translate "テストデータ" (6 chars) to "テスト" (3 chars) — shorter
        for entry in &mut entries {
            if entry.source == "テストデータ" {
                entry.translation = Some("テスト".to_string());
            }
        }

        let report = plugin.inject(&dir, &entries).unwrap();
        assert!(report.strings_written >= 1);

        // File size should be unchanged (padded with nulls)
        let new_len = fs::metadata(&file_path).unwrap().len();
        assert_eq!(original_len, new_len);
    }

    #[test]
    fn test_inject_longer_string_skips_not_hard_fail() {
        let dir = temp_wolf_dir();
        let plugin = WolfRpgPlugin::new();
        let mut entries = plugin.extract(&dir).unwrap();

        // Translate "勇者" (2 chars, 4 bytes SJIS) to something much longer
        for entry in &mut entries {
            if entry.source == "勇者" {
                entry.translation =
                    Some("この文字列は元の文字列よりもはるかに長いです".to_string());
            }
        }

        let report = plugin.inject(&dir, &entries).unwrap();
        assert!(report.strings_skipped >= 1);
        assert!(
            report
                .warnings
                .iter()
                .any(|w| w.contains("skipped because they are longer")),
            "expected length-skip summary, got: {:?}",
            report.warnings
        );
    }

    #[test]
    fn test_inject_identity_skips_write() {
        let dir = temp_wolf_dir();
        let plugin = WolfRpgPlugin::new();
        let mut entries = plugin.extract(&dir).unwrap();
        for entry in &mut entries {
            entry.translation = Some(entry.source.clone());
        }
        let report = plugin.inject(&dir, &entries).unwrap();
        assert_eq!(report.files_modified, 0);
        assert_eq!(report.strings_written, 0);
        assert!(report.strings_skipped >= 1);
    }

    #[test]
    fn test_inject_report_has_warning_on_truncation() {
        let dir = temp_wolf_dir();
        let plugin = WolfRpgPlugin::new();
        let mut entries = plugin.extract(&dir).unwrap();

        // Translate to shorter — should produce padding warning
        for entry in &mut entries {
            if entry.source == "テストデータ" {
                entry.translation = Some("テスト".to_string());
            }
        }

        let report = plugin.inject(&dir, &entries).unwrap();
        assert!(
            !report.warnings.is_empty(),
            "expected warnings for null padding"
        );
    }

    #[test]
    fn test_heuristic_metadata() {
        let dir = setup_fixture();
        let plugin = WolfRpgPlugin::new();
        let entries = plugin.extract(&dir).unwrap();

        for entry in &entries {
            let method = entry.metadata.get("extraction_method");
            assert_eq!(
                method,
                Some(&serde_json::Value::String("heuristic".to_string())),
                "entry {} missing heuristic metadata",
                entry.id
            );
            assert_eq!(
                entry.metadata.get("binary_slot"),
                Some(&serde_json::Value::String("sjis".to_string())),
                "entry {} missing binary_slot for validate/inject preflight",
                entry.id
            );
        }
    }
}
