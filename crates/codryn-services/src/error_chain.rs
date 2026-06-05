use anyhow::Result;
use codryn_store::Store;
use serde::Serialize;
use std::collections::{HashSet, VecDeque};

#[derive(Debug, Serialize)]
pub struct HandlerInfo {
    pub symbol: String,
    pub file_path: String,
    pub line: i64,
}

#[derive(Debug, Serialize)]
pub struct ErrorPath {
    pub chain: Vec<String>,
    pub first_handler: Option<HandlerInfo>,
}

#[derive(Debug, Serialize)]
pub struct ErrorChainResult {
    pub source: String,
    pub uncaught_paths: Vec<ErrorPath>,
    pub total_propagation_depth: usize,
}

pub struct ErrorChainService;

fn is_error_handler(name: &str) -> bool {
    let lower = name.to_lowercase();
    lower.contains("handle")
        || lower.contains("catch")
        || lower.contains("rescue")
        || lower.contains("try")
}

impl ErrorChainService {
    pub fn trace_error_flow(
        store: &Store,
        project: &str,
        symbol: &str,
        max_depth: i32,
    ) -> Result<ErrorChainResult> {
        let max_depth = if max_depth <= 0 { 5 } else { max_depth } as usize;

        // Find source symbol
        let nodes = store.search_nodes(project, symbol, 1)?;
        let source_node = nodes
            .first()
            .ok_or_else(|| anyhow::anyhow!("Symbol '{}' not found", symbol))?;

        let mut uncaught_paths = Vec::new();
        let mut visited = HashSet::new();
        let mut queue: VecDeque<(i64, Vec<String>)> = VecDeque::new();
        queue.push_back((source_node.id, vec![source_node.name.clone()]));
        visited.insert(source_node.id);

        let mut max_depth_seen = 0usize;

        while let Some((node_id, chain)) = queue.pop_front() {
            if chain.len() > max_depth {
                uncaught_paths.push(ErrorPath {
                    chain,
                    first_handler: None,
                });
                continue;
            }

            let callers =
                store.incoming_references_detailed(node_id, Some(&["CALLS"]), 20, None)?;
            if callers.is_empty() && chain.len() > 1 {
                uncaught_paths.push(ErrorPath {
                    chain,
                    first_handler: None,
                });
                continue;
            }

            for (caller, _edge_type, _conf, _src) in &callers {
                if !visited.insert(caller.id) {
                    continue;
                }
                let mut new_chain = chain.clone();
                new_chain.push(caller.name.clone());
                max_depth_seen = max_depth_seen.max(new_chain.len());

                if is_error_handler(&caller.name) {
                    // Found a handler — this path is caught
                    let _handled = ErrorPath {
                        chain: new_chain,
                        first_handler: Some(HandlerInfo {
                            symbol: caller.name.clone(),
                            file_path: caller.file_path.clone(),
                            line: caller.start_line as i64,
                        }),
                    };
                    // We don't add handled paths to uncaught
                } else {
                    queue.push_back((caller.id, new_chain));
                }
            }
        }

        Ok(ErrorChainResult {
            source: source_node.name.clone(),
            uncaught_paths,
            total_propagation_depth: max_depth_seen,
        })
    }
}
