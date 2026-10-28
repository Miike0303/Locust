//! Source for the generated .bench-w3 release-only paired benchmark binary.
use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Instant;

use locust_core::extraction::{Extraction, FormatPlugin, InjectionReport};
use locust_core::models::StringEntry;
use serde_json::json;

struct Fixture {
    root: PathBuf,
    files: Vec<(PathBuf, Vec<u8>)>,
}

impl Fixture {
    fn new(root: PathBuf, renpy: bool, count: usize, lines: usize) -> Self {
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
        let fixture = Self { root, files };
        fixture.restore();
        fixture
    }

    fn restore(&self) {
        for (path, bytes) in &self.files {
            fs::write(path, bytes).unwrap();
        }
    }
}

fn plugin(renpy: bool, baseline: bool) -> Box<dyn FormatPlugin> {
    match (renpy, baseline) {
        (true, false) => Box::new(locust_formats::renpy::RenPyPlugin::new()),
        (false, false) => Box::new(locust_formats::rpgmaker_mv::RpgMakerMvPlugin::new()),
        (true, true) => Box::new(locust_formats_serial_baseline::renpy::RenPyPlugin::new()),
        (false, true) => {
            Box::new(locust_formats_serial_baseline::rpgmaker_mv::RpgMakerMvPlugin::new())
        }
    }
}

fn canonical_entries(extraction: &Extraction) -> Vec<u8> {
    let mut rows = serde_json::to_value(&extraction.entries).unwrap();
    for row in rows.as_array_mut().unwrap() {
        row["created_at"] = json!("2000-01-01T00:00:00Z");
    }
    serde_json::to_vec(&(rows, &extraction.warnings, extraction.complete)).unwrap()
}

fn canonical_report(report: InjectionReport) -> serde_json::Value {
    let mut report = serde_json::to_value(report).unwrap();
    if let Some(files) = report
        .get_mut("files_written")
        .and_then(|v| v.as_array_mut())
    {
        files.sort_by_key(|v| v.to_string());
    }
    report
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

#[derive(Clone, Copy)]
struct Cpu {
    idle: u64,
    kernel: u64,
    user: u64,
    process: u64,
}

#[cfg(windows)]
fn cpu() -> Cpu {
    #[repr(C)]
    #[derive(Default)]
    struct FileTime {
        low: u32,
        high: u32,
    }
    impl FileTime {
        fn ticks(&self) -> u64 {
            (u64::from(self.high) << 32) | u64::from(self.low)
        }
    }
    #[link(name = "kernel32")]
    extern "system" {
        fn GetSystemTimes(idle: *mut FileTime, kernel: *mut FileTime, user: *mut FileTime) -> i32;
        fn GetProcessTimes(
            process: isize,
            creation: *mut FileTime,
            exit: *mut FileTime,
            kernel: *mut FileTime,
            user: *mut FileTime,
        ) -> i32;
    }
    let (mut idle, mut kernel, mut user) = (
        FileTime::default(),
        FileTime::default(),
        FileTime::default(),
    );
    let (mut creation, mut exit, mut process_kernel, mut process_user) = (
        FileTime::default(),
        FileTime::default(),
        FileTime::default(),
        FileTime::default(),
    );
    // SAFETY: all pointers reference valid output buffers with FILETIME layout;
    // -1 is Windows' documented current-process pseudo handle.
    assert_ne!(
        unsafe { GetSystemTimes(&mut idle, &mut kernel, &mut user) },
        0
    );
    assert_ne!(
        unsafe {
            GetProcessTimes(
                -1,
                &mut creation,
                &mut exit,
                &mut process_kernel,
                &mut process_user,
            )
        },
        0
    );
    Cpu {
        idle: idle.ticks(),
        kernel: kernel.ticks(),
        user: user.ticks(),
        process: process_kernel.ticks() + process_user.ticks(),
    }
}

#[cfg(not(windows))]
fn cpu() -> Cpu {
    Cpu {
        idle: 0,
        kernel: 0,
        user: 0,
        process: 0,
    }
}

#[cfg(windows)]
fn priority_class() -> u32 {
    #[link(name = "kernel32")]
    extern "system" {
        fn GetPriorityClass(process: isize) -> u32;
    }
    // SAFETY: -1 is Windows' documented current-process pseudo handle.
    let priority = unsafe { GetPriorityClass(-1) };
    assert_ne!(priority, 0, "GetPriorityClass failed");
    priority
}

#[cfg(not(windows))]
fn priority_class() -> u32 {
    0x20
}

fn sample(
    engine: &str,
    shape: &str,
    variant: &str,
    phase: &str,
    run: usize,
    attempt: usize,
    start: Instant,
    before: Cpu,
    expected_priority: u32,
) {
    let ms = start.elapsed().as_secs_f64() * 1000.0;
    let after = cpu();
    let priority = priority_class();
    assert_eq!(priority, expected_priority, "Benchmark priority changed");
    let total = (after.kernel - before.kernel + after.user - before.user) as f64;
    let busy = if total == 0.0 {
        0.0
    } else {
        100.0 * (total - (after.idle - before.idle) as f64) / total
    };
    let process = if total == 0.0 {
        0.0
    } else {
        100.0 * (after.process - before.process) as f64 / total
    };
    println!(
        "{}",
        json!({
            "kind": "sample", "engine": engine, "shape": shape,
            "variant": variant, "phase": phase, "run": run, "attempt": attempt,
            "ms": ms, "cpu_busy_pct": busy, "process_cpu_pct": process,
        "external_cpu_pct": (busy - process).max(0.0), "priority_class": priority
        })
    );
    std::io::stdout().flush().unwrap();
}

fn argument(name: &str) -> Option<String> {
    let args: Vec<_> = std::env::args().collect();
    args.windows(2)
        .find(|pair| pair[0] == name)
        .map(|pair| pair[1].clone())
}

fn main() {
    assert!(!cfg!(debug_assertions), "Use cargo build --release");
    if std::env::args().any(|argument| argument == "--build-manifest") {
        println!("{}", include_str!("source-manifest.json"));
        return;
    }
    let root = PathBuf::from(argument("--root").expect("--root fixture path"));
    let runs: usize = argument("--runs")
        .unwrap_or_else(|| "5".into())
        .parse()
        .unwrap();
    let attempt: usize = argument("--attempt")
        .unwrap_or_else(|| "1".into())
        .parse()
        .unwrap();
    let engine_filter = argument("--engine");
    let shape_filter = argument("--shape");
    let requested_priority = argument("--priority").unwrap_or_else(|| "normal".into());
    let expected_priority = match requested_priority.as_str() {
        "normal" => 0x20,
        "above-normal" => 0x8000,
        _ => panic!("Unsupported benchmark priority"),
    };
    assert_eq!(
        priority_class(),
        expected_priority,
        "Benchmark priority differs from requested class"
    );
    println!(
        "{}",
        json!({"kind": "runtime", "pid": std::process::id(), "requested_priority": requested_priority, "priority_class": expected_priority})
    );
    let variants = if argument("--default-only").as_deref() == Some("true") {
        vec![("baseline", Some(1), true), ("default", None, false)]
    } else {
        vec![
            ("baseline", Some(1), true),
            ("1", Some(1), false),
            ("2", Some(2), false),
            ("4", Some(4), false),
            ("8", Some(8), false),
        ]
    };
    for renpy in [true, false] {
        let engine = if renpy { "renpy" } else { "rpgmaker" };
        if engine_filter
            .as_deref()
            .is_some_and(|filter| filter != engine)
        {
            continue;
        }
        for (shape, count, lines) in [
            ("many-small", if renpy { 2000 } else { 1000 }, 4),
            ("few-large", 4, 10000),
        ] {
            if shape_filter
                .as_deref()
                .is_some_and(|filter| filter != shape)
            {
                continue;
            }
            let fixture = Fixture::new(root.join(engine).join(shape), renpy, count, lines);
            println!(
                "{}",
                json!({"kind":"progress", "message":format!("{engine}/{shape}: {count} files x {lines} lines, attempt {attempt}")})
            );
            let mut expected_entries = None;
            let mut expected_output = None;
            let mut expected_report = None;
            // Warm every configuration once, then rotate order between runs.
            for run in 0..=runs {
                for offset in 0..variants.len() {
                    let configuration = (offset + run) % variants.len();
                    let (variant, workers, baseline) = variants[configuration];
                    match workers {
                        Some(workers) => {
                            std::env::set_var("LOCUST_FILE_WORKERS", workers.to_string())
                        }
                        None => std::env::remove_var("LOCUST_FILE_WORKERS"),
                    };
                    let plugin = plugin(renpy, baseline);
                    fixture.restore();
                    let before = cpu();
                    let start = Instant::now();
                    let extraction = plugin.extract_with_diagnostics(&fixture.root).unwrap();
                    if run > 0 {
                        sample(
                            engine,
                            shape,
                            variant,
                            "extract",
                            run,
                            attempt,
                            start,
                            before,
                            expected_priority,
                        );
                    }
                    assert!(extraction.complete);
                    let entries = canonical_entries(&extraction);
                    if let Some(expected) = &expected_entries {
                        assert_eq!(&entries, expected, "baseline/candidate extraction mismatch");
                    } else {
                        expected_entries = Some(entries);
                    }
                    let translated: Vec<StringEntry> = extraction
                        .entries
                        .into_iter()
                        .map(|mut entry| {
                            entry.translation = Some(format!("Traducido {}", entry.source));
                            entry
                        })
                        .collect();
                    let before = cpu();
                    let start = Instant::now();
                    let report = plugin.inject(&fixture.root, &translated).unwrap();
                    if run > 0 {
                        sample(
                            engine,
                            shape,
                            variant,
                            "inject",
                            run,
                            attempt,
                            start,
                            before,
                            expected_priority,
                        );
                    }
                    let report = canonical_report(report);
                    if let Some(expected) = &expected_report {
                        assert_eq!(&report, expected, "baseline/candidate report mismatch");
                    } else {
                        expected_report = Some(report);
                    }
                    let output = tree(&fixture.root);
                    if let Some(expected) = &expected_output {
                        assert_eq!(
                            &output, expected,
                            "baseline/candidate BLAKE3 output mismatch"
                        );
                    } else {
                        expected_output = Some(output);
                    }
                }
            }
        }
    }
}
