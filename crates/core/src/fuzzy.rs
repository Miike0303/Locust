//! Pure, bounded Unicode character comparisons for translation memory.

pub const MAX_CHARS: usize = 2000;

/// Unicode lowercase with trimmed, single-space whitespace separators.
pub fn normalize(s: &str) -> String {
    s.split_whitespace()
        .map(str::to_lowercase)
        .collect::<Vec<_>>()
        .join(" ")
}

/// Reusable, bounded comparison input. Short inputs also retain the complete
/// normalized string so suggestions can reuse it as their deduplication key.
#[derive(Debug)]
pub struct PreparedText<'a> {
    chars: Vec<char>,
    normalized: String,
    uncapped_source: Option<&'a str>,
}

impl PreparedText<'_> {
    /// Consume the full normalized source for a deduplication key. Capped input
    /// is normalized in full only here, keeping comparison preparation bounded.
    pub fn into_normalized(self) -> String {
        match self.uncapped_source {
            Some(source) => normalize(source),
            None => self.normalized,
        }
    }
}

/// Normalize the first `MAX_CHARS` Unicode scalars once for repeated scoring.
/// Lowercase expansion is capped too. Normalize the raw prefix independently:
/// context-sensitive lowercase (e.g. final sigma) can differ at the cutoff.
pub fn prepare(s: &str) -> PreparedText<'_> {
    let end = s.char_indices().nth(MAX_CHARS).map_or(s.len(), |(i, _)| i);
    let normalized = normalize(&s[..end]);
    let chars = normalized.chars().take(MAX_CHARS).collect();
    PreparedText {
        chars,
        normalized,
        uncapped_source: (end < s.len()).then_some(s),
    }
}

fn comparison_chars(s: &str) -> Vec<char> {
    // Bound the input before normalization too, including whitespace-only input.
    let prefix: String = s.chars().take(MAX_CHARS).collect();
    normalize(&prefix).chars().take(MAX_CHARS).collect()
}

pub(crate) fn candidate_length_bounds(source: &str, threshold: f64) -> Option<(usize, usize)> {
    if !threshold.is_finite() || !(0.0..=1.0).contains(&threshold) {
        return None;
    }
    if threshold == 0.0 {
        return Some((0, MAX_CHARS));
    }
    let len = comparison_chars(source).len() as f64;
    // d >= |a-b| and score = 1-d/max(a,b) require a*t <= b <= a/t.
    // Round outward on comparison lengths. SQL must also admit raw strings
    // whose lengths can change through normalization or the comparison cap.
    Some((
        (len * threshold).floor() as usize,
        ((len / threshold).ceil() as usize).min(MAX_CHARS),
    ))
}

/// 1 - Levenshtein distance / longest length, after normalization. Two empty
/// strings score 1. Inputs are limited to their first 2000 Unicode characters.
pub fn similarity(a: &str, b: &str) -> f64 {
    similarity_at_threshold(a, b, 0.0).unwrap_or(0.0)
}

/// Return the exact score if it meets `threshold`, otherwise `None`.
/// Reject impossible length differences before allocating distance rows. The
/// remaining work uses a diagonal band and stops when no row can meet the bound.
pub fn similarity_at_threshold(a: &str, b: &str, threshold: f64) -> Option<f64> {
    if !threshold.is_finite() || !(0.0..=1.0).contains(&threshold) {
        return None;
    }
    similarity_prepared(&prepare(a), &prepare(b), threshold)
}

/// Score already prepared inputs with exactly the same threshold and distance
/// rules as `similarity_at_threshold`, without normalizing either input again.
pub fn similarity_prepared(
    a: &PreparedText<'_>,
    b: &PreparedText<'_>,
    threshold: f64,
) -> Option<f64> {
    if !threshold.is_finite() || !(0.0..=1.0).contains(&threshold) {
        return None;
    }
    let a = &a.chars;
    let b = &b.chars;
    let longest = a.len().max(b.len());
    if longest == 0 {
        return Some(1.0);
    }
    let score = |distance: usize| 1.0 - distance as f64 / longest as f64;
    if score(a.len().abs_diff(b.len())) < threshold {
        return None;
    }
    if a == b {
        return Some(1.0);
    }
    // Round outward: floating point rounding must never exclude a boundary hit.
    let bound = ((1.0 - threshold) * longest as f64).ceil() as usize;
    let outside = bound + 1;
    let mut previous: Vec<usize> = (0..=b.len()).collect();
    let mut current = vec![outside; b.len() + 1];
    for (i, ca) in a.iter().enumerate() {
        let row = i + 1;
        current.fill(outside);
        current[0] = row;
        let start = row.saturating_sub(bound).max(1);
        let end = (row + bound).min(b.len());
        let mut minimum = current[0];
        for column in start..=end {
            current[column] = (previous[column] + 1)
                .min(current[column - 1] + 1)
                .min(previous[column - 1] + usize::from(*ca != b[column - 1]));
            minimum = minimum.min(current[column]);
        }
        if minimum > bound {
            return None;
        }
        std::mem::swap(&mut previous, &mut current);
    }
    let result = score(previous[b.len()]);
    (result >= threshold).then_some(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fuzzy_sql_whitespace_exceptions_match_all_unicode_scalars() {
        let whitespace = (0..=0x10FFFF)
            .filter_map(char::from_u32)
            .filter(|c| c.is_whitespace())
            .collect::<Vec<_>>();
        // Database::memory_candidates explicitly tests these except U+0020,
        // which only changes length at an edge or next to another space.
        assert_eq!(
            whitespace,
            [
                '\u{0009}', '\u{000A}', '\u{000B}', '\u{000C}', '\u{000D}', '\u{0020}', '\u{0085}',
                '\u{00A0}', '\u{1680}', '\u{2000}', '\u{2001}', '\u{2002}', '\u{2003}', '\u{2004}',
                '\u{2005}', '\u{2006}', '\u{2007}', '\u{2008}', '\u{2009}', '\u{200A}', '\u{2028}',
                '\u{2029}', '\u{202F}', '\u{205F}', '\u{3000}',
            ],
            "update the SQL whitespace exceptions when Rust's Unicode set changes"
        );
    }

    #[test]
    fn fuzzy_sql_lowercase_exceptions_match_all_unicode_scalars() {
        let changes_length = (0..=0x10FFFF)
            .filter_map(char::from_u32)
            .filter(|c| c.to_lowercase().count() != 1)
            .collect::<Vec<_>>();
        assert_eq!(
            changes_length,
            ['\u{0130}'],
            "update the SQL lowercase exceptions when Rust's Unicode mappings change"
        );
    }

    #[test]
    fn fuzzy_candidate_bounds_use_comparison_lengths_and_round_outward() {
        assert_eq!(candidate_length_bounds("Hello world", 0.6), Some((6, 19)));
        assert_eq!(
            candidate_length_bounds("  Hello\tworld  ", 0.6),
            Some((6, 19))
        );
        assert_eq!(candidate_length_bounds("İ", 1.0), Some((2, 2)));
        assert_eq!(candidate_length_bounds(" \t\n", 1.0), Some((0, 0)));
        assert_eq!(
            candidate_length_bounds(&"a".repeat(MAX_CHARS + 100), 1.0),
            Some((MAX_CHARS, MAX_CHARS))
        );
        assert_eq!(
            candidate_length_bounds(&"İ".repeat(MAX_CHARS), 1.0),
            Some((MAX_CHARS, MAX_CHARS))
        );
        assert_eq!(candidate_length_bounds("Hello", 0.0), Some((0, MAX_CHARS)));
        assert_eq!(
            candidate_length_bounds("Hello", f64::from_bits(1)),
            Some((0, MAX_CHARS))
        );
        for threshold in [-1.0, 1.1, f64::NEG_INFINITY, f64::INFINITY, f64::NAN] {
            assert_eq!(candidate_length_bounds("Hello", threshold), None);
        }
    }

    #[test]
    fn fuzzy_identical_and_empty() {
        assert_eq!(similarity("Hello", "Hello"), 1.0);
        assert_eq!(similarity("", ""), 1.0);
        assert_eq!(similarity(" \n", ""), 1.0);
        assert_eq!(similarity("", "abc"), 0.0);
        assert_eq!(similarity("abc", ""), 0.0);
    }

    #[test]
    fn fuzzy_normalizes_case_and_unicode_whitespace() {
        assert_eq!(
            normalize("  HÉLLO\t世界\n\u{2003}Again  "),
            "héllo 世界 again"
        );
        assert_eq!(similarity("  HÉLLO\tWorld\n", "héllo world"), 1.0);
        assert_eq!(normalize("İ"), "i\u{307}");
    }

    #[test]
    fn fuzzy_counts_characters_not_bytes() {
        assert_eq!(similarity("你好世界", "你好世人"), 0.75);
        assert_eq!(similarity("café", "cafe"), 0.75);
        assert_eq!(similarity("é", "e"), 0.0);
    }

    #[test]
    fn fuzzy_known_distances_and_symmetry() {
        for (a, b, distance) in [
            ("kitten", "sitting", 3),
            ("book", "back", 2),
            ("abc", "ab", 1),
        ] {
            let expected = 1.0 - distance as f64 / a.len().max(b.len()) as f64;
            assert_eq!(similarity(a, b), expected);
            assert_eq!(similarity(b, a), expected);
        }
    }

    #[test]
    fn fuzzy_threshold_exits_for_impossible_lengths_and_distances() {
        // Length difference alone fails, before distance rows are allocated.
        assert_eq!(
            similarity_at_threshold("a", &"a".repeat(MAX_CHARS), 0.6),
            None
        );
        assert_eq!(similarity_at_threshold("abcde", "ab", 0.6), None);
        assert_eq!(similarity_at_threshold("aaaaa", "bbbbb", 0.6), None);
        assert_eq!(similarity_at_threshold("abcde", "abcxy", 0.6), Some(0.6));
        assert_eq!(similarity_at_threshold("abcde", "abcdx", 0.8), Some(0.8));
        assert_eq!(similarity_at_threshold("abc", "abc", 1.0), Some(1.0));
        assert_eq!(similarity_at_threshold("abc", "abd", 1.0), None);
        assert_eq!(similarity_at_threshold("a", "b", f64::NAN), None);
    }

    #[test]
    fn fuzzy_caps_inputs_by_character() {
        let prefix = "界".repeat(MAX_CHARS);
        assert_eq!(
            similarity(&format!("{prefix}a"), &format!("{prefix}b")),
            1.0
        );
        assert_eq!(similarity(&" ".repeat(MAX_CHARS + 10), ""), 1.0);
        // Lowercase expansion is capped as well.
        assert_eq!(comparison_chars(&"İ".repeat(MAX_CHARS)).len(), MAX_CHARS);
    }

    #[test]
    fn fuzzy_threshold_matches_unbounded_scores() {
        let mut words = vec![String::new()];
        for len in 1..=4 {
            for bits in 0..(1 << len) {
                words.push(
                    (0..len)
                        .map(|i| if bits & (1 << i) == 0 { 'a' } else { 'b' })
                        .collect(),
                );
            }
        }
        for a in &words {
            for b in &words {
                let score = similarity(a, b);
                for threshold in [0.0, 0.6, 0.8, 1.0] {
                    assert_eq!(
                        similarity_at_threshold(a, b, threshold),
                        (score >= threshold).then_some(score),
                        "{a:?}, {b:?}, {threshold}"
                    );
                }
            }
        }
    }

    // W5-03 scorer snapshot: do not delegate to the prepared scorer.
    fn legacy_similarity_at_threshold(a: &str, b: &str, threshold: f64) -> Option<f64> {
        if !threshold.is_finite() || !(0.0..=1.0).contains(&threshold) {
            return None;
        }
        // Frozen W5-03 preparation, independent of the prepared implementation.
        let chars = |s: &str| {
            let prefix: String = s.chars().take(MAX_CHARS).collect();
            normalize(&prefix)
                .chars()
                .take(MAX_CHARS)
                .collect::<Vec<_>>()
        };
        let a = chars(a);
        let b = chars(b);
        let longest = a.len().max(b.len());
        if longest == 0 {
            return Some(1.0);
        }
        let score = |distance: usize| 1.0 - distance as f64 / longest as f64;
        if score(a.len().abs_diff(b.len())) < threshold {
            return None;
        }
        if a == b {
            return Some(1.0);
        }
        // Round outward: floating point rounding must never exclude a boundary hit.
        let bound = ((1.0 - threshold) * longest as f64).ceil() as usize;
        let outside = bound + 1;
        let mut previous: Vec<usize> = (0..=b.len()).collect();
        let mut current = vec![outside; b.len() + 1];
        for (i, ca) in a.iter().enumerate() {
            let row = i + 1;
            current.fill(outside);
            current[0] = row;
            let start = row.saturating_sub(bound).max(1);
            let end = (row + bound).min(b.len());
            let mut minimum = current[0];
            for column in start..=end {
                current[column] = (previous[column] + 1)
                    .min(current[column - 1] + 1)
                    .min(previous[column - 1] + usize::from(*ca != b[column - 1]));
                minimum = minimum.min(current[column]);
            }
            if minimum > bound {
                return None;
            }
            std::mem::swap(&mut previous, &mut current);
        }
        let result = score(previous[b.len()]);
        (result >= threshold).then_some(result)
    }

    #[test]
    fn fuzzy_prepared_matches_legacy_score_bits_and_normalized_keys() {
        let mut fixtures = vec![
            "".to_owned(),
            " \t\n".into(),
            "Hello".into(),
            " hello \t".into(),
            "Hullo".into(),
            "Hexxo".into(),
            "xxxxx".into(),
            "HÉLLO 世界".into(),
            "héllo\t世界".into(),
            "café".into(),
            "cafe\u{301}".into(),
            "こんにちは 世界".into(),
            "İ".into(),
            "i\u{307}".into(),
            "ΟΣ".into(),
            "οσ".into(),
            "ος".into(),
            "hello\0world".into(),
            "👩\u{200d}🚀".into(),
        ];
        for len in 1..=4 {
            for bits in 0..(1 << len) {
                fixtures.push(
                    (0..len)
                        .map(|i| if bits & (1 << i) == 0 { 'a' } else { 'b' })
                        .collect(),
                );
            }
        }
        let mut pairs = Vec::new();
        for a in &fixtures {
            for b in &fixtures {
                pairs.push((a.clone(), b.clone()));
            }
        }
        // Exercise every scalar in the W5-03 whitespace/lowercase exception sets.
        for c in (0..=0x10FFFF).filter_map(char::from_u32) {
            if c.is_whitespace() {
                pairs.push((format!("{c}HÉLLO{c}{c}世界{c}"), "héllo 世界".into()));
            }
            if c.to_lowercase().count() != 1 {
                pairs.push((c.to_string(), c.to_lowercase().collect()));
            }
        }
        let prefix = "界".repeat(MAX_CHARS);
        pairs.extend([
            (format!("{prefix}a"), format!("{prefix}b")),
            (" ".repeat(MAX_CHARS + 10), String::new()),
            (format!("{}a", " ".repeat(MAX_CHARS)), String::new()),
            ("İ".repeat(MAX_CHARS), "i\u{307}".repeat(MAX_CHARS / 2)),
            // Greek final sigma depends on context beyond the comparison cutoff.
            (format!("{}ΑΣΑ", " ".repeat(MAX_CHARS - 2)), "ας".into()),
            (format!("{}ΑΣΑ", " ".repeat(MAX_CHARS - 2)), "ασ".into()),
            // The W5-03 normalized candidate-window fixtures in both directions.
            (
                format!("{}Hello world", " ".repeat(100)),
                "Hello world again".into(),
            ),
            (
                "Hello world".into(),
                format!("{}Hello world again  ", " ".repeat(100)),
            ),
            ("Hello world".into(), "Hello\t\tworld".into()),
            ("界".repeat(10), "界".repeat(16)),
            (
                "こんにちは 世界".into(),
                format!(
                    "{}こんにちは{}世界",
                    "\u{3000}".repeat(20),
                    "\u{3000}".repeat(20)
                ),
            ),
            ("i\u{307}".repeat(10), "İ".repeat(10)),
            ("İ".repeat(10), "i\u{307}".repeat(10)),
            (prefix.clone(), format!("{prefix}{}", "x".repeat(1000))),
            (format!("{prefix}{}", "x".repeat(1000)), prefix),
            ("hello\0world".into(), "Hello\0world".into()),
            (" \t".into(), "\u{2003}".repeat(100)),
        ]);
        let thresholds = [
            0.0,
            0.6,
            1.0,
            -1.0,
            1.1,
            f64::NEG_INFINITY,
            f64::INFINITY,
            f64::NAN,
        ];
        for (a, b) in &pairs {
            let query = prepare(a);
            let candidate = prepare(b);
            for threshold in thresholds {
                let expected = legacy_similarity_at_threshold(a, b, threshold).map(f64::to_bits);
                assert_eq!(
                    similarity_prepared(&query, &candidate, threshold).map(f64::to_bits),
                    expected,
                    "prepared: {a:?}, {b:?}, {threshold}"
                );
                assert_eq!(
                    similarity_at_threshold(a, b, threshold).map(f64::to_bits),
                    expected,
                    "wrapper: {a:?}, {b:?}, {threshold}"
                );
            }
            assert_eq!(query.into_normalized(), normalize(a));
            assert_eq!(candidate.into_normalized(), normalize(b));
        }
        println!("legacy equivalence: {} pairs, {} thresholds, {} bitwise score comparisons, {} full normalization keys",
            pairs.len(), thresholds.len(), pairs.len() * thresholds.len() * 2, pairs.len() * 2);
    }

    #[test]
    #[ignore = "manual 6000-candidate timing; run with --ignored --nocapture"]
    fn fuzzy_prepare_benchmark() {
        use std::hint::black_box;
        use std::time::{Duration, Instant};

        let query = "  HÉLLO\t世界  again  player\n";
        let sources = [
            "héllo 世界 again player",
            "  HÉLLO\t世界 again  player\n",
            "héllo 世界 again players",
            "héllo 世界 again gamer",
            "héllo 世界 once more player",
            "xxxxxxxxxxxxxxxxxxxxxxx",
        ];
        let candidates: Vec<_> = (0..6000).map(|i| sources[i % sources.len()]).collect();
        let rounds = 7;
        let mut old_time = Duration::ZERO;
        let mut new_time = Duration::ZERO;
        let mut old_preparations = 0;
        let mut new_preparations = 0;
        let mut accepted = 0;
        for round in 0..rounds {
            // Alternate execution order to reduce warm-cache/order bias.
            let mut old_results = Vec::new();
            let mut new_results = Vec::new();
            for old_first in [round % 2 == 0, round % 2 != 0] {
                let start = Instant::now();
                if old_first {
                    for candidate in &candidates {
                        old_preparations += 2;
                        if let Some(score) = legacy_similarity_at_threshold(
                            black_box(query),
                            black_box(candidate),
                            0.6,
                        ) {
                            old_results.push((score.to_bits(), normalize(candidate)));
                        }
                    }
                    old_time += start.elapsed();
                } else {
                    let prepared_query = prepare(black_box(query));
                    new_preparations += 1;
                    for candidate in &candidates {
                        let candidate = prepare(black_box(candidate));
                        new_preparations += 1;
                        if let Some(score) = similarity_prepared(&prepared_query, &candidate, 0.6) {
                            new_results.push((score.to_bits(), candidate.into_normalized()));
                        }
                    }
                    new_time += start.elapsed();
                }
            }
            assert_eq!(black_box(&old_results), black_box(&new_results));
            accepted = old_results.len();
        }
        println!("6000 candidates, {rounds} alternating rounds, mean/request: old={:.3} ms, prepared={:.3} ms, speedup={:.3}x; input preparations old={}, new={} (query 6000 -> 1, candidates=6000); normalizations old={}, new={}; accepted={accepted}",
            old_time.as_secs_f64() * 1000.0 / rounds as f64,
            new_time.as_secs_f64() * 1000.0 / rounds as f64,
            old_time.as_secs_f64() / new_time.as_secs_f64(),
            old_preparations / rounds, new_preparations / rounds,
            old_preparations / rounds + accepted, new_preparations / rounds);
    }
}
