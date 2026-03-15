use std::collections::HashSet;
use std::path::{Path, PathBuf};

use serde::Serialize;
use walkdir::WalkDir;

use crate::error::Result;

pub struct FontValidator;

impl FontValidator {
    pub fn check_coverage(
        font_path: &Path,
        translations: &[&str],
    ) -> Result<FontCoverageReport> {
        let font_data = std::fs::read(font_path)?;
        let face = ttf_parser::Face::parse(&font_data, 0).map_err(|e| {
            crate::error::LocustError::Other(anyhow::anyhow!("failed to parse font: {}", e))
        })?;

        let font_name = face
            .names()
            .into_iter()
            .find(|n| n.name_id == ttf_parser::name_id::FULL_NAME)
            .and_then(|n| n.to_string());

        // Collect all unique chars
        let mut unique_chars = HashSet::new();
        for text in translations {
            for ch in text.chars() {
                unique_chars.insert(ch);
            }
        }

        let total_unique_chars = unique_chars.len();
        let mut missing_chars = Vec::new();

        for &ch in &unique_chars {
            if face.glyph_index(ch).is_none() {
                missing_chars.push(ch);
            }
        }

        missing_chars.sort();
        let missing_count = missing_chars.len();
        let coverage_percent = if total_unique_chars == 0 {
            100.0
        } else {
            ((total_unique_chars - missing_count) as f32 / total_unique_chars as f32) * 100.0
        };

        Ok(FontCoverageReport {
            font_path: font_path.to_path_buf(),
            font_name,
            total_unique_chars,
            missing_chars,
            missing_count,
            coverage_percent,
            has_full_coverage: missing_count == 0,
        })
    }

    pub fn find_game_fonts(game_path: &Path) -> Vec<PathBuf> {
        let font_extensions = ["ttf", "otf", "woff", "woff2"];
        let mut fonts = Vec::new();

        for entry in WalkDir::new(game_path)
            .follow_links(false)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            if entry.file_type().is_file() {
                if let Some(ext) = entry.path().extension().and_then(|e| e.to_str()) {
                    if font_extensions.contains(&ext.to_lowercase().as_str()) {
                        fonts.push(entry.path().to_path_buf());
                    }
                }
            }
        }

        fonts
    }

    pub fn check_game_fonts(
        game_path: &Path,
        translations: &[&str],
    ) -> Result<Vec<FontCoverageReport>> {
        let fonts = Self::find_game_fonts(game_path);
        let mut reports = Vec::new();
        for font_path in &fonts {
            match Self::check_coverage(font_path, translations) {
                Ok(report) => reports.push(report),
                Err(e) => {
                    tracing::warn!("Failed to check font {}: {}", font_path.display(), e);
                }
            }
        }
        Ok(reports)
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct FontCoverageReport {
    pub font_path: PathBuf,
    pub font_name: Option<String>,
    pub total_unique_chars: usize,
    pub missing_chars: Vec<char>,
    pub missing_count: usize,
    pub coverage_percent: f32,
    pub has_full_coverage: bool,
}

#[derive(Debug, Serialize)]
pub struct FontSuggestion {
    pub font_name: String,
    pub covers_scripts: Vec<String>,
    pub download_url: String,
    pub license: String,
}

pub fn suggest_replacement_font(missing_chars: &[char]) -> Vec<FontSuggestion> {
    let mut suggestions = Vec::new();
    let mut needed_scripts = HashSet::new();

    for &ch in missing_chars {
        match ch {
            '\u{00C0}'..='\u{024F}' | '\u{1E00}'..='\u{1EFF}' => {
                needed_scripts.insert("Latin Extended");
            }
            '\u{0400}'..='\u{04FF}' => {
                needed_scripts.insert("Cyrillic");
            }
            '\u{4E00}'..='\u{9FFF}' | '\u{3040}'..='\u{309F}' | '\u{30A0}'..='\u{30FF}'
            | '\u{AC00}'..='\u{D7AF}' => {
                needed_scripts.insert("CJK");
            }
            '\u{0600}'..='\u{06FF}' => {
                needed_scripts.insert("Arabic");
            }
            '\u{0590}'..='\u{05FF}' => {
                needed_scripts.insert("Hebrew");
            }
            '\u{0E00}'..='\u{0E7F}' => {
                needed_scripts.insert("Thai");
            }
            _ => {}
        }
    }

    if needed_scripts.contains("CJK") {
        suggestions.push(FontSuggestion {
            font_name: "Noto Sans CJK".to_string(),
            covers_scripts: vec![
                "CJK".to_string(),
                "Latin Extended".to_string(),
                "Cyrillic".to_string(),
            ],
            download_url: "https://github.com/googlefonts/noto-cjk/releases".to_string(),
            license: "SIL Open Font License 1.1".to_string(),
        });
    }

    if needed_scripts.contains("Latin Extended")
        || needed_scripts.contains("Cyrillic")
        || needed_scripts.contains("Arabic")
        || needed_scripts.contains("Hebrew")
        || needed_scripts.contains("Thai")
    {
        let mut covers: Vec<String> = needed_scripts
            .iter()
            .filter(|s| **s != "CJK")
            .map(|s| s.to_string())
            .collect();
        covers.sort();
        if !covers.is_empty() {
            suggestions.push(FontSuggestion {
                font_name: "Noto Sans".to_string(),
                covers_scripts: covers,
                download_url: "https://github.com/googlefonts/noto-fonts/releases".to_string(),
                license: "SIL Open Font License 1.1".to_string(),
            });
        }
    }

    suggestions
}

/// Build a minimal valid TrueType font that covers ASCII (0x20-0x7E).
/// This is a hand-crafted minimal TTF for testing purposes.
#[cfg(test)]
pub fn build_minimal_ascii_font() -> Vec<u8> {
    // We'll use ttf-parser's own test approach: create a minimal OTF/TTF
    // Instead of hand-crafting binary, let's use a simpler approach:
    // Build a minimal font with just the required tables.

    // Minimal TTF structure:
    // Offset table + table records + cmap + head + hhea + hmtx + maxp + name + post

    let mut buf = Vec::new();

    // --- Offset table ---
    let num_tables: u16 = 8;
    buf.extend_from_slice(&[0x00, 0x01, 0x00, 0x00]); // sfVersion (TrueType)
    buf.extend_from_slice(&num_tables.to_be_bytes()); // numTables
    buf.extend_from_slice(&[0x00, 0x80]); // searchRange
    buf.extend_from_slice(&[0x00, 0x03]); // entrySelector
    buf.extend_from_slice(&[0x00, 0x00]); // rangeShift

    // We'll fill table records after building tables
    let table_records_offset = buf.len();
    // Reserve space for 8 table records (16 bytes each)
    buf.extend_from_slice(&vec![0u8; num_tables as usize * 16]);

    let mut tables: Vec<(&[u8; 4], Vec<u8>)> = Vec::new();

    // --- cmap table (format 4, covering 0x20-0x7E) ---
    let cmap = build_cmap_table();
    tables.push((b"cmap", cmap));

    // --- head table ---
    let head = build_head_table();
    tables.push((b"head", head));

    // --- hhea table ---
    let hhea = build_hhea_table();
    tables.push((b"hhea", hhea));

    // --- hmtx table ---
    let hmtx = build_hmtx_table();
    tables.push((b"hmtx", hmtx));

    // --- maxp table ---
    let maxp = build_maxp_table();
    tables.push((b"maxp", maxp));

    // --- name table ---
    let name = build_name_table();
    tables.push((b"name", name));

    // --- OS/2 table ---
    let os2 = build_os2_table();
    tables.push((b"OS/2", os2));

    // --- post table ---
    let post = build_post_table();
    tables.push((b"post", post));

    // Sort tables by tag
    tables.sort_by(|a, b| a.0.cmp(b.0));

    // Write table data and fill records
    let mut current_offset;
    for (idx, (tag, data)) in tables.iter().enumerate() {
        // Pad to 4-byte alignment
        while buf.len() % 4 != 0 {
            buf.push(0);
        }
        current_offset = buf.len();

        let record_offset = table_records_offset + idx * 16;
        // Tag
        buf[record_offset..record_offset + 4].copy_from_slice(*tag);
        // Checksum (0 for simplicity)
        buf[record_offset + 4..record_offset + 8].copy_from_slice(&[0, 0, 0, 0]);
        // Offset
        buf[record_offset + 8..record_offset + 12]
            .copy_from_slice(&(current_offset as u32).to_be_bytes());
        // Length
        buf[record_offset + 12..record_offset + 16]
            .copy_from_slice(&(data.len() as u32).to_be_bytes());

        buf.extend_from_slice(data);
    }

    buf
}

#[cfg(test)]
fn build_cmap_table() -> Vec<u8> {
    let mut buf = Vec::new();

    // cmap header
    buf.extend_from_slice(&0u16.to_be_bytes()); // version
    buf.extend_from_slice(&1u16.to_be_bytes()); // numTables

    // Encoding record: platform 3 (Windows), encoding 1 (Unicode BMP)
    buf.extend_from_slice(&3u16.to_be_bytes()); // platformID
    buf.extend_from_slice(&1u16.to_be_bytes()); // encodingID
    buf.extend_from_slice(&12u32.to_be_bytes()); // offset to subtable

    // Format 4 subtable covering 0x20-0x7E (95 chars) → glyph IDs 1-96
    let seg_count = 2u16; // one segment + end sentinel
    let seg_count_x2 = seg_count * 2;
    let search_range = 4u16;
    let entry_selector = 1u16;
    let range_shift = 0u16;

    let end_codes: Vec<u16> = vec![0x007E, 0xFFFF];
    let start_codes: Vec<u16> = vec![0x0020, 0xFFFF];
    let id_deltas: Vec<i16> = vec![-(0x0020i16 - 1), 1]; // map 0x20 → glyph 1
    let id_range_offsets: Vec<u16> = vec![0, 0];

    let length = 14 + seg_count as usize * 8; // header + arrays

    buf.extend_from_slice(&4u16.to_be_bytes()); // format
    buf.extend_from_slice(&(length as u16).to_be_bytes()); // length
    buf.extend_from_slice(&0u16.to_be_bytes()); // language
    buf.extend_from_slice(&seg_count_x2.to_be_bytes());
    buf.extend_from_slice(&search_range.to_be_bytes());
    buf.extend_from_slice(&entry_selector.to_be_bytes());
    buf.extend_from_slice(&range_shift.to_be_bytes());

    for &ec in &end_codes {
        buf.extend_from_slice(&ec.to_be_bytes());
    }
    buf.extend_from_slice(&0u16.to_be_bytes()); // reservedPad
    for &sc in &start_codes {
        buf.extend_from_slice(&sc.to_be_bytes());
    }
    for &id in &id_deltas {
        buf.extend_from_slice(&id.to_be_bytes());
    }
    for &iro in &id_range_offsets {
        buf.extend_from_slice(&iro.to_be_bytes());
    }

    buf
}

#[cfg(test)]
fn build_head_table() -> Vec<u8> {
    let mut buf = vec![0u8; 54];
    // majorVersion = 1
    buf[0..2].copy_from_slice(&1u16.to_be_bytes());
    // minorVersion = 0
    // magicNumber at offset 12
    buf[12..16].copy_from_slice(&0x5F0F3CF5u32.to_be_bytes());
    // flags at offset 16
    buf[16..18].copy_from_slice(&0x000Bu16.to_be_bytes());
    // unitsPerEm at offset 18
    buf[18..20].copy_from_slice(&1000u16.to_be_bytes());
    // indexToLocFormat at offset 50
    buf[50..52].copy_from_slice(&0u16.to_be_bytes());
    buf
}

#[cfg(test)]
fn build_hhea_table() -> Vec<u8> {
    let mut buf = vec![0u8; 36];
    buf[0..2].copy_from_slice(&1u16.to_be_bytes()); // majorVersion
    // ascender at offset 4
    buf[4..6].copy_from_slice(&800u16.to_be_bytes());
    // descender at offset 6
    buf[6..8].copy_from_slice(&(-200i16).to_be_bytes());
    // numberOfHMetrics at offset 34
    buf[34..36].copy_from_slice(&96u16.to_be_bytes());
    buf
}

#[cfg(test)]
fn build_hmtx_table() -> Vec<u8> {
    // 96 entries: advanceWidth=500, lsb=0
    let mut buf = Vec::new();
    for _ in 0..96 {
        buf.extend_from_slice(&500u16.to_be_bytes());
        buf.extend_from_slice(&0i16.to_be_bytes());
    }
    buf
}

#[cfg(test)]
fn build_maxp_table() -> Vec<u8> {
    let mut buf = vec![0u8; 6];
    // version 0.5 (for CFF-like simplicity)
    buf[0..4].copy_from_slice(&0x00005000u32.to_be_bytes());
    // numGlyphs
    buf[4..6].copy_from_slice(&96u16.to_be_bytes());
    buf
}

#[cfg(test)]
fn build_name_table() -> Vec<u8> {
    let name_string = b"TestFont";
    let mut buf = Vec::new();
    buf.extend_from_slice(&0u16.to_be_bytes()); // format
    buf.extend_from_slice(&1u16.to_be_bytes()); // count
    let string_offset = 6 + 12; // header + 1 record
    buf.extend_from_slice(&(string_offset as u16).to_be_bytes()); // stringOffset

    // Name record: platform 1 (Mac), encoding 0, language 0, nameID 4 (Full Name)
    buf.extend_from_slice(&1u16.to_be_bytes()); // platformID
    buf.extend_from_slice(&0u16.to_be_bytes()); // encodingID
    buf.extend_from_slice(&0u16.to_be_bytes()); // languageID
    buf.extend_from_slice(&4u16.to_be_bytes()); // nameID (Full Name)
    buf.extend_from_slice(&(name_string.len() as u16).to_be_bytes()); // length
    buf.extend_from_slice(&0u16.to_be_bytes()); // offset

    buf.extend_from_slice(name_string);
    buf
}

#[cfg(test)]
fn build_os2_table() -> Vec<u8> {
    vec![0u8; 78] // minimal OS/2 version 0
}

#[cfg(test)]
fn build_post_table() -> Vec<u8> {
    let mut buf = vec![0u8; 32];
    // version 3.0 (no glyph names)
    buf[0..4].copy_from_slice(&0x00030000u32.to_be_bytes());
    buf
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn tempdir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("locust_font_{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn create_test_font() -> PathBuf {
        let dir = tempdir();
        let font_path = dir.join("test_font.ttf");
        let font_data = build_minimal_ascii_font();
        fs::write(&font_path, &font_data).unwrap();
        font_path
    }

    #[test]
    fn test_check_coverage_all_present() {
        let font_path = create_test_font();
        let translations = &["Hello world", "Test 123"];
        let report = FontValidator::check_coverage(&font_path, translations).unwrap();
        assert_eq!(report.missing_count, 0);
        assert!(report.has_full_coverage);
        assert!((report.coverage_percent - 100.0).abs() < 0.01);
    }

    #[test]
    fn test_check_coverage_missing_chars() {
        let font_path = create_test_font();
        let translations = &["Hello ñ world"];
        let report = FontValidator::check_coverage(&font_path, translations).unwrap();
        assert!(report.missing_chars.contains(&'ñ'));
        assert!(!report.has_full_coverage);
    }

    #[test]
    fn test_find_game_fonts_empty_dir() {
        let dir = tempdir();
        let fonts = FontValidator::find_game_fonts(&dir);
        assert!(fonts.is_empty());
    }

    #[test]
    fn test_find_game_fonts_finds_ttf() {
        let dir = tempdir();
        let fonts_dir = dir.join("fonts");
        fs::create_dir_all(&fonts_dir).unwrap();
        fs::write(fonts_dir.join("test.ttf"), &build_minimal_ascii_font()).unwrap();
        let fonts = FontValidator::find_game_fonts(&dir);
        assert_eq!(fonts.len(), 1);
    }

    #[test]
    fn test_coverage_report_percent() {
        let font_path = create_test_font();
        // Create a string with 10 ASCII + 1 non-ASCII char (ñ)
        // The font covers ASCII, so 10 covered, 1 missing = ~90.9%
        let text = "abcdefghijñ";
        let translations = &[text];
        let report = FontValidator::check_coverage(&font_path, translations).unwrap();
        let expected = ((report.total_unique_chars - report.missing_count) as f32
            / report.total_unique_chars as f32)
            * 100.0;
        assert!((report.coverage_percent - expected).abs() < 0.01);
        assert_eq!(report.missing_count, 1);
    }

    #[test]
    fn test_suggest_latin_extended() {
        let missing = vec!['ñ', 'é'];
        let suggestions = suggest_replacement_font(&missing);
        assert!(!suggestions.is_empty());
        let has_latin = suggestions
            .iter()
            .any(|s| s.covers_scripts.iter().any(|sc| sc.contains("Latin")));
        assert!(has_latin);
    }

    #[test]
    fn test_suggest_cjk() {
        let missing = vec!['漢', '字'];
        let suggestions = suggest_replacement_font(&missing);
        assert!(!suggestions.is_empty());
        let has_cjk = suggestions
            .iter()
            .any(|s| s.font_name.contains("CJK"));
        assert!(has_cjk);
    }
}
