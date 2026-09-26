//! Owned archive replacement shared by Tyrano and YU-RIS. Callers acquire the
//! game lock before opening any source archive, and retain it through commit.
use locust_core::{
    error::{LocustError, Result},
    patch::{stream::StagingDir, zipsec::ensure_no_links, GameLock, PatchStore},
};
use std::{
    fs::{self, File},
    io::Write,
    path::{Path, PathBuf},
};

pub(crate) fn guard_target(lock: &GameLock, path: &Path) -> Result<()> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let name = path
        .file_name()
        .ok_or_else(|| LocustError::PatchError("archive target has no filename".into()))?;
    ensure_no_links(parent, Path::new(name))?;
    let canonical = parent.canonicalize()?.join(name);
    let relative = canonical.strip_prefix(lock.root()).map_err(|_| {
        LocustError::PatchError(format!(
            "archive target is outside selected game: {}",
            path.display()
        ))
    })?;
    ensure_no_links(lock.root(), relative)
}

struct Prepared {
    output: PathBuf,
    stage: StagingDir,
}

impl Prepared {
    fn create(
        lock: &GameLock,
        output: &Path,
        write: impl FnOnce(&mut File) -> Result<()>,
    ) -> Result<Self> {
        guard_target(lock, output)?;
        if !fs::symlink_metadata(output)?.is_file() {
            return Err(LocustError::PatchError(format!(
                "archive target is not a regular file: {}",
                output.display()
            )));
        }
        let parent = output
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        let stage = StagingDir::create_prepared(parent)?;
        let mut next = stage.create_file("replacement")?;
        write(&mut next)?;
        next.sync_all()?;
        let mut previous = stage.create_file("previous")?;
        std::io::copy(&mut File::open(output)?, &mut previous)?;
        previous.sync_all()?;
        Ok(Self {
            output: output.to_owned(),
            stage,
        })
    }

    fn restore(&self, lock: &GameLock) -> Result<()> {
        let mut restore = self.stage.create_file("restore")?;
        std::io::copy(&mut File::open(self.stage.child("previous"))?, &mut restore)?;
        restore.sync_all()?;
        drop(restore);
        guard_target(lock, &self.output)?;
        PatchStore::replace_file(&self.stage.child("restore"), &self.output)
    }
}

/// Prepare every output before changing any destination. ASAR unpacked files
/// and their size-bearing archive header form one batch. On a later install
/// failure restore earlier destinations; retain/report every failed recovery.
/// Successful backups are extensionless and deliberately kept for manual undo.
pub(crate) fn replace_files(lock: &GameLock, files: &[(PathBuf, Vec<u8>)]) -> Result<Vec<PathBuf>> {
    let mut seen = std::collections::HashSet::new();
    let mut prepared = Vec::new();
    for (path, bytes) in files {
        let key = path.canonicalize()?;
        #[cfg(windows)]
        let key = key.to_string_lossy().to_lowercase();
        if !seen.insert(key) {
            return Err(LocustError::PatchError(
                "duplicate archive replacement target".into(),
            ));
        }
        prepared.push(Prepared::create(lock, path, |file| {
            file.write_all(bytes)?;
            Ok(())
        })?);
    }
    commit(lock, prepared, |_, _| Ok(()))
}

fn commit(
    lock: &GameLock,
    mut prepared: Vec<Prepared>,
    before: impl Fn(usize, &Path) -> Result<()>,
) -> Result<Vec<PathBuf>> {
    for index in 0..prepared.len() {
        let current = &prepared[index];
        let installed = before(index, &current.output)
            .and_then(|_| guard_target(lock, &current.output))
            .and_then(|_| {
                PatchStore::replace_file(&current.stage.child("replacement"), &current.output)
            });
        if let Err(error) = installed {
            let mut failures = Vec::new();
            for previous in prepared[..index].iter_mut().rev() {
                if let Err(restore_error) = previous.restore(lock) {
                    previous.stage.disarm();
                    failures.push(format!(
                        "{}: {restore_error}; original retained at {}",
                        previous.output.display(),
                        previous.stage.child("previous").display()
                    ));
                }
            }
            return if failures.is_empty() {
                Err(error)
            } else {
                Err(LocustError::PatchError(format!(
                    "{error}; rollback incomplete: {}",
                    failures.join("; ")
                )))
            };
        }
    }
    Ok(prepared
        .into_iter()
        .map(|mut file| {
            file.stage.disarm();
            file.stage.child("previous")
        })
        .collect())
}

pub(crate) fn note_backups(backups: Vec<PathBuf>, warnings: &mut Vec<String>) {
    warnings.extend(
        backups
            .into_iter()
            .map(|path| format!("previous archive retained at {}", path.display())),
    );
}

#[cfg(test)]
#[path = "archive_replace_tests.rs"]
mod tests;
