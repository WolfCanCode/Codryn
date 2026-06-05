//! Property 6: Activate/Deactivate Inverse Pair
//!
//! **Validates: Requirements 2.2, 2.3**
//!
//! For any valid workspace path, calling `activate` followed by `deactivate`
//! SHALL return the filesystem to a state equivalent to the state before
//! activation — the steering file is removed.

use codryn_cli::activate::{activate, deactivate};
use codryn_cli::preferences::SteeringIntensity;
use proptest::prelude::*;
use tempfile::TempDir;

// ─── Strategies ──────────────────────────────────────────────────────────────

/// Generate a valid workspace directory name (alphanumeric + hyphens/underscores).
fn workspace_name_strategy() -> impl Strategy<Value = String> {
    "[a-z][a-z0-9_-]{2,20}".prop_map(|s| s)
}

/// Generate a random steering intensity (Lite or Full — the valid activation modes).
fn activation_intensity_strategy() -> impl Strategy<Value = SteeringIntensity> {
    prop_oneof![
        Just(SteeringIntensity::Lite),
        Just(SteeringIntensity::Full),
    ]
}

// ─── Property 6: Activate/Deactivate Inverse Pair ────────────────────────────

/// **Validates: Requirements 2.2, 2.3**
mod property6_activate_deactivate_inverse {
    use super::*;

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(50))]

        #[test]
        fn activate_then_deactivate_removes_steering_file(
            workspace_name in workspace_name_strategy(),
            intensity in activation_intensity_strategy()
        ) {
            let tmp = TempDir::new().expect("failed to create temp dir");
            let workspace = tmp.path().join(&workspace_name);
            std::fs::create_dir_all(&workspace).expect("failed to create workspace dir");

            let steering_file = workspace
                .join(".kiro")
                .join("steering")
                .join("codebase-memory.md");

            // Pre-activation: steering file should NOT exist
            prop_assert!(!steering_file.exists(), "steering file should not exist before activation");

            // Activate
            let activate_result = activate(&workspace, false, &intensity);
            prop_assert!(activate_result.is_ok(), "activate failed: {:?}", activate_result.err());

            // Post-activation: steering file SHOULD exist
            prop_assert!(steering_file.exists(), "steering file should exist after activation");

            // Deactivate
            let deactivate_result = deactivate(&workspace, false);
            prop_assert!(deactivate_result.is_ok(), "deactivate failed: {:?}", deactivate_result.err());

            // Post-deactivation: steering file should NOT exist (returned to pre-activation state)
            prop_assert!(
                !steering_file.exists(),
                "steering file should not exist after deactivation (filesystem not restored)"
            );
        }

        #[test]
        fn activate_then_deactivate_workspace_dir_equivalent_to_pre_activation(
            workspace_name in workspace_name_strategy(),
            intensity in activation_intensity_strategy()
        ) {
            let tmp = TempDir::new().expect("failed to create temp dir");
            let workspace = tmp.path().join(&workspace_name);
            std::fs::create_dir_all(&workspace).expect("failed to create workspace dir");

            // Take a filesystem snapshot before activation:
            // list all files in the workspace (should be empty)
            let pre_files = collect_files(&workspace);

            // Activate
            activate(&workspace, false, &intensity).expect("activate failed");

            // Deactivate
            deactivate(&workspace, false).expect("deactivate failed");

            // Take a filesystem snapshot after deactivation
            let post_files = collect_files(&workspace);

            // The .kiro/steering/ directory may still exist (empty), but the
            // steering file itself must be gone. Filter out empty directories
            // to compare meaningful file content.
            let pre_content_files: Vec<_> = pre_files.iter().filter(|p| p.is_file()).collect();
            let post_content_files: Vec<_> = post_files.iter().filter(|p| p.is_file()).collect();

            prop_assert_eq!(
                pre_content_files,
                post_content_files,
                "filesystem should be equivalent to pre-activation state (no leftover files)"
            );
        }
    }

    /// Collect all paths (files and dirs) within a directory recursively.
    fn collect_files(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
        let mut paths = Vec::new();
        if dir.exists() {
            collect_recursive(dir, &mut paths);
        }
        paths.sort();
        paths
    }

    fn collect_recursive(dir: &std::path::Path, paths: &mut Vec<std::path::PathBuf>) {
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                paths.push(path.clone());
                if path.is_dir() {
                    collect_recursive(&path, paths);
                }
            }
        }
    }
}
