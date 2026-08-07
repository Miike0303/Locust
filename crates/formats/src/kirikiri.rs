//! KiriKiri / KAG script plugin — Experimental (synthetic fixtures).
//!
//! # Spec sources (transforms verified — do not invent)
//! - Scrambled text-stream header `FE FE <mode> FF FE` and per-mode body transforms
//!   as implemented by the widely used reverse of the engine loader:
//!   https://github.com/arcusmaximus/KirikiriTools/blob/master/KirikiriDescrambler/Descrambler.cs
//!   https://github.com/arcusmaximus/KirikiriTools/blob/master/KirikiriDescrambler/Scrambler.cs
//!   (mode 0: per-UTF-16LE-unit XOR; mode 1: odd/even bit swap — self-inverse;
//!   mode 2: zlib-compressed UTF-16LE payload after length fields).
//! - Header signature documentation:
//!   https://github.com/arcusmaximus/KirikiriTools#kirikiridescrambler
//! - Engine family / KAG script conventions (`;` comments, `*` labels, `@` commands,
//!   `[tag]` markup): KiriKiri2 / KAG lineage — https://github.com/krkrz/krkr2
//! - Unencrypted XP3 containers: see [`crate::kirikiri_xp3`] (arcusmaximus Xp3Pack layout;
//!   inject writes `patch.xp3` — engines load it next to the exe and override base entries).
//!
//! # Mode transforms (UTF-16LE code units, little-endian byte pairs)
//! - **Mode 0 decode:** for each unit, if high==0 && low<0x20 leave as-is; else
//!   `high ^= (low & 0xFE); low ^= 1` (Descrambler order).
//! - **Mode 0 encode:** reverse of decode: skip same control units; else
//!   `low ^= 1; high ^= (low & 0xFE)` (Scrambler order — uses post-xor low).
//! - **Mode 1:** `c = ((c & 0xAAAA) >> 1) | ((c & 0x5555) << 1)` (self-inverse).
//! - **Mode 2:** detect header only; report unsupported (zlib path; miniz available
//!   but first-cut intentionally rejects compressed scripts).
//!
//! Out of scope: CxDec / Hxv4 encrypted XP3, `.tjs`/compiled `.scn`, mode-2 write-back,
//! rewriting base `.xp3` archives (patch.xp3 only).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use locust_core::error::{LocustError, Result};
use locust_core::extraction::{FormatPlugin, InjectionReport};
use locust_core::models::{OutputMode, StringEntry};
use tracing::warn;

use crate::kirikiri_xp3::{self, Xp3Archive};

/// How the on-disk bytes encode the decoded Unicode text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum KsEncoding {
    Utf16Le,
    Utf8,
    ShiftJis,
}

/// Optional FE FE cipher wrapper around a UTF-16LE payload.
/// Mode 2 (zlib) is rejected at decode and never stored here.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CipherMode {
    None,
    Mode0,
    Mode1,
}

#[derive(Clone, Debug)]
struct DecodedKs {
    text: String,
    encoding: KsEncoding,
    cipher: CipherMode,
    /// Prefer `\r\n` when rewriting if the source used it.
    crlf: bool,
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

fn str_from_utf16le_bytes(data: &[u8]) -> String {
    let units: Vec<u16> = data
        .chunks_exact(2)
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .collect();
    String::from_utf16_lossy(&units)
}

// ─── Decode / encode whole file ────────────────────────────────────────────

fn decode_ks_bytes(bytes: &[u8], file_label: &str) -> Result<DecodedKs> {
    // Cipher header: FE FE mode FF FE
    if bytes.len() >= 5 && bytes[0] == 0xFE && bytes[1] == 0xFE && bytes[3] == 0xFF && bytes[4] == 0xFE
    {
        let mode = bytes[2];
        let body = &bytes[5..];
        match mode {
            0 => {
                if !body.len().is_multiple_of(2) {
                    return Err(parse_err(file_label, "mode-0 cipher body has odd length"));
                }
                let mut data = body.to_vec();
                mode0_decode_units(&mut data);
                let text = str_from_utf16le_bytes(&data);
                let crlf = text.contains("\r\n");
                return Ok(DecodedKs {
                    text,
                    encoding: KsEncoding::Utf16Le,
                    cipher: CipherMode::Mode0,
                    crlf,
                });
            }
            1 => {
                if !body.len().is_multiple_of(2) {
                    return Err(parse_err(file_label, "mode-1 cipher body has odd length"));
                }
                let mut data = body.to_vec();
                mode1_swap_units(&mut data);
                let text = str_from_utf16le_bytes(&data);
                let crlf = text.contains("\r\n");
                return Ok(DecodedKs {
                    text,
                    encoding: KsEncoding::Utf16Le,
                    cipher: CipherMode::Mode1,
                    crlf,
                });
            }
            2 => {
                // Mode 2 is zlib-compressed UTF-16LE; first cut rejects it loudly.
                return Err(parse_err(
                    file_label,
                    "compressed .ks (mode 2) not yet supported",
                ));
            }
            _ => {
                return Err(parse_err(
                    file_label,
                    &format!("unsupported FE FE cipher mode {mode}"),
                ));
            }
        }
    }

    // Plain UTF-16LE BOM
    if bytes.len() >= 2 && bytes[0] == 0xFF && bytes[1] == 0xFE {
        let text = str_from_utf16le_bytes(&bytes[2..]);
        let crlf = text.contains("\r\n");
        return Ok(DecodedKs {
            text,
            encoding: KsEncoding::Utf16Le,
            cipher: CipherMode::None,
            crlf,
        });
    }

    // Plain UTF-8 BOM
    if bytes.len() >= 3 && bytes[0] == 0xEF && bytes[1] == 0xBB && bytes[2] == 0xBF {
        let text = String::from_utf8_lossy(&bytes[3..]).into_owned();
        let crlf = text.contains("\r\n");
        return Ok(DecodedKs {
            text,
            encoding: KsEncoding::Utf8,
            cipher: CipherMode::None,
            crlf,
        });
    }

    // No BOM: prefer strict UTF-8, else Shift-JIS (common for JP shipping).
    if let Ok(text) = std::str::from_utf8(bytes) {
        // Heuristic: if high bytes look like multi-byte UTF-8, keep UTF-8;
        // pure ASCII still fine as UTF-8 for roundtrip.
        let crlf = text.contains("\r\n");
        return Ok(DecodedKs {
            text: text.to_string(),
            encoding: KsEncoding::Utf8,
            cipher: CipherMode::None,
            crlf,
        });
    }

    let (cow, _, had_errors) = encoding_rs::SHIFT_JIS.decode(bytes);
    if had_errors {
        return Err(parse_err(
            file_label,
            "could not decode .ks as UTF-16LE/UTF-8/Shift-JIS or FE FE cipher",
        ));
    }
    let text = cow.into_owned();
    let crlf = text.contains("\r\n");
    Ok(DecodedKs {
        text,
        encoding: KsEncoding::ShiftJis,
        cipher: CipherMode::None,
        crlf,
    })
}

fn encode_ks_bytes(decoded: &DecodedKs) -> Result<Vec<u8>> {
    let text = normalize_newlines(&decoded.text, decoded.crlf);
    match decoded.cipher {
        CipherMode::Mode0 => {
            let mut body = utf16le_bytes_from_str(&text);
            mode0_encode_units(&mut body);
            let mut out = vec![0xFE, 0xFE, 0x00, 0xFF, 0xFE];
            out.extend_from_slice(&body);
            Ok(out)
        }
        CipherMode::Mode1 => {
            let mut body = utf16le_bytes_from_str(&text);
            mode1_swap_units(&mut body);
            let mut out = vec![0xFE, 0xFE, 0x01, 0xFF, 0xFE];
            out.extend_from_slice(&body);
            Ok(out)
        }
        CipherMode::None => match decoded.encoding {
            KsEncoding::Utf16Le => {
                let mut out = vec![0xFF, 0xFE];
                out.extend_from_slice(&utf16le_bytes_from_str(&text));
                Ok(out)
            }
            KsEncoding::Utf8 => {
                let mut out = vec![0xEF, 0xBB, 0xBF];
                out.extend_from_slice(text.as_bytes());
                Ok(out)
            }
            KsEncoding::ShiftJis => {
                let (bytes, _, had_errors) = encoding_rs::SHIFT_JIS.encode(&text);
                if had_errors {
                    return Err(parse_err(
                        "ks",
                        "could not re-encode translation as Shift-JIS",
                    ));
                }
                Ok(bytes.into_owned())
            }
        },
    }
}

fn normalize_newlines(text: &str, crlf: bool) -> String {
    let unified = text.replace("\r\n", "\n").replace('\r', "\n");
    if crlf {
        unified.replace('\n', "\r\n")
    } else {
        unified
    }
}

fn parse_err(file: &str, message: &str) -> LocustError {
    LocustError::ParseError {
        file: file.into(),
        message: message.into(),
    }
}

// ─── KAG line classification ───────────────────────────────────────────────

/// True for `;comment`, `*label`, `@command`, empty, or pure `[tag]`-only lines.
fn is_non_text_line(line: &str) -> bool {
    let t = line.trim();
    if t.is_empty() {
        return true;
    }
    if t.starts_with(';') || t.starts_with('*') || t.starts_with('@') {
        return true;
    }
    is_pure_tag_line(t)
}

/// Entire line is one or more `[...]` tags with no free text between them.
fn is_pure_tag_line(t: &str) -> bool {
    let mut rest = t;
    if !rest.starts_with('[') {
        return false;
    }
    while !rest.is_empty() {
        rest = rest.trim_start();
        if rest.is_empty() {
            return true;
        }
        if !rest.starts_with('[') {
            return false;
        }
        match rest.find(']') {
            Some(i) => rest = &rest[i + 1..],
            None => return false,
        }
    }
    true
}

fn is_player_text_line(line: &str) -> bool {
    !is_non_text_line(line)
}

/// Extract dialogue lines from decoded `.ks` text. `rel` is the id/source path
/// prefix (loose relative path or `archive.xp3/inner.ks`). `file_path` is stored
/// on each entry for inject routing.
fn extract_lines_from_text(
    text: &str,
    rel: &str,
    file_path: PathBuf,
) -> Vec<StringEntry> {
    let mut all = Vec::new();
    for (idx, line) in text.split('\n').enumerate() {
        let line = line.strip_suffix('\r').unwrap_or(line);
        let line_no = idx + 1;
        if !is_player_text_line(line) {
            continue;
        }
        let id = format!("{rel}#{line_no}");
        let mut entry = StringEntry::new(id, line, file_path.clone());
        entry.tags = vec!["dialogue".into()];
        all.push(entry);
    }
    all
}

/// Apply line translations to a decoded script; returns encoded bytes if changed.
fn apply_translations(
    bytes: &[u8],
    label: &str,
    file_entries: &[&StringEntry],
) -> Result<Option<(Vec<u8>, usize)>> {
    let mut decoded = decode_ks_bytes(bytes, label)?;
    let mut by_line: HashMap<usize, &str> = HashMap::new();
    for e in file_entries {
        if let Some(t) = e.translation.as_deref() {
            if let Some(n) = e.id.rsplit('#').next().and_then(|s| s.parse().ok()) {
                by_line.insert(n, t);
            }
        }
    }

    let mut out_lines = Vec::new();
    let mut changed = false;
    let mut written = 0usize;
    for (idx, line) in decoded.text.split('\n').enumerate() {
        let line = line.strip_suffix('\r').unwrap_or(line);
        let line_no = idx + 1;
        if let Some(t) = by_line.get(&line_no) {
            if is_player_text_line(line) && *t != line {
                out_lines.push((*t).to_string());
                changed = true;
                written += 1;
                continue;
            }
        }
        out_lines.push(line.to_string());
    }

    if !changed {
        return Ok(None);
    }
    decoded.text = out_lines.join("\n");
    let encoded = encode_ks_bytes(&decoded)?;
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

// ─── Plugin ────────────────────────────────────────────────────────────────

impl FormatPlugin for KirikiriPlugin {
    fn id(&self) -> &str {
        "kirikiri"
    }

    fn name(&self) -> &str {
        "KiriKiri / KAG"
    }

    fn description(&self) -> &str {
        "KiriKiri KAG loose .ks + unencrypted XP3 (UTF-16/UTF-8/SJIS; FE FE 0/1; patch.xp3 inject)"
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
        !Self::find_ks_files(path).is_empty() || Self::has_xp3(path)
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

        let mut all = Vec::new();

        // Loose .ks
        for fpath in &ks_files {
            let bytes = std::fs::read(fpath)?;
            let rel = fpath
                .strip_prefix(&root)
                .unwrap_or(fpath.as_path())
                .to_string_lossy()
                .replace('\\', "/");
            let decoded = decode_ks_bytes(&bytes, &rel)?;
            all.extend(extract_lines_from_text(
                &decoded.text,
                &rel,
                fpath.clone(),
            ));
        }

        // XP3 archives
        let mut xp3_parse_errors = 0usize;
        let mut last_xp3_err = String::new();
        let mut xp3_ks_seen = 0usize;
        let mut xp3_skipped = 0usize;

        for arch_path in &xp3_files {
            let arch_name = arch_path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("archive.xp3");
            let archive = match Xp3Archive::open(arch_path) {
                Ok(a) => a,
                Err(e) => {
                    xp3_parse_errors += 1;
                    last_xp3_err = e.to_string();
                    warn!(archive = %arch_path.display(), error = %e, "failed to open XP3");
                    continue;
                }
            };

            for entry in archive.ks_entries() {
                xp3_ks_seen += 1;
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
                let rel = format!("{arch_name}/{}", entry.name.replace('\\', "/"));
                let virtual_path = PathBuf::from(&rel);
                match decode_ks_bytes(&payload, &rel) {
                    Ok(decoded) => {
                        all.extend(extract_lines_from_text(
                            &decoded.text,
                            &rel,
                            virtual_path,
                        ));
                    }
                    Err(e) => {
                        // Likely cxdec-encrypted or non-text — skip, do not fail the whole extract.
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
        }

        if all.is_empty() && ks_files.is_empty() {
            // Only XP3 path and nothing usable
            if xp3_parse_errors > 0 && xp3_ks_seen == 0 {
                return Err(parse_err(
                    &path.display().to_string(),
                    &format!("failed to parse XP3 archive(s): {last_xp3_err}"),
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

        // Collect modified XP3 payloads for a single patch.xp3
        let mut patch_files: Vec<(String, Vec<u8>)> = Vec::new();
        // Cache opened base archives: archive file name → archive
        let mut archive_cache: HashMap<String, Xp3Archive> = HashMap::new();

        for (file_path, file_entries) in &by_file {
            if let Some((archive_name, inner)) = split_xp3_virtual_path(file_path) {
                if !archive_cache.contains_key(&archive_name) {
                    let arch_path = search_root.join(&archive_name);
                    match Xp3Archive::open(&arch_path) {
                        Ok(a) => {
                            archive_cache.insert(archive_name.clone(), a);
                        }
                        Err(e) => {
                            warnings.push(format!(
                                "cannot open base archive {archive_name} for inject: {e}"
                            ));
                            strings_skipped += file_entries.len();
                            continue;
                        }
                    }
                }
                let arch = archive_cache.get(&archive_name).unwrap();

                let entry = match arch.entries.iter().find(|e| {
                    e.name.replace('\\', "/") == inner.replace('\\', "/")
                }) {
                    Some(e) => e.clone(),
                    None => {
                        warnings.push(format!(
                            "entry {inner} not found in {archive_name}"
                        ));
                        strings_skipped += file_entries.len();
                        continue;
                    }
                };

                let bytes = match arch.read_entry(&entry) {
                    Ok(b) => b,
                    Err(e) => {
                        warnings.push(format!("read {archive_name}/{inner}: {e}"));
                        strings_skipped += file_entries.len();
                        continue;
                    }
                };
                let label = format!("{archive_name}/{inner}");
                match apply_translations(&bytes, &label, file_entries) {
                    Ok(Some((encoded, written))) => {
                        patch_files.push((inner.replace('\\', "/"), encoded));
                        strings_written += written;
                        files_modified += 1;
                    }
                    Ok(None) => {
                        strings_skipped += file_entries.len();
                    }
                    Err(e) => {
                        warnings.push(format!("cannot translate {label}: {e}"));
                        strings_skipped += file_entries.len();
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
            match apply_translations(&bytes, &label, file_entries) {
                Ok(Some((encoded, written))) => {
                    std::fs::write(&actual, &encoded)?;
                    files_modified += 1;
                    files_written.push(actual);
                    strings_written += written;
                }
                Ok(None) => {
                    strings_skipped += file_entries.len();
                }
                Err(e) => {
                    warnings.push(format!("cannot re-encode {label}: {e}"));
                    strings_skipped += file_entries.len();
                }
            }
        }

        if !patch_files.is_empty() {
            // Merge by inner name (last write wins)
            let mut merged: HashMap<String, Vec<u8>> = HashMap::new();
            for (name, data) in patch_files {
                merged.insert(name, data);
            }
            let list: Vec<(String, Vec<u8>)> = merged.into_iter().collect();
            match kirikiri_xp3::write_xp3(&list) {
                Ok(bytes) => {
                    let patch_path = search_root.join("patch.xp3");
                    std::fs::write(&patch_path, &bytes)?;
                    files_written.push(patch_path);
                }
                Err(e) => {
                    warnings.push(format!("failed to build patch.xp3: {e}"));
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

#[cfg(test)]
mod tests {
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

    fn write_mode2_stub(path: &Path) {
        // Header only — enough to hit the unsupported path.
        fs::write(path, [0xFE, 0xFE, 0x02, 0xFF, 0xFE, 0x00, 0x00]).unwrap();
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
    fn test_mode2_reports_unsupported() {
        let dir = tempdir();
        write_mode2_stub(&dir.join("compressed.ks"));
        let err = KirikiriPlugin::new().extract(&dir).unwrap_err().to_string();
        assert!(
            err.contains("mode 2") || err.contains("compressed"),
            "expected mode-2 message, got: {err}"
        );
    }

    #[test]
    fn test_xp3_malformed_extract_errors_naming_file() {
        let dir = tempdir();
        fs::write(dir.join("data.xp3"), b"XP3\r\n").unwrap();
        let err = KirikiriPlugin::new().extract(&dir).unwrap_err().to_string();
        assert!(
            err.contains("xp3") || err.contains("XP3") || err.contains("magic") || err.contains("parse"),
            "expected XP3 parse error, got: {err}"
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
            entries.iter().any(|e| e.id.starts_with("data.xp3/scenario/first.ks#")),
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
        assert!(patch.is_file(), "expected patch.xp3, written: {:?}", report.files_written);

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
        let arch =
            crate::kirikiri_xp3::write_xp3(&[("foo.ks".into(), garbage)]).unwrap();
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
    fn test_stability_experimental() {
        assert_eq!(
            KirikiriPlugin::new().stability(),
            locust_core::extraction::FormatStability::Experimental
        );
    }
}
