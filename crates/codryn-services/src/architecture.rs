use anyhow::Result;
use codryn_store::Store;
use serde::Serialize;
use std::collections::HashMap;

pub struct ArchitectureService;

#[derive(Debug, Serialize)]
pub struct ArchitectureResult {
    pub project: String,
    pub layers: Vec<Layer>,
    pub total_files: usize,
}

#[derive(Debug, Serialize)]
pub struct Layer {
    pub name: String,
    pub files: Vec<String>,
    pub total_count: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub inbound_edges: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub outbound_edges: Option<i64>,
}

const LAYER_RULES: &[(&str, &[&str])] = &[
    (
        "controllers",
        &["controller", "route", "handler", "resource", "endpoint"],
    ),
    (
        "services",
        &["service", "usecase", "use_case", "interactor"],
    ),
    (
        "repositories",
        &["repository", "repo", "dao", "store", "persistence"],
    ),
    ("models", &["model", "entity", "dto", "domain", "schema"]),
    (
        "components",
        &[".component.ts", ".component.tsx", ".component.js"],
    ),
    ("modules", &[".module.ts"]),
    ("directives", &[".directive.ts"]),
    ("pipes", &[".pipe.ts"]),
    (
        "config",
        &["config", "configuration", "properties", "settings"],
    ),
    (
        "middleware",
        &["middleware", "interceptor", "filter", "guard"],
    ),
    ("utils", &["util", "helper", "common", "shared", "lib"]),
];

const MAX_FILES_PER_LAYER: usize = 10;

fn classify_file(path: &str) -> Option<&'static str> {
    let p = path.to_lowercase();
    for (layer_name, patterns) in LAYER_RULES {
        if patterns.iter().any(|pat| p.contains(pat)) {
            return Some(layer_name);
        }
    }
    None
}

/// Map node-level layer property to architecture layer name.
fn node_layer_to_arch(layer: &str) -> &'static str {
    match layer {
        "controller" => "controllers",
        "service" => "services",
        "repository" => "repositories",
        "dto" | "entity" | "model" => "models",
        "config" => "config",
        "validator" => "middleware",
        _ => "other",
    }
}

/// Extract layer from node properties JSON.
fn classify_node_layer(properties_json: &str) -> Option<String> {
    serde_json::from_str::<serde_json::Value>(properties_json)
        .ok()?
        .get("layer")?
        .as_str()
        .map(|s| s.to_string())
}

impl ArchitectureService {
    pub fn get_architecture(store: &Store, project: &str) -> Result<ArchitectureResult> {
        let files = store.list_files(project)?;
        let total_files = files.len();
        let mut groups: HashMap<&'static str, Vec<String>> = HashMap::new();
        let mut classified = std::collections::HashSet::new();

        // First pass: classify files using node layer properties (from AST extraction)
        for f in &files {
            let nodes = store.get_nodes_for_file(project, f)?;
            for n in &nodes {
                if let Some(ref props) = n.properties_json {
                    if let Some(layer) = classify_node_layer(props) {
                        let arch_layer = node_layer_to_arch(&layer);
                        groups.entry(arch_layer).or_default().push(f.clone());
                        classified.insert(f.clone());
                        break;
                    }
                }
            }
        }

        // Second pass: path-based classification for files not yet classified
        for f in &files {
            if classified.contains(f) {
                continue;
            }
            if let Some(layer) = classify_file(f) {
                groups.entry(layer).or_default().push(f.clone());
            }
        }

        // If no files matched any layer, fall back to top-level directory grouping
        if groups.is_empty() && !files.is_empty() {
            for f in &files {
                let dir = f.split('/').next().unwrap_or("root");
                // Use a static leak trick is bad — just use "other" bucket
                groups.entry("other").or_default().push(f.clone());
                let _ = dir; // suppress warning
            }
        }

        let mut layers: Vec<Layer> = groups
            .into_iter()
            .map(|(name, mut file_list)| {
                let total_count = file_list.len();
                file_list.sort();
                file_list.truncate(MAX_FILES_PER_LAYER);

                // Aggregate edge counts for the layer
                let (mut inbound, mut outbound) = (0i64, 0i64);
                for f in &file_list {
                    if let Ok((i, o)) = store.get_file_edge_counts(project, f) {
                        inbound += i;
                        outbound += o;
                    }
                }

                Layer {
                    name: name.to_string(),
                    files: file_list,
                    total_count,
                    inbound_edges: Some(inbound),
                    outbound_edges: Some(outbound),
                }
            })
            .collect();

        // Sort layers by conventional order: controllers first, then services, repos, models, rest
        let order = |name: &str| -> usize {
            match name {
                "controllers" => 0,
                "services" => 1,
                "repositories" => 2,
                "models" => 3,
                "components" => 4,
                "modules" => 5,
                "middleware" => 6,
                "config" => 7,
                "utils" => 8,
                _ => 9,
            }
        };
        layers.sort_by_key(|l| order(&l.name));

        Ok(ArchitectureResult {
            project: project.to_string(),
            layers,
            total_files,
        })
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
            indexed_at: "now".into(),
            root_path: "/".into(),
        })
        .unwrap();
        s
    }

    fn add_node(s: &Store, fp: &str) {
        s.insert_node(&Node {
            id: 0,
            project: "p".into(),
            label: "Function".into(),
            name: fp.split('/').next_back().unwrap_or("f").into(),
            qualified_name: format!("p::{}", fp),
            file_path: fp.into(),
            start_line: 1,
            end_line: 10,
            properties_json: None,
        })
        .unwrap();
    }

    #[test]
    fn test_architecture_layer_detection() {
        let s = setup();
        add_node(&s, "src/controller/UserController.java");
        add_node(&s, "src/service/UserService.java");
        add_node(&s, "src/repository/UserRepo.java");
        add_node(&s, "src/model/User.java");

        let res = ArchitectureService::get_architecture(&s, "p").unwrap();
        assert_eq!(res.total_files, 4);
        assert!(res.layers.iter().any(|l| l.name == "controllers"));
        assert!(res.layers.iter().any(|l| l.name == "services"));
        assert!(res.layers.iter().any(|l| l.name == "repositories"));
        assert!(res.layers.iter().any(|l| l.name == "models"));
        // Controllers should come first
        assert_eq!(res.layers[0].name, "controllers");
    }

    #[test]
    fn test_architecture_angular() {
        let s = setup();
        add_node(&s, "src/app/user.component.ts");
        add_node(&s, "src/app/user.service.ts");
        add_node(&s, "src/app/app.module.ts");

        let res = ArchitectureService::get_architecture(&s, "p").unwrap();
        assert!(res.layers.iter().any(|l| l.name == "components"));
        assert!(res.layers.iter().any(|l| l.name == "services"));
        assert!(res.layers.iter().any(|l| l.name == "modules"));
    }

    #[test]
    fn test_architecture_never_empty() {
        let s = setup();
        add_node(&s, "src/foo/bar.ts");

        let res = ArchitectureService::get_architecture(&s, "p").unwrap();
        assert!(!res.layers.is_empty());
    }

    #[test]
    fn test_architecture_spring_boot_node_layers() {
        let s = setup();
        // Node with layer property but file path doesn't contain "controller"
        s.insert_node(&Node {
            id: 0,
            project: "p".into(),
            label: "Class".into(),
            name: "UserController".into(),
            qualified_name: "p.src.web.UserController".into(),
            file_path: "src/web/UserController.java".into(),
            start_line: 1,
            end_line: 50,
            properties_json: Some(
                r#"{"layer":"controller","annotations":["RestController"]}"#.into(),
            ),
        })
        .unwrap();
        s.insert_node(&Node {
            id: 0,
            project: "p".into(),
            label: "Class".into(),
            name: "UserService".into(),
            qualified_name: "p.src.logic.UserService".into(),
            file_path: "src/logic/UserService.java".into(),
            start_line: 1,
            end_line: 30,
            properties_json: Some(r#"{"layer":"service"}"#.into()),
        })
        .unwrap();

        let res = ArchitectureService::get_architecture(&s, "p").unwrap();
        assert!(
            res.layers.iter().any(|l| l.name == "controllers"),
            "should classify via node layer even without 'controller' in path"
        );
        assert!(
            res.layers.iter().any(|l| l.name == "services"),
            "should classify via node layer even without 'service' in path"
        );
    }
}
