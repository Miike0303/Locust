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

use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::path::{Path, PathBuf};

use locust_core::error::{LocustError, Result};
use locust_core::extraction::{FormatPlugin, InjectionReport};
use locust_core::models::{OutputMode, StringEntry};

/// Per-character code shift used by QSP string fields (engine `QSP_CODREMOV`).
const QSP_CODREMOV: u16 = 5;
const QSP_GAMEID: &str = "QSPGAME";
/// Bound model allocations even when the input contains enough empty records.
const MAX_QSP_COUNT: usize = 100_000;

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

fn parse_error(file: &str, message: impl Into<String>) -> LocustError {
    LocustError::ParseError {
        file: file.into(),
        message: message.into(),
    }
}

fn decode_field(units: &[u16]) -> Result<String> {
    let decoded: Vec<u16> = units.iter().copied().map(decode_unit).collect();
    String::from_utf16(&decoded).map_err(|_| parse_error("qsp", "invalid UTF-16 in encoded field"))
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
    let mut out = Vec::new();
    for u in units {
        out.extend_from_slice(&u.to_le_bytes());
    }
    out
}

/// Record payload ranges exclude CRLF and retain an unterminated final record.
/// A terminal CRLF is a terminator, not an additional empty record.
fn record_ranges(units: &[u16]) -> Result<Vec<Range<usize>>> {
    let mut ranges = Vec::new();
    let mut start = 0;
    for (i, pair) in units.windows(2).enumerate() {
        if pair == [13, 10] {
            ranges.push(start..i);
            // The window proves both units exist within this allocated slice.
            start = i
                .checked_add(pair.len())
                .ok_or_else(|| parse_error("qsp", "record offset overflow"))?;
        }
    }
    if start < units.len() {
        ranges.push(start..units.len());
    }
    Ok(ranges)
}

#[cfg(test)]
fn split_records(units: &[u16]) -> Vec<Vec<u16>> {
    let mut records: Vec<_> = record_ranges(units)
        .unwrap()
        .into_iter()
        .map(|range| units[range].to_vec())
        .collect();
    if units.ends_with(&[13, 10]) {
        records.push(Vec::new());
    }
    records
}

#[cfg(test)]
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

fn next_field<'a>(
    records: &mut impl Iterator<Item = &'a [u16]>,
    encoded: bool,
    file: &str,
) -> Result<String> {
    let units = records
        .next()
        .ok_or_else(|| parse_error(file, "truncated QSP record block"))?;
    if encoded {
        decode_field(units).map_err(|_| parse_error(file, "invalid UTF-16 in encoded field"))
    } else {
        String::from_utf16(units).map_err(|_| parse_error(file, "invalid UTF-16 in plain field"))
    }
}

fn next_count<'a>(
    records: &mut impl ExactSizeIterator<Item = &'a [u16]>,
    records_per_item: usize,
    kind: &str,
    file: &str,
) -> Result<usize> {
    let text = next_field(records, true, file)?;
    let count: usize = text
        .parse()
        .map_err(|_| parse_error(file, format!("invalid {kind} count: {text:?}")))?;
    if count > MAX_QSP_COUNT {
        return Err(parse_error(
            file,
            format!("{kind} count {count} exceeds limit {MAX_QSP_COUNT}"),
        ));
    }
    // Division avoids count multiplication overflow and precedes allocation.
    if count > records.len() / records_per_item {
        return Err(parse_error(
            file,
            format!(
                "{kind} count {count} exceeds remaining records ({})",
                records.len()
            ),
        ));
    }
    Ok(count)
}

fn parse_game(bytes: &[u8], file_label: &str) -> Result<QspGame> {
    let units = read_u16le_units(bytes).map_err(|e| parse_error(file_label, e.to_string()))?;
    let ranges = record_ranges(&units)?;
    let mut records = ranges.iter().map(|range| &units[range.clone()]);
    let header = next_field(&mut records, false, file_label)?;
    if header != QSP_GAMEID {
        return Err(parse_error(file_label, format!(
            "legacy pre-QSPGAME or unknown header {header:?}; only modern UTF-16LE QSPGAME is supported"
        )));
    }
    let version = next_field(&mut records, false, file_label)?;
    let password = next_field(&mut records, true, file_label)?;
    let loc_count = next_count(&mut records, 4, "location", file_label)?;
    let mut locations = Vec::with_capacity(loc_count);
    for _ in 0..loc_count {
        let name = next_field(&mut records, true, file_label)?;
        let description = next_field(&mut records, true, file_label)?;
        let code = next_field(&mut records, true, file_label)?;
        let acts_count = next_count(&mut records, 3, "action", file_label)?;
        let mut actions = Vec::with_capacity(acts_count);
        for _ in 0..acts_count {
            actions.push(QspAction {
                image: next_field(&mut records, true, file_label)?,
                name: next_field(&mut records, true, file_label)?,
                code: next_field(&mut records, true, file_label)?,
            });
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

/// Borrow record values in file order; None marks structural records.
fn game_record_fields(game: &QspGame) -> Vec<Option<&str>> {
    let mut fields = vec![
        None,
        Some(game.version.as_str()),
        Some(game.password.as_str()),
        None,
    ];
    for location in &game.locations {
        fields.extend([
            Some(location.name.as_str()),
            Some(location.description.as_str()),
            Some(location.code.as_str()),
            None,
        ]);
        for action in &location.actions {
            fields.extend([
                Some(action.image.as_str()),
                Some(action.name.as_str()),
                Some(action.code.as_str()),
            ]);
        }
    }
    fields
}

/// Encode only changed payloads; copy all other bytes from the original file.
fn splice_game(bytes: &[u8], before: &QspGame, after: &QspGame, file: &str) -> Result<Vec<u8>> {
    let units = read_u16le_units(bytes)?;
    let ranges = record_ranges(&units)?;
    let mut output = Vec::new();
    let mut cursor = 0;
    for ((old, new), range) in game_record_fields(before)
        .into_iter()
        .zip(game_record_fields(after))
        .zip(ranges)
    {
        if old == new {
            continue;
        }
        if let Some(text) = new {
            let start = range
                .start
                .checked_mul(2)
                .ok_or_else(|| parse_error(file, "record byte offset overflow"))?;
            let end = range
                .end
                .checked_mul(2)
                .ok_or_else(|| parse_error(file, "record byte offset overflow"))?;
            output.extend_from_slice(&bytes[cursor..start]);
            output.extend(write_u16le_units(&encode_field(text)));
            cursor = end;
        }
    }
    output.extend_from_slice(&bytes[cursor..]);
    Ok(output)
}

#[cfg(test)]
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

/// Scan one literal, returning its unescaped text and the index after it.
/// Unterminated literals consume the remaining code, as in the original scanner.
fn scan_quoted_literal(chars: &[char], start: usize) -> (String, usize) {
    let q = chars[start];
    let mut i = start + 1;
    let mut buf = String::new();
    while i < chars.len() {
        if chars[i] == q {
            // QSP doubles a quote to escape: '' or ""
            if chars.get(i + 1) == Some(&q) {
                buf.push(q);
                i += 2;
                continue;
            }
            i += 1; // closing quote
            break;
        }
        buf.push(chars[i]);
        i += 1;
    }
    (buf, i)
}

fn location_names(game: &QspGame) -> HashSet<String> {
    game.locations
        .iter()
        .map(|location| location.name.to_lowercase())
        .collect()
}

/// Retain a slot for every historically player-visible literal. Protected slots
/// are None, so removing executable references never renumbers later row IDs.
fn extract_quoted_literals(code: &str, locations: &HashSet<String>) -> Vec<Option<String>> {
    let mut out = Vec::new();
    let chars: Vec<char> = code.chars().collect();
    let mut i = 0;
    let mut statement_start = true;
    let mut depth = 0usize;
    // (parenthesis depth of first argument, protect only simple identifiers).
    // An outer first argument remains protected across nested calls/commas.
    let mut first_arguments: Vec<(usize, bool)> = Vec::new();
    while i < chars.len() {
        let ch = chars[i];
        if ch == '"' || ch == '\'' {
            let (buf, end) = scan_quoted_literal(&chars, i);
            i = end;
            if looks_player_visible(&buf) {
                let identifier = buf.chars().all(|c| c.is_alphanumeric() || c == '_');
                let protected = locations.contains(&buf.to_lowercase())
                    || first_arguments
                        .iter()
                        .any(|&(_, identifiers_only)| !identifiers_only || identifier);
                out.push((!protected).then_some(buf));
            }
            statement_start = false;
        } else if ch.is_alphanumeric() || ch == '_' || ch == '$' {
            let start = i;
            while i < chars.len()
                && (chars[i].is_alphanumeric() || chars[i] == '_' || chars[i] == '$')
            {
                i += 1;
            }
            let token: String = chars[start..i].iter().collect();
            let token = token.to_ascii_lowercase();
            if token == "else" {
                first_arguments.clear();
                statement_start = true;
                continue;
            }
            let next = chars[i..]
                .iter()
                .find(|c| !c.is_whitespace() || matches!(c, '\r' | '\n'));
            let command = statement_start
                && matches!(
                    token.as_str(),
                    "gt" | "goto" | "xgt" | "xgoto" | "gs" | "gosub" | "dynamic"
                );
            let function = matches!(token.as_str(), "func" | "$func" | "dyneval" | "$dyneval")
                && matches!(next, Some('(' | '\'' | '"'));
            if command || function {
                let argument_depth = depth + usize::from(next == Some(&'('));
                // DYNAMIC/DYNEVAL take code, not a location name. Only treat
                // their simple symbolic literals as identifiers here.
                let identifiers_only = matches!(token.as_str(), "dynamic" | "dyneval" | "$dyneval");
                first_arguments.push((argument_depth, identifiers_only));
            }
            statement_start = false;
        } else {
            match ch {
                '\r' | '\n' | '&' | ':' => {
                    first_arguments.clear();
                    depth = 0;
                    statement_start = true;
                }
                '(' => {
                    depth += 1;
                    statement_start = false;
                }
                ')' => {
                    first_arguments.retain(|&(argument_depth, _)| argument_depth < depth);
                    depth = depth.saturating_sub(1);
                    statement_start = false;
                }
                ',' => {
                    first_arguments.retain(|&(argument_depth, _)| argument_depth < depth);
                    statement_start = false;
                }
                c if c.is_whitespace() => {}
                _ => statement_start = false,
            }
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
    let locations = location_names(game);
    for (li, loc) in game.locations.iter().enumerate() {
        if looks_player_visible(&loc.description) {
            let id = format!("{filename}#loc{li}#desc");
            let mut e = StringEntry::new(id, &loc.description, file_path.to_path_buf());
            e.tags = vec!["description".into()];
            e.context = Some(format!("location:{}", loc.name));
            entries.push(e);
        }
        for (qi, lit) in extract_quoted_literals(&loc.code, &locations)
            .into_iter()
            .enumerate()
        {
            let Some(lit) = lit else { continue };
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
            for (qi, lit) in extract_quoted_literals(&act.code, &locations)
                .into_iter()
                .enumerate()
            {
                let Some(lit) = lit else { continue };
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
    let locations = location_names(game);
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
        let mut code_lits = extract_quoted_literals(&loc.code, &locations);
        let mut changed = false;
        for (qi, lit) in code_lits.iter_mut().enumerate() {
            let Some(lit) = lit else { continue };
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
            let mut act_lits = extract_quoted_literals(&act.code, &locations);
            let mut act_changed = false;
            for (qi, lit) in act_lits.iter_mut().enumerate() {
                let Some(lit) = lit else { continue };
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

/// Validate rows against the unmodified source; the last valid row wins.
fn validated_translations<'a>(
    game: &QspGame,
    entries: &[&'a StringEntry],
    report: &mut InjectionReport,
) -> Vec<&'a StringEntry> {
    let locations = location_names(game);
    let mut seen = std::collections::HashSet::new();
    let mut accepted = Vec::new();
    for entry in entries.iter().rev() {
        let reason = if entry.translation.is_none() {
            "untranslated"
        } else if let Some((_, suffix)) = entry.id.split_once("#loc") {
            match qsp_locator_target(game, suffix, &locations) {
                Err(reason) => reason,
                Ok(source) if source != entry.source => "source_changed",
                Ok(_) if !seen.insert(suffix) => "duplicate",
                Ok(_) if entry.translation.as_ref() == Some(&entry.source) => "unchanged",
                Ok(_) => {
                    accepted.push(*entry);
                    continue;
                }
            }
        } else {
            "invalid_locator"
        };
        report.skip(reason, 1);
    }
    accepted
}

fn qsp_locator_target(
    game: &QspGame,
    suffix: &str,
    locations: &HashSet<String>,
) -> std::result::Result<String, &'static str> {
    fn index(value: &str) -> std::result::Result<usize, &'static str> {
        let index: usize = value.parse().map_err(|_| "invalid_locator")?;
        if index.to_string() != value {
            return Err("invalid_locator");
        }
        Ok(index)
    }
    let parts: Vec<_> = suffix.split('#').collect();
    let location = game
        .locations
        .get(index(parts[0])?)
        .ok_or("target_missing")?;
    let code = match parts[1..] {
        ["desc"] => {
            return looks_player_visible(&location.description)
                .then(|| location.description.clone())
                .ok_or("target_missing")
        }
        ["code", literal] => Some((&location.code, literal)),
        [action, "name"] | [action, "code", _] if action.starts_with("act") => {
            let action = location
                .actions
                .get(index(&action[3..])?)
                .ok_or("target_missing")?;
            if parts.len() == 3 {
                return looks_player_visible(&action.name)
                    .then(|| action.name.clone())
                    .ok_or("target_missing");
            }
            Some((&action.code, parts[3]))
        }
        _ => None,
    };
    let (code, literal) = code.ok_or("invalid_locator")?;
    let literal = index(literal.strip_prefix("str").ok_or("invalid_locator")?)?;
    extract_quoted_literals(code, locations)
        .get(literal)
        .cloned()
        .flatten()
        .ok_or("target_missing")
}

/// Rewrite quoted literals in `code` left-to-right with `replacements`
/// (same slots as [`extract_quoted_literals`]); None copies protected text verbatim.
fn replace_quoted_in_order(code: &str, replacements: &[Option<String>]) -> String {
    let chars: Vec<char> = code.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    let mut rep_i = 0;
    while i < chars.len() {
        let q = chars[i];
        if q == '"' || q == '\'' {
            let start = i;
            let (buf, end) = scan_quoted_literal(&chars, start);
            i = end;
            if looks_player_visible(&buf) {
                let replacement = replacements.get(rep_i).and_then(Option::as_deref);
                rep_i += 1;
                let Some(replacement) = replacement.filter(|text| *text != buf) else {
                    out.extend(chars[start..i].iter());
                    continue;
                };
                out.push(q);
                for c in replacement.chars() {
                    if c == q {
                        out.push(q);
                        out.push(q);
                    } else {
                        out.push(c);
                    }
                }
                out.push(q);
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
                report
                    .warnings
                    .push(format!("missing game file {}", file_path.display()));
                report.skip("target_missing", file_entries.len());
                continue;
            }
            let bytes = std::fs::read(&actual)?;
            let fname = actual
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            let game = match parse_game(&bytes, &fname) {
                Ok(g) => g,
                Err(e) => {
                    report
                        .warnings
                        .push(format!("cannot parse {}: {e}", actual.display()));
                    report.skip("decode_error", file_entries.len());
                    continue;
                }
            };

            let accepted = validated_translations(&game, file_entries, &mut report);
            if accepted.is_empty() {
                continue;
            }
            let mut translated = game.clone();
            apply_translations(&mut translated, &accepted);
            let after = splice_game(&bytes, &game, &translated, &fname)?;
            if after == bytes {
                report.skip("unchanged", accepted.len());
                continue;
            }
            std::fs::write(&actual, &after)?;
            report.files_modified += 1;
            report.files_written.push(actual);
            report.strings_written += accepted.len();
        }

        Ok(report)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn fixture_records() -> Vec<Vec<u16>> {
        let bytes = build_minimal_qsp_game();
        let mut records = split_records(&read_u16le_units(&bytes).unwrap());
        records.pop(); // The fixture ends with CRLF.
        records
    }

    #[test]
    fn qsp_rejects_huge_counts_before_allocation() {
        // Start with a guaranteed capacity overflow so the unfixed parser cannot
        // attempt a multi-GB allocation on the subsequent u32::MAX cases.
        for field in [3, 7] {
            for count in [usize::MAX, u32::MAX as usize, 20] {
                let mut records = fixture_records();
                records[field] = encode_field(&count.to_string());
                let bytes = write_u16le_units(&join_records(&records));
                let result = std::panic::catch_unwind(|| parse_game(&bytes, "crafted.qsp"));
                assert!(result.is_ok(), "count {count} in field {field} panicked");
                let error = result.unwrap().unwrap_err();
                assert!(matches!(error, LocustError::ParseError { .. }));
                assert!(error.to_string().contains("count"), "{error}");
            }
        }
    }

    #[test]
    fn qsp_rejects_truncated_action_block() {
        for length in 8..11 {
            let mut records = fixture_records();
            records.truncate(length);
            let bytes = write_u16le_units(&join_records(&records));
            assert!(
                matches!(
                    parse_game(&bytes, "truncated.qsp"),
                    Err(LocustError::ParseError { .. })
                ),
                "accepted an action with {} fields",
                length - 8
            );
        }
    }

    #[test]
    fn qsp_count_limit_applies_even_with_enough_records() {
        for actions in [false, true] {
            let mut records = fixture_records();
            let count = 100_001;
            if actions {
                records.truncate(8);
                records[7] = encode_field(&count.to_string());
                records.extend((0..count * 3).map(|_| Vec::new()));
            } else {
                records.truncate(4);
                records[3] = encode_field(&count.to_string());
                for _ in 0..count {
                    records.extend([Vec::new(), Vec::new(), Vec::new(), encode_field("0")]);
                }
            }
            let bytes = write_u16le_units(&join_records(&records));
            let error = parse_game(&bytes, "limit.qsp").unwrap_err();
            assert!(matches!(error, LocustError::ParseError { .. }));
            assert!(
                error.to_string().contains("exceeds limit 100000"),
                "{error}"
            );
        }
    }

    #[test]
    fn qsp_edit_preserves_absent_final_terminator() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("game.qsp");
        let mut before = build_minimal_qsp_game();
        before.truncate(before.len() - 4);
        fs::write(&path, &before).unwrap();
        let plugin = QspPlugin::new();
        let mut entries = plugin.extract(&path).unwrap();
        let last = entries.last_mut().unwrap();
        last.translation = Some("Translated action code".into());
        let old_record = write_u16le_units(&encode_field("*pl 'You see trees and a path.'"));
        let mut expected = before[..before.len() - old_record.len()].to_vec();
        expected.extend(write_u16le_units(&encode_field(
            "*pl 'Translated action code'",
        )));
        let report = plugin.inject(&path, &entries).unwrap();
        assert_eq!((report.files_modified, report.strings_written), (1, 1));
        assert_eq!(fs::read(&path).unwrap(), expected);
    }

    #[test]
    fn qsp_extract_rejects_invalid_utf16() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("invalid.qsp");
        // Include plain header/version and shifted fields, including fields
        // that are validated but never extracted as translatable strings.
        for field in 0..11 {
            for surrogate in [0xD800, 0xDC00] {
                let mut records = fixture_records();
                records[field] = vec![if field < 2 {
                    surrogate
                } else {
                    encode_unit(surrogate)
                }];
                fs::write(&path, write_u16le_units(&join_records(&records))).unwrap();
                assert!(
                    matches!(
                        QspPlugin::new().extract(&path),
                        Err(LocustError::ParseError { .. })
                    ),
                    "field {field} accepted an unpaired surrogate"
                );
            }
        }
        let mut bytes = build_minimal_qsp_game();
        bytes.push(0x42);
        fs::write(&path, bytes).unwrap();
        assert!(matches!(
            QspPlugin::new().extract(&path),
            Err(LocustError::ParseError { .. })
        ));
    }

    #[test]
    fn qsp_inject_preserves_unedited_records_and_trailing_bytes() {
        for terminated in [false, true] {
            for target in ["#desc", "#code#str0", "#act0#name", "#act0#code#str0"] {
                let dir = tempfile::tempdir().unwrap();
                let path = dir.path().join("game.qsp");
                let mut records = fixture_records();
                records[3] = encode_field("0001");
                records[7] = encode_field("01");
                // An alternate encoding of code unit 5 must remain untouched.
                records[8] = vec![0];
                // An untouched unterminated literal must not gain a quote.
                records[6] = encode_field("*pl 'Hello, traveler!' & 'Unterminated text");
                let mut before = write_u16le_units(&join_records(&records));
                before.extend(write_u16le_units(&[0xD800, 13, 10, 0xDC00]));
                if terminated {
                    before.extend_from_slice(&[13, 0, 10, 0]);
                }
                fs::write(&path, &before).unwrap();
                let plugin = QspPlugin::new();
                let mut entry = plugin
                    .extract(&path)
                    .unwrap()
                    .into_iter()
                    .find(|e| e.id == format!("game.qsp#loc0{target}"))
                    .unwrap();
                entry.translation = Some("Translated text".into());
                let report = plugin.inject(&path, &[entry]).unwrap();
                assert_eq!((report.files_modified, report.strings_written), (1, 1));
                let record = match target {
                    "#desc" => 5,
                    "#code#str0" => 6,
                    "#act0#name" => 9,
                    _ => 10,
                };
                let replacement = match record {
                    6 => "*pl 'Translated text' & 'Unterminated text",
                    10 => "*pl 'Translated text'",
                    _ => "Translated text",
                };
                let start: usize = records[..record].iter().map(|r| (r.len() + 2) * 2).sum();
                let end = start + records[record].len() * 2;
                let mut expected = before[..start].to_vec();
                expected.extend(write_u16le_units(&encode_field(replacement)));
                expected.extend_from_slice(&before[end..]);
                assert_eq!(
                    fs::read(&path).unwrap(),
                    expected,
                    "{target}, terminated={terminated}"
                );
            }
        }
    }

    #[test]
    fn qsp_identity_inject_is_byte_identical() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("game.qsp");
        let mut records = fixture_records();
        records[3] = encode_field("0001");
        records[6] = encode_field("*pl 'Unterminated text");
        let mut before = write_u16le_units(&join_records(&records));
        before.extend_from_slice(&[0, 0xD8]);
        fs::write(&path, &before).unwrap();
        let plugin = QspPlugin::new();
        let mut entries = plugin.extract(&path).unwrap();
        for entry in &mut entries {
            entry.translation = Some(entry.source.clone());
        }
        for rows in [&[][..], entries.as_slice()] {
            let report = plugin.inject(&path, rows).unwrap();
            assert_eq!(report.files_modified, 0);
            assert_eq!(fs::read(&path).unwrap(), before);
        }
    }

    #[test]
    fn qsp_stale_sources_are_refused_without_modifying_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = create_qsp_fixture(dir.path()).join("game.qsp");
        let before = fs::read(&path).unwrap();
        let plugin = QspPlugin::new();
        let mut entries = plugin.extract(&path).unwrap();
        for entry in &mut entries {
            entry.source = "Stale source".into();
            entry.translation = Some("Translated text".into());
        }
        let report = plugin.inject(&path, &entries).unwrap();
        assert_eq!(
            report.skip_reasons.get("source_changed"),
            Some(&entries.len())
        );
        assert_eq!((report.files_modified, report.strings_written), (0, 0));
        assert_eq!(fs::read(&path).unwrap(), before);
    }

    #[test]
    fn c123_named_skips_for_unchanged_qsp_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = create_qsp_fixture(dir.path()).join("game.qsp");
        let before = fs::read(&path).unwrap();
        let plugin = QspPlugin::new();
        let mut entries = plugin.extract(&path).unwrap();
        entries.truncate(2);
        entries[1].translation = Some(entries[1].source.clone());
        let mut report = plugin.inject(&path, &entries).unwrap();
        assert_eq!((report.strings_written, report.strings_skipped), (0, 2));
        report.classify_remaining_skips();
        assert_eq!(
            report.skip_reasons,
            [("untranslated".into(), 1), ("unchanged".into(), 1)].into()
        );
        assert_eq!(fs::read(path).unwrap(), before);
    }

    #[test]
    fn c123_named_skips_for_missing_and_invalid_qsp() {
        let dir = tempfile::tempdir().unwrap();
        let path = create_qsp_fixture(dir.path()).join("game.qsp");
        let plugin = QspPlugin::new();
        let entries = plugin.extract(&path).unwrap();
        for reason in ["decode_error", "target_missing"] {
            if reason == "decode_error" {
                fs::write(&path, b"broken").unwrap();
            } else {
                fs::remove_file(&path).unwrap();
            }
            let report = plugin.inject(dir.path(), &entries).unwrap();
            assert_eq!(report.strings_skipped, entries.len());
            assert_eq!(report.skip_reasons, [(reason.into(), entries.len())].into());
        }
    }

    #[test]
    fn c123_named_skips_for_qsp_locators_and_duplicates() {
        let dir = tempfile::tempdir().unwrap();
        let path = create_qsp_fixture(dir.path()).join("game.qsp");
        let original = fs::read(&path).unwrap();
        let plugin = QspPlugin::new();
        for (id, reason) in [
            ("game.qsp#loc00#desc", "invalid_locator"),
            ("game.qsp#loc0#code#str00", "invalid_locator"),
            ("game.qsp#loc0#act00#name", "invalid_locator"),
            ("game.qsp#loc0#unknown", "invalid_locator"),
            ("game.qsp#loc9#desc", "target_missing"),
            ("game.qsp#loc0#act9#name", "target_missing"),
            ("game.qsp#loc0#code#str99", "target_missing"),
            ("game.qsp#loc0#act0#code#str99", "target_missing"),
            ("bad", "invalid_locator"),
        ] {
            let mut entry = StringEntry::new(id, "Old text", path.clone());
            entry.translation = Some("New text".into());
            let report = plugin.inject(&path, &[entry]).unwrap();
            assert_eq!((report.strings_written, report.strings_skipped), (0, 1));
            assert_eq!(report.skip_reasons, [(reason.into(), 1)].into(), "{id}");
        }
        let mut entries = plugin.extract(&path).unwrap();
        for entry in &mut entries {
            entry.translation = Some(entry.source.clone());
        }
        let mut earlier = entries[0].clone();
        earlier.translation = Some("Overwritten by the identity row".into());
        entries.insert(0, earlier);
        let report = plugin.inject(&path, &entries).unwrap();
        assert_eq!(
            (report.strings_written, report.strings_skipped),
            (0, entries.len())
        );
        assert_eq!(
            report.skip_reasons,
            [
                ("duplicate".into(), 1),
                ("unchanged".into(), entries.len() - 1)
            ]
            .into()
        );
        assert_eq!(fs::read(path).unwrap(), original);
    }

    fn tempdir() -> tempfile::TempDir {
        tempfile::Builder::new()
            .prefix("locust_qsp_")
            .tempdir()
            .unwrap()
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

    fn create_qsp_reference_fixture(dir: &Path, code: &str, action_code: &str) -> PathBuf {
        let path = dir.join("game.qsp");
        let mut records = fixture_records();
        records[4] = encode_field("Bedroom");
        records[5] = encode_field("");
        records[6] = encode_field(code);
        records[9] = encode_field("");
        records[10] = encode_field(action_code);
        fs::write(&path, write_u16le_units(&join_records(&records))).unwrap();
        path
    }

    #[test]
    fn qsp_references_extract_only_display_literals() {
        let dir = tempdir();
        let code = concat!(
            "gt 'Bedroom' & 'You enter the room.'\r\n",
            "GOTO \"Bedroom\" & *pl \"Hello\"\r\n",
            "gs 'Shop', 1 & act 'Look around': msg 'Pick a direction.'\r\n",
            "xgt 'Bedroom' & gosub 'helper'\r\n",
            "GT 'UpperTarget' & Gt 'MixedTarget' & XGOTO 'RemoteTarget'",
        );
        let path = create_qsp_reference_fixture(dir.path(), code, "");
        let entries = QspPlugin::new().extract(&path).unwrap();
        let rows: Vec<_> = entries
            .iter()
            .map(|e| (e.id.as_str(), e.source.as_str()))
            .collect();
        assert_eq!(
            rows,
            vec![
                ("game.qsp#loc0#code#str1", "You enter the room."),
                ("game.qsp#loc0#code#str3", "Hello"),
                ("game.qsp#loc0#code#str5", "Look around"),
                ("game.qsp#loc0#code#str6", "Pick a direction."),
            ]
        );
    }

    #[test]
    fn qsp_references_keep_display_row_ids_after_protected_literals() {
        let dir = tempdir();
        let code = "'123' & gt 'Shop' & '' & 'x' & *pl 'After the jump.'";
        let path = create_qsp_reference_fixture(dir.path(), code, code);
        let entries = QspPlugin::new().extract(&path).unwrap();
        let rows: Vec<_> = entries
            .iter()
            .map(|e| (e.id.as_str(), e.source.as_str()))
            .collect();
        // Only literals passing the historical visibility filter consume IDs.
        assert_eq!(
            rows,
            vec![
                ("game.qsp#loc0#code#str1", "After the jump."),
                ("game.qsp#loc0#act0#code#str1", "After the jump."),
            ]
        );
    }

    #[test]
    fn qsp_references_protect_location_names_in_any_expression() {
        let dir = tempdir();
        let code = concat!(
            "if $curloc = 'bEdRoOm': msg 'You are home.'\r\n",
            "$destination = \"BEDROOM\" & *pl 'Bedroom'\r\n",
            "msg 'Bedroom window' & msg 'Your bedroom is quiet.'",
        );
        let action_code = "if 'bedROOM' = $CURLOC: *pl 'Action text.'";
        let path = create_qsp_reference_fixture(dir.path(), code, action_code);
        let entries = QspPlugin::new().extract(&path).unwrap();
        let sources: Vec<_> = entries.iter().map(|e| e.source.as_str()).collect();
        assert_eq!(
            sources,
            [
                "You are home.",
                "Bedroom window",
                "Your bedroom is quiet.",
                "Action text.",
            ]
        );
    }

    #[test]
    fn qsp_references_only_protect_first_call_argument() {
        let dir = tempdir();
        let code = concat!(
            "gs 'Shop', 'Welcome to the shop.' & gosub('helper', 'Please wait.')\r\n",
            "$result = FUNC('calculate', 'Visible argument')\r\n",
            "*pl $FuNc ( 'describe', 'Another argument')\r\n",
            "dynamic 'helper_id', 'Dynamic argument'\r\n",
            "$result = $dyneval('helper_id', 'Evaluation argument')\r\n",
            "msg 'A dynamic story.' & $myfunc = 'Ordinary text.'",
        );
        let path = create_qsp_reference_fixture(dir.path(), code, "");
        let entries = QspPlugin::new().extract(&path).unwrap();
        let sources: Vec<_> = entries.iter().map(|e| e.source.as_str()).collect();
        assert_eq!(
            sources,
            [
                "Welcome to the shop.",
                "Please wait.",
                "Visible argument",
                "Another argument",
                "Dynamic argument",
                "Evaluation argument",
                "A dynamic story.",
                "Ordinary text.",
            ]
        );
    }

    #[test]
    fn qsp_references_handle_inline_blocks_and_doubled_quotes() {
        let dir = tempdir();
        let code = concat!(
            "if ready: Gt 'Guest''s room' & *pl 'Don''t go & hide.'\r\n",
            "act \"Look & listen\": XGoTo \"The \"\"annex\"\"\" & msg \"Say \"\"hi\"\"\"\r\n",
            "if ready: gs 'helper': else: goto 'Fallback' & 'Keep reading.'",
        );
        let path = create_qsp_reference_fixture(dir.path(), code, "");
        let entries = QspPlugin::new().extract(&path).unwrap();
        let sources: Vec<_> = entries.iter().map(|e| e.source.as_str()).collect();
        assert_eq!(
            sources,
            [
                "Don't go & hide.",
                "Look & listen",
                "Say \"hi\"",
                "Keep reading."
            ]
        );
    }

    #[test]
    fn qsp_references_inject_display_and_ignore_legacy_destination_rows() {
        let dir = tempdir();
        let code = concat!(
            "gt 'Bedroom' & 'You enter the room.' & GOTO \"Bedroom\" & *pl \"Hello\"\r\n",
            "gs 'Shop', 1 & act 'Look around': msg 'Don''t go & hide.'\r\n",
            "xgt 'Bedroom' & gosub 'helper' & GT 'Guest''s room' & Gt \"The \"\"annex\"\"\"",
        );
        let action_code = "xgoto 'Bedroom' & msg 'Action text.'";
        let path = create_qsp_reference_fixture(dir.path(), code, action_code);
        let mut entries = Vec::new();
        // Include old DB rows, with matching sources, for every protected ID.
        for (suffix, source, translation) in [
            ("code#str0", "Bedroom", "Dormitorio"),
            (
                "code#str1",
                "You enter the room.",
                "Entras en la habitación.",
            ),
            ("code#str2", "Bedroom", "Dormitorio"),
            ("code#str3", "Hello", "Hola"),
            ("code#str4", "Shop", "Tienda"),
            ("code#str5", "Look around", "Mirar alrededor"),
            ("code#str6", "Don't go & hide.", "L'ami & reste."),
            ("code#str7", "Bedroom", "Dormitorio"),
            ("code#str8", "helper", "ayudante"),
            ("code#str9", "Guest's room", "Habitación"),
            ("code#str10", "The \"annex\"", "El anexo"),
            ("act0#code#str0", "Bedroom", "Dormitorio"),
            ("act0#code#str1", "Action text.", "Texto de acción."),
        ] {
            let mut entry =
                StringEntry::new(format!("game.qsp#loc0#{suffix}"), source, path.clone());
            entry.translation = Some(translation.into());
            entries.push(entry);
        }
        let report = QspPlugin::new().inject(&path, &entries).unwrap();
        assert_eq!((report.files_modified, report.strings_written), (1, 5));
        assert_eq!(report.strings_skipped, 8);
        assert_eq!(report.skip_reasons.get("target_missing"), Some(&8));
        let mut expected = fixture_records();
        expected[4] = encode_field("Bedroom");
        expected[5] = encode_field("");
        expected[6] = encode_field(
            &code
                .replace("You enter the room.", "Entras en la habitación.")
                .replace("Hello", "Hola")
                .replace("Look around", "Mirar alrededor")
                .replace("Don''t go & hide.", "L''ami & reste."),
        );
        expected[9] = encode_field("");
        expected[10] = encode_field("xgoto 'Bedroom' & msg 'Texto de acción.'");
        assert_eq!(
            fs::read(&path).unwrap(),
            write_u16le_units(&join_records(&expected))
        );
    }

    #[test]
    fn qsp_references_legacy_rows_alone_leave_file_byte_identical() {
        let dir = tempdir();
        let code = "gt 'Shop' & if $curloc = 'BEDROOM': gs 'helper'";
        let path = create_qsp_reference_fixture(dir.path(), code, "goto 'Bedroom'");
        let before = fs::read(&path).unwrap();
        let mut entries = Vec::new();
        for (suffix, source) in [
            ("code#str0", "Shop"),
            ("code#str1", "BEDROOM"),
            ("code#str2", "helper"),
            ("act0#code#str0", "Bedroom"),
        ] {
            let mut entry =
                StringEntry::new(format!("game.qsp#loc0#{suffix}"), source, path.clone());
            entry.translation = Some("Never rewrite this".into());
            entries.push(entry);
        }
        let report = QspPlugin::new().inject(&path, &entries).unwrap();
        assert_eq!((report.files_modified, report.strings_written), (0, 0));
        assert_eq!(report.strings_skipped, 4);
        assert_eq!(fs::read(&path).unwrap(), before);
    }

    #[test]
    fn test_extract_doubled_quotes() {
        assert_eq!(
            extract_quoted_literals("*pl 'Don''t go.'", &HashSet::new()),
            vec![Some("Don't go.".into())]
        );
        assert_eq!(
            extract_quoted_literals(r#"*pl "He said ""hi""""#, &HashSet::new()),
            vec![Some(r#"He said "hi""#.into())]
        );
    }

    #[test]
    fn test_quoted_literal_boundaries_and_replacement_order() {
        assert!(extract_quoted_literals("''", &HashSet::new()).is_empty());
        assert_eq!(
            replace_quoted_in_order("''", &[Some("unused".into())]),
            "''"
        );
        assert_eq!(
            extract_quoted_literals("*pl 'Unterminated text", &HashSet::new()),
            vec![Some("Unterminated text".into())]
        );
        assert_eq!(
            replace_quoted_in_order("*pl 'Unterminated text", &[]),
            "*pl 'Unterminated text"
        );
        assert_eq!(
            replace_quoted_in_order("*pl 'Unterminated text", &[Some("Texto traducido".into())]),
            "*pl 'Texto traducido'"
        );

        let code = r#"'' & '123' & 'Don''t go.' & "He said ""hi""" & 'img/a.png' & 'Next line'"#;
        assert_eq!(
            extract_quoted_literals(code, &HashSet::new()),
            vec![
                Some("Don't go.".into()),
                Some(r#"He said "hi""#.into()),
                Some("Next line".into())
            ]
        );
        assert_eq!(
            replace_quoted_in_order(code, &extract_quoted_literals(code, &HashSet::new())),
            code
        );
        assert_eq!(
            replace_quoted_in_order(code, &[Some("L'ami".into()), Some("Dijo \"hola\"".into())]),
            r#"'' & '123' & 'L''ami' & "Dijo ""hola""" & 'img/a.png' & 'Next line'"#
        );
    }

    #[test]
    fn test_doubled_quotes_inject_roundtrip() {
        for (code, source, translation, expected_code) in [
            (
                "*pl 'Don''t go.'",
                "Don't go.",
                "L'ami reste.",
                "*pl 'L''ami reste.'",
            ),
            (
                r#"*pl "He said ""hi""""#,
                "He said \"hi\"",
                "Dijo \"hola\"",
                r#"*pl "Dijo ""hola""""#,
            ),
        ] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("game.qsp");
            let mut game = QspGame {
                version: "locust-test 1.0".into(),
                password: "No".into(),
                locations: vec![QspLocation {
                    name: "start".into(),
                    description: String::new(),
                    code: code.into(),
                    actions: Vec::new(),
                }],
            };
            fs::write(&path, serialize_game(&game)).unwrap();
            let plugin = QspPlugin::new();
            let mut entries = plugin.extract(dir.path()).unwrap();
            assert_eq!(entries.len(), 1, "{entries:?}");
            assert_eq!(entries[0].source, source);
            assert_eq!(entries[0].id, "game.qsp#loc0#code#str0");
            entries[0].translation = Some(translation.into());
            let report = plugin.inject(dir.path(), &entries).unwrap();
            assert_eq!(report.strings_written, 1, "{report:?}");
            assert_eq!(report.strings_skipped, 0, "{report:?}");
            assert_eq!(report.files_modified, 1, "{report:?}");
            game.locations[0].code = expected_code.into();
            assert_eq!(fs::read(&path).unwrap(), serialize_game(&game));
            let again = plugin.extract(dir.path()).unwrap();
            assert_eq!(again.len(), 1, "{again:?}");
            assert_eq!(again[0].id, entries[0].id);
            assert_eq!(again[0].source, translation);
        }
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
        assert_eq!(decode_field(&encode_field("Hello")).unwrap(), "Hello");
        assert_eq!(decode_field(&encode_field("Привет")).unwrap(), "Привет");
    }

    #[test]
    fn test_detect_qsp_dir() {
        let dir_guard = tempdir();
        let dir = dir_guard.path().to_path_buf();
        create_qsp_fixture(&dir);
        let plugin = QspPlugin::new();
        assert!(plugin.detect(&dir));
    }

    #[test]
    fn test_detect_non_qsp() {
        let dir_guard = tempdir();
        let dir = dir_guard.path().to_path_buf();
        fs::write(dir.join("readme.txt"), b"nope").unwrap();
        let plugin = QspPlugin::new();
        assert!(!plugin.detect(&dir));
    }

    #[test]
    fn test_extract_known_strings_and_stable_ids() {
        let dir_guard = tempdir();
        let dir = dir_guard.path().to_path_buf();
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
        let dir_guard = tempdir();
        let dir = dir_guard.path().to_path_buf();
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
        let dir_guard = tempdir();
        let dir = dir_guard.path().to_path_buf();
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
