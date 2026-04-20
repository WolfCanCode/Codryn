//! Vue single-file components: synthetic component node, `framework`, and `RENDERS` edges.

use codryn_discover::{DiscoveredFile, Language};
use codryn_foundation::fqn;
use codryn_graph_buffer::GraphBuffer;
use codryn_store::Store;
use regex::Regex;
use std::collections::HashMap;
use std::sync::LazyLock;

static IMPORT_LINE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(?m)^import\s+(.+)\s+from\s+["']([^"']+)["']"#).unwrap());
static TEMPLATE_TAG: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"<([A-Z][A-Za-z0-9-]*|[a-z][a-z0-9]*(?:-[a-z0-9]+)+)\b"#).unwrap());

const HTML_TAGS: &[&str] = &[
    "div", "span", "p", "a", "img", "ul", "ol", "li", "button", "input", "form", "label",
    "select", "option", "textarea", "table", "tr", "td", "th", "thead", "tbody", "svg", "path",
    "template", "slot", "router-view", "router-link",
];

fn extract_block<'a>(source: &'a str, open: &str, close: &str) -> Option<&'a str> {
    let i = source.find(open)?;
    let rest = &source[i + open.len()..];
    let j = rest.find(close)?;
    Some(&rest[..j])
}

fn file_stem_pascal(rel_path: &str) -> String {
    let stem = std::path::Path::new(rel_path)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("Anonymous");
    kebab_to_pascal(stem)
}

fn kebab_to_pascal(s: &str) -> String {
    s.split(|c: char| c == '-' || c == '_')
        .filter(|p| !p.is_empty())
        .map(|p| {
            let mut c = p.chars();
            match c.next() {
                None => String::new(),
                Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
            }
        })
        .collect()
}

fn parse_vue_imports(script: &str) -> HashMap<String, String> {
    let mut m = HashMap::new();
    for caps in IMPORT_LINE.captures_iter(script) {
        let lhs = caps.get(1).map(|g| g.as_str()).unwrap_or("");
        let path = caps.get(2).map(|g| g.as_str()).unwrap_or("").to_string();
        if lhs.starts_with('{') {
            let inner = lhs.trim_start_matches('{').trim_end_matches('}');
            for part in inner.split(',') {
                let name = part.trim().split_whitespace().last().unwrap_or("").trim();
                if !name.is_empty() {
                    m.insert(name.to_string(), path.clone());
                }
            }
        } else {
            let name = lhs.split_whitespace().next().unwrap_or("").trim();
            if !name.is_empty() {
                m.insert(name.to_string(), path);
            }
        }
    }
    m
}

fn is_component_tag(tag: &str) -> bool {
    if tag.is_empty() {
        return false;
    }
    if HTML_TAGS.contains(&tag.to_lowercase().as_str()) {
        return false;
    }
    tag.chars().next().is_some_and(|c| c.is_uppercase()) || tag.contains('-')
}

pub fn pass_vue_sfc(
    buf: &mut GraphBuffer,
    store: &Store,
    files: &[&DiscoveredFile],
    project: &str,
) {
    for f in files {
        if f.language != Language::Vue {
            continue;
        }
        let source = match std::fs::read_to_string(&f.abs_path) {
            Ok(s) => s,
            Err(_) => continue,
        };
        let script_raw = extract_block(&source, "<script setup", "</script>")
            .or_else(|| extract_block(&source, "<script", "</script>"));
        let Some(script_raw) = script_raw else {
            continue;
        };
        let script = script_raw
            .find('>')
            .map(|i| script_raw[i + 1..].trim())
            .unwrap_or(script_raw.trim());
        let template_raw = extract_block(&source, "<template>", "</template>")
            .or_else(|| extract_block(&source, "<template ", "</template>"));
        let Some(template_raw) = template_raw else {
            continue;
        };
        let template = template_raw
            .find('>')
            .map(|i| template_raw[i + 1..].trim())
            .unwrap_or(template_raw.trim());

        let comp_name = file_stem_pascal(&f.rel_path);
        let comp_qn = fqn::fqn_compute(project, &f.rel_path, Some(&comp_name));
        let props = serde_json::json!({
            "framework": "vue",
            "layer": "component",
            "selector": comp_name
        })
        .to_string();
        let lines = source.lines().count() as i32;
        buf.add_node(
            "Class",
            &comp_name,
            &comp_qn,
            &f.rel_path,
            1,
            lines.max(1),
            Some(props),
        );

        let imports = parse_vue_imports(script);
        let mut seen = std::collections::HashSet::new();
        for caps in TEMPLATE_TAG.captures_iter(template) {
            let raw = caps.get(1).unwrap().as_str();
            if !is_component_tag(raw) {
                continue;
            }
            let pascal = if raw.contains('-') {
                kebab_to_pascal(raw)
            } else {
                raw.to_string()
            };
            if !seen.insert(pascal.clone()) {
                continue;
            }
            let tgt = if let Some(spec) = imports.get(&pascal).or_else(|| imports.get(raw)) {
                if spec.starts_with('.') {
                    let base = std::path::Path::new(&f.rel_path)
                        .parent()
                        .unwrap_or(std::path::Path::new(""));
                    let joined = base.join(spec.trim_start_matches('.'));
                    let resolved = joined.to_string_lossy().replace('\\', "/");
                    let stem = std::path::Path::new(&resolved)
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or(&pascal);
                    fqn::fqn_compute(project, &resolved, Some(&kebab_to_pascal(stem)))
                } else {
                    format!("{project}.{pascal}")
                }
            } else {
                store
                    .search_nodes(project, &pascal, 6)
                    .ok()
                    .and_then(|ns| {
                        ns.into_iter()
                            .find(|n| matches!(n.label.as_str(), "Class" | "Function"))
                            .map(|n| n.qualified_name)
                    })
                    .unwrap_or_else(|| format!("{project}.{pascal}"))
            };
            buf.add_edge_by_qn(&comp_qn, &tgt, "RENDERS", None);
        }
    }
}
