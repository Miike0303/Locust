//! NScripter / ONScripter script plugin — Experimental (synthetic fixtures).
//!
//! # Spec sources (do not invent transforms)
//! - Container open order + XOR decrypt:
//!   https://github.com/ogapee/onscripter/blob/master/ScriptHandler.cpp
//!   (`ScriptHandler::readScript`, `ScriptHandler::readScriptSub`)
//!   Priority: `0.txt` → `00.txt` → `nscr_sec.dat` → `nscript.___` → `nscript.dat`
//!   (also `pscript.dat` UTF-8 path exists in ONScripter — out of scope here).
//!   - `encrypt_mode == 1` (`nscript.dat`): every byte `ch ^= 0x84`.
//!   - `encrypt_mode == 2` (`nscr_sec.dat`): rotating XOR
//!     `ch ^= magic[i % 5]` with `magic = {0x79, 0x57, 0x0D, 0x80, 0x04}`;
//!     counter resets at file start (self-inverse).
//! - Token classes (`readToken`): high-bit first char (`ch & 0x80`) and backtick
//!   `` ` `` start dialogue; ASCII-letter lines are commands; `*` labels; `;` comments.
//!
//! # Dialogue and literal command fields
//! After Shift-JIS decode, a line is dialogue if the first non-space char is
//! non-ASCII (SJIS lead ≥ 0x80 after decode) or a backtick. Inline wait markers
//! (`@`, `\`, `/` at EOL) and furigana stay inside the extracted string.
//! Leading `caption` and `rmenu` commands also expose their quoted display
//! values. `captionCommand` reads one string; `rmenuCommand` alternates strings
//! and dispatch labels. Expressions and malformed argument lists are excluded.
//! Injection splices only the selected byte ranges before reapplying container
//! XOR, preserving separators, dispatch names, and original newline bytes.
//!
//! Out of scope: `nscript.___` (mode-3 key table from EXE / `--key-exe`), multi-file
//! `1.txt`…`99.txt` concat, `pscript.dat` UTF-8, `arc.nsa` / `.sar` archive unpack,
//! real commercial game fixtures.

use std::borrow::Cow;
use std::collections::HashMap;
use std::ops::Range;
use std::path::{Path, PathBuf};

use locust_core::backup::RevisionOriginal;
use locust_core::error::Result;
use locust_core::extraction::{FormatPlugin, InjectionReport};
use locust_core::models::{OutputMode, StringEntry};

use crate::util::parse_err;

/// ONScripter `nscript.dat` / encrypt_mode 1 XOR constant (`readScriptSub`).
const NSCRIPT_DAT_XOR: u8 = 0x84;

/// ONScripter `nscr_sec.dat` / encrypt_mode 2 rotating XOR key (`readScriptSub`).
const NSCR_SEC_MAGIC: [u8; 5] = [0x79, 0x57, 0x0D, 0x80, 0x04];

/// Supported script containers, highest priority first (engine `readScript` order).
const SUPPORTED_CONTAINERS: &[&str] = &["0.txt", "00.txt", "nscr_sec.dat", "nscript.dat"];

/// Present in engine priority but not implemented in this cut.
const UNSUPPORTED_CONTAINERS: &[&str] = &["nscript.___"];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ContainerKind {
    /// Plain Shift-JIS text (`0.txt` / `00.txt`).
    Plain,
    /// Byte-wise XOR 0x84 then Shift-JIS (`nscript.dat`).
    Xor84,
    /// Rotating 5-byte XOR then Shift-JIS (`nscr_sec.dat`).
    XorRot5,
}

impl ContainerKind {
    fn from_name(name: &str) -> Option<Self> {
        match name {
            "0.txt" | "00.txt" => Some(Self::Plain),
            "nscript.dat" => Some(Self::Xor84),
            "nscr_sec.dat" => Some(Self::XorRot5),
            _ => None,
        }
    }
}

#[derive(Clone, Debug)]
struct SelectedContainer {
    /// On-disk file name used in ids (`0.txt`, `nscript.dat`, …).
    name: String,
    path: PathBuf,
    kind: ContainerKind,
}

pub struct NScripterPlugin;

impl NScripterPlugin {
    pub fn new() -> Self {
        Self
    }

    fn is_nsa(path: &Path) -> bool {
        path.extension()
            .and_then(|e| e.to_str())
            .map(|e| e.eq_ignore_ascii_case("nsa"))
            .unwrap_or(false)
    }

    fn root_dir(path: &Path) -> PathBuf {
        if path.is_dir() {
            path.to_path_buf()
        } else {
            path.parent().unwrap_or(path).to_path_buf()
        }
    }

    fn file_in_root(root: &Path, name: &str) -> Option<PathBuf> {
        let p = root.join(name);
        if p.is_file() {
            return Some(p);
        }
        // Case-insensitive fallback (Windows ships mixed case rarely).
        if let Ok(entries) = std::fs::read_dir(root) {
            for e in entries.flatten() {
                let p = e.path();
                if p.is_file() {
                    if let Some(fname) = p.file_name().and_then(|n| n.to_str()) {
                        if fname.eq_ignore_ascii_case(name) {
                            return Some(p);
                        }
                    }
                }
            }
        }
        None
    }

    fn has_nsa(root: &Path) -> bool {
        if root.is_file() {
            return Self::is_nsa(root);
        }
        if !root.is_dir() {
            return false;
        }
        std::fs::read_dir(root)
            .into_iter()
            .flatten()
            .flatten()
            .any(|e| e.path().is_file() && Self::is_nsa(&e.path()))
    }

    /// True when the container the ENGINE would load is one we don't support.
    /// `nscript.___` outranks `nscript.dat` in readScript order, so its mere
    /// presence must not be masked by a lower-priority `nscript.dat`.
    fn engine_picks_unsupported(root: &Path) -> bool {
        let has_unsupported = UNSUPPORTED_CONTAINERS
            .iter()
            .any(|n| Self::file_in_root(root, n).is_some());
        // Only containers that outrank nscript.___ in engine order suppress it.
        let higher_supported = ["0.txt", "00.txt", "nscr_sec.dat"]
            .iter()
            .any(|n| Self::file_in_root(root, n).is_some());
        has_unsupported && !higher_supported
    }

    /// Pick the highest-priority supported container (engine order).
    fn select_container(path: &Path) -> Option<SelectedContainer> {
        if path.is_file() {
            let name = path.file_name()?.to_str()?;
            let kind = ContainerKind::from_name(
                SUPPORTED_CONTAINERS
                    .iter()
                    .find(|n| name.eq_ignore_ascii_case(n))
                    .copied()
                    .unwrap_or(name),
            )?;
            // Normalize to canonical lower names we use in ids.
            let canon = SUPPORTED_CONTAINERS
                .iter()
                .find(|n| name.eq_ignore_ascii_case(n))
                .copied()
                .unwrap_or(name)
                .to_string();
            return Some(SelectedContainer {
                name: canon,
                path: path.to_path_buf(),
                kind,
            });
        }
        let root = path;
        for name in SUPPORTED_CONTAINERS {
            if let Some(p) = Self::file_in_root(root, name) {
                let kind = ContainerKind::from_name(name)?;
                return Some(SelectedContainer {
                    name: (*name).to_string(),
                    path: p,
                    kind,
                });
            }
        }
        None
    }

    fn detect_path(path: &Path) -> bool {
        if path.is_file() {
            if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                if SUPPORTED_CONTAINERS
                    .iter()
                    .any(|n| name.eq_ignore_ascii_case(n))
                    || UNSUPPORTED_CONTAINERS
                        .iter()
                        .any(|n| name.eq_ignore_ascii_case(n))
                    || Self::is_nsa(path)
                {
                    return true;
                }
            }
            return false;
        }
        if !path.is_dir() {
            return false;
        }
        SUPPORTED_CONTAINERS
            .iter()
            .any(|n| Self::file_in_root(path, n).is_some())
            || UNSUPPORTED_CONTAINERS
                .iter()
                .any(|n| Self::file_in_root(path, n).is_some())
            || Self::has_nsa(path)
    }
}

impl Default for NScripterPlugin {
    fn default() -> Self {
        Self::new()
    }
}

// ─── Decode / encode ───────────────────────────────────────────────────────

fn xor_bytes(data: &[u8], key: u8) -> Vec<u8> {
    data.iter().map(|b| b ^ key).collect()
}

/// ONScripter encrypt_mode 2: `ch ^= magic[i % 5]` (self-inverse).
fn xor_rot5_bytes(data: &[u8]) -> Vec<u8> {
    data.iter()
        .enumerate()
        .map(|(i, b)| b ^ NSCR_SEC_MAGIC[i % 5])
        .collect()
}

fn decode_container(bytes: &[u8], kind: ContainerKind, file_label: &str) -> Result<(String, bool)> {
    let decoded_owned;
    let plain: &[u8] = match kind {
        ContainerKind::Plain => bytes,
        ContainerKind::Xor84 => {
            decoded_owned = xor_bytes(bytes, NSCRIPT_DAT_XOR);
            &decoded_owned
        }
        ContainerKind::XorRot5 => {
            decoded_owned = xor_rot5_bytes(bytes);
            &decoded_owned
        }
    };
    let (cow, _, had_errors) = encoding_rs::SHIFT_JIS.decode(plain);
    if had_errors {
        return Err(parse_err(
            file_label,
            "could not decode NScripter script as Shift-JIS",
        ));
    }
    let text = cow.into_owned();
    let crlf = text.contains("\r\n");
    Ok((text, crlf))
}

/// Both supported XOR transforms are self-inverse. Work on the original plain
/// bytes so untouched CP932 aliases and mixed newline styles survive injection.
fn transform_container(bytes: &[u8], kind: ContainerKind) -> Vec<u8> {
    match kind {
        ContainerKind::Plain => bytes.to_vec(),
        ContainerKind::Xor84 => xor_bytes(bytes, NSCRIPT_DAT_XOR),
        ContainerKind::XorRot5 => xor_rot5_bytes(bytes),
    }
}

// ─── Line classification ───────────────────────────────────────────────────

/// First non-space byte ≥ 0x80 (SJIS lead) or backtick → player text.
/// Operates on decoded Unicode: non-ASCII first char covers SJIS lead bytes.
fn is_player_text_line(line: &str) -> bool {
    let t = line.trim_start_matches([' ', '\t']);
    match t.chars().next() {
        Some('`') => true,
        Some(c) => !c.is_ascii(),
        None => false,
    }
}

/// Keep ASCII translations in dialogue mode without changing their indentation.
fn normalize_dialogue_translation(text: &str) -> Cow<'_, str> {
    let trimmed = text.trim_start_matches([' ', '\t']);
    match trimmed.chars().next() {
        Some(c) if c.is_ascii() && c != '`' && !text.trim().is_empty() => {
            let mut normalized = text.to_owned();
            normalized.insert(text.len() - trimmed.len(), '`');
            Cow::Owned(normalized)
        }
        _ => Cow::Borrowed(text),
    }
}

#[derive(Debug)]
struct DisplayField {
    command: &'static str,
    argument: usize,
    range: Range<usize>,
}

/// Scan a leading literal caption or an entire literal rmenu argument list.
/// Engine authority: readToken, readStr/parseStr, readLabel and checkComma.
/// Quotes have no backslash escape. Expressions/aliases and malformed lists
/// are deliberately excluded, rather than exposing partial runtime values.
/// All syntax bytes are below 0x40 (or outside quoted strings), so this also
/// works on original Shift-JIS bytes without confusing multibyte trail bytes.
fn display_fields(line: &[u8]) -> Vec<DisplayField> {
    fn space(line: &[u8], p: &mut usize) {
        while matches!(line.get(*p), Some(b' ' | b'\t')) {
            *p += 1;
        }
    }
    fn end(line: &[u8], p: usize) -> bool {
        matches!(line.get(p), None | Some(b';' | b':'))
    }
    fn quoted(line: &[u8], p: &mut usize) -> Option<Range<usize>> {
        if line.get(*p) != Some(&b'"') {
            return None;
        }
        *p += 1;
        let start = *p;
        while !matches!(line.get(*p), None | Some(b'"' | b'\r' | b'\n' | 0)) {
            *p += 1;
        }
        if line.get(*p) != Some(&b'"') {
            return None;
        }
        let range = start..*p;
        *p += 1;
        space(line, p);
        Some(range)
    }
    let mut p = 0;
    space(line, &mut p);
    let start = p;
    while line
        .get(p)
        .is_some_and(|b| b.is_ascii_alphanumeric() || *b == b'_')
    {
        p += 1;
    }
    let command = if line[start..p].eq_ignore_ascii_case(b"caption") {
        "caption"
    } else if line[start..p].eq_ignore_ascii_case(b"rmenu") {
        "rmenu"
    } else {
        return Vec::new();
    };
    space(line, &mut p);
    let mut fields = Vec::new();
    loop {
        let Some(range) = quoted(line, &mut p) else {
            return Vec::new();
        };
        fields.push(DisplayField {
            command,
            argument: fields.len() * 2,
            range,
        });
        if command == "caption" {
            return if end(line, p) { fields } else { Vec::new() };
        }
        if line.get(p) != Some(&b',') {
            return Vec::new();
        }
        p += 1;
        space(line, &mut p);
        // readLabel: dispatch identifiers, never display strings.
        if line.get(p) == Some(&b'*') {
            p += 1;
            space(line, &mut p);
        }
        if !line
            .get(p)
            .is_some_and(|b| b.is_ascii_alphabetic() || *b == b'_')
        {
            return Vec::new();
        }
        while line
            .get(p)
            .is_some_and(|b| b.is_ascii_alphanumeric() || *b == b'_')
        {
            p += 1;
        }
        space(line, &mut p);
        if end(line, p) {
            return fields;
        }
        if line.get(p) != Some(&b',') {
            return Vec::new();
        }
        p += 1;
        space(line, &mut p);
    }
}

fn field_locator(field: &DisplayField) -> String {
    format!("{}:{}", field.command, field.argument)
}

fn entry_locator<'a>(id: &'a str, name: &str) -> Option<(usize, Option<&'a str>)> {
    let (file, locator) = id.split_once('#')?;
    if !file.eq_ignore_ascii_case(name) {
        return None;
    }
    let (line, field) = match locator.split_once(':') {
        Some((line, field)) => (line, Some(field)),
        None => (locator, None),
    };
    Some((line.parse::<usize>().ok()?.checked_sub(1)?, field))
}

fn line_skeleton<'a>(line: &'a [u8], fields: &[DisplayField]) -> Vec<&'a [u8]> {
    let mut start = 0;
    let mut parts = Vec::new();
    for field in fields {
        parts.push(&line[start..field.range.start]);
        start = field.range.end;
    }
    parts.push(&line[start..]);
    parts
}

// ─── Plugin ────────────────────────────────────────────────────────────────

impl FormatPlugin for NScripterPlugin {
    fn id(&self) -> &str {
        "nscripter"
    }

    fn name(&self) -> &str {
        "NScripter / ONScripter"
    }

    fn description(&self) -> &str {
        "NScripter scripts (0.txt / 00.txt; nscr_sec.dat rot5 XOR; nscript.dat XOR 0x84) — synthetic"
    }

    fn stability(&self) -> locust_core::extraction::FormatStability {
        locust_core::extraction::FormatStability::Experimental
    }

    fn supported_extensions(&self) -> &[&str] {
        &[".txt", ".dat", ".nsa"]
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

        // Engine priority: nscript.___ outranks nscript.dat. If the engine would
        // load the unsupported container, extracting nscript.dat would translate
        // a file the game never reads — error loudly instead.
        if Self::engine_picks_unsupported(&root) {
            return Err(parse_err(
                &label,
                "engine would load nscript.___ (unsupported mode-3 key table); \
                 this Experimental cut supports 0.txt, 00.txt, nscr_sec.dat, and nscript.dat \
                 (nscript.___ needs --key-exe / key table)",
            ));
        }
        let Some(selected) = Self::select_container(path) else {
            if Self::has_nsa(&root) {
                return Err(parse_err(
                    &label,
                    "arc.nsa / .nsa archive present but no supported script container \
                     (0.txt / 00.txt / nscr_sec.dat / nscript.dat); archive unpack is out of scope",
                ));
            }
            return Err(parse_err(
                &label,
                "no NScripter script container found \
                 (expected 0.txt, 00.txt, nscr_sec.dat, or nscript.dat)",
            ));
        };

        let bytes = std::fs::read(&selected.path)?;
        let (text, _crlf) = decode_container(&bytes, selected.kind, &selected.name)?;

        let mut all = Vec::new();
        for (idx, line) in text.split('\n').enumerate() {
            let line = line.strip_suffix('\r').unwrap_or(line);
            let line_no = idx + 1;
            if !is_player_text_line(line) {
                for field in display_fields(line.as_bytes()) {
                    let id = format!("{}#{}:{}", selected.name, line_no, field_locator(&field));
                    let mut entry =
                        StringEntry::new(id, &line[field.range.clone()], selected.path.clone());
                    entry.tags = vec![field.command.into()];
                    entry
                        .metadata
                        .insert("nscripter_line".into(), serde_json::json!(line));
                    all.push(entry);
                }
                continue;
            }
            let id = format!("{}#{}", selected.name, line_no);
            let mut entry = StringEntry::new(id, line, selected.path.clone());
            entry.tags = vec!["dialogue".into()];
            all.push(entry);
        }
        Ok(all)
    }

    fn prepare_revision_entries(
        &self,
        entries: &mut [StringEntry],
        originals: &HashMap<PathBuf, RevisionOriginal>,
    ) -> Result<()> {
        // Core supplies these keys only for a verified prior Direct output with
        // a verified pristine backup. RevisionOriginal's text reader is UTF-8
        // only, so use the extraction snapshot to validate source/field identity
        // and all non-value bytes on the line. Never relax normal inject checks.
        for current_path in originals.keys() {
            let Some(name) = current_path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            let Some(kind) = ContainerKind::from_name(&name.to_ascii_lowercase()) else {
                continue;
            };
            let bytes = std::fs::read(current_path)?;
            let (current, _) = decode_container(&bytes, kind, name)?;
            let lines: Vec<_> = current
                .split('\n')
                .map(|l| l.strip_suffix('\r').unwrap_or(l))
                .collect();
            for entry in entries.iter_mut().filter(|e| e.file_path == *current_path) {
                let Some((line_no, Some(locator))) = entry_locator(&entry.id, name) else {
                    continue;
                };
                let Some(old) = entry
                    .metadata
                    .get("nscripter_line")
                    .and_then(|v| v.as_str())
                else {
                    continue;
                };
                let Some(new) = lines.get(line_no) else {
                    continue;
                };
                let old_fields = display_fields(old.as_bytes());
                let new_fields = display_fields(new.as_bytes());
                if old_fields.len() != new_fields.len()
                    || line_skeleton(old.as_bytes(), &old_fields)
                        != line_skeleton(new.as_bytes(), &new_fields)
                {
                    continue;
                }
                for (a, b) in old_fields.iter().zip(&new_fields) {
                    if field_locator(a) == locator && entry.source == old[a.range.clone()] {
                        entry.source = new[b.range.clone()].to_owned();
                        entry
                            .metadata
                            .insert("nscripter_line".into(), serde_json::json!(new));
                        break;
                    }
                }
            }
        }
        Ok(())
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
        for (file_path, file_entries) in by_file {
            let actual = if file_path.exists() {
                file_path.clone()
            } else {
                search_root.join(file_path.file_name().unwrap_or_default())
            };
            let name = actual
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("script");
            let Some(kind) = ContainerKind::from_name(&name.to_ascii_lowercase()) else {
                report.skip("unsupported_container", file_entries.len());
                report.warnings.push(format!(
                    "unsupported container for inject: {}",
                    actual.display()
                ));
                continue;
            };
            let bytes = match std::fs::read(&actual) {
                Ok(bytes) => bytes,
                Err(error) => {
                    report.skip("read_error", file_entries.len());
                    report
                        .warnings
                        .push(format!("read {}: {error}", actual.display()));
                    continue;
                }
            };
            if let Err(error) = decode_container(&bytes, kind, name) {
                report.skip("decode_error", file_entries.len());
                report
                    .warnings
                    .push(format!("cannot decode {}: {error}", actual.display()));
                continue;
            }
            let plain = transform_container(&bytes, kind);
            let mut start = 0;
            let lines: Vec<_> = plain
                .split(|b| *b == b'\n')
                .map(|line| {
                    let offset = start;
                    start += line.len() + 1;
                    (offset, line.strip_suffix(b"\r").unwrap_or(line))
                })
                .collect();
            let mut counts = HashMap::new();
            for e in &file_entries {
                if e.translation.is_some() {
                    *counts.entry(e.id.as_str()).or_insert(0usize) += 1;
                }
            }
            let mut edits: Vec<(Range<usize>, Vec<u8>)> = Vec::new();
            for entry in file_entries {
                let Some(translation) = entry.translation.as_deref() else {
                    report.skip("missing_translation", 1);
                    continue;
                };
                // Resolve every row against the unmodified file, before changing
                // any lengths. Multiple equal labels therefore cannot drift.
                let replacement = (|| -> std::result::Result<_, &'static str> {
                    if counts.get(entry.id.as_str()).copied().unwrap_or(0) != 1 {
                        return Err("ambiguous_locator");
                    }
                    let (line_no, locator) =
                        entry_locator(&entry.id, name).ok_or("invalid_locator")?;
                    let (offset, raw) = lines.get(line_no).ok_or("invalid_locator")?;
                    let (line, _, _) = encoding_rs::SHIFT_JIS.decode(raw);
                    let (range, value) = if let Some(locator) = locator {
                        let fields = display_fields(raw);
                        let field = fields
                            .iter()
                            .find(|f| field_locator(f) == locator)
                            .ok_or("invalid_locator")?;
                        let (source, _, _) =
                            encoding_rs::SHIFT_JIS.decode(&raw[field.range.clone()]);
                        if source != entry.source {
                            return Err("source_mismatch");
                        }
                        if entry
                            .metadata
                            .get("nscripter_line")
                            .and_then(|v| v.as_str())
                            != Some(line.as_ref())
                        {
                            return Err("line_mismatch");
                        }
                        // parseStr terminates at the first quote, with no escape.
                        if translation.contains(['"', '\r', '\n', '\0']) {
                            return Err("unsafe_quoted_value");
                        }
                        (field.range.clone(), Cow::Borrowed(translation))
                    } else {
                        if !is_player_text_line(&line) {
                            return Err("not_dialogue");
                        }
                        (0..raw.len(), normalize_dialogue_translation(translation))
                    };
                    let (encoded, _, errors) = encoding_rs::SHIFT_JIS.encode(&value);
                    if errors {
                        return Err("not_encodable_as_Shift-JIS");
                    }
                    if raw[range.clone()] == *encoded {
                        return Err("unchanged");
                    }
                    Ok((
                        offset + range.start..offset + range.end,
                        encoded.into_owned(),
                    ))
                })();
                match replacement {
                    Ok(edit) => edits.push(edit),
                    Err(reason) => {
                        report.skip(reason, 1);
                        if reason != "unchanged" {
                            report
                                .warnings
                                .push(format!("{}: {reason}; skipped", entry.id));
                        }
                    }
                }
            }
            if edits.is_empty() {
                continue;
            }
            edits.sort_by_key(|(range, _)| range.start);
            if edits.windows(2).any(|w| w[0].0.end > w[1].0.start) {
                report.skip("overlapping_locators", edits.len());
                report
                    .warnings
                    .push(format!("{name}: overlapping locators; skipped"));
                continue;
            }
            let mut out = Vec::new();
            let mut cursor = 0;
            for (range, replacement) in &edits {
                out.extend_from_slice(&plain[cursor..range.start]);
                out.extend_from_slice(replacement);
                cursor = range.end;
            }
            out.extend_from_slice(&plain[cursor..]);
            std::fs::write(&actual, transform_container(&out, kind))?;
            report.files_modified += 1;
            report.files_written.push(actual);
            report.strings_written += edits.len();
        }
        Ok(report)
    }
}

// ─── Tests (synthetic fixtures only) ───────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn tempdir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("locust_nscr_{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Minimal NScripter-ish script with code + JP dialogue + English backtick + waits.
    fn sample_script() -> &'static str {
        "; comment line\r\n\
*start\r\n\
bg black,0\r\n\
wait 30\r\n\
`Hello, world!`\r\n\
こんにちは。@\r\n\
ナレーションです\\\r\n\
goto *end\r\n\
*end\r\n\
click"
    }

    fn encode_sjis_fixture(text: &str) -> Vec<u8> {
        let (bytes, _, err) = encoding_rs::SHIFT_JIS.encode(text);
        assert!(!err, "fixture must encode as Shift-JIS");
        bytes.into_owned()
    }

    fn write_0_txt(dir: &Path, text: &str) {
        fs::write(dir.join("0.txt"), encode_sjis_fixture(text)).unwrap();
    }

    fn write_nscript_dat(dir: &Path, text: &str) {
        let plain = encode_sjis_fixture(text);
        let xored = xor_bytes(&plain, NSCRIPT_DAT_XOR);
        fs::write(dir.join("nscript.dat"), xored).unwrap();
    }

    fn write_nscr_sec(dir: &Path, text: &str) {
        let plain = encode_sjis_fixture(text);
        let xored = xor_rot5_bytes(&plain);
        fs::write(dir.join("nscr_sec.dat"), xored).unwrap();
    }

    #[test]
    fn test_xor84_self_inverse() {
        let data = b"hello nscript";
        let once = xor_bytes(data, NSCRIPT_DAT_XOR);
        assert_ne!(once.as_slice(), data.as_slice());
        let twice = xor_bytes(&once, NSCRIPT_DAT_XOR);
        assert_eq!(twice.as_slice(), data.as_slice());
        // Spot-check constant matches ONScripter ScriptHandler.cpp readScriptSub.
        assert_eq!(NSCRIPT_DAT_XOR, 0x84);
    }

    #[test]
    fn test_xor_rot5_self_inverse_and_magic() {
        assert_eq!(NSCR_SEC_MAGIC, [0x79, 0x57, 0x0D, 0x80, 0x04]);
        let data = b"hello nscr_sec rotating key!!";
        let once = xor_rot5_bytes(data);
        assert_ne!(once.as_slice(), data.as_slice());
        // First five bytes use distinct key bytes.
        assert_eq!(once[0], data[0] ^ 0x79);
        assert_eq!(once[1], data[1] ^ 0x57);
        assert_eq!(once[5], data[5] ^ 0x79);
        let twice = xor_rot5_bytes(&once);
        assert_eq!(twice.as_slice(), data.as_slice());
    }

    #[test]
    fn test_is_player_text_line() {
        assert!(is_player_text_line("`English`"));
        assert!(is_player_text_line("  `padded`"));
        assert!(is_player_text_line("こんにちは。@"));
        assert!(is_player_text_line("「台詞」/"));
        assert!(!is_player_text_line("; comment"));
        assert!(!is_player_text_line("*start"));
        assert!(!is_player_text_line("bg black,0"));
        assert!(!is_player_text_line("wait 30"));
        assert!(!is_player_text_line("goto *end"));
        assert!(!is_player_text_line(""));
        assert!(!is_player_text_line("   "));
    }

    #[test]
    fn test_detect_0_txt_dir() {
        let dir = tempdir();
        write_0_txt(&dir, sample_script());
        assert!(NScripterPlugin::new().detect(&dir));
    }

    #[test]
    fn test_detect_nscript_dat_dir() {
        let dir = tempdir();
        write_nscript_dat(&dir, sample_script());
        assert!(NScripterPlugin::new().detect(&dir));
    }

    #[test]
    fn test_detect_arc_nsa_only_still_true() {
        let dir = tempdir();
        fs::write(dir.join("arc.nsa"), b"NSA\0fake").unwrap();
        assert!(NScripterPlugin::new().detect(&dir));
    }

    #[test]
    fn test_detect_non_nscripter() {
        let dir = tempdir();
        fs::write(dir.join("readme.md"), b"nope").unwrap();
        assert!(!NScripterPlugin::new().detect(&dir));
    }

    #[test]
    fn test_extract_0_txt_filters_and_ids() {
        let dir = tempdir();
        write_0_txt(&dir, sample_script());
        let plugin = NScripterPlugin::new();
        let entries = plugin.extract(&dir).unwrap();
        let sources: Vec<&str> = entries.iter().map(|e| e.source.as_str()).collect();

        assert!(
            sources.iter().any(|s| s.contains("Hello, world")),
            "missing backtick dialogue: {sources:?}"
        );
        assert!(
            sources.iter().any(|s| s.contains("こんにちは")),
            "missing JP dialogue: {sources:?}"
        );
        assert!(
            sources.iter().any(|s| s.contains("ナレーション")),
            "missing narration with wait marker: {sources:?}"
        );
        // Wait markers stay inside the string
        assert!(
            sources
                .iter()
                .any(|s| s.ends_with('@') || s.contains("。@")),
            "expected @ wait marker kept: {sources:?}"
        );
        assert!(
            sources.iter().any(|s| s.contains('\\')),
            "expected \\ wait marker kept: {sources:?}"
        );

        // Code lines must not leak
        assert!(
            sources.iter().all(|s| !s.starts_with(';')
                && !s.starts_with('*')
                && !s.starts_with("bg ")
                && !s.starts_with("wait ")
                && !s.starts_with("goto ")
                && *s != "click"),
            "non-text leaked: {sources:?}"
        );

        assert!(
            entries.iter().all(|e| e.id.starts_with("0.txt#")),
            "ids: {:?}",
            entries.iter().map(|e| &e.id).collect::<Vec<_>>()
        );
        for e in &entries {
            assert!(!e.metadata.contains_key("binary_slot"));
        }
    }

    #[test]
    fn test_extract_nscript_dat() {
        let dir = tempdir();
        write_nscript_dat(&dir, sample_script());
        let entries = NScripterPlugin::new().extract(&dir).unwrap();
        assert!(
            entries.iter().any(|e| e.source.contains("こんにちは")),
            "xor-decoded extract failed: {:?}",
            entries.iter().map(|e| &e.source).collect::<Vec<_>>()
        );
        assert!(entries.iter().all(|e| e.id.starts_with("nscript.dat#")));
    }

    #[test]
    fn test_priority_prefers_0_txt_over_nscript_dat() {
        let dir = tempdir();
        write_0_txt(&dir, "`from zero`\r\n");
        write_nscript_dat(&dir, "`from dat`\r\n");
        let entries = NScripterPlugin::new().extract(&dir).unwrap();
        assert!(
            entries.iter().any(|e| e.source.contains("from zero")),
            "expected 0.txt win: {:?}",
            entries.iter().map(|e| &e.source).collect::<Vec<_>>()
        );
        assert!(entries.iter().all(|e| !e.source.contains("from dat")));
    }

    #[test]
    fn test_inject_roundtrip_0_txt() {
        let dir = tempdir();
        write_0_txt(&dir, sample_script());
        roundtrip_translate(&dir, "`Hola, mundo!`");
    }

    fn assert_japanese_to_ascii_stays_dialogue(dir: &Path) {
        let plugin = NScripterPlugin::new();
        let mut entries = plugin.extract(dir).unwrap();
        assert_eq!(entries.len(), 1);
        entries[0].translation = Some("Hello.@".into());

        let report = plugin.inject(dir, &entries).unwrap();
        assert_eq!(report.files_modified, 1, "{report:?}");
        assert_eq!(report.strings_written, 1, "{report:?}");
        assert_eq!(report.strings_skipped, 0, "{report:?}");
        assert!(report.warnings.is_empty(), "{report:?}");

        let again = plugin.extract(dir).unwrap();
        assert_eq!(again.len(), 1, "ASCII translation must remain dialogue");
        assert_eq!(again[0].source, "`Hello.@");
        assert_eq!(again[0].id, entries[0].id);
    }

    #[test]
    fn japanese_to_ascii_stays_dialogue() {
        let dir = tempdir();
        write_0_txt(&dir, "こんにちは。@\n");
        assert_japanese_to_ascii_stays_dialogue(&dir);
        assert_eq!(fs::read(dir.join("0.txt")).unwrap(), b"`Hello.@\n");
    }

    #[test]
    fn japanese_to_ascii_stays_dialogue_nscr_sec() {
        let dir = tempdir();
        write_nscr_sec(&dir, "こんにちは。@\n");
        assert_japanese_to_ascii_stays_dialogue(&dir);
        assert_eq!(
            fs::read(dir.join("nscr_sec.dat")).unwrap(),
            xor_rot5_bytes(b"`Hello.@\n")
        );
    }

    #[test]
    fn japanese_to_ascii_stays_dialogue_nscript_dat() {
        let dir = tempdir();
        write_nscript_dat(&dir, "こんにちは。@\n");
        assert_japanese_to_ascii_stays_dialogue(&dir);
        assert_eq!(
            fs::read(dir.join("nscript.dat")).unwrap(),
            xor_bytes(b"`Hello.@\n", NSCRIPT_DAT_XOR)
        );
    }

    #[test]
    fn test_inject_preserves_dialogue_prefixes_and_indentation() {
        for (translation, expected) in [
            ("`Hi", "`Hi"),
            ("さようなら。@", "さようなら。@"),
            ("  Hi", "  `Hi"),
            (" \t Hi", " \t `Hi"),
            (" \t `Hi", " \t `Hi"),
            (" \t さようなら。@", " \t さようなら。@"),
            ("123", "`123"),
            (";Hi", "`;Hi"),
        ] {
            let dir = tempdir();
            write_0_txt(&dir, "こんにちは。@\n");
            let plugin = NScripterPlugin::new();
            let mut entries = plugin.extract(&dir).unwrap();
            entries[0].translation = Some(translation.into());

            let report = plugin.inject(&dir, &entries).unwrap();
            assert_eq!(report.strings_written, 1, "{translation:?}: {report:?}");
            assert_eq!(report.strings_skipped, 0, "{translation:?}: {report:?}");
            assert!(report.warnings.is_empty(), "{report:?}");
            assert_eq!(
                fs::read(dir.join("0.txt")).unwrap(),
                encode_sjis_fixture(&format!("{expected}\n")),
                "{translation:?}"
            );
            let again = plugin.extract(&dir).unwrap();
            assert_eq!(again.len(), 1, "{translation:?}");
            assert_eq!(again[0].source, expected, "{translation:?}");
        }
    }

    #[test]
    fn test_inject_keeps_empty_and_whitespace_translations() {
        for translation in ["", "   ", "\t", " \t ", " \t\n"] {
            let dir = tempdir();
            write_0_txt(&dir, "こんにちは。@\n");
            let plugin = NScripterPlugin::new();
            let mut entries = plugin.extract(&dir).unwrap();
            entries[0].translation = Some(translation.into());

            let report = plugin.inject(&dir, &entries).unwrap();
            assert_eq!(report.strings_written, 1, "{translation:?}: {report:?}");
            assert_eq!(report.strings_skipped, 0, "{translation:?}: {report:?}");
            assert!(report.warnings.is_empty(), "{report:?}");
            assert_eq!(
                fs::read(dir.join("0.txt")).unwrap(),
                format!("{translation}\n").as_bytes(),
                "{translation:?}"
            );
        }
    }

    #[test]
    fn test_inject_normalized_translation_is_skipped() {
        let dir = tempdir();
        let original = " \t `Hello.@\n";
        write_0_txt(&dir, original);
        let plugin = NScripterPlugin::new();
        let mut entries = plugin.extract(&dir).unwrap();
        entries[0].translation = Some(" \t Hello.@".into());

        let report = plugin.inject(&dir, &entries).unwrap();
        assert_eq!(report.files_modified, 0, "{report:?}");
        assert_eq!(report.strings_written, 0, "{report:?}");
        assert_eq!(report.strings_skipped, 1, "{report:?}");
        assert!(report.files_written.is_empty(), "{report:?}");
        assert!(report.warnings.is_empty(), "{report:?}");
        assert_eq!(fs::read(dir.join("0.txt")).unwrap(), original.as_bytes());
    }

    #[test]
    fn test_inject_normalized_translation_is_skipped_in_changed_file() {
        let dir = tempdir();
        write_0_txt(&dir, "`Hello.@\nこんにちは。@\n");
        let plugin = NScripterPlugin::new();
        let mut entries = plugin.extract(&dir).unwrap();
        entries[0].translation = Some("Hello.@".into());
        entries[1].translation = Some("Hi.@".into());

        let report = plugin.inject(&dir, &entries).unwrap();
        assert_eq!(report.files_modified, 1, "{report:?}");
        assert_eq!(report.strings_written, 1, "{report:?}");
        assert_eq!(report.strings_skipped, 1, "{report:?}");
        assert_eq!(report.files_written, vec![dir.join("0.txt")]);
        assert!(report.warnings.is_empty(), "{report:?}");
        assert_eq!(fs::read(dir.join("0.txt")).unwrap(), b"`Hello.@\n`Hi.@\n");
    }

    #[test]
    fn test_inject_roundtrip_nscript_dat() {
        let dir = tempdir();
        write_nscript_dat(&dir, sample_script());
        roundtrip_translate(&dir, "`Hola, mundo!`");
        // Cipher still applied after inject
        let bytes = fs::read(dir.join("nscript.dat")).unwrap();
        let plain = xor_bytes(&bytes, NSCRIPT_DAT_XOR);
        let (text, _, _) = encoding_rs::SHIFT_JIS.decode(&plain);
        assert!(text.contains("Hola, mundo"));
        // Not plaintext on disk
        assert!(!String::from_utf8_lossy(&bytes).contains("Hola"));
    }

    #[test]
    fn test_extract_nscr_sec() {
        let dir = tempdir();
        write_nscr_sec(&dir, sample_script());
        let entries = NScripterPlugin::new().extract(&dir).unwrap();
        assert!(
            entries.iter().any(|e| e.source.contains("こんにちは")),
            "rot5-decoded extract failed: {:?}",
            entries.iter().map(|e| &e.source).collect::<Vec<_>>()
        );
        assert!(entries.iter().all(|e| e.id.starts_with("nscr_sec.dat#")));
    }

    #[test]
    fn test_inject_roundtrip_nscr_sec() {
        let dir = tempdir();
        write_nscr_sec(&dir, sample_script());
        roundtrip_translate(&dir, "`Hola, mundo!`");
        let bytes = fs::read(dir.join("nscr_sec.dat")).unwrap();
        let plain = xor_rot5_bytes(&bytes);
        let (text, _, _) = encoding_rs::SHIFT_JIS.decode(&plain);
        assert!(text.contains("Hola, mundo"));
        assert!(!String::from_utf8_lossy(&bytes).contains("Hola"));
    }

    #[test]
    fn test_priority_prefers_0_txt_over_nscr_sec() {
        let dir = tempdir();
        write_0_txt(&dir, "`from zero`\r\n");
        write_nscr_sec(&dir, "`from sec`\r\n");
        let entries = NScripterPlugin::new().extract(&dir).unwrap();
        assert!(
            entries.iter().any(|e| e.source.contains("from zero")),
            "expected 0.txt win: {:?}",
            entries.iter().map(|e| &e.source).collect::<Vec<_>>()
        );
        assert!(entries.iter().all(|e| !e.source.contains("from sec")));
        assert!(entries.iter().all(|e| e.id.starts_with("0.txt#")));
    }

    #[test]
    fn test_priority_prefers_nscr_sec_over_nscript_dat() {
        let dir = tempdir();
        write_nscr_sec(&dir, "`from sec`\r\n");
        write_nscript_dat(&dir, "`from dat`\r\n");
        let entries = NScripterPlugin::new().extract(&dir).unwrap();
        assert!(
            entries.iter().any(|e| e.source.contains("from sec")),
            "expected nscr_sec.dat before nscript.dat: {:?}",
            entries.iter().map(|e| &e.source).collect::<Vec<_>>()
        );
        assert!(entries.iter().all(|e| !e.source.contains("from dat")));
    }

    fn roundtrip_translate(dir: &Path, new_en: &str) {
        let plugin = NScripterPlugin::new();
        let mut entries = plugin.extract(dir).unwrap();
        assert!(!entries.is_empty());
        for e in &mut entries {
            if e.source.contains("Hello, world") {
                e.translation = Some(new_en.to_string());
            }
        }
        let report = plugin.inject(dir, &entries).unwrap();
        assert!(report.files_modified >= 1, "{report:?}");
        let again = plugin.extract(dir).unwrap();
        assert!(
            again.iter().any(|e| e.source.contains("Hola")),
            "re-extract missing translation: {:?}",
            again.iter().map(|e| &e.source).collect::<Vec<_>>()
        );
        // Code still excluded
        assert!(again.iter().all(|e| is_player_text_line(&e.source)));
        // JP lines preserved
        assert!(again.iter().any(|e| e.source.contains("こんにちは")));
    }

    #[test]
    fn test_unsupported_only_nscript_underscore_errors_loudly() {
        let dir = tempdir();
        fs::write(dir.join("nscript.___"), b"\0\0\0").unwrap();
        let err = NScripterPlugin::new()
            .extract(&dir)
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("nscript.___") || err.contains("unsupported"),
            "expected unsupported container message, got: {err}"
        );
    }

    #[test]
    fn test_nscript_underscore_outranks_nscript_dat_errors_loudly() {
        // Engine readScript order: nscript.___ (mode 3) beats nscript.dat (mode 1).
        // Extracting nscript.dat here would translate a file the game never loads.
        let dir = tempdir();
        fs::write(dir.join("nscript.___"), b"\0\0\0").unwrap();
        write_nscript_dat(&dir, "`ignored by engine\n");
        let err = NScripterPlugin::new()
            .extract(&dir)
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("nscript.___"),
            "expected engine-priority unsupported error, got: {err}"
        );
        // A higher-priority supported container suppresses the error again.
        fs::write(dir.join("0.txt"), encode_sjis_fixture("`hello line\n")).unwrap();
        let entries = NScripterPlugin::new().extract(&dir).unwrap();
        assert!(!entries.is_empty());
    }

    #[test]
    fn test_nsa_only_extract_reports_loudly() {
        let dir = tempdir();
        fs::write(dir.join("arc.nsa"), b"NSA\0fake").unwrap();
        let err = NScripterPlugin::new()
            .extract(&dir)
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("nsa") || err.contains("archive"),
            "expected nsa-only message, got: {err}"
        );
    }

    #[test]
    fn test_sjis_unencodable_translation_warns_and_skips() {
        let dir = tempdir();
        write_0_txt(&dir, sample_script());
        let plugin = NScripterPlugin::new();
        let mut entries = plugin.extract(&dir).unwrap();
        let target = entries
            .iter_mut()
            .find(|e| e.source.contains("Hello, world"))
            .expect("dialogue line");
        // Emoji is not in Shift-JIS code page.
        target.translation = Some("`Hello \u{1F600}`".into());
        let original_id = target.id.clone();
        let report = plugin.inject(&dir, &entries).unwrap();
        assert!(
            report
                .warnings
                .iter()
                .any(|w| w.contains("Shift-JIS") || w.contains("encodable")),
            "expected SJIS warning: {:?}",
            report.warnings
        );
        assert!(report.strings_skipped >= 1, "{report:?}");
        // Original line must remain
        let again = plugin.extract(&dir).unwrap();
        assert!(
            again
                .iter()
                .any(|e| e.id == original_id && e.source.contains("Hello, world")),
            "original corrupted: {:?}",
            again.iter().map(|e| &e.source).collect::<Vec<_>>()
        );
        assert!(!again.iter().any(|e| e.source.contains('\u{1F600}')));
    }

    #[test]
    fn test_00_txt_supported() {
        let dir = tempdir();
        fs::write(dir.join("00.txt"), encode_sjis_fixture("`from 00`\r\n")).unwrap();
        let plugin = NScripterPlugin::new();
        assert!(plugin.detect(&dir));
        let entries = plugin.extract(&dir).unwrap();
        assert!(entries.iter().any(|e| e.source.contains("from 00")));
        assert!(entries.iter().all(|e| e.id.starts_with("00.txt#")));
    }
    #[test]
    fn c120_rmenu_fields_all_containers_and_newlines() {
        for name in SUPPORTED_CONTAINERS {
            for newline in ["\n", "\r\n"] {
                let dir = tempdir();
                let path = dir.join(name);
                let kind = ContainerKind::from_name(name).unwrap();
                let text = format!("; untouched{newline}caption \"題名\"{newline}rmenu \"Skip\",skip,\"Hide\",windowerase,\"Restart\",reset{newline}こんにちは。@{newline}bg \"image.bmp\",0{newline}");
                fs::write(&path, fixture_container(&text, kind)).unwrap();
                let plugin = NScripterPlugin::new();
                let mut entries = plugin.extract(&dir).unwrap();
                assert_eq!(entries.len(), 5, "{name} {newline:?}");
                assert_eq!(
                    entries
                        .iter()
                        .map(|e| e.source.as_str())
                        .collect::<Vec<_>>(),
                    ["題名", "Skip", "Hide", "Restart", "こんにちは。@"]
                );
                assert_eq!(
                    entries
                        .iter()
                        .map(|e| e.id.as_str())
                        .collect::<std::collections::HashSet<_>>()
                        .len(),
                    5
                );
                for e in &mut entries {
                    e.translation = Some(format!("TL {}", e.source));
                }
                let report = plugin.inject(&dir, &entries).unwrap();
                assert_eq!(
                    (report.strings_written, report.strings_skipped),
                    (5, 0),
                    "{report:?}"
                );
                let expected = text
                    .replace("\"題名\"", "\"TL 題名\"")
                    .replace("\"Skip\"", "\"TL Skip\"")
                    .replace("\"Hide\"", "\"TL Hide\"")
                    .replace("\"Restart\"", "\"TL Restart\"")
                    .replace("こんにちは。@", "`TL こんにちは。@");
                assert_eq!(fs::read(&path).unwrap(), fixture_container(&expected, kind));
                let again = plugin.extract(&dir).unwrap();
                assert_eq!(again.len(), 5);
                assert_eq!(
                    again.iter().map(|e| &e.id).collect::<Vec<_>>(),
                    entries.iter().map(|e| &e.id).collect::<Vec<_>>()
                );
                assert!(again
                    .iter()
                    .all(|e| e.source.trim_start_matches('`').starts_with("TL ")));
            }
        }
    }

    fn fixture_container(text: &str, kind: ContainerKind) -> Vec<u8> {
        let bytes = encode_sjis_fixture(text);
        match kind {
            ContainerKind::Plain => bytes,
            ContainerKind::Xor84 => xor_bytes(&bytes, NSCRIPT_DAT_XOR),
            ContainerKind::XorRot5 => xor_rot5_bytes(&bytes),
        }
    }

    #[test]
    fn c120_caption_japanese_and_syntax_rejections() {
        for translation in [
            "新しい題名",
            "New title",
            "comma,colon:;backslash\\",
            "bad\"quote",
            "bad\nline",
            "bad\rline",
            "bad\0nul",
            "bad😀",
        ] {
            let dir = tempdir();
            let original = " \tCaPtIoN \"Title\" ; comment\r\n";
            write_0_txt(&dir, original);
            let plugin = NScripterPlugin::new();
            let mut entries = plugin.extract(&dir).unwrap();
            assert_eq!(entries.len(), 1);
            assert_eq!(entries[0].source, "Title");
            entries[0].translation = Some(translation.into());
            let report = plugin.inject(&dir, &entries).unwrap();
            if translation.starts_with("bad") {
                assert_eq!(
                    (report.strings_written, report.strings_skipped),
                    (0, 1),
                    "{report:?}"
                );
                assert_eq!(report.skip_reasons.values().sum::<usize>(), 1);
                assert!(!report.warnings.is_empty());
                assert_eq!(
                    fs::read(dir.join("0.txt")).unwrap(),
                    encode_sjis_fixture(original)
                );
            } else {
                assert_eq!(
                    (report.strings_written, report.strings_skipped),
                    (1, 0),
                    "{report:?}"
                );
                assert_eq!(
                    fs::read(dir.join("0.txt")).unwrap(),
                    encode_sjis_fixture(&original.replace("Title", translation))
                );
                assert_eq!(plugin.extract(&dir).unwrap()[0].source, translation);
            }
        }
    }

    #[test]
    fn c120_duplicate_labels_keep_physical_positions() {
        let dir = tempdir();
        let original = "caption\t\"First\"\n\trmenu\t\"Same\" , skip , \"Same\" , reset\r\ncaption \"Last\"\n rmenu \"Other\",windowerase";
        write_0_txt(&dir, original);
        let plugin = NScripterPlugin::new();
        let mut entries = plugin.extract(&dir).unwrap();
        assert_eq!(entries.len(), 5);
        assert_ne!(entries[1].id, entries[2].id);
        entries[2].translation = Some("Only second label".into());
        let report = plugin.inject(&dir, &entries).unwrap();
        assert_eq!(report.strings_written, 1, "{report:?}");
        assert_eq!(
            fs::read(dir.join("0.txt")).unwrap(),
            encode_sjis_fixture(
                &original.replace("\"Same\" , reset", "\"Only second label\" , reset")
            )
        );
    }

    #[test]
    fn c120_token_scanner_excludes_technical_and_expression_arguments() {
        let dir = tempdir();
        write_0_txt(
            &dir,
            concat!(
                "bg \"caption.bmp\",0\n; caption \"Comment\"\n*caption\n",
                "caption_extra \"Not caption\"\nmov $0,\"Not text\"\n",
                "caption \"prefix\"+$0\ncaption $0\ncaption \"unterminated\n",
                "rmenu \"Broken\",\"skip\"\nrmenu \"Partial\",skip,\"Missing dispatch\"\n",
                "rmenu \"Expression\"+alias,skip\n",
                "caption\"Literal\\\" ; backslash does not escape the quote\n",
                "rmenu \"Comma, colon: semicolon;\",skip, \"OK\",reset : bg \"path.bmp\",0\n"
            ),
        );
        let entries = NScripterPlugin::new().extract(&dir).unwrap();
        assert_eq!(
            entries
                .iter()
                .map(|e| e.source.as_str())
                .collect::<Vec<_>>(),
            ["Literal\\", "Comma, colon: semicolon;", "OK"]
        );
    }

    #[test]
    fn c120_inject_rejects_stale_or_ambiguous_fields() {
        for change in ["source", "dispatch", "command", "locator", "duplicate"] {
            let dir = tempdir();
            write_0_txt(&dir, "rmenu \"Old\",skip,\"Other\",reset\n");
            let plugin = NScripterPlugin::new();
            let mut entries = plugin.extract(&dir).unwrap();
            assert_eq!(entries.len(), 2);
            entries.truncate(1);
            entries[0].translation = Some("Replacement".into());
            match change {
                "source" => write_0_txt(&dir, "rmenu \"Changed\",skip,\"Other\",reset\n"),
                "dispatch" => write_0_txt(&dir, "rmenu \"Old\",windowerase,\"Other\",reset\n"),
                "command" => write_0_txt(&dir, "other \"Old\",skip,\"Other\",reset\n"),
                "locator" => entries[0].id.push_str(":invalid"),
                "duplicate" => entries.push(entries[0].clone()),
                _ => unreachable!(),
            }
            let before = fs::read(dir.join("0.txt")).unwrap();
            let report = plugin.inject(&dir, &entries).unwrap();
            assert_eq!(report.strings_written, 0, "{change}: {report:?}");
            assert_eq!(
                report.strings_skipped,
                entries.len(),
                "{change}: {report:?}"
            );
            assert_eq!(report.skip_reasons.values().sum::<usize>(), entries.len());
            assert!(!report.warnings.is_empty());
            assert_eq!(fs::read(dir.join("0.txt")).unwrap(), before);
        }
    }

    #[test]
    fn c120_preserves_unmodified_sjis_bytes_and_mixed_newlines() {
        let dir = tempdir();
        // ED40 is a valid non-canonical encoding of a CP932 character. Re-encoding
        // the whole script would change it to FA5C, even in an untouched comment.
        let original = [
            b";".as_slice(),
            &[0xed, 0x40],
            b"\r\ncaption \"Title\"\n; tail\r\n",
        ]
        .concat();
        fs::write(dir.join("0.txt"), &original).unwrap();
        let plugin = NScripterPlugin::new();
        let mut entries = plugin.extract(&dir).unwrap();
        assert_eq!(entries.len(), 1);
        entries[0].translation = Some("New title".into());
        assert_eq!(plugin.inject(&dir, &entries).unwrap().strings_written, 1);
        assert_eq!(
            fs::read(dir.join("0.txt")).unwrap(),
            [
                b";".as_slice(),
                &[0xed, 0x40],
                b"\r\ncaption \"New title\"\n; tail\r\n"
            ]
            .concat()
        );
    }

    #[test]
    fn c120_verified_revision_retargets_changed_length_fields() {
        for name in SUPPORTED_CONTAINERS {
            let dir = tempdir();
            let path = dir.join(name);
            let kind = ContainerKind::from_name(name).unwrap();
            let text = "caption \"題名\"\nrmenu \"同じ\",skip,\"同じ\",reset\nこんにちは。@\n";
            fs::write(&path, fixture_container(text, kind)).unwrap();
            let saved = dir.join("original.bin");
            fs::copy(&path, &saved).unwrap();
            let originals = HashMap::from([(
                path.clone(),
                locust_core::backup::RevisionOriginal::capture(&saved).unwrap(),
            )]);
            let plugin = NScripterPlugin::new();
            let mut entries = plugin.extract(&dir).unwrap();
            assert_eq!(entries.len(), 4);
            for e in &mut entries {
                e.translation = Some(format!("TL {}", e.source));
            }
            assert_eq!(plugin.inject(&dir, &entries).unwrap().strings_written, 4);
            for e in &mut entries {
                e.translation = Some(format!("R2 {}", e.translation.as_ref().unwrap()));
            }
            let mut unverified = entries.clone();
            plugin
                .prepare_revision_entries(&mut unverified, &HashMap::new())
                .unwrap();
            assert_eq!(unverified[0].source, entries[0].source);
            let mut stale = entries.clone();
            stale[0].source = "wrong source".into();
            plugin
                .prepare_revision_entries(&mut stale, &originals)
                .unwrap();
            assert_eq!(stale[0].source, "wrong source");
            plugin
                .prepare_revision_entries(&mut entries, &originals)
                .unwrap();
            assert_eq!(entries[0].source, "TL 題名");
            let report = plugin.inject(&dir, &entries).unwrap();
            assert_eq!(
                (report.strings_written, report.strings_skipped),
                (4, 0),
                "{name}: {report:?}"
            );
            let expected = text
                .replace("\"題名\"", "\"R2 TL 題名\"")
                .replace("\"同じ\"", "\"R2 TL 同じ\"")
                .replace("こんにちは。@", "`R2 TL こんにちは。@");
            assert_eq!(fs::read(&path).unwrap(), fixture_container(&expected, kind));
            assert!(plugin
                .extract(&dir)
                .unwrap()
                .iter()
                .all(|e| e.source.trim_start_matches('`').starts_with("R2 ")));
        }
    }
}
