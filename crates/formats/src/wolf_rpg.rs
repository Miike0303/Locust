use std::collections::HashMap;
use std::path::{Path, PathBuf};

use locust_core::error::{LocustError, Result};
use locust_core::extraction::{FormatPlugin, InjectionReport};
use locust_core::models::{OutputMode, StringEntry};

use crate::discovery::find_capital_data_dir;

/// Minimum string length (in chars) to extract via heuristic scan
const MIN_STRING_LEN: usize = 2;

pub struct WolfRpgPlugin;

impl WolfRpgPlugin {
    pub fn new() -> Self {
        Self
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
        let regions = find_sjis_string_regions(bytes);

        for SjisStringRegion {
            offset,
            text,
            legacy_index,
        } in regions
        {
            if text.chars().count() < MIN_STRING_LEN {
                continue;
            }
            // Skip strings that look like binary artifacts
            if text
                .chars()
                .all(|c| c.is_ascii_punctuation() || c.is_ascii_digit())
            {
                continue;
            }

            // New kana-only rows never consume or collide with numeric legacy indices.
            let suffix = legacy_index.map_or_else(|| "k".to_string(), |idx| idx.to_string());
            let id = format!("{}#offset_{}#{}", filename, offset, suffix);
            let mut entry =
                StringEntry::new(id, &text, file_path.to_path_buf()).with_format_id("wolf-rpg");
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

struct SjisStringRegion {
    offset: usize,
    text: String,
    legacy_index: Option<usize>,
}

/// Recover complete text runs without changing the enumeration of legacy rows.
/// Keep the old scan as the source of numeric indices: scanning only with the new
/// boundaries would renumber later rows when a kana-only string becomes visible.
fn find_sjis_string_regions(bytes: &[u8]) -> Vec<SjisStringRegion> {
    let mut results: Vec<_> = find_legacy_sjis_string_regions(bytes)
        .into_iter()
        .enumerate()
        .map(|(idx, (offset, text))| SjisStringRegion {
            offset,
            text,
            legacy_index: Some(idx),
        })
        .collect();
    let legacy_count = results.len();
    let mut legacy_pos = 0;
    let mut pos = 0;

    while pos < bytes.len() {
        let start = pos;
        // Scan forward from boundaries so ASCII/kana-valued trail bytes are
        // always consumed with their lead, never mistaken for a new start.
        while let Some(width) = sjis_text_char_len(bytes, pos) {
            pos += width;
        }
        if pos == start {
            pos += 1;
            continue;
        }

        let mut has_legacy_row = false;
        while legacy_pos < legacy_count && results[legacy_pos].offset < pos {
            let region = &mut results[legacy_pos];
            if region.offset >= start {
                has_legacy_row = true;
                let (prefix, _, had_errors) =
                    encoding_rs::SHIFT_JIS.decode(&bytes[start..region.offset]);
                if !had_errors {
                    region.text.insert_str(0, &prefix);
                    // A recovered prefix intentionally replaces the old, truncated
                    // row's offset/id on reopen, but retains its numeric index.
                    region.offset = start;
                }
            }
            legacy_pos += 1;
        }

        if !has_legacy_row {
            let (text, _, had_errors) = encoding_rs::SHIFT_JIS.decode(&bytes[start..pos]);
            if !had_errors
                && text.chars().count() >= MIN_STRING_LEN
                && has_meaningful_content(&text)
                && is_plausible_halfwidth_word(&text)
            {
                results.push(SjisStringRegion {
                    offset: start,
                    text: text.into_owned(),
                    legacy_index: None,
                });
            }
        }
    }

    results.sort_unstable_by_key(|region| region.offset);
    results
}

/// Width of a printable token, respecting Shift-JIS lead/trail boundaries.
fn sjis_text_char_len(bytes: &[u8], pos: usize) -> Option<usize> {
    let b = *bytes.get(pos)?;
    if is_sjis_lead_byte(b) {
        let trail = *bytes.get(pos + 1)?;
        return ((0x40..=0x7E).contains(&trail) || (0x80..=0xFC).contains(&trail)).then_some(2);
    }
    ((0x20..=0x7E).contains(&b) || (0xA1..=0xDF).contains(&b)).then_some(1)
}

/// Conservative structural heuristic, not a dictionary: after stripping ASCII
/// digits, punctuation and spaces at the edges, require at least three kana or
/// long-vowel characters and two distinct full-size kana letters. Start with a
/// full-size kana; reject halfwidth punctuation, incompatible small kana/voicing
/// marks, repeated long-vowel marks and final sokuon. This rejects short/repetitive
/// noise and malformed random 0xA1..=0xDF runs without tightening legacy filters.
fn is_plausible_halfwidth_word(text: &str) -> bool {
    let word =
        text.trim_matches(|c: char| c.is_ascii_digit() || c.is_ascii_punctuation() || c == ' ');
    if !word.starts_with(|c: char| matches!(c, 'ｦ' | 'ｱ'..='ﾝ')) {
        return false;
    }

    let mut previous = None;
    let mut base = None;
    let mut kana_count = 0;
    let mut distinct_kana = 0u64;
    let mut chars = word.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            'ｧ'..='ｮ' => {
                let preceding = if matches!(previous, Some('ﾞ' | 'ﾟ')) {
                    base
                } else {
                    previous
                };
                if !is_compatible_small_kana(preceding, c) {
                    return false;
                }
                kana_count += 1;
            }
            'ｯ' if chars
                .peek()
                .is_some_and(|next| matches!(next, 'ｶ'..='ﾄ' | 'ﾊ'..='ﾎ')) =>
            {
                kana_count += 1;
            }
            'ｰ' if previous.is_some_and(|p| p != 'ｰ') => kana_count += 1,
            'ｦ' | 'ｱ'..='ﾝ' => {
                kana_count += 1;
                distinct_kana |= 1 << (c as u32 - 'ｦ' as u32);
                base = Some(c);
            }
            'ﾞ' if previous.is_some_and(|p| matches!(p, 'ｳ' | 'ｶ'..='ﾄ' | 'ﾊ'..='ﾎ')) =>
                {}
            'ﾟ' if previous.is_some_and(|p| ('ﾊ'..='ﾎ').contains(&p)) => {}
            _ => return false,
        }
        previous = Some(c);
    }
    kana_count >= MIN_STRING_LEN.max(3) && distinct_kana.count_ones() >= 2
}

/// Common small-vowel and contracted-sound combinations, including loanwords.
fn is_compatible_small_kana(preceding: Option<char>, small: char) -> bool {
    matches!(
        (preceding, small),
        (Some('ｳ' | 'ｸ' | 'ﾂ' | 'ﾌ'), 'ｧ' | 'ｫ')
            | (Some('ｳ' | 'ｽ' | 'ﾂ' | 'ﾃ' | 'ﾌ'), 'ｨ')
            | (Some('ﾄ' | 'ﾌ'), 'ｩ')
            | (
                Some(
                    'ｲ' | 'ｳ'
                        | 'ｷ'
                        | 'ｼ'
                        | 'ﾁ'
                        | 'ﾆ'
                        | 'ﾋ'
                        | 'ﾐ'
                        | 'ﾘ'
                        | 'ﾂ'
                        | 'ﾃ'
                        | 'ﾌ'
                ),
                'ｪ'
            )
            | (
                Some('ｷ' | 'ｼ' | 'ﾁ' | 'ﾆ' | 'ﾋ' | 'ﾐ' | 'ﾘ' | 'ﾃ' | 'ﾌ'),
                'ｬ'..='ｮ'
            )
    )
}

/// Original scan, deliberately unchanged to preserve every legacy numeric index.
fn find_legacy_sjis_string_regions(bytes: &[u8]) -> Vec<(usize, String)> {
    let mut results = Vec::new();
    let len = bytes.len();
    let mut i = 0;

    while i < len {
        // Look for start of a Shift-JIS string region
        if is_legacy_sjis_start(bytes, i) {
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

fn is_legacy_sjis_start(bytes: &[u8], pos: usize) -> bool {
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
            if let Some(data_dir) = find_capital_data_dir(path) {
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

        let data_dir = find_capital_data_dir(path).ok_or_else(|| LocustError::ParseError {
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
        // skip (not hard fail), source-slot validation, capped pad/length noise.
        let mut report = InjectionReport {
            skip_reasons: Default::default(),
            files_modified: 0,
            strings_written: 0,
            strings_skipped: 0,
            warnings: Vec::new(),
            files_written: Vec::new(),
        };
        let mut length_skipped = 0usize;
        let mut pad_noted = 0usize;
        let mut target_missed = 0usize;

        // Group by file
        let mut by_file: HashMap<PathBuf, Vec<&StringEntry>> = HashMap::new();
        for entry in entries {
            by_file
                .entry(entry.file_path.clone())
                .or_default()
                .push(entry);
        }

        let data_dir = if path.is_dir() {
            find_capital_data_dir(path).unwrap_or_else(|| path.to_path_buf())
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
                        report.skip("untranslated", 1);
                        continue;
                    }
                };

                // Encode original and translation to Shift-JIS
                let encoding = encoding_rs::SHIFT_JIS;
                let (orig_bytes, _, orig_err) = encoding.encode(&entry.source);
                if orig_err {
                    report.warnings.push(format!(
                        "could not encode original '{}' to Shift-JIS",
                        entry.id
                    ));
                    report.skip("not_encodable", 1);
                    continue;
                }

                let (trans_bytes, _, trans_err) = encoding.encode(translation);
                // CP932 silently maps characters such as yen to backslash.
                // Check text fidelity before even accepting byte identity.
                let (decoded, _, decode_err) = encoding.decode(&trans_bytes);
                if trans_err || decode_err || decoded != translation.as_str() {
                    report.warnings.push(format!(
                        "could not losslessly encode translation for '{}' to Shift-JIS",
                        entry.id
                    ));
                    report.skip("not_encodable", 1);
                    continue;
                }

                // Identity: nothing to rewrite.
                if trans_bytes.as_ref() == orig_bytes.as_ref() {
                    report.skip("unchanged", 1);
                    continue;
                }

                if trans_bytes.len() > orig_bytes.len() {
                    if length_skipped < 5 {
                        report.warnings.push(format!(
                            "translation for '{}' longer than original in Shift-JIS ({} > {} bytes), skipping",
                            entry.id,
                            trans_bytes.len(),
                            orig_bytes.len()
                        ));
                    }
                    length_skipped += 1;
                    report.skip("too_long", 1);
                    continue;
                }

                // Only rewrite the recorded slot if it still contains the source.
                let target = entry
                    .metadata
                    .get("byte_offset")
                    .and_then(serde_json::Value::as_u64)
                    .and_then(|offset| usize::try_from(offset).ok())
                    .and_then(|pos| {
                        let end = pos.checked_add(orig_bytes.len())?;
                        (bytes.get(pos..end)? == orig_bytes.as_ref()).then_some(pos)
                    });
                if let Some(pos) = target {
                    // Write translation bytes
                    bytes[pos..pos + trans_bytes.len()].copy_from_slice(&trans_bytes);
                    // Pad remaining with null bytes
                    if trans_bytes.len() < orig_bytes.len() {
                        for b in &mut bytes[pos + trans_bytes.len()..pos + orig_bytes.len()] {
                            *b = 0;
                        }
                        if pad_noted < 5 {
                            report.warnings.push(format!(
                                "padded {} null bytes for '{}'",
                                orig_bytes.len() - trans_bytes.len(),
                                entry.id
                            ));
                        }
                        pad_noted += 1;
                    }
                    report.strings_written += 1;
                    modified = true;
                } else {
                    if target_missed < 5 {
                        report.warnings.push(format!(
                            "could not find original bytes for '{}' in file",
                            entry.id
                        ));
                    }
                    target_missed += 1;
                    report.skip("target_missing", 1);
                }
            }

            if modified {
                std::fs::write(&actual_path, &bytes)?;
                report.files_modified += 1;
                report.files_written.push(actual_path);
            }
        }

        if length_skipped > 0 {
            report.warnings.push(format!(
                "{length_skipped} translation(s) skipped because they are longer than the \
                 original Wolf string (Shift-JIS byte length must be ≤ source). Shorten them or \
                 use a length-aware model; equal-length translations inject cleanly."
            ));
        }

        Ok(report)
    }
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

    fn build_null_separated_sjis_fixture(strings: &[&str]) -> Vec<u8> {
        let mut bytes = vec![0; 16];
        for text in strings {
            let (encoded, _, had_errors) = encoding_rs::SHIFT_JIS.encode(text);
            assert!(!had_errors);
            bytes.extend_from_slice(&encoded);
            bytes.push(0);
        }
        bytes
    }

    #[test]
    fn w6_wolf_extracts_complete_text_prefixes() {
        for source in [
            "123 apples",
            "!? 42 apples",
            "  (7) apples",
            "123 \"apples\"",
            "123 勇者ｺﾝ",
        ] {
            let bytes = build_null_separated_sjis_fixture(&[source, "Magic"]);
            let entries = WolfRpgPlugin::extract_strings_heuristic(
                &bytes,
                "BasicData.wolf",
                Path::new("BasicData.wolf"),
            );
            assert_eq!(entries.len(), 2, "{source}");
            assert_eq!(entries[0].source, source);
            assert_eq!(entries[0].metadata["byte_offset"].as_u64(), Some(16));
            assert_eq!(entries[0].id, "BasicData.wolf#offset_16#0");
            assert!(entries[1].id.ends_with("#1"));
        }
    }

    #[test]
    fn w6_wolf_extracts_halfwidth_katakana_bytes_in_full() {
        let bytes = [0, 0xBA, 0xDD, 0xC6, 0xC1, 0xCA, 0];
        let entries = WolfRpgPlugin::extract_strings_heuristic(
            &bytes,
            "BasicData.wolf",
            Path::new("BasicData.wolf"),
        );
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].source, "ｺﾝﾆﾁﾊ");
        assert_eq!(entries[0].id, "BasicData.wolf#offset_1#k");
        assert_eq!(entries[0].metadata["byte_offset"].as_u64(), Some(1));
        assert_eq!(entries[0].metadata["binary_slot"], "sjis");
    }

    #[test]
    fn w6_wolf_accepts_plausible_halfwidth_words() {
        let sources = [
            "ﾃｽﾄ",
            "ｶﾞｲﾄﾞ",
            "ﾊﾟｰﾃｨｰ",
            "ｺｰﾋｰ",
            "123 ｺﾝﾆﾁﾊ",
            "!? ﾃｽﾄ",
            "ｼｬﾝﾌﾟｰ",
            "ﾌｧﾝﾀｼﾞｰ",
            "ｼﾞｬﾝﾌﾟ",
            "ﾎｯﾄｺｰﾋｰ",
        ];
        let bytes = build_null_separated_sjis_fixture(&sources);
        let entries = WolfRpgPlugin::extract_strings_heuristic(
            &bytes,
            "BasicData.wolf",
            Path::new("BasicData.wolf"),
        );
        assert_eq!(
            entries
                .iter()
                .map(|entry| entry.source.as_str())
                .collect::<Vec<_>>(),
            sources
        );
        assert!(entries.iter().all(|entry| entry.id.ends_with("#k")));
    }

    #[test]
    fn w6_wolf_excludes_numeric_crumbs_and_halfwidth_noise() {
        let mut bytes = build_null_separated_sjis_fixture(&[
            "123456",
            "!? 123",
            "A",
            "123 A",
            "あ",
            "1あ",
            "ｶ",
            "ｶﾅ",
            "ｱｱｱｱ",
            "ﾞｺﾝﾆﾁﾊ",
            "ｺﾟﾝﾆﾁﾊ",
            "ｰﾃｽﾄ",
            "ｧﾃｽﾄ",
            "ｦｧｨｩｪｫ",
            "ﾐｺｨﾑｯ",
            "ｺｨﾃｽﾄ",
        ]);
        // All bytes decode as halfwidth characters, but these are not plausible words.
        bytes.extend_from_slice(&[0, 0xA1, 0xDF, 0xA8, 0xDE, 0xB6, 0]);
        bytes.extend_from_slice(&[0, 0xD0, 0xBA, 0xA8, 0xD1, 0xAF, 0xDF, 0]);
        bytes.extend_from_slice(&[0, 0xD0, 0xBA, 0xA8, 0xD1, 0xAF, 0]);
        let entries = WolfRpgPlugin::extract_strings_heuristic(
            &bytes,
            "BasicData.wolf",
            Path::new("BasicData.wolf"),
        );
        assert!(entries.is_empty(), "unexpected noise rows: {entries:?}");
    }

    #[test]
    fn w6_wolf_mixed_regions_keep_working() {
        let sources = ["Hello ｺﾝﾆﾁﾊ 勇者", "勇者123ｶﾀｶﾅ", "ｺﾝHello"];
        let bytes = build_null_separated_sjis_fixture(&sources);
        let entries = WolfRpgPlugin::extract_strings_heuristic(
            &bytes,
            "BasicData.wolf",
            Path::new("BasicData.wolf"),
        );
        assert_eq!(
            entries
                .iter()
                .map(|entry| entry.source.as_str())
                .collect::<Vec<_>>(),
            sources
        );
        assert!(entries
            .iter()
            .enumerate()
            .all(|(idx, entry)| entry.id.ends_with(&format!("#{idx}"))));
    }

    #[test]
    fn w6_wolf_new_katakana_rows_do_not_renumber_legacy_ids() {
        let bytes = build_null_separated_sjis_fixture(&[
            "ﾃｽﾄ",
            "あ",
            "Hello",
            "ｺﾝﾆﾁﾊ",
            "はい",
            "123!",
            "ｶﾞｲﾄﾞ",
            "Magic",
        ]);
        let entries = WolfRpgPlugin::extract_strings_heuristic(
            &bytes,
            "BasicData.wolf",
            Path::new("BasicData.wolf"),
        );
        assert_eq!(
            entries
                .iter()
                .filter(|entry| !entry.id.ends_with("#k"))
                .map(|entry| entry.id.as_str())
                .collect::<Vec<_>>(),
            [
                "BasicData.wolf#offset_23#0",
                "BasicData.wolf#offset_35#1",
                "BasicData.wolf#offset_51#2",
            ]
        );
        assert_eq!(
            entries
                .iter()
                .filter(|entry| entry.id.ends_with("#k"))
                .map(|entry| entry.id.as_str())
                .collect::<Vec<_>>(),
            [
                "BasicData.wolf#offset_16#k",
                "BasicData.wolf#offset_29#k",
                "BasicData.wolf#offset_45#k",
            ]
        );
    }

    #[test]
    fn w6_wolf_does_not_start_inside_sjis_pairs_or_cross_binary_boundaries() {
        for source in [
            "Ａ123 apples",
            "ア123 apples",
            "亜123 apples",
            "者123 apples",
        ] {
            let (encoded, _, had_errors) = encoding_rs::SHIFT_JIS.encode(source);
            assert!(!had_errors);
            let mut bytes = vec![0xFF, 0x01, 0];
            bytes.extend_from_slice(&encoded);
            bytes.extend_from_slice(&[0, 0x02, 0xFF]);
            let entries = WolfRpgPlugin::extract_strings_heuristic(
                &bytes,
                "BasicData.wolf",
                Path::new("BasicData.wolf"),
            );
            assert_eq!(entries.len(), 1);
            assert_eq!(entries[0].source, source);
            assert_eq!(entries[0].id, "BasicData.wolf#offset_3#0");
        }
        // Non-text bytes delimit prefixes even without an intervening NUL.
        let bytes = b"\xff\x01123 apples\x02\xffNext\0";
        let entries = WolfRpgPlugin::extract_strings_heuristic(
            bytes,
            "BasicData.wolf",
            Path::new("BasicData.wolf"),
        );
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].source, "123 apples");
        assert_eq!(entries[0].metadata["byte_offset"].as_u64(), Some(2));
        assert_eq!(entries[1].source, "Next");
    }

    #[test]
    fn w6_wolf_injects_complete_prefix_and_katakana_slots() {
        for translations in [["7 pears", "やあ"], ["987 grapes", "HELLO"]] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("BasicData.wolf");
            let mut bytes = vec![0xFF, 0x01];
            let mut slots = Vec::new();
            for source in ["123 apples", "ｺﾝﾆﾁﾊ"] {
                let (encoded, _, had_errors) = encoding_rs::SHIFT_JIS.encode(source);
                assert!(!had_errors);
                slots.push((bytes.len(), encoded.len()));
                bytes.extend_from_slice(&encoded);
                bytes.extend_from_slice(&[0, 0x02, 0xFF]);
            }
            // These excluded regions and binary sentinels must survive byte-for-byte.
            bytes.extend_from_slice(b"12345\0A\0");
            bytes.extend_from_slice(&[0xA1, 0xDF, 0xA8, 0xDE, 0xB6, 0]);
            fs::write(&path, &bytes).unwrap();
            let plugin = WolfRpgPlugin::new();
            let mut entries = plugin.extract(&path).unwrap();
            assert_eq!(entries.len(), 2);
            assert_eq!(entries[0].source, "123 apples");
            assert_eq!(entries[1].source, "ｺﾝﾆﾁﾊ");
            let mut expected = bytes.clone();
            for ((entry, translation), (offset, length)) in
                entries.iter_mut().zip(translations).zip(slots)
            {
                assert_eq!(entry.metadata["byte_offset"].as_u64(), Some(offset as u64));
                entry.translation = Some(translation.to_string());
                let (encoded, _, had_errors) = encoding_rs::SHIFT_JIS.encode(translation);
                assert!(!had_errors);
                expected[offset..offset + length].fill(0);
                expected[offset..offset + encoded.len()].copy_from_slice(&encoded);
            }
            entries.reverse();
            let report = plugin.inject(&path, &entries).unwrap();
            assert_eq!(report.strings_written, 2);
            assert_eq!(report.strings_skipped, 0);
            assert_eq!(report.files_modified, 1);
            assert_eq!(fs::read(&path).unwrap(), expected);
        }
    }

    fn assert_lossy_translation_refused(translation: &str, canonical: &str) {
        let (encoded, _, had_errors) = encoding_rs::SHIFT_JIS.encode(translation);
        assert!(!had_errors, "fixture must be silently mapped");
        let (decoded, _, had_errors) = encoding_rs::SHIFT_JIS.decode(&encoded);
        assert!(!had_errors);
        assert_eq!(decoded, canonical);
        assert_ne!(decoded, translation);

        // Also exercise the byte-identity path: a lossy translation must be
        // classified as an encoding refusal even when it encodes to the source.
        for source in ["Hello world", canonical] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("BasicData.wolf");
            let original = build_null_separated_sjis_fixture(&[source]);
            fs::write(&path, &original).unwrap();
            let plugin = WolfRpgPlugin::new();
            let mut entries = plugin.extract(&path).unwrap();
            assert_eq!(entries.len(), 1);
            entries[0].translation = Some(translation.into());
            let report = plugin.inject(&path, &entries).unwrap();
            assert_eq!((report.strings_written, report.strings_skipped), (0, 1));
            assert_eq!(report.skip_reasons, [("not_encodable".into(), 1)].into());
            assert_eq!(report.files_modified, 0);
            assert!(report.files_written.is_empty());
            assert_eq!(report.warnings.len(), 1);
            assert!(report.warnings[0].contains(&entries[0].id));
            assert!(report.warnings[0].contains("Shift-JIS"));
            assert_eq!(fs::read(&path).unwrap(), original);
        }
    }

    #[test]
    fn w9_wolf_yen_translation_is_refused() {
        assert_lossy_translation_refused("Pay ¥100", "Pay \\100");
    }

    #[test]
    fn w9_wolf_overline_translation_is_refused() {
        assert_lossy_translation_refused("Mark ‾100", "Mark ~100");
    }

    #[test]
    fn w9_wolf_minus_translation_is_refused() {
        assert_lossy_translation_refused("Score −1", "Score －1");
    }

    #[test]
    fn w9_wolf_representable_translations_keep_exact_slot_bytes() {
        for translation in ["Pay \\100", "Mark ~100", "Score －1", "こんにちは"] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("BasicData.wolf");
            let mut expected = build_null_separated_sjis_fixture(&["Hello world"]);
            fs::write(&path, &expected).unwrap();
            let plugin = WolfRpgPlugin::new();
            let mut entries = plugin.extract(&path).unwrap();
            assert_eq!(entries.len(), 1);
            entries[0].translation = Some(translation.into());
            let report = plugin.inject(&path, &entries).unwrap();
            assert_eq!((report.strings_written, report.strings_skipped), (1, 0));
            assert!(report.skip_reasons.is_empty());
            assert_eq!(report.files_modified, 1);
            assert_eq!(report.files_written, vec![path.clone()]);
            let (encoded, _, had_errors) = encoding_rs::SHIFT_JIS.encode(translation);
            assert!(!had_errors);
            expected[16..27].fill(0);
            expected[16..16 + encoded.len()].copy_from_slice(&encoded);
            assert_eq!(fs::read(&path).unwrap(), expected);
            assert_eq!(plugin.extract(&path).unwrap()[0].source, translation);
        }
    }

    #[test]
    fn w6_wolf_oversize_prefix_and_katakana_translations_leave_slots_intact() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("BasicData.wolf");
        let bytes = build_null_separated_sjis_fixture(&["123 apples", "ｺﾝﾆﾁﾊ"]);
        fs::write(&path, &bytes).unwrap();
        let plugin = WolfRpgPlugin::new();
        let mut entries = plugin.extract(&path).unwrap();
        assert_eq!(entries.len(), 2);
        for entry in &mut entries {
            entry.translation = Some("This translation exceeds either slot".to_string());
        }
        let report = plugin.inject(&path, &entries).unwrap();
        assert_eq!(report.strings_written, 0);
        assert_eq!(report.strings_skipped, 2);
        assert_eq!(report.skip_reasons, [("too_long".into(), 2)].into());
        assert_eq!(report.files_modified, 0);
        assert_eq!(fs::read(&path).unwrap(), bytes);
    }

    #[test]
    fn a3_wolf_repeated_slots_extract_and_inject() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("BasicData.wolf");
        let bytes = build_null_separated_sjis_fixture(&["はい", "はい"]);
        fs::write(&path, &bytes).unwrap();
        let plugin = WolfRpgPlugin::new();
        let mut entries = plugin.extract(&path).unwrap();

        assert_eq!(entries.len(), 2);
        assert!(entries.iter().all(|entry| entry.source == "はい"));
        assert_eq!(entries[0].metadata["byte_offset"].as_u64(), Some(16));
        assert_eq!(entries[1].metadata["byte_offset"].as_u64(), Some(21));
        assert_eq!(entries[0].id, "BasicData.wolf#offset_16#0");
        assert_eq!(entries[1].id, "BasicData.wolf#offset_21#1");

        entries[0].translation = Some("ええ".to_string());
        entries[1].translation = Some("うん".to_string());
        // Injection order must not decide which physical slot is rewritten.
        entries.reverse();
        let report = plugin.inject(&path, &entries).unwrap();

        assert_eq!(report.strings_written, 2);
        assert_eq!(report.strings_skipped, 0);
        assert_eq!(report.files_modified, 1);
        assert_eq!(report.files_written, vec![path.clone()]);
        assert_eq!(
            fs::read(&path).unwrap(),
            build_null_separated_sjis_fixture(&["ええ", "うん"])
        );
    }

    #[test]
    fn a3_wolf_inject_substring_collision_preserves_earlier_region() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("BasicData.wolf");
        let bytes = build_null_separated_sjis_fixture(&["はいいいえ", "はい"]);
        fs::write(&path, &bytes).unwrap();
        let plugin = WolfRpgPlugin::new();
        let mut entries = plugin.extract(&path).unwrap();
        assert_eq!(entries.len(), 2);
        let mut entry = entries.pop().unwrap();
        assert_eq!(entry.source, "はい");
        assert_eq!(entry.metadata["byte_offset"].as_u64(), Some(27));
        entry.translation = Some("否".to_string());

        let report = plugin.inject(&path, &[entry]).unwrap();

        assert_eq!(report.strings_written, 1);
        assert_eq!(report.strings_skipped, 0);
        assert_eq!(report.files_modified, 1);
        let injected = fs::read(&path).unwrap();
        assert_eq!(&injected[..27], &bytes[..27]);
        let (translation, _, had_errors) = encoding_rs::SHIFT_JIS.encode("否");
        assert!(!had_errors);
        assert_eq!(&injected[27..29], translation.as_ref());
        assert_eq!(&injected[29..], &[0, 0, 0]);
        assert_eq!(injected.len(), bytes.len());
    }

    #[test]
    fn a3_wolf_inject_offset_mismatch_skips_without_writing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("BasicData.wolf");
        let bytes = build_null_separated_sjis_fixture(&["うん", "はい"]);
        fs::write(&path, &bytes).unwrap();
        let plugin = WolfRpgPlugin::new();
        let mut entry = plugin.extract(&path).unwrap().pop().unwrap();
        assert_eq!(entry.source, "はい");
        entry.translation = Some("ええ".to_string());
        entry
            .metadata
            .insert("byte_offset".to_string(), serde_json::json!(16));

        let report = plugin.inject(&path, &[entry]).unwrap();

        assert_eq!(report.strings_written, 0);
        assert_eq!(report.strings_skipped, 1);
        assert_eq!(report.skip_reasons, [("target_missing".into(), 1)].into());
        assert_eq!(report.files_modified, 0);
        assert!(report.files_written.is_empty());
        assert_eq!(report.warnings.len(), 1);
        assert_eq!(fs::read(&path).unwrap(), bytes);
    }

    #[test]
    fn a3_wolf_inject_missing_or_invalid_offsets_skip_without_writing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("BasicData.wolf");
        let bytes = build_null_separated_sjis_fixture(&["はい"]);
        fs::write(&path, &bytes).unwrap();
        let plugin = WolfRpgPlugin::new();
        let mut entry = plugin.extract(&path).unwrap().pop().unwrap();
        entry.translation = Some("ええ".to_string());

        for offset in [
            None,
            Some(serde_json::Value::Null),
            Some(serde_json::json!("16")),
            Some(serde_json::json!(-1)),
            Some(serde_json::json!(20)), // Original would extend past EOF.
            Some(serde_json::json!(21)), // Exactly at EOF.
            Some(serde_json::json!(22)), // Beyond EOF.
            Some(serde_json::json!(u64::MAX)), // Conversion/addition must not overflow.
        ] {
            entry.metadata.remove("byte_offset");
            if let Some(offset) = &offset {
                entry.metadata.insert("byte_offset".into(), offset.clone());
            }
            let report = plugin.inject(&path, std::slice::from_ref(&entry)).unwrap();

            assert_eq!(report.strings_written, 0, "offset: {offset:?}");
            assert_eq!(report.strings_skipped, 1, "offset: {offset:?}");
            assert_eq!(report.skip_reasons, [("target_missing".into(), 1)].into());
            assert_eq!(report.files_modified, 0);
            assert!(report.files_written.is_empty());
            assert_eq!(report.warnings.len(), 1);
            assert_eq!(fs::read(&path).unwrap(), bytes);
        }
    }

    #[test]
    fn a3_wolf_extract_unique_regions_keep_existing_ids() {
        // Include rejected regions so the IDs retain the original enumeration.
        let bytes = build_null_separated_sjis_fixture(&["あ", "はい", "123!", "勇者", "Magic"]);
        let entries = WolfRpgPlugin::extract_strings_heuristic(
            &bytes,
            "BasicData.wolf",
            Path::new("BasicData.wolf"),
        );

        assert_eq!(
            entries
                .iter()
                .map(|entry| entry.id.as_str())
                .collect::<Vec<_>>(),
            [
                "BasicData.wolf#offset_19#0",
                "BasicData.wolf#offset_29#1",
                "BasicData.wolf#offset_34#2",
            ]
        );
    }

    #[test]
    fn c123_named_skips_cover_all_wolf_rejections() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("BasicData.wolf");
        fs::write(&path, b"Original text\0").unwrap();
        let cases = [
            ("Original text", None, "untranslated"),
            ("\u{1f600}", Some("Hello"), "not_encodable"),
            ("Original text", Some("\u{1f600}"), "not_encodable"),
            ("Original text", Some("Original text"), "unchanged"),
            (
                "Original text",
                Some("A much longer translation"),
                "too_long",
            ),
            ("Missing source", Some("Hello"), "target_missing"),
        ];
        for (source, translation, reason) in cases {
            let mut entry = StringEntry::new("test", source, path.clone());
            entry.translation = translation.map(str::to_string);
            let mut report = WolfRpgPlugin::new().inject(dir.path(), &[entry]).unwrap();
            assert_eq!((report.strings_written, report.strings_skipped), (0, 1));
            report.classify_remaining_skips();
            assert_eq!(report.skip_reasons, [(reason.into(), 1)].into(), "{reason}");
            assert_eq!(fs::read(&path).unwrap(), b"Original text\0");
        }
    }

    /// Every test gets its OWN fixture directory. This used to build into a
    /// fixed, git-tracked path under `tests/fixtures/wolf_rpg`, which four tests
    /// then wrote and read concurrently — cargo runs tests in a binary in
    /// parallel, so one test could read the `.wolf` file while another was
    /// mid-write, intermittently failing on missing strings. It also meant the
    /// suite overwrote a tracked repo file on every run.
    fn setup_fixture() -> tempfile::TempDir {
        temp_wolf_dir()
    }

    fn temp_wolf_dir() -> tempfile::TempDir {
        let dir_guard = tempfile::Builder::new()
            .prefix("locust_wolf_")
            .tempdir()
            .unwrap();
        let dir = dir_guard.path().to_path_buf();
        let data_dir = dir.join("Data");
        fs::create_dir_all(&data_dir).unwrap();
        let bytes = build_test_fixture();
        fs::write(data_dir.join("BasicData.wolf"), &bytes).unwrap();
        dir_guard
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
        let dir_guard = setup_fixture();
        let dir = dir_guard.path().to_path_buf();
        let plugin = WolfRpgPlugin::new();
        assert!(plugin.detect(&dir));
    }

    #[test]
    fn test_detect_wolf_file() {
        let dir_guard = setup_fixture();
        let dir = dir_guard.path().to_path_buf();
        let file = dir.join("Data").join("BasicData.wolf");
        let plugin = WolfRpgPlugin::new();
        assert!(plugin.detect(&file));
    }

    #[test]
    fn test_detect_non_wolf() {
        let dir_guard = tempfile::Builder::new()
            .prefix("locust_notwolf_")
            .tempdir()
            .unwrap();
        let dir = dir_guard.path().to_path_buf();
        let plugin = WolfRpgPlugin::new();
        assert!(!plugin.detect(&dir));
    }

    #[test]
    fn test_extract_heuristic_finds_strings() {
        let dir_guard = setup_fixture();
        let dir = dir_guard.path().to_path_buf();
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
        let dir_guard = temp_wolf_dir();
        let dir = dir_guard.path().to_path_buf();
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
        let dir_guard = temp_wolf_dir();
        let dir = dir_guard.path().to_path_buf();
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
        let dir_guard = temp_wolf_dir();
        let dir = dir_guard.path().to_path_buf();
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
        let dir_guard = temp_wolf_dir();
        let dir = dir_guard.path().to_path_buf();
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
        let dir_guard = setup_fixture();
        let dir = dir_guard.path().to_path_buf();
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
