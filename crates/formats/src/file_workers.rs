//! Bounded, read-only file preparation with coordinator-owned ordered effects.
use std::panic::{catch_unwind, resume_unwind, AssertUnwindSafe};
use std::sync::mpsc::sync_channel;

/// Only independent destinations may be prepared ahead of coordinator writes.
/// An alias anywhere in the batch selects the original serial loop for the
/// entire batch, preserving its file order, reads, errors and report accounting.
/// Unknown identities also select serial execution; probing must never surface
/// a later file's error before preceding writes have finished.
pub(crate) fn independent_files(
    paths: impl IntoIterator<Item = impl AsRef<std::path::Path>>,
) -> bool {
    let mut identities = std::collections::BTreeSet::new();
    let mut handles = Vec::new();
    for path in paths {
        let Ok(file) = std::fs::File::open(path) else {
            return false;
        };
        let Ok(identity) = locust_core::file_identity::identity(&file) else {
            return false;
        };
        if !identities.insert(identity) {
            return false;
        }
        // Do not allow identifier reuse while comparing the rest of the batch.
        handles.push(file);
    }
    true
}

pub(crate) fn worker_count() -> usize {
    std::env::var("LOCUST_FILE_WORKERS")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|count| *count > 0)
        // Release matrices under sustained external CPU contention did not
        // establish stable wins. Keep automatic execution on the original
        // loops; explicit counts retain the bounded parallel implementation.
        .unwrap_or(1)
        .min(8)
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum Phase {
    RenpyExtract,
    RenpyInject,
    RpgExtract,
    RpgInject,
}

fn explicit_workers() -> bool {
    std::env::var("LOCUST_FILE_WORKERS")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .is_some_and(|count| count > 1)
}

/// (large-file count, bytes per file, entries per injection group).
/// Conservative shape floors for automatic dispatch, currently bypassed by
/// the serial default. Explicit tuning uses a lower correctness-test floor.
pub(crate) fn thresholds(phase: Phase) -> (usize, u64, usize) {
    if explicit_workers() {
        (2, 64 * 1024, 1000)
    } else {
        match phase {
            Phase::RenpyExtract | Phase::RenpyInject => (4, 375 * 1024, 10000),
            Phase::RpgExtract | Phase::RpgInject => (4, 1024 * 1024, 20000),
        }
    }
}

/// Keep small-file workloads on the original loops. The lazy shape check is
/// never evaluated for one worker or more than 64 files, so those paths do not
/// stat inputs, allocate preparation buffers, or create channels.
/// Automatic execution stays serial until stable release evidence supports a
/// phase. Explicit counts allow measuring large-file candidate phases.
pub(crate) fn parallel(
    phase: Phase,
    file_count: usize,
    large_files: impl FnOnce() -> bool,
) -> bool {
    if !(2..=64).contains(&file_count) || worker_count() == 1 {
        return false;
    }
    // Every automatic phase is disabled by the one-worker default above.
    // Keep the explicit candidate dispatch unchanged from the release matrix.
    let default_enabled = matches!(phase, Phase::RpgInject);
    (explicit_workers() || default_enabled) && large_files()
}

/// Prepare at most one outstanding item per worker, then consume in input order.
/// Only `consume` may perform writes or emit diagnostics. Errors and panics in
/// later jobs cannot bypass preceding coordinator effects; on exit all workers
/// are joined. Memory and speculative work are bounded by the worker count.
pub(crate) fn ordered<T: Sync, R: Send, E>(
    items: &[T],
    prepare: impl Fn(&T) -> R + Sync,
    mut consume: impl FnMut(&T, R) -> Result<(), E>,
) -> Result<(), E> {
    let count = worker_count().min(items.len());
    if count <= 1 {
        for item in items {
            consume(item, prepare(item))?;
        }
        return Ok(());
    }
    std::thread::scope(|scope| {
        let mut jobs = Vec::with_capacity(count);
        let mut results = Vec::with_capacity(count);
        for _ in 0..count {
            let (send_job, receive_job) = sync_channel::<usize>(1);
            let (send_result, receive_result) = sync_channel(1);
            let prepare = &prepare;
            scope.spawn(move || {
                while let Ok(index) = receive_job.recv() {
                    let result = catch_unwind(AssertUnwindSafe(|| prepare(&items[index])));
                    if send_result.send(result).is_err() {
                        break;
                    }
                }
            });
            jobs.push(send_job);
            results.push(receive_result);
        }
        for (index, job) in jobs.iter().enumerate() {
            job.send(index).expect("file worker is running");
        }
        for (index, item) in items.iter().enumerate() {
            let lane = index % count;
            let result = results[lane]
                .recv()
                .expect("file worker returns an outcome");
            let result = match result {
                Ok(result) => result,
                Err(panic) => resume_unwind(panic),
            };
            consume(item, result)?;
            if index + count < items.len() {
                jobs[lane]
                    .send(index + count)
                    .expect("file worker is running");
            }
        }
        Ok(())
    })
}
