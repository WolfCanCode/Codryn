//! Steering file template generation and management.
//!
//! Provides lite (≤15 lines) and full (≤250 lines) steering templates,
//! plus utilities to write, switch, and remove steering files.

use crate::preferences::SteeringIntensity;
use anyhow::{Context, Result};
use std::path::Path;

/// Generate the lite steering template (≤15 lines).
///
/// Contains:
/// - A header identifying it as codryn steering
/// - A tool name listing (key tools)
/// - The analytics metadata instruction
pub fn lite_template() -> &'static str {
    concat!(
        "<!-- codryn:start -->\n",
        "# Codebase Memory (CBM) — Lite Steering\n",
        "\n",
        "## Available Tools\n",
        "find_symbol, get_symbol_details, find_references, impact_analysis,\n",
        "search_graph, get_architecture, trace_call_path, get_code_snippet,\n",
        "query_graph, find_routes, trace_backend_flow, get_file_overview,\n",
        "find_entrypoints, suggest_next_reads, search_linked_projects,\n",
        "explain_index_result, find_dead_code, detect_patterns, what_if\n",
        "\n",
        "## Analytics — MANDATORY\n",
        "Include `\"analytics\": {\"agent_name\": \"kiro\", \"model_name\": \"<model>\",\n",
        "\"input_tokens\": <n>, \"output_tokens\": <n>}` in EVERY tool call.\n",
        "<!-- codryn:end -->\n",
    )
}

/// Generate the full steering template (≤250 lines).
///
/// Contains the complete steering content with tool ordering, usage examples,
/// Cypher query examples, fallback rules, and the analytics instruction.
pub fn full_template() -> &'static str {
    include_str!("steering_full_template.md")
}

/// Write a steering file at the given path with the specified intensity.
///
/// - If `intensity` is `Full`, writes the full template.
/// - If `intensity` is `Lite`, writes the lite template.
/// - If `intensity` is `None`, removes the file if it exists.
/// - Creates parent directories if needed.
/// - Creates the file if it doesn't exist (rather than failing).
pub fn write_steering(path: &Path, intensity: &SteeringIntensity) -> Result<()> {
    match intensity {
        SteeringIntensity::None => {
            if path.exists() {
                std::fs::remove_file(path).with_context(|| {
                    format!("Failed to remove steering file at {}", path.display())
                })?;
            }
            Ok(())
        }
        SteeringIntensity::Lite | SteeringIntensity::Full => {
            let content = match intensity {
                SteeringIntensity::Lite => lite_template(),
                SteeringIntensity::Full => full_template(),
                _ => unreachable!(),
            };
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).with_context(|| {
                    format!("Failed to create parent directories for {}", path.display())
                })?;
            }
            std::fs::write(path, content)
                .with_context(|| format!("Failed to write steering file at {}", path.display()))?;
            Ok(())
        }
    }
}

/// Switch steering mode for an existing installation.
///
/// Replaces the existing steering file content with the template corresponding
/// to the given intensity. If the file doesn't exist, creates it.
/// If intensity is `None`, removes the file.
pub fn switch_mode(path: &Path, intensity: &SteeringIntensity) -> Result<()> {
    write_steering(path, intensity)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_lite_template_within_line_limit() {
        let content = lite_template();
        let line_count = content.lines().count();
        assert!(
            line_count <= 15,
            "lite_template has {} lines, expected ≤15",
            line_count
        );
    }

    #[test]
    fn test_lite_template_contains_required_sections() {
        let content = lite_template();
        assert!(content.contains("codryn"));
        assert!(content.contains("find_symbol"));
        assert!(content.contains("get_symbol_details"));
        assert!(content.contains("search_graph"));
        assert!(content.contains("analytics"));
    }

    #[test]
    fn test_full_template_within_line_limit() {
        let content = full_template();
        let line_count = content.lines().count();
        assert!(
            line_count <= 250,
            "full_template has {} lines, expected ≤250",
            line_count
        );
    }

    #[test]
    fn test_full_template_contains_required_sections() {
        let content = full_template();
        assert!(content.contains("codryn"));
        assert!(content.contains("find_symbol"));
        assert!(content.contains("analytics"));
        assert!(content.contains("get_symbol_details"));
    }

    #[test]
    fn test_write_steering_full_creates_file() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("steering").join("codryn.md");

        write_steering(&path, &SteeringIntensity::Full).unwrap();

        assert!(path.exists());
        let content = std::fs::read_to_string(&path).unwrap();
        assert_eq!(content, full_template());
    }

    #[test]
    fn test_write_steering_lite_creates_file() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("steering").join("codryn.md");

        write_steering(&path, &SteeringIntensity::Lite).unwrap();

        assert!(path.exists());
        let content = std::fs::read_to_string(&path).unwrap();
        assert_eq!(content, lite_template());
    }

    #[test]
    fn test_write_steering_none_removes_file() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("codryn.md");

        // Create the file first
        std::fs::write(&path, "existing content").unwrap();
        assert!(path.exists());

        write_steering(&path, &SteeringIntensity::None).unwrap();

        assert!(!path.exists());
    }

    #[test]
    fn test_write_steering_none_nonexistent_is_noop() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("nonexistent.md");

        // Should not fail even if the file doesn't exist
        write_steering(&path, &SteeringIntensity::None).unwrap();
        assert!(!path.exists());
    }

    #[test]
    fn test_switch_mode_overwrites_existing() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("codryn.md");

        // Write full first
        write_steering(&path, &SteeringIntensity::Full).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), full_template());

        // Switch to lite
        switch_mode(&path, &SteeringIntensity::Lite).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), lite_template());

        // Switch back to full
        switch_mode(&path, &SteeringIntensity::Full).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), full_template());
    }

    #[test]
    fn test_switch_mode_creates_nonexistent_file() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("new_dir").join("codryn.md");

        assert!(!path.exists());
        switch_mode(&path, &SteeringIntensity::Full).unwrap();
        assert!(path.exists());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), full_template());
    }

    #[test]
    fn test_switch_mode_none_removes_file() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("codryn.md");

        std::fs::write(&path, "some content").unwrap();
        switch_mode(&path, &SteeringIntensity::None).unwrap();
        assert!(!path.exists());
    }
}
