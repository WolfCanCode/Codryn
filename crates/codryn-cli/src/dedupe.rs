use anyhow::{Context, Result};
use std::path::Path;

/// Run the dedupe command against the graph for `project`.
///
/// - `apply`: if true, actually perform the merge; otherwise dry-run (default).
/// - `json`: if true, print machine-readable JSON instead of human-readable text.
pub fn run_dedupe(store_dir: &Path, project: &str, apply: bool, json: bool) -> Result<()> {
    let db_path = store_dir.join("graph.db");
    if !db_path.exists() {
        anyhow::bail!(
            "database not found at {}. Has the server been run at least once?",
            db_path.display()
        );
    }

    let store =
        codryn_store::Store::open(&db_path).context("failed to open store for deduplication")?;

    if apply {
        let removed = store
            .deduplicate_apply(project)
            .with_context(|| format!("deduplication failed for project '{project}'"))?;

        if json {
            println!("{}", serde_json::json!({ "removed": removed }));
        } else {
            println!("Removed {removed} duplicate node(s)");
        }
    } else {
        // Dry-run mode (default)
        let report = store
            .deduplicate_dry_run(project)
            .with_context(|| format!("deduplication dry-run failed for project '{project}'"))?;

        if json {
            print_json(&report)?;
        } else {
            print_human(&report, project);
        }
    }

    Ok(())
}

// ── JSON output ───────────────────────────────────────────────────────────────

fn print_json(report: &codryn_store::DedupeReport) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(report)?);
    Ok(())
}

// ── Human-readable output ─────────────────────────────────────────────────────

fn print_human(report: &codryn_store::DedupeReport, project: &str) {
    println!("DRY RUN - no changes made");
    println!();

    if report.groups.is_empty() {
        println!("✓ No duplicates found (project: {project})");
        return;
    }

    println!(
        "Found {} duplicate group(s) with {} total duplicate node(s) in project '{project}'",
        report.groups.len(),
        report.total_duplicates
    );
    println!();

    for group in &report.groups {
        let dup_ids: Vec<String> = group
            .duplicate_ids
            .iter()
            .map(|id| id.to_string())
            .collect();
        println!("  Qualified name: {}", group.qualified_name);
        println!("  Canonical ID:   {}", group.canonical_id);
        println!("  Duplicate IDs:  [{}]", dup_ids.join(", "));
        println!("  Reason:         {}", group.reason);
        println!();
    }

    println!(
        "Total: {} duplicate(s) would be removed",
        report.total_duplicates
    );
    println!();
    println!("Run with --apply to perform the merge.");
}
