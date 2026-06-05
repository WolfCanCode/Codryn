use crate::node_from_row;
use crate::types::*;
use anyhow::{Context, Result};
use rusqlite::{params, OptionalExtension};
use std::collections::HashMap;

impl crate::Store {
    // ── Nodes ─────────────────────────────────────────────────

    pub fn insert_node(&self, node: &Node) -> Result<i64> {
        self.conn.execute(
            "INSERT OR IGNORE INTO nodes (project, label, name, qualified_name, file_path, start_line, end_line, properties) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                node.project, node.label, node.name, node.qualified_name,
                node.file_path, node.start_line, node.end_line,
                node.properties_json.as_deref().unwrap_or("{}")
            ],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn find_node_by_qn(&self, project: &str, qn: &str) -> Result<Option<Node>> {
        self.conn
            .query_row(
                "SELECT id, project, label, name, qualified_name, file_path, start_line, end_line, properties \
                 FROM nodes WHERE project = ?1 AND qualified_name = ?2",
                params![project, qn],
                node_from_row,
            )
            .optional()
            .map_err(Into::into)
    }

    pub fn find_nodes_by_file_overlap(
        &self,
        project: &str,
        file_path: &str,
        start: i32,
        end: i32,
    ) -> Result<Vec<Node>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, project, label, name, qualified_name, file_path, start_line, end_line, properties \
             FROM nodes WHERE project = ?1 AND file_path = ?2 AND start_line <= ?4 AND end_line >= ?3 \
             AND label NOT IN ('Module','Package')",
        )?;
        let rows = stmt.query_map(params![project, file_path, start, end], node_from_row)?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }

    pub fn find_nodes_by_qn_suffix(&self, project: &str, suffix: &str) -> Result<Vec<Node>> {
        let pattern = format!("%.{}", suffix);
        let mut stmt = self.conn.prepare(
            "SELECT id, project, label, name, qualified_name, file_path, start_line, end_line, properties \
             FROM nodes WHERE project = ?1 AND (qualified_name LIKE ?2 OR qualified_name = ?3)",
        )?;
        let rows = stmt.query_map(params![project, pattern, suffix], node_from_row)?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }

    pub fn search_nodes(&self, project: &str, query: &str, limit: i32) -> Result<Vec<Node>> {
        let tokens: Vec<&str> = query.split_whitespace().collect();
        if tokens.len() <= 1 {
            let pattern = format!("%{}%", query);
            let mut stmt = self.conn.prepare(
                "SELECT id, project, label, name, qualified_name, file_path, start_line, end_line, properties \
                 FROM nodes WHERE project = ?1 AND (name LIKE ?2 OR qualified_name LIKE ?2) LIMIT ?3",
            )?;
            let rows = stmt.query_map(params![project, pattern, limit], node_from_row)?;
            return Ok(rows.filter_map(|r| r.ok()).collect());
        }
        let mut sql = String::from(
            "SELECT id, project, label, name, qualified_name, file_path, start_line, end_line, properties \
             FROM nodes WHERE project = ?1",
        );
        let mut param_values: Vec<String> = vec![project.to_owned()];
        for (i, token) in tokens.iter().enumerate() {
            let idx = i + 2;
            sql.push_str(&format!(
                " AND (name LIKE ?{idx} COLLATE NOCASE OR qualified_name LIKE ?{idx} COLLATE NOCASE)"
            ));
            param_values.push(format!("%{}%", token));
        }
        sql.push_str(&format!(" LIMIT ?{}", tokens.len() + 2));
        param_values.push(limit.to_string());
        let mut stmt = self.conn.prepare(&sql)?;
        let params: Vec<&dyn rusqlite::types::ToSql> = param_values
            .iter()
            .map(|v| v as &dyn rusqlite::types::ToSql)
            .collect();
        let rows = stmt.query_map(&*params, node_from_row)?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }

    pub fn node_degree(&self, node_id: i64) -> Result<(i32, i32)> {
        let in_deg: i32 = self.conn.query_row(
            "SELECT COUNT(*) FROM edges WHERE target_id = ?1",
            params![node_id],
            |r| r.get(0),
        )?;
        let out_deg: i32 = self.conn.query_row(
            "SELECT COUNT(*) FROM edges WHERE source_id = ?1",
            params![node_id],
            |r| r.get(0),
        )?;
        Ok((in_deg, out_deg))
    }

    pub fn node_neighbor_names(
        &self,
        node_id: i64,
        limit: i32,
    ) -> Result<(Vec<String>, Vec<String>)> {
        let mut callers = Vec::new();
        let mut callees = Vec::new();
        {
            let mut stmt = self.conn.prepare(
                "SELECT n.name FROM edges e JOIN nodes n ON n.id = e.source_id \
                 WHERE e.target_id = ?1 AND e.type IN ('CALLS','HTTP_CALLS','ASYNC_CALLS','USES') LIMIT ?2",
            )?;
            let rows = stmt.query_map(params![node_id, limit], |r| r.get::<_, String>(0))?;
            for r in rows.flatten() {
                callers.push(r);
            }
        }
        {
            let mut stmt = self.conn.prepare(
                "SELECT n.name FROM edges e JOIN nodes n ON n.id = e.target_id \
                 WHERE e.source_id = ?1 AND e.type IN ('CALLS','HTTP_CALLS','ASYNC_CALLS','USES') LIMIT ?2",
            )?;
            let rows = stmt.query_map(params![node_id, limit], |r| r.get::<_, String>(0))?;
            for r in rows.flatten() {
                callees.push(r);
            }
        }
        Ok((callers, callees))
    }

    pub fn list_files(&self, project: &str) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare(
            "SELECT DISTINCT file_path FROM nodes WHERE project = ?1 AND file_path != '' ORDER BY file_path",
        )?;
        let rows = stmt.query_map(params![project], |r| r.get::<_, String>(0))?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }

    pub fn insert_nodes_batch(&self, nodes: &[Node]) -> Result<Vec<(String, i64)>> {
        if nodes.is_empty() {
            return Ok(vec![]);
        }
        if self.is_bulk_mode() {
            self.conn
                .execute_batch("BEGIN IMMEDIATE")
                .context("failed to begin immediate transaction for insert_nodes_batch")?;
        } else {
            self.conn
                .execute_batch("BEGIN")
                .context("failed to begin transaction for insert_nodes_batch")?;
        }
        let mut results = Vec::with_capacity(nodes.len());
        let commit_result = (|| -> Result<()> {
            let mut insert = self.conn.prepare(
                "INSERT OR IGNORE INTO nodes (project, label, name, qualified_name, file_path, start_line, end_line, properties) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8) RETURNING id",
            )?;
            let mut select = self
                .conn
                .prepare("SELECT id FROM nodes WHERE project = ?1 AND qualified_name = ?2")?;
            for n in nodes {
                let id: i64 = match insert.query_row(
                    params![
                        n.project,
                        n.label,
                        n.name,
                        n.qualified_name,
                        n.file_path,
                        n.start_line,
                        n.end_line,
                        n.properties_json.as_deref().unwrap_or("{}")
                    ],
                    |r| r.get(0),
                ) {
                    Ok(id) => id,
                    Err(_) => {
                        select.query_row(params![n.project, n.qualified_name], |r| r.get(0))?
                    }
                };
                results.push((n.qualified_name.clone(), id));
            }
            Ok(())
        })();
        match commit_result {
            Ok(()) => {
                self.conn
                    .execute_batch("COMMIT")
                    .context("failed to commit insert_nodes_batch transaction")?;
                Ok(results)
            }
            Err(e) => {
                let _ = self.conn.execute_batch("ROLLBACK");
                Err(e)
            }
        }
    }

    pub fn search_nodes_filtered(
        &self,
        project: &str,
        query: &str,
        label: Option<&str>,
        limit: i32,
    ) -> Result<Vec<Node>> {
        match label {
            Some(l) => {
                let tokens: Vec<&str> = query.split_whitespace().collect();
                if tokens.len() <= 1 {
                    let pattern = format!("%{}%", query);
                    let mut stmt = self.conn.prepare(
                        "SELECT id, project, label, name, qualified_name, file_path, start_line, end_line, properties \
                         FROM nodes WHERE project = ?1 AND label = ?2 AND (name LIKE ?3 OR qualified_name LIKE ?3) LIMIT ?4",
                    )?;
                    let rows =
                        stmt.query_map(params![project, l, pattern, limit], node_from_row)?;
                    Ok(rows.filter_map(|r| r.ok()).collect())
                } else {
                    let mut sql = String::from(
                        "SELECT id, project, label, name, qualified_name, file_path, start_line, end_line, properties \
                         FROM nodes WHERE project = ?1 AND label = ?2",
                    );
                    let mut pv: Vec<String> = vec![project.to_owned(), l.to_owned()];
                    for (i, tok) in tokens.iter().enumerate() {
                        let idx = i + 3;
                        sql.push_str(&format!(" AND (name LIKE ?{idx} COLLATE NOCASE OR qualified_name LIKE ?{idx} COLLATE NOCASE)"));
                        pv.push(format!("%{}%", tok));
                    }
                    sql.push_str(&format!(" LIMIT ?{}", tokens.len() + 3));
                    pv.push(limit.to_string());
                    let mut stmt = self.conn.prepare(&sql)?;
                    let params: Vec<&dyn rusqlite::types::ToSql> = pv
                        .iter()
                        .map(|v| v as &dyn rusqlite::types::ToSql)
                        .collect();
                    let rows = stmt.query_map(&*params, node_from_row)?;
                    Ok(rows.filter_map(|r| r.ok()).collect())
                }
            }
            None => self.search_nodes(project, query, limit),
        }
    }

    pub fn find_node_by_property(
        &self,
        project: &str,
        key: &str,
        value: &str,
    ) -> Result<Option<Node>> {
        let pattern = format!("%\"{}\":\"{}\"%", key, value);
        let mut stmt = self.conn.prepare(
            "SELECT id, project, label, name, qualified_name, file_path, start_line, end_line, properties \
             FROM nodes WHERE project = ?1 AND properties LIKE ?2 LIMIT 1",
        )?;
        let mut rows = stmt.query_map(params![project, pattern], node_from_row)?;
        Ok(rows.next().and_then(|r| r.ok()))
    }

    pub fn find_matching_symbols_across_projects(
        &self,
        project_a: &str,
        project_b: &str,
    ) -> Result<Vec<(Node, Node)>> {
        let mut stmt = self.conn.prepare(
            "SELECT a.id, a.project, a.label, a.name, a.qualified_name, a.file_path, a.start_line, a.end_line, a.properties, \
                    b.id, b.project, b.label, b.name, b.qualified_name, b.file_path, b.start_line, b.end_line, b.properties \
             FROM nodes a JOIN nodes b ON a.name = b.name \
             WHERE a.project = ?1 AND b.project = ?2 \
             AND a.label IN ('Class', 'Interface') AND b.label IN ('Class', 'Interface')",
        )?;
        let rows = stmt.query_map(params![project_a, project_b], |row| {
            let a = Node {
                id: row.get(0)?,
                project: row.get(1)?,
                label: row.get(2)?,
                name: row.get(3)?,
                qualified_name: row.get(4)?,
                file_path: row.get(5)?,
                start_line: row.get(6)?,
                end_line: row.get(7)?,
                properties_json: row.get(8)?,
            };
            let b = Node {
                id: row.get(9)?,
                project: row.get(10)?,
                label: row.get(11)?,
                name: row.get(12)?,
                qualified_name: row.get(13)?,
                file_path: row.get(14)?,
                start_line: row.get(15)?,
                end_line: row.get(16)?,
                properties_json: row.get(17)?,
            };
            Ok((a, b))
        })?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }

    pub fn update_node_properties(&self, node_id: i64, properties: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE nodes SET properties = ?1 WHERE id = ?2",
            params![properties, node_id],
        )?;
        Ok(())
    }

    /// Compute fan-in and fan-out for all nodes in a project in two bulk queries.
    /// Returns a map of node_id -> (fan_in, fan_out).
    pub fn node_degrees_bulk(
        &self,
        project: &str,
    ) -> Result<std::collections::HashMap<i64, (i32, i32)>> {
        let mut degrees: std::collections::HashMap<i64, (i32, i32)> =
            std::collections::HashMap::new();

        // Fan-in: count incoming edges per target_id
        let mut stmt = self.conn.prepare(
            "SELECT target_id, COUNT(*) FROM edges WHERE project = ?1 GROUP BY target_id",
        )?;
        let rows = stmt.query_map(params![project], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, i32>(1)?))
        })?;
        for row in rows {
            let (id, count) = row?;
            degrees.entry(id).or_insert((0, 0)).0 = count;
        }

        // Fan-out: count outgoing edges per source_id
        let mut stmt = self.conn.prepare(
            "SELECT source_id, COUNT(*) FROM edges WHERE project = ?1 GROUP BY source_id",
        )?;
        let rows = stmt.query_map(params![project], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, i32>(1)?))
        })?;
        for row in rows {
            let (id, count) = row?;
            degrees.entry(id).or_insert((0, 0)).1 = count;
        }

        Ok(degrees)
    }

    /// Batch update properties for multiple nodes in a single transaction.
    pub fn update_node_properties_batch(&self, updates: &[(i64, String)]) -> Result<()> {
        if updates.is_empty() {
            return Ok(());
        }
        let tx = self
            .conn
            .unchecked_transaction()
            .context("failed to begin transaction for update_node_properties_batch")?;
        {
            let mut stmt = tx.prepare("UPDATE nodes SET properties = ?1 WHERE id = ?2")?;
            for (node_id, properties) in updates {
                stmt.execute(params![properties, node_id])?;
            }
        }
        tx.commit()
            .context("failed to commit update_node_properties_batch transaction")?;
        Ok(())
    }

    pub fn find_nodes_by_name(&self, project: &str, name: &str, limit: i32) -> Result<Vec<Node>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, project, label, name, qualified_name, file_path, start_line, end_line, properties \
             FROM nodes WHERE project = ?1 AND name = ?2 LIMIT ?3",
        )?;
        let rows = stmt.query_map(params![project, name, limit], node_from_row)?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }

    /// Resolve multiple QN suffixes in a single query using a temp table.
    /// Creates a temp table of suffixes, JOINs against nodes, and returns
    /// matches. Handles ambiguous matches by preferring exact name matches.
    pub fn resolve_qns_suffix_batch(
        &self,
        project: &str,
        suffixes: &[&str],
    ) -> Result<HashMap<String, i64>> {
        if suffixes.is_empty() {
            return Ok(HashMap::new());
        }

        // Create temp table
        self.conn
            .execute_batch("CREATE TEMP TABLE IF NOT EXISTS _batch_suffixes (suffix TEXT NOT NULL)")
            .context("failed to create temp table _batch_suffixes")?;

        // Clear any previous data
        self.conn
            .execute("DELETE FROM _batch_suffixes", [])
            .context("failed to clear _batch_suffixes")?;

        // Insert all suffixes into the temp table (chunked for safety)
        {
            let mut insert_stmt = self
                .conn
                .prepare("INSERT INTO _batch_suffixes (suffix) VALUES (?1)")
                .context("failed to prepare insert into _batch_suffixes")?;
            for suffix in suffixes {
                insert_stmt.execute(params![suffix])?;
            }
        }

        // JOIN against nodes: find candidates where node name matches the suffix
        // or qualified_name ends with '.suffix'
        let mut stmt = self.conn.prepare(
            "SELECT s.suffix, n.id, n.name, n.qualified_name \
             FROM _batch_suffixes s \
             JOIN nodes n ON n.project = ?1 \
               AND (n.name = s.suffix OR n.qualified_name LIKE '%.' || s.suffix)",
        )?;

        // Collect all candidates grouped by suffix
        let rows = stmt.query_map(params![project], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
            ))
        })?;

        // Group candidates by suffix
        let mut candidates: HashMap<String, Vec<(i64, String, String)>> = HashMap::new();
        for row in rows {
            let (suffix, id, name, qn) =
                row.context("failed to read row in resolve_qns_suffix_batch")?;
            candidates.entry(suffix).or_default().push((id, name, qn));
        }

        // Resolve ambiguous matches:
        // 1. Prefer exact name match (node.name == suffix)
        // 2. If still ambiguous, take the first candidate (lowest id)
        let mut result = HashMap::with_capacity(candidates.len());
        for (suffix, mut cands) in candidates {
            if cands.is_empty() {
                continue;
            }
            if cands.len() == 1 {
                result.insert(suffix, cands[0].0);
            } else {
                // Prefer exact name matches
                let exact: Vec<&(i64, String, String)> = cands
                    .iter()
                    .filter(|(_, name, _)| *name == suffix)
                    .collect();
                if exact.len() == 1 {
                    result.insert(suffix, exact[0].0);
                } else if !exact.is_empty() {
                    // Multiple exact name matches — take lowest id
                    let min_id = exact.iter().map(|(id, _, _)| *id).min().unwrap();
                    result.insert(suffix, min_id);
                } else {
                    // No exact name match — sort by id and take lowest
                    cands.sort_by_key(|(id, _, _)| *id);
                    result.insert(suffix, cands[0].0);
                }
            }
        }

        // Clean up temp table
        let _ = self.conn.execute("DELETE FROM _batch_suffixes", []);

        Ok(result)
    }

    /// Resolve multiple qualified names to node IDs in a single query.
    /// Uses `WHERE qualified_name IN (?, ?, ...)` with chunking for
    /// SQLite's variable limit (999 per query).
    pub fn resolve_qns_batch(&self, project: &str, qns: &[&str]) -> Result<HashMap<String, i64>> {
        if qns.is_empty() {
            return Ok(HashMap::new());
        }

        let mut result = HashMap::with_capacity(qns.len());

        // Chunk at 500 to stay well under SQLite's 999 variable limit
        // (1 variable used for project + up to 500 for the IN clause = 501 max)
        for chunk in qns.chunks(500) {
            let placeholders: String = (0..chunk.len())
                .map(|i| format!("?{}", i + 2))
                .collect::<Vec<_>>()
                .join(", ");

            let sql = format!(
                "SELECT qualified_name, id FROM nodes WHERE project = ?1 AND qualified_name IN ({})",
                placeholders
            );

            let mut stmt = self.conn.prepare(&sql)?;

            let mut params_vec: Vec<&dyn rusqlite::types::ToSql> =
                Vec::with_capacity(chunk.len() + 1);
            params_vec.push(&project as &dyn rusqlite::types::ToSql);
            for qn in chunk {
                params_vec.push(qn as &dyn rusqlite::types::ToSql);
            }

            let rows = stmt.query_map(&*params_vec, |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
            })?;

            for row in rows {
                let (qn, id) = row.context("failed to read row in resolve_qns_batch")?;
                result.insert(qn, id);
            }
        }

        Ok(result)
    }
}
