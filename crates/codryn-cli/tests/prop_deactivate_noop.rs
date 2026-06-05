//! Property 9: Deactivate No-Op on Inactive Workspace
//!
//! **Validates: Requirements 2.7**
//!
//! For any workspace path that is NOT in the activated workspaces list,
//! calling `deactivate` SHALL produce zero filesystem changes and complete
//! without error.

use codryn_cli::activate::deactivate;
use proptest::prelude::*;
use std::collections::BTreeSet;
use std::path::PathBuf;
use tempfile::TempDir;

// ─── Strategies ──────────────────────────────────────────────────────────────

/// Generate a random workspace directory name (alphanumeric, 1-30 chars).
fn workspace_name_strategy() -> impl Strategy<Value = String> {
    "[a-zA-Z][a-zA-Z0-9_-]{0,29}".prop_filter("non-empty workspace name", |s| !s.is_empty())
}

/// Snapshot a directory tree: collect all file paths and their contents.
fn snapshot_directory(root: &std::path::Path) -> BTreeSet<(PathBuf, Vec<u8>)> {
    let mut entries = BTreeSet::new();
    if !root.exists() {
        return entries;
    }
    collect_entries(root, root, &mut entries);
    entries
}

fn collect_entries(
    base: &std::path::Path,
    current: &std::path::Path,
    entries: &mut BTreeSet<(PathBuf, Vec<u8>)>,
) {
    if let Ok(read_dir) = std::fs::read_dir(current) {
        for entry in read_dir.flatten() {
            let path = entry.path();
            let relative = path.strip_prefix(base).unwrap_or(&path).to_path_buf();
            if path.is_file() {
                let content = std::fs::read(&path).unwrap_or_default();
                entries.insert((relative, content));
            } else if path.is_dir() {
                // Record directory existence with empty content marker
                entries.insert((relative.clone(), Vec::new()));
                collect_entries(base, &path, entries);
            }
        }
    }
}

// ─── Property 9: Deactivate No-Op on Inactive Workspace ─────────────────────

/// **Validates: Requirements 2.7**
mod property9_deactivate_noop {
    use super::*;

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(50))]

        #[test]
        fn deactivate_inactive_workspace_produces_no_changes(
            workspace_name in workspace_name_strategy()
        ) {
            let tmp = TempDir::new().expect("failed to create temp dir");
            let workspace = tmp.path().join(&workspace_name);

            // Create the workspace directory (but do NOT activate it)
            std::fs::create_dir_all(&workspace).unwrap();

            // Take a filesystem snapshot before deactivate
            let snapshot_before = snapshot_directory(&workspace);

            // Call deactivate on a workspace that was never activated
            let result = deactivate(&workspace, false);

            // Assert no error
            prop_assert!(
                result.is_ok(),
                "deactivate on inactive workspace should not error: {:?}",
                result.err()
            );

            // Take a filesystem snapshot after deactivate
            let snapshot_after = snapshot_directory(&workspace);

            // Assert snapshots are identical (zero filesystem changes)
            prop_assert_eq!(
                &snapshot_before,
                &snapshot_after,
                "deactivate on inactive workspace should produce zero filesystem changes"
            );
        }
    }
}
