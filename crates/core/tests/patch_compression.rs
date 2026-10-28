//! Synthetic compression regression tests and an opt-in, process-isolated benchmark.
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

use locust_core::database::{sha256_file, Database};
use locust_core::models::{StringEntry, StringStatus};
use locust_core::patch::manifest::{PatchManifest, VerificationTier};
use locust_core::patch::{
    apply, pack_injection_recording, rollback, verify, ApplyOptions, PackOptions, PatchCompression,
    RollbackOptions, VerificationOutcome,
};

const POLICIES: [PatchCompression; 3] = [
    PatchCompression::Default,
    PatchCompression::Fast,
    PatchCompression::Store,
];

fn options(root: &Path, output: PathBuf) -> PackOptions {
    PackOptions {
        game_path: root.join("recorded"),
        lang: Some("es".into()),
        output,
        pristine: Some(root.join("pristine")),
        engine: Some("synthetic".into()),
        project: root.join("project.db"),
        require_pristine: true,
        game: None,
    }
}

fn record(root: &Path, files: &[PathBuf]) {
    let db = Database::open(&root.join("project.db")).unwrap();
    let mut entry = StringEntry::new("synthetic", "Hello", files[0].clone());
    entry.translation = Some("Hola".into());
    entry.status = StringStatus::Translated;
    db.save_entries(&[entry]).unwrap();
    db.record_injection(Some("es"), &root.join("recorded"), files)
        .unwrap();
}

fn small_fixture() -> tempfile::TempDir {
    let root = tempfile::Builder::new()
        .prefix("w3-patch-compression-")
        .tempdir()
        .unwrap();
    fs::create_dir(root.path().join("recorded")).unwrap();
    fs::create_dir(root.path().join("pristine")).unwrap();
    fs::write(root.path().join("pristine/script.txt"), b"Hello").unwrap();
    let script = root.path().join("recorded/script.txt");
    let added = root.path().join("recorded/added.txt");
    fs::write(&script, "Hola, ¿cómo estás?\n".repeat(4096)).unwrap();
    fs::write(&added, b"new payload").unwrap();
    record(root.path(), &[script, added]);
    root
}

fn install_root(root: &Path) -> PathBuf {
    let install = root.join("install");
    fs::create_dir_all(&install).unwrap();
    fs::copy(root.join("pristine/script.txt"), install.join("script.txt")).unwrap();
    install
}

/// Re-run the former writer options over the exact same entry bytes and ZIP
/// timestamps. This retains the UUID/created_at rather than normalizing them.
fn rewrite(zip: &Path, policy: PatchCompression, tamper: bool) -> Vec<u8> {
    let mut archive = zip::ZipArchive::new(File::open(zip).unwrap()).unwrap();
    let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).unwrap();
        let mut bytes = Vec::new();
        entry.read_to_end(&mut bytes).unwrap();
        if tamper && entry.name() == "script.txt" {
            bytes[0] ^= 1;
        }
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(if policy == PatchCompression::Store {
                zip::CompressionMethod::Stored
            } else {
                zip::CompressionMethod::Deflated
            })
            .last_modified_time(entry.last_modified().unwrap_or_default());
        let opts = if policy == PatchCompression::Fast {
            opts.compression_level(Some(1))
        } else {
            opts
        };
        writer.start_file(entry.name(), opts).unwrap();
        writer.write_all(&bytes).unwrap();
    }
    writer.finish().unwrap().into_inner()
}

#[test]
fn patch_compression_implicit_and_explicit_defaults_match_legacy_zip_bytes() {
    let root = small_fixture();
    let db = Database::open(&root.path().join("project.db")).unwrap();
    for explicit in [false, true] {
        let output = root.path().join(format!("default-{explicit}.zip"));
        let opts = options(root.path(), output.clone());
        if explicit {
            pack_injection_recording(&db, opts.with_compression(PatchCompression::Default))
                .unwrap();
        } else {
            pack_injection_recording(&db, opts).unwrap();
        }
        let legacy = rewrite(&output, PatchCompression::Default, false);
        assert_eq!(fs::read(&output).unwrap(), legacy);
    }
}

#[test]
fn patch_compression_policies_preserve_hashes_strict_apply_and_rollback() {
    for policy in POLICIES {
        let root = small_fixture();
        let db = Database::open(&root.path().join("project.db")).unwrap();
        let output = root.path().join("patch.zip");
        let report = pack_injection_recording(
            &db,
            options(root.path(), output.clone()).with_compression(policy),
        )
        .unwrap();
        assert_eq!(report.tier, "strict");
        // Also distinguish fast's actual level 1 stream from default Deflate.
        assert_eq!(fs::read(&output).unwrap(), rewrite(&output, policy, false));
        let mut archive = zip::ZipArchive::new(File::open(&output).unwrap()).unwrap();
        let manifest: PatchManifest =
            serde_json::from_reader(archive.by_name(PatchManifest::FILENAME).unwrap()).unwrap();
        for index in 0..archive.len() {
            let entry = archive.by_index(index).unwrap();
            assert_eq!(
                entry.compression(),
                if policy == PatchCompression::Store {
                    zip::CompressionMethod::Stored
                } else {
                    zip::CompressionMethod::Deflated
                }
            );
        }
        for file in &manifest.files {
            let (hash, size) = sha256_file(&root.path().join("recorded").join(&file.path)).unwrap();
            assert_eq!(file.patched_sha256, hash);
            assert_eq!(file.size, size);
        }
        drop(archive);
        let install = install_root(root.path());
        let verified = verify(&install, &output).unwrap();
        assert_eq!(verified.outcome, VerificationOutcome::Clean);
        assert_eq!(verified.tier, Some(VerificationTier::Strict));
        let applied = apply(&install, &output, ApplyOptions::default(), |_| {}).unwrap();
        assert_eq!((applied.replaced, applied.added), (1, 1));
        for file in &manifest.files {
            assert_eq!(
                sha256_file(&install.join(&file.path)).unwrap().0,
                file.patched_sha256
            );
        }
        let restored = rollback(&install, RollbackOptions::default()).unwrap();
        assert_eq!((restored.restored, restored.deleted), (1, 1));
        assert_eq!(fs::read(install.join("script.txt")).unwrap(), b"Hello");
        assert!(!install.join("added.txt").exists());
    }
}

#[test]
fn patch_compression_policies_reject_drift_containment_and_corrupt_payloads() {
    for policy in POLICIES {
        let root = small_fixture();
        let db = Database::open(&root.path().join("project.db")).unwrap();
        for tree in ["recorded", "pristine"] {
            let output = root.path().join(tree).join("new/patch.zip");
            assert!(pack_injection_recording(
                &db,
                options(root.path(), output).with_compression(policy)
            )
            .is_err());
            assert!(!root.path().join(tree).join("new").exists());
        }
        let output = root.path().join("patch.zip");
        pack_injection_recording(
            &db,
            options(root.path(), output.clone()).with_compression(policy),
        )
        .unwrap();
        let corrupt = root.path().join("corrupt.zip");
        fs::write(&corrupt, rewrite(&output, policy, true)).unwrap();
        let install = install_root(root.path());
        assert!(apply(&install, &corrupt, ApplyOptions::default(), |_| {}).is_err());
        assert_eq!(fs::read(install.join("script.txt")).unwrap(), b"Hello");
        assert!(!install.join("added.txt").exists());
        assert!(!install.join(".locust/backup/manifest.json").exists());

        let sentinel = fs::read(&output).unwrap();
        let mut file = File::options()
            .write(true)
            .open(root.path().join("recorded/script.txt"))
            .unwrap();
        file.write_all(b"X").unwrap(); // Same size, different recorded hash.
        drop(file);
        assert!(pack_injection_recording(
            &db,
            options(root.path(), output.clone()).with_compression(policy)
        )
        .is_err());
        assert_eq!(fs::read(&output).unwrap(), sentinel);
    }
}

fn synthetic_fixture(root: &Path, container: bool) -> Vec<PathBuf> {
    fs::create_dir_all(root.join("recorded")).unwrap();
    fs::create_dir_all(root.join("pristine")).unwrap();
    let mut paths = Vec::new();
    let mut random = 0x9e37_79b9_u32;
    let (count, blocks) = if container { (2, 64) } else { (64, 1) };
    for index in 0..count {
        let name = format!(
            "synthetic-{index}.{}",
            if container { "bundle" } else { "txt" }
        );
        let path = root.join("recorded").join(&name);
        let mut file = std::io::BufWriter::new(File::create(&path).unwrap());
        for block in 0..blocks {
            let mut bytes = Vec::with_capacity(1024 * 1024);
            if container {
                // 6.25% metadata + high-entropy synthetic asset blocks, like a
                // modified container dominated by already-compressed resources.
                bytes.extend(b"asset metadata: offset length translated title\n".repeat(1600));
                bytes.resize(65536, b' ');
                while bytes.len() < 1024 * 1024 {
                    random ^= random << 13;
                    random ^= random >> 17;
                    random ^= random << 5;
                    bytes.extend(random.to_le_bytes());
                }
            } else {
                let mut line = 0;
                while bytes.len() < 1024 * 1024 {
                    bytes.extend(format!(
                        "scene {index:03} line {line:06}: Personaje {}: Hola, ¿cómo estás? Busca el objeto {} en la sala {}.\n",
                        line % 17, (line * 73) % 10007, (line * 13) % 997
                    ).as_bytes());
                    line += 1;
                }
                bytes.truncate(1024 * 1024);
            }
            if block == 0 {
                bytes[..8].copy_from_slice(b"patched!");
            }
            file.write_all(&bytes).unwrap();
        }
        file.flush().unwrap();
        drop(file);
        let pristine = root.join("pristine").join(name);
        fs::copy(&path, &pristine).unwrap();
        let mut original = File::options().write(true).open(pristine).unwrap();
        original.seek(SeekFrom::Start(0)).unwrap();
        original.write_all(b"original").unwrap();
        paths.push(path);
    }
    record(root, &paths);
    paths
}

#[cfg(windows)]
fn peak_rss_bytes() -> u64 {
    #[repr(C)]
    #[derive(Default)]
    struct Counters {
        cb: u32,
        page_fault_count: u32,
        peak_working_set_size: usize,
        working_set_size: usize,
        quota_peak_paged_pool_usage: usize,
        quota_paged_pool_usage: usize,
        quota_peak_non_paged_pool_usage: usize,
        quota_non_paged_pool_usage: usize,
        pagefile_usage: usize,
        peak_pagefile_usage: usize,
    }
    #[link(name = "kernel32")]
    extern "system" {
        fn GetCurrentProcess() -> *mut std::ffi::c_void;
        fn K32GetProcessMemoryInfo(
            process: *mut std::ffi::c_void,
            counters: *mut Counters,
            size: u32,
        ) -> i32;
    }
    let size = std::mem::size_of::<Counters>() as u32;
    let mut counters = Counters {
        cb: size,
        ..Default::default()
    };
    // SAFETY: the OS pseudo-handle is valid, and counters is writable for size.
    let success = unsafe { K32GetProcessMemoryInfo(GetCurrentProcess(), &mut counters, size) };
    assert_ne!(success, 0);
    counters.peak_working_set_size as u64
}

#[cfg(not(windows))]
fn peak_rss_bytes() -> u64 {
    let status = fs::read_to_string("/proc/self/status").expect("Linux /proc is required for RSS");
    status
        .lines()
        .find_map(|line| {
            line.strip_prefix("VmHWM:")?
                .split_whitespace()
                .next()?
                .parse::<u64>()
                .ok()
        })
        .unwrap()
        * 1024
}

#[test]
#[ignore = "benchmark subprocess; invoked by patch_compression_measurements"]
fn patch_compression_benchmark_child() {
    let Some(root) = std::env::var_os("LOCUST_COMPRESSION_BENCH_ROOT") else {
        return; // A standalone --ignored run has no parent benchmark fixture.
    };
    let root = PathBuf::from(root);
    let stage = std::env::var("LOCUST_COMPRESSION_BENCH_STAGE").unwrap();
    let policy: PatchCompression = std::env::var("LOCUST_COMPRESSION_BENCH_POLICY")
        .unwrap()
        .parse()
        .unwrap();
    let output = root.join("benchmark.zip");
    let (elapsed, size) = if stage == "pack" {
        let db = Database::open(&root.join("project.db")).unwrap();
        let started = Instant::now();
        let report =
            pack_injection_recording(&db, options(&root, output).with_compression(policy)).unwrap();
        assert_eq!(report.tier, "strict");
        (started.elapsed(), report.size_bytes)
    } else {
        let install = root.join("install");
        let started = Instant::now();
        apply(&install, &output, ApplyOptions::default(), |_| {}).unwrap();
        let elapsed = started.elapsed();
        // Capture RSS before rollback, and check every installed payload hash.
        let rss = peak_rss_bytes();
        let mut archive = zip::ZipArchive::new(File::open(&output).unwrap()).unwrap();
        let manifest: PatchManifest =
            serde_json::from_reader(archive.by_name(PatchManifest::FILENAME).unwrap()).unwrap();
        for file in &manifest.files {
            assert_eq!(
                sha256_file(&install.join(&file.path)).unwrap().0,
                file.patched_sha256
            );
        }
        let restored = rollback(&install, RollbackOptions::default()).unwrap();
        assert_eq!(restored.restored, manifest.files.len());
        for file in &manifest.files {
            assert_eq!(
                sha256_file(&install.join(&file.path)).unwrap().0,
                *file.original_sha256.as_ref().unwrap()
            );
        }
        println!(
            "W3_MEASURE {}",
            serde_json::json!({"ms": elapsed.as_secs_f64() * 1000.0, "rss": rss})
        );
        return;
    };
    println!(
        "W3_MEASURE {}",
        serde_json::json!({"ms": elapsed.as_secs_f64() * 1000.0, "rss": peak_rss_bytes(), "size": size})
    );
}

#[test]
#[ignore = "release benchmark: 64 MiB text and 128 MiB container, three fresh-process trials"]
fn patch_compression_measurements() {
    for container in [false, true] {
        let fixture = tempfile::Builder::new()
            .prefix("w3-compression-bench-")
            .tempdir()
            .unwrap();
        let root = fixture.path();
        let paths = synthetic_fixture(root, container);
        fs::create_dir(root.join("install")).unwrap();
        for path in &paths {
            let name = path.file_name().unwrap();
            fs::copy(
                root.join("pristine").join(name),
                root.join("install").join(name),
            )
            .unwrap();
        }
        for trial in 0..3 {
            for offset in 0..3 {
                let policy = ["default", "fast", "store"][(trial + offset) % 3];
                let mut measurements = serde_json::json!({"fixture": if container {"container"} else {"text"}, "trial": trial + 1, "policy": policy});
                for stage in ["pack", "apply"] {
                    let output = Command::new(std::env::current_exe().unwrap())
                        .args([
                            "--exact",
                            "patch_compression_benchmark_child",
                            "--ignored",
                            "--nocapture",
                        ])
                        .env("LOCUST_COMPRESSION_BENCH_ROOT", root)
                        .env("LOCUST_COMPRESSION_BENCH_STAGE", stage)
                        .env("LOCUST_COMPRESSION_BENCH_POLICY", policy)
                        .output()
                        .unwrap();
                    assert!(
                        output.status.success(),
                        "{}\n{}",
                        String::from_utf8_lossy(&output.stdout),
                        String::from_utf8_lossy(&output.stderr)
                    );
                    let stdout = String::from_utf8(output.stdout).unwrap();
                    let value = stdout
                        .lines()
                        .find_map(|line| line.strip_prefix("W3_MEASURE "))
                        .unwrap();
                    measurements[stage] = serde_json::from_str(value).unwrap();
                }
                println!("W3_RESULT {measurements}");
            }
        }
    }
}
