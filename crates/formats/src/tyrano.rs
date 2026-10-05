//! TyranoBuilder / TyranoScript scenario plugin — Experimental (synthetic fixtures).
//!
//! # Spec sources (do not invent transforms)
//! - Scenario parser (`parseScenario`, `makeTag`):
//!   https://raw.githubusercontent.com/ShikemokuMK/tyranoscript/master/tyrano/plugins/kag/kag.parser.js
//!   Line classes (after trim): `;` comment, `/*`/`*/` block comment (whole-line only),
//!   `#name` / `#name:face` → chara_ptext (speaker), `*label` / `*label|val` labels,
//!   `@tag …` full-line commands, else character-scan for inline `[tag]` + player text.
//! - Game layout (TyranoBuilder shipping tree; not re-fetched here):
//!   `data/scenario/*.ks` UTF-8 scenario scripts; engine assets under `tyrano/`.
//!   Desktop Electron packs use `app.asar` (see [`crate::tyrano_asar`]); inject rebuilds
//!   the asar in place with an exclusively owned backup and staged replacement.
//!   NW.js desktop packs use `package.nw` or a self-extracting `*.exe` with an
//!   appended ZIP (see [`crate::tyrano_nw`]).
//!
//! # Payload extraction
//! - Track comments and script/HTML bodies; scan quoted and nested tag brackets.
//! - Ordinary rich dialogue keeps protected inline controls. Known display
//!   attributes (`glink.text`, `chara_new.jname`, `ruby.text`) have separate
//!   value locators; text around those tags has independent span locators.
//! - `#name` bare ASCII identifiers are **not** emitted; non-identifier display names
//!   (e.g. `#表示名`) are emitted with tag `"speaker"` (name segment only; face after
//!   `:` is preserved on inject).
//! - Encoding: UTF-8 only; preserve BOM if the source file had one.
//!
//! Out of scope: `Config.tjs` string tables, dynamic parameter expressions,
//! JS/HTML translation, and real commercial game fixtures.

use std::collections::HashMap;
use std::ops::Range;
use std::path::{Path, PathBuf};

use crate::archive_replace::{guard_target, note_backups, replace_files};
use locust_core::backup::RevisionOriginal;
use locust_core::error::{LocustError, Result};
use locust_core::extraction::{FormatPlugin, InjectionReport};
use locust_core::models::{OutputMode, StringEntry};
use locust_core::patch::GameLock;
use tracing::warn;

use crate::tyrano_asar::{self, AsarArchive};
use crate::tyrano_nw::{self, NwArchive};

const UTF8_BOM: &[u8] = &[0xEF, 0xBB, 0xBF];

pub struct TyranoPlugin;

impl TyranoPlugin {
    pub fn new() -> Self {
        Self
    }

    fn is_ks(path: &Path) -> bool {
        path.extension()
            .and_then(|e| e.to_str())
            .map(|e| e.eq_ignore_ascii_case("ks"))
            .unwrap_or(false)
    }

    fn root_dir(path: &Path) -> PathBuf {
        if path.is_dir() {
            path.to_path_buf()
        } else {
            path.parent().unwrap_or(path).to_path_buf()
        }
    }

    /// Tyrano-like tree: `tyrano/` engine folder and/or `data/scenario/` layout.
    fn looks_like_tyrano(root: &Path) -> bool {
        root.join("tyrano").is_dir() || root.join("data").join("scenario").is_dir()
    }

    fn scenario_dir(root: &Path) -> PathBuf {
        root.join("data").join("scenario")
    }

    /// Collect loose `.ks` under `data/scenario/` (preferred), else any `.ks` when
    /// a `tyrano/` engine folder marks the tree as Tyrano.
    fn find_scenario_ks(root: &Path) -> Vec<PathBuf> {
        let mut out = Vec::new();
        let scenario = Self::scenario_dir(root);
        if scenario.is_dir() {
            collect_ks_under(&scenario, &mut out);
            return out;
        }
        if root.join("tyrano").is_dir() {
            // Engine present but no standard scenario dir — still search for loose .ks.
            collect_ks_under(root, &mut out);
            // Avoid pulling engine samples under tyrano/ if any; keep only non-tyrano paths.
            out.retain(|p| {
                p.strip_prefix(root)
                    .map(|rel| {
                        let s = rel.to_string_lossy().replace('\\', "/");
                        !s.starts_with("tyrano/")
                    })
                    .unwrap_or(true)
            });
        }
        out
    }

    /// `app.asar` at game root or under `resources/`.
    fn find_app_asars(root: &Path) -> Vec<PathBuf> {
        let mut out = Vec::new();
        if root.is_file() {
            if is_app_asar(root) {
                out.push(root.to_path_buf());
            }
            return out;
        }
        if !root.is_dir() {
            return out;
        }
        for candidate in [
            root.join("app.asar"),
            root.join("resources").join("app.asar"),
        ] {
            if candidate.is_file() {
                out.push(candidate);
            }
        }
        out
    }

    fn has_app_asar(root: &Path) -> bool {
        !Self::find_app_asars(root).is_empty()
    }

    fn find_nw_containers(root: &Path) -> Vec<PathBuf> {
        tyrano_nw::find_nw_containers(root)
    }

    fn has_nw_container(root: &Path) -> bool {
        !Self::find_nw_containers(root).is_empty()
    }

    fn detect_path(path: &Path) -> bool {
        if path.is_file() {
            if is_app_asar(path) {
                return true;
            }
            if tyrano_nw::is_package_nw_name(path) && tyrano_nw::probe_eocd_present(path) {
                return true;
            }
            if tyrano_nw::is_exe_name(path) && tyrano_nw::probe_scenario_in_zip_tail(path) {
                return true;
            }
            // Single .ks only if it lives under …/data/scenario/
            if !Self::is_ks(path) {
                return false;
            }
            let parent = path.parent().unwrap_or(path);
            let is_scenario = parent
                .file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.eq_ignore_ascii_case("scenario"))
                .unwrap_or(false);
            if !is_scenario {
                return false;
            }
            // parent is scenario; grandparent should be data
            return parent
                .parent()
                .and_then(|p| p.file_name())
                .and_then(|n| n.to_str())
                .map(|n| n.eq_ignore_ascii_case("data"))
                .unwrap_or(false);
        }
        if !path.is_dir() {
            return false;
        }
        // Prefer positive scenario .ks; also claim empty Tyrano trees so extract can
        // error loudly (and so KiriKiri never steals a bare tyrano/ shell).
        if !Self::find_scenario_ks(path).is_empty() {
            return true;
        }
        if Self::looks_like_tyrano(path) {
            return true;
        }
        // Electron pack: app.asar with scenario paths (and optional tyrano marker).
        if Self::has_app_asar(path) {
            for p in Self::find_app_asars(path) {
                if AsarArchive::peek_header_mentions_scenario(&p) {
                    return true;
                }
            }
            // Asar present + no header peek: still claim if tyrano/ exists beside it
            // (common desktop layout: resources/app.asar + resources/app.asar.unpacked/tyrano).
            if path.join("tyrano").is_dir()
                || path
                    .join("resources")
                    .join("app.asar.unpacked")
                    .join("tyrano")
                    .is_dir()
            {
                return true;
            }
        }
        // NW.js: package.nw or top-level exe with appended scenario ZIP.
        if Self::has_nw_container(path) {
            return true;
        }
        false
    }
}

fn is_app_asar(path: &Path) -> bool {
    path.file_name()
        .and_then(|n| n.to_str())
        .map(|n| n.eq_ignore_ascii_case("app.asar"))
        .unwrap_or(false)
}

impl Default for TyranoPlugin {
    fn default() -> Self {
        Self::new()
    }
}

fn collect_ks_under(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in walkdir::WalkDir::new(dir)
        .follow_links(false)
        .into_iter()
        .filter_entry(crate::discovery::is_game_entry)
        .filter_map(|e| e.ok())
    {
        let p = entry.path();
        if p.is_file()
            && p.extension()
                .and_then(|e| e.to_str())
                .map(|e| e.eq_ignore_ascii_case("ks"))
                .unwrap_or(false)
        {
            out.push(p.to_path_buf());
        }
    }
    out.sort();
}

fn parse_err(file: &str, message: impl Into<String>) -> LocustError {
    LocustError::ParseError {
        file: file.into(),
        message: message.into(),
    }
}

// ─── Decode / encode (UTF-8, optional BOM) ─────────────────────────────────

#[derive(Clone, Debug)]
struct DecodedKs {
    text: String,
    had_bom: bool,
}

fn decode_ks_utf8(bytes: &[u8], file_label: &str) -> Result<DecodedKs> {
    let (had_bom, body) = if bytes.starts_with(UTF8_BOM) {
        (true, &bytes[UTF8_BOM.len()..])
    } else {
        (false, bytes)
    };
    let text = std::str::from_utf8(body).map_err(|_| {
        parse_err(
            file_label,
            "scenario .ks is not valid UTF-8 (TyranoBuilder ships UTF-8; re-export or unpack first)",
        )
    })?;
    Ok(DecodedKs {
        text: text.to_string(),
        had_bom,
    })
}

fn encode_ks_utf8(decoded: &DecodedKs) -> Vec<u8> {
    let mut out = Vec::with_capacity(decoded.text.len() + 3);
    if decoded.had_bom {
        out.extend_from_slice(UTF8_BOM);
    }
    out.extend_from_slice(decoded.text.as_bytes());
    out
}

// ─── Line classification (kag.parser.js parseScenario) ─────────────────────

/// Bare Tyrano chara id: ASCII identifier used as `#akane` / `#akane:happy`.
fn is_bare_identifier(name: &str) -> bool {
    if name.is_empty() {
        return false;
    }
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !(first.is_ascii_alphabetic() || first == '_') {
        return false;
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// `#name` / `#name:face` → display name to extract, or None if bare id / empty.
fn speaker_display_name(line: &str) -> Option<&str> {
    let t = line.trim();
    if !t.starts_with('#') {
        return None;
    }
    let rest = t[1..].trim();
    if rest.is_empty() {
        return None;
    }
    let name = rest.split(':').next().unwrap_or("").trim();
    if name.is_empty() || is_bare_identifier(name) {
        return None;
    }
    Some(name)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LineKind {
    Skip,
    Text,
    Speaker,
}

#[derive(Debug)]
struct KsTag<'a> {
    range: Range<usize>,
    name: &'a str,
    body: Range<usize>,
}

/// parseScenario counts nested brackets and all three quote styles. Its tag
/// scanner does NOT treat backslashes inside quotes as quote escapes.
fn tag_end(line: &str, start: usize) -> Option<usize> {
    let mut depth = 0;
    let mut quote = None;
    for (offset, c) in line[start..].char_indices() {
        match c {
            '\'' | '"' | '`' if quote == Some(c) => quote = None,
            '\'' | '"' | '`' if quote.is_none() => quote = Some(c),
            '[' if quote.is_none() => depth += 1,
            ']' if quote.is_none() => {
                depth -= 1;
                if depth == 0 {
                    return Some(start + offset + 1);
                }
            }
            _ => {}
        }
    }
    None
}

fn ks_tag(line: &str, range: Range<usize>, body: Range<usize>) -> KsTag<'_> {
    // makeTag delimits the tag name with a literal space (not arbitrary WS).
    let name = line[body.clone()]
        .trim_start_matches(' ')
        .split(' ')
        .next()
        .unwrap_or("");
    KsTag { range, name, body }
}

#[derive(Default)]
struct ScanState {
    comment: bool,
    script: bool,
    html: bool,
}

struct KsLine<'a> {
    kind: LineKind,
    tags: Vec<KsTag<'a>>,
    text: Vec<Range<usize>>,
    split: bool,
}

fn scan_line<'a>(line: &'a str, state: &mut ScanState) -> KsLine<'a> {
    let t = line.trim();
    let start = line.len() - line.trim_start().len();
    let end = line.trim_end().len();
    let mut out = KsLine {
        kind: LineKind::Skip,
        tags: Vec::new(),
        text: Vec::new(),
        split: false,
    };
    // This deliberately mirrors the runtime's substring reset, before comments
    // or line-prefix dispatch, rather than inventing a JavaScript parser.
    if t.contains("endscript") {
        state.script = false;
    }
    if state.comment {
        if t == "*/" {
            state.comment = false;
        }
        return out;
    }
    if t == "/*" {
        state.comment = true;
        return out;
    }
    if t.is_empty() || t.starts_with(';') || t.starts_with('*') || t == "*/" {
        return out;
    }
    if t.starts_with('#') {
        if !state.script && !state.html && speaker_display_name(line).is_some() {
            out.kind = LineKind::Speaker;
        }
        return out;
    }
    if t.starts_with('@') {
        let tag = ks_tag(line, start..end, start + 1..end);
        match tag.name {
            "iscript" => state.script = true,
            "endscript" => state.script = false,
            "html" => state.html = true,
            "endhtml" => state.html = false,
            _ if !state.script && !state.html => out.tags.push(tag),
            _ => {}
        }
        return out;
    }
    if state.script {
        return out;
    }
    let mut pos = start;
    let mut text_start = start;
    while pos < end {
        match line.as_bytes()[pos] {
            b'\\' => {
                pos += 1;
                if pos < end {
                    pos += line[pos..].chars().next().unwrap().len_utf8();
                }
            }
            b'[' => {
                if !state.html && text_start < pos {
                    out.text.push(text_start..pos);
                }
                let Some(close) = tag_end(line, pos) else {
                    // The runtime compensates malformed tags; they are never
                    // safe translation targets. Keep only earlier free text.
                    out.split = true;
                    break;
                };
                let tag = ks_tag(line, pos..close, pos + 1..close - 1);
                match tag.name {
                    "iscript" if !state.html => {
                        state.script = true;
                        out.split = true;
                        break;
                    }
                    "html" => {
                        state.html = true;
                        out.split = true;
                    }
                    "endhtml" => {
                        state.html = false;
                        out.split = true;
                    }
                    _ if !state.html => out.tags.push(tag),
                    _ => {}
                }
                pos = close;
                text_start = pos;
            }
            _ => pos += line[pos..].chars().next().unwrap().len_utf8(),
        }
    }
    if pos >= end && !state.html && text_start < end {
        out.text.push(text_start..end);
    }
    out.text.retain(|r| !line[r.clone()].trim().is_empty());
    if !out.text.is_empty() {
        out.kind = LineKind::Text;
    }
    out
}

#[cfg(test)]
fn classify_lines(text: &str) -> Vec<LineKind> {
    let mut state = ScanState::default();
    text.split('\n')
        .map(|line| scan_line(line, &mut state).kind)
        .collect()
}

fn speaker_range(line: &str) -> Option<Range<usize>> {
    let t = line.trim_start().strip_prefix('#')?;
    let name = t.split(':').next()?.trim();
    if name.is_empty() {
        return None;
    }
    let start = name.as_ptr() as usize - line.as_ptr() as usize;
    Some(start..start + name.len())
}

#[cfg(test)]
fn rebuild_speaker_line(original: &str, new_name: &str) -> String {
    let mut out = original.to_string();
    out.replace_range(speaker_range(original).unwrap(), new_name);
    out
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum SlotKind {
    Dialogue,
    Text,
    Speaker,
    Attribute(Option<char>),
}

#[derive(Debug)]
struct KsSlot {
    locator: String,
    range: Range<usize>,
    kind: SlotKind,
    // Every byte outside the selected values on this line, including controls,
    // spacing, targets, storage and quotes. Independent of value lengths.
    guard: Vec<String>,
}

/// Only these runtime handlers expose the named parameters as display text.
/// Dynamic entities, escapes, duplicate parameters and malformed values are
/// deliberately not writable literal slots.
fn display_attribute(
    line: &str,
    tag: &KsTag<'_>,
) -> Option<(Range<usize>, Option<char>, &'static str)> {
    let key = match tag.name {
        "glink" | "ruby" => "text",
        "chara_new" => "jname",
        _ => return None,
    };
    let bytes = line.as_bytes();
    let mut pos = tag.body.start;
    while pos < tag.body.end && bytes[pos] == b' ' {
        pos += 1;
    }
    pos += tag.name.len();
    let mut found = None;
    while pos < tag.body.end {
        while pos < tag.body.end && bytes[pos] == b' ' {
            pos += 1;
        }
        let begin = pos;
        while pos < tag.body.end && !matches!(bytes[pos], b' ' | b'=') {
            pos += 1;
        }
        let name = &line[begin..pos];
        while pos < tag.body.end && bytes[pos] == b' ' {
            pos += 1;
        }
        if pos == tag.body.end {
            break;
        }
        if bytes[pos] != b'=' {
            continue;
        }
        pos += 1;
        while pos < tag.body.end && bytes[pos] == b' ' {
            pos += 1;
        }
        if pos == tag.body.end {
            break;
        }
        let quote = match bytes[pos] {
            b'\'' | b'"' | b'`' => {
                let q = bytes[pos] as char;
                pos += 1;
                Some(q)
            }
            _ => None,
        };
        let begin = pos;
        let stop = quote.unwrap_or(' ') as u8;
        while pos < tag.body.end && bytes[pos] != stop {
            pos += 1;
        }
        if quote.is_some() && pos == tag.body.end {
            return None;
        }
        let range = begin..pos;
        if name == key {
            if found.is_some() {
                return None;
            }
            let value = &line[range.clone()];
            if !literal_attribute(value) {
                return None;
            }
            found = Some((range, quote, key));
        }
        if quote.is_some() {
            pos += 1;
        }
    }
    found
}

fn literal_attribute(value: &str) -> bool {
    let value = value.trim();
    !value.is_empty()
        && !value.starts_with(['&', '%'])
        && value != "undefined"
        && !value.contains(['\\', '\r', '\n', '\t'])
}

fn scenario_slots(text: &str) -> Vec<KsSlot> {
    let mut state = ScanState::default();
    let mut slots = Vec::new();
    let mut offset = 0;
    for (idx, physical) in text.split_inclusive('\n').enumerate() {
        let line = physical.strip_suffix('\n').unwrap_or(physical);
        let line = line.strip_suffix('\r').unwrap_or(line);
        let scan = scan_line(line, &mut state);
        let line_no = idx + 1;
        let first_slot = slots.len();
        if scan.kind == LineKind::Speaker {
            slots.push(KsSlot {
                locator: line_no.to_string(),
                range: speaker_range(line).unwrap(),
                kind: SlotKind::Speaker,
                guard: Vec::new(),
            });
        } else {
            let mut attributes = Vec::new();
            for (tag_no, tag) in scan.tags.iter().enumerate() {
                if let Some((range, quote, name)) = display_attribute(line, tag) {
                    attributes.push(KsSlot {
                        locator: format!("{line_no}:attr:{tag_no}:{name}"),
                        range,
                        kind: SlotKind::Attribute(quote),
                        guard: Vec::new(),
                    });
                }
            }
            if scan.kind == LineKind::Text {
                if attributes.is_empty() && !scan.split {
                    slots.push(KsSlot {
                        locator: line_no.to_string(),
                        range: 0..line.len(),
                        kind: SlotKind::Dialogue,
                        guard: Vec::new(),
                    });
                } else {
                    for (part, range) in scan.text.into_iter().enumerate() {
                        slots.push(KsSlot {
                            locator: format!("{line_no}:text:{part}"),
                            range,
                            kind: SlotKind::Text,
                            guard: Vec::new(),
                        });
                    }
                }
            }
            slots.extend(attributes);
        }
        let line_slots = &mut slots[first_slot..];
        line_slots.sort_by_key(|s| s.range.start);
        let mut guard = Vec::new();
        let mut end = 0;
        for slot in line_slots.iter() {
            guard.push(line[end..slot.range.start].to_string());
            end = slot.range.end;
        }
        guard.push(line[end..].to_string());
        for slot in line_slots {
            slot.range.start += offset;
            slot.range.end += offset;
            slot.guard = guard.clone();
        }
        offset += physical.len();
    }
    slots
}

fn entries_from_ks_bytes(bytes: &[u8], rel: &str, file_path: PathBuf) -> Result<Vec<StringEntry>> {
    let decoded = decode_ks_utf8(bytes, rel)?;
    Ok(scenario_slots(&decoded.text)
        .into_iter()
        .map(|slot| {
            let mut entry = StringEntry::new(
                format!("{rel}#{}", slot.locator),
                &decoded.text[slot.range],
                file_path.clone(),
            );
            entry.tags.push(
                match slot.kind {
                    SlotKind::Speaker => "speaker",
                    SlotKind::Attribute(_) => "display_attribute",
                    _ => "dialogue",
                }
                .into(),
            );
            entry
                .metadata
                .insert("tyrano_guard".into(), serde_json::json!(slot.guard));
            entry
        })
        .collect())
}

fn slot_matches(entry: &StringEntry, slot: &KsSlot, text: &str) -> bool {
    entry.source == text[slot.range.clone()]
        && entry.metadata.get("tyrano_guard").is_none_or(|guard| *guard == serde_json::json!(slot.guard))
        // Legacy whole-line rows are supported only where the parser still
        // finds one unambiguous whole-line dialogue or literal speaker.
        && (entry.metadata.contains_key("tyrano_guard") || !slot.locator.contains(':'))
}

/// True when `new` differs from `old` only inside slot values, plus quotes a
/// previous injection wrapped around bare attribute values.
fn revision_aligned(old: &str, old_slots: &[KsSlot], new: &str, new_slots: &[KsSlot]) -> bool {
    if old_slots.len() != new_slots.len() {
        return false;
    }
    let (mut old_end, mut new_end) = (0, 0);
    let mut opened: Option<char> = None;
    for (a, b) in old_slots.iter().zip(new_slots) {
        let added = match (&a.kind, &b.kind) {
            (SlotKind::Attribute(None), SlotKind::Attribute(Some(q))) => Some(*q),
            (x, y) if x == y => None,
            _ => return false,
        };
        let mut between = &new[new_end..b.range.start];
        if let Some(q) = opened.take() {
            let Some(rest) = between.strip_prefix(q) else {
                return false;
            };
            between = rest;
        }
        if let Some(q) = added {
            let Some(rest) = between.strip_suffix(q) else {
                return false;
            };
            between = rest;
        }
        if a.locator != b.locator
            || old[old_end..a.range.start] != *between
            || (a.kind == SlotKind::Dialogue
                && !preserved_tags(&old[a.range.clone()], &new[b.range.clone()]))
        {
            return false;
        }
        opened = added;
        old_end = a.range.end;
        new_end = b.range.end;
    }
    let mut tail = &new[new_end..];
    if let Some(q) = opened {
        let Some(rest) = tail.strip_prefix(q) else {
            return false;
        };
        tail = rest;
    }
    old[old_end..] == *tail
}

fn prepare_ks_revision<'a>(
    original: &[u8],
    current: &[u8],
    label: &str,
    entries: impl Iterator<Item = &'a mut StringEntry>,
) -> Result<()> {
    let old = decode_ks_utf8(original, label)?;
    let new = decode_ks_utf8(current, label)?;
    let old_slots = scenario_slots(&old.text);
    let new_slots = scenario_slots(&new.text);
    if old.had_bom != new.had_bom || !revision_aligned(&old.text, &old_slots, &new.text, &new_slots)
    {
        return Ok(());
    }
    let index: HashMap<_, _> = old_slots
        .iter()
        .zip(&new_slots)
        .map(|(a, b)| (a.locator.as_str(), (a, b)))
        .collect();
    for entry in entries {
        let Some((_, locator)) = entry.id.rsplit_once('#') else {
            continue;
        };
        let Some((a, b)) = index.get(locator) else {
            continue;
        };
        if !slot_matches(entry, b, &new.text) && slot_matches(entry, a, &old.text) {
            entry.source = new.text[b.range.clone()].to_string();
            // Bare attributes may have gained quotes. Alignment proved that
            // nothing else changed in the guards or between scenario slots.
            if entry.metadata.contains_key("tyrano_guard") {
                entry
                    .metadata
                    .insert("tyrano_guard".into(), serde_json::json!(b.guard));
            }
        }
    }
    Ok(())
}

fn preserved_tags(source: &str, translation: &str) -> bool {
    let old = scan_line(source, &mut ScanState::default());
    let new = scan_line(translation, &mut ScanState::default());
    !new.split
        && old
            .tags
            .iter()
            .map(|t| &source[t.range.clone()])
            .eq(new.tags.iter().map(|t| &translation[t.range.clone()]))
}

/// The quote an unquoted attribute value must gain so the engine reads the
/// whole translation as one value: `Ok(None)` when it can stay bare, `Err`
/// when no quote style can hold it. Backticks are avoided because engines
/// older than V515 do not parse them; `"`/`'` keep inner spaces under the
/// default `KeepSpaceInParameterValue = 2`.
fn bare_attribute_quote(translation: &str) -> std::result::Result<Option<char>, ()> {
    if !translation
        .chars()
        .any(|c| c.is_whitespace() || matches!(c, '[' | ']' | '\'' | '"' | '`' | '='))
    {
        return Ok(None);
    }
    ['"', '\'']
        .into_iter()
        .find(|q| !translation.contains(*q))
        .map(Some)
        .ok_or(())
}

fn safe_replacement(slot: &KsSlot, source: &str, translation: &str) -> bool {
    if translation.is_empty() || translation.contains(['\r', '\n', '\0']) {
        return false;
    }
    match slot.kind {
        SlotKind::Attribute(quote) => {
            literal_attribute(translation)
                && match quote {
                    Some(q) => !translation.contains(q),
                    None => bare_attribute_quote(translation).is_ok(),
                }
        }
        SlotKind::Speaker => !translation.contains(':') && !translation.trim().is_empty(),
        SlotKind::Text => !translation.contains('['),
        SlotKind::Dialogue => {
            scan_line(translation, &mut ScanState::default()).kind == LineKind::Text
                && preserved_tags(source, translation)
        }
    }
}

fn empty_injection_report() -> InjectionReport {
    InjectionReport {
        skip_reasons: Default::default(),
        files_modified: 0,
        strings_written: 0,
        strings_skipped: 0,
        warnings: Vec::new(),
        files_written: Vec::new(),
    }
}

#[cfg(test)]
fn apply_ks_translations(
    bytes: &[u8],
    label: &str,
    file_entries: &[&StringEntry],
) -> Result<Option<(Vec<u8>, usize, usize)>> {
    apply_ks_translations_with_report(bytes, label, file_entries, &mut empty_injection_report())
}

/// All offsets refer to the original UTF-8 buffer. Replacing in reverse order
/// retains untouched bytes, mixed delimiters, BOM and the exact final newline.
fn apply_ks_translations_with_report(
    bytes: &[u8],
    label: &str,
    file_entries: &[&StringEntry],
    report: &mut InjectionReport,
) -> Result<Option<(Vec<u8>, usize, usize)>> {
    let mut decoded = decode_ks_utf8(bytes, label)?;
    let slots = scenario_slots(&decoded.text);
    let by_locator: HashMap<_, _> = slots.iter().map(|s| (s.locator.as_str(), s)).collect();
    let mut counts = HashMap::new();
    for entry in file_entries {
        if let Some((_, locator)) = entry.id.rsplit_once('#') {
            *counts.entry(locator).or_insert(0) += 1;
        }
    }
    let mut edits = Vec::new();
    for entry in file_entries {
        let Some(translation) = entry.translation.as_deref() else {
            report.skip("untranslated", 1);
            continue;
        };
        let Some((_, locator)) = entry.id.rsplit_once('#') else {
            report.skip("invalid_locator", 1);
            continue;
        };
        let Some(slot) = by_locator.get(locator) else {
            report.skip("target_missing", 1);
            continue;
        };
        let reason = if counts[locator] != 1 {
            Some("duplicate")
        } else if !slot_matches(entry, slot, &decoded.text) {
            Some("source_changed")
        } else if entry.require_current_translation().is_err() {
            Some("stale_translation")
        } else if entry.require_preserved_translation_controls().is_err() {
            Some("invalid_placeholders")
        } else if !safe_replacement(slot, &entry.source, translation) {
            Some("invalid_translation")
        } else if translation == entry.source {
            Some("unchanged")
        } else {
            None
        };
        if let Some(reason) = reason {
            report.skip(reason, 1);
            continue;
        }
        let value = match (&slot.kind, bare_attribute_quote(translation)) {
            (SlotKind::Attribute(None), Ok(Some(q))) => format!("{q}{translation}{q}"),
            _ => translation.to_string(),
        };
        edits.push((slot.range.clone(), value));
    }
    if edits.is_empty() {
        return Ok(None);
    }
    edits.sort_by_key(|(range, _)| range.start);
    for (range, value) in edits.iter().rev() {
        decoded.text.replace_range(range.clone(), value);
    }
    Ok(Some((
        encode_ks_utf8(&decoded),
        edits.len(),
        file_entries.len() - edits.len(),
    )))
}

/// Split `resources/app.asar/data/scenario/a.ks` → (`resources/app.asar`, `data/scenario/a.ks`).
fn split_asar_virtual_path(path: &Path) -> Option<(String, String)> {
    let s = path.to_string_lossy().replace('\\', "/");
    let lower = s.to_ascii_lowercase();
    let needle = "app.asar/";
    let idx = lower.find(needle)?;
    let archive = s[..idx + "app.asar".len()].to_string();
    let inner = s[idx + needle.len()..].to_string();
    if inner.is_empty() {
        return None;
    }
    Some((archive, inner))
}

/// Split `package.nw/data/scenario/a.ks` or `data.exe/data/scenario/a.ks`.
fn split_nw_virtual_path(path: &Path) -> Option<(String, String)> {
    let s = path.to_string_lossy().replace('\\', "/");
    let lower = s.to_ascii_lowercase();
    // Prefer package.nw (fixed name).
    if let Some(idx) = lower.find("package.nw/") {
        let archive = s[..idx + "package.nw".len()].to_string();
        let inner = s[idx + "package.nw/".len()..].to_string();
        if !inner.is_empty() {
            return Some((archive, inner));
        }
    }
    // Any `something.exe/` segment (case-insensitive).
    let parts: Vec<&str> = s.split('/').collect();
    for (i, part) in parts.iter().enumerate() {
        if part.len() >= 4 && part.to_ascii_lowercase().ends_with(".exe") {
            let archive = parts[..=i].join("/");
            let inner = parts[i + 1..].join("/");
            if !inner.is_empty() {
                return Some((archive, inner));
            }
        }
    }
    None
}

// ─── Plugin ────────────────────────────────────────────────────────────────

impl FormatPlugin for TyranoPlugin {
    fn id(&self) -> &str {
        "tyrano"
    }

    fn name(&self) -> &str {
        "TyranoBuilder / TyranoScript"
    }

    fn description(&self) -> &str {
        "TyranoBuilder data/scenario *.ks loose + app.asar + NW.js package.nw/data.exe (UTF-8)"
    }

    fn stability(&self) -> locust_core::extraction::FormatStability {
        locust_core::extraction::FormatStability::Experimental
    }

    fn supported_extensions(&self) -> &[&str] {
        &[".ks", ".asar", ".nw", ".exe"]
    }

    fn supported_modes(&self) -> Vec<OutputMode> {
        vec![OutputMode::Replace]
    }

    fn detect(&self, path: &Path) -> bool {
        Self::detect_path(path)
    }

    fn extract(&self, path: &Path) -> Result<Vec<StringEntry>> {
        let root = Self::root_dir(path);
        let label = path.display().to_string();

        let ks_files = if path.is_file() && Self::is_ks(path) && Self::detect_path(path) {
            vec![path.to_path_buf()]
        } else {
            Self::find_scenario_ks(&root)
        };
        let asars = Self::find_app_asars(&root);
        let nw_containers = if path.is_file()
            && (tyrano_nw::is_package_nw_name(path) || tyrano_nw::is_exe_name(path))
            && Self::detect_path(path)
        {
            vec![path.to_path_buf()]
        } else {
            Self::find_nw_containers(&root)
        };

        if ks_files.is_empty() && asars.is_empty() && nw_containers.is_empty() {
            if Self::looks_like_tyrano(&root) {
                return Err(parse_err(
                    &label,
                    "TyranoBuilder layout detected (tyrano/ and/or data/scenario/) but no loose \
                     scenario .ks, no app.asar, and no package.nw / scenario-bearing .exe",
                ));
            }
            return Err(parse_err(
                &label,
                "no TyranoBuilder scenario .ks files found (expected data/scenario/*.ks, \
                 app.asar, package.nw, or NW.js data.exe)",
            ));
        }

        let mut all = Vec::new();

        for fpath in &ks_files {
            let bytes = std::fs::read(fpath)?;
            let rel = fpath
                .strip_prefix(&root)
                .unwrap_or(fpath.as_path())
                .to_string_lossy()
                .replace('\\', "/");
            all.extend(entries_from_ks_bytes(&bytes, &rel, fpath.clone())?);
        }

        let mut asar_errors = 0usize;
        let mut last_asar_err = String::new();
        let mut nw_errors = 0usize;
        let mut last_nw_err = String::new();
        let mut ks_seen = 0usize;
        let mut ks_skipped = 0usize;

        for asar_path in &asars {
            let asar_rel = asar_path
                .strip_prefix(&root)
                .unwrap_or(asar_path.as_path())
                .to_string_lossy()
                .replace('\\', "/");
            let archive = match AsarArchive::open(asar_path) {
                Ok(a) => a,
                Err(e) => {
                    asar_errors += 1;
                    last_asar_err = e.to_string();
                    warn!(archive = %asar_rel, error = %e, "failed to open app.asar");
                    continue;
                }
            };
            for entry in archive.scenario_ks_entries() {
                ks_seen += 1;
                let payload = match archive.read_entry(entry) {
                    Ok(p) => p,
                    Err(e) => {
                        warn!(
                            archive = %asar_rel,
                            entry = %entry.path,
                            error = %e,
                            "asar .ks read failed; skipped"
                        );
                        ks_skipped += 1;
                        continue;
                    }
                };
                let rel = format!("{asar_rel}/{}", entry.path.replace('\\', "/"));
                let virtual_path = PathBuf::from(&rel);
                match entries_from_ks_bytes(&payload, &rel, virtual_path) {
                    Ok(entries) => all.extend(entries),
                    Err(e) => {
                        warn!(
                            archive = %asar_rel,
                            entry = %entry.path,
                            error = %e,
                            "asar .ks is not valid UTF-8 text; skipped"
                        );
                        ks_skipped += 1;
                    }
                }
            }
        }

        for nw_path in &nw_containers {
            let nw_rel = nw_path
                .strip_prefix(&root)
                .unwrap_or(nw_path.as_path())
                .to_string_lossy()
                .replace('\\', "/");
            let archive = match NwArchive::open(nw_path) {
                Ok(a) => a,
                Err(e) => {
                    nw_errors += 1;
                    last_nw_err = e.to_string();
                    warn!(archive = %nw_rel, error = %e, "failed to open NW.js package");
                    continue;
                }
            };
            for entry in archive.scenario_ks_entries() {
                ks_seen += 1;
                let payload = match archive.read_entry(entry) {
                    Ok(p) => p,
                    Err(e) => {
                        warn!(
                            archive = %nw_rel,
                            entry = %entry.path,
                            error = %e,
                            "NW.js .ks read failed; skipped"
                        );
                        ks_skipped += 1;
                        continue;
                    }
                };
                let rel = format!("{nw_rel}/{}", entry.path.replace('\\', "/"));
                let virtual_path = PathBuf::from(&rel);
                match entries_from_ks_bytes(&payload, &rel, virtual_path) {
                    Ok(entries) => all.extend(entries),
                    Err(e) => {
                        warn!(
                            archive = %nw_rel,
                            entry = %entry.path,
                            error = %e,
                            "NW.js .ks is not valid UTF-8 text; skipped"
                        );
                        ks_skipped += 1;
                    }
                }
            }
        }

        if all.is_empty() && ks_files.is_empty() {
            if asar_errors > 0 && nw_errors == 0 && ks_seen == 0 {
                return Err(parse_err(
                    &label,
                    format!("failed to parse app.asar: {last_asar_err}"),
                ));
            }
            if nw_errors > 0 && asar_errors == 0 && ks_seen == 0 {
                return Err(parse_err(
                    &label,
                    format!("failed to parse NW.js package: {last_nw_err}"),
                ));
            }
            if asar_errors > 0 && nw_errors > 0 && ks_seen == 0 {
                return Err(parse_err(
                    &label,
                    format!(
                        "failed to parse containers (asar: {last_asar_err}; nw: {last_nw_err})"
                    ),
                ));
            }
            if ks_seen == 0 {
                return Err(parse_err(
                    &label,
                    "no data/scenario/*.ks found in app.asar or NW.js package",
                ));
            }
            if ks_skipped > 0 {
                warn!(
                    skipped = ks_skipped,
                    "all container scenario .ks entries were skipped"
                );
            }
        }

        Ok(all)
    }

    fn revision_original_path(&self, file_path: &Path) -> PathBuf {
        file_path
            .ancestors()
            .find(|path| {
                is_app_asar(path)
                    || tyrano_nw::is_package_nw_name(path)
                    || tyrano_nw::is_exe_name(path)
            })
            .unwrap_or(file_path)
            .to_path_buf()
    }

    fn prepare_revision_entries(
        &self,
        entries: &mut [StringEntry],
        originals: &HashMap<PathBuf, RevisionOriginal>,
    ) -> Result<()> {
        // Core verified both the previous Direct output and pristine provenance.
        // Read the exact verified bytes, then prove alignment separately for each
        // scenario. A failed proof leaves source/guards and strict writes alone.
        for (current_path, original) in originals {
            let label = current_path.to_string_lossy();
            if Self::is_ks(current_path) {
                prepare_ks_revision(
                    original.read_text()?.as_bytes(),
                    &std::fs::read(current_path)?,
                    &label,
                    entries.iter_mut().filter(|e| e.file_path == *current_path),
                )?;
                continue;
            }
            let mut members: HashMap<String, Vec<&mut StringEntry>> = HashMap::new();
            for entry in entries.iter_mut() {
                if let Ok(member) = entry.file_path.strip_prefix(current_path) {
                    members
                        .entry(member.to_string_lossy().replace('\\', "/"))
                        .or_default()
                        .push(entry);
                }
            }
            if members.is_empty() {
                continue;
            }
            if is_app_asar(current_path) {
                let old = AsarArchive::from_bytes(current_path.clone(), original.read_bytes()?)
                    .map_err(|e| parse_err(&label, e.to_string()))?;
                let new = AsarArchive::open(current_path)
                    .map_err(|e| parse_err(&label, e.to_string()))?;
                for (member, rows) in members {
                    let old_bytes = old
                        .revision_scenario_bytes(&member)
                        .map_err(|e| parse_err(&label, e.to_string()))?;
                    let new_bytes = new
                        .revision_scenario_bytes(&member)
                        .map_err(|e| parse_err(&label, e.to_string()))?;
                    if let (Some(a), Some(b)) = (old_bytes, new_bytes) {
                        prepare_ks_revision(
                            &a,
                            &b,
                            &format!("{label}/{member}"),
                            rows.into_iter(),
                        )?;
                    }
                }
            } else if tyrano_nw::is_package_nw_name(current_path)
                || tyrano_nw::is_exe_name(current_path)
            {
                let old = NwArchive::from_bytes(current_path.clone(), original.read_bytes()?)
                    .map_err(|e| parse_err(&label, e.to_string()))?;
                let new =
                    NwArchive::open(current_path).map_err(|e| parse_err(&label, e.to_string()))?;
                for (member, rows) in members {
                    let old_bytes = old
                        .revision_scenario_bytes(&member)
                        .map_err(|e| parse_err(&label, e.to_string()))?;
                    let new_bytes = new
                        .revision_scenario_bytes(&member)
                        .map_err(|e| parse_err(&label, e.to_string()))?;
                    if let (Some(a), Some(b)) = (old_bytes, new_bytes) {
                        prepare_ks_revision(
                            &a,
                            &b,
                            &format!("{label}/{member}"),
                            rows.into_iter(),
                        )?;
                    }
                }
            }
        }
        Ok(())
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
        let mut report = empty_injection_report();

        let mut by_file: HashMap<PathBuf, Vec<&StringEntry>> = HashMap::new();
        for e in entries {
            by_file.entry(e.file_path.clone()).or_default().push(e);
        }

        let search_root = Self::root_dir(path);

        let mut asar_groups: HashMap<String, Vec<(String, Vec<&StringEntry>)>> = HashMap::new();
        let mut nw_groups: HashMap<String, Vec<(String, Vec<&StringEntry>)>> = HashMap::new();
        let mut loose: Vec<(PathBuf, Vec<&StringEntry>)> = Vec::new();

        for (file_path, file_entries) in by_file {
            if let Some((archive, inner)) = split_asar_virtual_path(&file_path) {
                asar_groups
                    .entry(archive)
                    .or_default()
                    .push((inner, file_entries));
            } else if let Some((archive, inner)) = split_nw_virtual_path(&file_path) {
                nw_groups
                    .entry(archive)
                    .or_default()
                    .push((inner, file_entries));
            } else {
                loose.push((file_path, file_entries));
            }
        }

        // Loose .ks
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
            match apply_ks_translations_with_report(&bytes, &label, &file_entries, &mut report) {
                Ok(Some((encoded, written, _))) => {
                    std::fs::write(&actual, &encoded)?;
                    report.files_modified += 1;
                    report.files_written.push(actual);
                    report.strings_written += written;
                }
                Ok(None) => {}
                Err(e) => {
                    report
                        .warnings
                        .push(format!("cannot re-encode {label}: {e}"));
                    report.skip("decode_error", file_entries.len());
                }
            }
        }

        // app.asar groups
        for (archive_rel, inners) in asar_groups {
            let arch_path = {
                let p = search_root.join(&archive_rel);
                if p.exists() {
                    p
                } else {
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
            let archive = match AsarArchive::open(&arch_path) {
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
            let mut unpacked_writes: Vec<(String, PathBuf, Vec<u8>)> = Vec::new();
            let mut arch_written = 0usize;

            for (inner, file_entries) in inners {
                let entry = match archive
                    .entries
                    .iter()
                    .find(|e| e.path.replace('\\', "/") == inner.replace('\\', "/"))
                {
                    Some(e) => e,
                    None => {
                        report
                            .warnings
                            .push(format!("entry {inner} not in {archive_rel}"));
                        report.skip("target_missing", file_entries.len());
                        continue;
                    }
                };
                if entry.unpacked {
                    let normalized = locust_core::patch::zipsec::normalize_entry_name(&entry.path);
                    let relative =
                        locust_core::patch::zipsec::safe_entry_path(&normalized, &entry.path)?;
                    guard_target(game_lock, &archive.unpacked_dir().join(relative))?;
                }
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
                match apply_ks_translations_with_report(&bytes, &label, &file_entries, &mut report)
                {
                    Ok(Some((encoded, written, _))) => {
                        arch_written += written;
                        let key = inner.replace('\\', "/");
                        if entry.unpacked {
                            let disk = archive
                                .unpacked_dir()
                                .join(inner.replace('/', std::path::MAIN_SEPARATOR_STR));
                            // Also update size in asar header via replacements map path
                            replacements.insert(key.clone(), encoded.clone());
                            unpacked_writes.push((key, disk, encoded));
                        } else {
                            replacements.insert(key, encoded);
                        }
                    }
                    Ok(None) => {}
                    Err(e) => {
                        report
                            .warnings
                            .push(format!("cannot translate {label}: {e}"));
                        report.skip("decode_error", file_entries.len());
                    }
                }
            }

            // Prepare the archive and all unpacked payloads together. A later
            // installation failure must restore earlier payloads and header.
            if !replacements.is_empty() {
                match tyrano_asar::rebuild_asar(&archive, &replacements) {
                    Ok(new_arch) => {
                        let mut outputs: Vec<(PathBuf, Vec<u8>)> = unpacked_writes
                            .into_iter()
                            .map(|(_, disk, data)| (disk, data))
                            .collect();
                        outputs.push((arch_path.clone(), new_arch));
                        match replace_files(game_lock, &outputs) {
                            Ok(backups) => {
                                note_backups(backups, &mut report.warnings);
                                report.files_modified += outputs.len();
                                report
                                    .files_written
                                    .extend(outputs.into_iter().map(|(path, _)| path));
                                report.strings_written += arch_written;
                            }
                            Err(e) => {
                                report
                                    .warnings
                                    .push(format!("safe-replace {archive_rel}: {e}"));
                                report.skip("write_error", arch_written);
                            }
                        }
                    }
                    Err(e) => {
                        report.warnings.push(format!("rebuild {archive_rel}: {e}"));
                        report.skip("rebuild_error", arch_written);
                    }
                }
            }
        }

        // NW.js package.nw / data.exe groups
        for (archive_rel, inners) in nw_groups {
            let arch_path = {
                let p = search_root.join(&archive_rel);
                if p.exists() {
                    p
                } else {
                    search_root.join(Path::new(&archive_rel).file_name().unwrap_or_default())
                }
            };
            if !arch_path.exists() {
                report
                    .warnings
                    .push(format!("missing NW.js package {archive_rel}"));
                for (_, fe) in &inners {
                    report.skip("target_missing", fe.len());
                }
                continue;
            }

            guard_target(game_lock, &arch_path)?;
            let archive = match NwArchive::open(&arch_path) {
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

            for (inner, file_entries) in inners {
                let entry = match archive
                    .entries
                    .iter()
                    .find(|e| e.path.replace('\\', "/") == inner.replace('\\', "/"))
                {
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
                match apply_ks_translations_with_report(&bytes, &label, &file_entries, &mut report)
                {
                    Ok(Some((encoded, written, _))) => {
                        arch_written += written;
                        replacements.insert(inner.replace('\\', "/"), encoded);
                    }
                    Ok(None) => {}
                    Err(e) => {
                        report
                            .warnings
                            .push(format!("cannot translate {label}: {e}"));
                        report.skip("decode_error", file_entries.len());
                    }
                }
            }

            if !replacements.is_empty() {
                match tyrano_nw::rebuild_nw_zip(&archive, &replacements) {
                    Ok(new_pkg) => {
                        match replace_files(game_lock, &[(arch_path.clone(), new_pkg)]) {
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
                        }
                    }
                    Err(e) => {
                        report.warnings.push(format!("rebuild {archive_rel}: {e}"));
                        report.skip("rebuild_error", arch_written);
                    }
                }
            }
        }

        Ok(report)
    }
}

// ─── Tests (synthetic fixtures only) ───────────────────────────────────────

#[cfg(test)]
mod tests {
    const C122_SCENARIO: &str = "\u{feff}; keep comment\r\n#表示名:face\r\nHello.[p]\n[glink text=Continue target=*next]\r\n@jump target=*done\n";

    fn c122_archive(kind: &str, scenario: &str) -> Vec<u8> {
        let members = [
            ("data/scenario/main.ks".into(), scenario.as_bytes().to_vec()),
            (
                "data/scenario/untouched.ks".into(),
                b"; no dialogue\r\n@jump target=*keep\n".to_vec(),
            ),
            ("data/keep.bin".into(), vec![0, 255, 128, 42]),
        ];
        if kind == "asar" {
            return tyrano_asar::write_asar(&members).unwrap();
        }
        use std::io::{Cursor, Write};
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        for (name, bytes) in &members {
            writer
                .start_file(
                    name,
                    zip::write::SimpleFileOptions::default()
                        .compression_method(zip::CompressionMethod::Deflated),
                )
                .unwrap();
            writer.write_all(bytes).unwrap();
        }
        let mut bytes = if kind == "exe" {
            b"MZ\x90\0UNRELATED_EXE_STUB".to_vec()
        } else {
            Vec::new()
        };
        bytes.extend(writer.finish().unwrap().into_inner());
        bytes
    }

    fn c122_path(root: &Path, kind: &str) -> PathBuf {
        root.join(match kind {
            "asar" => "resources/app.asar",
            "nw" => "package.nw",
            "exe" => "data.exe",
            "loose" => "data/scenario/main.ks",
            _ => unreachable!(),
        })
    }

    fn c122_write(path: &Path, kind: &str, text: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            path,
            if kind == "loose" {
                text.as_bytes().to_vec()
            } else {
                c122_archive(kind, text)
            },
        )
        .unwrap();
    }

    fn c122_members(path: &Path, kind: &str) -> HashMap<String, Vec<u8>> {
        if kind == "asar" {
            let archive = AsarArchive::open(path).unwrap();
            archive
                .entries
                .iter()
                .map(|e| (e.path.clone(), archive.read_entry(e).unwrap()))
                .collect()
        } else {
            let archive = NwArchive::open(path).unwrap();
            if kind == "exe" {
                assert_eq!(archive.exe_prefix(), b"MZ\x90\0UNRELATED_EXE_STUB");
            }
            archive
                .entries
                .iter()
                .map(|e| (e.path.clone(), archive.read_entry(e).unwrap()))
                .collect()
        }
    }

    fn c122_direct_revisions(kind: &str) {
        use locust_core::{
            backup::BackupManager,
            database::{Database, EntryFilter},
            extraction::inject_direct,
        };
        for select_file in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let game = dir.path().join("game");
            let archive = c122_path(&game, kind);
            c122_write(&archive, kind, C122_SCENARIO);
            fs::write(game.join("unrelated.bin"), b"outside archive\0\xff").unwrap();
            let original = fs::read(&archive).unwrap();
            let members = c122_members(&archive, kind);
            let selection = if select_file { &archive } else { &game };
            let registry = crate::default_registry();
            let plugin = registry.get("tyrano").unwrap();
            let db = Database::open_in_memory().unwrap();
            let manager = BackupManager::new(dir.path().join("backups"));
            let mut entries = plugin.extract(selection).unwrap();
            assert_eq!(entries.len(), 3);
            let mut pristine = None;
            for (generation, prefix) in ["TL ", "R2 TL ", "R3 TL "].into_iter().enumerate() {
                for entry in &mut entries {
                    entry.translation = Some(format!("{prefix}{}", entry.source));
                }
                // Save only translation edits; never extract/refresh the original rows.
                db.save_entries(&entries).unwrap();
                let report = inject_direct(
                    &registry,
                    &db,
                    &manager,
                    selection,
                    "tyrano",
                    &["en".into()],
                )
                .unwrap();
                assert_eq!(
                    (report.strings_written, report.strings_skipped),
                    (3, 0),
                    "{kind}, file={select_file}, generation {}: {report:?}",
                    generation + 1
                );
                assert!(report.reports["en"].skip_reasons.is_empty());
                let id = report.pristine_backup_id.unwrap();
                if let Some(first) = &pristine {
                    assert_eq!(&id, first);
                } else {
                    pristine = Some(id);
                }
                let extracted = plugin.extract(selection).unwrap();
                assert_eq!(extracted.len(), entries.len());
                assert!(extracted.iter().all(|e| e.source.starts_with(prefix)));
                let after = c122_members(&archive, kind);
                for (name, bytes) in &members {
                    if name != "data/scenario/main.ks" {
                        assert_eq!(&after[name], bytes);
                    }
                }
                let expected = C122_SCENARIO
                    .replace("表示名", &format!("{prefix}表示名"))
                    .replace("Hello.", &format!("{prefix}Hello."))
                    .replace("text=Continue", &format!("text=\"{prefix}Continue\""));
                assert_eq!(after["data/scenario/main.ks"], expected.as_bytes());
                assert_eq!(
                    fs::read(game.join("unrelated.bin")).unwrap(),
                    b"outside archive\0\xff"
                );
                let saved = db.get_entries(&EntryFilter::default()).unwrap();
                for entry in &entries {
                    assert_eq!(
                        saved.iter().find(|e| e.id == entry.id).unwrap().source,
                        entry.source
                    );
                }
            }
            manager.restore(&pristine.unwrap()).unwrap();
            assert_eq!(fs::read(&archive).unwrap(), original);
        }
    }

    #[test]
    fn c122_asar_direct_revision_without_refresh() {
        c122_direct_revisions("asar");
    }

    #[test]
    fn c122_nw_direct_revision_without_refresh() {
        c122_direct_revisions("nw");
    }

    #[test]
    fn c122_exe_direct_revision_without_refresh() {
        c122_direct_revisions("exe");
    }

    #[test]
    fn c122_revision_alignment_is_per_member_and_requires_original_match() {
        for kind in ["asar", "nw", "exe"] {
            for drift in [
                "none",
                "target",
                "command",
                "comment",
                "stale",
                "unverified",
            ] {
                let dir = tempfile::tempdir().unwrap();
                let path = c122_path(dir.path(), kind);
                c122_write(&path, kind, C122_SCENARIO);
                let plugin = TyranoPlugin::new();
                let mut entries = plugin.extract(dir.path()).unwrap();
                for entry in &mut entries {
                    entry.file_path = dir.path().join(&entry.file_path);
                    entry.translation = Some(format!("R2 TL {}", entry.source));
                }
                let original_path = dir.path().join("original.bin");
                fs::copy(&path, &original_path).unwrap();
                let mut originals = HashMap::from([(
                    path.clone(),
                    RevisionOriginal::capture(&original_path).unwrap(),
                )]);
                let mut current = C122_SCENARIO
                    .replace("表示名", "TL 表示名")
                    .replace("Hello.", "TL Hello.")
                    .replace("text=Continue", "text=\"TL Continue\"");
                match drift {
                    "target" => current = current.replace("*next", "*other"),
                    "command" => current = current.replace("@jump", "@call"),
                    "comment" => current = current.replace("keep comment", "changed comment"),
                    "stale" => {
                        for entry in &mut entries {
                            entry.source = "Unrelated stale source".into();
                        }
                    }
                    "unverified" => originals.clear(),
                    _ => {}
                }
                c122_write(&path, kind, &current);
                let before = fs::read(&path).unwrap();
                let sources: Vec<_> = entries.iter().map(|e| e.source.clone()).collect();
                plugin
                    .prepare_revision_entries(&mut entries, &originals)
                    .unwrap();
                if drift == "none" {
                    assert!(
                        entries.iter().all(|e| e.source.starts_with("TL ")),
                        "{kind}: verified payload did not retarget"
                    );
                } else {
                    assert_eq!(
                        entries.iter().map(|e| e.source.clone()).collect::<Vec<_>>(),
                        sources,
                        "{kind}/{drift}"
                    );
                }
                let report = plugin.inject(dir.path(), &entries).unwrap();
                if drift == "none" {
                    assert_eq!((report.strings_written, report.strings_skipped), (3, 0));
                } else {
                    assert_eq!(
                        (report.strings_written, report.strings_skipped),
                        (0, 3),
                        "{kind}/{drift}"
                    );
                    assert_eq!(fs::read(&path).unwrap(), before);
                    assert_eq!(
                        report.skip_reasons.get("source_changed"),
                        Some(&3),
                        "{kind}/{drift}: {report:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn c122_archive_and_loose_skips_are_classified() {
        for kind in ["asar", "nw", "exe", "loose"] {
            for reason in [
                "source_changed",
                "untranslated",
                "unchanged",
                "invalid_translation",
                "invalid_locator",
                "target_missing",
                "duplicate",
                "stale_translation",
            ] {
                let dir = tempfile::tempdir().unwrap();
                let path = c122_path(dir.path(), kind);
                c122_write(&path, kind, "Hello.[p]\n");
                let plugin = TyranoPlugin::new();
                let mut entries = plugin.extract(dir.path()).unwrap();
                entries[0].translation = Some("Translated.[p]".into());
                match reason {
                    "source_changed" => entries[0].source = "Stale.[p]".into(),
                    "untranslated" => entries[0].translation = None,
                    "unchanged" => entries[0].translation = Some(entries[0].source.clone()),
                    "invalid_translation" => entries[0].translation = Some("bad\n[p]".into()),
                    "invalid_locator" => entries[0].id = "no-locator".into(),
                    "target_missing" => entries[0].id = "scene#9999".into(),
                    "duplicate" => entries.push(entries[0].clone()),
                    "stale_translation" => {
                        entries[0]
                            .metadata
                            .insert("locust_stale_translation".into(), serde_json::json!({}));
                    }
                    _ => unreachable!(),
                }
                let before = fs::read(&path).unwrap();
                let report = plugin.inject(dir.path(), &entries).unwrap();
                assert_eq!(
                    (report.strings_written, report.strings_skipped),
                    (0, entries.len()),
                    "{kind}/{reason}: {report:?}"
                );
                assert_eq!(
                    report.skip_reasons.get(reason),
                    Some(&entries.len()),
                    "{kind}/{reason}: {report:?}"
                );
                assert_eq!(
                    report.skip_reasons.values().sum::<usize>(),
                    report.strings_skipped
                );
                assert_eq!(fs::read(&path).unwrap(), before);
            }
        }
    }

    fn c122_replace_members(path: &Path, kind: &str, replacements: HashMap<String, Vec<u8>>) {
        let bytes = if kind == "asar" {
            tyrano_asar::rebuild_asar(&AsarArchive::open(path).unwrap(), &replacements).unwrap()
        } else {
            tyrano_nw::rebuild_nw_zip(&NwArchive::open(path).unwrap(), &replacements).unwrap()
        };
        fs::write(path, bytes).unwrap();
    }

    #[test]
    fn c122_only_aligned_archive_members_retarget() {
        for kind in ["asar", "nw", "exe"] {
            let dir = tempfile::tempdir().unwrap();
            let path = c122_path(dir.path(), kind);
            c122_write(&path, kind, C122_SCENARIO);
            let sibling = "data/scenario/untouched.ks";
            c122_replace_members(
                &path,
                kind,
                HashMap::from([(sibling.into(), b"Other.[p]\n".to_vec())]),
            );
            let plugin = TyranoPlugin::new();
            let mut entries = plugin.extract(dir.path()).unwrap();
            assert_eq!(entries.len(), 4);
            for entry in &mut entries {
                entry.file_path = dir.path().join(&entry.file_path);
                entry.translation = Some(format!("R2 TL {}", entry.source));
            }
            let original = dir.path().join("original.bin");
            fs::copy(&path, &original).unwrap();
            let originals =
                HashMap::from([(path.clone(), RevisionOriginal::capture(&original).unwrap())]);
            let drifted = C122_SCENARIO
                .replace("表示名", "TL 表示名")
                .replace("Hello.", "TL Hello.")
                .replace("text=Continue", "text=\"TL Continue\"")
                .replace("*next", "*different");
            c122_replace_members(
                &path,
                kind,
                HashMap::from([
                    ("data/scenario/main.ks".into(), drifted.as_bytes().to_vec()),
                    (sibling.into(), b"TL Other.[p]\n".to_vec()),
                ]),
            );
            let before = c122_members(&path, kind);
            plugin
                .prepare_revision_entries(&mut entries, &originals)
                .unwrap();
            assert_eq!(
                entries
                    .iter()
                    .find(|e| e.id.contains("untouched.ks#"))
                    .unwrap()
                    .source,
                "TL Other.[p]"
            );
            assert!(entries
                .iter()
                .filter(|e| e.id.contains("main.ks#"))
                .all(|e| !e.source.starts_with("TL ")));
            let report = plugin.inject(dir.path(), &entries).unwrap();
            assert_eq!((report.strings_written, report.strings_skipped), (1, 3));
            assert_eq!(report.skip_reasons.get("source_changed"), Some(&3));
            let after = c122_members(&path, kind);
            assert_eq!(after[sibling], b"R2 TL Other.[p]\n");
            for (member, bytes) in before {
                if member != sibling {
                    assert_eq!(after[&member], bytes);
                }
            }
        }
    }

    #[test]
    fn c122_changed_archive_original_is_rejected_before_retargeting() {
        for kind in ["asar", "nw", "exe"] {
            let dir = tempfile::tempdir().unwrap();
            let path = c122_path(dir.path(), kind);
            c122_write(&path, kind, C122_SCENARIO);
            let plugin = TyranoPlugin::new();
            let mut entries = plugin.extract(dir.path()).unwrap();
            for entry in &mut entries {
                entry.file_path = dir.path().join(&entry.file_path);
            }
            let original = dir.path().join("original.bin");
            fs::copy(&path, &original).unwrap();
            let originals =
                HashMap::from([(path.clone(), RevisionOriginal::capture(&original).unwrap())]);
            let mut damaged = fs::read(&original).unwrap();
            let last = damaged.len() - 1;
            damaged[last] ^= 1;
            fs::write(&original, damaged).unwrap();
            c122_write(&path, kind, &C122_SCENARIO.replace("Hello.", "TL Hello."));
            let before = fs::read(&path).unwrap();
            let sources: Vec<_> = entries.iter().map(|e| e.source.clone()).collect();
            assert!(plugin
                .prepare_revision_entries(&mut entries, &originals)
                .unwrap_err()
                .to_string()
                .contains("changed since verification"));
            assert_eq!(
                entries.iter().map(|e| e.source.clone()).collect::<Vec<_>>(),
                sources
            );
            assert_eq!(fs::read(path).unwrap(), before);
        }
    }

    fn c119_entries(text: &str) -> Vec<StringEntry> {
        entries_from_ks_bytes(text.as_bytes(), "scene.ks", PathBuf::from("scene.ks")).unwrap()
    }

    fn c119_apply(text: &str, entries: &[StringEntry]) -> Option<(Vec<u8>, usize, usize)> {
        apply_ks_translations(
            text.as_bytes(),
            "scene.ks",
            &entries.iter().collect::<Vec<_>>(),
        )
        .unwrap()
    }

    #[test]
    fn c119_script_state_and_choice_value_only() {
        let text =
            "[iscript]\nwindow.flag=1;\n[endscript]\n[glink text=\"Continue\" target=\"*next\"]\n";
        let mut entries = c119_entries(text);
        assert_eq!(
            entries
                .iter()
                .map(|e| e.source.as_str())
                .collect::<Vec<_>>(),
            ["Continue"]
        );
        entries[0].translation = Some("Continuar".into());
        let (out, written, skipped) = c119_apply(text, &entries).unwrap();
        assert_eq!((written, skipped), (1, 0));
        assert_eq!(out, text.replace("Continue", "Continuar").as_bytes());
        let text = "@iscript\nwindow.x='[glink text=secret]';\n@endscript\n[html]\n<div>hidden</div>\n[endhtml]\nVisible.[p]\n";
        assert_eq!(
            c119_entries(text)
                .iter()
                .map(|e| e.source.as_str())
                .collect::<Vec<_>>(),
            ["Visible.[p]"]
        );
    }

    #[test]
    fn c119_quoted_brackets_adjacent_tags_and_escaped_text() {
        let text = concat!(
            "[eval exp=\"f.x[0]=1\"]\n",
            "[eval exp='f.x[0]=1'][wait time=2]\n",
            "[eval exp=`f.x[0]=1`] [eval exp=f.x[0]]\n",
            "@eval exp=\"f.x[0]=1\"\n",
            "Hello.[p]\n",
            "\\[literal] text[p]\n",
            "[eval exp=\"f.x[0]=1\"]Visible.[p]\n",
        );
        let entries = c119_entries(text);
        assert_eq!(
            entries
                .iter()
                .map(|e| e.source.as_str())
                .collect::<Vec<_>>(),
            [
                "Hello.[p]",
                "\\[literal] text[p]",
                "[eval exp=\"f.x[0]=1\"]Visible.[p]"
            ]
        );
    }

    #[test]
    fn c119_display_names_preserve_references_faces_and_spacing() {
        let text = "[chara_new name=\"akane\" jname=\"あかね\" storage=\"face.png\"]\r\n#akane:happy\n  #  ??? :happy  \r\n";
        let mut entries = c119_entries(text);
        assert_eq!(
            entries
                .iter()
                .map(|e| e.source.as_str())
                .collect::<Vec<_>>(),
            ["あかね", "???"]
        );
        entries[0].translation = Some("Akane".into());
        entries[1].translation = Some("Desconocida".into());
        let (out, written, _) = c119_apply(text, &entries).unwrap();
        assert_eq!(written, 2);
        assert_eq!(
            out,
            text.replace("あかね", "Akane")
                .replace("???", "Desconocida")
                .as_bytes()
        );
    }

    #[test]
    fn c119_ruby_readings_and_base_spans_are_independent() {
        let text = "[ruby text=\"かん\" name=keep]漢[ruby text=じ]字[l]\n";
        let mut entries = c119_entries(text);
        assert_eq!(
            entries
                .iter()
                .map(|e| e.source.as_str())
                .collect::<Vec<_>>(),
            ["かん", "漢", "じ", "字"]
        );
        for (entry, replacement) in entries.iter_mut().zip(["kan", "Kan", "ji", "ji"]) {
            entry.translation = Some(replacement.into());
        }
        let (out, written, skipped) = c119_apply(text, &entries).unwrap();
        assert_eq!((written, skipped), (4, 0));
        assert_eq!(
            out,
            b"[ruby text=\"kan\" name=keep]Kan[ruby text=ji]ji[l]\n"
        );
        assert_eq!(
            c119_entries(std::str::from_utf8(&out).unwrap())
                .iter()
                .map(|e| e.source.as_str())
                .collect::<Vec<_>>(),
            ["kan", "Kan", "ji", "ji"]
        );
    }

    #[test]
    fn c119_unquoted_attribute_gains_quotes_when_translation_needs_them() {
        use locust_core::backup::RevisionOriginal;
        let text = "[ruby text=かん]簡[l]\n[glink text=はい target=*yes]\n[glink text=いいえ target=*no]\n";
        let mut entries = c119_entries(text);
        assert_eq!(
            entries
                .iter()
                .map(|e| e.source.as_str())
                .collect::<Vec<_>>(),
            ["かん", "簡", "はい", "いいえ"]
        );
        for (entry, replacement) in
            entries
                .iter_mut()
                .zip(["kan ji", "Kan", "Seguir adelante", "No \"ahora\""])
        {
            entry.translation = Some(replacement.into());
        }
        let (out, written, skipped) = c119_apply(text, &entries).unwrap();
        assert_eq!((written, skipped), (4, 0));
        let expected = "[ruby text=\"kan ji\"]Kan[l]\n[glink text=\"Seguir adelante\" target=*yes]\n[glink text='No \"ahora\"' target=*no]\n";
        assert_eq!(std::str::from_utf8(&out).unwrap(), expected);
        assert_eq!(
            c119_entries(expected)
                .iter()
                .map(|e| e.source.as_str())
                .collect::<Vec<_>>(),
            ["kan ji", "Kan", "Seguir adelante", "No \"ahora\""]
        );
        // A value that no single quote style can hold is still rejected.
        let mut both = entries[2].clone();
        both.translation = Some("a \"b\" 'c'".into());
        assert!(c119_apply(text, &[both]).is_none());

        // The next Direct injection retargets the now-quoted slots.
        let dir = tempfile::tempdir().unwrap();
        let original = dir.path().join("original.ks");
        let current = dir.path().join("scene.ks");
        fs::write(&original, text).unwrap();
        fs::write(&current, expected).unwrap();
        let mut entries =
            entries_from_ks_bytes(text.as_bytes(), "scene.ks", current.clone()).unwrap();
        for (entry, replacement) in entries
            .iter_mut()
            .zip(["kan ji 2", "Kan 2", "Seguir 2", "Ahora no"])
        {
            entry.translation = Some(replacement.into());
        }
        let originals = HashMap::from([(
            current.clone(),
            RevisionOriginal::capture(&original).unwrap(),
        )]);
        TyranoPlugin::new()
            .prepare_revision_entries(&mut entries, &originals)
            .unwrap();
        let (out, written, skipped) = apply_ks_translations(
            expected.as_bytes(),
            "scene.ks",
            &entries.iter().collect::<Vec<_>>(),
        )
        .unwrap()
        .unwrap();
        assert_eq!((written, skipped), (4, 0));
        assert_eq!(
            std::str::from_utf8(&out).unwrap(),
            "[ruby text=\"kan ji 2\"]Kan 2[l]\n[glink text=\"Seguir 2\" target=*yes]\n[glink text='Ahora no' target=*no]\n"
        );
    }

    #[test]
    fn c119_attribute_allowlist_and_literal_syntax() {
        let text = concat!(
            "@glink text='Continue' target=*next\n",
            "[glink text=`Next` target=*last]\n",
            "[glink text=&f.choice][glink text=%label][chara_new jname=&f.name]\n",
            "[eval text=secret exp='f.x[0]'][button text=technical]\n",
        );
        let mut entries = c119_entries(text);
        assert_eq!(
            entries
                .iter()
                .map(|e| e.source.as_str())
                .collect::<Vec<_>>(),
            ["Continue", "Next"]
        );
        entries[0].translation = Some("Seguir".into());
        entries[1].translation = Some("Después".into());
        assert_eq!(
            c119_apply(text, &entries).unwrap().0,
            text.replace("Continue", "Seguir")
                .replace("Next", "Después")
                .as_bytes()
        );
        for replacement in ["bad' target=*oops", "&f.code", "line\nbreak"] {
            entries[0].translation = Some(replacement.into());
            assert!(
                c119_apply(text, &entries[..1]).is_none(),
                "unsafe attribute {replacement:?}"
            );
        }
    }

    #[test]
    fn c119_stale_legacy_and_duplicate_locators_are_rejected() {
        let text = "Actual.[p]\n[eval exp=\"f.x[0]=1\"]\n[iscript]\nwindow.x=1;\n[endscript]\n";
        for (id, source) in [
            ("scene.ks#1", "Old.[p]"),
            ("scene.ks#2", "[eval exp=\"f.x[0]=1\"]"),
            ("scene.ks#4", "window.x=1;"),
        ] {
            let mut entry = StringEntry::new(id, source, PathBuf::from("scene.ks"));
            entry.translation = Some(format!("Translated {source}"));
            assert!(c119_apply(text, &[entry]).is_none(), "unsafe old row {id}");
        }
        let mut entries = c119_entries("Actual.[p]\n");
        entries[0].translation = Some("First.[p]".into());
        let mut duplicate = entries[0].clone();
        duplicate.translation = Some("Second.[p]".into());
        entries.push(duplicate);
        assert!(c119_apply("Actual.[p]\n", &entries).is_none());
    }

    #[test]
    fn c119_missing_or_changed_controls_never_write() {
        let text = "Hello.[p]\n";
        let mut entries = c119_entries(text);
        for replacement in [
            "Hola.",
            "Hola.[r]",
            "[eval exp=evil]Hola.[p]",
            "Hola.[p]\nNew line",
        ] {
            entries[0].translation = Some(replacement.into());
            assert!(
                c119_apply(text, &entries).is_none(),
                "unsafe dialogue {replacement:?}"
            );
        }
    }

    #[test]
    fn c119_preserves_bom_mixed_delimiters_and_no_final_newline() {
        let text = "\u{feff}; keep\r\nHello.[p]\n; bare\r; still here\r\nLast.[l]";
        let mut entries = c119_entries(text);
        let entry = entries
            .iter_mut()
            .find(|e| e.source == "Hello.[p]")
            .unwrap();
        entry.translation = Some("Hola.[p]".into());
        assert_eq!(
            c119_apply(text, &entries).unwrap().0,
            text.replace("Hello", "Hola").as_bytes()
        );
    }

    #[test]
    fn c119_verified_revision_retargets_only_matching_original_slots() {
        use locust_core::backup::RevisionOriginal;
        let dir = tempfile::tempdir().unwrap();
        let original = dir.path().join("original.ks");
        let current = dir.path().join("scene.ks");
        let source = "Hello.[p]\n[glink text=\"Continue\" target=*next]\n";
        fs::write(&original, source).unwrap();
        fs::write(
            &current,
            "First.[p]\n[glink text=\"First choice\" target=*next]\n",
        )
        .unwrap();
        let mut entries =
            entries_from_ks_bytes(source.as_bytes(), "scene.ks", current.clone()).unwrap();
        for entry in &mut entries {
            entry.translation = Some(
                entry
                    .source
                    .replace("Hello", "Second")
                    .replace("Continue", "Second choice"),
            );
        }
        let originals = HashMap::from([(
            current.clone(),
            RevisionOriginal::capture(&original).unwrap(),
        )]);
        TyranoPlugin::new()
            .prepare_revision_entries(&mut entries, &originals)
            .unwrap();
        assert_eq!(entries[0].source, "First.[p]");
        let bytes = fs::read(&current).unwrap();
        let (out, written, skipped) =
            apply_ks_translations(&bytes, "scene.ks", &entries.iter().collect::<Vec<_>>())
                .unwrap()
                .unwrap();
        assert_eq!((written, skipped), (2, 0));
        assert_eq!(
            out,
            b"Second.[p]\n[glink text=\"Second choice\" target=*next]\n"
        );
        // Unverified rows cannot acquire a new source from the current file.
        let mut stale =
            entries_from_ks_bytes(source.as_bytes(), "scene.ks", current.clone()).unwrap();
        stale[0].source = "Unrelated.[p]".into();
        TyranoPlugin::new()
            .prepare_revision_entries(&mut stale, &originals)
            .unwrap();
        assert_eq!(stale[0].source, "Unrelated.[p]");
        // A changed command target prevents retargeting the old attributes.
        fs::write(
            &current,
            "First.[p]\n[glink text=\"First choice\" target=*other]\n",
        )
        .unwrap();
        let mut drifted = entries_from_ks_bytes(source.as_bytes(), "scene.ks", current).unwrap();
        TyranoPlugin::new()
            .prepare_revision_entries(&mut drifted, &originals)
            .unwrap();
        assert_eq!(drifted[1].source, "Continue");
    }

    #[test]
    fn held_lock_injects_loose_and_archive_without_releasing_exclusion() {
        for packed in [false, true] {
            let root = tempfile::tempdir().unwrap();
            let other = tempfile::tempdir().unwrap();
            let path = if packed {
                let path = root.path().join("package.nw");
                fs::write(&path, build_nw_zip_bytes(sample_scenario())).unwrap();
                path
            } else {
                write_tyrano_layout(root.path(), "scene.ks", sample_scenario(), false)
            };
            let original = fs::read(&path).unwrap();
            let plugin = TyranoPlugin::new();
            let mut entries = plugin.extract(root.path()).unwrap();
            entries.retain(|e| e.source == "This is narration.");
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
    fn all_packed_formats_roundtrip_quoted_text_and_preserve_unrelated_sidecars() {
        for kind in ["asar", "nw", "exe"] {
            let dir = tempfile::tempdir().unwrap();
            let (path, bytes) = match kind {
                "asar" => (
                    dir.path().join("app.asar"),
                    crate::tyrano_asar::write_asar(&[(
                        "data/scenario/scene1.ks".into(),
                        sample_scenario().as_bytes().to_vec(),
                    )])
                    .unwrap(),
                ),
                "nw" => (
                    dir.path().join("package.nw"),
                    build_nw_zip_bytes(sample_scenario()),
                ),
                _ => {
                    let mut bytes = b"MZ\x90\x00NEUTRAL_STUB".to_vec();
                    bytes.extend(build_nw_zip_bytes(sample_scenario()));
                    (dir.path().join("data.exe"), bytes)
                }
            };
            fs::write(&path, &bytes).unwrap();
            let sidecar = std::path::PathBuf::from(format!("{}.locust-old", path.display()));
            fs::create_dir(&sidecar).unwrap();
            fs::write(sidecar.join("sentinel"), b"recursive exact bytes").unwrap();
            let plugin = TyranoPlugin::new();
            let mut entries = plugin.extract(dir.path()).unwrap();
            let translated = "He said \"hello\"; it's C:\\folder.";
            for e in &mut entries {
                if e.source == "This is narration." {
                    e.translation = Some(translated.into());
                }
            }
            let report = plugin.inject(dir.path(), &entries).unwrap();
            assert_eq!(report.strings_written, 1, "{kind}: {report:?}");
            assert_eq!(fs::read(backup_path(&report)).unwrap(), bytes);
            assert_eq!(
                fs::read(sidecar.join("sentinel")).unwrap(),
                b"recursive exact bytes"
            );
            assert_eq!(report.files_written, vec![path]);
            let again = plugin.extract(dir.path()).unwrap();
            assert!(
                again.iter().any(|e| e.source == translated),
                "{kind}: {again:?}"
            );
            assert_eq!(again.len(), entries.len());
        }
    }

    #[test]
    fn unpacked_payload_and_archive_header_are_committed_with_exact_backups() {
        let dir = tempfile::tempdir().unwrap();
        let arch_path = dir.path().join("app.asar");
        let disk = dir.path().join("app.asar.unpacked/data/scenario/scene.ks");
        fs::create_dir_all(disk.parent().unwrap()).unwrap();
        let original = b"Original narration.\n";
        fs::write(&disk, original).unwrap();
        let json=serde_json::json!({"files":{"data":{"files":{"scenario":{"files":{"scene.ks":{"size":original.len(),"unpacked":true}}}}}}}).to_string();
        let pad = (4 - json.len() % 4) % 4;
        let mut bytes = Vec::new();
        for n in [
            4u32,
            (8 + json.len() + pad) as u32,
            (4 + json.len() + pad) as u32,
            json.len() as u32,
        ] {
            bytes.extend(n.to_le_bytes());
        }
        bytes.extend(json.as_bytes());
        bytes.resize(bytes.len() + pad, 0);
        fs::write(&arch_path, &bytes).unwrap();
        let plugin = TyranoPlugin::new();
        let mut entries = plugin.extract(dir.path()).unwrap();
        assert_eq!(entries.len(), 1);
        entries[0].translation = Some("A much longer \"quoted\" narration.".into());
        let report = plugin.inject(dir.path(), &entries).unwrap();
        assert_eq!(report.files_modified, 2, "{report:?}");
        assert_eq!(report.strings_written, 1);
        let backups: Vec<_> = report
            .warnings
            .iter()
            .filter_map(|w| w.strip_prefix("previous archive retained at "))
            .map(|p| fs::read(p).unwrap())
            .collect();
        assert!(backups.contains(&bytes));
        assert!(backups.contains(&original.to_vec()));
        let archive = AsarArchive::open(&arch_path).unwrap();
        assert_eq!(archive.entries[0].size, fs::metadata(&disk).unwrap().len());
        assert_eq!(
            plugin.extract(dir.path()).unwrap()[0].source,
            "A much longer \"quoted\" narration."
        );
        assert_eq!(report.files_written, vec![disk, arch_path]);
    }

    #[test]
    fn archive_basename_fallback_is_confined_to_selected_game() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("package.nw");
        fs::write(&path, build_nw_zip_bytes(sample_scenario())).unwrap();
        let plugin = TyranoPlugin::new();
        let mut entries = plugin.extract(dir.path()).unwrap();
        for e in &mut entries {
            e.file_path = PathBuf::from("obsolete/location").join(&e.file_path);
            if e.source == "This is narration." {
                e.translation = Some("Fallback translation.".into());
            }
        }
        let report = plugin.inject(dir.path(), &entries).unwrap();
        assert_eq!(report.strings_written, 1, "{report:?}");
        assert_eq!(report.files_written, vec![path]);
        assert!(plugin
            .extract(dir.path())
            .unwrap()
            .iter()
            .any(|e| e.source == "Fallback translation."));
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
        let path = dir.path().join("package.nw");
        std::fs::write(&path, b"original archive").unwrap();
        let sidecar = dir.path().join("package.nw.locust-old");
        std::fs::write(&sidecar, b"unrelated exact bytes").unwrap();
        let lock = locust_core::patch::GameLock::acquire(dir.path()).unwrap();
        crate::archive_replace::replace_files(&lock, &[(path, b"new archive".to_vec())]).unwrap();
        assert_eq!(std::fs::read(sidecar).unwrap(), b"unrelated exact bytes");
    }
    use super::*;
    use std::fs;

    fn tempdir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("locust_tyrano_{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Minimal TyranoScript-ish scenario (grammar shapes from kag.parser.js).
    fn sample_scenario() -> &'static str {
        "; comment line\r\n\
*start\r\n\
@wait time=100\r\n\
[wait time=50]\r\n\
#akane\r\n\
こんにちは。[r]\r\n\
#表示名\r\n\
This is narration.\r\n\
#表示名:happy\r\n\
[chara_show name=\"akane\"]\r\n\
_  leading underscore text\r\n\
/*\r\n\
block comment body\r\n\
*/\r\n\
@jump target=*end\r\n"
    }

    fn write_tyrano_layout(root: &Path, filename: &str, text: &str, bom: bool) -> PathBuf {
        let scenario = root.join("data").join("scenario");
        fs::create_dir_all(&scenario).unwrap();
        // Engine folder marker (empty is enough for detect).
        fs::create_dir_all(root.join("tyrano")).unwrap();
        let path = scenario.join(filename);
        let mut bytes = Vec::new();
        if bom {
            bytes.extend_from_slice(UTF8_BOM);
        }
        bytes.extend_from_slice(text.as_bytes());
        fs::write(&path, bytes).unwrap();
        path
    }

    #[test]
    fn test_is_bare_identifier() {
        assert!(is_bare_identifier("akane"));
        assert!(is_bare_identifier("chara_01"));
        assert!(is_bare_identifier("Hero-2"));
        assert!(!is_bare_identifier("表示名"));
        assert!(!is_bare_identifier("Alice Smith"));
        assert!(!is_bare_identifier(""));
        assert!(!is_bare_identifier("1hero"));
    }

    #[test]
    fn test_speaker_display_name() {
        assert_eq!(speaker_display_name("#表示名"), Some("表示名"));
        assert_eq!(speaker_display_name("#表示名:happy"), Some("表示名"));
        assert_eq!(speaker_display_name("  #表示名  "), Some("表示名"));
        assert_eq!(speaker_display_name("#akane"), None);
        assert_eq!(speaker_display_name("#akane:happy"), None);
        assert_eq!(speaker_display_name("not a speaker"), None);
    }

    #[test]
    fn test_classify_filters_structural() {
        let text = sample_scenario();
        let kinds = classify_lines(text);
        let lines: Vec<&str> = text
            .split('\n')
            .map(|l| l.strip_suffix('\r').unwrap_or(l))
            .collect();
        assert_eq!(lines.len(), kinds.len());
        for (line, kind) in lines.iter().zip(kinds.iter()) {
            let t = line.trim();
            if t.starts_with(';')
                || t.starts_with('*')
                || t.starts_with('@')
                || t == "[wait time=50]"
                || t == "[chara_show name=\"akane\"]"
                || t == "/*"
                || t == "*/"
                || t == "block comment body"
                || t == "#akane"
                || t.is_empty()
            {
                assert_eq!(*kind, LineKind::Skip, "expected skip for {line:?}");
            }
        }
        assert!(kinds.contains(&LineKind::Text));
        assert!(kinds.contains(&LineKind::Speaker));
    }

    #[test]
    fn test_detect_scenario_layout() {
        let dir = tempdir();
        write_tyrano_layout(&dir, "scene1.ks", sample_scenario(), false);
        assert!(TyranoPlugin::new().detect(&dir));
    }

    #[test]
    fn test_detect_tyrano_dir_without_ks_still_true() {
        let dir = tempdir();
        fs::create_dir_all(dir.join("tyrano")).unwrap();
        fs::create_dir_all(dir.join("data").join("scenario")).unwrap();
        assert!(TyranoPlugin::new().detect(&dir));
    }

    #[test]
    fn test_detect_bare_ks_dir_is_not_tyrano() {
        // Loose .ks without tyrano/ or data/scenario/ → KiriKiri territory.
        let dir = tempdir();
        fs::write(dir.join("scenario.ks"), sample_scenario().as_bytes()).unwrap();
        assert!(!TyranoPlugin::new().detect(&dir));
    }

    #[test]
    fn test_detect_non_tyrano() {
        let dir = tempdir();
        fs::write(dir.join("readme.md"), b"nope").unwrap();
        assert!(!TyranoPlugin::new().detect(&dir));
    }

    #[test]
    fn test_registry_tyrano_before_kirikiri() {
        use crate::kirikiri::KirikiriPlugin;
        use locust_core::extraction::FormatRegistry;

        // Tyrano layout with .ks must not be claimed by KiriKiri.
        let tyrano_dir = tempdir();
        write_tyrano_layout(&tyrano_dir, "scene1.ks", sample_scenario(), false);

        let bare_ks = tempdir();
        fs::write(bare_ks.join("scenario.ks"), sample_scenario().as_bytes()).unwrap();

        // Mimic default_registry order: tyrano then kirikiri.
        let mut reg = FormatRegistry::new();
        reg.register(Box::new(TyranoPlugin::new()));
        reg.register(Box::new(KirikiriPlugin::new()));

        let t = reg.detect(&tyrano_dir).expect("tyrano layout");
        assert_eq!(t.id(), "tyrano", "tyrano-layout must win over kirikiri");

        let k = reg.detect(&bare_ks).expect("bare .ks");
        assert_eq!(k.id(), "kirikiri", "bare .ks must still go to kirikiri");

        // default_registry must also order tyrano before kirikiri.
        let def = crate::default_registry();
        assert_eq!(def.detect(&tyrano_dir).map(|p| p.id()), Some("tyrano"));
        assert_eq!(def.detect(&bare_ks).map(|p| p.id()), Some("kirikiri"));
    }

    #[test]
    fn test_extract_known_lines_and_ids() {
        let dir = tempdir();
        write_tyrano_layout(&dir, "scene1.ks", sample_scenario(), false);
        let plugin = TyranoPlugin::new();
        let entries = plugin.extract(&dir).unwrap();
        let sources: Vec<&str> = entries.iter().map(|e| e.source.as_str()).collect();

        assert!(
            sources.iter().any(|s| s.contains("こんにちは")),
            "missing JP dialogue: {sources:?}"
        );
        assert!(
            sources.iter().any(|s| s.contains("[r]")),
            "inline tags must stay in string: {sources:?}"
        );
        assert!(
            sources.contains(&"This is narration."),
            "missing narration: {sources:?}"
        );
        assert!(
            sources.iter().any(|s| s.contains("leading underscore")),
            "underscore text: {sources:?}"
        );

        // Speakers
        let speakers: Vec<&StringEntry> = entries
            .iter()
            .filter(|e| e.tags.iter().any(|t| t == "speaker"))
            .collect();
        assert!(
            speakers.iter().any(|e| e.source == "表示名"),
            "display name speaker: {:?}",
            speakers.iter().map(|e| &e.source).collect::<Vec<_>>()
        );
        assert!(
            !sources.contains(&"akane"),
            "bare identifier must not be extracted: {sources:?}"
        );

        // Structural excluded
        assert!(
            sources.iter().all(|s| {
                let t = s.trim();
                !t.starts_with(';')
                    && !t.starts_with('*')
                    && !t.starts_with('@')
                    && *s != "[wait time=50]"
                    && *s != "block comment body"
            }),
            "non-text leaked: {sources:?}"
        );

        assert!(
            entries
                .iter()
                .all(|e| e.id.starts_with("data/scenario/scene1.ks#")),
            "ids: {:?}",
            entries.iter().map(|e| &e.id).collect::<Vec<_>>()
        );
        for e in &entries {
            assert!(!e.metadata.contains_key("binary_slot"));
        }
    }

    #[test]
    fn test_inject_roundtrip_no_bom() {
        let dir = tempdir();
        write_tyrano_layout(&dir, "scene1.ks", sample_scenario(), false);
        roundtrip_translate(&dir, "Hola, mundo!", false);
    }

    #[test]
    fn test_inject_roundtrip_with_bom() {
        let dir = tempdir();
        write_tyrano_layout(&dir, "scene1.ks", sample_scenario(), true);
        roundtrip_translate(&dir, "Hola, mundo!", true);
        let bytes = fs::read(dir.join("data/scenario/scene1.ks")).unwrap();
        assert!(
            bytes.starts_with(UTF8_BOM),
            "BOM must be preserved after inject"
        );
    }

    #[test]
    fn test_inject_speaker_display_name() {
        let dir = tempdir();
        write_tyrano_layout(&dir, "scene1.ks", sample_scenario(), false);
        let plugin = TyranoPlugin::new();
        let mut entries = plugin.extract(&dir).unwrap();
        let mut speaker_count = 0usize;
        for e in &mut entries {
            if e.tags.iter().any(|t| t == "speaker") && e.source == "表示名" {
                e.translation = Some("Nombre".into());
                speaker_count += 1;
            }
        }
        assert!(speaker_count >= 2, "expected #表示名 and #表示名:happy");
        let report = plugin.inject(&dir, &entries).unwrap();
        assert!(report.files_modified >= 1, "{report:?}");
        let text = fs::read_to_string(dir.join("data/scenario/scene1.ks")).unwrap();
        assert!(
            text.contains("#Nombre\r\n") || text.contains("#Nombre\n"),
            "rewritten speaker: {text}"
        );
        assert!(
            text.contains("#Nombre:happy"),
            "face suffix preserved: {text}"
        );
        // Bare id unchanged
        assert!(text.contains("#akane"));
        assert!(!text.contains("#表示名"));
    }

    fn roundtrip_translate(dir: &Path, new_narration: &str, expect_bom: bool) {
        let plugin = TyranoPlugin::new();
        let mut entries = plugin.extract(dir).unwrap();
        assert!(!entries.is_empty());
        for e in &mut entries {
            if e.source.contains("This is narration") {
                e.translation = Some(new_narration.to_string());
            }
        }
        let report = plugin.inject(dir, &entries).unwrap();
        assert!(report.files_modified >= 1, "{report:?}");

        let path = dir.join("data/scenario/scene1.ks");
        let bytes = fs::read(&path).unwrap();
        assert_eq!(bytes.starts_with(UTF8_BOM), expect_bom);

        let again = plugin.extract(dir).unwrap();
        assert!(
            again.iter().any(|e| e.source.contains(new_narration)),
            "re-extract missing translation: {:?}",
            again.iter().map(|e| &e.source).collect::<Vec<_>>()
        );
        // JP dialogue preserved
        assert!(again.iter().any(|e| e.source.contains("こんにちは")));
        // Structural still excluded
        assert!(again.iter().all(|e| {
            let t = e.source.trim();
            !t.starts_with(';') && !t.starts_with('*') && !t.starts_with('@')
        }));
    }

    #[test]
    fn test_tyrano_layout_without_scenario_errors_loudly() {
        let dir = tempdir();
        fs::create_dir_all(dir.join("tyrano")).unwrap();
        fs::create_dir_all(dir.join("data").join("scenario")).unwrap();
        // No .ks files
        let err = TyranoPlugin::new().extract(&dir).unwrap_err().to_string();
        assert!(
            err.contains("asar")
                || err.contains("loose")
                || err.contains("no loose")
                || err.contains("out of scope")
                || err.contains("data.exe")
                || err.contains("no TyranoBuilder"),
            "expected loud archive/missing-scenario message, got: {err}"
        );
    }

    #[test]
    fn test_asar_e2e_extract_inject_with_locust_old() {
        let dir = tempdir();
        let resources = dir.join("resources");
        fs::create_dir_all(&resources).unwrap();
        // Optional tyrano marker for detect fallback
        fs::create_dir_all(resources.join("app.asar.unpacked").join("tyrano")).unwrap();

        let asar_bytes = crate::tyrano_asar::write_asar(&[(
            "data/scenario/scene1.ks".into(),
            sample_scenario().as_bytes().to_vec(),
        )])
        .unwrap();
        let asar_path = resources.join("app.asar");
        fs::write(&asar_path, &asar_bytes).unwrap();

        let plugin = TyranoPlugin::new();
        assert!(plugin.detect(&dir));
        let mut entries = plugin.extract(&dir).unwrap();
        assert!(
            entries
                .iter()
                .any(|e| e.source.contains("This is narration")),
            "missing dialogue: {:?}",
            entries.iter().map(|e| &e.source).collect::<Vec<_>>()
        );
        assert!(
            entries.iter().any(|e| e
                .id
                .starts_with("resources/app.asar/data/scenario/scene1.ks#")),
            "ids: {:?}",
            entries.iter().map(|e| &e.id).collect::<Vec<_>>()
        );

        for e in &mut entries {
            if e.source.contains("This is narration") {
                e.translation = Some("Esta es narracion.".into());
            }
        }
        let report = plugin.inject(&dir, &entries).unwrap();
        assert!(report.files_modified >= 1, "{report:?}");

        let backup = backup_path(&report);
        assert!(backup.is_file(), "expected .locust-old at {backup:?}");
        assert!(asar_path.is_file());

        let again = plugin.extract(&dir).unwrap();
        assert!(
            again.iter().any(|e| e.source.contains("narracion")),
            "re-extract missing translation: {:?}",
            again.iter().map(|e| &e.source).collect::<Vec<_>>()
        );
        assert!(again.iter().any(|e| e.source.contains("こんにちは")));
    }

    fn build_nw_zip_bytes(scenario: &str) -> Vec<u8> {
        use std::io::{Cursor, Write as _};
        use zip::write::SimpleFileOptions;
        use zip::{CompressionMethod, ZipWriter};
        let mut buf = Cursor::new(Vec::new());
        {
            let mut z = ZipWriter::new(&mut buf);
            let opts = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
            z.start_file("package.json", opts).unwrap();
            z.write_all(br#"{"name":"t","main":"index.html"}"#).unwrap();
            z.start_file("data/scenario/scene1.ks", opts).unwrap();
            z.write_all(scenario.as_bytes()).unwrap();
            z.start_file("data/other/keep.bin", opts).unwrap();
            z.write_all(b"UNTOUCHED_PAYLOAD_99").unwrap();
            z.finish().unwrap();
        }
        buf.into_inner()
    }

    #[test]
    fn test_package_nw_e2e_extract_inject_locust_old() {
        let dir = tempdir();
        let nw_path = dir.join("package.nw");
        fs::write(&nw_path, build_nw_zip_bytes(sample_scenario())).unwrap();

        let plugin = TyranoPlugin::new();
        assert!(plugin.detect(&dir));
        let mut entries = plugin.extract(&dir).unwrap();
        assert!(
            entries
                .iter()
                .any(|e| e.id.starts_with("package.nw/data/scenario/scene1.ks#")),
            "ids: {:?}",
            entries.iter().map(|e| &e.id).collect::<Vec<_>>()
        );
        for e in &mut entries {
            if e.source.contains("This is narration") {
                e.translation = Some("Esta es narracion NW.".into());
            }
        }
        let report = plugin.inject(&dir, &entries).unwrap();
        assert!(report.files_modified >= 1, "{report:?}");
        let backup = backup_path(&report);
        assert!(backup.is_file(), "expected .locust-old");

        // Untouched asset still present after inject.
        let arch = NwArchive::open(&nw_path).unwrap();
        let asset = arch
            .entries
            .iter()
            .find(|e| e.path == "data/other/keep.bin")
            .unwrap();
        assert_eq!(arch.read_entry(asset).unwrap(), b"UNTOUCHED_PAYLOAD_99");

        let again = plugin.extract(&dir).unwrap();
        assert!(again.iter().any(|e| e.source.contains("narracion NW")));
    }

    #[test]
    fn test_data_exe_e2e_extract_inject() {
        let dir = tempdir();
        let mut exe = b"MZ\x90\x00FAKE_NW_STUB!!!!".to_vec();
        let prefix = exe.clone();
        exe.extend_from_slice(&build_nw_zip_bytes(sample_scenario()));
        let exe_path = dir.join("data.exe");
        fs::write(&exe_path, &exe).unwrap();

        let plugin = TyranoPlugin::new();
        assert!(plugin.detect(&dir));
        let mut entries = plugin.extract(&dir).unwrap();
        assert!(
            entries
                .iter()
                .any(|e| e.id.contains("data.exe/data/scenario/scene1.ks")),
            "ids: {:?}",
            entries.iter().map(|e| &e.id).collect::<Vec<_>>()
        );
        for e in &mut entries {
            if e.source.contains("This is narration") {
                e.translation = Some("Narracion en exe.".into());
            }
        }
        let report = plugin.inject(&dir, &entries).unwrap();
        let out = fs::read(&exe_path).unwrap();
        assert!(
            out.starts_with(&prefix),
            "exe prefix must be preserved after inject"
        );
        let backup = backup_path(&report);
        assert!(backup.is_file());
        let again = plugin.extract(&dir).unwrap();
        assert!(again.iter().any(|e| e.source.contains("Narracion en exe")));
    }

    #[test]
    fn repeated_injection_keeps_every_prior_backup() {
        let dir = tempdir();
        let path = dir.join("package.nw");
        let original = build_nw_zip_bytes(sample_scenario());
        fs::write(&path, &original).unwrap();
        let plugin = TyranoPlugin::new();
        let mut entries = plugin.extract(&dir).unwrap();
        for e in &mut entries {
            if e.source.contains("This is narration") {
                e.translation = Some("First replacement".into());
            }
        }
        let first = plugin.inject(&dir, &entries).unwrap();
        let backup = backup_path(&first);
        let first_bytes = fs::read(&path).unwrap();
        let mut entries = plugin.extract(&dir).unwrap();
        for e in &mut entries {
            if e.source.contains("First replacement") {
                e.translation = Some("Second replacement".into());
            }
        }
        let second = plugin.inject(&dir, &entries).unwrap();
        let second_backup = backup_path(&second);
        assert_ne!(backup, second_backup);
        assert_eq!(fs::read(backup).unwrap(), original);
        assert_eq!(fs::read(second_backup).unwrap(), first_bytes);
        assert!(plugin
            .extract(&dir)
            .unwrap()
            .iter()
            .any(|e| e.source == "Second replacement"));
    }

    #[test]
    fn test_stability_experimental() {
        assert_eq!(
            TyranoPlugin::new().stability(),
            locust_core::extraction::FormatStability::Experimental
        );
    }

    #[test]
    fn test_rebuild_speaker_line() {
        assert_eq!(rebuild_speaker_line("#表示名", "Name"), "#Name");
        assert_eq!(rebuild_speaker_line("#表示名:happy", "Name"), "#Name:happy");
        assert_eq!(
            rebuild_speaker_line("  #表示名:happy", "Name"),
            "  #Name:happy"
        );
    }
}
