use crate::edge_from_row;
use crate::types::*;
use anyhow::{Context, Result};
use rusqlite::{params, Connection};

/// Extract `_confidence` and `_edge_source` from a properties JSON string.
/// Returns (confidence, edge_source, cleaned_properties) where the cleaned
/// properties have the internal fields removed.
fn extract_confidence_from_props(props_str: &str) -> (Option<f64>, Option<String>, String) {
    if let Ok(mut obj) = serde_json::from_str::<serde_json::Value>(props_str) {
        let confidence = obj.get("_confidence").and_then(|v| v.as_f64());
        let edge_source = obj
            .get("_edge_source")
            .and_then(|v| v.as_str())
            .map(|s| s.to_owned());

        // Remove internal fields from properties
        if let Some(map) = obj.as_object_mut() {
            map.remove("_confidence");
            map.remove("_edge_source");
        }

        let clean = if obj.as_object().is_none_or(|m| m.is_empty()) {
            "{}".to_owned()
        } else {
            obj.to_string()
        };

        (confidence, edge_source, clean)
    } else {
        (None, None, props_str.to_owned())
    }
}

impl crate::Store {
    // ── Edges ─────────────────────────────────────────────────

    pub fn insert_edge(&self, edge: &Edge) -> Result<i64> {
        let props_str = edge.properties_json.as_deref().unwrap_or("{}");
        let (confidence, edge_source, clean_props) = extract_confidence_from_props(props_str);
        self.conn.execute(
            "INSERT OR IGNORE INTO edges (project, source_id, target_id, type, properties, confidence, edge_source) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                edge.project,
                edge.source_id,
                edge.target_id,
                edge.edge_type,
                clean_props,
                confidence,
                edge_source,
            ],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn get_edges_by_type(&self, project: &str, edge_type: &str) -> Result<Vec<Edge>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, project, source_id, target_id, type, properties \
             FROM edges WHERE project = ?1 AND type = ?2",
        )?;
        let rows = stmt.query_map(params![project, edge_type], edge_from_row)?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }

    pub fn get_node_by_id(&self, id: i64) -> Result<Option<Node>> {
        use crate::node_from_row;
        use rusqlite::OptionalExtension;
        self.conn
            .query_row(
                "SELECT id, project, label, name, qualified_name, file_path, \
                 start_line, end_line, properties FROM nodes WHERE id = ?1",
                params![id],
                node_from_row,
            )
            .optional()
            .map_err(Into::into)
    }

    pub fn get_edges(&self, project: &str, limit: i32) -> Result<Vec<Edge>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, project, source_id, target_id, type, properties FROM edges WHERE project = ?1 LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![project, limit], edge_from_row)?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }

    pub fn find_edges_by_url_path(&self, project: &str, keyword: &str) -> Result<Vec<Edge>> {
        let pattern = format!("%{}%", keyword);
        let mut stmt = self.conn.prepare(
            "SELECT id, project, source_id, target_id, type, properties FROM edges \
             WHERE project = ?1 AND properties LIKE ?2",
        )?;
        let rows = stmt.query_map(params![project, pattern], edge_from_row)?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }

    pub fn insert_edges_batch(&self, edges: &[Edge]) -> Result<()> {
        if edges.is_empty() {
            return Ok(());
        }
        // Chunk large edge batches to avoid holding a single long transaction
        // that blocks concurrent readers (UI). 10K edges per chunk keeps each
        // transaction under ~100ms on typical hardware.
        const CHUNK_SIZE: usize = 10_000;

        for chunk in edges.chunks(CHUNK_SIZE) {
            if self.is_bulk_mode() {
                self.conn
                    .execute_batch("BEGIN IMMEDIATE")
                    .context("failed to begin immediate transaction for insert_edges_batch")?;
            } else {
                self.conn
                    .execute_batch("BEGIN")
                    .context("failed to begin transaction for insert_edges_batch")?;
            }
            let commit_result = (|| -> Result<()> {
                let mut stmt = self.conn.prepare(
                    "INSERT OR IGNORE INTO edges (project, source_id, target_id, type, properties, confidence, edge_source) \
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                )?;
                for e in chunk {
                    let props_str = e.properties_json.as_deref().unwrap_or("{}");
                    // Extract _confidence and _edge_source from properties_json
                    let (confidence, edge_source, clean_props) =
                        extract_confidence_from_props(props_str);
                    stmt.execute(params![
                        e.project,
                        e.source_id,
                        e.target_id,
                        e.edge_type,
                        clean_props,
                        confidence,
                        edge_source,
                    ])?;
                }
                Ok(())
            })();
            match commit_result {
                Ok(()) => {
                    self.conn
                        .execute_batch("COMMIT")
                        .context("failed to commit insert_edges_batch transaction")?;
                }
                Err(e) => {
                    let _ = self.conn.execute_batch("ROLLBACK");
                    return Err(e);
                }
            }
        }
        Ok(())
    }

    // ── File Hashes ───────────────────────────────────────────

    pub fn upsert_file_hash_batch(&self, hashes: &[FileHash]) -> Result<()> {
        let tx = self
            .conn
            .unchecked_transaction()
            .context("failed to begin transaction for upsert_file_hash_batch")?;
        {
            let mut stmt = tx.prepare(
                "INSERT INTO file_hashes (project, rel_path, sha256, mtime_ns, size) \
                 VALUES (?1, ?2, ?3, ?4, ?5) \
                 ON CONFLICT(project, rel_path) DO UPDATE SET sha256=?3, mtime_ns=?4, size=?5",
            )?;
            for h in hashes {
                stmt.execute(params![h.project, h.rel_path, h.sha256, h.mtime_ns, h.size])?;
            }
        }
        tx.commit()
            .context("failed to commit upsert_file_hash_batch transaction")?;
        Ok(())
    }

    /// Alias for `upsert_file_hash_batch` — persists SHA-256 hashes after successful extraction.
    /// This is the canonical name used by the incremental indexing algorithm.
    pub fn store_file_hashes_batch(&self, hashes: &[FileHash]) -> Result<()> {
        self.upsert_file_hash_batch(hashes)
    }

    /// Delete only the edges (not nodes) whose source or target belongs to the given file paths.
    /// Used in incremental mode to clear stale CALLS/IMPORTS edges for changed files
    /// while preserving File and Folder nodes (which are managed by pass_structure).
    pub fn delete_edges_for_files(&self, project: &str, file_paths: &[&str]) -> Result<usize> {
        if file_paths.is_empty() {
            return Ok(0);
        }
        let tx = self
            .conn
            .unchecked_transaction()
            .context("failed to begin transaction for delete_edges_for_files")?;
        let mut total_edges = 0usize;
        {
            let mut stmt = tx.prepare(
                "DELETE FROM edges WHERE project = ?1 AND (source_id IN \
                 (SELECT id FROM nodes WHERE project = ?1 AND file_path = ?2) \
                 OR target_id IN \
                 (SELECT id FROM nodes WHERE project = ?1 AND file_path = ?2))",
            )?;
            for fp in file_paths {
                let count = stmt.execute(params![project, fp])?;
                total_edges += count;
            }
        }
        tx.commit()
            .context("failed to commit delete_edges_for_files transaction")?;
        if total_edges > 0 {
            tracing::info!(
                count = total_edges,
                files = file_paths.len(),
                "deleted stale edges for changed files (nodes preserved)"
            );
        }
        Ok(total_edges)
    }

    /// Delete all nodes and their associated edges for the given file paths.
    /// Unlike `delete_nodes_for_changed_files`, this removes ALL nodes including
    /// File and Folder nodes. Used for files that have been deleted from disk or
    /// whose content has changed and needs full re-extraction.
    ///
    /// Since foreign keys may be disabled during bulk indexing mode, this method
    /// explicitly deletes edges referencing the affected nodes before deleting
    /// the nodes themselves.
    pub fn delete_nodes_for_files(&self, project: &str, file_paths: &[&str]) -> Result<usize> {
        if file_paths.is_empty() {
            return Ok(0);
        }
        let tx = self
            .conn
            .unchecked_transaction()
            .context("failed to begin transaction for delete_nodes_for_files")?;
        let mut total_nodes = 0usize;
        {
            // First delete edges referencing nodes in these files (handles bulk mode
            // where CASCADE is disabled due to PRAGMA foreign_keys = OFF)
            let mut edge_stmt = tx.prepare(
                "DELETE FROM edges WHERE project = ?1 AND (source_id IN \
                 (SELECT id FROM nodes WHERE project = ?1 AND file_path = ?2) \
                 OR target_id IN \
                 (SELECT id FROM nodes WHERE project = ?1 AND file_path = ?2))",
            )?;
            let mut node_stmt =
                tx.prepare("DELETE FROM nodes WHERE project = ?1 AND file_path = ?2")?;
            for fp in file_paths {
                edge_stmt.execute(params![project, fp])?;
                let count = node_stmt.execute(params![project, fp])?;
                total_nodes += count;
            }
        }
        tx.commit()
            .context("failed to commit delete_nodes_for_files transaction")?;
        if total_nodes > 0 {
            tracing::info!(
                count = total_nodes,
                files = file_paths.len(),
                "deleted all nodes and edges for removed/changed files"
            );
        }
        Ok(total_nodes)
    }

    pub fn get_file_hashes(&self, project: &str) -> Result<Vec<FileHash>> {
        let mut stmt = self.conn.prepare(
            "SELECT project, rel_path, sha256, mtime_ns, size FROM file_hashes WHERE project = ?1",
        )?;
        let rows = stmt.query_map(params![project], |row| {
            Ok(FileHash {
                project: row.get(0)?,
                rel_path: row.get(1)?,
                sha256: row.get(2)?,
                mtime_ns: row.get(3)?,
                size: row.get(4)?,
            })
        })?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }

    /// Delete all edges whose type starts with a given prefix for a project.
    /// E.g., prefix "CROSS_" deletes CROSS_HTTP, CROSS_CHANNEL, CROSS_ASYNC, etc.
    /// Returns the number of deleted edges.
    pub fn delete_edges_by_type_prefix(&self, project: &str, prefix: &str) -> Result<usize> {
        let pattern = format!("{}%", prefix);
        let count = self.conn.execute(
            "DELETE FROM edges WHERE project = ?1 AND type LIKE ?2",
            params![project, pattern],
        )?;
        Ok(count)
    }

    pub fn conn(&self) -> &Connection {
        &self.conn
    }
}
