//! Property 25: Artifact Discovery Completeness
//!
//! **Validates: Requirements 6.1, 6.7**
//!
//! For any set of installed artifacts at known paths (via preferences file),
//! `discover_artifacts` SHALL find all artifacts that physically exist on disk,
//! grouped into the correct category (steering/skill/mcp_config/data).
//!
//! This test focuses on workspace-based discovery which is fully controllable
//! in tests — we generate random workspaces with steering and skill files,
//! create a preferences struct pointing to them, create the files on disk,
//! then verify `discover_artifacts` finds everything that exists.

use codryn_cli::preferences::{InstallPreferences, SteeringIntensity, WorkspaceActivation};
use codryn_cli::uninstall::{discover_artifacts, ArtifactCategory};
use proptest::prelude::*;
use std::path::PathBuf;
use tempfile::TempDir;

// ─── Strategies ──────────────────────────────────────────────────────────────

/// Generate a workspace name that is valid as a directory name.
fn workspace_name_strategy() -> impl Strategy<Value = String> {
    "[a-z][a-z0-9_-]{2,12}"
}

/// Generate a random steering intensity.
fn steering_intensity_strategy() -> impl Strategy<Value = SteeringIntensity> {
    prop_oneof![
        Just(SteeringIntensity::Full),
        Just(SteeringIntensity::Lite),
        Just(SteeringIntensity::None),
    ]
}

/// Describes which files should physically exist for a workspace.
#[derive(Debug, Clone)]
struct WorkspaceFileSpec {
    name: String,
    intensity: SteeringIntensity,
    has_steering: bool,
    has_skill: bool,
}

/// Generate 1-6 workspace specs with distinct names.
fn workspace_specs_strategy() -> impl Strategy<Value = Vec<WorkspaceFileSpec>> {
    prop::collection::hash_set(workspace_name_strategy(), 1..=6).prop_flat_map(|names| {
        let n = names.len();
        let names_vec: Vec<String> = names.into_iter().collect();
        (
            Just(names_vec),
            prop::collection::vec(steering_intensity_strategy(), n),
            prop::collection::vec(any::<bool>(), n),
            prop::collection::vec(any::<bool>(), n),
        )
            .prop_map(|(names, intensities, steerings, skills)| {
                names
                    .into_iter()
                    .zip(intensities)
                    .zip(steerings)
                    .zip(skills)
                    .map(|(((name, intensity), has_steering), has_skill)| WorkspaceFileSpec {
                        name,
                        intensity,
                        has_steering,
                        has_skill,
                    })
                    .collect()
            })
    })
}

// ─── Helpers ─────────────────────────────────────────────────────────────────

/// The filename used by the uninstall module for steering/skill files.
const STEERING_FILENAME: &str = "codebase-memory.md";

/// Create workspace directories and files on disk according to the spec.
/// Returns the workspace paths and a record of which files were created.
fn setup_workspaces(
    tmp: &TempDir,
    specs: &[WorkspaceFileSpec],
) -> Vec<(PathBuf, bool, bool)> {
    let mut results = Vec::new();

    for spec in specs {
        let ws_path = tmp.path().join(&spec.name);
        std::fs::create_dir_all(&ws_path).unwrap();

        let steering_created = if spec.has_steering {
            let steering_dir = ws_path.join(".kiro").join("steering");
            std::fs::create_dir_all(&steering_dir).unwrap();
            let steering_file = steering_dir.join(STEERING_FILENAME);
            std::fs::write(&steering_file, "# Steering content\n").unwrap();
            true
        } else {
            false
        };

        let skill_created = if spec.has_skill {
            let skill_dir = ws_path.join(".kiro").join("skills");
            std::fs::create_dir_all(&skill_dir).unwrap();
            let skill_file = skill_dir.join(STEERING_FILENAME);
            std::fs::write(&skill_file, "# Skill content\n").unwrap();
            true
        } else {
            false
        };

        results.push((ws_path, steering_created, skill_created));
    }

    results
}

/// Build an InstallPreferences with activated_workspaces pointing to the given paths.
fn build_preferences(
    workspace_paths: &[(PathBuf, bool, bool)],
    specs: &[WorkspaceFileSpec],
) -> InstallPreferences {
    let activations: Vec<WorkspaceActivation> = workspace_paths
        .iter()
        .zip(specs.iter())
        .map(|((path, _, _), spec)| WorkspaceActivation {
            path: path.clone(),
            activated_at: "2024-01-01T00:00:00Z".to_string(),
            steering_intensity: spec.intensity.clone(),
        })
        .collect();

    InstallPreferences {
        activated_workspaces: Some(activations),
        ..Default::default()
    }
}

// ─── Property 25: Artifact Discovery Completeness ────────────────────────────

/// **Validates: Requirements 6.1, 6.7**
mod property25_artifact_discovery_completeness {
    use super::*;

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(40))]

        #[test]
        fn discovers_all_existing_workspace_steering_files(
            specs in workspace_specs_strategy()
        ) {
            let tmp = TempDir::new().expect("failed to create temp dir");

            // Set up workspace directories and files
            let workspace_info = setup_workspaces(&tmp, &specs);

            // Build preferences pointing to these workspaces
            let prefs = build_preferences(&workspace_info, &specs);

            // Call discover_artifacts
            let artifacts = discover_artifacts(&Some(prefs));

            // For each workspace that has a steering file on disk,
            // verify it appears in the results with the correct category
            for (ws_path, has_steering, _) in &workspace_info {
                if *has_steering {
                    let expected_path = ws_path
                        .join(".kiro")
                        .join("steering")
                        .join(STEERING_FILENAME);

                    let found = artifacts.iter().any(|a| {
                        a.path == expected_path
                            && matches!(a.category, ArtifactCategory::SteeringFile)
                    });

                    prop_assert!(
                        found,
                        "Steering file at {:?} exists on disk but was not discovered",
                        expected_path
                    );
                }
            }
        }

        #[test]
        fn discovers_all_existing_workspace_skill_files(
            specs in workspace_specs_strategy()
        ) {
            let tmp = TempDir::new().expect("failed to create temp dir");

            // Set up workspace directories and files
            let workspace_info = setup_workspaces(&tmp, &specs);

            // Build preferences pointing to these workspaces
            let prefs = build_preferences(&workspace_info, &specs);

            // Call discover_artifacts
            let artifacts = discover_artifacts(&Some(prefs));

            // For each workspace that has a skill file on disk,
            // verify it appears in the results with the correct category
            for (ws_path, _, has_skill) in &workspace_info {
                if *has_skill {
                    let expected_path = ws_path
                        .join(".kiro")
                        .join("skills")
                        .join(STEERING_FILENAME);

                    let found = artifacts.iter().any(|a| {
                        a.path == expected_path
                            && matches!(a.category, ArtifactCategory::SkillFile)
                    });

                    prop_assert!(
                        found,
                        "Skill file at {:?} exists on disk but was not discovered",
                        expected_path
                    );
                }
            }
        }

        #[test]
        fn does_not_report_nonexistent_files(
            specs in workspace_specs_strategy()
        ) {
            let tmp = TempDir::new().expect("failed to create temp dir");

            // Set up workspace directories and files
            let workspace_info = setup_workspaces(&tmp, &specs);

            // Build preferences pointing to these workspaces
            let prefs = build_preferences(&workspace_info, &specs);

            // Call discover_artifacts
            let artifacts = discover_artifacts(&Some(prefs));

            // Filter to only workspace-scoped artifacts within our temp dir
            let ws_artifacts: Vec<_> = artifacts
                .iter()
                .filter(|a| a.path.starts_with(tmp.path()))
                .collect();

            // Every reported workspace artifact must physically exist on disk
            for artifact in &ws_artifacts {
                prop_assert!(
                    artifact.path.exists(),
                    "Artifact reported at {:?} does not exist on disk (category: {:?})",
                    artifact.path,
                    artifact.category
                );
            }
        }

        #[test]
        fn categories_match_file_locations(
            specs in workspace_specs_strategy()
        ) {
            let tmp = TempDir::new().expect("failed to create temp dir");

            // Set up workspace directories and files
            let workspace_info = setup_workspaces(&tmp, &specs);

            // Build preferences pointing to these workspaces
            let prefs = build_preferences(&workspace_info, &specs);

            // Call discover_artifacts
            let artifacts = discover_artifacts(&Some(prefs));

            // Filter to only workspace-scoped artifacts within our temp dir
            let ws_artifacts: Vec<_> = artifacts
                .iter()
                .filter(|a| a.path.starts_with(tmp.path()))
                .collect();

            // Verify each artifact's category matches its path structure
            for artifact in &ws_artifacts {
                let path_str = artifact.path.to_string_lossy();

                if path_str.contains("/steering/") || path_str.contains("\\steering\\") {
                    prop_assert_eq!(
                        artifact.category.clone(),
                        ArtifactCategory::SteeringFile,
                        "File in steering/ directory should have SteeringFile category: {:?}",
                        artifact.path
                    );
                } else if path_str.contains("/skills/") || path_str.contains("\\skills\\") {
                    prop_assert_eq!(
                        artifact.category.clone(),
                        ArtifactCategory::SkillFile,
                        "File in skills/ directory should have SkillFile category: {:?}",
                        artifact.path
                    );
                }
            }
        }

        #[test]
        fn discovery_count_matches_existing_files(
            specs in workspace_specs_strategy()
        ) {
            let tmp = TempDir::new().expect("failed to create temp dir");

            // Set up workspace directories and files
            let workspace_info = setup_workspaces(&tmp, &specs);

            // Build preferences pointing to these workspaces
            let prefs = build_preferences(&workspace_info, &specs);

            // Count how many steering and skill files we actually created
            let expected_steering_count = workspace_info
                .iter()
                .filter(|(_, has_steering, _)| *has_steering)
                .count();
            let expected_skill_count = workspace_info
                .iter()
                .filter(|(_, _, has_skill)| *has_skill)
                .count();

            // Call discover_artifacts
            let artifacts = discover_artifacts(&Some(prefs));

            // Count workspace-scoped artifacts within our temp dir
            let found_steering_count = artifacts
                .iter()
                .filter(|a| {
                    a.path.starts_with(tmp.path())
                        && matches!(a.category, ArtifactCategory::SteeringFile)
                })
                .count();
            let found_skill_count = artifacts
                .iter()
                .filter(|a| {
                    a.path.starts_with(tmp.path())
                        && matches!(a.category, ArtifactCategory::SkillFile)
                })
                .count();

            prop_assert_eq!(
                found_steering_count,
                expected_steering_count,
                "Expected {} workspace steering files discovered, got {}",
                expected_steering_count,
                found_steering_count
            );
            prop_assert_eq!(
                found_skill_count,
                expected_skill_count,
                "Expected {} workspace skill files discovered, got {}",
                expected_skill_count,
                found_skill_count
            );
        }
    }
}
