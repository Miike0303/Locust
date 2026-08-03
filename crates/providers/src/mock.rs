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
fn mock_fit(source: &str, target_lang: &str) -> String {
    let max = source.len();
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
}
