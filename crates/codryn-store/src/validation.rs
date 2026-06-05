use anyhow::{Context, Result};
use rusqlite::params;
use serde::Serialize;

/// Report produced by `Store::validate_graph()`.
/// Each field lists the IDs of affected nodes or edges.
#[derive(Debug, Serialize)]
pub struct ValidationReport {
    /// Edge IDs whose source_id or target_id does not exist in the nodes table.
    pub dangling_edges: Vec<i64>,
    /// Node IDs that have no edges (neither as source nor as target).
    pub orphan_nodes: Vec<i64>,
    /// Qualified names that appear more than once within the project,
    /// paired with the list of node IDs sharing that name.
    pub duplicate_qns: Vec<(String, Vec<i64>)>,
    /// (node_id, field_name) pairs where a required property (name,
    /// qualified_name, or file_path) is empty or NULL.
    pub missing_properties: Vec<(i64, String)>,
    /// Node IDs whose `properties` column contains non-NULL, non-empty text
    /// that is not valid JSON.
    pub invalid_properties_json: Vec<i64>,
    /// Edge IDs where source_id == target_id (self-loops).
    pub self_loops: Vec<i64>,
    /// Edge IDs where the source node's project or the target node's project
    /// differs from the edge's own project column.
    pub cross_project_edges: Vec<i64>,
    /// Total number of individual issues found across all categories.
    pub total_issues: usize,
}

impl crate::Store {
    /// Run all validation checks for `project` and return a `ValidationReport`.
    pub fn validate_graph(&self, project: &str) -> Result<ValidationReport> {
        let dangling_edges = self.find_dangling_edges(project)?;
        let orphan_nodes = self.find_orphan_nodes(project)?;
        let duplicate_qns = self.find_duplicate_qns(project)?;
        let missing_properties = self.find_missing_properties(project)?;
        let invalid_properties_json = self.find_invalid_properties_json(project)?;
        let self_loops = self.find_self_loops(project)?;
        let cross_project_edges = self.find_cross_project_edges(project)?;

        let total_issues = dangling_edges.len()
            + orphan_nodes.len()
            + duplicate_qns
                .iter()
                .map(|(_, ids)| ids.len().saturating_sub(1))
                .sum::<usize>()
            + missing_properties.len()
            + invalid_properties_json.len()
            + self_loops.len()
            + cross_project_edges.len();

        Ok(ValidationReport {
            dangling_edges,
            orphan_nodes,
            duplicate_qns,
            missing_properties,
            invalid_properties_json,
            self_loops,
            cross_project_edges,
            total_issues,
        })
    }

    /// Apply safe fixes to the graph for `project`:
    /// - Delete dangling edges (edges referencing non-existent nodes).
    /// - Set `properties` to NULL for nodes with invalid (non-JSON) properties.
    ///
    /// Does NOT merge duplicate nodes.
    ///
    /// Returns the total number of fixes applied.
    pub fn fix_safe(&self, project: &str) -> Result<usize> {
        let mut fixes = 0usize;

        // --- Fix 1: Remove dangling edges ---
        // Edges where source_id is not in nodes
        let deleted_source: usize = self
            .conn
            .execute(
                "DELETE FROM edges WHERE project = ?1 \
                 AND source_id NOT IN (SELECT id FROM nodes)",
                params![project],
            )
            .context("fix_safe: failed to delete edges with missing source_id")?;

        // Edges where target_id is not in nodes
        let deleted_target: usize = self
            .conn
            .execute(
                "DELETE FROM edges WHERE project = ?1 \
                 AND target_id NOT IN (SELECT id FROM nodes)",
                params![project],
            )
            .context("fix_safe: failed to delete edges with missing target_id")?;

        fixes += deleted_source + deleted_target;

        // --- Fix 2: Nullify invalid properties_json ---
        // Collect node IDs with invalid JSON first, then update them.
        let invalid_ids = self.find_invalid_properties_json(project)?;
        if !invalid_ids.is_empty() {
            let tx = self
                .conn
                .unchecked_transaction()
                .context("fix_safe: failed to begin transaction for properties cleanup")?;
            {
                let mut stmt = tx
                    .prepare("UPDATE nodes SET properties = NULL WHERE id = ?1")
                    .context("fix_safe: failed to prepare properties update")?;
                for id in &invalid_ids {
                    stmt.execute(params![id])
                        .context("fix_safe: failed to nullify invalid properties")?;
                }
            }
            tx.commit()
                .context("fix_safe: failed to commit properties cleanup")?;
            fixes += invalid_ids.len();
        }

        Ok(fixes)
    }

    // ── Private helpers ───────────────────────────────────────

    /// Edges whose source_id or target_id does not exist in the nodes table.
    fn find_dangling_edges(&self, project: &str) -> Result<Vec<i64>> {
        let mut ids = Vec::new();

        // Missing source
        {
            let mut stmt = self.conn.prepare(
                "SELECT e.id FROM edges e \
                 WHERE e.project = ?1 \
                 AND NOT EXISTS (SELECT 1 FROM nodes n WHERE n.id = e.source_id)",
            )?;
            let rows = stmt.query_map(params![project], |r| r.get::<_, i64>(0))?;
            for r in rows.flatten() {
                ids.push(r);
            }
        }

        // Missing target
        {
            let mut stmt = self.conn.prepare(
                "SELECT e.id FROM edges e \
                 WHERE e.project = ?1 \
                 AND NOT EXISTS (SELECT 1 FROM nodes n WHERE n.id = e.target_id)",
            )?;
            let rows = stmt.query_map(params![project], |r| r.get::<_, i64>(0))?;
            for r in rows.flatten() {
                if !ids.contains(&r) {
                    ids.push(r);
                }
            }
        }

        Ok(ids)
    }

    /// Nodes that have no edges (neither as source nor as target).
    fn find_orphan_nodes(&self, project: &str) -> Result<Vec<i64>> {
        let mut stmt = self.conn.prepare(
            "SELECT n.id FROM nodes n \
             WHERE n.project = ?1 \
             AND NOT EXISTS (SELECT 1 FROM edges e WHERE e.source_id = n.id) \
             AND NOT EXISTS (SELECT 1 FROM edges e WHERE e.target_id = n.id)",
        )?;
        let rows = stmt.query_map(params![project], |r| r.get::<_, i64>(0))?;
        Ok(rows.flatten().collect())
    }

    /// Qualified names that appear more than once within the project.
    fn find_duplicate_qns(&self, project: &str) -> Result<Vec<(String, Vec<i64>)>> {
        // Find all QNs that have duplicates
        let mut dup_qns: Vec<String> = Vec::new();
        {
            let mut stmt = self.conn.prepare(
                "SELECT qualified_name FROM nodes \
                 WHERE project = ?1 \
                 GROUP BY qualified_name \
                 HAVING COUNT(*) > 1",
            )?;
            let rows = stmt.query_map(params![project], |r| r.get::<_, String>(0))?;
            for r in rows.flatten() {
                dup_qns.push(r);
            }
        }

        let mut result = Vec::new();
        for qn in dup_qns {
            let mut ids: Vec<i64> = Vec::new();
            let mut stmt = self.conn.prepare(
                "SELECT id FROM nodes WHERE project = ?1 AND qualified_name = ?2 ORDER BY id",
            )?;
            let rows = stmt.query_map(params![project, &qn], |r| r.get::<_, i64>(0))?;
            for r in rows.flatten() {
                ids.push(r);
            }
            result.push((qn, ids));
        }

        Ok(result)
    }

    /// Nodes where `name`, `qualified_name`, or `file_path` is empty or NULL.
    fn find_missing_properties(&self, project: &str) -> Result<Vec<(i64, String)>> {
        let mut issues: Vec<(i64, String)> = Vec::new();

        // name is empty or null
        {
            let mut stmt = self.conn.prepare(
                "SELECT id FROM nodes WHERE project = ?1 AND (name IS NULL OR name = '')",
            )?;
            let rows = stmt.query_map(params![project], |r| r.get::<_, i64>(0))?;
            for id in rows.flatten() {
                issues.push((id, "name".to_string()));
            }
        }

        // qualified_name is empty or null
        {
            let mut stmt = self.conn.prepare(
                "SELECT id FROM nodes WHERE project = ?1 AND (qualified_name IS NULL OR qualified_name = '')",
            )?;
            let rows = stmt.query_map(params![project], |r| r.get::<_, i64>(0))?;
            for id in rows.flatten() {
                issues.push((id, "qualified_name".to_string()));
            }
        }

        // file_path is empty or null
        {
            let mut stmt = self.conn.prepare(
                "SELECT id FROM nodes WHERE project = ?1 AND (file_path IS NULL OR file_path = '')",
            )?;
            let rows = stmt.query_map(params![project], |r| r.get::<_, i64>(0))?;
            for id in rows.flatten() {
                issues.push((id, "file_path".to_string()));
            }
        }

        Ok(issues)
    }

    /// Nodes whose `properties` column is non-NULL, non-empty, and not valid JSON.
    fn find_invalid_properties_json(&self, project: &str) -> Result<Vec<i64>> {
        // Fetch all nodes that have a non-null, non-empty properties value
        let mut stmt = self.conn.prepare(
            "SELECT id, properties FROM nodes \
             WHERE project = ?1 AND properties IS NOT NULL AND properties != '' AND properties != '{}'",
        )?;
        let rows = stmt.query_map(params![project], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
        })?;

        let mut invalid = Vec::new();
        for row in rows.flatten() {
            let (id, props) = row;
            if serde_json::from_str::<serde_json::Value>(&props).is_err() {
                invalid.push(id);
            }
        }
        Ok(invalid)
    }

    /// Edges where source_id == target_id.
    fn find_self_loops(&self, project: &str) -> Result<Vec<i64>> {
        let mut stmt = self
            .conn
            .prepare("SELECT id FROM edges WHERE project = ?1 AND source_id = target_id")?;
        let rows = stmt.query_map(params![project], |r| r.get::<_, i64>(0))?;
        Ok(rows.flatten().collect())
    }

    /// Edges where the source node's project or the target node's project
    /// differs from the edge's own project column.
    fn find_cross_project_edges(&self, project: &str) -> Result<Vec<i64>> {
        let mut stmt = self.conn.prepare(
            "SELECT e.id FROM edges e \
             LEFT JOIN nodes src ON src.id = e.source_id \
             LEFT JOIN nodes tgt ON tgt.id = e.target_id \
             WHERE e.project = ?1 \
             AND (src.project IS NOT NULL AND src.project != e.project \
                  OR tgt.project IS NOT NULL AND tgt.project != e.project)",
        )?;
        let rows = stmt.query_map(params![project], |r| r.get::<_, i64>(0))?;
        Ok(rows.flatten().collect())
    }
}
