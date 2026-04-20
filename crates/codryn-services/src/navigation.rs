use anyhow::Result;
use codryn_store::Store;
use serde::Serialize;

pub struct NavigationService;

// -- Response types --

#[derive(Debug, Serialize)]
pub struct FileOverview {
    pub project: String,
    pub file: FileInfo,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub symbols: Option<Vec<SymbolInfo>>,
    pub symbol_count: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub imports: Option<Vec<ImportInfo>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exports: Option<Vec<ExportInfo>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub graph_summary: Option<GraphSummary>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notes: Option<Vec<String>>,
}

#[derive(Debug, Serialize)]
pub struct FileInfo {
    pub file_path: String,
    pub language: String,
    pub exists_in_index: bool,
}

#[derive(Debug, Serialize)]
pub struct SymbolInfo {
    pub name: String,
    pub qualified_name: String,
    pub label: String,
    pub start_line: i32,
    pub end_line: i32,
}

#[derive(Debug, Serialize)]
pub struct ImportInfo {
    pub name: String,
    pub source: String,
}

#[derive(Debug, Serialize)]
pub struct ExportInfo {
    pub name: String,
    pub kind: String,
}

#[derive(Debug, Serialize)]
pub struct GraphSummary {
    pub inbound_edges: i64,
    pub outbound_edges: i64,
}

#[derive(Debug, Serialize)]
pub struct EntrypointResult {
    pub project: String,
    pub entry_type: String,
    pub candidates: Vec<EntrypointCandidate>,
    pub count: usize,
}

#[derive(Debug, Serialize)]
pub struct EntrypointCandidate {
    pub name: String,
    pub qualified_name: String,
    pub label: String,
    pub file_path: String,
    pub reason: String,
    pub score: f64,
}

#[derive(Debug, Serialize)]
pub struct SuggestionsResult {
    pub project: String,
    pub goal: String,
    pub origin: serde_json::Value,
    pub suggestions: Vec<Suggestion>,
    pub count: usize,
}

#[derive(Debug, Serialize)]
pub struct Suggestion {
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub qualified_name: Option<String>,
    pub reason: String,
    pub score: f64,
}

const DEFAULT_LIMIT: usize = 10;

impl NavigationService {
    pub fn file_overview(
        store: &Store,
        project: &str,
        file_path: &str,
        include_symbols: bool,
        include_imports: bool,
        include_exports: bool,
        include_neighbors: bool,
    ) -> Result<FileOverview> {
        let exists = store.has_file_hash(project, file_path).unwrap_or(false);
        let lang = codryn_discover::detect_language(file_path);
        let lang_str = if lang == codryn_discover::Language::Unknown {
            "unknown".to_string()
        } else {
            format!("{:?}", lang)
        };

        let all_nodes = store.get_nodes_for_file(project, file_path)?;
        let symbol_count = all_nodes.len();
        let mut notes = Vec::new();

        let symbols = if include_symbols {
            let capped: Vec<SymbolInfo> = all_nodes
                .iter()
                .filter(|n| !matches!(n.label.as_str(), "Module" | "File" | "Folder" | "Project"))
                .take(DEFAULT_LIMIT)
                .map(|n| SymbolInfo {
                    name: n.name.clone(),
                    qualified_name: n.qualified_name.clone(),
                    label: n.label.clone(),
                    start_line: n.start_line,
                    end_line: n.end_line,
                })
                .collect();
            if symbol_count > DEFAULT_LIMIT {
                notes.push(format!(
                    "Showing {} of {} symbols",
                    capped.len(),
                    symbol_count
                ));
            }
            Some(capped)
        } else {
            None
        };

        let imports = if include_imports {
            let mut imp = Vec::new();
            for node in &all_nodes {
                if let Ok(neighbors) = store.node_neighbors_detailed(
                    node.id,
                    "out",
                    Some(&["IMPORTS"]),
                    DEFAULT_LIMIT as i32,
                ) {
                    for (name, _qn, _label, file, _sl, _et) in neighbors {
                        imp.push(ImportInfo {
                            name: name.clone(),
                            source: file,
                        });
                    }
                }
            }
            imp.truncate(DEFAULT_LIMIT);
            if !notes.iter().any(|n| n.contains("import")) {
                notes.push("Imports inferred from IMPORTS edges".into());
            }
            Some(imp)
        } else {
            None
        };

        let exports = if include_exports {
            let exp: Vec<ExportInfo> = all_nodes
                .iter()
                .filter(|n| {
                    matches!(
                        n.label.as_str(),
                        "Function" | "Class" | "Interface" | "Method"
                    )
                })
                .take(DEFAULT_LIMIT)
                .map(|n| ExportInfo {
                    name: n.name.clone(),
                    kind: "inferred".into(),
                })
                .collect();
            notes.push("Exports inferred from top-level symbol definitions".into());
            Some(exp)
        } else {
            None
        };

        let graph_summary = if include_neighbors {
            let (inbound, outbound) = store
                .get_file_edge_counts(project, file_path)
                .unwrap_or((0, 0));
            Some(GraphSummary {
                inbound_edges: inbound,
                outbound_edges: outbound,
            })
        } else {
            None
        };

        Ok(FileOverview {
            project: project.to_string(),
            file: FileInfo {
                file_path: file_path.to_string(),
                language: lang_str,
                exists_in_index: exists,
            },
            symbols,
            symbol_count,
            imports,
            exports,
            graph_summary,
            notes: if notes.is_empty() { None } else { Some(notes) },
        })
    }

    pub fn find_entrypoints(
        store: &Store,
        project: &str,
        scope: Option<&str>,
        entry_type: Option<&str>,
        limit: i32,
    ) -> Result<EntrypointResult> {
        let limit = if limit <= 0 {
            DEFAULT_LIMIT
        } else {
            limit as usize
        };
        let et = entry_type.unwrap_or("any");
        let all_nodes = store.get_all_nodes(project)?;
        let mut candidates: Vec<EntrypointCandidate> = Vec::new();
        let mut seen = std::collections::HashSet::new();

        for node in &all_nodes {
            if matches!(
                node.label.as_str(),
                "Module" | "File" | "Folder" | "Project" | "Package"
            ) {
                continue;
            }
            if let Some(s) = scope {
                if !node.file_path.starts_with(s) {
                    continue;
                }
            }

            let fp_lower = node.file_path.to_lowercase();
            let name_lower = node.name.to_lowercase();
            let props = node.properties_json.as_deref().unwrap_or("{}");

            let mut ann_score: i32 = 0;
            let mut fp_score: i32 = 0;
            let mut name_score: i32 = 0;
            let mut reasons: Vec<&str> = Vec::new();

            // Annotation scoring (Java/Spring Boot)
            if props.contains("RestController") {
                ann_score += 40;
                reasons.push("@RestController");
            } else if props.contains("Controller") && !props.contains("ControllerAdvice") {
                ann_score += 35;
                reasons.push("@Controller");
            }
            if props.contains("GetMapping")
                || props.contains("PostMapping")
                || props.contains("PutMapping")
                || props.contains("PatchMapping")
                || props.contains("DeleteMapping")
                || props.contains("RequestMapping")
            {
                ann_score += 30;
                reasons.push("HTTP mapping");
            }
            if props.contains("\"Test\"")
                || props.contains("@Test")
                || name_lower.starts_with("test")
            {
                ann_score -= 40;
            }
            if props.contains("Configuration") {
                ann_score -= 25;
            }
            if props.contains("ExceptionHandler") || props.contains("ControllerAdvice") {
                ann_score -= 25;
            }

            // File path scoring
            if fp_lower.contains("controller") || fp_lower.contains("/api/") {
                fp_score += 20;
                reasons.push("controller/api package");
            } else if fp_lower.contains("route") || fp_lower.contains("handler") {
                fp_score += 15;
                reasons.push("route/handler path");
            }
            if fp_lower.contains("/test/")
                || fp_lower.contains("/tests/")
                || fp_lower.contains(".test.")
                || fp_lower.contains(".spec.")
            {
                fp_score -= 30;
            }

            // Name scoring
            let bootstrap = [
                "main",
                "bootstrap",
                "init",
                "start",
                "createserver",
                "app",
                "run",
            ];
            if bootstrap.iter().any(|p| name_lower == *p) {
                name_score += 25;
                reasons.push("bootstrap-style name");
            }

            // Route label
            if node.label == "Route" {
                ann_score += 35;
                reasons.push("Route node");
            }

            // entry_type filter
            let passes = match et {
                "http" | "route" => {
                    ann_score > 0
                        || node.label == "Route"
                        || fp_lower.contains("controller")
                        || fp_lower.contains("route")
                        || fp_lower.contains("handler")
                }
                "cli" => {
                    name_lower == "main"
                        || name_lower.contains("command")
                        || fp_lower.contains("cli")
                }
                "lambda" => {
                    name_lower == "handler"
                        || fp_lower.contains("handler")
                        || fp_lower.contains("lambda")
                }
                "bootstrap" => bootstrap.iter().any(|p| name_lower == *p),
                "public_api" => matches!(node.label.as_str(), "Function" | "Class" | "Interface"),
                _ => true,
            };
            if !passes {
                continue;
            }

            let raw = ann_score + fp_score + name_score;
            if raw <= 0 {
                continue;
            }
            let score = (raw as f64 / 100.0).clamp(0.0, 1.0);
            let reason = if reasons.is_empty() {
                "matched entry pattern".into()
            } else {
                reasons.join(", ")
            };

            if seen.insert(node.qualified_name.clone()) {
                candidates.push(EntrypointCandidate {
                    name: node.name.clone(),
                    qualified_name: node.qualified_name.clone(),
                    label: node.label.clone(),
                    file_path: node.file_path.clone(),
                    reason,
                    score,
                });
            }
        }

        candidates.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        candidates.truncate(limit);
        let count = candidates.len();
        Ok(EntrypointResult {
            project: project.to_string(),
            entry_type: et.to_string(),
            candidates,
            count,
        })
    }

    pub fn suggest_next_reads(
        store: &Store,
        project: &str,
        qualified_name: Option<&str>,
        file_path: Option<&str>,
        goal: Option<&str>,
        limit: i32,
    ) -> Result<SuggestionsResult> {
        let limit = if limit <= 0 {
            DEFAULT_LIMIT
        } else {
            limit as usize
        };
        let goal_str = goal.unwrap_or("understand");

        // Resolve origin nodes
        let origin_nodes = if let Some(qn) = qualified_name {
            match store.find_node_by_qn(project, qn)? {
                Some(n) => vec![n],
                None => return Err(anyhow::anyhow!("Symbol not found: {}", qn)),
            }
        } else if let Some(fp) = file_path {
            store.get_nodes_for_file(project, fp)?
        } else {
            return Err(anyhow::anyhow!("Provide qualified_name or file_path"));
        };

        if origin_nodes.is_empty() {
            return Err(anyhow::anyhow!("No nodes found for the given origin"));
        }

        let origin_json = if let Some(qn) = qualified_name {
            serde_json::json!({"qualified_name": qn})
        } else {
            serde_json::json!({"file_path": file_path.unwrap_or("")})
        };

        let mut suggestions: Vec<Suggestion> = Vec::new();
        let mut seen = std::collections::HashSet::new();

        let call_types: &[&str] = &["CALLS", "ASYNC_CALLS", "HTTP_CALLS"];
        let import_types: &[&str] = &["IMPORTS"];
        let inherit_types: &[&str] = &["INHERITS", "IMPLEMENTS"];

        for node in &origin_nodes {
            // Callees
            if let Ok(neighbors) =
                store.node_neighbors_detailed(node.id, "out", Some(call_types), limit as i32)
            {
                for (_name, qn, _label, fp, _sl, _et) in &neighbors {
                    if seen.insert(qn.clone()) {
                        let score = match goal_str {
                            "understand" | "trace" => 0.95,
                            "debug" => 0.9,
                            _ => 0.9,
                        };
                        suggestions.push(Suggestion {
                            kind: "symbol".into(),
                            file_path: Some(fp.clone()),
                            qualified_name: Some(qn.clone()),
                            reason: format!("callee of {}", node.name),
                            score,
                        });
                    }
                }
            }

            // Callers
            if let Ok(neighbors) =
                store.node_neighbors_detailed(node.id, "in", Some(call_types), limit as i32)
            {
                for (_name, qn, _label, fp, _sl, _et) in &neighbors {
                    if seen.insert(qn.clone()) {
                        let score = match goal_str {
                            "debug" | "refactor" => 0.95,
                            "trace" => 0.85,
                            _ => 0.85,
                        };
                        suggestions.push(Suggestion {
                            kind: "symbol".into(),
                            file_path: Some(fp.clone()),
                            qualified_name: Some(qn.clone()),
                            reason: format!("caller of {}", node.name),
                            score,
                        });
                    }
                }
            }

            // Imports
            if let Ok(neighbors) =
                store.node_neighbors_detailed(node.id, "out", Some(import_types), limit as i32)
            {
                for (_name, qn, _label, fp, _sl, _et) in &neighbors {
                    if seen.insert(qn.clone()) {
                        let score = match goal_str {
                            "understand" => 0.9,
                            "refactor" => 0.85,
                            _ => 0.8,
                        };
                        suggestions.push(Suggestion {
                            kind: "file".into(),
                            file_path: Some(fp.clone()),
                            qualified_name: Some(qn.clone()),
                            reason: "imported dependency".into(),
                            score,
                        });
                    }
                }
            }

            // Imported by
            if let Ok(neighbors) =
                store.node_neighbors_detailed(node.id, "in", Some(import_types), limit as i32)
            {
                for (_name, qn, _label, fp, _sl, _et) in &neighbors {
                    if seen.insert(qn.clone()) {
                        let score = match goal_str {
                            "refactor" => 0.9,
                            _ => 0.7,
                        };
                        suggestions.push(Suggestion {
                            kind: "file".into(),
                            file_path: Some(fp.clone()),
                            qualified_name: Some(qn.clone()),
                            reason: "imports the target".into(),
                            score,
                        });
                    }
                }
            }

            // Inheritance/implements
            if let Ok(neighbors) =
                store.node_neighbors_detailed(node.id, "out", Some(inherit_types), limit as i32)
            {
                for (_name, qn, _label, fp, _sl, _et) in &neighbors {
                    if seen.insert(qn.clone()) {
                        suggestions.push(Suggestion {
                            kind: "symbol".into(),
                            file_path: Some(fp.clone()),
                            qualified_name: Some(qn.clone()),
                            reason: "parent type".into(),
                            score: 0.85,
                        });
                    }
                }
            }
        }

        suggestions.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        suggestions.truncate(limit);
        let count = suggestions.len();

        Ok(SuggestionsResult {
            project: project.to_string(),
            goal: goal_str.to_string(),
            origin: origin_json,
            suggestions,
            count,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use codryn_store::{Edge, FileHash, Node, Project};

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
    fn test_file_overview() {
        let s = setup_store();
        s.upsert_file_hash_batch(&[FileHash {
            project: "p".into(),
            rel_path: "src/main.ts".into(),
            sha256: "abc".into(),
            mtime_ns: 0,
            size: 100,
        }])
        .unwrap();
        let id1 = insert_node(&s, "main", "src/main.ts::main", "Function", "src/main.ts");
        let id2 = insert_node(
            &s,
            "helper",
            "src/helper.ts::helper",
            "Function",
            "src/helper.ts",
        );
        insert_edge(&s, id1, id2, "CALLS");

        let ov = NavigationService::file_overview(&s, "p", "src/main.ts", true, true, true, true)
            .unwrap();
        assert!(ov.file.exists_in_index);
        assert_eq!(ov.symbol_count, 1);
        assert!(ov.symbols.unwrap().len() == 1);
        let gs = ov.graph_summary.unwrap();
        assert_eq!(gs.outbound_edges, 1);
    }

    #[test]
    fn test_find_entrypoints() {
        let s = setup_store();
        insert_node(&s, "main", "p::main", "Function", "src/main.ts");
        insert_node(&s, "helper", "p::helper", "Function", "src/utils.ts");
        insert_node(
            &s,
            "createServer",
            "p::createServer",
            "Function",
            "src/server.ts",
        );

        let res = NavigationService::find_entrypoints(&s, "p", None, None, 10).unwrap();
        assert!(res.count >= 2);
        assert_eq!(res.candidates[0].name, "main");
    }

    #[test]
    fn test_find_entrypoints_spring_boot() {
        let s = setup_store();
        // Controller with @RestController + @GetMapping should rank highest
        s.insert_node(&Node {
            id: 0, project: "p".into(), label: "Method".into(), name: "getUsers".into(),
            qualified_name: "p::getUsers".into(), file_path: "src/controller/UserController.java".into(),
            start_line: 10, end_line: 20,
            properties_json: Some(r#"{"annotations":["RestController"],"annotation":"GetMapping","http_method":"GET"}"#.into()),
        }).unwrap();
        // Test class should rank low
        s.insert_node(&Node {
            id: 0,
            project: "p".into(),
            label: "Method".into(),
            name: "testGetUsers".into(),
            qualified_name: "p::testGetUsers".into(),
            file_path: "src/test/UserControllerTest.java".into(),
            start_line: 5,
            end_line: 15,
            properties_json: Some(r#"{"annotations":["Test"]}"#.into()),
        })
        .unwrap();
        // Config class should rank low
        s.insert_node(&Node {
            id: 0,
            project: "p".into(),
            label: "Class".into(),
            name: "AppConfig".into(),
            qualified_name: "p::AppConfig".into(),
            file_path: "src/config/AppConfig.java".into(),
            start_line: 1,
            end_line: 30,
            properties_json: Some(r#"{"annotations":["Configuration"]}"#.into()),
        })
        .unwrap();

        let res = NavigationService::find_entrypoints(&s, "p", None, Some("http"), 10).unwrap();
        assert!(!res.candidates.is_empty());
        assert_eq!(res.candidates[0].name, "getUsers");
        // Test and config should not appear in http filter
        assert!(!res.candidates.iter().any(|c| c.name == "testGetUsers"));
    }

    #[test]
    fn test_suggest_next_reads() {
        let s = setup_store();
        let a = insert_node(&s, "funcA", "p::funcA", "Function", "src/a.ts");
        let b = insert_node(&s, "funcB", "p::funcB", "Function", "src/b.ts");
        let c = insert_node(&s, "modC", "p::modC", "Module", "src/c.ts");
        insert_edge(&s, a, b, "CALLS");
        insert_edge(&s, a, c, "IMPORTS");

        let res = NavigationService::suggest_next_reads(
            &s,
            "p",
            Some("p::funcA"),
            None,
            Some("understand"),
            10,
        )
        .unwrap();
        assert!(res.count >= 2);
        // Callee should be ranked high
        assert!(res
            .suggestions
            .iter()
            .any(|s| s.qualified_name.as_deref() == Some("p::funcB")));
    }
}
