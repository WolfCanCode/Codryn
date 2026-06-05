//! Dead code detection service.

use anyhow::Result;
use codryn_store::Store;
use rusqlite::params;
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct DeadCodeResult {
    pub symbol: String,
    pub qualified_name: String,
    pub file_path: String,
    pub label: String,
    pub confidence: String,
    pub reason: String,
}

pub struct FindDeadCodeArgs {
    pub project: String,
    pub scope: Option<String>,
    pub limit: Option<i32>,
}

pub fn find_dead_code(
    store: &Store,
    project: &str,
    scope: Option<&str>,
    limit: Option<i32>,
) -> Result<Vec<DeadCodeResult>> {
    if project.trim().is_empty() {
        anyhow::bail!("Project name is required");
    }

    let limit = limit.unwrap_or(50);
    let conn = store.conn();

    let sql = if scope.is_some() {
        "SELECT n.id, n.name, n.qualified_name, n.file_path, n.label, n.properties \
         FROM nodes n \
         WHERE n.project = ?1 \
           AND n.label IN ('Function','Method','Class') \
           AND n.file_path LIKE ?3 \
           AND NOT EXISTS ( \
             SELECT 1 FROM edges e WHERE e.target_id = n.id AND e.project = ?1 \
           ) \
         LIMIT ?2"
    } else {
        "SELECT n.id, n.name, n.qualified_name, n.file_path, n.label, n.properties \
         FROM nodes n \
         WHERE n.project = ?1 \
           AND n.label IN ('Function','Method','Class') \
           AND NOT EXISTS ( \
             SELECT 1 FROM edges e WHERE e.target_id = n.id AND e.project = ?1 \
           ) \
         LIMIT ?2"
    };

    let mut stmt = conn.prepare(sql)?;

    let rows: Vec<(i64, String, String, String, String, Option<String>)> = if let Some(s) = scope {
        let pattern = format!("{}%", s);
        stmt.query_map(params![project, limit, pattern], |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
                row.get(5)?,
            ))
        })?
        .filter_map(|r| r.ok())
        .collect()
    } else {
        stmt.query_map(params![project, limit], |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
                row.get(5)?,
            ))
        })?
        .filter_map(|r| r.ok())
        .collect()
    };

    let mut results = Vec::new();
    for (_id, name, qn, file_path, label, props) in rows {
        if let Some(ref p) = props {
            if p.contains("\"is_test\":true") || p.contains("\"is_test\": true") {
                continue;
            }
            if p.contains("\"is_exported\":true") || p.contains("\"is_exported\": true") {
                continue;
            }
        }
        results.push(DeadCodeResult {
            symbol: name,
            qualified_name: qn,
            file_path,
            label: label.clone(),
            confidence: "high".into(),
            reason: format!(
                "No incoming references found for this {}",
                label.to_lowercase()
            ),
        });
    }

    Ok(results)
}
