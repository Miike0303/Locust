//! NScripter / ONScripter script plugin — Experimental (synthetic fixtures).
//!
//! # Spec sources (do not invent transforms)
//! - Container open order + XOR decrypt:
//!   https://github.com/ogapee/onscripter/blob/master/ScriptHandler.cpp
//!   (`ScriptHandler::readScript`, `ScriptHandler::readScriptSub`)
//!   Priority: `0.txt` → `00.txt` → `nscr_sec.dat` → `nscript.___` → `nscript.dat`
//!   (also `pscript.dat` UTF-8 path exists in ONScripter — out of scope here).
//!   `encrypt_mode == 1` (`nscript.dat`): every byte `ch ^= 0x84`.
//! - Token classes (`readToken`): high-bit first char (`ch & 0x80`) and backtick
//!   `` ` `` start dialogue; ASCII-letter lines are commands; `*` labels; `;` comments.
//!
//! # First-cut line heuristic (over-extraction OK)
//! After Shift-JIS decode, a line is player text iff the first non-space char is
//! non-ASCII (SJIS lead ≥ 0x80 after decode) or a backtick. Inline wait markers
//! (`@`, `\`, `/` at EOL) and furigana stay inside the extracted string.
//!
//! Out of scope: `nscr_sec.dat` (mode-2 5-byte magic XOR), `nscript.___` (mode-3
//! key table from EXE), multi-file `1.txt`…`99.txt` concat, `pscript.dat` UTF-8,
//! `arc.nsa` / `.sar` archive unpack, real commercial game fixtures.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use locust_core::error::{LocustError, Result};
use locust_core::extraction::{FormatPlugin, InjectionReport};
use locust_core::models::{OutputMode, StringEntry};

/// ONScripter `nscript.dat` / encrypt_mode 1 XOR constant (`readScriptSub`).
const NSCRIPT_DAT_XOR: u8 = 0x84;

/// Supported script containers, highest priority first (engine order subset).
const SUPPORTED_CONTAINERS: &[&str] = &["0.txt", "00.txt", "nscript.dat"];

/// Present in engine priority but not implemented in this cut.
const UNSUPPORTED_CONTAINERS: &[&str] = &["nscr_sec.dat", "nscript.___"];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ContainerKind {
    /// Plain Shift-JIS text (`0.txt` / `00.txt`).
    Plain,
    /// Byte-wise XOR 0x84 then Shift-JIS (`nscript.dat`).
    Xor84,
}

impl ContainerKind {
    fn from_name(name: &str) -> Option<Self> {
        match name {
            "0.txt" | "00.txt" => Some(Self::Plain),
            "nscript.dat" => Some(Self::Xor84),
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

    fn has_unsupported_only_containers(root: &Path) -> bool {
        let has_unsupported = UNSUPPORTED_CONTAINERS
            .iter()
            .any(|n| Self::file_in_root(root, n).is_some());
        let has_supported = SUPPORTED_CONTAINERS
            .iter()
            .any(|n| Self::file_in_root(root, n).is_some());
        has_unsupported && !has_supported
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

fn parse_err(file: &str, message: impl Into<String>) -> LocustError {
    LocustError::ParseError {
        file: file.into(),
        message: message.into(),
    }
}

// ─── Decode / encode ───────────────────────────────────────────────────────

fn xor_bytes(data: &[u8], key: u8) -> Vec<u8> {
    data.iter().map(|b| b ^ key).collect()
}

fn decode_container(bytes: &[u8], kind: ContainerKind, file_label: &str) -> Result<(String, bool)> {
    let xored;
    let plain: &[u8] = match kind {
        ContainerKind::Plain => bytes,
        ContainerKind::Xor84 => {
            xored = xor_bytes(bytes, NSCRIPT_DAT_XOR);
            &xored
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

fn encode_container(text: &str, kind: ContainerKind, crlf: bool, file_label: &str) -> Result<Vec<u8>> {
    let normalized = if crlf {
        text.replace("\r\n", "\n")
            .replace('\r', "\n")
            .replace('\n', "\r\n")
    } else {
        text.replace("\r\n", "\n").replace('\r', "\n")
    };
    let (encoded, _, had_errors) = encoding_rs::SHIFT_JIS.encode(&normalized);
    if had_errors {
        return Err(parse_err(
            file_label,
            "could not re-encode NScripter script as Shift-JIS",
        ));
    }
    let sjis = encoded.into_owned();
    Ok(match kind {
        ContainerKind::Plain => sjis,
        ContainerKind::Xor84 => xor_bytes(&sjis, NSCRIPT_DAT_XOR),
    })
}

/// Try to encode a single replacement line as Shift-JIS (strict).
fn try_encode_sjis_line(s: &str) -> Option<String> {
    let (bytes, _, had_errors) = encoding_rs::SHIFT_JIS.encode(s);
    if had_errors {
        return None;
    }
    // Round-trip via SJIS so we store what will actually be written.
    let (cow, _, _) = encoding_rs::SHIFT_JIS.decode(&bytes);
    Some(cow.into_owned())
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

// ─── Plugin ────────────────────────────────────────────────────────────────

impl FormatPlugin for NScripterPlugin {
    fn id(&self) -> &str {
        "nscripter"
    }

    fn name(&self) -> &str {
        "NScripter / ONScripter"
    }

    fn description(&self) -> &str {
        "NScripter scripts (0.txt / 00.txt Shift-JIS; nscript.dat XOR 0x84) — synthetic fixtures"
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

        let Some(selected) = Self::select_container(path) else {
            if Self::has_unsupported_only_containers(&root) {
                return Err(parse_err(
                    &label,
                    "unsupported NScripter container only (nscr_sec.dat / nscript.___); \
                     this Experimental cut supports 0.txt, 00.txt, and nscript.dat",
                ));
            }
            if Self::has_nsa(&root) {
                return Err(parse_err(
                    &label,
                    "arc.nsa / .nsa archive present but no supported script container \
                     (0.txt / 00.txt / nscript.dat); archive unpack is out of scope",
                ));
            }
            return Err(parse_err(
                &label,
                "no NScripter script container found (expected 0.txt, 00.txt, or nscript.dat)",
            ));
        };

        let bytes = std::fs::read(&selected.path)?;
        let (text, _crlf) = decode_container(&bytes, selected.kind, &selected.name)?;

        let mut all = Vec::new();
        for (idx, line) in text.split('\n').enumerate() {
            let line = line.strip_suffix('\r').unwrap_or(line);
            let line_no = idx + 1;
            if !is_player_text_line(line) {
                continue;
            }
            let id = format!("{}#{}", selected.name, line_no);
            let mut entry = StringEntry::new(id, line, selected.path.clone());
            entry.tags = vec!["dialogue".into()];
            all.push(entry);
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

            let name = actual
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("script");
            let kind = match ContainerKind::from_name(
                SUPPORTED_CONTAINERS
                    .iter()
                    .find(|n| name.eq_ignore_ascii_case(n))
                    .copied()
                    .unwrap_or(name),
            ) {
                Some(k) => k,
                None => {
                    warnings.push(format!(
                        "unsupported container for inject: {}",
                        actual.display()
                    ));
                    strings_skipped += file_entries.len();
                    continue;
                }
            };

            let bytes = match std::fs::read(&actual) {
                Ok(b) => b,
                Err(e) => {
                    warnings.push(format!("read {}: {e}", actual.display()));
                    strings_skipped += file_entries.len();
                    continue;
                }
            };
            let (text, crlf) = match decode_container(&bytes, kind, name) {
                Ok(v) => v,
                Err(e) => {
                    warnings.push(format!("cannot decode {}: {e}", actual.display()));
                    strings_skipped += file_entries.len();
                    continue;
                }
            };

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
            let mut file_written = 0usize;
            let mut file_skipped = 0usize;
            for (idx, line) in text.split('\n').enumerate() {
                let line = line.strip_suffix('\r').unwrap_or(line);
                let line_no = idx + 1;
                if let Some(t) = by_line.get(&line_no) {
                    if is_player_text_line(line) && *t != line {
                        match try_encode_sjis_line(t) {
                            Some(encoded_line) => {
                                out_lines.push(encoded_line);
                                changed = true;
                                file_written += 1;
                                continue;
                            }
                            None => {
                                warnings.push(format!(
                                    "{name}#{line_no}: translation not encodable as Shift-JIS; skipped"
                                ));
                                file_skipped += 1;
                                // Keep original line — do not corrupt the file.
                            }
                        }
                    } else {
                        file_skipped += 1;
                    }
                }
                out_lines.push(line.to_string());
            }

            if !changed {
                strings_skipped += file_entries.len();
                continue;
            }

            let joined = out_lines.join("\n");
            let encoded = match encode_container(&joined, kind, crlf, name) {
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
            strings_written += file_written;
            strings_skipped += file_skipped;
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
            sources.iter().any(|s| s.ends_with('@') || s.contains("。@")),
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
    fn test_unsupported_only_nscr_sec_errors_loudly() {
        let dir = tempdir();
        fs::write(dir.join("nscr_sec.dat"), b"\0\0\0").unwrap();
        let err = NScripterPlugin::new().extract(&dir).unwrap_err().to_string();
        assert!(
            err.contains("nscr_sec") || err.contains("unsupported"),
            "expected unsupported container message, got: {err}"
        );
    }

    #[test]
    fn test_unsupported_only_nscript_underscore_errors_loudly() {
        let dir = tempdir();
        fs::write(dir.join("nscript.___"), b"\0\0\0").unwrap();
        let err = NScripterPlugin::new().extract(&dir).unwrap_err().to_string();
        assert!(
            err.contains("nscript.___") || err.contains("unsupported"),
            "expected unsupported container message, got: {err}"
        );
    }

    #[test]
    fn test_nsa_only_extract_reports_loudly() {
        let dir = tempdir();
        fs::write(dir.join("arc.nsa"), b"NSA\0fake").unwrap();
        let err = NScripterPlugin::new().extract(&dir).unwrap_err().to_string();
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
            report.warnings.iter().any(|w| w.contains("Shift-JIS") || w.contains("encodable")),
            "expected SJIS warning: {:?}",
            report.warnings
        );
        assert!(report.strings_skipped >= 1, "{report:?}");
        // Original line must remain
        let again = plugin.extract(&dir).unwrap();
        assert!(
            again.iter().any(|e| e.id == original_id && e.source.contains("Hello, world")),
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
}
