use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

/// Default backup filename appended to the store directory.
const DEFAULT_BACKUP_NAME: &str = "graph.db.backup";

/// Run the backup command: creates a consistent copy of the graph database.
/// If `output` is None, defaults to `graph.db.backup` in the same directory as the database.
pub fn run_backup(store_dir: &Path, output: Option<&Path>) -> Result<PathBuf> {
    let db_path = store_dir.join("graph.db");
    if !db_path.exists() {
        anyhow::bail!(
            "database not found at {}. Has the server been run at least once?",
            db_path.display()
        );
    }

    let dest = match output {
        Some(p) => p.to_path_buf(),
        None => store_dir.join(DEFAULT_BACKUP_NAME),
    };

    let store = codryn_store::Store::open(&db_path).context("failed to open store for backup")?;

    store
        .backup_to(&dest)
        .with_context(|| format!("backup failed to {}", dest.display()))?;

    Ok(dest)
}

/// Run the restore command: replaces the current database with a backup file.
/// Refuses to proceed if the MCP server appears to be running (cannot acquire exclusive lock).
pub fn run_restore(store_dir: &Path, source: Option<&Path>) -> Result<()> {
    let db_path = store_dir.join("graph.db");

    let src = match source {
        Some(p) => p.to_path_buf(),
        None => store_dir.join(DEFAULT_BACKUP_NAME),
    };

    if !src.exists() {
        anyhow::bail!("backup file not found at {}", src.display());
    }

    codryn_store::Store::restore_from(&src, &db_path)
        .with_context(|| format!("restore failed from {}", src.display()))?;

    Ok(())
}
