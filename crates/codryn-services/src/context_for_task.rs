/// Track 5.9 — `get_context_for_task` service.
///
/// Collapses 3–4 tool calls into one: given a symbol + task type, returns
/// everything an agent needs to modify, debug, test, or document it.
use anyhow::Result;
use codryn_store::Store;
use serde::Serialize;

pub struct ContextForTaskService;

#[derive(Debug, Serialize)]
pub struct TaskContext {
    pub project: String,
    pub symbol: SymbolDetail,
    pub task: String,
    pub context: TaskSpecificContext,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub warnings: Option<Vec<String>>,
    pub token_estimate: usize,
}

#[derive(Debug, Serialize)]
pub struct SymbolDetail {
    pub name: String,
    pub qualified_name: String,
    pub label: String,
    pub file_path: String,
    pub start_line: i32,
    pub end_line: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snippet: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub properties: Option<serde_json::Value>,
}

#[derive(Debug, Serialize)]
#[serde(untagged)]
pub enum TaskSpecificContext {
    Modify(ModifyContext),
    Debug(DebugContext),
    Test(TestContext),
    Document(DocumentContext),
}

#[derive(Debug, Serialize)]
pub struct ModifyContext {
    pub callers: Vec<CallerInfo>,
    pub callees: Vec<CalleeInfo>,
    pub imports: Vec<ImportInfo>,
    pub related_tests: Vec<TestRef>,
    pub impact_summary: String,
}

#[derive(Debug, Serialize)]
pub struct DebugContext {
    pub callers: Vec<CallerInfo>,
    pub callees: Vec<CalleeInfo>,
    pub call_chain_depth2: Vec<ChainNode>,
    pub imports: Vec<ImportInfo>,
}

#[derive(Debug, Serialize)]
pub struct TestContext {
    pub existing_tests: Vec<TestRef>,
    pub similar_tested_functions: Vec<SimilarFn>,
    pub dependencies_to_mock: Vec<MockCandidate>,
    pub imports: Vec<ImportInfo>,
}

#[derive(Debug, Serialize)]
pub struct DocumentContext {
    pub callers: Vec<CallerInfo>,
    pub usage_examples: Vec<UsageExample>,
    pub related_symbols: Vec<RelatedSymbol>,
}

#[derive(Debug, Serialize)]
pub struct CallerInfo {
    pub name: String,
    pub qualified_name: String,
    pub file_path: String,
    pub start_line: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snippet: Option<String>,
    pub edge_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confidence: Option<f64>,
}

#[derive(Debug, Serialize)]
pub struct CalleeInfo {
    pub name: String,
    pub qualified_name: String,
    pub file_path: String,
    pub start_line: i32,
    pub edge_type: String,
}

#[derive(Debug, Serialize)]
pub struct ImportInfo {
    pub name: String,
    pub file_path: String,
    pub edge_type: String,
}

#[derive(Debug, Serialize)]
pub struct TestRef {
    pub name: String,
    pub qualified_name: String,
    pub file_path: String,
}

#[derive(Debug, Serialize)]
pub struct ChainNode {
    pub name: String,
    pub qualified_name: String,
    pub file_path: String,
    pub depth: usize,
    pub direction: String,
}

#[derive(Debug, Serialize)]
pub struct MockCandidate {
    pub name: String,
    pub qualified_name: String,
    pub file_path: String,
    pub reason: String,
}

#[derive(Debug, Serialize)]
pub struct SimilarFn {
    pub name: String,
    pub qualified_name: String,
    pub file_path: String,
    pub has_tests: bool,
}

#[derive(Debug, Serialize)]
pub struct UsageExample {
    pub caller_name: String,
    pub file_path: String,
    pub start_line: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snippet: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct RelatedSymbol {
    pub name: String,
    pub qualified_name: String,
    pub label: String,
    pub file_path: String,
    pub relationship: String,
}

const MAX_CALLERS: usize = 10;
const MAX_CALLEES: usize = 10;
const MAX_CHAIN: usize = 15;
const MAX_TESTS: usize = 5;
const MAX_SNIPPET_LINES: usize = 30;

impl ContextForTaskService {
    pub fn get_context(
        store: &Store,
        project: &str,
        symbol_name: &str,
        qualified_name: Option<&str>,
        task: &str,
    ) -> Result<TaskContext> {
        // Resolve the symbol
        let node = if let Some(qn) = qualified_name {
            store
                .find_node_by_qn(project, qn)?
                .ok_or_else(|| anyhow::anyhow!("Symbol not found: {}", qn))?
        } else {
            let results = store.find_symbol_ranked(project, symbol_name, None, false, 5)?;
            results
                .into_iter()
                .next()
                .map(|(n, _, _)| n)
                .ok_or_else(|| anyhow::anyhow!("Symbol not found: {}", symbol_name))?
        };

        // Get snippet from code store
        let snippet = store
            .get_code_content(project, &node.qualified_name)
            .ok()
            .flatten()
            .map(|content| {
                let lines: Vec<&str> = content.lines().collect();
                let start = (node.start_line as usize).saturating_sub(1);
                let end = (node.end_line as usize).min(start + MAX_SNIPPET_LINES);
                let end = end.min(lines.len());
                if start < lines.len() {
                    lines[start..end].join("\n")
                } else {
                    content
                        .lines()
                        .take(MAX_SNIPPET_LINES)
                        .collect::<Vec<_>>()
                        .join("\n")
                }
            });

        let props = node
            .properties_json
            .as_deref()
            .and_then(|p| serde_json::from_str(p).ok());

        let symbol = SymbolDetail {
            name: node.name.clone(),
            qualified_name: node.qualified_name.clone(),
            label: node.label.clone(),
            file_path: node.file_path.clone(),
            start_line: node.start_line,
            end_line: node.end_line,
            snippet,
            properties: props,
        };

        let context = match task {
            "modify" => {
                TaskSpecificContext::Modify(Self::build_modify_context(store, project, &node)?)
            }
            "debug" => {
                TaskSpecificContext::Debug(Self::build_debug_context(store, project, &node)?)
            }
            "test" => TaskSpecificContext::Test(Self::build_test_context(store, project, &node)?),
            "document" => {
                TaskSpecificContext::Document(Self::build_document_context(store, project, &node)?)
            }
            _ => TaskSpecificContext::Modify(Self::build_modify_context(store, project, &node)?),
        };

        // Rough token estimate (4 chars per token)
        let json_size = serde_json::to_string(&context)
            .map(|s| s.len())
            .unwrap_or(0);
        let token_estimate = json_size / 4;

        let mut warnings = Vec::new();
        if token_estimate > 8000 {
            warnings.push(format!(
                "Response is large (~{} tokens). Consider narrowing scope.",
                token_estimate
            ));
        }

        Ok(TaskContext {
            project: project.to_string(),
            symbol,
            task: task.to_string(),
            context,
            warnings: if warnings.is_empty() {
                None
            } else {
                Some(warnings)
            },
            token_estimate,
        })
    }

    fn build_modify_context(
        store: &Store,
        project: &str,
        node: &codryn_store::Node,
    ) -> Result<ModifyContext> {
        let callers = Self::get_callers(store, project, node, MAX_CALLERS);
        let callees = Self::get_callees(store, project, node, MAX_CALLEES);
        let imports = Self::get_imports(store, project, node, 10);
        let related_tests = Self::find_tests(store, project, node, MAX_TESTS);

        let impact_summary = format!(
            "{} direct callers, {} callees, {} imports",
            callers.len(),
            callees.len(),
            imports.len()
        );

        Ok(ModifyContext {
            callers,
            callees,
            imports,
            related_tests,
            impact_summary,
        })
    }

    fn build_debug_context(
        store: &Store,
        project: &str,
        node: &codryn_store::Node,
    ) -> Result<DebugContext> {
        let callers = Self::get_callers(store, project, node, MAX_CALLERS);
        let callees = Self::get_callees(store, project, node, MAX_CALLEES);
        let imports = Self::get_imports(store, project, node, 10);

        // 2-hop call chain
        let mut chain = Vec::new();
        // Callers of callers (depth 2 upstream)
        for caller in callers.iter().take(3) {
            if let Ok(Some(caller_node)) = store.find_node_by_qn(project, &caller.qualified_name) {
                let upstream = Self::get_callers(store, project, &caller_node, 3);
                for up in upstream {
                    chain.push(ChainNode {
                        name: up.name,
                        qualified_name: up.qualified_name,
                        file_path: up.file_path,
                        depth: 2,
                        direction: "upstream".into(),
                    });
                }
            }
        }
        // Callees of callees (depth 2 downstream)
        for callee in callees.iter().take(3) {
            if let Ok(Some(callee_node)) = store.find_node_by_qn(project, &callee.qualified_name) {
                let downstream = Self::get_callees(store, project, &callee_node, 3);
                for down in downstream {
                    chain.push(ChainNode {
                        name: down.name,
                        qualified_name: down.qualified_name,
                        file_path: down.file_path,
                        depth: 2,
                        direction: "downstream".into(),
                    });
                }
            }
        }
        chain.truncate(MAX_CHAIN);

        Ok(DebugContext {
            callers,
            callees,
            call_chain_depth2: chain,
            imports,
        })
    }

    fn build_test_context(
        store: &Store,
        project: &str,
        node: &codryn_store::Node,
    ) -> Result<TestContext> {
        let existing_tests = Self::find_tests(store, project, node, MAX_TESTS);
        let imports = Self::get_imports(store, project, node, 10);

        // Find similar functions in the same file that have tests
        let similar = Self::find_similar_tested(store, project, node, 5);

        // Dependencies to mock = callees that are in different files
        let callees = Self::get_callees(store, project, node, 15);
        let mock_candidates: Vec<MockCandidate> = callees
            .into_iter()
            .filter(|c| c.file_path != node.file_path)
            .map(|c| MockCandidate {
                name: c.name.clone(),
                qualified_name: c.qualified_name.clone(),
                file_path: c.file_path.clone(),
                reason: format!("external dependency called by {}", node.name),
            })
            .take(8)
            .collect();

        Ok(TestContext {
            existing_tests,
            similar_tested_functions: similar,
            dependencies_to_mock: mock_candidates,
            imports,
        })
    }

    fn build_document_context(
        store: &Store,
        project: &str,
        node: &codryn_store::Node,
    ) -> Result<DocumentContext> {
        let callers = Self::get_callers(store, project, node, MAX_CALLERS);

        // Usage examples = callers with snippets
        let usage_examples: Vec<UsageExample> = callers
            .iter()
            .take(5)
            .map(|c| UsageExample {
                caller_name: c.name.clone(),
                file_path: c.file_path.clone(),
                start_line: c.start_line,
                snippet: c.snippet.clone(),
            })
            .collect();

        // Related symbols = inheritance, implements, members
        let related = Self::get_related_symbols(store, project, node, 10);

        Ok(DocumentContext {
            callers,
            usage_examples,
            related_symbols: related,
        })
    }

    fn get_callers(
        store: &Store,
        project: &str,
        node: &codryn_store::Node,
        limit: usize,
    ) -> Vec<CallerInfo> {
        let call_types = &["CALLS", "ASYNC_CALLS", "HTTP_CALLS", "USES"];
        let refs = store
            .incoming_references_detailed(node.id, Some(call_types), limit as i32, None)
            .unwrap_or_default();

        refs.into_iter()
            .map(|(n, et, conf, _src)| {
                let snippet = store
                    .get_code_content(project, &n.qualified_name)
                    .ok()
                    .flatten()
                    .map(|content| content.lines().take(10).collect::<Vec<_>>().join("\n"));
                CallerInfo {
                    name: n.name,
                    qualified_name: n.qualified_name,
                    file_path: n.file_path,
                    start_line: n.start_line,
                    snippet,
                    edge_type: et,
                    confidence: conf,
                }
            })
            .collect()
    }

    fn get_callees(
        store: &Store,
        _project: &str,
        node: &codryn_store::Node,
        limit: usize,
    ) -> Vec<CalleeInfo> {
        let call_types = &["CALLS", "ASYNC_CALLS", "HTTP_CALLS", "USES"];
        store
            .node_neighbors_detailed(node.id, "out", Some(call_types), limit as i32)
            .unwrap_or_default()
            .into_iter()
            .map(|(name, qn, _label, fp, sl, et)| CalleeInfo {
                name,
                qualified_name: qn,
                file_path: fp,
                start_line: sl,
                edge_type: et,
            })
            .collect()
    }

    fn get_imports(
        store: &Store,
        _project: &str,
        node: &codryn_store::Node,
        limit: usize,
    ) -> Vec<ImportInfo> {
        let import_types = &["IMPORTS"];
        store
            .node_neighbors_detailed(node.id, "out", Some(import_types), limit as i32)
            .unwrap_or_default()
            .into_iter()
            .map(|(name, _qn, _label, fp, _sl, et)| ImportInfo {
                name,
                file_path: fp,
                edge_type: et,
            })
            .collect()
    }

    fn find_tests(
        store: &Store,
        project: &str,
        node: &codryn_store::Node,
        limit: usize,
    ) -> Vec<TestRef> {
        // Look for nodes in test files that reference this symbol
        let all_nodes = store.get_all_nodes(project).unwrap_or_default();
        all_nodes
            .into_iter()
            .filter(|n| {
                let fp = n.file_path.to_lowercase();
                (fp.contains("/test/")
                    || fp.contains("/tests/")
                    || fp.contains(".test.")
                    || fp.contains(".spec.")
                    || fp.contains("_test."))
                    && (n.name.to_lowercase().contains(&node.name.to_lowercase())
                        || n.qualified_name
                            .to_lowercase()
                            .contains(&node.name.to_lowercase()))
            })
            .take(limit)
            .map(|n| TestRef {
                name: n.name,
                qualified_name: n.qualified_name,
                file_path: n.file_path,
            })
            .collect()
    }

    fn find_similar_tested(
        store: &Store,
        project: &str,
        node: &codryn_store::Node,
        limit: usize,
    ) -> Vec<SimilarFn> {
        // Find functions in the same file with similar labels
        let file_nodes = store
            .get_nodes_for_file(project, &node.file_path)
            .unwrap_or_default();
        let all_nodes = store.get_all_nodes(project).unwrap_or_default();
        let test_qns: std::collections::HashSet<String> = all_nodes
            .iter()
            .filter(|n| {
                let fp = n.file_path.to_lowercase();
                fp.contains("/test/")
                    || fp.contains("/tests/")
                    || fp.contains(".test.")
                    || fp.contains(".spec.")
                    || fp.contains("_test.")
            })
            .flat_map(|n| {
                store
                    .node_neighbors_detailed(n.id, "out", Some(&["CALLS", "USES"]), 5)
                    .unwrap_or_default()
                    .into_iter()
                    .map(|(_, qn, _, _, _, _)| qn)
            })
            .collect();

        file_nodes
            .into_iter()
            .filter(|n| n.id != node.id && matches!(n.label.as_str(), "Function" | "Method"))
            .take(limit)
            .map(|n| {
                let has_tests = test_qns.contains(&n.qualified_name);
                SimilarFn {
                    name: n.name,
                    qualified_name: n.qualified_name,
                    file_path: n.file_path,
                    has_tests,
                }
            })
            .collect()
    }

    fn get_related_symbols(
        store: &Store,
        _project: &str,
        node: &codryn_store::Node,
        limit: usize,
    ) -> Vec<RelatedSymbol> {
        let inherit_types = &["INHERITS", "IMPLEMENTS", "CONTAINS", "DEFINES"];
        let mut related = Vec::new();

        // Outbound relationships
        for (name, qn, label, fp, _sl, et) in store
            .node_neighbors_detailed(node.id, "out", Some(inherit_types), limit as i32)
            .unwrap_or_default()
        {
            related.push(RelatedSymbol {
                name,
                qualified_name: qn,
                label,
                file_path: fp,
                relationship: et,
            });
        }

        // Inbound relationships
        for (name, qn, label, fp, _sl, et) in store
            .node_neighbors_detailed(node.id, "in", Some(inherit_types), limit as i32)
            .unwrap_or_default()
        {
            related.push(RelatedSymbol {
                name,
                qualified_name: qn,
                label,
                file_path: fp,
                relationship: format!("inverse_{}", et),
            });
        }

        related.truncate(limit);
        related
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

    fn add_edge(s: &Store, src: i64, tgt: i64, et: &str) {
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
    fn test_context_for_modify() {
        let s = setup();
        let target = add_node(&s, "processOrder", "Function", "src/orders.rs");
        let caller = add_node(&s, "handleRequest", "Function", "src/handler.rs");
        let callee = add_node(&s, "validateOrder", "Function", "src/validation.rs");
        add_edge(&s, caller, target, "CALLS");
        add_edge(&s, target, callee, "CALLS");

        let ctx =
            ContextForTaskService::get_context(&s, "p", "processOrder", None, "modify").unwrap();
        assert_eq!(ctx.symbol.name, "processOrder");
        assert_eq!(ctx.task, "modify");
        if let TaskSpecificContext::Modify(m) = &ctx.context {
            assert_eq!(m.callers.len(), 1);
            assert_eq!(m.callees.len(), 1);
            assert_eq!(m.callers[0].name, "handleRequest");
            assert_eq!(m.callees[0].name, "validateOrder");
        } else {
            panic!("Expected Modify context");
        }
    }

    #[test]
    fn test_context_for_debug() {
        let s = setup();
        let target = add_node(&s, "processOrder", "Function", "src/orders.rs");
        let caller = add_node(&s, "handleRequest", "Function", "src/handler.rs");
        add_edge(&s, caller, target, "CALLS");

        let ctx =
            ContextForTaskService::get_context(&s, "p", "processOrder", None, "debug").unwrap();
        if let TaskSpecificContext::Debug(d) = &ctx.context {
            assert_eq!(d.callers.len(), 1);
        } else {
            panic!("Expected Debug context");
        }
    }

    #[test]
    fn test_context_not_found() {
        let s = setup();
        let result = ContextForTaskService::get_context(&s, "p", "nonexistent", None, "modify");
        assert!(result.is_err());
    }
}
