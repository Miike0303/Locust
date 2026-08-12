//! QSP (QuestSoft Player) binary game plugin — Experimental (synthetic fixtures).
//!
//! # Spec sources (do not invent transforms)
//! - Engine loader/layout: https://github.com/QSPFoundation/qsp/blob/master/qsp/game.c
//!   (`qspOpenGame`, `qspCheckGame`) — `QSPGAME` header, CRLF records, per-location
//!   name / desc / code / actsCount / (image, desc, code)×N.
//! - String encode/decode: https://github.com/QSPFoundation/qsp/blob/master/qsp/coding.c
//!   and `QSP_CODREMOV` (= 5) in coding.h — each UTF-16 code unit is stored as
//!   `unit - 5` (with special case when unit == 5 → store `(u16)-5`); decode adds 5.
//! - Line delimiter `\\r\\n`, game id `QSPGAME`:
//!   https://github.com/QSPFoundation/qsp/blob/master/qsp/text.h
//!   https://github.com/QSPFoundation/qsp/blob/master/qsp/game.h
//! - Independent TS reference matching the same rules:
//!   https://github.com/QSPFoundation/converters/blob/main/src/qsp-byte-stream.ts
//!   https://github.com/QSPFoundation/converters/blob/main/src/readers/qsp.ts
//!   https://github.com/QSPFoundation/converters/blob/main/src/writers/qsp.ts
//! - Container notes: https://github.com/QSPFoundation/qsp-cli (`.qsp`/`.gam` = binary;
//!   `.qsps` = source text — not handled here).
//!
//! Strings are CRLF-delimited variable-length records, not fixed inject slots —
//! no `binary_slot` metadata.
//!
//! Out of scope: legacy pre-`QSPGAME` `.gam` layout, single-byte CP1251 binary,
//! password cracking, full QSP script AST (code fields keep source for inject;
//! quoted player-visible literals are extracted heuristically).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use locust_core::error::{LocustError, Result};
use locust_core::extraction::{FormatPlugin, InjectionReport};
use locust_core::models::{OutputMode, StringEntry};

/// Per-character code shift used by QSP string fields (engine `QSP_CODREMOV`).
const QSP_CODREMOV: u16 = 5;
const QSP_GAMEID: &str = "QSPGAME";

pub struct QspPlugin;

impl QspPlugin {
    pub fn new() -> Self {
        Self
    }

    fn is_qsp_ext(path: &Path) -> bool {
        path.extension()
            .and_then(|e| e.to_str())
            .map(|e| {
                let e = e.to_ascii_lowercase();
                e == "qsp" || e == "gam"
            })
            .unwrap_or(false)
    }

    fn find_qsp_files(path: &Path) -> Vec<PathBuf> {
        let mut out = Vec::new();
        if path.is_file() {
            if Self::is_qsp_ext(path) {
                out.push(path.to_path_buf());
            }
            return out;
        }
        if !path.is_dir() {
            return out;
        }
        if let Ok(entries) = std::fs::read_dir(path) {
            for e in entries.flatten() {
                let p = e.path();
                if p.is_file() && Self::is_qsp_ext(&p) {
                    out.push(p);
                }
            }
        }
        out
    }
}

impl Default for QspPlugin {
    fn default() -> Self {
        Self::new()
    }
}

// ─── CODREMOV transform (UTF-16 code units) ────────────────────────────────

fn decode_unit(ch: u16) -> u16 {
    // C: if (ch == (unsigned short)-QSP_CODREMOV) ch = QSP_CODREMOV; else ch += QSP_CODREMOV;
    if ch == (0u16).wrapping_sub(QSP_CODREMOV) {
        QSP_CODREMOV
    } else {
        ch.wrapping_add(QSP_CODREMOV)
    }
}

fn encode_unit(ch: u16) -> u16 {
    // C: if (ch == QSP_CODREMOV) ch = (unsigned short)-QSP_CODREMOV; else ch -= QSP_CODREMOV;
    if ch == QSP_CODREMOV {
        (0u16).wrapping_sub(QSP_CODREMOV)
    } else {
        ch.wrapping_sub(QSP_CODREMOV)
    }
}

fn decode_field(units: &[u16]) -> String {
    let decoded: Vec<u16> = units.iter().copied().map(decode_unit).collect();
    String::from_utf16_lossy(&decoded)
}

fn encode_field(s: &str) -> Vec<u16> {
    s.encode_utf16().map(encode_unit).collect()
}

// ─── UTF-16LE record I/O ───────────────────────────────────────────────────

fn read_u16le_units(bytes: &[u8]) -> Result<Vec<u16>> {
    if bytes.len() < 2 {
        return Err(LocustError::ParseError {
            file: "qsp".into(),
            message: "file too small for QSP binary".into(),
        });
    }
    // UCS2 games have 0x00 as the high byte of the first code unit of "Q" (0x0051).
    if bytes[1] != 0 {
        return Err(LocustError::ParseError {
            file: "qsp".into(),
            message: "single-byte (CP1251) QSP binary is not supported in this Experimental cut; \
                      re-export as UTF-16LE QSPGAME (see qsp-cli / converters)"
                .into(),
        });
    }
    if !bytes.len().is_multiple_of(2) {
        return Err(LocustError::ParseError {
            file: "qsp".into(),
            message: "UTF-16LE QSP file has odd length".into(),
        });
    }
    let mut units = Vec::with_capacity(bytes.len() / 2);
    for chunk in bytes.chunks_exact(2) {
        units.push(u16::from_le_bytes([chunk[0], chunk[1]]));
    }
    Ok(units)
}

fn write_u16le_units(units: &[u16]) -> Vec<u8> {
    let mut out = Vec::with_capacity(units.len() * 2);
    for u in units {
        out.extend_from_slice(&u.to_le_bytes());
    }
    out
}

/// Split UTF-16 units on CRLF (0x000D 0x000A).
fn split_records(units: &[u16]) -> Vec<Vec<u16>> {
    let mut records = Vec::new();
    let mut start = 0usize;
    let mut i = 0usize;
    while i < units.len() {
        if units[i] == 0x000D && i + 1 < units.len() && units[i + 1] == 0x000A {
            records.push(units[start..i].to_vec());
            i += 2;
            start = i;
        } else {
            i += 1;
        }
    }
    if start < units.len() {
        records.push(units[start..].to_vec());
    } else if start == units.len() && !units.is_empty() {
        // File ended with CRLF → trailing empty record (harmless).
        records.push(Vec::new());
    }
    records
}

fn join_records(records: &[Vec<u16>]) -> Vec<u16> {
    let mut out = Vec::new();
    for rec in records {
        out.extend_from_slice(rec);
        out.push(0x000D);
        out.push(0x000A);
    }
    out
}

// ─── Game model ────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
struct QspAction {
    image: String,
    name: String,
    code: String,
}

#[derive(Clone, Debug)]
struct QspLocation {
    name: String,
    description: String,
    code: String,
    actions: Vec<QspAction>,
}

#[derive(Clone, Debug)]
struct QspGame {
    version: String,
    password: String,
    locations: Vec<QspLocation>,
}

fn parse_game(bytes: &[u8], file_label: &str) -> Result<QspGame> {
    let units = read_u16le_units(bytes)?;
    let records = split_records(&units);
    if records.is_empty() {
        return Err(LocustError::ParseError {
            file: file_label.into(),
            message: "empty QSP file".into(),
        });
    }

    // Header game id is plain (not CODREMOV-shifted) — matches engine compare.
    let header = String::from_utf16_lossy(&records[0]);
    if header != QSP_GAMEID {
        return Err(LocustError::ParseError {
            file: file_label.into(),
            message: format!(
                "legacy pre-QSPGAME or unknown header {header:?}; only modern UTF-16LE QSPGAME is supported"
            ),
        });
    }
    if records.len() < 4 {
        return Err(LocustError::ParseError {
            file: file_label.into(),
            message: "truncated QSPGAME header".into(),
        });
    }

    let version = String::from_utf16_lossy(&records[1]);
    let password = decode_field(&records[2]);
    let loc_count: usize = decode_field(&records[3]).parse().map_err(|_| {
        LocustError::ParseError {
            file: file_label.into(),
            message: format!(
                "invalid location count field: {:?}",
                decode_field(&records[3])
            ),
        }
    })?;

    let mut idx = 4;
    let mut locations = Vec::with_capacity(loc_count);
    for _ in 0..loc_count {
        if idx + 3 > records.len() {
            return Err(LocustError::ParseError {
                file: file_label.into(),
                message: "truncated location block".into(),
            });
        }
        let name = decode_field(&records[idx]);
        let description = decode_field(&records[idx + 1]);
        let code = decode_field(&records[idx + 2]);
        idx += 3;
        if idx >= records.len() {
            return Err(LocustError::ParseError {
                file: file_label.into(),
                message: "missing actsCount".into(),
            });
        }
        let acts_count: usize = decode_field(&records[idx]).parse().map_err(|_| {
            LocustError::ParseError {
                file: file_label.into(),
                message: "invalid actsCount".into(),
            }
        })?;
        idx += 1;
        let mut actions = Vec::with_capacity(acts_count);
        for _ in 0..acts_count {
            if idx + 3 > records.len() {
                return Err(LocustError::ParseError {
                    file: file_label.into(),
                    message: "truncated action block".into(),
                });
            }
            actions.push(QspAction {
                image: decode_field(&records[idx]),
                name: decode_field(&records[idx + 1]),
                code: decode_field(&records[idx + 2]),
            });
            idx += 3;
        }
        locations.push(QspLocation {
            name,
            description,
            code,
            actions,
        });
    }

    Ok(QspGame {
        version,
        password,
        locations,
    })
}

fn serialize_game(game: &QspGame) -> Vec<u8> {
    // Plain header lines (no CODREMOV), matching converters writeQsp.
    let mut records: Vec<Vec<u16>> = vec![
        QSP_GAMEID.encode_utf16().collect(),
        game.version.encode_utf16().collect(),
        encode_field(&game.password),
        encode_field(&game.locations.len().to_string()),
    ];

    for loc in &game.locations {
        records.push(encode_field(&loc.name));
        records.push(encode_field(&loc.description));
        records.push(encode_field(&loc.code));
        records.push(encode_field(&loc.actions.len().to_string()));
        for act in &loc.actions {
            records.push(encode_field(&act.image));
            records.push(encode_field(&act.name));
            records.push(encode_field(&act.code));
        }
    }

    write_u16le_units(&join_records(&records))
}

// ─── Extraction helpers ────────────────────────────────────────────────────

/// Pull double-quoted and single-quoted string literals from a QSP code block.
fn extract_quoted_literals(code: &str) -> Vec<String> {
    let mut out = Vec::new();
    let chars: Vec<char> = code.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let q = chars[i];
        if q == '"' || q == '\'' {
            i += 1;
            let mut buf = String::new();
            while i < chars.len() && chars[i] != q {
                // QSP doubles a quote to escape: '' or ""
                if i + 1 < chars.len() && chars[i] == q && chars[i + 1] == q {
                    buf.push(q);
                    i += 2;
                    continue;
                }
                buf.push(chars[i]);
                i += 1;
            }
            if i < chars.len() {
                i += 1; // closing quote
            }
            if looks_player_visible(&buf) {
                out.push(buf);
            }
        } else {
            i += 1;
        }
    }
    out
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
    if (t.contains('/') || t.contains('\\')) && !t.contains(' ') {
        return false;
    }
    t.chars().any(|c| c.is_alphabetic())
}

fn game_to_entries(game: &QspGame, filename: &str, file_path: &Path) -> Vec<StringEntry> {
    let mut entries = Vec::new();
    for (li, loc) in game.locations.iter().enumerate() {
        if looks_player_visible(&loc.description) {
            let id = format!("{filename}#loc{li}#desc");
            let mut e = StringEntry::new(id, &loc.description, file_path.to_path_buf());
            e.tags = vec!["description".into()];
            e.context = Some(format!("location:{}", loc.name));
            entries.push(e);
        }
        for (qi, lit) in extract_quoted_literals(&loc.code).into_iter().enumerate() {
            let id = format!("{filename}#loc{li}#code#str{qi}");
            let mut e = StringEntry::new(id, &lit, file_path.to_path_buf());
            e.tags = vec!["code_string".into()];
            e.context = Some(format!("location:{} code", loc.name));
            entries.push(e);
        }
        for (ai, act) in loc.actions.iter().enumerate() {
            if looks_player_visible(&act.name) {
                let id = format!("{filename}#loc{li}#act{ai}#name");
                let mut e = StringEntry::new(id, &act.name, file_path.to_path_buf());
                e.tags = vec!["action".into()];
                e.context = Some(format!("location:{} action", loc.name));
                entries.push(e);
            }
            for (qi, lit) in extract_quoted_literals(&act.code).into_iter().enumerate() {
                let id = format!("{filename}#loc{li}#act{ai}#code#str{qi}");
                let mut e = StringEntry::new(id, &lit, file_path.to_path_buf());
                e.tags = vec!["code_string".into()];
                e.context = Some(format!("location:{} action code", loc.name));
                entries.push(e);
            }
        }
    }
    entries
}

fn apply_translations(game: &mut QspGame, entries: &[&StringEntry]) {
    let mut by_suffix: HashMap<String, &str> = HashMap::new();
    for e in entries {
        if let Some(t) = e.translation.as_deref() {
            if let Some(pos) = e.id.find("#loc") {
                by_suffix.insert(e.id[pos + 1..].to_string(), t);
            }
        }
    }

    for (li, loc) in game.locations.iter_mut().enumerate() {
        let desc_key = format!("loc{li}#desc");
        if let Some(t) = by_suffix.get(&desc_key) {
            loc.description = (*t).to_string();
        }
        let mut code_lits = extract_quoted_literals(&loc.code);
        let mut changed = false;
        for (qi, lit) in code_lits.iter_mut().enumerate() {
            let key = format!("loc{li}#code#str{qi}");
            if let Some(t) = by_suffix.get(&key) {
                *lit = (*t).to_string();
                changed = true;
            }
        }
        if changed {
            loc.code = replace_quoted_in_order(&loc.code, &code_lits);
        }

        for (ai, act) in loc.actions.iter_mut().enumerate() {
            let name_key = format!("loc{li}#act{ai}#name");
            if let Some(t) = by_suffix.get(&name_key) {
                act.name = (*t).to_string();
            }
            let mut act_lits = extract_quoted_literals(&act.code);
            let mut act_changed = false;
            for (qi, lit) in act_lits.iter_mut().enumerate() {
                let key = format!("loc{li}#act{ai}#code#str{qi}");
                if let Some(t) = by_suffix.get(&key) {
                    *lit = (*t).to_string();
                    act_changed = true;
                }
            }
            if act_changed {
                act.code = replace_quoted_in_order(&act.code, &act_lits);
            }
        }
    }
}

/// Rewrite quoted literals in `code` left-to-right with `replacements`
/// (same order / filter as [`extract_quoted_literals`]).
fn replace_quoted_in_order(code: &str, replacements: &[String]) -> String {
    let chars: Vec<char> = code.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    let mut rep_i = 0;
    while i < chars.len() {
        let q = chars[i];
        if q == '"' || q == '\'' {
            let start = i;
            i += 1;
            let mut buf = String::new();
            while i < chars.len() && chars[i] != q {
                if i + 1 < chars.len() && chars[i] == q && chars[i + 1] == q {
                    buf.push(q);
                    i += 2;
                    continue;
                }
                buf.push(chars[i]);
                i += 1;
            }
            if i < chars.len() {
                i += 1;
            }
            if looks_player_visible(&buf) && rep_i < replacements.len() {
                out.push(q);
                for c in replacements[rep_i].chars() {
                    if c == q {
                        out.push(q);
                        out.push(q);
                    } else {
                        out.push(c);
                    }
                }
                out.push(q);
                rep_i += 1;
            } else {
                for ch in &chars[start..i] {
                    out.push(*ch);
                }
            }
        } else {
            out.push(chars[i]);
            i += 1;
        }
    }
    out
}

impl FormatPlugin for QspPlugin {
    fn id(&self) -> &str {
        "qsp"
    }

    fn name(&self) -> &str {
        "QSP (QuestSoft Player)"
    }

    fn description(&self) -> &str {
        "QuestSoft Player binary games (.qsp / .gam) — UTF-16LE QSPGAME"
    }

    fn stability(&self) -> locust_core::extraction::FormatStability {
        // Phase-2 style: synthetic QSPGAME fixture only (no commercial title in library).
        locust_core::extraction::FormatStability::Experimental
    }

    fn supported_extensions(&self) -> &[&str] {
        &[".qsp", ".gam"]
    }

    fn supported_modes(&self) -> Vec<OutputMode> {
        vec![OutputMode::Replace]
    }

    fn detect(&self, path: &Path) -> bool {
        !Self::find_qsp_files(path).is_empty()
    }

    fn extract(&self, path: &Path) -> Result<Vec<StringEntry>> {
        let files = Self::find_qsp_files(path);
        if files.is_empty() {
            return Err(LocustError::ParseError {
                file: path.display().to_string(),
                message: "no .qsp or .gam files found".into(),
            });
        }
        let mut all = Vec::new();
        for fpath in &files {
            let bytes = std::fs::read(fpath)?;
            let fname = fpath
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            let game = parse_game(&bytes, &fname)?;
            all.extend(game_to_entries(&game, &fname, fpath));
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
                warnings.push(format!("missing game file {}", file_path.display()));
                strings_skipped += file_entries.len();
                continue;
            }
            let bytes = std::fs::read(&actual)?;
            let fname = actual
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            let mut game = match parse_game(&bytes, &fname) {
                Ok(g) => g,
                Err(e) => {
                    warnings.push(format!("cannot parse {}: {e}", actual.display()));
                    strings_skipped += file_entries.len();
                    continue;
                }
            };

            let before = serialize_game(&game);
            apply_translations(&mut game, file_entries);
            let after = serialize_game(&game);
            if after == before {
                strings_skipped += file_entries.len();
                continue;
            }
            std::fs::write(&actual, &after)?;
            files_modified += 1;
            files_written.push(actual);
            strings_written += file_entries
                .iter()
                .filter(|e| e.translation.is_some())
                .count();
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
        let dir = std::env::temp_dir().join(format!("locust_qsp_{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Minimal valid UTF-16LE QSPGAME matching converters `writeQsp` layout.
    fn build_minimal_qsp_game() -> Vec<u8> {
        let game = QspGame {
            version: "locust-test 1.0".into(),
            password: "No".into(),
            locations: vec![QspLocation {
                name: "start".into(),
                description: "You stand in a quiet forest clearing.".into(),
                code: "*pl 'Hello, traveler!'\r\n*pl \"Welcome to the quest.\"".into(),
                actions: vec![QspAction {
                    image: String::new(),
                    name: "Look around".into(),
                    code: "*pl 'You see trees and a path.'".into(),
                }],
            }],
        };
        serialize_game(&game)
    }

    fn create_qsp_fixture(dir: &Path) -> PathBuf {
        let path = dir.join("game.qsp");
        fs::write(&path, build_minimal_qsp_game()).unwrap();
        dir.to_path_buf()
    }

    #[test]
    fn test_codremov_roundtrip_units() {
        // NUL never occurs in QSP string fields; encode(0) intentionally collides with
        // encode(5) (both store 0xFFFB) per the engine transform — do not roundtrip 0.
        for ch in [5u16, 32, 65, 0x0410, 0x4E00] {
            assert_eq!(decode_unit(encode_unit(ch)), ch, "unit {ch:#x}");
        }
        // Char 5 stores as (u16)-5 per engine coding.c
        assert_eq!(encode_unit(5), (0u16).wrapping_sub(5));
        assert_eq!(decode_unit((0u16).wrapping_sub(5)), 5);
        assert_eq!(decode_field(&encode_field("Hello")), "Hello");
        assert_eq!(decode_field(&encode_field("Привет")), "Привет");
    }

    #[test]
    fn test_detect_qsp_dir() {
        let dir = tempdir();
        create_qsp_fixture(&dir);
        let plugin = QspPlugin::new();
        assert!(plugin.detect(&dir));
    }

    #[test]
    fn test_detect_non_qsp() {
        let dir = tempdir();
        fs::write(dir.join("readme.txt"), b"nope").unwrap();
        let plugin = QspPlugin::new();
        assert!(!plugin.detect(&dir));
    }

    #[test]
    fn test_extract_known_strings_and_stable_ids() {
        let dir = tempdir();
        create_qsp_fixture(&dir);
        let plugin = QspPlugin::new();
        let entries = plugin.extract(&dir).unwrap();
        let sources: Vec<&str> = entries.iter().map(|e| e.source.as_str()).collect();
        assert!(
            sources.iter().any(|s| s.contains("quiet forest")),
            "description missing: {sources:?}"
        );
        assert!(
            sources.contains(&"Look around"),
            "action name missing: {sources:?}"
        );
        assert!(
            sources.iter().any(|s| s.contains("Hello, traveler")),
            "code string missing: {sources:?}"
        );
        // Stable id pattern
        assert!(
            entries.iter().any(|e| e.id.contains("#loc0#desc")),
            "ids: {:?}",
            entries.iter().map(|e| &e.id).collect::<Vec<_>>()
        );
        // No binary_slot — variable-length records
        for e in &entries {
            assert!(
                !e.metadata.contains_key("binary_slot"),
                "QSP must not set binary_slot"
            );
        }
    }

    #[test]
    fn test_inject_roundtrip() {
        let dir = tempdir();
        create_qsp_fixture(&dir);
        let plugin = QspPlugin::new();
        let mut entries = plugin.extract(&dir).unwrap();
        for e in &mut entries {
            if e.source.contains("quiet forest") {
                e.translation = Some("Estás en un claro del bosque.".into());
            }
            if e.source == "Look around" {
                e.translation = Some("Mirar alrededor".into());
            }
            if e.source.contains("Hello, traveler") {
                e.translation = Some("¡Hola, viajero!".into());
            }
        }
        let report = plugin.inject(&dir, &entries).unwrap();
        assert!(report.files_modified >= 1, "{report:?}");

        let again = plugin.extract(&dir).unwrap();
        let sources: Vec<&str> = again.iter().map(|e| e.source.as_str()).collect();
        assert!(
            sources.iter().any(|s| s.contains("claro del bosque")),
            "re-extract missing desc: {sources:?}"
        );
        assert!(
            sources.contains(&"Mirar alrededor"),
            "re-extract missing action: {sources:?}"
        );
        assert!(
            sources.iter().any(|s| s.contains("Hola, viajero")),
            "re-extract missing code string: {sources:?}"
        );
    }

    #[test]
    fn test_legacy_header_is_reported_not_silent() {
        let dir = tempdir();
        // Fake single-byte / non-QSPGAME content with .qsp extension
        let path = dir.join("old.qsp");
        // High byte nonzero → CP1251 path error
        fs::write(&path, b"not-a-real-qsp-file!!!!").unwrap();
        let plugin = QspPlugin::new();
        let err = plugin.extract(&dir).unwrap_err().to_string();
        assert!(
            err.contains("not supported") || err.contains("QSPGAME") || err.contains("single-byte"),
            "expected clear skip message, got: {err}"
        );
    }

    #[test]
    fn test_stability_is_experimental() {
        let plugin = QspPlugin::new();
        assert_eq!(
            plugin.stability(),
            locust_core::extraction::FormatStability::Experimental
        );
    }
}
