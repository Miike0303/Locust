//! Native storage kernels used by backup and Replace. Run explicitly with
//! cargo test -p locust-core --test workflow_copy_verify -- --ignored --nocapture
//! This is a warm-cache microbenchmark, not a whole CLI workflow measurement.
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use locust_core::backup::BackupManager;
use locust_core::database::sha256_file;

fn jobs(workers: usize, files: &[PathBuf], work: impl Fn(&Path) + Sync) -> Duration {
    let started = Instant::now();
    if workers == 1 {
        for path in files {
            work(path);
        }
        return started.elapsed();
    }
    std::thread::scope(|scope| {
        for worker in 0..workers {
            let work = &work;
            // Match the production helper's bounded, strided file assignment.
            scope.spawn(move || {
                for path in files.iter().skip(worker).step_by(workers) {
                    work(path);
                }
            });
        }
    });
    started.elapsed()
}

#[test]
#[ignore = "explicit storage benchmark; creates about 200 MiB of generated files"]
fn benchmark_workflow_storage_kernels() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("source");
    fs::create_dir(&source).unwrap();
    let mut files = Vec::new();
    let block = vec![0x5a; 1024 * 1024];
    for index in 0..512 {
        let path = source.join(format!("small-{index:04}.bin"));
        fs::write(&path, &block[..4096]).unwrap();
        files.push(path);
    }
    for index in 0..3 {
        let path = source.join(format!("large-{index:02}.bin"));
        let mut stream = File::create_new(&path).unwrap();
        for _ in 0..32 {
            stream.write_all(&block).unwrap();
        }
        files.push(path);
    }
    println!("kernel | repeat | workers | seconds | logical bytes");
    for repeat in 0..3 {
        let orders = [[1, 2, 4], [2, 4, 1], [4, 1, 2]];
        for workers in orders[repeat] {
            let elapsed = jobs(workers, &files, |path| {
                std::hint::black_box(sha256_file(path).unwrap());
            });
            println!(
                "hash | {repeat} | {workers} | {:.6} | {}",
                elapsed.as_secs_f64(),
                98 * 1024 * 1024
            );
            for sync in [false, true] {
                let destination = root.path().join("copy");
                fs::create_dir(&destination).unwrap();
                let elapsed = jobs(workers, &files, |path| {
                    let mut input = File::open(path).unwrap();
                    let output_path = destination.join(path.file_name().unwrap());
                    let mut output = File::create_new(output_path).unwrap();
                    std::io::copy(&mut input, &mut output).unwrap();
                    if sync {
                        output.sync_all().unwrap();
                    }
                    output
                        .set_permissions(input.metadata().unwrap().permissions())
                        .unwrap();
                });
                println!(
                    "{} | {repeat} | {workers} | {:.6} | {}",
                    if sync { "backup-copy" } else { "replace-copy" },
                    elapsed.as_secs_f64(),
                    98 * 1024 * 1024
                );
                let mut buffer = vec![0u8; block.len()];
                for path in &files {
                    let mut copy = File::open(destination.join(path.file_name().unwrap())).unwrap();
                    assert_eq!(
                        copy.metadata().unwrap().len(),
                        fs::metadata(path).unwrap().len()
                    );
                    loop {
                        let size = copy.read(&mut buffer).unwrap();
                        if size == 0 {
                            break;
                        }
                        assert_eq!(&buffer[..size], &block[..size]);
                    }
                }
                fs::remove_dir_all(destination).unwrap();
            }
        }
    }
}

#[test]
fn backup_payload_rejects_same_size_restored_timestamp_drift() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("source");
    fs::create_dir(&source).unwrap();
    fs::write(source.join("asset.bin"), b"original").unwrap();
    for name in ["large-a.bin", "large-b.bin", "large-c.bin"] {
        fs::write(source.join(name), vec![0x5a; 3 * 1024 * 1024]).unwrap();
    }
    let manager = BackupManager::new(root.path().join("backups"));
    let backup = manager.create_backup(&source).unwrap();
    let payload = backup.path.join("payload/asset.bin");
    let before = fs::metadata(&payload).unwrap();
    fs::write(&payload, b"modified").unwrap();
    File::options()
        .write(true)
        .open(&payload)
        .unwrap()
        .set_times(fs::FileTimes::new().set_modified(before.modified().unwrap()))
        .unwrap();
    assert_eq!(fs::metadata(&payload).unwrap().len(), before.len());
    assert_eq!(
        fs::metadata(&payload).unwrap().modified().unwrap(),
        before.modified().unwrap()
    );
    assert!(manager
        .with_pristine_tree(&backup.id, &source, |_| Ok(()))
        .is_err());
    assert_eq!(fs::read(source.join("asset.bin")).unwrap(), b"original");
}
