use codryn_cli::preferences::{
    InstallPreferences, InstallScope, SteeringChoice, SteeringIntensity, WorkspaceActivation,
};
use proptest::prelude::*;
use std::path::PathBuf;

// ─── Strategies ──────────────────────────────────────────────────────────────

fn install_scope_strategy() -> impl Strategy<Value = InstallScope> {
    prop_oneof![
        Just(InstallScope::Global),
        Just(InstallScope::WorkspaceOnly),
        Just(InstallScope::Both),
    ]
}

fn steering_intensity_strategy() -> impl Strategy<Value = SteeringIntensity> {
    prop_oneof![
        Just(SteeringIntensity::Full),
        Just(SteeringIntensity::Lite),
        Just(SteeringIntensity::None),
    ]
}

fn steering_choice_strategy() -> impl Strategy<Value = SteeringChoice> {
    prop_oneof![
        Just(SteeringChoice::Yes),
        Just(SteeringChoice::No),
        Just(SteeringChoice::WorkspaceOnly),
    ]
}

/// Generate a valid path-like string (avoids TOML serialization issues with special chars)
fn path_strategy() -> impl Strategy<Value = PathBuf> {
    "[a-z][a-z0-9_/]{1,30}".prop_map(|s| PathBuf::from(format!("/{}", s)))
}

/// Generate a valid ISO 8601-like timestamp string
fn timestamp_strategy() -> impl Strategy<Value = String> {
    (
        2020u32..2030,
        1u32..13,
        1u32..29,
        0u32..24,
        0u32..60,
        0u32..60,
    )
        .prop_map(|(y, mo, d, h, mi, s)| {
            format!("{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z", y, mo, d, h, mi, s)
        })
}

fn workspace_activation_strategy() -> impl Strategy<Value = WorkspaceActivation> {
    (
        path_strategy(),
        timestamp_strategy(),
        steering_intensity_strategy(),
    )
        .prop_map(
            |(path, activated_at, steering_intensity)| WorkspaceActivation {
                path,
                activated_at,
                steering_intensity,
            },
        )
}

fn ide_name_strategy() -> impl Strategy<Value = String> {
    prop_oneof![
        Just("vscode".to_string()),
        Just("cursor".to_string()),
        Just("kiro".to_string()),
        Just("windsurf".to_string()),
        Just("claude-desktop".to_string()),
    ]
}

fn install_preferences_strategy() -> impl Strategy<Value = InstallPreferences> {
    (
        proptest::option::of(install_scope_strategy()),
        proptest::option::of(steering_choice_strategy()),
        proptest::option::of(steering_intensity_strategy()),
        proptest::option::of(steering_intensity_strategy()),
        proptest::option::of(proptest::collection::vec(ide_name_strategy(), 0..5)),
        proptest::option::of(proptest::collection::vec(
            workspace_activation_strategy(),
            0..4,
        )),
    )
        .prop_map(
            |(
                scope,
                steering,
                global_intensity,
                workspace_intensity,
                selected_ides,
                activated_workspaces,
            )| {
                InstallPreferences {
                    scope,
                    steering,
                    global_intensity,
                    workspace_intensity,
                    selected_ides,
                    activated_workspaces,
                }
            },
        )
}

// ─── Property 5: Preferences Round-Trip ──────────────────────────────────────

/// **Validates: Requirements 1.8**
/// Property 5: Preferences Round-Trip
/// For any valid `InstallPreferences` struct, serializing to TOML and
/// deserializing back produces a value equal to the original.
mod property5_preferences_round_trip {
    use super::*;

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(100))]

        #[test]
        fn serialize_deserialize_roundtrip(prefs in install_preferences_strategy()) {
            // Serialize to TOML
            let toml_str = toml::to_string_pretty(&prefs)
                .expect("serialization should not fail for valid InstallPreferences");

            // Deserialize back
            let loaded: InstallPreferences = toml::from_str(&toml_str)
                .expect("deserialization of freshly-serialized TOML should not fail");

            // Assert equality
            prop_assert_eq!(&loaded, &prefs);
        }
    }
}
