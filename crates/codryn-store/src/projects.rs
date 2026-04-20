use crate::node_from_row;
use crate::types::*;
use anyhow::Result;
use rusqlite::{params, OptionalExtension};

impl crate::Store {
    // ── Projects ──────────────────────────────────────────────

    pub fn upsert_project(&self, project: &Project) -> Result<()> {
        self.conn.execute(
            "INSERT INTO projects (name, indexed_at, root_path) VALUES (?1, ?2, ?3) \
             ON CONFLICT(name) DO UPDATE SET indexed_at=?2, root_path=?3",
            params![project.name, project.indexed_at, project.root_path],
        )?;
        Ok(())
    }

    pub fn list_projects(&self) -> Result<Vec<Project>> {
        let mut stmt = self
            .conn
            .prepare("SELECT name, indexed_at, root_path FROM projects")?;
        let rows = stmt.query_map([], |row| {
            Ok(Project {
                name: row.get(0)?,
                indexed_at: row.get(1)?,
                root_path: row.get(2)?,
            })
        })?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }

    pub fn delete_project(&self, name: &str) -> Result<()> {
        self.conn.execute(
            "DELETE FROM project_links WHERE source_project = ?1 OR target_project = ?1",
            params![name],
        )?;
        self.conn
            .execute("DELETE FROM edges WHERE project = ?1", params![name])?;
        self.conn
            .execute("DELETE FROM nodes WHERE project = ?1", params![name])?;
        self.conn
            .execute("DELETE FROM file_hashes WHERE project = ?1", params![name])?;
        self.conn
            .execute("DELETE FROM code_fts WHERE project = ?1", params![name])?;
        self.conn
            .execute("DELETE FROM projects WHERE name = ?1", params![name])?;
        Ok(())
    }

    pub fn delete_project_edges(&self, project: &str) -> Result<()> {
        self.conn
            .execute("DELETE FROM edges WHERE project = ?1", params![project])?;
        Ok(())
    }

    pub fn delete_project_code_fts(&self, project: &str) -> Result<()> {
        self.conn
            .execute("DELETE FROM code_fts WHERE project = ?1", params![project])?;
        Ok(())
    }

    pub fn get_all_nodes(&self, project: &str) -> Result<Vec<Node>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, project, label, name, qualified_name, file_path, start_line, end_line, properties \
             FROM nodes WHERE project = ?1",
        )?;
        let rows = stmt.query_map(params![project], node_from_row)?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }

    // ── Project Links ─────────────────────────────────────────

    pub fn link_projects(&self, a: &str, b: &str) -> Result<()> {
        let now = chrono::Utc::now().to_rfc3339();
        self.conn.execute(
            "INSERT OR IGNORE INTO project_links (source_project, target_project, created_at) VALUES (?1, ?2, ?3)",
            params![a, b, now],
        )?;
        self.conn.execute(
            "INSERT OR IGNORE INTO project_links (source_project, target_project, created_at) VALUES (?1, ?2, ?3)",
            params![b, a, now],
        )?;
        Ok(())
    }

    pub fn unlink_projects(&self, a: &str, b: &str) -> Result<()> {
        self.conn.execute(
            "DELETE FROM project_links WHERE source_project = ?1 AND target_project = ?2",
            params![a, b],
        )?;
        self.conn.execute(
            "DELETE FROM project_links WHERE source_project = ?1 AND target_project = ?2",
            params![b, a],
        )?;
        Ok(())
    }

    pub fn get_linked_projects(&self, project: &str) -> Result<Vec<ProjectLink>> {
        let mut stmt = self.conn.prepare(
            "SELECT source_project, target_project, created_at FROM project_links WHERE source_project = ?1",
        )?;
        let rows = stmt.query_map(params![project], |row| {
            Ok(ProjectLink {
                source_project: row.get(0)?,
                target_project: row.get(1)?,
                created_at: row.get(2)?,
            })
        })?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }

    // ── ADRs ──────────────────────────────────────────────

    pub fn create_adr(&self, project: &str, id: &str, title: &str, content: &str) -> Result<()> {
        let now = chrono::Utc::now().to_rfc3339();
        self.conn.execute(
            "INSERT OR REPLACE INTO decisions (id, project, title, content, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![id, project, title, content, now],
        )?;
        Ok(())
    }

    pub fn list_adrs(&self, project: &str) -> Result<Vec<Adr>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, project, title, content, created_at FROM decisions WHERE project = ?1 ORDER BY created_at",
        )?;
        let rows = stmt.query_map(params![project], |row| {
            Ok(Adr {
                id: row.get(0)?,
                project: row.get(1)?,
                title: row.get(2)?,
                content: row.get(3)?,
                created_at: row.get(4)?,
            })
        })?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }

    pub fn get_adr(&self, project: &str, id: &str) -> Result<Option<Adr>> {
        self.conn.query_row(
            "SELECT id, project, title, content, created_at FROM decisions WHERE project = ?1 AND id = ?2",
            params![project, id],
            |row| Ok(Adr { id: row.get(0)?, project: row.get(1)?, title: row.get(2)?, content: row.get(3)?, created_at: row.get(4)? }),
        ).optional().map_err(Into::into)
    }

    // ── Trace Ingestion ───────────────────────────────────

    pub fn ingest_trace(
        &self,
        project: &str,
        source_name: &str,
        target_name: &str,
        edge_type: &str,
    ) -> Result<()> {
        let mut stmt = self
            .conn
            .prepare("SELECT id FROM nodes WHERE project = ?1 AND name = ?2 LIMIT 1")?;
        let src_id: Option<i64> = stmt
            .query_row(params![project, source_name], |r| r.get(0))
            .optional()?;
        let tgt_id: Option<i64> = stmt
            .query_row(params![project, target_name], |r| r.get(0))
            .optional()?;
        if let (Some(sid), Some(tid)) = (src_id, tgt_id) {
            self.insert_edge(&Edge {
                id: 0,
                project: project.to_owned(),
                source_id: sid,
                target_id: tid,
                edge_type: edge_type.to_owned(),
                properties_json: None,
            })?;
        }
        Ok(())
    }
}
