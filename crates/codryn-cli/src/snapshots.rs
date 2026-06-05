use anyhow::{Context, Result};
use std::path::Path;

/// Run the `snapshots` command, listing recent graph summary snapshots for `project`.
///
/// - `limit`: maximum number of snapshots to show (default 10).
/// - `json`: if true, print machine-readable JSON instead of a human-readable table.
pub fn run_snapshots(store_dir: &Path, project: &str, limit: usize, json: bool) -> Result<()> {
    let db_path = store_dir.join("graph.db");
    if !db_path.exists() {
        anyhow::bail!(
            "database not found at {}. Has the server been run at least once?",
            db_path.display()
        );
    }

    let store = codryn_store::Store::open(&db_path).context("failed to open store for snapshots")?;

    let snapshots = store
        .list_snapshots(project, limit)
        .with_context(|| format!("failed to list snapshots for project '{project}'"))?;

    if json {
        println!("{}", serde_json::to_string_pretty(&snapshots)?);
    } else {
        print_snapshots_human(&snapshots, project, limit);
    }

    Ok(())
}

/// Run the `diff` command, comparing two graph summary snapshots.
///
/// - `from_id` / `to_id`: explicit snapshot IDs to compare.
/// - `latest`: if true, automatically pick the two most recent snapshots.
/// - `json`: if true, print machine-readable JSON.
pub fn run_diff(
    store_dir: &Path,
    project: &str,
    from_id: Option<i64>,
    to_id: Option<i64>,
    latest: bool,
    json: bool,
) -> Result<()> {
    let db_path = store_dir.join("graph.db");
    if !db_path.exists() {
        anyhow::bail!(
            "database not found at {}. Has the server been run at least once?",
            db_path.display()
        );
    }

    let store = codryn_store::Store::open(&db_path).context("failed to open store for diff")?;

    let (resolved_from, resolved_to) = if latest {
        // Get the 2 most recent snapshots; from = older, to = newer
        let recent = store
            .list_snapshots(project, 2)
            .with_context(|| format!("failed to list snapshots for project '{project}'"))?;
        if recent.len() < 2 {
            anyhow::bail!(
                "project '{}' has fewer than 2 snapshots; cannot diff with --latest",
                project
            );
        }
        // list_snapshots returns most-recent first, so recent[0] is newer, recent[1] is older
        (recent[1].id, recent[0].id)
    } else {
        match (from_id, to_id) {
            (Some(f), Some(t)) => (f, t),
            _ => anyhow::bail!("either --latest or both --from <id> and --to <id> are required"),
        }
    };

    let diff = store
        .diff_snapshots(resolved_from, resolved_to)
        .with_context(|| {
            format!(
                "failed to diff snapshots {} → {}",
                resolved_from, resolved_to
            )
        })?;

    if json {
        println!("{}", serde_json::to_string_pretty(&diff)?);
    } else {
        print_diff_human(&diff);
    }

    Ok(())
}

// ── Human-readable output ─────────────────────────────────────────────────────

fn print_snapshots_human(
    snapshots: &[codryn_store::GraphSummarySnapshot],
    project: &str,
    limit: usize,
) {
    println!(
        "Snapshots for project '{}' (showing last {}):\n",
        project, limit
    );

    if snapshots.is_empty() {
        println!("  No snapshots found.");
        return;
    }

    let id_w = 6;
    let ts_w = 26;
    let nodes_w = 7;
    let edges_w = 7;
    let hash_w = 18;
    let run_w = 36;

    println!(
        "  {:<id_w$}  {:<ts_w$}  {:>nodes_w$}  {:>edges_w$}  {:<hash_w$}  {:<run_w$}",
        "ID",
        "Timestamp",
        "Nodes",
        "Edges",
        "ContentHash",
        "IndexRunId",
        id_w = id_w,
        ts_w = ts_w,
        nodes_w = nodes_w,
        edges_w = edges_w,
        hash_w = hash_w,
        run_w = run_w,
    );
    println!(
        "  {}",
        "─".repeat(id_w + 2 + ts_w + 2 + nodes_w + 2 + edges_w + 2 + hash_w + 2 + run_w)
    );

    for snap in snapshots {
        // Truncate content hash to 16 chars for display
        let hash_display = if snap.content_hash.len() > 16 {
            &snap.content_hash[..16]
        } else {
            &snap.content_hash
        };

        let run_id_display = snap.index_run_id.as_deref().unwrap_or("-");

        println!(
            "  {:<id_w$}  {:<ts_w$}  {:>nodes_w$}  {:>edges_w$}  {:<hash_w$}  {:<run_w$}",
            snap.id,
            snap.timestamp,
            snap.total_nodes,
            snap.total_edges,
            hash_display,
            run_id_display,
            id_w = id_w,
            ts_w = ts_w,
            nodes_w = nodes_w,
            edges_w = edges_w,
            hash_w = hash_w,
            run_w = run_w,
        );
    }
}

fn print_diff_human(diff: &codryn_store::GraphDiff) {
    println!(
        "Diff: snapshot {} → snapshot {}\n",
        diff.from_snapshot_id, diff.to_snapshot_id
    );

    let sign = |n: i64| {
        if n >= 0 {
            format!("+{}", n)
        } else {
            n.to_string()
        }
    };

    println!("  Nodes: {}", sign(diff.node_delta));
    println!("  Edges: {}", sign(diff.edge_delta));

    if !diff.label_changes.is_empty() {
        println!("\n  Label changes:");
        let mut labels: Vec<(&String, &i64)> = diff.label_changes.iter().collect();
        labels.sort_by_key(|(k, _)| k.as_str());
        for (label, delta) in labels {
            println!("    {:<30}  {}", label, sign(*delta));
        }
    }

    if !diff.edge_type_changes.is_empty() {
        println!("\n  Edge type changes:");
        let mut edge_types: Vec<(&String, &i64)> = diff.edge_type_changes.iter().collect();
        edge_types.sort_by_key(|(k, _)| k.as_str());
        for (edge_type, delta) in edge_types {
            println!("    {:<30}  {}", edge_type, sign(*delta));
        }
    }

    if diff.label_changes.is_empty() && diff.edge_type_changes.is_empty() {
        println!("\n  No label or edge type changes.");
    }
}
