use anyhow::Result;
use codryn_discover::{DiscoveredFile, Language};
use codryn_foundation::fqn;
use codryn_graph_buffer::GraphBuffer;
use codryn_store::Store;
use rayon::prelude::*;
use std::collections::HashSet;
use std::path::Path;
use std::sync::LazyLock;

use crate::registry::Registry;

static CALL_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"\b(\w+)\s*\(").unwrap());

/// Pass 1: Create structure nodes (Project, Folder, File).
pub fn pass_structure(
    buf: &mut GraphBuffer,
    project: &str,
    _repo_path: &Path,
    files: &[DiscoveredFile],
) {
    // Project node
    let proj_qn = project.to_owned();
    buf.add_node("Project", project, &proj_qn, "", 0, 0, None);

    // Collect unique directories
    let mut dirs = HashSet::new();
    for f in files {
        let mut dir = String::new();
        for part in f.rel_path.split('/') {
            if part.contains('.') && f.rel_path.ends_with(part) {
                break; // this is the filename
            }
            if !dir.is_empty() {
                dir.push('/');
            }
            dir.push_str(part);
            dirs.insert(dir.clone());
        }
    }

    for dir in &dirs {
        let folder_qn = fqn::fqn_folder(project, dir);
        let name = dir.rsplit('/').next().unwrap_or(dir);
        buf.add_node("Folder", name, &folder_qn, dir, 0, 0, None);
    }

    // File nodes + CONTAINS edges (Folder→File)
    for f in files {
        let file_qn = fqn::fqn_module(project, &f.rel_path);
        let name = Path::new(&f.rel_path)
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or(&f.rel_path);
        let props = serde_json::json!({ "language": f.language.name() }).to_string();
        buf.add_node("File", name, &file_qn, &f.rel_path, 0, 0, Some(props));

        // CONTAINS edge: parent folder → file
        let parent_dir = Path::new(&f.rel_path)
            .parent()
            .and_then(|p| p.to_str())
            .unwrap_or("");
        let parent_qn = if parent_dir.is_empty() {
            project.to_owned() // Project node
        } else {
            fqn::fqn_folder(project, parent_dir)
        };
        buf.add_edge_by_qn(&parent_qn, &file_qn, "CONTAINS", None);
    }

    // CONTAINS edges: Project → top-level folders, parent folder → child folder
    for dir in &dirs {
        let folder_qn = fqn::fqn_folder(project, dir);
        let parent_dir = Path::new(dir.as_str())
            .parent()
            .and_then(|p| p.to_str())
            .unwrap_or("");
        let parent_qn = if parent_dir.is_empty() {
            project.to_owned()
        } else {
            fqn::fqn_folder(project, parent_dir)
        };
        buf.add_edge_by_qn(&parent_qn, &folder_qn, "CONTAINS", None);
    }
}

/// Pass 3: Resolve call edges using the registry and Aho-Corasick.
pub fn pass_calls(buf: &mut GraphBuffer, reg: &Registry, files: &[&DiscoveredFile], project: &str) {
    if reg.is_empty() {
        return;
    }

    let names = reg.all_names();
    let ac = aho_corasick::AhoCorasick::builder()
        .ascii_case_insensitive(false)
        .kind(Some(aho_corasick::AhoCorasickKind::ContiguousNFA))
        .build(&names);

    let ac = match ac {
        Ok(a) => a,
        Err(e) => {
            tracing::error!(
                patterns = names.len(),
                error = %e,
                "pass_calls: AhoCorasick build failed — no CALLS/USES edges will be created"
            );
            return;
        }
    };

    // Process files in parallel, collect (src_qn, tgt_qn, edge_type) tuples
    let edge_tuples: Vec<(String, String, String)> = files
        .par_iter()
        .flat_map(|f| {
            let source = match std::fs::read_to_string(&f.abs_path) {
                Ok(s) => s,
                Err(_) => return vec![],
            };

            let call_sites: HashSet<&str> = CALL_RE
                .find_iter(&source)
                .map(|m| m.as_str()[..m.as_str().len() - 1].trim())
                .collect();

            // Build byte-offset → line-number lookup
            let mut line_starts: Vec<usize> = vec![0];
            for (i, b) in source.bytes().enumerate() {
                if b == b'\n' {
                    line_starts.push(i + 1);
                }
            }

            // Get functions in this file for caller resolution
            let file_fns = reg.entries_for_file(&f.rel_path);

            let module_qn = fqn::fqn_module(project, &f.rel_path);
            let mut seen = HashSet::new();
            let mut edges = Vec::new();
            for mat in ac.find_iter(&source) {
                let name = names[mat.pattern().as_usize()];
                if seen.contains(name) {
                    continue;
                }
                seen.insert(name);

                // Convert byte offset to 1-based line number
                let line_num = (line_starts.partition_point(|&off| off <= mat.start())) as i32;

                // Find the containing function for this call site
                let caller_qn = file_fns
                    .iter()
                    .rev()
                    .find(|e| e.start_line <= line_num && e.end_line >= line_num)
                    .map(|e| e.qualified_name.as_str())
                    .unwrap_or(module_qn.as_str());

                let entries = reg.lookup(name);
                for entry in entries {
                    if entry.file_path == f.rel_path {
                        continue;
                    }
                    let edge_type = if call_sites.contains(name)
                        && matches!(entry.label.as_str(), "Function" | "Method")
                    {
                        "CALLS"
                    } else {
                        "USES"
                    };
                    edges.push((
                        caller_qn.to_owned(),
                        entry.qualified_name.clone(),
                        edge_type.to_owned(),
                    ));
                }
            }
            edges
        })
        .collect();

    // Add edges to buffer serially (buffer is not Send)
    for (src, tgt, etype) in edge_tuples {
        buf.add_edge_by_qn(&src, &tgt, &etype, None);
    }
}

/// Pass 4: Extract import edges.
pub fn pass_imports(buf: &mut GraphBuffer, files: &[&DiscoveredFile], project: &str) {
    for f in files {
        let source = match std::fs::read_to_string(&f.abs_path) {
            Ok(s) => s,
            Err(_) => continue,
        };

        let module_qn = fqn::fqn_module(project, &f.rel_path);

        for line in source.lines() {
            let trimmed = line.trim();
            // Python: import X / from X import Y
            if trimmed.starts_with("import ") || trimmed.starts_with("from ") {
                // Skip Java imports — handled below
                if !trimmed.contains(';') {
                    if let Some(target) = parse_import_target(trimmed) {
                        let target_qn = format!("{}.{}", project, target.replace('/', "."));
                        buf.add_edge_by_qn(&module_qn, &target_qn, "IMPORTS", None);
                    }
                }
            }
            // JS/TS: import ... from '...'
            if trimmed.contains("from '")
                || trimmed.contains("from \"")
                || trimmed.contains("require(")
            {
                if let Some(target) = parse_js_import(trimmed) {
                    let target_qn = if target.contains('/') {
                        fqn::fqn_module(project, &format!("{}.ts", target))
                    } else {
                        format!("{}.{}", project, target)
                    };
                    buf.add_edge_by_qn(&module_qn, &target_qn, "IMPORTS", None);
                }
            }
            // Rust: use crate::...
            if trimmed.starts_with("use ") {
                if let Some(target) = parse_rust_use(trimmed) {
                    let target_qn = format!("{}.{}", project, target);
                    buf.add_edge_by_qn(&module_qn, &target_qn, "IMPORTS", None);
                }
            }
            // Java/Kotlin: import com.example.Foo;
            if let Some(target) = parse_java_import(trimmed) {
                let target_qn = format!("{}.{}", project, target);
                buf.add_edge_by_qn(&module_qn, &target_qn, "IMPORTS", None);
            }
            // Go: import "..."
            if trimmed.starts_with("import ") && (trimmed.contains('"') || trimmed.contains('(')) {
                // Handled by the Python branch above for simple cases
            }
        }
    }
}

fn parse_import_target(line: &str) -> Option<String> {
    // "from foo.bar import baz" -> "foo.bar"
    // "import foo.bar" -> "foo.bar"
    if let Some(rest) = line.strip_prefix("from ") {
        let end = rest.find(' ').unwrap_or(rest.len());
        return Some(rest[..end].to_owned());
    }
    if let Some(rest) = line.strip_prefix("import ") {
        let end = rest.find([' ', ',']).unwrap_or(rest.len());
        return Some(rest[..end].to_owned());
    }
    None
}

fn parse_js_import(line: &str) -> Option<String> {
    // Extract path from: from './foo' or require('./foo')
    for delim in &["from '", "from \"", "require('", "require(\""] {
        if let Some(start) = line.find(delim) {
            let rest = &line[start + delim.len()..];
            let end_char = if delim.ends_with('\'') { '\'' } else { '"' };
            let end = rest.find(end_char)?;
            let path = &rest[..end];
            // Strip leading ./ or ../
            let clean = path.trim_start_matches("./").trim_start_matches("../");
            return Some(clean.to_owned());
        }
    }
    None
}

fn parse_rust_use(line: &str) -> Option<String> {
    // "use crate::foo::bar;" -> "foo.bar"
    let rest = line.strip_prefix("use ")?.trim_end_matches(';').trim();
    let path = rest.strip_prefix("crate::").unwrap_or(rest);
    Some(path.replace("::", "."))
}

fn parse_java_import(line: &str) -> Option<String> {
    // "import com.example.Foo;" -> "Foo"
    // "import static com.example.Foo.bar;" -> "Foo"
    let rest = line.strip_prefix("import ")?.trim_end_matches(';').trim();
    let rest = rest.strip_prefix("static ").unwrap_or(rest).trim();
    // Get the last component (class name) — that's what we can resolve
    let class_name = rest.rsplit('.').next()?;
    if class_name.is_empty() || class_name == "*" {
        return None;
    }
    Some(class_name.to_owned())
}

/// Pass 5: Semantic edges (INHERITS, IMPLEMENTS) from class declarations.
static EXTENDS_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(
        r"(?:class|interface)\s+\w+(?:<[^>]*>)?\s+extends\s+([\w,\s<>]+?)(?:\s+implements|\s*\{|$)",
    )
    .unwrap()
});
static IMPLEMENTS_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(
        r"class\s+\w+(?:<[^>]*>)?(?:\s+extends\s+[\w<>]+)?\s+implements\s+([\w,\s<>]+?)(?:\s*\{|$)",
    )
    .unwrap()
});
static CLASS_NAME_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"(?:class|interface)\s+(\w+)").unwrap());
static PY_INHERITS_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"^class\s+(\w+)\s*\(([^)]+)\)").unwrap());
static RUST_IMPL_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"^impl(?:<[^>]*>)?\s+(\w+)\s+for\s+(\w+)").unwrap());

pub fn pass_semantic(store: &Store, project: &str, files: &[&DiscoveredFile]) -> Result<()> {
    use codryn_graph_buffer::GraphBuffer;

    let mut buf = GraphBuffer::new(project);

    for f in files {
        let source = match std::fs::read_to_string(&f.abs_path) {
            Ok(s) => s,
            Err(_) => continue,
        };

        match f.language {
            Language::TypeScript | Language::Tsx | Language::JavaScript => {
                for line in source.lines() {
                    let trimmed = line.trim();
                    // class name
                    let class_name = CLASS_NAME_RE
                        .captures(trimmed)
                        .and_then(|c| c.get(1))
                        .map(|m| m.as_str().to_owned());

                    if let Some(ref cname) = class_name {
                        let src_qn = fqn::fqn_compute(project, &f.rel_path, Some(cname));
                        // extends
                        if let Some(caps) = EXTENDS_RE.captures(trimmed) {
                            for parent in caps.get(1).unwrap().as_str().split(',') {
                                let parent = parent.trim().split('<').next().unwrap_or("").trim();
                                if !parent.is_empty() {
                                    let tgt_qn = format!("{}.{}", project, parent);
                                    buf.add_edge_by_qn(&src_qn, &tgt_qn, "INHERITS", None);
                                }
                            }
                        }
                        // implements
                        if let Some(caps) = IMPLEMENTS_RE.captures(trimmed) {
                            for iface in caps.get(1).unwrap().as_str().split(',') {
                                let iface = iface.trim().split('<').next().unwrap_or("").trim();
                                if !iface.is_empty() {
                                    let tgt_qn = format!("{}.{}", project, iface);
                                    buf.add_edge_by_qn(&src_qn, &tgt_qn, "IMPLEMENTS", None);
                                }
                            }
                        }
                    }
                }
            }
            Language::Python | Language::Ruby => {
                for line in source.lines() {
                    if let Some(caps) = PY_INHERITS_RE.captures(line.trim()) {
                        let child = caps.get(1).unwrap().as_str();
                        let src_qn = fqn::fqn_compute(project, &f.rel_path, Some(child));
                        for parent in caps.get(2).unwrap().as_str().split(',') {
                            let parent = parent.trim();
                            if !parent.is_empty() && parent != "object" {
                                let tgt_qn = format!("{}.{}", project, parent);
                                buf.add_edge_by_qn(&src_qn, &tgt_qn, "INHERITS", None);
                            }
                        }
                    }
                }
            }
            Language::Rust => {
                for line in source.lines() {
                    if let Some(caps) = RUST_IMPL_RE.captures(line.trim()) {
                        let trait_name = caps.get(1).unwrap().as_str();
                        let struct_name = caps.get(2).unwrap().as_str();
                        let src_qn = fqn::fqn_compute(project, &f.rel_path, Some(struct_name));
                        let tgt_qn = format!("{}.{}", project, trait_name);
                        buf.add_edge_by_qn(&src_qn, &tgt_qn, "IMPLEMENTS", None);
                    }
                }
            }
            _ => {}
        }
    }

    buf.flush(store)?;
    Ok(())
}

// ── REST Contract Indexing ────────────────────────────

/// Pass: Route nodes for REST controllers. Java/Kotlin now handled by AST extractors.
pub fn pass_rest_contracts(
    _buf: &mut GraphBuffer,
    _reg: &Registry,
    _files: &[&DiscoveredFile],
    _project: &str,
) {
    // Express/Fastify routes are handled by `pass_express_routes`.
}

/// Express / Fastify style `app.get('/path', fn)` and `router.post(...)` (heuristic).
pub fn pass_express_routes(
    buf: &mut GraphBuffer,
    reg: &Registry,
    files: &[&DiscoveredFile],
    project: &str,
) {
    static ROUTE_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
        // Rust `regex` does not support backreferences, so we match quoted strings via two
        // alternatives (single-quoted or double-quoted).
        regex::Regex::new(
            r#"(?s)\b(?:app|router)\s*\.\s*(get|post|put|patch|delete|all|use)\s*\(\s*(?:'([^']+)'|"([^"]+)")\s*,\s*(\w+)\s*[,\)]"#,
        )
        .expect("pass_express_routes: ROUTE_RE must compile")
    });
    static FASTIFY_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
        regex::Regex::new(
            r#"(?s)\b(?:fastify|f)\s*\.\s*(get|post|put|patch|delete|all)\s*\(\s*(?:'([^']+)'|"([^"]+)")\s*,\s*(\w+)\s*[,\)]"#,
        )
        .expect("pass_express_routes: FASTIFY_RE must compile")
    });

    for f in files {
        if !matches!(
            f.language,
            Language::TypeScript | Language::Tsx | Language::JavaScript
        ) {
            continue;
        }
        let source = match std::fs::read_to_string(&f.abs_path) {
            Ok(s) => s,
            Err(_) => continue,
        };
        for caps in ROUTE_RE.captures_iter(&source) {
            let method = caps
                .get(1)
                .map(|m| m.as_str().to_uppercase())
                .unwrap_or_default();
            let path = caps
                .get(2)
                .or_else(|| caps.get(3))
                .map(|m| m.as_str())
                .unwrap_or("/");
            let path = if path.starts_with('/') {
                path.to_string()
            } else {
                format!("/{path}")
            };
            let handler = caps.get(4).map(|m| m.as_str()).unwrap_or("handler");
            emit_express_route(buf, reg, project, &f.rel_path, &method, &path, handler);
        }
        for caps in FASTIFY_RE.captures_iter(&source) {
            let method = caps
                .get(1)
                .map(|m| m.as_str().to_uppercase())
                .unwrap_or_default();
            let path = caps
                .get(2)
                .or_else(|| caps.get(3))
                .map(|m| m.as_str())
                .unwrap_or("/");
            let path = if path.starts_with('/') {
                path.to_string()
            } else {
                format!("/{path}")
            };
            let handler = caps.get(4).map(|m| m.as_str()).unwrap_or("handler");
            emit_express_route(buf, reg, project, &f.rel_path, &method, &path, handler);
        }
    }
}

fn emit_express_route(
    buf: &mut GraphBuffer,
    reg: &Registry,
    project: &str,
    file_rel: &str,
    method: &str,
    path: &str,
    handler: &str,
) {
    if !reg.lookup(handler).iter().any(|e| e.file_path == file_rel) {
        return;
    }
    let path_key = path
        .trim_start_matches('/')
        .replace('/', "_")
        .replace(['{', '}', '*'], "_");
    let route_qn = format!("{project}.express.route.{method}.{path_key}");
    let display = format!("{method} {path}");
    let handler_qn = fqn::fqn_compute(project, file_rel, Some(handler));
    let props = serde_json::json!({
        "http_method": method,
        "path": path,
        "method_name": handler,
        "source": "express"
    })
    .to_string();
    buf.add_node("Route", &display, &route_qn, file_rel, 1, 1, Some(props));
    buf.add_edge_by_qn(&handler_qn, &route_qn, "HANDLES_ROUTE", None);
}

/// Pass: Create Route nodes + HANDLES_ROUTE/ACCEPTS_DTO/RETURNS_DTO edges for Spring Boot controllers.
/// Runs on ALL Java/Kotlin files (not just changed) so edges survive reindex.
pub fn pass_spring_routes(buf: &mut GraphBuffer, files: &[&DiscoveredFile], project: &str) {
    for f in files {
        match f.language {
            Language::Java => {
                crate::spring_java::create_routes(buf, project, f);
            }
            Language::Kotlin => {
                crate::spring_kotlin::create_routes(buf, project, f);
            }
            _ => {}
        }
    }
}

// ── Angular Template Awareness ────────────────────────

static CUSTOM_ELEMENT_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"<([\w]+-[\w-]+)").unwrap());

/// Pass: Parse Angular .component.html files for custom element selectors and create RENDERS edges.
pub fn pass_angular_templates(
    buf: &mut GraphBuffer,
    store: &Store,
    files: &[&DiscoveredFile],
    project: &str,
) {
    for f in files {
        if !f.rel_path.ends_with(".component.html") {
            continue;
        }
        let source = match std::fs::read_to_string(&f.abs_path) {
            Ok(s) => s,
            Err(_) => continue,
        };

        // Resolve parent component: co-located .component.ts
        let ts_path = f.rel_path.replace(".component.html", ".component.ts");
        let parent_qn = fqn::fqn_module(project, &ts_path);
        // Find the actual class node in the TS file
        let parent_nodes = store
            .search_nodes_filtered(project, &ts_path, Some("Class"), 1)
            .unwrap_or_default();
        let parent_qn = parent_nodes
            .first()
            .map(|n| n.qualified_name.as_str())
            .unwrap_or(parent_qn.as_str());

        // Find all custom element tags
        let mut seen = HashSet::new();
        for caps in CUSTOM_ELEMENT_RE.captures_iter(&source) {
            let selector = caps.get(1).unwrap().as_str();
            if !seen.insert(selector.to_owned()) {
                continue;
            }
            // Look up the component with this selector
            if let Ok(Some(child)) = store.find_node_by_property(project, "selector", selector) {
                buf.add_edge_by_qn(parent_qn, &child.qualified_name, "RENDERS", None);
            }
        }
    }
}

// ── Cross-Project Name-Based Auto-Linking ─────────────

/// Pass: Create MAPS_TO edges between classes/interfaces with the same name across linked projects.
pub fn pass_cross_project_mapping(buf: &mut GraphBuffer, store: &Store, project: &str) {
    let links = store.get_linked_projects(project).unwrap_or_default();
    for link in &links {
        let matches = store
            .find_matching_symbols_across_projects(project, &link.target_project)
            .unwrap_or_default();
        for (a, b) in &matches {
            buf.add_edge_by_qn(&a.qualified_name, &b.qualified_name, "MAPS_TO", None);
        }
    }
}

#[cfg(test)]
mod express_route_tests {
    #[test]
    fn express_regex_compiles_without_backrefs() {
        // Rust `regex` doesn't support backreferences (`\1`, `\2`, ...). This test ensures
        // we don't accidentally reintroduce them.
        let _ = regex::Regex::new(
            r#"(?s)\b(?:app|router)\s*\.\s*(get|post|put|patch|delete|all|use)\s*\(\s*(?:'([^']+)'|"([^"]+)")\s*,\s*(\w+)\s*[,\)]"#,
        )
        .unwrap();
        let _ = regex::Regex::new(
            r#"(?s)\b(?:fastify|f)\s*\.\s*(get|post|put|patch|delete|all)\s*\(\s*(?:'([^']+)'|"([^"]+)")\s*,\s*(\w+)\s*[,\)]"#,
        )
        .unwrap();
    }
}
