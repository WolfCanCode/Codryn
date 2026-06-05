use anyhow::{Context, Result};
use std::path::Path;

/// Run a raw Cypher query against the store.
///
/// - `json`: machine-readable JSON output (raw result)
/// - human mode: pretty-printed JSON
pub fn run_query(store_dir: &Path, project: &str, cypher: &str, json: bool) -> Result<()> {
    let db_path = store_dir.join("graph.db");
    if !db_path.exists() {
        anyhow::bail!(
            "database not found at {}. Has the server been run at least once?",
            db_path.display()
        );
    }

    let store = codryn_store::Store::open(&db_path).context("failed to open store for query")?;

    let result = codryn_cypher::execute(&store, project, cypher)
        .with_context(|| format!("cypher query failed for project '{project}'"))?;

    if json {
        println!("{}", result);
    } else {
        println!("{}", serde_json::to_string_pretty(&result)?);
    }

    Ok(())
}

/// Find a symbol by name and display its details.
pub fn run_symbol(store_dir: &Path, project: &str, name: &str, json: bool) -> Result<()> {
    let db_path = store_dir.join("graph.db");
    if !db_path.exists() {
        anyhow::bail!(
            "database not found at {}. Has the server been run at least once?",
            db_path.display()
        );
    }

    let store =
        codryn_store::Store::open(&db_path).context("failed to open store for symbol lookup")?;

    let results = store
        .find_symbol_ranked(project, name, None, false, 10)
        .with_context(|| format!("symbol lookup failed for project '{project}'"))?;

    if json {
        let rows: Vec<serde_json::Value> = results
            .iter()
            .map(|(node, match_type, score)| {
                serde_json::json!({
                    "name": node.name,
                    "qualified_name": node.qualified_name,
                    "label": node.label,
                    "file_path": node.file_path,
                    "start_line": node.start_line,
                    "end_line": node.end_line,
                    "match_type": match_type,
                    "score": score,
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&rows)?);
    } else {
        if results.is_empty() {
            println!("No symbols found matching '{name}' in project '{project}'.");
            return Ok(());
        }
        println!("Symbols matching '{}' in project '{}':\n", name, project);
        for (node, match_type, score) in &results {
            println!("  Name:           {}", node.name);
            println!("  Qualified Name: {}", node.qualified_name);
            println!("  Label:          {}", node.label);
            println!("  File:           {}:{}", node.file_path, node.start_line);
            println!("  End Line:       {}", node.end_line);
            println!("  Match Type:     {}", match_type);
            println!("  Score:          {:.2}", score);
            println!();
        }
    }

    Ok(())
}

/// Find incoming references to a symbol by qualified name or name.
pub fn run_refs(
    store_dir: &Path,
    project: &str,
    qn: &str,
    min_confidence: Option<f64>,
    json: bool,
) -> Result<()> {
    let db_path = store_dir.join("graph.db");
    if !db_path.exists() {
        anyhow::bail!(
            "database not found at {}. Has the server been run at least once?",
            db_path.display()
        );
    }

    let store =
        codryn_store::Store::open(&db_path).context("failed to open store for refs lookup")?;

    // Resolve the symbol by QN or name
    let node_id = resolve_symbol(&store, project, qn)?;

    let refs = store
        .incoming_references_detailed(node_id, None, 30, min_confidence)
        .with_context(|| format!("refs query failed for '{qn}' in project '{project}'"))?;

    if json {
        let rows: Vec<serde_json::Value> = refs
            .iter()
            .map(|(node, edge_type, confidence, edge_source)| {
                serde_json::json!({
                    "source_name": node.name,
                    "source_qualified_name": node.qualified_name,
                    "label": node.label,
                    "file_path": node.file_path,
                    "line": node.start_line,
                    "edge_type": edge_type,
                    "confidence": confidence,
                    "edge_source": edge_source,
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&rows)?);
    } else {
        if refs.is_empty() {
            println!("No references found for '{qn}' in project '{project}'.");
            return Ok(());
        }
        println!(
            "References to '{}' in project '{}' ({} found):\n",
            qn,
            project,
            refs.len()
        );

        // Column widths
        let name_w = refs
            .iter()
            .map(|(n, _, _, _)| n.name.len())
            .max()
            .unwrap_or(4)
            .max(4);
        let file_w = refs
            .iter()
            .map(|(n, _, _, _)| format!("{}:{}", n.file_path, n.start_line).len())
            .max()
            .unwrap_or(4)
            .max(4);
        let edge_w = refs
            .iter()
            .map(|(_, et, _, _)| et.len())
            .max()
            .unwrap_or(4)
            .max(9);

        println!(
            "  {:<name_w$}  {:<file_w$}  {:<edge_w$}  {:>10}  Source",
            "Name",
            "File",
            "Edge Type",
            "Confidence",
            name_w = name_w,
            file_w = file_w,
            edge_w = edge_w,
        );
        println!(
            "  {:<name_w$}  {:<file_w$}  {:<edge_w$}  {:>10}  ------",
            "-".repeat(name_w),
            "-".repeat(file_w),
            "-".repeat(edge_w),
            "----------",
            name_w = name_w,
            file_w = file_w,
            edge_w = edge_w,
        );

        for (node, edge_type, confidence, edge_source) in &refs {
            let loc = format!("{}:{}", node.file_path, node.start_line);
            let conf_str = confidence
                .map(|c| format!("{:.2}", c))
                .unwrap_or_else(|| "-".to_string());
            let src_str = edge_source.as_deref().unwrap_or("-");
            println!(
                "  {:<name_w$}  {:<file_w$}  {:<edge_w$}  {:>10}  {}",
                node.name,
                loc,
                edge_type,
                conf_str,
                src_str,
                name_w = name_w,
                file_w = file_w,
                edge_w = edge_w,
            );
        }
    }

    Ok(())
}

/// Run impact analysis (BFS) for a symbol by qualified name or name.
pub fn run_impact(
    store_dir: &Path,
    project: &str,
    qn: &str,
    depth: Option<i32>,
    min_confidence: Option<f64>,
    json: bool,
) -> Result<()> {
    let db_path = store_dir.join("graph.db");
    if !db_path.exists() {
        anyhow::bail!(
            "database not found at {}. Has the server been run at least once?",
            db_path.display()
        );
    }

    let store =
        codryn_store::Store::open(&db_path).context("failed to open store for impact analysis")?;

    // Resolve the symbol by QN or name
    let node_id = resolve_symbol(&store, project, qn)?;

    let max_depth = depth.unwrap_or(3);
    let (direct, all, files) = store
        .impact_bfs_with_confidence(node_id, max_depth, 50, min_confidence)
        .with_context(|| format!("impact analysis failed for '{qn}' in project '{project}'"))?;

    // Compute indirect dependents (all minus direct)
    let direct_ids: std::collections::HashSet<i64> = direct.iter().map(|n| n.id).collect();
    let indirect: Vec<_> = all
        .iter()
        .filter(|(n, _)| !direct_ids.contains(&n.id))
        .collect();

    // Risk level heuristic
    let risk_level = match all.len() {
        0 => "none",
        1..=5 => "low",
        6..=20 => "medium",
        _ => "high",
    };

    if json {
        let direct_json: Vec<serde_json::Value> = direct
            .iter()
            .map(|n| {
                serde_json::json!({
                    "name": n.name,
                    "qualified_name": n.qualified_name,
                    "label": n.label,
                    "file_path": n.file_path,
                    "start_line": n.start_line,
                })
            })
            .collect();
        let indirect_json: Vec<serde_json::Value> = indirect
            .iter()
            .map(|(n, depth)| {
                serde_json::json!({
                    "name": n.name,
                    "qualified_name": n.qualified_name,
                    "label": n.label,
                    "file_path": n.file_path,
                    "start_line": n.start_line,
                    "depth": depth,
                })
            })
            .collect();
        let output = serde_json::json!({
            "symbol": qn,
            "project": project,
            "max_depth": max_depth,
            "risk_level": risk_level,
            "direct_dependents": direct_json,
            "indirect_dependents": indirect_json,
            "affected_files": files,
        });
        println!("{}", serde_json::to_string_pretty(&output)?);
    } else {
        println!(
            "Impact Analysis for '{}' in project '{}' (depth {}):\n",
            qn, project, max_depth
        );
        println!("  Risk Level:          {}", risk_level.to_uppercase());
        println!("  Direct Dependents:   {}", direct.len());
        println!("  Indirect Dependents: {}", indirect.len());
        println!("  Affected Files:      {}", files.len());

        if !direct.is_empty() {
            println!("\n  Direct Dependents:");
            for n in &direct {
                println!(
                    "    {} ({}) — {}:{}",
                    n.name, n.label, n.file_path, n.start_line
                );
            }
        }

        if !indirect.is_empty() {
            println!("\n  Indirect Dependents:");
            for (n, d) in &indirect {
                println!(
                    "    [depth {}] {} ({}) — {}:{}",
                    d, n.name, n.label, n.file_path, n.start_line
                );
            }
        }

        if !files.is_empty() {
            println!("\n  Affected Files:");
            for f in &files {
                println!("    {}", f);
            }
        }
    }

    Ok(())
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Resolve a symbol by qualified name or name, returning its node ID.
/// Tries exact QN match first, then ranked name search.
fn resolve_symbol(store: &codryn_store::Store, project: &str, qn: &str) -> Result<i64> {
    let results = store
        .find_symbol_ranked(project, qn, None, false, 1)
        .with_context(|| format!("symbol resolution failed for '{qn}' in project '{project}'"))?;

    results
        .into_iter()
        .next()
        .map(|(n, _, _)| n.id)
        .ok_or_else(|| anyhow::anyhow!("symbol '{}' not found in project '{}'", qn, project))
}
