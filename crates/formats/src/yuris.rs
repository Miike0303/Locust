//! YU-RIS engine plugin — Experimental (synthetic fixtures + real-game E2E).
//!
//! # Spec sources (do not invent transforms)
//! - Scenario layout + expression opcodes (Notes / scenario walk):
//!   https://github.com/arcusmaximus/VNTranslationTools/blob/main/VNTextPatch.Shared/Scripts/Yuris/Notes.txt
//!   https://github.com/arcusmaximus/VNTranslationTools/blob/main/VNTextPatch.Shared/Scripts/Yuris/YurisScenarioScript.cs
//!   https://github.com/arcusmaximus/VNTranslationTools/blob/main/VNTextPatch.Shared/Scripts/Yuris/YurisAttribute.cs
//! - Magic router (YSTB / YSCF / skip others e.g. YSTD):
//!   https://github.com/arcusmaximus/VNTranslationTools/blob/main/VNTextPatch.Shared/Scripts/Yuris/YurisScript.cs
//! - Requested raw URL (unverified / 404 on `master` — file lives under `main` as scenario notes):
//!   https://raw.githubusercontent.com/arcusmaximus/VNTranslationTools/master/VNTextPatch.Shared/Scripts/Yuris/YstbFile.cs
//!
//! # YSTB header (v5 family, version e.g. 0x22B — measured on real `yst*.ybn`)
//! ```text
//! 0x00 magic "YSTB"
//! 0x04 version u32
//! 0x08 num_instructions u32
//! 0x0C instructions_size u32  (= num * 4)
//! 0x10 attribute_descriptors_size u32
//! 0x14 attribute_values_size u32
//! 0x18 line_numbers_size u32
//! 0x1C padding u32
//! ```
//! Sections follow in order, each XOR'd with a 4-byte key (when non-zero).
//! Key derivation (only): first attribute descriptor's offset field is always
//! plaintext 0, so the encrypted u32 at `attr_section_start+8` *is* the key
//! (VNTextPatch; verified on Injuu Kangoku RE yst00000–04 / yst00042 → B4 62 6A D8).
//!
//! YPF containers: see [`crate::yuris_ypf`] (GARbro ArcYPF layout; inject rebuilds
//! the archive in place with a `.locust-old` safety rename).
//!
//! Out of scope: ysc.ybn command-name table (WORD/_/GOSUB filtering uses structural
//! heuristics instead — over-extraction OK); exotic per-title YPF swap schemes.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use locust_core::error::{LocustError, Result};
use locust_core::extraction::{FormatPlugin, InjectionReport};
use locust_core::models::{OutputMode, StringEntry};
use tracing::warn;

use crate::yuris_ypf::{self, YpfArchive};

const YSTB_MAGIC: &[u8; 4] = b"YSTB";
const HEADER_SIZE: usize = 0x20;
const INST_SIZE: usize = 4;
const ATTR_DESC_SIZE: usize = 12;
const PUSH_STRING: u8 = 0x4D;

const ATTR_RAW: i16 = 0;
const ATTR_EXPRESSION: i16 = 3;

pub struct YurisPlugin;

impl YurisPlugin {
    pub fn new() -> Self {
        Self
    }

    fn is_ybn(path: &Path) -> bool {
        path.extension()
            .and_then(|e| e.to_str())
            .map(|e| e.eq_ignore_ascii_case("ybn"))
            .unwrap_or(false)
    }

    fn is_ypf(path: &Path) -> bool {
        path.extension()
            .and_then(|e| e.to_str())
            .map(|e| e.eq_ignore_ascii_case("ypf"))
            .unwrap_or(false)
    }

    fn find_ybn_files(root: &Path) -> Vec<PathBuf> {
        let mut out = Vec::new();
        if root.is_file() {
            if Self::is_ybn(root) {
                out.push(root.to_path_buf());
            }
            return out;
        }
        if !root.is_dir() {
            return out;
        }
        for entry in walkdir::WalkDir::new(root)
            .follow_links(false)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            let p = entry.path();
            // Skip tool/backup trees (Injuu ships VNTranslationTools + res/ copies).
            if path_is_yuris_noise_dir(p) {
                continue;
            }
            if p.is_file() && Self::is_ybn(p) {
                out.push(p.to_path_buf());
            }
        }
        // Prefer one path per basename (pac/ > ysbin/ > anything else).
        dedupe_ybn_by_basename(out)
    }

    /// Top-level `*.ypf` plus `ysbin/*.ypf` (common YU-RIS layout).
    fn find_ypf_files(root: &Path) -> Vec<PathBuf> {
        let mut out = Vec::new();
        if root.is_file() {
            if Self::is_ypf(root) {
                out.push(root.to_path_buf());
            }
            return out;
        }
        if !root.is_dir() {
            return out;
        }
        let mut push_ypf = |dir: &Path| {
            if let Ok(entries) = std::fs::read_dir(dir) {
                for e in entries.flatten() {
                    let p = e.path();
                    if p.is_file() && Self::is_ypf(&p) {
                        out.push(p);
                    }
                }
            }
        };
        push_ypf(root);
        let ysbin = root.join("ysbin");
        if ysbin.is_dir() {
            push_ypf(&ysbin);
        }
        out.sort();
        out.dedup();
        out
    }

    fn has_ypf(root: &Path) -> bool {
        !Self::find_ypf_files(root).is_empty()
    }

    fn has_ybn_or_ypf(root: &Path) -> bool {
        !Self::find_ybn_files(root).is_empty() || Self::has_ypf(root)
    }

    fn root_dir(path: &Path) -> PathBuf {
        if path.is_dir() {
            path.to_path_buf()
        } else {
            path.parent().unwrap_or(path).to_path_buf()
        }
    }
}

impl Default for YurisPlugin {
    fn default() -> Self {
        Self::new()
    }
}

fn parse_err(file: &str, message: impl Into<String>) -> LocustError {
    LocustError::ParseError {
        file: file.into(),
        message: message.into(),
    }
}

// ─── Header / sections ─────────────────────────────────────────────────────

#[derive(Clone, Debug)]
struct YstbHeader {
    /// Format field at 0x04 (e.g. 0x22B v5 family). Kept for layout documentation.
    #[allow(dead_code)]
    version: u32,
    /// Format field at 0x08; validated against instructions_size (= n * 4).
    #[allow(dead_code)]
    num_instructions: u32,
    instructions_size: u32,
    attr_desc_size: u32,
    attr_values_size: u32,
    line_numbers_size: u32,
}

#[derive(Clone, Debug)]
struct AttrDesc {
    type_: i16,
    size: u32,
    offset: u32,
}

#[derive(Clone, Debug)]
struct ExtractedString {
    /// Sequential index among extracted strings in this file.
    arg_index: usize,
    /// Index into attribute descriptor list.
    attr_index: usize,
    text: String,
    attr_type: i16,
}

fn read_u32(data: &[u8], off: usize) -> Option<u32> {
    data.get(off..off + 4)
        .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

fn read_i16(data: &[u8], off: usize) -> Option<i16> {
    data.get(off..off + 2)
        .map(|b| i16::from_le_bytes([b[0], b[1]]))
}

fn write_u32(data: &mut [u8], off: usize, v: u32) {
    data[off..off + 4].copy_from_slice(&v.to_le_bytes());
}

fn parse_header(data: &[u8], file_label: &str) -> Result<YstbHeader> {
    if data.len() < HEADER_SIZE {
        return Err(parse_err(file_label, "file too small for YSTB header"));
    }
    if &data[0..4] != YSTB_MAGIC {
        return Err(parse_err(
            file_label,
            format!(
                "not YSTB magic (got {:?})",
                String::from_utf8_lossy(&data[0..4])
            ),
        ));
    }
    let version = read_u32(data, 0x04).unwrap();
    let num_instructions = read_u32(data, 0x08).unwrap();
    let instructions_size = read_u32(data, 0x0C).unwrap();
    let attr_desc_size = read_u32(data, 0x10).unwrap();
    let attr_values_size = read_u32(data, 0x14).unwrap();
    let line_numbers_size = read_u32(data, 0x18).unwrap();

    if instructions_size as usize != num_instructions as usize * INST_SIZE {
        return Err(parse_err(
            file_label,
            format!(
                "instructions_size {instructions_size} != num_instructions {num_instructions} * 4"
            ),
        ));
    }

    let expected_len = HEADER_SIZE
        + instructions_size as usize
        + attr_desc_size as usize
        + attr_values_size as usize
        + line_numbers_size as usize;
    if data.len() < expected_len {
        return Err(parse_err(
            file_label,
            format!(
                "truncated YSTB: have {} bytes, header claims {expected_len}",
                data.len()
            ),
        ));
    }

    Ok(YstbHeader {
        version,
        num_instructions,
        instructions_size,
        attr_desc_size,
        attr_values_size,
        line_numbers_size,
    })
}

fn section_offsets(hdr: &YstbHeader) -> (usize, usize, usize, usize) {
    let inst = HEADER_SIZE;
    let attr_desc = inst + hdr.instructions_size as usize;
    let attr_vals = attr_desc + hdr.attr_desc_size as usize;
    let lines = attr_vals + hdr.attr_values_size as usize;
    (inst, attr_desc, attr_vals, lines)
}

// ─── XOR ───────────────────────────────────────────────────────────────────

/// XOR all four payload sections with a repeating 4-byte little-endian key.
/// Matches VNTextPatch `ToggleScriptEncryption` (sizes at 0x0C..0x1C, data from 0x20).
fn toggle_encryption(data: &mut [u8], key: u32) {
    if key == 0 {
        return;
    }
    let key_bytes = key.to_le_bytes();
    let mut data_offset = HEADER_SIZE;
    let mut size_offset = 0x0C;
    while size_offset < 0x1C {
        let size = match read_u32(data, size_offset) {
            Some(s) => s as usize,
            None => return,
        };
        let end = data_offset.saturating_add(size).min(data.len());
        for (i, b) in data[data_offset..end].iter_mut().enumerate() {
            *b ^= key_bytes[i % 4];
        }
        data_offset += size;
        size_offset += 4;
    }
}

/// Derive the 4-byte XOR key (sole method).
///
/// The first attribute descriptor's `offset` field is always plaintext 0, so
/// the encrypted LE u32 at `attr_section_start + 8` **is** the key verbatim
/// (VNTextPatch `YurisScenarioScript`; auditor-confirmed on six Injuu .ybn files).
///
/// Returns `None` when the attribute section is smaller than one descriptor
/// (no strings to extract).
fn detect_xor_key(data: &[u8], hdr: &YstbHeader) -> Option<u32> {
    if hdr.attr_desc_size < ATTR_DESC_SIZE as u32 {
        return None;
    }
    let (_, attr_off, _, _) = section_offsets(hdr);
    if attr_off + ATTR_DESC_SIZE > data.len() {
        return None;
    }
    read_u32(data, attr_off + 8)
}

/// After XOR-decrypt: first descriptor must be well-formed.
/// Layout: `u16 id`, `i16 type`, `u32 size`, `u32 offset` with `offset == 0`
/// and `size <= attribute_values_size`. Failure means bad key or unsupported layout.
fn verify_first_attr_descriptor(data: &[u8], hdr: &YstbHeader, file_label: &str) -> Result<()> {
    if hdr.attr_desc_size < ATTR_DESC_SIZE as u32 {
        return Ok(());
    }
    let (_, attr_off, _, _) = section_offsets(hdr);
    if attr_off + ATTR_DESC_SIZE > data.len() {
        return Err(parse_err(
            file_label,
            "truncated attribute descriptor section after decrypt (bad key or unsupported layout)",
        ));
    }
    // u16 id at +0 (layout documentation; value unused for the check)
    let _id = read_i16(data, attr_off).ok_or_else(|| {
        parse_err(file_label, "truncated first attribute id after decrypt")
    })?;
    let _type = read_i16(data, attr_off + 2).ok_or_else(|| {
        parse_err(file_label, "truncated first attribute type after decrypt")
    })?;
    let size = read_u32(data, attr_off + 4).ok_or_else(|| {
        parse_err(file_label, "truncated first attribute size after decrypt")
    })?;
    let offset = read_u32(data, attr_off + 8).ok_or_else(|| {
        parse_err(file_label, "truncated first attribute offset after decrypt")
    })?;

    if offset != 0 {
        return Err(parse_err(
            file_label,
            format!(
                "bad XOR key or unsupported YSTB layout: first attribute offset is {offset:#x}, expected 0"
            ),
        ));
    }
    if size > hdr.attr_values_size {
        return Err(parse_err(
            file_label,
            format!(
                "bad XOR key or unsupported YSTB layout: first attribute size {size} > values section {}",
                hdr.attr_values_size
            ),
        ));
    }
    Ok(())
}

// ─── Attribute / string decode ─────────────────────────────────────────────

fn parse_attr_descs(data: &[u8], attr_desc_off: usize, attr_desc_size: u32) -> Result<Vec<AttrDesc>> {
    if !(attr_desc_size as usize).is_multiple_of(ATTR_DESC_SIZE) {
        return Err(parse_err(
            "ystb",
            format!("attribute descriptor size {attr_desc_size} not divisible by 12"),
        ));
    }
    let n = attr_desc_size as usize / ATTR_DESC_SIZE;
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let off = attr_desc_off + i * ATTR_DESC_SIZE;
        // id (i16 at +0) unused for extract/inject
        let _id = read_i16(data, off).ok_or_else(|| parse_err("ystb", "truncated attr id"))?;
        let type_ =
            read_i16(data, off + 2).ok_or_else(|| parse_err("ystb", "truncated attr type"))?;
        let size =
            read_u32(data, off + 4).ok_or_else(|| parse_err("ystb", "truncated attr size"))?;
        let offset =
            read_u32(data, off + 8).ok_or_else(|| parse_err("ystb", "truncated attr offset"))?;
        out.push(AttrDesc {
            type_,
            size,
            offset,
        });
    }
    Ok(out)
}

fn decode_sjis(bytes: &[u8]) -> String {
    let (cow, _, _had_errors) = encoding_rs::SHIFT_JIS.decode(bytes);
    cow.into_owned()
}

fn encode_sjis(s: &str) -> Vec<u8> {
    let (bytes, _, _) = encoding_rs::SHIFT_JIS.encode(s);
    bytes.into_owned()
}

fn unquote_string(s: &str) -> String {
    let b = s.as_bytes();
    if b.len() >= 2 && b[0] == b[b.len() - 1] {
        // Double / single / backtick delimiters (engine only checks first==last).
        let inner = &s[1..s.len() - 1];
        return unescape_c_light(inner);
    }
    s.to_string()
}

fn unescape_c_light(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some('r') => out.push('\r'),
                Some('\\') => out.push('\\'),
                Some(other) => {
                    out.push('\\');
                    out.push(other);
                }
                None => out.push('\\'),
            }
        } else {
            out.push(c);
        }
    }
    out
}

fn escape_c_light(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => {} // VNTextPatch drops bare \r on write
            '\\' => out.push_str("\\\\"),
            _ => out.push(c),
        }
    }
    out
}

/// Decode a single attribute value to player-facing text when possible.
fn decode_attr_value(data: &[u8], attr_vals_off: usize, attr: &AttrDesc) -> Option<String> {
    let start = attr_vals_off.checked_add(attr.offset as usize)?;
    let end = start.checked_add(attr.size as usize)?;
    if end > data.len() {
        return None;
    }
    let slice = &data[start..end];
    match attr.type_ {
        ATTR_RAW => {
            if slice.is_empty() {
                return None;
            }
            let s = decode_sjis(slice);
            if looks_player_visible(&s) {
                Some(s)
            } else {
                None
            }
        }
        ATTR_EXPRESSION => evaluate_push_string(slice),
        _ => None,
    }
}

fn evaluate_push_string(slice: &[u8]) -> Option<String> {
    // 4D XX XX <quoted SJIS string>
    if slice.len() < 3 || slice[0] != PUSH_STRING {
        return None;
    }
    let arg_len = u16::from_le_bytes([slice[1], slice[2]]) as usize;
    if 3 + arg_len != slice.len() {
        return None;
    }
    let s = decode_sjis(&slice[3..3 + arg_len]);
    let s = unquote_string(&s);
    let s = s.replace('\n', "\r\n").replace("\r\r\n", "\r\n");
    if looks_player_visible(&s) {
        Some(s)
    } else {
        None
    }
}

fn looks_player_visible(s: &str) -> bool {
    let t = s.trim();
    if t.chars().count() < 2 {
        return false;
    }
    // Binary attribute crumbs (`V\x03`) and other control-bearing garbage.
    if t.chars()
        .any(|c| c.is_control() && c != '\n' && c != '\r' && c != '\t')
    {
        return false;
    }
    if t.contains('\u{FFFD}') {
        return false;
    }
    if t.chars()
        .all(|c| c.is_ascii_digit() || c == '.' || c == '-' || c == '+')
    {
        return false;
    }
    // Skip path-like tokens without spaces.
    if (t.contains('/') || t.contains('\\')) && !t.contains(' ') && !t.chars().any(is_cjk) {
        return false;
    }
    // Very short pure-ASCII identifiers (engine tokens), keep CJK/dialogue.
    if t.chars().count() <= 3
        && t.is_ascii()
        && !t.chars().any(|c| c.is_ascii_whitespace())
        && !t.chars().any(|c| c == '.' || c == '!' || c == '?')
    {
        // Allow short UI like "OK" / "Sí" handled above via non-ascii / punctuation.
        if t.chars().all(|c| c.is_ascii_alphanumeric()) {
            return false;
        }
    }
    // Script commands / resource IDs (es.*, MAC.*, st01, HSE_056, BTN.PLATE…).
    if is_yuris_engine_token(t) {
        return false;
    }
    t.chars()
        .any(|c| c.is_alphabetic() || is_cjk(c) || c == '「' || c == '『' || c == '（')
}

/// True for pure-ASCII YU-RIS engine / resource identifiers that are not dialogue.
///
/// Real games pack thousands of `es.*` ops, `MAC.*` macros, and asset codes
/// (`st01`, `HSE_056`, `BTN.PLATE`) into attribute strings. Keep spaced dialogue,
/// CJK, accented UI (`Sí`), and SFX with strong punctuation (`*Thud*`).
fn is_yuris_engine_token(t: &str) -> bool {
    if !t.is_ascii() {
        return false;
    }
    if t.chars().any(|c| c.is_ascii_whitespace()) {
        return false;
    }
    // Dialogue / SFX punctuation → not an engine token.
    if t.chars().any(|c| {
        matches!(
            c,
            '!' | '?'
                | ','
                | '"'
                | '\''
                | '`'
                | '('
                | ')'
                | '['
                | ']'
                | '{'
                | '}'
                | '~'
                | '*'
                | ';'
                | ':'
        )
    }) {
        return false;
    }
    if t.contains("...") {
        return false;
    }

    let lower = t.to_ascii_lowercase();
    if lower.starts_with("es.") || lower.starts_with("mac.") {
        return true;
    }

    let id_charset = t
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.' || c == '-');
    if !id_charset {
        // Hotkeys like SHIFT+V / CTRL+S.
        if t.contains('+')
            && t.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '+' || c == '_' || c == '-')
        {
            return true;
        }
        return false;
    }

    // Digits → asset / state codes (st01, ysr000, HSE_056, cg160b_030).
    if t.chars().any(|c| c.is_ascii_digit()) {
        return true;
    }

    // Dotted multi-segment commands (BTN.PLATE, SCENARIO_TITLE is underscore).
    if t.contains('.') {
        let parts: Vec<&str> = t.split('.').filter(|p| !p.is_empty()).collect();
        if parts.len() >= 2
            && parts.iter().all(|p| {
                let alnum = p.chars().filter(|c| *c != '_').all(|c| c.is_ascii_alphanumeric());
                alnum && !p.is_empty() && p.len() <= 28
            })
        {
            return true;
        }
    }

    // snake_case / SCREAMING_SNAKE resource labels.
    if t.contains('_') && t.chars().filter(|c| *c != '_').all(|c| c.is_ascii_alphanumeric()) {
        return true;
    }

    // Short lowercase resource colors / stubs (black, white, tran, trbn).
    if t.len() <= 6
        && t.chars().all(|c| c.is_ascii_alphabetic() && c.is_ascii_lowercase())
    {
        return true;
    }

    // ALL-CAPS codes and system labels (CLRX, BACKLOG, ESCMODE, AUTOSAVETIMING).
    if t.len() >= 3
        && t.chars()
            .all(|c| c.is_ascii_alphabetic() && c.is_ascii_uppercase())
    {
        return true;
    }

    false
}

/// Skip tool/output/backup directories that re-host the same yst*.ybn set.
fn path_is_yuris_noise_dir(p: &Path) -> bool {
    for comp in p.components() {
        let Some(s) = comp.as_os_str().to_str() else {
            continue;
        };
        let lower = s.to_ascii_lowercase();
        if matches!(
            lower.as_str(),
            "vntranslationtools"
                | "__pycache__"
                | "gameupdate"
                | "output"
                | "output.ja.bak"
                | ".git"
                | ".locust"
        ) || lower.ends_with(".bak")
            || lower.starts_with("output.")
        {
            return true;
        }
    }
    false
}

/// Keep a single `.ybn` per file name, preferring game `pac/` over loose `res/`.
fn dedupe_ybn_by_basename(paths: Vec<PathBuf>) -> Vec<PathBuf> {
    use std::collections::BTreeMap;
    fn rank(p: &Path) -> i32 {
        let s = p.to_string_lossy().to_ascii_lowercase().replace('\\', "/");
        if s.contains("/pac/") {
            0
        } else if s.contains("/ysbin/") {
            1
        } else if s.contains("/res/") {
            3
        } else {
            2
        }
    }
    let mut best: BTreeMap<String, PathBuf> = BTreeMap::new();
    for p in paths {
        let key = p
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        if key.is_empty() {
            continue;
        }
        match best.get(&key) {
            None => {
                best.insert(key, p);
            }
            Some(prev) if rank(&p) < rank(prev) => {
                best.insert(key, p);
            }
            _ => {}
        }
    }
    best.into_values().collect()
}

fn is_cjk(c: char) -> bool {
    matches!(
        c as u32,
        0x3040..=0x30FF | 0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF | 0xFF66..=0xFF9D
    )
}

fn serialize_attr_value(attr_type: i16, text: &str) -> Result<Vec<u8>> {
    match attr_type {
        ATTR_RAW => {
            // Map \r\n → YU-RIS control EF F0 is optional for Experimental; keep SJIS as-is.
            Ok(encode_sjis(text))
        }
        ATTR_EXPRESSION => {
            // Quote with backticks so content may contain both " and ' (VNTextPatch).
            if text.contains('`') {
                return Err(parse_err(
                    "ystb",
                    format!("message cannot contain backticks [{text}]"),
                ));
            }
            let body = format!("`{}`", escape_c_light(text));
            let body_bytes = encode_sjis(&body);
            let mut out = Vec::with_capacity(3 + body_bytes.len());
            out.push(PUSH_STRING);
            let n = body_bytes.len() as u16;
            out.extend_from_slice(&n.to_le_bytes());
            out.extend_from_slice(&body_bytes);
            Ok(out)
        }
        other => Err(parse_err(
            "ystb",
            format!("cannot serialize attribute type {other}"),
        )),
    }
}

// ─── Parse / extract / inject body ─────────────────────────────────────────

#[derive(Clone, Debug)]
struct DecryptedYstb {
    data: Vec<u8>,
    hdr: YstbHeader,
    key: u32,
    attrs: Vec<AttrDesc>,
    strings: Vec<ExtractedString>,
}

fn load_ystb(bytes: &[u8], file_label: &str) -> Result<Option<DecryptedYstb>> {
    if bytes.len() < 4 {
        return Ok(None);
    }
    // Non-YSTB magics (YSTD stub, YSTL, YSCM, …): skip silently — no strings.
    if &bytes[0..4] != YSTB_MAGIC {
        return Ok(None);
    }

    let hdr = parse_header(bytes, file_label)?;
    let mut data = bytes.to_vec();

    // Attr section shorter than one descriptor → no strings (not an error).
    let Some(key) = detect_xor_key(&data, &hdr) else {
        return Ok(Some(DecryptedYstb {
            data,
            hdr,
            key: 0,
            attrs: Vec::new(),
            strings: Vec::new(),
        }));
    };

    toggle_encryption(&mut data, key);
    verify_first_attr_descriptor(&data, &hdr, file_label)?;

    let (_, attr_desc_off, attr_vals_off, _) = section_offsets(&hdr);
    let attrs = parse_attr_descs(&data, attr_desc_off, hdr.attr_desc_size)?;

    let mut strings = Vec::new();
    for (attr_index, attr) in attrs.iter().enumerate() {
        if let Some(text) = decode_attr_value(&data, attr_vals_off, attr) {
            let arg_index = strings.len();
            strings.push(ExtractedString {
                arg_index,
                attr_index,
                text,
                attr_type: attr.type_,
            });
        }
    }

    Ok(Some(DecryptedYstb {
        data,
        hdr,
        key,
        attrs,
        strings,
    }))
}

fn inject_into_ystb(ystb: &DecryptedYstb, translations: &HashMap<usize, &str>) -> Result<Vec<u8>> {
    let (_, attr_desc_off, attr_vals_off, line_off) = section_offsets(&ystb.hdr);

    // Build new attribute-values blob; track per-descriptor (size, offset).
    let mut new_values: Vec<u8> = Vec::new();
    let mut new_meta: Vec<(u32, u32)> = Vec::with_capacity(ystb.attrs.len()); // (size, offset)

    // Map attr_index → new text when translated.
    let mut by_attr: HashMap<usize, &str> = HashMap::new();
    for s in &ystb.strings {
        if let Some(t) = translations.get(&s.arg_index) {
            by_attr.insert(s.attr_index, *t);
        }
    }

    for (i, attr) in ystb.attrs.iter().enumerate() {
        let old_start = attr_vals_off + attr.offset as usize;
        let old_end = old_start + attr.size as usize;
        let new_bytes = if let Some(text) = by_attr.get(&i) {
            serialize_attr_value(attr.type_, text)?
        } else if old_end <= ystb.data.len() {
            ystb.data[old_start..old_end].to_vec()
        } else {
            Vec::new()
        };
        let offset = new_values.len() as u32;
        let size = new_bytes.len() as u32;
        new_values.extend_from_slice(&new_bytes);
        new_meta.push((size, offset));
    }

    // Assemble: header + instructions + updated descs + new values + line numbers
    let inst_size = ystb.hdr.instructions_size as usize;
    let desc_size = ystb.hdr.attr_desc_size as usize;
    let line_size = ystb.hdr.line_numbers_size as usize;

    let mut out = Vec::with_capacity(HEADER_SIZE + inst_size + desc_size + new_values.len() + line_size);
    out.extend_from_slice(&ystb.data[..HEADER_SIZE]);
    write_u32(&mut out, 0x14, new_values.len() as u32);

    let inst_off = HEADER_SIZE;
    out.extend_from_slice(&ystb.data[inst_off..inst_off + inst_size]);

    // Attribute descriptors with patched size/offset
    let mut descs = ystb.data[attr_desc_off..attr_desc_off + desc_size].to_vec();
    for (i, (size, offset)) in new_meta.iter().enumerate() {
        let base = i * ATTR_DESC_SIZE;
        if base + 12 <= descs.len() {
            descs[base + 4..base + 8].copy_from_slice(&size.to_le_bytes());
            descs[base + 8..base + 12].copy_from_slice(&offset.to_le_bytes());
        }
    }
    out.extend_from_slice(&descs);
    out.extend_from_slice(&new_values);

    if line_off + line_size <= ystb.data.len() {
        out.extend_from_slice(&ystb.data[line_off..line_off + line_size]);
    }

    // Re-XOR payload sections (header sizes must already be final).
    toggle_encryption(&mut out, ystb.key);
    Ok(out)
}

/// Extract string entries from one YSTB payload. `rel` is the id prefix;
/// `file_path` is stored for inject routing (loose path or `archive.ypf/inner`).
fn entries_from_ystb_bytes(
    bytes: &[u8],
    rel: &str,
    file_path: PathBuf,
) -> Result<Vec<StringEntry>> {
    let Some(ystb) = load_ystb(bytes, rel)? else {
        return Ok(Vec::new());
    };
    let mut all = Vec::with_capacity(ystb.strings.len());
    for s in &ystb.strings {
        let id = format!("{rel}#arg{}", s.arg_index);
        let mut entry = StringEntry::new(id, &s.text, file_path.clone());
        entry.tags = vec!["dialogue".into()];
        entry.context = Some(format!("attr_type={}", s.attr_type));
        all.push(entry);
    }
    Ok(all)
}

fn translations_from_entries<'a>(
    file_entries: &[&'a StringEntry],
) -> (HashMap<usize, &'a str>, usize) {
    let mut translations = HashMap::new();
    let mut skipped = 0usize;
    for e in file_entries {
        let Some(t) = e.translation.as_deref() else {
            skipped += 1;
            continue;
        };
        if let Some(pos) = e.id.rfind("#arg") {
            if let Ok(idx) = e.id[pos + 4..].parse::<usize>() {
                translations.insert(idx, t);
                continue;
            }
        }
        skipped += 1;
    }
    (translations, skipped)
}

/// Split `ysbin/test.ypf/yst00000.ybn` → (`ysbin/test.ypf` relative path, `yst00000.ybn`).
fn split_ypf_virtual_path(path: &Path) -> Option<(String, String)> {
    let s = path.to_string_lossy().replace('\\', "/");
    let lower = s.to_ascii_lowercase();
    let idx = lower.find(".ypf/")?;
    let archive = s[..=idx + 3].to_string();
    let inner = s[idx + 5..].to_string();
    if inner.is_empty() {
        return None;
    }
    Some((archive, inner))
}

/// Replace `path` with `new_bytes` after moving the original to `path` + `.locust-old`.
/// Restores the backup if the write fails.
fn replace_file_with_backup(path: &Path, new_bytes: &[u8]) -> Result<()> {
    let backup = {
        let mut s = path.to_string_lossy().into_owned();
        s.push_str(".locust-old");
        PathBuf::from(s)
    };
    if backup.exists() {
        std::fs::remove_file(&backup).ok();
    }
    std::fs::rename(path, &backup).map_err(|e| {
        parse_err(
            &path.display().to_string(),
            format!("cannot move aside for backup: {e}"),
        )
    })?;
    if let Err(e) = std::fs::write(path, new_bytes) {
        let _ = std::fs::rename(&backup, path);
        return Err(parse_err(
            &path.display().to_string(),
            format!("write failed after backup (restored): {e}"),
        ));
    }
    Ok(())
}

// ─── FormatPlugin ──────────────────────────────────────────────────────────

impl FormatPlugin for YurisPlugin {
    fn id(&self) -> &str {
        "yuris"
    }

    fn name(&self) -> &str {
        "YU-RIS"
    }

    fn description(&self) -> &str {
        "YU-RIS YSTB .ybn (XOR; Shift-JIS) + YPF unpack/repack (common versions)"
    }

    fn stability(&self) -> locust_core::extraction::FormatStability {
        locust_core::extraction::FormatStability::Experimental
    }

    fn supported_extensions(&self) -> &[&str] {
        &[".ybn", ".ypf"]
    }

    fn supported_modes(&self) -> Vec<OutputMode> {
        vec![OutputMode::Replace]
    }

    fn detect(&self, path: &Path) -> bool {
        Self::has_ybn_or_ypf(path)
    }

    fn extract(&self, path: &Path) -> Result<Vec<StringEntry>> {
        let root = Self::root_dir(path);
        let ybn_files = Self::find_ybn_files(path);
        let ypf_files = Self::find_ypf_files(path);

        if ybn_files.is_empty() && ypf_files.is_empty() {
            return Err(parse_err(
                &path.display().to_string(),
                "no .ybn script files or .ypf archives found",
            ));
        }

        let mut all = Vec::new();

        for fpath in &ybn_files {
            let bytes = std::fs::read(fpath)?;
            let rel = fpath
                .strip_prefix(&root)
                .unwrap_or(fpath.as_path())
                .to_string_lossy()
                .replace('\\', "/");
            // Loose files stay loud: a corrupt YSTB is an Err naming the file
            // (audited contract; warn+skip is only for entries inside a YPF).
            all.extend(entries_from_ystb_bytes(&bytes, &rel, fpath.clone())?);
        }

        let mut ypf_parse_errors = 0usize;
        let mut last_ypf_err = String::new();
        let mut ybn_seen = 0usize;
        let mut ybn_skipped = 0usize;

        for arch_path in &ypf_files {
            let arch_rel = arch_path
                .strip_prefix(&root)
                .unwrap_or(arch_path.as_path())
                .to_string_lossy()
                .replace('\\', "/");
            let archive = match YpfArchive::open(arch_path) {
                Ok(a) => a,
                Err(e) => {
                    ypf_parse_errors += 1;
                    last_ypf_err = e.to_string();
                    warn!(archive = %arch_rel, error = %e, "failed to open YPF");
                    continue;
                }
            };

            for entry in archive.ybn_entries() {
                ybn_seen += 1;
                let payload = match archive.read_entry(entry) {
                    Ok(p) => p,
                    Err(e) => {
                        warn!(
                            archive = %arch_rel,
                            entry = %entry.name,
                            error = %e,
                            "YPF .ybn read failed; skipped"
                        );
                        ybn_skipped += 1;
                        continue;
                    }
                };
                let rel = format!("{arch_rel}/{}", entry.name.replace('\\', "/"));
                let virtual_path = PathBuf::from(&rel);
                match entries_from_ystb_bytes(&payload, &rel, virtual_path) {
                    Ok(entries) => all.extend(entries),
                    Err(e) => {
                        warn!(
                            archive = %arch_rel,
                            entry = %entry.name,
                            error = %e,
                            "YPF .ybn YSTB parse failed; skipped"
                        );
                        ybn_skipped += 1;
                    }
                }
            }
        }

        if all.is_empty() && ybn_files.is_empty() {
            if ypf_parse_errors > 0 && ybn_seen == 0 {
                return Err(parse_err(
                    &path.display().to_string(),
                    format!("failed to parse YPF archive(s): {last_ypf_err}"),
                ));
            }
            if ybn_seen == 0 {
                return Err(parse_err(
                    &path.display().to_string(),
                    "no .ybn scripts found in YPF archives",
                ));
            }
            if ybn_skipped > 0 {
                warn!(
                    skipped = ybn_skipped,
                    "all YPF .ybn entries were skipped (parse/read failures)"
                );
            }
        }

        Ok(all)
    }

    fn inject(&self, path: &Path, entries: &[StringEntry]) -> Result<InjectionReport> {
        let mut files_modified = 0;
        let mut strings_written = 0;
        let mut strings_skipped = 0;
        let mut warnings = Vec::new();
        let mut files_written = Vec::new();

        let mut by_file: HashMap<PathBuf, Vec<&StringEntry>> = HashMap::new();
        for e in entries {
            by_file.entry(e.file_path.clone()).or_default().push(e);
        }

        let search_root = Self::root_dir(path);

        // Group YPF virtual paths by archive relative path
        let mut ypf_groups: HashMap<String, Vec<(String, Vec<&StringEntry>)>> = HashMap::new();
        let mut loose: Vec<(PathBuf, Vec<&StringEntry>)> = Vec::new();

        for (file_path, file_entries) in by_file {
            if let Some((archive, inner)) = split_ypf_virtual_path(&file_path) {
                ypf_groups
                    .entry(archive)
                    .or_default()
                    .push((inner, file_entries));
            } else {
                loose.push((file_path, file_entries));
            }
        }

        // Loose .ybn
        for (file_path, file_entries) in loose {
            let actual = if file_path.exists() {
                file_path.clone()
            } else {
                let as_rel = search_root.join(&file_path);
                if as_rel.exists() {
                    as_rel
                } else {
                    search_root.join(file_path.file_name().unwrap_or_default())
                }
            };
            if !actual.exists() {
                warnings.push(format!("missing script {}", file_path.display()));
                strings_skipped += file_entries.len();
                continue;
            }

            let bytes = match std::fs::read(&actual) {
                Ok(b) => b,
                Err(e) => {
                    warnings.push(format!("read {}: {e}", actual.display()));
                    strings_skipped += file_entries.len();
                    continue;
                }
            };
            let label = actual.display().to_string();
            let ystb = match load_ystb(&bytes, &label) {
                Ok(Some(y)) => y,
                Ok(None) => {
                    warnings.push(format!("skip non-YSTB {}", actual.display()));
                    strings_skipped += file_entries.len();
                    continue;
                }
                Err(e) => {
                    warnings.push(format!("cannot parse {}: {e}", actual.display()));
                    strings_skipped += file_entries.len();
                    continue;
                }
            };

            let (translations, skipped) = translations_from_entries(&file_entries);
            strings_skipped += skipped;
            if translations.is_empty() {
                continue;
            }

            let new_bytes = match inject_into_ystb(&ystb, &translations) {
                Ok(b) => b,
                Err(e) => {
                    warnings.push(format!("inject {}: {e}", actual.display()));
                    strings_skipped += file_entries.len();
                    continue;
                }
            };
            std::fs::write(&actual, &new_bytes)?;
            files_modified += 1;
            files_written.push(actual);
            strings_written += translations.len();
        }

        // YPF archives — rebuild each affected archive in place with .locust-old backup
        for (archive_rel, inners) in ypf_groups {
            let arch_path = {
                let p = search_root.join(&archive_rel);
                if p.exists() {
                    p
                } else {
                    // basename only
                    search_root.join(Path::new(&archive_rel).file_name().unwrap_or_default())
                }
            };
            if !arch_path.exists() {
                warnings.push(format!("missing archive {archive_rel}"));
                for (_, fe) in &inners {
                    strings_skipped += fe.len();
                }
                continue;
            }

            let archive = match YpfArchive::open(&arch_path) {
                Ok(a) => a,
                Err(e) => {
                    warnings.push(format!("cannot open {archive_rel}: {e}"));
                    for (_, fe) in &inners {
                        strings_skipped += fe.len();
                    }
                    continue;
                }
            };

            let mut replacements: HashMap<String, Vec<u8>> = HashMap::new();
            let mut arch_written = 0usize;

            for (inner, file_entries) in inners {
                let entry = match archive
                    .entries
                    .iter()
                    .find(|e| e.name.replace('\\', "/") == inner.replace('\\', "/"))
                {
                    Some(e) => e,
                    None => {
                        warnings.push(format!("entry {inner} not in {archive_rel}"));
                        strings_skipped += file_entries.len();
                        continue;
                    }
                };
                let bytes = match archive.read_entry(entry) {
                    Ok(b) => b,
                    Err(e) => {
                        warnings.push(format!("read {archive_rel}/{inner}: {e}"));
                        strings_skipped += file_entries.len();
                        continue;
                    }
                };
                let label = format!("{archive_rel}/{inner}");
                let ystb = match load_ystb(&bytes, &label) {
                    Ok(Some(y)) => y,
                    Ok(None) => {
                        warnings.push(format!("skip non-YSTB {label}"));
                        strings_skipped += file_entries.len();
                        continue;
                    }
                    Err(e) => {
                        warnings.push(format!("cannot parse {label}: {e}"));
                        strings_skipped += file_entries.len();
                        continue;
                    }
                };
                let (translations, skipped) = translations_from_entries(&file_entries);
                strings_skipped += skipped;
                if translations.is_empty() {
                    continue;
                }
                match inject_into_ystb(&ystb, &translations) {
                    Ok(new_bytes) => {
                        replacements.insert(inner.replace('\\', "/"), new_bytes);
                        arch_written += translations.len();
                    }
                    Err(e) => {
                        warnings.push(format!("inject {label}: {e}"));
                        strings_skipped += file_entries.len();
                    }
                }
            }

            if replacements.is_empty() {
                continue;
            }

            match yuris_ypf::rebuild_ypf(&archive, &replacements) {
                Ok(new_arch) => match replace_file_with_backup(&arch_path, &new_arch) {
                    Ok(()) => {
                        files_modified += 1;
                        files_written.push(arch_path.clone());
                        strings_written += arch_written;
                    }
                    Err(e) => {
                        warnings.push(format!("safe-replace {archive_rel}: {e}"));
                        strings_skipped += arch_written;
                    }
                },
                Err(e) => {
                    warnings.push(format!("rebuild {archive_rel}: {e}"));
                    strings_skipped += arch_written;
                }
            }
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

// ─── Tests (synthetic fixtures only) ───────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn tempdir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("locust_yuris_{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Ground-truth key from real yst00042.ybn audit: bytes B4 62 6A D8 (LE u32).
    const TRUE_KEY_B4626AD8: u32 = u32::from_le_bytes([0xB4, 0x62, 0x6A, 0xD8]);

    /// Expression pushstring with double-quoted body (feeds CP932 scorer + extract).
    fn pushstring_double_quoted(s: &str) -> Vec<u8> {
        let body = format!("\"{}\"", escape_c_light(s));
        let body_bytes = encode_sjis(&body);
        let mut out = Vec::with_capacity(3 + body_bytes.len());
        out.push(PUSH_STRING);
        out.extend_from_slice(&(body_bytes.len() as u16).to_le_bytes());
        out.extend_from_slice(&body_bytes);
        out
    }

    /// Build a valid XORed YSTB. First attr offset is always 0 (key = encrypted attr+8).
    fn build_ystb_with_inst_pad(
        key: u32,
        strings: &[&str],
        extra_inst: &[[u8; 4]],
        zero_inst_pad: usize,
        zero_val_pad: usize,
    ) -> Vec<u8> {
        let value_blobs: Vec<Vec<u8>> = strings
            .iter()
            .map(|s| pushstring_double_quoted(s))
            .collect();

        let mut values = Vec::new();
        let mut offsets = Vec::new();
        for blob in &value_blobs {
            offsets.push(values.len() as u32);
            values.extend_from_slice(blob);
        }
        // Optional zero padding in values (real files often have low-entropy tails).
        let zero_pad = zero_val_pad.max(4);
        for _ in 0..zero_pad {
            values.extend_from_slice(&[0, 0, 0, 0]);
        }
        // Align values section to 4 bytes (real sections are 4-aligned).
        while !values.len().is_multiple_of(4) {
            values.push(0);
        }

        let num_inst = 2u32 + extra_inst.len() as u32 + zero_inst_pad as u32;
        let mut instructions = Vec::new();
        instructions.extend_from_slice(&[0x01, 0x01, 0x00, 0x00]);
        instructions.extend_from_slice(&[0x02, 0x01, 0x00, 0x00]);
        for inst in extra_inst {
            instructions.extend_from_slice(inst);
        }
        for _ in 0..zero_inst_pad {
            instructions.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]);
        }

        let mut descs = Vec::new();
        for (i, blob) in value_blobs.iter().enumerate() {
            descs.extend_from_slice(&0i16.to_le_bytes());
            descs.extend_from_slice(&ATTR_EXPRESSION.to_le_bytes());
            descs.extend_from_slice(&(blob.len() as u32).to_le_bytes());
            descs.extend_from_slice(&offsets[i].to_le_bytes());
        }

        // One line number per instruction (ascending small u32s, as in real YSTB).
        let line_numbers: Vec<u8> = {
            let mut l = Vec::with_capacity(num_inst as usize * 4);
            for i in 1u32..=num_inst {
                l.extend_from_slice(&i.to_le_bytes());
            }
            l
        };

        let mut data = Vec::new();
        data.extend_from_slice(YSTB_MAGIC);
        data.extend_from_slice(&0x22Bu32.to_le_bytes());
        data.extend_from_slice(&num_inst.to_le_bytes());
        data.extend_from_slice(&(num_inst * 4).to_le_bytes());
        data.extend_from_slice(&(descs.len() as u32).to_le_bytes());
        data.extend_from_slice(&(values.len() as u32).to_le_bytes());
        data.extend_from_slice(&(line_numbers.len() as u32).to_le_bytes());
        data.extend_from_slice(&0u32.to_le_bytes());

        data.extend_from_slice(&instructions);
        data.extend_from_slice(&descs);
        data.extend_from_slice(&values);
        data.extend_from_slice(&line_numbers);

        toggle_encryption(&mut data, key);
        data
    }

    /// Standard fixture: first attr offset is 0 → encrypted attr+8 is the key.
    fn build_minimal_ystb(key: u32, s1: &str, s2: &str) -> Vec<u8> {
        build_ystb_with_inst_pad(key, &[s1, s2], &[], 8, 4)
    }

    fn create_fixture(dir: &Path) -> PathBuf {
        let bytes = build_minimal_ystb(
            TRUE_KEY_B4626AD8,
            "Hello, traveler!",
            "Welcome home.",
        );
        let sub = dir.join("ysbin");
        fs::create_dir_all(&sub).unwrap();
        let path = sub.join("yst00001.ybn");
        fs::write(&path, bytes).unwrap();
        dir.to_path_buf()
    }

    #[test]
    fn test_detect_ybn_dir_recursive() {
        let dir = tempdir();
        create_fixture(&dir);
        let plugin = YurisPlugin::new();
        assert!(plugin.detect(&dir));
    }

    #[test]
    fn test_detect_ypf_only_still_true() {
        let dir = tempdir();
        fs::write(dir.join("ysbin.ypf"), b"YPF\0fake").unwrap();
        let plugin = YurisPlugin::new();
        assert!(plugin.detect(&dir));
    }

    #[test]
    fn test_detect_non_yuris() {
        let dir = tempdir();
        fs::write(dir.join("readme.txt"), b"nope").unwrap();
        let plugin = YurisPlugin::new();
        assert!(!plugin.detect(&dir));
    }

    #[test]
    fn test_xor_key_auto_detect() {
        let key = TRUE_KEY_B4626AD8;
        let bytes = build_minimal_ystb(key, "es.FONT.SET", "es.TX.End");
        let hdr = parse_header(&bytes, "t").unwrap();
        let (_, attr_off, _, _) = section_offsets(&hdr);
        // Sole derivation: encrypted first-descriptor offset field (plaintext 0).
        let raw = u32::from_le_bytes(
            bytes[attr_off + 8..attr_off + 12]
                .try_into()
                .expect("12-byte first descriptor"),
        );
        assert_eq!(raw, key, "encrypted attr+8 must be the XOR key verbatim");
        let detected = detect_xor_key(&bytes, &hdr).expect("attr section present");
        assert_eq!(detected, key, "detect_xor_key must read attr_desc+8");
    }

    /// Corrupt the first attribute descriptor so post-decrypt sanity fails —
    /// extract must Err with the filename, never emit garbage strings.
    #[test]
    fn test_bad_first_descriptor_errors_not_garbage() {
        let dir = tempdir();
        let mut bytes = build_minimal_ystb(
            TRUE_KEY_B4626AD8,
            "Hello, traveler!",
            "Welcome home.",
        );
        let hdr = parse_header(&bytes, "t").unwrap();
        let (_, attr_off, _, _) = section_offsets(&hdr);
        // Corrupt only the size field (+4..+8). Leave the key dword at +8 intact:
        // derived key stays correct, but decoded size becomes ~!size and fails
        // size <= values_size. (Flipping the whole descriptor including +8 is
        // self-cancelling: key' = key^FF decrypts plain^FF back to plain.)
        for b in &mut bytes[attr_off + 4..attr_off + 8] {
            *b ^= 0xFF;
        }
        let path = dir.join("corrupt.ybn");
        fs::write(&path, &bytes).unwrap();

        let plugin = YurisPlugin::new();
        let err = plugin.extract(&dir).unwrap_err().to_string();
        assert!(
            err.contains("corrupt.ybn"),
            "error must name the file, got: {err}"
        );
        assert!(
            err.contains("bad XOR key")
                || err.contains("unsupported")
                || err.contains("layout")
                || err.contains("offset"),
            "error must describe bad key/layout, got: {err}"
        );
    }

    #[test]
    fn test_looks_player_visible_rejects_binary_crumbs() {
        assert!(!looks_player_visible("V\u{0003}"));
        assert!(!looks_player_visible("OK")); // short pure-ASCII token
        assert!(!looks_player_visible("12"));
        assert!(looks_player_visible("Hello, traveler"));
        assert!(looks_player_visible("「こんにちは」"));
        assert!(looks_player_visible("Sí"));
    }

    #[test]
    fn test_looks_player_visible_rejects_engine_script_tokens() {
        // YU-RIS script / resource identifiers (Injuu Kangoku RE noise).
        for s in [
            "es.SND",
            "es.SP.WA.SET",
            "ES.CONFIG.VOL.CHARA.EVO.SLIDER.VDEF",
            "MAC.EV",
            "MAC.BG",
            "st01",
            "ysr000",
            "sys005",
            "HSE_056",
            "SE_114",
            "EF01",
            "CUT01",
            "SP001",
            "BTN.PLATE",
            "SCENARIO_TITLE",
            "LNO_CM",
            "black",
            "tran",
            "BACKLOG",
            "ESCMODE",
            "SHIFT+V",
            "tip/foo",
        ] {
            assert!(
                !looks_player_visible(s),
                "engine token should be rejected: {s}"
            );
        }
        // Player-facing dialogue / UI must survive.
        for s in [
            "Hello, traveler",
            "Y entonces...",
            "Para siempre.",
            "John「 Liz!」",
            "Salir",
            "Saltar",
            "Texto",
            "John",
            "*Thud*",
            "「こんにちは」",
            "なし",
        ] {
            assert!(
                looks_player_visible(s),
                "player text should be kept: {s}"
            );
        }
    }

    #[test]
    fn test_ybn_discovery_skips_tool_trees_and_dedupes() {
        let dir = tempdir();
        let pac = dir.join("pac").join("ysbin").join("ysbin");
        let tools = dir.join("VNTranslationTools").join("ysbin");
        let res = dir.join("res");
        fs::create_dir_all(&pac).unwrap();
        fs::create_dir_all(&tools).unwrap();
        fs::create_dir_all(&res).unwrap();
        // Minimal valid-ish file name only — discovery does not parse.
        fs::write(pac.join("yst00000.ybn"), b"YSTB").unwrap();
        fs::write(tools.join("yst00000.ybn"), b"YSTB").unwrap();
        fs::write(res.join("yst00000.ybn"), b"YSTB").unwrap();
        fs::write(res.join("yst00001.ybn"), b"YSTB").unwrap();
        let found = YurisPlugin::find_ybn_files(&dir);
        let names: Vec<String> = found
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().to_string())
            .collect();
        assert_eq!(
            names.iter().filter(|n| n.eq_ignore_ascii_case("yst00000.ybn")).count(),
            1,
            "dedupe basename: {found:?}"
        );
        assert!(
            found.iter().any(|p| p.to_string_lossy().to_ascii_lowercase().contains("pac")),
            "prefer pac/: {found:?}"
        );
        assert!(
            !found.iter().any(|p| p
                .to_string_lossy()
                .to_ascii_lowercase()
                .contains("vntranslationtools")),
            "skip tools: {found:?}"
        );
        assert!(
            found.iter().any(|p| p
                .file_name()
                .unwrap()
                .to_string_lossy()
                .eq_ignore_ascii_case("yst00001.ybn")),
            "unique res file kept: {found:?}"
        );
    }

    #[test]
    fn test_extract_known_strings_and_stable_ids() {
        let dir = tempdir();
        create_fixture(&dir);
        let plugin = YurisPlugin::new();
        let entries = plugin.extract(&dir).unwrap();
        let sources: Vec<&str> = entries.iter().map(|e| e.source.as_str()).collect();
        assert!(
            sources.iter().any(|s| s.contains("Hello, traveler")),
            "missing s1: {sources:?}"
        );
        assert!(
            sources.iter().any(|s| s.contains("Welcome home")),
            "missing s2: {sources:?}"
        );
        assert!(
            entries.iter().any(|e| e.id.ends_with("#arg0")),
            "ids: {:?}",
            entries.iter().map(|e| &e.id).collect::<Vec<_>>()
        );
        assert!(
            entries.iter().any(|e| e.id.contains("yst00001.ybn#arg")),
            "expected relpath#argN ids: {:?}",
            entries.iter().map(|e| &e.id).collect::<Vec<_>>()
        );
        for e in &entries {
            assert!(
                !e.metadata.contains_key("binary_slot"),
                "YU-RIS rebuild must not set binary_slot"
            );
        }
    }

    #[test]
    fn test_inject_roundtrip() {
        let dir = tempdir();
        create_fixture(&dir);
        let plugin = YurisPlugin::new();
        let mut entries = plugin.extract(&dir).unwrap();
        for e in &mut entries {
            if e.source.contains("Hello, traveler") {
                e.translation = Some("¡Hola, viajero!".into());
            }
            if e.source.contains("Welcome home") {
                e.translation = Some("Bienvenido a casa.".into());
            }
        }
        let report = plugin.inject(&dir, &entries).unwrap();
        assert!(report.files_modified >= 1, "{report:?}");

        let again = plugin.extract(&dir).unwrap();
        let sources: Vec<&str> = again.iter().map(|e| e.source.as_str()).collect();
        assert!(
            sources.iter().any(|s| s.contains("Hola, viajero")),
            "re-extract missing: {sources:?}"
        );
        assert!(
            sources.iter().any(|s| s.contains("Bienvenido")),
            "re-extract missing: {sources:?}"
        );
    }

    #[test]
    fn test_ypf_malformed_extract_errors_naming_file() {
        let dir = tempdir();
        fs::write(dir.join("ysbin.ypf"), b"YPF\0fake").unwrap();
        let plugin = YurisPlugin::new();
        let err = plugin.extract(&dir).unwrap_err().to_string();
        assert!(
            err.contains("YPF")
                || err.contains("ypf")
                || err.contains("magic")
                || err.contains("parse")
                || err.contains("small"),
            "expected YPF parse error, got: {err}"
        );
    }

    #[test]
    fn test_ypf_e2e_extract_inject_with_locust_old() {
        let dir = tempdir();
        let ysbin = dir.join("ysbin");
        fs::create_dir_all(&ysbin).unwrap();

        let ystb = build_ystb_with_inst_pad(
            TRUE_KEY_B4626AD8,
            &["Hello, world!", "Second line"],
            &[],
            0,
            0,
        );
        let ypf_bytes = crate::yuris_ypf::write_ypf(
            0x1E4,
            0xFF,
            &[("yst00000.ybn".into(), ystb, true)],
        )
        .unwrap();
        let ypf_path = ysbin.join("test.ypf");
        fs::write(&ypf_path, &ypf_bytes).unwrap();

        let plugin = YurisPlugin::new();
        assert!(plugin.detect(&dir));
        let mut entries = plugin.extract(&dir).unwrap();
        assert!(
            entries.iter().any(|e| e.source.contains("Hello")),
            "missing strings: {:?}",
            entries.iter().map(|e| &e.source).collect::<Vec<_>>()
        );
        assert!(
            entries
                .iter()
                .any(|e| e.id.contains("ysbin/test.ypf/") && e.id.contains("yst00000.ybn#arg")),
            "ids: {:?}",
            entries.iter().map(|e| &e.id).collect::<Vec<_>>()
        );

        for e in &mut entries {
            if e.source.contains("Hello") {
                e.translation = Some("Hola, mundo!".into());
            }
        }
        let report = plugin.inject(&dir, &entries).unwrap();
        assert!(report.files_modified >= 1, "{report:?}");

        let backup = PathBuf::from(format!("{}.locust-old", ypf_path.display()));
        assert!(backup.is_file(), "expected .locust-old backup at {backup:?}");
        assert!(ypf_path.is_file(), "rebuilt ypf must exist");

        let again = plugin.extract(&dir).unwrap();
        assert!(
            again.iter().any(|e| e.source.contains("Hola")),
            "re-extract missing translation: {:?}",
            again.iter().map(|e| &e.source).collect::<Vec<_>>()
        );
        assert!(again.iter().any(|e| e.source.contains("Second line")));
    }

    #[test]
    fn test_ystd_stub_skipped() {
        let dir = tempdir();
        // 16-byte YSTD stub (real games ship pac/ysbin/ysbin/yst.ybn like this)
        let mut stub = Vec::new();
        stub.extend_from_slice(b"YSTD");
        stub.extend_from_slice(&1u32.to_le_bytes());
        stub.extend_from_slice(&0u32.to_le_bytes());
        stub.extend_from_slice(&0u32.to_le_bytes());
        assert_eq!(stub.len(), 16);
        fs::write(dir.join("yst.ybn"), &stub).unwrap();

        // Also a real YSTB so extract succeeds overall
        let ystb = build_minimal_ystb(
            TRUE_KEY_B4626AD8,
            "Only real script text!",
            "Second line ok.",
        );
        fs::write(dir.join("yst00002.ybn"), ystb).unwrap();

        let plugin = YurisPlugin::new();
        let entries = plugin.extract(&dir).unwrap();
        assert!(
            entries.iter().any(|e| e.source.contains("Only real script")),
            "YSTB strings missing: {:?}",
            entries.iter().map(|e| &e.source).collect::<Vec<_>>()
        );
        // Stub contributed nothing and did not error
        assert!(!entries.iter().any(|e| e.id.contains("yst.ybn")));
    }

    #[test]
    fn test_stability_is_experimental() {
        let plugin = YurisPlugin::new();
        assert_eq!(
            plugin.stability(),
            locust_core::extraction::FormatStability::Experimental
        );
    }
}
