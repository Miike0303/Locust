use std::path::PathBuf;

use comfy_table::Table;
use locust_core::backup::BackupManager;
use locust_core::database::{Database, EntryFilter};
use locust_core::export;

use crate::util::{locust_backup_root, open_existing_project, require_existing_project};

pub(crate) fn cmd_backups() -> anyhow::Result<()> {
    let manager = BackupManager::new(locust_backup_root());
    let report = manager.list_backups_report()?;
    for backup in &report.entries {
        println!(
            "{}\t{}\t{} file(s), {} bytes\t{}",
            backup.id,
            locust_core::injection_transaction::display_path(&backup.source_path).display(),
            backup.file_count,
            backup.size_bytes,
            backup.created_at
        );
    }
    for (id, error) in &report.unreadable {
        eprintln!("warning: backup {id} has an unreadable manifest.json: {error}");
    }
    if report.entries.is_empty() {
        if !report.unreadable.is_empty() {
            anyhow::bail!("No readable injection backups found.");
        }
        println!("No injection backups found.");
    }
    let usage = manager.storage_usage()?;
    println!(
        "Total: {} logical bytes, {} on-disk bytes",
        usage.logical_bytes, usage.on_disk_bytes
    );
    Ok(())
}

pub(crate) fn cmd_delete_backup(id: &str, confirm_unverified: bool) -> anyhow::Result<()> {
    BackupManager::new(locust_backup_root())
        .delete_backup_with_confirmation(id, confirm_unverified)?;
    println!("Deleted backup {id}.");
    Ok(())
}

pub(crate) fn cmd_restore_backup(id: &str, dry_run: bool) -> anyhow::Result<()> {
    let manager = BackupManager::new(locust_backup_root());
    let preview = if dry_run {
        manager.preview_restore(id)?
    } else {
        manager.restore_with_report(id)?
    };
    if dry_run {
        println!(
            "Destination: {}",
            locust_core::injection_transaction::display_path(&preview.destination).display()
        );
        println!("Would replace {} file(s):", preview.replaced.len());
        for path in &preview.replaced {
            println!("  {}", path.display());
        }
        println!(
            "Would recreate {} missing file(s):",
            preview.recreated.len()
        );
        for path in &preview.recreated {
            println!("  {}", path.display());
        }
        println!("Already identical: {}", preview.identical.len());
    } else {
        println!("Restored backup {id} to its original game location.");
    }
    let action = if dry_run { "Would remove" } else { "Removed" };
    println!(
        "{action} {} file(s) created by Locust:",
        preview.removed.len()
    );
    for path in &preview.removed {
        println!("  {}", path.display());
    }
    println!(
        "{action} {} empty directory/directories created by Locust:",
        preview.removed_directories.len()
    );
    for path in &preview.removed_directories {
        println!("  {}", path.display());
    }
    println!("Kept {} added path(s):", preview.kept.len());
    for kept in &preview.kept {
        println!("  {}: {}", kept.path.display(), kept.reason);
    }
    Ok(())
}

pub(crate) fn cmd_pivot(
    source: PathBuf,
    output: PathBuf,
    require_complete: bool,
) -> anyhow::Result<()> {
    let src_db = open_existing_project(&source)?;
    let result = if require_complete {
        src_db.pivot_to_complete(&output)?
    } else {
        // Legacy partial workflow, also used by HTTP and Tauri pivots.
        src_db.pivot_to(&output)?
    };

    let mut table = Table::new();
    table.set_header(vec!["Metric", "Value"]);
    table.add_row(vec!["Pivoted project", &result.database_path]);
    table.add_row(vec!["Source entries used", &result.entries.to_string()]);
    println!("{table}");
    println!(
        "\nNow translate the new project into any language, e.g.:\n  \
         locust translate \"{}\" -p grok-sub -s en -t fr",
        output.display()
    );
    Ok(())
}

pub(crate) fn cmd_backup_project(project: PathBuf, output: PathBuf) -> anyhow::Result<()> {
    // Refuse before opening: on Linux even opening and closing the project
    // rewrites its file (WAL checkpoint). `snapshot_to` re-checks both.
    require_existing_project(&project)?;
    export::check_export_destination(&output, &project)?;
    if output.try_exists()? {
        anyhow::bail!(
            "checkpoint destination already exists: {}",
            output.display()
        );
    }
    let db = open_existing_project(&project)?;
    db.snapshot_to(&output)?;
    let checkpoint = Database::open(&output)?;
    let count = checkpoint.count_entries(&EntryFilter::default())?;
    drop(checkpoint);
    let size = std::fs::metadata(&output)?.len();
    println!(
        "Checkpoint: {} ({size} bytes, {count} strings)",
        output.display()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use locust_core::models::{StringEntry, StringStatus};
    use std::fs;

    #[test]
    fn cmd_pivot_skips_pending_and_refuses_overwrite() {
        let dir_guard = tempfile::Builder::new()
            .prefix("locust_cli_pivot_")
            .tempdir()
            .unwrap();
        let dir = dir_guard.path().to_path_buf();
        let src_path = dir.join("src.locust.db");
        let src = Database::open(&src_path).unwrap();
        let mut done = StringEntry::new("a", "Hello", PathBuf::from("f.json"));
        done.translation = Some("Hola".into());
        done.status = StringStatus::Translated;
        src.save_entries(&[
            done,
            StringEntry::new("b", "World", PathBuf::from("f.json")),
        ])
        .unwrap();
        drop(src);

        let out = dir.join("out.locust.db");
        cmd_pivot(src_path.clone(), out.clone(), false).unwrap();
        let new_db = Database::open(&out).unwrap();
        let pivoted = new_db.get_entries(&EntryFilter::default()).unwrap();
        assert_eq!(pivoted.len(), 1);
        assert_eq!(pivoted[0].source, "Hola");
        drop(new_db);

        let src = Database::open(&src_path).unwrap();
        assert_eq!(src.get_entries(&EntryFilter::default()).unwrap().len(), 2);

        let err = cmd_pivot(src_path, out.clone(), false).expect_err("must refuse overwrite");
        assert!(
            err.to_string().to_lowercase().contains("exist")
                || err.to_string().to_lowercase().contains("overwrite"),
            "{err}"
        );
        let _ = fs::remove_dir_all(&dir);
    }
}
