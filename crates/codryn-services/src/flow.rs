use anyhow::Result;
use codryn_store::Store;
use serde::Serialize;
use std::collections::{HashSet, VecDeque};

pub struct FlowAnalysisService;

#[derive(Debug, Serialize)]
pub struct FlowResult {
    pub project: String,
    pub flow_type: String,
    pub paths: Vec<FlowPath>,
    pub count: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notes: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize)]
pub struct FlowPath {
    pub score: f64,
    pub reason: String,
    pub steps: Vec<FlowStep>,
}

#[derive(Debug, Clone, Serialize)]
pub struct FlowStep {
    pub qualified_name: String,
    pub label: String,
    pub file_path: String,
}

const LAYER_PATTERNS: &[(&str, u8)] = &[
    ("route", 0),
    ("controller", 1),
    ("handler", 1),
    ("service", 2),
    ("usecase", 2),
    ("repository", 3),
    ("repo", 3),
    ("store", 3),
    ("dao", 3),
    ("model", 4),
    ("entity", 4),
];

fn detect_layer(file_path: &str) -> Option<u8> {
    let fp = file_path.to_lowercase();
    LAYER_PATTERNS
        .iter()
        .find(|(pat, _)| fp.contains(pat))
        .map(|(_, layer)| *layer)
}

fn score_path(steps: &[FlowStep]) -> (f64, String) {
    if steps.len() < 2 {
        return (0.5, "single-step path".into());
    }
    let layers: Vec<Option<u8>> = steps.iter().map(|s| detect_layer(&s.file_path)).collect();
    let known: Vec<u8> = layers.iter().filter_map(|l| *l).collect();

    if known.len() >= 2 {
        // Check if layers are monotonically increasing (architectural flow)
        let monotonic = known.windows(2).all(|w| w[0] <= w[1]);
        if monotonic {
            let layer_names: Vec<&str> = known
                .iter()
                .map(|l| match l {
                    0 => "route",
                    1 => "controller",
                    2 => "service",
                    3 => "repository",
                    _ => "model",
                })
                .collect();
            let mut deduped = layer_names.clone();
            deduped.dedup();
            let reason = deduped.join("→") + " pattern";
            let score = 0.7 + (known.len() as f64 * 0.05).min(0.25);
            return (score, reason);
        }
    }

    let score = 0.5 + (steps.len() as f64 * 0.03).min(0.2);
    (score, "call chain".into())
}

fn edge_matches_flow_type(edge_type: &str, flow_type: &str) -> bool {
    match flow_type {
        "request" => matches!(
            edge_type,
            "CALLS" | "ASYNC_CALLS" | "HTTP_CALLS" | "HANDLES_ROUTE"
        ),
        "render" => matches!(edge_type, "RENDERS" | "CALLS"),
        "data" => matches!(edge_type, "CALLS" | "ASYNC_CALLS" | "IMPORTS" | "USES"),
        _ => true,
    }
}

impl FlowAnalysisService {
    #[allow(clippy::too_many_arguments)]
    pub fn trace_data_flow(
        store: &Store,
        project: &str,
        source: Option<&str>,
        target: Option<&str>,
        file_path: Option<&str>,
        flow_type: Option<&str>,
        max_depth: i32,
        limit: i32,
        _include_linked: bool,
    ) -> Result<FlowResult> {
        let max_depth = if max_depth <= 0 { 5 } else { max_depth };
        let limit = if limit <= 0 { 10usize } else { limit as usize };
        let ft = flow_type.unwrap_or("any");
        let mut notes = Vec::new();

        // Resolve source nodes
        let source_nodes = if let Some(src) = source {
            let by_qn = store.find_node_by_qn(project, src)?;
            if let Some(n) = by_qn {
                vec![n]
            } else {
                store.search_nodes(project, src, 5)?
            }
        } else if let Some(fp) = file_path {
            store.get_nodes_for_file(project, fp)?
        } else {
            return Err(anyhow::anyhow!("Provide source, or file_path"));
        };

        if source_nodes.is_empty() {
            return Err(anyhow::anyhow!("No source nodes found"));
        }

        // Resolve target node id if provided
        let target_id = if let Some(tgt) = target {
            store
                .find_node_by_qn(project, tgt)?
                .or_else(|| store.search_nodes(project, tgt, 1).ok()?.into_iter().next())
                .map(|n| n.id)
        } else {
            None
        };

        let mut all_paths: Vec<FlowPath> = Vec::new();

        // BFS from each source, tracking full paths
        for start in &source_nodes {
            // (node_id, path_so_far)
            let mut queue: VecDeque<(i64, Vec<FlowStep>)> = VecDeque::new();
            let mut visited: HashSet<i64> = HashSet::new();

            let initial_step = FlowStep {
                qualified_name: start.qualified_name.clone(),
                label: start.label.clone(),
                file_path: start.file_path.clone(),
            };
            queue.push_back((start.id, vec![initial_step]));
            visited.insert(start.id);

            while let Some((node_id, path)) = queue.pop_front() {
                if path.len() as i32 > max_depth {
                    continue;
                }
                if all_paths.len() >= limit {
                    break;
                }

                let edges = store
                    .get_edges_from_node(node_id, "out", 50)
                    .unwrap_or_default();

                for (tgt_id, _name, qn, label, fp, _sl, edge_type) in &edges {
                    if ft != "any" && !edge_matches_flow_type(edge_type, ft) {
                        continue;
                    }

                    let mut new_path = path.clone();
                    new_path.push(FlowStep {
                        qualified_name: qn.clone(),
                        label: label.clone(),
                        file_path: fp.clone(),
                    });

                    // If target reached, record path
                    if let Some(tid) = target_id {
                        if *tgt_id == tid {
                            let (score, reason) = score_path(&new_path);
                            all_paths.push(FlowPath {
                                score,
                                reason,
                                steps: new_path,
                            });
                            continue;
                        }
                    }

                    // Record paths of length >= 2 even without target
                    if target_id.is_none() && new_path.len() >= 2 {
                        let (score, reason) = score_path(&new_path);
                        all_paths.push(FlowPath {
                            score,
                            reason,
                            steps: new_path.clone(),
                        });
                    }

                    if !visited.contains(tgt_id) {
                        visited.insert(*tgt_id);
                        queue.push_back((*tgt_id, new_path));
                    }
                }
            }
        }

        all_paths.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        // Deduplicate: keep longest path per unique last step
        let mut seen_endpoints = HashSet::new();
        all_paths.retain(|p| {
            if let Some(last) = p.steps.last() {
                seen_endpoints.insert(last.qualified_name.clone())
            } else {
                false
            }
        });

        all_paths.truncate(limit);

        if all_paths.is_empty() {
            notes.push("No flow paths found — graph may lack sufficient call/import edges".into());
        }

        let count = all_paths.len();
        Ok(FlowResult {
            project: project.to_string(),
            flow_type: ft.to_string(),
            paths: all_paths,
            count,
            notes: if notes.is_empty() { None } else { Some(notes) },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use codryn_store::{Edge, Node, Project};

    fn setup_store() -> Store {
        let s = Store::open_in_memory().unwrap();
        s.upsert_project(&Project {
            name: "p".into(),
            indexed_at: "now".into(),
            root_path: "/tmp".into(),
        })
        .unwrap();
        s
    }

    fn insert_node(s: &Store, name: &str, qn: &str, label: &str, fp: &str) -> i64 {
        s.insert_node(&Node {
            id: 0,
            project: "p".into(),
            label: label.into(),
            name: name.into(),
            qualified_name: qn.into(),
            file_path: fp.into(),
            start_line: 1,
            end_line: 10,
            properties_json: None,
        })
        .unwrap()
    }

    fn insert_edge(s: &Store, src: i64, tgt: i64, et: &str) {
        s.insert_edge(&Edge {
            id: 0,
            project: "p".into(),
            source_id: src,
            target_id: tgt,
            edge_type: et.into(),
            properties_json: None,
        })
        .unwrap();
    }

    #[test]
    fn test_trace_data_flow_simple_chain() {
        let s = setup_store();
        let r = insert_node(
            &s,
            "getUser",
            "p::getUser",
            "Function",
            "src/routes/user.ts",
        );
        let c = insert_node(
            &s,
            "handleGetUser",
            "p::handleGetUser",
            "Function",
            "src/controllers/user.ts",
        );
        let svc = insert_node(
            &s,
            "userService",
            "p::userService",
            "Function",
            "src/services/user.ts",
        );
        insert_edge(&s, r, c, "CALLS");
        insert_edge(&s, c, svc, "CALLS");

        let res = FlowAnalysisService::trace_data_flow(
            &s,
            "p",
            Some("p::getUser"),
            None,
            None,
            None,
            5,
            10,
            false,
        )
        .unwrap();
        assert!(!res.paths.is_empty());
        // Should detect architectural pattern
        let best = &res.paths[0];
        assert!(best.steps.len() >= 2);
    }

    #[test]
    fn test_trace_with_target() {
        let s = setup_store();
        let a = insert_node(&s, "a", "p::a", "Function", "src/a.ts");
        let b = insert_node(&s, "b", "p::b", "Function", "src/b.ts");
        let c = insert_node(&s, "c", "p::c", "Function", "src/c.ts");
        insert_edge(&s, a, b, "CALLS");
        insert_edge(&s, b, c, "CALLS");

        let res = FlowAnalysisService::trace_data_flow(
            &s,
            "p",
            Some("p::a"),
            Some("p::c"),
            None,
            None,
            5,
            10,
            false,
        )
        .unwrap();
        assert!(res.paths.iter().any(|p| p
            .steps
            .last()
            .map(|s| s.qualified_name == "p::c")
            .unwrap_or(false)));
    }
}
