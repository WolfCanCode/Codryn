use anyhow::{Context, Result};
use std::path::Path;

/// Run the doc-coverage command.
///
/// - `module_filter`: optional substring to filter modules by file path
/// - `json`:          machine-readable JSON output
pub fn run_doc_coverage(
    store_dir: &Path,
    project: &str,
    module_filter: Option<&str>,
    json: bool,
) -> Result<()> {
    let db_path = store_dir.join("graph.db");
    if !db_path.exists() {
        anyhow::bail!(
            "database not found at {}. Has the server been run at least once?",
            db_path.display()
        );
    }

    let store = codryn_store::Store::open(&db_path)
        .context("failed to open store for doc coverage report")?;

    let rows = store
        .query_doc_coverage(project, module_filter)
        .with_context(|| format!("doc coverage query failed for project '{project}'"))?;

    if json {
        print_json(&rows)?;
    } else {
        print_human(&rows, project);
    }

    Ok(())
}

// ── JSON output ───────────────────────────────────────────────────────────────

fn print_json(rows: &[codryn_store::DocCoverageRow]) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(rows)?);
    Ok(())
}

// ── Human-readable output ─────────────────────────────────────────────────────

fn print_human(rows: &[codryn_store::DocCoverageRow], project: &str) {
    if rows.is_empty() {
        println!("No symbols with documentation data found for project '{project}'.");
        println!("Tip: re-index the project to populate doc coverage metrics.");
        return;
    }

    println!("Doc Coverage Report for project '{project}':\n");

    // Column widths
    let module_w = rows
        .iter()
        .map(|r| r.module.len())
        .max()
        .unwrap_or(6)
        .max(6);

    println!(
        "  {:<module_w$}  {:>5}  {:>10}  Coverage",
        "Module",
        "Total",
        "Documented",
        module_w = module_w,
    );
    println!(
        "  {:<module_w$}  {:>5}  {:>10}  --------",
        "-".repeat(module_w),
        "-----",
        "----------",
        module_w = module_w,
    );

    let mut total_symbols: u64 = 0;
    let mut total_documented: u64 = 0;

    for row in rows {
        total_symbols += row.total_symbols as u64;
        total_documented += row.documented_symbols as u64;

        let attention = if row.needs_attention {
            "  ⚠ needs attention"
        } else {
            ""
        };

        println!(
            "  {:<module_w$}  {:>5}  {:>10}  {:.1}%{}",
            row.module,
            row.total_symbols,
            row.documented_symbols,
            row.coverage_pct,
            attention,
            module_w = module_w,
        );
    }

    println!();
    let overall_pct = if total_symbols > 0 {
        total_documented as f64 / total_symbols as f64 * 100.0
    } else {
        0.0
    };
    println!(
        "Overall: {} symbols, {} documented ({:.1}%)",
        total_symbols, total_documented, overall_pct
    );
}
