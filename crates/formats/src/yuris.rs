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
//! the archive in place with an exclusively owned backup and staged replacement).
//!
//! Command roles come from the shipped YSCM table, with a measured 0x22B fallback.
//! Unknown indirect string roles retain the conservative lexical filter.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::archive_replace::{guard_target, note_backups, replace_files};
use locust_core::error::{LocustError, Result};
use locust_core::extraction::{FormatPlugin, InjectionReport};
use locust_core::models::{OutputMode, StringEntry};
use locust_core::patch::GameLock;
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
            .filter_entry(crate::discovery::is_game_entry)
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
    /// Validated v5 layout (0x22B).
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
    id: i16,
    type_: i16,
    size: u32,
    offset: u32,
    /// IF/ELSE/LOOP raw descriptors hold an instruction index and a values
    /// address, not a byte length and payload (measured on 0x22B).
    branch_target: bool,
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
    let _id = read_i16(data, attr_off)
        .ok_or_else(|| parse_err(file_label, "truncated first attribute id after decrypt"))?;
    let _type = read_i16(data, attr_off + 2)
        .ok_or_else(|| parse_err(file_label, "truncated first attribute type after decrypt"))?;
    let size = read_u32(data, attr_off + 4)
        .ok_or_else(|| parse_err(file_label, "truncated first attribute size after decrypt"))?;
    let offset = read_u32(data, attr_off + 8)
        .ok_or_else(|| parse_err(file_label, "truncated first attribute offset after decrypt"))?;

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

fn parse_attr_descs(
    data: &[u8],
    attr_desc_off: usize,
    attr_desc_size: u32,
) -> Result<Vec<AttrDesc>> {
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
        let id = read_i16(data, off).ok_or_else(|| parse_err("ystb", "truncated attr id"))?;
        let type_ =
            read_i16(data, off + 2).ok_or_else(|| parse_err("ystb", "truncated attr type"))?;
        let size =
            read_u32(data, off + 4).ok_or_else(|| parse_err("ystb", "truncated attr size"))?;
        let offset =
            read_u32(data, off + 8).ok_or_else(|| parse_err("ystb", "truncated attr offset"))?;
        out.push(AttrDesc {
            id,
            type_,
            size,
            offset,
            branch_target: false,
        });
    }
    Ok(out)
}

#[cfg(test)]
fn decode_sjis(bytes: &[u8]) -> String {
    let (cow, _, _had_errors) = encoding_rs::SHIFT_JIS.decode(bytes);
    cow.into_owned()
}

fn encode_sjis(s: &str) -> Result<Vec<u8>> {
    let (bytes, _, had_errors) = encoding_rs::SHIFT_JIS.encode(s);
    if had_errors {
        return Err(parse_err("ystb", "text cannot be encoded in Shift-JIS"));
    }
    Ok(bytes.into_owned())
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

// Decode only at SJIS character boundaries: an 0xEF trail byte is not a control.
// Literal CR/LF bytes use separate tokens so they cannot turn into engine breaks.
const RAW_TOKENS: [(&str, &[u8]); 6] = [
    ("{{yuris:F2}}", &[0xEF, 0xF2]),
    ("{{yuris:F3}}", &[0xEF, 0xF3]),
    ("{{yuris:F5}}", &[0xEF, 0xF5]),
    ("{{yuris:CRLF}}", b"\r\n"),
    ("{{yuris:CR}}", b"\r"),
    ("{{yuris:LF}}", b"\n"),
];

fn sjis_lead(b: u8) -> bool {
    matches!(b, 0x81..=0x9F | 0xE0..=0xFC)
}

fn decode_raw(slice: &[u8]) -> String {
    let mut out = String::new();
    let mut i = 0;
    while i < slice.len() {
        let rest = &slice[i..];
        let token = if rest.starts_with(&[0xEF, 0xF0]) {
            Some(("\r\n", 2))
        } else {
            RAW_TOKENS
                .iter()
                .find_map(|(token, bytes)| rest.starts_with(bytes).then_some((*token, bytes.len())))
        };
        if let Some((token, size)) = token {
            out.push_str(token);
            i += size;
        } else {
            let size = if sjis_lead(slice[i]) && rest.len() > 1 {
                2
            } else {
                1
            };
            let unit = &rest[..size];
            let (s, _, errors) = encoding_rs::SHIFT_JIS.decode(unit);
            // Patched games also contain tunneled glyphs (e.g. 81 01) and
            // literal control bytes. Preserve them as opaque bytes, without
            // guessing a Unicode character or changing them on re-encoding.
            if errors
                || s.chars().any(|c| c.is_control() && c != '\t')
                || encode_sjis(&s).ok().as_deref() != Some(unit)
                || rest.starts_with(b"{{yuris:")
            {
                out.push_str("{{yuris:bytes:");
                for byte in unit {
                    out.push_str(&format!("{byte:02X}"));
                }
                out.push_str("}}");
            } else {
                out.push_str(&s);
            }
            i += size;
        }
    }
    out
}

fn raw_byte_token(text: &str) -> Option<(usize, Vec<u8>)> {
    let hex = text.strip_prefix("{{yuris:bytes:")?.split_once("}}")?.0;
    if !matches!(hex.len(), 2 | 4) || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let bytes = (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).ok())
        .collect::<Option<Vec<_>>>()?;
    Some(("{{yuris:bytes:".len() + hex.len() + 2, bytes))
}

fn encode_raw(text: &str) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    let mut plain = String::new();
    let mut rest = text;
    while !rest.is_empty() {
        if let Some((size, bytes)) = raw_byte_token(rest) {
            out.extend_from_slice(&encode_sjis(&plain)?);
            plain.clear();
            out.extend_from_slice(&bytes);
            rest = &rest[size..];
            continue;
        }
        let token = if rest.starts_with("\r\n") {
            Some((2, &[0xEF, 0xF0][..]))
        } else if rest.starts_with('\n') {
            Some((1, &[0xEF, 0xF0][..]))
        } else {
            RAW_TOKENS
                .iter()
                .find_map(|(token, bytes)| rest.starts_with(token).then_some((token.len(), *bytes)))
        };
        if let Some((size, bytes)) = token {
            out.extend_from_slice(&encode_sjis(&plain)?);
            plain.clear();
            out.extend_from_slice(bytes);
            rest = &rest[size..];
        } else {
            if rest.starts_with("{{yuris:") || rest.starts_with('\r') {
                return Err(parse_err("ystb", "unknown raw control token or bare CR"));
            }
            let c = rest.chars().next().unwrap();
            plain.push(c);
            rest = &rest[c.len_utf8()..];
        }
    }
    out.extend_from_slice(&encode_sjis(&plain)?);
    Ok(out)
}

fn raw_control_sequence(text: &str) -> Vec<&str> {
    let mut controls = Vec::new();
    let mut rest = text;
    while !rest.is_empty() {
        if rest.starts_with("\r\n") || rest.starts_with('\n') {
            controls.push("F0");
            rest = &rest[if rest.starts_with('\r') { 2 } else { 1 }..];
        } else if let Some((token, _)) = RAW_TOKENS.iter().find(|(t, _)| rest.starts_with(t)) {
            controls.push(*token);
            rest = &rest[token.len()..];
        } else if let Some((size, _)) = raw_byte_token(rest) {
            controls.push(&rest[..size]);
            rest = &rest[size..];
        } else {
            rest = &rest[rest.chars().next().unwrap().len_utf8()..];
        }
    }
    controls
}

/// Decode a literal, before applying its command role.
fn decode_attr_value(data: &[u8], attr_vals_off: usize, attr: &AttrDesc) -> Option<String> {
    if attr.branch_target {
        return None;
    }
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
            Some(decode_raw(slice))
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
    let body = &slice[3..3 + arg_len];
    if body.len() < 2 || !matches!(body[0], b'"' | b'\'' | b'`') || body.first() != body.last() {
        return None;
    }
    let (s, _, errors) = encoding_rs::SHIFT_JIS.decode(body);
    if errors {
        return None;
    }
    let s = unquote_string(&s);
    let s = s.replace('\n', "\r\n").replace("\r\r\n", "\r\n");
    Some(s)
}

fn looks_player_visible(s: &str) -> bool {
    let t = s.trim();
    // Opaque bytes are admitted only when the command proves a display role.
    if t.contains("{{yuris:") {
        return false;
    }
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
                let alnum = p
                    .chars()
                    .filter(|c| *c != '_')
                    .all(|c| c.is_ascii_alphanumeric());
                alnum && !p.is_empty() && p.len() <= 28
            })
        {
            return true;
        }
    }

    // snake_case / SCREAMING_SNAKE resource labels.
    if t.contains('_')
        && t.chars()
            .filter(|c| *c != '_')
            .all(|c| c.is_ascii_alphanumeric())
    {
        return true;
    }

    // Short lowercase resource colors / stubs (black, white, tran, trbn).
    if t.len() <= 6
        && t.chars()
            .all(|c| c.is_ascii_alphabetic() && c.is_ascii_lowercase())
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
        ATTR_RAW => encode_raw(text),
        ATTR_EXPRESSION => {
            // Quote with backticks so content may contain both " and ' (VNTextPatch).
            if text.contains('`') {
                return Err(parse_err(
                    "ystb",
                    format!("message cannot contain backticks [{text}]"),
                ));
            }
            let body = format!("`{}`", escape_c_light(text));
            let body_bytes = encode_sjis(&body)?;
            let mut out = Vec::with_capacity(3 + body_bytes.len());
            out.push(PUSH_STRING);
            let n = u16::try_from(body_bytes.len())
                .map_err(|_| parse_err("ystb", "expression string exceeds u16 length"))?;
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
struct YurisCommand {
    name: String,
    params: Vec<String>,
}

type CommandList = Vec<YurisCommand>;

/// Shipped YSCM: 16-byte header, NUL name, u8 argc, NUL parameter + u16 flags.
fn parse_commands(bytes: &[u8]) -> Option<CommandList> {
    if bytes.get(..4)? != b"YSCM" || bytes.len() < 16 || read_u32(bytes, 4)? != 0x22B {
        return None;
    }
    let count = read_u32(bytes, 8)? as usize;
    if count == 0 || count > 256 {
        return None;
    }
    fn string(bytes: &[u8], pos: &mut usize) -> Option<String> {
        let size = bytes.get(*pos..)?.iter().position(|&b| b == 0)?;
        let raw = bytes.get(*pos..*pos + size)?;
        if !raw.is_ascii() {
            return None;
        }
        *pos += size + 1;
        Some(String::from_utf8(raw.to_vec()).ok()?.to_ascii_uppercase())
    }
    let mut pos = 16;
    let mut commands = Vec::with_capacity(count);
    for _ in 0..count {
        let name = string(bytes, &mut pos)?;
        if name.is_empty() {
            return None;
        }
        let argc = *bytes.get(pos)?;
        pos += 1;
        let mut params = Vec::new();
        for _ in 0..argc {
            params.push(string(bytes, &mut pos)?);
            bytes.get(pos..pos + 2)?;
            pos += 2;
        }
        commands.push(YurisCommand { name, params });
    }
    Some(commands)
}

fn sibling_commands(path: &Path) -> Option<CommandList> {
    parse_commands(&std::fs::read(path.parent()?.join("ysc.ybn")).ok()?)
}

/// Measured version 0x22B YSCM IDs (audit yuris-commands.json); never reuse for
/// another version. Only roles needed here are mapped; others stay indirect.
fn fallback_command(op: u8) -> (&'static str, &'static [&'static str]) {
    match op {
        1 => ("CG", &[]),
        2 => ("CGACT", &[]),
        10 => ("DIALOG", &["CAPTION", "STR"]),
        11 => ("ELSE", &[]),
        20 => ("FILEACT", &[]),
        21 => ("FILEINFO", &[]),
        26 => ("FONT", &["BNO", "NO", "NAME"]),
        42 => ("GO", &["#"]),
        43 => ("GOSUB", &["#"]),
        44 => ("IF", &[]),
        55 => ("LOOP", &[]),
        105 => ("WINDOW", &["NO", "CAPTION"]),
        108 => ("WORD", &[""]),
        117 => ("SYSTEMMODE", &[]),
        _ => ("", &[]),
    }
}

#[derive(Clone, Copy, PartialEq)]
enum TextRole {
    Display,
    Technical,
    Indirect,
}

fn attr_role(
    name: &str,
    param: &str,
    arg: usize,
    values: &[Option<String>],
    text_mode: bool,
) -> TextRole {
    match name {
        "WORD" if arg == 0 => TextRole::Display,
        "GOSUB" => {
            if arg == 0 || param == "#" {
                TextRole::Technical
            } else if values.first().and_then(|s| s.as_deref()).is_some_and(|t| {
                t.eq_ignore_ascii_case("ES.CHAR.NAME") || t.eq_ignore_ascii_case("ES.SEL.SET")
            }) {
                TextRole::Display
            } else {
                TextRole::Indirect
            }
        }
        "GO" | "IF" | "ELSE" | "LOOP" => TextRole::Technical,
        "DIALOG" if param == "CAPTION" || param.starts_with("STR") || param == "DEFSTR" => {
            TextRole::Display
        }
        "WINDOW" | "SYSTEMMODE" if param == "CAPTION" => TextRole::Display,
        "CGACT" if param == "SETSTR" => {
            // TEXT=1 means SETSTR is drawn text, not an asset lookup. 0x42 is
            // the shipped push-byte expression; Long is the other scalar form.
            if text_mode {
                TextRole::Display
            } else {
                TextRole::Indirect
            }
        }
        "FONT" if param == "NAME" => TextRole::Technical,
        "FILEACT" | "FILEINFO" => TextRole::Technical,
        "CG" | "CGACT" | "CGEND" | "CGINFO" | "SOUND" | "SOUNDINFO" | "SOUNDEND" | "MOVIE"
        | "SAVE" | "LOAD"
            if matches!(param, "FILE" | "FILE2" | "FOLDER" | "SET" | "SET2") =>
        {
            TextRole::Technical
        }
        _ => TextRole::Indirect,
    }
}

fn display_literal(text: &str) -> bool {
    !text.is_empty()
        && !text.contains('\u{FFFD}')
        && !text
            .chars()
            .any(|c| c.is_control() && !matches!(c, '\r' | '\n' | '\t'))
}

#[derive(Clone, Debug)]
struct DecryptedYstb {
    data: Vec<u8>,
    hdr: YstbHeader,
    key: u32,
    attrs: Vec<AttrDesc>,
    strings: Vec<ExtractedString>,
    caption: bool,
}

#[cfg(test)]
fn load_ystb(bytes: &[u8], file_label: &str) -> Result<Option<DecryptedYstb>> {
    load_ystb_with_commands(bytes, file_label, None)
}

fn load_ystb_with_commands(
    bytes: &[u8],
    file_label: &str,
    commands: Option<&CommandList>,
) -> Result<Option<DecryptedYstb>> {
    if bytes.len() < 4 {
        return Ok(None);
    }
    if &bytes[..4] == b"YSCF" {
        const CAPTION: usize = 0x4C;
        let Some(size) = read_i16(bytes, CAPTION).filter(|&n| n >= 0) else {
            return Ok(None);
        };
        if read_u32(bytes, 4) != Some(0x22B) || CAPTION + 2 + size as usize != bytes.len() {
            return Ok(None);
        }
        let (text, _, errors) = encoding_rs::SHIFT_JIS.decode(&bytes[CAPTION + 2..]);
        if errors || !display_literal(&text) {
            return Ok(None);
        }
        return Ok(Some(DecryptedYstb {
            data: bytes.to_vec(),
            hdr: YstbHeader {
                version: 0,
                num_instructions: 0,
                instructions_size: 0,
                attr_desc_size: 0,
                attr_values_size: 0,
                line_numbers_size: 0,
            },
            key: 0,
            attrs: Vec::new(),
            strings: vec![ExtractedString {
                arg_index: 0,
                attr_index: 0,
                text: text.into_owned(),
                attr_type: ATTR_RAW,
            }],
            caption: true,
        }));
    }
    // Non-text magics (YSTD stub, YSTL, YSCM, …).
    if &bytes[0..4] != YSTB_MAGIC {
        return Ok(None);
    }

    let hdr = parse_header(bytes, file_label)?;
    if hdr.version != 0x22B {
        return Ok(None);
    }
    let mut data = bytes.to_vec();

    // Attr section shorter than one descriptor → no strings (not an error).
    let Some(key) = detect_xor_key(&data, &hdr) else {
        return Ok(Some(DecryptedYstb {
            data,
            hdr,
            key: 0,
            attrs: Vec::new(),
            strings: Vec::new(),
            caption: false,
        }));
    };

    toggle_encryption(&mut data, key);
    verify_first_attr_descriptor(&data, &hdr, file_label)?;

    let (_, attr_desc_off, attr_vals_off, _) = section_offsets(&hdr);
    let mut attrs = parse_attr_descs(&data, attr_desc_off, hdr.attr_desc_size)?;
    let mut strings = Vec::new();
    let mut attr_index = 0;
    for inst in data[HEADER_SIZE..attr_desc_off].chunks_exact(INST_SIZE) {
        let count = inst[1] as usize;
        let end = attr_index + count;
        let Some(group) = attrs.get_mut(attr_index..end) else {
            return Err(parse_err(
                file_label,
                "instruction attribute count exceeds descriptors",
            ));
        };
        let command = commands.and_then(|c| c.get(inst[0] as usize));
        if commands.is_some() && command.is_none() {
            return Ok(None);
        }
        let (fallback_name, fallback_params) = fallback_command(inst[0]);
        let name = command.map_or(fallback_name, |c| c.name.as_str());
        for (arg, attr) in group.iter_mut().enumerate() {
            attr.branch_target = attr.type_ == ATTR_RAW
                && attr.id == 0
                && matches!((name, arg), ("IF" | "ELSE", 1 | 2) | ("LOOP", 1));
            let valid = if attr.branch_target {
                attr.size <= hdr.num_instructions && attr.offset <= hdr.attr_values_size
            } else {
                attr.offset
                    .checked_add(attr.size)
                    .is_some_and(|end| end <= hdr.attr_values_size)
            };
            if !valid {
                return Err(parse_err(
                    file_label,
                    "invalid attribute payload or branch target",
                ));
            }
        }
        let values: Vec<_> = group
            .iter()
            .map(|a| decode_attr_value(&data, attr_vals_off, a))
            .collect();
        let text_id = command.map_or(Some(64), |c| c.params.iter().position(|p| p == "TEXT"));
        let text_mode = name == "CGACT"
            && group.iter().any(|a| {
                if Some(a.id as usize) != text_id {
                    return false;
                }
                let start = attr_vals_off + a.offset as usize;
                let raw = &data[start..start + a.size as usize];
                (a.type_ == ATTR_EXPRESSION && raw == [0x42, 1, 0, 1])
                    || (a.type_ == 1 && raw == 1i32.to_le_bytes())
            });
        for (arg, (attr, text)) in group.iter().zip(&values).enumerate() {
            let fallback_param = if name == "CGACT" && attr.id == 11 {
                "SETSTR"
            } else if name == "CG" && attr.id == 46 {
                "FILE"
            } else if name == "SYSTEMMODE" && attr.id == 12 {
                "CAPTION"
            } else {
                fallback_params.get(attr.id as usize).copied().unwrap_or("")
            };
            let param = command.map_or(fallback_param, |c| {
                c.params.get(attr.id as usize).map_or("", String::as_str)
            });
            let role = attr_role(name, param, arg, &values, text_mode);
            if let Some(text) = text.as_ref().filter(|t| match role {
                TextRole::Display => display_literal(t),
                TextRole::Indirect => looks_player_visible(t),
                TextRole::Technical => false,
            }) {
                strings.push(ExtractedString {
                    arg_index: strings.len(),
                    attr_index: attr_index + arg,
                    text: text.clone(),
                    attr_type: attr.type_,
                });
            }
        }
        attr_index = end;
    }
    if attr_index != attrs.len() {
        return Err(parse_err(file_label, "unassociated attribute descriptors"));
    }

    Ok(Some(DecryptedYstb {
        data,
        hdr,
        key,
        attrs,
        strings,
        caption: false,
    }))
}

fn inject_into_ystb(
    ystb: &DecryptedYstb,
    translations: &HashMap<usize, String>,
) -> Result<Vec<u8>> {
    if ystb.caption {
        let Some(text) = translations.get(&0) else {
            return Ok(ystb.data.clone());
        };
        let caption = encode_sjis(text)?;
        let size = i16::try_from(caption.len())
            .map_err(|_| parse_err("yscf", "caption exceeds i16 length"))?;
        let mut out = ystb.data[..0x4C].to_vec();
        out.extend_from_slice(&size.to_le_bytes());
        out.extend_from_slice(&caption);
        return Ok(out);
    }
    let (_, attr_desc_off, attr_vals_off, line_off) = section_offsets(&ystb.hdr);

    // Build new attribute-values blob; track per-descriptor (size, offset).
    let mut new_values: Vec<u8> = Vec::new();
    let mut new_meta: Vec<(u32, u32)> = Vec::with_capacity(ystb.attrs.len()); // (size, offset)

    // Map attr_index → new text when translated.
    let mut by_attr: HashMap<usize, &str> = HashMap::new();
    for s in &ystb.strings {
        if let Some(t) = translations.get(&s.arg_index) {
            by_attr.insert(s.attr_index, t.as_str());
        }
    }

    let mut edits = Vec::new();
    for (&index, &text) in &by_attr {
        let attr = &ystb.attrs[index];
        let start = attr.offset as usize;
        let end = start + attr.size as usize;
        if ystb.attrs.iter().enumerate().any(|(i, a)| {
            let offset = a.offset as usize;
            i != index
                && offset < end
                && if a.branch_target {
                    start < offset
                } else {
                    start < offset + a.size as usize
                }
        }) {
            return Err(parse_err(
                "ystb",
                "cannot edit aliased/overlapping attribute payload",
            ));
        }
        if raw_control_sequence(
            &ystb
                .strings
                .iter()
                .find(|s| s.attr_index == index)
                .unwrap()
                .text,
        ) != raw_control_sequence(text)
            && attr.type_ == ATTR_RAW
        {
            return Err(parse_err("ystb", "raw control sequence changed"));
        }
        edits.push((start, end, index, serialize_attr_value(attr.type_, text)?));
    }
    edits.sort_by_key(|e| e.0);
    let old_values = &ystb.data[attr_vals_off..line_off];
    let mut cursor = 0;
    for (start, end, _, bytes) in &edits {
        new_values.extend_from_slice(&old_values[cursor..*start]);
        new_values.extend_from_slice(bytes);
        cursor = *end;
    }
    new_values.extend_from_slice(&old_values[cursor..]);
    for (i, attr) in ystb.attrs.iter().enumerate() {
        let mut offset = i64::from(attr.offset);
        for (start, end, index, bytes) in &edits {
            if *index != i && *end <= attr.offset as usize {
                offset += bytes.len() as i64 - (end - start) as i64;
            }
        }
        let size = edits
            .iter()
            .find(|e| e.2 == i)
            .map_or(attr.size as usize, |e| e.3.len());
        new_meta.push((
            u32::try_from(size).map_err(|_| parse_err("ystb", "attribute too large"))?,
            u32::try_from(offset).map_err(|_| parse_err("ystb", "attribute offset overflow"))?,
        ));
    }

    // Assemble: header + instructions + updated descs + new values + line numbers
    let inst_size = ystb.hdr.instructions_size as usize;
    let desc_size = ystb.hdr.attr_desc_size as usize;
    let line_size = ystb.hdr.line_numbers_size as usize;

    let mut out =
        Vec::with_capacity(HEADER_SIZE + inst_size + desc_size + new_values.len() + line_size);
    out.extend_from_slice(&ystb.data[..HEADER_SIZE]);
    write_u32(
        &mut out,
        0x14,
        u32::try_from(new_values.len())
            .map_err(|_| parse_err("ystb", "values section too large"))?,
    );

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

    out.extend_from_slice(&ystb.data[line_off..]);

    // Re-XOR payload sections (header sizes must already be final).
    toggle_encryption(&mut out, ystb.key);
    Ok(out)
}

/// Extract string entries from one YSTB payload. `rel` is the id prefix;
/// `file_path` is stored for inject routing (loose path or `archive.ypf/inner`).
#[cfg(test)]
fn entries_from_ystb_bytes(
    bytes: &[u8],
    rel: &str,
    file_path: PathBuf,
) -> Result<Vec<StringEntry>> {
    entries_with_commands(bytes, rel, file_path, None)
}

/// Fingerprint of everything an injection never rewrites: instructions, each
/// descriptor's id/type (plus a branch's instruction index), line numbers and
/// tail. Payloads, offsets and section sizes are excluded, so a script that
/// Locust already injected (Direct mode) still accepts its own physical rows,
/// while an added, removed or retyped attribute invalidates every old row.
fn layout_context(ystb: &DecryptedYstb) -> String {
    use sha1::{Digest, Sha1};
    let mut h = Sha1::new();
    if ystb.caption {
        h.update(&ystb.data[..0x4C.min(ystb.data.len())]);
    } else {
        let (inst, attr_desc, _, lines) = section_offsets(&ystb.hdr);
        h.update(ystb.hdr.version.to_le_bytes());
        h.update(ystb.hdr.num_instructions.to_le_bytes());
        h.update(&ystb.data[inst..attr_desc]);
        for a in &ystb.attrs {
            h.update(a.id.to_le_bytes());
            h.update(a.type_.to_le_bytes());
            h.update([u8::from(a.branch_target)]);
            if a.branch_target {
                h.update(a.size.to_le_bytes());
            }
        }
        h.update(&ystb.data[lines..]);
    }
    format!("yuris-layout-sha1={:x}", h.finalize())
}

fn entries_with_commands(
    bytes: &[u8],
    rel: &str,
    file_path: PathBuf,
    commands: Option<&CommandList>,
) -> Result<Vec<StringEntry>> {
    let Some(ystb) = load_ystb_with_commands(bytes, rel, commands)? else {
        return Ok(Vec::new());
    };
    let mut all = Vec::with_capacity(ystb.strings.len());
    let context = layout_context(&ystb);
    for s in &ystb.strings {
        let id = if ystb.caption {
            format!("{rel}#caption")
        } else {
            format!("{rel}#attr{}", s.attr_index)
        };
        let mut entry = StringEntry::new(id, &s.text, file_path.clone());
        entry.tags = vec!["dialogue".into()];
        entry.context = Some(context.clone());
        all.push(entry);
    }
    Ok(all)
}

// These errors originate in this module's YSTB serializers. Keep the reason
// separate from the warning (which includes the script name and error details).
fn ystb_skip_reason(error: &LocustError) -> &'static str {
    let LocustError::ParseError { message, .. } = error else {
        return "rebuild_error";
    };
    match message.as_str() {
        "cannot edit aliased/overlapping attribute payload" => "overlapping_locators",
        "text cannot be encoded in Shift-JIS" => "not_encodable",
        "unknown raw control token or bare CR" | "raw control sequence changed" => {
            "unsafe_controls"
        }
        "expression string exceeds u16 length"
        | "caption exceeds i16 length"
        | "attribute too large"
        | "attribute offset overflow"
        | "values section too large" => "too_long",
        _ if message.starts_with("message cannot contain backticks [") => "invalid_translation",
        _ if message.starts_with("cannot serialize attribute type ") => "unsupported",
        _ => "rebuild_error",
    }
}

#[cfg(test)]
fn translations_from_entries(
    file_entries: &[&StringEntry],
    ystb: &DecryptedYstb,
    warnings: &mut Vec<String>,
) -> (HashMap<usize, String>, usize) {
    let (translations, reasons) = translations_with_skip_reasons(file_entries, ystb, warnings);
    (translations, reasons.values().sum())
}

fn translations_with_skip_reasons(
    file_entries: &[&StringEntry],
    ystb: &DecryptedYstb,
    warnings: &mut Vec<String>,
) -> (
    HashMap<usize, String>,
    std::collections::BTreeMap<String, usize>,
) {
    let mut translations = HashMap::new();
    let mut reasons = std::collections::BTreeMap::<String, usize>::new();
    let context = layout_context(ystb);
    for e in file_entries {
        let Some(t) = e.translation.as_deref() else {
            *reasons.entry("untranslated".into()).or_default() += 1;
            continue;
        };
        let index = if ystb.caption && e.id.ends_with("#caption") {
            Some(0)
        } else {
            e.id.rsplit_once("#attr")
                .and_then(|(_, n)| n.parse::<usize>().ok())
        };
        if let Some(index) = index {
            if let Some(s) = ystb.strings.iter().find(|s| s.attr_index == index) {
                // The current text is not compared with `e.source`: after a Direct
                // injection it is the previous translation by design.
                if e.context.as_deref() != Some(context.as_str()) {
                    warnings.push(format!("skip {}: physical layout changed", e.id));
                    *reasons.entry("source_changed".into()).or_default() += 1;
                    continue;
                }
                // Messages carry the author's own hard wraps; a provider hands
                // back one flat line. `escape_c_light` drops bare CR, so
                // rejoining on LF reproduces the file's original `\n` escapes.
                let text = if s.attr_type == ATTR_RAW || ystb.caption {
                    t.to_string()
                } else {
                    crate::rpgmaker_mv::rewrap_to_source_width(&e.source, t)
                };
                // Reject only this string; other translations in the same script
                // can still be written, preserving the original attribute bytes.
                let encoded = if ystb.caption {
                    encode_sjis(&text)
                } else {
                    serialize_attr_value(s.attr_type, &text)
                };
                if let Err(err) = encoded {
                    warnings.push(format!("skip {}: {err}", e.id));
                    *reasons.entry(ystb_skip_reason(&err).into()).or_default() += 1;
                    continue;
                }
                if !ystb.caption
                    && s.attr_type == ATTR_RAW
                    && raw_control_sequence(&s.text) != raw_control_sequence(&text)
                {
                    warnings.push(format!("skip {}: protected control sequence changed", e.id));
                    *reasons.entry("unsafe_controls".into()).or_default() += 1;
                    continue;
                }
                translations.insert(s.arg_index, text);
                continue;
            }
        }
        warnings.push(format!("skip {}: obsolete or unknown YU-RIS locator", e.id));
        *reasons.entry("invalid_locator".into()).or_default() += 1;
    }
    (translations, reasons)
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

#[cfg(test)]
thread_local! {
    static YPF_LOOKUP_NORMALIZATIONS: std::cell::Cell<Option<usize>> = const { std::cell::Cell::new(None) };
}

fn normalize_ypf_lookup_name(name: &str) -> String {
    #[cfg(test)]
    YPF_LOOKUP_NORMALIZATIONS.with(|count| {
        if let Some(n) = count.get() {
            count.set(Some(n + 1));
        }
    });
    name.replace('\\', "/")
}

/// Index each archive once, preserving the first match for duplicate names.
struct YpfMemberIndex<'a> {
    entries: &'a [yuris_ypf::YpfEntry],
    by_name: HashMap<String, usize>,
}

impl<'a> YpfMemberIndex<'a> {
    fn new(entries: &'a [yuris_ypf::YpfEntry]) -> Self {
        let mut by_name = HashMap::with_capacity(entries.len());
        for (i, entry) in entries.iter().enumerate() {
            by_name
                .entry(normalize_ypf_lookup_name(&entry.name))
                .or_insert(i);
        }
        Self { entries, by_name }
    }

    fn find(&self, inner: &str) -> Option<&'a yuris_ypf::YpfEntry> {
        let normalized = normalize_ypf_lookup_name(inner);
        self.by_name.get(&normalized).map(|&i| &self.entries[i])
    }
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
            let commands = sibling_commands(fpath);
            all.extend(entries_with_commands(
                &bytes,
                &rel,
                fpath.clone(),
                commands.as_ref(),
            )?);
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

            let commands = archive
                .ybn_entries()
                .find(|e| {
                    e.name
                        .replace('\\', "/")
                        .rsplit('/')
                        .next()
                        .is_some_and(|n| n.eq_ignore_ascii_case("ysc.ybn"))
                })
                .and_then(|e| archive.read_entry(e).ok())
                .and_then(|b| parse_commands(&b));
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
                match entries_with_commands(&payload, &rel, virtual_path, commands.as_ref()) {
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
        let search_root = Self::root_dir(path);
        let game_lock = GameLock::acquire(if search_root.as_os_str().is_empty() {
            Path::new(".")
        } else {
            &search_root
        })?;
        self.inject_under_lock(path, entries, &game_lock)
    }

    fn inject_under_lock(
        &self,
        path: &Path,
        entries: &[StringEntry],
        game_lock: &GameLock,
    ) -> Result<InjectionReport> {
        game_lock.validate_selection(path)?;
        let mut report = InjectionReport {
            skip_reasons: Default::default(),
            files_modified: 0,
            strings_written: 0,
            strings_skipped: 0,
            warnings: Vec::new(),
            files_written: Vec::new(),
        };

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
                report
                    .warnings
                    .push(format!("missing script {}", file_path.display()));
                report.skip("target_missing", file_entries.len());
                continue;
            }

            guard_target(game_lock, &actual)?;
            let bytes = match std::fs::read(&actual) {
                Ok(b) => b,
                Err(e) => {
                    report
                        .warnings
                        .push(format!("read {}: {e}", actual.display()));
                    report.skip("read_error", file_entries.len());
                    continue;
                }
            };
            let label = actual.display().to_string();
            let commands = sibling_commands(&actual);
            let ystb = match load_ystb_with_commands(&bytes, &label, commands.as_ref()) {
                Ok(Some(y)) => y,
                Ok(None) => {
                    report
                        .warnings
                        .push(format!("skip non-YSTB {}", actual.display()));
                    report.skip("unsupported", file_entries.len());
                    continue;
                }
                Err(e) => {
                    report
                        .warnings
                        .push(format!("cannot parse {}: {e}", actual.display()));
                    report.skip("decode_error", file_entries.len());
                    continue;
                }
            };

            let (translations, skipped) =
                translations_with_skip_reasons(&file_entries, &ystb, &mut report.warnings);
            for (reason, count) in skipped {
                report.skip(&reason, count);
            }
            if translations.is_empty() {
                continue;
            }

            let new_bytes = match inject_into_ystb(&ystb, &translations) {
                Ok(b) => b,
                Err(e) => {
                    report
                        .warnings
                        .push(format!("inject {}: {e}", actual.display()));
                    report.skip(ystb_skip_reason(&e), file_entries.len());
                    continue;
                }
            };
            std::fs::write(&actual, &new_bytes)?;
            report.files_modified += 1;
            report.files_written.push(actual);
            report.strings_written += translations.len();
        }

        // YPF archives — rebuild each affected archive in place with an exclusively owned backup
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
                report
                    .warnings
                    .push(format!("missing archive {archive_rel}"));
                for (_, fe) in &inners {
                    report.skip("target_missing", fe.len());
                }
                continue;
            }

            guard_target(game_lock, &arch_path)?;
            let archive = match YpfArchive::open(&arch_path) {
                Ok(a) => a,
                Err(e) => {
                    report
                        .warnings
                        .push(format!("cannot open {archive_rel}: {e}"));
                    for (_, fe) in &inners {
                        report.skip("archive_error", fe.len());
                    }
                    continue;
                }
            };

            let mut replacements: HashMap<String, Vec<u8>> = HashMap::new();
            let mut arch_written = 0usize;
            let member_index = YpfMemberIndex::new(&archive.entries);
            let commands = archive
                .ybn_entries()
                .find(|e| {
                    e.name
                        .replace('\\', "/")
                        .rsplit('/')
                        .next()
                        .is_some_and(|n| n.eq_ignore_ascii_case("ysc.ybn"))
                })
                .and_then(|e| archive.read_entry(e).ok())
                .and_then(|b| parse_commands(&b));

            for (inner, file_entries) in inners {
                let entry = match member_index.find(&inner) {
                    Some(e) => e,
                    None => {
                        report
                            .warnings
                            .push(format!("entry {inner} not in {archive_rel}"));
                        report.skip("target_missing", file_entries.len());
                        continue;
                    }
                };
                let bytes = match archive.read_entry(entry) {
                    Ok(b) => b,
                    Err(e) => {
                        report
                            .warnings
                            .push(format!("read {archive_rel}/{inner}: {e}"));
                        report.skip("read_error", file_entries.len());
                        continue;
                    }
                };
                let label = format!("{archive_rel}/{inner}");
                let ystb = match load_ystb_with_commands(&bytes, &label, commands.as_ref()) {
                    Ok(Some(y)) => y,
                    Ok(None) => {
                        report.warnings.push(format!("skip non-YSTB {label}"));
                        report.skip("unsupported", file_entries.len());
                        continue;
                    }
                    Err(e) => {
                        report.warnings.push(format!("cannot parse {label}: {e}"));
                        report.skip("decode_error", file_entries.len());
                        continue;
                    }
                };
                let (translations, skipped) =
                    translations_with_skip_reasons(&file_entries, &ystb, &mut report.warnings);
                for (reason, count) in skipped {
                    report.skip(&reason, count);
                }
                if translations.is_empty() {
                    continue;
                }
                match inject_into_ystb(&ystb, &translations) {
                    Ok(new_bytes) => {
                        replacements.insert(inner.replace('\\', "/"), new_bytes);
                        arch_written += translations.len();
                    }
                    Err(e) => {
                        report.warnings.push(format!("inject {label}: {e}"));
                        report.skip(ystb_skip_reason(&e), file_entries.len());
                    }
                }
            }

            if replacements.is_empty() {
                continue;
            }

            match yuris_ypf::rebuild_ypf(&archive, &replacements) {
                Ok(new_arch) => match replace_files(game_lock, &[(arch_path.clone(), new_arch)]) {
                    Ok(backups) => {
                        note_backups(backups, &mut report.warnings);
                        report.files_modified += 1;
                        report.files_written.push(arch_path.clone());
                        report.strings_written += arch_written;
                    }
                    Err(e) => {
                        report
                            .warnings
                            .push(format!("safe-replace {archive_rel}: {e}"));
                        report.skip("write_error", arch_written);
                    }
                },
                Err(e) => {
                    report.warnings.push(format!("rebuild {archive_rel}: {e}"));
                    report.skip("rebuild_error", arch_written);
                }
            }
        }

        Ok(report)
    }
}

// ─── Tests (synthetic fixtures only) ───────────────────────────────────────

#[cfg(test)]
mod tests {
    #[test]
    fn c123_named_skips_for_yuris_rows() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("story.ybn");
        let original = real_layout(&[(108, vec![(0, ATTR_RAW, b"Hello world".to_vec())])], 0);
        fs::write(&path, &original).unwrap();
        let plugin = YurisPlugin::new();
        let entry = plugin.extract(dir.path()).unwrap().remove(0);
        for reason in [
            "untranslated",
            "not_encodable",
            "source_changed",
            "invalid_locator",
            "unsafe_controls",
        ] {
            let mut row = entry.clone();
            row.translation = match reason {
                "untranslated" => None,
                "not_encodable" => Some("\u{1f600}".into()),
                "unsafe_controls" => Some("Hello\rworld".into()),
                _ => Some("Translated".into()),
            };
            if reason == "source_changed" {
                row.context = None;
            }
            if reason == "invalid_locator" {
                row.id = "story.ybn#arg0".into();
            }
            let mut report = plugin.inject(dir.path(), &[row]).unwrap();
            assert_eq!((report.strings_written, report.strings_skipped), (0, 1));
            report.classify_remaining_skips();
            assert_eq!(report.skip_reasons, [(reason.into(), 1)].into(), "{reason}");
            assert_eq!(fs::read(&path).unwrap(), original);
        }
    }

    #[test]
    fn c123_named_skips_for_aliased_yuris_payload() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("story.ybn");
        let mut original = real_layout(
            &[
                (108, vec![(0, ATTR_RAW, b"Hello world".to_vec())]),
                (108, vec![(0, ATTR_RAW, b"Hello world".to_vec())]),
            ],
            0,
        );
        let hdr = parse_header(&original, "story").unwrap();
        let (_, desc, _, _) = section_offsets(&hdr);
        write_u32(&mut original, desc + ATTR_DESC_SIZE + 8, 0);
        fs::write(&path, &original).unwrap();
        let plugin = YurisPlugin::new();
        let mut entries = plugin.extract(dir.path()).unwrap();
        assert_eq!(entries.len(), 2);
        for entry in &mut entries {
            entry.translation = Some(format!("TL {}", entry.source));
        }
        let report = plugin.inject(dir.path(), &entries).unwrap();
        assert_eq!((report.strings_written, report.strings_skipped), (0, 2));
        assert_eq!(
            report.skip_reasons,
            [("overlapping_locators".into(), 2)].into()
        );
        assert_eq!(fs::read(path).unwrap(), original);
    }
    // Real four-byte instructions and twelve-byte descriptors, including
    // values padding and a tail that are deliberately outside all attributes.
    type FixtureCommand = (u8, Vec<(i16, i16, Vec<u8>)>);

    fn real_layout(commands: &[FixtureCommand], key: u32) -> Vec<u8> {
        let mut inst = Vec::new();
        let mut desc = Vec::new();
        let mut values = Vec::new();
        let mut lines = Vec::new();
        for (i, (op, attrs)) in commands.iter().enumerate() {
            inst.extend_from_slice(&[*op, attrs.len() as u8, 0xA7, 0x39]);
            lines.extend_from_slice(&(900 + i as u32).to_le_bytes());
            for (id, typ, raw) in attrs {
                desc.extend_from_slice(&id.to_le_bytes());
                desc.extend_from_slice(&typ.to_le_bytes());
                desc.extend_from_slice(&(raw.len() as u32).to_le_bytes());
                desc.extend_from_slice(&(values.len() as u32).to_le_bytes());
                values.extend_from_slice(raw);
                values.extend_from_slice(b"\0GAP\0");
            }
        }
        let mut out = b"YSTB".to_vec();
        for field in [
            0x22B,
            commands.len() as u32,
            inst.len() as u32,
            desc.len() as u32,
            values.len() as u32,
            lines.len() as u32,
            0xCAFE1234,
        ] {
            out.extend_from_slice(&field.to_le_bytes());
        }
        out.extend(inst);
        out.extend(desc);
        out.extend(values);
        out.extend(lines);
        out.extend_from_slice(b"UNRELATED TAIL\0\xFF");
        toggle_encryption(&mut out, key);
        out
    }

    fn fixture_expr(text: &str) -> Vec<u8> {
        pushstring_double_quoted(text)
    }

    #[test]
    fn vn02_display_tunneled_glyphs_and_literal_controls_preserve_opaque_bytes() {
        let raw = b"Hello\x81\x01\xEF\xF0world\r\x08\x00{{yuris:F2}}\x81".to_vec();
        let original = real_layout(
            &[
                (108, vec![(0, 0, raw.clone())]),
                (250, vec![(0, 0, raw.clone())]),
            ],
            TRUE_KEY_B4626AD8,
        );
        let mut rows = entries_from_ystb_bytes(&original, "story.ybn", "story.ybn".into()).unwrap();
        assert_eq!(rows.len(), 1, "opaque indirect strings must stay excluded");
        assert!(!rows[0].source.contains('\u{FFFD}'));
        let parsed = load_ystb(&original, "story").unwrap().unwrap();
        rows[0].translation = Some(format!("AUDIT {}", rows[0].source));
        let mut warnings = Vec::new();
        let (translations, skipped) =
            translations_from_entries(&[&rows[0]], &parsed, &mut warnings);
        assert_eq!(skipped, 0, "{warnings:?}");
        let changed = inject_into_ystb(&parsed, &translations).unwrap();
        let after = load_ystb(&changed, "story").unwrap().unwrap();
        let (_, _, off, _) = section_offsets(&after.hdr);
        assert_eq!(
            &after.data[off..off + after.attrs[0].size as usize],
            [b"AUDIT ".as_slice(), &raw].concat()
        );
        rows[0].translation = Some(rows[0].source.replace("{{yuris:bytes:8101}}", ""));
        let (translations, skipped) =
            translations_from_entries(&[&rows[0]], &parsed, &mut warnings);
        assert!(translations.is_empty());
        assert_eq!(skipped, 1);
    }

    #[test]
    fn vn02_branch_addresses_are_relocated_without_treating_indices_as_lengths() {
        for opcode in [11, 44, 55] {
            let mut original = real_layout(
                &[
                    (108, vec![(0, 0, b"Hello display".to_vec())]),
                    (opcode, vec![(0, 1, vec![0x42, 1, 0, 1]), (0, 0, vec![])]),
                ],
                0,
            );
            let hdr = parse_header(&original, "branch").unwrap();
            let (_, desc, _, _) = section_offsets(&hdr);
            // Real branch descriptors use size as instruction index, with an
            // address that may be exactly at the end of the values section.
            write_u32(&mut original, desc + 2 * ATTR_DESC_SIZE + 4, 2);
            write_u32(
                &mut original,
                desc + 2 * ATTR_DESC_SIZE + 8,
                hdr.attr_values_size,
            );
            toggle_encryption(&mut original, TRUE_KEY_B4626AD8);
            let before = load_ystb(&original, "branch").unwrap().unwrap();
            assert_eq!(before.strings.len(), 1);
            let output =
                inject_into_ystb(&before, &HashMap::from([(0, "AUDIT Hello display".into())]))
                    .unwrap();
            let after = load_ystb(&output, "branch").unwrap().unwrap();
            assert_eq!(after.attrs[2].size, 2);
            assert_eq!(after.attrs[2].offset, hdr.attr_values_size + 6);
            let (_, _, _, old_lines) = section_offsets(&before.hdr);
            let (_, _, _, new_lines) = section_offsets(&after.hdr);
            assert_eq!(&before.data[old_lines..], &after.data[new_lines..]);
        }
    }

    #[test]
    fn vn02_word_controls_are_reversible_and_protected() {
        for key in [0, TRUE_KEY_B4626AD8, 0x12345678] {
            for raw in [
                b"Hello\xEF\xF0world".to_vec(),
                [
                    encode_sjis("日本語").unwrap(),
                    b"\xEF\xF0".to_vec(),
                    encode_sjis("世界").unwrap(),
                    b"\xEF\xF2\xEF\xF3\xEF\xF5\xEF\xF2\\p".to_vec(),
                ]
                .concat(),
            ] {
                let original = real_layout(&[(108, vec![(0, ATTR_RAW, raw.clone())])], key);
                let mut entries =
                    entries_from_ystb_bytes(&original, "story.ybn", "story.ybn".into()).unwrap();
                assert_eq!(entries.len(), 1);
                assert!(entries[0].source.contains("\r\n"));
                entries[0].translation = Some(format!("AUDIT {}", entries[0].source));
                let before = load_ystb(&original, "story").unwrap().unwrap();
                let mut warnings = Vec::new();
                let (translations, skipped) =
                    translations_from_entries(&[&entries[0]], &before, &mut warnings);
                assert_eq!(skipped, 0, "{warnings:?}");
                let output = inject_into_ystb(&before, &translations).unwrap();
                let after = load_ystb(&output, "story").unwrap().unwrap();
                let (_, _, off, _) = section_offsets(&after.hdr);
                let a = &after.attrs[0];
                assert_eq!(
                    &after.data[off..off + a.size as usize],
                    [b"AUDIT ".as_slice(), &raw].concat()
                );
                let again =
                    entries_from_ystb_bytes(&output, "story.ybn", "story.ybn".into()).unwrap();
                assert_eq!(again[0].source, entries[0].translation.as_deref().unwrap());
            }
        }
    }

    #[test]
    fn vn02_raw_controls_cannot_be_dropped_reordered_or_retyped() {
        let original = real_layout(
            &[(
                108,
                vec![(0, 0, b"Hello\xEF\xF2world\xEF\xF3\xEF\xF5".to_vec())],
            )],
            0,
        );
        let mut entries =
            entries_from_ystb_bytes(&original, "story.ybn", "story.ybn".into()).unwrap();
        assert_eq!(entries.len(), 1);
        let parsed = load_ystb(&original, "story").unwrap().unwrap();
        for text in [
            "Translated.",
            "{{yuris:F3}}{{yuris:F2}}{{yuris:F5}}",
            "{{yuris:F2}}{{yuris:F3}}{{yuris:F2}}",
        ] {
            entries[0].translation = Some(text.into());
            let mut warnings = Vec::new();
            let (ts, skipped) = translations_from_entries(&[&entries[0]], &parsed, &mut warnings);
            assert!(ts.is_empty());
            assert_eq!(skipped, 1);
        }
    }

    #[test]
    fn vn02_literal_newlines_and_sjis_trail_bytes_keep_their_raw_type() {
        // 88 EF is a complete CP932 character; EF is its trail byte.
        let raw = b"Hello\r\n\n\r\x88\xEF\xEF\xF2\\p".to_vec();
        let original = real_layout(&[(108, vec![(0, 0, raw.clone())])], TRUE_KEY_B4626AD8);
        let rows = entries_from_ystb_bytes(&original, "story.ybn", "story.ybn".into()).unwrap();
        assert_eq!(rows.len(), 1);
        let parsed = load_ystb(&original, "story").unwrap().unwrap();
        let changed = inject_into_ystb(
            &parsed,
            &HashMap::from([(0, format!("AUDIT {}", rows[0].source))]),
        )
        .unwrap();
        let after = load_ystb(&changed, "story").unwrap().unwrap();
        let (_, _, off, _) = section_offsets(&after.hdr);
        assert_eq!(
            &after.data[off..off + after.attrs[0].size as usize],
            [b"AUDIT ".as_slice(), &raw].concat()
        );
    }

    #[test]
    fn vn02_roles_admit_short_display_and_exclude_lookups() {
        let commands = vec![
            (108, vec![(0, 0, b"...".to_vec())]),
            (108, vec![(0, 0, b"A".to_vec())]),
            (108, vec![(0, 0, b"OK".to_vec())]),
            (26, vec![(2, 3, fixture_expr("MS Gothic"))]),
            (43, vec![(0, 3, fixture_expr("Japanese dispatch target"))]),
            (
                43,
                vec![
                    (0, 3, fixture_expr("ES.CHAR.NAME")),
                    (33, 3, fixture_expr("Q")),
                ],
            ),
            (
                43,
                vec![
                    (0, 3, fixture_expr("ES.SEL.SET")),
                    (33, 3, fixture_expr("...")),
                ],
            ),
            (
                2,
                vec![(64, 3, vec![0x42, 1, 0, 1]), (11, 3, fixture_expr("UI"))],
            ),
        ];
        let rows = entries_from_ystb_bytes(
            &real_layout(&commands, TRUE_KEY_B4626AD8),
            "story.ybn",
            "story.ybn".into(),
        )
        .unwrap();
        assert_eq!(
            rows.iter().map(|r| r.source.as_str()).collect::<Vec<_>>(),
            ["...", "A", "OK", "Q", "...", "UI"]
        );
    }

    #[test]
    fn vn02_shipped_yscm_overrides_version_fallback() {
        let dir = tempfile::tempdir().unwrap();
        let mut table = b"YSCM".to_vec();
        for n in [0x22Bu32, 2, 0] {
            table.extend_from_slice(&n.to_le_bytes());
        }
        table.extend_from_slice(b"WORD\0\x01\0\0\0FONT\0\x01NAME\0\0\0");
        fs::write(dir.path().join("ysc.ybn"), table).unwrap();
        fs::write(
            dir.path().join("story.ybn"),
            real_layout(
                &[
                    (0, vec![(0, 0, b"X".to_vec())]),
                    (1, vec![(0, 3, fixture_expr("MS Gothic"))]),
                ],
                0,
            ),
        )
        .unwrap();
        let rows = YurisPlugin::new().extract(dir.path()).unwrap();
        assert_eq!(
            rows.iter().map(|r| r.source.as_str()).collect::<Vec<_>>(),
            ["X"]
        );
    }

    #[test]
    fn vn02_yscf_caption_changes_only_length_and_caption() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("yscfg.ybn");
        let mut original: Vec<_> = (0..0x4Cu8).collect();
        original[..4].copy_from_slice(b"YSCF");
        original[4..8].copy_from_slice(&0x22Bu32.to_le_bytes());
        original.extend_from_slice(&5i16.to_le_bytes());
        original.extend_from_slice(b"Title");
        fs::write(&path, &original).unwrap();
        let plugin = YurisPlugin::new();
        let mut rows = plugin.extract(dir.path()).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].source, "Title");
        rows[0].translation = Some("New project title".into());
        assert_eq!(plugin.inject(dir.path(), &rows).unwrap().strings_written, 1);
        let output = fs::read(path).unwrap();
        assert_eq!(output[..0x4C], original[..0x4C]);
        assert_eq!(
            &output[0x4C..],
            [17i16.to_le_bytes().as_slice(), b"New project title"].concat()
        );
        assert_eq!(
            plugin.extract(dir.path()).unwrap()[0].source,
            "New project title"
        );
        let mut unsupported = original;
        unsupported.push(0);
        assert!(
            entries_from_ystb_bytes(&unsupported, "yscfg.ybn", "yscfg.ybn".into())
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn vn02_stale_dense_and_physical_rows_never_overwrite_shifted_attributes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("story.ybn");
        let plugin = YurisPlugin::new();
        let first = real_layout(
            &[
                (108, vec![(0, 0, b"Same display".to_vec())]),
                (108, vec![(0, 0, b"Existing display".to_vec())]),
            ],
            0,
        );
        fs::write(&path, &first).unwrap();
        let mut rows = plugin.extract(dir.path()).unwrap();
        rows[1].translation = Some("DO NOT OVERWRITE".into());
        let physical = rows[1].clone();
        rows[1].id = "story.ybn#arg1".into();
        for changed in [
            real_layout(
                &[
                    (108, vec![(0, 0, b"Inserted display".to_vec())]),
                    (108, vec![(0, 0, b"Same display".to_vec())]),
                    (108, vec![(0, 0, b"Existing display".to_vec())]),
                ],
                0,
            ),
            real_layout(&[(108, vec![(0, 0, b"Existing display".to_vec())])], 0),
            real_layout(
                &[
                    (108, vec![(0, 0, b"Existing display".to_vec())]),
                    (108, vec![(0, 0, b"Existing display".to_vec())]),
                    (108, vec![(1, 0, b"Retyped id".to_vec())]),
                ],
                0,
            ),
        ] {
            fs::write(&path, &changed).unwrap();
            for entry in [&rows[1], &physical] {
                let report = plugin
                    .inject(dir.path(), std::slice::from_ref(entry))
                    .unwrap();
                assert_eq!(report.strings_written, 0, "{report:?}");
                assert_eq!(report.strings_skipped, 1);
                assert_eq!(fs::read(&path).unwrap(), changed);
            }
        }
    }

    #[test]
    fn vn02_reinjection_into_an_injected_script_writes_the_edited_translation() {
        // Direct mode writes into the game file, so the next injection (after the
        // user fixes a translation) reads a file whose payloads and section sizes
        // already changed. Physical rows must still apply.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("story.ybn");
        let plugin = YurisPlugin::new();
        let original = real_layout(
            &[
                (108, vec![(0, 0, b"First display".to_vec())]),
                (108, vec![(0, 0, b"Second display".to_vec())]),
            ],
            TRUE_KEY_B4626AD8,
        );
        fs::write(&path, &original).unwrap();
        let mut rows = plugin.extract(dir.path()).unwrap();
        assert_eq!(rows.len(), 2);
        rows[0].translation = Some("Primera linea".into());
        rows[1].translation = Some("Segunda linea".into());
        let first = plugin.inject(dir.path(), &rows).unwrap();
        assert_eq!(first.strings_written, 2, "{first:?}");

        rows[1].translation = Some("Segunda linea corregida".into());
        let second = plugin.inject(dir.path(), &rows).unwrap();
        assert_eq!(second.strings_written, 2, "{second:?}");
        assert_eq!(second.strings_skipped, 0, "{second:?}");
        let after: Vec<_> = plugin
            .extract(dir.path())
            .unwrap()
            .into_iter()
            .map(|e| e.source)
            .collect();
        assert_eq!(after, ["Primera linea", "Segunda linea corregida"]);
    }

    #[test]
    fn vn02_changed_attribute_preserves_sections_gaps_tail_and_other_payloads() {
        let original = real_layout(
            &[
                (108, vec![(0, 0, b"Hello\xEF\xF0world".to_vec())]),
                (26, vec![(2, 3, fixture_expr("MS Gothic"))]),
                (108, vec![(0, 3, fixture_expr("Untouched display"))]),
                (250, vec![(78, 1, vec![0xDE, 0xAD, 0xBE, 0xEF])]),
            ],
            TRUE_KEY_B4626AD8,
        );
        let before = load_ystb(&original, "story").unwrap().unwrap();
        let output = inject_into_ystb(
            &before,
            &HashMap::from([(0, "AUDIT Hello\r\nworld".into())]),
        )
        .unwrap();
        let after = load_ystb(&output, "story").unwrap().unwrap();
        let (_, bd, bv, bl) = section_offsets(&before.hdr);
        let (_, ad, av, al) = section_offsets(&after.hdr);
        assert_eq!(&before.data[..0x14], &after.data[..0x14]);
        assert_eq!(&before.data[0x18..bd], &after.data[0x18..ad]);
        assert_eq!(&before.data[bl..], &after.data[al..]);
        assert_eq!(
            &after.data[av..al],
            [b"AUDIT ".as_slice(), &before.data[bv..bl]].concat()
        );
        for (i, (a, b)) in before.attrs.iter().zip(&after.attrs).enumerate() {
            assert_eq!(
                &before.data[bd + i * 12..bd + i * 12 + 4],
                &after.data[ad + i * 12..ad + i * 12 + 4]
            );
            if i > 0 {
                assert_eq!(
                    &before.data[bv + a.offset as usize..bv + a.offset as usize + a.size as usize],
                    &after.data[av + b.offset as usize..av + b.offset as usize + b.size as usize]
                );
            }
        }
    }

    #[test]
    fn vn02_unknown_layouts_are_safe_skips_or_errors() {
        let mut original = real_layout(&[(108, vec![(0, 0, b"Visible".to_vec())])], 0);
        write_u32(&mut original, 4, 0x99);
        assert!(
            entries_from_ystb_bytes(&original, "story.ybn", "story.ybn".into())
                .unwrap()
                .is_empty()
        );
        write_u32(&mut original, 4, 0x22B);
        original[33] = 0;
        assert!(entries_from_ystb_bytes(&original, "story.ybn", "story.ybn".into()).is_err());
    }

    fn lookup_members(names: impl IntoIterator<Item = String>) -> Vec<yuris_ypf::YpfEntry> {
        names
            .into_iter()
            .map(|name| yuris_ypf::YpfEntry {
                name,
                index_name: Vec::new(),
                name_hash: 0,
                file_type: 0,
                is_packed: false,
                unpacked_size: 0,
                packed_size: 0,
                offset: 0,
                checksum: 0,
                extra: Vec::new(),
            })
            .collect()
    }

    #[test]
    fn ypf_member_index_preserves_first_match_and_name_rules() {
        let members = lookup_members(
            [
                "scripts\\first.ybn",
                "scripts/first.ybn",
                "scripts/second.ybn",
                "Scripts/first.ybn",
            ]
            .map(String::from),
        );
        let index = YpfMemberIndex::new(&members);
        for (name, expected) in [
            ("scripts/first.ybn", 0),
            ("scripts\\first.ybn", 0),
            ("scripts\\second.ybn", 2),
            ("Scripts/first.ybn", 3),
        ] {
            assert!(std::ptr::eq(index.find(name).unwrap(), &members[expected]));
        }
        assert!(index.find("scripts/FIRST.ybn").is_none());
        assert!(index.find("scripts/missing.ybn").is_none());
        assert!(YpfMemberIndex::new(&[]).find("missing.ybn").is_none());
    }

    #[test]
    fn ypf_member_index_normalizations_scale_with_members_plus_scripts() {
        // Place scripts after unrelated assets so a per-script scan is costly.
        let members = lookup_members(
            (0..16_000)
                .map(|i| format!("images\\asset{i:05}.png"))
                .chain((0..2_000).map(|i| format!("scripts\\yst{i:05}.ybn"))),
        );
        let scripts: Vec<_> = (0..2_000)
            .map(|i| format!("scripts/yst{i:05}.ybn"))
            .collect();

        YPF_LOOKUP_NORMALIZATIONS.with(|count| count.set(Some(0)));
        let index = YpfMemberIndex::new(&members);
        let member_normalizations =
            YPF_LOOKUP_NORMALIZATIONS.with(|count| count.replace(Some(0)).unwrap());
        for (i, script) in scripts.iter().enumerate() {
            assert!(std::ptr::eq(
                index.find(script).unwrap(),
                &members[16_000 + i]
            ));
        }
        let lookup_normalizations =
            YPF_LOOKUP_NORMALIZATIONS.with(|count| count.replace(None).unwrap());
        eprintln!(
            "YPF normalizations: members={member_normalizations}, lookups={lookup_normalizations}"
        );
        assert!(
            member_normalizations <= 20_000,
            "member normalizations: {member_normalizations} > 20000"
        );
        assert!(
            lookup_normalizations <= 2_000,
            "lookup normalizations: {lookup_normalizations} > 2000"
        );
    }

    #[test]
    fn ypf_missing_members_keep_warning_and_skipped_count() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("game.ypf");
        let ystb = build_minimal_ystb(TRUE_KEY_B4626AD8, "Hello original!", "Untouched line");
        let original =
            yuris_ypf::write_ypf(0x1E4, 0xFF, &[("scripts/present.ybn".into(), ystb, true)])
                .unwrap();
        fs::write(&path, &original).unwrap();
        let mut entries = Vec::new();
        for inner in ["scripts/missing.ybn", "scripts/PRESENT.ybn"] {
            for arg in 0..2 {
                let virtual_path = format!("game.ypf/{inner}");
                let mut entry = StringEntry::new(
                    format!("{virtual_path}#arg{arg}"),
                    "Original text",
                    PathBuf::from(virtual_path),
                );
                entry.translation = Some("Translated text".into());
                entries.push(entry);
            }
        }

        let report = YurisPlugin::new().inject(root.path(), &entries).unwrap();
        assert_eq!(report.strings_skipped, 4, "{report:?}");
        assert_eq!(report.strings_written, 0, "{report:?}");
        assert_eq!(report.files_modified, 0, "{report:?}");
        assert!(report.files_written.is_empty(), "{report:?}");
        let mut warnings = report.warnings;
        warnings.sort();
        assert_eq!(
            warnings,
            [
                "entry scripts/PRESENT.ybn not in game.ypf",
                "entry scripts/missing.ybn not in game.ypf",
            ]
        );
        assert_eq!(fs::read(&path).unwrap(), original);
    }

    #[test]
    fn held_lock_injects_loose_and_archive_without_releasing_exclusion() {
        for packed in [false, true] {
            let root = tempfile::tempdir().unwrap();
            let other = tempfile::tempdir().unwrap();
            let bytes = build_ystb_with_inst_pad(
                TRUE_KEY_B4626AD8,
                &["Hello original!", "Untouched line"],
                &[],
                0,
                0,
            );
            let path = if packed {
                let path = root.path().join("game.ypf");
                fs::write(
                    &path,
                    crate::yuris_ypf::write_ypf(
                        0x1E4,
                        0xFF,
                        &[("yst00000.ybn".into(), bytes, true)],
                    )
                    .unwrap(),
                )
                .unwrap();
                path
            } else {
                let path = root.path().join("yst00000.ybn");
                fs::write(&path, bytes).unwrap();
                path
            };
            let original = fs::read(&path).unwrap();
            let plugin = YurisPlugin::new();
            let mut entries = plugin.extract(root.path()).unwrap();
            entries.retain(|e| e.source == "Hello original!");
            assert_eq!(entries.len(), 1);
            entries[0].translation = Some("Texto traducido.".into());
            let wrong = GameLock::acquire(other.path()).unwrap();
            assert!(plugin
                .inject_under_lock(root.path(), &entries, &wrong)
                .is_err());
            assert_eq!(fs::read(&path).unwrap(), original);
            let lock = GameLock::acquire(root.path()).unwrap();
            assert!(plugin.inject(root.path(), &entries).is_err());
            assert_eq!(fs::read(&path).unwrap(), original);
            let report = plugin
                .inject_under_lock(root.path(), &entries, &lock)
                .unwrap();
            assert_eq!(report.strings_written, 1, "{report:?}");
            assert!(plugin
                .extract(root.path())
                .unwrap()
                .iter()
                .any(|e| e.source == "Texto traducido."));
            assert!(GameLock::acquire(root.path()).is_err());
            drop(lock);
            assert!(GameLock::acquire(root.path()).is_ok());
        }
    }

    #[test]
    fn c121_ypf_direct_revisions_exclude_private_backups() {
        use locust_core::{backup::BackupManager, database::Database, extraction::inject_direct};
        let root = tempfile::tempdir().unwrap();
        let archive = root.path().join("ysbin.ypf");
        let ystb = build_minimal_ystb(TRUE_KEY_B4626AD8, "Hello original!", "Welcome home.");
        let original =
            crate::yuris_ypf::write_ypf(0x1E4, 0xFF, &[("yst00000.ybn".into(), ystb, true)])
                .unwrap();
        fs::write(&archive, &original).unwrap();
        let registry = crate::default_registry();
        let plugin = registry.get("yuris").unwrap();
        let db = Database::open_in_memory().unwrap();
        let backups = tempfile::tempdir().unwrap();
        let manager = BackupManager::new(backups.path().to_owned());
        let mut entries = plugin.extract(root.path()).unwrap();
        assert_eq!(entries.len(), 2);
        let mut pristine = None;
        for prefix in ["TL ", "R2 TL "] {
            for entry in &mut entries {
                entry.translation = Some(format!("{prefix}{}", entry.source));
            }
            db.save_entries(&entries).unwrap();
            let report = inject_direct(
                &registry,
                &db,
                &manager,
                root.path(),
                "yuris",
                &["en".into()],
            )
            .expect("YPF Direct injection must recognize its retained staging backup");
            assert_eq!(
                (report.strings_written, report.strings_skipped),
                (2, 0),
                "{report:?}"
            );
            assert_eq!(report.files_modified, 1);
            assert!(report.reports["en"].skip_reasons.is_empty());
            let extracted = plugin.extract(root.path()).unwrap();
            assert_eq!(extracted.len(), entries.len());
            for entry in &entries {
                assert!(extracted
                    .iter()
                    .any(|out| Some(&out.source) == entry.translation.as_ref()));
            }
            assert!(walkdir::WalkDir::new(root.path()).into_iter().all(|entry| {
                !entry
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".locust-stage-")
            }));
            let id = report.pristine_backup_id.unwrap();
            if let Some(first) = &pristine {
                assert_eq!(&id, first);
            } else {
                pristine = Some(id);
            }
        }
        manager.restore(&pristine.unwrap()).unwrap();
        assert_eq!(fs::read(archive).unwrap(), original);
    }

    #[test]
    fn ypf_reinjection_keeps_original_backup_sidecar_and_quoted_text() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("game.ypf");
        let ystb = build_ystb_with_inst_pad(
            TRUE_KEY_B4626AD8,
            &["Hello original!", "Untouched line"],
            &[],
            0,
            0,
        );
        let original =
            crate::yuris_ypf::write_ypf(0x1E4, 0xFF, &[("yst00000.ybn".into(), ystb, true)])
                .unwrap();
        fs::write(&path, &original).unwrap();
        let sidecar = root.path().join("game.ypf.locust-old");
        fs::write(&sidecar, b"unrelated exact bytes").unwrap();
        let plugin = YurisPlugin::new();
        let first_text = "She said \"yes\"; it's fine.";
        let mut entries = plugin.extract(root.path()).unwrap();
        for e in &mut entries {
            if e.source == "Hello original!" {
                e.translation = Some(first_text.into());
            }
        }
        let first = plugin.inject(root.path(), &entries).unwrap();
        assert_eq!(first.strings_written, 1, "{first:?}");
        let first_backup = backup_path(&first);
        let first_bytes = fs::read(&path).unwrap();
        let mut entries = plugin.extract(root.path()).unwrap();
        assert!(
            entries.iter().any(|e| e.source == first_text),
            "{entries:?}"
        );
        for e in &mut entries {
            if e.source == first_text {
                e.translation = Some("Second \"quoted\" pass.".into());
            }
        }
        let second = plugin.inject(root.path(), &entries).unwrap();
        assert_eq!(second.strings_written, 1);
        let second_backup = backup_path(&second);
        assert_ne!(first_backup, second_backup);
        assert_eq!(fs::read(first_backup).unwrap(), original);
        assert_eq!(fs::read(second_backup).unwrap(), first_bytes);
        assert_eq!(fs::read(sidecar).unwrap(), b"unrelated exact bytes");
        assert_eq!(second.files_written, vec![path]);
        let final_entries = plugin.extract(root.path()).unwrap();
        assert_eq!(final_entries.len(), 2);
        assert!(final_entries
            .iter()
            .any(|e| e.source == "Second \"quoted\" pass."));
        assert!(final_entries.iter().any(|e| e.source == "Untouched line"));
    }
    fn backup_path(report: &locust_core::extraction::InjectionReport) -> std::path::PathBuf {
        report
            .warnings
            .iter()
            .find_map(|w| w.strip_prefix("previous archive retained at "))
            .map(std::path::PathBuf::from)
            .expect("reported owned backup")
    }

    #[test]
    fn unrelated_backup_sentinel_survives_replacement() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("game.ypf");
        std::fs::write(&path, b"original archive").unwrap();
        let sidecar = dir.path().join("game.ypf.locust-old");
        std::fs::write(&sidecar, b"unrelated exact bytes").unwrap();
        let lock = locust_core::patch::GameLock::acquire(dir.path()).unwrap();
        crate::archive_replace::replace_files(&lock, &[(path, b"new archive".to_vec())]).unwrap();
        assert_eq!(std::fs::read(sidecar).unwrap(), b"unrelated exact bytes");
    }
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
        let body_bytes = encode_sjis(&body).unwrap();
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
        instructions.extend_from_slice(&[108, 0x01, 0x00, 0x00]);
        instructions.extend_from_slice(&[108, 0x01, 0x00, 0x00]);
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
        let bytes = build_minimal_ystb(TRUE_KEY_B4626AD8, "Hello, traveler!", "Welcome home.");
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
        let mut bytes = build_minimal_ystb(TRUE_KEY_B4626AD8, "Hello, traveler!", "Welcome home.");
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
            assert!(looks_player_visible(s), "player text should be kept: {s}");
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
            names
                .iter()
                .filter(|n| n.eq_ignore_ascii_case("yst00000.ybn"))
                .count(),
            1,
            "dedupe basename: {found:?}"
        );
        assert!(
            found
                .iter()
                .any(|p| p.to_string_lossy().to_ascii_lowercase().contains("pac")),
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
            entries.iter().any(|e| e.id.ends_with("#attr0")),
            "ids: {:?}",
            entries.iter().map(|e| &e.id).collect::<Vec<_>>()
        );
        assert!(
            entries.iter().any(|e| e.id.contains("yst00001.ybn#attr")),
            "expected relpath#attrN ids: {:?}",
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
    fn test_serialize_attr_value_rejects_unmappable_shift_jis() {
        for attr_type in [ATTR_RAW, ATTR_EXPRESSION] {
            let err = serialize_attr_value(attr_type, "Hola, señor.").unwrap_err();
            assert!(err.to_string().contains("Shift-JIS"), "{err}");
        }
    }

    #[test]
    fn test_serialize_attr_value_shift_jis_byte_exact_roundtrip() {
        for (text, expected) in [
            ("Hola, viajero.", b"Hola, viajero.".as_slice()),
            ("日本語", &[0x93, 0xFA, 0x96, 0x7B, 0x8C, 0xEA][..]),
        ] {
            let raw = serialize_attr_value(ATTR_RAW, text).unwrap();
            assert_eq!(raw, expected);
            assert_eq!(decode_sjis(&raw), text);
            assert_eq!(
                serialize_attr_value(ATTR_RAW, &decode_sjis(&raw)).unwrap(),
                raw
            );

            let expression = serialize_attr_value(ATTR_EXPRESSION, text).unwrap();
            let mut expected_expression = vec![PUSH_STRING];
            expected_expression.extend_from_slice(&((expected.len() + 2) as u16).to_le_bytes());
            expected_expression.push(b'`');
            expected_expression.extend_from_slice(expected);
            expected_expression.push(b'`');
            assert_eq!(expression, expected_expression);
            let decoded = evaluate_push_string(&expression).unwrap();
            assert_eq!(decoded, text);
            assert_eq!(
                serialize_attr_value(ATTR_EXPRESSION, &decoded).unwrap(),
                expression
            );
        }
    }

    #[test]
    fn test_inject_skips_unmappable_shift_jis_in_loose_and_archive() {
        for archived in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let original =
                build_minimal_ystb(TRUE_KEY_B4626AD8, "Hello, traveler!", "Welcome home.");
            let path = if archived {
                let path = dir.path().join("game.ypf");
                let archive =
                    yuris_ypf::write_ypf(0x1E4, 0xFF, &[("yst00001.ybn".into(), original, true)])
                        .unwrap();
                fs::write(&path, archive).unwrap();
                path
            } else {
                let path = dir.path().join("yst00001.ybn");
                fs::write(&path, original).unwrap();
                path
            };
            let plugin = YurisPlugin::new();
            let mut entries = plugin.extract(dir.path()).unwrap();
            assert_eq!(entries.len(), 2);
            let skipped_id = entries
                .iter()
                .find(|e| e.source == "Hello, traveler!")
                .unwrap()
                .id
                .clone();
            for entry in &mut entries {
                entry.translation = Some(
                    if entry.id == skipped_id {
                        "Hola, señor."
                    } else {
                        "Hola, viajero."
                    }
                    .into(),
                );
            }
            let report = plugin.inject(dir.path(), &entries).unwrap();
            assert_eq!(report.strings_written, 1, "{report:?}");
            assert_eq!(report.strings_skipped, 1, "{report:?}");
            assert_eq!(report.files_modified, 1, "{report:?}");
            assert_eq!(report.files_written, vec![path]);
            assert_eq!(
                report
                    .warnings
                    .iter()
                    .filter(|w| w.contains(&skipped_id) && w.contains("Shift-JIS"))
                    .count(),
                1,
                "{report:?}"
            );
            let again = plugin.extract(dir.path()).unwrap();
            assert_eq!(again.len(), 2);
            for entry in again {
                assert_eq!(
                    entry.source,
                    if entry.id == skipped_id {
                        "Hello, traveler!"
                    } else {
                        "Hola, viajero."
                    }
                );
            }
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
        assert_eq!(report.files_modified, 1, "{report:?}");
        assert_eq!(report.strings_written, 1, "{report:?}");
        assert_eq!(report.strings_skipped, 1, "{report:?}");
        assert_eq!(report.warnings.len(), 1, "{report:?}");
        assert!(report.warnings[0].contains("Shift-JIS"), "{report:?}");

        let again = plugin.extract(&dir).unwrap();
        let sources: Vec<&str> = again.iter().map(|e| e.source.as_str()).collect();
        assert_eq!(sources, vec!["Hello, traveler!", "Bienvenido a casa."]);
    }

    #[test]
    fn test_inject_rewraps_flattened_multiline_message() {
        // YU-RIS messages carry the author's own hard wraps (confirmed on a
        // real title: mid-sentence breaks). A provider returns one flat line,
        // which used to be written back as-is and overflow the textbox.
        let dir = tempdir();
        create_fixture(&dir);
        let plugin = YurisPlugin::new();

        // Stage 1: give the fixture a hand-wrapped message. The source is
        // single-line here, so this is written verbatim.
        let wrapped = "Hola viajero del norte\nbienvenido a esta posada.";
        let mut entries = plugin.extract(&dir).unwrap();
        let target = entries
            .iter()
            .find(|e| e.source.contains("Hello, traveler"))
            .expect("fixture should have the traveler line")
            .id
            .clone();
        for e in &mut entries {
            if e.id == target {
                e.translation = Some(wrapped.to_string());
            }
        }
        plugin.inject(&dir, &entries).unwrap();

        // Stage 2: the source is now multi-line and the provider flattens it.
        let flat = "Saludos viajero venido desde las montanas del norte, bienvenido.";
        let mut entries = plugin.extract(&dir).unwrap();
        let src = entries
            .iter()
            .find(|e| e.id == target)
            .expect("target entry")
            .source
            .clone();
        assert!(
            src.contains('\n'),
            "setup: source should now be multi-line, got {src:?}"
        );
        let width = src
            .lines()
            .map(crate::rpgmaker_mv::visible_len)
            .max()
            .unwrap();
        for e in &mut entries {
            if e.id == target {
                e.translation = Some(flat.to_string());
            }
        }
        plugin.inject(&dir, &entries).unwrap();

        let written = plugin
            .extract(&dir)
            .unwrap()
            .into_iter()
            .find(|e| e.id == target)
            .expect("target entry")
            .source;
        assert!(written.contains('\n'), "should be re-wrapped: {written:?}");
        for line in written.lines() {
            assert!(
                crate::rpgmaker_mv::visible_len(line) <= width,
                "line over budget: {line:?}"
            );
        }
        assert_eq!(
            written.split_whitespace().collect::<Vec<_>>(),
            flat.split_whitespace().collect::<Vec<_>>(),
            "re-wrapping must not change wording"
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
        let ypf_bytes =
            crate::yuris_ypf::write_ypf(0x1E4, 0xFF, &[("yst00000.ybn".into(), ystb, true)])
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
                .any(|e| e.id.contains("ysbin/test.ypf/") && e.id.contains("yst00000.ybn#attr")),
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

        let backup = backup_path(&report);
        assert!(
            backup.is_file(),
            "expected .locust-old backup at {backup:?}"
        );
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
            entries
                .iter()
                .any(|e| e.source.contains("Only real script")),
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
    #[test]
    fn recovery_discovery_yuris_preserves_user_loose_scripts() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        create_fixture(root);
        let source = fs::read(root.join("ysbin/yst00001.ybn")).unwrap();
        let user = root.join(".locust-injections-user");
        fs::create_dir_all(&user).unwrap();
        fs::write(user.join("yst00002.ybn"), &source).unwrap();
        let plugin = YurisPlugin::new();
        let before = crate::recovery_discovery_tests::projection(plugin.extract(root).unwrap());
        assert_eq!(before.len(), 4);
        for name in [".locust", ".locust-injections"] {
            let internal = root.join(name).join("results");
            fs::create_dir_all(&internal).unwrap();
            fs::write(internal.join("yst90001.ybn"), &source).unwrap();
            fs::write(internal.join("sentinel.bin"), b"preserve recovery bytes").unwrap();
        }
        crate::recovery_discovery_tests::assert_unchanged(&plugin, root, before);
    }
}
