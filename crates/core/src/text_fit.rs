//! Advisory, unkerned horizontal advances for explicitly configured MV/MZ dialogue.
//! This does not model an engine, wrap text, select fallback fonts, or mutate rows.

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::font_validation::read_font_data;
use crate::models::{StringEntry, FORMAT_ID_METADATA_KEY};

pub const MAX_INPUT_SCALARS: usize = 16_384;
const CACHE_CAPACITY: usize = 4;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TextFitRequest {
    pub entry_id: String,
    pub font_path: PathBuf,
    pub font_size_px: f64,
    pub usable_width_px: f64,
    pub draft_text: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EstimateStatus {
    WithinWidth,
    ExceedsWidth,
    Unavailable,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LineEstimate {
    pub status: EstimateStatus,
    pub advance_px: Option<f64>,
    pub usable_width_px: f64,
    pub reason: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TextFitResponse {
    pub source: Vec<LineEstimate>,
    pub draft: Vec<LineEstimate>,
}

/// The MV extractor owns both MV and MZ rows under `rpgmaker-mv`. Database
/// fields also use the dialogue tag, so the message locator is required too.
pub fn eligible_entry(entry: &StringEntry) -> bool {
    let Some(locator) = entry.id.strip_suffix("#msg") else {
        return false;
    };
    let Some((_, command)) = locator.rsplit_once("#cmd_") else {
        return false;
    };
    entry
        .metadata
        .get(FORMAT_ID_METADATA_KEY)
        .and_then(|value| value.as_str())
        == Some("rpgmaker-mv")
        && entry.tags.iter().any(|tag| tag == "dialogue")
        && !entry.tags.iter().any(|tag| tag == "scroll_text")
        && !command.is_empty()
        && command.bytes().all(|byte| byte.is_ascii_digit())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ProfileKey {
    hash: [u8; 32],
    size: u64,
    width: u64,
}

#[derive(Debug)]
struct FontMetrics {
    units_per_em: u16,
    advances: HashMap<char, Option<u16>>,
}

type ParsedFont = std::result::Result<Arc<FontMetrics>, String>;

#[derive(Clone, Debug, PartialEq, Eq)]
struct FileStamp {
    length: u64,
    modified: Option<SystemTime>,
    created: Option<SystemTime>,
}

impl FileStamp {
    fn read(path: &Path) -> std::result::Result<Self, String> {
        let metadata = std::fs::metadata(path).map_err(|err| format!("font_read_failed: {err}"))?;
        Ok(Self {
            length: metadata.len(),
            modified: metadata.modified().ok(),
            created: metadata.created().ok(),
        })
    }
}

struct CachedPath {
    path: PathBuf,
    stamp: FileStamp,
    hash: [u8; 32],
}

struct CachedResult {
    key: ProfileKey,
    source: String,
    draft: String,
    response: TextFitResponse,
}

/// Four paths, four parsed faces, four profiles, and four bounded text results.
/// Unchanged path metadata avoids disk reads on each debounced draft revision.
/// Changed metadata triggers a bounded read and SHA-256 content identity check.
/// Replacing a file while preserving length and both timestamps is not detected;
/// callers can clear the cache when explicit external edits require a reread.
#[derive(Default)]
pub struct TextFitCache {
    paths: VecDeque<CachedPath>,
    fonts: VecDeque<([u8; 32], ParsedFont)>,
    profiles: VecDeque<(ProfileKey, ParsedFont)>,
    results: VecDeque<CachedResult>,
    font_reads: usize,
    font_parses: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TextFitCacheStats {
    pub font_reads: usize,
    pub font_parses: usize,
    pub cached_paths: usize,
    pub cached_fonts: usize,
    pub cached_profiles: usize,
    pub cached_results: usize,
}

impl TextFitCache {
    pub fn clear(&mut self) {
        *self = Self::default();
    }

    pub fn stats(&self) -> TextFitCacheStats {
        TextFitCacheStats {
            font_reads: self.font_reads,
            font_parses: self.font_parses,
            cached_paths: self.paths.len(),
            cached_fonts: self.fonts.len(),
            cached_profiles: self.profiles.len(),
            cached_results: self.results.len(),
        }
    }

    fn font(
        &mut self,
        path: &Path,
        size: f64,
        width: f64,
    ) -> Result<(ProfileKey, ParsedFont), String> {
        if path.as_os_str().is_empty() {
            return Err("font_not_selected".into());
        }
        if !path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| {
                extension.eq_ignore_ascii_case("ttf") || extension.eq_ignore_ascii_case("otf")
            })
        {
            return Err("unsupported_font_format: select a single static TTF/OTF face".into());
        }
        let stamp = FileStamp::read(path)?;
        let cached_hash = self
            .paths
            .iter()
            .find(|cached| cached.path == path && cached.stamp == stamp)
            .map(|cached| cached.hash);
        let (hash, fresh_bytes) = match cached_hash {
            Some(hash) if self.fonts.iter().any(|(stored, _)| *stored == hash) => (hash, None),
            _ => {
                self.font_reads += 1;
                let bytes =
                    read_font_data(path).map_err(|err| format!("font_read_failed: {err}"))?;
                // Refuse a concurrent edit instead of caching bytes under an old stamp.
                if FileStamp::read(path)? != stamp {
                    return Err("font_changed_during_read: retry the estimate".into());
                }
                let hash: [u8; 32] = Sha256::digest(&bytes).into();
                self.paths.retain(|cached| cached.path != path);
                push_bounded(
                    &mut self.paths,
                    CachedPath {
                        path: path.to_path_buf(),
                        stamp,
                        hash,
                    },
                );
                (hash, Some(bytes))
            }
        };
        let key = ProfileKey {
            hash,
            size: size.to_bits(),
            width: width.to_bits(),
        };
        if let Some(index) = self.profiles.iter().position(|(stored, _)| *stored == key) {
            let profile = self.profiles.remove(index).expect("profile index exists");
            let result = profile.1.clone();
            self.profiles.push_back(profile);
            return Ok((key, result));
        }
        let parsed = if let Some(index) = self.fonts.iter().position(|(stored, _)| *stored == hash)
        {
            let font = self.fonts.remove(index).expect("font index exists");
            let parsed = font.1.clone();
            self.fonts.push_back(font);
            parsed
        } else {
            self.font_parses += 1;
            let parsed = parse_font(fresh_bytes.as_deref().expect("uncached font was read"));
            push_bounded(&mut self.fonts, (hash, parsed.clone()));
            parsed
        };
        push_bounded(&mut self.profiles, (key, parsed.clone()));
        Ok((key, parsed))
    }
}

fn push_bounded<T>(cache: &mut VecDeque<T>, value: T) {
    if cache.len() == CACHE_CAPACITY {
        cache.pop_front();
    }
    cache.push_back(value);
}

/// Read-only entry adapter. Hosts should call this on their blocking worker pool.
/// Invalid profile/font inputs yield unavailable lines; row/limit violations fail
/// the request so unrelated records cannot use this advisory endpoint.
pub fn estimate_entry(
    entry: &StringEntry,
    request: &TextFitRequest,
    cache: &mut TextFitCache,
) -> Result<TextFitResponse, String> {
    if request.entry_id != entry.id {
        return Err("entry_id_mismatch".into());
    }
    if !eligible_entry(entry) {
        return Err(
            "ineligible_entry: only extractor-owned MV/MZ dialogue messages are supported".into(),
        );
    }
    let source = entry.source.as_str();
    let draft = request
        .draft_text
        .as_deref()
        .or(entry.translation.as_deref())
        .unwrap_or_default();
    check_limit(source, draft)?;
    if let Some(reason) = profile_error(request.font_size_px, request.usable_width_px) {
        return Ok(unavailable_texts(
            source,
            draft,
            request.usable_width_px,
            reason,
        ));
    }
    let (key, font) = match cache.font(
        &request.font_path,
        request.font_size_px,
        request.usable_width_px,
    ) {
        Ok(font) => font,
        Err(reason) => {
            return Ok(unavailable_texts(
                source,
                draft,
                request.usable_width_px,
                &reason,
            ))
        }
    };
    if let Some(cached) = cache
        .results
        .iter()
        .find(|cached| cached.key == key && cached.source == source && cached.draft == draft)
    {
        return Ok(cached.response.clone());
    }
    let response = measure_with_font(
        &font,
        request.font_size_px,
        request.usable_width_px,
        source,
        draft,
    );
    push_bounded(
        &mut cache.results,
        CachedResult {
            key,
            source: source.into(),
            draft: draft.into(),
            response: response.clone(),
        },
    );
    Ok(response)
}

/// Pure calculation seam for owned fixtures; does no I/O or mutation.
pub fn estimate_texts(
    font_data: &[u8],
    font_size_px: f64,
    usable_width_px: f64,
    source: &str,
    draft: &str,
) -> Result<TextFitResponse, String> {
    check_limit(source, draft)?;
    if let Some(reason) = profile_error(font_size_px, usable_width_px) {
        return Ok(unavailable_texts(source, draft, usable_width_px, reason));
    }
    Ok(measure_with_font(
        &parse_font(font_data),
        font_size_px,
        usable_width_px,
        source,
        draft,
    ))
}

fn check_limit(source: &str, draft: &str) -> Result<(), String> {
    if source
        .chars()
        .chain(draft.chars())
        .take(MAX_INPUT_SCALARS + 1)
        .count()
        > MAX_INPUT_SCALARS
    {
        Err(format!(
            "input_limit: source and draft exceed {MAX_INPUT_SCALARS} Unicode scalars"
        ))
    } else {
        Ok(())
    }
}

fn profile_error(size: f64, width: f64) -> Option<&'static str> {
    if !size.is_finite() || size <= 0.0 {
        Some("invalid_font_size: choose a positive finite size")
    } else if !width.is_finite() || width <= 0.0 {
        Some("invalid_usable_width: choose a positive finite content width")
    } else {
        None
    }
}

fn parse_font(data: &[u8]) -> ParsedFont {
    if data.len() as u64 > 64 * 1024 * 1024 {
        return Err("font_too_large: font exceeds the 64 MiB safety limit".into());
    }
    if data.starts_with(b"ttcf") {
        return Err("font_collection: select a single static TTF/OTF face".into());
    }
    if data.starts_with(b"wOFF") || data.starts_with(b"wOF2") {
        return Err("unsupported_font_format: web fonts are unavailable".into());
    }
    if !data.starts_with(&[0, 1, 0, 0]) && !data.starts_with(b"OTTO") {
        return Err("invalid_font: expected a single static TTF/OTF face".into());
    }
    let raw = ttf_parser::RawFace::parse(data, 0).map_err(|err| format!("invalid_font: {err}"))?;
    if raw
        .table_records
        .into_iter()
        .any(|record| record.tag == ttf_parser::Tag::from_bytes(b"fvar"))
    {
        return Err("variable_font: variation settings are unsupported".into());
    }
    let face = ttf_parser::Face::parse(data, 0).map_err(|err| format!("invalid_font: {err}"))?;
    if face.is_variable() {
        return Err("variable_font: variation settings are unsupported".into());
    }
    if face.tables().hmtx.is_none() {
        return Err("missing_metrics: font has no usable horizontal advances".into());
    }
    let mut advances = HashMap::new();
    for codepoint in (0x20..=0x024f).chain(0x1e00..=0x206f) {
        if let Some(ch) = char::from_u32(codepoint).filter(|ch| supported_character(*ch)) {
            if let Some(glyph) = face.glyph_index(ch) {
                advances.insert(ch, face.glyph_hor_advance(glyph));
            }
        }
    }
    Ok(Arc::new(FontMetrics {
        units_per_em: face.units_per_em(),
        advances,
    }))
}

fn supported_character(ch: char) -> bool {
    matches!(ch, ' '..='~')
        || (matches!(ch, '\u{00c0}'..='\u{024f}' | '\u{1e00}'..='\u{1eff}') && ch.is_alphabetic())
        || matches!(ch, '¡' | '§' | '«' | '¶' | '·' | '»' | '¿')
        || "‐‑‒–—―‘’‚‛“”„‟†‡•‣․‥…‹›‼‽⁇⁈⁉".contains(ch)
}

fn lines(text: &str) -> impl Iterator<Item = &str> {
    let mut split = text.split('\n').peekable();
    std::iter::from_fn(move || {
        split.next().map(|line| {
            if split.peek().is_some() {
                line.strip_suffix('\r').unwrap_or(line)
            } else {
                line
            }
        })
    })
}

fn unavailable_line(width: f64, reason: &str) -> LineEstimate {
    LineEstimate {
        status: EstimateStatus::Unavailable,
        advance_px: None,
        // JSON cannot represent nonfinite numbers, even for a refused profile.
        usable_width_px: if width.is_finite() { width } else { 0.0 },
        reason: Some(reason.into()),
    }
}

fn unavailable_texts(source: &str, draft: &str, width: f64, reason: &str) -> TextFitResponse {
    TextFitResponse {
        source: lines(source)
            .map(|_| unavailable_line(width, reason))
            .collect(),
        draft: lines(draft)
            .map(|_| unavailable_line(width, reason))
            .collect(),
    }
}

fn measure_with_font(
    font: &ParsedFont,
    size: f64,
    width: f64,
    source: &str,
    draft: &str,
) -> TextFitResponse {
    let font = match font {
        Ok(font) => font,
        Err(reason) => return unavailable_texts(source, draft, width, reason),
    };
    TextFitResponse {
        source: lines(source)
            .map(|line| measure_line(font, size, width, line))
            .collect(),
        draft: lines(draft)
            .map(|line| measure_line(font, size, width, line))
            .collect(),
    }
}

fn measure_line(font: &FontMetrics, size: f64, width: f64, line: &str) -> LineEstimate {
    if line.contains('\\') {
        return unavailable_line(
            width,
            "backslash_control: any backslash syntax is unsupported",
        );
    }
    if line.contains('\t') {
        return unavailable_line(width, "tab: tab advance depends on unmeasured layout");
    }
    // Do not mistake an ASCII plugin/name/style tag for plain dialogue. No tag
    // is stripped and no partial width is returned, including malformed tags.
    if line.as_bytes().windows(2).any(|pair| match pair[0] {
        b'<' => pair[1].is_ascii_alphabetic() || matches!(pair[1], b'/' | b'!'),
        b'[' => pair[1].is_ascii_alphabetic() || pair[1] == b'/',
        b'{' => pair[1].is_ascii_alphabetic() || pair[1] == b'{',
        _ => false,
    }) {
        return unavailable_line(width, "inline_markup: inline tags are unsupported");
    }
    let mut units = 0_u64;
    for ch in line.chars() {
        if !supported_character(ch) {
            return unavailable_line(
                width,
                &format!("unsupported_character: U+{:04X}", ch as u32),
            );
        }
        match font.advances.get(&ch) {
            Some(Some(advance)) => units += u64::from(*advance),
            Some(None) => {
                return unavailable_line(width, &format!("missing_metrics: U+{:04X}", ch as u32))
            }
            None => return unavailable_line(width, &format!("missing_glyph: U+{:04X}", ch as u32)),
        }
    }
    let advance = units as f64 * size / f64::from(font.units_per_em);
    if !advance.is_finite() {
        return unavailable_line(
            width,
            "nonfinite_advance: chosen size exceeds numeric range",
        );
    }
    LineEstimate {
        status: if advance <= width {
            EstimateStatus::WithinWidth
        } else {
            EstimateStatus::ExceedsWidth
        },
        advance_px: Some(advance),
        usable_width_px: width,
        reason: line.is_empty().then(|| "no_text".into()),
    }
}
