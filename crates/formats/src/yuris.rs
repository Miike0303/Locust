//! YU-RIS engine plugin — Experimental (synthetic fixtures).
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
//! Out of scope: YPF archive unpack, ysc.ybn command-name table (WORD/_/GOSUB
//! filtering uses structural heuristics instead — over-extraction OK).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use locust_core::error::{LocustError, Result};
use locust_core::extraction::{FormatPlugin, InjectionReport};
use locust_core::models::{OutputMode, StringEntry};

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
            if p.is_file() && Self::is_ybn(p) {
                out.push(p.to_path_buf());
            }
        }
        out
    }

    fn has_ypf(root: &Path) -> bool {
        if root.is_file() {
            return Self::is_ypf(root);
        }
        if !root.is_dir() {
            return false;
        }
        walkdir::WalkDir::new(root)
            .follow_links(false)
            .into_iter()
            .filter_map(|e| e.ok())
            .any(|e| e.path().is_file() && Self::is_ypf(e.path()))
    }

    fn has_ybn_or_ypf(root: &Path) -> bool {
        if root.is_file() {
            return Self::is_ybn(root) || Self::is_ypf(root);
        }
        if !root.is_dir() {
            return false;
        }
        walkdir::WalkDir::new(root)
            .follow_links(false)
            .into_iter()
            .filter_map(|e| e.ok())
            .any(|e| {
                let p = e.path();
                p.is_file() && (Self::is_ybn(p) || Self::is_ypf(p))
            })
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
    if t.chars()
        .all(|c| c.is_ascii_digit() || c == '.' || c == '-' || c == '+')
    {
        return false;
    }
    // Skip path-like tokens without spaces.
    if (t.contains('/') || t.contains('\\')) && !t.contains(' ') && !t.chars().any(is_cjk) {
        return false;
    }
    t.chars()
        .any(|c| c.is_alphabetic() || is_cjk(c) || c == '「' || c == '『' || c == '（')
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

// ─── FormatPlugin ──────────────────────────────────────────────────────────

impl FormatPlugin for YurisPlugin {
    fn id(&self) -> &str {
        "yuris"
    }

    fn name(&self) -> &str {
        "YU-RIS"
    }

    fn description(&self) -> &str {
        "YU-RIS loose YSTB .ybn scripts (XOR; Shift-JIS); YPF archives not yet"
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
        let ybn_files = Self::find_ybn_files(path);
        if ybn_files.is_empty() {
            if Self::has_ypf(path) {
                return Err(parse_err(
                    &path.display().to_string(),
                    "no loose .ybn scripts; ypf archives not yet supported",
                ));
            }
            return Err(parse_err(
                &path.display().to_string(),
                "no .ybn script files found",
            ));
        }

        let root = if path.is_dir() {
            path.to_path_buf()
        } else {
            path.parent().unwrap_or(path).to_path_buf()
        };

        let mut all = Vec::new();
        for fpath in &ybn_files {
            let bytes = std::fs::read(fpath)?;
            let rel = fpath
                .strip_prefix(&root)
                .unwrap_or(fpath.as_path())
                .to_string_lossy()
                .replace('\\', "/");
            let Some(ystb) = load_ystb(&bytes, &rel)? else {
                // Non-YSTB (YSTD stub etc.): skip, no error.
                continue;
            };
            for s in &ystb.strings {
                let id = format!("{rel}#arg{}", s.arg_index);
                let mut entry = StringEntry::new(id, &s.text, fpath.clone());
                entry.tags = vec!["dialogue".into()];
                entry.context = Some(format!("attr_type={}", s.attr_type));
                // Variable-length rebuild — no binary_slot.
                all.push(entry);
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

        let search_root = if path.is_dir() {
            path.to_path_buf()
        } else {
            path.parent().unwrap_or(path).to_path_buf()
        };

        for (file_path, file_entries) in &by_file {
            let actual = if file_path.exists() {
                file_path.clone()
            } else {
                search_root.join(file_path.file_name().unwrap_or_default())
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

            let mut translations: HashMap<usize, &str> = HashMap::new();
            for e in file_entries {
                let Some(t) = e.translation.as_deref() else {
                    strings_skipped += 1;
                    continue;
                };
                // id ends with #argN
                if let Some(pos) = e.id.rfind("#arg") {
                    if let Ok(idx) = e.id[pos + 4..].parse::<usize>() {
                        translations.insert(idx, t);
                    }
                }
            }
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
    fn test_ypf_only_extract_reports_loudly() {
        let dir = tempdir();
        fs::write(dir.join("ysbin.ypf"), b"YPF\0fake").unwrap();
        let plugin = YurisPlugin::new();
        let err = plugin.extract(&dir).unwrap_err().to_string();
        assert!(
            err.contains("ypf") && err.contains("not yet supported"),
            "expected ypf skip message, got: {err}"
        );
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
