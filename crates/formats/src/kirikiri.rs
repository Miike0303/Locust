//! KiriKiri / KAG loose `.ks` script plugin — Experimental (synthetic fixtures).
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
//! Out of scope: XP3 archive extraction, `.tjs`/compiled `.scn`, mode-2 write-back,
//! engine-private encryption beyond the public FE FE stream.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use locust_core::error::{LocustError, Result};
use locust_core::extraction::{FormatPlugin, InjectionReport};
use locust_core::models::{OutputMode, StringEntry};

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

    fn has_xp3(root: &Path) -> bool {
        if root.is_file() {
            return Self::is_xp3(root);
        }
        if !root.is_dir() {
            return false;
        }
        walkdir::WalkDir::new(root)
            .follow_links(false)
            .into_iter()
            .filter_map(|e| e.ok())
            .any(|e| e.path().is_file() && Self::is_xp3(e.path()))
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

// ─── Plugin ────────────────────────────────────────────────────────────────

impl FormatPlugin for KirikiriPlugin {
    fn id(&self) -> &str {
        "kirikiri"
    }

    fn name(&self) -> &str {
        "KiriKiri / KAG"
    }

    fn description(&self) -> &str {
        "KiriKiri KAG loose .ks scripts (UTF-16LE / UTF-8 / Shift-JIS; FE FE mode 0/1)"
    }

    fn stability(&self) -> locust_core::extraction::FormatStability {
        locust_core::extraction::FormatStability::Experimental
    }

    fn supported_extensions(&self) -> &[&str] {
        &[".ks"]
    }

    fn supported_modes(&self) -> Vec<OutputMode> {
        vec![OutputMode::Replace]
    }

    fn detect(&self, path: &Path) -> bool {
        !Self::find_ks_files(path).is_empty() || Self::has_xp3(path)
    }

    fn extract(&self, path: &Path) -> Result<Vec<StringEntry>> {
        let ks_files = Self::find_ks_files(path);
        if ks_files.is_empty() {
            if Self::has_xp3(path) {
                return Err(parse_err(
                    &path.display().to_string(),
                    "no loose .ks scripts; xp3 archives not yet supported",
                ));
            }
            return Err(parse_err(
                &path.display().to_string(),
                "no .ks script files found",
            ));
        }

        let root = if path.is_dir() {
            path.to_path_buf()
        } else {
            path.parent().unwrap_or(path).to_path_buf()
        };

        let mut all = Vec::new();
        for fpath in &ks_files {
            let bytes = std::fs::read(fpath)?;
            let rel = fpath
                .strip_prefix(&root)
                .unwrap_or(fpath.as_path())
                .to_string_lossy()
                .replace('\\', "/");
            let decoded = decode_ks_bytes(&bytes, &rel)?;
            for (idx, line) in decoded.text.split('\n').enumerate() {
                let line = line.strip_suffix('\r').unwrap_or(line);
                let line_no = idx + 1;
                if !is_player_text_line(line) {
                    continue;
                }
                let id = format!("{rel}#{line_no}");
                let mut entry = StringEntry::new(id, line, fpath.clone());
                entry.tags = vec!["dialogue".into()];
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

            let bytes = std::fs::read(&actual)?;
            let label = actual.display().to_string();
            let mut decoded = match decode_ks_bytes(&bytes, &label) {
                Ok(d) => d,
                Err(e) => {
                    warnings.push(format!("cannot decode {}: {e}", actual.display()));
                    strings_skipped += file_entries.len();
                    continue;
                }
            };

            // Map line number (1-based) → translation via id suffix `#N`
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
            for (idx, line) in decoded.text.split('\n').enumerate() {
                let line = line.strip_suffix('\r').unwrap_or(line);
                let line_no = idx + 1;
                if let Some(t) = by_line.get(&line_no) {
                    if is_player_text_line(line) && *t != line {
                        out_lines.push((*t).to_string());
                        changed = true;
                        strings_written += 1;
                        continue;
                    }
                }
                out_lines.push(line.to_string());
            }

            if !changed {
                strings_skipped += file_entries.len();
                continue;
            }

            let joined = out_lines.join("\n");
            decoded.text = joined;
            let encoded = match encode_ks_bytes(&decoded) {
                Ok(b) => b,
                Err(e) => {
                    warnings.push(format!("cannot re-encode {}: {e}", actual.display()));
                    strings_skipped += file_entries.len();
                    continue;
                }
            };
            std::fs::write(&actual, &encoded)?;
            files_modified += 1;
            files_written.push(actual);
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
    fn test_xp3_only_extract_reports_loudly() {
        let dir = tempdir();
        fs::write(dir.join("data.xp3"), b"XP3\r\n").unwrap();
        let err = KirikiriPlugin::new().extract(&dir).unwrap_err().to_string();
        assert!(
            err.contains("xp3") && err.contains("not yet supported"),
            "expected xp3 skip message, got: {err}"
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
