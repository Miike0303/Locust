//! Unregistered, inert, bounded Dialogic timeline visible-span codec (GD-3).
//!
//! # Primary contract
//!
//! Dialogic **2.0-alpha-16**, commit `fed4736c400d809d248e77f2dd40ae93b5feb4db`,
//! is a compatible reference, **not the installed addon version**. All paths
//! below are relative to `addons/dialogic/` at that commit:
//!
//! * [`Resources/TimelineResourceLoader.gd:34-46`](https://github.com/dialogic-godot/dialogic/blob/fed4736c400d809d248e77f2dd40ae93b5feb4db/addons/dialogic/Resources/TimelineResourceLoader.gd#L34-L46)
//!   reads text; [`Resources/timeline.gd:24-28,94-138`](https://github.com/dialogic-godot/dialogic/blob/fed4736c400d809d248e77f2dd40ae93b5feb4db/addons/dialogic/Resources/timeline.gd#L94-L138)
//!   splits LF, removes leading indentation, skips blank lines and chooses the
//!   first matching event. Following nonblank lines belong to that event until
//!   `is_string_full_event` succeeds.
//! * [`Core/DialogicResourceUtil.gd:154-168`](https://github.com/dialogic-godot/dialogic/blob/fed4736c400d809d248e77f2dd40ae93b5feb4db/addons/dialogic/Core/DialogicResourceUtil.gd#L154-L168)
//!   gets events from indexers and puts Text last. Registered stock events were
//!   checked in `Modules/{Audio,Background,Call,Character,Choice,Clear,Comment,
//!   Condition,Core,End,History,Jump,Save,Settings,Signal,Style,Text,TextInput,
//!   Variable,Voice,Wait,WaitInput}/index.gd:5-6` (Jump:5-6 lists three;
//!   Audio:5-6 lists two). For example
//!   [`Modules/Audio/index.gd:5-6`](https://github.com/dialogic-godot/dialogic/blob/fed4736c400d809d248e77f2dd40ae93b5feb4db/addons/dialogic/Modules/Audio/index.gd#L5-L6).
//!   `Modules/Glossary/index.gd:5-7` registers none. Dynamic/custom indexers are
//!   unknown: this codec deliberately supports only the stock syntax below.
//!   Shortcode names come from `get_shortcode` in these files (relative to
//!   `Modules/`): `Audio/event_music.gd:56-57`, `Audio/event_sound.gd:52-53`,
//!   `Background/event_background.gd:78-79`, `Clear/event_clear.gd:79-80`,
//!   `End/event_end.gd:32-33`, `History/event_history.gd:46-47`,
//!   `Save/event_save.gd:49-50`, `Signal/event_signal.gd:52-53`,
//!   `Style/event_style.gd:39-40`, `TextInput/event_text_input.gd:59-60`,
//!   `Voice/event_voice.gd:56-57`, `Wait/event_wait.gd:74-75`, and
//!   `WaitInput/event_wait_input.gd:36-37`.
//! * [`Resources/event.gd:215-250,383-406`](https://github.com/dialogic-godot/dialogic/blob/fed4736c400d809d248e77f2dd40ae93b5feb4db/addons/dialogic/Resources/event.gd#L215-L250)
//!   removes `#id:` before translatable event loading/testing. Stock shortcode
//!   recognition is prefix `[` + name + space or `]`; fullness is trailing `]`.
//!   Shortcode arguments are opaque here, never interpreted or translated.
//! * [`Modules/Text/event_text.gd:39-40,268-324`](https://github.com/dialogic-godot/dialogic/blob/fed4736c400d809d248e77f2dd40ae93b5feb4db/addons/dialogic/Modules/Text/event_text.gd#L268-L324)
//!   has an optional quoted/unquoted name, optional `(portrait)`, unescaped `:`,
//!   and text. It decodes `\\:` and `\\` + LF. A trailing `\\` makes ONE event
//!   consume following lines. `_` means no character; unknown names can create
//!   runtime characters. We instead diagnose unknown/ambiguous speakers. Escaped
//!   event prefixes and `<Empty Text Event>` are outside this narrow profile.
//! * [`Modules/Choice/event_choice.gd:30,64-118,125-135`](https://github.com/dialogic-godot/dialogic/blob/fed4736c400d809d248e77f2dd40ae93b5feb4db/addons/dialogic/Modules/Choice/event_choice.gd#L64-L135)
//!   recognizes any leading `-`, with the supported spelling `- text`, escaped
//!   `\\|`, optional `|`, `[if ...]`, and optional parameter bracket. Only the
//!   main label is supported here. `alt_text` (another translatable property),
//!   malformed suffixes and ambiguous choices diagnose the whole event.
//! * [`Modules/Condition/event_condition.gd:84-99`](https://github.com/dialogic-godot/dialogic/blob/fed4736c400d809d248e77f2dd40ae93b5feb4db/addons/dialogic/Modules/Condition/event_condition.gd#L84-L99)
//!   accepts `if`/`elif`, `if `/`elif ` prefixes, and ANY `else` prefix, with
//!   **no colon requirement**. These produce diagnostics, even natural prose.
//! * Other stock prefix tests (paths relative to `Modules/`) are
//!   `Character/event_character.gd:327-330`
//!   (`join`, `leave`, `update`), `Call/event_call.gd:120-123` (`do`),
//!   `Jump/event_jump.gd:88-91` (`jump`), `Jump/event_label.gd:66-69` (`label`),
//!   `Jump/event_return.gd:46-49` (exact `return`),
//!   `Variable/event_variable.gd:223-224` (`set`),
//!   `Settings/event_setting.gd:140-141` (`setting`),
//!   `Comment/event_comment.gd:46-49` (`#`), and
//!   `Core/event_end_branch.gd:79-82` (`<<END BRANCH>>`). Prefix matches do not
//!   require word boundaries. Label display names and text-input properties
//!   remain unsupported and diagnosed rather than emitted as arbitrary strings.
//! * [`Core/DialogicResourceUtil.gd:19-26,54-88`](https://github.com/dialogic-godot/dialogic/blob/fed4736c400d809d248e77f2dd40ae93b5feb4db/addons/dialogic/Core/DialogicResourceUtil.gd#L54-L88)
//!   initially suggests the filename without extension, qualifies collisions
//!   with directories, and permits identifier overrides. `CharacterSet` accepts
//!   filenames or explicit identifiers from the caller's authoritative directory;
//!   it never treats display names/nicknames as speaker identifiers.
//! * [`Resources/character.gd:9-10`](https://github.com/dialogic-godot/dialogic/blob/fed4736c400d809d248e77f2dd40ae93b5feb4db/addons/dialogic/Resources/character.gd#L9-L10)
//!   declares `display_name` and `nicknames`.
//!   [`Resources/CharacterResourceLoader.gd:35-46`](https://github.com/dialogic-godot/dialogic/blob/fed4736c400d809d248e77f2dd40ae93b5feb4db/addons/dialogic/Resources/CharacterResourceLoader.gd#L35-L46)
//!   uses `str_to_var`/`dict_to_inst`; **we never do**. A tiny lexical scanner
//!   extracts just those two top-level fields. Unknown values (including calls,
//!   NodePath, Color and nested containers) are balanced and skipped opaquely.
//!   Known strings accept JSON string escapes; other escapes explicitly fail.
//!   Character names are metadata only, NOT translatable spans in this unit.
//!
//! # Deliberate codec policy (not additional engine guarantees)
//!
//! At most 4 MiB input/output, 64 KiB per physical line excluding LF/CRLF, and
//! 64 nested delimiters per protected/opaque value. No evaluation, filesystem access, registration, PCK
//! writes or extraction wiring. BOM and bare CR are explicitly refused. CRLF is
//! treated as a physical separator like LF; this is a codec compatibility policy,
//! not a claim that alpha16's raw LF splitter normalizes CRLF at runtime.
//!
//! Span line indices are zero-based, ordinal is zero (one span per event), and
//! an optional `#id:` supplements rather than replaces positional identity.
//! `range` encloses the original text pieces, including internal continuation
//! gaps. `visible_text` is the translator representation: complete `[...]`,
//! `{...}`, and supported escapes become immutable U+E000 + decimal ordinal +
//! U+E001 placeholders. Tags need balanced delimiters, not paired BBCode names
//! (Godot supports standalone tags). Original tokens must occur once, in order,
//! in their original segment. Raw new tags/variables/escapes are refused.
//! Unknown bracket prefixes are diagnosed; recognized BBCode/inline prefixes
//! use the conservative allowlist in `inline_prefix`. Custom syntax is unverified.
//! An unknown unfinished bracket event quarantines subsequent nonblank lines
//! through a trailing `]`; this is conservative, not a custom-parser claim.
//!
//! A continuation span has one translator LF per original continuation. Rebuild
//! requires exactly that segment count, maps each LF to the original `\\` plus
//! LF/CRLF and next indentation, and preserves each segment's edge whitespace.
//! Empty segments, dangling/blank-interrupted continuations, early IDs and
//! unsupported content diagnose the entire group. Final newline and every byte
//! outside replaced pieces survive. Rebuild checks exact original provenance,
//! rejects altered timelines/unknown IDs, validates hazards and bounds, then
//! reparses to verify identities, speakers and diagnostics. No runtime, font or
//! linguistic correctness is implied.

use std::collections::{HashMap, HashSet};
use std::io;
use std::ops::Range;

pub const MAX_INPUT_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_LINE_BYTES: usize = 64 * 1024;
const MAX_DEPTH: usize = 64;
const OPEN: char = '\u{e000}';
const CLOSE: char = '\u{e001}';

pub type Result<T> = io::Result<T>;

fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CharacterSet {
    identifiers: HashSet<String>,
}

impl CharacterSet {
    /// Use a file stem, never display_name or nicknames. Collisions need an
    /// explicit authoritative directory identifier, not an invented alias.
    pub fn insert_filename(&mut self, path: &str) -> Result<()> {
        let filename = path.rsplit(['/', '\\']).next().unwrap_or("");
        let stem = filename
            .strip_suffix(".dch")
            .ok_or_else(|| invalid("character filename must end with .dch"))?;
        self.insert_identifier(stem)
    }

    /// For callers that have independently established directory overrides.
    pub fn insert_identifier(&mut self, identifier: &str) -> Result<()> {
        if identifier.is_empty()
            || identifier.len() > MAX_LINE_BYTES
            || identifier.chars().any(char::is_control)
            || identifier.contains(['"', ':', '(', ')', OPEN, CLOSE])
        {
            return Err(invalid("unsupported character identifier"));
        }
        if !self.identifiers.insert(identifier.to_owned()) {
            return Err(invalid(
                "duplicate character identifier; directory authority required",
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SpanKind {
    Text,
    TextContinued,
    Choice,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct SpanId {
    pub line: usize,
    pub kind: SpanKind,
    pub ordinal: usize,
    pub translation_id: Option<String>,
}

/// A refused event, never also emitted as a translatable span.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Unsupported {
    pub line: usize,
    pub reason: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Span {
    pub id: SpanId,
    pub range: Range<usize>,
    /// Translator text with immutable placeholders; see module policy.
    pub visible_text: String,
    pub kind: SpanKind,
    pub speaker: Option<String>,
    pieces: Vec<Piece>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Piece {
    range: Range<usize>,
    template: String,
    tokens: Vec<(String, String)>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Timeline {
    pub spans: Vec<Span>,
    pub diagnostics: Vec<Unsupported>,
    original: String,
    characters: CharacterSet,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CharacterInfo {
    pub display_name: Option<String>,
    pub nicknames: Vec<String>,
    /// Explicitly records that character names are metadata only in GD-3.
    pub diagnostics: Vec<Unsupported>,
}

struct Line {
    start: usize,
    end: usize,
}

fn lines(text: &str) -> Result<Vec<Line>> {
    if text.len() > MAX_INPUT_BYTES {
        return Err(invalid("Dialogic input exceeds 4 MiB"));
    }
    if text.starts_with('\u{feff}') {
        return Err(invalid("Dialogic BOM is outside the supported profile"));
    }
    let mut result = Vec::new();
    let mut start = 0;
    for raw in text.split_inclusive('\n') {
        let body = raw.strip_suffix('\n').unwrap_or(raw);
        let body = if raw.ends_with('\n') {
            body.strip_suffix('\r').unwrap_or(body)
        } else {
            body
        };
        if body.len() > MAX_LINE_BYTES {
            return Err(invalid("Dialogic physical line exceeds 64 KiB"));
        }
        if body.contains('\r') {
            return Err(invalid("bare CR is outside the supported Dialogic profile"));
        }
        result.push(Line {
            start,
            end: start + body.len(),
        });
        start += raw.len();
    }
    Ok(result)
}

fn diagnostic(line: usize, reason: impl Into<String>) -> Unsupported {
    Unsupported {
        line,
        reason: reason.into(),
    }
}

const SHORTCODES: &[&str] = &[
    "music",
    "sound",
    "background",
    "clear",
    "end_timeline",
    "history",
    "save",
    "signal",
    "style",
    "text_input",
    "voice",
    "wait",
    "wait_input",
];

fn shortcode(text: &str) -> Option<&'static str> {
    SHORTCODES.iter().copied().find(|name| {
        text.strip_prefix('[')
            .and_then(|s| s.strip_prefix(name))
            .is_some_and(|s| s.starts_with([' ', ']']))
    })
}

fn condition(text: &str) -> bool {
    matches!(text, "if" | "elif")
        || text.starts_with("if ")
        || text.starts_with("elif ")
        || text.starts_with("else")
}

fn command(text: &str) -> bool {
    [
        "join",
        "leave",
        "update",
        "do",
        "jump",
        "label",
        "set",
        "setting",
        "<<END BRANCH>>",
    ]
    .iter()
    .any(|prefix| text.starts_with(prefix))
        || text == "return"
        || text.starts_with('#')
}

fn inline_prefix(text: &str) -> bool {
    let name = text
        .trim_start_matches('[')
        .split(['=', ']', ' '])
        .next()
        .unwrap_or("");
    matches!(
        name.trim_start_matches('/'),
        "b" | "i"
            | "u"
            | "s"
            | "color"
            | "bgcolor"
            | "font"
            | "font_size"
            | "outline_size"
            | "outline_color"
            | "url"
            | "img"
            | "center"
            | "left"
            | "right"
            | "fill"
            | "indent"
            | "table"
            | "cell"
            | "ul"
            | "ol"
            | "li"
            | "code"
            | "p"
            | "br"
            | "lb"
            | "rb"
            | "wave"
            | "tornado"
            | "shake"
            | "fade"
            | "rainbow"
            | "pulse"
            | "n"
            | "n+"
            | "pause"
            | "speed"
            | "signal"
            | "portrait"
            | "extra_data"
    )
}

/// Return (bytes of prefix, speaker). Reject ambiguous regex spellings rather
/// than silently translating an identifier. This is a subset of Text's regex.
fn speaker_prefix(text: &str) -> std::result::Result<(usize, Option<String>), &'static str> {
    let bytes = text.as_bytes();
    let mut quote = false;
    let mut depth = 0usize;
    let mut colon = None;
    for (i, &byte) in bytes.iter().enumerate() {
        if byte == b'"' && (i == 0 || bytes[i - 1] != b'\\') {
            quote = !quote;
        }
        if quote {
            continue;
        }
        match byte {
            b'[' | b'{' => depth += 1,
            b']' | b'}' => depth = depth.saturating_sub(1),
            b':' if depth == 0 && (i == 0 || bytes[i - 1] != b'\\') => {
                colon = Some(i);
                break;
            }
            _ => {}
        }
    }
    let Some(colon) = colon else {
        return Ok((0, None));
    };
    let prefix = text[..colon].trim();
    let (name, rest) = if let Some(quoted) = prefix.strip_prefix('"') {
        let end = quoted.find('"').ok_or("ambiguous speaker prefix")?;
        (&quoted[..end], quoted[end + 1..].trim())
    } else {
        let end = prefix.find('(').unwrap_or(prefix.len());
        let name = prefix[..end].trim();
        if name.contains(char::is_whitespace) {
            // Text's unquoted name cannot contain spaces. Multiword prose with
            // a colon is ordinary text, but exotic portrait syntax is refused.
            if end < prefix.len() {
                return Err("ambiguous speaker prefix");
            }
            return Ok((0, None));
        }
        (name, prefix[end..].trim())
    };
    let portrait_valid = rest.is_empty()
        || (rest.starts_with('(')
            && rest.ends_with(')')
            && !rest[1..rest.len() - 1].contains(['(', ')']));
    if name.is_empty() || name.contains(['"', ')', '\\']) || !portrait_valid {
        return Err("ambiguous speaker prefix");
    }
    Ok((colon + 1, Some(name.to_owned())))
}

/// Balanced lexical token, supporting nested brackets/braces and quoted args.
fn token_end(text: &str, start: usize) -> std::result::Result<usize, &'static str> {
    let bytes = text.as_bytes();
    let mut stack = Vec::new();
    let mut quote = false;
    let mut escaped = false;
    for (i, &byte) in bytes.iter().enumerate().skip(start) {
        if escaped {
            escaped = false;
            continue;
        }
        if byte == b'\\' {
            escaped = true;
            continue;
        }
        if byte == b'"' {
            quote = !quote;
            continue;
        }
        if quote {
            continue;
        }
        match byte {
            b'[' => stack.push(b']'),
            b'{' => stack.push(b'}'),
            b']' | b'}' => {
                if stack.pop() != Some(byte) {
                    return Err("unbalanced protected delimiter");
                }
                if stack.is_empty() {
                    return Ok(i + 1);
                }
            }
            _ => {}
        }
        if stack.len() > MAX_DEPTH {
            return Err("protected delimiter nesting exceeds 64");
        }
    }
    Err("unbalanced protected delimiter")
}

fn piece(
    text: &str,
    range: Range<usize>,
    ordinal: &mut usize,
) -> std::result::Result<Piece, &'static str> {
    let source = &text[range.clone()];
    if source.is_empty() {
        return Err("empty text segment");
    }
    let mut template = String::new();
    let mut tokens = Vec::new();
    let mut at = 0;
    while at < source.len() {
        let ch = source[at..].chars().next().expect("character boundary");
        let end = match ch {
            '[' | '{' => Some(token_end(source, at)?),
            '\\' => {
                if !source[at + 1..].starts_with([':', '|']) {
                    return Err("unsupported text escape");
                }
                Some(at + 2)
            }
            ']' | '}' => return Err("unbalanced protected delimiter"),
            OPEN | CLOSE | '\u{feff}' => return Err("reserved placeholder or BOM in text"),
            c if c.is_control() => return Err("control character in text"),
            _ => None,
        };
        if let Some(end) = end {
            let marker = format!("{OPEN}{ordinal}{CLOSE}");
            *ordinal += 1;
            template.push_str(&marker);
            tokens.push((marker, source[at..end].to_owned()));
            at = end;
        } else {
            template.push(ch);
            at += ch.len_utf8();
        }
    }
    Ok(Piece {
        range,
        template,
        tokens,
    })
}

fn trimmed_range(text: &str, range: Range<usize>) -> Range<usize> {
    let raw = &text[range.clone()];
    let start = range.start + raw.len() - raw.trim_start().len();
    start..start + raw.trim().len()
}

/// The supported choice label ends at an unescaped pipe, or `[if` without one.
fn choice_end(text: &str) -> std::result::Result<usize, &'static str> {
    if !text.starts_with("- ") {
        return Err("unsupported choice spelling");
    }
    let mut at = 2;
    let mut boundary = text.len();
    while at < text.len() {
        if text[at..].starts_with("\\|") {
            at += 2;
            continue;
        }
        if text[at..].starts_with('|') {
            boundary = at;
            break;
        }
        if text[at..].starts_with("[if") {
            boundary = at;
            break;
        }
        if text[at..].starts_with('[') || text[at..].starts_with('{') {
            let end = token_end(text, at)?;
            // Choice's regex treats pipes as delimiters even inside BBCode or
            // variables. Do not claim such a token is visible label text.
            if text[at..end].replace("\\|", "").contains('|') || text[at + 1..end].contains("[if") {
                return Err("choice delimiter inside protected token");
            }
            at = end;
        } else {
            at += text[at..]
                .chars()
                .next()
                .expect("character boundary")
                .len_utf8();
        }
    }
    let mut suffix = text[boundary..].trim_start_matches('|').trim();
    if suffix.contains("alt_text") {
        return Err("choice alt_text is unsupported");
    }
    let mut brackets = 0;
    while !suffix.is_empty() {
        if !suffix.starts_with('[') || brackets >= 2 {
            return Err("unsupported choice suffix");
        }
        let end = token_end(suffix, 0)?;
        if brackets == 0 && suffix.starts_with("[if") {
            if !suffix.starts_with("[if ") {
                return Err("unsupported choice condition");
            }
        } else if !suffix[..end].contains('=') {
            return Err("unsupported choice parameters");
        }
        brackets += 1;
        suffix = suffix[end..].trim();
    }
    Ok(boundary)
}

fn text_span(
    original: &str,
    physical: &[Line],
    first: usize,
    last: usize,
    choice: bool,
    characters: &CharacterSet,
) -> std::result::Result<Span, &'static str> {
    let mut ranges = Vec::new();
    let mut speaker = None;
    let mut translation_id = None;
    for (index, line) in physical.iter().enumerate().take(last + 1).skip(first) {
        let mut range = trimmed_range(original, line.start..line.end);
        if index < last {
            // Continuation is based on the exact physical suffix, not trimming.
            range.end = line.end - 1;
        }
        let body = &original[range.clone()];
        if let Some(id_at) = body.find("#id:") {
            if index != last || body[id_at + 4..].contains("#id:") {
                return Err("ambiguous or early #id tag");
            }
            let id = body[id_at + 4..].trim();
            if id.is_empty()
                || id.chars().any(char::is_whitespace)
                || id.contains(['\\', '[', ']', '{', '}'])
            {
                return Err("unsupported #id tag");
            }
            translation_id = Some(id.to_owned());
            range.end = range.start + id_at;
        }
        if index == first {
            let body = &original[range.clone()];
            if choice {
                range.end = range.start + choice_end(body)?;
                range.start += 2;
            } else {
                if body.starts_with('\\') {
                    return Err("escaped event prefix is unsupported");
                }
                if body.starts_with('[') && !inline_prefix(body) {
                    return Err("unknown bracket prefix or custom event");
                }
                let (prefix, name) = speaker_prefix(body)?;
                if let Some(name) = &name {
                    if name != "_" && !characters.identifiers.contains(name) {
                        return Err("unknown speaker identifier");
                    }
                }
                speaker = name;
                range.start += prefix;
            }
        }
        range = trimmed_range(original, range);
        if &original[range.clone()] == "<Empty Text Event>" {
            return Err("empty text event sentinel is unsupported");
        }
        ranges.push(range);
    }
    let kind = if choice {
        SpanKind::Choice
    } else if first != last {
        SpanKind::TextContinued
    } else {
        SpanKind::Text
    };
    let mut ordinal = 0;
    let pieces = ranges
        .into_iter()
        .map(|r| piece(original, r, &mut ordinal))
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let visible_text = pieces
        .iter()
        .map(|p| p.template.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    Ok(Span {
        id: SpanId {
            line: first,
            kind,
            ordinal: 0,
            translation_id,
        },
        range: pieces[0].range.start..pieces.last().expect("nonempty pieces").range.end,
        visible_text,
        kind,
        speaker,
        pieces,
    })
}

pub fn parse_timeline(text: &str, characters: &CharacterSet) -> Result<Timeline> {
    let physical = lines(text)?;
    let mut timeline = Timeline {
        spans: Vec::new(),
        diagnostics: Vec::new(),
        original: text.to_owned(),
        characters: characters.clone(),
    };
    let mut at = 0;
    while at < physical.len() {
        let first = at;
        let content = text[physical[at].start..physical[at].end].trim();
        if content.is_empty() {
            at += 1;
            continue;
        }
        if let Some(name) = shortcode(content) {
            // The primary shortcode loader can consume multiple physical lines.
            while !text[physical[at].start..physical[at].end]
                .trim()
                .ends_with(']')
                && at + 1 < physical.len()
                && !text[physical[at + 1].start..physical[at + 1].end]
                    .trim()
                    .is_empty()
            {
                at += 1;
            }
            if name == "text_input" {
                timeline.diagnostics.push(diagnostic(
                    first,
                    "text_input translatable properties are unsupported",
                ));
            } else if !text[physical[at].start..physical[at].end]
                .trim()
                .ends_with(']')
            {
                timeline
                    .diagnostics
                    .push(diagnostic(first, "incomplete shortcode event"));
            }
            at += 1;
            continue;
        }
        if condition(content) {
            timeline
                .diagnostics
                .push(diagnostic(first, "condition event (not evaluated)"));
            at += 1;
            continue;
        }
        if command(content) {
            if content.starts_with("label") {
                timeline
                    .diagnostics
                    .push(diagnostic(first, "label display_name is unsupported"));
            }
            at += 1;
            continue;
        }
        if content.starts_with('[') && !inline_prefix(content) {
            // Custom indexers are unknown. Conservatively quarantine an
            // incomplete bracket event and its nonblank body as one diagnostic.
            while !text[physical[at].start..physical[at].end]
                .trim()
                .ends_with(']')
                && at + 1 < physical.len()
                && !text[physical[at + 1].start..physical[at + 1].end]
                    .trim()
                    .is_empty()
            {
                at += 1;
            }
            timeline
                .diagnostics
                .push(diagnostic(first, "unknown bracket prefix or custom event"));
            at += 1;
            continue;
        }
        let choice = content.starts_with('-');
        let mut interrupted = false;
        if !choice {
            while text[physical[at].start..physical[at].end].ends_with('\\') {
                if at + 1 == physical.len()
                    || text[physical[at + 1].start..physical[at + 1].end]
                        .trim()
                        .is_empty()
                {
                    interrupted = true;
                    break;
                }
                at += 1;
            }
        }
        if interrupted {
            timeline.diagnostics.push(diagnostic(
                first,
                "dangling or blank-interrupted continuation",
            ));
        } else {
            match text_span(text, &physical, first, at, choice, characters) {
                Ok(span) => timeline.spans.push(span),
                Err(reason) => timeline.diagnostics.push(diagnostic(first, reason)),
            }
        }
        at += 1;
    }
    Ok(timeline)
}

fn decode_piece(piece: &Piece, translation: &str) -> Result<String> {
    if translation.is_empty() || translation.trim() != translation {
        return Err(invalid(
            "translation has empty segment or altered edge whitespace",
        ));
    }
    let mut rest = translation;
    let mut output = String::new();
    let mut tokens = piece.tokens.iter();
    loop {
        let index = rest.find(OPEN).unwrap_or(rest.len());
        let literal = &rest[..index];
        if literal.contains(['[', ']', '{', '}', '\\', CLOSE, '\u{feff}'])
            || literal.contains("#id:")
            || literal.chars().any(char::is_control)
        {
            return Err(invalid(
                "translation alters protected syntax or contains control characters",
            ));
        }
        output.push_str(literal);
        rest = &rest[index..];
        if rest.is_empty() {
            break;
        }
        let (marker, raw) = tokens
            .next()
            .ok_or_else(|| invalid("unexpected protected placeholder"))?;
        rest = rest
            .strip_prefix(marker)
            .ok_or_else(|| invalid("altered or reordered protected placeholder"))?;
        output.push_str(raw);
    }
    if tokens.next().is_some() {
        return Err(invalid("missing protected placeholder"));
    }
    Ok(output)
}

pub fn rebuild(
    original: &str,
    timeline: &Timeline,
    translations: &HashMap<SpanId, String>,
) -> Result<String> {
    if timeline.original != original {
        return Err(invalid("timeline does not match original bytes"));
    }
    let canonical = parse_timeline(original, &timeline.characters)?;
    if &canonical != timeline {
        return Err(invalid("timeline metadata was modified"));
    }
    let ids = timeline.spans.iter().map(|s| &s.id).collect::<HashSet<_>>();
    if translations.keys().any(|id| !ids.contains(id)) {
        return Err(invalid("translation references unknown SpanId"));
    }
    let mut output = String::new();
    let mut cursor = 0;
    for span in &timeline.spans {
        let Some(translation) = translations.get(&span.id) else {
            continue;
        };
        if translation.len() > MAX_INPUT_BYTES {
            return Err(invalid("translation exceeds 4 MiB"));
        }
        let segments = translation.split('\n').collect::<Vec<_>>();
        if segments.len() != span.pieces.len() {
            return Err(invalid("translation changes continuation segment count"));
        }
        for (index, (piece, segment)) in span.pieces.iter().zip(segments).enumerate() {
            let decoded = decode_piece(piece, segment)?;
            if index == 0 && span.kind != SpanKind::Choice {
                if command(&decoded)
                    || condition(&decoded)
                    || shortcode(&decoded).is_some()
                    || decoded.starts_with('-')
                {
                    return Err(invalid(
                        "translation introduces a command, condition or choice",
                    ));
                }
                if speaker_prefix(&decoded).map_err(invalid)?.1.is_some() {
                    return Err(invalid("translation introduces a speaker prefix"));
                }
            }
            let added = piece.range.start - cursor + decoded.len();
            if output.len() + added > MAX_INPUT_BYTES {
                return Err(invalid("rebuilt timeline exceeds 4 MiB"));
            }
            output.push_str(&original[cursor..piece.range.start]);
            output.push_str(&decoded);
            cursor = piece.range.end;
        }
    }
    if output.len() + original.len() - cursor > MAX_INPUT_BYTES {
        return Err(invalid("rebuilt timeline exceeds 4 MiB"));
    }
    output.push_str(&original[cursor..]);
    let reparsed = parse_timeline(&output, &timeline.characters)?;
    // Diagnostics check is defense in depth: new diagnostics also reduce the span count.
    if reparsed.diagnostics != timeline.diagnostics
        || reparsed.spans.len() != timeline.spans.len()
        || reparsed.spans.iter().zip(&timeline.spans).any(|(a, b)| {
            a.id != b.id
                || a.speaker != b.speaker
                || a.visible_text != *translations.get(&b.id).unwrap_or(&b.visible_text)
        })
    {
        return Err(invalid("translation changes timeline event structure"));
    }
    Ok(output)
}

struct Scanner<'a> {
    text: &'a str,
    at: usize,
}

impl Scanner<'_> {
    fn whitespace(&mut self) {
        while self.peek().is_some_and(u8::is_ascii_whitespace) {
            self.at += 1;
        }
    }
    fn peek(&self) -> Option<&u8> {
        self.text.as_bytes().get(self.at)
    }
    fn take(&mut self, expected: u8) -> Result<()> {
        self.whitespace();
        if self.peek() != Some(&expected) {
            return Err(invalid("malformed character dictionary"));
        }
        self.at += 1;
        Ok(())
    }
    fn string_end(&mut self) -> Result<Range<usize>> {
        self.whitespace();
        let start = self.at;
        self.take(b'"')?;
        let mut escaped = false;
        while let Some(&byte) = self.peek() {
            self.at += 1;
            if escaped {
                escaped = false;
                continue;
            }
            if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                return Ok(start..self.at);
            } else if byte < 32 {
                return Err(invalid("control character in Variant string"));
            }
        }
        Err(invalid("unterminated Variant string"))
    }
    fn string(&mut self) -> Result<String> {
        let range = self.string_end()?;
        serde_json::from_str(&self.text[range])
            .map_err(|_| invalid("unsupported character string escape"))
    }
    fn skip_value(&mut self) -> Result<()> {
        self.whitespace();
        let start = self.at;
        let mut stack = Vec::new();
        while let Some(&byte) = self.peek() {
            if stack.is_empty() && matches!(byte, b',' | b'}') {
                break;
            }
            if byte == b'"' {
                self.string_end()?;
                continue;
            }
            match byte {
                b'[' => stack.push(b']'),
                b'{' => stack.push(b'}'),
                b'(' => stack.push(b')'),
                b']' | b'}' | b')' => {
                    if stack.pop() != Some(byte) {
                        return Err(invalid("unbalanced opaque Variant value"));
                    }
                }
                _ => {}
            }
            if stack.len() > MAX_DEPTH {
                return Err(invalid("Variant nesting exceeds 64"));
            }
            self.at += 1;
        }
        if !stack.is_empty() || self.text[start..self.at].trim().is_empty() {
            return Err(invalid("incomplete opaque Variant value"));
        }
        Ok(())
    }
}

pub fn parse_character(text: &str) -> Result<CharacterInfo> {
    lines(text)?;
    let mut scanner = Scanner { text, at: 0 };
    scanner.take(b'{')?;
    let mut display_name = None;
    let mut nicknames = Vec::new();
    let mut seen = HashSet::new();
    loop {
        scanner.whitespace();
        if scanner.peek() == Some(&b'}') {
            scanner.at += 1;
            break;
        }
        let key = scanner.string()?;
        scanner.take(b':')?;
        if matches!(key.as_str(), "display_name" | "nicknames") && !seen.insert(key.clone()) {
            return Err(invalid("duplicate character identity field"));
        }
        match key.as_str() {
            "display_name" => display_name = Some(scanner.string()?),
            "nicknames" => {
                scanner.take(b'[')?;
                scanner.whitespace();
                while scanner.peek() != Some(&b']') {
                    nicknames.push(scanner.string()?);
                    scanner.whitespace();
                    if scanner.peek() != Some(&b',') {
                        break;
                    }
                    scanner.at += 1;
                    scanner.whitespace();
                }
                scanner.take(b']')?;
            }
            _ => scanner.skip_value()?,
        }
        scanner.whitespace();
        if scanner.peek() == Some(&b'}') {
            scanner.at += 1;
            break;
        }
        scanner.take(b',')?;
    }
    scanner.whitespace();
    if scanner.at != text.len() {
        return Err(invalid("trailing data after character dictionary"));
    }
    Ok(CharacterInfo {
        display_name,
        nicknames,
        diagnostics: vec![diagnostic(
            0,
            "character names are metadata only (not emitted as spans)",
        )],
    })
}
