use std::path::PathBuf;
use std::time::Instant;

use locust_core::models::StringEntry;
use locust_core::text_fit::{
    eligible_entry, estimate_entry, estimate_texts, EstimateStatus, TextFitCache, TextFitRequest,
    TextFitResponse, MAX_INPUT_SCALARS,
};

fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/proportional-latin.ttf")
}

fn ascii_fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../cli/tests/fixtures/synthetic-ascii.ttf")
}

fn entry(source: &str) -> StringEntry {
    StringEntry::new(
        "Map001.json#0#event_1#page_0#cmd_2#msg",
        source,
        PathBuf::from("Map001.json"),
    )
    .with_format_id("rpgmaker-mv")
    .with_tags(vec!["dialogue".into()])
}

fn request(entry: &StringEntry, draft: &str) -> TextFitRequest {
    TextFitRequest {
        entry_id: entry.id.clone(),
        font_path: fixture(),
        font_size_px: 20.0,
        usable_width_px: 30.0,
        draft_text: Some(draft.into()),
    }
}

fn measure(source: &str, draft: &str, size: f64, width: f64) -> TextFitResponse {
    estimate_texts(
        &std::fs::read(fixture()).unwrap(),
        size,
        width,
        source,
        draft,
    )
    .unwrap()
}

fn unavailable(response: &TextFitResponse, reason: &str) {
    for line in response.source.iter().chain(&response.draft) {
        assert_eq!(line.status, EstimateStatus::Unavailable);
        assert_eq!(line.advance_px, None);
        assert!(line.reason.as_deref().unwrap().contains(reason), "{line:?}");
    }
}

#[test]
fn proportional_advances_distinguish_equal_scalar_counts() {
    let result = measure("iii", "WWW", 20.0, 30.0);
    assert_eq!(result.source[0].advance_px, Some(12.0));
    assert_eq!(result.draft[0].advance_px, Some(54.0));
    assert_eq!(result.source[0].status, EstimateStatus::WithinWidth);
    assert_eq!(result.draft[0].status, EstimateStatus::ExceedsWidth);
}

#[test]
fn existing_fixed_fixture_anchors_independent_arithmetic() {
    let data = std::fs::read(ascii_fixture()).unwrap();
    let result = estimate_texts(&data, 28.0, 100.0, "Hola\n \n", "iii\nWWW").unwrap();
    assert_eq!(
        result
            .source
            .iter()
            .map(|l| l.advance_px)
            .collect::<Vec<_>>(),
        vec![Some(56.0), Some(14.0), Some(0.0)]
    );
    assert_eq!(result.draft[0].advance_px, Some(42.0));
    assert_eq!(result.draft[1].advance_px, Some(42.0));
}

#[test]
fn boundaries_scaling_and_fractional_advances_are_preserved() {
    assert_eq!(
        measure("WWW", "", 20.0, 54.0).source[0].status,
        EstimateStatus::WithinWidth
    );
    assert_eq!(
        measure("WWW", "", 20.0, 53.9).source[0].status,
        EstimateStatus::ExceedsWidth
    );
    assert_eq!(
        measure("WWW", "", 40.0, 200.0).source[0].advance_px,
        Some(108.0)
    );
    assert_eq!(
        measure("iii", "", 20.5, 30.0).source[0].advance_px,
        Some(12.3)
    );
}

#[test]
fn line_breaks_spaces_and_trailing_blanks_are_not_rewritten() {
    let result = measure(" i \r\n\r\nW\n", "", 20.0, 30.0);
    assert_eq!(
        result
            .source
            .iter()
            .map(|l| l.advance_px)
            .collect::<Vec<_>>(),
        vec![Some(24.0), Some(0.0), Some(18.0), Some(0.0)]
    );
    assert_eq!(result.draft[0].reason.as_deref(), Some("no_text"));
    unavailable(&measure("\r", "\r", 20.0, 30.0), "U+000D");
}

#[test]
fn source_and_draft_availability_are_independent() {
    let result = measure("あ", "Hola", 20.0, 100.0);
    assert_eq!(result.source[0].status, EstimateStatus::Unavailable);
    assert_eq!(result.draft[0].advance_px, Some(40.0));
    let result = measure("Hello\nあ", "\t\nWWW", 20.0, 100.0);
    assert_eq!(result.source[0].advance_px, Some(50.0));
    assert_eq!(result.draft[1].advance_px, Some(54.0));
}

#[test]
fn selected_face_only_and_precomposed_latin_coverage() {
    assert_eq!(
        measure("éñĀ—", "", 20.0, 100.0).source[0].advance_px,
        Some(40.0)
    );
    let result = estimate_texts(
        &std::fs::read(ascii_fixture()).unwrap(),
        20.0,
        100.0,
        "é",
        "é",
    )
    .unwrap();
    unavailable(&result, "missing_glyph: U+00E9");
    unavailable(&measure("ȳ", "ȳ", 20.0, 100.0), "missing_glyph: U+0233");
}

#[test]
fn all_backslash_syntax_including_malformed_and_literal_is_unavailable() {
    for text in [
        r"\V[1]",
        r"\N[1]",
        r"\P[1]",
        r"\G",
        r"\I[1]",
        r"\{",
        r"\}",
        r"\C[1]",
        r"\.",
        r"\|",
        r"\!",
        r"\>",
        r"\<",
        r"\^",
        r"\$",
        r"\\",
        r"\n<Name>",
        r"\V[",
        r"\plugin[x]",
        "\\",
    ] {
        unavailable(&measure(text, text, 20.0, 100.0), "backslash_control");
    }
}

#[test]
fn marks_complex_scripts_emoji_controls_and_markup_are_unavailable() {
    for (text, reason) in [
        ("\t", "tab"),
        ("e\u{301}", "U+0301"),
        ("😀", "U+1F600"),
        ("مرحبا", "U+0645"),
        ("אב", "U+05D0"),
        ("अ", "U+0905"),
        ("\u{200d}", "U+200D"),
        ("\u{fe0f}", "U+FE0F"),
        ("\u{202e}", "U+202E"),
        ("\0", "U+0000"),
        ("<name>Alice</name>", "inline_markup"),
        ("<font", "inline_markup"),
        ("[b]Hi[/b]", "inline_markup"),
        ("[color=red]Hi[/color]", "inline_markup"),
        ("{name}", "inline_markup"),
        ("{{name}}", "inline_markup"),
    ] {
        unavailable(&measure(text, text, 20.0, 100.0), reason);
    }
}

#[test]
fn only_provenance_kind_and_owned_message_locator_admit_a_row() {
    let valid = entry("Hello");
    assert!(eligible_entry(&valid));
    let mut renamed = valid.clone();
    renamed.file_path = "unrelated.bin".into();
    assert!(eligible_entry(&renamed));
    for format in ["", "rpgmaker-vxa", "rpgmaker-xp", "lcf", "unknown"] {
        assert!(!eligible_entry(&valid.clone().with_format_id(format)));
    }
    let mut no_provenance = valid.clone();
    no_provenance.metadata.clear();
    assert!(!eligible_entry(&no_provenance));
    for tag in [
        "scroll_text",
        "choice",
        "name",
        "description",
        "plugin",
        "pack",
        "Dialogue",
    ] {
        assert!(!eligible_entry(&valid.clone().with_tags(vec![tag.into()])));
    }
    for id in [
        "Map001.json#cmd_2#speaker",
        "Items.json#0#description",
        "System.json#0#message1",
        "Pack.json#cmd_x#msg",
        "Map001.json#cmd_2#msg#extra",
        "Map001.json#msg",
    ] {
        let mut candidate = valid.clone();
        candidate.id = id.into();
        assert!(!eligible_entry(&candidate), "{id}");
    }
}

#[test]
fn entry_guard_rejects_wrong_id_or_ineligible_row() {
    let mut row = entry("Hello");
    let mut req = request(&row, "Hola");
    req.entry_id = "other".into();
    assert!(estimate_entry(&row, &req, &mut TextFitCache::default()).is_err());
    req.entry_id = row.id.clone();
    row.tags.clear();
    assert!(estimate_entry(&row, &req, &mut TextFitCache::default()).is_err());
}

#[test]
fn invalid_profile_cannot_be_bypassed_by_empty_text() {
    let font = std::fs::read(fixture()).unwrap();
    for invalid in [0.0, -1.0, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        unavailable(
            &estimate_texts(&font, invalid, 30.0, "", "").unwrap(),
            "invalid_font_size",
        );
        unavailable(
            &estimate_texts(&font, 20.0, invalid, "", "").unwrap(),
            "invalid_usable_width",
        );
    }
    unavailable(
        &estimate_texts(&font, f64::MAX, 30.0, "WWW", "WWW").unwrap(),
        "nonfinite_advance",
    );
}

#[test]
fn malformed_web_collection_variable_and_missing_metrics_fonts_are_unavailable() {
    for bytes in [
        vec![],
        vec![0, 1, 0, 0],
        b"wOFFinvalid".to_vec(),
        b"wOF2invalid".to_vec(),
        b"ttcfinvalid".to_vec(),
    ] {
        unavailable(&estimate_texts(&bytes, 20.0, 30.0, "", "").unwrap(), "font");
    }
    let font = std::fs::read(fixture()).unwrap();
    for (old, new, reason) in [
        (b"hmtx", b"xxxx", "missing_metrics"),
        (b"name", b"fvar", "variable_font"),
    ] {
        let mut altered = font.clone();
        let record = altered[12..]
            .chunks_exact(16)
            .position(|record| &record[..4] == old)
            .unwrap();
        altered[12 + record * 16..16 + record * 16].copy_from_slice(new);
        unavailable(
            &estimate_texts(&altered, 20.0, 30.0, "", "").unwrap(),
            reason,
        );
    }
}

#[test]
fn explicit_font_path_extension_and_existing_read_limit_are_enforced() {
    let row = entry("");
    let mut req = request(&row, "");
    let mut cache = TextFitCache::default();
    req.font_path = "".into();
    unavailable(
        &estimate_entry(&row, &req, &mut cache).unwrap(),
        "font_not_selected",
    );
    req.font_path = fixture().with_file_name("nonexistent.ttf");
    unavailable(
        &estimate_entry(&row, &req, &mut cache).unwrap(),
        "font_read_failed",
    );
    req.font_path = fixture().with_extension("woff");
    unavailable(
        &estimate_entry(&row, &req, &mut cache).unwrap(),
        "unsupported_font_format",
    );
    let directory = tempfile::tempdir().unwrap();
    req.font_path = directory.path().join("oversized.ttf");
    std::fs::File::create(&req.font_path)
        .unwrap()
        .set_len(64 * 1024 * 1024 + 1)
        .unwrap();
    unavailable(&estimate_entry(&row, &req, &mut cache).unwrap(), "64 MiB");
}

#[test]
fn combined_scalar_ceiling_includes_line_separators() {
    let font = std::fs::read(fixture()).unwrap();
    assert!(estimate_texts(&font, 20.0, 30.0, &"i".repeat(MAX_INPUT_SCALARS), "").is_ok());
    assert!(estimate_texts(&font, 20.0, 30.0, &"i".repeat(MAX_INPUT_SCALARS), "i").is_err());
    assert!(estimate_texts(&font, 20.0, 30.0, &"\n".repeat(MAX_INPUT_SCALARS), "i").is_err());
    assert!(estimate_texts(&font, 20.0, 30.0, &"é".repeat(MAX_INPUT_SCALARS), "").is_ok());
}

#[test]
fn request_is_read_only_and_optional_draft_uses_translation() {
    let mut row = entry("Hello\r\n");
    row.translation = Some("Hola\n".into());
    let before = serde_json::to_vec(&row).unwrap();
    let mut req = request(&row, "");
    req.draft_text = None;
    let req_before = serde_json::to_vec(&req).unwrap();
    let result = estimate_entry(&row, &req, &mut TextFitCache::default()).unwrap();
    assert_eq!(result.draft[0].advance_px, Some(40.0));
    assert_eq!(result.draft.len(), 2);
    assert_eq!(serde_json::to_vec(&row).unwrap(), before);
    assert_eq!(serde_json::to_vec(&req).unwrap(), req_before);
}

#[test]
fn cache_uses_content_and_settings_boundedly_without_stale_text() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("chosen.ttf");
    std::fs::copy(fixture(), &path).unwrap();
    let row = entry("iii");
    let mut req = request(&row, "WWW");
    req.font_path = path.clone();
    let mut cache = TextFitCache::default();
    assert_eq!(
        estimate_entry(&row, &req, &mut cache).unwrap().draft[0].advance_px,
        Some(54.0)
    );
    req.draft_text = Some("i".into());
    assert_eq!(
        estimate_entry(&row, &req, &mut cache).unwrap().draft[0].advance_px,
        Some(4.0)
    );
    assert_eq!(cache.stats().font_parses, 1);
    assert_eq!(cache.stats().font_reads, 1);
    req.font_size_px = 40.0;
    assert_eq!(
        estimate_entry(&row, &req, &mut cache).unwrap().source[0].advance_px,
        Some(24.0)
    );
    std::fs::copy(ascii_fixture(), &path).unwrap();
    assert_eq!(
        estimate_entry(&row, &req, &mut cache).unwrap().source[0].advance_px,
        Some(60.0)
    );
    assert_eq!(cache.stats().font_parses, 2);
    for size in 1..=12 {
        req.font_size_px = size as f64;
        estimate_entry(&row, &req, &mut cache).unwrap();
    }
    assert!(cache.stats().cached_profiles <= 4);
    assert!(cache.stats().cached_paths <= 4);
}

#[test]
fn serialized_status_is_shared_backend_contract() {
    let value = serde_json::to_value(measure("iii\nあ", "WWW", 20.0, 30.0)).unwrap();
    assert_eq!(value["source"][0]["status"], "within_width");
    assert_eq!(value["source"][1]["status"], "unavailable");
    assert!(value["source"][1]["advance_px"].is_null());
    assert_eq!(value["draft"][0]["status"], "exceeds_width");
}

#[test]
fn bounded_repeated_distinct_cold_warm_timings() {
    let distinct = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789 éñĀ—";
    for count in [1024, 4096, MAX_INPUT_SCALARS] {
        for (kind, source) in [
            ("repeated", "i".repeat(count)),
            ("distinct", distinct.chars().cycle().take(count).collect()),
        ] {
            let mut cache = TextFitCache::default();
            let row = entry(&source);
            let req = request(&row, "");
            let start = Instant::now();
            estimate_entry(&row, &req, &mut cache).unwrap();
            let cold = start.elapsed();
            let start = Instant::now();
            let result = estimate_entry(&row, &req, &mut cache).unwrap();
            assert!(result.source[0].advance_px.unwrap() > 0.0);
            eprintln!(
                "text-fit {count} {kind}: cold={cold:?}, warm={:?}",
                start.elapsed()
            );
            eprintln!("text-fit cache: {:?}", cache.stats());
            assert_eq!(cache.stats().font_parses, 1);
            assert_eq!(cache.stats().font_reads, 1);
        }
    }
}
