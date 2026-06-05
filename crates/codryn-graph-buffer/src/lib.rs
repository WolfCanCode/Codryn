use anyhow::Result;
use codryn_store::{Edge, Node, Store};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Edge derivation source for confidence scoring.
/// Determines how trustworthy a graph edge is based on how it was derived.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EdgeSource {
    /// Structural edge extracted directly from AST (DEFINES, CONTAINS, DECLARES).
    AstStructural,
    /// Name-based relation extracted from AST (CALLS by matching function name only).
    AstNameMatch,
    /// Relation resolved through local import/path/module resolver.
    ImportResolver,
    /// Relation resolved by a language-specific adapter.
    DedicatedAdapter,
    /// Relation resolved by an external language server (tsserver, rust-analyzer, gopls).
    ExternalLsp,
    /// Relation confirmed by compiler-native index or equivalent authoritative source.
    CompilerIndex,
    /// Relation produced by Aho-Corasick textual matching.
    AhoCorasickMatch,
    /// Relation produced by regex matching.
    RegexMatch,
    /// Relation produced by fallback heuristic logic.
    Heuristic,
}

impl EdgeSource {
    /// Returns the confidence value associated with this edge source.
    pub fn confidence(self) -> f64 {
        match self {
            EdgeSource::CompilerIndex => 0.98,
            EdgeSource::ExternalLsp => 0.95,
            EdgeSource::AstStructural => 0.90,
            EdgeSource::DedicatedAdapter => 0.85,
            EdgeSource::ImportResolver => 0.82,
            EdgeSource::AstNameMatch => 0.60,
            EdgeSource::AhoCorasickMatch => 0.55,
            EdgeSource::RegexMatch => 0.45,
            EdgeSource::Heuristic => 0.30,
        }
    }

    /// Returns the string representation used for storage.
    pub fn as_str(self) -> &'static str {
        match self {
            EdgeSource::AstStructural => "AstStructural",
            EdgeSource::AstNameMatch => "AstNameMatch",
            EdgeSource::ImportResolver => "ImportResolver",
            EdgeSource::DedicatedAdapter => "DedicatedAdapter",
            EdgeSource::ExternalLsp => "ExternalLsp",
            EdgeSource::CompilerIndex => "CompilerIndex",
            EdgeSource::AhoCorasickMatch => "AhoCorasickMatch",
            EdgeSource::RegexMatch => "RegexMatch",
            EdgeSource::Heuristic => "Heuristic",
        }
    }
}

impl std::fmt::Display for EdgeSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

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
    /// Defaults to `EdgeSource::AstNameMatch` for backward compatibility.
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

    /// Add an edge with already-resolved IDs and a specific EdgeSource.
    /// The confidence and edge_source are embedded in properties_json for extraction at flush time.
    pub fn add_edge_with_source(
        &mut self,
        source_id: i64,
        target_id: i64,
        edge_type: &str,
        source: EdgeSource,
        properties_json: Option<String>,
    ) {
        let mut props = if let Some(ref p) = properties_json {
            serde_json::from_str::<serde_json::Value>(p).unwrap_or(serde_json::json!({}))
        } else {
            serde_json::json!({})
        };
        props["_confidence"] = serde_json::json!(source.confidence());
        props["_edge_source"] = serde_json::json!(source.as_str());

        self.edges.push(Edge {
            id: 0,
            project: self.project.clone(),
            source_id,
            target_id,
            edge_type: edge_type.to_owned(),
            properties_json: Some(props.to_string()),
        });
    }

    /// Add an edge with confidence metadata from a specific EdgeSource.
    /// The confidence value and edge_source are stored in the properties_json.
    pub fn add_edge_with_confidence(
        &mut self,
        source_qn: &str,
        target_qn: &str,
        edge_type: &str,
        source: EdgeSource,
        properties_json: Option<String>,
    ) {
        // Merge confidence metadata into properties
        let mut props = if let Some(ref p) = properties_json {
            serde_json::from_str::<serde_json::Value>(p).unwrap_or(serde_json::json!({}))
        } else {
            serde_json::json!({})
        };
        props["_confidence"] = serde_json::json!(source.confidence());
        props["_edge_source"] = serde_json::json!(source.as_str());

        // Store with source_id/target_id = 0, resolve at flush (same as add_edge_by_qn)
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
                    "_props": props.to_string(),
                })
                .to_string(),
            ),
        });
    }

    /// Merge another buffer's edges and nodes into this one.
    /// The source buffer is consumed. qn_to_id maps are merged.
    pub fn merge_from(&mut self, other: GraphBuffer) {
        self.nodes.extend(other.nodes);
        self.edges.extend(other.edges);
        self.qn_to_id.extend(other.qn_to_id);
        self.code_snippets.extend(other.code_snippets);
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
        // Batch exact-match resolution
        let missing_refs: Vec<&str> = missing_qns
            .iter()
            .filter(|qn| !self.qn_to_id.contains_key(*qn))
            .map(|s| s.as_str())
            .collect();
        if !missing_refs.is_empty() {
            let resolved = store.resolve_qns_batch(&self.project, &missing_refs)?;
            self.qn_to_id.extend(resolved);
        }

        // Batch suffix-match for remaining unresolved
        let still_missing: Vec<&str> = missing_qns
            .iter()
            .filter(|qn| !self.qn_to_id.contains_key(*qn))
            .map(|qn| qn.rsplit('.').next().unwrap_or(qn))
            .filter(|s| !s.is_empty())
            .collect();
        if !still_missing.is_empty() {
            let suffix_resolved = store.resolve_qns_suffix_batch(&self.project, &still_missing)?;
            // Map original QNs to resolved IDs
            for qn in &missing_qns {
                if !self.qn_to_id.contains_key(qn) {
                    let suffix = qn.rsplit('.').next().unwrap_or(qn);
                    if let Some(&id) = suffix_resolved.get(suffix) {
                        self.qn_to_id.insert(qn.clone(), id);
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

        // Flush code content for FTS (with compression for large snippets)
        if !self.code_snippets.is_empty() {
            store.upsert_code_content_compressed(&self.code_snippets)?;
            self.code_snippets.clear();
        }

        Ok(())
    }
}
