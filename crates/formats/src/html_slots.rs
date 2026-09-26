//! Byte ranges for editable HTML values. The original document is never
//! reserialized: scripts, handlers, comments and inline formatting stay intact.
use std::ops::Range;

pub(super) struct Slot {
    pub range: Range<usize>,
    pub source: String,
    pub kind: String,
}

fn tag_end(bytes: &[u8], mut pos: usize) -> Option<usize> {
    let mut quote = None;
    while pos < bytes.len() {
        match (quote, bytes[pos]) {
            (Some(q), b) if q == b => quote = None,
            (None, b'\'' | b'"') => quote = Some(bytes[pos]),
            (None, b'>') => return Some(pos + 1),
            _ => {}
        }
        pos += 1;
    }
    None
}

fn push_slot(content: &str, range: Range<usize>, kind: String, slots: &mut Vec<Slot>) {
    let raw = &content[range.clone()];
    let start = range.start + raw.len() - raw.trim_start().len();
    let end = range.end - (raw.len() - raw.trim_end().len());
    if start >= end {
        return;
    }
    let source = decode_entities_in(&content[start..end], kind.starts_with("attr:"));
    if super::is_translatable_text(&source) {
        slots.push(Slot {
            range: start..end,
            source,
            kind,
        });
    }
}

pub(super) fn scan(content: &str) -> Vec<Slot> {
    let bytes = content.as_bytes();
    let lower = content.to_ascii_lowercase();
    let mut pos = 0;
    let mut stack: Vec<String> = Vec::new();
    let mut slots = Vec::new();
    while pos < bytes.len() {
        if bytes[pos] != b'<' {
            let end = content[pos..].find('<').map_or(bytes.len(), |n| pos + n);
            let suppressed = stack.iter().any(|t| super::is_skip_tag(t));
            let in_head = stack.iter().any(|t| t == "head");
            if !suppressed && (!in_head || stack.last().is_some_and(|t| t == "title")) {
                let kind = format!("text:{}", stack.last().map_or("body", String::as_str));
                push_slot(content, pos..end, kind, &mut slots);
            }
            pos = end;
            continue;
        }
        if content[pos..].starts_with("<!--") {
            pos = content[pos + 4..]
                .find("-->")
                .map_or(bytes.len(), |n| pos + 4 + n + 3);
            continue;
        }
        // Invalid/malformed angle brackets are left untouched.
        let Some(end) = tag_end(bytes, pos + 1) else {
            break;
        };
        let closing = bytes.get(pos + 1) == Some(&b'/');
        let mut name_start = pos + 1 + usize::from(closing);
        while name_start < end && bytes[name_start].is_ascii_whitespace() {
            name_start += 1;
        }
        let mut name_end = name_start;
        while name_end < end
            && (bytes[name_end].is_ascii_alphanumeric() || matches!(bytes[name_end], b'-' | b':'))
        {
            name_end += 1;
        }
        if name_end == name_start {
            pos = end;
            continue;
        }
        let name = lower[name_start..name_end].to_string();
        if closing {
            if let Some(index) = stack.iter().rposition(|tag| tag == &name) {
                stack.truncate(index);
            }
            pos = end;
            continue;
        }
        if matches!(
            name.as_str(),
            "script" | "style" | "textarea" | "xmp" | "iframe"
        ) {
            let needle = format!("</{name}");
            let mut search = end;
            let mut close = None;
            while let Some(found) = lower[search..].find(&needle) {
                let index = search + found;
                let boundary = index + needle.len();
                if bytes
                    .get(boundary)
                    .is_some_and(|b| b.is_ascii_whitespace() || *b == b'>')
                {
                    close = tag_end(bytes, boundary);
                    break;
                }
                search = boundary;
            }
            pos = close.unwrap_or(bytes.len());
            continue;
        }
        let suppressed =
            super::is_skip_tag(&name) || stack.iter().any(|tag| super::is_skip_tag(tag));
        if !suppressed {
            let mut attr_pos = name_end;
            while attr_pos < end - 1 {
                while attr_pos < end - 1
                    && (bytes[attr_pos].is_ascii_whitespace() || bytes[attr_pos] == b'/')
                {
                    attr_pos += 1;
                }
                let begin = attr_pos;
                while attr_pos < end - 1
                    && !bytes[attr_pos].is_ascii_whitespace()
                    && !matches!(bytes[attr_pos], b'=' | b'>' | b'/')
                {
                    attr_pos += 1;
                }
                if begin == attr_pos {
                    attr_pos += 1;
                    continue;
                }
                let attr = &lower[begin..attr_pos];
                while attr_pos < end - 1 && bytes[attr_pos].is_ascii_whitespace() {
                    attr_pos += 1;
                }
                if bytes.get(attr_pos) != Some(&b'=') {
                    continue;
                }
                attr_pos += 1;
                while attr_pos < end - 1 && bytes[attr_pos].is_ascii_whitespace() {
                    attr_pos += 1;
                }
                let quote = bytes
                    .get(attr_pos)
                    .copied()
                    .filter(|b| matches!(b, b'"' | b'\''));
                if quote.is_some() {
                    attr_pos += 1;
                }
                let start = attr_pos;
                while attr_pos < end - 1
                    && match quote {
                        Some(q) => bytes[attr_pos] != q,
                        None => !bytes[attr_pos].is_ascii_whitespace() && bytes[attr_pos] != b'>',
                    }
                {
                    attr_pos += 1;
                }
                // Only quoted values are editable: inserting spaces in an
                // unquoted attribute could create additional attributes.
                if quote.is_some()
                    && matches!(
                        attr,
                        "alt" | "title" | "placeholder" | "aria-label" | "label"
                    )
                {
                    push_slot(content, start..attr_pos, format!("attr:{attr}"), &mut slots);
                }
                if quote.is_some() {
                    attr_pos += 1;
                }
            }
        }
        let self_closing_foreign =
            matches!(name.as_str(), "svg" | "math") && content[pos..end].ends_with("/>");
        let void_element = matches!(
            name.as_str(),
            "area"
                | "base"
                | "br"
                | "col"
                | "embed"
                | "hr"
                | "img"
                | "input"
                | "link"
                | "meta"
                | "param"
                | "source"
                | "track"
                | "wbr"
        );
        if !self_closing_foreign && !void_element {
            stack.push(name);
        }
        pos = end;
    }
    slots
}

#[cfg(test)]
pub(super) fn decode_entities(value: &str) -> String {
    decode_entities_in(value, false)
}

fn decode_entities_in(value: &str, attribute: bool) -> String {
    let mut out = String::with_capacity(value.len());
    let mut pos = 0;
    while let Some(offset) = value[pos..].find('&') {
        let start = pos + offset;
        out.push_str(&value[pos..start]);
        let rest = &value[start + 1..];
        let mut decoded = None;
        if let Some(number) = rest.strip_prefix('#') {
            let (digits, radix, prefix) = if number.starts_with(['x', 'X']) {
                (&number[1..], 16, 2)
            } else {
                (number, 10, 1)
            };
            let count = digits
                .bytes()
                .take_while(|b| {
                    if radix == 16 {
                        b.is_ascii_hexdigit()
                    } else {
                        b.is_ascii_digit()
                    }
                })
                .count();
            if count > 0 {
                let code = u32::from_str_radix(&digits[..count], radix).unwrap_or(0xfffd);
                let mut ch = if (0x80..=0x9f).contains(&code) {
                    markup5ever::data::C1_REPLACEMENTS[(code - 0x80) as usize]
                        .or_else(|| char::from_u32(code))
                        .unwrap()
                } else {
                    char::from_u32(code).unwrap_or('\u{fffd}')
                };
                if ch == '\0' {
                    ch = '\u{fffd}';
                }
                let consumed =
                    prefix + count + usize::from(digits.as_bytes().get(count) == Some(&b';'));
                decoded = Some((ch.to_string(), consumed));
            }
        } else {
            let length = rest
                .bytes()
                .take(33)
                .take_while(|b| b.is_ascii_alphanumeric() || *b == b';')
                .count();
            for end in (1..=length).rev() {
                let name = &rest[..end];
                if let Some(&(first, second)) = markup5ever::data::NAMED_ENTITIES.get(name) {
                    if first == 0 {
                        continue;
                    }
                    if !name.ends_with(';')
                        && attribute
                        && rest
                            .as_bytes()
                            .get(end)
                            .is_some_and(|b| b.is_ascii_alphanumeric() || *b == b'=')
                    {
                        continue;
                    }
                    let mut text = char::from_u32(first).unwrap().to_string();
                    if second != 0 {
                        text.push(char::from_u32(second).unwrap());
                    }
                    decoded = Some((text, end));
                    break;
                }
            }
        }
        if let Some((decoded, consumed)) = decoded {
            out.push_str(&decoded);
            pos = start + 1 + consumed;
        } else {
            out.push('&');
            pos = start + 1;
        }
    }
    out.push_str(&value[pos..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn semicolonless_entities_follow_text_and_attribute_rules() {
        assert_eq!(
            decode_entities("Tom &amp Jerry &copy 2026"),
            "Tom & Jerry © 2026"
        );
        assert_eq!(
            decode_entities("Price &#65 and &#x42 next"),
            "Price A and B next"
        );
        assert_eq!(decode_entities("A &notin; B &notin C"), "A ∉ B ¬in C");
        assert_eq!(decode_entities("Use &ampere here"), "Use &ere here");
        assert_eq!(
            decode_entities_in("Use &ampere &copycat &amp= here", true),
            "Use &ampere &copycat &amp= here"
        );
        assert_eq!(
            decode_entities_in("Tom &amp Jerry &copy 2026 &amp;next", true),
            "Tom & Jerry © 2026 &next"
        );
    }

    #[test]
    fn numeric_entities_apply_html_c1_and_invalid_scalar_replacements() {
        assert_eq!(
            decode_entities("Price &#128; or &#x80 next"),
            "Price € or € next"
        );
        assert_eq!(decode_entities_in("Price &#128", true), "Price €");
        assert_eq!(
            decode_entities("Invalid &#0; &#xD800; &#1114112;"),
            "Invalid � � �"
        );
        assert_eq!(
            decode_entities("Incomplete &#; &#x;"),
            "Incomplete &#; &#x;"
        );
    }

    #[test]
    fn self_closing_foreign_roots_do_not_hide_following_html_text() {
        for prefix in [
            "<svg/>",
            "<svg />",
            "<SVG aria-label='icon'/>",
            "<math/>",
            "<svg><path /></svg>",
        ] {
            let html = format!("{prefix}<p>Text after the icon</p>");
            let slots = scan(&html);
            assert_eq!(slots.len(), 1, "{html}");
            assert_eq!(slots[0].source, "Text after the icon", "{html}");
            assert_eq!(&html[slots[0].range.clone()], "Text after the icon");
        }
    }
}
