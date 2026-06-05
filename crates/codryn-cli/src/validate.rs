use anyhow::{Context, Result};
use std::path::Path;

/// Run the validate command against the graph for `project`.
///
/// - `fix_safe`: if true, also call `Store::fix_safe()` and report the number of fixes applied.
/// - `json`: if true, print machine-readable JSON instead of a human-readable table.
pub fn run_validate(store_dir: &Path, project: &str, fix_safe: bool, json: bool) -> Result<()> {
    let db_path = store_dir.join("graph.db");
    if !db_path.exists() {
        anyhow::bail!(
            "database not found at {}. Has the server been run at least once?",
            db_path.display()
        );
    }

    let store = codryn_store::Store::open(&db_path).context("failed to open store for validation")?;

    let report = store
        .validate_graph(project)
        .with_context(|| format!("validation failed for project '{project}'"))?;

    let fixes_applied: Option<usize> = if fix_safe {
        let n = store
            .fix_safe(project)
            .with_context(|| format!("fix_safe failed for project '{project}'"))?;
        Some(n)
    } else {
        None
    };

    if json {
        print_json(&report, fixes_applied)?;
    } else {
        print_human(&report, project, fixes_applied);
    }

    Ok(())
}

// ── JSON output ───────────────────────────────────────────────────────────────

fn print_json(report: &codryn_store::ValidationReport, fixes_applied: Option<usize>) -> Result<()> {
    #[derive(serde::Serialize)]
    struct Output<'a> {
        #[serde(flatten)]
        report: &'a codryn_store::ValidationReport,
        #[serde(skip_serializing_if = "Option::is_none")]
        fixes_applied: Option<usize>,
    }

    let out = Output {
        report,
        fixes_applied,
    };
    println!("{}", serde_json::to_string_pretty(&out)?);
    Ok(())
}

// ── Human-readable output ─────────────────────────────────────────────────────

fn print_human(report: &codryn_store::ValidationReport, project: &str, fixes_applied: Option<usize>) {
    if report.total_issues == 0 {
        println!("✓ Graph is valid (project: {project})");
    } else {
        println!(
            "Found {} issue(s) in project '{project}'",
            report.total_issues
        );
        println!();

        print_edge_group(
            "Dangling Edges",
            &report.dangling_edges,
            "edge IDs referencing non-existent nodes",
        );

        print_node_group(
            "Orphan Nodes",
            &report.orphan_nodes,
            "node IDs with no edges",
        );

        print_dup_group(&report.duplicate_qns);

        print_missing_props_group(&report.missing_properties);

        print_node_group(
            "Invalid Properties JSON",
            &report.invalid_properties_json,
            "node IDs with malformed properties JSON",
        );

        print_edge_group(
            "Self-loops",
            &report.self_loops,
            "edge IDs where source == target",
        );

        print_edge_group(
            "Cross-project Edges",
            &report.cross_project_edges,
            "edge IDs spanning multiple projects",
        );
    }

    if let Some(n) = fixes_applied {
        println!();
        println!("Applied {n} fix(es)");
    }
}

/// Print a group of edge IDs with a label and description.
fn print_edge_group(label: &str, ids: &[i64], description: &str) {
    if ids.is_empty() {
        return;
    }
    println!("  {label}: {} ({description})", ids.len());
    print_examples(ids);
}

/// Print a group of node IDs with a label and description.
fn print_node_group(label: &str, ids: &[i64], description: &str) {
    if ids.is_empty() {
        return;
    }
    println!("  {label}: {} ({description})", ids.len());
    print_examples(ids);
}

/// Print duplicate qualified name groups.
fn print_dup_group(dups: &[(String, Vec<i64>)]) {
    if dups.is_empty() {
        return;
    }
    // Count total extra copies (each group contributes len-1 duplicates)
    let total: usize = dups
        .iter()
        .map(|(_, ids)| ids.len().saturating_sub(1))
        .sum();
    println!(
        "  Duplicate Qualified Names: {total} extra copies across {} group(s)",
        dups.len()
    );
    for (qn, ids) in dups.iter().take(3) {
        let example_ids: Vec<String> = ids.iter().take(5).map(|id| id.to_string()).collect();
        println!("    '{qn}' → [{}]", example_ids.join(", "));
    }
    if dups.len() > 3 {
        println!("    … and {} more group(s)", dups.len() - 3);
    }
}

/// Print missing-property issues grouped by field name.
fn print_missing_props_group(issues: &[(i64, String)]) {
    if issues.is_empty() {
        return;
    }
    println!("  Missing Properties: {} issue(s)", issues.len());
    // Group by field name for readability
    let mut by_field: std::collections::HashMap<&str, Vec<i64>> = std::collections::HashMap::new();
    for (id, field) in issues {
        by_field.entry(field.as_str()).or_default().push(*id);
    }
    let mut fields: Vec<&str> = by_field.keys().copied().collect();
    fields.sort_unstable();
    for field in fields {
        let ids = &by_field[field];
        let examples: Vec<String> = ids.iter().take(5).map(|id| id.to_string()).collect();
        println!(
            "    field '{}': {} node(s) — e.g. [{}]",
            field,
            ids.len(),
            examples.join(", ")
        );
    }
}

/// Print up to 5 example IDs from a slice.
fn print_examples(ids: &[i64]) {
    let examples: Vec<String> = ids.iter().take(5).map(|id| id.to_string()).collect();
    if ids.len() > 5 {
        println!(
            "    e.g. [{}] … and {} more",
            examples.join(", "),
            ids.len() - 5
        );
    } else {
        println!("    e.g. [{}]", examples.join(", "));
    }
}
