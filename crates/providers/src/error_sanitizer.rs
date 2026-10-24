const REDACTED: &str = "[REDACTED]";
const MAX_ERROR_CHARS: usize = 500;

// Scan JSON string boundaries rather than treating escaped quotes or whitespace
// as delimiters. Error bodies may contain JSON embedded in ordinary prose.
fn json_string_end(body: &str, start: usize) -> usize {
    let bytes = body.as_bytes();
    let mut cursor = start + 1;
    while cursor < bytes.len() {
        match bytes[cursor] {
            b'\\' => cursor += 2,
            b'"' => return cursor + 1,
            _ => cursor += 1,
        }
    }
    bytes.len()
}

fn skip_whitespace(body: &str, mut cursor: usize) -> usize {
    while body
        .as_bytes()
        .get(cursor)
        .is_some_and(u8::is_ascii_whitespace)
    {
        cursor += 1;
    }
    cursor
}

fn json_header_ranges(body: &str, ranges: &mut Vec<(usize, usize)>) {
    let mut cursor = 0;
    while let Some(offset) = body[cursor..].find('"') {
        let start = cursor + offset;
        let end = json_string_end(body, start);
        cursor = end;
        let Ok(key) = serde_json::from_str::<String>(&body[start..end]) else {
            continue;
        };
        if !matches!(
            key.to_ascii_lowercase().as_str(),
            "authorization"
                | "x-api-key"
                | "api-key"
                | "x-goog-api-key"
                | "cookie"
                | "api_key"
                | "key"
                | "access_token"
                | "refresh_token"
                | "token"
        ) {
            continue;
        }
        let colon = skip_whitespace(body, end);
        if body.as_bytes().get(colon) != Some(&b':') {
            continue;
        }
        let mut value = skip_whitespace(body, colon + 1);
        let array = body.as_bytes().get(value) == Some(&b'[');
        if array {
            value = skip_whitespace(body, value + 1);
        }
        while body.as_bytes().get(value) == Some(&b'"') {
            let end = json_string_end(body, value);
            // Keep the JSON delimiters, and remove the complete encoded value.
            let content_end = if body.as_bytes().get(end - 1) == Some(&b'"') {
                end - 1
            } else {
                end
            };
            ranges.push((value + 1, content_end));
            cursor = end;
            if !array {
                break;
            }
            value = skip_whitespace(body, end);
            if body.as_bytes().get(value) != Some(&b',') {
                break;
            }
            value = skip_whitespace(body, value + 1);
        }
    }
}

// JSON escapes remain literal bytes in a serialized diagnostic string.
fn whitespace_end(body: &str, start: usize) -> usize {
    let mut cursor = start;
    loop {
        let next = skip_whitespace(body, cursor);
        if next != cursor {
            cursor = next;
            continue;
        }
        let tail = &body[cursor..];
        if [r"\t", r"\n", r"\r", r"\f", r"\b"]
            .iter()
            .any(|escape| tail.starts_with(escape))
        {
            cursor += 2;
        } else if let Some(hex) = tail.strip_prefix(r"\u").and_then(|s| s.get(..4)) {
            if u32::from_str_radix(hex, 16)
                .ok()
                .and_then(char::from_u32)
                .is_some_and(char::is_whitespace)
            {
                cursor += 6;
            } else {
                break;
            }
        } else {
            break;
        }
    }
    cursor
}

// Decode percent escapes with original byte offsets so partial/mixed-case URL
// encoding cannot hide a configured secret. Also accept form-encoded spaces.
fn percent_secret_ranges(body: &str, secrets: &[&str], ranges: &mut Vec<(usize, usize)>) {
    if !body.contains(['%', '+']) {
        return;
    }
    for form in [false, true] {
        let mut decoded = Vec::with_capacity(body.len());
        let mut offsets = Vec::with_capacity(body.len());
        let mut cursor = 0;
        while cursor < body.len() {
            let start = cursor;
            let byte = body.as_bytes()[cursor];
            let hex = if byte == b'%' {
                body.as_bytes()
                    .get(cursor + 1..cursor + 3)
                    .and_then(|pair| std::str::from_utf8(pair).ok())
                    .and_then(|pair| u8::from_str_radix(pair, 16).ok())
            } else {
                None
            };
            let value = if let Some(value) = hex {
                cursor += 3;
                value
            } else {
                cursor += 1;
                if form && byte == b'+' {
                    b' '
                } else {
                    byte
                }
            };
            decoded.push(value);
            offsets.push((start, cursor));
        }
        for secret in secrets.iter().filter(|s| !s.is_empty()) {
            for (start, window) in decoded.windows(secret.len()).enumerate() {
                if window == secret.as_bytes() {
                    ranges.push((offsets[start].0, offsets[start + secret.len() - 1].1));
                }
            }
        }
    }
}

/// `match_indices` skips overlapping occurrences, so `aXaXaX` with secret
/// `aXaX` would leave a credential fragment behind. Step one character at a time.
fn overlapping_ranges(haystack: &str, needle: &str, ranges: &mut Vec<(usize, usize)>) {
    let mut from = 0;
    while let Some(offset) = haystack[from..].find(needle) {
        let start = from + offset;
        ranges.push((start, start + needle.len()));
        from = start + haystack[start..].chars().next().map_or(1, char::len_utf8);
    }
}

fn configured_secret_ranges(body: &str, secrets: &[&str], ranges: &mut Vec<(usize, usize)>) {
    if secrets.iter().all(|secret| secret.is_empty()) {
        return;
    }
    for secret in secrets.iter().filter(|s| !s.is_empty()) {
        overlapping_ranges(body, secret, ranges);
    }
    percent_secret_ranges(body, secrets, ranges);
    if !body.contains('\\') {
        return;
    }
    // Decode JSON escape units even in prose excerpts without enclosing quotes.
    // Map each decoded byte back to its complete original escape, so Unicode
    // escapes and surrogate pairs cannot leave a partial credential behind.
    let mut decoded = Vec::with_capacity(body.len());
    let mut offsets = Vec::with_capacity(body.len());
    let mut cursor = 0;
    while cursor < body.len() {
        let start = cursor;
        let escape = if body.as_bytes()[cursor] == b'\\' {
            [2, 6, 12].into_iter().find_map(|len| {
                let value = body.get(cursor..cursor + len)?;
                let decoded = serde_json::from_str::<String>(&format!("\"{value}\"")).ok()?;
                Some((len, decoded))
            })
        } else {
            None
        };
        if let Some((len, value)) = escape {
            cursor += len;
            decoded.extend_from_slice(value.as_bytes());
            offsets.extend(std::iter::repeat_n((start, cursor), value.len()));
        } else {
            cursor += 1;
            decoded.push(body.as_bytes()[start]);
            offsets.push((start, cursor));
        }
    }
    let decoded = String::from_utf8(decoded).expect("JSON escapes and original text are UTF-8");
    let mut decoded_ranges = Vec::new();
    for secret in secrets.iter().filter(|s| !s.is_empty()) {
        overlapping_ranges(&decoded, secret, &mut decoded_ranges);
    }
    percent_secret_ranges(&decoded, secrets, &mut decoded_ranges);
    for (start, end) in decoded_ranges {
        ranges.push((offsets[start].0, offsets[end - 1].1));
    }
}

fn credential_end(body: &str, start: usize, query: bool) -> usize {
    let mut cursor = start;
    while let Some(&byte) = body.as_bytes().get(cursor) {
        if byte.is_ascii_whitespace()
            || if query {
                matches!(byte, b'&' | b'#' | b'"' | b'\'' | b'>' | b')')
            } else {
                byte == b'"'
            }
        {
            break;
        }
        // A JSON escape pair belongs to a scheme token, even when it escapes
        // a quote or whitespace. Unicode escape digits are consumed normally.
        cursor += if !query && byte == b'\\' && cursor + 1 < body.len() {
            2
        } else {
            1
        };
    }
    cursor
}

/// Sanitize before truncating so a credential crossing the cutoff cannot leak a
/// prefix. Work on UTF-8 boundaries and count characters, not bytes.
pub(crate) fn sanitize_provider_error(body: &str, secrets: &[&str]) -> String {
    let mut result = redact_provider_error(body, secrets);
    if let Some((cutoff, _)) = result.char_indices().nth(MAX_ERROR_CHARS - 1) {
        if result.chars().count() > MAX_ERROR_CHARS {
            result.truncate(cutoff);
            result.push('…');
        }
    }
    result
}

// CLI diagnostics retain their tail, but must still redact before that cutoff.
pub(crate) fn redact_provider_error(body: &str, secrets: &[&str]) -> String {
    // ASCII case folding preserves byte offsets even in a Unicode response.
    let sanitized = body;
    let lower = body.to_ascii_lowercase();
    let mut ranges = Vec::new();
    configured_secret_ranges(body, secrets, &mut ranges);
    json_header_ranges(body, &mut ranges);
    for label in [
        "bearer",
        "basic",
        "sk-",
        "authorization",
        "x-api-key",
        "x-goog-api-key",
        "cookie",
        "api_key",
        "api-key",
        "key",
        "access_token",
        "refresh_token",
        "client_secret",
        "token",
    ] {
        for (offset, _) in lower.match_indices(label) {
            if offset > 0
                && matches!(lower.as_bytes()[offset - 1], b'a'..=b'z' | b'0'..=b'9' | b'_' | b'-')
            {
                continue;
            }
            let mut start = offset + label.len();
            let mut query = false;
            let scheme_separator = if matches!(label, "bearer" | "basic" | "token") {
                whitespace_end(body, start)
            } else {
                start
            };
            let scheme = scheme_separator > start || label == "sk-";
            if label == "sk-" {
                start = offset;
            } else if scheme_separator > start {
                start = scheme_separator;
            } else if matches!(label, "bearer" | "basic") {
                continue;
            } else {
                // Accept plain headers, query parameters and JSON header maps.
                if matches!(lower.as_bytes().get(start), Some(b'\'' | b'"')) {
                    start += 1;
                }
                start = skip_whitespace(body, start);
                if !matches!(lower.as_bytes().get(start), Some(b':' | b'=')) {
                    continue;
                }
                query = lower.as_bytes()[start] == b'=';
                start = skip_whitespace(body, start + 1);
            }
            let quote = sanitized
                .as_bytes()
                .get(start)
                .copied()
                .filter(|b| matches!(b, b'\'' | b'"'));
            if quote.is_some() {
                start += 1;
            }
            if quote == Some(b'"') {
                let end = json_string_end(body, start - 1);
                let end = if body.as_bytes().get(end - 1) == Some(&b'"') {
                    end - 1
                } else {
                    end
                };
                ranges.push((start, end));
                continue;
            }
            // JSON string arrays were redacted element by element above; keep their
            // structure. A value that only looks like an array (`[secret]`) is not
            // handled there and must still be redacted here.
            if !scheme
                && !query
                && sanitized.as_bytes().get(start) == Some(&b'[')
                && sanitized.as_bytes().get(skip_whitespace(body, start + 1)) == Some(&b'"')
            {
                continue;
            }
            let end = credential_end(body, start, query);
            if end > start {
                ranges.push((start, end));
            }
        }
    }
    ranges.sort_unstable();
    let mut result = String::new();
    let mut cursor = 0;
    for (start, end) in ranges {
        if end <= cursor {
            continue;
        }
        if start >= cursor {
            result.push_str(&sanitized[cursor..start]);
            result.push_str(REDACTED);
        }
        cursor = end;
    }
    result.push_str(&sanitized[cursor..]);

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bracketed_header_value_that_is_not_a_json_array_is_redacted() {
        let clean = sanitize_provider_error("401 X-Api-Key: [bracketheadersentinel] rest", &[]);
        assert!(!clean.contains("bracketheadersentinel"), "{clean}");
        let clean = sanitize_provider_error(r#"{"X-Api-Key":["array-header-sentinel"]}"#, &[]);
        assert!(!clean.contains("array-header-sentinel"), "{clean}");
    }

    #[test]
    fn overlapping_configured_secret_occurrences_leave_no_fragment() {
        let secret = "qzvkmnrpqzvkmnrp";
        let clean = sanitize_provider_error("echo qzvkmnrpqzvkmnrpqzvkmnrp end", &[secret]);
        assert!(!clean.contains("qzvkmnrp"), "{clean}");
        // The third copy is JSON-escaped (q is 'q') and overlaps the first two.
        let escaped = r#"{"m":"qzvkmnrpqzvkmnrpqzvkmnrp"}"#;
        let clean = sanitize_provider_error(escaped, &[secret]);
        assert!(
            !clean.contains("qzvkmnrp") && !clean.contains("zvkmnrp"),
            "{clean}"
        );
    }

    #[test]
    fn redacts_configured_values_and_common_secret_shapes() {
        let body = r#"opaque-value opaque-value-long Authorization: bEaReR bearer-value
X-API-KEY: header-value
{"x-api-key": "json header value", "api_key": "json-value", "access_token":"access-value"}
https://example.test?key=query-value&safe=yes sk-proj-token-value"#;
        let clean = sanitize_provider_error(body, &["", "opaque-value", "opaque-value-long"]);
        for secret in [
            "opaque-value",
            "-long",
            "bearer-value",
            "header-value",
            "json header value",
            "json-value",
            "access-value",
            "query-value",
            "sk-proj-token-value",
        ] {
            assert!(!clean.contains(secret), "leaked {secret}: {clean}");
        }
        assert!(clean.contains("&safe=yes"));
        assert!(clean.contains(REDACTED));
    }

    #[test]
    fn truncates_unicode_after_redaction_including_secrets_crossing_cutoff() {
        let body = format!("{}cutoff-credential{}", "日".repeat(495), "日".repeat(30));
        let clean = sanitize_provider_error(&body, &["cutoff-credential"]);
        assert_eq!(clean.chars().count(), MAX_ERROR_CHARS);
        assert!(clean.ends_with('…'));
        assert!(!clean.contains("cuto"));
        assert_eq!(
            sanitize_provider_error(&"日".repeat(500), &[]),
            "日".repeat(500)
        );
        assert_eq!(
            sanitize_provider_error("Bad request: invalid model", &[]),
            "Bad request: invalid model"
        );
    }

    #[test]
    fn json_headers_preserve_structure_and_consume_escaped_values() {
        for header in [
            "AUTHORIZATION",
            "x-api-key",
            "Api-Key",
            "x-goog-api-key",
            "Cookie",
        ] {
            let body = format!(
                r#"{{"{header}":["array-sentinel", "escaped\"quote-sentinel", "line\ttail-sentinel"], "safe":"diagnostic"}}"#
            );
            let clean = sanitize_provider_error(&body, &[]);
            for secret in ["array-sentinel", "quote-sentinel", "tail-sentinel"] {
                assert!(!clean.contains(secret), "{clean}");
            }
            let parsed: serde_json::Value = serde_json::from_str(&clean).unwrap();
            assert_eq!(
                parsed[header],
                serde_json::json!([REDACTED, REDACTED, REDACTED])
            );
            assert_eq!(parsed["safe"], "diagnostic");
        }
    }

    #[test]
    fn escaped_bearer_whitespace_and_all_query_credentials_are_redacted() {
        for separator in [r"\t", r"\n", r"\r", r"\f", r"\u0020", r"\u0009"] {
            let body = format!(r#"{{"message":"Bearer{separator}bearer-sentinel"}}"#);
            assert!(!sanitize_provider_error(&body, &[]).contains("bearer-sentinel"));
        }
        for parameter in ["key", "API_KEY", "access_token", "Token"] {
            let body = format!("https://example.test/?{parameter}=query-sentinel&safe=yes exact-configured-sentinel");
            let clean = sanitize_provider_error(&body, &["exact-configured-sentinel"]);
            assert!(!clean.contains("query-sentinel"), "{clean}");
            assert!(!clean.contains("exact-configured-sentinel"), "{clean}");
            assert!(clean.contains("&safe=yes"), "{clean}");
        }
    }

    #[test]
    fn conservative_boundaries_preserve_only_definite_terminators() {
        for parameter in [
            "token",
            "key",
            "api_key",
            "access_token",
            "client_secret",
            "refresh_token",
        ] {
            for punctuation in [",", "/", r"\", "%2F", "+", ".", "="] {
                for terminator in [
                    "&safe=yes",
                    "#safe",
                    " safe",
                    "\tsafe",
                    "\"safe",
                    "'safe",
                    ">safe",
                    ")safe",
                    "",
                ] {
                    let body = format!("?{parameter}=prefix{punctuation}qzvkmnrp{terminator}");
                    assert_eq!(
                        sanitize_provider_error(&body, &[]),
                        format!("?{parameter}={REDACTED}{terminator}")
                    );
                }
            }
        }
        for scheme in ["Bearer", "Basic", "Token"] {
            for punctuation in [
                ",", "/", r"\/", r"\\", r#"\""#, r"\t", r"\u002f", "%2F", "+", ".", "=", "&", "#",
                ";", ")", "'", "]", "}", "\u{a0}",
            ] {
                let body = format!(
                    r#"{{"message":"{scheme} prefix{punctuation}qzvkmnrp","safe":"diagnostic"}}"#
                );
                assert_eq!(
                    sanitize_provider_error(&body, &[]),
                    format!(r#"{{"message":"{scheme} {REDACTED}","safe":"diagnostic"}}"#)
                );
            }
            for terminator in [" safe", "\tsafe", "\nsafe", "\rsafe", ""] {
                let body = format!("{scheme} prefix,qzvkmnrp{terminator}");
                assert_eq!(
                    sanitize_provider_error(&body, &[]),
                    format!("{scheme} {REDACTED}{terminator}")
                );
            }
        }
    }

    #[test]
    fn configured_secrets_are_redacted_in_encoded_forms() {
        for (secret, body) in [
            (
                "opaque/credential+qzvkmnrp",
                r#"{"message":"opaque\/credential+qzvkmnrp"}"#,
            ),
            (
                "opaque/credential+qzvkmnrp",
                r#"{"message":"opaque\u002fcredential+qzvkmnrp"}"#,
            ),
            (
                "opaque/credential+qzvkmnrp",
                "opaque%2fcredential%2Bqzvkmnrp",
            ),
            ("opaque credential", "opaque+credential"),
            ("opaque/credential", "%6F%70%61%71%75%65%2fcredential"),
            (
                "opaque\"\\credential",
                r#"{"message":"opaque\"\\credential"}"#,
            ),
            (
                "opaque\u{1f600}credential",
                r#"{"message":"opaque\ud83d\ude00credential"}"#,
            ),
        ] {
            let clean = sanitize_provider_error(body, &[secret]);
            assert!(clean.contains(REDACTED), "{body} => {clean}");
            assert!(!clean.contains("credential"), "{body} => {clean}");
        }
    }

    #[test]
    fn bracket_prefixes_are_values_except_in_json_header_arrays() {
        for body in [
            "?token=[qzvkmnrp&safe=yes",
            "Bearer [qzvkmnrp",
            "Basic [qzvkmnrp",
            "Token [qzvkmnrp",
        ] {
            let clean = sanitize_provider_error(body, &[]);
            assert!(!clean.contains("qzvkmnrp"), "{clean}");
        }
    }
    #[test]
    fn configured_secret_escapes_in_diagnostic_excerpts_are_redacted() {
        for (secret, body) in [
            ("opaque/credential", r"rejected opaque\/credential"),
            ("opaque/credential", r"rejected opaque\u002fcredential"),
            (
                "opaque\u{1f600}credential",
                r"rejected opaque\ud83d\ude00credential",
            ),
        ] {
            let clean = sanitize_provider_error(body, &[secret]);
            assert!(!clean.contains("opaque"), "{clean}");
            assert!(!clean.contains("credential"), "{clean}");
        }
    }
}
