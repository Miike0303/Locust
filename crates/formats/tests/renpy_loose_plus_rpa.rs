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

use locust_core::extraction::FormatPlugin;
use locust_core::models::StringEntry;
use locust_formats::renpy::RenPyPlugin;

/// The RPA-3.0 obfuscation key real Ren'Py builds ship with.
const RPA_KEY: i64 = 0x42424242;

/// BINPUT below memo index 256, LONG_BINPUT above — exactly what a real index
/// does once it holds more than ~85 members. Returns the memo index used.
fn emit_put(pickle: &mut Vec<u8>, memo: &mut u32) -> u32 {
    let idx = *memo;
    *memo += 1;
    if idx < 256 {
        pickle.push(0x71); // BINPUT
        pickle.push(idx as u8);
    } else {
        pickle.push(0x72); // LONG_BINPUT
        pickle.extend_from_slice(&idx.to_le_bytes());
    }
    idx
}

/// Python's LONG1: 1-byte length + minimal little-endian two's-complement,
/// `n = bit_length() // 8 + 1`. Fixture values are always non-negative
/// (offset/length XORed with the key).
fn emit_long1(pickle: &mut Vec<u8>, v: i64) {
    assert!(v >= 0, "fixture LONG1 values are always non-negative");
    pickle.push(0x8a);
    if v == 0 {
        pickle.push(0);
        return;
    }
    let bit_length = 64 - (v as u64).leading_zeros();
    let n = (bit_length / 8 + 1) as usize;
    pickle.push(n as u8);
    pickle.extend_from_slice(&v.to_le_bytes()[..n]);
}

/// Hand-build an RPA-3.0 archive whose index uses the SAME pickle opcodes a
/// shipped Ren'Py game emits (verified opcode-for-opcode against a real
/// 271-member scripts.rpa — see docs/vn-tools/make_real_rpa.py):
///
///   header: "RPA-3.0 <hex index_offset> <hex key>\n"
///   body:   raw concatenated file contents
///   index:  zlib-compressed pickle of { filename: [(offset^key, length^key, prefix="")] }
///
///   PROTO 2 · EMPTY_DICT · put · MARK
///   per member: BINUNICODE(name) · put · EMPTY_LIST · put ·
///               LONG1(offset^key) · BININT(length^key) ·
///               (SHORT_BINSTRING("") + put ONCE, BINGET thereafter) ·
///               TUPLE3 · APPEND
///   SETITEMS · STOP
///
/// The previous fixture used SHORT_BINUNICODE + TUPLE2 + SETITEM with key 0 —
/// a shape no shipped game produces — which is exactly how the parser passed
/// its tests while reading zero members out of every real archive.
fn build_rpa(files: &[(&str, &[u8])]) -> Vec<u8> {
    // Placeholder header to get a stable length: 16 hex digits for the offset,
    // 8 for the key — both stay the same length once the real values go in.
    let header_len = format!("RPA-3.0 {:016x} {:08x}\n", 0u64, 0u32).len();

    let mut body = Vec::new();
    let mut index_entries: Vec<(String, i64, i64)> = Vec::new();
    let mut pos = header_len as i64;
    for (name, content) in files {
        index_entries.push((name.to_string(), pos, content.len() as i64));
        body.extend_from_slice(content);
        pos += content.len() as i64;
    }
    let index_offset = pos;

    let mut pickle: Vec<u8> = vec![0x80, 0x02]; // PROTO 2
    let mut memo: u32 = 0;
    pickle.push(0x7d); // EMPTY_DICT
    emit_put(&mut pickle, &mut memo);
    pickle.push(0x28); // MARK

    // A real index writes the empty prefix string ONCE and fetches it from the
    // memo for every later member — that is where its BINGETs go.
    let mut prefix_memo: Option<u32> = None;
    for (name, offset, length) in &index_entries {
        pickle.push(0x58); // BINUNICODE — real indexes use the 4-byte form
        pickle.extend_from_slice(&(name.len() as u32).to_le_bytes());
        pickle.extend_from_slice(name.as_bytes());
        emit_put(&mut pickle, &mut memo);

        pickle.push(0x5d); // EMPTY_LIST
        emit_put(&mut pickle, &mut memo);

        emit_long1(&mut pickle, offset ^ RPA_KEY);
        pickle.push(0x4a); // BININT
        pickle.extend_from_slice(&((length ^ RPA_KEY) as i32).to_le_bytes());

        match prefix_memo {
            None => {
                pickle.push(0x55); // SHORT_BINSTRING — the empty prefix
                pickle.push(0);
                prefix_memo = Some(emit_put(&mut pickle, &mut memo));
            }
            Some(idx) if idx < 256 => {
                pickle.push(0x68); // BINGET
                pickle.push(idx as u8);
            }
            Some(idx) => {
                pickle.push(0x6a); // LONG_BINGET
                pickle.extend_from_slice(&idx.to_le_bytes());
            }
        }
        pickle.push(0x87); // TUPLE3
        pickle.push(0x61); // APPEND
    }
    pickle.push(0x75); // SETITEMS
    pickle.push(0x2e); // STOP

    let compressed = miniz_oxide::deflate::compress_to_vec_zlib(&pickle, 6);

    let header = format!("RPA-3.0 {:016x} {:08x}\n", index_offset, RPA_KEY as u32);
    assert_eq!(
        header.len(),
        header_len,
        "header length must match placeholder"
    );

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
        println!(
            "  file_path={} id={} source={:?}",
            e.file_path.display(),
            e.id,
            e.source
        );
    }

    let archive_entry = entries
        .iter()
        .find(|e| e.source.contains("Hello from the archive"));
    let loose_entry = entries
        .iter()
        .find(|e| e.source.contains("Hello from the loose file"));

    assert!(
        archive_entry.is_some(),
        "archive dialogue line was not extracted at all"
    );
    assert!(
        loose_entry.is_some(),
        "loose dialogue line was not extracted at all"
    );

    // Confirm the entries are indeed a MIX of .rpa- and .rpy-sourced file_paths,
    // which is the precondition for the alleged bug.
    let has_rpa_entry = entries
        .iter()
        .any(|e| e.file_path.extension().is_some_and(|ext| ext == "rpa"));
    let has_rpy_entry = entries
        .iter()
        .any(|e| e.file_path.extension().is_some_and(|ext| ext == "rpy"));
    println!("mixed entries present: has_rpa_entry={has_rpa_entry} has_rpy_entry={has_rpy_entry}");
    assert!(
        has_rpa_entry && has_rpy_entry,
        "precondition failed: entries are not mixed rpa+rpy"
    );

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
        entries
            .iter()
            .any(|e| e.source.contains("Hello from the archive copy")),
        "archive dialogue line was not extracted"
    );
    assert!(
        entries
            .iter()
            .any(|e| e.source.contains("Hello from the loose override")),
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
        entries
            .iter()
            .any(|e| e.source.contains("Hello from the archive subdir")),
        "archive dialogue line was not extracted"
    );
    assert!(
        entries
            .iter()
            .any(|e| e.source.contains("Hello from the loose day1")),
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
    let mut entry = StringEntry::new("loose.rpy#99", "Hello there!", game_dir.join("loose.rpy"));
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

/// Walk the fixture's decompressed index pickle and collect the SET of opcodes
/// it emits. Panics on any opcode outside the 15 a real archive uses, so the
/// fixture can never silently drift back to an unrepresentative shape.
fn index_opcode_set(rpa: &[u8]) -> std::collections::BTreeSet<u8> {
    let newline = rpa
        .iter()
        .position(|&b| b == b'\n')
        .expect("no header line");
    let header = std::str::from_utf8(&rpa[..newline]).expect("header not utf-8");
    let mut parts = header.split_whitespace();
    parts.next(); // "RPA-3.0"
    let index_offset =
        usize::from_str_radix(parts.next().expect("no index offset"), 16).expect("bad offset");
    let pickle = miniz_oxide::inflate::decompress_to_vec_zlib(&rpa[index_offset..])
        .expect("index must decompress");

    let mut ops = std::collections::BTreeSet::new();
    let mut i = 0usize;
    while i < pickle.len() {
        let op = pickle[i];
        i += 1;
        ops.insert(op);
        match op {
            0x80 | 0x71 | 0x68 => i += 1, // PROTO / BINPUT / BINGET
            0x72 | 0x6a | 0x4a => i += 4, // LONG_BINPUT / LONG_BINGET / BININT
            0x58 => {
                // BINUNICODE: u32 length + utf8
                let n = u32::from_le_bytes(pickle[i..i + 4].try_into().unwrap()) as usize;
                i += 4 + n;
            }
            0x55 | 0x8a => {
                // SHORT_BINSTRING / LONG1: u8 length + payload
                let n = pickle[i] as usize;
                i += 1 + n;
            }
            0x7d | 0x28 | 0x5d | 0x87 | 0x61 | 0x75 => {} // no argument
            0x2e => break,                                // STOP
            other => panic!(
                "fixture emitted opcode {other:#04x}, which the real archive's index never uses"
            ),
        }
    }
    ops
}

/// The fixture must emit exactly the 15 opcodes measured on a real shipped
/// archive's index (Area69 scripts.rpa, protocol 2) — nothing more, nothing
/// less. Encoders switch encodings by size, so structural resemblance is not
/// enough: the opcode histogram is what decides representativeness.
#[test]
fn test_fixture_index_opcode_set_matches_real_archive() {
    // 152 members: enough for the memo index to pass 255 (LONG_BINPUT) and for
    // the shared empty prefix to be fetched via BINGET — two details that only
    // appear at scale and that a two-entry fixture silently misses.
    let contents: Vec<(String, Vec<u8>)> = (0..152)
        .map(|i| {
            (
                format!("kNPCs/npc_{i:03}.rpy"),
                format!("label npc_{i:03}:\n    e \"NPC {i:03} says something unique.\"\n")
                    .into_bytes(),
            )
        })
        .collect();
    let files: Vec<(&str, &[u8])> = contents
        .iter()
        .map(|(n, c)| (n.as_str(), c.as_slice()))
        .collect();
    let rpa_bytes = build_rpa(&files);

    let expected: std::collections::BTreeSet<u8> = [
        0x80, // PROTO
        0x7d, // EMPTY_DICT
        0x71, // BINPUT
        0x72, // LONG_BINPUT
        0x28, // MARK
        0x58, // BINUNICODE
        0x5d, // EMPTY_LIST
        0x8a, // LONG1
        0x4a, // BININT
        0x55, // SHORT_BINSTRING
        0x68, // BINGET
        0x87, // TUPLE3
        0x61, // APPEND
        0x75, // SETITEMS
        0x2e, // STOP
    ]
    .into_iter()
    .collect();
    assert_eq!(
        index_opcode_set(&rpa_bytes),
        expected,
        "fixture index must use exactly the opcode set a real archive emits"
    );
}

/// Regression test for the field failure: a real-shaped index (BINUNICODE
/// filenames, LONG1 offsets, memoized prefix, SETITEMS batch) at a scale where
/// LONG_BINPUT and BINGET appear must have EVERY member extracted, each entry
/// carrying the .rpa as its file_path.
#[test]
fn test_real_shape_152_member_archive_extracts_every_member() {
    let dir = std::env::temp_dir().join(format!("locust_renpy_scale_{}", uuid::Uuid::new_v4()));
    let game_dir = dir.join("game");
    fs::create_dir_all(&game_dir).unwrap();

    let contents: Vec<(String, Vec<u8>)> = (0..152)
        .map(|i| {
            (
                format!("kNPCs/npc_{i:03}.rpy"),
                format!("label npc_{i:03}:\n    e \"NPC {i:03} says something unique.\"\n")
                    .into_bytes(),
            )
        })
        .collect();
    let files: Vec<(&str, &[u8])> = contents
        .iter()
        .map(|(n, c)| (n.as_str(), c.as_slice()))
        .collect();
    let rpa_path = game_dir.join("scripts.rpa");
    fs::write(&rpa_path, build_rpa(&files)).unwrap();

    let plugin = RenPyPlugin::new();
    let entries = plugin.extract(&rpa_path).expect("extract failed");

    for i in 0..152 {
        let needle = format!("NPC {i:03} says something unique.");
        assert!(
            entries.iter().any(|e| e.source == needle),
            "member {i} dialogue missing — index not fully parsed (got {} entries)",
            entries.len()
        );
    }
    assert!(
        entries.iter().all(|e| e.file_path == rpa_path),
        "every archive entry must carry the .rpa as its file_path"
    );
}

/// Build a complete RPA-3.0 archive around an arbitrary raw index pickle.
/// `body` is the concatenated member contents (may be empty for a memberless
/// index); the index offset in the header points just past it.
fn wrap_index_pickle(pickle: &[u8], body: &[u8]) -> Vec<u8> {
    let header_len = format!("RPA-3.0 {:016x} {:08x}\n", 0u64, 0u32).len();
    let index_offset = header_len + body.len();
    let compressed = miniz_oxide::deflate::compress_to_vec_zlib(pickle, 6);
    let header = format!(
        "RPA-3.0 {:016x} {:08x}\n",
        index_offset as u64, RPA_KEY as u32
    );
    assert_eq!(
        header.len(),
        header_len,
        "header length must match placeholder"
    );
    let mut out = header.into_bytes();
    out.extend_from_slice(body);
    out.extend_from_slice(&compressed);
    out
}

/// A structurally valid archive whose index is a genuinely EMPTY dict —
/// `PROTO · EMPTY_DICT · STOP`, exactly what rpatool or a placeholder archive
/// produces from `{}`. Direct extraction of a named .rpa that can only ever
/// yield zero strings must still fail loudly (directory scans degrade this to
/// a warning) — but since the parse was CLEAN, the message must say the
/// archive is empty, not blame unsupported opcodes: a derailed parse is now a
/// hard error of its own that names the offending opcode.
#[test]
fn test_index_with_zero_members_is_a_loud_error_not_empty_success() {
    let dir = std::env::temp_dir().join(format!("locust_renpy_empty_{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&dir).unwrap();

    let pickle: Vec<u8> = vec![0x80, 0x02, 0x7d, 0x2e]; // PROTO 2 · EMPTY_DICT · STOP
    let rpa_path = dir.join("scripts.rpa");
    fs::write(&rpa_path, wrap_index_pickle(&pickle, b"")).unwrap();

    let plugin = RenPyPlugin::new();
    let err = plugin
        .extract(&rpa_path)
        .expect_err("a memberless index must be reported loudly, not returned as zero strings");
    let msg = err.to_string();
    assert!(
        msg.contains("zero members"),
        "the error must say the index parsed to zero members, got: {msg}"
    );
    assert!(
        !msg.contains("does not support"),
        "a cleanly parsed empty index must not claim unsupported opcodes — that \
         diagnosis is false for this input, got: {msg}"
    );
}

/// An unknown ARGUMENTED opcode mid-index must be a hard stop naming the byte
/// in hex and its offset — never a silent skip. The killer scenario: at least
/// one SETITEMS batch was already harvested when the unknown opcode arrives,
/// so the index is non-empty, the memberless guard can never fire, and a skip
/// would silently drop every remaining member — the same silent-truncation
/// class the BINUNICODE fix exists to kill, previously detectable only in its
/// total form. The fixture uses the exact member shape a shipped archive emits
/// (see build_rpa) poisoned with BINFLOAT (0x47) — a REAL protocol-2 opcode
/// this parser has no arm for, which is precisely the class of input (real
/// opcode, missing arm) that caused the original field failure.
#[test]
fn test_unknown_argumented_opcode_after_harvested_members_is_a_hard_error() {
    let dir = std::env::temp_dir().join(format!("locust_renpy_derail_{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&dir).unwrap();

    let header_len = format!("RPA-3.0 {:016x} {:08x}\n", 0u64, 0u32).len();
    let member_a = b"label a:\n    e \"From member a.\"\n";
    let member_b = b"label b:\n    e \"From member b.\"\n";
    let a_off = header_len as i64;
    let b_off = a_off + member_a.len() as i64;
    let mut body = member_a.to_vec();
    body.extend_from_slice(member_b);

    // Same opcode-for-opcode member emission as build_rpa, but with each member
    // committed by its OWN MARK + SETITEMS batch so member a is already in the
    // index when the poison opcode appears inside member b's batch.
    fn emit_member(pickle: &mut Vec<u8>, memo: &mut u32, name: &str, off: i64, len: i64) {
        pickle.push(0x28); // MARK
        pickle.push(0x58); // BINUNICODE
        pickle.extend_from_slice(&(name.len() as u32).to_le_bytes());
        pickle.extend_from_slice(name.as_bytes());
        emit_put(pickle, memo);
        pickle.push(0x5d); // EMPTY_LIST
        emit_put(pickle, memo);
        emit_long1(pickle, off ^ RPA_KEY);
        pickle.push(0x4a); // BININT
        pickle.extend_from_slice(&((len ^ RPA_KEY) as i32).to_le_bytes());
        pickle.push(0x55); // SHORT_BINSTRING — the empty prefix
        pickle.push(0);
        emit_put(pickle, memo);
        pickle.push(0x87); // TUPLE3
        pickle.push(0x61); // APPEND
    }

    let mut pickle: Vec<u8> = vec![0x80, 0x02, 0x7d]; // PROTO 2 · EMPTY_DICT
    let mut memo: u32 = 0;
    emit_put(&mut pickle, &mut memo);

    emit_member(
        &mut pickle,
        &mut memo,
        "a.rpy",
        a_off,
        member_a.len() as i64,
    );
    pickle.push(0x75); // SETITEMS — member a is harvested; the index is non-empty

    emit_member(
        &mut pickle,
        &mut memo,
        "b.rpy",
        b_off,
        member_b.len() as i64,
    );
    // BINFLOAT: opcode byte + 8-byte big-endian float argument. Skipping only
    // the opcode byte would feed those 8 argument bytes back into the opcode
    // loop and derail the stream past member b.
    let poison_offset = pickle.len();
    pickle.push(0x47);
    pickle.extend_from_slice(&1.0f64.to_be_bytes());
    pickle.push(0x75); // SETITEMS
    pickle.push(0x2e); // STOP

    let rpa_path = dir.join("scripts.rpa");
    fs::write(&rpa_path, wrap_index_pickle(&pickle, &body)).unwrap();

    let plugin = RenPyPlugin::new();
    let err = plugin.extract(&rpa_path).expect_err(
        "an unknown argumented opcode mid-index must be a hard error, \
         not a silently truncated member list",
    );
    let msg = err.to_string();
    assert!(
        msg.contains("0x47"),
        "the error must name the offending opcode in hex, got: {msg}"
    );
    assert!(
        msg.contains(&format!("offset {poison_offset}")),
        "the error must name the byte offset ({poison_offset}) of the opcode, got: {msg}"
    );
}

/// A failing archive must not leak its extraction temp directory. The temp dir
/// is created BEFORE the archive is read, and every `?` between creation and
/// the end-of-function cleanup previously early-returned past it — one leaked
/// `locust_rpa_<uuid>` per failing archive per run. A Drop guard must cover
/// every exit path.
#[test]
fn test_failing_archive_extraction_leaks_no_temp_directory() {
    let dir = std::env::temp_dir().join(format!("locust_renpy_leak_{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&dir).unwrap();

    // Memberless index: parses cleanly to zero members, so extraction errors
    // AFTER the temp dir has been created — the exact leaking path.
    let pickle: Vec<u8> = vec![0x80, 0x02, 0x7d, 0x2e]; // PROTO 2 · EMPTY_DICT · STOP
    let rpa_path = dir.join("scripts.rpa");
    fs::write(&rpa_path, wrap_index_pickle(&pickle, b"")).unwrap();

    let temp_root = std::env::temp_dir();
    let rpa_dirs = || -> std::collections::BTreeSet<String> {
        std::fs::read_dir(&temp_root)
            .map(|it| {
                it.filter_map(|e| e.ok())
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .filter(|n| n.starts_with("locust_rpa_"))
                    .collect()
            })
            .unwrap_or_default()
    };
    let before = rpa_dirs();

    let plugin = RenPyPlugin::new();
    plugin
        .extract(&rpa_path)
        .expect_err("a memberless archive must error (see the zero-members test)");

    // Concurrent tests in this binary create their own locust_rpa_ dirs and
    // remove them within milliseconds; a leak from OUR failed call never
    // disappears. Poll so transients settle, then require zero new dirs.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let mut leaked: Vec<String>;
    loop {
        leaked = rpa_dirs().difference(&before).cloned().collect();
        if leaked.is_empty() || std::time::Instant::now() > deadline {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    assert!(
        leaked.is_empty(),
        "failed archive extraction leaked temp dir(s) in {}: {leaked:?}",
        temp_root.display()
    );
}

/// Shared writer that captures tracing output so a test can assert on it.
#[derive(Clone, Default)]
struct LogBuffer(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

impl LogBuffer {
    fn contents(&self) -> String {
        String::from_utf8_lossy(&self.0.lock().unwrap()).into_owned()
    }
}

impl std::io::Write for LogBuffer {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for LogBuffer {
    type Writer = LogBuffer;
    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

/// The other half of the loud-failure contract: script members WERE extracted
/// from the archive but zero strings came out of them. That can be legitimate
/// (an archive of pure-code scripts) so it must not abort — but it must warn,
/// because it is also the symptom of a harvester regression.
#[test]
fn test_script_members_without_harvestable_strings_warn_instead_of_silence() {
    let dir = std::env::temp_dir().join(format!("locust_renpy_nostr_{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&dir).unwrap();

    let script = b"# configuration only, nothing translatable\n$ flag = True\n";
    let rpa_path = dir.join("scripts.rpa");
    fs::write(&rpa_path, build_rpa(&[("script.rpy", script)])).unwrap();

    let buf = LogBuffer::default();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(buf.clone())
        .with_max_level(tracing::Level::WARN)
        .with_ansi(false)
        .finish();

    let plugin = RenPyPlugin::new();
    let entries = tracing::subscriber::with_default(subscriber, || {
        plugin.extract(&rpa_path).expect("extract failed")
    });

    assert!(
        entries.is_empty(),
        "precondition: this archive's scripts contain nothing translatable"
    );
    let logs = buf.contents();
    assert!(
        logs.contains("no strings were harvested"),
        "extracting script members without harvesting a single string must warn, got logs: {logs:?}"
    );
}

/// The inverse guard: a translations-only archive (every member under tl/) is a
/// state real games ship in, and the harvest loop DELIBERATELY skips tl/
/// members. Counting them toward "members extracted but no strings harvested"
/// made every such archive warn spuriously. Only members the harvester actually
/// considers may arm the warning.
#[test]
fn test_translations_only_archive_does_not_warn_about_no_strings() {
    let dir = std::env::temp_dir().join(format!("locust_renpy_tlonly_{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&dir).unwrap();

    let tl_script = b"translate spanish start_x1:\n    e \"Hola desde tl!\"\n";
    let rpa_path = dir.join("scripts.rpa");
    fs::write(
        &rpa_path,
        build_rpa(&[("tl/spanish/script.rpy", tl_script)]),
    )
    .unwrap();

    let buf = LogBuffer::default();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(buf.clone())
        .with_max_level(tracing::Level::WARN)
        .with_ansi(false)
        .finish();

    let plugin = RenPyPlugin::new();
    let entries = tracing::subscriber::with_default(subscriber, || {
        plugin.extract(&rpa_path).expect("extract failed")
    });

    assert!(
        entries.is_empty(),
        "precondition: tl/ members are deliberately not harvested"
    );
    let logs = buf.contents();
    assert!(
        !logs.contains("no strings were harvested"),
        "a translations-only archive is legitimate and must not trigger the \
         no-strings warning, got logs: {logs:?}"
    );
}
