/// Track 5.3 — `get_project_summary` service.
///
/// Returns a structured onboarding brief for a project in a single call,
/// collapsing what would otherwise require 4–5 separate tool calls
/// (`get_architecture`, `find_entrypoints`, `sample_graph`, `find_routes`,
/// `get_graph_schema`).
use anyhow::Result;
use codryn_store::Store;
use serde::Serialize;
use std::collections::HashMap;

pub struct ProjectSummaryService;

#[derive(Debug, Serialize)]
pub struct ProjectSummary {
    pub project: String,
    pub root_path: String,
    pub last_indexed: String,
    pub stats: GraphStats,
    pub languages: Vec<LanguageStat>,
    pub architecture: ArchSummary,
    pub top_symbols: Vec<TopSymbol>,
    pub entry_points: Vec<EntryPoint>,
    pub route_count: usize,
    pub linked_projects: Vec<String>,
    pub detected_patterns: Vec<String>,
    pub suggested_first_reads: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub warnings: Option<Vec<String>>,
}

#[derive(Debug, Serialize)]
pub struct GraphStats {
    pub total_nodes: i64,
    pub total_edges: i64,
    pub function_count: i64,
    pub class_count: i64,
    pub file_count: i64,
}

#[derive(Debug, Serialize)]
pub struct LanguageStat {
    pub language: String,
    pub file_count: usize,
    pub percentage: f64,
}

#[derive(Debug, Serialize)]
pub struct ArchSummary {
    pub layers: Vec<String>,
    pub total_files: usize,
}

#[derive(Debug, Serialize)]
pub struct TopSymbol {
    pub name: String,
    pub qualified_name: String,
    pub label: String,
    pub file_path: String,
    pub fan_in: i64,
    pub fan_out: i64,
}

#[derive(Debug, Serialize)]
pub struct EntryPoint {
    pub name: String,
    pub qualified_name: String,
    pub label: String,
    pub file_path: String,
    pub reason: String,
}

impl ProjectSummaryService {
    pub fn get_summary(store: &Store, project: &str) -> Result<ProjectSummary> {
        // --- Project metadata ---
        let projects = store.list_projects().unwrap_or_default();
        let proj_meta = projects.iter().find(|p| p.name == project);
        let root_path = proj_meta
            .map(|p| p.root_path.as_str())
            .unwrap_or("")
            .to_string();
        let last_indexed = proj_meta
            .map(|p| p.indexed_at.as_str())
            .unwrap_or("")
            .to_string();

        // --- Graph stats ---
        let schema = store
            .get_graph_schema(project)
            .unwrap_or_else(|_| codryn_store::SchemaInfo {
                node_labels: vec![],
                edge_types: vec![],
                total_nodes: 0,
                total_edges: 0,
            });
        let total_nodes = schema.total_nodes;
        let total_edges = schema.total_edges;
        let function_count = schema
            .node_labels
            .iter()
            .find(|l| l.label == "Function")
            .map(|l| l.count)
            .unwrap_or(0)
            + schema
                .node_labels
                .iter()
                .find(|l| l.label == "Method")
                .map(|l| l.count)
                .unwrap_or(0);
        let class_count = schema
            .node_labels
            .iter()
            .find(|l| l.label == "Class")
            .map(|l| l.count)
            .unwrap_or(0)
            + schema
                .node_labels
                .iter()
                .find(|l| l.label == "Interface")
                .map(|l| l.count)
                .unwrap_or(0);
        let file_count = schema
            .node_labels
            .iter()
            .find(|l| l.label == "File")
            .map(|l| l.count)
            .unwrap_or(0);

        let stats = GraphStats {
            total_nodes,
            total_edges,
            function_count,
            class_count,
            file_count,
        };

        // --- Language breakdown ---
        let files = store.list_files(project).unwrap_or_default();
        let total_files = files.len();
        let mut lang_counts: HashMap<String, usize> = HashMap::new();
        for f in &files {
            let lang = codryn_discover::detect_language(f);
            if lang != codryn_discover::Language::Unknown {
                *lang_counts.entry(lang.name().to_string()).or_insert(0) += 1;
            }
        }
        let mut languages: Vec<LanguageStat> = lang_counts
            .into_iter()
            .map(|(language, count)| LanguageStat {
                percentage: if total_files > 0 {
                    (count as f64 / total_files as f64 * 100.0 * 10.0).round() / 10.0
                } else {
                    0.0
                },
                language,
                file_count: count,
            })
            .collect();
        languages.sort_by_key(|a| std::cmp::Reverse(a.file_count));
        languages.truncate(10);

        // --- Architecture layers ---
        let arch_result =
            crate::architecture::ArchitectureService::get_architecture(store, project)
                .unwrap_or_else(|_| crate::architecture::ArchitectureResult {
                    project: project.to_string(),
                    layers: vec![],
                    total_files: 0,
                });
        let arch = ArchSummary {
            layers: arch_result.layers.iter().map(|l| l.name.clone()).collect(),
            total_files: arch_result.total_files,
        };

        // --- Top symbols by centrality (fan_in + fan_out) ---
        let top_symbols = Self::get_top_symbols(store, project, 10);

        // --- Entry points ---
        let entry_points = Self::get_entry_points(store, project, 5);

        // --- Route count ---
        let route_count = schema
            .node_labels
            .iter()
            .find(|l| l.label == "Route")
            .map(|l| l.count as usize)
            .unwrap_or(0);

        // --- Linked projects ---
        let linked_projects = store
            .get_linked_projects(project)
            .unwrap_or_default()
            .into_iter()
            .map(|p| p.target_project)
            .collect();

        // --- Detected patterns ---
        let detected_patterns = Self::detect_patterns(&schema, &arch_result, &languages);

        // --- Suggested first reads ---
        let suggested_first_reads = Self::suggest_first_reads(&files, &root_path);

        // --- Warnings ---
        let mut warnings = Vec::new();
        if total_nodes > 10 && total_edges == 0 {
            warnings.push("Graph has nodes but no edges — indexing may be incomplete".into());
        }
        if total_nodes == 0 {
            warnings.push("Project has no indexed nodes — run index_repository first".into());
        }

        Ok(ProjectSummary {
            project: project.to_string(),
            root_path,
            last_indexed,
            stats,
            languages,
            architecture: arch,
            top_symbols,
            entry_points,
            route_count,
            linked_projects,
            detected_patterns,
            suggested_first_reads,
            warnings: if warnings.is_empty() {
                None
            } else {
                Some(warnings)
            },
        })
    }

    fn get_top_symbols(store: &Store, project: &str, limit: usize) -> Vec<TopSymbol> {
        // Use node_degrees_bulk for efficiency
        let degrees = match store.node_degrees_bulk(project) {
            Ok(d) => d,
            Err(_) => return vec![],
        };

        // Get all function/class/method nodes
        let all_nodes = match store.get_all_nodes(project) {
            Ok(n) => n,
            Err(_) => return vec![],
        };

        let mut scored: Vec<(i64, i64, &codryn_store::Node)> = all_nodes
            .iter()
            .filter(|n| {
                matches!(
                    n.label.as_str(),
                    "Function" | "Method" | "Class" | "Interface"
                )
            })
            .map(|n| {
                let (fan_in, fan_out) = degrees.get(&n.id).copied().unwrap_or((0, 0));
                (fan_in as i64 + fan_out as i64, fan_in as i64, n)
            })
            .collect();

        scored.sort_by_key(|a| std::cmp::Reverse(a.0));
        scored.truncate(limit);

        scored
            .into_iter()
            .map(|(_, fan_in, n)| {
                let (_, fan_out) = degrees.get(&n.id).copied().unwrap_or((0, 0));
                TopSymbol {
                    name: n.name.clone(),
                    qualified_name: n.qualified_name.clone(),
                    label: n.label.clone(),
                    file_path: n.file_path.clone(),
                    fan_in,
                    fan_out: fan_out as i64,
                }
            })
            .collect()
    }

    fn get_entry_points(store: &Store, project: &str, limit: usize) -> Vec<EntryPoint> {
        let result = crate::navigation::NavigationService::find_entrypoints(
            store,
            project,
            None,
            Some("any"),
            limit as i32,
        );
        match result {
            Ok(r) => r
                .candidates
                .into_iter()
                .map(|c| EntryPoint {
                    name: c.name,
                    qualified_name: c.qualified_name,
                    label: c.label,
                    file_path: c.file_path,
                    reason: c.reason,
                })
                .collect(),
            Err(_) => vec![],
        }
    }

    fn detect_patterns(
        schema: &codryn_store::SchemaInfo,
        arch: &crate::architecture::ArchitectureResult,
        languages: &[LanguageStat],
    ) -> Vec<String> {
        let mut patterns = Vec::new();
        let layer_names: Vec<&str> = arch.layers.iter().map(|l| l.name.as_str()).collect();
        // MVC pattern
        if layer_names.contains(&"controllers")
            && layer_names.contains(&"models")
            && (layer_names.contains(&"services") || layer_names.contains(&"repositories"))
        {
            patterns.push("MVC / layered architecture".into());
        }

        // Repository pattern
        if layer_names.contains(&"repositories") {
            patterns.push("Repository pattern".into());
        }

        // Service layer
        if layer_names.contains(&"services") {
            patterns.push("Service layer".into());
        }

        // Angular SPA
        if layer_names.contains(&"components") && layer_names.contains(&"modules") {
            patterns.push("Angular SPA".into());
        }

        // Spring Boot
        let has_java = languages.iter().any(|l| l.language == "Java");
        let has_kotlin = languages.iter().any(|l| l.language == "Kotlin");
        let has_routes = schema
            .edge_types
            .iter()
            .any(|e| e.edge_type == "HANDLES_ROUTE");
        if (has_java || has_kotlin) && has_routes {
            patterns.push("Spring Boot REST API".into());
        }

        // Go service
        let has_go = languages.iter().any(|l| l.language == "Go");
        if has_go && has_routes {
            patterns.push("Go HTTP service".into());
        }

        // Microservice indicators
        if schema.node_labels.iter().any(|l| l.label == "Route") && arch.total_files < 100 {
            patterns.push("Microservice (small footprint)".into());
        }

        // Event-driven
        let has_events = schema
            .edge_types
            .iter()
            .any(|e| e.edge_type == "EMITS" || e.edge_type == "SUBSCRIBES_TO");
        if has_events {
            patterns.push("Event-driven".into());
        }

        patterns
    }

    fn suggest_first_reads(files: &[String], root_path: &str) -> Vec<String> {
        let mut suggestions = Vec::new();
        let priority_files = [
            "README.md",
            "README.rst",
            "README.txt",
            "main.go",
            "main.rs",
            "main.py",
            "main.ts",
            "main.js",
            "index.ts",
            "index.js",
            "app.ts",
            "app.js",
            "app.py",
            "Application.java",
            "Main.java",
            "src/main.rs",
            "cmd/main.go",
        ];

        for priority in &priority_files {
            if files.iter().any(|f| {
                f == priority
                    || f.ends_with(&format!("/{}", priority))
                    || f == &priority.to_lowercase()
            }) {
                suggestions.push(priority.to_string());
                if suggestions.len() >= 5 {
                    break;
                }
            }
        }

        // Add config files if not already suggested
        let config_patterns = [
            "config.toml",
            "config.yaml",
            "application.yml",
            ".env.example",
        ];
        for pat in &config_patterns {
            if suggestions.len() >= 5 {
                break;
            }
            if files.iter().any(|f| f.ends_with(pat)) {
                suggestions.push(pat.to_string());
            }
        }

        let _ = root_path; // used for future path resolution
        suggestions
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use codryn_store::{Node, Project};

    fn setup() -> Store {
        let s = Store::open_in_memory().unwrap();
        s.upsert_project(&Project {
            name: "p".into(),
            indexed_at: "2024-01-01T00:00:00Z".into(),
            root_path: "/tmp/p".into(),
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
    fn test_summary_empty_project() {
        let s = setup();
        let summary = ProjectSummaryService::get_summary(&s, "p").unwrap();
        assert_eq!(summary.project, "p");
        assert_eq!(summary.stats.total_nodes, 0);
        assert!(summary.warnings.is_some());
    }

    #[test]
    fn test_summary_with_nodes() {
        let s = setup();
        add_node(&s, "main", "Function", "src/main.rs");
        add_node(&s, "UserService", "Class", "src/service/user.rs");
        add_node(&s, "UserController", "Class", "src/controller/user.rs");

        let summary = ProjectSummaryService::get_summary(&s, "p").unwrap();
        assert_eq!(summary.stats.total_nodes, 3);
        assert!(!summary.languages.is_empty());
        assert!(summary.languages.iter().any(|l| l.language == "Rust"));
    }

    #[test]
    fn test_summary_top_symbols() {
        let s = setup();
        let a = add_node(&s, "funcA", "Function", "src/a.rs");
        let b = add_node(&s, "funcB", "Function", "src/b.rs");
        let c = add_node(&s, "funcC", "Function", "src/c.rs");
        // funcA has highest centrality
        s.insert_edge(&codryn_store::Edge {
            id: 0,
            project: "p".into(),
            source_id: b,
            target_id: a,
            edge_type: "CALLS".into(),
            properties_json: None,
        })
        .unwrap();
        s.insert_edge(&codryn_store::Edge {
            id: 0,
            project: "p".into(),
            source_id: c,
            target_id: a,
            edge_type: "CALLS".into(),
            properties_json: None,
        })
        .unwrap();

        let summary = ProjectSummaryService::get_summary(&s, "p").unwrap();
        assert!(!summary.top_symbols.is_empty());
        assert_eq!(summary.top_symbols[0].name, "funcA");
    }
}
