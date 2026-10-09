//! KiriKiri / KAG script plugin — Experimental (synthetic fixtures).
//!
//! # Spec sources (transforms verified — do not invent)
//! - Scrambled text-stream header `FE FE <mode> FF FE` and per-mode body transforms
//!   as implemented by the widely used reverse of the engine loader:
//!   https://github.com/arcusmaximus/KirikiriTools/blob/master/KirikiriDescrambler/Descrambler.cs
//!   https://github.com/arcusmaximus/KirikiriTools/blob/master/KirikiriDescrambler/Scrambler.cs
//!   (mode 0: per-UTF-16LE-unit XOR; mode 1: odd/even bit swap — self-inverse;
//!   mode 2: zlib-compressed UTF-16LE after size fields — see below).
//! - Header signature documentation:
//!   https://github.com/arcusmaximus/KirikiriTools#kirikiridescrambler
//! - Engine family / KAG script conventions (`;` comments, `*` labels, `@` commands,
//!   `[tag]` markup): KiriKiri2 / KAG lineage — https://github.com/krkrz/krkr2
//! - Unencrypted XP3 containers: see [`crate::kirikiri_xp3`] (arcusmaximus Xp3Pack layout;
//!   inject writes the next contiguous `patchN.xp3`, preserving existing game archives).
//!
//! # Mode transforms (UTF-16LE code units, little-endian byte pairs)
//! - **Mode 0 decode:** for each unit, if high==0 && low<0x20 leave as-is; else
//!   `high ^= (low & 0xFE); low ^= 1` (Descrambler order).
//! - **Mode 0 encode:** reverse of decode: skip same control units; else
//!   `low ^= 1; high ^= (low & 0xFE)` (Scrambler order — uses post-xor low).
//! - **Mode 1:** `c = ((c & 0xAAAA) >> 1) | ((c & 0x5555) << 1)` (self-inverse).
//! - **Mode 2** (Descrambler `Decompress` / Scrambler `Compress`): after `FE FE 02 FF FE`,
//!   `u64` compressed_size LE, `u64` uncompressed_size LE, then a zlib stream of that
//!   compressed size. Inflated payload is raw UTF-16LE code units (no extra BOM inside
//!   the stream — the cipher header already carries `FF FE`). Write-back re-emits the
//!   same layout with correct sizes.
//!
//! Out of scope: CxDec / Hxv4 encrypted XP3, `.tjs`/compiled `.scn`,
//! rewriting existing game `.xp3` archives. Only a hash-verified, transaction-created
//! Locust overlay can be revised; other archives remain byte-identical.

#[cfg(test)]
use std::cell::Cell;
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use locust_core::error::{LocustError, Result};
use locust_core::extraction::{FormatPlugin, InjectionReport};
use locust_core::models::{OutputMode, StringEntry};
use tracing::warn;

use crate::kirikiri_xp3::{self, Xp3Archive, Xp3Entry};
use crate::util::parse_err;

// One increment per archive-entry name inspected by a lookup. `None` means this
// thread is not measuring (production tests stay quiet under --test-threads>1).
#[cfg(test)]
thread_local! {
    static XP3_NAME_INSPECTIONS: Cell<Option<usize>> = const { Cell::new(None) };
    static FIND_KS_FILES_CALLS: Cell<Option<usize>> = const { Cell::new(None) };
}

fn note_xp3_name_inspection() {
    #[cfg(test)]
    XP3_NAME_INSPECTIONS.with(|c| {
        if let Some(n) = c.get() {
            c.set(Some(n + 1));
        }
    });
}

/// First index of each normalized entry name. Later duplicates do not replace it.
fn index_xp3_names(entries: &[Xp3Entry]) -> HashMap<String, usize> {
    let mut by_name = HashMap::with_capacity(entries.len());
    for (i, entry) in entries.iter().enumerate() {
        note_xp3_name_inspection();
        by_name.entry(entry.name.clone()).or_insert(i);
    }
    by_name
}

struct CachedXp3 {
    archive: Xp3Archive,
    by_name: HashMap<String, usize>,
}

/// How the on-disk bytes encode the decoded Unicode text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum KsEncoding {
    Utf16Le,
    Utf8,
    ShiftJis,
}

/// Optional FE FE cipher wrapper around a UTF-16LE payload.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CipherMode {
    None,
    Mode0,
    Mode1,
    /// Zlib-compressed UTF-16LE after two u64 size fields.
    Mode2,
}

#[derive(Clone, Debug)]
struct DecodedKs {
    text: String,
    encoding: KsEncoding,
    cipher: CipherMode,
    /// Original decoded payload, including noncanonical Shift-JIS byte spellings.
    plain: Vec<u8>,
    bom: bool,
    /// Mode-2 bytes beyond the compressed stream must survive an edit.
    tail: Vec<u8>,
}

pub struct KirikiriPlugin;

impl KirikiriPlugin {
    pub fn new() -> Self {
        Self
    }

    fn is_ks(path: &Path) -> bool {
        path.extension()
            .and_then(|e| e.to_str())
            .map(|e| e.eq_ignore_ascii_case("ks"))
            .unwrap_or(false)
    }

    fn is_xp3(path: &Path) -> bool {
        path.extension()
            .and_then(|e| e.to_str())
            .map(|e| e.eq_ignore_ascii_case("xp3"))
            .unwrap_or(false)
    }

    fn find_ks_files(root: &Path) -> Vec<PathBuf> {
        #[cfg(test)]
        FIND_KS_FILES_CALLS.with(|c| {
            if let Some(n) = c.get() {
                c.set(Some(n + 1));
            }
        });

        let mut out = Vec::new();
        if root.is_file() {
            if Self::is_ks(root) {
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
            if p.is_file() && Self::is_ks(p) {
                out.push(p.to_path_buf());
            }
        }
        out
    }

    /// Top-level `.xp3` files in a game directory (or the path itself if it is one).
    fn find_top_level_xp3(root: &Path) -> Vec<PathBuf> {
        if root.is_file() {
            return if Self::is_xp3(root) {
                vec![root.to_path_buf()]
            } else {
                Vec::new()
            };
        }
        if !root.is_dir() {
            return Vec::new();
        }
        let mut out = Vec::new();
        if let Ok(entries) = std::fs::read_dir(root) {
            for e in entries.flatten() {
                let p = e.path();
                if p.is_file() && Self::is_xp3(&p) {
                    out.push(p);
                }
            }
        }
        out.sort();
        out
    }

    fn has_xp3(root: &Path) -> bool {
        !Self::find_top_level_xp3(root).is_empty()
    }

    fn root_dir(path: &Path) -> PathBuf {
        if path.is_dir() {
            path.to_path_buf()
        } else {
            path.parent().unwrap_or(path).to_path_buf()
        }
    }
}

impl Default for KirikiriPlugin {
    fn default() -> Self {
        Self::new()
    }
}

// ─── Cipher (FE FE modes 0/1) ──────────────────────────────────────────────

fn mode0_decode_units(data: &mut [u8]) {
    let mut i = 0;
    while i + 1 < data.len() {
        if data[i + 1] == 0 && data[i] < 0x20 {
            i += 2;
            continue;
        }
        data[i + 1] ^= data[i] & 0xFE;
        data[i] ^= 1;
        i += 2;
    }
}

fn mode0_encode_units(data: &mut [u8]) {
    let mut i = 0;
    while i + 1 < data.len() {
        if data[i + 1] == 0 && data[i] < 0x20 {
            i += 2;
            continue;
        }
        data[i] ^= 1;
        data[i + 1] ^= data[i] & 0xFE;
        i += 2;
    }
}

fn mode1_swap_units(data: &mut [u8]) {
    let mut i = 0;
    while i + 1 < data.len() {
        let c = u16::from_le_bytes([data[i], data[i + 1]]);
        let swapped = ((c & 0xAAAA) >> 1) | ((c & 0x5555) << 1);
        let bytes = swapped.to_le_bytes();
        data[i] = bytes[0];
        data[i + 1] = bytes[1];
        i += 2;
    }
}

fn utf16le_bytes_from_str(s: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(s.encode_utf16().count() * 2);
    for u in s.encode_utf16() {
        out.extend_from_slice(&u.to_le_bytes());
    }
    out
}

fn str_from_utf16le_bytes(data: &[u8], label: &str) -> Result<String> {
    if !data.len().is_multiple_of(2) {
        return Err(parse_err(label, "odd UTF-16LE payload length"));
    }
    let units: Vec<u16> = data
        .chunks_exact(2)
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .collect();
    String::from_utf16(&units).map_err(|_| parse_err(label, "invalid UTF-16LE payload"))
}

// Decode strictly: a lossy decoder cannot support byte-preserving editing.
fn decode_ks_bytes(bytes: &[u8], label: &str) -> Result<DecodedKs> {
    let mut plain;
    let encoding;
    let cipher;
    let mut bom = false;
    let mut tail = Vec::new();
    if bytes.starts_with(&[0xFE, 0xFE]) {
        if bytes.len() < 5 || bytes[3..5] != [0xFF, 0xFE] {
            return Err(parse_err(label, "invalid FE FE cipher header"));
        }
        encoding = KsEncoding::Utf16Le;
        match bytes[2] {
            0 | 1 => {
                plain = bytes[5..].to_vec();
                if !plain.len().is_multiple_of(2) {
                    return Err(parse_err(label, "odd cipher payload length"));
                }
                if bytes[2] == 0 {
                    cipher = CipherMode::Mode0;
                    mode0_decode_units(&mut plain);
                } else {
                    cipher = CipherMode::Mode1;
                    mode1_swap_units(&mut plain);
                }
            }
            2 => {
                cipher = CipherMode::Mode2;
                if bytes.len() < 21 {
                    return Err(parse_err(label, "mode-2 missing size fields"));
                }
                let size = usize::try_from(u64::from_le_bytes(bytes[5..13].try_into().unwrap()))
                    .map_err(|_| parse_err(label, "mode-2 size overflow"))?;
                let expected = u64::from_le_bytes(bytes[13..21].try_into().unwrap());
                let end = 21usize
                    .checked_add(size)
                    .filter(|end| *end <= bytes.len())
                    .ok_or_else(|| parse_err(label, "mode-2 truncated zlib stream"))?;
                plain = miniz_oxide::inflate::decompress_to_vec_zlib(&bytes[21..end])
                    .map_err(|e| parse_err(label, format!("mode-2 zlib inflate: {e:?}")))?;
                if expected != 0 && plain.len() as u64 != expected {
                    return Err(parse_err(label, "mode-2 inflated size mismatch"));
                }
                tail.extend_from_slice(&bytes[end..]);
            }
            _ => return Err(parse_err(label, "unsupported FE FE cipher mode")),
        }
    } else if bytes.starts_with(&[0xFF, 0xFE]) {
        encoding = KsEncoding::Utf16Le;
        cipher = CipherMode::None;
        bom = true;
        plain = bytes[2..].to_vec();
    } else if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
        encoding = KsEncoding::Utf8;
        cipher = CipherMode::None;
        bom = true;
        plain = bytes[3..].to_vec();
    } else {
        encoding = if std::str::from_utf8(bytes).is_ok() {
            KsEncoding::Utf8
        } else {
            KsEncoding::ShiftJis
        };
        cipher = CipherMode::None;
        plain = bytes.to_vec();
    }
    let text = match encoding {
        KsEncoding::Utf16Le => str_from_utf16le_bytes(&plain, label)?,
        KsEncoding::Utf8 => String::from_utf8(plain.clone())
            .map_err(|_| parse_err(label, "invalid UTF-8 payload"))?,
        KsEncoding::ShiftJis => {
            let (text, errors) = encoding_rs::SHIFT_JIS.decode_without_bom_handling(&plain);
            if errors {
                return Err(parse_err(label, "invalid Shift-JIS payload"));
            }
            text.into_owned()
        }
    };
    Ok(DecodedKs {
        text,
        encoding,
        cipher,
        plain,
        bom,
        tail,
    })
}

struct KsEdit {
    start: usize,
    end: usize,
    replacement: String,
}

/// Map Unicode boundaries to the ORIGINAL payload. Re-encoding unchanged SJIS
/// would corrupt duplicate/noncanonical CP932 spellings even after strict decode.
fn payload_boundaries(decoded: &DecodedKs) -> HashMap<usize, usize> {
    let mut offsets = HashMap::new();
    let mut raw = 0;
    for (offset, ch) in decoded.text.char_indices() {
        offsets.insert(offset, raw);
        raw += match decoded.encoding {
            KsEncoding::Utf8 => ch.len_utf8(),
            KsEncoding::Utf16Le => ch.len_utf16() * 2,
            KsEncoding::ShiftJis => {
                if matches!(decoded.plain[raw], 0x81..=0x9f | 0xe0..=0xfc) {
                    2
                } else {
                    1
                }
            }
        };
    }
    offsets.insert(decoded.text.len(), raw);
    offsets
}

fn encode_ks_edits(decoded: &DecodedKs, edits: &[KsEdit]) -> Result<Vec<u8>> {
    let boundaries = payload_boundaries(decoded);
    let mut plain = Vec::new();
    let mut cursor = 0;
    for edit in edits {
        let start = boundaries[&edit.start];
        let end = boundaries[&edit.end];
        plain.extend_from_slice(&decoded.plain[cursor..start]);
        match decoded.encoding {
            KsEncoding::Utf8 => plain.extend_from_slice(edit.replacement.as_bytes()),
            KsEncoding::Utf16Le => {
                plain.extend_from_slice(&utf16le_bytes_from_str(&edit.replacement))
            }
            KsEncoding::ShiftJis => {
                let (bytes, _, errors) = encoding_rs::SHIFT_JIS.encode(&edit.replacement);
                let (roundtrip, _) = encoding_rs::SHIFT_JIS.decode_without_bom_handling(&bytes);
                if errors || roundtrip != edit.replacement {
                    return Err(parse_err(
                        "ks",
                        "translation cannot round-trip as Shift-JIS",
                    ));
                }
                plain.extend_from_slice(&bytes);
            }
        }
        cursor = end;
    }
    plain.extend_from_slice(&decoded.plain[cursor..]);
    let mut out = Vec::new();
    match decoded.cipher {
        CipherMode::None => {
            if decoded.bom {
                out.extend_from_slice(match decoded.encoding {
                    KsEncoding::Utf16Le => &[0xFF, 0xFE],
                    _ => &[0xEF, 0xBB, 0xBF],
                });
            }
            out.extend_from_slice(&plain);
        }
        CipherMode::Mode0 | CipherMode::Mode1 => {
            let mode = if decoded.cipher == CipherMode::Mode0 {
                mode0_encode_units(&mut plain);
                0
            } else {
                mode1_swap_units(&mut plain);
                1
            };
            out.extend_from_slice(&[0xFE, 0xFE, mode, 0xFF, 0xFE]);
            out.extend_from_slice(&plain);
        }
        CipherMode::Mode2 => {
            // Compression changes the stream globally; the inflated untouched
            // bytes, wrapper kind and trailing bytes remain exact.
            let compressed = miniz_oxide::deflate::compress_to_vec_zlib(&plain, 6);
            out.extend_from_slice(&[0xFE, 0xFE, 2, 0xFF, 0xFE]);
            out.extend_from_slice(&(compressed.len() as u64).to_le_bytes());
            out.extend_from_slice(&(plain.len() as u64).to_le_bytes());
            out.extend_from_slice(&compressed);
            out.extend_from_slice(&decoded.tail);
        }
    }
    Ok(out)
}

// ─── KAG line classification ───────────────────────────────────────────────

/// True for `;comment`, `*label`, `@command`, empty, pure `[tag]`-only lines,
/// pure ellipsis/dot filler, TJS `//` comments, brace-only lines, or obvious
/// TJS/KAG script statements (not player dialogue).
fn is_non_text_line(line: &str) -> bool {
    let t = line.trim();
    if t.is_empty() {
        return true;
    }
    if t.starts_with(';') || t.starts_with('*') || t.starts_with('@') || t.starts_with("//") {
        return true;
    }
    // KAG line-continuation: pure command lines often end with `\`.
    // Taimanin / many scripts: `[cm][SYSMENU]\`, `[endif]\`, `[NAME_W n="…"]\`.
    let core = t.trim_end_matches(|c: char| c == '\\' || c.is_whitespace());
    if core.is_empty() {
        return true;
    }
    // Strip trailing `// …` so `];  // init` classifies as punctuation noise.
    let core = match core.find("//") {
        Some(i) => core[..i].trim_end(),
        None => core,
    };
    if core.is_empty() {
        return true;
    }
    if is_pure_ellipsis_line(core) {
        return true;
    }
    if is_pure_tag_line(core) {
        return true;
    }
    if is_tjs_or_brace_noise(core) {
        return true;
    }
    false
}

/// Brace-only lines and obvious TJS/KAG script statements (Taimanin macros).
/// Keeps dialogue that merely *contains* braces or code-like fragments.
fn is_tjs_or_brace_noise(t: &str) -> bool {
    let compact: String = t.chars().filter(|c| !c.is_whitespace()).collect();
    if matches!(
        compact.as_str(),
        "{" | "}" | "};" | "];" | "else" | "else{" | "}else{" | "}else"
    ) {
        return true;
    }
    let low = t.trim_start();
    if low.starts_with("for(")
        || low.starts_with("for (")
        || low.starts_with("while(")
        || low.starts_with("while (")
        || low.starts_with("function(")
        || low.starts_with("function ")
        || low.starts_with("var ")
        || low.starts_with("const ")
        || low.starts_with("let ")
    {
        return true;
    }
    // Bare control keywords used as whole lines in macro scripts.
    if matches!(low, "else" | "return" | "break" | "continue") {
        return true;
    }
    // Engine storage assignments: `tf.con_vol=[];`, `sf.masked[i]=0;`
    // (no dialogue quotes / fullwidth brackets).
    let has_dialogue_mark = t
        .chars()
        .any(|c| matches!(c, '「' | '」' | '『' | '』' | '（' | '）') || c == '"' || c == '\'');
    if !has_dialogue_mark
        && (low.starts_with("tf.")
            || low.starts_with("sf.")
            || low.starts_with("f.")
            || low.starts_with("kag."))
        && (t.contains('=') || t.ends_with(';'))
    {
        return true;
    }
    false
}

/// Pure pause/filler lines: `......`, fullwidth-space + dots, `…` runs.
/// Ochiru Hitozuma (and many KAG scripts) pad timing with these — not dialogue.
fn is_pure_ellipsis_line(t: &str) -> bool {
    let mut n = 0usize;
    for c in t.chars() {
        if c.is_whitespace() || c == '\u{3000}' {
            continue;
        }
        if matches!(c, '.' | '…' | '・' | '．') {
            n += 1;
            continue;
        }
        return false;
    }
    n >= 2
}

/// Entire line is one or more `[...]` tags with no free text between them.
///
/// Uses bracket depth so attribute expressions with nested `[]` still count as
/// one tag: `[eval exp="sf.x[tf.i]=1"]` (Taimanin / KAG).
fn is_pure_tag_line(t: &str) -> bool {
    let tags = kag_tags(t);
    !tags.is_empty()
        && text_between_tags(t, &tags)
            .iter()
            .all(|(_, text)| text.trim().is_empty())
}

fn is_player_text_line(line: &str) -> bool {
    !is_non_text_line(line)
}

struct KagTag<'a> {
    start: usize,
    end: usize,
    name: &'a str,
    attrs_start: usize,
}

/// `[[` is a displayed bracket. Quotes shield `]`; KAG escapes attribute
/// characters with a backtick, NOT a TJS/backslash string escape.
fn kag_tags(line: &str) -> Vec<KagTag<'_>> {
    let bytes = line.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i..].starts_with(b"[[") {
            i += 2;
            continue;
        }
        if bytes[i] != b'[' {
            i += 1;
            continue;
        }
        let start = i;
        i += 1;
        let name_start = i;
        while i < bytes.len() && !bytes[i].is_ascii_whitespace() && bytes[i] != b']' {
            i += 1;
        }
        let name = &line[name_start..i];
        let attrs_start = i;
        let mut quote = None;
        while i < bytes.len() {
            let c = bytes[i];
            if c == b'`' {
                i = (i + 2).min(bytes.len());
                continue;
            }
            if let Some(q) = quote {
                if c == q {
                    quote = None;
                }
            } else if c == b'\'' || c == b'"' {
                quote = Some(c);
            } else if c == b']' {
                break;
            }
            i += 1;
        }
        if i == bytes.len() {
            break;
        }
        i += 1;
        out.push(KagTag {
            start,
            end: i,
            name,
            attrs_start,
        });
    }
    out
}

fn text_between_tags<'a>(line: &'a str, tags: &[KagTag<'_>]) -> Vec<(usize, &'a str)> {
    let mut out = Vec::new();
    let mut start = 0;
    for tag in tags {
        out.push((start, &line[start..tag.start]));
        start = tag.end;
    }
    out.push((start, &line[start..]));
    out
}

fn has_unescaped_bracket(text: &str) -> bool {
    let mut chars = text.chars();
    while let Some(ch) = chars.next() {
        if ch == '[' && chars.next() != Some('[') {
            return true;
        }
    }
    false
}

/// Content ranges exclude CR, LF, and CRLF independently (including a final
/// separator). Logical row numbers are stable across a text-span edit.
fn kag_lines(text: &str) -> Vec<(usize, &str)> {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut start = 0;
    let mut i = 0;
    while i < bytes.len() {
        if matches!(bytes[i], b'\r' | b'\n') {
            out.push((start, &text[start..i]));
            i += if bytes[i..].starts_with(b"\r\n") {
                2
            } else {
                1
            };
            start = i;
        } else {
            i += 1;
        }
    }
    if start < text.len() {
        out.push((start, &text[start..]));
    }
    out
}

enum KagSlotKind {
    Dialogue,
    Attribute(Option<u8>),
}

struct KagSlot {
    locator: String,
    start: usize,
    end: usize,
    source: String,
    kind: KagSlotKind,
}

fn unescape_attribute(raw: &str) -> String {
    let mut chars = raw.chars();
    let mut out = String::new();
    while let Some(c) = chars.next() {
        if c == '`' {
            if let Some(next) = chars.next() {
                out.push(next);
            }
        } else {
            out.push(c);
        }
    }
    out
}

fn display_attribute(tag: &str) -> Option<&'static str> {
    if tag.eq_ignore_ascii_case("name") {
        Some("text")
    } else if tag.eq_ignore_ascii_case("name_w") {
        Some("n")
    } else {
        None
    }
}

fn attribute_slots(
    line: &str,
    tag: &KagTag<'_>,
    ordinal: usize,
    no: usize,
    base: usize,
) -> Vec<KagSlot> {
    let Some(wanted) = display_attribute(tag.name) else {
        return Vec::new();
    };
    let bytes = line.as_bytes();
    let mut i = tag.attrs_start;
    let mut found = Vec::new();
    while i < tag.end - 1 {
        while i < tag.end - 1 && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        let key_start = i;
        while i < tag.end - 1 && !bytes[i].is_ascii_whitespace() && bytes[i] != b'=' {
            i += 1;
        }
        let key = &line[key_start..i];
        while i < tag.end - 1 && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if i == tag.end - 1 || bytes[i] != b'=' {
            continue;
        }
        i += 1;
        while i < tag.end - 1 && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        let expression = i < tag.end - 1 && matches!(bytes[i], b'&' | b'%');
        if expression {
            i += 1;
        }
        let quote = if i < tag.end - 1 && matches!(bytes[i], b'\'' | b'"') {
            let q = bytes[i];
            i += 1;
            Some(q)
        } else {
            None
        };
        let start = i;
        while i < tag.end - 1 {
            if bytes[i] == b'`' {
                i = (i + 2).min(tag.end - 1);
                continue;
            }
            if quote.map_or_else(|| bytes[i].is_ascii_whitespace(), |q| bytes[i] == q) {
                break;
            }
            i += 1;
        }
        let end = i;
        if quote.is_some() {
            i += 1;
        }
        if key.eq_ignore_ascii_case(wanted) {
            // Duplicate attributes are ambiguous, even if one is an expression.
            let value = unescape_attribute(&line[start..end]);
            found.push((start, end, quote, expression, value));
        }
    }
    if found.len() != 1 {
        return Vec::new();
    }
    let (start, end, quote, expression, source) = found.pop().unwrap();
    if expression || source.starts_with(['&', '%']) || source.trim().is_empty() {
        return Vec::new();
    }
    vec![KagSlot {
        locator: format!("kag:{no}:attr:{ordinal}:{wanted}"),
        start: base + start,
        end: base + end,
        source,
        kind: KagSlotKind::Attribute(quote),
    }]
}

fn kag_slots(text: &str) -> Vec<KagSlot> {
    let mut out = Vec::new();
    let mut script = false;
    for (index, (base, line)) in kag_lines(text).into_iter().enumerate() {
        let no = index + 1;
        let trimmed = line.trim();
        // Only standalone commands end TJS. A quoted "[endscript]" inside
        // script code must not admit the remainder of the script as dialogue.
        let command = trimmed.trim_end_matches('\\').trim_end();
        let ends = command.eq_ignore_ascii_case("[endscript]")
            || command.eq_ignore_ascii_case("@endscript");
        if script {
            if ends {
                script = false;
            }
            continue;
        }
        if trimmed.starts_with([';', '*']) {
            continue;
        }
        let starts =
            command.eq_ignore_ascii_case("[iscript]") || command.eq_ignore_ascii_case("@iscript");
        if starts {
            script = true;
            continue;
        }
        if ends {
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix('@') {
            // @ commands use the same attribute grammar, with no closing ].
            let synthetic = format!("[{rest}]");
            let tags = kag_tags(&synthetic);
            if let Some(tag) = tags.first() {
                let indent = line.len() - line.trim_start().len();
                out.extend(attribute_slots(&synthetic, tag, 0, no, base + indent));
            }
            continue;
        }
        let tags = kag_tags(line);
        for (ordinal, tag) in tags.iter().enumerate() {
            out.extend(attribute_slots(line, tag, ordinal, no, base));
        }
        if is_player_text_line(line) {
            out.push(KagSlot {
                locator: format!("kag:{no}"),
                start: base,
                end: base + line.len(),
                source: line.into(),
                kind: KagSlotKind::Dialogue,
            });
        }
    }
    out
}

fn extract_lines_from_text(text: &str, rel: &str, file_path: PathBuf) -> Vec<StringEntry> {
    kag_slots(text)
        .into_iter()
        .map(|slot| {
            let mut entry = StringEntry::new(
                format!("{rel}#{}", slot.locator),
                slot.source,
                file_path.clone(),
            );
            entry.tags = vec![match slot.kind {
                KagSlotKind::Dialogue => "dialogue",
                KagSlotKind::Attribute(_) => "speaker",
            }
            .into()];
            entry
        })
        .collect()
}

fn add_text_edit(edits: &mut Vec<KsEdit>, start: usize, source: &str, replacement: &str) {
    if source == replacement {
        return;
    }
    let prefix: usize = source
        .chars()
        .zip(replacement.chars())
        .take_while(|(a, b)| a == b)
        .map(|(c, _)| c.len_utf8())
        .sum();
    let suffix: usize = source[prefix..]
        .chars()
        .rev()
        .zip(replacement[prefix..].chars().rev())
        .take_while(|(a, b)| a == b)
        .map(|(c, _)| c.len_utf8())
        .sum();
    edits.push(KsEdit {
        start: start + prefix,
        end: start + source.len() - suffix,
        replacement: replacement[prefix..replacement.len() - suffix].into(),
    });
}

fn escape_attribute(value: &str, quote: Option<u8>) -> String {
    let mut out = String::new();
    for c in value.chars() {
        if c == '`'
            || quote == Some(c as u8) && c.is_ascii()
            || quote.is_none() && (c.is_whitespace() || matches!(c, ']' | '"' | '\''))
        {
            out.push('`');
        }
        out.push(c);
    }
    out
}

/// Legacy #N addresses LF-delimited rows, never reinterpret it as a CR logical
/// row. Both legacy and versioned locators must match the current source exactly.
#[cfg(test)]
fn apply_translations(
    bytes: &[u8],
    label: &str,
    file_entries: &[&StringEntry],
) -> Result<Option<(Vec<u8>, usize)>> {
    apply_translations_classified(bytes, label, file_entries).map_err(|(_, error)| error)
}

type KsInjectionResult = std::result::Result<Option<(Vec<u8>, usize)>, (&'static str, LocustError)>;

fn apply_translations_classified(
    bytes: &[u8],
    label: &str,
    file_entries: &[&StringEntry],
) -> KsInjectionResult {
    let decoded = decode_ks_bytes(bytes, label).map_err(|error| {
        let reason = match &error {
            LocustError::ParseError { message, .. }
                if message == "unsupported FE FE cipher mode" =>
            {
                "unsupported"
            }
            _ => "decode_error",
        };
        (reason, error)
    })?;
    let reject = |reason, message| (reason, parse_err(label, message));
    let slots = kag_slots(&decoded.text);
    let mut edits = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut written = 0;
    for entry in file_entries {
        let Some(translation) = entry.translation.as_deref() else {
            continue;
        };
        let (rel, locator) = entry
            .id
            .rsplit_once('#')
            .ok_or_else(|| reject("invalid_locator", "missing KAG locator"))?;
        let normalized_label = label.replace('\\', "/");
        let normalized_rel = rel.replace('\\', "/");
        if normalized_label != normalized_rel
            && !normalized_label.ends_with(&format!("/{normalized_rel}"))
        {
            return Err(reject(
                "invalid_locator",
                "locator addresses a different script",
            ));
        }
        let slot = if locator.starts_with("kag:") {
            slots.iter().find(|slot| slot.locator == locator)
        } else {
            let no: usize = locator
                .parse()
                .map_err(|_| reject("invalid_locator", "invalid legacy KAG locator"))?;
            let mut offset = 0;
            let mut found = None;
            for (i, raw) in decoded.text.split('\n').enumerate() {
                let line = raw.strip_suffix('\r').unwrap_or(raw);
                if i + 1 == no && !line.contains('\r') {
                    found = slots.iter().find(|slot| {
                        slot.start == offset
                            && slot.end == offset + line.len()
                            && matches!(slot.kind, KagSlotKind::Dialogue)
                    });
                    break;
                }
                offset += raw.len() + 1;
            }
            found
        }
        .ok_or_else(|| reject("invalid_locator", "stale or unsafe KAG locator"))?;
        if slot.source != entry.source {
            return Err(reject(
                "source_changed",
                "stale source or ambiguous KAG row",
            ));
        }
        if !seen.insert(&slot.locator) {
            return Err(reject(
                "ambiguous_target",
                "stale source or ambiguous KAG row",
            ));
        }
        if translation == slot.source {
            continue;
        }
        if translation.contains(['\r', '\n', '\0']) {
            return Err(reject(
                "unsafe_controls",
                "translation changes line delimiters",
            ));
        }
        match slot.kind {
            KagSlotKind::Attribute(quote) => {
                if translation.trim().is_empty() || translation.starts_with(['&', '%']) {
                    return Err(reject(
                        "invalid_translation",
                        "display attribute must remain a literal",
                    ));
                }
                let raw = &decoded.text[slot.start..slot.end];
                // Keep original spelling/escaping in the shared prefix/suffix.
                // A raw spelling can differ from canonical escaping. In that
                // case preserve unchanged displayed characters via raw offsets.
                let prefix = slot
                    .source
                    .chars()
                    .zip(translation.chars())
                    .take_while(|(a, b)| a == b)
                    .count();
                let source_chars: Vec<char> = slot.source.chars().collect();
                let translated_chars: Vec<char> = translation.chars().collect();
                let suffix = source_chars[prefix..]
                    .iter()
                    .rev()
                    .zip(translated_chars[prefix..].iter().rev())
                    .take_while(|(a, b)| a == b)
                    .count();
                let mut boundaries = vec![0];
                let mut it = raw.char_indices();
                while let Some((_, ch)) = it.next() {
                    let end = if ch == '`' {
                        let (i, c) = it.next().unwrap();
                        i + c.len_utf8()
                    } else {
                        boundaries.last().copied().unwrap() + ch.len_utf8()
                    };
                    boundaries.push(end);
                }
                let chars: Vec<char> = translation.chars().collect();
                let middle: String = chars[prefix..chars.len() - suffix].iter().collect();
                let replacement = format!(
                    "{}{}{}",
                    &raw[..boundaries[prefix]],
                    escape_attribute(&middle, quote),
                    &raw[boundaries[boundaries.len() - 1 - suffix]..]
                );
                add_text_edit(&mut edits, slot.start, raw, &replacement);
            }
            KagSlotKind::Dialogue => {
                let old_tags = kag_tags(&slot.source);
                let new_tags = kag_tags(translation);
                if old_tags.len() != new_tags.len()
                    || old_tags
                        .iter()
                        .zip(&new_tags)
                        .any(|(a, b)| slot.source[a.start..a.end] != translation[b.start..b.end])
                {
                    return Err(reject(
                        "invalid_placeholders",
                        "translation changes protected KAG tags",
                    ));
                }
                let old_text = text_between_tags(&slot.source, &old_tags);
                let new_text = text_between_tags(translation, &new_tags);
                let old_tail = slot.source.trim_end();
                let new_tail = translation.trim_end();
                if old_tail.ends_with('\\') != new_tail.ends_with('\\')
                    || !is_player_text_line(translation)
                {
                    return Err(reject(
                        "unsafe_controls",
                        "translation changes continuation or command controls",
                    ));
                }
                for ((offset, old), (_, new)) in old_text.iter().zip(new_text) {
                    // A continuation and its trailing whitespace are protected.
                    if old.trim_end().ends_with('\\') {
                        let suffix = &old[old.trim_end().len() - 1..];
                        if !new.ends_with(suffix) {
                            return Err(reject(
                                "unsafe_controls",
                                "translation changes continuation",
                            ));
                        }
                    }
                    if has_unescaped_bracket(new) {
                        return Err(reject(
                            "unsupported_markup",
                            "translation introduces an unterminated KAG tag",
                        ));
                    }
                    add_text_edit(&mut edits, slot.start + offset, old, new);
                }
            }
        }
        written += 1;
    }
    if edits.is_empty() {
        return Ok(None);
    }
    edits.sort_by_key(|edit| (edit.start, edit.end));
    if edits
        .windows(2)
        .any(|pair| pair[0].end > pair[1].start || pair[0].start == pair[1].start)
    {
        return Err(reject(
            "overlapping_locators",
            "overlapping KAG translations",
        ));
    }
    let encoded = encode_ks_edits(&decoded, &edits).map_err(|error| ("not_encodable", error))?;
    Ok(Some((encoded, written)))
}

/// Split `data.xp3/scenario/foo.ks` into (`data.xp3`, `scenario/foo.ks`).
fn split_xp3_virtual_path(path: &Path) -> Option<(String, String)> {
    let s = path.to_string_lossy().replace('\\', "/");
    let lower = s.to_ascii_lowercase();
    let idx = lower.find(".xp3/")?;
    let archive = s[..=idx + 3].to_string(); // includes ".xp3"
    let inner = s[idx + 5..].to_string();
    if inner.is_empty() {
        return None;
    }
    Some((archive, inner))
}

/// Normalize a script identity for multi-source dedupe (Taimanin / Ochiru-style trees).
///
/// - `data.xp3/scenario/newgame03.ks` → `newgame03.ks`
/// - `patch2.xp3/newgame03.ks` → `newgame03.ks`
/// - `unencrypted/newgame03.ks` → `newgame03.ks`
/// - `data.xp3/scenario/select/movie.ks` → `select/movie.ks`
fn normalize_script_key(rel_or_virtual: &str) -> String {
    let mut s = rel_or_virtual.replace('\\', "/").to_ascii_lowercase();
    if let Some(idx) = s.find(".xp3/") {
        s = s[idx + 5..].to_string();
    }
    for prefix in [
        "unencrypted/",
        "vntranslationtools/",
        "output/",
        "scenario/",
        "data/scenario/",
        "data/",
    ] {
        if let Some(rest) = s.strip_prefix(prefix) {
            s = rest.to_string();
        }
    }
    s
}

/// True when decoded `.ks` text is plausibly KAG script rather than ciphertext.
///
/// cxdec-protected archives hold payloads that still survive UTF-16LE decoding
/// but come out as control-character soup. They must not count as a source, or
/// they outrank the plaintext copy of the same script and it is dropped.
fn looks_like_readable_ks(text: &str) -> bool {
    let mut ctrl = 0usize;
    let mut total = 0usize;
    for c in text.chars() {
        total += 1;
        if c == '\u{FFFD}' || (c.is_control() && !matches!(c, '\n' | '\r' | '\t')) {
            ctrl += 1;
        }
    }
    // Real scripts carry essentially no C0 bytes beyond newlines and tabs.
    total == 0 || ctrl * 20 <= total
}

/// Higher rank wins when the same script key appears in multiple places.
/// Prefer higher-numbered `patchN.xp3`, then other XP3, then loose (not tool dumps).
fn script_source_rank(rel_or_virtual: &str) -> i32 {
    let s = rel_or_virtual.replace('\\', "/").to_ascii_lowercase();
    if let Some(idx) = s.find(".xp3/") {
        // `patch2.xp3/foo` → arch_file `patch2.xp3`
        let arch_file = &s[..=idx + 3];
        let stem = arch_file.trim_end_matches(".xp3");
        if stem == "patch" || stem.starts_with("patch") {
            let n = stem.trim_start_matches("patch");
            let num: i32 = if n.is_empty() {
                1
            } else {
                n.parse().unwrap_or(1)
            };
            return 1000 + num;
        }
        return 100;
    }
    // Loose tool dumps are lowest priority (often full copies of scenario/).
    if s.contains("/unencrypted/")
        || s.starts_with("unencrypted/")
        || s.contains("vntranslationtools")
        || s.contains("/output/")
        || s.starts_with("output/")
    {
        return 1;
    }
    10
}

/// Winning relative/virtual paths after per-key rank selection.
fn select_best_script_rels(candidates: &[String]) -> std::collections::HashSet<String> {
    let mut best: BTreeMap<String, (i32, String)> = BTreeMap::new();
    for rel in candidates {
        let key = normalize_script_key(rel);
        if key.is_empty() {
            continue;
        }
        let rank = script_source_rank(rel);
        match best.get(&key) {
            None => {
                best.insert(key, (rank, rel.clone()));
            }
            Some((prev_rank, _)) if rank > *prev_rank => {
                best.insert(key, (rank, rel.clone()));
            }
            _ => {}
        }
    }
    best.into_values().map(|(_, rel)| rel).collect()
}

// KAG loads patch.xp3, then patch2, patch3, ... until the first missing number.
// Later auto paths win: https://www.ultrasync.net/dee/kr2helps/kag3doc/contents/Distribute.html
fn patch_number(name: &str) -> Option<u32> {
    let lower = name.to_ascii_lowercase();
    let stem = lower.strip_suffix(".xp3")?.strip_prefix("patch")?;
    if stem.is_empty() {
        return Some(1);
    }
    let n: u32 = stem.parse().ok()?;
    (n >= 2 && stem == n.to_string()).then_some(n)
}

struct PatchTarget {
    path: PathBuf,
    /// Only present when a completed transaction proves creation and every revision.
    owned_hash: Option<(String, u64)>,
}

type PatchRefusal = (&'static str, String);

fn patch_metadata(path: &Path) -> Result<serde_json::Value> {
    locust_core::patch::zipsec::ensure_no_links(
        path.parent().unwrap(),
        Path::new(path.file_name().unwrap()),
    )?;
    let meta = std::fs::symlink_metadata(path)?;
    if !meta.is_file() || meta.len() > 64 * 1024 * 1024 {
        return Err(parse_err("XP3 provenance", "invalid transaction metadata"));
    }
    Ok(serde_json::from_slice(&std::fs::read(path)?)?)
}

fn recorded_hash(value: &serde_json::Value) -> Option<(String, u64)> {
    let hash = value["sha256"].as_str()?;
    (hash.len() == 64 && hash.bytes().all(|c| c.is_ascii_hexdigit()))
        .then(|| (hash.to_owned(), value["size"].as_u64()))
        .and_then(|(hash, size)| size.map(|size| (hash, size)))
}

/// Core deliberately excludes journals from its work copy. Read the real journal
/// only after binding this exact work directory to the active Preparing operation.
/// This read-only adapter uses schema 1; unknown schemas fail closed.
fn patch_transaction_root(root: &Path) -> Result<Option<PathBuf>> {
    use locust_core::injection_transaction::{validate_store, STORE_DIR};
    let root = root.canonicalize()?;
    if validate_store(&root)? {
        return Ok(Some(root));
    }
    let Some(operation) = root
        .parent()
        .filter(|_| root.file_name().is_some_and(|s| s == "work"))
    else {
        return Ok(None);
    };
    let Some(operations) = operation
        .parent()
        .filter(|p| p.file_name().is_some_and(|s| s == "operations"))
    else {
        return Ok(None);
    };
    let Some(store) = operations
        .parent()
        .filter(|p| p.file_name().is_some_and(|s| s == STORE_DIR))
    else {
        return Ok(None);
    };
    let game = store
        .parent()
        .ok_or_else(|| parse_err("XP3 provenance", "missing game root"))?;
    if !validate_store(game)? {
        return Err(parse_err("XP3 provenance", "missing transaction store"));
    }
    locust_core::patch::zipsec::ensure_no_links(game, root.strip_prefix(game).unwrap())?;
    let active = patch_metadata(&store.join("active.json"))?;
    if active["schema_version"] != 1
        || active["format"] != "kirikiri"
        || active["game_root"].as_str().map(Path::new) != Some(game)
        || active["transaction_id"].as_str() != operation.file_name().and_then(|s| s.to_str())
        || patch_metadata(&operation.join("phase.json"))? != "preparing"
    {
        return Err(parse_err(
            "XP3 provenance",
            "work directory is not the active KiriKiri transaction",
        ));
    }
    Ok(Some(game.to_owned()))
}

/// Reachability from original:null proves creation, including later revisions.
/// A matching result hash alone does not confer ownership of a game's own patch.
fn owned_patches(root: &Path) -> Result<BTreeMap<String, Vec<(String, u64)>>> {
    let mut owned: BTreeMap<String, Vec<(String, u64)>> = BTreeMap::new();
    let Some(game) = patch_transaction_root(root)? else {
        return Ok(owned);
    };
    let operations = game
        .join(locust_core::injection_transaction::STORE_DIR)
        .join("operations");
    locust_core::patch::zipsec::ensure_no_links(&game, operations.strip_prefix(&game).unwrap())?;
    let mut revisions = Vec::new();
    for entry in std::fs::read_dir(&operations)? {
        let entry = entry?;
        let id = entry.file_name();
        if id
            .to_str()
            .and_then(|s| uuid::Uuid::parse_str(s).ok())
            .is_none()
        {
            continue;
        }
        let dir = entry.path();
        locust_core::patch::zipsec::ensure_no_links(&game, dir.strip_prefix(&game).unwrap())?;
        if patch_metadata(&dir.join("phase.json"))? != "completed" {
            continue;
        }
        let plan = patch_metadata(&dir.join("plan.json"))?;
        if plan["schema_version"] != 1
            || plan["game_root"].as_str().map(Path::new) != Some(game.as_path())
            || plan["transaction_id"].as_str() != id.to_str()
        {
            return Err(parse_err(
                "XP3 provenance",
                "transaction plan identity mismatch",
            ));
        }
        let files = plan["files"]
            .as_array()
            .ok_or_else(|| parse_err("XP3 provenance", "missing transaction files"))?;
        for change in files {
            let Some(name) = change["path"]
                .as_str()
                .filter(|s| patch_number(s).is_some())
            else {
                continue;
            };
            let result = recorded_hash(&change["result"])
                .ok_or_else(|| parse_err("XP3 provenance", "invalid result hash"))?;
            if change
                .get("original")
                .is_some_and(serde_json::Value::is_null)
            {
                owned.entry(name.to_owned()).or_default().push(result);
            } else {
                let original = recorded_hash(&change["original"])
                    .ok_or_else(|| parse_err("XP3 provenance", "invalid original hash"))?;
                revisions.push((name.to_owned(), original, result));
            }
        }
    }
    loop {
        let mut progress = false;
        for (name, before, after) in &revisions {
            if let Some(hashes) = owned.get_mut(name) {
                if hashes.contains(before) && !hashes.contains(after) {
                    hashes.push(after.clone());
                    progress = true;
                }
            }
        }
        if !progress {
            break;
        }
    }
    Ok(owned)
}

fn choose_patch_target(root: &Path) -> std::result::Result<PatchTarget, PatchRefusal> {
    let provenance_error = |e: LocustError| ("patch_provenance_invalid", e.to_string());
    let mut patches = BTreeMap::new();
    for entry in std::fs::read_dir(root).map_err(|e| provenance_error(e.into()))? {
        let entry = entry.map_err(|e| provenance_error(e.into()))?;
        if let Some(n) = entry.file_name().to_str().and_then(patch_number) {
            if !entry
                .file_type()
                .map_err(|e| provenance_error(e.into()))?
                .is_file()
                || patches.insert(n, entry.path()).is_some()
            {
                return Err((
                    "patch_target_conflict",
                    "patch slot is ambiguous or not a regular file".into(),
                ));
            }
        }
    }
    for (expected, &actual) in (1u32..).zip(patches.keys()) {
        if expected != actual {
            return Err((
                "patch_sequence_gap",
                format!(
                    "missing patch slot {expected}; filling it could activate later game archives"
                ),
            ));
        }
    }
    let mut target = None;
    for (name, hashes) in owned_patches(root).map_err(provenance_error)? {
        let path = root.join(&name);
        if !path.try_exists().map_err(|e| provenance_error(e.into()))? {
            continue;
        }
        let hash = locust_core::database::sha256_file(&path).map_err(provenance_error)?;
        if !hashes.contains(&hash) {
            return Err((
                "patch_provenance_mismatch",
                format!("Locust archive {name} changed since its recorded transaction"),
            ));
        }
        if patch_number(&name) != patches.last_key_value().map(|(&n, _)| n) || target.is_some() {
            return Err(("patch_precedence_conflict", "Locust overlay is not the unique last patch; refusing to write an ignored revision".into()));
        }
        target = Some(PatchTarget {
            path,
            owned_hash: Some(hash),
        });
    }
    if let Some(target) = target {
        return Ok(target);
    }
    let next = patches
        .last_key_value()
        .map_or(Some(1), |(&n, _)| n.checked_add(1))
        .ok_or_else(|| ("patch_target_conflict", "patch number overflow".into()))?;
    Ok(PatchTarget {
        path: root.join(if next == 1 {
            "patch.xp3".to_owned()
        } else {
            format!("patch{next}.xp3")
        }),
        owned_hash: None,
    })
}

/// Tokenize just enough TJS to recognize the documented loader without accepting
/// commented-out code or a string containing it. String tokens retain delimiters.
fn loader_tokens(text: &str) -> Vec<String> {
    let mut chars = text.chars().peekable();
    let mut tokens = Vec::new();
    while let Some(c) = chars.next() {
        if c.is_whitespace() {
            continue;
        }
        if c == '/' && chars.peek() == Some(&'/') {
            for ch in chars.by_ref() {
                if ch == '\n' {
                    break;
                }
            }
        } else if c == '/' && chars.peek() == Some(&'*') {
            chars.next();
            while let Some(ch) = chars.next() {
                if ch == '*' && chars.peek() == Some(&'/') {
                    chars.next();
                    break;
                }
            }
        } else if matches!(c, '\'' | '"') {
            let mut token = String::from("\"");
            while let Some(ch) = chars.next() {
                if ch == c {
                    break;
                }
                token.push(ch);
                if ch == '\\' {
                    if let Some(next) = chars.next() {
                        token.push(next);
                    }
                }
            }
            token.push('"');
            tokens.push(token);
        } else if c.is_alphanumeric() || c == '_' {
            let mut token = c.to_string();
            while chars
                .peek()
                .is_some_and(|ch| ch.is_alphanumeric() || *ch == '_')
            {
                token.push(chars.next().unwrap());
            }
            tokens.push(token);
        } else {
            tokens.push(c.to_string());
        }
    }
    tokens
}

fn has_numbered_patch_loop(text: &str) -> bool {
    let tokens = loader_tokens(text);
    // KAG3's actual Initialize.tjs. Accept whitespace, comments, quote styles,
    // optional `var`, and any loop variable. Unrecognized custom loaders refuse.
    for start in 0..tokens.len() {
        let mut tail = &tokens[start..];
        if !tail.starts_with(&["for".into(), "(".into()]) {
            continue;
        }
        tail = &tail[2..];
        if tail.first().is_some_and(|s| s == "var") {
            tail = &tail[1..];
        }
        let Some(variable) = tail.first() else {
            continue;
        };
        let expected = loader_tokens(&format!(
            r#"{variable}=2;;{variable}++) {{
            if(Storages.isExistentStorage(System.exePath+"patch"+{variable}+".xp3"))
                Storages.addAutoPath(System.exePath+"patch"+{variable}+".xp3>");
            else break;
        }}"#
        ));
        if tail.starts_with(&expected) {
            return true;
        }
    }
    false
}

fn check_numbered_patch_loader(
    root: &Path,
    report: &mut InjectionReport,
) -> std::result::Result<(), PatchRefusal> {
    let mut readable = Vec::new();
    for entry in walkdir::WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .filter_entry(crate::discovery::is_game_entry)
        .filter_map(|e| e.ok())
    {
        if entry.file_type().is_file() && entry.file_name().eq_ignore_ascii_case("Initialize.tjs") {
            if let Ok(bytes) = std::fs::read(entry.path()) {
                if let Ok(decoded) = decode_ks_bytes(&bytes, "Initialize.tjs") {
                    if looks_like_readable_ks(&decoded.text) {
                        readable.push((entry.path().display().to_string(), decoded.text));
                    }
                }
            }
        }
    }
    for path in KirikiriPlugin::find_top_level_xp3(root) {
        let Ok(archive) = Xp3Archive::open(&path) else {
            continue;
        };
        for entry in &archive.entries {
            if entry.flags & 0x8000_0000 != 0
                || !entry
                    .name
                    .rsplit('/')
                    .next()
                    .is_some_and(|s| s.eq_ignore_ascii_case("Initialize.tjs"))
            {
                continue;
            }
            if let Ok(bytes) = archive.read_entry(entry) {
                if let Ok(decoded) = decode_ks_bytes(&bytes, "Initialize.tjs") {
                    if looks_like_readable_ks(&decoded.text) {
                        readable.push((format!("{}/{}", path.display(), entry.name), decoded.text));
                    }
                }
            }
        }
    }
    // Conservatively refuse if any readable game initializer lacks the known loop:
    // custom bootstraps can choose a different initializer than script precedence.
    for (label, text) in &readable {
        if !has_numbered_patch_loop(text) {
            return Err((
                "numbered_patch_loading_unsupported",
                format!("readable {label} does not contain the documented numbered-patch loop"),
            ));
        }
    }
    if readable.is_empty() {
        let message = "Initialize.tjs is not readable; using the documented default KAG numbered-patch loading rule";
        warn!("{message}");
        report.warnings.push(message.into());
    }
    Ok(())
}

// ─── Plugin ────────────────────────────────────────────────────────────────

impl FormatPlugin for KirikiriPlugin {
    fn id(&self) -> &str {
        "kirikiri"
    }

    fn name(&self) -> &str {
        "KiriKiri / KAG"
    }

    fn description(&self) -> &str {
        "KiriKiri KAG loose .ks + unencrypted XP3 (UTF-16/UTF-8/SJIS; FE FE 0/1/2; numbered patch inject)"
    }

    fn stability(&self) -> locust_core::extraction::FormatStability {
        locust_core::extraction::FormatStability::Experimental
    }

    fn supported_extensions(&self) -> &[&str] {
        &[".ks", ".xp3"]
    }

    fn supported_modes(&self) -> Vec<OutputMode> {
        vec![OutputMode::Replace]
    }

    fn detect(&self, path: &Path) -> bool {
        Self::has_xp3(path) || !Self::find_ks_files(path).is_empty()
    }

    fn extract(&self, path: &Path) -> Result<Vec<StringEntry>> {
        let root = Self::root_dir(path);
        let ks_files = Self::find_ks_files(path);
        let xp3_files = Self::find_top_level_xp3(path);

        if ks_files.is_empty() && xp3_files.is_empty() {
            return Err(parse_err(
                &path.display().to_string(),
                "no .ks script files or .xp3 archives found",
            ));
        }

        // Collect candidate script identities first, then keep the best source per
        // normalized key (patchN.xp3 > data.xp3 > loose; unencrypted tool dumps lose).
        let mut loose_cands: Vec<(String, PathBuf)> = Vec::new();
        for fpath in &ks_files {
            let rel = fpath
                .strip_prefix(&root)
                .unwrap_or(fpath.as_path())
                .to_string_lossy()
                .replace('\\', "/");
            loose_cands.push((rel, fpath.clone()));
        }

        let mut xp3_parse_errors = 0usize;
        let mut last_xp3_err = String::new();
        let mut xp3_ks_seen = 0usize;
        let mut xp3_skipped = 0usize;
        // (rel virtual path, archive path, first entry with that name)
        let mut xp3_cands: Vec<(String, PathBuf, Xp3Entry)> = Vec::new();
        let mut open_archives: HashMap<PathBuf, Xp3Archive> = HashMap::new();

        for arch_path in &xp3_files {
            let arch_name = arch_path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("archive.xp3")
                .to_string();
            let archive = match Xp3Archive::open(arch_path) {
                Ok(a) => a,
                Err(e) => {
                    xp3_parse_errors += 1;
                    last_xp3_err = e.to_string();
                    warn!(archive = %arch_path.display(), error = %e, "failed to open XP3");
                    continue;
                }
            };
            // ks_entries() is in index order, so the first yield of a name is the
            // same entry the old linear find returned. Later duplicates reuse it.
            let mut first_ks: HashMap<String, Xp3Entry> = HashMap::new();
            for entry in archive.ks_entries() {
                note_xp3_name_inspection();
                xp3_ks_seen += 1;
                let inner = entry.name.replace('\\', "/");
                let rel = format!("{arch_name}/{inner}");
                let canonical = first_ks
                    .entry(entry.name.clone())
                    .or_insert_with(|| entry.clone())
                    .clone();
                xp3_cands.push((rel, arch_path.clone(), canonical));
            }
            open_archives.insert(arch_path.clone(), archive);
        }

        // Decode BEFORE ranking: an encrypted archive can outrank a plaintext
        // copy of the same script, and picking it would drop the only readable
        // source. Rank only arbitrates between sources that actually decoded.
        let mut readable: Vec<(String, PathBuf, String)> = Vec::new();

        for (rel, fpath) in &loose_cands {
            let bytes = std::fs::read(fpath)?;
            let decoded = decode_ks_bytes(&bytes, rel)?;
            if !looks_like_readable_ks(&decoded.text) {
                warn!(script = %rel, "loose .ks decoded to non-script bytes; skipped");
                continue;
            }
            readable.push((rel.clone(), fpath.clone(), decoded.text));
        }

        for (rel, arch_path, entry) in &xp3_cands {
            let Some(archive) = open_archives.get(arch_path) else {
                continue;
            };
            let arch_name = arch_path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("archive.xp3");
            let payload = match archive.read_entry(entry) {
                Ok(p) => p,
                Err(e) => {
                    warn!(
                        archive = %arch_name,
                        entry = %entry.name,
                        error = %e,
                        "failed to read XP3 .ks entry; skipped"
                    );
                    xp3_skipped += 1;
                    continue;
                }
            };
            let virtual_path = PathBuf::from(rel);
            match decode_ks_bytes(&payload, rel) {
                Ok(decoded) if looks_like_readable_ks(&decoded.text) => {
                    readable.push((rel.clone(), virtual_path, decoded.text));
                }
                Ok(_) => {
                    warn!(
                        archive = %arch_name,
                        entry = %entry.name,
                        "XP3 .ks decoded to non-script bytes (cxdec/encrypted?); skipped"
                    );
                    xp3_skipped += 1;
                }
                Err(e) => {
                    warn!(
                        archive = %arch_name,
                        entry = %entry.name,
                        error = %e,
                        "XP3 .ks payload did not decode as text (cxdec/encrypted?); skipped"
                    );
                    xp3_skipped += 1;
                }
            }
        }

        let readable_rels: Vec<String> = readable.iter().map(|(r, _, _)| r.clone()).collect();
        let winners = select_best_script_rels(&readable_rels);

        let mut all = Vec::new();
        for (rel, src_path, text) in &readable {
            if winners.contains(rel) {
                all.extend(extract_lines_from_text(text, rel, src_path.clone()));
            }
        }

        if all.is_empty() && ks_files.is_empty() {
            if xp3_parse_errors > 0 && xp3_ks_seen == 0 {
                return Err(parse_err(
                    &path.display().to_string(),
                    format!("failed to parse XP3 archive(s): {last_xp3_err}"),
                ));
            }
            if xp3_ks_seen == 0 {
                return Err(parse_err(
                    &path.display().to_string(),
                    "no .ks scripts found in top-level XP3 archives",
                ));
            }
            if xp3_skipped > 0 {
                warn!(
                    skipped = xp3_skipped,
                    "all XP3 .ks entries were skipped (decode/read failures)"
                );
            }
        }

        Ok(all)
    }

    fn inject(&self, path: &Path, entries: &[StringEntry]) -> Result<InjectionReport> {
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

        // Collect modified XP3 payloads for one new or transaction-owned overlay.
        let mut patch_files: Vec<(String, Vec<u8>)> = Vec::new();
        // Cache opened base archives: archive file name → archive + name index
        let mut archive_cache: HashMap<String, CachedXp3> = HashMap::new();

        let patch_target = if by_file.keys().any(|p| split_xp3_virtual_path(p).is_some()) {
            let target = choose_patch_target(&search_root).and_then(|target| {
                check_numbered_patch_loader(&search_root, &mut report)?;
                Ok(target)
            });
            if let Err((reason, message)) = &target {
                report.warnings.push(format!("{reason}: {message}"));
            }
            Some(target)
        } else {
            None
        };
        let mut pending_patch_strings = 0;

        for (file_path, file_entries) in &by_file {
            if let Some((archive_name, inner)) = split_xp3_virtual_path(file_path) {
                if let Some(Err((reason, _))) = &patch_target {
                    let skipped = file_entries
                        .iter()
                        .filter(|entry| entry.translation.is_some())
                        .count();
                    if skipped > 0 {
                        report.skip(reason, skipped);
                    }
                    continue;
                }
                if !archive_cache.contains_key(&archive_name) {
                    let arch_path = search_root.join(&archive_name);
                    match Xp3Archive::open(&arch_path) {
                        Ok(a) => {
                            let by_name = index_xp3_names(&a.entries);
                            archive_cache.insert(
                                archive_name.clone(),
                                CachedXp3 {
                                    archive: a,
                                    by_name,
                                },
                            );
                        }
                        Err(e) => {
                            report.warnings.push(format!(
                                "cannot open base archive {archive_name} for inject: {e}"
                            ));
                            report.skip("archive_error", file_entries.len());
                            continue;
                        }
                    }
                }
                let cached = archive_cache.get(&archive_name).unwrap();
                let inner_norm = inner.replace('\\', "/");

                let entry = match cached
                    .by_name
                    .get(&inner_norm)
                    .and_then(|&i| cached.archive.entries.get(i))
                {
                    Some(e) => e.clone(),
                    None => {
                        report
                            .warnings
                            .push(format!("entry {inner} not found in {archive_name}"));
                        report.skip("target_missing", file_entries.len());
                        continue;
                    }
                };

                let bytes = match cached.archive.read_entry(&entry) {
                    Ok(b) => b,
                    Err(e) => {
                        report
                            .warnings
                            .push(format!("read {archive_name}/{inner}: {e}"));
                        report.skip("read_error", file_entries.len());
                        continue;
                    }
                };
                let label = format!("{archive_name}/{inner}");
                match apply_translations_classified(&bytes, &label, file_entries) {
                    Ok(Some((encoded, written))) => {
                        patch_files.push((inner.replace('\\', "/"), encoded));
                        pending_patch_strings += written;
                    }
                    Ok(None) => {
                        // Only a wholly unchanged file counted these rows before.
                        for entry in file_entries {
                            report.skip(
                                if entry.translation.is_none() {
                                    "untranslated"
                                } else {
                                    "unchanged"
                                },
                                1,
                            );
                        }
                    }
                    Err((reason, e)) => {
                        report
                            .warnings
                            .push(format!("cannot translate {label}: {e}"));
                        report.skip(reason, file_entries.len());
                    }
                }
                continue;
            }

            // Loose file path
            let actual = if file_path.exists() {
                file_path.clone()
            } else {
                let as_rel = search_root.join(file_path);
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
            match apply_translations_classified(&bytes, &label, file_entries) {
                Ok(Some((encoded, written))) => {
                    std::fs::write(&actual, &encoded)?;
                    report.files_modified += 1;
                    report.files_written.push(actual);
                    report.strings_written += written;
                }
                Ok(None) => {
                    // Only a wholly unchanged file counted these rows before.
                    for entry in file_entries {
                        report.skip(
                            if entry.translation.is_none() {
                                "untranslated"
                            } else {
                                "unchanged"
                            },
                            1,
                        );
                    }
                }
                Err((reason, e)) => {
                    report
                        .warnings
                        .push(format!("cannot re-encode {label}: {e}"));
                    report.skip(reason, file_entries.len());
                }
            }
        }

        if !patch_files.is_empty() {
            let target = patch_target.as_ref().unwrap().as_ref().unwrap();
            let mut merged = BTreeMap::new();
            // Preserve previously translated members omitted from this revision.
            if target.owned_hash.is_some() {
                let archive = Xp3Archive::open(&target.path)
                    .map_err(|e| parse_err("Locust patch", e.to_string()))?;
                for entry in &archive.entries {
                    let data = archive
                        .read_entry(entry)
                        .map_err(|e| parse_err("Locust patch", e.to_string()))?;
                    if merged.insert(entry.name.clone(), data).is_some() {
                        return Err(parse_err("Locust patch", "duplicate owned member"));
                    }
                }
            }
            let mut changed = std::collections::HashSet::new();
            for (name, data) in patch_files {
                if !changed.insert(name.clone()) {
                    return Err(parse_err(
                        "patch.xp3",
                        "ambiguous duplicate patch output member",
                    ));
                }
                merged.insert(name, data);
            }
            let list: Vec<(String, Vec<u8>)> = merged.into_iter().collect();
            match kirikiri_xp3::write_xp3(&list) {
                Ok(bytes) => {
                    // create_new is also a last-moment guard against overwriting
                    // a patch another process installed while we were preparing.
                    use std::io::Write;
                    let patch_path = target.path.clone();
                    let mut options = std::fs::OpenOptions::new();
                    options.write(true);
                    if let Some(expected) = &target.owned_hash {
                        if locust_core::database::sha256_file(&patch_path)? != *expected {
                            return Err(parse_err(
                                "Locust patch",
                                "owned archive changed before revision",
                            ));
                        }
                        options.truncate(true);
                    } else {
                        options.create_new(true);
                    }
                    let mut output = options.open(&patch_path)?;
                    output.write_all(&bytes)?;
                    output.sync_all()?;
                    report.files_modified += 1;
                    report.strings_written += pending_patch_strings;
                    report.files_written.push(patch_path);
                }
                Err(e) => {
                    report.skip("rebuild_error", pending_patch_strings);
                    report
                        .warnings
                        .push(format!("failed to build patch.xp3 (0 archive writes): {e}"));
                }
            }
        }

        Ok(report)
    }
}

#[cfg(test)]
mod tests {
    const C124_INIT: &str = r#"
useArchiveIfExists("patch.xp3");
for(var i = 2; ; i++) {
    if(Storages.isExistentStorage(System.exePath + "patch" + i + ".xp3"))
        Storages.addAutoPath(System.exePath + "patch" + i + ".xp3>");
    else break;
}
"#;

    fn c124_game(root: &Path) -> Vec<StringEntry> {
        for (name, text) in [("patch.xp3", "OLD"), ("patch2.xp3", "Hello.")] {
            fs::write(
                root.join(name),
                kirikiri_xp3::write_xp3(&[
                    (
                        "scenario/route/story.ks".into(),
                        format!("{text}\n").into_bytes(),
                    ),
                    ("keep.bin".into(), vec![0, 1, 2, 255]),
                ])
                .unwrap(),
            )
            .unwrap();
        }
        let mut rows = KirikiriPlugin::new().extract(root).unwrap();
        assert_eq!(rows.len(), 1);
        assert!(rows[0].file_path.to_string_lossy().contains("patch2.xp3"));
        rows[0].translation = Some("TL Hello.".into());
        rows
    }

    #[test]
    fn c124_next_patch_preserves_archives_and_member_path() {
        let dir = tempfile::tempdir().unwrap();
        let rows = c124_game(dir.path());
        let originals: Vec<_> = ["patch.xp3", "patch2.xp3"]
            .iter()
            .map(|name| (name, fs::read(dir.path().join(name)).unwrap()))
            .collect();
        let report = KirikiriPlugin::new().inject(dir.path(), &rows).unwrap();
        assert_eq!(report.strings_written, 1, "{report:?}");
        assert_eq!(report.strings_skipped, 0);
        assert_eq!(report.files_written, vec![dir.path().join("patch3.xp3")]);
        assert!(report
            .warnings
            .iter()
            .any(|w| w.contains("Initialize.tjs") && w.contains("documented default")));
        let archive = Xp3Archive::open(&dir.path().join("patch3.xp3")).unwrap();
        assert_eq!(archive.entries.len(), 1);
        assert_eq!(archive.entries[0].name, "scenario/route/story.ks");
        let extracted = KirikiriPlugin::new().extract(dir.path()).unwrap();
        assert_eq!(extracted[0].source, "TL Hello.");
        assert!(extracted[0]
            .file_path
            .to_string_lossy()
            .contains("patch3.xp3"));
        for (name, bytes) in originals {
            assert_eq!(fs::read(dir.path().join(name)).unwrap(), bytes);
        }
    }

    #[test]
    fn c124_direct_revision_uses_transaction_owned_patch() {
        use locust_core::{backup::BackupManager, database::Database, extraction::inject_direct};
        let dir = tempfile::tempdir().unwrap();
        let game = dir.path().join("game");
        fs::create_dir(&game).unwrap();
        let mut rows = c124_game(&game);
        let db = Database::open_in_memory().unwrap();
        db.save_entries(&rows).unwrap();
        let backups = BackupManager::new(dir.path().join("backups"));
        let registry = crate::default_registry();
        let first =
            inject_direct(&registry, &db, &backups, &game, "kirikiri", &["en".into()]).unwrap();
        assert_eq!(first.strings_written, 1, "{first:?}");
        for n in 2..=3 {
            rows[0].translation = Some(format!("R{n} TL Hello."));
            db.save_entries(&rows).unwrap();
            let revised =
                inject_direct(&registry, &db, &backups, &game, "kirikiri", &["en".into()]).unwrap();
            assert_eq!(revised.strings_written, 1, "{revised:?}");
            assert!(!game.join("patch4.xp3").exists());
            let extracted = KirikiriPlugin::new().extract(&game).unwrap();
            assert_eq!(extracted[0].source, format!("R{n} TL Hello."));
        }
    }

    #[test]
    fn c124_readable_custom_initialize_refuses_numbered_patch() {
        for archived in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let rows = c124_game(dir.path());
            let init = format!("useArchiveIfExists(\"patch.xp3\");\n/* {C124_INIT} */\n");
            if archived {
                fs::write(
                    dir.path().join("system.xp3"),
                    kirikiri_xp3::write_xp3(&[("system/Initialize.tjs".into(), init.into_bytes())])
                        .unwrap(),
                )
                .unwrap();
            } else {
                fs::write(dir.path().join("Initialize.tjs"), init).unwrap();
            }
            let report = KirikiriPlugin::new().inject(dir.path(), &rows).unwrap();
            assert_eq!(report.strings_written, 0);
            assert_eq!(
                report
                    .skip_reasons
                    .get("numbered_patch_loading_unsupported"),
                Some(&1),
                "{report:?}"
            );
            assert!(!dir.path().join("patch3.xp3").exists());
        }
    }

    #[test]
    fn c124_readable_standard_initialize_allows_numbered_patch() {
        for archived in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let rows = c124_game(dir.path());
            if archived {
                fs::write(
                    dir.path().join("system.xp3"),
                    kirikiri_xp3::write_xp3(&[(
                        "system/Initialize.tjs".into(),
                        C124_INIT.as_bytes().to_vec(),
                    )])
                    .unwrap(),
                )
                .unwrap();
            } else {
                fs::write(dir.path().join("Initialize.tjs"), C124_INIT).unwrap();
            }
            let report = KirikiriPlugin::new().inject(dir.path(), &rows).unwrap();
            assert_eq!(report.strings_written, 1, "{report:?}");
            assert!(report.warnings.is_empty(), "{report:?}");
        }
    }

    #[test]
    fn c124_patch_gap_is_not_silently_activated() {
        let dir = tempfile::tempdir().unwrap();
        let rows = c124_game(dir.path());
        fs::rename(dir.path().join("patch2.xp3"), dir.path().join("patch4.xp3")).unwrap();
        let report = KirikiriPlugin::new().inject(dir.path(), &rows).unwrap();
        assert_eq!(report.strings_written, 0);
        assert_eq!(
            report.skip_reasons.get("patch_sequence_gap"),
            Some(&1),
            "{report:?}"
        );
        assert!(!dir.path().join("patch2.xp3").exists());
        assert!(!dir.path().join("patch5.xp3").exists());
    }

    #[test]
    fn c124_no_transaction_never_adopts_archive_by_name_or_content() {
        let dir = tempfile::tempdir().unwrap();
        let rows = c124_game(dir.path());
        let plugin = KirikiriPlugin::new();
        plugin.inject(dir.path(), &rows).unwrap();
        let previous = fs::read(dir.path().join("patch3.xp3")).unwrap();
        // A standalone call has no completed Direct transaction proving ownership.
        let report = plugin.inject(dir.path(), &rows).unwrap();
        assert_eq!(report.files_written, vec![dir.path().join("patch4.xp3")]);
        assert_eq!(fs::read(dir.path().join("patch3.xp3")).unwrap(), previous);
    }

    #[test]
    fn c124_owned_archive_drift_and_later_game_patch_refuse() {
        use locust_core::{backup::BackupManager, database::Database, extraction::inject_direct};
        let dir = tempfile::tempdir().unwrap();
        let game = dir.path().join("game");
        fs::create_dir(&game).unwrap();
        let rows = c124_game(&game);
        let db = Database::open_in_memory().unwrap();
        db.save_entries(&rows).unwrap();
        inject_direct(
            &crate::default_registry(),
            &db,
            &BackupManager::new(dir.path().join("backups")),
            &game,
            "kirikiri",
            &["en".into()],
        )
        .unwrap();
        let patch = game.join("patch3.xp3");
        let original = fs::read(&patch).unwrap();
        let edited = [original.as_slice(), b"USER EDIT"].concat();
        fs::write(&patch, &edited).unwrap();
        let report = KirikiriPlugin::new().inject(&game, &rows).unwrap();
        assert_eq!(
            report.skip_reasons.get("patch_provenance_mismatch"),
            Some(&1)
        );
        assert_eq!(fs::read(&patch).unwrap(), edited);
        assert!(!game.join("patch4.xp3").exists());
        fs::write(&patch, &original).unwrap();
        fs::write(game.join("patch4.xp3"), &original).unwrap();
        let report = KirikiriPlugin::new().inject(&game, &rows).unwrap();
        assert_eq!(
            report.skip_reasons.get("patch_precedence_conflict"),
            Some(&1)
        );
        assert_eq!(fs::read(&patch).unwrap(), original);
        assert!(!game.join("patch5.xp3").exists());
    }

    #[test]
    fn c124_partial_revision_retains_other_owned_members() {
        use locust_core::{backup::BackupManager, database::Database, extraction::inject_direct};
        let dir = tempfile::tempdir().unwrap();
        let game = dir.path().join("game");
        fs::create_dir(&game).unwrap();
        c124_game(&game);
        fs::write(
            game.join("data.xp3"),
            kirikiri_xp3::write_xp3(&[("other.ks".into(), b"Second.\n".to_vec())]).unwrap(),
        )
        .unwrap();
        let mut rows = KirikiriPlugin::new().extract(&game).unwrap();
        for row in &mut rows {
            row.translation = Some(format!("TL {}", row.source));
        }
        let db = Database::open_in_memory().unwrap();
        db.save_entries(&rows).unwrap();
        inject_direct(
            &crate::default_registry(),
            &db,
            &BackupManager::new(dir.path().join("backups")),
            &game,
            "kirikiri",
            &["en".into()],
        )
        .unwrap();
        rows.retain(|row| row.source == "Hello.");
        rows[0].translation = Some("R2 TL Hello.".into());
        let report = KirikiriPlugin::new().inject(&game, &rows).unwrap();
        assert_eq!(report.strings_written, 1);
        assert!(!game.join("patch4.xp3").exists());
        let extracted = KirikiriPlugin::new().extract(&game).unwrap();
        assert!(extracted.iter().any(|row| row.source == "TL Second."));
        assert!(extracted.iter().any(|row| row.source == "R2 TL Hello."));
    }

    #[test]
    fn c124_loader_comments_and_strings_are_not_executable_loops() {
        assert!(!has_numbered_patch_loop(&format!("/*{C124_INIT}*/")));
        assert!(!has_numbered_patch_loop(&format!(
            "var text = '{}';",
            C124_INIT
        )));
        assert!(has_numbered_patch_loop(
            &C124_INIT.replace("var i", "i").replace('"', "'")
        ));
    }

    #[test]
    fn c123_named_skips_for_existing_patch_preserve_zero_count() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("patch.xp3"), b"existing patch").unwrap();
        std::fs::write(
            dir.path().join("Initialize.tjs"),
            "useArchiveIfExists('patch.xp3');",
        )
        .unwrap();
        let mut entry = StringEntry::new(
            "data.xp3/story.ks#kag:1",
            "Hello",
            "data.xp3/story.ks".into(),
        );
        let plugin = KirikiriPlugin::new();
        let report = plugin.inject(dir.path(), &[entry.clone()]).unwrap();
        assert_eq!((report.strings_written, report.strings_skipped), (0, 0));
        assert!(report.skip_reasons.is_empty(), "{report:?}");
        entry.translation = Some("Translated".into());
        let report = plugin.inject(dir.path(), &[entry]).unwrap();
        assert_eq!((report.strings_written, report.strings_skipped), (0, 1));
        assert_eq!(
            report.skip_reasons,
            [("numbered_patch_loading_unsupported".into(), 1)].into()
        );
        assert_eq!(
            std::fs::read(dir.path().join("patch.xp3")).unwrap(),
            b"existing patch"
        );
    }

    #[test]
    fn c123_named_skips_for_kag_noops_and_rejections() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("story.ks");
        let original = b"Hello world[l]\n";
        std::fs::write(&path, original).unwrap();
        let plugin = KirikiriPlugin::new();
        let entry = plugin.extract(&path).unwrap().remove(0);
        for (translation, reason) in [
            (None, "untranslated"),
            (Some("Hello world[l]"), "unchanged"),
            (Some("Hello\nworld[l]"), "unsafe_controls"),
            (Some("Translated world"), "invalid_placeholders"),
        ] {
            let mut row = entry.clone();
            row.translation = translation.map(str::to_string);
            let mut report = plugin.inject(&path, &[row]).unwrap();
            assert_eq!((report.strings_written, report.strings_skipped), (0, 1));
            report.classify_remaining_skips();
            assert_eq!(report.skip_reasons, [(reason.into(), 1)].into(), "{reason}");
            assert_eq!(std::fs::read(&path).unwrap(), original);
        }
    }

    #[test]
    fn c123_named_skips_for_kag_encoding_and_stale_source() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("story.ks");
        let (bytes, _, _) = encoding_rs::SHIFT_JIS.encode("こんにちは世界\n");
        std::fs::write(&path, &bytes).unwrap();
        let plugin = KirikiriPlugin::new();
        let entry = plugin.extract(&path).unwrap().remove(0);
        for reason in [
            "not_encodable",
            "source_changed",
            "invalid_locator",
            "ambiguous_target",
        ] {
            let mut row = entry.clone();
            row.translation = Some(
                if reason == "not_encodable" {
                    "\u{1f600}"
                } else {
                    "Hello"
                }
                .into(),
            );
            if reason == "source_changed" {
                row.source = "Old source".into();
            }
            if reason == "invalid_locator" {
                row.id = "no locator".into();
            }
            let rows = if reason == "ambiguous_target" {
                vec![row.clone(), row]
            } else {
                vec![row]
            };
            let report = plugin.inject(&path, &rows).unwrap();
            assert_eq!(
                (report.strings_written, report.strings_skipped),
                (0, rows.len())
            );
            assert_eq!(
                report.skip_reasons,
                [(reason.into(), rows.len())].into(),
                "{reason}"
            );
            assert_eq!(std::fs::read(&path).unwrap(), bytes.as_ref());
        }
    }
    use super::*;
    use std::fs;

    fn tempdir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("locust_krkr_{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn sample_script() -> &'static str {
        "; comment line\r\n\
*start\r\n\
@wait time=100\r\n\
[wait time=50]\r\n\
[name] Hello, world!\r\n\
This is narration.\r\n\
@jump target=*end\r\n"
    }

    #[test]
    fn vn01_bare_cr() {
        let rows = extract_lines_from_text(
            ";comment\rHello.\rGoodbye.\r",
            "story.ks",
            "story.ks".into(),
        );
        assert_eq!(
            rows.iter().map(|r| r.source.as_str()).collect::<Vec<_>>(),
            ["Hello.", "Goodbye."]
        );
    }

    #[test]
    fn vn01_script_state() {
        for text in [
            "[iscript]\nSystem.exit();\n[endscript]\nHello.\n",
            "  [iscript]\\\r  System.inform('quoted [endscript]');\r\n\t\"player looking text\";\n  System.exit();\r  [endscript]\\\nHello.\n",
            "@iscript\n  \"Hello inside code\";\n@endscript\nHello.\n",
            "[iscript]\nSystem.exit();\nHello.\n",
        ] {
            let rows = extract_lines_from_text(text, "story.ks", "story.ks".into());
            assert_eq!(rows.iter().map(|r| r.source.as_str()).collect::<Vec<_>>(),
                if !text.contains("endscript") { vec![] } else { vec!["Hello."] });
        }
    }

    #[test]
    fn vn01_lossless_encodings_and_separators() {
        let dir = tempdir();
        let path = dir.join("story.ks");
        let original = "Hello.\n;comment\rGoodbye.\r\nAnother.\r";
        let expected = format!("AUDIT {original}");
        let mut fixtures = vec![original.as_bytes().to_vec()];
        fixtures.push([&[0xEF, 0xBB, 0xBF][..], original.as_bytes()].concat());
        fixtures.push([&[0xFF, 0xFE][..], &utf16le_bytes_from_str(original)].concat());
        for mode in 0..=2 {
            let mut body = utf16le_bytes_from_str(original);
            let mut wrapped = vec![0xFE, 0xFE, mode, 0xFF, 0xFE];
            match mode {
                0 => mode0_encode_units(&mut body),
                1 => mode1_swap_units(&mut body),
                _ => {
                    let compressed = miniz_oxide::deflate::compress_to_vec_zlib(&body, 6);
                    wrapped.extend_from_slice(&(compressed.len() as u64).to_le_bytes());
                    wrapped.extend_from_slice(&(body.len() as u64).to_le_bytes());
                    body = compressed;
                }
            }
            wrapped.extend_from_slice(&body);
            fixtures.push(wrapped);
        }
        for bytes in fixtures {
            fs::write(&path, &bytes).unwrap();
            let plugin = KirikiriPlugin::new();
            let mut rows = plugin.extract(&dir).unwrap();
            let untouched: Vec<_> = rows
                .iter()
                .filter(|r| r.source != "Hello.")
                .map(|r| (r.id.clone(), r.source.clone()))
                .collect();
            rows.iter_mut()
                .find(|r| r.source == "Hello.")
                .unwrap()
                .translation = Some("AUDIT Hello.".into());
            let report = plugin.inject(&dir, &rows).unwrap();
            assert_eq!(report.strings_written, 1, "{report:?}");
            let after = fs::read(&path).unwrap();
            let decoded = decode_ks_bytes(&after, "story.ks").unwrap();
            assert_eq!(decoded.text, expected);
            let before_decoded = decode_ks_bytes(&bytes, "story.ks").unwrap();
            assert_eq!(decoded.encoding, before_decoded.encoding);
            assert_eq!(decoded.cipher, before_decoded.cipher);
            if bytes.starts_with(&[0xFE, 0xFE, 2]) {
                // The compression stream may change; the entire inflated suffix must not.
                let plain = miniz_oxide::inflate::decompress_to_vec_zlib(&after[21..]).unwrap();
                assert_eq!(plain, utf16le_bytes_from_str(&expected));
            } else {
                let insertion = if before_decoded.encoding == KsEncoding::Utf16Le {
                    let mut prefix = utf16le_bytes_from_str("AUDIT ");
                    match before_decoded.cipher {
                        CipherMode::Mode0 => mode0_encode_units(&mut prefix),
                        CipherMode::Mode1 => mode1_swap_units(&mut prefix),
                        _ => {}
                    }
                    prefix
                } else {
                    b"AUDIT ".to_vec()
                };
                let header = if bytes.starts_with(&[0xFE, 0xFE]) {
                    5
                } else if bytes.starts_with(&[0xFF, 0xFE]) {
                    2
                } else if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
                    3
                } else {
                    0
                };
                assert_eq!(
                    after,
                    [&bytes[..header], insertion.as_slice(), &bytes[header..]].concat()
                );
            }
            let again = plugin.extract(&dir).unwrap();
            assert!(again.iter().any(|r| r.source == "AUDIT Hello."));
            for (id, source) in untouched {
                assert!(again.iter().any(|r| r.id == id && r.source == source));
            }
        }
    }

    #[test]
    fn vn01_sjis_alias_bytes_and_mode2_tail() {
        // CP932 has duplicate mappings; full-file encode changes these bytes.
        let bytes = [b"Hello.\n;".as_slice(), &[0x87, 0x90], b"\rGoodbye.\r"].concat();
        let mut rows = extract_lines_from_text(
            &decode_ks_bytes(&bytes, "story.ks").unwrap().text,
            "story.ks",
            "story.ks".into(),
        );
        rows[0].translation = Some("AUDIT Hello.".into());
        let refs: Vec<_> = rows.iter().collect();
        let (after, written) = apply_translations(&bytes, "story.ks", &refs)
            .unwrap()
            .unwrap();
        assert_eq!(written, 1);
        assert_eq!(after, [b"AUDIT ".as_slice(), &bytes].concat());
        let dir = tempdir();
        let path = dir.join("story.ks");
        write_mode2_ks(&path, "Hello.\rGoodbye.\n");
        let mut bytes = fs::read(&path).unwrap();
        bytes.extend_from_slice(b"opaque tail");
        fs::write(&path, &bytes).unwrap();
        let plugin = KirikiriPlugin::new();
        let mut rows = plugin.extract(&dir).unwrap();
        rows[0].translation = Some("AUDIT Hello.".into());
        assert_eq!(plugin.inject(&dir, &rows).unwrap().strings_written, 1);
        let after = fs::read(&path).unwrap();
        assert!(after.ends_with(b"opaque tail"));
        assert_eq!(
            decode_ks_bytes(&after, "story.ks").unwrap().text,
            "AUDIT Hello.\rGoodbye.\n"
        );
    }

    #[test]
    fn vn01_known_display_attributes() {
        let text = "[NAME_W  n=\"Asagi\" storage=\"a[0].ks\"]\\\r[name text='Mitsuki' other=\"quoted ] [ brackets\"][p]\n@name text=Unquoted storage=label\r[name text=\"Mi`\"tsuki\"][name text=\"Second\"]Hello.[p]\\\r";
        let mut rows = extract_lines_from_text(text, "story.ks", "story.ks".into());
        for name in ["Asagi", "Mitsuki", "Unquoted", "Mi\"tsuki", "Second"] {
            let row = rows
                .iter_mut()
                .find(|r| r.source == name)
                .expect("known literal attribute");
            row.translation = Some(format!("AUDIT {name}"));
        }
        let refs: Vec<_> = rows.iter().collect();
        let (after, written) = apply_translations(text.as_bytes(), "story.ks", &refs)
            .unwrap()
            .unwrap();
        assert_eq!(written, 5);
        let expected = text
            .replace("Asagi", "AUDIT Asagi")
            .replace("Mitsuki", "AUDIT Mitsuki")
            .replace("Unquoted", "AUDIT` Unquoted")
            .replace("Mi`\"tsuki", "AUDIT Mi`\"tsuki")
            .replace("Second", "AUDIT Second");
        assert_eq!(after, expected.as_bytes());
        let again = extract_lines_from_text(&expected, "story.ks", "story.ks".into());
        for name in ["Asagi", "Mitsuki", "Unquoted", "Mi\"tsuki", "Second"] {
            assert!(again.iter().any(|r| r.source == format!("AUDIT {name}")));
        }
        let arbitrary = "[macro name=private][jump storage=id text=private][name text=\"%n\"][NAME_W n=&f.name][name text=one text=two]\n";
        assert!(extract_lines_from_text(arbitrary, "story.ks", "story.ks".into()).is_empty());
    }

    #[test]
    fn vn01_stale_ambiguous_and_protected_rows() {
        let bytes = b"Hello.[p]\\\nGoodbye.\n";
        let mut row = extract_lines_from_text(
            std::str::from_utf8(bytes).unwrap(),
            "story.ks",
            "story.ks".into(),
        )
        .remove(0);
        row.translation = Some("Hola.".into());
        assert!(
            apply_translations(bytes, "story.ks", &[&row]).is_err(),
            "missing tag and continuation must fail"
        );
        row.translation = Some("Hola.[p]\\".into());
        row.source = "stale source".into();
        assert!(apply_translations(bytes, "story.ks", &[&row]).is_err());
        row.source = "Hello.[p]\\".into();
        assert!(apply_translations(bytes, "story.ks", &[&row, &row]).is_err());
        // An old LF-row locator remains valid after source validation.
        row.id = "story.ks#1".into();
        let (after, _) = apply_translations(bytes, "story.ks", &[&row])
            .unwrap()
            .unwrap();
        assert_eq!(after, b"Hola.[p]\\\nGoodbye.\n");
        row.id = "story.ks#2".into();
        assert!(apply_translations(bytes, "story.ks", &[&row]).is_err());
        row.id = "story.ks#1".into();
        assert!(apply_translations(b"Hello.[p]\\\rGoodbye.\r", "story.ks", &[&row]).is_err());
        row.id = "other.ks#1".into();
        assert!(apply_translations(bytes, "story.ks", &[&row]).is_err());
    }

    #[test]
    fn vn01_existing_xp3_retention_and_precedence() {
        let dir = tempdir();
        let data =
            kirikiri_xp3::write_xp3(&[("story.ks".into(), b"Old base.\n".to_vec())]).unwrap();
        let patch = kirikiri_xp3::write_xp3(&[
            ("keep.bin".into(), b"UNTOUCHED RESOURCE".to_vec()),
            ("story.ks".into(), b"Old patch.\n".to_vec()),
        ])
        .unwrap();
        let patch2 = kirikiri_xp3::write_xp3(&[
            ("story.ks".into(), b"Hello.\n".to_vec()),
            ("keep2.bin".into(), b"SECOND RESOURCE".to_vec()),
        ])
        .unwrap();
        for (name, bytes) in [
            ("data.xp3", &data),
            ("patch.xp3", &patch),
            ("patch2.xp3", &patch2),
        ] {
            fs::write(dir.join(name), bytes).unwrap();
        }
        let plugin = KirikiriPlugin::new();
        let mut rows = plugin.extract(&dir).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].source, "Hello.");
        rows[0].translation = Some("AUDIT Hello.".into());
        let report = plugin.inject(&dir, &rows).unwrap();
        let retained = Xp3Archive::open(&dir.join("patch.xp3")).unwrap();
        let keep = retained
            .entries
            .iter()
            .find(|e| e.name == "keep.bin")
            .expect("unrelated member survives");
        assert_eq!(retained.read_entry(keep).unwrap(), b"UNTOUCHED RESOURCE");
        let effective = plugin
            .extract(&dir)
            .unwrap()
            .iter()
            .any(|r| r.source == "AUDIT Hello.");
        assert!(
            effective
                || report.strings_written == 0
                    && report.files_modified == 0
                    && report.files_written.is_empty()
                    && !report.warnings.is_empty()
        );
        if !effective {
            for (name, bytes) in [
                ("data.xp3", &data),
                ("patch.xp3", &patch),
                ("patch2.xp3", &patch2),
            ] {
                assert_eq!(
                    fs::read(dir.join(name)).unwrap(),
                    *bytes,
                    "{name} metadata and payload bytes survive refusal"
                );
            }
        }
    }

    #[test]
    fn vn01_encrypted_existing_patch_preserved_with_new_overlay() {
        let dir = tempdir();
        fs::write(
            dir.join("data.xp3"),
            kirikiri_xp3::write_xp3(&[("story.ks".into(), b"Hello.\n".to_vec())]).unwrap(),
        )
        .unwrap();
        let protected =
            kirikiri_xp3::write_xp3_protected("private.bin", b"opaque cipher bytes").unwrap();
        fs::write(dir.join("patch.xp3"), &protected).unwrap();
        let plugin = KirikiriPlugin::new();
        let mut rows = plugin.extract(&dir).unwrap();
        rows[0].translation = Some("Hola.".into());
        let report = plugin.inject(&dir, &rows).unwrap();
        assert_eq!(report.strings_written, 1);
        assert_eq!(report.files_modified, 1);
        assert_eq!(fs::read(dir.join("patch.xp3")).unwrap(), protected);
        assert_eq!(plugin.extract(&dir).unwrap()[0].source, "Hola.");
        assert_eq!(report.files_written, vec![dir.join("patch2.xp3")]);
    }

    fn write_utf16le_ks(path: &Path, text: &str) {
        let mut bytes = vec![0xFF, 0xFE];
        bytes.extend_from_slice(&utf16le_bytes_from_str(text));
        fs::write(path, bytes).unwrap();
    }

    fn write_sjis_ks(path: &Path, text: &str) {
        let (bytes, _, err) = encoding_rs::SHIFT_JIS.encode(text);
        assert!(!err, "fixture must encode as SJIS");
        fs::write(path, bytes.as_ref()).unwrap();
    }

    fn write_mode1_ks(path: &Path, text: &str) {
        let mut body = utf16le_bytes_from_str(text);
        mode1_swap_units(&mut body);
        let mut out = vec![0xFE, 0xFE, 0x01, 0xFF, 0xFE];
        out.extend_from_slice(&body);
        fs::write(path, out).unwrap();
    }

    /// Build a valid mode-2 FE FE file (zlib UTF-16LE, Scrambler layout).
    fn write_mode2_ks(path: &Path, text: &str) {
        let utf16 = utf16le_bytes_from_str(text);
        let compressed = miniz_oxide::deflate::compress_to_vec_zlib(&utf16, 6);
        let mut out = vec![0xFE, 0xFE, 0x02, 0xFF, 0xFE];
        out.extend_from_slice(&(compressed.len() as u64).to_le_bytes());
        out.extend_from_slice(&(utf16.len() as u64).to_le_bytes());
        out.extend_from_slice(&compressed);
        fs::write(path, out).unwrap();
    }

    #[test]
    fn test_mode1_bit_swap_self_inverse() {
        let mut data = utf16le_bytes_from_str("Ab");
        let orig = data.clone();
        mode1_swap_units(&mut data);
        assert_ne!(data, orig);
        mode1_swap_units(&mut data);
        assert_eq!(data, orig);
    }

    #[test]
    fn test_mode0_roundtrip_units() {
        let mut data = utf16le_bytes_from_str("Hello");
        let orig = data.clone();
        mode0_encode_units(&mut data);
        assert_ne!(data, orig);
        mode0_decode_units(&mut data);
        assert_eq!(data, orig);
    }

    #[test]
    fn test_detect_ks_dir() {
        let dir = tempdir();
        write_utf16le_ks(&dir.join("scenario.ks"), sample_script());
        assert!(KirikiriPlugin::new().detect(&dir));
    }

    #[test]
    fn test_detect_xp3_only_still_true() {
        let dir = tempdir();
        fs::write(dir.join("data.xp3"), b"XP3\r\n").unwrap();
        assert!(KirikiriPlugin::new().detect(&dir));
    }

    #[test]
    fn test_detect_non_krkr() {
        let dir = tempdir();
        fs::write(dir.join("readme.txt"), b"nope").unwrap();
        assert!(!KirikiriPlugin::new().detect(&dir));
    }

    #[test]
    fn test_pure_ellipsis_lines_are_non_text() {
        assert!(is_non_text_line("......"));
        assert!(is_non_text_line("\u{3000}.................."));
        assert!(is_non_text_line("…………………………………………"));
        assert!(is_non_text_line("  ...  "));
        // Real dialogue with an ellipsis must stay extractable.
        assert!(is_player_text_line("Hello..."));
        assert!(is_player_text_line("\u{3000}Estoy enamorado―――"));
        assert!(is_player_text_line("【Haruki】"));
    }

    #[test]
    fn test_tjs_brace_and_comment_lines_are_non_text() {
        for s in [
            "{",
            "}",
            "\t}",
            "\t\t{",
            "};",
            "];",
            "\t];  // 配列を初期化",
            "//-----------------------------------------------------",
            "else",
            "\t\t\telse",
            "for(var i=1;i<=10;i++){",
            "for(var i = 0; i < (i_max+1); i++) {",
            "tf.con_vol=[];",
            "sf.masked[i] = 0;",
        ] {
            assert!(is_non_text_line(s), "TJS/brace noise should drop: {s:?}");
        }
        // Dialogue must stay.
        assert!(is_player_text_line("Hello {player}"));
        assert!(is_player_text_line("「Kuh......!」[T_NEXT]\\"));
        assert!(is_player_text_line("\u{3000}And then......[T_NEXT]\\"));
    }

    #[test]
    fn test_kag_tag_lines_with_trailing_backslash_are_non_text() {
        for s in [
            r#"[cm][SYSMENU]\"#,
            r#"[endif]\"#,
            r#"[NAME_W n="Asagi"]\"#,
            r#"[NAME_W n="アサギ"]\"#,
            r#"[s]\"#,
            r#"[eval exp="f.vol=sf.se_vol*10"]\"#,
            // Nested [] inside attribute (first `]` is NOT the tag closer).
            r#"[eval exp="sf.ch_voice_flg[tf.c_vo_num]=1"]\"#,
            r#"[freeimage layer=base page=fore]\"#,
            r#"[cm][SYSMENU]\ "#, // trailing space after \
        ] {
            assert!(
                is_non_text_line(s),
                "pure tag + line-cont should drop: {s:?}"
            );
        }
        // Dialogue with a tag prefix or free text must stay.
        assert!(is_player_text_line(r#"[name] Hello world\"#));
        assert!(is_player_text_line("Hello\\"));
        assert!(is_player_text_line("【Haruki】"));
        // Tag + dialogue on same line (common): keep.
        assert!(is_player_text_line(r#"「…………」[T_NEXT]\"#));
    }

    #[test]
    fn test_extract_utf16le_filters_and_ids() {
        let dir = tempdir();
        write_utf16le_ks(&dir.join("scenario.ks"), sample_script());
        let plugin = KirikiriPlugin::new();
        let entries = plugin.extract(&dir).unwrap();
        let sources: Vec<&str> = entries.iter().map(|e| e.source.as_str()).collect();
        assert!(
            sources.iter().any(|s| s.contains("Hello, world")),
            "missing dialogue: {sources:?}"
        );
        assert!(
            sources.contains(&"This is narration."),
            "missing narration: {sources:?}"
        );
        assert!(
            sources.iter().all(|s| !s.starts_with(';')
                && !s.starts_with('*')
                && !s.starts_with('@')
                && *s != "[wait time=50]"),
            "non-text leaked: {sources:?}"
        );
        assert!(
            entries.iter().any(|e| e.id.contains("scenario.ks#")),
            "ids: {:?}",
            entries.iter().map(|e| &e.id).collect::<Vec<_>>()
        );
        for e in &entries {
            assert!(!e.metadata.contains_key("binary_slot"));
        }
    }

    #[test]
    fn test_inject_roundtrip_utf16le() {
        let dir = tempdir();
        write_utf16le_ks(&dir.join("scenario.ks"), sample_script());
        roundtrip_translate(&dir, "Hola, mundo!");
    }

    #[test]
    fn test_inject_roundtrip_sjis() {
        let dir = tempdir();
        // ASCII-only SJIS is fine and avoids unmappable glyphs in the fixture.
        write_sjis_ks(&dir.join("scenario.ks"), sample_script());
        roundtrip_translate(&dir, "Hola, mundo!");
    }

    #[test]
    fn test_inject_roundtrip_mode1() {
        let dir = tempdir();
        write_mode1_ks(&dir.join("scenario.ks"), sample_script());
        // Confirm cipher still present after inject
        roundtrip_translate(&dir, "Hola, mundo!");
        let bytes = fs::read(dir.join("scenario.ks")).unwrap();
        assert_eq!(&bytes[0..5], &[0xFE, 0xFE, 0x01, 0xFF, 0xFE]);
    }

    #[test]
    fn test_inject_roundtrip_mode2() {
        let dir = tempdir();
        write_mode2_ks(&dir.join("scenario.ks"), sample_script());
        roundtrip_translate(&dir, "Hola, mundo!");
        let bytes = fs::read(dir.join("scenario.ks")).unwrap();
        assert_eq!(&bytes[0..5], &[0xFE, 0xFE, 0x02, 0xFF, 0xFE]);
        // Size fields present and zlib still inflates
        assert!(bytes.len() >= 5 + 16);
        let comp_size = u64::from_le_bytes(bytes[5..13].try_into().unwrap()) as usize;
        let uncomp_size = u64::from_le_bytes(bytes[13..21].try_into().unwrap()) as usize;
        assert_eq!(bytes.len(), 5 + 16 + comp_size);
        let plain =
            miniz_oxide::inflate::decompress_to_vec_zlib(&bytes[21..21 + comp_size]).unwrap();
        assert_eq!(plain.len(), uncomp_size);
    }

    fn roundtrip_translate(dir: &Path, new_text_fragment: &str) {
        let plugin = KirikiriPlugin::new();
        let mut entries = plugin.extract(dir).unwrap();
        assert!(!entries.is_empty());
        for e in &mut entries {
            if e.source.contains("Hello, world") {
                e.translation = Some(format!("[name] {new_text_fragment}"));
            }
        }
        let report = plugin.inject(dir, &entries).unwrap();
        assert!(report.files_modified >= 1, "{report:?}");
        let again = plugin.extract(dir).unwrap();
        assert!(
            again.iter().any(|e| e.source.contains(new_text_fragment)),
            "re-extract missing translation: {:?}",
            again.iter().map(|e| &e.source).collect::<Vec<_>>()
        );
        // Non-text lines must still be excluded
        assert!(again.iter().all(|e| is_player_text_line(&e.source)));
    }

    #[test]
    fn test_mode2_extract_dialogue() {
        let dir = tempdir();
        write_mode2_ks(&dir.join("scenario.ks"), sample_script());
        let plugin = KirikiriPlugin::new();
        let entries = plugin.extract(&dir).unwrap();
        let sources: Vec<&str> = entries.iter().map(|e| e.source.as_str()).collect();
        assert!(
            sources.iter().any(|s| s.contains("Hello, world")),
            "mode-2 missing dialogue: {sources:?}"
        );
        assert!(
            sources.contains(&"This is narration."),
            "mode-2 missing narration: {sources:?}"
        );
        assert!(
            sources
                .iter()
                .all(|s| !s.starts_with(';') && !s.starts_with('*') && !s.starts_with('@')),
            "mode-2 non-text leaked: {sources:?}"
        );
    }

    #[test]
    fn test_mode2_bad_zlib_errors_naming_file() {
        let dir = tempdir();
        let path = dir.join("bad.ks");
        // Valid header + sizes claiming a short zlib blob of garbage
        let mut bytes = vec![0xFE, 0xFE, 0x02, 0xFF, 0xFE];
        let garbage = [0x00u8, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07];
        bytes.extend_from_slice(&(garbage.len() as u64).to_le_bytes());
        bytes.extend_from_slice(&100u64.to_le_bytes()); // claimed uncompressed
        bytes.extend_from_slice(&garbage);
        fs::write(&path, &bytes).unwrap();
        let err = KirikiriPlugin::new().extract(&dir).unwrap_err().to_string();
        assert!(
            err.contains("mode-2") || err.contains("zlib") || err.contains("inflate"),
            "expected mode-2 zlib error, got: {err}"
        );
        assert!(
            err.contains("bad.ks") || err.contains("ks"),
            "error should name the file, got: {err}"
        );
    }

    #[test]
    fn test_xp3_malformed_extract_errors_naming_file() {
        let dir = tempdir();
        fs::write(dir.join("data.xp3"), b"XP3\r\n").unwrap();
        let err = KirikiriPlugin::new().extract(&dir).unwrap_err().to_string();
        assert!(
            err.contains("xp3")
                || err.contains("XP3")
                || err.contains("magic")
                || err.contains("parse"),
            "expected XP3 parse error, got: {err}"
        );
    }

    #[test]
    fn test_script_key_rank_and_dedupe_helpers() {
        assert_eq!(
            normalize_script_key("data.xp3/scenario/newgame03.ks"),
            "newgame03.ks"
        );
        assert_eq!(
            normalize_script_key("patch2.xp3/newgame03.ks"),
            "newgame03.ks"
        );
        assert_eq!(
            normalize_script_key("unencrypted/newgame03.ks"),
            "newgame03.ks"
        );
        assert_eq!(
            normalize_script_key("data.xp3/scenario/select/movie.ks"),
            "select/movie.ks"
        );
        assert!(script_source_rank("patch2.xp3/a.ks") > script_source_rank("patch.xp3/a.ks"));
        assert!(script_source_rank("patch.xp3/a.ks") > script_source_rank("data.xp3/a.ks"));
        assert!(script_source_rank("data.xp3/a.ks") > script_source_rank("unencrypted/a.ks"));

        let cands = vec![
            "data.xp3/scenario/a.ks".into(),
            "patch2.xp3/a.ks".into(),
            "unencrypted/a.ks".into(),
            "data.xp3/scenario/only_base.ks".into(),
        ];
        let win = select_best_script_rels(&cands);
        assert!(win.contains("patch2.xp3/a.ks"), "{win:?}");
        assert!(!win.contains("data.xp3/scenario/a.ks"), "{win:?}");
        assert!(!win.contains("unencrypted/a.ks"), "{win:?}");
        assert!(win.contains("data.xp3/scenario/only_base.ks"), "{win:?}");
    }

    #[test]
    fn test_looks_like_readable_ks_rejects_control_soup() {
        assert!(looks_like_readable_ks(
            "; comment\n*start\n[tag]\nHola\r\n\t"
        ));
        assert!(looks_like_readable_ks(""));
        let soup: String = (0..300)
            .map(|i| if i % 3 == 0 { '\u{3}' } else { 'j' })
            .collect();
        assert!(!looks_like_readable_ks(&soup));
        assert!(!looks_like_readable_ks(&"\u{FFFD}".repeat(50)));
    }

    #[test]
    fn test_extract_falls_back_when_higher_rank_source_is_ciphertext() {
        // A cxdec archive outranks the plaintext dump but decodes to control
        // soup. Ranking before decoding used to pick it and drop the only
        // readable copy, leaving the game with almost nothing extracted.
        let dir = tempdir();
        let cipher: String = (0..400)
            .map(|i| if i % 3 == 0 { '\u{3}' } else { 'j' })
            .collect();
        let mut ks_cipher = vec![0xFF, 0xFE];
        ks_cipher.extend_from_slice(&utf16le_bytes_from_str(&cipher));
        let data = crate::kirikiri_xp3::write_xp3(&[("scenario/a.ks".into(), ks_cipher)]).unwrap();
        fs::write(dir.join("data.xp3"), &data).unwrap();
        let unenc = dir.join("unencrypted");
        fs::create_dir_all(&unenc).unwrap();
        write_utf16le_ks(&unenc.join("a.ks"), "; comment\n*start\nHello from UNENC\n");

        let plugin = KirikiriPlugin::new();
        let entries = plugin.extract(&dir).unwrap();
        let sources: Vec<&str> = entries.iter().map(|e| e.source.as_str()).collect();
        assert!(
            sources.iter().any(|s| s.contains("Hello from UNENC")),
            "readable dump must win over ciphertext: {sources:?}"
        );
        assert!(
            !sources.iter().any(|s| s.contains('\u{3}')),
            "ciphertext must not be extracted: {sources:?}"
        );
    }

    #[test]
    fn test_extract_prefers_patch_over_data_and_unencrypted() {
        let dir = tempdir();
        let script = "; comment\n*start\nHello from DATA\n";
        let script_patch = "; comment\n*start\nHello from PATCH2\n";
        let mut ks_data = vec![0xFF, 0xFE];
        ks_data.extend_from_slice(&utf16le_bytes_from_str(script));
        let mut ks_patch = vec![0xFF, 0xFE];
        ks_patch.extend_from_slice(&utf16le_bytes_from_str(script_patch));
        let data = crate::kirikiri_xp3::write_xp3(&[("scenario/a.ks".into(), ks_data)]).unwrap();
        let patch2 = crate::kirikiri_xp3::write_xp3(&[("a.ks".into(), ks_patch)]).unwrap();
        fs::write(dir.join("data.xp3"), &data).unwrap();
        fs::write(dir.join("patch2.xp3"), &patch2).unwrap();
        let unenc = dir.join("unencrypted");
        fs::create_dir_all(&unenc).unwrap();
        write_utf16le_ks(&unenc.join("a.ks"), "; comment\n*start\nHello from UNENC\n");

        let plugin = KirikiriPlugin::new();
        let entries = plugin.extract(&dir).unwrap();
        let sources: Vec<&str> = entries.iter().map(|e| e.source.as_str()).collect();
        assert!(
            sources.iter().any(|s| s.contains("Hello from PATCH2")),
            "prefer patch2: {sources:?}"
        );
        assert!(
            !sources.iter().any(|s| s.contains("Hello from DATA")),
            "drop data copy: {sources:?}"
        );
        assert!(
            !sources.iter().any(|s| s.contains("Hello from UNENC")),
            "drop unencrypted copy: {sources:?}"
        );
        assert!(
            entries.iter().all(|e| e.id.starts_with("patch2.xp3/")
                || e.file_path.to_string_lossy().contains("patch2")),
            "ids/paths should be patch2: {:?}",
            entries.iter().map(|e| &e.id).collect::<Vec<_>>()
        );
    }

    #[test]
    fn test_xp3_e2e_extract_and_patch_inject() {
        let dir = tempdir();
        // Build UTF-16LE .ks payload and pack into data.xp3
        let mut ks = vec![0xFF, 0xFE];
        ks.extend_from_slice(&utf16le_bytes_from_str(sample_script()));
        let arch = crate::kirikiri_xp3::write_xp3(&[("scenario/first.ks".into(), ks)]).unwrap();
        fs::write(dir.join("data.xp3"), &arch).unwrap();

        let plugin = KirikiriPlugin::new();
        assert!(plugin.detect(&dir));
        let mut entries = plugin.extract(&dir).unwrap();
        assert!(
            entries.iter().any(|e| e.source.contains("Hello, world")),
            "missing dialogue from XP3: {:?}",
            entries.iter().map(|e| &e.source).collect::<Vec<_>>()
        );
        assert!(
            entries
                .iter()
                .any(|e| e.id.starts_with("data.xp3/scenario/first.ks#")),
            "ids: {:?}",
            entries.iter().map(|e| &e.id).collect::<Vec<_>>()
        );

        for e in &mut entries {
            if e.source.contains("Hello, world") {
                e.translation = Some("[name] Hola, mundo!".into());
            }
        }
        let report = plugin.inject(&dir, &entries).unwrap();
        assert!(report.files_modified >= 1, "{report:?}");
        let patch = dir.join("patch.xp3");
        assert!(
            patch.is_file(),
            "expected patch.xp3, written: {:?}",
            report.files_written
        );

        // Re-read patch and confirm translation payload
        let patch_arch = Xp3Archive::open(&patch).unwrap();
        let e = patch_arch
            .ks_entries()
            .next()
            .expect("patch should contain .ks");
        let payload = patch_arch.read_entry(e).unwrap();
        let decoded = decode_ks_bytes(&payload, "patch").unwrap();
        assert!(
            decoded.text.contains("Hola, mundo"),
            "patch missing translation: {}",
            decoded.text
        );
    }

    #[test]
    fn test_xp3_garbage_ks_skipped_not_crash() {
        let dir = tempdir();
        // Mode-2 cipher header: existing decoder rejects (not plausible text / unsupported).
        let garbage = vec![0xFE, 0xFE, 0x02, 0xFF, 0xFE, 0x00, 0x00];
        let arch = crate::kirikiri_xp3::write_xp3(&[("foo.ks".into(), garbage)]).unwrap();
        fs::write(dir.join("data.xp3"), arch).unwrap();
        let plugin = KirikiriPlugin::new();
        // Must not panic; skip with warn → empty Ok or soft error
        let result = plugin.extract(&dir);
        match result {
            Ok(entries) => {
                assert!(
                    entries.is_empty(),
                    "undecodable .ks should not yield dialogue: {:?}",
                    entries.iter().map(|e| &e.source).collect::<Vec<_>>()
                );
            }
            Err(e) => {
                let s = e.to_string();
                assert!(!s.contains("panic"), "{s}");
            }
        }
    }

    #[test]
    fn xp3_lookup_is_linear() {
        const FILLER: usize = 400;
        const SCRIPTS: usize = 100;
        let dir = tempdir();
        let mut files: Vec<(String, Vec<u8>)> = Vec::with_capacity(FILLER + SCRIPTS);
        for i in 0..FILLER {
            files.push((format!("img/{i:04}.png"), vec![0, 1, 2, 3]));
        }
        let ks_utf16 = |text: &str| {
            let mut bytes = vec![0xFF, 0xFE];
            bytes.extend_from_slice(&utf16le_bytes_from_str(text));
            bytes
        };
        // Scripts sit at the end. The last entry repeats the first script name
        // with different text; lookup must keep the earlier payload.
        for i in 0..SCRIPTS - 1 {
            let text = format!("; c\nHello SCRIPT{i:03}\n");
            files.push((format!("scenario/s{i:03}.ks"), ks_utf16(&text)));
        }
        files.push((
            "scenario/s000.ks".into(),
            ks_utf16("; c\nHello DUP_SECOND\n"),
        ));
        let n_entries = files.len();
        let n_scripts = files
            .iter()
            .filter(|(name, _)| name.ends_with(".ks"))
            .count();
        assert_eq!(n_entries, FILLER + SCRIPTS);
        assert_eq!(n_scripts, SCRIPTS);

        let arch = crate::kirikiri_xp3::write_xp3(&files).unwrap();
        let archive_path = dir.join("game.xp3");
        fs::write(&archive_path, arch).unwrap();
        assert_eq!(
            Xp3Archive::open(&archive_path).unwrap().entries.len(),
            n_entries
        );

        let plugin = KirikiriPlugin::new();
        XP3_NAME_INSPECTIONS.with(|c| c.set(Some(0)));
        let mut entries = plugin.extract(&dir).unwrap();

        let mut expected: Vec<String> = (0..SCRIPTS - 1)
            .map(|i| format!("Hello SCRIPT{i:03}"))
            .collect();
        // Both candidates for the duplicated name resolve to the first entry.
        expected.push("Hello SCRIPT000".into());
        expected.sort();
        let mut got: Vec<String> = entries.iter().map(|e| e.source.clone()).collect();
        got.sort();
        assert_eq!(
            got, expected,
            "duplicate name must resolve to the first entry"
        );
        assert!(entries.iter().all(|e| !e.source.contains("DUP_SECOND")));
        assert!(entries.iter().any(|e| {
            e.id == "game.xp3/scenario/s000.ks#kag:2" && e.source == "Hello SCRIPT000"
        }));
        assert!(entries.iter().any(|e| {
            e.id == "game.xp3/scenario/s098.ks#kag:2" && e.source == "Hello SCRIPT098"
        }));

        let target = entries
            .iter_mut()
            .find(|e| e.source == "Hello SCRIPT098")
            .unwrap();
        target.translation = Some("Hola SCRIPT098".into());
        let report = plugin.inject(&dir, &entries).unwrap();
        assert!(
            report.warnings.iter().all(|w| !w.contains("not found")),
            "{:?}",
            report.warnings
        );
        assert!(report.strings_written >= 1, "{report:?}");

        let inspections = XP3_NAME_INSPECTIONS.with(|c| c.replace(None)).unwrap();
        assert!(
            inspections >= n_entries,
            "name inspections {inspections} never indexed {n_entries} archive entries"
        );
        assert!(
            inspections <= n_entries + n_scripts,
            "name inspections {inspections} exceeded {n_entries} entries + {n_scripts} scripts"
        );
    }

    #[test]
    fn detect_xp3_skips_ks_walk() {
        let xp3_dir = tempdir();
        fs::write(xp3_dir.join("game.xp3"), b"XP3\r\n").unwrap();
        fs::create_dir_all(xp3_dir.join("nested").join("deeper")).unwrap();
        fs::write(xp3_dir.join("nested").join("deeper").join("note.txt"), b"x").unwrap();

        FIND_KS_FILES_CALLS.with(|c| c.set(Some(0)));
        assert!(KirikiriPlugin::new().detect(&xp3_dir));
        let walks = FIND_KS_FILES_CALLS.with(|c| c.replace(None));
        assert_eq!(
            walks,
            Some(0),
            "a top-level xp3 must not walk the tree for .ks files"
        );

        let ks_dir = tempdir();
        write_utf16le_ks(&ks_dir.join("scenario.ks"), sample_script());
        FIND_KS_FILES_CALLS.with(|c| c.set(Some(0)));
        assert!(KirikiriPlugin::new().detect(&ks_dir));
        let walks = FIND_KS_FILES_CALLS.with(|c| c.replace(None));
        assert!(
            walks.is_some_and(|n| n >= 1),
            "a .ks-only directory still needs find_ks_files, got {walks:?}"
        );
    }

    #[test]
    fn test_stability_experimental() {
        assert_eq!(
            KirikiriPlugin::new().stability(),
            locust_core::extraction::FormatStability::Experimental
        );
    }
}
