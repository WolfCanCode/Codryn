use anyhow::{Context, Result};
use std::path::Path;

/// Run the complexity command.
///
/// - `min_cyclomatic`: only include nodes with cyclomatic >= this value (default 0)
/// - `min_cognitive`:  only include nodes with cognitive  >= this value (default 0)
/// - `top`:            limit results (default 20)
/// - `json`:           machine-readable JSON output
pub fn run_complexity(
    store_dir: &Path,
    project: &str,
    min_cyclomatic: Option<u32>,
    min_cognitive: Option<u32>,
    top: Option<usize>,
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
        codryn_store::Store::open(&db_path).context("failed to open store for complexity report")?;

    let limit = top.unwrap_or(20) as i64;
    let min_cyc = min_cyclomatic.unwrap_or(0) as i64;
    let min_cog = min_cognitive.unwrap_or(0) as i64;

    let rows = store
        .query_complexity(project, min_cyc, min_cog, limit)
        .with_context(|| format!("complexity query failed for project '{project}'"))?;

    if json {
        print_json(&rows)?;
    } else {
        print_human(&rows, project, top.unwrap_or(20));
    }

    Ok(())
}

// ── JSON output ───────────────────────────────────────────────────────────────

fn print_json(rows: &[codryn_store::ComplexityRow]) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(rows)?);
    Ok(())
}

// ── Human-readable output ─────────────────────────────────────────────────────

fn print_human(rows: &[codryn_store::ComplexityRow], project: &str, top: usize) {
    if rows.is_empty() {
        println!("No symbols with complexity data found for project '{project}'.");
        println!("Tip: re-index the project to populate complexity metrics.");
        return;
    }

    println!(
        "Complexity Report for project '{}' (top {}):\n",
        project, top
    );

    // Column widths
    let name_w = rows.iter().map(|r| r.name.len()).max().unwrap_or(4).max(4);
    let file_w = rows
        .iter()
        .map(|r| {
            let loc = format!("{}:{}", r.file_path, r.start_line);
            loc.len()
        })
        .max()
        .unwrap_or(4)
        .max(4);

    println!(
        "  {:<name_w$}  {:<file_w$}  {:>10}  {:>9}",
        "Name",
        "File",
        "Cyclomatic",
        "Cognitive",
        name_w = name_w,
        file_w = file_w,
    );
    println!(
        "  {:<name_w$}  {:<file_w$}  {:>10}  {:>9}",
        "-".repeat(name_w),
        "-".repeat(file_w),
        "----------",
        "---------",
        name_w = name_w,
        file_w = file_w,
    );

    for row in rows {
        let loc = format!("{}:{}", row.file_path, row.start_line);
        println!(
            "  {:<name_w$}  {:<file_w$}  {:>10}  {:>9}",
            row.name,
            loc,
            row.cyclomatic_complexity,
            row.cognitive_complexity,
            name_w = name_w,
            file_w = file_w,
        );
    }
}
