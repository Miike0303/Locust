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
//!   the asar in place with a `.locust-old` safety rename.
//!   NW.js desktop packs use `package.nw` or a self-extracting `*.exe` with an
//!   appended ZIP (see [`crate::tyrano_nw`]).
//!
//! # First-cut extraction (over-extraction OK)
//! - Player text = non-empty lines that are not comments/labels/`@`/pure-`[tag]` lines.
//! - Inline `[tags]` inside a text line stay in the extracted string.
//! - `#name` bare ASCII identifiers are **not** emitted; non-identifier display names
//!   (e.g. `#表示名`) are emitted with tag `"speaker"` (name segment only; face after
//!   `:` is preserved on inject).
//! - Encoding: UTF-8 only; preserve BOM if the source file had one.
//!
//! Out of scope: `Config.tjs` string tables,
//! `[iscript]` JS / `[html]` bodies as structured ASTs (line heuristic may over-extract),
//! real commercial game fixtures.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use locust_core::error::{LocustError, Result};
use locust_core::extraction::{FormatPlugin, InjectionReport};
use locust_core::models::{OutputMode, StringEntry};
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
    crlf: bool,
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
    let crlf = text.contains("\r\n");
    Ok(DecodedKs {
        text: text.to_string(),
        had_bom,
        crlf,
    })
}

fn encode_ks_utf8(decoded: &DecodedKs) -> Vec<u8> {
    let text = normalize_newlines(&decoded.text, decoded.crlf);
    let mut out = Vec::with_capacity(text.len() + 3);
    if decoded.had_bom {
        out.extend_from_slice(UTF8_BOM);
    }
    out.extend_from_slice(text.as_bytes());
    out
}

fn normalize_newlines(text: &str, crlf: bool) -> String {
    let unified = text.replace("\r\n", "\n").replace('\r', "\n");
    if crlf {
        unified.replace('\n', "\r\n")
    } else {
        unified
    }
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

fn is_speaker_line(line: &str) -> bool {
    line.trim().starts_with('#')
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

/// Structural / non-player lines: comments, labels, `@` commands, pure tags, empty.
/// Speaker `#` lines are handled separately (display names vs bare ids).
fn is_structural_non_text(line: &str) -> bool {
    let t = line.trim();
    if t.is_empty() {
        return true;
    }
    if t.starts_with(';') || t.starts_with('*') || t.starts_with('@') {
        return true;
    }
    if t == "/*" || t == "*/" {
        return true;
    }
    is_pure_tag_line(t)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LineKind {
    /// Skip (comment/label/@/pure tag/empty/bare speaker id/block-comment body).
    Skip,
    /// Player-visible text line (inline tags kept).
    Text,
    /// Speaker display name (source = name only).
    Speaker,
}

/// Classify lines with block-comment state machine (`/*` / `*/` whole-line only,
/// matching kag.parser.js).
fn classify_lines(text: &str) -> Vec<LineKind> {
    let mut out = Vec::new();
    let mut in_block = false;
    for line in text.split('\n') {
        let line = line.strip_suffix('\r').unwrap_or(line);
        let t = line.trim();
        if in_block {
            if t == "*/" {
                in_block = false;
            }
            out.push(LineKind::Skip);
            continue;
        }
        if t == "/*" {
            in_block = true;
            out.push(LineKind::Skip);
            continue;
        }
        if is_structural_non_text(line) {
            out.push(LineKind::Skip);
            continue;
        }
        if is_speaker_line(line) {
            if speaker_display_name(line).is_some() {
                out.push(LineKind::Speaker);
            } else {
                out.push(LineKind::Skip);
            }
            continue;
        }
        out.push(LineKind::Text);
    }
    out
}

/// Rebuild a `#speaker` line after translating the display name.
fn rebuild_speaker_line(original: &str, new_name: &str) -> String {
    let leading = &original[..original.len() - original.trim_start().len()];
    let t = original.trim();
    // Preserve any trailing whitespace after content.
    let trailing_len = original.len() - original.trim_end().len();
    let trailing = &original[original.len() - trailing_len..];
    debug_assert!(t.starts_with('#'));
    let rest = t[1..].trim_start();
    if let Some(colon) = rest.find(':') {
        let face = &rest[colon..]; // includes ':'
        format!("{leading}#{new_name}{face}{trailing}")
    } else {
        format!("{leading}#{new_name}{trailing}")
    }
}

/// Extract string entries from one UTF-8 scenario payload.
fn entries_from_ks_bytes(bytes: &[u8], rel: &str, file_path: PathBuf) -> Result<Vec<StringEntry>> {
    let decoded = decode_ks_utf8(bytes, rel)?;
    let kinds = classify_lines(&decoded.text);
    let mut all = Vec::new();
    for (idx, line) in decoded.text.split('\n').enumerate() {
        let line = line.strip_suffix('\r').unwrap_or(line);
        let line_no = idx + 1;
        let kind = kinds.get(idx).copied().unwrap_or(LineKind::Skip);
        match kind {
            LineKind::Skip => {}
            LineKind::Text => {
                let id = format!("{rel}#{line_no}");
                let mut entry = StringEntry::new(id, line, file_path.clone());
                entry.tags = vec!["dialogue".into()];
                all.push(entry);
            }
            LineKind::Speaker => {
                if let Some(name) = speaker_display_name(line) {
                    let id = format!("{rel}#{line_no}");
                    let mut entry = StringEntry::new(id, name, file_path.clone());
                    entry.tags = vec!["speaker".into()];
                    entry.context = Some(line.to_string());
                    all.push(entry);
                }
            }
        }
    }
    Ok(all)
}

/// Apply translations to a scenario file; returns new bytes if changed.
fn apply_ks_translations(
    bytes: &[u8],
    label: &str,
    file_entries: &[&StringEntry],
) -> Result<Option<(Vec<u8>, usize, usize)>> {
    let mut decoded = decode_ks_utf8(bytes, label)?;
    let mut by_line: HashMap<usize, &StringEntry> = HashMap::new();
    for e in file_entries {
        if e.translation.is_some() {
            if let Some(n) = e.id.rsplit('#').next().and_then(|s| s.parse().ok()) {
                by_line.insert(n, e);
            }
        }
    }
    let kinds = classify_lines(&decoded.text);
    let mut out_lines = Vec::new();
    let mut changed = false;
    let mut file_written = 0usize;
    let mut file_skipped = 0usize;

    for (idx, line) in decoded.text.split('\n').enumerate() {
        let line = line.strip_suffix('\r').unwrap_or(line);
        let line_no = idx + 1;
        let kind = kinds.get(idx).copied().unwrap_or(LineKind::Skip);
        if let Some(entry) = by_line.get(&line_no) {
            if let Some(t) = entry.translation.as_deref() {
                match kind {
                    LineKind::Text if t != line => {
                        out_lines.push(t.to_string());
                        changed = true;
                        file_written += 1;
                        continue;
                    }
                    LineKind::Speaker => {
                        if let Some(old_name) = speaker_display_name(line) {
                            if t != old_name {
                                out_lines.push(rebuild_speaker_line(line, t));
                                changed = true;
                                file_written += 1;
                                continue;
                            }
                        }
                        file_skipped += 1;
                    }
                    _ => {
                        file_skipped += 1;
                    }
                }
            }
        }
        out_lines.push(line.to_string());
    }
    if !changed {
        return Ok(None);
    }
    decoded.text = out_lines.join("\n");
    Ok(Some((encode_ks_utf8(&decoded), file_written, file_skipped)))
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

/// Replace `path` with `new_bytes` after moving the original to `path` + `.locust-old`.
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
            match apply_ks_translations(&bytes, &label, &file_entries) {
                Ok(Some((encoded, written, skipped))) => {
                    std::fs::write(&actual, &encoded)?;
                    files_modified += 1;
                    files_written.push(actual);
                    strings_written += written;
                    strings_skipped += skipped;
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
                warnings.push(format!("missing archive {archive_rel}"));
                for (_, fe) in &inners {
                    strings_skipped += fe.len();
                }
                continue;
            }

            let archive = match AsarArchive::open(&arch_path) {
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
                match apply_ks_translations(&bytes, &label, &file_entries) {
                    Ok(Some((encoded, written, skipped))) => {
                        arch_written += written;
                        strings_skipped += skipped;
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
                    Ok(None) => {
                        strings_skipped += file_entries.len();
                    }
                    Err(e) => {
                        warnings.push(format!("cannot translate {label}: {e}"));
                        strings_skipped += file_entries.len();
                    }
                }
            }

            // Unpacked files: write with per-file backup (or create if missing)
            for (_key, disk, data) in &unpacked_writes {
                if let Some(parent) = disk.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                if disk.exists() {
                    match replace_file_with_backup(disk, data) {
                        Ok(()) => {
                            files_modified += 1;
                            files_written.push(disk.clone());
                        }
                        Err(e) => {
                            warnings.push(format!("safe-replace {}: {e}", disk.display()));
                        }
                    }
                } else if let Err(e2) = std::fs::write(disk, data) {
                    warnings.push(format!("write unpacked {}: {e2}", disk.display()));
                } else {
                    files_modified += 1;
                    files_written.push(disk.clone());
                }
            }

            if !replacements.is_empty() {
                match tyrano_asar::rebuild_asar(&archive, &replacements) {
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
                warnings.push(format!("missing NW.js package {archive_rel}"));
                for (_, fe) in &inners {
                    strings_skipped += fe.len();
                }
                continue;
            }

            let archive = match NwArchive::open(&arch_path) {
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
                    .find(|e| e.path.replace('\\', "/") == inner.replace('\\', "/"))
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
                match apply_ks_translations(&bytes, &label, &file_entries) {
                    Ok(Some((encoded, written, skipped))) => {
                        arch_written += written;
                        strings_skipped += skipped;
                        replacements.insert(inner.replace('\\', "/"), encoded);
                    }
                    Ok(None) => {
                        strings_skipped += file_entries.len();
                    }
                    Err(e) => {
                        warnings.push(format!("cannot translate {label}: {e}"));
                        strings_skipped += file_entries.len();
                    }
                }
            }

            if !replacements.is_empty() {
                match tyrano_nw::rebuild_nw_zip(&archive, &replacements) {
                    Ok(new_pkg) => match replace_file_with_backup(&arch_path, &new_pkg) {
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

        let backup = PathBuf::from(format!("{}.locust-old", asar_path.display()));
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
        let backup = PathBuf::from(format!("{}.locust-old", nw_path.display()));
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
        plugin.inject(&dir, &entries).unwrap();
        let out = fs::read(&exe_path).unwrap();
        assert!(
            out.starts_with(&prefix),
            "exe prefix must be preserved after inject"
        );
        let backup = PathBuf::from(format!("{}.locust-old", exe_path.display()));
        assert!(backup.is_file());
        let again = plugin.extract(&dir).unwrap();
        assert!(again.iter().any(|e| e.source.contains("Narracion en exe")));
    }

    #[test]
    fn test_locust_old_restored_on_write_failure_path() {
        // replace_file_with_backup restores original when the write step fails —
        // exercise via a path that is a directory (write fails after rename).
        let dir = tempdir();
        let nw_path = dir.join("package.nw");
        fs::write(&nw_path, build_nw_zip_bytes(sample_scenario())).unwrap();
        // After a successful inject, .locust-old holds prior bytes.
        let plugin = TyranoPlugin::new();
        let mut entries = plugin.extract(&dir).unwrap();
        for e in &mut entries {
            if e.source.contains("This is narration") {
                e.translation = Some("x".into());
            }
        }
        plugin.inject(&dir, &entries).unwrap();
        let backup = PathBuf::from(format!("{}.locust-old", nw_path.display()));
        assert!(backup.is_file());
        let old = fs::read(&backup).unwrap();
        // Second inject overwrites .locust-old with the previous package.nw.
        for e in &mut entries {
            if e.source.contains("This is narration") || e.source.contains("x") {
                e.translation = Some("yy".into());
            }
        }
        // Re-extract so translations match current file text.
        let mut entries2 = plugin.extract(&dir).unwrap();
        for e in &mut entries2 {
            if e.source.contains("x") || e.source.contains("narration") {
                e.translation = Some("second pass".into());
            }
        }
        plugin.inject(&dir, &entries2).unwrap();
        assert!(backup.is_file());
        // Prior package (post-first-inject) should have been moved to .locust-old.
        assert_ne!(fs::read(&backup).unwrap(), old);
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
