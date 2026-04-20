use anyhow::Result;
use codryn_store::{Edge, Node, Store};
use std::collections::HashMap;

/// In-memory staging buffer for nodes and edges before flushing to the store.
pub struct GraphBuffer {
    project: String,
    nodes: Vec<Node>,
    edges: Vec<Edge>,
    /// Maps qualified_name -> node_id after flush
    qn_to_id: HashMap<String, i64>,
    /// Code content for FTS indexing: (project, qualified_name, content)
    code_snippets: Vec<(String, String, String)>,
}

impl GraphBuffer {
    pub fn new(project: &str) -> Self {
        Self {
            project: project.to_owned(),
            nodes: Vec::new(),
            edges: Vec::new(),
            qn_to_id: HashMap::new(),
            code_snippets: Vec::new(),
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn add_node(
        &mut self,
        label: &str,
        name: &str,
        qualified_name: &str,
        file_path: &str,
        start_line: i32,
        end_line: i32,
        properties_json: Option<String>,
    ) {
        self.nodes.push(Node {
            id: 0,
            project: self.project.clone(),
            label: label.to_owned(),
            name: name.to_owned(),
            qualified_name: qualified_name.to_owned(),
            file_path: file_path.to_owned(),
            start_line,
            end_line,
            properties_json,
        });
    }

    /// Queue an edge. source/target are qualified names resolved at flush time.
    pub fn add_edge_by_qn(
        &mut self,
        source_qn: &str,
        target_qn: &str,
        edge_type: &str,
        properties_json: Option<String>,
    ) {
        // Store with source_id/target_id = 0, resolve at flush
        self.edges.push(Edge {
            id: 0,
            project: self.project.clone(),
            source_id: 0,
            target_id: 0,
            edge_type: edge_type.to_owned(),
            properties_json: Some(
                serde_json::json!({
                    "_src_qn": source_qn,
                    "_tgt_qn": target_qn,
                    "_props": properties_json,
                })
                .to_string(),
            ),
        });
    }

    /// Add an edge with already-resolved IDs.
    pub fn add_edge(
        &mut self,
        source_id: i64,
        target_id: i64,
        edge_type: &str,
        properties_json: Option<String>,
    ) {
        self.edges.push(Edge {
            id: 0,
            project: self.project.clone(),
            source_id,
            target_id,
            edge_type: edge_type.to_owned(),
            properties_json,
        });
    }

    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }
    pub fn edge_count(&self) -> usize {
        self.edges.len()
    }

    pub fn get_node_id(&self, qn: &str) -> Option<i64> {
        self.qn_to_id.get(qn).copied()
    }

    pub fn add_code_content(&mut self, qualified_name: &str, content: &str) {
        self.code_snippets.push((
            self.project.clone(),
            qualified_name.to_owned(),
            content.to_owned(),
        ));
    }

    /// Seed qn_to_id from all existing nodes in the store (needed for incremental reindex).
    pub fn seed_ids_from_store(&mut self, store: &Store) -> Result<()> {
        for node in store.get_all_nodes(&self.project)? {
            self.qn_to_id.insert(node.qualified_name, node.id);
        }
        Ok(())
    }

    /// Take all buffered edges out, leaving the buffer empty.
    pub fn take_edges(&mut self) -> Vec<Edge> {
        std::mem::take(&mut self.edges)
    }

    /// Put edges back into the buffer.
    pub fn restore_edges(&mut self, edges: Vec<Edge>) {
        self.edges = edges;
    }

    /// Flush all buffered nodes and edges to the store.
    pub fn flush(&mut self, store: &Store) -> Result<()> {
        // Insert nodes
        let results = store.insert_nodes_batch(&self.nodes)?;
        for (qn, id) in &results {
            self.qn_to_id.insert(qn.clone(), *id);
        }

        // Collect all unresolved QNs so we can do a single store lookup
        let mut missing_qns: std::collections::HashSet<String> = std::collections::HashSet::new();
        for e in &self.edges {
            if e.source_id == 0 || e.target_id == 0 {
                if let Some(ref props_str) = e.properties_json {
                    if let Ok(v) = serde_json::from_str::<serde_json::Value>(props_str) {
                        let src_qn = v["_src_qn"].as_str().unwrap_or("").to_owned();
                        let tgt_qn = v["_tgt_qn"].as_str().unwrap_or("").to_owned();
                        if !self.qn_to_id.contains_key(&src_qn) {
                            missing_qns.insert(src_qn);
                        }
                        if !self.qn_to_id.contains_key(&tgt_qn) {
                            missing_qns.insert(tgt_qn);
                        }
                    }
                }
            }
        }
        // Resolve missing QNs from store — try exact match first, then suffix match
        for qn in &missing_qns {
            if let Ok(Some(node)) = store.find_node_by_qn(&self.project, qn) {
                self.qn_to_id.insert(qn.clone(), node.id);
            } else {
                // Suffix match: "project.ClassName" -> find node whose QN ends with ".ClassName"
                let suffix = qn.rsplit('.').next().unwrap_or(qn);
                if !suffix.is_empty() {
                    let candidates = store
                        .find_nodes_by_qn_suffix(&self.project, suffix)
                        .unwrap_or_default();
                    if candidates.len() == 1 {
                        self.qn_to_id.insert(qn.clone(), candidates[0].id);
                    }
                }
            }
        }

        // Resolve QN-based edges
        let mut resolved_edges = Vec::new();
        let mut dropped = 0usize;
        for e in &self.edges {
            if e.source_id != 0 && e.target_id != 0 {
                resolved_edges.push(e.clone());
                continue;
            }
            if let Some(ref props_str) = e.properties_json {
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(props_str) {
                    let src_qn = v["_src_qn"].as_str().unwrap_or("");
                    let tgt_qn = v["_tgt_qn"].as_str().unwrap_or("");
                    let real_props = v["_props"].as_str().map(|s| s.to_owned());
                    let src_id = self.qn_to_id.get(src_qn);
                    let tgt_id = self.qn_to_id.get(tgt_qn);
                    if let (Some(&sid), Some(&tid)) = (src_id, tgt_id) {
                        resolved_edges.push(Edge {
                            id: 0,
                            project: self.project.clone(),
                            source_id: sid,
                            target_id: tid,
                            edge_type: e.edge_type.clone(),
                            properties_json: real_props,
                        });
                    } else {
                        dropped += 1;
                        if dropped <= 5 {
                            tracing::debug!(
                                src_qn = src_qn,
                                tgt_qn = tgt_qn,
                                src_found = src_id.is_some(),
                                tgt_found = tgt_id.is_some(),
                                edge_type = e.edge_type.as_str(),
                                "edge dropped: QN not resolved"
                            );
                        }
                    }
                }
            }
        }
        if dropped > 0 {
            tracing::debug!(
                dropped = dropped,
                resolved = resolved_edges.len(),
                "edge resolution summary"
            );
        }

        store.insert_edges_batch(&resolved_edges)?;
        self.nodes.clear();
        self.edges.clear();

        // Flush code content for FTS
        if !self.code_snippets.is_empty() {
            store.upsert_code_content_batch(&self.code_snippets)?;
            self.code_snippets.clear();
        }

        Ok(())
    }
}
