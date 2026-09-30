//! Inert, bounded pickle graph reader for RPC2 scripts. No imports or calls execute.
use std::collections::{HashMap, HashSet};

const MAX_OPS: usize = 4_000_000;
const MAX_NODES: usize = 1_000_000;
const MAX_STACK: usize = 32_768;
const MAX_MEMO: usize = 1_000_000;
const MAX_STRING_BYTES: usize = 64 * 1024 * 1024;
const MAX_EDGES: usize = 4_000_000;
const MAX_DEPTH: usize = 256;
type Id = usize;
type Error = &'static str;

enum Value<'a> {
    Scalar,
    Text(&'a str),
    Seq(Vec<Id>),
    Dict(Vec<Id>), // alternating keys/values; memo aliases retain identity
    Global(&'a str, &'a str),
    Object {
        class: Id,
        args: Id,
        state: Option<Id>,
        items: Vec<Id>,
    },
}

struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
    arena: Vec<Value<'a>>,
    stack: Vec<Option<Id>>, // None is MARK, never a Python None
    memo: HashMap<usize, Id>,
    strings: usize,
    edges: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], Error> {
        let end = self.pos.checked_add(n).ok_or("length overflow")?;
        let bytes = self.data.get(self.pos..end).ok_or("truncated argument")?;
        self.pos = end;
        Ok(bytes)
    }
    fn byte(&mut self) -> Result<u8, Error> {
        Ok(self.take(1)?[0])
    }
    fn u32(&mut self) -> Result<usize, Error> {
        let b = self.take(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize)
    }
    fn line(&mut self) -> Result<&'a str, Error> {
        let n = self.data[self.pos..]
            .iter()
            .position(|&b| b == b'\n')
            .ok_or("truncated line")?;
        let b = self.take(n + 1)?;
        std::str::from_utf8(&b[..n]).map_err(|_| "invalid text argument")
    }
    fn push(&mut self, id: Option<Id>) -> Result<(), Error> {
        if self.stack.len() >= MAX_STACK {
            return Err("stack limit");
        }
        self.stack.push(id);
        Ok(())
    }
    fn add(&mut self, v: Value<'a>) -> Result<(), Error> {
        if self.arena.len() >= MAX_NODES {
            return Err("node limit");
        }
        let id = self.arena.len();
        self.arena.push(v);
        self.push(Some(id))
    }
    fn pop(&mut self) -> Result<Id, Error> {
        self.stack
            .pop()
            .flatten()
            .ok_or("stack underflow or unexpected MARK")
    }
    fn top(&self) -> Result<Id, Error> {
        self.stack
            .last()
            .copied()
            .flatten()
            .ok_or("missing stack value")
    }
    fn marked(&mut self) -> Result<Vec<Id>, Error> {
        let at = self
            .stack
            .iter()
            .rposition(Option::is_none)
            .ok_or("missing MARK")?;
        let ids = self.stack.drain(at + 1..).flatten().collect();
        self.stack.pop();
        Ok(ids)
    }
    fn charge_edges(&mut self, n: usize) -> Result<(), Error> {
        self.edges = self.edges.checked_add(n).ok_or("edge limit")?;
        if self.edges > MAX_EDGES {
            return Err("edge limit");
        }
        Ok(())
    }
    fn sequence(&mut self, n: Option<usize>, dict: bool) -> Result<(), Error> {
        let ids = if let Some(n) = n {
            let mut ids = Vec::with_capacity(n);
            for _ in 0..n {
                ids.push(self.pop()?);
            }
            ids.reverse();
            ids
        } else {
            self.marked()?
        };
        if dict && !ids.len().is_multiple_of(2) {
            return Err("odd dictionary items");
        }
        self.charge_edges(ids.len())?;
        self.add(if dict {
            Value::Dict(ids)
        } else {
            Value::Seq(ids)
        })
    }
    fn extend(&mut self, ids: Vec<Id>, dict: bool) -> Result<(), Error> {
        if dict && !ids.len().is_multiple_of(2) {
            return Err("odd dictionary items");
        }
        self.charge_edges(ids.len())?;
        let id = self.top()?;
        match &mut self.arena[id] {
            Value::Seq(v) if !dict => v.extend(ids),
            Value::Dict(v) if dict => v.extend(ids),
            Value::Object { items, .. } => items.extend(ids),
            _ => return Err("invalid container mutation"),
        }
        Ok(())
    }
    fn text(&mut self, n: usize, unicode: bool) -> Result<(), Error> {
        self.strings = self.strings.checked_add(n).ok_or("string byte limit")?;
        if self.strings > MAX_STRING_BYTES {
            return Err("string byte limit");
        }
        // Check the complete slice before any allocation. Python-2 byte strings
        // that aren't UTF-8 are opaque; they are not runtime Unicode dialogue.
        let bytes = self.take(n)?;
        match std::str::from_utf8(bytes) {
            Ok(s) => self.add(Value::Text(s)),
            Err(_) if !unicode => self.add(Value::Scalar),
            Err(_) => Err("invalid UTF-8 Unicode"),
        }
    }
    fn global(&mut self) -> Result<(), Error> {
        let module = self.line()?;
        let name = self.line()?;
        self.add(Value::Global(module, name))
    }
    fn object(&mut self, class: Id, args: Id) -> Result<(), Error> {
        if !matches!(self.arena[class], Value::Global(..)) {
            return Err("invalid callable/class");
        }
        self.charge_edges(2)?;
        self.add(Value::Object {
            class,
            args,
            state: None,
            items: Vec::new(),
        })
    }
    fn memo_put(&mut self, index: usize) -> Result<(), Error> {
        if index >= MAX_MEMO || (self.memo.len() >= MAX_MEMO && !self.memo.contains_key(&index)) {
            return Err("memo limit");
        }
        self.memo.insert(index, self.top()?);
        Ok(())
    }
    fn memo_get(&mut self, index: usize) -> Result<(), Error> {
        let id = *self.memo.get(&index).ok_or("missing memo entry")?;
        self.push(Some(id))
    }
    fn read(mut self) -> Result<(Vec<Value<'a>>, Id), Error> {
        for _ in 0..MAX_OPS {
            let op = self.byte()?;
            match op {
                0x80 => {
                    if self.byte()? > 4 {
                        return Err("unsupported protocol");
                    }
                }
                b'.' => {
                    let root = self.pop()?;
                    if !self.stack.is_empty() {
                        return Err("unbalanced STOP");
                    }
                    if !matches!(
                        self.arena[root],
                        Value::Seq(_) | Value::Dict(_) | Value::Object { .. }
                    ) {
                        return Err("not an AST graph");
                    }
                    return Ok((self.arena, root));
                }
                b'(' => self.push(None)?,
                b'0' => {
                    self.stack.pop().ok_or("POP underflow")?;
                }
                b'1' => {
                    self.marked()?;
                }
                b'2' => {
                    let id = self.top()?;
                    self.push(Some(id))?;
                }
                b'N' | 0x88 | 0x89 => self.add(Value::Scalar)?,
                b'K' | b'M' | b'J' | b'G' => {
                    self.take(match op {
                        b'K' => 1,
                        b'M' => 2,
                        b'J' => 4,
                        _ => 8,
                    })?;
                    self.add(Value::Scalar)?;
                }
                0x8a | 0x8b => {
                    let n = if op == 0x8a {
                        self.byte()? as usize
                    } else {
                        self.u32()?
                    };
                    self.take(n)?;
                    self.add(Value::Scalar)?;
                }
                b'X' | b'T' | b'B' => {
                    let n = self.u32()?;
                    self.text(n, op == b'X')?;
                }
                b'U' | b'C' | 0x8c => {
                    let n = self.byte()? as usize;
                    self.text(n, op == 0x8c)?;
                }
                b'I' | b'L' | b'F' => {
                    self.line()?;
                    self.add(Value::Scalar)?;
                }
                b'q' | b'r' | b'p' => {
                    let n = match op {
                        b'q' => self.byte()? as usize,
                        b'r' => self.u32()?,
                        _ => self.line()?.parse().map_err(|_| "bad memo index")?,
                    };
                    self.memo_put(n)?;
                }
                b'h' | b'j' | b'g' => {
                    let n = match op {
                        b'h' => self.byte()? as usize,
                        b'j' => self.u32()?,
                        _ => self.line()?.parse().map_err(|_| "bad memo index")?,
                    };
                    self.memo_get(n)?;
                }
                0x94 => self.memo_put(self.memo.len())?,
                b')' | b']' => self.sequence(Some(0), false)?,
                b'}' => self.sequence(Some(0), true)?,
                0x85..=0x87 => self.sequence(Some((op - 0x84) as usize), false)?,
                b't' | b'l' => self.sequence(None, false)?,
                b'd' => self.sequence(None, true)?,
                b'a' => {
                    let id = self.pop()?;
                    self.extend(vec![id], false)?;
                }
                b'e' => {
                    let ids = self.marked()?;
                    self.extend(ids, false)?;
                }
                b's' => {
                    let v = self.pop()?;
                    let k = self.pop()?;
                    self.extend(vec![k, v], true)?;
                }
                b'u' => {
                    let ids = self.marked()?;
                    self.extend(ids, true)?;
                }
                b'c' => self.global()?,
                0x93 => {
                    let name = self.pop()?;
                    let module = self.pop()?;
                    let (Value::Text(m), Value::Text(n)) = (&self.arena[module], &self.arena[name])
                    else {
                        return Err("invalid STACK_GLOBAL");
                    };
                    self.add(Value::Global(m, n))?;
                }
                0x81 | b'R' => {
                    let args = self.pop()?;
                    let class = self.pop()?;
                    self.object(class, args)?;
                }
                b'b' => {
                    let state = self.pop()?;
                    let object = self.top()?;
                    self.charge_edges(1)?;
                    if let Value::Object { state: s, .. } = &mut self.arena[object] {
                        *s = Some(state);
                    } else {
                        return Err("BUILD on non-object");
                    }
                }
                b'o' | b'i' => {
                    if op == b'i' {
                        self.global()?;
                    }
                    let mut ids = self.marked()?;
                    if ids.is_empty() {
                        return Err("empty object arguments");
                    }
                    let class = if op == b'o' {
                        ids.remove(0)
                    } else {
                        ids.pop().ok_or("missing class")?
                    };
                    self.charge_edges(ids.len())?;
                    self.add(Value::Seq(ids))?;
                    let args = self.pop()?;
                    self.object(class, args)?;
                }
                // Persistent IDs and extension registries cannot be resolved
                // safely without an external environment. Fail into the guarded
                // fallback, never treat their payload as an AST or execute it.
                _ => return Err("unknown/unsupported opcode"),
            }
        }
        Err("opcode limit")
    }
}

struct Graph<'a> {
    values: Vec<Value<'a>>,
}
impl<'a> Graph<'a> {
    fn seq(&self, id: Id) -> &[Id] {
        match &self.values[id] {
            Value::Seq(v) => v,
            Value::Object { items, .. } => items,
            _ => &[],
        }
    }
    fn text(&self, id: Id) -> Option<&'a str> {
        match self.values[id] {
            Value::Text(s) => Some(s),
            _ => None,
        }
    }
    fn expression(&self, id: Id) -> Option<(Id, &'a str)> {
        self.text(id)
            .map(|s| (id, s))
            .or_else(|| match &self.values[id] {
                Value::Object { class, args, .. }
                    if self.class_is(*class, "renpy.ast", "PyExpr")
                        || self.class_is(*class, "renpy.ast", "PyExprCache") =>
                {
                    self.seq(*args)
                        .first()
                        .and_then(|&v| self.text(v).map(|s| (v, s)))
                }
                _ => None,
            })
    }
    fn class_is(&self, id: Id, module: &str, name: &str) -> bool {
        matches!(self.values[id], Value::Global(m, n) if m == module && n == name)
    }
    fn fields(&self, state: Id, out: &mut Vec<(Id, Id)>) {
        // BUILD commonly uses either a dict or (dict, slotdict). Do not search
        // arbitrary nested state: it could contain code or cyclic containers.
        if let Value::Dict(v) = &self.values[state] {
            out.extend(v.chunks_exact(2).map(|v| (v[0], v[1])));
        } else if let Value::Seq(v) = &self.values[state] {
            for &id in v {
                if let Value::Dict(d) = &self.values[id] {
                    out.extend(d.chunks_exact(2).map(|v| (v[0], v[1])));
                }
            }
        }
    }
}

/// Cycle 98 audit: 310 pickles, 54,296 Say and 580 Menu nodes; the old
/// heuristic missed 8,740 Say.what rows and harvested 35,055 code/locator rows,
/// 6,318 other fields, and 636 unmapped strings. Visible fields are Say.what,
/// Menu.items[*][0] (captions AND choices), Translate.block, TranslateString.old,
/// Screen.screen, and SLDisplayable positional[0] for Text/_label/_textbutton
/// (966/116/393 displayables), plus text/tooltip keywords (324 tooltips).
/// Static literals in those fields: 786 Text, 108 labels, 268 textbuttons,
/// 263 tooltips; TL old-strings prove 6/1/2/7 occurrences translatable.
/// SLBlock/SLScreen/SLFor.children and If/SLIf.entries[*][1] are structure only.
/// No PyCode/Python, Define/Default, Image/ATL, UserStatement raw source,
/// conditions, actions, filenames, styles, or locators are walked. Say.who is
/// an expression: none of the 17 quoted names occurred in TL old-strings.
/// Translate nodes weren't in this sample; their old/block fields follow the
/// audit's TL old/comment contract, and never include translated new strings.
pub(crate) fn visible_text(data: &[u8]) -> Result<Vec<String>, Error> {
    if data.len() > MAX_STRING_BYTES {
        return Err("pickle byte limit");
    }
    let reader = Reader {
        data,
        pos: 0,
        arena: Vec::new(),
        stack: Vec::new(),
        memo: HashMap::new(),
        strings: 0,
        edges: 0,
    };
    let (values, root) = reader.read()?;
    let graph = Graph { values };
    let mut pending = vec![(root, 0usize)];
    let mut visited = vec![false; graph.values.len()];
    let mut out = Vec::new();
    let mut selected = HashSet::new();
    let mut selected_expressions = HashSet::new();
    let mut output_bytes = 0usize;
    let mut work = 0usize;
    while let Some((id, depth)) = pending.pop() {
        work += 1;
        if work > MAX_EDGES || depth > MAX_DEPTH {
            return Err("graph walk limit");
        }
        if visited[id] {
            continue;
        }
        visited[id] = true;
        let mut children = Vec::new();
        match &graph.values[id] {
            Value::Seq(v) => children.extend(v),
            Value::Dict(v) => children.extend(v.chunks_exact(2).map(|v| v[1])),
            Value::Object { class, state, .. } => {
                let Value::Global(module, name) = graph.values[*class] else {
                    continue;
                };
                let mut fields = Vec::new();
                if let Some(state) = state {
                    graph.fields(*state, &mut fields);
                }
                work = work.saturating_add(fields.len());
                if work > MAX_EDGES {
                    return Err("field walk limit");
                }
                let field = |name: &str| {
                    fields
                        .iter()
                        .rev()
                        .find(|&&(k, _)| graph.text(k) == Some(name))
                        .map(|&(_, v)| v)
                };
                let mut direct = |value: Id| -> Result<(), Error> {
                    if let Some(s) = graph.text(value) {
                        if selected.insert(value) {
                            charge_output(&mut output_bytes, s.len())?;
                            out.push((value, s.to_owned()));
                        }
                    }
                    Ok(())
                };
                match (module, name) {
                    ("renpy.ast", "Say") => {
                        if let Some(v) = field("what") {
                            direct(v)?;
                        }
                    }
                    ("renpy.ast", "TranslateString") => {
                        if let Some(v) = field("old") {
                            direct(v)?;
                        }
                    }
                    ("renpy.ast", "Menu") => {
                        if let Some(items) = field("items") {
                            for &item in graph.seq(items) {
                                let parts = graph.seq(item);
                                if parts.len() == 3 {
                                    direct(parts[0])?;
                                    children.push(parts[2]);
                                }
                            }
                        }
                    }
                    (
                        "renpy.ast",
                        "Label"
                        | "Init"
                        | "While"
                        | "Translate"
                        | "TranslateBlock"
                        | "TranslateEarlyBlock",
                    ) => children.extend(field("block")),
                    ("renpy.ast", "Screen") => children.extend(field("screen")),
                    ("renpy.ast", "If") | ("renpy.sl2.slast", "SLIf" | "SLShowIf") => {
                        if let Some(entries) = field("entries") {
                            for &entry in graph.seq(entries) {
                                if let Some(&block) = graph.seq(entry).get(1) {
                                    children.push(block);
                                }
                            }
                        }
                    }
                    ("renpy.sl2.slast", "SLScreen" | "SLBlock" | "SLFor" | "SLDisplayable") => {
                        children.extend(field("children"));
                        if name == "SLDisplayable" {
                            let named_text = field("name")
                                .and_then(|v| graph.text(v))
                                .is_some_and(|n| matches!(n, "text" | "label" | "textbutton"));
                            let class_text = field("displayable").is_some_and(|v| {
                                graph.class_is(v, "renpy.text.text", "Text")
                                    || graph.class_is(v, "renpy.ui", "_label")
                                    || graph.class_is(v, "renpy.ui", "_textbutton")
                            });
                            if named_text || class_text {
                                if let Some(&v) =
                                    field("positional").and_then(|p| graph.seq(p).first())
                                {
                                    collect_expression(
                                        &graph,
                                        v,
                                        &mut out,
                                        &mut selected_expressions,
                                        &mut output_bytes,
                                    )?;
                                }
                            }
                            if let Some(keywords) = field("keyword") {
                                for &pair in graph.seq(keywords) {
                                    let pair = graph.seq(pair);
                                    if pair.len() == 2
                                        && graph
                                            .text(pair[0])
                                            .is_some_and(|k| matches!(k, "text" | "tooltip"))
                                    {
                                        collect_expression(
                                            &graph,
                                            pair[1],
                                            &mut out,
                                            &mut selected_expressions,
                                            &mut output_bytes,
                                        )?;
                                    }
                                }
                            }
                        }
                    }
                    ("renpy.sl2.slast", "SLUse") => {
                        children.extend(field("block"));
                        children.extend(field("ast"));
                    }
                    ("renpy.python", "RevertableList" | "RevertableDict")
                    | ("renpy.revertable", "RevertableList" | "RevertableDict") => {
                        children.extend(graph.seq(id))
                    }
                    _ => {}
                }
            }
            _ => {}
        }
        if pending.len().saturating_add(children.len()) > MAX_EDGES {
            return Err("walk stack limit");
        }
        pending.extend(children.into_iter().rev().map(|id| (id, depth + 1)));
    }
    // Node IDs follow pickle occurrence order, including memo-shared strings.
    // Sort before deduplication by the caller; never trim or re-escape what.
    out.sort_by_key(|(id, _)| *id);
    Ok(out.into_iter().map(|(_, s)| s).collect())
}

fn charge_output(bytes: &mut usize, n: usize) -> Result<(), Error> {
    *bytes = bytes.checked_add(n).ok_or("output byte limit")?;
    if *bytes > MAX_STRING_BYTES {
        return Err("output byte limit");
    }
    Ok(())
}

fn collect_expression(
    graph: &Graph<'_>,
    id: Id,
    out: &mut Vec<(Id, String)>,
    seen: &mut HashSet<Id>,
    bytes: &mut usize,
) -> Result<(), Error> {
    let Some((source_id, expr)) = graph.expression(id) else {
        return Ok(());
    };
    // PyExpr objects can share their source through the memo. Charge/decode
    // that source once, even if millions of wrappers point at the same bytes.
    if !seen.insert(source_id) {
        return Ok(());
    }
    charge_output(bytes, expr.len())?;
    let expr = expr.trim();
    if let Some((text, end)) = literal(expr, 0) {
        if expr[end..].trim().is_empty() {
            if text.is_empty() {
                return Ok(());
            }
            out.push((source_id, text));
            return Ok(());
        }
    }
    // Extract only literal arguments of explicit _/__/renpy translation calls.
    // Dynamic variables, arbitrary quoted arguments and code stay opaque.
    let bytes = expr.as_bytes();
    let mut at = 0;
    while at < bytes.len() {
        if let Some((_, end)) = literal(expr, at) {
            at = end;
            continue;
        }
        let tail = &expr[at..];
        let prefix = [
            "renpy.translate_string",
            "renpy.translation.translate_string",
            "__",
            "_",
        ]
        .into_iter()
        .find(|p| tail.starts_with(p));
        if let Some(prefix) = prefix {
            let boundary = at == 0
                || !matches!(bytes[at - 1], b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'_' | b'.');
            let rest = tail[prefix.len()..].trim_start();
            if boundary && rest.starts_with('(') {
                let arg = rest[1..].trim_start();
                if let Some((text, end)) = literal(arg, 0) {
                    if !text.is_empty() && arg[end..].trim_start().starts_with(')') {
                        out.push((source_id, text));
                    }
                    at = expr.len() - arg.len() + end;
                    continue;
                }
            }
        }
        at += expr[at..].chars().next().map_or(1, char::len_utf8);
    }
    Ok(())
}

/// Decode a Python string literal without evaluation. This is only for screen
/// expressions; Say.what already contains the exact runtime Unicode string.
fn literal(s: &str, at: usize) -> Option<(String, usize)> {
    let b = s.as_bytes();
    let mut i = at;
    let mut raw = false;
    for _ in 0..2 {
        match b.get(i) {
            Some(b'r' | b'R') => {
                raw = true;
                i += 1;
            }
            Some(b'u' | b'U') => i += 1,
            _ => break,
        }
    }
    let q = *b.get(i)?;
    if !matches!(q, b'\'' | b'"') {
        return None;
    }
    let triple = b.get(i..i + 3).is_some_and(|v| v == [q, q, q]);
    let width = if triple { 3 } else { 1 };
    i += width;
    let mut out = String::new();
    while i < b.len() {
        if b[i] == q && (!triple || b.get(i..i + 3).is_some_and(|v| v == [q, q, q])) {
            return Some((out, i + width));
        }
        if b[i] != b'\\' {
            let c = s[i..].chars().next()?;
            out.push(c);
            i += c.len_utf8();
            continue;
        }
        i += 1;
        let c = *b.get(i)?;
        i += 1;
        if raw {
            out.push('\\');
            let ch = s[i - 1..].chars().next()?;
            out.push(ch);
            i += ch.len_utf8() - 1;
            continue;
        }
        match c {
            b'n' => out.push('\n'),
            b'r' => out.push('\r'),
            b't' => out.push('\t'),
            b'a' => out.push('\x07'),
            b'b' => out.push('\x08'),
            b'f' => out.push('\x0c'),
            b'v' => out.push('\x0b'),
            b'\\' | b'\'' | b'"' => out.push(c as char),
            b'\n' => {}
            b'\r' => {
                if b.get(i) == Some(&b'\n') {
                    i += 1;
                }
            }
            b'N' => return None, // named Unicode escapes require a Unicode database
            b'x' | b'u' | b'U' => {
                let n = match c {
                    b'x' => 2,
                    b'u' => 4,
                    _ => 8,
                };
                let digits = s.get(i..i + n)?;
                let code = u32::from_str_radix(digits, 16).ok()?;
                out.push(char::from_u32(code)?);
                i += n;
            }
            b'0'..=b'7' => {
                let mut code = (c - b'0') as u32;
                for _ in 0..2 {
                    if let Some(d @ b'0'..=b'7') = b.get(i) {
                        code = code * 8 + (d - b'0') as u32;
                        i += 1;
                    } else {
                        break;
                    }
                }
                out.push(char::from_u32(code)?);
            }
            _ => {
                out.push('\\');
                let ch = s[i - 1..].chars().next()?;
                out.push(ch);
                i += ch.len_utf8() - 1;
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reader(data: &[u8]) -> Reader<'_> {
        Reader {
            data,
            pos: 0,
            arena: Vec::new(),
            stack: Vec::new(),
            memo: HashMap::new(),
            strings: 0,
            edges: 0,
        }
    }

    #[test]
    fn budgets_reject_before_allocating_payloads_or_sparse_memos() {
        let mut r = reader(b"x");
        r.strings = MAX_STRING_BYTES;
        assert_eq!(r.text(1, true), Err("string byte limit"));
        assert!(r.arena.is_empty());
        assert_eq!(r.pos, 0);
        let mut r = reader(&[]);
        r.add(Value::Scalar).unwrap();
        assert_eq!(r.memo_put(MAX_MEMO), Err("memo limit"));
        assert!(r.memo.is_empty());
        r.edges = MAX_EDGES;
        assert_eq!(r.charge_edges(1), Err("edge limit"));
        let mut bytes = MAX_STRING_BYTES;
        assert_eq!(charge_output(&mut bytes, 1), Err("output byte limit"));
        let mut r = reader(&[]);
        assert_eq!(r.text(usize::MAX, true), Err("string byte limit"));
        assert!(r.arena.is_empty());
    }

    #[test]
    fn operation_and_node_budgets_are_enforced() {
        let mut p = Vec::with_capacity(MAX_OPS + 2);
        p.extend_from_slice(&[0x80, 2]);
        for _ in 0..MAX_OPS / 2 {
            p.extend_from_slice(b"(0");
        }
        assert_eq!(visible_text(&p), Err("opcode limit"));
        let mut p = Vec::with_capacity(MAX_NODES * 2 + 2);
        p.extend_from_slice(&[0x80, 2]);
        for _ in 0..=MAX_NODES {
            p.extend_from_slice(b"N0");
        }
        assert_eq!(visible_text(&p), Err("node limit"));
    }

    #[test]
    fn shared_expression_source_is_decoded_once() {
        let graph = Graph {
            values: vec![
                Value::Text("_('Back')"),
                Value::Seq(vec![0]),
                Value::Global("renpy.ast", "PyExpr"),
                Value::Object {
                    class: 2,
                    args: 1,
                    state: None,
                    items: Vec::new(),
                },
                Value::Object {
                    class: 2,
                    args: 1,
                    state: None,
                    items: Vec::new(),
                },
            ],
        };
        let mut seen = HashSet::new();
        let mut out = Vec::new();
        let mut bytes = 0;
        collect_expression(&graph, 3, &mut out, &mut seen, &mut bytes).unwrap();
        collect_expression(&graph, 4, &mut out, &mut seen, &mut bytes).unwrap();
        assert_eq!(out, [(0, "Back".to_owned())]);
        assert_eq!(bytes, "_('Back')".len());
    }

    #[test]
    fn screen_literals_decode_without_evaluating_expressions() {
        for (source, expected) in [
            (r#"u'\u00a1Hola!\n'"#, "¡Hola!\n"),
            (r#"r'\hola\é'"#, "\\hola\\é"),
            ("'''first\nsecond'''", "first\nsecond"),
            ("'a\\\r\nb'", "ab"),
            (r#"'\101\x42\U0001f642'"#, "AB🙂"),
        ] {
            assert_eq!(
                literal(source, 0),
                Some((expected.to_owned(), source.len()))
            );
        }
        for source in [
            r#"'\xzz'"#,
            r#"'\uD800'"#,
            r#"'\N{LATIN SMALL LETTER A}'"#,
            "'unfinished",
        ] {
            assert_eq!(literal(source, 0), None);
        }
        let graph = Graph {
            values: vec![Value::Text(
                "FileTime(slot, format=_('Time'), empty=__('Empty'))",
            )],
        };
        let mut out = Vec::new();
        collect_expression(&graph, 0, &mut out, &mut HashSet::new(), &mut 0).unwrap();
        assert_eq!(out, [(0, "Time".to_owned()), (0, "Empty".to_owned())]);
        let graph = Graph {
            values: vec![Value::Text("ShowMenu('Code title') + flag + player.name")],
        };
        let mut out = Vec::new();
        collect_expression(&graph, 0, &mut out, &mut HashSet::new(), &mut 0).unwrap();
        assert!(out.is_empty());
    }
}
