use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use locust_core::backup::RevisionOriginal;
use locust_core::error::{LocustError, Result};
use locust_core::extraction::{FormatPlugin, InjectionReport};
use locust_core::models::{OutputMode, StringEntry};

use crate::discovery::find_capital_data_dir;

// ─── Ruby Marshal parser/writer ────────────────────────────────────────────

/// Marshal is a graph, not a JSON-like tree. Parsed definitions have stable IDs;
/// wire indices are assigned afresh on each write. Wrappers do not occupy slots.
/// The original syntax tree retains definitions removed by message splices, so a
/// surviving link can emit its target at its new first occurrence (including cycles).
#[derive(Clone, Debug)]
pub enum MarshalValue {
    Nil,
    Bool(bool),
    Int(i64),
    EncodedInt {
        value: Box<MarshalValue>,
        original: i64,
        bytes: Vec<u8>,
    },
    Str(String),
    Symbol(String),
    Array(Vec<MarshalValue>),
    Hash(Vec<(MarshalValue, MarshalValue)>),
    // Kept for callers constructing fixtures. Parsed records use OrderedObject.
    Object {
        class: String,
        ivars: HashMap<String, MarshalValue>,
    },
    UserDefined {
        class: String,
        data: Vec<u8>,
    },
    Unsupported,
    Document {
        root: Box<MarshalValue>,
        original: Arc<MarshalValue>,
    },
    Definition {
        id: usize,
        symbol: bool,
        value: Box<MarshalValue>,
    },
    Link(usize),
    SymbolLink {
        id: usize,
        name: Vec<u8>,
    },
    BinaryString(Vec<u8>),
    BinarySymbol(Vec<u8>),
    // Retain the exact spelling, including Ruby 1.8's binary mantissa suffix.
    Float(Vec<u8>),
    Bignum {
        sign: u8,
        words: Vec<u8>,
    },
    HashDefault {
        pairs: Vec<(MarshalValue, MarshalValue)>,
        default: Box<MarshalValue>,
    },
    OrderedObject {
        kind: u8,
        class: Box<MarshalValue>,
        ivars: Vec<(MarshalValue, MarshalValue)>,
    },
    Ivar {
        value: Box<MarshalValue>,
        ivars: Vec<(MarshalValue, MarshalValue)>,
    },
    Named {
        kind: u8,
        class: Box<MarshalValue>,
        value: Box<MarshalValue>,
    },
    Regexp {
        source: Vec<u8>,
        flags: u8,
    },
    ClassRef {
        kind: u8,
        name: Vec<u8>,
    },
}

fn marshal_error(message: impl Into<String>) -> LocustError {
    LocustError::ParseError {
        file: String::new(),
        message: message.into(),
    }
}

type DefinitionKey = (bool, usize);

impl MarshalValue {
    pub fn parse(bytes: &[u8]) -> Result<MarshalValue> {
        if !bytes.starts_with(&[4, 8]) {
            return Err(marshal_error("invalid Ruby Marshal header"));
        }
        let mut reader = MarshalReader::new(&bytes[2..]);
        let root = reader.read_value(false)?;
        if reader.pos != reader.data.len() {
            return Err(marshal_error("trailing Ruby Marshal data"));
        }
        if reader.objects == 0 && reader.symbols.is_empty() {
            return Ok(root);
        }
        Ok(Self::Document {
            original: Arc::new(root.clone()),
            root: Box::new(root),
        })
    }

    /// Transparent access to the payload, without dropping its wire metadata.
    pub fn value(&self) -> &Self {
        match self {
            Self::Document { root, .. } => root.value(),
            Self::Definition { value, .. }
            | Self::Ivar { value, .. }
            | Self::EncodedInt { value, .. } => value.value(),
            Self::Named {
                kind: b'C' | b'e',
                value,
                ..
            } => value.value(),
            _ => self,
        }
    }

    pub fn value_mut(&mut self) -> &mut Self {
        match self {
            Self::Document { root, .. } => root.value_mut(),
            Self::Definition { value, .. }
            | Self::Ivar { value, .. }
            | Self::EncodedInt { value, .. } => value.value_mut(),
            Self::Named {
                kind: b'C' | b'e',
                value,
                ..
            } => value.value_mut(),
            _ => self,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self.value() {
            Self::Str(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_array(&self) -> Option<&[MarshalValue]> {
        match self.value() {
            Self::Array(a) => Some(a),
            _ => None,
        }
    }

    fn symbol_bytes(&self) -> Option<&[u8]> {
        match self.value() {
            Self::Symbol(s) => Some(s.as_bytes()),
            Self::BinarySymbol(s) | Self::SymbolLink { name: s, .. } => Some(s),
            _ => None,
        }
    }

    pub fn get_ivar(&self, name: &str) -> Option<&MarshalValue> {
        let value = match self.value() {
            Self::Object { ivars, .. } => ivars.get(name),
            Self::OrderedObject { ivars, .. } => ivars
                .iter()
                .find(|(k, _)| k.symbol_bytes() == Some(name.as_bytes()))
                .map(|(_, v)| v),
            _ => None,
        };
        value.map(Self::value)
    }

    pub fn get_ivar_mut(&mut self, name: &str) -> Option<&mut MarshalValue> {
        let value = match self.value_mut() {
            Self::Object { ivars, .. } => ivars.get_mut(name),
            Self::OrderedObject { ivars, .. } => ivars
                .iter_mut()
                .find(|(k, _)| k.symbol_bytes() == Some(name.as_bytes()))
                .map(|(_, v)| v),
            _ => None,
        };
        value.map(Self::value_mut)
    }

    /// Fallible serialization is used by injection: no unsupported value can
    /// become nil, and an unresolved graph edge prevents the entire file write.
    pub fn try_serialize(&self) -> Result<Vec<u8>> {
        let mut writer = MarshalWriter::new();
        if let Self::Document { original, .. } = self {
            writer.index(original);
        }
        writer.index(self);
        writer.write_header();
        writer.write_value(self, None, false)?;
        Ok(writer.buf)
    }

    pub fn serialize(&self) -> Vec<u8> {
        self.try_serialize()
            .expect("unrepresentable Ruby Marshal graph")
    }

    fn children<'v>(&'v self, visit: &mut impl FnMut(&'v Self)) {
        match self {
            Self::Document { root, .. } => visit(root),
            Self::Definition { value, .. } | Self::EncodedInt { value, .. } => visit(value),
            Self::Array(a) => a.iter().for_each(visit),
            Self::Hash(pairs) => {
                for (k, v) in pairs {
                    visit(k);
                    visit(v);
                }
            }
            Self::HashDefault { pairs, default } => {
                for (k, v) in pairs {
                    visit(k);
                    visit(v);
                }
                visit(default);
            }
            Self::Object { ivars, .. } => ivars.values().for_each(visit),
            Self::OrderedObject { class, ivars, .. } => {
                visit(class);
                for (k, v) in ivars {
                    visit(k);
                    visit(v);
                }
            }
            Self::Ivar { value, ivars } => {
                visit(value);
                for (k, v) in ivars {
                    visit(k);
                    visit(v);
                }
            }
            Self::Named { class, value, .. } => {
                visit(class);
                visit(value);
            }
            _ => {}
        }
    }

    fn children_mut(&mut self, visit: &mut impl FnMut(&mut Self)) {
        match self {
            Self::Document { root, .. } => visit(root),
            Self::Definition { value, .. } | Self::EncodedInt { value, .. } => visit(value),
            Self::Array(a) => a.iter_mut().for_each(visit),
            Self::Hash(pairs) => {
                for (k, v) in pairs {
                    visit(k);
                    visit(v);
                }
            }
            Self::HashDefault { pairs, default } => {
                for (k, v) in pairs {
                    visit(k);
                    visit(v);
                }
                visit(default);
            }
            Self::Object { ivars, .. } => ivars.values_mut().for_each(visit),
            Self::OrderedObject { class, ivars, .. } => {
                visit(class);
                for (k, v) in ivars {
                    visit(k);
                    visit(v);
                }
            }
            Self::Ivar { value, ivars } => {
                visit(value);
                for (k, v) in ivars {
                    visit(k);
                    visit(v);
                }
            }
            Self::Named { class, value, .. } => {
                visit(class);
                visit(value);
            }
            _ => {}
        }
    }

    fn next_object_id(&self) -> usize {
        let mut next = match self {
            Self::Definition {
                id, symbol: false, ..
            } => id + 1,
            _ => 0,
        };
        self.children(&mut |child| next = next.max(child.next_object_id()));
        next
    }

    /// New 401 commands are distinct objects. Remap their internal links too;
    /// symbol identities continue to belong to the document's symbol table.
    fn fresh_clone(&self, next: &mut usize) -> Self {
        fn allocate(v: &MarshalValue, next: &mut usize, ids: &mut HashMap<usize, usize>) {
            if let MarshalValue::Definition {
                id, symbol: false, ..
            } = v
            {
                ids.entry(*id).or_insert_with(|| {
                    let id = *next;
                    *next += 1;
                    id
                });
            }
            v.children(&mut |child| allocate(child, next, ids));
        }
        fn remap(v: &mut MarshalValue, ids: &HashMap<usize, usize>) {
            match v {
                MarshalValue::Definition {
                    id, symbol: false, ..
                }
                | MarshalValue::Link(id) => {
                    if let Some(new) = ids.get(id) {
                        *id = *new;
                    }
                }
                _ => {}
            }
            v.children_mut(&mut |child| remap(child, ids));
        }
        let mut ids = HashMap::new();
        allocate(self, next, &mut ids);
        let mut copy = self.clone();
        remap(&mut copy, &ids);
        copy
    }
}

struct MarshalReader<'a> {
    data: &'a [u8],
    pos: usize,
    symbols: Vec<Vec<u8>>,
    objects: usize,
    depth: usize,
}

impl<'a> MarshalReader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self {
            data,
            pos: 0,
            symbols: Vec::new(),
            objects: 0,
            depth: 0,
        }
    }

    fn read_byte(&mut self) -> Result<u8> {
        let byte = *self
            .data
            .get(self.pos)
            .ok_or_else(|| marshal_error("unexpected end of marshal data"))?;
        self.pos += 1;
        Ok(byte)
    }

    fn read_bytes(&mut self, n: usize) -> Result<&'a [u8]> {
        if n > self.data.len() - self.pos {
            return Err(marshal_error("unexpected end of marshal data"));
        }
        let slice = &self.data[self.pos..self.pos + n];
        self.pos += n;
        Ok(slice)
    }

    fn read_packed_int(&mut self) -> Result<i64> {
        let c = self.read_byte()? as i8;
        match c {
            0 => Ok(0),
            1..=4 | -4..=-1 => {
                let mut val = if c < 0 { -1i64 } else { 0i64 };
                for i in 0..c.unsigned_abs() {
                    val = (val & !(0xffi64 << (8 * i))) | ((self.read_byte()? as i64) << (8 * i));
                }
                Ok(val)
            }
            5..=127 => Ok(c as i64 - 5),
            _ => Ok(c as i64 + 5),
        }
    }

    fn count(&mut self, minimum_bytes: usize) -> Result<usize> {
        let n = usize::try_from(self.read_packed_int()?)
            .map_err(|_| marshal_error("negative Marshal length"))?;
        if n > (self.data.len() - self.pos) / minimum_bytes {
            return Err(marshal_error("Marshal length exceeds remaining input"));
        }
        Ok(n)
    }

    fn blob(&mut self) -> Result<Vec<u8>> {
        let count = self.count(1)?;
        Ok(self.read_bytes(count)?.to_vec())
    }

    fn register(&mut self) -> usize {
        let id = self.objects;
        self.objects += 1;
        id
    }

    fn definition(id: usize, symbol: bool, value: MarshalValue) -> MarshalValue {
        MarshalValue::Definition {
            id,
            symbol,
            value: Box::new(value),
        }
    }

    // Put wrappers INSIDE the definition, so a moved first occurrence keeps them.
    fn wrap(
        value: MarshalValue,
        wrapper: impl FnOnce(MarshalValue) -> MarshalValue,
    ) -> MarshalValue {
        match value {
            MarshalValue::Definition { id, symbol, value } => {
                Self::definition(id, symbol, wrapper(*value))
            }
            value => wrapper(value),
        }
    }

    fn symbol(&mut self) -> Result<MarshalValue> {
        let value = self.read_value(false)?;
        if value.symbol_bytes().is_none() {
            return Err(marshal_error("expected Marshal symbol"));
        }
        Ok(value)
    }

    fn ivars(&mut self) -> Result<Vec<(MarshalValue, MarshalValue)>> {
        let count = self.count(2)?;
        let mut fields = Vec::with_capacity(count);
        for _ in 0..count {
            fields.push((self.symbol()?, self.read_value(false)?));
        }
        Ok(fields)
    }

    fn read_value(&mut self, in_ivar: bool) -> Result<MarshalValue> {
        if self.depth >= 128 {
            return Err(marshal_error("Marshal nesting limit exceeded"));
        }
        self.depth += 1;
        let result = self.read_value_inner(in_ivar);
        self.depth -= 1;
        result
    }

    fn read_value_inner(&mut self, in_ivar: bool) -> Result<MarshalValue> {
        use MarshalValue as V;
        let tag = self.read_byte()?;
        let value = match tag {
            b'0' => V::Nil,
            b'T' => V::Bool(true),
            b'F' => V::Bool(false),
            b'i' => {
                let start = self.pos;
                let original = self.read_packed_int()?;
                let bytes = &self.data[start..self.pos];
                let mut canonical = MarshalWriter::new();
                canonical.write_packed_int(original);
                if canonical.buf == bytes {
                    V::Int(original)
                } else {
                    V::EncodedInt {
                        value: Box::new(V::Int(original)),
                        original,
                        bytes: bytes.to_vec(),
                    }
                }
            }
            b':' => {
                let bytes = self.blob()?;
                let id = self.symbols.len();
                self.symbols.push(bytes.clone());
                let symbol = match String::from_utf8(bytes) {
                    Ok(s) => V::Symbol(s),
                    Err(e) => V::BinarySymbol(e.into_bytes()),
                };
                Self::definition(id, true, symbol)
            }
            b';' => {
                let id = usize::try_from(self.read_packed_int()?)
                    .map_err(|_| marshal_error("negative symbol link"))?;
                let name = self
                    .symbols
                    .get(id)
                    .ok_or_else(|| marshal_error(format!("invalid symbol link: {id}")))?
                    .clone();
                V::SymbolLink { id, name }
            }
            b'@' => {
                let id = usize::try_from(self.read_packed_int()?)
                    .map_err(|_| marshal_error("negative object link"))?;
                if id >= self.objects {
                    return Err(marshal_error(format!("invalid object link: {id}")));
                }
                V::Link(id)
            }
            b'I' => {
                let value = self.read_value(true)?;
                let late = matches!(value, V::Named { kind: b'u', .. });
                let ivars = self.ivars()?;
                let wrapped = Self::wrap(value, |value| V::Ivar {
                    value: Box::new(value),
                    ivars,
                });
                if late {
                    Self::definition(self.register(), false, wrapped)
                } else {
                    wrapped
                }
            }
            b'C' | b'e' => {
                let class = Box::new(self.symbol()?);
                let value = self.read_value(false)?;
                Self::wrap(value, |value| V::Named {
                    kind: tag,
                    class,
                    value: Box::new(value),
                })
            }
            b'o' | b'S' => {
                let id = self.register();
                let class = Box::new(self.symbol()?);
                let ivars = self.ivars()?;
                Self::definition(
                    id,
                    false,
                    V::OrderedObject {
                        kind: tag,
                        class,
                        ivars,
                    },
                )
            }
            b'U' | b'd' | b'u' => {
                let class = Box::new(self.symbol()?);
                let id = if tag != b'u' {
                    Some(self.register())
                } else {
                    None
                };
                let value = Box::new(if tag == b'u' {
                    V::BinaryString(self.blob()?)
                } else {
                    self.read_value(false)?
                });
                let value = V::Named {
                    kind: tag,
                    class,
                    value,
                };
                if tag == b'u' && in_ivar {
                    value
                } else {
                    Self::definition(id.unwrap_or_else(|| self.register()), false, value)
                }
            }
            b'[' => {
                let count = self.count(1)?;
                let id = self.register();
                let mut values = Vec::with_capacity(count);
                for _ in 0..count {
                    values.push(self.read_value(false)?);
                }
                Self::definition(id, false, V::Array(values))
            }
            b'{' | b'}' => {
                let count = self.count(2)?;
                let id = self.register();
                let mut pairs = Vec::with_capacity(count);
                for _ in 0..count {
                    pairs.push((self.read_value(false)?, self.read_value(false)?));
                }
                let value = if tag == b'}' {
                    V::HashDefault {
                        pairs,
                        default: Box::new(self.read_value(false)?),
                    }
                } else {
                    V::Hash(pairs)
                };
                Self::definition(id, false, value)
            }
            b'"' | b'f' | b'/' | b'c' | b'm' | b'M' => {
                let data = self.blob()?;
                let value = match tag {
                    b'"' => match String::from_utf8(data) {
                        Ok(s) => V::Str(s),
                        Err(e) => V::BinaryString(e.into_bytes()),
                    },
                    b'f' => V::Float(data),
                    b'/' => V::Regexp {
                        source: data,
                        flags: self.read_byte()?,
                    },
                    _ => V::ClassRef {
                        kind: tag,
                        name: data,
                    },
                };
                Self::definition(self.register(), false, value)
            }
            b'l' => {
                let sign = self.read_byte()?;
                if sign != b'+' && sign != b'-' {
                    return Err(marshal_error("invalid Bignum sign"));
                }
                let count = self.count(2)?;
                let words = self.read_bytes(count * 2)?.to_vec();
                Self::definition(self.register(), false, V::Bignum { sign, words })
            }
            _ => {
                return Err(marshal_error(format!(
                    "unsupported Marshal tag 0x{tag:02x} at byte {}",
                    self.pos + 1
                )))
            }
        };
        Ok(value)
    }
}

struct MarshalWriter<'a> {
    buf: Vec<u8>,
    definitions: HashMap<DefinitionKey, &'a MarshalValue>,
    emitted: HashMap<DefinitionKey, usize>,
    symbols: HashMap<Vec<u8>, usize>,
    symbol_count: usize,
    object_count: usize,
    depth: usize,
}

impl<'a> MarshalWriter<'a> {
    fn new() -> Self {
        Self {
            buf: Vec::new(),
            definitions: HashMap::new(),
            emitted: HashMap::new(),
            symbols: HashMap::new(),
            symbol_count: 0,
            object_count: 0,
            depth: 0,
        }
    }

    fn index(&mut self, value: &'a MarshalValue) {
        if let MarshalValue::Definition { id, symbol, .. } = value {
            self.definitions.insert((*symbol, *id), value);
        }
        value.children(&mut |child| self.index(child));
    }

    fn write_header(&mut self) {
        self.buf.extend_from_slice(&[4, 8]);
    }

    fn write_packed_int(&mut self, mut val: i64) {
        if val == 0 {
            self.buf.push(0);
            return;
        }
        if (1..123).contains(&val) {
            self.buf.push((val + 5) as u8);
            return;
        }
        if (-123..0).contains(&val) {
            self.buf.push((val - 5) as u8);
            return;
        }
        let negative = val < 0;
        let pos = self.buf.len();
        self.buf.push(0);
        loop {
            self.buf.push(val as u8);
            val >>= 8;
            if val == 0 || val == -1 {
                break;
            }
        }
        let n = (self.buf.len() - pos - 1) as i8;
        self.buf[pos] = if negative { -n } else { n } as u8;
    }

    fn blob(&mut self, bytes: &[u8]) {
        self.write_packed_int(bytes.len() as i64);
        self.buf.extend_from_slice(bytes);
    }

    fn register(&mut self, key: Option<DefinitionKey>, symbol: bool) {
        let count = if symbol {
            &mut self.symbol_count
        } else {
            &mut self.object_count
        };
        if let Some(key) = key {
            self.emitted.insert(key, *count);
        }
        *count += 1;
    }

    fn link(&mut self, key: DefinitionKey) -> Result<()> {
        if let Some(&index) = self.emitted.get(&key) {
            self.buf.push(if key.0 { b';' } else { b'@' });
            self.write_packed_int(index as i64);
            Ok(())
        } else {
            let value = *self
                .definitions
                .get(&key)
                .ok_or_else(|| marshal_error(format!("unresolved Marshal link: {key:?}")))?;
            self.write_value(value, None, false)
        }
    }

    fn write_symbol(&mut self, name: &[u8], key: Option<DefinitionKey>) {
        if key.is_none() {
            if let Some(&id) = self.symbols.get(name) {
                self.buf.push(b';');
                self.write_packed_int(id as i64);
                return;
            }
        }
        self.symbols
            .entry(name.to_vec())
            .or_insert(self.symbol_count);
        self.register(key, true);
        self.buf.push(b':');
        self.blob(name);
    }

    #[cfg(test)]
    fn write_string_with_encoding(&mut self, s: &str) {
        self.buf.extend_from_slice(b"I\"");
        self.blob(s.as_bytes());
        self.register(None, false);
        self.write_packed_int(1);
        self.write_symbol(b"E", None);
        self.buf.push(b'T');
    }

    fn ivars(&mut self, ivars: &'a [(MarshalValue, MarshalValue)]) -> Result<()> {
        self.write_packed_int(ivars.len() as i64);
        for (k, v) in ivars {
            self.write_value(k, None, false)?;
            self.write_value(v, None, false)?;
        }
        Ok(())
    }

    fn write_value(
        &mut self,
        value: &'a MarshalValue,
        key: Option<DefinitionKey>,
        in_ivar: bool,
    ) -> Result<()> {
        if self.depth >= 512 {
            return Err(marshal_error("Marshal write nesting limit exceeded"));
        }
        self.depth += 1;
        let result = self.write_inner(value, key, in_ivar);
        self.depth -= 1;
        result
    }

    fn write_inner(
        &mut self,
        value: &'a MarshalValue,
        key: Option<DefinitionKey>,
        in_ivar: bool,
    ) -> Result<()> {
        use MarshalValue as V;
        match value {
            V::Document { root, .. } => self.write_value(root, key, in_ivar)?,
            V::Definition { id, symbol, value } => {
                let key = (*symbol, *id);
                if self.emitted.contains_key(&key) {
                    self.link(key)?;
                } else {
                    self.write_value(value, Some(key), in_ivar)?;
                }
            }
            V::Link(id) => self.link((false, *id))?,
            V::SymbolLink { id, .. } => self.link((true, *id))?,
            V::Nil => self.buf.push(b'0'),
            V::Bool(b) => self.buf.push(if *b { b'T' } else { b'F' }),
            V::Int(n) => {
                if !(-0x1_0000_0000..=0xffff_ffff).contains(n) {
                    return Err(marshal_error("integer outside Marshal fixnum wire range"));
                }
                self.buf.push(b'i');
                self.write_packed_int(*n);
            }
            V::EncodedInt {
                value,
                original,
                bytes,
            } => {
                if matches!(value.as_ref(), V::Int(n) if n == original) {
                    self.buf.push(b'i');
                    self.buf.extend_from_slice(bytes);
                } else {
                    self.write_value(value, key, in_ivar)?;
                }
            }
            V::Str(s) => {
                self.buf.push(b'"');
                self.blob(s.as_bytes());
                self.register(key, false);
            }
            V::BinaryString(s) => {
                self.buf.push(b'"');
                self.blob(s);
                self.register(key, false);
            }
            V::Symbol(s) => self.write_symbol(s.as_bytes(), key),
            V::BinarySymbol(s) => self.write_symbol(s, key),
            V::Float(bytes) => {
                self.buf.push(b'f');
                self.blob(bytes);
                self.register(key, false);
            }
            V::Bignum { sign, words } => {
                if !matches!(sign, b'+' | b'-') || words.len() % 2 != 0 {
                    return Err(marshal_error("invalid Bignum"));
                }
                self.buf.extend_from_slice(&[b'l', *sign]);
                self.write_packed_int((words.len() / 2) as i64);
                self.buf.extend_from_slice(words);
                self.register(key, false);
            }
            V::Regexp { source, flags } => {
                self.buf.push(b'/');
                self.blob(source);
                self.buf.push(*flags);
                self.register(key, false);
            }
            V::ClassRef { kind, name } => {
                if !matches!(kind, b'c' | b'm' | b'M') {
                    return Err(marshal_error("invalid class/module tag"));
                }
                self.buf.push(*kind);
                self.blob(name);
                self.register(key, false);
            }
            V::Array(values) => {
                self.buf.push(b'[');
                self.write_packed_int(values.len() as i64);
                self.register(key, false);
                for v in values {
                    self.write_value(v, None, false)?;
                }
            }
            V::Hash(pairs) | V::HashDefault { pairs, .. } => {
                self.buf.push(if matches!(value, V::HashDefault { .. }) {
                    b'}'
                } else {
                    b'{'
                });
                self.write_packed_int(pairs.len() as i64);
                self.register(key, false);
                for (k, v) in pairs {
                    self.write_value(k, None, false)?;
                    self.write_value(v, None, false)?;
                }
                if let V::HashDefault { default, .. } = value {
                    self.write_value(default, None, false)?;
                }
            }
            V::Object { class, ivars } => {
                self.buf.push(b'o');
                self.register(key, false);
                self.write_symbol(class.as_bytes(), None);
                self.write_packed_int(ivars.len() as i64);
                let mut fields: Vec<_> = ivars.iter().collect();
                fields.sort_by_key(|(k, _)| *k);
                for (k, v) in fields {
                    self.write_symbol(k.as_bytes(), None);
                    self.write_value(v, None, false)?;
                }
            }
            V::OrderedObject { kind, class, ivars } => {
                if !matches!(kind, b'o' | b'S') {
                    return Err(marshal_error("invalid record tag"));
                }
                self.buf.push(*kind);
                self.register(key, false);
                self.write_value(class, None, false)?;
                self.ivars(ivars)?;
            }
            V::Ivar { value, ivars } => {
                self.buf.push(b'I');
                self.write_value(value, key, true)?;
                self.ivars(ivars)?;
                if matches!(value.as_ref(), V::Named { kind: b'u', .. }) {
                    self.register(key, false);
                }
            }
            V::Named { kind, class, value } => {
                if !matches!(kind, b'C' | b'e' | b'U' | b'd' | b'u') {
                    return Err(marshal_error("invalid named tag"));
                }
                self.buf.push(*kind);
                self.write_value(class, None, false)?;
                match kind {
                    b'C' | b'e' => self.write_value(value, key, false)?,
                    b'U' | b'd' => {
                        self.register(key, false);
                        self.write_value(value, None, false)?;
                    }
                    b'u' => {
                        let V::BinaryString(data) = value.as_ref() else {
                            return Err(marshal_error("invalid user-defined payload"));
                        };
                        self.blob(data);
                        if !in_ivar {
                            self.register(key, false);
                        }
                    }
                    _ => unreachable!(),
                }
            }
            V::UserDefined { class, data } => {
                self.buf.push(b'u');
                self.write_symbol(class.as_bytes(), None);
                self.blob(data);
                self.register(key, false);
            }
            V::Unsupported => return Err(marshal_error("unsupported Marshal value")),
        }
        Ok(())
    }
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

    fn fields_for_file(stem: &str) -> &'static [(&'static str, &'static str)] {
        let lower = stem.to_lowercase();
        match lower.as_str() {
            "actors" | "classes" | "enemies" | "states" => ACTOR_FIELDS,
            "skills" => SKILL_FIELDS,
            "items" | "weapons" | "armors" => ITEM_FIELDS,
            _ => &[],
        }
    }

    /// Extract from the exact tree injection will mutate, before any message
    /// splices change command indices or wrapping widths.
    fn extract_root(filename: &str, root: &MarshalValue, path: &Path) -> Vec<StringEntry> {
        let stem = strip_marshal_ext(filename).to_lowercase();
        if stem.starts_with("map") {
            Self::extract_map_file(filename, root, path)
        } else if stem == "commonevents" {
            Self::extract_common_events(filename, root, path)
        } else {
            Self::extract_array_file(filename, root, path)
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
                                let mut entry = StringEntry::new(id, s, file_path.to_path_buf());
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

    fn extract_map_file(filename: &str, root: &MarshalValue, file_path: &Path) -> Vec<StringEntry> {
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
                let mut skip_until = 0usize;
                for (cmd_idx, cmd) in list.iter().enumerate() {
                    if cmd_idx < skip_until {
                        continue;
                    }
                    let code = match cmd.get_ivar("@code") {
                        Some(MarshalValue::Int(c)) => *c,
                        _ => continue,
                    };
                    let params = match cmd.get_ivar("@parameters") {
                        Some(MarshalValue::Array(a)) => a,
                        _ => continue,
                    };
                    match code {
                        // Message text. XP carries the FIRST line inside the
                        // Show Text command (101); VX Ace keeps 101 as a
                        // face/position header and puts all lines in 401s.
                        // Either way the whole box merges into one #msg entry.
                        101 | 401 => {
                            let mut lines: Vec<String> = Vec::new();
                            match params.first().and_then(MarshalValue::as_str) {
                                Some(text) if code == 401 || !text.trim().is_empty() => {
                                    lines.push(text.to_string());
                                }
                                // VX Ace header, or XP Show Text with an empty
                                // first line: the 401 run that follows anchors
                                // its own block instead.
                                _ => continue,
                            }
                            let mut end = cmd_idx + 1;
                            while let Some(next) = list.get(end) {
                                if cmd_code(next) != Some(401) {
                                    break;
                                }
                                lines.push(cmd_first_str(next).unwrap_or_default());
                                end += 1;
                            }
                            skip_until = end;

                            let text = lines.join("\n");
                            if !text.trim().is_empty() {
                                let id = format!(
                                    "{}#0#event_{}#page_{}#cmd_{}#msg",
                                    filename, ev_id, page_idx, cmd_idx
                                );
                                let mut entry =
                                    StringEntry::new(id, &text, file_path.to_path_buf());
                                entry.tags = vec!["dialogue".to_string()];
                                entries.push(entry);
                            }
                        }
                        102 => {
                            if let Some(MarshalValue::Array(choices)) =
                                params.first().map(MarshalValue::value)
                            {
                                for (ci, choice) in choices.iter().enumerate() {
                                    if let Some(text) = choice.as_str() {
                                        if !text.trim().is_empty() {
                                            let id = format!(
                                                "{}#0#event_{}#page_{}#cmd_{}#choice_{}",
                                                filename, ev_id, page_idx, cmd_idx, ci
                                            );
                                            let mut entry =
                                                StringEntry::new(id, text, file_path.to_path_buf());
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
            let mut skip_until = 0usize;
            for (cmd_idx, cmd) in list.iter().enumerate() {
                if cmd_idx < skip_until {
                    continue;
                }
                let code = match cmd.get_ivar("@code") {
                    Some(MarshalValue::Int(c)) => *c,
                    _ => continue,
                };
                let params = match cmd.get_ivar("@parameters") {
                    Some(MarshalValue::Array(a)) => a,
                    _ => continue,
                };
                if code == 101 || code == 401 {
                    let mut lines: Vec<String> = Vec::new();
                    match params.first().and_then(MarshalValue::as_str) {
                        Some(text) if code == 401 || !text.trim().is_empty() => {
                            lines.push(text.to_string());
                        }
                        _ => continue,
                    }
                    let mut end = cmd_idx + 1;
                    while let Some(next) = list.get(end) {
                        if cmd_code(next) != Some(401) {
                            break;
                        }
                        lines.push(cmd_first_str(next).unwrap_or_default());
                        end += 1;
                    }
                    skip_until = end;

                    let text = lines.join("\n");
                    if !text.trim().is_empty() {
                        let id = format!("{}#{}#cmd_{}#msg", filename, ev_idx, cmd_idx);
                        let mut entry = StringEntry::new(id, &text, file_path.to_path_buf());
                        entry.tags = vec!["dialogue".to_string()];
                        entries.push(entry);
                    }
                }
            }
        }
        entries
    }

    fn apply_translations(root: &mut MarshalValue, filename: &str, entries: &[StringEntry]) {
        // Providers flatten hand-wrapped fields (item/actor `description`), so
        // restore the source line width before anything writes them back.
        let rewrapped: Vec<(&str, String)> = entries
            .iter()
            .filter_map(|e| {
                e.translation.as_deref().map(|t| {
                    (
                        e.id.as_str(),
                        crate::rpgmaker_mv::rewrap_to_source_width(&e.source, t),
                    )
                })
            })
            .collect();
        let lookup: HashMap<&str, &str> =
            rewrapped.iter().map(|(id, t)| (*id, t.as_str())).collect();

        if lookup.is_empty() {
            return;
        }

        let stem_lower = strip_marshal_ext(filename).to_lowercase();
        let mut next_object_id = root.next_object_id();

        if stem_lower.starts_with("map") && stem_lower != "mapinfos" {
            // Map files: navigate @events → @pages → @list → commands
            Self::apply_map_translations(root, filename, &lookup, &mut next_object_id);
        } else if stem_lower == "commonevents" {
            // CommonEvents: navigate array → @list → commands
            Self::apply_common_event_translations(root, filename, &lookup, &mut next_object_id);
        } else {
            // Array data files (Actors, Items, etc.): update ivars
            Self::apply_array_translations(root, filename, &lookup);
        }
    }

    fn apply_array_translations(
        root: &mut MarshalValue,
        filename: &str,
        lookup: &HashMap<&str, &str>,
    ) {
        let stem = strip_marshal_ext(filename);
        let fields = Self::fields_for_file(stem);

        if let MarshalValue::Array(arr) = root.value_mut() {
            for (idx, item) in arr.iter_mut().enumerate() {
                if matches!(item, MarshalValue::Nil) {
                    continue;
                }
                for &(field, _) in fields {
                    let id = format!("{}#{}#{}", filename, idx, field);
                    if let Some(&translation) = lookup.get(id.as_str()) {
                        if let Some(MarshalValue::Str(s)) = item.get_ivar_mut(field) {
                            *s = translation.to_string();
                        }
                    }
                }
            }
        }
    }

    fn apply_map_translations(
        root: &mut MarshalValue,
        filename: &str,
        lookup: &HashMap<&str, &str>,
        next_object_id: &mut usize,
    ) {
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
                let prefix = format!("{}#0#event_{}#page_{}", filename, ev_id, page_idx);
                Self::apply_list_translations(list, lookup, &prefix, next_object_id);
            }
        }
    }

    fn apply_common_event_translations(
        root: &mut MarshalValue,
        filename: &str,
        lookup: &HashMap<&str, &str>,
        next_object_id: &mut usize,
    ) {
        let arr = match root.value_mut() {
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
            let prefix = format!("{}#{}", filename, ev_idx);
            Self::apply_list_translations(list, lookup, &prefix, next_object_id);
        }
    }

    /// Apply every translation belonging to one event command list. Message
    /// blocks (#msg) splice the command run and may change its length, so
    /// operations run in descending command order to keep indices valid.
    fn apply_list_translations(
        list: &mut Vec<MarshalValue>,
        lookup: &HashMap<&str, &str>,
        prefix: &str,
        next_object_id: &mut usize,
    ) {
        enum Op<'a> {
            Msg(&'a str),
            Line(&'a str),
            Choice(usize, &'a str),
        }

        let mut ops: Vec<(usize, Op)> = Vec::new();
        for (id, translation) in lookup {
            let Some(suffix) = id.strip_prefix(prefix) else {
                continue;
            };
            let Some(rest) = suffix.strip_prefix("#cmd_") else {
                continue;
            };
            let mut parts = rest.splitn(2, '#');
            let Some(idx) = parts.next().and_then(|s| s.parse::<usize>().ok()) else {
                continue;
            };
            match parts.next() {
                None => ops.push((idx, Op::Line(translation))),
                Some("msg") => ops.push((idx, Op::Msg(translation))),
                Some(c) => {
                    if let Some(ci) = c.strip_prefix("choice_").and_then(|s| s.parse().ok()) {
                        ops.push((idx, Op::Choice(ci, translation)));
                    }
                }
            }
        }
        ops.sort_by_key(|(idx, _)| std::cmp::Reverse(*idx));

        for (idx, op) in ops {
            match op {
                Op::Msg(t) => Self::apply_message_block(list, idx, t, next_object_id),
                Op::Line(t) => {
                    if let Some(cmd) = list.get_mut(idx) {
                        set_cmd_text(cmd, t);
                    }
                }
                Op::Choice(ci, t) => {
                    if let Some(cmd) = list.get_mut(idx) {
                        if let Some(MarshalValue::Array(params)) = cmd.get_ivar_mut("@parameters") {
                            if let Some(MarshalValue::Array(choices)) =
                                params.first_mut().map(MarshalValue::value_mut)
                            {
                                if let Some(MarshalValue::Str(s)) =
                                    choices.get_mut(ci).map(MarshalValue::value_mut)
                                {
                                    *s = t.to_string();
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    /// Replace a message block anchored at `cmd_idx` (XP: a 101 carrying the
    /// first line; VX Ace: the first 401 of a run) with the translation
    /// re-wrapped to the original line width. Continuation 401s are spliced,
    /// so the run may grow or shrink.
    fn apply_message_block(
        list: &mut Vec<MarshalValue>,
        cmd_idx: usize,
        translation: &str,
        next_object_id: &mut usize,
    ) {
        let anchor_code = match list.get(cmd_idx).and_then(cmd_code) {
            Some(c @ (101 | 401)) => c,
            _ => return,
        };
        if anchor_code == 101 && cmd_first_str(&list[cmd_idx]).is_none() {
            // VX Ace header without text — blocks are anchored at 401s there
            return;
        }

        let mut max_width = cmd_first_str(&list[cmd_idx])
            .map(|t| crate::rpgmaker_mv::visible_len(&t))
            .unwrap_or(0);
        let mut end = cmd_idx + 1;
        while let Some(next) = list.get(end) {
            if cmd_code(next) != Some(401) {
                break;
            }
            if let Some(t) = cmd_first_str(next) {
                max_width = max_width.max(crate::rpgmaker_mv::visible_len(&t));
            }
            end += 1;
        }

        let width = max_width.max(40);
        let flat = translation.split_whitespace().collect::<Vec<_>>().join(" ");
        let mut lines = crate::rpgmaker_mv::wrap_message(&flat, width).into_iter();

        // First line goes into the anchor command itself
        let first = lines.next().unwrap_or_default();
        set_cmd_text(&mut list[cmd_idx], &first);

        // Continuation lines become 401s modeled on an existing one (or the anchor)
        let template = if end > cmd_idx + 1 {
            list[cmd_idx + 1].clone()
        } else {
            make_401_like(&list[cmd_idx])
        };
        let new_401s: Vec<MarshalValue> = lines
            .enumerate()
            .map(|(index, line)| {
                let mut cmd = if cmd_idx + 1 + index < end {
                    list[cmd_idx + 1 + index].clone()
                } else {
                    template.fresh_clone(next_object_id)
                };
                set_cmd_text(&mut cmd, &line);
                cmd
            })
            .collect();
        list.splice(cmd_idx + 1..end, new_401s);
    }
}

fn cmd_code(cmd: &MarshalValue) -> Option<i64> {
    match cmd.get_ivar("@code") {
        Some(MarshalValue::Int(c)) => Some(*c),
        _ => None,
    }
}

fn cmd_first_str(cmd: &MarshalValue) -> Option<String> {
    match cmd.get_ivar("@parameters") {
        Some(MarshalValue::Array(a)) => {
            a.first().and_then(MarshalValue::as_str).map(str::to_string)
        }
        _ => None,
    }
}

fn set_cmd_text(cmd: &mut MarshalValue, text: &str) {
    if let Some(MarshalValue::Array(params)) = cmd.get_ivar_mut("@parameters") {
        if let Some(first) = params.first_mut() {
            *first.value_mut() = MarshalValue::Str(text.to_string());
        } else {
            params.push(MarshalValue::Str(text.to_string()));
        }
    }
}

/// Clone `anchor` into a continuation-line command (@code 401, single text param).
fn make_401_like(anchor: &MarshalValue) -> MarshalValue {
    let mut cmd = anchor.clone();
    if let Some(code) = cmd.get_ivar_mut("@code") {
        *code = MarshalValue::Int(401);
    }
    if let Some(MarshalValue::Array(params)) = cmd.get_ivar_mut("@parameters") {
        params.truncate(1);
    }
    set_cmd_text(&mut cmd, "");
    cmd
}

impl Default for RpgMakerVxaPlugin {
    fn default() -> Self {
        Self::new()
    }
}

fn is_marshal_ext(ext: &std::ffi::OsStr) -> bool {
    ext == "rvdata2" || ext == "rvdata" || ext == "rxdata"
}

fn strip_marshal_ext(filename: &str) -> &str {
    filename
        .strip_suffix(".rvdata2")
        .or_else(|| filename.strip_suffix(".rvdata"))
        .or_else(|| filename.strip_suffix(".rxdata"))
        .unwrap_or(filename)
}

impl FormatPlugin for RpgMakerVxaPlugin {
    fn id(&self) -> &str {
        "rpgmaker-vxa"
    }

    fn name(&self) -> &str {
        "RPG Maker VX Ace / VX / XP"
    }

    fn description(&self) -> &str {
        "RPG Maker VX Ace (.rvdata2), VX (.rvdata) and XP (.rxdata) files (Ruby Marshal)"
    }

    fn supported_extensions(&self) -> &[&str] {
        &[".rvdata2", ".rvdata", ".rxdata"]
    }

    fn supported_modes(&self) -> Vec<OutputMode> {
        vec![OutputMode::Replace]
    }

    fn detect(&self, path: &Path) -> bool {
        if path.is_dir() {
            if let Some(data_dir) = find_capital_data_dir(path) {
                return std::fs::read_dir(&data_dir)
                    .map(|entries| {
                        entries
                            .filter_map(|e| e.ok())
                            .any(|e| e.path().extension().is_some_and(is_marshal_ext))
                    })
                    .unwrap_or(false);
            }
            return false;
        }
        if path.is_file() {
            return path.extension().is_some_and(is_marshal_ext);
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
            return Ok(Self::extract_root(&filename, &root, path));
        }

        let data_dir = find_capital_data_dir(path).ok_or_else(|| LocustError::ParseError {
            file: path.display().to_string(),
            message: "could not find Data directory".to_string(),
        })?;

        let mut all = Vec::new();
        for entry in std::fs::read_dir(&data_dir)? {
            let entry = entry?;
            let fpath = entry.path();
            if fpath.extension().is_some_and(is_marshal_ext) {
                let bytes = std::fs::read(&fpath)?;
                match MarshalValue::parse(&bytes) {
                    Ok(root) => {
                        let fname = fpath
                            .file_name()
                            .unwrap_or_default()
                            .to_string_lossy()
                            .to_string();
                        all.extend(Self::extract_root(&fname, &root, &fpath));
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
        self.inject_with_originals(path, entries, &HashMap::new())
    }

    fn inject_revision(
        &self,
        path: &Path,
        entries: &mut [StringEntry],
        originals: &HashMap<PathBuf, RevisionOriginal>,
    ) -> Result<InjectionReport> {
        self.inject_with_originals(path, entries, originals)
    }
}

impl RpgMakerVxaPlugin {
    fn inject_with_originals(
        &self,
        path: &Path,
        entries: &[StringEntry],
        originals: &HashMap<PathBuf, RevisionOriginal>,
    ) -> Result<InjectionReport> {
        let mut files_modified = 0;
        let mut strings_written = 0;
        let mut strings_skipped = 0;
        let mut files_written: Vec<PathBuf> = Vec::new();
        let mut skip_reasons = std::collections::BTreeMap::new();
        let mut warnings = Vec::new();

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
            find_capital_data_dir(path).unwrap_or_else(|| path.to_path_buf())
        } else {
            path.parent().unwrap_or(path).to_path_buf()
        };

        for (filename, file_entries) in &by_file {
            let file_path = data_dir.join(filename);
            if !file_path.exists() {
                strings_skipped += file_entries.len();
                *skip_reasons.entry("missing_target".into()).or_default() += file_entries.len();
                continue;
            }

            // Core verifies the previous Direct result and pristine provenance.
            // Read through the supplied verifier so even a backup changed after
            // that check cannot be used. Rebuild every translated field from
            // this baseline, never from an earlier injection's command layout.
            let bytes = match originals.get(&file_path) {
                Some(original) => original.read_bytes()?,
                None => std::fs::read(&file_path)?,
            };
            let mut root = match MarshalValue::parse(&bytes) {
                Ok(root) => root,
                Err(error) => {
                    strings_skipped += file_entries.len();
                    *skip_reasons
                        .entry("unsupported_marshal".into())
                        .or_default() += file_entries.len();
                    warnings.push(format!("{}: {error}", file_path.display()));
                    continue;
                }
            };
            let current: HashMap<_, _> = Self::extract_root(filename, &root, &file_path)
                .into_iter()
                .map(|entry| (entry.id, entry.source))
                .collect();

            let mut valid = Vec::new();
            for entry in file_entries {
                let reason = if entry.translation.is_none() {
                    Some("untranslated")
                } else {
                    match current.get(&entry.id) {
                        None => Some("missing_target"),
                        Some(source) if source != &entry.source => Some("source_changed"),
                        Some(_) => None,
                    }
                };
                if let Some(reason) = reason {
                    strings_skipped += 1;
                    *skip_reasons.entry(reason.into()).or_default() += 1;
                } else {
                    valid.push((*entry).clone());
                }
            }

            if valid.is_empty() {
                continue;
            }
            Self::apply_translations(&mut root, filename, &valid);

            let new_bytes = match root.try_serialize() {
                Ok(bytes) => bytes,
                Err(error) => {
                    strings_skipped += valid.len();
                    *skip_reasons
                        .entry("unsupported_marshal".into())
                        .or_default() += valid.len();
                    warnings.push(format!("{}: {error}", file_path.display()));
                    continue;
                }
            };
            std::fs::write(&file_path, new_bytes)?;
            strings_written += valid.len();
            files_modified += 1;
            files_written.push(file_path);
        }

        Ok(InjectionReport {
            skip_reasons,
            files_modified,
            strings_written,
            strings_skipped,
            warnings,
            files_written,
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
                m.insert(
                    "@name".to_string(),
                    MarshalValue::Str("TestHero".to_string()),
                );
                m.insert(
                    "@description".to_string(),
                    MarshalValue::Str("A test hero".to_string()),
                );
                m.insert("@note".to_string(), MarshalValue::Str(String::new()));
                m.insert(
                    "@nickname".to_string(),
                    MarshalValue::Str("Brave".to_string()),
                );
                m.insert("@id".to_string(), MarshalValue::Int(1));
                m
            },
        },
        MarshalValue::Object {
            class: "RPG::Actor".to_string(),
            ivars: {
                let mut m = HashMap::new();
                m.insert(
                    "@name".to_string(),
                    MarshalValue::Str("TestMage".to_string()),
                );
                m.insert(
                    "@description".to_string(),
                    MarshalValue::Str("A test mage".to_string()),
                );
                m.insert("@note".to_string(), MarshalValue::Str(String::new()));
                m.insert(
                    "@nickname".to_string(),
                    MarshalValue::Str("Wise".to_string()),
                );
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

    fn make_cmd(code: i64, params: Vec<MarshalValue>) -> MarshalValue {
        MarshalValue::Object {
            class: "RPG::EventCommand".to_string(),
            ivars: {
                let mut m = HashMap::new();
                m.insert("@code".to_string(), MarshalValue::Int(code));
                m.insert("@indent".to_string(), MarshalValue::Int(0));
                m.insert("@parameters".to_string(), MarshalValue::Array(params));
                m
            },
        }
    }

    /// XP-style map: Show Text (101) carries the first line, 401s continue it.
    fn create_xp_map_fixture() -> PathBuf {
        let dir = tempdir();
        let data_dir = dir.join("Data");
        fs::create_dir_all(&data_dir).unwrap();

        let list = MarshalValue::Array(vec![
            make_cmd(
                101,
                vec![MarshalValue::Str("Hello there, brave".to_string())],
            ),
            make_cmd(
                401,
                vec![MarshalValue::Str("adventurer of the realm!".to_string())],
            ),
            make_cmd(0, vec![]),
        ]);
        let page = MarshalValue::Object {
            class: "RPG::Event::Page".to_string(),
            ivars: {
                let mut m = HashMap::new();
                m.insert("@list".to_string(), list);
                m
            },
        };
        let event = MarshalValue::Object {
            class: "RPG::Event".to_string(),
            ivars: {
                let mut m = HashMap::new();
                m.insert("@pages".to_string(), MarshalValue::Array(vec![page]));
                m
            },
        };
        let map = MarshalValue::Object {
            class: "RPG::Map".to_string(),
            ivars: {
                let mut m = HashMap::new();
                m.insert(
                    "@events".to_string(),
                    MarshalValue::Hash(vec![(MarshalValue::Int(1), event)]),
                );
                m
            },
        };
        fs::write(data_dir.join("Map001.rxdata"), map.serialize()).unwrap();
        dir
    }

    #[test]
    fn test_xp_message_block_extract_and_rewrap() {
        let dir = create_xp_map_fixture();
        let plugin = RpgMakerVxaPlugin::new();
        let entries = plugin.extract(&dir).unwrap();

        // 101 first line + 401 continuation merged into one block
        let msg = entries
            .iter()
            .find(|e| e.id.ends_with("#msg"))
            .unwrap_or_else(|| {
                panic!(
                    "no #msg entry: {:?}",
                    entries.iter().map(|e| &e.id).collect::<Vec<_>>()
                )
            });
        assert_eq!(msg.source, "Hello there, brave\nadventurer of the realm!");

        // Inject a longer Spanish translation and verify structure
        let mut entries = entries;
        for e in &mut entries {
            if e.id.ends_with("#msg") {
                e.translation = Some(
                    "¡Hola, valiente aventurero de todos los reinos conocidos y por conocer, bienvenido seas a estas tierras!"
                        .to_string(),
                );
            }
        }
        plugin.inject(&dir, &entries).unwrap();

        let re = plugin.extract(&dir).unwrap();
        let msg = re.iter().find(|e| e.id.ends_with("#msg")).unwrap();
        // Round-trip: merged text equals the flat translation
        assert_eq!(
            msg.source.replace('\n', " "),
            "¡Hola, valiente aventurero de todos los reinos conocidos y por conocer, bienvenido seas a estas tierras!"
        );
        // And it re-wrapped into more than one line
        assert!(msg.source.contains('\n'), "{}", msg.source);
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
        assert!(
            hero.is_some(),
            "entries: {:?}",
            entries.iter().map(|e| &e.id).collect::<Vec<_>>()
        );
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
        let hero = entries2
            .iter()
            .find(|e| e.id == "Actors.rvdata2#1#@name")
            .unwrap();
        assert_eq!(hero.source, "TranslatedHero");
    }

    #[test]
    fn test_inject_rewraps_flattened_multiline_description() {
        let dir = create_vxa_fixture();
        let plugin = RpgMakerVxaPlugin::new();
        const ID: &str = "Actors.rvdata2#1#@description";

        // Give the fixture a hand-wrapped description to translate against.
        // The source here is single-line, so this value is written verbatim.
        let wrapped = "Un héroe de prueba\nde las montañas del norte.";
        let mut entries = plugin.extract(&dir).unwrap();
        for e in &mut entries {
            if e.id == ID {
                e.translation = Some(wrapped.to_string());
            }
        }
        plugin.inject(&dir, &entries).unwrap();

        // Now the source is multi-line and a provider hands back one flat line.
        let flat = "Una heroina legendaria nacida en las montanas heladas del norte.";
        let mut entries = plugin.extract(&dir).unwrap();
        assert_eq!(
            entries.iter().find(|e| e.id == ID).unwrap().source,
            wrapped,
            "setup: description should now be multi-line"
        );
        for e in &mut entries {
            if e.id == ID {
                e.translation = Some(flat.to_string());
            }
        }
        plugin.inject(&dir, &entries).unwrap();

        let out = plugin.extract(&dir).unwrap();
        let written = &out.iter().find(|e| e.id == ID).unwrap().source;
        let width = wrapped
            .lines()
            .map(crate::rpgmaker_mv::visible_len)
            .max()
            .unwrap();
        assert!(written.contains('\n'), "should be re-wrapped: {written:?}");
        for line in written.lines() {
            assert!(
                crate::rpgmaker_mv::visible_len(line) <= width,
                "line over budget: {line:?}"
            );
        }
        assert_eq!(
            written.split_whitespace().collect::<Vec<_>>(),
            flat.split_whitespace().collect::<Vec<_>>(),
            "re-wrapping must not change wording"
        );
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
