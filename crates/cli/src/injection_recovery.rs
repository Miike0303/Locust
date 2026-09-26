use std::path::Path;

use locust_core::injection_transaction::{self, InjectionRecoveryOptions};

pub(crate) fn status(game_path: &Path) -> anyhow::Result<()> {
    let report = injection_transaction::status(game_path)?;
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}

pub(crate) fn recover(game_path: &Path, force: bool) -> anyhow::Result<()> {
    let report = injection_transaction::recover(
        game_path,
        InjectionRecoveryOptions {
            force,
            ..Default::default()
        },
    )?;
    println!("{}", serde_json::to_string_pretty(&report)?);
    if !force && !report.preserved_conflicts.is_empty() {
        anyhow::bail!("injection recovery refused: modified files were preserved; review inject-status before retrying with --force");
    }
    Ok(())
}
