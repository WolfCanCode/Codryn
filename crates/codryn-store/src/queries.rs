use crate::node_from_row;
use crate::types::*;
use crate::{FileDiagnostics, ImpactResult, NeighborInfo};
use anyhow::Result;
use rusqlite::{params, OptionalExtension};

const SCORE_CUTOFF: f64 = 0.3;

/// Common stop words to remove from search queries.
const STOP_WORDS: &[&str] = &[
    "a", "an", "the", "is", "are", "was", "were", "be", "been", "being", "have", "has", "had",
    "do", "does", "did", "will", "would", "could", "should", "may", "might", "shall", "can",
    "need", "must", "in", "on", "at", "to", "for", "of", "with", "by", "from", "as", "into",
    "through", "during", "before", "after", "above", "below", "and", "or", "not", "but", "if",
    "then", "else", "when", "where", "this", "that", "these", "those", "it", "its",
];

/// Remove stop words, single-character tokens, and non-alphanumeric characters
/// from a search query, returning normalized semantic keywords.
pub fn extract_semantic_keywords(query: &str) -> Vec<String> {
    query
        .split_whitespace()
        .map(|t| t.to_lowercase())
        .map(|t| {
            t.chars()
                .filter(|c| c.is_alphanumeric() || *c == '_')
                .collect::<String>()
        })
        .filter(|t| t.len() > 1)
        .filter(|t| !STOP_WORDS.contains(&t.as_str()))
        .filter(|t| !t.is_empty())
        .collect()
}

fn apply_penalties_and_cutoff(
    mut results: Vec<(crate::Node, &'static str, f64)>,
) -> Vec<(crate::Node, &'static str, f64)> {
    for (node, _, score) in &mut results {
        let fp = node.file_path.to_lowercase();
        // Test file penalty
        if fp.contains("/test/")
            || fp.contains("/tests/")
            || fp.contains(".test.")
            || fp.contains(".spec.")
            || fp.contains("_test.")
        {
            *score -= 0.4;
        }
        // Mock/fixture penalty
        if fp.contains("mock")
            || fp.contains("fixture")
            || fp.contains("fake")
            || fp.contains("stub")
        {
            *score -= 0.3;
        }
    }
    // Re-sort after penalties
    results.sort_by(|a, b| b.2.partial_cmp(&a.2).unwrap_or(std::cmp::Ordering::Equal));
    // Apply cutoff
    results.retain(|(_, _, s)| *s >= SCORE_CUTOFF);
    results
}

impl crate::Store {
    // ── Schema / Architecture ─────────────────────────────

    pub fn get_graph_schema(&self, project: &str) -> Result<SchemaInfo> {
        let mut node_labels = Vec::new();
        {
            let mut stmt = self.conn.prepare(
                "SELECT label, COUNT(*) FROM nodes WHERE project = ?1 GROUP BY label ORDER BY COUNT(*) DESC",
            )?;
            let rows = stmt.query_map(params![project], |r| {
                Ok(LabelCount {
                    label: r.get(0)?,
                    count: r.get(1)?,
                })
            })?;
            for r in rows.flatten() {
                node_labels.push(r);
            }
        }
        let mut edge_types = Vec::new();
        {
            let mut stmt = self.conn.prepare(
                "SELECT type, COUNT(*) FROM edges WHERE project = ?1 GROUP BY type ORDER BY COUNT(*) DESC",
            )?;
            let rows = stmt.query_map(params![project], |r| {
                Ok(TypeCount {
                    edge_type: r.get(0)?,
                    count: r.get(1)?,
                })
            })?;
            for r in rows.flatten() {
                edge_types.push(r);
            }
        }
        let total_nodes: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM nodes WHERE project = ?1",
            params![project],
            |r| r.get(0),
        )?;
        let total_edges: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM edges WHERE project = ?1",
            params![project],
            |r| r.get(0),
        )?;
        Ok(SchemaInfo {
            node_labels,
            edge_types,
            total_nodes,
            total_edges,
        })
    }

    pub fn get_architecture(&self, project: &str) -> Result<Vec<Node>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, project, label, name, qualified_name, file_path, start_line, end_line, properties \
             FROM nodes WHERE project = ?1 AND label IN ('Module','Package','Folder','Project') ORDER BY qualified_name",
        )?;
        let rows = stmt.query_map(params![project], node_from_row)?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }

    // ── FTS5 Code Search ─────────────────────────────────

    pub fn upsert_code_content_batch(&self, items: &[(String, String, String)]) -> Result<()> {
        if items.is_empty() {
            return Ok(());
        }
        let tx = self.conn.unchecked_transaction()?;
        {
            let mut stmt = tx.prepare("INSERT OR REPLACE INTO code_fts(project, qualified_name, content) VALUES (?1, ?2, ?3)")?;
            for (project, qn, content) in items {
                stmt.execute(params![project, qn, content])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Store code content with optional zstd compression.
    /// Writes uncompressed content to FTS5 for search, and compressed content
    /// to `code_blobs` for storage.
    pub fn upsert_code_content_compressed(&self, items: &[(String, String, String)]) -> Result<()> {
        if items.is_empty() {
            return Ok(());
        }
        let tx = self.conn.unchecked_transaction()?;
        {
            let mut fts_stmt = tx.prepare(
                "INSERT OR REPLACE INTO code_fts(project, qualified_name, content) VALUES (?1, ?2, ?3)",
            )?;
            let mut blob_stmt = tx.prepare(
                "INSERT OR REPLACE INTO code_blobs(project, qualified_name, content, is_compressed) VALUES (?1, ?2, ?3, ?4)",
            )?;
            for (project, qn, content) in items {
                // FTS5 always gets uncompressed content for search
                fts_stmt.execute(params![project, qn, content])?;
                // Blob table gets potentially compressed content for storage
                let compressed = crate::compressed_store::maybe_compress(content);
                let is_compressed = compressed.len() < content.len()
                    && content.len() >= crate::compressed_store::COMPRESSION_THRESHOLD;
                blob_stmt.execute(params![project, qn, compressed, is_compressed as i32])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Read code content from `code_blobs`, transparently decompressing if needed.
    pub fn get_code_content(&self, project: &str, qn: &str) -> Result<Option<String>> {
        let mut stmt = self.conn.prepare(
            "SELECT content, is_compressed FROM code_blobs WHERE project = ?1 AND qualified_name = ?2",
        )?;
        let result = stmt
            .query_row(params![project, qn], |row| {
                let data: Vec<u8> = row.get(0)?;
                let is_compressed: bool = row.get::<_, i32>(1)? != 0;
                Ok(if is_compressed {
                    crate::compressed_store::maybe_decompress(&data)
                } else {
                    String::from_utf8_lossy(&data).into_owned()
                })
            })
            .optional()?;
        Ok(result)
    }

    pub fn search_code_fts(&self, project: &str, query: &str, limit: i32) -> Result<Vec<Node>> {
        let clean: String = query
            .chars()
            .filter(|c| c.is_alphanumeric() || *c == '_' || *c == ' ')
            .collect();
        if clean.trim().is_empty() {
            return Ok(vec![]);
        }
        let terms: Vec<String> = clean
            .split_whitespace()
            .map(|t| format!("\"{}\"", t))
            .collect();
        let fts_query = terms.join(" AND ");
        let mut stmt = self.conn.prepare(
            "SELECT n.id, n.project, n.label, n.name, n.qualified_name, n.file_path, n.start_line, n.end_line, n.properties \
             FROM code_fts f JOIN nodes n ON n.project = f.project AND n.qualified_name = f.qualified_name \
             WHERE f.project = ?1 AND code_fts MATCH ?2 LIMIT ?3",
        )?;
        let rows = stmt.query_map(params![project, fts_query, limit], node_from_row)?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }

    /// Search code FTS with BM25 ranking.
    /// Returns nodes paired with their BM25 relevance scores.
    /// Weights: project=0.0, qualified_name=1.0, content=5.0
    pub fn search_code_fts_bm25(
        &self,
        project: &str,
        query: &str,
        limit: i32,
    ) -> Result<Vec<(Node, f64)>> {
        let keywords = extract_semantic_keywords(query);
        if keywords.is_empty() {
            return Ok(vec![]);
        }

        // Build FTS5 query with OR-joined quoted terms for broader matching
        let fts_terms: Vec<String> = keywords.iter().map(|t| format!("\"{}\"", t)).collect();
        let fts_query = fts_terms.join(" OR ");

        let mut stmt = self.conn.prepare(
            "SELECT n.id, n.project, n.label, n.name, n.qualified_name, \
             n.file_path, n.start_line, n.end_line, n.properties, \
             bm25(code_fts, 0.0, 1.0, 5.0) AS rank \
             FROM code_fts f \
             JOIN nodes n ON n.project = f.project AND n.qualified_name = f.qualified_name \
             WHERE f.project = ?1 AND code_fts MATCH ?2 \
             ORDER BY rank \
             LIMIT ?3",
        )?;

        let rows = stmt.query_map(params![project, fts_query, limit], |row| {
            let node = node_from_row(row)?;
            let rank: f64 = row.get(9)?;
            Ok((node, rank))
        })?;

        Ok(rows.filter_map(|r| r.ok()).collect())
    }

    pub fn search_nodes_broad(
        &self,
        project: &str,
        query: &str,
        label: Option<&str>,
        limit: i32,
    ) -> Result<Vec<Node>> {
        let mut seen = std::collections::HashSet::new();
        let mut results = Vec::new();
        let lim = limit as usize;

        // 1. Exact name match (highest priority)
        for n in self.search_nodes_filtered(project, query, label, limit)? {
            if seen.insert(n.id) {
                results.push(n);
            }
        }

        // 2. BM25-ranked FTS search (replaces old unranked FTS)
        if results.len() < lim {
            if let Ok(fts_results) = self.search_code_fts_bm25(project, query, limit) {
                for (n, _rank) in fts_results {
                    if results.len() >= lim {
                        break;
                    }
                    if label.is_some() && label != Some(n.label.as_str()) {
                        continue;
                    }
                    if seen.insert(n.id) {
                        results.push(n);
                    }
                }
            }
        }

        // 3. Fallback: property LIKE search
        if results.len() < lim {
            for token in query.split_whitespace() {
                if results.len() >= lim {
                    break;
                }
                let pattern = format!("%{}%", token);
                let remaining = (lim - results.len()) as i32;
                let sql = if let Some(l) = label {
                    let mut stmt = self.conn.prepare(
                        "SELECT id, project, label, name, qualified_name, file_path, start_line, end_line, properties \
                         FROM nodes WHERE project = ?1 AND label = ?2 AND properties LIKE ?3 LIMIT ?4",
                    )?;
                    let v: Vec<_> = stmt
                        .query_map(params![project, l, pattern, remaining], node_from_row)?
                        .filter_map(|r| r.ok())
                        .collect();
                    v
                } else {
                    let mut stmt = self.conn.prepare(
                        "SELECT id, project, label, name, qualified_name, file_path, start_line, end_line, properties \
                         FROM nodes WHERE project = ?1 AND properties LIKE ?2 LIMIT ?3",
                    )?;
                    let v: Vec<_> = stmt
                        .query_map(params![project, pattern, remaining], node_from_row)?
                        .filter_map(|r| r.ok())
                        .collect();
                    v
                };
                for n in sql {
                    if results.len() >= lim {
                        break;
                    }
                    if seen.insert(n.id) {
                        results.push(n);
                    }
                }
            }
        }
        Ok(results)
    }

    /// Enhanced search using BM25 ranking with the same layered approach as
    /// `search_nodes_broad`, but exposing BM25 relevance scores in results.
    /// Layer: exact name match (highest priority) → BM25-ranked FTS → property LIKE fallback.
    pub fn search_nodes_broad_bm25(
        &self,
        project: &str,
        query: &str,
        label: Option<&str>,
        limit: i32,
    ) -> Result<Vec<(Node, f64)>> {
        let mut seen = std::collections::HashSet::new();
        let mut results: Vec<(Node, f64)> = Vec::new();
        let lim = limit as usize;

        // 1. Exact name match (highest priority, score 0.0 = best for BM25)
        for n in self.search_nodes_filtered(project, query, label, limit)? {
            if seen.insert(n.id) {
                // Exact matches get the best possible score (BM25 returns negative values,
                // more negative = better; we use a very negative value for exact matches)
                results.push((n, -100.0));
            }
        }

        // 2. BM25-ranked FTS search
        if results.len() < lim {
            if let Ok(fts_results) = self.search_code_fts_bm25(project, query, limit) {
                for (n, rank) in fts_results {
                    if results.len() >= lim {
                        break;
                    }
                    if label.is_some() && label != Some(n.label.as_str()) {
                        continue;
                    }
                    if seen.insert(n.id) {
                        results.push((n, rank));
                    }
                }
            }
        }

        // 3. Fallback: property LIKE search (worst score)
        if results.len() < lim {
            for token in query.split_whitespace() {
                if results.len() >= lim {
                    break;
                }
                let pattern = format!("%{}%", token);
                let remaining = (lim - results.len()) as i32;
                let fallback_nodes = if let Some(l) = label {
                    let mut stmt = self.conn.prepare(
                        "SELECT id, project, label, name, qualified_name, file_path, start_line, end_line, properties \
                         FROM nodes WHERE project = ?1 AND label = ?2 AND properties LIKE ?3 LIMIT ?4",
                    )?;
                    let v: Vec<_> = stmt
                        .query_map(params![project, l, pattern, remaining], node_from_row)?
                        .filter_map(|r| r.ok())
                        .collect();
                    v
                } else {
                    let mut stmt = self.conn.prepare(
                        "SELECT id, project, label, name, qualified_name, file_path, start_line, end_line, properties \
                         FROM nodes WHERE project = ?1 AND properties LIKE ?2 LIMIT ?3",
                    )?;
                    let v: Vec<_> = stmt
                        .query_map(params![project, pattern, remaining], node_from_row)?
                        .filter_map(|r| r.ok())
                        .collect();
                    v
                };
                for n in fallback_nodes {
                    if results.len() >= lim {
                        break;
                    }
                    if seen.insert(n.id) {
                        // Fallback results get worst score
                        results.push((n, 0.0));
                    }
                }
            }
        }
        Ok(results)
    }

    pub fn list_symbols_in_directory(
        &self,
        project: &str,
        dir_prefix: &str,
        limit: i32,
    ) -> Result<Vec<Node>> {
        let pattern = format!("{}%", dir_prefix);
        let mut stmt = self.conn.prepare(
            "SELECT id, project, label, name, qualified_name, file_path, start_line, end_line, properties \
             FROM nodes WHERE project = ?1 AND file_path LIKE ?2 \
             AND label NOT IN ('Module', 'File', 'Folder', 'Project') ORDER BY file_path, start_line LIMIT ?3",
        )?;
        let rows = stmt.query_map(params![project, pattern, limit], node_from_row)?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }

    // ── Call Tracing ──────────────────────────────────────

    pub fn trace_calls(
        &self,
        project: &str,
        source_name: &str,
        target_name: Option<&str>,
        max_depth: i32,
    ) -> Result<Vec<(String, String, String, String)>> {
        let sources = self.search_nodes(project, source_name, 10)?;
        if sources.is_empty() {
            return Ok(vec![]);
        }
        let mut visited = std::collections::HashSet::new();
        let mut queue: std::collections::VecDeque<(i64, i32)> = std::collections::VecDeque::new();
        let mut results = Vec::new();
        for s in &sources {
            if s.name == source_name || s.name.contains(source_name) {
                queue.push_back((s.id, 0));
                visited.insert(s.id);
            }
        }
        let mut stmt = self.conn.prepare(
            "SELECT e.target_id, n.name, n.file_path, src.name, src.file_path \
             FROM edges e JOIN nodes n ON n.id = e.target_id JOIN nodes src ON src.id = e.source_id \
             WHERE e.source_id = ?1 AND e.type IN ('CALLS','ASYNC_CALLS') AND e.project = ?2",
        )?;
        while let Some((node_id, depth)) = queue.pop_front() {
            if depth >= max_depth {
                continue;
            }
            let rows: Vec<(i64, String, String, String, String)> = stmt
                .query_map(params![node_id, project], |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                    ))
                })?
                .filter_map(|r| r.ok())
                .collect();
            for (tgt_id, tgt_name, tgt_file, src_name, src_file) in rows {
                results.push((src_name.clone(), tgt_name.clone(), src_file, tgt_file));
                if let Some(target) = target_name {
                    if tgt_name == target || tgt_name.contains(target) {
                        return Ok(results);
                    }
                }
                if !visited.contains(&tgt_id) {
                    visited.insert(tgt_id);
                    queue.push_back((tgt_id, depth + 1));
                }
            }
        }
        Ok(results)
    }

    // ── Symbol Lookup (ranked) ─────────────────────────────

    pub fn find_symbol_ranked(
        &self,
        project: &str,
        query: &str,
        label: Option<&str>,
        exact: bool,
        limit: i32,
    ) -> Result<Vec<(Node, &'static str, f64)>> {
        let mut results: Vec<(Node, &'static str, f64)> = Vec::new();
        let lim = limit as usize;
        if let Some(n) = self.find_node_by_qn(project, query)? {
            if label.is_none() || label == Some(n.label.as_str()) {
                results.push((n, "exact_qualified_name", 1.0));
            }
        }
        if results.len() < lim {
            let mut stmt = self.conn.prepare(
                "SELECT id, project, label, name, qualified_name, file_path, start_line, end_line, properties \
                 FROM nodes WHERE project = ?1 AND name = ?2 LIMIT ?3",
            )?;
            let rows = stmt.query_map(params![project, query, limit], node_from_row)?;
            for n in rows.flatten() {
                if results.iter().any(|(r, _, _)| r.id == n.id) {
                    continue;
                }
                if label.is_some() && label != Some(n.label.as_str()) {
                    continue;
                }
                results.push((n, "exact_name", 0.9));
            }
        }
        if exact {
            results.truncate(lim);
            return Ok(apply_penalties_and_cutoff(results));
        }
        if results.len() < lim {
            for n in self.find_nodes_by_qn_suffix(project, query)? {
                if results.len() >= lim {
                    break;
                }
                if results.iter().any(|(r, _, _)| r.id == n.id) {
                    continue;
                }
                if label.is_some() && label != Some(n.label.as_str()) {
                    continue;
                }
                results.push((n, "suffix_match", 0.7));
            }
        }
        if results.len() < lim {
            let remaining = (lim - results.len()) as i32;
            let nodes = match label {
                Some(l) => self.search_nodes_filtered(project, query, Some(l), remaining)?,
                None => self.search_nodes(project, query, remaining)?,
            };
            for n in nodes {
                if results.len() >= lim {
                    break;
                }
                if results.iter().any(|(r, _, _)| r.id == n.id) {
                    continue;
                }
                results.push((n, "fuzzy", 0.4));
            }
        }
        results.truncate(lim);
        Ok(apply_penalties_and_cutoff(results))
    }

    // ── Symbol Neighborhood ───────────────────────────────

    pub fn node_neighbors_detailed(
        &self,
        node_id: i64,
        direction: &str,
        edge_types: Option<&[&str]>,
        limit: i32,
    ) -> Result<Vec<NeighborInfo>> {
        let mut results = Vec::new();
        if direction == "in" || direction == "both" {
            let mut stmt = self.conn.prepare(
                "SELECT n.name, n.qualified_name, n.label, n.file_path, n.start_line, e.type \
                 FROM edges e JOIN nodes n ON n.id = e.source_id WHERE e.target_id = ?1 LIMIT ?2",
            )?;
            let rows = stmt.query_map(params![node_id, limit], |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                ))
            })?;
            for r in rows.flatten() {
                let et: &String = &r.5;
                if edge_types.is_none() || edge_types.unwrap().iter().any(|t| t == et) {
                    results.push(r);
                }
            }
        }
        if direction == "out" || direction == "both" {
            let mut stmt = self.conn.prepare(
                "SELECT n.name, n.qualified_name, n.label, n.file_path, n.start_line, e.type \
                 FROM edges e JOIN nodes n ON n.id = e.target_id WHERE e.source_id = ?1 LIMIT ?2",
            )?;
            let rows = stmt.query_map(params![node_id, limit], |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                ))
            })?;
            for r in rows.flatten() {
                let et: &String = &r.5;
                if edge_types.is_none() || edge_types.unwrap().iter().any(|t| t == et) {
                    results.push(r);
                }
            }
        }
        results.truncate(limit as usize);
        Ok(results)
    }

    // ── Incoming References ───────────────────────────────

    pub fn incoming_references(
        &self,
        node_id: i64,
        edge_types: Option<&[&str]>,
        limit: i32,
    ) -> Result<Vec<(Node, String)>> {
        let mut stmt = self.conn.prepare(
            "SELECT n.id, n.project, n.label, n.name, n.qualified_name, n.file_path, n.start_line, n.end_line, n.properties, e.type \
             FROM edges e JOIN nodes n ON n.id = e.source_id WHERE e.target_id = ?1 LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![node_id, limit], |r| {
            Ok((node_from_row(r)?, r.get::<_, String>(9)?))
        })?;
        let mut results = Vec::new();
        for r in rows.flatten() {
            if let Some(types) = edge_types {
                if !types.iter().any(|t| *t == r.1) {
                    continue;
                }
            }
            results.push(r);
        }
        Ok(results)
    }

    // ── Impact Traversal (BFS) ────────────────────────────

    pub fn impact_bfs(&self, node_id: i64, max_depth: i32, limit: i32) -> Result<ImpactResult> {
        let mut visited = std::collections::HashSet::new();
        let mut queue = std::collections::VecDeque::new();
        let mut direct = Vec::new();
        let mut all = Vec::new();
        let mut files = std::collections::HashSet::new();
        visited.insert(node_id);
        queue.push_back((node_id, 0i32));
        let mut stmt = self.conn.prepare(
            "SELECT n.id, n.project, n.label, n.name, n.qualified_name, n.file_path, n.start_line, n.end_line, n.properties \
             FROM edges e JOIN nodes n ON n.id = e.source_id WHERE e.target_id = ?1",
        )?;
        while let Some((nid, depth)) = queue.pop_front() {
            if depth >= max_depth {
                continue;
            }
            if all.len() >= limit as usize {
                break;
            }
            let rows: Vec<Node> = stmt
                .query_map(params![nid], node_from_row)?
                .filter_map(|r| r.ok())
                .collect();
            for n in rows {
                if visited.contains(&n.id) {
                    continue;
                }
                visited.insert(n.id);
                if !n.file_path.is_empty() {
                    files.insert(n.file_path.clone());
                }
                if depth == 0 {
                    direct.push(n.clone());
                }
                all.push((n.clone(), depth + 1));
                queue.push_back((n.id, depth + 1));
            }
        }
        let mut file_list: Vec<String> = files.into_iter().collect();
        file_list.sort();
        Ok((direct, all, file_list))
    }

    // ── File Diagnostics / Navigation ─────────────────────

    pub fn file_diagnostics(&self, project: &str, file_path: &str) -> Result<FileDiagnostics> {
        let mut label_counts = Vec::new();
        {
            let mut stmt = self.conn.prepare("SELECT label, COUNT(*) FROM nodes WHERE project = ?1 AND file_path = ?2 GROUP BY label")?;
            let rows =
                stmt.query_map(params![project, file_path], |r| Ok((r.get(0)?, r.get(1)?)))?;
            for r in rows.flatten() {
                label_counts.push(r);
            }
        }
        let mut out_edges = Vec::new();
        {
            let mut stmt = self.conn.prepare("SELECT e.type, COUNT(*) FROM edges e JOIN nodes n ON n.id = e.source_id WHERE n.project = ?1 AND n.file_path = ?2 GROUP BY e.type")?;
            let rows =
                stmt.query_map(params![project, file_path], |r| Ok((r.get(0)?, r.get(1)?)))?;
            for r in rows.flatten() {
                out_edges.push(r);
            }
        }
        let mut in_edges = Vec::new();
        {
            let mut stmt = self.conn.prepare("SELECT e.type, COUNT(*) FROM edges e JOIN nodes n ON n.id = e.target_id WHERE n.project = ?1 AND n.file_path = ?2 GROUP BY e.type")?;
            let rows =
                stmt.query_map(params![project, file_path], |r| Ok((r.get(0)?, r.get(1)?)))?;
            for r in rows.flatten() {
                in_edges.push(r);
            }
        }
        Ok((label_counts, out_edges, in_edges))
    }

    pub fn has_file_hash(&self, project: &str, rel_path: &str) -> Result<bool> {
        let count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM file_hashes WHERE project = ?1 AND rel_path = ?2 AND is_deleted = 0",
            params![project, rel_path],
            |r| r.get(0),
        )?;
        Ok(count > 0)
    }

    /// Mark file hashes not in `live_paths` as deleted. Returns count marked.
    pub fn mark_deleted_files(&self, project: &str, live_paths: &[String]) -> Result<usize> {
        // Reset all to not-deleted first
        self.conn.execute(
            "UPDATE file_hashes SET is_deleted = 0 WHERE project = ?1",
            params![project],
        )?;
        if live_paths.is_empty() {
            // All files are deleted — remove their nodes too
            let node_count: i64 = self.conn.query_row(
                "SELECT COUNT(*) FROM nodes WHERE project = ?1 AND file_path != ''",
                params![project],
                |r| r.get(0),
            )?;
            if node_count > 0 {
                self.conn.execute(
                    "DELETE FROM nodes WHERE project = ?1 AND file_path != ''",
                    params![project],
                )?;
                tracing::info!(count = node_count, "cleaned up nodes for deleted files");
            }
            let count = self.conn.execute(
                "UPDATE file_hashes SET is_deleted = 1 WHERE project = ?1",
                params![project],
            )?;
            return Ok(count);
        }
        // Build a temp table of live paths for efficient NOT IN
        self.conn
            .execute_batch("CREATE TEMP TABLE IF NOT EXISTS _live_paths (p TEXT PRIMARY KEY)")?;
        self.conn.execute("DELETE FROM _live_paths", [])?;
        let tx = self.conn.unchecked_transaction()?;
        {
            let mut stmt = tx.prepare("INSERT OR IGNORE INTO _live_paths (p) VALUES (?1)")?;
            for p in live_paths {
                stmt.execute(params![p])?;
            }
        }
        tx.commit()?;
        let count = self.conn.execute(
            "UPDATE file_hashes SET is_deleted = 1 WHERE project = ?1 AND rel_path NOT IN (SELECT p FROM _live_paths)",
            params![project],
        )?;

        // Delete nodes whose file_path corresponds to a file no longer in the live set.
        // Only target nodes with file-like paths (containing a dot for the extension)
        // to avoid deleting Folder nodes whose file_path is a directory path.
        // This prevents stale nodes from accumulating when files are removed
        // (e.g., node_modules excluded after a discovery fix, or files deleted from disk).
        let cleaned = self.conn.execute(
            "DELETE FROM nodes WHERE project = ?1 AND file_path != '' AND file_path LIKE '%.%' AND file_path NOT IN (SELECT p FROM _live_paths)",
            params![project],
        )?;
        if cleaned > 0 {
            tracing::info!(
                count = cleaned,
                "cleaned up nodes for deleted/excluded files"
            );
        }

        // Also clean up Folder nodes that no longer have any child nodes.
        // This handles directories that became empty after file cleanup.
        let cleaned_folders = self.conn.execute(
            "DELETE FROM nodes WHERE project = ?1 AND label = 'Folder' AND id NOT IN (\
                SELECT DISTINCT source_id FROM edges WHERE project = ?1 AND type = 'CONTAINS'\
            )",
            params![project],
        )?;
        if cleaned_folders > 0 {
            tracing::info!(count = cleaned_folders, "cleaned up empty folder nodes");
        }

        self.conn.execute("DROP TABLE IF EXISTS _live_paths", [])?;
        Ok(count)
    }

    /// Delete all symbol nodes (Function, Class, Method, Interface, Module, Route, Selector)
    /// for the given file paths. Preserves File and Folder nodes since those are managed
    /// by pass_structure. This is called before re-extracting changed files so that
    /// renamed/deleted symbols don't accumulate as stale nodes.
    pub fn delete_nodes_for_changed_files(
        &self,
        project: &str,
        file_paths: &[&str],
    ) -> Result<usize> {
        if file_paths.is_empty() {
            return Ok(0);
        }
        let tx = self.conn.unchecked_transaction()?;
        let mut total = 0usize;
        {
            let mut stmt = tx.prepare(
                "DELETE FROM nodes WHERE project = ?1 AND file_path = ?2 AND label NOT IN ('File', 'Folder')",
            )?;
            for fp in file_paths {
                let count = stmt.execute(params![project, fp])?;
                total += count;
            }
        }
        tx.commit()?;
        if total > 0 {
            tracing::info!(
                count = total,
                files = file_paths.len(),
                "deleted stale nodes for changed files"
            );
        }
        Ok(total)
    }

    pub fn count_nodes_for_file(&self, project: &str, file_path: &str) -> Result<i64> {
        self.conn
            .query_row(
                "SELECT COUNT(*) FROM nodes WHERE project = ?1 AND file_path = ?2",
                params![project, file_path],
                |r| r.get(0),
            )
            .map_err(Into::into)
    }

    pub fn get_nodes_for_file(&self, project: &str, file_path: &str) -> Result<Vec<Node>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, project, label, name, qualified_name, file_path, start_line, end_line, properties \
             FROM nodes WHERE project = ?1 AND file_path = ?2 ORDER BY start_line",
        )?;
        let rows = stmt.query_map(params![project, file_path], node_from_row)?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }

    /// Get language counts for a project from File node properties.
    pub fn get_project_languages(&self, project: &str) -> Result<Vec<(String, i64)>> {
        let mut stmt = self.conn.prepare(
            "SELECT json_extract(properties, '$.language') as lang, COUNT(*) \
             FROM nodes WHERE project = ?1 AND label = 'File' AND lang IS NOT NULL \
             GROUP BY lang ORDER BY COUNT(*) DESC",
        )?;
        let rows = stmt.query_map(params![project], |r| Ok((r.get(0)?, r.get(1)?)))?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }

    pub fn get_file_edge_counts(&self, project: &str, file_path: &str) -> Result<(i64, i64)> {
        let outbound: i64 = self.conn.query_row("SELECT COUNT(*) FROM edges e JOIN nodes n ON n.id = e.source_id WHERE n.project = ?1 AND n.file_path = ?2", params![project, file_path], |r| r.get(0))?;
        let inbound: i64 = self.conn.query_row("SELECT COUNT(*) FROM edges e JOIN nodes n ON n.id = e.target_id WHERE n.project = ?1 AND n.file_path = ?2", params![project, file_path], |r| r.get(0))?;
        Ok((inbound, outbound))
    }

    #[allow(clippy::type_complexity)]
    pub fn get_edges_from_node(
        &self,
        node_id: i64,
        direction: &str,
        limit: i32,
    ) -> Result<Vec<(i64, String, String, String, String, i32, String)>> {
        let sql = if direction == "in" {
            "SELECT n.id, n.name, n.qualified_name, n.label, n.file_path, n.start_line, e.type FROM edges e JOIN nodes n ON n.id = e.source_id WHERE e.target_id = ?1 LIMIT ?2"
        } else {
            "SELECT n.id, n.name, n.qualified_name, n.label, n.file_path, n.start_line, e.type FROM edges e JOIN nodes n ON n.id = e.target_id WHERE e.source_id = ?1 LIMIT ?2"
        };
        let mut stmt = self.conn.prepare(sql)?;
        let rows = stmt.query_map(params![node_id, limit], |r| {
            Ok((
                r.get(0)?,
                r.get(1)?,
                r.get(2)?,
                r.get(3)?,
                r.get(4)?,
                r.get(5)?,
                r.get(6)?,
            ))
        })?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }

    pub fn get_nodes_by_label(&self, project: &str, label: &str, limit: i32) -> Result<Vec<Node>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, project, label, name, qualified_name, file_path, start_line, end_line, properties \
             FROM nodes WHERE project = ?1 AND label = ?2 LIMIT ?3",
        )?;
        let rows = stmt.query_map(params![project, label, limit], node_from_row)?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }

    /// Return distinct non-null `source` values from Route node properties.
    pub fn get_route_sources(&self, project: &str) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare(
            "SELECT DISTINCT json_extract(properties, '$.source') \
             FROM nodes WHERE project = ?1 AND label = 'Route' \
             AND json_extract(properties, '$.source') IS NOT NULL",
        )?;
        let rows = stmt.query_map(params![project], |r| r.get::<_, String>(0))?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }

    /// Return distinct non-null `framework` values tagged on symbol nodes.
    pub fn get_node_frameworks(&self, project: &str) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare(
            "SELECT DISTINCT json_extract(properties, '$.framework') \
             FROM nodes WHERE project = ?1 \
             AND json_extract(properties, '$.framework') IS NOT NULL",
        )?;
        let rows = stmt.query_map(params![project], |r| r.get::<_, String>(0))?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }

    /// Spring Boot routes omit `source`; other route passes set it.
    pub fn has_spring_routes(&self, project: &str) -> Result<bool> {
        let count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM nodes WHERE project = ?1 AND label = 'Route' \
             AND (properties IS NULL OR json_extract(properties, '$.source') IS NULL)",
            params![project],
            |r| r.get(0),
        )?;
        Ok(count > 0)
    }

    /// Find Route nodes with their handler, request DTO, and response DTO.
    /// Supports fuzzy scope matching across path, controller, handler, and package.
    pub fn find_routes(
        &self,
        project: &str,
        scope: Option<&str>,
        method: Option<&str>,
        limit: i32,
        include_deleted: bool,
    ) -> Result<Vec<RouteInfo>> {
        let routes = self.get_nodes_by_label(project, "Route", 1000)?;
        let mut results = Vec::new();

        for route in &routes {
            let props: serde_json::Value =
                serde_json::from_str(route.properties_json.as_deref().unwrap_or("{}"))
                    .unwrap_or_default();
            let http_method = props
                .get("http_method")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let path = props
                .get("path")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();

            if let Some(m) = method {
                if !http_method.eq_ignore_ascii_case(m) {
                    continue;
                }
            }

            // Find handler via HANDLES_ROUTE edge (inbound)
            let handler_edge = self
                .get_edges_from_node(route.id, "in", 5)
                .unwrap_or_default()
                .into_iter()
                .find(|(_, _, _, _, _, _, et)| et == "HANDLES_ROUTE");

            let (handler_name, handler_qn, handler_fp, extraction_confidence) =
                if let Some((_, name, qn, _, fp, _, _)) = &handler_edge {
                    (name.clone(), qn.clone(), fp.clone(), 1.0)
                } else if let Some(mn) = props.get("method_name").and_then(|v| v.as_str()) {
                    (
                        mn.to_string(),
                        route.qualified_name.clone(),
                        route.file_path.clone(),
                        0.7,
                    )
                } else {
                    let derived = route
                        .qualified_name
                        .rsplit('.')
                        .next()
                        .unwrap_or(&route.name);
                    (
                        derived.to_string(),
                        route.qualified_name.clone(),
                        route.file_path.clone(),
                        0.5,
                    )
                };

            // Find DTOs via ACCEPTS_DTO / RETURNS_DTO edges (outbound)
            let outbound = self
                .get_edges_from_node(route.id, "out", 10)
                .unwrap_or_default();
            let request_dto = outbound
                .iter()
                .find(|e| e.6 == "ACCEPTS_DTO")
                .map(|e| e.1.clone())
                .or_else(|| {
                    props
                        .get("request_dto_type")
                        .and_then(|v| v.as_str())
                        .map(String::from)
                });
            let response_dto = outbound
                .iter()
                .find(|e| e.6 == "RETURNS_DTO")
                .map(|e| e.1.clone())
                .or_else(|| {
                    props
                        .get("response_dto_type")
                        .and_then(|v| v.as_str())
                        .map(String::from)
                });

            let file_path = if !handler_fp.is_empty() {
                handler_fp
            } else {
                route.file_path.clone()
            };

            // Stale file filtering
            if !include_deleted
                && !file_path.is_empty()
                && !self.has_file_hash(project, &file_path).unwrap_or(true)
            {
                continue;
            }

            // Scope matching via ScopeMatchingService
            let (score, reason) = if let Some(s) = scope {
                use codryn_foundation::scope_matching::ScopeMatchingService;
                let fields = [
                    path.as_str(),
                    file_path.as_str(),
                    handler_name.as_str(),
                    route.file_path.as_str(),
                ];
                match ScopeMatchingService::score(s, &fields) {
                    Some(m) => (m.score, Some(format!("{} match", m.match_type))),
                    None => continue,
                }
            } else {
                (1.0, None)
            };

            results.push(RouteInfo {
                method: http_method,
                path,
                handler: handler_name,
                route_node_qn: route.qualified_name.clone(),
                qualified_name: handler_qn,
                file_path,
                controller: route.file_path.clone(),
                request_dto,
                response_dto,
                score,
                extraction_confidence,
                reason,
            });
        }

        results.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        results.truncate(limit as usize);
        Ok(results)
    }

    // ── Metadata Filtering ────────────────────────────────

    pub fn get_nodes_by_metadata(
        &self,
        project: &str,
        filters: &MetadataFilter,
        limit: i32,
    ) -> Result<Vec<Node>> {
        let mut sql = String::from(
            "SELECT id, project, label, name, qualified_name, file_path, start_line, end_line, properties \
             FROM nodes WHERE project = ?1",
        );
        let mut param_index = 2u32;
        let mut params_vec: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();

        if let Some(label) = &filters.label {
            sql.push_str(&format!(" AND label = ?{}", param_index));
            params_vec.push(Box::new(label.clone()));
            param_index += 1;
        }

        if let Some(is_test) = filters.is_test {
            sql.push_str(&format!(
                " AND json_extract(properties, '$.is_test') = ?{}",
                param_index
            ));
            params_vec.push(Box::new(is_test));
            param_index += 1;
        }

        if let Some(is_exported) = filters.is_exported {
            sql.push_str(&format!(
                " AND json_extract(properties, '$.is_exported') = ?{}",
                param_index
            ));
            params_vec.push(Box::new(is_exported));
            param_index += 1;
        }

        if let Some(is_entry_point) = filters.is_entry_point {
            sql.push_str(&format!(
                " AND json_extract(properties, '$.is_entry_point') = ?{}",
                param_index
            ));
            params_vec.push(Box::new(is_entry_point));
            param_index += 1;
        }

        if let Some(min_complexity) = filters.min_complexity {
            sql.push_str(&format!(
                " AND CAST(json_extract(properties, '$.complexity') AS INTEGER) >= ?{}",
                param_index
            ));
            params_vec.push(Box::new(min_complexity));
            let _ = param_index;
        }

        sql.push_str(" LIMIT ?");
        // Append limit param index at the end
        let limit_idx = 2 + params_vec.len() as u32;
        // Rewrite: use positional
        sql = sql.replacen("LIMIT ?", &format!("LIMIT ?{}", limit_idx), 1);

        let mut all_params: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
        all_params.push(Box::new(project.to_string()));
        all_params.append(&mut params_vec);
        all_params.push(Box::new(limit));

        let param_refs: Vec<&dyn rusqlite::types::ToSql> =
            all_params.iter().map(|p| p.as_ref()).collect();

        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(param_refs.as_slice(), node_from_row)?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }
}
