/// Reproduction test for the alleged "loose .rpy + .rpa mix drops loose
/// translations on inject" defect in the Ren'Py plugin.
///
/// Builds a game/ dir containing BOTH:
///   - game/scripts.rpa   (a minimal hand-built RPA-3.0 archive containing script.rpy)
///   - game/loose.rpy     (a loose file sitting next to the archive, not inside it)
///
/// Extracts, assigns a translation to every entry, injects, then checks
/// whether the loose file's translation actually made it to disk.
use std::fs;
use std::path::Path;

use locust_core::extraction::FormatPlugin;
use locust_core::models::StringEntry;
use locust_formats::renpy::RenPyPlugin;

/// Hand-build a minimal RPA-3.0 archive (no obfuscation key) containing the
/// given (filename, content) pairs, based on the format documented and read
/// by `RenPyPlugin::extract_rpa` / `parse_rpa_pickle`:
///   header: "RPA-3.0 <hex index_offset> <hex key>\n"
///   body:   raw concatenated file contents
///   index:  zlib-compressed pickle of { filename: [(offset, length, prefix="")] }
fn build_rpa(files: &[(&str, &[u8])]) -> Vec<u8> {
    // Placeholder header to get a stable length: 16 hex digits for the offset,
    // key fixed to "0" (no obfuscation) — both stay the same length once we
    // substitute the real offset later.
    let header_len = format!("RPA-3.0 {:016x} {:x}\n", 0u64, 0u32).len();

    let mut body = Vec::new();
    let mut index_entries: Vec<(String, u64, usize)> = Vec::new();
    let mut pos = header_len as u64;
    for (name, content) in files {
        index_entries.push((name.to_string(), pos, content.len()));
        body.extend_from_slice(content);
        pos += content.len() as u64;
    }
    let index_offset = pos;

    // Build the pickle: PROTO 2, EMPTY_DICT, then per file:
    //   SHORT_BINUNICODE(key) EMPTY_LIST BININT(offset) BININT(length) TUPLE2 APPEND SETITEM
    let mut pickle: Vec<u8> = vec![0x80, 0x02, 0x7d]; // PROTO 2, EMPTY_DICT
    for (name, offset, length) in &index_entries {
        pickle.push(0x8c); // SHORT_BINUNICODE
        pickle.push(name.len() as u8);
        pickle.extend_from_slice(name.as_bytes());

        pickle.push(0x5d); // EMPTY_LIST
        pickle.push(0x4a); // BININT
        pickle.extend_from_slice(&(*offset as i32).to_le_bytes());
        pickle.push(0x4a); // BININT
        pickle.extend_from_slice(&(*length as i32).to_le_bytes());
        pickle.push(0x86); // TUPLE2
        pickle.push(0x61); // APPEND
        pickle.push(0x73); // SETITEM
    }
    pickle.push(0x2e); // STOP

    let compressed = miniz_oxide::deflate::compress_to_vec_zlib(&pickle, 6);

    let header = format!("RPA-3.0 {:016x} {:x}\n", index_offset, 0u32);
    assert_eq!(header.len(), header_len, "header length must match placeholder");

    let mut out = Vec::new();
    out.extend_from_slice(header.as_bytes());
    out.extend_from_slice(&body);
    out.extend_from_slice(&compressed);
    out
}

#[test]
fn test_loose_rpy_translation_survives_inject_alongside_rpa() {
    let dir = std::env::temp_dir().join(format!("locust_renpy_mixed_{}", uuid::Uuid::new_v4()));
    let game_dir = dir.join("game");
    fs::create_dir_all(&game_dir).unwrap();

    // .rpy that will live INSIDE the archive.
    let archive_rpy = b"label archive_start:\n    e \"Hello from the archive!\"\n".to_vec();
    let rpa_bytes = build_rpa(&[("script.rpy", &archive_rpy)]);
    fs::write(game_dir.join("scripts.rpa"), &rpa_bytes).unwrap();

    // Loose .rpy sitting next to the archive — NOT inside it.
    let loose_content = "label loose_start:\n    e \"Hello from the loose file!\"\n";
    fs::write(game_dir.join("loose.rpy"), loose_content).unwrap();

    let plugin = RenPyPlugin::new();

    // --- EXTRACT ---
    let mut entries = plugin.extract(&dir).expect("extract failed");
    println!("extract() returned {} entries:", entries.len());
    for e in &entries {
        println!("  file_path={} id={} source={:?}", e.file_path.display(), e.id, e.source);
    }

    let archive_entry = entries
        .iter()
        .find(|e| e.source.contains("Hello from the archive"));
    let loose_entry = entries
        .iter()
        .find(|e| e.source.contains("Hello from the loose file"));

    assert!(archive_entry.is_some(), "archive dialogue line was not extracted at all");
    assert!(loose_entry.is_some(), "loose dialogue line was not extracted at all");

    // Confirm the entries are indeed a MIX of .rpa- and .rpy-sourced file_paths,
    // which is the precondition for the alleged bug.
    let has_rpa_entry = entries
        .iter()
        .any(|e| e.file_path.extension().map_or(false, |ext| ext == "rpa"));
    let has_rpy_entry = entries
        .iter()
        .any(|e| e.file_path.extension().map_or(false, |ext| ext == "rpy"));
    println!("mixed entries present: has_rpa_entry={has_rpa_entry} has_rpy_entry={has_rpy_entry}");
    assert!(has_rpa_entry && has_rpy_entry, "precondition failed: entries are not mixed rpa+rpy");

    // --- ASSIGN TRANSLATIONS to every entry ---
    for e in &mut entries {
        e.translation = Some(format!("[ES] {}", e.source));
    }

    // --- INJECT ---
    let report = plugin.inject(&dir, &entries).expect("inject failed");
    println!(
        "inject report: files_modified={} strings_written={} strings_skipped={}",
        report.files_modified, report.strings_written, report.strings_skipped
    );

    // --- CHECK OUTCOME on disk ---
    let loose_on_disk = fs::read_to_string(game_dir.join("loose.rpy")).unwrap();
    println!("game/loose.rpy on disk after inject:\n{loose_on_disk}");

    let loose_translated = loose_on_disk.contains("[ES] Hello from the loose file!");

    assert!(
        loose_translated,
        "BUG CONFIRMED: loose .rpy translation was dropped on inject. \
         game/loose.rpy still contains the untranslated source text:\n{}",
        loose_on_disk
    );
}

/// Regression test for RFIX-4: a loose game/script.rpy and an archive member
/// ALSO named script.rpy collide on the same destination path. Ren'Py always
/// loads the loose file with priority, so the archive-sourced translation for
/// this filename should never reach disk (it would clobber the loose file's
/// distinct content); the loose file's own translation must survive intact,
/// and the report must not falsely claim the archive-side translation was
/// written.
#[test]
fn test_colliding_filename_loose_translation_survives_and_report_reconciles() {
    let dir = std::env::temp_dir().join(format!("locust_renpy_collide_{}", uuid::Uuid::new_v4()));
    let game_dir = dir.join("game");
    fs::create_dir_all(&game_dir).unwrap();

    // Archive member named "script.rpy" — distinct content from the loose file below.
    let archive_rpy = b"label archive_start:\n    e \"Hello from the archive copy!\"\n".to_vec();
    let rpa_bytes = build_rpa(&[("script.rpy", &archive_rpy)]);
    fs::write(game_dir.join("scripts.rpa"), &rpa_bytes).unwrap();

    // Loose file, SAME filename "script.rpy", sitting directly in game/ — this is
    // the standard Ren'Py "loose file overrides archive member" pattern.
    let loose_content = "label loose_start:\n    e \"Hello from the loose override!\"\n";
    fs::write(game_dir.join("script.rpy"), loose_content).unwrap();

    let plugin = RenPyPlugin::new();

    // --- EXTRACT ---
    let mut entries = plugin.extract(&dir).expect("extract failed");
    let total_entries = entries.len();
    assert!(
        entries.iter().any(|e| e.source.contains("Hello from the archive copy")),
        "archive dialogue line was not extracted"
    );
    assert!(
        entries.iter().any(|e| e.source.contains("Hello from the loose override")),
        "loose dialogue line was not extracted"
    );

    // --- ASSIGN TRANSLATIONS to every entry ---
    for e in &mut entries {
        e.translation = Some(format!("[ES] {}", e.source));
    }

    // --- INJECT ---
    let report = plugin.inject(&dir, &entries).expect("inject failed");
    println!(
        "inject report: files_modified={} strings_written={} strings_skipped={} warnings={:?}",
        report.files_modified, report.strings_written, report.strings_skipped, report.warnings
    );

    // --- CHECK OUTCOME on disk ---
    let script_on_disk = fs::read_to_string(game_dir.join("script.rpy")).unwrap();
    println!("game/script.rpy on disk after inject:\n{script_on_disk}");

    assert!(
        script_on_disk.contains("[ES] Hello from the loose override!"),
        "loose file's own translation must survive the collision, but game/script.rpy contains:\n{}",
        script_on_disk
    );
    assert!(
        !script_on_disk.contains("Hello from the archive copy"),
        "the archive-sourced content must never clobber the loose file's distinct content:\n{}",
        script_on_disk
    );

    // The report must reconcile: nothing is silently dropped without being
    // accounted for as written or skipped.
    assert_eq!(
        report.strings_written + report.strings_skipped,
        total_entries,
        "written + skipped must reconcile with total extracted entries"
    );
}

/// Regression test for TFIX-1: an archive member at a SUBDIRECTORY path (e.g.
/// `scripts/day1.rpy`) whose bare basename matches an UNRELATED loose
/// `game/day1.rpy` must NOT be treated as a destination collision — the two
/// write to different final paths (`game/scripts/day1.rpy` vs
/// `game/day1.rpy`), so no real collision exists. A basename-only filter
/// (rather than a full, normalized destination-path comparison) wrongly drops
/// the archive translation here even though nothing would actually be
/// clobbered; this test fails under that basename filter because the archive
/// translation never reaches disk.
#[test]
fn test_archive_member_in_subdirectory_does_not_collide_with_same_basename_loose_file() {
    let dir = std::env::temp_dir().join(format!("locust_renpy_subdir_{}", uuid::Uuid::new_v4()));
    let game_dir = dir.join("game");
    fs::create_dir_all(&game_dir).unwrap();

    // Archive member at "scripts/day1.rpy" — a SUBDIRECTORY path.
    let archive_rpy = b"label archive_start:\n    e \"Hello from the archive subdir!\"\n".to_vec();
    let rpa_bytes = build_rpa(&[("scripts/day1.rpy", &archive_rpy)]);
    fs::write(game_dir.join("scripts.rpa"), &rpa_bytes).unwrap();

    // Loose file with the SAME BASENAME but at a DIFFERENT path: game/day1.rpy.
    // This is a different, unrelated file — not a collision at all.
    let loose_content = "label loose_start:\n    e \"Hello from the loose day1!\"\n";
    fs::write(game_dir.join("day1.rpy"), loose_content).unwrap();

    let plugin = RenPyPlugin::new();

    let mut entries = plugin.extract(&dir).expect("extract failed");
    assert!(
        entries.iter().any(|e| e.source.contains("Hello from the archive subdir")),
        "archive dialogue line was not extracted"
    );
    assert!(
        entries.iter().any(|e| e.source.contains("Hello from the loose day1")),
        "loose dialogue line was not extracted"
    );

    for e in &mut entries {
        e.translation = Some(format!("[ES] {}", e.source));
    }

    let report = plugin.inject(&dir, &entries).expect("inject failed");
    println!(
        "inject report: files_modified={} strings_written={} strings_skipped={} warnings={:?}",
        report.files_modified, report.strings_written, report.strings_skipped, report.warnings
    );

    let archive_dest = game_dir.join("scripts").join("day1.rpy");
    assert!(
        archive_dest.exists(),
        "archive translation must be written to game/scripts/day1.rpy: no real \
         destination collision exists with game/day1.rpy"
    );
    let archive_on_disk = fs::read_to_string(&archive_dest).unwrap();
    assert!(
        archive_on_disk.contains("[ES] Hello from the archive subdir!"),
        "game/scripts/day1.rpy must contain the archive-sourced translation:\n{}",
        archive_on_disk
    );

    let loose_on_disk = fs::read_to_string(game_dir.join("day1.rpy")).unwrap();
    assert!(
        loose_on_disk.contains("[ES] Hello from the loose day1!"),
        "game/day1.rpy must contain the loose translation:\n{}",
        loose_on_disk
    );
}

/// Regression test for TFIX-4: a loose entry whose line number is STALE (no
/// longer present in the current file — e.g. after an upstream edit between
/// extraction and inject) must be counted as skipped, never as written. This
/// test fails if the counting logic reverts to incrementing `strings_written`
/// up front when the (filename, line) entry is added to the lookup map,
/// instead of only after a real match against the file's current content is
/// found.
#[test]
fn test_stale_line_number_counted_as_skipped_not_written() {
    let dir = std::env::temp_dir().join(format!("locust_renpy_stale_{}", uuid::Uuid::new_v4()));
    let game_dir = dir.join("game");
    fs::create_dir_all(&game_dir).unwrap();

    let original_content = "label start:\n    e \"Hello there!\"\n";
    fs::write(game_dir.join("loose.rpy"), original_content).unwrap();

    // Line 99 does not exist in this two-line file — a stale line number.
    let mut entry = StringEntry::new(
        "loose.rpy#99",
        "Hello there!",
        game_dir.join("loose.rpy"),
    );
    entry.translation = Some("[ES] Hello there!".to_string());
    entry.tags = vec!["dialogue".to_string()];

    let plugin = RenPyPlugin::new();
    let report = plugin.inject(&dir, &[entry]).expect("inject failed");
    println!(
        "inject report: files_modified={} strings_written={} strings_skipped={}",
        report.files_modified, report.strings_written, report.strings_skipped
    );

    assert_eq!(
        report.strings_written, 0,
        "a stale line number must never be counted as written"
    );
    assert_eq!(
        report.strings_skipped, 1,
        "a stale line number must be counted as skipped"
    );
    assert_eq!(
        report.strings_written + report.strings_skipped,
        1,
        "written + skipped must reconcile with the single entry passed in"
    );

    let on_disk = fs::read_to_string(game_dir.join("loose.rpy")).unwrap();
    assert_eq!(
        on_disk, original_content,
        "file must not be rewritten when nothing was actually matched"
    );
}

/// A translation identical to its source is a no-op: the loose loop must count it
/// as skipped and leave the file byte-for-byte alone, rather than counting it
/// written and rewriting the file (which also deletes its .rpyc and forces a
/// needless recompile). This matches the identity guards already present in the
/// rpyc-filter and RPA partitions, so all three agree.
#[test]
fn test_loose_identity_translation_is_skipped_and_file_untouched() {
    let dir = std::env::temp_dir().join(format!("locust_renpy_ident_{}", uuid::Uuid::new_v4()));
    let game_dir = dir.join("game");
    fs::create_dir_all(&game_dir).unwrap();

    let original_content = "label start:\n    e \"Hello there!\"\n";
    let loose_path = game_dir.join("loose.rpy");
    fs::write(&loose_path, original_content).unwrap();

    // Sanity: the .rpyc twin must survive, since nothing should be rewritten.
    let rpyc_twin = game_dir.join("loose.rpyc");
    fs::write(&rpyc_twin, b"stale-compiled").unwrap();

    let mut entry = StringEntry::new("loose.rpy#2", "Hello there!", loose_path.clone());
    entry.translation = Some("Hello there!".to_string()); // identical to source
    entry.tags = vec!["dialogue".to_string()];

    let plugin = RenPyPlugin::new();
    let report = plugin.inject(&dir, &[entry]).expect("inject failed");
    println!(
        "inject report: files_modified={} strings_written={} strings_skipped={}",
        report.files_modified, report.strings_written, report.strings_skipped
    );

    assert_eq!(
        report.strings_written, 0,
        "an identity translation must never be counted as written"
    );
    assert_eq!(
        report.strings_skipped, 1,
        "an identity translation must be counted as skipped"
    );
    assert_eq!(
        report.files_modified, 0,
        "no file should be reported modified when nothing actually changed"
    );

    let on_disk = fs::read_to_string(&loose_path).unwrap();
    assert_eq!(
        on_disk, original_content,
        "file must be left byte-for-byte identical"
    );
    assert!(
        rpyc_twin.exists(),
        "the .rpyc twin must not be deleted when no rewrite happened"
    );
}

/// A stale-filter removal failure must SURFACE. The rpyc report is merged into
/// the aggregate report, and that merge previously copied the counters but
/// dropped the warnings, so in a mixed batch (rpyc entries plus loose ones) a
/// stale `zzz_locust_translate.rpy` that could not be deleted kept applying
/// outdated translations with nothing reported to the caller.
///
/// The removal is forced to fail portably by making the path a DIRECTORY:
/// `exists()` is true but `remove_file` cannot delete it, on every platform.
#[test]
fn test_stale_filter_removal_warning_reaches_caller_in_mixed_batch() {
    let dir = std::env::temp_dir().join(format!("locust_renpy_warn_{}", uuid::Uuid::new_v4()));
    let game_dir = dir.join("game");
    fs::create_dir_all(&game_dir).unwrap();

    let loose_path = game_dir.join("loose.rpy");
    fs::write(&loose_path, "label start:\n    e \"Hello there!\"\n").unwrap();

    // Undeletable stale filter: a directory where a file is expected.
    fs::create_dir_all(game_dir.join("zzz_locust_translate.rpy")).unwrap();

    // A loose entry keeps this a MIXED batch, so inject cannot take the
    // rpyc-only early return that used to be the sole path surfacing warnings.
    let mut loose = StringEntry::new("loose.rpy#2", "Hello there!", loose_path.clone());
    loose.translation = Some("[ES] Hello there!".to_string());
    loose.tags = vec!["dialogue".to_string()];

    // An identity rpyc entry drives the rpyc path to strings_written == 0,
    // which is the branch that attempts the stale-file removal.
    let mut rpyc = StringEntry::new("script.rpyc#0", "Unchanged", game_dir.join("script.rpyc"));
    rpyc.translation = Some("Unchanged".to_string());
    rpyc.tags = vec!["dialogue".to_string(), "rpyc".to_string()];

    let plugin = RenPyPlugin::new();
    let report = plugin.inject(&dir, &[loose, rpyc]).expect("inject failed");
    println!("warnings: {:?}", report.warnings);

    assert!(
        report
            .warnings
            .iter()
            .any(|w| w.contains("could not remove stale translation filter")),
        "the stale-filter removal failure must be surfaced to the caller even when \
         the batch also contains non-rpyc entries; got warnings: {:?}",
        report.warnings
    );

    // The loose translation must still have been applied — a warning is not an abort.
    let on_disk = fs::read_to_string(&loose_path).unwrap();
    assert!(
        on_disk.contains("[ES] Hello there!"),
        "a non-fatal stale-removal failure must not prevent loose translations"
    );
}
