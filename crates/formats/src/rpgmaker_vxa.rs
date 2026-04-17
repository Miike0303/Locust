use std::collections::HashMap;
use std::path::{Path, PathBuf};

use locust_core::error::{LocustError, Result};
use locust_core::extraction::{FormatPlugin, InjectionReport};
use locust_core::models::{OutputMode, StringEntry};

// ─── Ruby Marshal parser/writer ────────────────────────────────────────────

#[derive(Clone, Debug)]
pub enum MarshalValue {
    Nil,
    Bool(bool),
    Int(i64),
    Str(String),
    Symbol(String),
    Array(Vec<MarshalValue>),
    Hash(Vec<(MarshalValue, MarshalValue)>),
    Object {
        class: String,
        ivars: HashMap<String, MarshalValue>,
    },
    UserDefined {
        class: String,
        data: Vec<u8>,
    },
    Unsupported,
}

impl MarshalValue {
    pub fn parse(bytes: &[u8]) -> Result<MarshalValue> {
        if bytes.len() < 2 || bytes[0] != 4 || bytes[1] != 8 {
            return Err(LocustError::ParseError {
                file: String::new(),
                message: "invalid Ruby Marshal header".to_string(),
            });
        }
        let mut reader = MarshalReader::new(&bytes[2..]);
        reader.read_value()
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            MarshalValue::Str(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_array(&self) -> Option<&[MarshalValue]> {
        match self {
            MarshalValue::Array(a) => Some(a),
            _ => None,
        }
    }

    pub fn get_ivar(&self, name: &str) -> Option<&MarshalValue> {
        match self {
            MarshalValue::Object { ivars, .. } => ivars.get(name),
            _ => None,
        }
    }

    pub fn get_ivar_mut(&mut self, name: &str) -> Option<&mut MarshalValue> {
        match self {
            MarshalValue::Object { ivars, .. } => ivars.get_mut(name),
            _ => None,
        }
    }

    pub fn serialize(&self) -> Vec<u8> {
        let mut writer = MarshalWriter::new();
        writer.write_header();
        writer.write_value(self);
        writer.buf
    }
}

struct MarshalReader<'a> {
    data: &'a [u8],
    pos: usize,
    symbols: Vec<String>,
    objects: Vec<usize>, // placeholder indices for object refs
}

impl<'a> MarshalReader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self {
            data,
            pos: 0,
            symbols: Vec::new(),
            objects: Vec::new(),
        }
    }

    fn read_byte(&mut self) -> Result<u8> {
        if self.pos >= self.data.len() {
            return Err(LocustError::ParseError {
                file: String::new(),
                message: "unexpected end of marshal data".to_string(),
            });
        }
        let b = self.data[self.pos];
        self.pos += 1;
        Ok(b)
    }

    fn read_bytes(&mut self, n: usize) -> Result<&'a [u8]> {
        if self.pos + n > self.data.len() {
            return Err(LocustError::ParseError {
                file: String::new(),
                message: "unexpected end of marshal data".to_string(),
            });
        }
        let slice = &self.data[self.pos..self.pos + n];
        self.pos += n;
        Ok(slice)
    }

    fn read_packed_int(&mut self) -> Result<i64> {
        let c = self.read_byte()? as i8;
        if c == 0 {
            return Ok(0);
        }
        if c > 0 && c <= 4 {
            let n = c as usize;
            let mut val = 0i64;
            for i in 0..n {
                val |= (self.read_byte()? as i64) << (8 * i);
            }
            return Ok(val);
        }
        if c >= -4 && c < 0 {
            let n = (-c) as usize;
            let mut val = -1i64;
            for i in 0..n {
                val &= !(0xFF << (8 * i));
                val |= (self.read_byte()? as i64) << (8 * i);
            }
            return Ok(val);
        }
        // Small integers: c > 5 => c - 5, c < -4 => c + 5
        if c > 4 {
            Ok((c as i64) - 5)
        } else {
            Ok((c as i64) + 5)
        }
    }

    fn read_symbol(&mut self) -> Result<String> {
        let len = self.read_packed_int()? as usize;
        let bytes = self.read_bytes(len)?;
        let s = String::from_utf8_lossy(bytes).to_string();
        self.symbols.push(s.clone());
        Ok(s)
    }

    fn read_symbol_or_ref(&mut self) -> Result<String> {
        let tag = self.read_byte()?;
        match tag {
            b':' => self.read_symbol(),
            b';' => {
                let idx = self.read_packed_int()? as usize;
                self.symbols
                    .get(idx)
                    .cloned()
                    .ok_or_else(|| LocustError::ParseError {
                        file: String::new(),
                        message: format!("invalid symbol ref: {}", idx),
                    })
            }
            _ => Err(LocustError::ParseError {
                file: String::new(),
                message: format!("expected symbol, got 0x{:02x}", tag),
            }),
        }
    }

    fn read_raw_string(&mut self) -> Result<String> {
        let len = self.read_packed_int()? as usize;
        let bytes = self.read_bytes(len)?;
        Ok(String::from_utf8_lossy(bytes).to_string())
    }

    fn read_value(&mut self) -> Result<MarshalValue> {
        let tag = self.read_byte()?;
        match tag {
            b'0' => Ok(MarshalValue::Nil),
            b'T' => Ok(MarshalValue::Bool(true)),
            b'F' => Ok(MarshalValue::Bool(false)),
            b'i' => {
                let val = self.read_packed_int()?;
                Ok(MarshalValue::Int(val))
            }
            b'"' => {
                self.objects.push(self.pos);
                let s = self.read_raw_string()?;
                Ok(MarshalValue::Str(s))
            }
            b':' => {
                let s = self.read_symbol()?;
                Ok(MarshalValue::Symbol(s))
            }
            b';' => {
                let idx = self.read_packed_int()? as usize;
                let s = self.symbols.get(idx).cloned().unwrap_or_default();
                Ok(MarshalValue::Symbol(s))
            }
            b'@' => {
                let _idx = self.read_packed_int()?;
                // Object reference — we can't easily resolve, return Unsupported
                Ok(MarshalValue::Unsupported)
            }
            b'[' => {
                self.objects.push(self.pos);
                let count = self.read_packed_int()? as usize;
                let mut arr = Vec::with_capacity(count);
                for _ in 0..count {
                    arr.push(self.read_value()?);
                }
                Ok(MarshalValue::Array(arr))
            }
            b'{' => {
                self.objects.push(self.pos);
                let count = self.read_packed_int()? as usize;
                let mut pairs = Vec::with_capacity(count);
                for _ in 0..count {
                    let k = self.read_value()?;
                    let v = self.read_value()?;
                    pairs.push((k, v));
                }
                Ok(MarshalValue::Hash(pairs))
            }
            b'o' => {
                self.objects.push(self.pos);
                let class = self.read_symbol_or_ref()?;
                let ivar_count = self.read_packed_int()? as usize;
                let mut ivars = HashMap::new();
                for _ in 0..ivar_count {
                    let key = self.read_symbol_or_ref()?;
                    let val = self.read_value()?;
                    ivars.insert(key, val);
                }
                Ok(MarshalValue::Object { class, ivars })
            }
            b'I' => {
                // IVAR wrapper (typically wraps a string with encoding info)
                self.objects.push(self.pos);
                let inner = self.read_value()?;
                let ivar_count = self.read_packed_int()? as usize;
                for _ in 0..ivar_count {
                    let _key = self.read_symbol_or_ref()?;
                    let _val = self.read_value()?;
                }
                // Return the inner value (usually a string)
                Ok(inner)
            }
            b'u' => {
                self.objects.push(self.pos);
                let class = self.read_symbol_or_ref()?;
                let len = self.read_packed_int()? as usize;
                let data = self.read_bytes(len)?.to_vec();
                Ok(MarshalValue::UserDefined { class, data })
            }
            _ => {
                // Skip unknown types gracefully
                Ok(MarshalValue::Unsupported)
            }
        }
    }
}

struct MarshalWriter {
    buf: Vec<u8>,
    symbols: Vec<String>,
}

impl MarshalWriter {
    fn new() -> Self {
        Self {
            buf: Vec::new(),
            symbols: Vec::new(),
        }
    }

    fn write_header(&mut self) {
        self.buf.push(4);
        self.buf.push(8);
    }

    fn write_packed_int(&mut self, val: i64) {
        if val == 0 {
            self.buf.push(0);
            return;
        }
        if val > 0 && val < 123 {
            self.buf.push((val + 5) as u8);
            return;
        }
        if val < 0 && val > -124 {
            self.buf.push((val - 5) as u8);
            return;
        }
        if val > 0 {
            let bytes = val.to_le_bytes();
            let n = if val <= 0xFF {
                1
            } else if val <= 0xFFFF {
                2
            } else if val <= 0xFF_FFFF {
                3
            } else {
                4
            };
            self.buf.push(n as u8);
            self.buf.extend_from_slice(&bytes[..n]);
        } else {
            let bytes = val.to_le_bytes();
            let n = if val >= -0x80 {
                1
            } else if val >= -0x8000 {
                2
            } else if val >= -0x80_0000 {
                3
            } else {
                4
            };
            self.buf.push((-n_i8(n)) as u8);
            self.buf.extend_from_slice(&bytes[..n as usize]);
        }
    }

    fn write_symbol(&mut self, s: &str) {
        if let Some(idx) = self.symbols.iter().position(|sym| sym == s) {
            self.buf.push(b';');
            self.write_packed_int(idx as i64);
        } else {
            self.symbols.push(s.to_string());
            self.buf.push(b':');
            let bytes = s.as_bytes();
            self.write_packed_int(bytes.len() as i64);
            self.buf.extend_from_slice(bytes);
        }
    }

    fn write_string_with_encoding(&mut self, s: &str) {
        // IVAR wrapper with encoding
        self.buf.push(b'I');
        self.buf.push(b'"');
        let bytes = s.as_bytes();
        self.write_packed_int(bytes.len() as i64);
        self.buf.extend_from_slice(bytes);
        // 1 ivar: :E => true (UTF-8 encoding)
        self.write_packed_int(1);
        self.write_symbol("E");
        self.buf.push(b'T');
    }

    fn write_value(&mut self, val: &MarshalValue) {
        match val {
            MarshalValue::Nil => self.buf.push(b'0'),
            MarshalValue::Bool(true) => self.buf.push(b'T'),
            MarshalValue::Bool(false) => self.buf.push(b'F'),
            MarshalValue::Int(v) => {
                self.buf.push(b'i');
                self.write_packed_int(*v);
            }
            MarshalValue::Str(s) => {
                self.write_string_with_encoding(s);
            }
            MarshalValue::Symbol(s) => {
                self.write_symbol(s);
            }
            MarshalValue::Array(arr) => {
                self.buf.push(b'[');
                self.write_packed_int(arr.len() as i64);
                for item in arr {
                    self.write_value(item);
                }
            }
            MarshalValue::Hash(pairs) => {
                self.buf.push(b'{');
                self.write_packed_int(pairs.len() as i64);
                for (k, v) in pairs {
                    self.write_value(k);
                    self.write_value(v);
                }
            }
            MarshalValue::Object { class, ivars } => {
                self.buf.push(b'o');
                self.write_symbol(class);
                self.write_packed_int(ivars.len() as i64);
                for (key, val) in ivars {
                    self.write_symbol(key);
                    self.write_value(val);
                }
            }
            MarshalValue::UserDefined { class, data } => {
                self.buf.push(b'u');
                self.write_symbol(class);
                self.write_packed_int(data.len() as i64);
                self.buf.extend_from_slice(data);
            }
            MarshalValue::Unsupported => {
                self.buf.push(b'0'); // write nil for unsupported
            }
        }
    }
}

fn n_i8(n: usize) -> i8 {
    n as i8
}

// ─── VXA Plugin ────────────────────────────────────────────────────────────

const ACTOR_FIELDS: &[(&str, &str)] = &[
    ("@name", "actor_name"),
    ("@description", "description"),
    ("@note", "note"),
    ("@nickname", "actor_name"),
];

const SKILL_FIELDS: &[(&str, &str)] = &[
    ("@name", "actor_name"),
    ("@description", "description"),
    ("@note", "note"),
    ("@message1", "dialogue"),
    ("@message2", "dialogue"),
];

const ITEM_FIELDS: &[(&str, &str)] = &[
    ("@name", "actor_name"),
    ("@description", "description"),
    ("@note", "note"),
];

pub struct RpgMakerVxaPlugin;

impl RpgMakerVxaPlugin {
    pub fn new() -> Self {
        Self
    }

    fn find_data_dir(path: &Path) -> Option<PathBuf> {
        if path.is_dir() {
            let data = path.join("Data");
            if data.is_dir() {
                return Some(data);
            }
        }
        None
    }

    fn fields_for_file(stem: &str) -> &'static [(&'static str, &'static str)] {
        let lower = stem.to_lowercase();
        match lower.as_str() {
            "actors" | "classes" | "enemies" | "states" => ACTOR_FIELDS,
            "skills" => SKILL_FIELDS,
            "items" | "weapons" | "armors" => ITEM_FIELDS,
            _ => &[],
        }
    }

    fn extract_array_file(
        filename: &str,
        root: &MarshalValue,
        file_path: &Path,
    ) -> Vec<StringEntry> {
        let mut entries = Vec::new();
        let stem = strip_marshal_ext(filename);
        let fields = Self::fields_for_file(stem);

        if let Some(arr) = root.as_array() {
            for (idx, item) in arr.iter().enumerate() {
                if matches!(item, MarshalValue::Nil) {
                    continue;
                }
                for &(field, tag) in fields {
                    if let Some(val) = item.get_ivar(field) {
                        if let Some(s) = val.as_str() {
                            if !s.trim().is_empty() {
                                let id = format!("{}#{}#{}", filename, idx, field);
                                let mut entry =
                                    StringEntry::new(id, s, file_path.to_path_buf());
                                entry.tags = vec![tag.to_string()];
                                entries.push(entry);
                            }
                        }
                    }
                }
            }
        }
        entries
    }

    fn extract_map_file(
        filename: &str,
        root: &MarshalValue,
        file_path: &Path,
    ) -> Vec<StringEntry> {
        let mut entries = Vec::new();
        let events = match root.get_ivar("@events") {
            Some(v) => v,
            None => return entries,
        };

        let event_pairs = match events {
            MarshalValue::Hash(pairs) => pairs,
            _ => return entries,
        };

        for (ev_key, ev_val) in event_pairs {
            let ev_id = match ev_key {
                MarshalValue::Int(i) => *i,
                _ => continue,
            };
            let pages = match ev_val.get_ivar("@pages") {
                Some(MarshalValue::Array(a)) => a,
                _ => continue,
            };
            for (page_idx, page) in pages.iter().enumerate() {
                let list = match page.get_ivar("@list") {
                    Some(MarshalValue::Array(a)) => a,
                    _ => continue,
                };
                for (cmd_idx, cmd) in list.iter().enumerate() {
                    let code = match cmd.get_ivar("@code") {
                        Some(MarshalValue::Int(c)) => *c,
                        _ => continue,
                    };
                    let params = match cmd.get_ivar("@parameters") {
                        Some(MarshalValue::Array(a)) => a,
                        _ => continue,
                    };
                    match code {
                        401 => {
                            if let Some(MarshalValue::Str(text)) = params.first() {
                                if !text.trim().is_empty() {
                                    let id = format!(
                                        "{}#0#event_{}#page_{}#cmd_{}",
                                        filename, ev_id, page_idx, cmd_idx
                                    );
                                    let mut entry =
                                        StringEntry::new(id, text.as_str(), file_path.to_path_buf());
                                    entry.tags = vec!["dialogue".to_string()];
                                    entries.push(entry);
                                }
                            }
                        }
                        102 => {
                            if let Some(MarshalValue::Array(choices)) = params.first() {
                                for (ci, choice) in choices.iter().enumerate() {
                                    if let Some(text) = choice.as_str() {
                                        if !text.trim().is_empty() {
                                            let id = format!(
                                                "{}#0#event_{}#page_{}#cmd_{}#choice_{}",
                                                filename, ev_id, page_idx, cmd_idx, ci
                                            );
                                            let mut entry = StringEntry::new(
                                                id,
                                                text,
                                                file_path.to_path_buf(),
                                            );
                                            entry.tags = vec!["menu".to_string()];
                                            entries.push(entry);
                                        }
                                    }
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
        entries
    }

    fn extract_common_events(
        filename: &str,
        root: &MarshalValue,
        file_path: &Path,
    ) -> Vec<StringEntry> {
        let mut entries = Vec::new();
        let arr = match root.as_array() {
            Some(a) => a,
            None => return entries,
        };
        for (ev_idx, event) in arr.iter().enumerate() {
            if matches!(event, MarshalValue::Nil) {
                continue;
            }
            let list = match event.get_ivar("@list") {
                Some(MarshalValue::Array(a)) => a,
                _ => continue,
            };
            for (cmd_idx, cmd) in list.iter().enumerate() {
                let code = match cmd.get_ivar("@code") {
                    Some(MarshalValue::Int(c)) => *c,
                    _ => continue,
                };
                let params = match cmd.get_ivar("@parameters") {
                    Some(MarshalValue::Array(a)) => a,
                    _ => continue,
                };
                if code == 401 {
                    if let Some(MarshalValue::Str(text)) = params.first() {
                        if !text.trim().is_empty() {
                            let id = format!("{}#{}#cmd_{}", filename, ev_idx, cmd_idx);
                            let mut entry =
                                StringEntry::new(id, text.as_str(), file_path.to_path_buf());
                            entry.tags = vec!["dialogue".to_string()];
                            entries.push(entry);
                        }
                    }
                }
            }
        }
        entries
    }

    fn apply_translations(root: &mut MarshalValue, filename: &str, entries: &[StringEntry]) {
        let lookup: HashMap<&str, &str> = entries
            .iter()
            .filter_map(|e| {
                e.translation
                    .as_deref()
                    .map(|t| (e.id.as_str(), t))
            })
            .collect();

        if lookup.is_empty() {
            return;
        }

        let stem_lower = strip_marshal_ext(filename).to_lowercase();

        if stem_lower.starts_with("map") && stem_lower != "mapinfos" {
            // Map files: navigate @events → @pages → @list → commands
            Self::apply_map_translations(root, filename, &lookup);
        } else if stem_lower == "commonevents" {
            // CommonEvents: navigate array → @list → commands
            Self::apply_common_event_translations(root, filename, &lookup);
        } else {
            // Array data files (Actors, Items, etc.): update ivars
            Self::apply_array_translations(root, filename, &lookup);
        }
    }

    fn apply_array_translations(root: &mut MarshalValue, filename: &str, lookup: &HashMap<&str, &str>) {
        let stem = strip_marshal_ext(filename);
        let fields = Self::fields_for_file(stem);

        if let MarshalValue::Array(arr) = root {
            for (idx, item) in arr.iter_mut().enumerate() {
                if matches!(item, MarshalValue::Nil) {
                    continue;
                }
                if let MarshalValue::Object { ivars, .. } = item {
                    for &(field, _) in fields {
                        let id = format!("{}#{}#{}", filename, idx, field);
                        if let Some(&translation) = lookup.get(id.as_str()) {
                            if let Some(val) = ivars.get_mut(field) {
                                if let MarshalValue::Str(s) = val {
                                    *s = translation.to_string();
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    fn apply_map_translations(root: &mut MarshalValue, filename: &str, lookup: &HashMap<&str, &str>) {
        let events = match root.get_ivar_mut("@events") {
            Some(v) => v,
            None => return,
        };

        let event_pairs = match events {
            MarshalValue::Hash(pairs) => pairs,
            _ => return,
        };

        for (ev_key, ev_val) in event_pairs.iter_mut() {
            let ev_id = match ev_key {
                MarshalValue::Int(i) => *i,
                _ => continue,
            };
            let pages = match ev_val.get_ivar_mut("@pages") {
                Some(MarshalValue::Array(a)) => a,
                _ => continue,
            };
            for (page_idx, page) in pages.iter_mut().enumerate() {
                let list = match page.get_ivar_mut("@list") {
                    Some(MarshalValue::Array(a)) => a,
                    _ => continue,
                };
                for (cmd_idx, cmd) in list.iter_mut().enumerate() {
                    let code = match cmd.get_ivar("@code") {
                        Some(MarshalValue::Int(c)) => *c,
                        _ => continue,
                    };
                    let params = match cmd.get_ivar_mut("@parameters") {
                        Some(MarshalValue::Array(a)) => a,
                        _ => continue,
                    };
                    match code {
                        401 => {
                            let id = format!("{}#0#event_{}#page_{}#cmd_{}", filename, ev_id, page_idx, cmd_idx);
                            if let Some(&translation) = lookup.get(id.as_str()) {
                                if let Some(MarshalValue::Str(s)) = params.first_mut() {
                                    *s = translation.to_string();
                                }
                            }
                        }
                        102 => {
                            if let Some(MarshalValue::Array(choices)) = params.first_mut() {
                                for (ci, choice) in choices.iter_mut().enumerate() {
                                    let id = format!("{}#0#event_{}#page_{}#cmd_{}#choice_{}", filename, ev_id, page_idx, cmd_idx, ci);
                                    if let Some(&translation) = lookup.get(id.as_str()) {
                                        if let MarshalValue::Str(s) = choice {
                                            *s = translation.to_string();
                                        }
                                    }
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
    }

    fn apply_common_event_translations(root: &mut MarshalValue, filename: &str, lookup: &HashMap<&str, &str>) {
        let arr = match root {
            MarshalValue::Array(a) => a,
            _ => return,
        };
        for (ev_idx, event) in arr.iter_mut().enumerate() {
            if matches!(event, MarshalValue::Nil) {
                continue;
            }
            let list = match event.get_ivar_mut("@list") {
                Some(MarshalValue::Array(a)) => a,
                _ => continue,
            };
            for (cmd_idx, cmd) in list.iter_mut().enumerate() {
                let code = match cmd.get_ivar("@code") {
                    Some(MarshalValue::Int(c)) => *c,
                    _ => continue,
                };
                if code == 401 {
                    let id = format!("{}#{}#cmd_{}", filename, ev_idx, cmd_idx);
                    if let Some(&translation) = lookup.get(id.as_str()) {
                        let params = match cmd.get_ivar_mut("@parameters") {
                            Some(MarshalValue::Array(a)) => a,
                            _ => continue,
                        };
                        if let Some(MarshalValue::Str(s)) = params.first_mut() {
                            *s = translation.to_string();
                        }
                    }
                }
            }
        }
    }
}

impl Default for RpgMakerVxaPlugin {
    fn default() -> Self {
        Self::new()
    }
}

fn is_marshal_ext(ext: &std::ffi::OsStr) -> bool {
    ext == "rvdata2" || ext == "rxdata"
}

fn strip_marshal_ext(filename: &str) -> &str {
    filename
        .strip_suffix(".rvdata2")
        .or_else(|| filename.strip_suffix(".rxdata"))
        .unwrap_or(filename)
}

impl FormatPlugin for RpgMakerVxaPlugin {
    fn id(&self) -> &str {
        "rpgmaker-vxa"
    }

    fn name(&self) -> &str {
        "RPG Maker VX Ace / XP"
    }

    fn description(&self) -> &str {
        "RPG Maker VX Ace (.rvdata2) and XP (.rxdata) files (Ruby Marshal)"
    }

    fn supported_extensions(&self) -> &[&str] {
        &[".rvdata2", ".rxdata"]
    }

    fn supported_modes(&self) -> Vec<OutputMode> {
        vec![OutputMode::Replace]
    }

    fn detect(&self, path: &Path) -> bool {
        if path.is_dir() {
            if let Some(data_dir) = Self::find_data_dir(path) {
                return std::fs::read_dir(&data_dir)
                    .map(|entries| {
                        entries
                            .filter_map(|e| e.ok())
                            .any(|e| {
                                e.path()
                                    .extension()
                                    .map_or(false, |ext| is_marshal_ext(ext))
                            })
                    })
                    .unwrap_or(false);
            }
            return false;
        }
        if path.is_file() {
            return path.extension().map_or(false, |ext| is_marshal_ext(ext));
        }
        false
    }

    fn extract(&self, path: &Path) -> Result<Vec<StringEntry>> {
        if path.is_file() {
            let bytes = std::fs::read(path)?;
            let root = MarshalValue::parse(&bytes)?;
            let filename = path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            let stem_lower = strip_marshal_ext(&filename).to_lowercase();

            if stem_lower.starts_with("map") {
                return Ok(Self::extract_map_file(&filename, &root, path));
            }
            if stem_lower == "commonevents" {
                return Ok(Self::extract_common_events(&filename, &root, path));
            }
            return Ok(Self::extract_array_file(&filename, &root, path));
        }

        let data_dir = Self::find_data_dir(path).ok_or_else(|| LocustError::ParseError {
            file: path.display().to_string(),
            message: "could not find Data directory".to_string(),
        })?;

        let mut all = Vec::new();
        for entry in std::fs::read_dir(&data_dir)? {
            let entry = entry?;
            let fpath = entry.path();
            if fpath.extension().map_or(false, |e| is_marshal_ext(e)) {
                let bytes = std::fs::read(&fpath)?;
                match MarshalValue::parse(&bytes) {
                    Ok(root) => {
                        let fname = fpath.file_name().unwrap_or_default().to_string_lossy().to_string();
                        let stem_lower = strip_marshal_ext(&fname).to_lowercase();
                        if stem_lower.starts_with("map") {
                            all.extend(Self::extract_map_file(&fname, &root, &fpath));
                        } else if stem_lower == "commonevents" {
                            all.extend(Self::extract_common_events(&fname, &root, &fpath));
                        } else {
                            all.extend(Self::extract_array_file(&fname, &root, &fpath));
                        }
                    }
                    Err(e) => {
                        tracing::warn!("Failed to parse {}: {}", fpath.display(), e);
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

        let mut by_file: HashMap<String, Vec<&StringEntry>> = HashMap::new();
        for entry in entries {
            let filename = entry
                .file_path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            by_file.entry(filename).or_default().push(entry);
        }

        let data_dir = if path.is_dir() {
            Self::find_data_dir(path).unwrap_or_else(|| path.to_path_buf())
        } else {
            path.parent().unwrap_or(path).to_path_buf()
        };

        for (filename, file_entries) in &by_file {
            let file_path = data_dir.join(filename);
            if !file_path.exists() {
                continue;
            }

            let bytes = std::fs::read(&file_path)?;
            let mut root = MarshalValue::parse(&bytes)?;

            for entry in file_entries {
                if entry.translation.is_some() {
                    strings_written += 1;
                } else {
                    strings_skipped += 1;
                }
            }

            // Convert Vec<&StringEntry> to Vec<StringEntry> for apply
            let owned: Vec<StringEntry> = file_entries.iter().map(|e| (*e).clone()).collect();
            Self::apply_translations(&mut root, filename, &owned);

            let new_bytes = root.serialize();
            std::fs::write(&file_path, new_bytes)?;
            files_modified += 1;
        }

        Ok(InjectionReport {
            files_modified,
            strings_written,
            strings_skipped,
            warnings: Vec::new(),
        })
    }
}

// ─── Build fixture data for tests ──────────────────────────────────────────

/// Build a minimal valid .rvdata2 with an array of 2 actor-like objects
pub fn build_test_fixture() -> Vec<u8> {
    let actors = MarshalValue::Array(vec![
        MarshalValue::Nil,
        MarshalValue::Object {
            class: "RPG::Actor".to_string(),
            ivars: {
                let mut m = HashMap::new();
                m.insert("@name".to_string(), MarshalValue::Str("TestHero".to_string()));
                m.insert(
                    "@description".to_string(),
                    MarshalValue::Str("A test hero".to_string()),
                );
                m.insert("@note".to_string(), MarshalValue::Str(String::new()));
                m.insert("@nickname".to_string(), MarshalValue::Str("Brave".to_string()));
                m.insert("@id".to_string(), MarshalValue::Int(1));
                m
            },
        },
        MarshalValue::Object {
            class: "RPG::Actor".to_string(),
            ivars: {
                let mut m = HashMap::new();
                m.insert("@name".to_string(), MarshalValue::Str("TestMage".to_string()));
                m.insert(
                    "@description".to_string(),
                    MarshalValue::Str("A test mage".to_string()),
                );
                m.insert("@note".to_string(), MarshalValue::Str(String::new()));
                m.insert("@nickname".to_string(), MarshalValue::Str("Wise".to_string()));
                m.insert("@id".to_string(), MarshalValue::Int(2));
                m
            },
        },
    ]);
    actors.serialize()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn tempdir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("locust_vxa_{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn create_vxa_fixture() -> PathBuf {
        let dir = tempdir();
        let data_dir = dir.join("Data");
        fs::create_dir_all(&data_dir).unwrap();
        let bytes = build_test_fixture();
        fs::write(data_dir.join("Actors.rvdata2"), &bytes).unwrap();
        dir
    }

    #[test]
    fn test_marshal_parse_integer() {
        // Build: header + int(42)
        let mut w = MarshalWriter::new();
        w.write_header();
        w.buf.push(b'i');
        w.write_packed_int(42);
        let val = MarshalValue::parse(&w.buf).unwrap();
        match val {
            MarshalValue::Int(v) => assert_eq!(v, 42),
            _ => panic!("expected Int, got {:?}", val),
        }
    }

    #[test]
    fn test_marshal_parse_string() {
        // Build: header + IVAR string "hello"
        let mut w = MarshalWriter::new();
        w.write_header();
        w.write_string_with_encoding("hello");
        let val = MarshalValue::parse(&w.buf).unwrap();
        assert_eq!(val.as_str(), Some("hello"));
    }

    #[test]
    fn test_marshal_parse_array() {
        let arr = MarshalValue::Array(vec![
            MarshalValue::Str("alpha".to_string()),
            MarshalValue::Str("beta".to_string()),
        ]);
        let bytes = arr.serialize();
        let parsed = MarshalValue::parse(&bytes).unwrap();
        let items = parsed.as_array().unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].as_str(), Some("alpha"));
        assert_eq!(items[1].as_str(), Some("beta"));
    }

    #[test]
    fn test_detect_vxa_directory() {
        let dir = create_vxa_fixture();
        let plugin = RpgMakerVxaPlugin::new();
        assert!(plugin.detect(&dir));
    }

    #[test]
    fn test_detect_ignores_mv() {
        let dir = tempdir();
        let data_dir = dir.join("data"); // lowercase = MV style
        fs::create_dir_all(&data_dir).unwrap();
        fs::write(data_dir.join("Actors.json"), "[]").unwrap();
        let plugin = RpgMakerVxaPlugin::new();
        assert!(!plugin.detect(&dir));
    }

    #[test]
    fn test_extract_strings_from_fixture() {
        let dir = create_vxa_fixture();
        let plugin = RpgMakerVxaPlugin::new();
        let entries = plugin.extract(&dir).unwrap();

        let hero = entries.iter().find(|e| e.id == "Actors.rvdata2#1#@name");
        assert!(hero.is_some(), "entries: {:?}", entries.iter().map(|e| &e.id).collect::<Vec<_>>());
        assert_eq!(hero.unwrap().source, "TestHero");

        let mage = entries.iter().find(|e| e.id == "Actors.rvdata2#2#@name");
        assert!(mage.is_some());
        assert_eq!(mage.unwrap().source, "TestMage");
    }

    #[test]
    fn test_inject_roundtrip() {
        let dir = create_vxa_fixture();
        let plugin = RpgMakerVxaPlugin::new();
        let mut entries = plugin.extract(&dir).unwrap();

        for entry in &mut entries {
            if entry.id == "Actors.rvdata2#1#@name" {
                entry.translation = Some("TranslatedHero".to_string());
            }
        }

        plugin.inject(&dir, &entries).unwrap();

        // Re-extract and verify
        let entries2 = plugin.extract(&dir).unwrap();
        let hero = entries2.iter().find(|e| e.id == "Actors.rvdata2#1#@name").unwrap();
        assert_eq!(hero.source, "TranslatedHero");
    }

    #[test]
    fn test_inject_preserves_binary_structure() {
        let dir = create_vxa_fixture();
        let file_path = dir.join("Data").join("Actors.rvdata2");
        let original_len = fs::metadata(&file_path).unwrap().len();

        let plugin = RpgMakerVxaPlugin::new();
        let mut entries = plugin.extract(&dir).unwrap();
        for entry in &mut entries {
            if entry.id == "Actors.rvdata2#1#@name" {
                // Same length replacement
                entry.translation = Some("TestHero".to_string());
            }
        }
        plugin.inject(&dir, &entries).unwrap();

        let new_len = fs::metadata(&file_path).unwrap().len();
        let ratio = new_len as f64 / original_len as f64;
        assert!(
            ratio > 0.9 && ratio < 1.1,
            "file size changed too much: {} -> {} (ratio: {:.2})",
            original_len,
            new_len,
            ratio
        );
    }
}
