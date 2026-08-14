use async_trait::async_trait;

use locust_core::error::{LocustError, Result};
use locust_core::models::{TranslationRequest, TranslationResult};
use locust_core::translation::TranslationProvider;

pub struct MockProvider;

/// Build a recognisable mock string that never exceeds `source` in **UTF-8
/// bytes** *or* **UTF-16LE bytes** (BMP: 2 × UTF-16 code units).
///
/// - **Unity** inject: UTF-8 length must be ≤ source
/// - **Unreal** inject: UTF-16LE length must be ≤ source
/// - **Wolf** inject: Shift-JIS; for typical CJK/ASCII, UTF-8 ≤ source is a
///   conservative outer bound (SJIS is ≤ UTF-8 for those scripts)
///
/// The old `"[MOCK:es] {source}"` form always grew the string and made mock
/// pipelines write zero files on binary engines. Pure UTF-8 capping still
/// overshot Unreal: ASCII mock tags are 1 byte UTF-8 but 2 bytes UTF-16 each.
///
/// When both budgets allow, the result starts with `[MOCK:{lang}]`. Otherwise
/// short sources get a same-slot reverse so inject still has something
/// different without overflowing.
///
/// **Placeholder tokens** (`{PL_0}`, `{PL_1}`, …) are always kept intact.
fn mock_fit(source: &str, target_lang: &str) -> String {
    if source.is_empty() {
        return String::new();
    }
    if source.contains("{PL_") {
        return mock_fit_preserving_pl(source, target_lang);
    }
    mock_fit_plain(source, target_lang, source.len(), utf16_byte_len(source))
}

fn utf16_byte_len(s: &str) -> usize {
    s.encode_utf16().count() * 2
}

fn fits_slots(s: &str, max_utf8: usize, max_utf16: usize) -> bool {
    s.len() <= max_utf8 && utf16_byte_len(s) <= max_utf16
}

/// Grow `prefix` by appending chars from `fill` while both inject slots hold.
fn append_while_fits(prefix: &str, fill: &str, max_utf8: usize, max_utf16: usize) -> String {
    let mut out = prefix.to_string();
    for ch in fill.chars() {
        let mut trial = out.clone();
        trial.push(ch);
        if fits_slots(&trial, max_utf8, max_utf16) {
            out = trial;
        } else {
            break;
        }
    }
    out
}

fn mock_fit_plain(source: &str, target_lang: &str, max_utf8: usize, max_utf16: usize) -> String {
    if max_utf8 == 0 || max_utf16 == 0 {
        return String::new();
    }
    let tag = format!("[MOCK:{target_lang}] ");
    if fits_slots(tag.trim_end(), max_utf8, max_utf16) {
        // Prefer tag (+ trailing space when space itself still fits).
        let with_space = if fits_slots(&tag, max_utf8, max_utf16) {
            tag.as_str()
        } else {
            tag.trim_end()
        };
        let out = append_while_fits(with_space, source, max_utf8, max_utf16);
        if fits_slots(&out, max_utf8, max_utf16) && !out.is_empty() {
            return out;
        }
    }
    // Tag does not fit both slots: reverse graphemes under dual budget.
    let rev: String = source.chars().rev().collect();
    append_while_fits("", &rev, max_utf8, max_utf16)
}

/// Keep every `{PL_N}` token byte-for-byte; mock only free text under the
/// remaining dual budget so translate→restore never drops tokens.
fn mock_fit_preserving_pl(source: &str, target_lang: &str) -> String {
    let max_utf8 = source.len();
    let max_utf16 = utf16_byte_len(source);
    let segments = split_pl_segments(source);
    let token_utf8: usize = segments
        .iter()
        .map(|s| match s {
            PlSeg::Token(t) => t.len(),
            PlSeg::Free(_) => 0,
        })
        .sum();
    let token_utf16: usize = segments
        .iter()
        .map(|s| match s {
            PlSeg::Token(t) => utf16_byte_len(t),
            PlSeg::Free(_) => 0,
        })
        .sum();
    if token_utf8 > max_utf8 || token_utf16 > max_utf16 {
        return source.to_string();
    }
    let free_concat: String = segments
        .iter()
        .filter_map(|s| match s {
            PlSeg::Free(f) => Some(f.as_str()),
            PlSeg::Token(_) => None,
        })
        .collect();
    let mocked_free = mock_fit_plain(
        &free_concat,
        target_lang,
        max_utf8 - token_utf8,
        max_utf16 - token_utf16,
    );

    let mut out = String::with_capacity(max_utf8);
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
    debug_assert!(
        fits_slots(&out, max_utf8, max_utf16),
        "mock_fit PL out exceeds slots: out={out:?} u8={} u16={}",
        out.len(),
        utf16_byte_len(&out)
    );
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
        let ch_len = source[i..]
            .chars()
            .next()
            .map(|c| c.len_utf8())
            .unwrap_or(1);
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

/// Test/dev provider that always fails. Used to exercise fallback chains.
pub struct AlwaysErrorProvider;

#[async_trait]
impl TranslationProvider for AlwaysErrorProvider {
    fn id(&self) -> &str {
        "always-error"
    }

    fn name(&self) -> &str {
        "Always Error"
    }

    fn is_free(&self) -> bool {
        true
    }

    fn requires_api_key(&self) -> bool {
        false
    }

    async fn translate(&self, _requests: &[TranslationRequest]) -> Result<Vec<TranslationResult>> {
        Err(LocustError::ProviderError(
            "primary provider refused".into(),
        ))
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
    fn mock_fit_never_exceeds_source_utf8_or_utf16() {
        let cases = [
            "",
            "Hi",
            "Hello, world!",
            "a",
            "日本語テスト",
            "勇者",
            &"x".repeat(100),
            &"漢".repeat(20),
        ];
        for src in cases {
            let out = mock_fit(src, "es");
            assert!(
                out.len() <= src.len(),
                "utf8: mock_fit({src:?}) = {out:?} ({} > {})",
                out.len(),
                src.len()
            );
            assert!(
                utf16_byte_len(&out) <= utf16_byte_len(src),
                "utf16: mock_fit({src:?}) = {out:?} ({} > {} utf16 bytes)",
                utf16_byte_len(&out),
                utf16_byte_len(src)
            );
        }
    }

    #[test]
    fn mock_fit_keeps_tag_when_source_is_long_enough_ascii() {
        let src = "This is a long enough dialogue line for the mock tag.";
        let out = mock_fit(src, "es");
        assert!(out.starts_with("[MOCK:es]"), "{out}");
        assert!(out.len() <= src.len());
        assert!(utf16_byte_len(&out) <= utf16_byte_len(src));
    }

    #[test]
    fn mock_fit_cjk_short_does_not_use_ascii_tag_over_utf16() {
        // 3 CJK chars: 9 UTF-8 bytes, 6 UTF-16 bytes. ASCII tag alone is 9/18.
        let src = "日本語";
        let out = mock_fit(src, "es");
        assert!(
            !out.contains("[MOCK:"),
            "tag must not win when it blows UTF-16 budget: {out}"
        );
        assert!(out.len() <= src.len());
        assert!(utf16_byte_len(&out) <= utf16_byte_len(src));
        // Reverse path should still change the string when length > 1.
        assert_ne!(out, src);
    }

    #[test]
    fn mock_fit_short_source_is_reversed_not_oversize() {
        let out = mock_fit("Hero", "es");
        assert_eq!(out, "oreH");
        assert!(out.len() <= 4);
        assert!(utf16_byte_len(&out) <= utf16_byte_len("Hero"));
    }

    #[test]
    fn mock_fit_preserves_pl_tokens_under_length_cap() {
        // Sanitized form after PlaceholderProcessor::extract("{i}Hello{/i}")
        let src = "{PL_0}Hello there, dialogue line long enough{PL_1}";
        let out = mock_fit(src, "es");
        assert!(out.len() <= src.len(), "out={out:?} src_len={}", src.len());
        assert!(utf16_byte_len(&out) <= utf16_byte_len(src));
        assert!(out.contains("{PL_0}"), "dropped open token: {out}");
        assert!(out.contains("{PL_1}"), "dropped close token: {out}");
    }

    #[test]
    fn mock_fit_pure_pl_token_stays_restorable() {
        let src = "{PL_0}";
        let out = mock_fit(src, "es");
        assert!(out.contains("{PL_0}"));
        assert!(out.len() <= src.len());
        assert!(utf16_byte_len(&out) <= utf16_byte_len(src));
    }
}
