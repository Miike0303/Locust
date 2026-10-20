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
    // Lists, tuples and sets are inert sequences in pickle encounter order.
    // Sets do not hash, compare or deduplicate children (which could run code).
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
    sets: HashSet<Id>, // mutable set identities; bounded by the arena node cap
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
    fn u64(&mut self) -> Result<usize, Error> {
        let b = self.take(8)?;
        usize::try_from(u64::from_le_bytes([
            b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7],
        ]))
        .map_err(|_| "length overflow")
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
                    if self.byte()? > 5 {
                        return Err("unsupported protocol");
                    }
                }
                0x95 => {
                    let n = self.u64()?;
                    if n > self.data.len() - self.pos {
                        return Err("FRAME length exceeds remaining input");
                    }
                    // FRAME is a buffering hint, not a separate opcode stream.
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
                0x8d | 0x8e | 0x96 => {
                    let n = self.u64()?;
                    self.text(n, op == 0x8d)?;
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
                0x8f => {
                    self.sequence(Some(0), false)?;
                    self.sets.insert(self.top()?);
                }
                0x90 => {
                    let ids = self.marked()?;
                    if !self.sets.contains(&self.top()?) {
                        return Err("ADDITEMS on non-set");
                    }
                    self.extend(ids, false)?;
                }
                0x91 => self.sequence(None, false)?,
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
                0x81 | 0x92 | b'R' => {
                    if op == 0x92 {
                        // Keep (args, kwargs) as an inert args graph, preserving
                        // memo aliases and charging both wrapper edges.
                        self.sequence(Some(2), false)?;
                    }
                    let args = self.pop()?;
                    let class = self.pop()?;
                    self.object(class, args)?;
                }
                0x97 => return Err("out-of-band buffers unavailable"),
                0x98 => {
                    self.top()?; // READONLY_BUFFER only changes mutability.
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
        sets: HashSet::new(),
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
pub(crate) mod tests {
    use super::*;

    fn reader(data: &[u8]) -> Reader<'_> {
        Reader {
            data,
            pos: 0,
            arena: Vec::new(),
            stack: Vec::new(),
            memo: HashMap::new(),
            sets: HashSet::new(),
            strings: 0,
            edges: 0,
        }
    }

    // Handwritten CPython protocol-5 layout for ({}, [Say(), Menu()]), with
    // instance __dict__ state and a memo-shared renpy.ast module name. The
    // single frame contains 96 bytes, including STOP; no Python build step.
    pub(crate) fn protocol5_say_menu_pickle() -> &'static [u8] {
        b"\x80\x05\x95\x60\0\0\0\0\0\0\0\
          }\x94]\x94(\
          \x8c\x09renpy.ast\x94\x8c\x03Say\x94\x93\x94)\x81\x94}\x94\
          \x8c\x04what\x94\x8c\x06Hello!\x94sb\
          h\x02\x8c\x04Menu\x94\x93\x94)\x81\x94}\x94\
          \x8c\x05items\x94]\x94\x8c\x08Continue\x94NN\x87\x94asbe\x86\x94."
    }

    fn wide_string(op: u8, n: u64, payload: &[u8]) -> Vec<u8> {
        let mut p = vec![op];
        p.extend_from_slice(&n.to_le_bytes());
        p.extend_from_slice(payload);
        p.extend_from_slice(&[0x85, b'.']);
        p
    }

    #[test]
    fn protocol_headers_accept_zero_through_five_and_reject_six() {
        for protocol in 0..=5 {
            assert_eq!(visible_text(&[0x80, protocol, b']', b'.']), Ok(vec![]));
        }
        assert_eq!(
            visible_text(&[0x80, 6, b']', b'.']),
            Err("unsupported protocol")
        );
        assert_eq!(visible_text(&[0x80]), Err("truncated argument"));
    }

    #[test]
    fn frames_are_buffering_hints() {
        for n in [0u64, 1, 2] {
            let mut p = vec![0x95];
            p.extend_from_slice(&n.to_le_bytes());
            p.extend_from_slice(b"].");
            assert_eq!(visible_text(&p), Ok(vec![]));
        }
        // Consecutive frames can split the pickle between opcodes.
        let p = b"\x95\x01\0\0\0\0\0\0\0]\x95\x01\0\0\0\0\0\0\0.";
        assert_eq!(visible_text(p), Ok(vec![]));
    }

    #[test]
    fn frames_reject_truncated_lengths_and_lengths_past_input() {
        for n in 0..8 {
            let mut p = vec![0x95];
            p.extend_from_slice(&[0; 8][..n]);
            assert_eq!(visible_text(&p), Err("truncated argument"));
        }
        let mut p = vec![0x95];
        p.extend_from_slice(&3u64.to_le_bytes());
        p.extend_from_slice(b"].");
        assert_eq!(
            visible_text(&p),
            Err("FRAME length exceeds remaining input")
        );
        p[1..9].copy_from_slice(&u64::MAX.to_le_bytes());
        assert!(visible_text(&p).is_err());
    }

    #[test]
    fn unicode8_bytes8_and_bytearray8_reuse_text_handling() {
        for op in [0x8d, 0x8e, 0x96] {
            for text in ["", "Hello!", "¡Hola! 🙂"] {
                let p = wide_string(op, text.len() as u64, text.as_bytes());
                let (values, root) = reader(&p).read().unwrap();
                let graph = Graph { values };
                assert_eq!(graph.seq(root).len(), 1);
                assert_eq!(graph.text(graph.seq(root)[0]), Some(text));
            }
            let p = wide_string(op, 1, &[0xff]);
            if op == 0x8d {
                assert_eq!(visible_text(&p), Err("invalid UTF-8 Unicode"));
            } else {
                let (values, root) = reader(&p).read().unwrap();
                let Value::Seq(ids) = &values[root] else {
                    panic!("expected sequence");
                };
                assert!(matches!(values[ids[0]], Value::Scalar));
            }
        }
    }

    #[test]
    fn wide_strings_reject_truncated_lengths_and_payloads() {
        for op in [0x8d, 0x8e, 0x96] {
            for n in 0..8 {
                let mut p = vec![op];
                p.extend_from_slice(&[0; 8][..n]);
                assert_eq!(visible_text(&p), Err("truncated argument"));
            }
            let mut p = vec![op];
            p.extend_from_slice(&2u64.to_le_bytes());
            p.push(b'x');
            assert_eq!(visible_text(&p), Err("truncated argument"));
        }
    }

    #[test]
    fn wide_strings_reject_oversize_and_charge_cumulative_bytes() {
        for op in [0x8d, 0x8e, 0x96] {
            for n in [MAX_STRING_BYTES as u64 + 1, u64::MAX] {
                let p = wide_string(op, n, &[]);
                let expected = if usize::try_from(n).is_ok() {
                    "string byte limit"
                } else {
                    "length overflow"
                };
                assert_eq!(visible_text(&p), Err(expected));
            }
            let p = wide_string(op, 2, b"ab");
            let mut r = reader(&p);
            r.strings = MAX_STRING_BYTES - 2;
            assert!(r.read().is_ok());
            let mut r = reader(&p);
            r.strings = MAX_STRING_BYTES - 1;
            assert!(matches!(r.read(), Err("string byte limit")));
        }
    }

    #[test]
    fn empty_set_is_an_inert_sequence() {
        let (values, root) = reader(b"\x8f.").read().unwrap();
        assert!(matches!(&values[root], Value::Seq(ids) if ids.is_empty()));
        assert_eq!(visible_text(b"\x8f"), Err("truncated argument"));
    }

    #[test]
    fn additems_preserves_items_and_memo_identity() {
        let p = b"\x8f\x94(NK\x01\x90(\x8c\x01x\x90h\x00\x86.";
        let (values, root) = reader(p).read().unwrap();
        let Value::Seq(pair) = &values[root] else {
            panic!("expected tuple");
        };
        assert_eq!(pair[0], pair[1]);
        let Value::Seq(items) = &values[pair[0]] else {
            panic!("expected set sequence");
        };
        assert_eq!(items.len(), 3);
        assert!(matches!(values[items[2]], Value::Text("x")));
        assert_eq!(visible_text(b"\x8f(\x90."), Ok(vec![]));
    }

    #[test]
    fn additems_rejects_missing_mark_or_set() {
        for p in [
            &b"\x90."[..],
            &b"(N\x90."[..],
            &b"N(N\x90."[..],
            &b"](N\x90."[..],
            &b"}(N\x90."[..],
            &b"(\x91(N\x90."[..],
            &b"cbuiltins\nset\n)\x81(N\x90."[..],
        ] {
            assert!(visible_text(p).is_err(), "accepted {p:?}");
        }
    }

    #[test]
    fn frozenset_is_an_inert_sequence_and_requires_mark() {
        for (p, count) in [(&b"(\x91."[..], 0), (&b"(NK\x01\x91."[..], 2)] {
            let (values, root) = reader(p).read().unwrap();
            assert!(matches!(&values[root], Value::Seq(ids) if ids.len() == count));
        }
        assert_eq!(visible_text(b"N\x91."), Err("missing MARK"));
        assert_eq!(visible_text(b"(N\x91"), Err("truncated argument"));
    }

    #[test]
    fn sets_charge_the_same_edges_as_lists() {
        for p in [&b"\x8f(NN\x90."[..], &b"(NN\x91."[..]] {
            let mut r = reader(p);
            r.edges = MAX_EDGES - 2;
            assert!(r.read().is_ok());
            let mut r = reader(p);
            r.edges = MAX_EDGES - 1;
            assert!(matches!(r.read(), Err("edge limit")));
        }
    }

    #[test]
    fn newobj_ex_retains_positional_and_keyword_graphs_without_execution() {
        let p = b"cos\nsystem\n\x8c\x03arg\x85}\x8c\x03key\x8c\x05childs\x92.";
        let (values, root) = reader(p).read().unwrap();
        let Value::Object { class, args, .. } = &values[root] else {
            panic!("expected inert object");
        };
        assert!(matches!(values[*class], Value::Global("os", "system")));
        let args = *args;
        let graph = Graph { values };
        let args = graph.seq(args);
        assert_eq!(args.len(), 2);
        assert_eq!(graph.text(graph.seq(args[0])[0]), Some("arg"));
        let Value::Dict(kwargs) = &graph.values[args[1]] else {
            panic!("kwargs were dropped");
        };
        assert_eq!(graph.text(kwargs[0]), Some("key"));
        assert_eq!(graph.text(kwargs[1]), Some("child"));
    }

    #[test]
    fn newobj_ex_rejects_missing_arguments_and_invalid_class() {
        for p in [
            &b"\x92."[..],
            &b"cos\nsystem\n\x92."[..],
            &b"cos\nsystem\n)\x92."[..],
            &b"N)}\x92."[..],
        ] {
            assert!(visible_text(p).is_err());
        }
    }

    #[test]
    fn newobj_ex_charges_wrapper_and_object_edges() {
        let p = b"crenpy.ast\nSay\n)}\x92.";
        let mut r = reader(p);
        r.edges = MAX_EDGES - 4;
        assert!(r.read().is_ok());
        let mut r = reader(p);
        r.edges = MAX_EDGES - 3;
        assert!(matches!(r.read(), Err("edge limit")));
    }

    #[test]
    fn next_buffer_clearly_rejects_unavailable_external_data() {
        for p in [&b"\x97."[..], &b"]\x97\x98."[..]] {
            assert_eq!(visible_text(p), Err("out-of-band buffers unavailable"));
        }
    }

    #[test]
    fn readonly_buffer_is_a_noop_requiring_a_top_value() {
        let p = b"]\x94\x98h\x00a.";
        let (values, root) = reader(p).read().unwrap();
        assert!(matches!(&values[root], Value::Seq(ids) if ids == &[root]));
        assert_eq!(visible_text(b"\x98."), Err("missing stack value"));
        assert_eq!(visible_text(b"(\x98."), Err("missing stack value"));
    }

    #[test]
    fn protocol5_cpython_layout_harvests_say_and_menu() {
        let p = protocol5_say_menu_pickle();
        assert_eq!(p.len() - 11, 96);
        assert_eq!(
            visible_text(p),
            Ok(vec!["Hello!".into(), "Continue".into()])
        );
        for n in 0..p.len() {
            assert!(visible_text(&p[..n]).is_err(), "accepted prefix {n}");
        }
    }

    #[test]
    fn persistent_ids_and_extensions_remain_rejected() {
        for op in [b'P', b'Q', 0x82, 0x83, 0x84] {
            assert_eq!(visible_text(&[op]), Err("unknown/unsupported opcode"));
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
