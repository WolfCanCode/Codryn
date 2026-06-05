//! **Validates: Requirements 1.6**
//!
//! Property 3: Preferences Fallback to Defaults
//!
//! For any partial preferences file (containing a random subset of configuration keys),
//! non-interactive mode SHALL use the stored value for each present key and the documented
//! default for each absent key: scope=workspace-only, steering=workspace-only,
//! intensity=lite (global) or full (workspace).

use codryn_cli::preferences::{InstallPreferences, InstallScope, SteeringIntensity};
use proptest::prelude::*;

// ── Strategies ────────────────────────────────────────────────────────────────

fn arb_install_scope() -> impl Strategy<Value = InstallScope> {
    prop_oneof![
        Just(InstallScope::Global),
        Just(InstallScope::WorkspaceOnly),
        Just(InstallScope::Both),
    ]
}

fn arb_steering_intensity() -> impl Strategy<Value = SteeringIntensity> {
    prop_oneof![
        Just(SteeringIntensity::Full),
        Just(SteeringIntensity::Lite),
        Just(SteeringIntensity::None),
    ]
}

/// Generate a partial TOML string with a random subset of preference keys present.
/// Returns (toml_string, included_scope, included_global_intensity, included_workspace_intensity)
fn arb_partial_toml() -> impl Strategy<
    Value = (
        String,
        Option<InstallScope>,
        Option<SteeringIntensity>,
        Option<SteeringIntensity>,
    ),
> {
    (
        proptest::option::of(arb_install_scope()),
        proptest::option::of(arb_steering_intensity()),
        proptest::option::of(arb_steering_intensity()),
    )
        .prop_map(|(scope, global_intensity, workspace_intensity)| {
            let mut toml_parts = Vec::new();

            if let Some(ref s) = scope {
                let scope_str = match s {
                    InstallScope::Global => "global",
                    InstallScope::WorkspaceOnly => "workspace-only",
                    InstallScope::Both => "both",
                };
                toml_parts.push(format!("scope = \"{}\"", scope_str));
            }

            if let Some(ref gi) = global_intensity {
                let gi_str = match gi {
                    SteeringIntensity::Full => "full",
                    SteeringIntensity::Lite => "lite",
                    SteeringIntensity::None => "none",
                };
                toml_parts.push(format!("global-intensity = \"{}\"", gi_str));
            }

            if let Some(ref wi) = workspace_intensity {
                let wi_str = match wi {
                    SteeringIntensity::Full => "full",
                    SteeringIntensity::Lite => "lite",
                    SteeringIntensity::None => "none",
                };
                toml_parts.push(format!("workspace-intensity = \"{}\"", wi_str));
            }

            let toml_string = toml_parts.join("\n");
            (toml_string, scope, global_intensity, workspace_intensity)
        })
}

// ── Property Tests ────────────────────────────────────────────────────────────

proptest! {
    #![proptest_config(ProptestConfig::with_cases(200))]

    /// Property 3: For any partial TOML file, effective_scope() returns the stored
    /// value if scope is present, or WorkspaceOnly (the documented default) if absent.
    #[test]
    fn effective_scope_uses_stored_or_default(
        (toml_string, scope, _global_intensity, _workspace_intensity) in arb_partial_toml()
    ) {
        let prefs: InstallPreferences = toml::from_str(&toml_string)
            .expect("Valid partial TOML should always parse");

        let effective = prefs.effective_scope();

        match scope {
            Some(expected_scope) => {
                // Stored value is used when present
                prop_assert_eq!(effective, expected_scope);
            }
            None => {
                // Default: WorkspaceOnly
                prop_assert_eq!(effective, InstallScope::WorkspaceOnly);
            }
        }
    }

    /// Property 3: For any partial TOML file, effective_intensity(Global) returns
    /// the stored global_intensity if present, or Lite (the documented default) if absent.
    #[test]
    fn effective_intensity_global_uses_stored_or_default(
        (toml_string, _scope, global_intensity, _workspace_intensity) in arb_partial_toml()
    ) {
        let prefs: InstallPreferences = toml::from_str(&toml_string)
            .expect("Valid partial TOML should always parse");

        let effective = prefs.effective_intensity(&InstallScope::Global);

        match global_intensity {
            Some(expected) => {
                // Stored value is used when present
                prop_assert_eq!(effective, expected);
            }
            None => {
                // Default for Global: Lite
                prop_assert_eq!(effective, SteeringIntensity::Lite);
            }
        }
    }

    /// Property 3: For any partial TOML file, effective_intensity(WorkspaceOnly) returns
    /// the stored workspace_intensity if present, or Full (the documented default) if absent.
    #[test]
    fn effective_intensity_workspace_uses_stored_or_default(
        (toml_string, _scope, _global_intensity, workspace_intensity) in arb_partial_toml()
    ) {
        let prefs: InstallPreferences = toml::from_str(&toml_string)
            .expect("Valid partial TOML should always parse");

        let effective = prefs.effective_intensity(&InstallScope::WorkspaceOnly);

        match workspace_intensity {
            Some(expected) => {
                // Stored value is used when present
                prop_assert_eq!(effective, expected);
            }
            None => {
                // Default for WorkspaceOnly: Full
                prop_assert_eq!(effective, SteeringIntensity::Full);
            }
        }
    }

    /// Property 3: For any partial TOML file, effective_intensity(Both) returns
    /// the stored workspace_intensity if present, or Full (the documented default) if absent.
    /// (Both uses the same logic as WorkspaceOnly per the design)
    #[test]
    fn effective_intensity_both_uses_stored_or_default(
        (toml_string, _scope, _global_intensity, workspace_intensity) in arb_partial_toml()
    ) {
        let prefs: InstallPreferences = toml::from_str(&toml_string)
            .expect("Valid partial TOML should always parse");

        let effective = prefs.effective_intensity(&InstallScope::Both);

        match workspace_intensity {
            Some(expected) => {
                // Stored value is used when present
                prop_assert_eq!(effective, expected);
            }
            None => {
                // Default for Both: Full (same as WorkspaceOnly)
                prop_assert_eq!(effective, SteeringIntensity::Full);
            }
        }
    }
}
