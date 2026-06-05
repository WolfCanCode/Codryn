//! Property 8: Activate Idempotence
//!
//! **Validates: Requirements 2.6**
//!
//! For any workspace path, calling `activate` twice with the same parameters
//! SHALL produce the same final state as calling it once — the second call
//! succeeds without error and the resulting files and preferences entry are
//! identical.

use codryn_cli::activate::activate;
use codryn_cli::preferences::SteeringIntensity;
use proptest::prelude::*;
use tempfile::TempDir;

// ─── Strategies ──────────────────────────────────────────────────────────────

/// Generate a random workspace directory name (simple alphanumeric).
fn workspace_name_strategy() -> impl Strategy<Value = String> {
    "[a-z][a-z0-9_]{2,15}".prop_map(|s| s)
}

/// Generate a random steering intensity (Lite or Full — the two valid options
/// for activate; None doesn't make sense for activation).
fn intensity_strategy() -> impl Strategy<Value = SteeringIntensity> {
    prop_oneof![
        Just(SteeringIntensity::Lite),
        Just(SteeringIntensity::Full),
    ]
}

// ─── Property 8: Activate Idempotence ────────────────────────────────────────

/// **Validates: Requirements 2.6**
mod property8_activate_idempotence {
    use super::*;

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(50))]

        #[test]
        fn activate_twice_same_as_once(
            workspace_name in workspace_name_strategy(),
            intensity in intensity_strategy()
        ) {
            let tmp = TempDir::new().expect("failed to create temp dir");
            let workspace = tmp.path().join(&workspace_name);
            std::fs::create_dir_all(&workspace).expect("failed to create workspace dir");

            let steering_file = workspace
                .join(".kiro")
                .join("steering")
                .join("codebase-memory.md");

            // First activation
            let result1 = activate(&workspace, false, &intensity);
            prop_assert!(result1.is_ok(), "First activate failed: {:?}", result1.err());

            // Read state after first activation
            prop_assert!(steering_file.exists(), "Steering file should exist after first activate");
            let content_after_first = std::fs::read_to_string(&steering_file)
                .expect("failed to read steering file after first activate");

            // Second activation (same parameters)
            let result2 = activate(&workspace, false, &intensity);
            prop_assert!(result2.is_ok(), "Second activate failed: {:?}", result2.err());

            // Read state after second activation
            prop_assert!(steering_file.exists(), "Steering file should exist after second activate");
            let content_after_second = std::fs::read_to_string(&steering_file)
                .expect("failed to read steering file after second activate");

            // Assert both states are identical (same file content)
            prop_assert_eq!(
                content_after_first,
                content_after_second,
                "Steering file content should be identical after first and second activation"
            );
        }
    }
}
