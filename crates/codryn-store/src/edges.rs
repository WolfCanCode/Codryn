use crate::edge_from_row;
use crate::types::*;
use anyhow::Result;
use rusqlite::{params, Connection};

impl crate::Store {
    // ── Edges ─────────────────────────────────────────────────

    pub fn insert_edge(&self, edge: &Edge) -> Result<i64> {
        self.conn.execute(
            "INSERT OR IGNORE INTO edges (project, source_id, target_id, type, properties) \
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                edge.project,
                edge.source_id,
                edge.target_id,
                edge.edge_type,
                edge.properties_json.as_deref().unwrap_or("{}")
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
        let tx = self.conn.unchecked_transaction()?;
        {
            let mut stmt = tx.prepare(
                "INSERT OR IGNORE INTO edges (project, source_id, target_id, type, properties) \
                 VALUES (?1, ?2, ?3, ?4, ?5)",
            )?;
            for e in edges {
                stmt.execute(params![
                    e.project,
                    e.source_id,
                    e.target_id,
                    e.edge_type,
                    e.properties_json.as_deref().unwrap_or("{}")
                ])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    // ── File Hashes ───────────────────────────────────────────

    pub fn upsert_file_hash_batch(&self, hashes: &[FileHash]) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
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
        tx.commit()?;
        Ok(())
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
