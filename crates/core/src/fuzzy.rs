//! Pure, bounded Unicode character comparisons for translation memory.

pub const MAX_CHARS: usize = 2000;

/// Unicode lowercase with trimmed, single-space whitespace separators.
pub fn normalize(s: &str) -> String {
    s.split_whitespace()
        .map(str::to_lowercase)
        .collect::<Vec<_>>()
        .join(" ")
}

fn comparison_chars(s: &str) -> Vec<char> {
    // Bound the input before normalization too, including whitespace-only input.
    let prefix: String = s.chars().take(MAX_CHARS).collect();
    normalize(&prefix).chars().take(MAX_CHARS).collect()
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
    let a = comparison_chars(a);
    let b = comparison_chars(b);
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
}
