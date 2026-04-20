//! React / Solid.js: tag symbols with `framework` and add coarse `RENDERS` edges from JSX.

use codryn_discover::{DiscoveredFile, Language};
use codryn_foundation::fqn;
use codryn_graph_buffer::GraphBuffer;
use codryn_store::Store;
use regex::Regex;
use std::collections::HashMap;
use std::sync::LazyLock;

static IMPORT_NAMED: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?m)^import\s+\{([^}]+)\}\s+from\s+["']([^"']+)["']"#).unwrap()
});
static IMPORT_DEFAULT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?m)^import\s+(\w+)\s+from\s+["']([^"']+)["']"#).unwrap()
});
static JSX_TAG: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"<([A-Z][A-Za-z0-9_]*)[\s/>]").unwrap());

fn likely_jsx(source: &str) -> bool {
    source.contains('<') && JSX_TAG.is_match(source)
}

fn detect_framework(source: &str) -> Option<&'static str> {
    let solid = source.contains("from \"solid-js")
        || source.contains("from 'solid-js")
        || source.contains("from \"solid-js/")
        || source.contains("from 'solid-js/");
    let react = source.contains("from \"react\"")
        || source.contains("from 'react'")
        || source.contains("from \"react/")
        || source.contains("from 'react/");
    if solid && !react {
        return Some("solid");
    }
    if solid && react {
        // Prefer Solid when solid-js hooks present
        if source.contains("createSignal") || source.contains("createMemo") {
            return Some("solid");
        }
    }
    if react {
        return Some("react");
    }
    if solid {
        return Some("solid");
    }
    None
}

fn parse_import_map(source: &str) -> HashMap<String, String> {
    let mut m: HashMap<String, String> = HashMap::new();
    for caps in IMPORT_NAMED.captures_iter(source) {
        let names = caps.get(1).map(|g| g.as_str()).unwrap_or("");
        let path = caps.get(2).map(|g| g.as_str()).unwrap_or("").to_string();
        for part in names.split(',') {
            let part = part.trim();
            let name = part.split_whitespace().last().unwrap_or(part).trim();
            let name = name.trim_start_matches('{').trim_end_matches('}');
            if !name.is_empty() && name != "type" {
                m.insert(name.to_string(), path.clone());
            }
        }
    }
    for caps in IMPORT_DEFAULT.captures_iter(source) {
        let name = caps.get(1).map(|g| g.as_str()).unwrap_or("");
        let path = caps.get(2).map(|g| g.as_str()).unwrap_or("").to_string();
        if !name.is_empty() && name != "type" {
            m.insert(name.to_string(), path);
        }
    }
    m
}

fn resolve_tag_to_qn(store: &Store, project: &str, tag: &str) -> Option<String> {
    let nodes = store.search_nodes(project, tag, 8).ok()?;
    nodes
        .into_iter()
        .find(|n| matches!(n.label.as_str(), "Class" | "Function"))
        .map(|n| n.qualified_name)
}

/// Add `RENDERS` edges from likely root component in this file to resolved children.
pub fn pass_jsx_framework(
    buf: &mut GraphBuffer,
    store: &Store,
    files: &[&DiscoveredFile],
    project: &str,
) {
    for f in files {
        if !matches!(
            f.language,
            Language::TypeScript | Language::Tsx | Language::JavaScript
        ) {
            continue;
        }
        let rel = f.rel_path.replace('\\', "/");
        if rel.ends_with(".component.ts") || rel.ends_with(".component.tsx") {
            continue;
        }
        let source = match std::fs::read_to_string(&f.abs_path) {
            Ok(s) => s,
            Err(_) => continue,
        };
        if !likely_jsx(&source) {
            continue;
        }
        if detect_framework(&source).is_none() {
            continue;
        }
        let imports = parse_import_map(&source);
        let own_nodes = store.get_nodes_for_file(project, &f.rel_path).unwrap_or_default();
        let root = own_nodes
            .iter()
            .find(|n| {
                (n.label == "Function" || n.label == "Class")
                    && n.name.chars().next().is_some_and(|c| c.is_uppercase())
            })
            .or_else(|| own_nodes.iter().find(|n| n.label == "Function"))
            .or_else(|| own_nodes.iter().find(|n| n.label == "Class"));
        let Some(root) = root else {
            continue;
        };
        let src_qn = root.qualified_name.clone();
        let mut seen_tags = std::collections::HashSet::new();
        for caps in JSX_TAG.captures_iter(&source) {
            let tag = caps.get(1).unwrap().as_str();
            if tag == "Fragment" || tag.starts_with("Suspense") {
                continue;
            }
            if !seen_tags.insert(tag.to_string()) {
                continue;
            }
            let tgt_qn = if let Some(spec) = imports.get(tag) {
                if spec.starts_with('.') {
                    let base = std::path::Path::new(&f.rel_path)
                        .parent()
                        .unwrap_or(std::path::Path::new(""));
                    let joined = base.join(spec.trim_start_matches('.'));
                    let resolved = joined.to_string_lossy().replace('\\', "/");
                    let stem = std::path::Path::new(&resolved)
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or(tag);
                    fqn::fqn_compute(project, &resolved, Some(stem))
                } else {
                    format!("{project}.{tag}")
                }
            } else {
                resolve_tag_to_qn(store, project, tag)
                    .unwrap_or_else(|| format!("{project}.{tag}"))
            };
            buf.add_edge_by_qn(&src_qn, &tgt_qn, "RENDERS", None);
        }
    }
}

/// Merge `framework` into Class/Function nodes for TS/TSX files that use React or Solid.
pub fn pass_jsx_framework_props(store: &Store, files: &[&DiscoveredFile], project: &str) {
    for f in files {
        if !matches!(
            f.language,
            Language::TypeScript | Language::Tsx | Language::JavaScript
        ) {
            continue;
        }
        let rel = f.rel_path.replace('\\', "/");
        if rel.ends_with(".component.ts") || rel.ends_with(".component.tsx") {
            continue;
        }
        let source = match std::fs::read_to_string(&f.abs_path) {
            Ok(s) => s,
            Err(_) => continue,
        };
        let Some(framework) = detect_framework(&source) else {
            continue;
        };
        if !likely_jsx(&source) {
            continue;
        }
        let nodes = match store.get_nodes_for_file(project, &f.rel_path) {
            Ok(n) => n,
            Err(_) => continue,
        };
        for n in nodes {
            if n.label != "Function" && n.label != "Class" {
                continue;
            }
            let mut v: serde_json::Value =
                serde_json::from_str(n.properties_json.as_deref().unwrap_or("{}")).unwrap_or_default();
            if v.get("decorator").is_some() {
                continue;
            }
            if v.get("framework").is_some() {
                continue;
            }
            v["framework"] = serde_json::Value::String(framework.into());
            v["layer"] = serde_json::Value::String("component".into());
            let _ = store.update_node_properties(n.id, &v.to_string());
        }
    }
}
