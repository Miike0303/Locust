use async_trait::async_trait;

use locust_core::error::Result;
use locust_core::models::{TranslationRequest, TranslationResult};
use locust_core::translation::{LangPair, TranslationProvider};

pub struct MockProvider;

/// Build a recognisable mock string that never exceeds `source` in **UTF-8
/// bytes**. Unity / Wolf / Unreal injectors refuse longer replacements; the
/// old `"[MOCK:es] {source}"` form always grew the string and made mock
/// pipelines write zero files on those engines.
///
/// When the source is long enough the result starts with `[MOCK:{lang}]`.
/// Shorter sources get a same-length reverse so inject still has something
/// different to write without overflowing the slot.
///
/// **Placeholder tokens** (`{PL_0}`, `{PL_1}`, …) inserted by
/// `PlaceholderProcessor::extract` before the provider runs are always kept
/// intact so `restore` does not fail under length capping.
fn mock_fit(source: &str, target_lang: &str) -> String {
    let max = source.len();
    if max == 0 {
        return String::new();
    }
    if source.contains("{PL_") {
        return mock_fit_preserving_pl(source, target_lang, max);
    }
    mock_fit_plain(source, target_lang, max)
}

fn mock_fit_plain(source: &str, target_lang: &str, max: usize) -> String {
    if max == 0 {
        return String::new();
    }
    let tag = format!("[MOCK:{target_lang}]");
    // Prefer full tag + space + as much of the source as fits.
    if tag.len() + 1 < max {
        let mut out = String::with_capacity(max);
        out.push_str(&tag);
        out.push(' ');
        let rest = truncate_utf8_bytes(source, max - out.len());
        out.push_str(&rest);
        debug_assert!(out.len() <= max);
        return out;
    }
    // Tag does not fit: reverse graphemes to stay ≤ max and stay non-identity
    // when source length > 1.
    let rev: String = source.chars().rev().collect();
    truncate_utf8_bytes(&rev, max)
}

/// Keep every `{PL_N}` token byte-for-byte; mock only free text under the
/// remaining budget so translate→restore never drops tokens on binary engines.
fn mock_fit_preserving_pl(source: &str, target_lang: &str, max: usize) -> String {
    let segments = split_pl_segments(source);
    let token_bytes: usize = segments
        .iter()
        .map(|s| match s {
            PlSeg::Token(t) => t.len(),
            PlSeg::Free(_) => 0,
        })
        .sum();
    if token_bytes > max {
        // Should not happen when tokens come from `source`; fail safe to identity.
        return source.to_string();
    }
    let free_budget = max - token_bytes;
    let free_concat: String = segments
        .iter()
        .filter_map(|s| match s {
            PlSeg::Free(f) => Some(f.as_str()),
            PlSeg::Token(_) => None,
        })
        .collect();
    let mocked_free = mock_fit_plain(&free_concat, target_lang, free_budget);

    let mut out = String::with_capacity(max);
    let mut free_emitted = false;
    for seg in &segments {
        match seg {
            PlSeg::Token(t) => out.push_str(t),
            PlSeg::Free(_) => {
                if !free_emitted {
                    out.push_str(&mocked_free);
                    free_emitted = true;
                }
            }
        }
    }
    debug_assert!(out.len() <= max);
    debug_assert!(
        pl_tokens(source).iter().all(|t| out.contains(t.as_str())),
        "mock_fit dropped a PL token: source={source:?} out={out:?}"
    );
    out
}

enum PlSeg {
    Free(String),
    Token(String),
}

fn split_pl_segments(source: &str) -> Vec<PlSeg> {
    let mut out = Vec::new();
    let mut i = 0;
    let mut free_start = 0;
    while i < source.len() {
        if let Some(tok_len) = pl_token_len_at(source, i) {
            if free_start < i {
                out.push(PlSeg::Free(source[free_start..i].to_string()));
            }
            out.push(PlSeg::Token(source[i..i + tok_len].to_string()));
            i += tok_len;
            free_start = i;
            continue;
        }
        // Advance one Unicode scalar so free slices stay on char boundaries.
        let ch_len = source[i..].chars().next().map(|c| c.len_utf8()).unwrap_or(1);
        i += ch_len;
    }
    if free_start < source.len() {
        out.push(PlSeg::Free(source[free_start..].to_string()));
    }
    out
}

/// Length of a `{PL_<digits>}` token starting at `i`, if any.
fn pl_token_len_at(source: &str, i: usize) -> Option<usize> {
    let rest = &source[i..];
    if !rest.starts_with("{PL_") {
        return None;
    }
    let after = &rest[4..];
    let digits = after.bytes().take_while(|b| b.is_ascii_digit()).count();
    if digits == 0 {
        return None;
    }
    if after.as_bytes().get(digits) == Some(&b'}') {
        Some(4 + digits + 1)
    } else {
        None
    }
}

fn pl_tokens(source: &str) -> Vec<String> {
    split_pl_segments(source)
        .into_iter()
        .filter_map(|s| match s {
            PlSeg::Token(t) => Some(t),
            PlSeg::Free(_) => None,
        })
        .collect()
}

fn truncate_utf8_bytes(s: &str, max_bytes: usize) -> String {
    if s.len() <= max_bytes {
        return s.to_string();
    }
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    s[..end].to_string()
}

#[async_trait]
impl TranslationProvider for MockProvider {
    fn id(&self) -> &str {
        "mock"
    }

    fn name(&self) -> &str {
        "Mock (testing)"
    }

    fn is_free(&self) -> bool {
        true
    }

    fn requires_api_key(&self) -> bool {
        false
    }

    async fn translate(&self, requests: &[TranslationRequest]) -> Result<Vec<TranslationResult>> {
        Ok(requests
            .iter()
            .map(|r| TranslationResult {
                entry_id: r.entry_id.clone(),
                translation: mock_fit(&r.source, &r.target_lang),
                detected_source_lang: None,
                provider: "mock".to_string(),
                tokens_used: None,
                input_tokens: None,
                output_tokens: None,
                cost_usd: None,
            })
            .collect())
    }

    async fn estimate_cost(&self, _char_count: usize, _target_lang: &str) -> Option<f64> {
        None
    }

    async fn health_check(&self) -> Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mock_fit_never_exceeds_source_bytes() {
        let cases = [
            "",
            "Hi",
            "Hello, world!",
            "a",
            "日本語テスト",
            &"x".repeat(100),
        ];
        for src in cases {
            let out = mock_fit(src, "es");
            assert!(
                out.len() <= src.len(),
                "mock_fit({src:?}) = {out:?} ({} > {} bytes)",
                out.len(),
                src.len()
            );
        }
    }

    #[test]
    fn mock_fit_keeps_tag_when_source_is_long_enough() {
        let src = "This is a long enough dialogue line for the mock tag.";
        let out = mock_fit(src, "es");
        assert!(out.starts_with("[MOCK:es]"), "{out}");
        assert!(out.len() <= src.len());
    }

    #[test]
    fn mock_fit_short_source_is_reversed_not_oversize() {
        let out = mock_fit("Hero", "es");
        assert_eq!(out, "oreH");
        assert!(out.len() <= 4);
    }

    #[test]
    fn mock_fit_preserves_pl_tokens_under_length_cap() {
        // Sanitized form after PlaceholderProcessor::extract("{i}Hello{/i}")
        let src = "{PL_0}Hello there, dialogue line long enough{PL_1}";
        let out = mock_fit(src, "es");
        assert!(out.len() <= src.len(), "out={out:?} src_len={}", src.len());
        assert!(out.contains("{PL_0}"), "dropped open token: {out}");
        assert!(out.contains("{PL_1}"), "dropped close token: {out}");
    }

    #[test]
    fn mock_fit_pure_pl_token_stays_restorable() {
        let src = "{PL_0}";
        let out = mock_fit(src, "es");
        assert!(out.contains("{PL_0}"));
        assert!(out.len() <= src.len());
    }
}
