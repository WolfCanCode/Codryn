use anyhow::{Context, Result};
use std::path::Path;

/// Run the index-runs command, listing recent index runs for `project`.
///
/// - `limit`: maximum number of runs to show (default 10).
/// - `json`: if true, print machine-readable JSON instead of a human-readable table.
pub fn run_index_runs(store_dir: &Path, project: &str, limit: usize, json: bool) -> Result<()> {
    let db_path = store_dir.join("graph.db");
    if !db_path.exists() {
        anyhow::bail!(
            "database not found at {}. Has the server been run at least once?",
            db_path.display()
        );
    }

    let store =
        codryn_store::Store::open(&db_path).context("failed to open store for index-runs")?;

    let runs = store
        .list_index_runs(project, limit)
        .with_context(|| format!("failed to list index runs for project '{project}'"))?;

    if json {
        print_json(&runs)?;
    } else {
        print_human(&runs, project, limit);
    }

    Ok(())
}

// ── JSON output ───────────────────────────────────────────────────────────────

fn print_json(runs: &[codryn_store::IndexRun]) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(runs)?);
    Ok(())
}

// ── Human-readable output ─────────────────────────────────────────────────────

fn print_human(runs: &[codryn_store::IndexRun], project: &str, limit: usize) {
    println!(
        "Index Runs for project '{}' (showing last {}):\n",
        project, limit
    );

    if runs.is_empty() {
        println!("  No index runs found.");
        return;
    }

    // Column widths
    let id_w = 32;
    let mode_w = 6;
    let status_w = 10;
    let started_w = 24;
    let nodes_w = 6;
    let edges_w = 6;
    let commit_w = 10;

    println!(
        "  {:<id_w$}  {:<mode_w$}  {:<status_w$}  {:<started_w$}  {:>nodes_w$}  {:>edges_w$}  {:<commit_w$}",
        "ID", "Mode", "Status", "Started", "Nodes", "Edges", "Commit",
        id_w = id_w,
        mode_w = mode_w,
        status_w = status_w,
        started_w = started_w,
        nodes_w = nodes_w,
        edges_w = edges_w,
        commit_w = commit_w,
    );

    for run in runs {
        let commit = run
            .git_commit
            .as_deref()
            .map(|c| {
                // Truncate to 7 chars like a short git hash
                if c.len() > 7 {
                    &c[..7]
                } else {
                    c
                }
            })
            .unwrap_or("-");

        println!(
            "  {:<id_w$}  {:<mode_w$}  {:<status_w$}  {:<started_w$}  {:>nodes_w$}  {:>edges_w$}  {:<commit_w$}",
            run.id,
            run.mode,
            run.status.to_string(),
            run.started_at,
            run.node_count,
            run.edge_count,
            commit,
            id_w = id_w,
            mode_w = mode_w,
            status_w = status_w,
            started_w = started_w,
            nodes_w = nodes_w,
            edges_w = edges_w,
            commit_w = commit_w,
        );
    }
}
