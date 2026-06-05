//! Property 10: Steering Mode Switch Replaces Correctly
//!
//! **Validates: Requirements 3.2, 3.3, 3.7**
//!
//! For any valid mode value (Lite, Full, or None) and for any initial steering
//! file content (including empty or non-existent), running `switch_mode` SHALL
//! result in the file containing exactly the corresponding template content —
//! `lite_template()` for Lite, `full_template()` for Full, and file removal for None.

use codryn_cli::preferences::SteeringIntensity;
use codryn_cli::steering::{full_template, lite_template, switch_mode};
use proptest::prelude::*;
use tempfile::TempDir;

// ─── Strategies ──────────────────────────────────────────────────────────────

/// Represents the initial state of the steering file before switch_mode is called.
#[derive(Debug, Clone)]
enum InitialFileState {
    /// File does not exist
    NonExistent,
    /// File exists but is empty
    Empty,
    /// File exists with random content
    RandomContent(String),
}

fn initial_file_state_strategy() -> impl Strategy<Value = InitialFileState> {
    prop_oneof![
        Just(InitialFileState::NonExistent),
        Just(InitialFileState::Empty),
        "\\PC{1,200}".prop_map(InitialFileState::RandomContent),
    ]
}

fn steering_intensity_strategy() -> impl Strategy<Value = SteeringIntensity> {
    prop_oneof![
        Just(SteeringIntensity::Lite),
        Just(SteeringIntensity::Full),
        Just(SteeringIntensity::None),
    ]
}

// ─── Property 10: Steering Mode Switch Replaces Correctly ────────────────────

/// **Validates: Requirements 3.2, 3.3, 3.7**
mod property10_steering_mode_switch {
    use super::*;

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(100))]

        #[test]
        fn switch_mode_produces_correct_content(
            initial_state in initial_file_state_strategy(),
            intensity in steering_intensity_strategy()
        ) {
            let tmp = TempDir::new().expect("failed to create temp dir");
            let path = tmp.path().join("steering").join("codryn.md");

            // Set up the initial file state
            match &initial_state {
                InitialFileState::NonExistent => {
                    // Do nothing — file should not exist
                }
                InitialFileState::Empty => {
                    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
                    std::fs::write(&path, "").unwrap();
                }
                InitialFileState::RandomContent(content) => {
                    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
                    std::fs::write(&path, content).unwrap();
                }
            }

            // Execute switch_mode
            let result = switch_mode(&path, &intensity);
            prop_assert!(result.is_ok(), "switch_mode failed: {:?}", result.err());

            // Assert the correct outcome based on intensity
            match intensity {
                SteeringIntensity::Lite => {
                    prop_assert!(path.exists(), "File should exist after switch to Lite");
                    let content = std::fs::read_to_string(&path).unwrap();
                    prop_assert_eq!(content, lite_template());
                }
                SteeringIntensity::Full => {
                    prop_assert!(path.exists(), "File should exist after switch to Full");
                    let content = std::fs::read_to_string(&path).unwrap();
                    prop_assert_eq!(content, full_template());
                }
                SteeringIntensity::None => {
                    prop_assert!(!path.exists(), "File should not exist after switch to None");
                }
            }
        }
    }
}
