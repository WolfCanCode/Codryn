use anyhow::{Context, Result};
use rusqlite::params;
use serde::Serialize;
use tracing::info;

/// Report produced by `Store::deduplicate_dry_run()`.
/// Describes all duplicate groups found and what would be merged.
#[derive(Debug, Serialize)]
pub struct DedupeReport {
    /// Each group represents one qualified name that has multiple nodes.
    pub groups: Vec<DedupeGroup>,
    /// Total number of duplicate (non-canonical) nodes that would be removed.
    pub total_duplicates: usize,
}

/// A single group of duplicate nodes sharing the same qualified name.
#[derive(Debug, Serialize)]
pub struct DedupeGroup {
    /// The qualified name shared by all nodes in this group.
    pub qualified_name: String,
    /// The ID of the node chosen as canonical (will be kept).
    pub canonical_id: i64,
    /// IDs of the duplicate nodes that would be removed (edges redirected to canonical).
    pub duplicate_ids: Vec<i64>,
    /// Human-readable reason for canonical selection.
    /// Either "most recent" or "richest properties".
    pub reason: String,
}

/// A group of near-duplicate nodes sharing the same (name, file_path, label).
#[derive(Debug, Clone, Serialize)]
pub struct NearDedupeGroup {
    /// The shared name across all nodes in this group.
    pub name: String,
    /// The shared file_path across all nodes in this group.
    pub file_path: String,
    /// The shared label across all nodes in this group.
    pub label: String,
    /// The ID of the node chosen as survivor (will be kept).
    pub survivor_id: i64,
    /// IDs of the nodes that will be discarded (edges redirected to survivor).
    pub discarded_ids: Vec<i64>,
}

impl crate::Store {
    /// Scan for duplicate qualified names within `project` and return a report
    /// describing what would be merged. Does NOT mutate the graph.
    pub fn deduplicate_dry_run(&self, project: &str) -> Result<DedupeReport> {
        let groups = self.find_dedupe_groups(project)?;
        let total_duplicates = groups.iter().map(|g| g.duplicate_ids.len()).sum();
        Ok(DedupeReport {
            groups,
            total_duplicates,
        })
    }

    /// Apply deduplication for `project`: for each group of duplicate nodes,
    /// redirect all edges from duplicates to the canonical node, then delete
    /// the duplicate nodes.
    ///
    /// Returns the total number of duplicate nodes removed.
    pub fn deduplicate_apply(&self, project: &str) -> Result<usize> {
        let groups = self.find_dedupe_groups(project)?;
        let mut removed = 0usize;

        for group in &groups {
            for &dup_id in &group.duplicate_ids {
                self.redirect_edges(dup_id, group.canonical_id)
                    .with_context(|| {
                        format!(
                            "failed to redirect edges from node {} to {}",
                            dup_id, group.canonical_id
                        )
                    })?;
                removed += 1;
            }
        }

        Ok(removed)
    }

    /// Detect near-duplicate nodes within a project.
    ///
    /// Near-duplicates are nodes sharing the same `name`, `file_path`, and `label`
    /// but having different `node_id` values. Returns groups of near-duplicates
    /// with the survivor selected based on the most recent `indexed_at` timestamp,
    /// with ties broken by lexicographically smallest node_id.
    pub fn detect_near_duplicates(&self, project: &str) -> Result<Vec<NearDedupeGroup>> {
        // Find all (name, file_path, label) combinations that appear more than once
        let mut stmt = self.conn.prepare(
            "SELECT name, file_path, label FROM nodes \
             WHERE project = ?1 AND file_path != '' \
             GROUP BY name, file_path, label \
             HAVING COUNT(*) > 1",
        )?;

        let groups: Vec<(String, String, String)> = stmt
            .query_map(params![project], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })?
            .filter_map(|r| r.ok())
            .collect();

        let mut result = Vec::new();

        for (name, file_path, label) in groups {
            // Fetch all node IDs in this group
            let mut id_stmt = self.conn.prepare(
                "SELECT id FROM nodes \
                 WHERE project = ?1 AND name = ?2 AND file_path = ?3 AND label = ?4 \
                 ORDER BY id ASC",
            )?;

            let node_ids: Vec<i64> = id_stmt
                .query_map(params![project, &name, &file_path, &label], |row| {
                    row.get::<_, i64>(0)
                })?
                .filter_map(|r| r.ok())
                .collect();

            if node_ids.len() < 2 {
                continue;
            }

            // Select survivor: most recent indexed_at, tie-break by smallest node_id.
            // Since all nodes are in the same project, they share the same indexed_at.
            // The tie-breaker (lexicographically smallest node_id) determines the survivor.
            // For integer IDs, we use the numerically smallest ID as the "lexicographically smallest".
            let survivor_id = *node_ids.first().unwrap(); // smallest ID (already sorted ASC)

            let discarded_ids: Vec<i64> = node_ids
                .iter()
                .copied()
                .filter(|&id| id != survivor_id)
                .collect();

            result.push(NearDedupeGroup {
                name,
                file_path,
                label,
                survivor_id,
                discarded_ids,
            });
        }

        Ok(result)
    }

    /// Merge near-duplicate nodes within a project.
    ///
    /// For each group of near-duplicates (same name, file_path, label):
    /// - Keeps the survivor (most recent indexed_at, tie-break: smallest node_id)
    /// - Redirects all edges from discarded nodes to the survivor
    /// - Removes self-referential edges (where source_id == target_id on survivor)
    /// - Deletes the discarded nodes
    /// - Logs each merge operation
    ///
    /// Returns the total number of merge operations performed (one per group).
    pub fn merge_near_duplicates(&self, project: &str) -> Result<usize> {
        let groups = self.detect_near_duplicates(project)?;
        let mut merge_count = 0usize;

        for group in &groups {
            let mut redirected_edge_count = 0i64;

            for &discarded_id in &group.discarded_ids {
                let redirected =
                    self.redirect_edges_for_near_dedup(discarded_id, group.survivor_id)?;
                redirected_edge_count += redirected;
            }

            // Remove self-referential edges on the survivor
            let self_refs_removed = self.remove_self_referential_edges(group.survivor_id)?;
            if self_refs_removed > 0 {
                info!(
                    survivor_id = group.survivor_id,
                    self_refs_removed, "removed self-referential edges after near-duplicate merge"
                );
            }

            info!(
                survivor_id = group.survivor_id,
                discarded_ids = ?group.discarded_ids,
                redirected_edge_count,
                name = %group.name,
                file_path = %group.file_path,
                label = %group.label,
                "near-duplicate merge completed"
            );

            merge_count += 1;
        }

        Ok(merge_count)
    }

    /// Redirect all edges from `old_id` to `new_id` for near-duplicate merging.
    /// Returns the count of edges that were redirected (not counting duplicates that were deleted).
    fn redirect_edges_for_near_dedup(&self, old_id: i64, new_id: i64) -> Result<i64> {
        let mut redirected = 0i64;

        // Redirect outgoing edges: source_id = old_id → new_id
        // Only update if an equivalent edge does not already exist on new_id
        let outgoing_updated = self.conn.execute(
            "UPDATE edges SET source_id = ?2 \
             WHERE source_id = ?1 \
             AND NOT EXISTS (\
               SELECT 1 FROM edges e2 \
               WHERE e2.source_id = ?2 \
                 AND e2.target_id = edges.target_id \
                 AND e2.type = edges.type\
             )",
            params![old_id, new_id],
        )?;
        redirected += outgoing_updated as i64;

        // Redirect incoming edges: target_id = old_id → new_id
        // Only update if an equivalent edge does not already exist on new_id
        let incoming_updated = self.conn.execute(
            "UPDATE edges SET target_id = ?2 \
             WHERE target_id = ?1 \
             AND NOT EXISTS (\
               SELECT 1 FROM edges e2 \
               WHERE e2.target_id = ?2 \
                 AND e2.source_id = edges.source_id \
                 AND e2.type = edges.type\
             )",
            params![old_id, new_id],
        )?;
        redirected += incoming_updated as i64;

        // Delete any remaining edges that still reference old_id
        // (these are duplicates that couldn't be redirected)
        self.conn.execute(
            "DELETE FROM edges WHERE source_id = ?1 OR target_id = ?1",
            params![old_id],
        )?;

        // Delete the discarded node
        self.conn
            .execute("DELETE FROM nodes WHERE id = ?1", params![old_id])?;

        Ok(redirected)
    }

    /// Remove self-referential edges on a node (where source_id == target_id).
    /// Returns the number of edges removed.
    fn remove_self_referential_edges(&self, node_id: i64) -> Result<i64> {
        let removed = self.conn.execute(
            "DELETE FROM edges WHERE source_id = ?1 AND target_id = ?1",
            params![node_id],
        )?;
        Ok(removed as i64)
    }

    /// Redirect all edges referencing `old_id` to `new_id`, avoiding duplicate
    /// edges. After redirection, any remaining edges still referencing `old_id`
    /// (which would be duplicates of edges already on `new_id`) are deleted.
    /// Finally, the `old_id` node itself is deleted.
    fn redirect_edges(&self, old_id: i64, new_id: i64) -> Result<()> {
        // Redirect outgoing edges: source_id = old_id → new_id
        // Only update if an equivalent edge (same new_id source, same target, same type)
        // does not already exist.
        self.conn
            .execute(
                "UPDATE edges SET source_id = ?2 \
                 WHERE source_id = ?1 \
                 AND NOT EXISTS (\
                   SELECT 1 FROM edges e2 \
                   WHERE e2.source_id = ?2 \
                     AND e2.target_id = edges.target_id \
                     AND e2.type = edges.type\
                 )",
                params![old_id, new_id],
            )
            .context("redirect_edges: failed to update outgoing edges")?;

        // Redirect incoming edges: target_id = old_id → new_id
        // Only update if an equivalent edge (same source, same new_id target, same type)
        // does not already exist.
        self.conn
            .execute(
                "UPDATE edges SET target_id = ?2 \
                 WHERE target_id = ?1 \
                 AND NOT EXISTS (\
                   SELECT 1 FROM edges e2 \
                   WHERE e2.target_id = ?2 \
                     AND e2.source_id = edges.source_id \
                     AND e2.type = edges.type\
                 )",
                params![old_id, new_id],
            )
            .context("redirect_edges: failed to update incoming edges")?;

        // Delete any remaining edges that still reference old_id
        // (these are duplicates that couldn't be redirected because equivalent
        // edges already exist on new_id).
        self.conn
            .execute(
                "DELETE FROM edges WHERE source_id = ?1 OR target_id = ?1",
                params![old_id],
            )
            .context("redirect_edges: failed to delete residual edges")?;

        // Delete the old (duplicate) node itself.
        self.conn
            .execute("DELETE FROM nodes WHERE id = ?1", params![old_id])
            .context("redirect_edges: failed to delete old node")?;

        Ok(())
    }

    // ── Private helpers ───────────────────────────────────────

    /// Find all duplicate groups for `project` and compute the canonical node
    /// for each group using the richness heuristic.
    fn find_dedupe_groups(&self, project: &str) -> Result<Vec<DedupeGroup>> {
        // Step 1: find all qualified names that appear more than once.
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

        let mut groups = Vec::new();

        for qn in dup_qns {
            // Step 2: fetch all nodes for this QN, ordered by id ascending.
            let mut node_rows: Vec<(i64, String, i32, i32, Option<String>)> = Vec::new();
            {
                let mut stmt = self.conn.prepare(
                    "SELECT id, file_path, start_line, end_line, properties \
                     FROM nodes \
                     WHERE project = ?1 AND qualified_name = ?2 \
                     ORDER BY id ASC",
                )?;
                let rows = stmt.query_map(params![project, &qn], |r| {
                    Ok((
                        r.get::<_, i64>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, i32>(2)?,
                        r.get::<_, i32>(3)?,
                        r.get::<_, Option<String>>(4)?,
                    ))
                })?;
                for r in rows.flatten() {
                    node_rows.push(r);
                }
            }

            if node_rows.len() < 2 {
                continue;
            }

            // Step 3: select canonical node.
            let (canonical_id, reason) = self.select_canonical(project, &node_rows)?;

            let duplicate_ids: Vec<i64> = node_rows
                .iter()
                .map(|(id, _, _, _, _)| *id)
                .filter(|&id| id != canonical_id)
                .collect();

            groups.push(DedupeGroup {
                qualified_name: qn,
                canonical_id,
                duplicate_ids,
                reason,
            });
        }

        Ok(groups)
    }

    /// Select the canonical node from a list of candidate rows for the same QN.
    ///
    /// Selection priority:
    /// 1. Most recently indexed project (via `projects.indexed_at`) — if the
    ///    project has a meaningful timestamp, prefer the node with the highest
    ///    node id (proxy for insertion order within the same project).
    /// 2. Richest node: count non-empty/non-null fields
    ///    (file_path, start_line != 0, end_line != 0, properties != '{}'/NULL,
    ///    has_docs in properties, cyclomatic_complexity in properties).
    /// 3. Tie-break: lowest id (first inserted).
    ///
    /// Returns `(canonical_id, reason)`.
    fn select_canonical(
        &self,
        project: &str,
        node_rows: &[(i64, String, i32, i32, Option<String>)],
    ) -> Result<(i64, String)> {
        // Fetch the project's indexed_at timestamp to use as a recency signal.
        // If multiple nodes exist within the same project, we use the node id
        // as a proxy for insertion order (higher id = more recently inserted).
        let indexed_at: Option<String> = self
            .conn
            .query_row(
                "SELECT indexed_at FROM projects WHERE name = ?1",
                params![project],
                |r| r.get(0),
            )
            .ok();

        // If we have a meaningful indexed_at, prefer the highest node id
        // (most recently inserted within this project's last index run).
        if indexed_at.as_deref().is_some_and(|s| !s.is_empty()) {
            // Find the node with the highest id (most recently inserted).
            let max_id = node_rows
                .iter()
                .map(|(id, _, _, _, _)| *id)
                .max()
                .unwrap_or(node_rows[0].0);

            // Check if there's a clear "most recent" winner (highest id).
            // We only use this heuristic if the highest-id node is unique.
            let max_count = node_rows
                .iter()
                .filter(|(id, _, _, _, _)| *id == max_id)
                .count();

            if max_count == 1 {
                return Ok((max_id, "most recent".to_string()));
            }
        }

        // Fall back to richness scoring.
        let mut best_id = node_rows[0].0;
        let mut best_score = 0i32;

        for (id, file_path, start_line, end_line, properties) in node_rows {
            let score = compute_richness_score(file_path, *start_line, *end_line, properties);
            if score > best_score {
                best_score = score;
                best_id = *id;
            }
            // Tie-break: keep the lower id (first inserted), so we don't update
            // best_id when scores are equal.
        }

        Ok((best_id, "richest properties".to_string()))
    }
}

/// Compute a richness score for a node based on how many meaningful fields it has.
/// Higher score = more information = preferred as canonical.
fn compute_richness_score(
    file_path: &str,
    start_line: i32,
    end_line: i32,
    properties: &Option<String>,
) -> i32 {
    let mut score = 0i32;

    // Has a non-empty file path
    if !file_path.is_empty() {
        score += 1;
    }

    // Has a meaningful line span (start_line > 0)
    if start_line > 0 {
        score += 1;
    }

    // Has a meaningful end line (end_line > start_line)
    if end_line > start_line {
        score += 1;
    }

    // Has non-trivial properties JSON
    if let Some(props) = properties {
        if !props.is_empty() && props != "{}" {
            score += 1;

            // Bonus for specific rich properties
            if let Ok(obj) = serde_json::from_str::<serde_json::Value>(props) {
                if obj
                    .get("has_docs")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false)
                {
                    score += 1;
                }
                if obj.get("doc_lines").and_then(|v| v.as_u64()).unwrap_or(0) > 0 {
                    score += 1;
                }
                if obj.get("cyclomatic_complexity").is_some() {
                    score += 1;
                }
                if obj.get("cognitive_complexity").is_some() {
                    score += 1;
                }
            }
        }
    }

    score
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Edge, Node, Project, Store};

    fn test_store() -> Store {
        Store::open_in_memory().unwrap()
    }

    fn setup_project(s: &Store, name: &str) {
        s.upsert_project(&Project {
            name: name.into(),
            indexed_at: "2025-01-01T00:00:00Z".into(),
            root_path: "/tmp".into(),
        })
        .unwrap();
    }

    fn make_node(project: &str, qn: &str, file_path: &str, start: i32, end: i32) -> Node {
        Node {
            id: 0,
            project: project.into(),
            label: "Function".into(),
            name: qn.split("::").last().unwrap_or(qn).into(),
            qualified_name: qn.into(),
            file_path: file_path.into(),
            start_line: start,
            end_line: end,
            properties_json: None,
        }
    }

    /// Insert a second node with the same qualified_name, bypassing the
    /// UNIQUE(project, qualified_name) constraint via PRAGMA writable_schema.
    /// This simulates a corrupted / pre-dedup graph state.
    fn insert_duplicate_node(
        s: &Store,
        project: &str,
        qn: &str,
        file_path: &str,
        start: i32,
        end: i32,
    ) -> i64 {
        s.conn()
            .execute_batch(&format!(
                "PRAGMA writable_schema = ON;\
                 CREATE TABLE IF NOT EXISTS _nodes_tmp (\
                   id INTEGER PRIMARY KEY AUTOINCREMENT,\
                   project TEXT NOT NULL,\
                   label TEXT NOT NULL,\
                   name TEXT NOT NULL,\
                   qualified_name TEXT NOT NULL,\
                   file_path TEXT DEFAULT '',\
                   start_line INTEGER DEFAULT 0,\
                   end_line INTEGER DEFAULT 0,\
                   properties TEXT DEFAULT '{{}}'\
                 );\
                 INSERT INTO _nodes_tmp SELECT * FROM nodes;\
                 INSERT INTO _nodes_tmp \
                   (project, label, name, qualified_name, file_path, start_line, end_line, properties) \
                   VALUES ('{project}', 'Function', '{name}', '{qn}', '{file_path}', {start}, {end}, '{{}}');\
                 DROP TABLE nodes;\
                 ALTER TABLE _nodes_tmp RENAME TO nodes;\
                 PRAGMA writable_schema = OFF;",
                project = project,
                name = qn.split("::").last().unwrap_or(qn),
                qn = qn,
                file_path = file_path,
                start = start,
                end = end,
            ))
            .unwrap();
        // Return the id of the just-inserted row
        s.conn()
            .query_row(
                "SELECT id FROM nodes WHERE project = ?1 AND qualified_name = ?2 ORDER BY id DESC LIMIT 1",
                rusqlite::params![project, qn],
                |r| r.get(0),
            )
            .unwrap()
    }

    // ── dry_run does not mutate ───────────────────────────────

    #[test]
    fn test_dry_run_does_not_mutate() {
        let s = test_store();
        setup_project(&s, "p");

        // Insert two nodes with the same QN (bypass UNIQUE constraint for the second)
        s.insert_node(&make_node("p", "p::foo", "src/a.rs", 1, 10))
            .unwrap();
        insert_duplicate_node(&s, "p", "p::foo", "src/b.rs", 5, 20);

        let report = s.deduplicate_dry_run("p").unwrap();
        assert_eq!(report.groups.len(), 1);
        assert_eq!(report.total_duplicates, 1);

        // Graph must be unchanged after dry-run
        let schema = s.get_graph_schema("p").unwrap();
        assert_eq!(schema.total_nodes, 2);
    }

    // ── duplicate detection ───────────────────────────────────

    #[test]
    fn test_no_duplicates_returns_empty_report() {
        let s = test_store();
        setup_project(&s, "p");

        s.insert_node(&make_node("p", "p::foo", "src/a.rs", 1, 10))
            .unwrap();
        s.insert_node(&make_node("p", "p::bar", "src/b.rs", 1, 10))
            .unwrap();

        let report = s.deduplicate_dry_run("p").unwrap();
        assert!(report.groups.is_empty());
        assert_eq!(report.total_duplicates, 0);
    }

    #[test]
    fn test_detects_multiple_duplicate_groups() {
        let s = test_store();
        setup_project(&s, "p");

        // Two duplicates for "p::foo"
        s.insert_node(&make_node("p", "p::foo", "src/a.rs", 1, 10))
            .unwrap();
        insert_duplicate_node(&s, "p", "p::foo", "src/b.rs", 5, 20);

        // Two duplicates for "p::bar"
        s.insert_node(&make_node("p", "p::bar", "src/c.rs", 1, 5))
            .unwrap();
        insert_duplicate_node(&s, "p", "p::bar", "src/d.rs", 2, 8);

        let report = s.deduplicate_dry_run("p").unwrap();
        assert_eq!(report.groups.len(), 2);
        assert_eq!(report.total_duplicates, 2);
    }

    // ── canonical selection ───────────────────────────────────

    #[test]
    fn test_canonical_prefers_most_recent_by_id() {
        let s = test_store();
        setup_project(&s, "p");

        // Both nodes have the same richness; higher id = more recent
        let id1 = s
            .insert_node(&make_node("p", "p::foo", "src/a.rs", 1, 10))
            .unwrap();
        let id2 = insert_duplicate_node(&s, "p", "p::foo", "src/a.rs", 1, 10);

        let report = s.deduplicate_dry_run("p").unwrap();
        assert_eq!(report.groups.len(), 1);
        let group = &report.groups[0];
        // Higher id should be canonical ("most recent")
        assert_eq!(group.canonical_id, id2);
        assert!(group.duplicate_ids.contains(&id1));
        assert_eq!(group.reason, "most recent");
    }

    #[test]
    fn test_canonical_prefers_richest_when_tied_recency() {
        let s = test_store();
        // Use a project with no indexed_at to force richness path
        s.upsert_project(&Project {
            name: "p".into(),
            indexed_at: "".into(),
            root_path: "/tmp".into(),
        })
        .unwrap();

        // Node 1: has file_path and span (richer)
        let id1 = s
            .insert_node(&make_node("p", "p::foo", "src/a.rs", 1, 10))
            .unwrap();
        // Node 2: no file_path, no span (poorer)
        let id2 = insert_duplicate_node(&s, "p", "p::foo", "", 0, 0);

        let report = s.deduplicate_dry_run("p").unwrap();
        assert_eq!(report.groups.len(), 1);
        let group = &report.groups[0];
        // id1 is richer (has file_path + span)
        assert_eq!(group.canonical_id, id1);
        assert!(group.duplicate_ids.contains(&id2));
        assert_eq!(group.reason, "richest properties");
    }

    // ── edge redirection ──────────────────────────────────────

    #[test]
    fn test_apply_redirects_edges_and_removes_duplicate() {
        let s = test_store();
        setup_project(&s, "p");

        let id1 = s
            .insert_node(&make_node("p", "p::foo", "src/a.rs", 1, 10))
            .unwrap();
        let id2 = insert_duplicate_node(&s, "p", "p::foo", "src/a.rs", 1, 10);
        let caller_id = s
            .insert_node(&make_node("p", "p::caller", "src/c.rs", 1, 5))
            .unwrap();

        // Edge pointing to the duplicate (id1)
        s.insert_edge(&Edge {
            id: 0,
            project: "p".into(),
            source_id: caller_id,
            target_id: id1,
            edge_type: "CALLS".into(),
            properties_json: None,
        })
        .unwrap();

        let removed = s.deduplicate_apply("p").unwrap();
        assert_eq!(removed, 1);

        // The duplicate node should be gone
        let schema = s.get_graph_schema("p").unwrap();
        // caller + canonical remain; duplicate removed
        assert_eq!(schema.total_nodes, 2);

        // The edge should now point to the canonical node (id2)
        let edges = s.get_edges("p", 100).unwrap();
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].target_id, id2);
    }

    #[test]
    fn test_apply_no_duplicate_edges_created() {
        let s = test_store();
        setup_project(&s, "p");

        let id1 = s
            .insert_node(&make_node("p", "p::foo", "src/a.rs", 1, 10))
            .unwrap();
        let id2 = s
            .insert_node(&make_node("p", "p::foo", "src/a.rs", 1, 10))
            .unwrap();
        let caller_id = s
            .insert_node(&make_node("p", "p::caller", "src/c.rs", 1, 5))
            .unwrap();

        // Same edge exists pointing to BOTH id1 and id2 (simulates pre-existing dup)
        s.insert_edge(&Edge {
            id: 0,
            project: "p".into(),
            source_id: caller_id,
            target_id: id1,
            edge_type: "CALLS".into(),
            properties_json: None,
        })
        .unwrap();
        s.insert_edge(&Edge {
            id: 0,
            project: "p".into(),
            source_id: caller_id,
            target_id: id2,
            edge_type: "CALLS".into(),
            properties_json: None,
        })
        .unwrap();

        s.deduplicate_apply("p").unwrap();

        // After dedup, only one CALLS edge from caller to canonical should exist
        let edges = s.get_edges("p", 100).unwrap();
        let calls_edges: Vec<_> = edges
            .iter()
            .filter(|e| e.edge_type == "CALLS" && e.source_id == caller_id)
            .collect();
        assert_eq!(calls_edges.len(), 1);
        assert_eq!(calls_edges[0].target_id, id2);
    }

    #[test]
    fn test_apply_preserves_unique_edges_from_both_nodes() {
        let s = test_store();
        setup_project(&s, "p");

        let id1 = s
            .insert_node(&make_node("p", "p::foo", "src/a.rs", 1, 10))
            .unwrap();
        let id2 = s
            .insert_node(&make_node("p", "p::foo", "src/a.rs", 1, 10))
            .unwrap();
        let caller_a = s
            .insert_node(&make_node("p", "p::callerA", "src/a.rs", 20, 30))
            .unwrap();
        let caller_b = s
            .insert_node(&make_node("p", "p::callerB", "src/b.rs", 1, 5))
            .unwrap();

        // callerA → id1 (unique edge)
        s.insert_edge(&Edge {
            id: 0,
            project: "p".into(),
            source_id: caller_a,
            target_id: id1,
            edge_type: "CALLS".into(),
            properties_json: None,
        })
        .unwrap();
        // callerB → id2 (unique edge on the other duplicate)
        s.insert_edge(&Edge {
            id: 0,
            project: "p".into(),
            source_id: caller_b,
            target_id: id2,
            edge_type: "CALLS".into(),
            properties_json: None,
        })
        .unwrap();

        s.deduplicate_apply("p").unwrap();

        // Both unique edges should now point to the canonical node
        let edges = s.get_edges("p", 100).unwrap();
        let calls_edges: Vec<_> = edges.iter().filter(|e| e.edge_type == "CALLS").collect();
        assert_eq!(calls_edges.len(), 2);
        assert!(calls_edges.iter().all(|e| e.target_id == id2));
    }

    // ── richness scoring ──────────────────────────────────────

    #[test]
    fn test_richness_score_empty_node() {
        let score = compute_richness_score("", 0, 0, &None);
        assert_eq!(score, 0);
    }

    #[test]
    fn test_richness_score_full_node() {
        let props = Some(
            r#"{"has_docs":true,"doc_lines":5,"cyclomatic_complexity":3,"cognitive_complexity":2}"#
                .to_string(),
        );
        let score = compute_richness_score("src/foo.rs", 1, 20, &props);
        // file_path(1) + start_line(1) + end_line>start(1) + non-trivial props(1)
        // + has_docs(1) + doc_lines(1) + cyclomatic(1) + cognitive(1) = 8
        assert_eq!(score, 8);
    }

    #[test]
    fn test_richness_score_partial_node() {
        let score = compute_richness_score("src/foo.rs", 1, 0, &None);
        // file_path(1) + start_line(1) = 2
        assert_eq!(score, 2);
    }

    // ── Near-duplicate detection tests ────────────────────────

    #[test]
    fn test_detect_near_duplicates_empty_when_no_duplicates() {
        let s = test_store();
        setup_project(&s, "p");

        s.insert_node(&make_node("p", "p::foo", "src/a.rs", 1, 10))
            .unwrap();
        s.insert_node(&make_node("p", "p::bar", "src/b.rs", 1, 10))
            .unwrap();

        let groups = s.detect_near_duplicates("p").unwrap();
        assert!(groups.is_empty());
    }

    #[test]
    fn test_detect_near_duplicates_finds_same_name_file_label() {
        let s = test_store();
        setup_project(&s, "p");

        // Two nodes with same name, file_path, label but different qualified_name
        s.insert_node(&Node {
            id: 0,
            project: "p".into(),
            label: "Function".into(),
            name: "foo".into(),
            qualified_name: "p::module_a::foo".into(),
            file_path: "src/a.rs".into(),
            start_line: 1,
            end_line: 10,
            properties_json: None,
        })
        .unwrap();
        s.insert_node(&Node {
            id: 0,
            project: "p".into(),
            label: "Function".into(),
            name: "foo".into(),
            qualified_name: "p::module_b::foo".into(),
            file_path: "src/a.rs".into(),
            start_line: 1,
            end_line: 10,
            properties_json: None,
        })
        .unwrap();

        let groups = s.detect_near_duplicates("p").unwrap();
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].name, "foo");
        assert_eq!(groups[0].file_path, "src/a.rs");
        assert_eq!(groups[0].label, "Function");
        assert_eq!(groups[0].discarded_ids.len(), 1);
    }

    #[test]
    fn test_detect_near_duplicates_different_labels_not_grouped() {
        let s = test_store();
        setup_project(&s, "p");

        // Same name and file_path but different labels — NOT near-duplicates
        s.insert_node(&Node {
            id: 0,
            project: "p".into(),
            label: "Function".into(),
            name: "Foo".into(),
            qualified_name: "p::Foo_fn".into(),
            file_path: "src/a.rs".into(),
            start_line: 1,
            end_line: 10,
            properties_json: None,
        })
        .unwrap();
        s.insert_node(&Node {
            id: 0,
            project: "p".into(),
            label: "Class".into(),
            name: "Foo".into(),
            qualified_name: "p::Foo_cls".into(),
            file_path: "src/a.rs".into(),
            start_line: 1,
            end_line: 10,
            properties_json: None,
        })
        .unwrap();

        let groups = s.detect_near_duplicates("p").unwrap();
        assert!(groups.is_empty());
    }

    #[test]
    fn test_detect_near_duplicates_survivor_is_smallest_id() {
        let s = test_store();
        setup_project(&s, "p");

        let id1 = s
            .insert_node(&Node {
                id: 0,
                project: "p".into(),
                label: "Function".into(),
                name: "foo".into(),
                qualified_name: "p::v1::foo".into(),
                file_path: "src/a.rs".into(),
                start_line: 1,
                end_line: 10,
                properties_json: None,
            })
            .unwrap();
        let id2 = s
            .insert_node(&Node {
                id: 0,
                project: "p".into(),
                label: "Function".into(),
                name: "foo".into(),
                qualified_name: "p::v2::foo".into(),
                file_path: "src/a.rs".into(),
                start_line: 1,
                end_line: 10,
                properties_json: None,
            })
            .unwrap();

        let groups = s.detect_near_duplicates("p").unwrap();
        assert_eq!(groups.len(), 1);
        // Survivor should be the smallest node_id (tie-breaker)
        assert_eq!(groups[0].survivor_id, id1);
        assert_eq!(groups[0].discarded_ids, vec![id2]);
    }

    #[test]
    fn test_merge_near_duplicates_redirects_edges() {
        let s = test_store();
        setup_project(&s, "p");

        let id1 = s
            .insert_node(&Node {
                id: 0,
                project: "p".into(),
                label: "Function".into(),
                name: "foo".into(),
                qualified_name: "p::v1::foo".into(),
                file_path: "src/a.rs".into(),
                start_line: 1,
                end_line: 10,
                properties_json: None,
            })
            .unwrap();
        let id2 = s
            .insert_node(&Node {
                id: 0,
                project: "p".into(),
                label: "Function".into(),
                name: "foo".into(),
                qualified_name: "p::v2::foo".into(),
                file_path: "src/a.rs".into(),
                start_line: 1,
                end_line: 10,
                properties_json: None,
            })
            .unwrap();
        let caller_id = s
            .insert_node(&Node {
                id: 0,
                project: "p".into(),
                label: "Function".into(),
                name: "caller".into(),
                qualified_name: "p::caller".into(),
                file_path: "src/b.rs".into(),
                start_line: 1,
                end_line: 5,
                properties_json: None,
            })
            .unwrap();

        // Edge from caller to the discarded node (id2)
        s.insert_edge(&Edge {
            id: 0,
            project: "p".into(),
            source_id: caller_id,
            target_id: id2,
            edge_type: "CALLS".into(),
            properties_json: None,
        })
        .unwrap();

        let merge_count = s.merge_near_duplicates("p").unwrap();
        assert_eq!(merge_count, 1);

        // The discarded node (id2) should be gone
        let schema = s.get_graph_schema("p").unwrap();
        assert_eq!(schema.total_nodes, 2); // id1 (survivor) + caller

        // The edge should now point to the survivor (id1)
        let edges = s.get_edges("p", 100).unwrap();
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].source_id, caller_id);
        assert_eq!(edges[0].target_id, id1);
    }

    #[test]
    fn test_merge_near_duplicates_removes_self_referential_edges() {
        let s = test_store();
        setup_project(&s, "p");

        let id1 = s
            .insert_node(&Node {
                id: 0,
                project: "p".into(),
                label: "Function".into(),
                name: "foo".into(),
                qualified_name: "p::v1::foo".into(),
                file_path: "src/a.rs".into(),
                start_line: 1,
                end_line: 10,
                properties_json: None,
            })
            .unwrap();
        let id2 = s
            .insert_node(&Node {
                id: 0,
                project: "p".into(),
                label: "Function".into(),
                name: "foo".into(),
                qualified_name: "p::v2::foo".into(),
                file_path: "src/a.rs".into(),
                start_line: 1,
                end_line: 10,
                properties_json: None,
            })
            .unwrap();

        // Edge from id1 (survivor) to id2 (discarded) — after redirect becomes self-referential
        s.insert_edge(&Edge {
            id: 0,
            project: "p".into(),
            source_id: id1,
            target_id: id2,
            edge_type: "CALLS".into(),
            properties_json: None,
        })
        .unwrap();

        let merge_count = s.merge_near_duplicates("p").unwrap();
        assert_eq!(merge_count, 1);

        // No edges should remain (the self-referential edge should be removed)
        let edges = s.get_edges("p", 100).unwrap();
        assert!(edges.is_empty());
    }

    #[test]
    fn test_merge_near_duplicates_handles_multiple_groups() {
        let s = test_store();
        setup_project(&s, "p");

        // Group 1: two "foo" functions in src/a.rs
        s.insert_node(&Node {
            id: 0,
            project: "p".into(),
            label: "Function".into(),
            name: "foo".into(),
            qualified_name: "p::v1::foo".into(),
            file_path: "src/a.rs".into(),
            start_line: 1,
            end_line: 10,
            properties_json: None,
        })
        .unwrap();
        s.insert_node(&Node {
            id: 0,
            project: "p".into(),
            label: "Function".into(),
            name: "foo".into(),
            qualified_name: "p::v2::foo".into(),
            file_path: "src/a.rs".into(),
            start_line: 1,
            end_line: 10,
            properties_json: None,
        })
        .unwrap();

        // Group 2: two "bar" functions in src/b.rs
        s.insert_node(&Node {
            id: 0,
            project: "p".into(),
            label: "Function".into(),
            name: "bar".into(),
            qualified_name: "p::v1::bar".into(),
            file_path: "src/b.rs".into(),
            start_line: 1,
            end_line: 5,
            properties_json: None,
        })
        .unwrap();
        s.insert_node(&Node {
            id: 0,
            project: "p".into(),
            label: "Function".into(),
            name: "bar".into(),
            qualified_name: "p::v2::bar".into(),
            file_path: "src/b.rs".into(),
            start_line: 1,
            end_line: 5,
            properties_json: None,
        })
        .unwrap();

        let merge_count = s.merge_near_duplicates("p").unwrap();
        assert_eq!(merge_count, 2);

        // Should have 2 nodes remaining (one survivor per group)
        let schema = s.get_graph_schema("p").unwrap();
        assert_eq!(schema.total_nodes, 2);
    }

    #[test]
    fn test_merge_near_duplicates_no_duplicates_returns_zero() {
        let s = test_store();
        setup_project(&s, "p");

        s.insert_node(&make_node("p", "p::foo", "src/a.rs", 1, 10))
            .unwrap();
        s.insert_node(&make_node("p", "p::bar", "src/b.rs", 1, 10))
            .unwrap();

        let merge_count = s.merge_near_duplicates("p").unwrap();
        assert_eq!(merge_count, 0);
    }

    #[test]
    fn test_merge_near_duplicates_preserves_edges_from_both_discarded() {
        let s = test_store();
        setup_project(&s, "p");

        // Three near-duplicates
        let id1 = s
            .insert_node(&Node {
                id: 0,
                project: "p".into(),
                label: "Function".into(),
                name: "foo".into(),
                qualified_name: "p::v1::foo".into(),
                file_path: "src/a.rs".into(),
                start_line: 1,
                end_line: 10,
                properties_json: None,
            })
            .unwrap();
        let id2 = s
            .insert_node(&Node {
                id: 0,
                project: "p".into(),
                label: "Function".into(),
                name: "foo".into(),
                qualified_name: "p::v2::foo".into(),
                file_path: "src/a.rs".into(),
                start_line: 1,
                end_line: 10,
                properties_json: None,
            })
            .unwrap();
        let id3 = s
            .insert_node(&Node {
                id: 0,
                project: "p".into(),
                label: "Function".into(),
                name: "foo".into(),
                qualified_name: "p::v3::foo".into(),
                file_path: "src/a.rs".into(),
                start_line: 1,
                end_line: 10,
                properties_json: None,
            })
            .unwrap();
        let caller_a = s
            .insert_node(&Node {
                id: 0,
                project: "p".into(),
                label: "Function".into(),
                name: "caller_a".into(),
                qualified_name: "p::caller_a".into(),
                file_path: "src/b.rs".into(),
                start_line: 1,
                end_line: 5,
                properties_json: None,
            })
            .unwrap();
        let caller_b = s
            .insert_node(&Node {
                id: 0,
                project: "p".into(),
                label: "Function".into(),
                name: "caller_b".into(),
                qualified_name: "p::caller_b".into(),
                file_path: "src/c.rs".into(),
                start_line: 1,
                end_line: 5,
                properties_json: None,
            })
            .unwrap();

        // caller_a → id2 (discarded)
        s.insert_edge(&Edge {
            id: 0,
            project: "p".into(),
            source_id: caller_a,
            target_id: id2,
            edge_type: "CALLS".into(),
            properties_json: None,
        })
        .unwrap();
        // caller_b → id3 (discarded)
        s.insert_edge(&Edge {
            id: 0,
            project: "p".into(),
            source_id: caller_b,
            target_id: id3,
            edge_type: "CALLS".into(),
            properties_json: None,
        })
        .unwrap();

        let merge_count = s.merge_near_duplicates("p").unwrap();
        assert_eq!(merge_count, 1);

        // Both edges should now point to survivor (id1)
        let edges = s.get_edges("p", 100).unwrap();
        assert_eq!(edges.len(), 2);
        assert!(edges.iter().all(|e| e.target_id == id1));

        // Only 3 nodes remain: survivor + caller_a + caller_b
        let schema = s.get_graph_schema("p").unwrap();
        assert_eq!(schema.total_nodes, 3);

        // Suppress unused variable warnings
        let _ = (id2, id3);
    }

    #[test]
    fn test_detect_near_duplicates_ignores_empty_file_path() {
        let s = test_store();
        setup_project(&s, "p");

        // Two nodes with same name and label but empty file_path — should NOT be grouped
        s.insert_node(&Node {
            id: 0,
            project: "p".into(),
            label: "Function".into(),
            name: "foo".into(),
            qualified_name: "p::v1::foo".into(),
            file_path: "".into(),
            start_line: 0,
            end_line: 0,
            properties_json: None,
        })
        .unwrap();
        s.insert_node(&Node {
            id: 0,
            project: "p".into(),
            label: "Function".into(),
            name: "foo".into(),
            qualified_name: "p::v2::foo".into(),
            file_path: "".into(),
            start_line: 0,
            end_line: 0,
            properties_json: None,
        })
        .unwrap();

        let groups = s.detect_near_duplicates("p").unwrap();
        assert!(groups.is_empty());
    }
}
