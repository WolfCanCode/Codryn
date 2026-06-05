use anyhow::{Context, Result};
use chrono::Utc;
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::Store;

/// A compact representation of a route endpoint stored in a snapshot.
/// Used for API surface diff comparisons (Requirement 25.4).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct SnapshotRoute {
    pub method: String,
    pub path: String,
    pub handler: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_dto: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_dto: Option<String>,
}

/// A summary snapshot of the graph state at a point in time.
/// Captures node/edge counts and label distributions, linked to an optional index run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphSummarySnapshot {
    pub id: i64,
    pub project: String,
    pub index_run_id: Option<String>,
    pub timestamp: String,
    pub total_nodes: i64,
    pub total_edges: i64,
    pub label_counts: HashMap<String, i64>,
    pub edge_type_counts: HashMap<String, i64>,
    pub content_hash: String,
    /// Route endpoints captured at snapshot time for API surface diff.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub routes: Option<Vec<SnapshotRoute>>,
}

/// The difference between two graph summary snapshots.
#[derive(Debug, Serialize)]
pub struct GraphDiff {
    pub from_snapshot_id: i64,
    pub to_snapshot_id: i64,
    pub node_delta: i64,
    pub edge_delta: i64,
    pub label_changes: HashMap<String, i64>,
    pub edge_type_changes: HashMap<String, i64>,
}

/// Compute a simple deterministic content hash from graph counts.
/// Format: hex-encoded XOR-folded hash of the canonical string representation.
fn compute_content_hash(
    total_nodes: i64,
    total_edges: i64,
    label_counts: &HashMap<String, i64>,
    edge_type_counts: &HashMap<String, i64>,
) -> String {
    // Build a canonical sorted string representation
    let mut label_parts: Vec<String> = label_counts
        .iter()
        .map(|(k, v)| format!("{}={}", k, v))
        .collect();
    label_parts.sort();

    let mut edge_parts: Vec<String> = edge_type_counts
        .iter()
        .map(|(k, v)| format!("{}={}", k, v))
        .collect();
    edge_parts.sort();

    let canonical = format!(
        "nodes:{};edges:{};labels:[{}];edge_types:[{}]",
        total_nodes,
        total_edges,
        label_parts.join(","),
        edge_parts.join(",")
    );

    // Simple deterministic hash: FNV-1a 64-bit
    let mut hash: u64 = 14695981039346656037u64;
    for byte in canonical.bytes() {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(1099511628211u64);
    }
    format!("{:016x}", hash)
}

impl Store {
    /// Record a graph summary snapshot for the given project.
    /// Captures current node/edge counts, label distributions, and a content hash.
    /// Optionally links the snapshot to an index run.
    pub fn record_snapshot(
        &self,
        project: &str,
        index_run_id: Option<&str>,
    ) -> Result<GraphSummarySnapshot> {
        // Query total node count
        let total_nodes: i64 = self
            .conn
            .query_row(
                "SELECT COUNT(*) FROM nodes WHERE project = ?1",
                rusqlite::params![project],
                |row| row.get(0),
            )
            .context("failed to count nodes for snapshot")?;

        // Query total edge count
        let total_edges: i64 = self
            .conn
            .query_row(
                "SELECT COUNT(*) FROM edges WHERE project = ?1",
                rusqlite::params![project],
                |row| row.get(0),
            )
            .context("failed to count edges for snapshot")?;

        // Query per-label node counts
        let mut label_stmt = self
            .conn
            .prepare("SELECT label, COUNT(*) FROM nodes WHERE project = ?1 GROUP BY label")?;
        let label_counts: HashMap<String, i64> = label_stmt
            .query_map(rusqlite::params![project], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
            })
            .context("failed to query label counts")?
            .collect::<rusqlite::Result<HashMap<_, _>>>()
            .context("failed to collect label counts")?;

        // Query per-type edge counts
        let mut edge_type_stmt = self
            .conn
            .prepare("SELECT type, COUNT(*) FROM edges WHERE project = ?1 GROUP BY type")?;
        let edge_type_counts: HashMap<String, i64> = edge_type_stmt
            .query_map(rusqlite::params![project], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
            })
            .context("failed to query edge type counts")?
            .collect::<rusqlite::Result<HashMap<_, _>>>()
            .context("failed to collect edge type counts")?;

        // Compute content hash
        let content_hash =
            compute_content_hash(total_nodes, total_edges, &label_counts, &edge_type_counts);

        // Serialize counts to JSON
        let label_counts_json =
            serde_json::to_string(&label_counts).context("failed to serialize label counts")?;
        let edge_type_counts_json = serde_json::to_string(&edge_type_counts)
            .context("failed to serialize edge type counts")?;

        let timestamp = Utc::now().format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string();

        // Capture route data for API surface diff (Requirement 25.4)
        let routes = self.capture_snapshot_routes(project);
        let routes_json = routes
            .as_ref()
            .map(|r| serde_json::to_string(r).unwrap_or_default());

        // Insert snapshot
        self.conn
            .execute(
                "INSERT INTO _snapshots \
                 (project, index_run_id, timestamp, total_nodes, total_edges, \
                  label_counts_json, edge_type_counts_json, content_hash, routes_json) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                rusqlite::params![
                    project,
                    index_run_id,
                    timestamp,
                    total_nodes,
                    total_edges,
                    label_counts_json,
                    edge_type_counts_json,
                    content_hash,
                    routes_json,
                ],
            )
            .context("failed to insert snapshot")?;

        let id = self.conn.last_insert_rowid();

        Ok(GraphSummarySnapshot {
            id,
            project: project.to_string(),
            index_run_id: index_run_id.map(|s| s.to_string()),
            timestamp,
            total_nodes,
            total_edges,
            label_counts,
            edge_type_counts,
            content_hash,
            routes,
        })
    }

    /// List snapshots for a project, ordered by timestamp DESC (most recent first).
    pub fn list_snapshots(&self, project: &str, limit: usize) -> Result<Vec<GraphSummarySnapshot>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, project, index_run_id, timestamp, total_nodes, total_edges, \
                    label_counts_json, edge_type_counts_json, content_hash, routes_json \
             FROM _snapshots \
             WHERE project = ?1 \
             ORDER BY timestamp DESC \
             LIMIT ?2",
        )?;

        let snapshots = stmt
            .query_map(rusqlite::params![project, limit as i64], |row| {
                let label_counts_json: String = row.get(6)?;
                let edge_type_counts_json: String = row.get(7)?;
                let routes_json: Option<String> = row.get(9).unwrap_or(None);
                // Deserialize JSON — use empty map on parse failure
                let label_counts: HashMap<String, i64> =
                    serde_json::from_str(&label_counts_json).unwrap_or_default();
                let edge_type_counts: HashMap<String, i64> =
                    serde_json::from_str(&edge_type_counts_json).unwrap_or_default();
                let routes: Option<Vec<SnapshotRoute>> =
                    routes_json.and_then(|j| serde_json::from_str(&j).ok());
                Ok(GraphSummarySnapshot {
                    id: row.get(0)?,
                    project: row.get(1)?,
                    index_run_id: row.get(2)?,
                    timestamp: row.get(3)?,
                    total_nodes: row.get(4)?,
                    total_edges: row.get(5)?,
                    label_counts,
                    edge_type_counts,
                    content_hash: row.get(8)?,
                    routes,
                })
            })
            .context("failed to query snapshots")?
            .collect::<rusqlite::Result<Vec<_>>>()
            .context("failed to collect snapshots")?;

        Ok(snapshots)
    }

    /// Compute the diff between two snapshots identified by their IDs.
    /// `from_id` is the older snapshot; `to_id` is the newer snapshot.
    pub fn diff_snapshots(&self, from_id: i64, to_id: i64) -> Result<GraphDiff> {
        let from = self
            .get_snapshot_by_id(from_id)?
            .with_context(|| format!("snapshot not found: {}", from_id))?;
        let to = self
            .get_snapshot_by_id(to_id)?
            .with_context(|| format!("snapshot not found: {}", to_id))?;

        let node_delta = to.total_nodes - from.total_nodes;
        let edge_delta = to.total_edges - from.total_edges;

        // Compute label changes: for each label in either snapshot, compute delta
        let mut label_changes: HashMap<String, i64> = HashMap::new();
        for (label, &count) in &from.label_counts {
            let to_count = to.label_counts.get(label).copied().unwrap_or(0);
            let delta = to_count - count;
            if delta != 0 {
                label_changes.insert(label.clone(), delta);
            }
        }
        for (label, &count) in &to.label_counts {
            if !from.label_counts.contains_key(label) {
                // New label in `to` that didn't exist in `from`
                label_changes.insert(label.clone(), count);
            }
        }

        // Compute edge type changes
        let mut edge_type_changes: HashMap<String, i64> = HashMap::new();
        for (edge_type, &count) in &from.edge_type_counts {
            let to_count = to.edge_type_counts.get(edge_type).copied().unwrap_or(0);
            let delta = to_count - count;
            if delta != 0 {
                edge_type_changes.insert(edge_type.clone(), delta);
            }
        }
        for (edge_type, &count) in &to.edge_type_counts {
            if !from.edge_type_counts.contains_key(edge_type) {
                edge_type_changes.insert(edge_type.clone(), count);
            }
        }

        Ok(GraphDiff {
            from_snapshot_id: from_id,
            to_snapshot_id: to_id,
            node_delta,
            edge_delta,
            label_changes,
            edge_type_changes,
        })
    }

    /// Capture current route endpoints for a project as compact snapshot data.
    /// Returns None if no routes exist (avoids storing empty arrays).
    fn capture_snapshot_routes(&self, project: &str) -> Option<Vec<SnapshotRoute>> {
        // Use include_deleted=true to capture all routes regardless of file staleness
        let routes = self.find_routes(project, None, None, 500, true).ok()?;
        if routes.is_empty() {
            return None;
        }
        let snapshot_routes: Vec<SnapshotRoute> = routes
            .into_iter()
            .map(|r| SnapshotRoute {
                method: r.method.to_uppercase(),
                path: r.path,
                handler: r.handler,
                request_dto: r.request_dto,
                response_dto: r.response_dto,
            })
            .collect();
        Some(snapshot_routes)
    }

    /// Get the most recent snapshot that contains route data for a project.
    /// Used by the API surface diff feature (Requirement 25.4).
    pub fn get_latest_route_snapshot(&self, project: &str) -> Result<Option<GraphSummarySnapshot>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, project, index_run_id, timestamp, total_nodes, total_edges, \
                    label_counts_json, edge_type_counts_json, content_hash, routes_json \
             FROM _snapshots \
             WHERE project = ?1 AND routes_json IS NOT NULL \
             ORDER BY timestamp DESC \
             LIMIT 1",
        )?;

        let result = stmt
            .query_row(rusqlite::params![project], |row| {
                let label_counts_json: String = row.get(6)?;
                let edge_type_counts_json: String = row.get(7)?;
                let routes_json: Option<String> = row.get(9).unwrap_or(None);
                let label_counts: HashMap<String, i64> =
                    serde_json::from_str(&label_counts_json).unwrap_or_default();
                let edge_type_counts: HashMap<String, i64> =
                    serde_json::from_str(&edge_type_counts_json).unwrap_or_default();
                let routes: Option<Vec<SnapshotRoute>> =
                    routes_json.and_then(|j| serde_json::from_str(&j).ok());
                Ok(GraphSummarySnapshot {
                    id: row.get(0)?,
                    project: row.get(1)?,
                    index_run_id: row.get(2)?,
                    timestamp: row.get(3)?,
                    total_nodes: row.get(4)?,
                    total_edges: row.get(5)?,
                    label_counts,
                    edge_type_counts,
                    content_hash: row.get(8)?,
                    routes,
                })
            })
            .optional()
            .context("failed to get latest route snapshot")?;

        Ok(result)
    }

    /// Prune old snapshots for a project, keeping only the `retain` most recent ones.
    /// Returns the number of deleted snapshots.
    pub fn prune_old_snapshots(&self, project: &str, retain: usize) -> Result<usize> {
        // Collect all snapshot IDs ordered by timestamp DESC
        let mut stmt = self
            .conn
            .prepare("SELECT id FROM _snapshots WHERE project = ?1 ORDER BY timestamp DESC")?;
        let all_ids: Vec<i64> = stmt
            .query_map(rusqlite::params![project], |row| row.get(0))
            .context("failed to query snapshot IDs for pruning")?
            .collect::<rusqlite::Result<Vec<_>>>()
            .context("failed to collect snapshot IDs")?;

        if all_ids.len() <= retain {
            return Ok(0);
        }

        // IDs to delete: everything after the first `retain` entries
        let ids_to_delete = &all_ids[retain..];
        let mut deleted = 0usize;

        for &id in ids_to_delete {
            let rows = self
                .conn
                .execute(
                    "DELETE FROM _snapshots WHERE id = ?1",
                    rusqlite::params![id],
                )
                .context("failed to delete snapshot")?;
            deleted += rows;
        }

        Ok(deleted)
    }

    /// Fetch a single snapshot by its ID.
    fn get_snapshot_by_id(&self, id: i64) -> Result<Option<GraphSummarySnapshot>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, project, index_run_id, timestamp, total_nodes, total_edges, \
                    label_counts_json, edge_type_counts_json, content_hash, routes_json \
             FROM _snapshots \
             WHERE id = ?1",
        )?;

        let result = stmt
            .query_row(rusqlite::params![id], |row| {
                let label_counts_json: String = row.get(6)?;
                let edge_type_counts_json: String = row.get(7)?;
                let routes_json: Option<String> = row.get(9).unwrap_or(None);
                let label_counts: HashMap<String, i64> =
                    serde_json::from_str(&label_counts_json).unwrap_or_default();
                let edge_type_counts: HashMap<String, i64> =
                    serde_json::from_str(&edge_type_counts_json).unwrap_or_default();
                let routes: Option<Vec<SnapshotRoute>> =
                    routes_json.and_then(|j| serde_json::from_str(&j).ok());
                Ok(GraphSummarySnapshot {
                    id: row.get(0)?,
                    project: row.get(1)?,
                    index_run_id: row.get(2)?,
                    timestamp: row.get(3)?,
                    total_nodes: row.get(4)?,
                    total_edges: row.get(5)?,
                    label_counts,
                    edge_type_counts,
                    content_hash: row.get(8)?,
                    routes,
                })
            })
            .optional()
            .context("failed to get snapshot by id")?;

        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Edge, Node, Project, Store};

    fn test_store() -> Store {
        Store::open_in_memory().unwrap()
    }

    fn setup_project(store: &Store, project: &str) {
        store
            .upsert_project(&Project {
                name: project.to_string(),
                indexed_at: "2025-01-01".to_string(),
                root_path: "/tmp".to_string(),
            })
            .unwrap();
    }

    fn insert_node(store: &Store, project: &str, label: &str, name: &str) -> i64 {
        store
            .insert_node(&Node {
                id: 0,
                project: project.to_string(),
                label: label.to_string(),
                name: name.to_string(),
                qualified_name: format!("{}.{}", project, name),
                file_path: "src/lib.rs".to_string(),
                start_line: 1,
                end_line: 10,
                properties_json: None,
            })
            .unwrap()
    }

    fn insert_edge(store: &Store, project: &str, source_id: i64, target_id: i64, edge_type: &str) {
        store
            .insert_edge(&Edge {
                id: 0,
                project: project.to_string(),
                source_id,
                target_id,
                edge_type: edge_type.to_string(),
                properties_json: None,
            })
            .unwrap();
    }

    #[test]
    fn test_record_snapshot_captures_correct_counts() {
        let store = test_store();
        setup_project(&store, "proj");

        let n1 = insert_node(&store, "proj", "Function", "foo");
        let n2 = insert_node(&store, "proj", "Function", "bar");
        let n3 = insert_node(&store, "proj", "Class", "MyClass");
        insert_edge(&store, "proj", n1, n2, "CALLS");
        insert_edge(&store, "proj", n3, n1, "CALLS");

        let snapshot = store.record_snapshot("proj", None).unwrap();

        assert_eq!(snapshot.project, "proj");
        assert_eq!(snapshot.total_nodes, 3);
        assert_eq!(snapshot.total_edges, 2);
        assert_eq!(
            snapshot.label_counts.get("Function").copied().unwrap_or(0),
            2
        );
        assert_eq!(snapshot.label_counts.get("Class").copied().unwrap_or(0), 1);
        assert_eq!(
            snapshot.edge_type_counts.get("CALLS").copied().unwrap_or(0),
            2
        );
        assert!(snapshot.id > 0);
        assert!(!snapshot.content_hash.is_empty());
        assert!(snapshot.index_run_id.is_none());
    }

    #[test]
    fn test_record_snapshot_linked_to_index_run_id() {
        let store = test_store();
        setup_project(&store, "proj");

        let snapshot = store.record_snapshot("proj", Some("run-abc-123")).unwrap();

        assert_eq!(snapshot.index_run_id, Some("run-abc-123".to_string()));
    }

    #[test]
    fn test_record_snapshot_empty_project() {
        let store = test_store();
        setup_project(&store, "empty");

        let snapshot = store.record_snapshot("empty", None).unwrap();

        assert_eq!(snapshot.total_nodes, 0);
        assert_eq!(snapshot.total_edges, 0);
        assert!(snapshot.label_counts.is_empty());
        assert!(snapshot.edge_type_counts.is_empty());
    }

    #[test]
    fn test_list_snapshots_returns_most_recent_first() {
        let store = test_store();
        setup_project(&store, "proj");

        // Insert 3 snapshots with known timestamps
        store
            .conn
            .execute(
                "INSERT INTO _snapshots \
                 (project, index_run_id, timestamp, total_nodes, total_edges, \
                  label_counts_json, edge_type_counts_json, content_hash) \
                 VALUES ('proj', NULL, '2025-01-01T10:00:00.000Z', 10, 5, '{}', '{}', 'hash1')",
                [],
            )
            .unwrap();
        store
            .conn
            .execute(
                "INSERT INTO _snapshots \
                 (project, index_run_id, timestamp, total_nodes, total_edges, \
                  label_counts_json, edge_type_counts_json, content_hash) \
                 VALUES ('proj', NULL, '2025-01-03T10:00:00.000Z', 30, 15, '{}', '{}', 'hash3')",
                [],
            )
            .unwrap();
        store
            .conn
            .execute(
                "INSERT INTO _snapshots \
                 (project, index_run_id, timestamp, total_nodes, total_edges, \
                  label_counts_json, edge_type_counts_json, content_hash) \
                 VALUES ('proj', NULL, '2025-01-02T10:00:00.000Z', 20, 10, '{}', '{}', 'hash2')",
                [],
            )
            .unwrap();

        let snapshots = store.list_snapshots("proj", 10).unwrap();
        assert_eq!(snapshots.len(), 3);
        // Most recent first
        assert_eq!(snapshots[0].content_hash, "hash3");
        assert_eq!(snapshots[1].content_hash, "hash2");
        assert_eq!(snapshots[2].content_hash, "hash1");
    }

    #[test]
    fn test_list_snapshots_respects_limit() {
        let store = test_store();
        setup_project(&store, "proj");

        for i in 0..5 {
            store.record_snapshot("proj", None).unwrap();
            // Ensure distinct timestamps by inserting with explicit timestamps
            let _ = store.conn.execute(
                &format!(
                    "UPDATE _snapshots SET timestamp = '2025-01-0{}T10:00:00.000Z' WHERE id = (SELECT MAX(id) FROM _snapshots)",
                    i + 1
                ),
                [],
            );
        }

        let snapshots = store.list_snapshots("proj", 3).unwrap();
        assert_eq!(snapshots.len(), 3);
    }

    #[test]
    fn test_list_snapshots_filters_by_project() {
        let store = test_store();
        setup_project(&store, "proj_a");
        setup_project(&store, "proj_b");

        store.record_snapshot("proj_a", None).unwrap();
        store.record_snapshot("proj_a", None).unwrap();
        store.record_snapshot("proj_b", None).unwrap();

        let snaps_a = store.list_snapshots("proj_a", 10).unwrap();
        let snaps_b = store.list_snapshots("proj_b", 10).unwrap();

        assert_eq!(snaps_a.len(), 2);
        assert_eq!(snaps_b.len(), 1);
    }

    #[test]
    fn test_diff_snapshots_computes_deltas() {
        let store = test_store();
        setup_project(&store, "proj");

        // Snapshot 1: 2 Function nodes, 1 CALLS edge
        let n1 = insert_node(&store, "proj", "Function", "foo");
        let n2 = insert_node(&store, "proj", "Function", "bar");
        insert_edge(&store, "proj", n1, n2, "CALLS");
        let snap1 = store.record_snapshot("proj", None).unwrap();

        // Add more nodes/edges for snapshot 2
        let n3 = insert_node(&store, "proj", "Class", "MyClass");
        insert_edge(&store, "proj", n3, n1, "CALLS");
        insert_edge(&store, "proj", n3, n2, "IMPORTS");
        let snap2 = store.record_snapshot("proj", None).unwrap();

        let diff = store.diff_snapshots(snap1.id, snap2.id).unwrap();

        assert_eq!(diff.from_snapshot_id, snap1.id);
        assert_eq!(diff.to_snapshot_id, snap2.id);
        assert_eq!(diff.node_delta, 1); // +1 Class node
        assert_eq!(diff.edge_delta, 2); // +2 edges
                                        // Class label appeared: +1
        assert_eq!(diff.label_changes.get("Class").copied().unwrap_or(0), 1);
        // Function label unchanged: should not appear in changes
        assert!(!diff.label_changes.contains_key("Function"));
        // CALLS: +1 (was 1, now 2)
        assert_eq!(diff.edge_type_changes.get("CALLS").copied().unwrap_or(0), 1);
        // IMPORTS: new type, +1
        assert_eq!(
            diff.edge_type_changes.get("IMPORTS").copied().unwrap_or(0),
            1
        );
    }

    #[test]
    fn test_diff_snapshots_negative_delta() {
        let store = test_store();
        setup_project(&store, "proj");

        // Snapshot 1: 3 nodes
        let n1 = insert_node(&store, "proj", "Function", "foo");
        let n2 = insert_node(&store, "proj", "Function", "bar");
        let n3 = insert_node(&store, "proj", "Function", "baz");
        let snap1 = store.record_snapshot("proj", None).unwrap();

        // Delete a node and take snapshot 2
        store
            .conn
            .execute("DELETE FROM nodes WHERE id = ?1", rusqlite::params![n3])
            .unwrap();
        let snap2 = store.record_snapshot("proj", None).unwrap();

        let diff = store.diff_snapshots(snap1.id, snap2.id).unwrap();
        assert_eq!(diff.node_delta, -1);
        // Function count went from 3 to 2: delta = -1
        assert_eq!(diff.label_changes.get("Function").copied().unwrap_or(0), -1);

        // Suppress unused variable warnings
        let _ = n1;
        let _ = n2;
    }

    #[test]
    fn test_diff_snapshots_nonexistent_id_errors() {
        let store = test_store();
        setup_project(&store, "proj");

        let snap = store.record_snapshot("proj", None).unwrap();
        let result = store.diff_snapshots(snap.id, 9999);
        assert!(result.is_err());
    }

    #[test]
    fn test_prune_old_snapshots_respects_retention() {
        let store = test_store();
        setup_project(&store, "proj");

        // Insert 5 snapshots with distinct timestamps
        for i in 1..=5 {
            store
                .conn
                .execute(
                    &format!(
                        "INSERT INTO _snapshots \
                         (project, index_run_id, timestamp, total_nodes, total_edges, \
                          label_counts_json, edge_type_counts_json, content_hash) \
                         VALUES ('proj', NULL, '2025-01-0{}T10:00:00.000Z', {}, 0, '{{}}', '{{}}', 'hash{}')",
                        i, i * 10, i
                    ),
                    [],
                )
                .unwrap();
        }

        let deleted = store.prune_old_snapshots("proj", 3).unwrap();
        assert_eq!(deleted, 2);

        let remaining = store.list_snapshots("proj", 10).unwrap();
        assert_eq!(remaining.len(), 3);
        // The 3 most recent should remain (timestamps 5, 4, 3)
        assert_eq!(remaining[0].content_hash, "hash5");
        assert_eq!(remaining[1].content_hash, "hash4");
        assert_eq!(remaining[2].content_hash, "hash3");
    }

    #[test]
    fn test_prune_old_snapshots_noop_when_under_limit() {
        let store = test_store();
        setup_project(&store, "proj");

        store.record_snapshot("proj", None).unwrap();
        store.record_snapshot("proj", None).unwrap();

        let deleted = store.prune_old_snapshots("proj", 10).unwrap();
        assert_eq!(deleted, 0);

        let remaining = store.list_snapshots("proj", 10).unwrap();
        assert_eq!(remaining.len(), 2);
    }

    #[test]
    fn test_prune_old_snapshots_only_affects_target_project() {
        let store = test_store();
        setup_project(&store, "proj_a");
        setup_project(&store, "proj_b");

        for i in 1..=5 {
            store
                .conn
                .execute(
                    &format!(
                        "INSERT INTO _snapshots \
                         (project, index_run_id, timestamp, total_nodes, total_edges, \
                          label_counts_json, edge_type_counts_json, content_hash) \
                         VALUES ('proj_a', NULL, '2025-01-0{}T10:00:00.000Z', 0, 0, '{{}}', '{{}}', 'a{}')",
                        i, i
                    ),
                    [],
                )
                .unwrap();
        }
        store.record_snapshot("proj_b", None).unwrap();

        store.prune_old_snapshots("proj_a", 2).unwrap();

        let remaining_a = store.list_snapshots("proj_a", 10).unwrap();
        let remaining_b = store.list_snapshots("proj_b", 10).unwrap();

        assert_eq!(remaining_a.len(), 2);
        assert_eq!(remaining_b.len(), 1); // unaffected
    }

    #[test]
    fn test_content_hash_is_deterministic() {
        let mut label_counts = HashMap::new();
        label_counts.insert("Function".to_string(), 10i64);
        label_counts.insert("Class".to_string(), 5i64);

        let mut edge_type_counts = HashMap::new();
        edge_type_counts.insert("CALLS".to_string(), 20i64);

        let h1 = compute_content_hash(15, 20, &label_counts, &edge_type_counts);
        let h2 = compute_content_hash(15, 20, &label_counts, &edge_type_counts);
        assert_eq!(h1, h2);
    }

    #[test]
    fn test_content_hash_differs_for_different_counts() {
        let label_counts = HashMap::new();
        let edge_type_counts = HashMap::new();

        let h1 = compute_content_hash(10, 5, &label_counts, &edge_type_counts);
        let h2 = compute_content_hash(11, 5, &label_counts, &edge_type_counts);
        assert_ne!(h1, h2);
    }
}
