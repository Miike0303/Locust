use std::collections::HashSet;
use std::io::Read;
use std::path::{Path, PathBuf};

use serde::Serialize;
use walkdir::WalkDir;

use crate::error::Result;

pub(crate) fn read_font_data(path: &Path) -> Result<Vec<u8>> {
    const LIMIT: u64 = 64 * 1024 * 1024;
    let file = std::fs::File::open(path)?;
    if file.metadata()?.len() > LIMIT {
        return Err(crate::error::LocustError::Other(anyhow::anyhow!(
            "font exceeds the 64 MiB safety limit"
        )));
    }
    let mut bytes = Vec::new();
    file.take(LIMIT + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > LIMIT {
        return Err(crate::error::LocustError::Other(anyhow::anyhow!(
            "font exceeds the 64 MiB safety limit"
        )));
    }
    Ok(bytes)
}

fn required_characters(translations: &[&str]) -> Vec<char> {
    // Deduplicate the translated corpus once, then reuse across every font face.
    let mut unique_chars = HashSet::new();
    for text in translations {
        for ch in text.chars() {
            if !ch.is_whitespace()
                && !ch.is_control()
                && !matches!(ch,
                    '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}' |
                    '\u{2060}'..='\u{206F}' | '\u{FE00}'..='\u{FE0F}' |
                    '\u{FEFF}' | '\u{E0100}'..='\u{E01EF}')
            {
                unique_chars.insert(ch);
            }
        }
    }

    let mut chars: Vec<char> = unique_chars.into_iter().collect();
    chars.sort_unstable();
    chars
}

fn is_active_font_tree_entry(entry: &walkdir::DirEntry) -> bool {
    // Recovery copies are not engine resources. Keep depth zero so an explicit
    // audit of a backup folder still examines the folder the caller selected.
    entry.depth() == 0
        || !entry.file_type().is_dir()
        || ![".locust", ".locust-injections", ".git"]
            .iter()
            .any(|name| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .eq_ignore_ascii_case(name)
            })
}

pub struct FontValidator;

impl FontValidator {
    pub fn check_coverage(font_path: &Path, translations: &[&str]) -> Result<FontCoverageReport> {
        let font_data = read_font_data(font_path)?;
        Self::coverage_for_face(font_path, &font_data, 0, &required_characters(translations))
    }

    pub(crate) fn coverage_from_data(
        font_path: &Path,
        data: &[u8],
        translations: &[&str],
    ) -> Result<FontCoverageReport> {
        Self::coverage_for_face(font_path, data, 0, &required_characters(translations))
    }

    /// A collection's faces are evaluated independently: their glyph sets are
    /// never unioned because an engine may select only one face.
    pub fn check_all_faces(
        font_path: &Path,
        translations: &[&str],
    ) -> Result<Vec<FontCoverageReport>> {
        Self::check_faces_for_characters(font_path, &required_characters(translations))
    }

    fn check_faces_for_characters(
        font_path: &Path,
        characters: &[char],
    ) -> Result<Vec<FontCoverageReport>> {
        let data = read_font_data(font_path)?;
        let count = ttf_parser::fonts_in_collection(&data).unwrap_or(1);
        if count == 0 || count > 1024 {
            return Err(crate::error::LocustError::Other(anyhow::anyhow!(
                "invalid or excessive font collection face count: {count}"
            )));
        }
        (0..count)
            .map(|index| Self::coverage_for_face(font_path, &data, index, characters))
            .collect()
    }

    fn coverage_for_face(
        font_path: &Path,
        data: &[u8],
        face_index: u32,
        characters: &[char],
    ) -> Result<FontCoverageReport> {
        if data.starts_with(b"wOFF") || data.starts_with(b"wOF2") {
            return Err(crate::error::LocustError::Other(anyhow::anyhow!(
                "WOFF/WOFF2 coverage is unsupported; supply the original TTF/OTF for checking"
            )));
        }
        let face = ttf_parser::Face::parse(data, face_index).map_err(|e| {
            crate::error::LocustError::Other(anyhow::anyhow!(
                "failed to parse font face {face_index}: {e}"
            ))
        })?;

        let font_name = face
            .names()
            .into_iter()
            .find(|n| n.name_id == ttf_parser::name_id::FULL_NAME)
            .and_then(|n| n.to_string());

        let total_unique_chars = characters.len();
        let mut missing_chars = Vec::new();

        for &ch in characters {
            if face.glyph_index(ch).is_none() {
                missing_chars.push(ch);
            }
        }

        let missing_count = missing_chars.len();
        let coverage_percent = if total_unique_chars == 0 {
            100.0
        } else {
            ((total_unique_chars - missing_count) as f32 / total_unique_chars as f32) * 100.0
        };

        Ok(FontCoverageReport {
            font_path: font_path.to_path_buf(),
            font_name,
            face_index,
            total_unique_chars,
            missing_chars,
            missing_count,
            coverage_percent,
            has_full_coverage: missing_count == 0,
        })
    }

    pub fn find_game_fonts(game_path: &Path) -> Vec<PathBuf> {
        let font_extensions = ["ttf", "otf", "ttc", "otc", "woff", "woff2"];
        let mut fonts = Vec::new();

        for entry in WalkDir::new(game_path)
            .follow_links(false)
            .into_iter()
            .filter_entry(is_active_font_tree_entry)
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

        fonts.sort();
        fonts
    }

    pub fn check_game_fonts(
        game_path: &Path,
        translations: &[&str],
    ) -> Result<Vec<FontCoverageReport>> {
        Ok(Self::audit_game_fonts(game_path, translations)?.fonts)
    }

    /// Returns failures alongside successful faces, so an empty result cannot
    /// silently imply that unreadable or web fonts have full glyph coverage.
    pub fn audit_game_fonts(game_path: &Path, translations: &[&str]) -> Result<FontAuditReport> {
        if !game_path.is_dir() {
            return Err(crate::error::LocustError::Other(anyhow::anyhow!(
                "game path is not a directory: {}",
                game_path.display()
            )));
        }
        let mut audit = FontAuditReport::default();
        let mut characters = None;
        for entry in WalkDir::new(game_path)
            .follow_links(false)
            .into_iter()
            .filter_entry(is_active_font_tree_entry)
        {
            let entry = match entry {
                Ok(entry) => entry,
                Err(error) => {
                    audit.issues.push(FontAuditIssue {
                        font_path: error.path().unwrap_or(game_path).into(),
                        message: error.to_string(),
                    });
                    continue;
                }
            };
            if !entry.file_type().is_file() {
                continue;
            }
            let ext = entry
                .path()
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("")
                .to_ascii_lowercase();
            if !["ttf", "otf", "ttc", "otc", "woff", "woff2"].contains(&ext.as_str()) {
                continue;
            }
            match Self::check_faces_for_characters(
                entry.path(),
                characters.get_or_insert_with(|| required_characters(translations)),
            ) {
                Ok(reports) => audit.fonts.extend(reports),
                Err(error) => audit.issues.push(FontAuditIssue {
                    font_path: entry.path().into(),
                    message: error.to_string(),
                }),
            }
        }
        audit.fonts.sort_by(|a, b| {
            a.font_path
                .cmp(&b.font_path)
                .then(a.face_index.cmp(&b.face_index))
        });
        audit.issues.sort_by(|a, b| a.font_path.cmp(&b.font_path));
        Ok(audit)
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct FontCoverageReport {
    pub font_path: PathBuf,
    pub font_name: Option<String>,
    pub face_index: u32,
    pub total_unique_chars: usize,
    pub missing_chars: Vec<char>,
    pub missing_count: usize,
    pub coverage_percent: f32,
    pub has_full_coverage: bool,
}

/// Coverage describes Unicode cmap presence only, not shaping, line fit,
/// fallback selection, or Unity TextMeshPro's separately generated SDF atlas.
pub const FONT_COVERAGE_LIMITATION: &str = "Coverage checks Unicode cmap entries only. Zero required characters do not establish coverage. Shaping, RTL order, line fit and engine font selection require runtime checks. Unity TextMeshPro needs its font asset/SDF atlas regenerated with the target glyphs or a configured TMP fallback asset; replacing a TTF does not update that atlas. Suggested families are candidates, not a guarantee of every required glyph; verify the actual font, especially for supplementary CJK characters.";

#[derive(Debug, Default, Serialize)]
pub struct FontAuditReport {
    pub fonts: Vec<FontCoverageReport>,
    pub issues: Vec<FontAuditIssue>,
}

#[derive(Debug, Serialize)]
pub struct FontAuditIssue {
    pub font_path: PathBuf,
    pub message: String,
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
            '\u{0400}'..='\u{052F}' | '\u{2DE0}'..='\u{2DFF}' | '\u{A640}'..='\u{A69F}' => {
                needed_scripts.insert("Cyrillic");
            }
            // Unicode 17 block ranges, including supplementary ideographs.
            // These trigger a candidate family, not guaranteed glyph coverage.
            '\u{2E80}'..='\u{2EFF}'
            | '\u{2F00}'..='\u{2FDF}'
            | '\u{3000}'..='\u{303F}'
            | '\u{3040}'..='\u{30FF}'
            | '\u{3100}'..='\u{312F}'
            | '\u{3130}'..='\u{318F}'
            | '\u{3190}'..='\u{31BF}'
            | '\u{31C0}'..='\u{33FF}'
            | '\u{3400}'..='\u{4DBF}'
            | '\u{4E00}'..='\u{9FFF}'
            | '\u{1100}'..='\u{11FF}'
            | '\u{A960}'..='\u{A97F}'
            | '\u{AC00}'..='\u{D7AF}'
            | '\u{D7B0}'..='\u{D7FF}'
            | '\u{F900}'..='\u{FAFF}'
            | '\u{FE30}'..='\u{FE4F}'
            | '\u{FF00}'..='\u{FFEF}'
            | '\u{1AFF0}'..='\u{1AFFF}'
            | '\u{1B000}'..='\u{1B16F}'
            | '\u{20000}'..='\u{2A6DF}'
            | '\u{2A700}'..='\u{2EE5F}'
            | '\u{2F800}'..='\u{2FA1F}'
            | '\u{30000}'..='\u{3347F}' => {
                needed_scripts.insert("CJK");
            }
            '\u{0600}'..='\u{06FF}'
            | '\u{0750}'..='\u{077F}'
            | '\u{0870}'..='\u{089F}'
            | '\u{08A0}'..='\u{08FF}'
            | '\u{FB50}'..='\u{FDFF}'
            | '\u{FE70}'..='\u{FEFF}' => {
                needed_scripts.insert("Arabic");
            }
            '\u{0590}'..='\u{05FF}' | '\u{FB1D}'..='\u{FB4F}' => {
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
            covers_scripts: vec!["CJK".to_string()],
            download_url: "https://github.com/notofonts/noto-cjk/releases".to_string(),
            license: "SIL Open Font License 1.1".to_string(),
        });
    }

    if needed_scripts.contains("Latin Extended") || needed_scripts.contains("Cyrillic") {
        let mut covers: Vec<String> = needed_scripts
            .iter()
            .filter(|s| matches!(**s, "Latin Extended" | "Cyrillic"))
            .map(|s| s.to_string())
            .collect();
        covers.sort();
        if !covers.is_empty() {
            suggestions.push(FontSuggestion {
                font_name: "Noto Sans".to_string(),
                covers_scripts: covers,
                download_url: "https://github.com/notofonts/latin-greek-cyrillic".to_string(),
                license: "SIL Open Font License 1.1".to_string(),
            });
        }
    }

    for script in ["Arabic", "Hebrew", "Thai"] {
        if needed_scripts.contains(script) {
            suggestions.push(FontSuggestion {
                font_name: format!("Noto Sans {script}"),
                covers_scripts: vec![script.into()],
                download_url: format!(
                    "https://github.com/notofonts/{}/releases",
                    script.to_ascii_lowercase()
                ),
                license: "SIL Open Font License 1.1".into(),
            });
        }
    }
    suggestions
}

/// Union missing glyphs across game fonts and map them to Noto families.
pub fn suggestions_for_font_reports(fonts: &[FontCoverageReport]) -> Vec<FontSuggestion> {
    let mut missing = Vec::new();
    for report in fonts {
        missing.extend(report.missing_chars.iter().copied());
    }
    suggest_replacement_font(&missing)
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
        fs::write(fonts_dir.join("test.ttf"), build_minimal_ascii_font()).unwrap();
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
        let has_cjk = suggestions.iter().any(|s| s.font_name.contains("CJK"));
        assert!(has_cjk);
    }

    #[test]
    fn test_suggestions_for_font_reports_unions_missing() {
        let fonts = vec![FontCoverageReport {
            font_path: PathBuf::from("a.ttf"),
            font_name: Some("A".into()),
            face_index: 0,
            total_unique_chars: 2,
            missing_chars: vec!['ñ'],
            missing_count: 1,
            coverage_percent: 50.0,
            has_full_coverage: false,
        }];
        let s = suggestions_for_font_reports(&fonts);
        assert!(!s.is_empty());
        assert!(s.iter().any(|x| x.font_name.contains("Noto")));
        // Negative: empty missing → no shopping list
        assert!(suggestions_for_font_reports(&[]).is_empty());
    }
}

#[cfg(test)]
mod font_audit_tests {
    use super::*;

    #[test]
    fn ignores_spacing_controls_and_nonprinting_format_characters() {
        let dir = tempfile::tempdir().unwrap();
        let font = dir.path().join("font.ttf");
        std::fs::write(&font, build_minimal_ascii_font()).unwrap();
        let report =
            FontValidator::check_coverage(&font, &["A \t\n\r\0\u{00a0}\u{200d}\u{2067}\u{fe0f}"])
                .unwrap();
        assert_eq!(report.total_unique_chars, 1);
        assert!(report.has_full_coverage);
        let report = FontValidator::check_coverage(&font, &[" \t\n"]).unwrap();
        assert_eq!(report.total_unique_chars, 0);
        assert_eq!(report.coverage_percent, 100.0);
    }

    #[test]
    fn collection_faces_have_independent_coverage() {
        let first = build_minimal_ascii_font();
        let mut second = first.clone();
        let tables = u16::from_be_bytes(second[4..6].try_into().unwrap()) as usize;
        for index in 0..tables {
            let record = 12 + index * 16;
            if &second[record..record + 4] == b"cmap" {
                let cmap = u32::from_be_bytes(second[record + 8..record + 12].try_into().unwrap())
                    as usize;
                second[cmap + 26..cmap + 28].copy_from_slice(&0x4e5eu16.to_be_bytes());
                second[cmap + 32..cmap + 34].copy_from_slice(&0x4e00u16.to_be_bytes());
                second[cmap + 36..cmap + 38].copy_from_slice(&(1i16 - 0x4e00).to_be_bytes());
            }
        }
        let mut collection = b"ttcf\x00\x01\x00\x00\x00\x00\x00\x02".to_vec();
        collection.extend_from_slice(&20u32.to_be_bytes());
        collection.extend_from_slice(&(20u32 + first.len() as u32).to_be_bytes());
        for mut font in [first, second] {
            let offset = collection.len() as u32;
            for index in 0..tables {
                let record = 12 + index * 16 + 8;
                let old = u32::from_be_bytes(font[record..record + 4].try_into().unwrap());
                font[record..record + 4].copy_from_slice(&(old + offset).to_be_bytes());
            }
            collection.extend(font);
        }
        let dir = tempfile::tempdir().unwrap();
        let font = dir.path().join("collection.ttc");
        std::fs::write(&font, collection).unwrap();
        let audit = FontValidator::audit_game_fonts(dir.path(), &["A一"]).unwrap();
        assert!(audit.issues.is_empty());
        assert_eq!(audit.fonts.len(), 2);
        assert_eq!(audit.fonts[0].face_index, 0);
        assert_eq!(audit.fonts[0].missing_chars, vec!['一']);
        assert_eq!(audit.fonts[1].face_index, 1);
        assert_eq!(audit.fonts[1].missing_chars, vec!['A']);
    }

    #[test]
    fn audit_exposes_unreadable_and_web_fonts() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("good.otf"), build_minimal_ascii_font()).unwrap();
        std::fs::write(dir.path().join("bad.ttf"), b"broken").unwrap();
        std::fs::write(dir.path().join("web.woff2"), b"wOF2broken").unwrap();
        let audit = FontValidator::audit_game_fonts(dir.path(), &["abc"]).unwrap();
        assert_eq!(audit.fonts.len(), 1);
        assert_eq!(audit.issues.len(), 2);
        assert!(audit.issues[1].message.contains("unsupported"));
        assert!(FontValidator::audit_game_fonts(&dir.path().join("missing"), &[]).is_err());
    }

    #[test]
    fn script_suggestions_name_actual_families() {
        let suggestions = suggest_replacement_font(&['ا', 'א', 'ก']);
        assert_eq!(suggestions.len(), 3);
        assert!(suggestions
            .iter()
            .any(|s| s.font_name == "Noto Sans Arabic"));
        assert!(!suggestions.iter().any(|s| s.font_name == "Noto Sans"));
    }
}

#[test]
fn font_audit_rejects_oversize_sparse_font_before_reading() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("oversize.ttf");
    std::fs::File::create(&path)
        .unwrap()
        .set_len(64 * 1024 * 1024 + 1)
        .unwrap();
    assert!(FontValidator::check_coverage(&path, &[])
        .unwrap_err()
        .to_string()
        .contains("64 MiB"));
    assert!(FontValidator::check_all_faces(&path, &[])
        .unwrap_err()
        .to_string()
        .contains("64 MiB"));
    let audit = FontValidator::audit_game_fonts(dir.path(), &[]).unwrap();
    assert!(audit.fonts.is_empty());
    assert_eq!(audit.issues.len(), 1);
}

#[cfg(test)]
mod font_followup_tests {
    use super::*;

    #[test]
    fn audit_skips_recovery_fonts_but_allows_an_explicit_backup_root() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".locust/backup/fonts")).unwrap();
        std::fs::create_dir(dir.path().join(".git")).unwrap();
        std::fs::write(dir.path().join("font.ttf"), build_minimal_ascii_font()).unwrap();
        std::fs::write(
            dir.path().join(".locust/backup/fonts/stale.ttf"),
            b"bad font",
        )
        .unwrap();
        std::fs::write(dir.path().join(".git/not-a-game-font.ttf"), b"bad font").unwrap();
        let audit = FontValidator::audit_game_fonts(dir.path(), &["a"]).unwrap();
        assert_eq!(audit.fonts.len(), 1);
        assert!(audit.issues.is_empty());
        assert_eq!(FontValidator::find_game_fonts(dir.path()).len(), 1);
        let explicit =
            FontValidator::audit_game_fonts(&dir.path().join(".locust/backup"), &["a"]).unwrap();
        assert_eq!(explicit.issues.len(), 1);
    }

    #[test]
    fn audit_and_discovery_exclude_injection_recovery_but_keep_similar_names() {
        let dir = tempfile::tempdir().unwrap();
        for folder in [
            "fonts",
            ".locust-injections/operations/id/work/fonts",
            ".LOCUST-INJECTIONS/operations/id/work/fonts",
            ".locust-injections-old/fonts",
        ] {
            let root = dir.path().join(folder);
            std::fs::create_dir_all(&root).unwrap();
            std::fs::write(root.join("valid.ttf"), build_minimal_ascii_font()).unwrap();
            std::fs::write(root.join("bad.woff2"), b"wOF2 unsupported font").unwrap();
        }
        let audit = FontValidator::audit_game_fonts(dir.path(), &["A"]).unwrap();
        assert_eq!(
            audit.fonts.len(),
            2,
            "only active and similarly named user folders"
        );
        assert_eq!(audit.issues.len(), 2);
        assert_eq!(FontValidator::find_game_fonts(dir.path()).len(), 4);
        for folder in [
            ".locust-injections",
            ".locust-injections/operations/id/work/fonts",
        ] {
            let root = dir.path().join(folder);
            let explicit = FontValidator::audit_game_fonts(&root, &["A"]).unwrap();
            assert_eq!(explicit.fonts.len(), 1);
            assert_eq!(explicit.issues.len(), 1);
            assert_eq!(FontValidator::find_game_fonts(&root).len(), 2);
        }
    }

    #[test]
    fn shared_required_characters_keep_combining_marks_and_supplementary_glyphs() {
        let chars = required_characters(&["A\u{301}\u{20000}", "A A\u{301}\n\u{fe0f}"]);
        assert_eq!(chars, vec!['A', '\u{301}', '\u{20000}']);
        let font = build_minimal_ascii_font();
        let first =
            FontValidator::coverage_for_face(Path::new("one.ttf"), &font, 0, &chars).unwrap();
        let second =
            FontValidator::coverage_from_data(Path::new("two.ttf"), &font, &["A\u{301}\u{20000}"])
                .unwrap();
        assert_eq!(first.missing_chars, second.missing_chars);
        assert_eq!(first.missing_chars, vec!['\u{301}', '\u{20000}']);
    }

    #[test]
    fn cjk_extension_and_compatibility_characters_get_candidate_family() {
        for ch in [
            '\u{3400}',
            '\u{20000}',
            '\u{2A700}',
            '\u{2EE5F}',
            '\u{2F800}',
            '\u{30000}',
            '\u{31350}',
            '\u{323B0}',
            '\u{F900}',
            '\u{1B000}',
        ] {
            let suggestions = suggest_replacement_font(&[ch]);
            assert_eq!(suggestions.len(), 1, "{ch:?}");
            assert_eq!(suggestions[0].font_name, "Noto Sans CJK");
            assert_eq!(suggestions[0].covers_scripts, vec!["CJK"]);
            assert_eq!(
                suggestions[0].download_url,
                "https://github.com/notofonts/noto-cjk/releases"
            );
        }
        for ch in [
            '\u{2A6E0}',
            '\u{2FA20}',
            '\u{33480}',
            '\u{1B170}',
            '\u{1F600}',
        ] {
            assert!(suggest_replacement_font(&[ch]).is_empty(), "{ch:?}");
        }
    }

    #[test]
    fn script_families_have_specific_sources_and_presentation_forms() {
        let suggestions = suggest_replacement_font(&['\u{FB50}', '\u{FB1D}', '\u{0E01}']);
        let names: Vec<_> = suggestions.iter().map(|s| s.font_name.as_str()).collect();
        assert_eq!(
            names,
            vec!["Noto Sans Arabic", "Noto Sans Hebrew", "Noto Sans Thai"]
        );
        for (suggestion, script) in suggestions.iter().zip(["arabic", "hebrew", "thai"]) {
            assert_eq!(
                suggestion.download_url,
                format!("https://github.com/notofonts/{script}/releases")
            );
            assert_eq!(suggestion.covers_scripts.len(), 1);
        }
    }
}
