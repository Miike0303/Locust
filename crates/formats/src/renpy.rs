use std::collections::HashMap;
use std::io::{Read as IoRead, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use locust_core::error::Result;
use locust_core::extraction::{FormatPlugin, InjectionReport};
use locust_core::models::{OutputMode, StringEntry};

pub struct RenPyPlugin;

impl RenPyPlugin {
    pub fn new() -> Self {
        Self
    }

    fn find_game_dir(path: &Path) -> Option<PathBuf> {
        if path.is_dir() {
            let game = path.join("game");
            if game.is_dir() {
                return Some(game);
            }
        }
        None
    }

    fn has_rpy_files(dir: &Path) -> bool {
        std::fs::read_dir(dir)
            .map(|entries| {
                entries.filter_map(|e| e.ok()).any(|e| {
                    e.path().extension().map_or(false, |ext| ext == "rpy")
                })
            })
            .unwrap_or(false)
    }

    fn extract_rpa_archive(&self, rpa_path: &Path) -> Result<Vec<StringEntry>> {
        let temp_dir = std::env::temp_dir().join(format!(
            "locust_rpa_{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&temp_dir)?;

        let extracted_files = Self::extract_rpa(rpa_path, &temp_dir)?;

        let mut all = Vec::new();
        for file in &extracted_files {
            // Skip tl/ directory files
            if let Ok(rel) = file.strip_prefix(&temp_dir) {
                let rel_str = rel.to_string_lossy();
                if rel_str.starts_with("tl/") || rel_str.starts_with("tl\\") {
                    continue;
                }
            }
            match Self::extract_file(file) {
                Ok(mut entries) => {
                    // Rewrite file_path to reference the original RPA
                    for entry in &mut entries {
                        entry.file_path = rpa_path.to_path_buf();
                    }
                    all.extend(entries);
                }
                Err(e) => {
                    tracing::warn!("Failed to extract {}: {}", file.display(), e);
                }
            }
        }

        // Cleanup temp dir
        let _ = std::fs::remove_dir_all(&temp_dir);

        Ok(all)
    }

    fn has_rpa_files(dir: &Path) -> bool {
        std::fs::read_dir(dir)
            .map(|entries| {
                entries.filter_map(|e| e.ok()).any(|e| {
                    e.path().extension().map_or(false, |ext| ext == "rpa")
                })
            })
            .unwrap_or(false)
    }

    /// Extract .rpy files from a .rpa archive (Ren'Py Archive format).
    /// RPA-3.0 header: `RPA-3.0 <hex_offset> <hex_key>\n`
    /// At offset: zlib-compressed pickle with a dict of filename -> [(offset, length, prefix)]
    fn extract_rpa(rpa_path: &Path, temp_dir: &Path) -> Result<Vec<PathBuf>> {
        let mut file = std::fs::File::open(rpa_path)?;
        let mut header_buf = [0u8; 256];
        let n = file.read(&mut header_buf)?;
        let header = String::from_utf8_lossy(&header_buf[..n]);

        let first_line = header.lines().next().unwrap_or("");
        let parts: Vec<&str> = first_line.split_whitespace().collect();

        if parts.len() < 3 || !parts[0].starts_with("RPA-") {
            return Err(locust_core::error::LocustError::ParseError {
                file: rpa_path.display().to_string(),
                message: "not a valid RPA archive".to_string(),
            });
        }

        let index_offset = u64::from_str_radix(parts[1], 16).map_err(|_| {
            locust_core::error::LocustError::ParseError {
                file: rpa_path.display().to_string(),
                message: "invalid RPA index offset".to_string(),
            }
        })?;

        let key = i64::from_str_radix(parts[2], 16).unwrap_or(0);

        // Read the index (zlib-compressed pickle)
        file.seek(SeekFrom::Start(index_offset))?;
        let mut compressed = Vec::new();
        file.read_to_end(&mut compressed)?;

        // Decompress with zlib (raw deflate with zlib wrapper)
        let decompressed = miniz_oxide::inflate::decompress_to_vec_zlib(&compressed).map_err(|e| {
            locust_core::error::LocustError::ParseError {
                file: rpa_path.display().to_string(),
                message: format!("failed to decompress RPA index: {:?}", e),
            }
        })?;

        // Parse the Python pickle to extract file entries
        // We use a simplified pickle parser that handles the common RPA format
        let index = parse_rpa_pickle(&decompressed, key)?;


        let mut extracted_files = Vec::new();
        for (name, offset, length) in &index {
            // Only extract .rpy files (skip .rpyc compiled files unless no .rpy available)
            if !name.ends_with(".rpy") && !name.ends_with(".rpyc") {
                continue;
            }
            // Prefer .rpy over .rpyc — if both exist, the .rpy will be used
            if name.ends_with(".rpyc") {
                let rpy_name = name.strip_suffix("c").unwrap();
                if index.iter().any(|(n, _, _)| n == rpy_name) {
                    continue;
                }
                // Skip compiled files - can't extract text from binary rpyc
                continue;
            }

            file.seek(SeekFrom::Start(*offset))?;
            let mut data = vec![0u8; *length];
            file.read_exact(&mut data)?;

            let rel_path = Path::new(name);
            let out_path = temp_dir.join(rel_path);
            if let Some(parent) = out_path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(&out_path, &data)?;
            extracted_files.push(out_path);
        }

        Ok(extracted_files)
    }

    fn extract_file(file_path: &Path) -> Result<Vec<StringEntry>> {
        let content = std::fs::read_to_string(file_path)?;
        let filename = file_path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();

        let mut entries = Vec::new();
        let mut in_menu = false;

        for (line_idx, line) in content.lines().enumerate() {
            let line_num = line_idx + 1;
            let trimmed = line.trim();

            // Track menu blocks
            if trimmed == "menu:" {
                in_menu = true;
                continue;
            }

            // Menu choice: "Choice text":
            if in_menu {
                if let Some(text) = extract_menu_choice(trimmed) {
                    let id = format!("{}#{}", filename, line_num);
                    let mut entry = StringEntry::new(id, text, file_path.to_path_buf());
                    entry.tags = vec!["menu".to_string()];
                    entries.push(entry);
                    continue;
                }
                // If line is not indented more or is a non-string line, check if still in menu
                if !trimmed.is_empty()
                    && !trimmed.starts_with('"')
                    && !trimmed.starts_with("jump")
                    && !trimmed.starts_with("pass")
                    && !trimmed.starts_with('#')
                {
                    // Could be a say statement after menu — exit menu
                    if !line.starts_with("        ") && !line.starts_with("\t\t") {
                        in_menu = false;
                    }
                }
            }

            // _("text") pattern
            if let Some(text) = extract_underscore_call(trimmed) {
                let id = format!("{}#{}", filename, line_num);
                let mut entry = StringEntry::new(id, text, file_path.to_path_buf());
                entry.tags = vec!["ui_label".to_string()];
                entries.push(entry);
                continue;
            }

            // define gui.xxx = "text"
            if let Some(text) = extract_define_string(trimmed) {
                let id = format!("{}#{}", filename, line_num);
                let mut entry = StringEntry::new(id, text, file_path.to_path_buf());
                entry.tags = vec!["ui_label".to_string()];
                entries.push(entry);
                continue;
            }

            // say statement: character "text" or just "text"
            if !in_menu {
                if let Some((character, text)) = extract_say_statement(trimmed) {
                    let id = format!("{}#{}", filename, line_num);
                    let mut entry = StringEntry::new(id, text, file_path.to_path_buf());
                    entry.tags = vec!["dialogue".to_string()];
                    if let Some(ch) = character {
                        entry.context = Some(ch.to_string());
                    }
                    entries.push(entry);
                }
            }
        }

        Ok(entries)
    }
}

fn extract_quoted_string(s: &str) -> Option<(&str, usize)> {
    let s = s.trim();
    if !s.starts_with('"') {
        return None;
    }
    let inner = &s[1..];
    let mut end = 0;
    let mut escaped = false;
    for (i, ch) in inner.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if ch == '\\' {
            escaped = true;
            continue;
        }
        if ch == '"' {
            end = i;
            return Some((&inner[..end], 1 + end + 1));
        }
    }
    None
}

fn extract_say_statement(line: &str) -> Option<(Option<&str>, &str)> {
    let trimmed = line.trim();

    // Skip non-say lines
    if trimmed.is_empty()
        || trimmed.starts_with('#')
        || trimmed.starts_with("label ")
        || trimmed.starts_with("jump ")
        || trimmed.starts_with("return")
        || trimmed.starts_with("define ")
        || trimmed.starts_with("default ")
        || trimmed.starts_with("menu:")
        || trimmed.starts_with("if ")
        || trimmed.starts_with("elif ")
        || trimmed.starts_with("else:")
        || trimmed.starts_with("while ")
        || trimmed.starts_with("for ")
        || trimmed.starts_with("python:")
        || trimmed.starts_with("init ")
        || trimmed.starts_with("$")
        || trimmed.starts_with("scene ")
        || trimmed.starts_with("show ")
        || trimmed.starts_with("hide ")
        || trimmed.starts_with("with ")
        || trimmed.starts_with("play ")
        || trimmed.starts_with("stop ")
        || trimmed.starts_with("pause")
        || trimmed.starts_with("call ")
        || trimmed.starts_with("pass")
        || trimmed.starts_with("translate ")
        || trimmed.starts_with("_")
        // Image/UI property keywords
        || trimmed.starts_with("idle ")
        || trimmed.starts_with("hover ")
        || trimmed.starts_with("insensitive ")
        || trimmed.starts_with("selected_idle ")
        || trimmed.starts_with("selected_hover ")
        || trimmed.starts_with("ground ")
        || trimmed.starts_with("image ")
        || trimmed.starts_with("add ")
        || trimmed.starts_with("use ")
        || trimmed.starts_with("screen ")
        || trimmed.starts_with("style ")
        || trimmed.starts_with("transform ")
        || trimmed.starts_with("at ")
        || trimmed.starts_with("xpos ")
        || trimmed.starts_with("ypos ")
        || trimmed.starts_with("xalign ")
        || trimmed.starts_with("yalign ")
        || trimmed.starts_with("xsize ")
        || trimmed.starts_with("ysize ")
        || trimmed.starts_with("text_align ")
        || trimmed.starts_with("action ")
        || trimmed.starts_with("hovered ")
        || trimmed.starts_with("unhovered ")
        || trimmed.starts_with("background ")
    {
        return None;
    }

    // Narrator: just "text"
    if trimmed.starts_with('"') {
        let (text, _) = extract_quoted_string(trimmed)?;
        if !text.is_empty() && !is_file_reference(text) {
            return Some((None, text));
        }
        return None;
    }

    // Character say: `identifier "text"`
    let parts: Vec<&str> = trimmed.splitn(2, ' ').collect();
    if parts.len() == 2 {
        let character = parts[0];
        // Character must be a simple identifier
        if character
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_')
        {
            let rest = parts[1].trim();
            if rest.starts_with('"') {
                let (text, _) = extract_quoted_string(rest)?;
                if !text.is_empty() && !is_file_reference(text) {
                    return Some((Some(character), text));
                }
            }
        }
    }

    None
}

/// Check if a string looks like a file path/reference (not translatable text)
fn is_file_reference(text: &str) -> bool {
    let t = text.trim();
    // File extensions
    if t.ends_with(".png") || t.ends_with(".jpg") || t.ends_with(".jpeg") || t.ends_with(".webp") ||
       t.ends_with(".gif") || t.ends_with(".svg") || t.ends_with(".bmp") ||
       t.ends_with(".mp3") || t.ends_with(".ogg") || t.ends_with(".wav") || t.ends_with(".flac") ||
       t.ends_with(".mp4") || t.ends_with(".webm") || t.ends_with(".avi") || t.ends_with(".ogv") ||
       t.ends_with(".ttf") || t.ends_with(".otf") || t.ends_with(".woff") ||
       t.ends_with(".rpy") || t.ends_with(".rpyc") || t.ends_with(".rpa") ||
       t.ends_with(".json") || t.ends_with(".txt") || t.ends_with(".xml") || t.ends_with(".csv") {
        return true;
    }
    // Path-like patterns
    if (t.contains('/') || t.contains('\\')) && !t.contains(' ') {
        return true;
    }
    // Color hex codes
    if t.starts_with('#') && t.len() <= 9 && t[1..].chars().all(|c| c.is_ascii_hexdigit()) {
        return true;
    }
    false
}

fn extract_menu_choice(line: &str) -> Option<&str> {
    let trimmed = line.trim();
    if !trimmed.starts_with('"') {
        return None;
    }
    // "Choice text": or "Choice text"
    let (text, end) = extract_quoted_string(trimmed)?;
    if text.is_empty() {
        return None;
    }
    let after = trimmed[end..].trim();
    if after.is_empty() || after == ":" || after.starts_with(':') {
        return Some(text);
    }
    None
}

fn extract_define_string(line: &str) -> Option<&str> {
    let trimmed = line.trim();
    if !trimmed.starts_with("define ") {
        return None;
    }
    // Skip _() calls — handled separately
    if trimmed.contains("_(") {
        return None;
    }
    let eq_pos = trimmed.find('=')?;
    let after_eq = trimmed[eq_pos + 1..].trim();
    let (text, _) = extract_quoted_string(after_eq)?;
    if !text.is_empty() && !is_file_reference(text) {
        Some(text)
    } else {
        None
    }
}

fn extract_underscore_call(line: &str) -> Option<&str> {
    let trimmed = line.trim();
    let start = trimmed.find("_(\"")?;
    let inner = &trimmed[start + 2..]; // after `_(`
    let (text, _) = extract_quoted_string(inner)?;
    if !text.is_empty() {
        Some(text)
    } else {
        None
    }
}

/// Simplified Python pickle parser for RPA index data.
/// The pickle contains a dict mapping filenames (str) to lists of (offset, length, prefix) tuples.
/// We only need to extract the filename, offset, and length.
fn parse_rpa_pickle(data: &[u8], key: i64) -> Result<Vec<(String, u64, usize)>> {
    let mut result = Vec::new();
    let mut pos = 0;
    let len = data.len();

    // Python 2 pickle protocol 2 tokens we care about:
    // \x80\x02 = proto 2
    // } = EMPTY_DICT
    // q/r = SHORT_BINPUT/LONG_BINPUT (memo)
    // X = SHORT_BINUNICODE (4-byte len + utf8)
    // ] = EMPTY_LIST
    // ( = MARK
    // J = BININT (4 bytes little-endian signed)
    // K = BININT1 (1 byte unsigned)
    // M = BININT2 (2 bytes unsigned)
    // \x8a = LONG1 (1-byte length + n bytes little-endian)
    // t = TUPLE
    // a = APPEND
    // e = APPENDS
    // u = SETITEMS
    // s = SETITEM
    // . = STOP

    let mut stack: Vec<PickleVal> = Vec::new();
    let mut mark_stack: Vec<usize> = Vec::new();
    let mut memo: Vec<PickleVal> = Vec::new();
    let mut current_key: Option<String> = None;

    while pos < len {
        let op = data[pos];
        pos += 1;
        match op {
            0x80 => { pos += 1; } // PROTO
            0x95 => { pos += 8; } // FRAME (protocol 4+) — skip 8-byte frame length
            0x94 => { // MEMOIZE (protocol 4+) — store stack top in memo
                if let Some(top) = stack.last() {
                    memo.push(top.clone());
                }
            }
            0x7d => stack.push(PickleVal::Dict), // EMPTY_DICT
            0x5d => stack.push(PickleVal::List(Vec::new())), // EMPTY_LIST
            0x28 => mark_stack.push(stack.len()), // MARK
            0x71 => { // SHORT_BINPUT (memo)
                if pos >= len { break; }
                let idx = data[pos] as usize;
                pos += 1;
                if let Some(top) = stack.last() {
                    while memo.len() <= idx { memo.push(PickleVal::None); }
                    memo[idx] = top.clone();
                }
            }
            0x72 => { // LONG_BINPUT
                if pos + 4 > len { break; }
                let idx = u32::from_le_bytes([data[pos], data[pos+1], data[pos+2], data[pos+3]]) as usize;
                pos += 4;
                if let Some(top) = stack.last() {
                    while memo.len() <= idx { memo.push(PickleVal::None); }
                    memo[idx] = top.clone();
                }
            }
            0x68 => { // SHORT_BINGET
                if pos >= len { break; }
                let idx = data[pos] as usize;
                pos += 1;
                let val = memo.get(idx).cloned().unwrap_or(PickleVal::None);
                stack.push(val);
            }
            0x6a => { // LONG_BINGET
                if pos + 4 > len { break; }
                let idx = u32::from_le_bytes([data[pos], data[pos+1], data[pos+2], data[pos+3]]) as usize;
                pos += 4;
                let val = memo.get(idx).cloned().unwrap_or(PickleVal::None);
                stack.push(val);
            }
            0x43 => { // SHORT_BINBYTES
                if pos >= len { break; }
                let slen = data[pos] as usize;
                pos += 1;
                if pos + slen > len { break; }
                let s = String::from_utf8_lossy(&data[pos..pos+slen]).to_string();
                pos += slen;
                stack.push(PickleVal::Str(s));
            }
            0x44 => { // BINBYTES (4-byte len)
                if pos + 4 > len { break; }
                let slen = u32::from_le_bytes([data[pos], data[pos+1], data[pos+2], data[pos+3]]) as usize;
                pos += 4;
                if pos + slen > len { break; }
                let s = String::from_utf8_lossy(&data[pos..pos+slen]).to_string();
                pos += slen;
                stack.push(PickleVal::Str(s));
            }
            0x8e => { // BINBYTES8 (8-byte len, protocol 4+)
                if pos + 8 > len { break; }
                let slen = u64::from_le_bytes([data[pos], data[pos+1], data[pos+2], data[pos+3], data[pos+4], data[pos+5], data[pos+6], data[pos+7]]) as usize;
                pos += 8;
                if pos + slen > len { break; }
                let s = String::from_utf8_lossy(&data[pos..pos+slen]).to_string();
                pos += slen;
                stack.push(PickleVal::Str(s));
            }
            0x8c => { // SHORT_BINUNICODE (protocol 4+) — 1-byte length
                if pos >= len { break; }
                let slen = data[pos] as usize;
                pos += 1;
                if pos + slen > len { break; }
                let s = String::from_utf8_lossy(&data[pos..pos+slen]).to_string();
                pos += slen;
                stack.push(PickleVal::Str(s));
            }
            0x55 => { // SHORT_BINSTRING
                if pos >= len { break; }
                let slen = data[pos] as usize;
                pos += 1;
                if pos + slen > len { break; }
                let s = String::from_utf8_lossy(&data[pos..pos+slen]).to_string();
                pos += slen;
                stack.push(PickleVal::Str(s));
            }
            0x54 => { // BINSTRING (4-byte len)
                if pos + 4 > len { break; }
                let slen = u32::from_le_bytes([data[pos], data[pos+1], data[pos+2], data[pos+3]]) as usize;
                pos += 4;
                if pos + slen > len { break; }
                let s = String::from_utf8_lossy(&data[pos..pos+slen]).to_string();
                pos += slen;
                stack.push(PickleVal::Str(s));
            }
            0x4a => { // BININT
                if pos + 4 > len { break; }
                let v = i32::from_le_bytes([data[pos], data[pos+1], data[pos+2], data[pos+3]]) as i64;
                pos += 4;
                stack.push(PickleVal::Int(v));
            }
            0x4b => { // BININT1
                if pos >= len { break; }
                stack.push(PickleVal::Int(data[pos] as i64));
                pos += 1;
            }
            0x4d => { // BININT2
                if pos + 2 > len { break; }
                let v = u16::from_le_bytes([data[pos], data[pos+1]]) as i64;
                pos += 2;
                stack.push(PickleVal::Int(v));
            }
            0x8a => { // LONG1
                if pos >= len { break; }
                let nbytes = data[pos] as usize;
                pos += 1;
                if pos + nbytes > len { break; }
                let mut v: i64 = 0;
                for i in 0..nbytes.min(8) {
                    v |= (data[pos + i] as i64) << (i * 8);
                }
                pos += nbytes;
                stack.push(PickleVal::Int(v));
            }
            0x74 => { // TUPLE
                let mark = mark_stack.pop().unwrap_or(0).min(stack.len());
                let items: Vec<PickleVal> = stack.drain(mark..).collect();
                stack.push(PickleVal::Tuple(items));
            }
            0x85 => { // TUPLE1
                let v = stack.pop().unwrap_or(PickleVal::None);
                stack.push(PickleVal::Tuple(vec![v]));
            }
            0x86 => { // TUPLE2
                let b = stack.pop().unwrap_or(PickleVal::None);
                let a = stack.pop().unwrap_or(PickleVal::None);
                stack.push(PickleVal::Tuple(vec![a, b]));
            }
            0x87 => { // TUPLE3
                let c = stack.pop().unwrap_or(PickleVal::None);
                let b = stack.pop().unwrap_or(PickleVal::None);
                let a = stack.pop().unwrap_or(PickleVal::None);
                stack.push(PickleVal::Tuple(vec![a, b, c]));
            }
            0x61 => { // APPEND
                let val = stack.pop().unwrap_or(PickleVal::None);
                if let Some(PickleVal::List(ref mut list)) = stack.last_mut() {
                    list.push(val);
                }
            }
            0x65 => { // APPENDS
                let mark = mark_stack.pop().unwrap_or(stack.len()).min(stack.len());
                let items: Vec<PickleVal> = stack.drain(mark..).collect();
                if let Some(PickleVal::List(ref mut list)) = stack.last_mut() {
                    list.extend(items);
                }
            }
            0x73 => { // SETITEM
                let val = stack.pop().unwrap_or(PickleVal::None);
                let k = stack.pop().unwrap_or(PickleVal::None);
                if let PickleVal::Str(ref name) = k {
                    current_key = Some(name.clone());
                }
                // Process: key should be a string (filename), val should be a list of tuples
                if let (Some(ref filename), PickleVal::List(ref items)) = (&current_key, &val) {
                    for item in items {
                        if let PickleVal::Tuple(ref t) = item {
                            if t.len() >= 2 {
                                let offset = t[0].as_int().unwrap_or(0) ^ key;
                                let length = t[1].as_int().unwrap_or(0) ^ key;
                                let prefix_len = if t.len() >= 3 {
                                    if let PickleVal::Str(ref s) = t[2] { s.len() } else { 0 }
                                } else { 0 };
                                result.push((
                                    filename.clone(),
                                    (offset as u64) + prefix_len as u64,
                                    (length as usize).saturating_sub(prefix_len),
                                ));
                            }
                        }
                    }
                    current_key = None;
                }
            }
            0x75 => { // SETITEMS
                let mark = mark_stack.pop().unwrap_or(0).min(stack.len());
                let items: Vec<PickleVal> = stack.drain(mark..).collect();
                // Items come in pairs: key, val, key, val, ...
                let mut i = 0;
                while i + 1 < items.len() {
                    let k = &items[i];
                    let v = &items[i + 1];
                    if let PickleVal::Str(ref filename) = k {
                        if let PickleVal::List(ref entries) = v {
                            for entry in entries {
                                if let PickleVal::Tuple(ref t) = entry {
                                    if t.len() >= 2 {
                                        let offset = t[0].as_int().unwrap_or(0) ^ key;
                                        let length = t[1].as_int().unwrap_or(0) ^ key;
                                        let prefix_len = if t.len() >= 3 {
                                            if let PickleVal::Str(ref s) = t[2] { s.len() } else { 0 }
                                        } else { 0 };
                                        result.push((
                                            filename.clone(),
                                            (offset as u64) + prefix_len as u64,
                                            (length as usize).saturating_sub(prefix_len),
                                        ));
                                    }
                                }
                            }
                        }
                    }
                    i += 2;
                }
            }
            0x4e => stack.push(PickleVal::None), // NONE
            0x88 => stack.push(PickleVal::Int(1)), // NEWTRUE
            0x89 => stack.push(PickleVal::Int(0)), // NEWFALSE
            0x2e => break, // STOP
            _ => {} // Skip unknown opcodes
        }
    }

    Ok(result)
}

#[derive(Debug, Clone)]
enum PickleVal {
    None,
    Int(i64),
    Str(String),
    List(Vec<PickleVal>),
    Tuple(Vec<PickleVal>),
    Dict,
}

impl PickleVal {
    fn as_int(&self) -> Option<i64> {
        match self {
            PickleVal::Int(v) => Some(*v),
            _ => None,
        }
    }
}

impl Default for RenPyPlugin {
    fn default() -> Self {
        Self::new()
    }
}

impl FormatPlugin for RenPyPlugin {
    fn id(&self) -> &str {
        "renpy"
    }

    fn name(&self) -> &str {
        "Ren'Py"
    }

    fn description(&self) -> &str {
        "Ren'Py visual novel .rpy script files"
    }

    fn supported_extensions(&self) -> &[&str] {
        &[".rpy"]
    }

    fn supported_modes(&self) -> Vec<OutputMode> {
        vec![OutputMode::Replace, OutputMode::Add]
    }

    fn detect(&self, path: &Path) -> bool {
        if path.is_file() {
            let ext = path.extension().unwrap_or_default();
            return ext == "rpy" || ext == "rpa";
        }
        if path.is_dir() {
            if let Some(game_dir) = Self::find_game_dir(path) {
                return Self::has_rpy_files(&game_dir) || Self::has_rpa_files(&game_dir);
            }
        }
        false
    }

    fn extract(&self, path: &Path) -> Result<Vec<StringEntry>> {
        if path.is_file() {
            if path.extension().map_or(false, |e| e == "rpa") {
                return self.extract_rpa_archive(path);
            }
            return Self::extract_file(path);
        }

        let game_dir = Self::find_game_dir(path).ok_or_else(|| {
            locust_core::error::LocustError::ParseError {
                file: path.display().to_string(),
                message: "could not find game/ directory".to_string(),
            }
        })?;

        // First try .rpy files directly
        let mut all = Vec::new();
        let mut found_rpy = false;
        for entry in walkdir::WalkDir::new(&game_dir)
            .follow_links(false)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            let fpath = entry.path();
            if fpath.extension().map_or(false, |e| e == "rpy") {
                // Skip tl/ directory and renpy/ engine dir
                if let Ok(rel) = fpath.strip_prefix(&game_dir) {
                    if rel.starts_with("tl") {
                        continue;
                    }
                }
                // Skip the renpy engine directory
                if let Some(parent_root) = game_dir.parent() {
                    if fpath.starts_with(parent_root.join("renpy")) {
                        continue;
                    }
                }
                found_rpy = true;
                match Self::extract_file(fpath) {
                    Ok(entries) => all.extend(entries),
                    Err(e) => {
                        tracing::warn!("Failed to extract {}: {}", fpath.display(), e);
                    }
                }
            }
        }

        // If no .rpy files found, try extracting from .rpa archives
        if !found_rpy {
            for entry in std::fs::read_dir(&game_dir)?.filter_map(|e| e.ok()) {
                let fpath = entry.path();
                if fpath.extension().map_or(false, |e| e == "rpa") {
                    if fpath.file_name().map_or(false, |n| {
                        let name = n.to_string_lossy();
                        name.contains("script") || name == "archive.rpa"
                    }) {
                        match self.extract_rpa_archive(&fpath) {
                            Ok(entries) => all.extend(entries),
                            Err(e) => {
                                tracing::warn!("Failed to extract RPA {}: {}", fpath.display(), e);
                            }
                        }
                    }
                }
            }

            // If still nothing found from named archives, try all .rpa files
            if all.is_empty() {
                for entry in std::fs::read_dir(&game_dir)?.filter_map(|e| e.ok()) {
                    let fpath = entry.path();
                    if fpath.extension().map_or(false, |e| e == "rpa") {
                        match self.extract_rpa_archive(&fpath) {
                            Ok(entries) => all.extend(entries),
                            Err(e) => {
                                tracing::warn!("Failed to extract RPA {}: {}", fpath.display(), e);
                            }
                        }
                    }
                }
            }
        }

        Ok(all)
    }

    fn inject(&self, path: &Path, entries: &[StringEntry]) -> Result<InjectionReport> {
        let mut files_modified = 0;
        let mut strings_written = 0;
        let mut strings_skipped = 0;

        // Group by file
        let mut by_file: HashMap<PathBuf, Vec<&StringEntry>> = HashMap::new();
        for entry in entries {
            by_file
                .entry(entry.file_path.clone())
                .or_default()
                .push(entry);
        }

        for (file_path, file_entries) in &by_file {
            if !file_path.exists() {
                continue;
            }
            let content = std::fs::read_to_string(file_path)?;
            let filename = file_path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();

            // Build lookup: line_num -> translation
            let mut line_translations: HashMap<usize, &str> = HashMap::new();
            let mut source_lookup: HashMap<usize, &str> = HashMap::new();
            for entry in file_entries {
                let id_suffix = entry.id.strip_prefix(&format!("{}#", filename));
                if let Some(num_str) = id_suffix {
                    if let Ok(line_num) = num_str.parse::<usize>() {
                        if let Some(ref t) = entry.translation {
                            line_translations.insert(line_num, t.as_str());
                            source_lookup.insert(line_num, entry.source.as_str());
                            strings_written += 1;
                        } else {
                            strings_skipped += 1;
                        }
                    }
                }
            }

            let mut new_lines = Vec::new();
            for (line_idx, line) in content.lines().enumerate() {
                let line_num = line_idx + 1;
                if let Some(&translation) = line_translations.get(&line_num) {
                    if let Some(&source) = source_lookup.get(&line_num) {
                        // Replace the source string with translation in-place
                        let new_line = line.replacen(
                            &format!("\"{}\"", source),
                            &format!("\"{}\"", translation),
                            1,
                        );
                        new_lines.push(new_line);
                        continue;
                    }
                }
                new_lines.push(line.to_string());
            }

            std::fs::write(file_path, new_lines.join("\n"))?;
            files_modified += 1;
        }

        Ok(InjectionReport {
            files_modified,
            strings_written,
            strings_skipped,
            warnings: Vec::new(),
        })
    }

    fn inject_add(
        &self,
        path: &Path,
        lang: &str,
        entries: &[StringEntry],
    ) -> Result<InjectionReport> {
        let game_dir = if path.is_dir() {
            Self::find_game_dir(path).unwrap_or_else(|| path.join("game"))
        } else {
            path.parent()
                .unwrap_or(path)
                .to_path_buf()
        };

        let tl_dir = game_dir.join("tl").join(lang);
        std::fs::create_dir_all(&tl_dir)?;

        let mut strings_written = 0;
        let mut strings_skipped = 0;

        // Ren'Py translate strings block: works for all string types (say, menu, define, etc.)
        // This is the most reliable translation method as it doesn't depend on internal hash IDs.
        // Format:
        //   translate <lang> strings:
        //       old "source text"
        //       new "translated text"
        let mut string_pairs: Vec<(&str, &str)> = Vec::new();

        for entry in entries {
            if let Some(ref translation) = entry.translation {
                if translation != &entry.source {
                    string_pairs.push((&entry.source, translation.as_str()));
                    strings_written += 1;
                } else {
                    strings_skipped += 1;
                }
            } else {
                strings_skipped += 1;
            }
        }

        if !string_pairs.is_empty() {
            let mut lines = Vec::new();
            lines.push(format!("translate {} strings:", lang));
            lines.push(String::new());

            for (source, translation) in &string_pairs {
                // Escape quotes in strings
                let escaped_source = source.replace('\\', "\\\\").replace('"', "\\\"");
                let escaped_translation = translation.replace('\\', "\\\\").replace('"', "\\\"");
                lines.push(format!("    old \"{}\"", escaped_source));
                lines.push(format!("    new \"{}\"", escaped_translation));
                lines.push(String::new());
            }

            let tl_file = tl_dir.join("locust_strings.rpy");
            std::fs::write(&tl_file, lines.join("\n"))?;
        }

        Ok(InjectionReport {
            files_modified: 1,
            strings_written,
            strings_skipped,
            warnings: Vec::new(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn fixture_dir() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("fixtures")
            .join("renpy")
    }

    fn temp_renpy_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("locust_renpy_{}", uuid::Uuid::new_v4()));
        copy_dir(&fixture_dir(), &dir);
        dir
    }

    fn copy_dir(src: &Path, dst: &Path) {
        fs::create_dir_all(dst).unwrap();
        for entry in walkdir::WalkDir::new(src).follow_links(false) {
            let entry = entry.unwrap();
            let rel = entry.path().strip_prefix(src).unwrap();
            let dest = dst.join(rel);
            if entry.file_type().is_dir() {
                fs::create_dir_all(&dest).unwrap();
            } else {
                fs::copy(entry.path(), &dest).unwrap();
            }
        }
    }

    #[test]
    fn test_detect_renpy_dir() {
        let dir = fixture_dir();
        let plugin = RenPyPlugin::new();
        assert!(plugin.detect(&dir));
    }

    #[test]
    fn test_detect_renpy_file() {
        let file = fixture_dir().join("game").join("script.rpy");
        let plugin = RenPyPlugin::new();
        assert!(plugin.detect(&file));
    }

    #[test]
    fn test_detect_non_renpy() {
        let dir = std::env::temp_dir().join(format!("locust_notrenpy_{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        let plugin = RenPyPlugin::new();
        assert!(!plugin.detect(&dir));
    }

    #[test]
    fn test_extract_say_statements() {
        let plugin = RenPyPlugin::new();
        let entries = plugin.extract(&fixture_dir()).unwrap();
        let hello = entries.iter().find(|e| e.source == "Hello, world!");
        assert!(hello.is_some(), "entries: {:?}", entries.iter().map(|e| (&e.id, &e.source)).collect::<Vec<_>>());
        assert_eq!(hello.unwrap().context, Some("e".to_string()));

        let narrator = entries
            .iter()
            .find(|e| e.source == "This is the narrator speaking.");
        assert!(narrator.is_some());
        assert!(narrator.unwrap().context.is_none());
    }

    #[test]
    fn test_extract_menu_choices() {
        let plugin = RenPyPlugin::new();
        let entries = plugin.extract(&fixture_dir()).unwrap();
        let left = entries.iter().find(|e| e.source == "Go left");
        assert!(left.is_some());
        assert!(left.unwrap().tags.contains(&"menu".to_string()));

        let right = entries.iter().find(|e| e.source == "Go right");
        assert!(right.is_some());
        assert!(right.unwrap().tags.contains(&"menu".to_string()));
    }

    #[test]
    fn test_extract_define_strings() {
        let plugin = RenPyPlugin::new();
        let entries = plugin.extract(&fixture_dir()).unwrap();
        let title = entries.iter().find(|e| e.source == "My Visual Novel");
        assert!(title.is_some());
        assert!(title.unwrap().tags.contains(&"ui_label".to_string()));
    }

    #[test]
    fn test_extract_python_i18n() {
        let plugin = RenPyPlugin::new();
        let entries = plugin.extract(&fixture_dir()).unwrap();
        let version = entries.iter().find(|e| e.source == "Version 1.0");
        assert!(version.is_some());
        assert!(version.unwrap().tags.contains(&"ui_label".to_string()));
    }

    #[test]
    fn test_inject_replace_roundtrip() {
        let dir = temp_renpy_dir();
        let plugin = RenPyPlugin::new();
        let mut entries = plugin.extract(&dir).unwrap();

        for entry in &mut entries {
            if entry.source == "Hello, world!" {
                entry.translation = Some("Hola, mundo!".to_string());
            }
        }

        plugin.inject(&dir, &entries).unwrap();

        let content = fs::read_to_string(dir.join("game").join("script.rpy")).unwrap();
        assert!(content.contains("\"Hola, mundo!\""));
        assert!(!content.contains("\"Hello, world!\""));
    }

    #[test]
    fn test_inject_add_creates_tl_dir() {
        let dir = temp_renpy_dir();
        let plugin = RenPyPlugin::new();
        let mut entries = plugin.extract(&dir).unwrap();
        for entry in &mut entries {
            entry.translation = Some(format!("[es] {}", entry.source));
        }

        plugin.inject_add(&dir, "es", &entries).unwrap();

        let tl_dir = dir.join("game").join("tl").join("es");
        assert!(tl_dir.exists());
    }

    #[test]
    fn test_inject_add_format() {
        let dir = temp_renpy_dir();
        let plugin = RenPyPlugin::new();
        let mut entries = plugin.extract(&dir).unwrap();
        for entry in &mut entries {
            entry.translation = Some(format!("[es] {}", entry.source));
        }

        plugin.inject_add(&dir, "es", &entries).unwrap();

        let tl_dir = dir.join("game").join("tl").join("es");
        // Check that at least one translation file was created
        let tl_files: Vec<_> = fs::read_dir(&tl_dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .collect();
        assert!(!tl_files.is_empty());

        // Read and check format
        let content = fs::read_to_string(tl_files[0].path()).unwrap();
        assert!(content.contains("translate es"));
    }

    #[test]
    fn test_entry_ids_include_line_numbers() {
        let plugin = RenPyPlugin::new();
        let entries = plugin.extract(&fixture_dir()).unwrap();
        for entry in &entries {
            let parts: Vec<&str> = entry.id.split('#').collect();
            assert_eq!(parts.len(), 2, "id should be filename#line: {}", entry.id);
            assert!(
                parts[1].parse::<usize>().is_ok(),
                "second part should be a number: {}",
                entry.id
            );
        }
    }
}
