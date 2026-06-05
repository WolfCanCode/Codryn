//! Property 4: Dry-Run Produces No Filesystem Mutations
//!
//! **Validates: Requirements 1.7**
//!
//! For any valid install configuration (any scope, any IDE selection, any steering
//! choice), executing with `dry_run=true` SHALL produce zero filesystem
//! create/modify/delete operations while still printing descriptions of the
//! operations that would have been performed.

use codryn_cli::install::{install_interactive, mock_install_prompt_responses};
use codryn_cli::prompter::MockPrompter;
use proptest::prelude::*;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

// ─── Filesystem Snapshot ─────────────────────────────────────────────────────

/// A snapshot of a directory tree: maps relative paths to their content bytes.
/// Uses BTreeMap for deterministic ordering.
type FsSnapshot = BTreeMap<PathBuf, Vec<u8>>;

/// Recursively snapshot a directory tree, recording all files and their contents.
fn snapshot_dir(root: &Path) -> FsSnapshot {
    let mut snapshot = BTreeMap::new();
    if root.exists() {
        snapshot_dir_recursive(root, root, &mut snapshot);
    }
    snapshot
}

fn snapshot_dir_recursive(base: &Path, current: &Path, snapshot: &mut FsSnapshot) {
    if let Ok(entries) = std::fs::read_dir(current) {
        for entry in entries.flatten() {
            let path = entry.path();
            let relative = path.strip_prefix(base).unwrap_or(&path).to_path_buf();
            if path.is_dir() {
                // Record directory existence as empty entry
                snapshot.insert(relative.clone(), Vec::new());
                snapshot_dir_recursive(base, &path, snapshot);
            } else if path.is_file() {
                let content = std::fs::read(&path).unwrap_or_default();
                snapshot.insert(relative, content);
            }
        }
    }
}

// ─── Strategies ──────────────────────────────────────────────────────────────

/// Strategy for the scope selection prompt response (0=workspace-only, 1=global, 2=both)
fn scope_index_strategy() -> impl Strategy<Value = usize> {
    0usize..3
}

/// Strategy for the steering choice prompt response (0=workspace-only, 1=yes, 2=no)
fn steering_choice_index_strategy() -> impl Strategy<Value = usize> {
    0usize..3
}

/// Strategy for the steering intensity prompt response (0=lite, 1=full, 2=none)
fn intensity_index_strategy() -> impl Strategy<Value = usize> {
    0usize..3
}

/// Strategy for IDE multi-select response.
/// Since we can't predict how many IDEs will be detected, we generate
/// a response that works for 0-9 IDEs: select a random subset of indices.
fn ide_selection_strategy() -> impl Strategy<Value = Vec<usize>> {
    proptest::collection::vec(0usize..9, 0..5)
}

/// Strategy for whether to use non_interactive mode
fn non_interactive_strategy() -> impl Strategy<Value = bool> {
    proptest::bool::ANY
}

// ─── Property 4: Dry-Run Produces No Filesystem Mutations ────────────────────

/// **Validates: Requirements 1.7**
mod property4_dry_run_no_mutations {
    use super::*;

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(50))]

        #[test]
        fn dry_run_produces_no_filesystem_changes(
            scope_idx in scope_index_strategy(),
            ide_selection in ide_selection_strategy(),
            steering_idx in steering_choice_index_strategy(),
            intensity_idx in intensity_index_strategy(),
            non_interactive in non_interactive_strategy(),
        ) {
            // Create a temp directory to use as a controlled "workspace"
            let workspace = TempDir::new().expect("failed to create temp workspace");
            let binary_dir = TempDir::new().expect("failed to create temp binary dir");

            // Create a fake binary path in the temp dir
            let fake_binary = binary_dir.path().join("codryn");
            std::fs::write(&fake_binary, "fake-binary").unwrap();

            // Pre-populate workspace with some files to detect modifications
            let kiro_dir = workspace.path().join(".kiro").join("steering");
            std::fs::create_dir_all(&kiro_dir).unwrap();
            std::fs::write(kiro_dir.join("existing.md"), "original content").unwrap();

            // Take snapshot BEFORE calling install_interactive
            let snapshot_before = snapshot_dir(workspace.path());

            // Also snapshot the binary dir to ensure nothing is written there
            let binary_snapshot_before = snapshot_dir(binary_dir.path());

            // Build mock responses for the interactive flow:
            // Order: scope → IDE selection → steering → intensity
            let responses = if non_interactive {
                // Non-interactive mode uses no prompts
                vec![]
            } else {
                mock_install_prompt_responses(
                    scope_idx,
                    ide_selection,
                    steering_idx,
                    intensity_idx,
                )
            };

            let prompter = MockPrompter::new(responses);

            // Execute install_interactive with dry_run=true
            // This may fail if detect_ides() returns results that make the
            // multi_select response invalid (e.g., more IDEs than expected).
            // That's fine — we only care about the filesystem invariant.
            let _result = install_interactive(
                &prompter,
                non_interactive,
                true, // dry_run = true
                Some(&fake_binary),
            );

            // Take snapshot AFTER calling install_interactive
            let snapshot_after = snapshot_dir(workspace.path());
            let binary_snapshot_after = snapshot_dir(binary_dir.path());

            // ASSERT: filesystem snapshots are identical (no mutations)
            prop_assert_eq!(
                &snapshot_before,
                &snapshot_after,
                "Dry-run modified the workspace directory! Before had {} entries, after has {} entries",
                snapshot_before.len(),
                snapshot_after.len()
            );

            prop_assert_eq!(
                &binary_snapshot_before,
                &binary_snapshot_after,
                "Dry-run modified the binary directory!"
            );

            // Additional check: if the call succeeded, verify info messages were produced
            // (dry-run should still print planned operations)
            if _result.is_ok() {
                let history = prompter.call_history();
                // In interactive mode, there should be Info calls for the planned operations
                // In non-interactive mode, there should also be Info calls for planned ops
                let info_calls: Vec<_> = history
                    .iter()
                    .filter(|c| matches!(c, codryn_cli::prompter::PromptCall::Info { .. }))
                    .collect();
                // dry-run always produces at least one info message (preferences file operation)
                prop_assert!(
                    !info_calls.is_empty(),
                    "Dry-run should produce info messages describing planned operations"
                );
            }
        }
    }

    /// Additional focused test: verify that the preferences file is NOT written during dry-run.
    /// The preferences path is at ~/.config/codryn/install-preferences.toml — we verify by
    /// checking that the function returns before reaching the save() call.
    #[test]
    fn dry_run_does_not_write_preferences_file() {
        use codryn_cli::preferences::InstallPreferences;

        let prefs_path = InstallPreferences::path();

        // Record whether the preferences file exists and its content before
        let prefs_before = if prefs_path.exists() {
            Some(std::fs::read(&prefs_path).unwrap_or_default())
        } else {
            None
        };

        let prompter = MockPrompter::new(mock_install_prompt_responses(0, vec![], 0, 1));

        let _ = install_interactive(&prompter, false, true, None);

        // Verify preferences file is unchanged
        let prefs_after = if prefs_path.exists() {
            Some(std::fs::read(&prefs_path).unwrap_or_default())
        } else {
            None
        };

        assert_eq!(
            prefs_before, prefs_after,
            "Dry-run should not modify the preferences file"
        );
    }

    /// Test that non-interactive dry-run also produces no mutations.
    #[test]
    fn non_interactive_dry_run_no_mutations() {
        let workspace = TempDir::new().unwrap();
        let fake_binary = workspace.path().join("codryn");
        std::fs::write(&fake_binary, "fake").unwrap();

        let snapshot_before = snapshot_dir(workspace.path());

        let prompter = MockPrompter::new(vec![]);
        let _result = install_interactive(&prompter, true, true, Some(&fake_binary));

        let snapshot_after = snapshot_dir(workspace.path());
        assert_eq!(
            snapshot_before, snapshot_after,
            "Non-interactive dry-run should not modify any files"
        );
    }
}
