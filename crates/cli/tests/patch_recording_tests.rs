//! Per-cell integration tests for the injection-recording contract:
//! `locust inject` records (root, rel, hash) per language for everything it
//! wrote, and `locust patch` packs exclusively from that recording. Every
//! error path is exercised end-to-end, INCLUDING executing the command the
//! error advises and asserting it unblocks the state that produced it —
//! an advised command that cannot help is a closed loop, not a remedy.
//!
//! Fixture reachability: every database state used here is produced by the
//! shipped pipeline itself — `locust extract` on a real fixture tree, then
//! `locust translate -p mock`, or (for Wolf RPG, whose byte-patched strings
//! must not grow) translations written through the same public Database API
//! `locust import` uses.

use assert_cmd::Command;
use predicates::prelude::*;
use std::fs;
use std::path::{Path, PathBuf};

fn locust() -> Command {
    Command::cargo_bin("locust").unwrap()
}

fn base_dir() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("locust_patchrec_{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&dir).unwrap();
    dir
}

const HTML_BODY: &str = "<html><head><title>demo</title></head><body>\n\
    <p>Hello world, adventurer!</p>\n\
    <p>The journey begins now.</p>\n\
    </body></html>\n";

/// A minimal HTML game (engine class E4: path-derived writes, no Add mode),
/// extracted and mock-translated through the REAL pipeline.
fn html_game_project(base: &Path) -> (PathBuf, PathBuf) {
    let game = base.join("htmlgame");
    fs::create_dir_all(&game).unwrap();
    fs::write(game.join("index.html"), HTML_BODY).unwrap();
    let db = base.join("project.locust.db");
    locust()
        .arg("extract")
        .arg(&game)
        .arg("-o")
        .arg(&db)
        .assert()
        .success();
    locust()
        .arg("translate")
        .arg(&db)
        .args(["-p", "mock", "-s", "en", "-t", "es"])
        .assert()
        .success();
    (game, db)
}

/// A minimal Wolf RPG game (engine class E5: `inject` prefers
/// `entry.file_path` — the ORIGINAL tree — over the path it is handed).
/// Extracted through the real pipeline; translations must encode to
/// Shift-JIS no longer than the original bytes, so they are written through
/// the public Database API in the exact shape `locust import` produces
/// (translation set + status Translated) — the MOCK provider's prefixed
/// output would exceed the in-place byte budget.
fn wolf_game_project(base: &Path) -> (PathBuf, PathBuf) {
    let game = base.join("wolfgame");
    let data = game.join("Data");
    fs::create_dir_all(&data).unwrap();
    fs::write(
        data.join("BasicData.wolf"),
        locust_formats::wolf_rpg::build_test_fixture(),
    )
    .unwrap();
    let db_path = base.join("project.locust.db");
    locust()
        .arg("extract")
        .arg(&game)
        .arg("-o")
        .arg(&db_path)
        .assert()
        .success();

    let db = locust_core::database::Database::open(&db_path).unwrap();
    let entries = db
        .get_entries(&locust_core::database::EntryFilter::default())
        .unwrap();
    assert!(
        !entries.is_empty(),
        "the wolf fixture must extract at least one string"
    );
    let translated: Vec<_> = entries
        .into_iter()
        .map(|mut e| {
            // ASCII encodes 1 byte/char in Shift-JIS; every source string in
            // the fixture is at least 4 Shift-JIS bytes long.
            e.translation = Some(match e.source.as_str() {
                "テストデータ" => "test".to_string(),
                "勇者" => "yu".to_string(),
                _ => "mage".to_string(),
            });
            e.status = locust_core::models::StringStatus::Translated;
            e
        })
        .collect();
    db.save_entries(&translated).unwrap();
    (game, db_path)
}

fn copy_dir(src: &Path, dst: &Path) {
    fs::create_dir_all(dst).unwrap();
    for entry in fs::read_dir(src).unwrap().flatten() {
        let p = entry.path();
        let d = dst.join(entry.file_name());
        if p.is_dir() {
            copy_dir(&p, &d);
        } else {
            fs::copy(&p, &d).unwrap();
        }
    }
}

fn zip_entry_bytes(p: &Path, name: &str) -> Vec<u8> {
    use std::io::Read as _;
    let f = fs::File::open(p).unwrap();
    let mut a = zip::ZipArchive::new(f).unwrap();
    let mut e = a
        .by_name(name)
        .unwrap_or_else(|_| panic!("\"{name}\" not found in {}", p.display()));
    let mut buf = Vec::new();
    e.read_to_end(&mut buf).unwrap();
    buf
}

fn zip_entry_string(p: &Path, name: &str) -> String {
    String::from_utf8_lossy(&zip_entry_bytes(p, name)).into_owned()
}

fn stderr_of(assert: assert_cmd::assert::Assert) -> String {
    String::from_utf8_lossy(&assert.get_output().stderr).into_owned()
}

// ─── R1/A1 family: Replace/Add without a language ───────────────────────────

#[test]
fn replace_and_add_without_a_language_bail_loudly() {
    // These runs used to iterate zero languages: nothing copied, nothing
    // injected, nothing recorded — exit 0. Two commands later the user
    // shipped an untranslated "patch".
    let base = base_dir();
    let (game, db) = html_game_project(&base);
    let original = fs::read_to_string(game.join("index.html")).unwrap();
    let out = base.join("out");

    locust()
        .arg("inject")
        .arg(&game)
        .arg("-P")
        .arg(&db)
        .arg("-o")
        .arg(&out)
        .assert()
        .failure()
        .stderr(predicate::str::contains("requires at least one language"));

    locust()
        .arg("inject")
        .arg(&game)
        .arg("-P")
        .arg(&db)
        .args(["-m", "add"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("requires at least one language"));

    assert_eq!(
        fs::read_to_string(game.join("index.html")).unwrap(),
        original,
        "a refused inject must not touch the game"
    );
}

// ─── D11 family: --direct with several languages ────────────────────────────

#[test]
fn direct_inject_records_every_requested_language() {
    // The old code recorded `languages.first()` only, silently orphaning
    // `patch -l <second>`.
    let base = base_dir();
    let (game, db) = html_game_project(&base);
    locust()
        .arg("inject")
        .arg(&game)
        .arg("-P")
        .arg(&db)
        .arg("--direct")
        .args(["-l", "es", "fr"])
        .assert()
        .success();

    let zip_fr = base.join("fr.zip");
    locust()
        .arg("patch")
        .arg(&game)
        .arg("-P")
        .arg(&db)
        .args(["-l", "fr"])
        .arg("-o")
        .arg(&zip_fr)
        .assert()
        .success();
    assert!(
        zip_entry_string(&zip_fr, "index.html").contains("[MOCK:es]"),
        "the second language's recording must pack the translated file"
    );
}

// ─── R6: patch pointed at the ORIGINAL after a copy (Replace) inject ─────────

#[test]
fn patch_pointed_at_the_original_after_a_copy_inject_names_the_recorded_root() {
    let base = base_dir();
    let (game, db) = html_game_project(&base);
    let out = base.join("out");
    locust()
        .arg("inject")
        .arg(&game)
        .arg("-P")
        .arg(&db)
        .args(["-l", "es"])
        .arg("-o")
        .arg(&out)
        .assert()
        .success();
    let copy = out.join("htmlgame-es");
    assert!(
        copy.join("index.html").exists(),
        "Replace mode must have produced the per-language copy"
    );

    // The original tree exists and holds the same rel UNTRANSLATED — the old
    // flow silently read it from there and shipped an untranslated zip.
    let zip = base.join("es.zip");
    let assert = locust()
        .arg("patch")
        .arg(&game)
        .arg("-P")
        .arg(&db)
        .args(["-l", "es"])
        .arg("-o")
        .arg(&zip)
        .assert()
        .failure();
    let stderr = stderr_of(assert);
    assert!(
        stderr.contains(&copy.display().to_string()),
        "the error must name the recorded root to point patch at: {stderr}"
    );
    assert!(!zip.exists(), "no zip may be written from the wrong tree");

    // Execute the advice: patch pointed at the recorded root.
    locust()
        .arg("patch")
        .arg(&copy)
        .arg("-P")
        .arg(&db)
        .args(["-l", "es"])
        .arg("-o")
        .arg(&zip)
        .assert()
        .success();
    assert!(
        zip_entry_string(&zip, "index.html").contains("[MOCK:es]"),
        "the packed file must be the TRANSLATED copy's bytes"
    );
}

// ─── A7: unsupported Add mode must be loud, with per-engine advice ──────────

#[test]
fn unsupported_add_mode_fails_loudly_and_the_advice_unblocks() {
    // E4 flavor (HTML): Add is unsupported, but Replace and direct both work.
    let base = base_dir();
    let (game, db) = html_game_project(&base);
    let assert = locust()
        .arg("inject")
        .arg(&game)
        .arg("-P")
        .arg(&db)
        .args(["-m", "add", "-l", "es"])
        .assert()
        .failure();
    let stderr = stderr_of(assert);
    assert!(
        stderr.contains("does not support Add mode"),
        "the per-language failure must be printed, not swallowed: {stderr}"
    );
    assert!(
        stderr.contains("es:"),
        "the failed language must be named: {stderr}"
    );
    assert!(
        stderr.contains("--direct -l es"),
        "the advice must include direct mode: {stderr}"
    );

    // Execute the advised command verbatim; it must unblock patch.
    locust()
        .arg("inject")
        .arg(&game)
        .arg("-P")
        .arg(&db)
        .arg("--direct")
        .args(["-l", "es"])
        .assert()
        .success();
    let zip = base.join("es.zip");
    locust()
        .arg("patch")
        .arg(&game)
        .arg("-P")
        .arg(&db)
        .args(["-l", "es"])
        .arg("-o")
        .arg(&zip)
        .assert()
        .success();
    assert!(zip_entry_string(&zip, "index.html").contains("[MOCK:es]"));
}

#[test]
fn unsupported_add_advice_is_direct_only_for_entry_tree_writers() {
    // E5 flavor (Wolf RPG): the Replace branch hits the containment
    // hard-error for this engine class, so the advice must NOT send the user
    // there — per-engine advice, like the containment error's.
    let base = base_dir();
    let (game, db) = wolf_game_project(&base);
    let assert = locust()
        .arg("inject")
        .arg(&game)
        .arg("-P")
        .arg(&db)
        .args(["-m", "add", "-l", "es"])
        .assert()
        .failure();
    let stderr = stderr_of(assert);
    assert!(stderr.contains("does not support Add mode"), "{stderr}");
    assert!(
        stderr.contains("--direct -l es"),
        "direct mode is the advice that works for this engine: {stderr}"
    );
    assert!(
        !stderr.contains("-o <output_dir>"),
        "Replace advice would dead-end in the containment error for this \
         engine and must not be offered: {stderr}"
    );

    // Execute the advised command verbatim; it must unblock patch.
    locust()
        .arg("inject")
        .arg(&game)
        .arg("-P")
        .arg(&db)
        .arg("--direct")
        .args(["-l", "es"])
        .assert()
        .success();
    let zip = base.join("es.zip");
    locust()
        .arg("patch")
        .arg(&game)
        .arg("-P")
        .arg(&db)
        .args(["-l", "es"])
        .arg("-o")
        .arg(&zip)
        .assert()
        .success();
    let bytes = zip_entry_bytes(&zip, "Data/BasicData.wolf");
    assert!(
        bytes.windows(4).any(|w| w == b"test"),
        "the packed wolf file must carry the injected translation bytes"
    );
}

// ─── Decision-6 pin: key set {es, NULL} + patch without -l ──────────────────

#[test]
fn patch_without_lang_over_es_and_unspecified_keys_requires_a_choice() {
    // Reachable state: the user direct-injects one copy with -l es, then
    // re-injects a second pristine copy forgetting -l (which records the
    // reserved language-unspecified key). Both rules claim this cell; the
    // multiple-keys error wins — never a silent match of either key.
    let base = base_dir();
    let (game1, db) = html_game_project(&base);
    let game2 = base.join("htmlgame2");
    fs::create_dir_all(&game2).unwrap();
    fs::write(game2.join("index.html"), HTML_BODY).unwrap();

    locust()
        .arg("inject")
        .arg(&game1)
        .arg("-P")
        .arg(&db)
        .arg("--direct")
        .args(["-l", "es"])
        .assert()
        .success();
    locust()
        .arg("inject")
        .arg(&game2)
        .arg("-P")
        .arg(&db)
        .arg("--direct")
        .assert()
        .success();

    let zip = base.join("patch.zip");
    let assert = locust()
        .arg("patch")
        .arg(&game1)
        .arg("-P")
        .arg(&db)
        .arg("-o")
        .arg(&zip)
        .assert()
        .failure();
    let stderr = stderr_of(assert);
    assert!(
        stderr.contains("es"),
        "the named key must be listed: {stderr}"
    );
    assert!(
        stderr.contains("(unspecified)"),
        "the language-unspecified key must be listed: {stderr}"
    );
    assert!(
        stderr.contains("-l"),
        "the error must require an explicit -l: {stderr}"
    );
    assert!(!zip.exists());

    // Execute the advised choice; it must unblock.
    locust()
        .arg("patch")
        .arg(&game1)
        .arg("-P")
        .arg(&db)
        .args(["-l", "es"])
        .arg("-o")
        .arg(&zip)
        .assert()
        .success();
    assert!(zip_entry_string(&zip, "index.html").contains("[MOCK:es]"));
}

// ─── Decision 5: a database with no recording (legacy or pre-inject) ────────

#[test]
fn a_database_with_no_recording_names_a_command_that_provably_unblocks() {
    let base = base_dir();
    let (game, db) = html_game_project(&base);
    let zip = base.join("es.zip");
    // The exact command the error must print — executed verbatim below.
    let advised = format!(
        "locust inject \"{}\" -P \"{}\" --direct -l es",
        game.display(),
        db.display()
    );

    locust()
        .arg("patch")
        .arg(&game)
        .arg("-P")
        .arg(&db)
        .args(["-l", "es"])
        .arg("-o")
        .arg(&zip)
        .assert()
        .failure()
        .stderr(
            predicate::str::contains("no injection has been recorded")
                .and(predicate::str::contains(advised)),
        );
    assert!(!zip.exists());

    // Execute the advised command verbatim (same argv the printed string
    // denotes); the previously blocked patch must now succeed.
    locust()
        .arg("inject")
        .arg(&game)
        .arg("-P")
        .arg(&db)
        .arg("--direct")
        .args(["-l", "es"])
        .assert()
        .success();
    locust()
        .arg("patch")
        .arg(&game)
        .arg("-P")
        .arg(&db)
        .args(["-l", "es"])
        .arg("-o")
        .arg(&zip)
        .assert()
        .success();
    assert!(zip_entry_string(&zip, "index.html").contains("[MOCK:es]"));
}

// ─── Decision 3: record-time containment (E5 Replace) ───────────────────────

#[test]
fn replace_that_writes_outside_its_root_records_nothing_and_the_remedy_unblocks() {
    // Wolf RPG injects into `entry.file_path` — the ORIGINAL tree — even in
    // Replace mode, so the per-language copy never receives the writes. The
    // recording must refuse (hard error, nothing recorded) instead of
    // pointing `patch` at a tree injection never targeted. The remedy must
    // include restoring the original: a bare `--direct` re-run from this
    // state writes nothing (the original bytes were already replaced) and
    // records nothing — a closed loop.
    let base = base_dir();
    let (game, db) = wolf_game_project(&base);
    let pristine = base.join("pristine");
    copy_dir(&game, &pristine);
    let out = base.join("out");
    let wolf_file = game.join("Data").join("BasicData.wolf");

    let assert = locust()
        .arg("inject")
        .arg(&game)
        .arg("-P")
        .arg(&db)
        .args(["-l", "es"])
        .arg("-o")
        .arg(&out)
        .assert()
        .failure();
    let stderr = stderr_of(assert);
    assert!(
        stderr.contains("OUTSIDE its target root"),
        "the containment violation must be loud: {stderr}"
    );
    assert!(
        stderr.contains(&wolf_file.display().to_string()),
        "the escaping file must be named: {stderr}"
    );
    assert!(
        stderr.contains("wolfgame-es"),
        "the expected root (the Replace copy) must be named: {stderr}"
    );
    assert!(
        stderr.contains("Nothing was recorded"),
        "the user must know no recording exists: {stderr}"
    );
    assert!(
        stderr.contains("Backup"),
        "the original was already mutated when this fires — the backup must \
         be pointed out: {stderr}"
    );
    assert!(stderr.contains("restore"), "{stderr}");
    assert!(stderr.contains("--direct -l es"), "{stderr}");

    // Nothing recorded → patch hits the no-recording error, never a silent
    // wrong zip.
    let zip = base.join("es.zip");
    locust()
        .arg("patch")
        .arg(&game)
        .arg("-P")
        .arg(&db)
        .args(["-l", "es"])
        .arg("-o")
        .arg(&zip)
        .assert()
        .failure()
        .stderr(predicate::str::contains("no injection has been recorded"));

    // Follow the remedy verbatim: restore the original, then direct inject.
    fs::remove_dir_all(&game).unwrap();
    copy_dir(&pristine, &game);
    locust()
        .arg("inject")
        .arg(&game)
        .arg("-P")
        .arg(&db)
        .arg("--direct")
        .args(["-l", "es"])
        .assert()
        .success();
    locust()
        .arg("patch")
        .arg(&game)
        .arg("-P")
        .arg(&db)
        .args(["-l", "es"])
        .arg("-o")
        .arg(&zip)
        .assert()
        .success();
    let bytes = zip_entry_bytes(&zip, "Data/BasicData.wolf");
    assert!(
        bytes.windows(4).any(|w| w == b"test"),
        "the packed wolf file must carry the injected translation bytes"
    );
}

// ─── Ren'Py Replace containment: restore-first remedy ───────────────────────

#[test]
fn renpy_replace_containment_says_restore_first_and_the_remedy_unblocks() {
    // Ren'Py writes loose scripts to `entry.file_path` — the ORIGINAL tree —
    // even in Replace mode, so by the time the containment error fires the
    // original scripts are already mutated. A bare `--direct` re-run skips
    // every already-translated line (the source text is gone): zero writes
    // for a loose-only game, or a recording that silently omits the loose
    // translations for a mixed game. The remedy must lead with the restore.
    let base = base_dir();
    let game = base.join("renpygame");
    let game_sub = game.join("game");
    fs::create_dir_all(&game_sub).unwrap();
    fs::write(
        game_sub.join("script.rpy"),
        "label start:\n    e \"Hello world, adventurer!\"\n    \"The journey begins now.\"\n",
    )
    .unwrap();
    let db = base.join("project.locust.db");
    locust()
        .arg("extract")
        .arg(&game)
        .arg("-o")
        .arg(&db)
        .assert()
        .success();
    locust()
        .arg("translate")
        .arg(&db)
        .args(["-p", "mock", "-s", "en", "-t", "es"])
        .assert()
        .success();

    let pristine = base.join("pristine");
    copy_dir(&game, &pristine);
    let out = base.join("out");

    let assert = locust()
        .arg("inject")
        .arg(&game)
        .arg("-P")
        .arg(&db)
        .args(["-l", "es"])
        .arg("-o")
        .arg(&out)
        .assert()
        .failure();
    let stderr = stderr_of(assert);
    assert!(
        stderr.contains("OUTSIDE its target root"),
        "the containment violation must be loud: {stderr}"
    );
    // The judge's premise, proven on disk: the ORIGINAL loose script was
    // already rewritten when the error fired.
    let mutated = fs::read_to_string(game_sub.join("script.rpy")).unwrap();
    assert!(
        mutated.contains("[MOCK:es]"),
        "the original loose script must already be mutated in this state"
    );
    assert!(
        stderr.contains("restore"),
        "the remedy must lead with restoring the original: {stderr}"
    );
    assert!(stderr.contains("-m add"), "{stderr}");
    assert!(stderr.contains("--direct -l es"), "{stderr}");

    // Nothing recorded → patch is blocked, never a silent wrong zip.
    let zip = base.join("es.zip");
    locust()
        .arg("patch")
        .arg(&game)
        .arg("-P")
        .arg(&db)
        .args(["-l", "es"])
        .arg("-o")
        .arg(&zip)
        .assert()
        .failure()
        .stderr(predicate::str::contains("no injection has been recorded"));

    // Follow the remedy verbatim: restore the original, then direct mode.
    fs::remove_dir_all(&game).unwrap();
    copy_dir(&pristine, &game);
    locust()
        .arg("inject")
        .arg(&game)
        .arg("-P")
        .arg(&db)
        .arg("--direct")
        .args(["-l", "es"])
        .assert()
        .success();
    locust()
        .arg("patch")
        .arg(&game)
        .arg("-P")
        .arg(&db)
        .args(["-l", "es"])
        .arg("-o")
        .arg(&zip)
        .assert()
        .success();
    assert!(
        zip_entry_string(&zip, "game/script.rpy").contains("[MOCK:es]"),
        "the packed loose script must carry the translations"
    );
}

// ─── Legacy DB on an already-injected entry-tree game ───────────────────────

#[test]
fn legacy_database_on_an_already_injected_entry_tree_game_gets_a_restore_first_remedy() {
    // The shipped pipeline reaches this state: an older Locust injected the
    // tree (byte-patching the ORIGINAL .wolf files) and kept only the legacy
    // recording table, which the migration drops on the next open. `patch`
    // then advises `--direct` — but the byte-scan injector needs the ORIGINAL
    // source bytes, finds nothing in the already-translated tree, writes 0
    // files, and records nothing: the identical error forever unless the
    // advice carries the restore-first note.
    let base = base_dir();
    let (game, db_path) = wolf_game_project(&base);
    let pristine = base.join("pristine");
    copy_dir(&game, &pristine);

    // An inject mutates the originals (the state an older Locust left)...
    locust()
        .arg("inject")
        .arg(&game)
        .arg("-P")
        .arg(&db_path)
        .arg("--direct")
        .args(["-l", "es"])
        .assert()
        .success();
    // ...and its database predates the recording contract: recreate the
    // legacy table exactly as the old schema spelled it.
    let conn = rusqlite::Connection::open(&db_path).unwrap();
    conn.execute_batch(
        "DROP TABLE injected_files;
         CREATE TABLE injected_files (
             id INTEGER PRIMARY KEY AUTOINCREMENT,
             file_path TEXT NOT NULL,
             lang TEXT,
             recorded_at TEXT NOT NULL
         );",
    )
    .unwrap();
    drop(conn);

    // patch: the migration drops the legacy table → no recording → the
    // advice must warn about the already-injected dead end.
    let zip = base.join("es.zip");
    let assert = locust()
        .arg("patch")
        .arg(&game)
        .arg("-P")
        .arg(&db_path)
        .args(["-l", "es"])
        .arg("-o")
        .arg(&zip)
        .assert()
        .failure();
    let stderr = stderr_of(assert);
    assert!(
        stderr.contains("no injection has been recorded"),
        "{stderr}"
    );
    assert!(stderr.contains("--direct -l es"), "{stderr}");
    assert!(
        stderr.contains("0 files written") && stderr.contains("restore the original"),
        "the advice must explain the already-injected dead end and its way out: {stderr}"
    );

    // Prove the loop the note breaks: the bare --direct re-run writes 0
    // files and records nothing, and patch stays blocked.
    let rerun = locust()
        .arg("inject")
        .arg(&game)
        .arg("-P")
        .arg(&db_path)
        .arg("--direct")
        .args(["-l", "es"])
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&rerun.get_output().stdout).into_owned();
    assert!(
        stdout.contains("0 files written") && stdout.contains("nothing was recorded"),
        "the zero-write run must say nothing was recorded: {stdout}"
    );
    locust()
        .arg("patch")
        .arg(&game)
        .arg("-P")
        .arg(&db_path)
        .args(["-l", "es"])
        .arg("-o")
        .arg(&zip)
        .assert()
        .failure()
        .stderr(predicate::str::contains("no injection has been recorded"));

    // Follow the note: restore the originals, re-run, patch succeeds.
    fs::remove_dir_all(&game).unwrap();
    copy_dir(&pristine, &game);
    locust()
        .arg("inject")
        .arg(&game)
        .arg("-P")
        .arg(&db_path)
        .arg("--direct")
        .args(["-l", "es"])
        .assert()
        .success();
    locust()
        .arg("patch")
        .arg(&game)
        .arg("-P")
        .arg(&db_path)
        .args(["-l", "es"])
        .arg("-o")
        .arg(&zip)
        .assert()
        .success();
    let bytes = zip_entry_bytes(&zip, "Data/BasicData.wolf");
    assert!(
        bytes.windows(4).any(|w| w == b"test"),
        "the packed wolf file must carry the injected translation bytes"
    );
}

// ─── A first-ever inject that writes zero files must explain itself ─────────

#[test]
fn a_first_direct_inject_that_writes_zero_files_explains_why_nothing_was_recorded() {
    // Wolf RPG skips every translation that cannot encode to Shift-JIS (€ is
    // not in the code page) — a state `locust import` can hand it. The old
    // behavior recorded nothing and exited 0 silently; `patch` then advised
    // the exact command that had just written zero files.
    let base = base_dir();
    let game = base.join("wolfgame");
    let data = game.join("Data");
    fs::create_dir_all(&data).unwrap();
    fs::write(
        data.join("BasicData.wolf"),
        locust_formats::wolf_rpg::build_test_fixture(),
    )
    .unwrap();
    let db_path = base.join("project.locust.db");
    locust()
        .arg("extract")
        .arg(&game)
        .arg("-o")
        .arg(&db_path)
        .assert()
        .success();
    let db = locust_core::database::Database::open(&db_path).unwrap();
    let entries = db
        .get_entries(&locust_core::database::EntryFilter::default())
        .unwrap();
    let translated: Vec<_> = entries
        .into_iter()
        .map(|mut e| {
            e.translation = Some("€".to_string()); // unencodable in Shift-JIS
            e.status = locust_core::models::StringStatus::Translated;
            e
        })
        .collect();
    db.save_entries(&translated).unwrap();
    drop(db);

    let run = locust()
        .arg("inject")
        .arg(&game)
        .arg("-P")
        .arg(&db_path)
        .arg("--direct")
        .args(["-l", "es"])
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&run.get_output().stdout).into_owned();
    assert!(
        stdout.contains("0 files written") && stdout.contains("nothing was recorded"),
        "a zero-write first inject must say nothing was recorded: {stdout}"
    );
    assert!(
        stdout.contains("could not be applied"),
        "the skip cause must be named: {stdout}"
    );
    assert!(
        stdout.contains("could not encode"),
        "the plugin's skip warnings must be surfaced, not just counted: {stdout}"
    );

    // patch is blocked — and its advice must not be a bare loop back into
    // the command that just reported zero writes.
    let zip = base.join("es.zip");
    let assert = locust()
        .arg("patch")
        .arg(&game)
        .arg("-P")
        .arg(&db_path)
        .args(["-l", "es"])
        .arg("-o")
        .arg(&zip)
        .assert()
        .failure();
    let stderr = stderr_of(assert);
    assert!(
        stderr.contains("no injection has been recorded"),
        "{stderr}"
    );
    assert!(
        stderr.contains("0 files written"),
        "the advice must acknowledge the zero-write outcome instead of \
         advising it blindly: {stderr}"
    );
}

// ─── Decision 8: hash verification at pack time ──────────────────────────────

#[test]
fn a_recorded_file_changed_since_injection_is_refused_and_reinject_unblocks() {
    let base = base_dir();
    let (game, db) = html_game_project(&base);
    locust()
        .arg("inject")
        .arg(&game)
        .arg("-P")
        .arg(&db)
        .arg("--direct")
        .args(["-l", "es"])
        .assert()
        .success();

    // The user re-copied the game: the original, untranslated file is back
    // and no longer matches the recorded hash.
    fs::write(game.join("index.html"), HTML_BODY).unwrap();

    let zip = base.join("es.zip");
    locust()
        .arg("patch")
        .arg(&game)
        .arg("-P")
        .arg(&db)
        .args(["-l", "es"])
        .arg("-o")
        .arg(&zip)
        .assert()
        .failure()
        .stderr(
            predicate::str::contains("changed on disk since injection")
                .and(predicate::str::contains("--direct -l es")),
        );
    assert!(!zip.exists(), "unverified bytes must never ship");

    // Execute the advice: re-inject (rewrites the file and refreshes the
    // recording), then patch.
    locust()
        .arg("inject")
        .arg(&game)
        .arg("-P")
        .arg(&db)
        .arg("--direct")
        .args(["-l", "es"])
        .assert()
        .success();
    locust()
        .arg("patch")
        .arg(&game)
        .arg("-P")
        .arg(&db)
        .args(["-l", "es"])
        .arg("-o")
        .arg(&zip)
        .assert()
        .success();
    assert!(zip_entry_string(&zip, "index.html").contains("[MOCK:es]"));
}

// ─── Decision 11: an empty re-run keeps the previous recording VISIBLY ──────

#[test]
fn a_rerun_that_writes_nothing_keeps_the_previous_recording_visibly() {
    // Wolf RPG byte-patches in place: after the first direct inject the
    // original Shift-JIS bytes are gone, so a re-run finds nothing to write.
    // The previous recording must survive (its files are still on disk) and
    // the keep must be SAID, not silent — silent keeps were the
    // stale-recording hazard. (HTML cannot reach this state: the mock
    // translation contains the source substring, so a re-run replaces again.)
    let base = base_dir();
    let (game, db) = wolf_game_project(&base);
    locust()
        .arg("inject")
        .arg(&game)
        .arg("-P")
        .arg(&db)
        .arg("--direct")
        .args(["-l", "es"])
        .assert()
        .success();

    let rerun = locust()
        .arg("inject")
        .arg(&game)
        .arg("-P")
        .arg(&db)
        .arg("--direct")
        .args(["-l", "es"])
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&rerun.get_output().stdout).into_owned();
    assert!(
        stdout.contains("0 files written") && stdout.contains("previous recording"),
        "keeping the old recording must be visible, not silent: {stdout}"
    );

    // The kept recording still packs the translated bytes.
    let zip = base.join("es.zip");
    locust()
        .arg("patch")
        .arg(&game)
        .arg("-P")
        .arg(&db)
        .args(["-l", "es"])
        .arg("-o")
        .arg(&zip)
        .assert()
        .success();
    let bytes = zip_entry_bytes(&zip, "Data/BasicData.wolf");
    assert!(bytes.windows(4).any(|w| w == b"test"));
}

// ─── F9: recordings survive cwd changes ─────────────────────────────────────

#[test]
fn a_recording_made_with_relative_paths_packs_from_any_cwd() {
    // The old recording stored paths as spelled; a relative inject path was
    // later absolutized against PATCH's cwd, breaking every recorded path
    // when patch ran from anywhere else. Roots are absolutized at record
    // time now.
    let base = base_dir();
    let (game, db) = html_game_project(&base);

    locust()
        .current_dir(&base)
        .arg("inject")
        .arg("htmlgame")
        .args(["-P", "project.locust.db", "--direct", "-l", "es"])
        .assert()
        .success();

    let elsewhere = base.join("elsewhere");
    fs::create_dir_all(&elsewhere).unwrap();
    let zip = base.join("es.zip");
    locust()
        .current_dir(&elsewhere)
        .arg("patch")
        .arg(&game)
        .arg("-P")
        .arg(&db)
        .args(["-l", "es"])
        .arg("-o")
        .arg(&zip)
        .assert()
        .success();
    assert!(zip_entry_string(&zip, "index.html").contains("[MOCK:es]"));
}
