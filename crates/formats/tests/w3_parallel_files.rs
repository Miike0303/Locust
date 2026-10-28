//! Synthetic fixtures only. Run the ignored benchmark with --ignored --nocapture.
use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Instant;

use locust_core::backup::RevisionOriginal;
use locust_core::extraction::{Extraction, FormatPlugin};
use locust_core::models::StringEntry;
use locust_formats::{renpy::RenPyPlugin, rpgmaker_mv::RpgMakerMvPlugin};
use serde_json::json;

// Exercise the private helper's real implementation without a public test API.
#[path = "../src/file_workers.rs"]
mod file_workers;

static ENV_LOCK: Mutex<()> = Mutex::new(());

struct Workers(Option<std::ffi::OsString>);

impl Workers {
    fn set(n: usize) -> Self {
        let previous = std::env::var_os("LOCUST_FILE_WORKERS");
        std::env::set_var("LOCUST_FILE_WORKERS", n.to_string());
        Self(previous)
    }

    fn default_policy() -> Self {
        let previous = std::env::var_os("LOCUST_FILE_WORKERS");
        std::env::remove_var("LOCUST_FILE_WORKERS");
        Self(previous)
    }
}

impl Drop for Workers {
    fn drop(&mut self) {
        match &self.0 {
            Some(value) => std::env::set_var("LOCUST_FILE_WORKERS", value),
            None => std::env::remove_var("LOCUST_FILE_WORKERS"),
        }
    }
}

struct Fixture {
    _dir: tempfile::TempDir,
    root: PathBuf,
    files: Vec<(PathBuf, Vec<u8>)>,
}

impl Fixture {
    fn new(renpy: bool, count: usize, lines: usize) -> Self {
        // Use the OS temp location, never a repository tmp/ tree or real game.
        let dir = tempfile::Builder::new()
            .prefix("locust-w3-")
            .tempdir()
            .unwrap();
        let root = dir.path().join("fixture");
        let data = root.join(if renpy { "game" } else { "data" });
        fs::create_dir_all(&data).unwrap();
        let mut files = Vec::new();
        if !renpy {
            files.push((
                data.join("System.json"),
                br#"{"gameTitle":"Synthetic game"}"#.to_vec(),
            ));
        }
        for file in 0..count {
            let (name, bytes) = if renpy {
                let mut text = format!("label scene_{file}:\r\n");
                for line in 0..lines {
                    text.push_str(&format!(
                        "    e \"Source {file} line {line} with words\"\r\n"
                    ));
                }
                (format!("script{file:04}.rpy"), text.into_bytes())
            } else {
                let mut list = Vec::new();
                for line in 0..lines {
                    list.push(json!({"code":101,"indent":0,"parameters":["",0,0,2,"Alice"]}));
                    list.push(json!({"code":401,"indent":0,"parameters":[format!("Source {file} line {line} with words")]}));
                }
                let map = json!({"displayName":format!("Place {file}"),"events":[null,{"pages":[{"list":list}]}]});
                (
                    format!("Map{file:04}.json"),
                    serde_json::to_vec(&map).unwrap(),
                )
            };
            files.push((data.join(name), bytes));
        }
        let fixture = Self {
            _dir: dir,
            root,
            files,
        };
        fixture.restore();
        fixture
    }

    fn restore(&self) {
        for (path, bytes) in &self.files {
            fs::write(path, bytes).unwrap();
        }
    }

    fn plugin(renpy: bool) -> Box<dyn FormatPlugin> {
        if renpy {
            Box::new(RenPyPlugin::new())
        } else {
            Box::new(RpgMakerMvPlugin::new())
        }
    }
}

fn translated(mut entries: Vec<StringEntry>) -> Vec<StringEntry> {
    for entry in &mut entries {
        entry.translation = Some(format!("Traducido {}", entry.source));
    }
    entries
}

fn serialized(extraction: &Extraction) -> Vec<u8> {
    // Wall-clock creation times differ even between two serial extractions.
    // Canonical JSON object keys also remove HashMap serialization randomness.
    let mut rows = serde_json::to_value(&extraction.entries).unwrap();
    for row in rows.as_array_mut().unwrap() {
        row["created_at"] = json!("2000-01-01T00:00:00Z");
    }
    serde_json::to_vec(&(rows, &extraction.warnings, extraction.complete)).unwrap()
}

fn tree(root: &Path) -> BTreeMap<PathBuf, blake3::Hash> {
    walkdir::WalkDir::new(root)
        .into_iter()
        .map(|entry| entry.unwrap())
        .filter(|entry| entry.file_type().is_file())
        .map(|entry| {
            (
                entry.path().strip_prefix(root).unwrap().to_path_buf(),
                blake3::hash(&fs::read(entry.path()).unwrap()),
            )
        })
        .collect()
}

#[cfg(windows)]
fn peak_rss() -> usize {
    #[repr(C)]
    #[derive(Default)]
    struct Counters {
        size: u32,
        faults: u32,
        peak_working_set: usize,
        working_set: usize,
        peak_paged_pool: usize,
        paged_pool: usize,
        peak_nonpaged_pool: usize,
        nonpaged_pool: usize,
        pagefile: usize,
        peak_pagefile: usize,
    }
    #[link(name = "psapi")]
    extern "system" {
        fn GetProcessMemoryInfo(process: isize, counters: *mut Counters, size: u32) -> i32;
    }
    let size = std::mem::size_of::<Counters>() as u32;
    let mut counters = Counters {
        size,
        ..Counters::default()
    };
    // SAFETY: -1 is the Windows current-process pseudo handle; the output
    // buffer has the documented PROCESS_MEMORY_COUNTERS layout and size.
    assert_ne!(unsafe { GetProcessMemoryInfo(-1, &mut counters, size) }, 0);
    counters.peak_working_set
}

#[cfg(not(windows))]
fn peak_rss() -> usize {
    0
}

#[test]
fn default_policy_keeps_small_files_on_the_original_serial_loops() {
    use file_workers::Phase;
    let _lock = ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let _workers = Workers::default_policy();
    for phase in [
        Phase::RenpyExtract,
        Phase::RenpyInject,
        Phase::RpgExtract,
        Phase::RpgInject,
    ] {
        // The benchmark's many-small shape must bypass the worker/preparation
        // path even on a machine with many available CPUs. Returning false
        // selects the plugins' original serial loops before preparation buffers.
        for file_count in [2, 4, 32] {
            assert!(!file_workers::parallel(phase, file_count, || false));
        }
        for file_count in [65, 1000, 2000] {
            assert!(!file_workers::parallel(phase, file_count, || {
                panic!("many-small shapes must not scan or prepare files")
            }));
        }
    }
}

#[test]
fn default_policy_does_not_enable_unproven_large_file_phases() {
    use file_workers::Phase;
    let _lock = ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let _workers = Workers::default_policy();
    assert_eq!(file_workers::worker_count(), 1);
    for phase in [
        Phase::RenpyExtract,
        Phase::RenpyInject,
        Phase::RpgExtract,
        Phase::RpgInject,
    ] {
        assert!(!file_workers::parallel(phase, 4, || {
            panic!("unproven automatic phases must skip preparation")
        }));
    }
}

#[test]
fn default_thresholds_require_the_measured_large_file_shape() {
    use file_workers::Phase;
    let _lock = ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let _workers = Workers::default_policy();
    for phase in [Phase::RenpyExtract, Phase::RenpyInject] {
        assert_eq!(file_workers::thresholds(phase), (4, 375 * 1024, 10000));
    }
    for phase in [Phase::RpgExtract, Phase::RpgInject] {
        assert_eq!(file_workers::thresholds(phase), (4, 1024 * 1024, 20000));
    }
    for workers in [2, 4, 8] {
        let _workers = Workers::set(workers);
        for phase in [
            Phase::RenpyExtract,
            Phase::RenpyInject,
            Phase::RpgExtract,
            Phase::RpgInject,
        ] {
            assert_eq!(file_workers::thresholds(phase), (2, 64 * 1024, 1000));
        }
    }
}

#[test]
fn one_worker_and_single_files_skip_even_the_shape_probe() {
    use file_workers::Phase;
    let _lock = ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    for phase in [
        Phase::RenpyExtract,
        Phase::RenpyInject,
        Phase::RpgExtract,
        Phase::RpgInject,
    ] {
        let _workers = Workers::set(1);
        for file_count in [0, 1, 4, 32, 1000, 2000] {
            assert!(!file_workers::parallel(phase, file_count, || {
                panic!("one worker must not scan or prepare files")
            }));
        }
        let _workers = Workers::set(8);
        for file_count in [0, 1] {
            assert!(!file_workers::parallel(phase, file_count, || {
                panic!("a single file must not scan or prepare files")
            }));
        }
    }
}

#[test]
fn explicit_workers_exercise_the_parallel_large_file_path() {
    use file_workers::Phase;
    let _lock = ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    for workers in [2, 4, 8] {
        let _workers = Workers::set(workers);
        for phase in [
            Phase::RenpyExtract,
            Phase::RenpyInject,
            Phase::RpgExtract,
            Phase::RpgInject,
        ] {
            // The large variants below have four or eight files, with at least
            // two files above both the size and the injection-entry thresholds.
            assert!(file_workers::parallel(phase, 4, || true));
        }
    }
}

#[test]
fn extraction_and_injection_are_identical() {
    let _lock = ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    for renpy in [true, false] {
        for (count, lines) in [(32, 3), (4, 300), (4, 2000)] {
            let f = Fixture::new(renpy, count, lines);
            let plugin = Fixture::plugin(renpy);
            let _workers = Workers::set(1);
            let serial = plugin.extract_with_diagnostics(&f.root).unwrap();
            assert!(serial.complete);
            let entries = translated(serial.entries.clone());
            let report = serde_json::to_value(plugin.inject(&f.root, &entries).unwrap()).unwrap();
            let expected = tree(&f.root);
            for workers in [2, 4, 8] {
                f.restore();
                let _workers = Workers::set(workers);
                assert_eq!(
                    serialized(&serial),
                    serialized(&plugin.extract_with_diagnostics(&f.root).unwrap())
                );
                assert_eq!(
                    report,
                    serde_json::to_value(plugin.inject(&f.root, &entries).unwrap()).unwrap()
                );
                assert_eq!(expected, tree(&f.root));
            }
        }
    }
}

#[test]
fn corrupt_files_preserve_diagnostics_and_serial_write_prefix() {
    let _lock = ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    for (renpy, lines) in [(true, 3), (false, 3), (true, 2000), (false, 2000)] {
        let f = Fixture::new(renpy, 8, lines);
        let plugin = Fixture::plugin(renpy);
        let entries = translated(plugin.extract(&f.root).unwrap());
        // First-seen entry order is the coordinator's file order.
        let mut paths = Vec::new();
        for entry in &entries {
            if !paths.contains(&entry.file_path) {
                paths.push(entry.file_path.clone());
            }
        }
        let corrupt = || {
            f.restore();
            fs::write(&paths[2], if renpy { &[0xff][..] } else { b"[" }).unwrap();
            fs::write(&paths[4], if renpy { &[0xff, 0xff][..] } else { b"}" }).unwrap();
        };
        corrupt();
        let _workers = Workers::set(1);
        let diagnostics = plugin.extract_with_diagnostics(&f.root).unwrap();
        assert!(!diagnostics.complete);
        assert_eq!(diagnostics.warnings.len(), 2);
        let first_error = plugin
            .inject(
                &f.root,
                &entries
                    .iter()
                    .filter(|e| e.file_path == paths[2])
                    .cloned()
                    .collect::<Vec<_>>(),
            )
            .unwrap_err()
            .to_string();
        let error = plugin.inject(&f.root, &entries).unwrap_err().to_string();
        assert_eq!(first_error, error);
        let expected = tree(&f.root);
        assert_ne!(
            fs::read(&paths[0]).unwrap(),
            f.files.iter().find(|(p, _)| p == &paths[0]).unwrap().1
        );
        assert_eq!(
            fs::read(&paths[3]).unwrap(),
            f.files.iter().find(|(p, _)| p == &paths[3]).unwrap().1
        );
        for workers in [2, 4, 8] {
            corrupt();
            let _workers = Workers::set(workers);
            assert_eq!(
                serialized(&diagnostics),
                serialized(&plugin.extract_with_diagnostics(&f.root).unwrap())
            );
            assert_eq!(
                error,
                plugin.inject(&f.root, &entries).unwrap_err().to_string()
            );
            assert_eq!(expected, tree(&f.root));
        }
    }
}

#[test]
fn revision_outputs_and_verified_originals_are_identical() {
    let _lock = ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    for (renpy, lines) in [(true, 3), (false, 3), (true, 2000), (false, 2000)] {
        let f = Fixture::new(renpy, 8, lines);
        let plugin = Fixture::plugin(renpy);
        let _workers = Workers::set(1);
        let rows = plugin.extract(&f.root).unwrap();
        let backup = f._dir.path().join("pristine");
        fs::create_dir(&backup).unwrap();
        let mut originals = HashMap::new();
        for (path, bytes) in &f.files {
            let original = backup.join(path.file_name().unwrap());
            fs::write(&original, bytes).unwrap();
            originals.insert(path.clone(), RevisionOriginal::capture(&original).unwrap());
        }
        let pristine = tree(&backup);
        plugin.inject(&f.root, &translated(rows.clone())).unwrap();
        let first: Vec<_> = f
            .files
            .iter()
            .map(|(p, _)| (p, fs::read(p).unwrap()))
            .collect();
        let mut expected = None;
        for workers in [1, 2, 4, 8] {
            for (path, bytes) in &first {
                fs::write(path, bytes).unwrap();
            }
            let _workers = Workers::set(workers);
            let mut revised = rows.clone();
            for row in &mut revised {
                row.translation = Some(format!("Segunda {}", row.source));
            }
            let report = plugin
                .inject_revision(&f.root, &mut revised, &originals)
                .unwrap();
            let result = (
                serde_json::to_value(report).unwrap(),
                tree(&f.root),
                serialized(&Extraction {
                    entries: revised,
                    warnings: vec![],
                    complete: true,
                }),
            );
            if let Some(expected) = &expected {
                assert_eq!(expected, &result);
            } else {
                expected = Some(result);
            }
            assert_eq!(pristine, tree(&backup));
        }
        // A captured proof must fail identically, without mutating the backup.
        let bad_backup = backup.join(f.files[0].0.file_name().unwrap());
        fs::write(&bad_backup, b"changed after verification").unwrap();
        let mut expected = None;
        for workers in [1, 8] {
            for (path, bytes) in &first {
                fs::write(path, bytes).unwrap();
            }
            let _workers = Workers::set(workers);
            let mut revised = translated(rows.clone());
            let error = plugin
                .inject_revision(&f.root, &mut revised, &originals)
                .unwrap_err()
                .to_string();
            assert!(error.contains("revision original"), "{error}");
            let result = (error, tree(&f.root));
            if let Some(expected) = &expected {
                assert_eq!(expected, &result);
            } else {
                expected = Some(result);
            }
        }
    }
}

#[test]
fn worker_errors_and_panics_commit_only_the_serial_prefix() {
    let _lock = ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    for panic in [false, true] {
        let mut expected = None;
        for workers in [1, 2, 4, 8] {
            let _workers = Workers::set(workers);
            let dir = tempfile::Builder::new()
                .prefix("locust-w3-worker-")
                .tempdir()
                .unwrap();
            let outcome = std::panic::catch_unwind(|| {
                file_workers::ordered(
                    &[0, 1, 2, 3, 4, 5],
                    |index| {
                        if *index == 2 {
                            assert!(!panic, "synthetic worker panic");
                            return Err("synthetic worker error");
                        }
                        Ok(index.to_string())
                    },
                    |index, result| {
                        fs::write(dir.path().join(index.to_string()), result?).unwrap();
                        Ok::<_, &str>(())
                    },
                )
            });
            if panic {
                assert!(outcome.is_err());
            } else {
                assert_eq!(outcome.unwrap(), Err("synthetic worker error"));
            }
            let actual = tree(dir.path());
            assert_eq!(actual.len(), 2);
            if let Some(expected) = &expected {
                assert_eq!(expected, &actual);
            } else {
                expected = Some(actual);
            }
        }
    }
}

#[test]
fn aliased_files_observe_preceding_coordinator_writes() {
    let _lock = ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    for (renpy, lines) in [(true, 1), (false, 1), (true, 2000), (false, 2000)] {
        let f = Fixture::new(renpy, 2, lines);
        let plugin = Fixture::plugin(renpy);
        let source = &f.files[usize::from(!renpy)].0;
        let alias = &f.files[usize::from(!renpy) + 1].0;
        fs::remove_file(alias).unwrap();
        fs::hard_link(source, alias).unwrap();
        let _workers = Workers::set(1);
        let rows = translated(plugin.extract(&f.root).unwrap());
        let bytes = fs::read(source).unwrap();
        let report = serde_json::to_value(plugin.inject(&f.root, &rows).unwrap()).unwrap();
        let expected = tree(&f.root);
        for workers in [1, 2, 4, 8] {
            // Reset independent files too. Both saved map paths now name the
            // source inode, so restore its original bytes after the full reset.
            f.restore();
            fs::write(source, &bytes).unwrap();
            let _workers = Workers::set(workers);
            assert_eq!(
                report,
                serde_json::to_value(plugin.inject(&f.root, &rows).unwrap()).unwrap()
            );
            assert_eq!(expected, tree(&f.root));
        }
    }
}

#[test]
fn physical_file_identity_rejects_aliases_and_unknown_destinations() {
    let f = Fixture::new(true, 2, 1);
    let first = &f.files[0].0;
    let second = &f.files[1].0;
    fs::write(second, fs::read(first).unwrap()).unwrap();
    // Identical content is independent; different spellings of one identity
    // are not. This exercises the guard without requiring hard-link creation.
    assert!(file_workers::independent_files([first, second]));
    assert!(!file_workers::independent_files([first, first]));
    let alias = first
        .parent()
        .unwrap()
        .join("..")
        .join("game")
        .join(first.file_name().unwrap());
    assert!(!file_workers::independent_files([first, &alias]));
    assert!(!file_workers::independent_files([
        first,
        &first.with_file_name("missing.rpy")
    ]));
}

fn check_differently_spelled_alias(renpy: bool, case_alias: bool) {
    for lines in [1, 2000] {
        let f = Fixture::new(renpy, 2, lines);
        let plugin = Fixture::plugin(renpy);
        let source = &f.files[usize::from(!renpy)].0;
        let independent = &f.files[usize::from(!renpy) + 1].0;
        let alias = if case_alias {
            source.with_file_name(if renpy {
                "SCRIPT0000.rpy"
            } else {
                "map0000.json"
            })
        } else {
            source
                .parent()
                .unwrap()
                .join("..")
                .join("game")
                .join(source.file_name().unwrap())
        };
        assert_eq!(fs::read(source).unwrap(), fs::read(&alias).unwrap());
        assert!(!file_workers::independent_files([source, &alias]));
        let _workers = Workers::set(1);
        let extracted = plugin.extract(&f.root).unwrap();
        let first: Vec<_> = extracted
            .iter()
            .filter(|row| &row.file_path == source)
            .cloned()
            .map(|mut row| {
                // Keep the synthetic ASCII text's width, so RPG message
                // wrapping cannot shift the later group's command IDs.
                row.translation = Some(format!("X{}", &row.source[1..]));
                row
            })
            .collect();
        let middle = translated(
            extracted
                .iter()
                .filter(|row| &row.file_path == independent)
                .cloned()
                .collect(),
        );
        let mut last = first.clone();
        for row in &mut last {
            row.file_path = alias.clone();
            let (_, suffix) = row.id.split_once('#').unwrap();
            row.id = format!("{}#{suffix}", alias.file_name().unwrap().to_string_lossy());
            // This group must observe the first group's committed translation,
            // then apply a second edit. A stale preparation cannot write it.
            row.source = row.translation.take().unwrap();
            row.translation = Some(format!("Y{}", &row.source[1..]));
        }
        let rows: Vec<_> = first.into_iter().chain(middle).chain(last).collect();
        let mut expected = None;
        for workers in [1, 1, 2, 4, 8] {
            f.restore();
            let _workers = Workers::set(workers);
            let report = plugin.inject(&f.root, &rows).unwrap();
            // The independent group is interleaved between two writes to the
            // alias, so the serial baseline must complete all three groups.
            assert_eq!(report.files_modified, 3);
            assert_eq!(report.strings_skipped, 0);
            assert_eq!(report.strings_written, rows.len());
            let result = (serde_json::to_value(report).unwrap(), tree(&f.root));
            if let Some(expected) = &expected {
                assert_eq!(
                    expected, &result,
                    "renpy={renpy}, lines={lines}, workers={workers}"
                );
            } else {
                expected = Some(result);
            }
        }
    }
}

#[test]
fn renpy_path_aliases_observe_preceding_coordinator_writes() {
    let _lock = ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    check_differently_spelled_alias(true, false);
}

#[cfg(windows)]
#[test]
fn case_aliases_observe_preceding_coordinator_writes_in_both_engines() {
    let _lock = ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    for renpy in [true, false] {
        check_differently_spelled_alias(renpy, true);
    }
}

#[test]
fn revision_proofs_observe_preceding_coordinator_writes() {
    let _lock = ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let f = Fixture::new(false, 2, 2000);
    let plugin = Fixture::plugin(false);
    let _workers = Workers::set(1);
    let rows: Vec<_> = translated(plugin.extract(&f.root).unwrap())
        .into_iter()
        .filter(|row| f.files[1..].iter().any(|(path, _)| path == &row.file_path))
        .collect();
    let first = rows[0].file_path.clone();
    let later = rows
        .iter()
        .find(|row| row.file_path != first)
        .unwrap()
        .file_path
        .clone();
    for path in [&first, &later] {
        assert!(rows.iter().filter(|row| &row.file_path == path).count() >= 1000);
    }
    // The later target's proof aliases the earlier live file. The API permits
    // capturing that path directly, so this also runs where hard links are
    // unavailable. The preceding write must invalidate the captured proof at
    // its serial position, even if a worker already verified the old bytes.
    let originals = HashMap::from([(later.clone(), RevisionOriginal::capture(&first).unwrap())]);
    let pristine = tree(&f.root);
    let mut expected = None;
    for workers in [1, 2, 4, 8] {
        f.restore();
        let _workers = Workers::set(workers);
        if workers > 1 {
            assert!(file_workers::parallel(
                file_workers::Phase::RpgInject,
                2,
                || true
            ));
        }
        let error = plugin
            .inject_revision(&f.root, &mut rows.clone(), &originals)
            .unwrap_err()
            .to_string();
        assert!(error.contains("revision original"), "{error}");
        let actual = tree(&f.root);
        let first_relative = first.strip_prefix(&f.root).unwrap();
        let later_relative = later.strip_prefix(&f.root).unwrap();
        assert_ne!(pristine[first_relative], actual[first_relative]);
        assert_eq!(pristine[later_relative], actual[later_relative]);
        let result = (error, actual);
        if let Some(expected) = &expected {
            assert_eq!(expected, &result);
        } else {
            expected = Some(result);
        }
    }
}

#[test]
fn compressed_and_bom_files_restore_exact_pristine_revision_bytes() {
    let _lock = ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    for lines in [3, 2000] {
        let mut f = Fixture::new(false, 8, lines);
        for (index, (path, bytes)) in f.files.iter_mut().enumerate() {
            fs::remove_file(&path).unwrap();
            if index % 2 == 0 {
                bytes.splice(0..0, [0xef, 0xbb, 0xbf]);
            } else {
                *bytes =
                    lz_str::compress_to_base64(std::str::from_utf8(bytes).unwrap()).into_bytes();
                *path = path.with_extension("jsono");
            }
        }
        f.restore();
        let pristine = tree(&f.root);
        let plugin = Fixture::plugin(false);
        let rows = plugin.extract(&f.root).unwrap();
        let backup = f._dir.path().join("pristine");
        fs::create_dir(&backup).unwrap();
        let mut originals = HashMap::new();
        for (path, bytes) in &f.files {
            let original = backup.join(path.file_name().unwrap());
            fs::write(&original, bytes).unwrap();
            originals.insert(path.clone(), RevisionOriginal::capture(&original).unwrap());
        }
        let mut expected = None;
        for workers in [1, 2, 4, 8] {
            f.restore();
            let _workers = Workers::set(workers);
            plugin.inject(&f.root, &translated(rows.clone())).unwrap();
            let mut restored = rows.clone();
            for row in &mut restored {
                row.translation = Some(row.source.clone());
            }
            let report = plugin
                .inject_revision(&f.root, &mut restored, &originals)
                .unwrap();
            assert_eq!(pristine, tree(&f.root));
            let report = serde_json::to_value(report).unwrap();
            if let Some(expected) = &expected {
                assert_eq!(expected, &report);
            } else {
                expected = Some(report);
            }
        }
    }
}

#[test]
fn loose_compiled_diagnostics_twins_and_duplicate_basenames_are_ordered() {
    let _lock = ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    for lines in [3, 2000] {
        let f = Fixture::new(true, 8, lines);
        let game = f.root.join("game");
        for sub in ["a", "b"] {
            fs::create_dir(game.join(sub)).unwrap();
            fs::write(
                game.join(sub).join("same.rpy"),
                b"label nested:\n    e \"Nested dialogue\"",
            )
            .unwrap();
            fs::write(game.join(sub).join("same.rpyc"), b"ignored exact twin").unwrap();
            fs::write(
                game.join(sub).join("corrupt.rpyc"),
                b"invalid compiled script",
            )
            .unwrap();
        }
        let plugin = Fixture::plugin(true);
        let _workers = Workers::set(1);
        let serial = plugin.extract_with_diagnostics(&f.root).unwrap();
        assert!(!serial.complete);
        assert_eq!(serial.warnings.len(), 2);
        let entries = translated(serial.entries.clone());
        let report = serde_json::to_value(plugin.inject(&f.root, &entries).unwrap()).unwrap();
        let expected = tree(&f.root);
        for workers in [2, 4, 8] {
            f.restore();
            for sub in ["a", "b"] {
                fs::write(
                    game.join(sub).join("same.rpy"),
                    b"label nested:\n    e \"Nested dialogue\"",
                )
                .unwrap();
                fs::write(game.join(sub).join("same.rpyc"), b"ignored exact twin").unwrap();
            }
            let _workers = Workers::set(workers);
            assert_eq!(
                serialized(&serial),
                serialized(&plugin.extract_with_diagnostics(&f.root).unwrap())
            );
            assert_eq!(
                report,
                serde_json::to_value(plugin.inject(&f.root, &entries).unwrap()).unwrap()
            );
            assert_eq!(expected, tree(&f.root));
        }
    }
}

#[test]
fn worker_preparations_overlap_and_respect_the_bound() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let _lock = ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    for (requested, bound) in [(4, 4), (32, 8)] {
        let _workers = Workers::set(requested);
        let active = AtomicUsize::new(0);
        let peak = AtomicUsize::new(0);
        let barrier = std::sync::Barrier::new(bound);
        file_workers::ordered(
            &(0..32).collect::<Vec<_>>(),
            |index| {
                let count = active.fetch_add(1, Ordering::SeqCst) + 1;
                peak.fetch_max(count, Ordering::SeqCst);
                if *index < bound {
                    barrier.wait();
                }
                active.fetch_sub(1, Ordering::SeqCst);
            },
            |_, ()| Ok::<_, ()>(()),
        )
        .unwrap();
        assert_eq!(peak.load(Ordering::SeqCst), bound);
    }
}

#[test]
#[ignore = "synthetic wall-time benchmark; run explicitly with --ignored --nocapture"]
fn benchmark_files() {
    let _lock = ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    for renpy in [true, false] {
        for (shape, count, lines) in [
            ("many-small", if renpy { 2000 } else { 1000 }, 4),
            ("few-large", 4, 10000),
        ] {
            let f = Fixture::new(renpy, count, lines);
            let plugin = Fixture::plugin(renpy);
            let mut expected_extraction = None;
            let mut expected_output = None;
            for workers in [1, 2, 4, 8] {
                let _workers = Workers::set(workers);
                let mut extract = Vec::new();
                let mut inject = Vec::new();
                for _ in 0..3 {
                    f.restore();
                    let start = Instant::now();
                    let entries = plugin.extract_with_diagnostics(&f.root).unwrap();
                    extract.push(start.elapsed().as_secs_f64() * 1000.0);
                    assert!(entries.complete);
                    let bytes = serialized(&entries);
                    if let Some(expected) = &expected_extraction {
                        assert_eq!(expected, &bytes);
                    } else {
                        expected_extraction = Some(bytes);
                    }
                    let entries = translated(entries.entries);
                    let start = Instant::now();
                    plugin.inject(&f.root, &entries).unwrap();
                    inject.push(start.elapsed().as_secs_f64() * 1000.0);
                    let output = tree(&f.root);
                    if let Some(expected) = &expected_output {
                        assert_eq!(expected, &output);
                    } else {
                        expected_output = Some(output);
                    }
                }
                extract.sort_by(f64::total_cmp);
                inject.sort_by(f64::total_cmp);
                println!(
                    "W3,{},{shape},{workers},{:.3},{:.3}",
                    if renpy { "renpy" } else { "rpgmaker" },
                    extract[1],
                    inject[1]
                );
                println!("W3,process_peak_rss_bytes,{}", peak_rss());
            }
        }
    }
}
