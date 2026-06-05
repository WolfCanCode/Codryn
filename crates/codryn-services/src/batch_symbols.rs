/// Track 5.12 — `get_symbols_batch` service.
///
/// Resolves multiple symbols in a single call, returning details for each.
/// Eliminates N sequential `get_symbol_details` calls for agents that need
/// details on multiple symbols (e.g., all methods of a class, all route handlers).
use anyhow::Result;
use codryn_store::Store;
use serde::Serialize;
use std::collections::HashMap;

pub struct BatchSymbolsService;

#[derive(Debug, Serialize)]
pub struct BatchSymbolsResult {
    pub project: String,
    pub symbols: Vec<SymbolDetails>,
    pub not_found: Vec<String>,
    pub count: usize,
    pub internal_edges: Vec<InternalEdge>,
}

#[derive(Debug, Serialize)]
pub struct SymbolDetails {
    pub name: String,
    pub qualified_name: String,
    pub label: String,
    pub file_path: String,
    pub start_line: i32,
    pub end_line: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signature: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub docstring: Option<String>,
    pub caller_count: i64,
    pub callee_count: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cyclomatic_complexity: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cognitive_complexity: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub has_docs: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub layer: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub annotations: Option<Vec<String>>,
}

#[derive(Debug, Serialize)]
pub struct InternalEdge {
    pub source_qn: String,
    pub target_qn: String,
    pub edge_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confidence: Option<f64>,
}

const MAX_BATCH_SIZE: usize = 50;

impl BatchSymbolsService {
    /// Resolve a list of symbol names or qualified names.
    pub fn get_batch(
        store: &Store,
        project: &str,
        names: &[String],
        include_internal_edges: bool,
    ) -> Result<BatchSymbolsResult> {
        let names = if names.len() > MAX_BATCH_SIZE {
            &names[..MAX_BATCH_SIZE]
        } else {
            names
        };

        let mut symbols = Vec::new();
        let mut not_found = Vec::new();
        let mut resolved_ids: Vec<i64> = Vec::new();

        // Bulk-resolve QNs first (fast path)
        let qn_refs: Vec<&str> = names.iter().map(|s| s.as_str()).collect();
        let qn_map = store
            .resolve_qns_batch(project, &qn_refs)
            .unwrap_or_default();

        // Get degrees in bulk for all resolved nodes
        let all_degrees = store.node_degrees_bulk(project).unwrap_or_default();

        for name in names {
            // Try exact QN first
            let node = if let Some(&_id) = qn_map.get(name.as_str()) {
                // We have the ID, fetch the node
                store
                    .find_node_by_qn(project, name)
                    .ok()
                    .flatten()
                    .or_else(|| {
                        // Fallback: search by name
                        store
                            .find_symbol_ranked(project, name, None, false, 1)
                            .ok()
                            .and_then(|mut v| v.pop().map(|(n, _, _)| n))
                    })
            } else {
                // Try ranked symbol lookup
                store
                    .find_symbol_ranked(project, name, None, false, 1)
                    .ok()
                    .and_then(|mut v| v.pop().map(|(n, _, _)| n))
            };

            match node {
                Some(n) => {
                    let (fan_in, fan_out) = all_degrees.get(&n.id).copied().unwrap_or((0, 0));
                    let details = Self::build_details(&n, fan_in as i64, fan_out as i64);
                    resolved_ids.push(n.id);
                    symbols.push(details);
                }
                None => {
                    not_found.push(name.clone());
                }
            }
        }

        // Find internal edges between the resolved symbols
        let internal_edges = if include_internal_edges && resolved_ids.len() > 1 {
            Self::find_internal_edges(store, project, &resolved_ids, &symbols)
        } else {
            vec![]
        };

        let count = symbols.len();
        Ok(BatchSymbolsResult {
            project: project.to_string(),
            symbols,
            not_found,
            count,
            internal_edges,
        })
    }

    /// Get all methods/members of a class.
    pub fn get_class_members(
        store: &Store,
        project: &str,
        class_name: &str,
    ) -> Result<BatchSymbolsResult> {
        // Find the class node
        let class_node = store
            .find_symbol_ranked(project, class_name, Some("Class"), false, 1)?
            .into_iter()
            .next()
            .map(|(n, _, _)| n)
            .ok_or_else(|| anyhow::anyhow!("Class not found: {}", class_name))?;

        // Find all methods with this class as parent (via CONTAINS or DEFINES edges)
        let all_nodes = store.get_all_nodes(project)?;
        let all_degrees = store.node_degrees_bulk(project).unwrap_or_default();

        let members: Vec<SymbolDetails> = all_nodes
            .iter()
            .filter(|n| {
                n.file_path == class_node.file_path
                    && matches!(n.label.as_str(), "Method" | "Function")
                    && n.start_line >= class_node.start_line
                    && n.end_line <= class_node.end_line
                    && n.id != class_node.id
            })
            .map(|n| {
                let (fan_in, fan_out) = all_degrees.get(&n.id).copied().unwrap_or((0, 0));
                Self::build_details(n, fan_in as i64, fan_out as i64)
            })
            .collect();

        let count = members.len();
        Ok(BatchSymbolsResult {
            project: project.to_string(),
            symbols: members,
            not_found: vec![],
            count,
            internal_edges: vec![],
        })
    }

    /// Get all symbols in a file.
    pub fn get_file_symbols(
        store: &Store,
        project: &str,
        file_path: &str,
    ) -> Result<BatchSymbolsResult> {
        let nodes = store.get_nodes_for_file(project, file_path)?;
        let all_degrees = store.node_degrees_bulk(project).unwrap_or_default();

        let symbols: Vec<SymbolDetails> = nodes
            .iter()
            .filter(|n| {
                !matches!(
                    n.label.as_str(),
                    "Module" | "File" | "Folder" | "Project" | "Package"
                )
            })
            .take(MAX_BATCH_SIZE)
            .map(|n| {
                let (fan_in, fan_out) = all_degrees.get(&n.id).copied().unwrap_or((0, 0));
                Self::build_details(n, fan_in as i64, fan_out as i64)
            })
            .collect();

        let count = symbols.len();
        Ok(BatchSymbolsResult {
            project: project.to_string(),
            symbols,
            not_found: vec![],
            count,
            internal_edges: vec![],
        })
    }

    fn build_details(n: &codryn_store::Node, fan_in: i64, fan_out: i64) -> SymbolDetails {
        let props: Option<serde_json::Value> = n
            .properties_json
            .as_deref()
            .and_then(|p| serde_json::from_str(p).ok());

        let signature = props
            .as_ref()
            .and_then(|p| p.get("signature"))
            .and_then(|v| v.as_str())
            .map(String::from);

        let docstring = props
            .as_ref()
            .and_then(|p| p.get("docstring").or_else(|| p.get("doc_comment")))
            .and_then(|v| v.as_str())
            .map(String::from);

        let cyclomatic_complexity = props
            .as_ref()
            .and_then(|p| p.get("cyclomatic_complexity"))
            .and_then(|v| v.as_i64());

        let cognitive_complexity = props
            .as_ref()
            .and_then(|p| p.get("cognitive_complexity"))
            .and_then(|v| v.as_i64());

        let has_docs = props
            .as_ref()
            .and_then(|p| p.get("has_docs"))
            .and_then(|v| v.as_bool());

        let layer = props
            .as_ref()
            .and_then(|p| p.get("layer"))
            .and_then(|v| v.as_str())
            .map(String::from);

        let annotations = props
            .as_ref()
            .and_then(|p| p.get("annotations"))
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            });

        SymbolDetails {
            name: n.name.clone(),
            qualified_name: n.qualified_name.clone(),
            label: n.label.clone(),
            file_path: n.file_path.clone(),
            start_line: n.start_line,
            end_line: n.end_line,
            signature,
            docstring,
            caller_count: fan_in,
            callee_count: fan_out,
            cyclomatic_complexity,
            cognitive_complexity,
            has_docs,
            layer,
            annotations,
        }
    }

    fn find_internal_edges(
        store: &Store,
        project: &str,
        node_ids: &[i64],
        symbols: &[SymbolDetails],
    ) -> Vec<InternalEdge> {
        let id_set: std::collections::HashSet<i64> = node_ids.iter().copied().collect();
        let qn_map: HashMap<i64, &str> = node_ids
            .iter()
            .zip(symbols.iter())
            .map(|(&id, s)| (id, s.qualified_name.as_str()))
            .collect();

        let mut edges = Vec::new();
        for &node_id in node_ids {
            // Get outbound edges from this node
            if let Ok(neighbors) = store.node_neighbors_detailed(node_id, "out", None, 50) {
                for (_, tgt_qn, _, _, _, et) in neighbors {
                    // Find if target is in our set
                    if let Ok(Some(tgt_node)) = store.find_node_by_qn(project, &tgt_qn) {
                        if id_set.contains(&tgt_node.id) {
                            if let Some(&src_qn) = qn_map.get(&node_id) {
                                edges.push(InternalEdge {
                                    source_qn: src_qn.to_string(),
                                    target_qn: tgt_qn,
                                    edge_type: et,
                                    confidence: None,
                                });
                            }
                        }
                    }
                }
            }
        }
        edges
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use codryn_store::{Edge, Node, Project};

    fn setup() -> Store {
        let s = Store::open_in_memory().unwrap();
        s.upsert_project(&Project {
            name: "p".into(),
            indexed_at: "now".into(),
            root_path: "/tmp".into(),
        })
        .unwrap();
        s
    }

    fn add_node(s: &Store, name: &str, label: &str, fp: &str) -> i64 {
        s.insert_node(&Node {
            id: 0,
            project: "p".into(),
            label: label.into(),
            name: name.into(),
            qualified_name: format!("p::{}", name),
            file_path: fp.into(),
            start_line: 1,
            end_line: 10,
            properties_json: None,
        })
        .unwrap()
    }

    #[test]
    fn test_batch_resolve() {
        let s = setup();
        add_node(&s, "funcA", "Function", "src/a.rs");
        add_node(&s, "funcB", "Function", "src/b.rs");

        let result = BatchSymbolsService::get_batch(
            &s,
            "p",
            &[
                "funcA".to_string(),
                "funcB".to_string(),
                "nonexistent".to_string(),
            ],
            false,
        )
        .unwrap();

        assert_eq!(result.count, 2);
        assert_eq!(result.not_found.len(), 1);
        assert_eq!(result.not_found[0], "nonexistent");
    }

    #[test]
    fn test_batch_internal_edges() {
        let s = setup();
        let a = add_node(&s, "funcA", "Function", "src/a.rs");
        let b = add_node(&s, "funcB", "Function", "src/b.rs");
        s.insert_edge(&Edge {
            id: 0,
            project: "p".into(),
            source_id: a,
            target_id: b,
            edge_type: "CALLS".into(),
            properties_json: None,
        })
        .unwrap();

        let result = BatchSymbolsService::get_batch(
            &s,
            "p",
            &["funcA".to_string(), "funcB".to_string()],
            true,
        )
        .unwrap();

        assert_eq!(result.count, 2);
        assert_eq!(result.internal_edges.len(), 1);
        assert_eq!(result.internal_edges[0].edge_type, "CALLS");
    }

    #[test]
    fn test_file_symbols() {
        let s = setup();
        add_node(&s, "funcA", "Function", "src/a.rs");
        add_node(&s, "funcB", "Function", "src/a.rs");
        add_node(&s, "funcC", "Function", "src/b.rs");

        let result = BatchSymbolsService::get_file_symbols(&s, "p", "src/a.rs").unwrap();
        assert_eq!(result.count, 2);
    }

    #[test]
    fn test_batch_respects_max_size() {
        let s = setup();
        let names: Vec<String> = (0..60).map(|i| format!("func{}", i)).collect();
        // Only add 60 nodes
        for name in &names {
            add_node(&s, name, "Function", "src/a.rs");
        }

        let result = BatchSymbolsService::get_batch(&s, "p", &names, false).unwrap();
        // Should be capped at MAX_BATCH_SIZE = 50
        assert!(result.count <= 50);
    }
}
