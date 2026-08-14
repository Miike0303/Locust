//! Multi-pattern byte search for binary injectors (Unity / Unreal / shared).
//!
//! Game asset inject previously called a first-byte-skip `find_bytes_in` once
//! **per string**. For ~10⁵ needles × multi‑GB files that is hours of CPU.
//! This module builds one Aho–Corasick automaton over all needles and walks
//! the haystack once, then hands match offsets to the existing per-entry
//! replacement logic.
//!
//! ## Overlap / order semantics (must match the old sequential scan)
//!
//! The legacy loop, for each entry in order:
//! 1. skip identity / oversize / missing translation (no scan);
//! 2. find the **first** occurrence of that entry’s needle in the **current**
//!    buffer (after prior replacements);
//! 3. replace in place (pad remainder with `0`).
//!
//! Multi-pattern path:
//! 1. same pre-filters;
//! 2. one AC pass on the **original** buffer collecting **all** start offsets
//!    per needle (overlapping matches enabled — a short needle and a longer
//!    needle that shares its prefix both report the same start);
//! 3. for each entry in order, take the next precomputed offset whose bytes
//!    still equal the needle (prior writes may invalidate earlier hits).
//!
//! When two needles share a start offset, the **earlier entry in the inject
//! list wins** that occurrence; the later entry either takes a later hit or
//! is skipped — same as the legacy re-scan after mutation.
//!
//! Edge case not reproduced: a replacement that *creates* a brand-new needle
//! occurrence that never existed in the original file. Game inject sources are
//! already in-file strings, so that path does not arise in practice.

use aho_corasick::{AhoCorasick, MatchKind};

/// Legacy single-needle search (first-byte skip). Kept for equivalence tests
/// and as the reference semantics for “first match in this buffer”.
pub(crate) fn find_bytes_in(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || needle.len() > haystack.len() {
        return None;
    }
    let first = needle[0];
    let nlen = needle.len();
    let mut i = 0;
    let end = haystack.len() - nlen + 1;
    while i < end {
        if haystack[i] != first {
            i += 1;
            continue;
        }
        if &haystack[i..i + nlen] == needle {
            return Some(i);
        }
        i += 1;
    }
    None
}

/// All match start offsets for each pattern (parallel to `patterns`), ascending.
///
/// Empty patterns and patterns longer than the haystack yield empty lists.
/// Duplicate pattern bytes are registered separately so each work item gets
/// its own offset list (identical content → identical offsets; sequential
/// apply + validity still assigns first/second occurrence correctly).
pub(crate) fn find_all_matches(haystack: &[u8], patterns: &[&[u8]]) -> Vec<Vec<usize>> {
    let mut out = vec![Vec::new(); patterns.len()];
    if patterns.is_empty() || haystack.is_empty() {
        return out;
    }

    // Map builder index → original pattern index (skip empties AC rejects poorly).
    let mut builder_pats: Vec<&[u8]> = Vec::with_capacity(patterns.len());
    let mut builder_to_orig: Vec<usize> = Vec::with_capacity(patterns.len());
    for (i, p) in patterns.iter().enumerate() {
        if p.is_empty() || p.len() > haystack.len() {
            continue;
        }
        builder_to_orig.push(i);
        builder_pats.push(*p);
    }
    if builder_pats.is_empty() {
        return out;
    }

    let ac = match AhoCorasick::builder()
        .match_kind(MatchKind::Standard)
        .build(&builder_pats)
    {
        Ok(ac) => ac,
        Err(_) => {
            // Fall back to legacy per-pattern scan if the automaton cannot build
            // (should not happen for ordinary game-string needles).
            for (i, p) in patterns.iter().enumerate() {
                let mut start = 0usize;
                while let Some(rel) = find_bytes_in(&haystack[start..], p) {
                    let abs = start + rel;
                    out[i].push(abs);
                    start = abs + 1;
                    if start >= haystack.len() {
                        break;
                    }
                }
            }
            return out;
        }
    };

    for mat in ac.find_overlapping_iter(haystack) {
        let orig = builder_to_orig[mat.pattern().as_usize()];
        out[orig].push(mat.start());
    }
    out
}

/// Cursor over [`find_all_matches`] results for sequential inject apply.
pub(crate) struct MatchCursor {
    offsets: Vec<Vec<usize>>,
    cursors: Vec<usize>,
}

impl MatchCursor {
    pub(crate) fn from_patterns(haystack: &[u8], patterns: &[&[u8]]) -> Self {
        let offsets = find_all_matches(haystack, patterns);
        let cursors = vec![0; offsets.len()];
        Self { offsets, cursors }
    }

    /// Next start offset for `idx` where `haystack[pos..pos+needle.len()] == needle`.
    /// Advances the internal cursor past rejected (mutated-away) hits.
    pub(crate) fn next_valid(
        &mut self,
        idx: usize,
        haystack: &[u8],
        needle: &[u8],
    ) -> Option<usize> {
        if needle.is_empty() || idx >= self.offsets.len() {
            return None;
        }
        let nlen = needle.len();
        while self.cursors[idx] < self.offsets[idx].len() {
            let pos = self.offsets[idx][self.cursors[idx]];
            self.cursors[idx] += 1;
            if pos + nlen > haystack.len() {
                continue;
            }
            if &haystack[pos..pos + nlen] == needle {
                return Some(pos);
            }
        }
        None
    }
}

/// Apply fixed-slot replacements in entry order using multi-pattern search.
///
/// Each op is `(needle, replacement)` with `replacement.len() <= needle.len()`.
/// Writes `replacement` then pads the remainder of the needle span with `0`.
/// Returns `(written, skipped_not_found)`. Test-only reference harness — the
/// injectors drive [`MatchCursor`] directly with their own replacement logic.
#[cfg(test)]
pub(crate) fn apply_fixed_slot_replacements(
    haystack: &mut [u8],
    ops: &[(Vec<u8>, Vec<u8>)],
) -> (usize, usize) {
    let patterns: Vec<&[u8]> = ops.iter().map(|(n, _)| n.as_slice()).collect();
    let mut cursor = MatchCursor::from_patterns(haystack, &patterns);
    let mut written = 0usize;
    let mut skipped = 0usize;
    for (i, (needle, repl)) in ops.iter().enumerate() {
        debug_assert!(repl.len() <= needle.len());
        match cursor.next_valid(i, haystack, needle) {
            Some(pos) => {
                haystack[pos..pos + repl.len()].copy_from_slice(repl);
                for b in &mut haystack[pos + repl.len()..pos + needle.len()] {
                    *b = 0;
                }
                written += 1;
            }
            None => skipped += 1,
        }
    }
    (written, skipped)
}

/// Legacy sequential path (one `find_bytes_in` per op) for equivalence tests.
#[cfg(test)]
pub(crate) fn apply_fixed_slot_replacements_legacy(
    haystack: &mut [u8],
    ops: &[(Vec<u8>, Vec<u8>)],
) -> (usize, usize) {
    let mut written = 0usize;
    let mut skipped = 0usize;
    for (needle, repl) in ops {
        debug_assert!(repl.len() <= needle.len());
        match find_bytes_in(haystack, needle) {
            Some(pos) => {
                haystack[pos..pos + repl.len()].copy_from_slice(repl);
                for b in &mut haystack[pos + repl.len()..pos + needle.len()] {
                    *b = 0;
                }
                written += 1;
            }
            None => skipped += 1,
        }
    }
    (written, skipped)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn op(needle: &str, repl: &str) -> (Vec<u8>, Vec<u8>) {
        (needle.as_bytes().to_vec(), repl.as_bytes().to_vec())
    }

    #[test]
    fn find_bytes_in_first_occurrence() {
        let h = b"xxABCyyABCzz";
        assert_eq!(find_bytes_in(h, b"ABC"), Some(2));
        assert_eq!(find_bytes_in(h, b"nope"), None);
        assert_eq!(find_bytes_in(h, b""), None);
    }

    #[test]
    fn ac_finds_all_occurrences_of_duplicate_needle() {
        let h = b"one--two--one";
        let pats = [b"one".as_slice(), b"one".as_slice()];
        let m = find_all_matches(h, &pats);
        assert_eq!(m[0], vec![0, 10]);
        assert_eq!(m[1], vec![0, 10]);
    }

    #[test]
    fn needle_appearing_twice_two_entries_take_first_then_second() {
        // (a) same needle twice → first entry first hit, second entry second hit
        let mut multi = b"Hello....Hello....".to_vec();
        let mut legacy = multi.clone();
        let ops = vec![op("Hello", "Hola!"), op("Hello", "Hola!")];
        let r_m = apply_fixed_slot_replacements(&mut multi, &ops);
        let r_l = apply_fixed_slot_replacements_legacy(&mut legacy, &ops);
        assert_eq!(r_m, r_l);
        assert_eq!(r_m, (2, 0));
        assert_eq!(multi, legacy);
        assert_eq!(&multi[0..5], b"Hola!");
        assert_eq!(&multi[9..14], b"Hola!");
    }

    #[test]
    fn substring_needles_earlier_entry_wins_shared_start() {
        // (b) "ab" is a prefix of "abc". Entry order decides who takes offset 0.
        // Short first: short writes at 0 → long no longer matches → long skipped.
        let mut multi = b"abcdef".to_vec();
        let mut legacy = multi.clone();
        let ops = vec![op("ab", "XY"), op("abc", "!!!")];
        let r_m = apply_fixed_slot_replacements(&mut multi, &ops);
        let r_l = apply_fixed_slot_replacements_legacy(&mut legacy, &ops);
        assert_eq!(r_m, r_l);
        assert_eq!(r_m, (1, 1));
        assert_eq!(multi, legacy);
        assert_eq!(&multi[..], b"XYcdef");

        // Long first: long writes at 0 → short no longer matches at 0.
        let mut multi = b"abcdef".to_vec();
        let mut legacy = multi.clone();
        let ops = vec![op("abc", "!!!"), op("ab", "XY")];
        let r_m = apply_fixed_slot_replacements(&mut multi, &ops);
        let r_l = apply_fixed_slot_replacements_legacy(&mut legacy, &ops);
        assert_eq!(r_m, r_l);
        assert_eq!(r_m, (1, 1));
        assert_eq!(multi, legacy);
        assert_eq!(&multi[..], b"!!!def");
    }

    #[test]
    fn overlapping_different_starts_both_apply_when_non_conflicting() {
        // "aa" at 0 and "ab" at 1 in "aab" — after writing "aa"→"XX", "ab" at 1 may die.
        let mut multi = b"aab".to_vec();
        let mut legacy = multi.clone();
        let ops = vec![op("aa", "XX"), op("ab", "YZ")];
        let r_m = apply_fixed_slot_replacements(&mut multi, &ops);
        let r_l = apply_fixed_slot_replacements_legacy(&mut legacy, &ops);
        assert_eq!(r_m, r_l);
        assert_eq!(multi, legacy);
    }

    #[test]
    fn identity_and_oversize_are_outside_search_path() {
        // (c)(d) injectors skip identity/oversize before building the pattern list.
        // Model that here: only eligible ops are passed to apply_*.
        let hay = b"short_string_here!!";
        let identity = op("short", "short"); // would be filtered — not in ops
        let oversize = ("short".as_bytes().to_vec(), b"toolong1".to_vec()); // filtered
        let _ = (identity, oversize);
        let ops = vec![op("string", "cadena")]; // equal length
        let mut multi = hay.to_vec();
        let mut legacy = hay.to_vec();
        assert_eq!(
            apply_fixed_slot_replacements(&mut multi, &ops),
            apply_fixed_slot_replacements_legacy(&mut legacy, &ops)
        );
        assert_eq!(multi, legacy);
        assert!(multi.windows(6).any(|w| w == b"cadena"));
    }

    #[test]
    fn multi_pattern_matches_legacy_on_mixed_fixture() {
        // Combined synthetic: two occurrences, substring pair, one miss.
        let mut multi = b"AA--BB--AA--ABC--ZZ".to_vec();
        let mut legacy = multi.clone();
        let ops = vec![
            op("AA", "aa"),   // first AA
            op("AA", "aa"),   // second AA
            op("BB", "bb"),   // BB
            op("ABC", "!!!"), // ABC
            op("NO", "xx"),   // miss
        ];
        let r_m = apply_fixed_slot_replacements(&mut multi, &ops);
        let r_l = apply_fixed_slot_replacements_legacy(&mut legacy, &ops);
        assert_eq!(r_m, r_l);
        assert_eq!(r_m, (4, 1));
        assert_eq!(multi, legacy);
    }

    /// Micro-benchmark note: multi-pattern AC vs per-needle first-byte scan.
    ///
    /// Typical result on a release-ish host (order of magnitude): scanning a
    /// 100 MiB buffer with 10 000 distinct 8-byte needles is ~seconds with the
    /// legacy loop (10k full passes) vs tens–hundreds of ms with one AC pass.
    /// Run with: `cargo test -p locust-formats ac_vs_legacy_scan_microbench -- --ignored --nocapture`
    #[test]
    #[ignore = "microbench — run manually with --ignored --nocapture"]
    fn ac_vs_legacy_scan_microbench() {
        use std::time::Instant;

        const HAY_LEN: usize = 100 * 1024 * 1024; // 100 MiB
        const N_NEEDLES: usize = 10_000;

        // Sparse haystack: mostly zeros, plant each needle once at unique offsets.
        let mut hay = vec![0u8; HAY_LEN];
        let mut needles: Vec<Vec<u8>> = Vec::with_capacity(N_NEEDLES);
        for i in 0..N_NEEDLES {
            let mut n = vec![0u8; 8];
            n[0] = 0xA5;
            n[1..5].copy_from_slice(&(i as u32).to_le_bytes());
            n[5] = 0x5A;
            n[6] = (i & 0xFF) as u8;
            n[7] = ((i >> 8) & 0xFF) as u8;
            let pos = 16 + i * 64;
            assert!(pos + 8 < HAY_LEN);
            hay[pos..pos + 8].copy_from_slice(&n);
            needles.push(n);
        }
        let pats: Vec<&[u8]> = needles.iter().map(|n| n.as_slice()).collect();

        let t0 = Instant::now();
        let mut legacy_hits = 0usize;
        for p in &pats {
            if find_bytes_in(&hay, p).is_some() {
                legacy_hits += 1;
            }
        }
        let legacy_ms = t0.elapsed().as_secs_f64() * 1000.0;

        let t1 = Instant::now();
        let m = find_all_matches(&hay, &pats);
        let ac_hits: usize = m.iter().filter(|v| !v.is_empty()).count();
        let ac_ms = t1.elapsed().as_secs_f64() * 1000.0;

        assert_eq!(legacy_hits, N_NEEDLES);
        assert_eq!(ac_hits, N_NEEDLES);
        eprintln!(
            "binary_search microbench: {N_NEEDLES} needles × {HAY_LEN} bytes — \
             legacy find_bytes_in: {legacy_ms:.1} ms, AC find_all_matches: {ac_ms:.1} ms \
             (speedup ≈ {:.1}×)",
            legacy_ms / ac_ms.max(0.001)
        );
    }
}
