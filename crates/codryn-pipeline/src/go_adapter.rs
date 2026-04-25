use codryn_discover::DiscoveredFile;
use codryn_foundation::fqn;
use codryn_graph_buffer::GraphBuffer;
use tree_sitter::{Node, Parser};

use crate::go_common;
use crate::registry::Registry;

fn make_parser() -> Option<Parser> {
    let mut parser = Parser::new();
    let lang: tree_sitter::Language = tree_sitter_go::LANGUAGE.into();
    parser.set_language(&lang).ok()?;
    Some(parser)
}

fn node_text<'a>(node: &Node, src: &'a [u8]) -> &'a str {
    node.utf8_text(src).unwrap_or("")
}

/// Extract the receiver type name from a method_declaration's parameter_list.
/// Handles both `(r *Type)` and `(r Type)`.
fn receiver_type_name(node: &Node, src: &[u8]) -> Option<String> {
    let params = node.child_by_field_name("receiver")?;
    // receiver is a parameter_list with one parameter_declaration
    for i in 0..params.child_count() {
        let param = params.child(i).unwrap();
        if param.kind() == "parameter_declaration" {
            if let Some(typ) = param.child_by_field_name("type") {
                let text = node_text(&typ, src);
                // Strip pointer: *Handler -> Handler
                return Some(text.trim_start_matches('*').to_string());
            }
        }
    }
    None
}

/// Check if a function name is a Go test/benchmark/example function.
fn test_kind(name: &str) -> Option<&'static str> {
    if name.starts_with("Test") && name.len() > 4 {
        Some("test")
    } else if name.starts_with("Benchmark") && name.len() > 9 {
        Some("benchmark")
    } else if name.starts_with("Example") {
        Some("example")
    } else {
        None
    }
}

/// Extract definitions from a Go file using tree-sitter AST.
pub fn extract_go(buf: &mut GraphBuffer, reg: &mut Registry, project: &str, file: &DiscoveredFile) {
    let source = match std::fs::read_to_string(&file.abs_path) {
        Ok(s) => s,
        Err(_) => return,
    };
    let mut parser = match make_parser() {
        Some(p) => p,
        None => {
            super::extraction::extract_file(buf, reg, project, file);
            return;
        }
    };
    let tree = match parser.parse(&source, None) {
        Some(t) => t,
        None => {
            super::extraction::extract_file(buf, reg, project, file);
            return;
        }
    };

    let src = source.as_bytes();
    let root = tree.root_node();
    let is_test_file = file.rel_path.ends_with("_test.go");

    for i in 0..root.child_count() {
        let child = root.child(i).unwrap();
        match child.kind() {
            "type_declaration" => {
                extract_type_decl(buf, reg, project, file, src, &child, is_test_file);
            }
            "function_declaration" => {
                extract_function(buf, reg, project, file, src, &child, is_test_file);
            }
            "method_declaration" => {
                extract_method(buf, reg, project, file, src, &child, is_test_file);
            }
            _ => {}
        }
    }

    // Ginkgo BDD specs: Describe/Context/It/When/BeforeEach/AfterEach
    if is_test_file {
        extract_ginkgo_specs(buf, reg, project, file, src, &root, &[]);
    }

    // Module node
    let module_qn = fqn::fqn_module(project, &file.rel_path);
    let module_name = std::path::Path::new(&file.rel_path)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(&file.rel_path);
    let lines: Vec<&str> = source.lines().collect();
    buf.add_node(
        "Module",
        module_name,
        &module_qn,
        &file.rel_path,
        1,
        lines.len() as i32,
        None,
    );
}

/// Register-only variant for incremental reindex (unchanged files).
pub fn register_go(reg: &mut Registry, project: &str, file: &DiscoveredFile) {
    let source = match std::fs::read_to_string(&file.abs_path) {
        Ok(s) => s,
        Err(_) => return,
    };
    let mut parser = match make_parser() {
        Some(p) => p,
        None => {
            super::extraction::register_file(reg, project, file);
            return;
        }
    };
    let tree = match parser.parse(&source, None) {
        Some(t) => t,
        None => {
            super::extraction::register_file(reg, project, file);
            return;
        }
    };
    let src = source.as_bytes();
    let root = tree.root_node();
    for i in 0..root.child_count() {
        let child = root.child(i).unwrap();
        match child.kind() {
            "type_declaration" => register_type_decl(reg, project, file, src, &child),
            "function_declaration" => {
                if let Some(name_node) = child.child_by_field_name("name") {
                    let name = node_text(&name_node, src);
                    let s = child.start_position().row as i32 + 1;
                    let e = child.end_position().row as i32 + 1;
                    let qn = fqn::fqn_compute(project, &file.rel_path, Some(name));
                    reg.register(name, &qn, &file.rel_path, "Function", s, e);
                }
            }
            "method_declaration" => {
                if let Some(name_node) = child.child_by_field_name("name") {
                    let name = node_text(&name_node, src);
                    let s = child.start_position().row as i32 + 1;
                    let e = child.end_position().row as i32 + 1;
                    let qn = fqn::fqn_compute(project, &file.rel_path, Some(name));
                    reg.register(name, &qn, &file.rel_path, "Method", s, e);
                }
            }
            _ => {}
        }
    }
}

// ── Type declarations (struct, interface) ─────────────

fn extract_type_decl(
    buf: &mut GraphBuffer,
    reg: &mut Registry,
    project: &str,
    file: &DiscoveredFile,
    src: &[u8],
    node: &Node,
    is_test_file: bool,
) {
    // type_declaration can contain multiple type_spec children
    for i in 0..node.child_count() {
        let spec = node.child(i).unwrap();
        if spec.kind() != "type_spec" {
            continue;
        }
        let name = match spec.child_by_field_name("name") {
            Some(n) => node_text(&n, src).to_string(),
            None => continue,
        };
        let type_node = match spec.child_by_field_name("type") {
            Some(t) => t,
            None => continue,
        };
        let (label, kind_str) = match type_node.kind() {
            "struct_type" => ("Class", "struct"),
            "interface_type" => ("Interface", "interface"),
            _ => ("Class", "type"),
        };

        let start = spec.start_position().row as i32 + 1;
        let end = spec.end_position().row as i32 + 1;
        let qn = fqn::fqn_compute(project, &file.rel_path, Some(&name));
        let layer = go_common::classify_go_layer(&name, &file.rel_path);

        let mut props = serde_json::Map::new();
        props.insert("kind".into(), serde_json::json!(kind_str));
        if let Some(l) = layer {
            props.insert("layer".into(), serde_json::json!(l));
        }
        if is_test_file {
            props.insert("is_test".into(), serde_json::json!(true));
        }
        // Store interface method signatures for IMPLEMENTS resolution
        if type_node.kind() == "interface_type" {
            let methods = extract_interface_methods(&type_node, src);
            if !methods.is_empty() {
                props.insert("interface_methods".into(), serde_json::json!(methods));
            }
        }

        let props_json = Some(serde_json::Value::Object(props).to_string());
        buf.add_node(label, &name, &qn, &file.rel_path, start, end, props_json);
        reg.register(&name, &qn, &file.rel_path, label, start, end);

        // FTS content
        buf.add_code_content(&qn, node_text(&spec, src));
    }
}

fn register_type_decl(
    reg: &mut Registry,
    project: &str,
    file: &DiscoveredFile,
    src: &[u8],
    node: &Node,
) {
    for i in 0..node.child_count() {
        let spec = node.child(i).unwrap();
        if spec.kind() != "type_spec" {
            continue;
        }
        let name = match spec.child_by_field_name("name") {
            Some(n) => node_text(&n, src).to_string(),
            None => continue,
        };
        let type_node = match spec.child_by_field_name("type") {
            Some(t) => t,
            None => continue,
        };
        let label = if type_node.kind() == "interface_type" {
            "Interface"
        } else {
            "Class"
        };
        let s = spec.start_position().row as i32 + 1;
        let e = spec.end_position().row as i32 + 1;
        let qn = fqn::fqn_compute(project, &file.rel_path, Some(&name));
        reg.register(&name, &qn, &file.rel_path, label, s, e);
    }
}

/// Extract method signatures from an interface_type node for IMPLEMENTS matching.
fn extract_interface_methods(iface_node: &Node, src: &[u8]) -> Vec<String> {
    let mut methods = Vec::new();
    for i in 0..iface_node.child_count() {
        let child = iface_node.child(i).unwrap();
        if child.kind() == "method_elem" || child.kind() == "method_spec" {
            if let Some(name) = child.child_by_field_name("name") {
                methods.push(node_text(&name, src).to_string());
            }
        }
    }
    methods
}

// ── Functions and methods ─────────────────────────────

fn extract_function(
    buf: &mut GraphBuffer,
    reg: &mut Registry,
    project: &str,
    file: &DiscoveredFile,
    src: &[u8],
    node: &Node,
    is_test_file: bool,
) {
    let name = match node.child_by_field_name("name") {
        Some(n) => node_text(&n, src).to_string(),
        None => return,
    };
    let start = node.start_position().row as i32 + 1;
    let end = node.end_position().row as i32 + 1;
    let qn = fqn::fqn_compute(project, &file.rel_path, Some(&name));
    let layer = go_common::classify_go_layer(&name, &file.rel_path);

    let mut props = serde_json::Map::new();
    if let Some(l) = layer {
        props.insert("layer".into(), serde_json::json!(l));
    }
    if is_test_file {
        props.insert("is_test".into(), serde_json::json!(true));
        if let Some(tk) = test_kind(&name) {
            props.insert("test_kind".into(), serde_json::json!(tk));
        }
    }
    // Detect entry point: func main()
    if name == "main" {
        props.insert("is_entry_point".into(), serde_json::json!(true));
    }

    let props_json = if props.is_empty() {
        None
    } else {
        Some(serde_json::Value::Object(props).to_string())
    };
    buf.add_node(
        "Function",
        &name,
        &qn,
        &file.rel_path,
        start,
        end,
        props_json,
    );
    reg.register(&name, &qn, &file.rel_path, "Function", start, end);
    buf.add_code_content(&qn, node_text(node, src));
}

fn extract_method(
    buf: &mut GraphBuffer,
    reg: &mut Registry,
    project: &str,
    file: &DiscoveredFile,
    src: &[u8],
    node: &Node,
    is_test_file: bool,
) {
    let name = match node.child_by_field_name("name") {
        Some(n) => node_text(&n, src).to_string(),
        None => return,
    };
    let recv_type = receiver_type_name(node, src).unwrap_or_default();
    let start = node.start_position().row as i32 + 1;
    let end = node.end_position().row as i32 + 1;
    let qn = fqn::fqn_compute(project, &file.rel_path, Some(&name));
    let layer = go_common::classify_go_layer(&recv_type, &file.rel_path);

    let mut props = serde_json::Map::new();
    if !recv_type.is_empty() {
        props.insert("receiver".into(), serde_json::json!(recv_type));
        props.insert("receiver_type".into(), serde_json::json!(recv_type));
    }
    if let Some(l) = layer {
        props.insert("layer".into(), serde_json::json!(l));
    }
    if is_test_file {
        props.insert("is_test".into(), serde_json::json!(true));
        if let Some(tk) = test_kind(&name) {
            props.insert("test_kind".into(), serde_json::json!(tk));
        }
    }
    // Check if receiver is pointer (for IMPLEMENTS resolution)
    if let Some(recv_node) = node.child_by_field_name("receiver") {
        for j in 0..recv_node.child_count() {
            let p = recv_node.child(j).unwrap();
            if p.kind() == "parameter_declaration" {
                if let Some(t) = p.child_by_field_name("type") {
                    if t.kind() == "pointer_type" {
                        props.insert("pointer_receiver".into(), serde_json::json!(true));
                    }
                }
            }
        }
    }

    let props_json = if props.is_empty() {
        None
    } else {
        Some(serde_json::Value::Object(props).to_string())
    };
    buf.add_node("Method", &name, &qn, &file.rel_path, start, end, props_json);
    reg.register(&name, &qn, &file.rel_path, "Method", start, end);
    buf.add_code_content(&qn, node_text(node, src));

    // CONTAINS edge: struct → method
    if !recv_type.is_empty() {
        let struct_qn = format!("{project}.{recv_type}");
        buf.add_edge_by_qn(&struct_qn, &qn, "CONTAINS", None);
    }
}

// ── Ginkgo BDD spec extraction ────────────────────────

const GINKGO_CONTAINERS: &[&str] = &["Describe", "Context", "When"];
const GINKGO_SPECS: &[&str] = &["It", "Specify"];
const GINKGO_SETUP: &[&str] = &[
    "BeforeEach",
    "AfterEach",
    "JustBeforeEach",
    "JustAfterEach",
    "BeforeSuite",
    "AfterSuite",
];

fn extract_ginkgo_specs(
    buf: &mut GraphBuffer,
    reg: &mut Registry,
    project: &str,
    file: &DiscoveredFile,
    src: &[u8],
    node: &Node,
    path: &[String],
) {
    for i in 0..node.child_count() {
        let child = node.child(i).unwrap();
        if child.kind() == "call_expression" {
            if let Some(func) = child.child_by_field_name("function") {
                let fname = node_text(&func, src);
                if GINKGO_CONTAINERS.contains(&fname)
                    || GINKGO_SPECS.contains(&fname)
                    || GINKGO_SETUP.contains(&fname)
                {
                    let desc = extract_first_string_arg(&child, src).unwrap_or_default();
                    let label = format!("{fname}({desc})");
                    let mut full_path = path.to_vec();
                    full_path.push(label.clone());
                    let qn = fqn::fqn_compute(project, &file.rel_path, Some(&full_path.join("/")));
                    let s = child.start_position().row as i32 + 1;
                    let e = child.end_position().row as i32 + 1;

                    let test_kind = if GINKGO_SPECS.contains(&fname) {
                        "ginkgo_spec"
                    } else if GINKGO_SETUP.contains(&fname) {
                        "ginkgo_setup"
                    } else {
                        "ginkgo_container"
                    };

                    let props = serde_json::json!({
                        "is_test": true,
                        "test_kind": test_kind,
                        "test_framework": "ginkgo",
                        "ginkgo_type": fname,
                        "description": desc,
                    })
                    .to_string();

                    buf.add_node("Function", &label, &qn, &file.rel_path, s, e, Some(props));
                    reg.register(&label, &qn, &file.rel_path, "Function", s, e);

                    // Recurse into the closure body for nested specs
                    if let Some(args) = child.child_by_field_name("arguments") {
                        extract_ginkgo_specs(buf, reg, project, file, src, &args, &full_path);
                    }
                    continue;
                }
            }
        }
        // Recurse into non-ginkgo nodes to find nested specs
        if child.child_count() > 0 {
            extract_ginkgo_specs(buf, reg, project, file, src, &child, path);
        }
    }
}

fn extract_first_string_arg(call: &Node, src: &[u8]) -> Option<String> {
    let args = call.child_by_field_name("arguments")?;
    for i in 0..args.child_count() {
        let c = args.child(i).unwrap();
        if c.kind() == "interpreted_string_literal" || c.kind() == "raw_string_literal" {
            let text = node_text(&c, src);
            return Some(text.trim_matches('"').trim_matches('`').to_string());
        }
    }
    None
}

// ── Route extraction ──────────────────────────────────

/// Create Route nodes for Go HTTP handlers (net/http + frameworks).
pub fn pass_go_routes(buf: &mut GraphBuffer, files: &[&DiscoveredFile], project: &str) {
    for f in files {
        if f.language != codryn_discover::Language::Go {
            continue;
        }
        let source = match std::fs::read_to_string(&f.abs_path) {
            Ok(s) => s,
            Err(_) => continue,
        };
        let mut parser = match make_parser() {
            Some(p) => p,
            None => continue,
        };
        let tree = match parser.parse(&source, None) {
            Some(t) => t,
            None => continue,
        };
        let src = source.as_bytes();
        extract_routes_from_node(&tree.root_node(), src, buf, project, f);
    }
}

fn extract_routes_from_node(
    node: &Node,
    src: &[u8],
    buf: &mut GraphBuffer,
    project: &str,
    file: &DiscoveredFile,
) {
    for i in 0..node.child_count() {
        let child = node.child(i).unwrap();
        // Look for call expressions
        if child.kind() == "call_expression" {
            try_extract_route(&child, src, buf, project, file);
        }
        // Recurse into function bodies, blocks, etc.
        extract_routes_from_node(&child, src, buf, project, file);
    }
}

fn try_extract_route(
    call: &Node,
    src: &[u8],
    buf: &mut GraphBuffer,
    project: &str,
    file: &DiscoveredFile,
) {
    let func_node = match call.child_by_field_name("function") {
        Some(f) => f,
        None => return,
    };
    let args = match call.child_by_field_name("arguments") {
        Some(a) => a,
        None => return,
    };

    let func_text = node_text(&func_node, src);

    // net/http: http.HandleFunc("/path", handler) or http.Handle("/path", handler)
    // Go 1.22+: http.HandleFunc("GET /path", handler)
    if func_text == "http.HandleFunc" || func_text == "http.Handle" {
        if let Some((path, handler)) = extract_two_args(&args, src) {
            let (method, clean_path) = parse_method_from_path(&path);
            create_route_node(buf, project, file, call, src, method, &clean_path, &handler);
        }
        return;
    }

    // Selector-based: r.GET("/path", handler), r.HandleFunc("/path", handler).Methods("GET"), etc.
    if func_node.kind() == "selector_expression" {
        let method_name = func_node
            .child_by_field_name("field")
            .map(|f| node_text(&f, src))
            .unwrap_or("");

        // Gin/Echo: r.GET, r.POST, etc. (uppercase)
        // Chi/Fiber: r.Get, r.Post, etc. (title case)
        if let Some(http_method) = go_common::method_call_to_http(method_name) {
            if let Some((path, handler)) = extract_two_args(&args, src) {
                create_route_node(buf, project, file, call, src, http_method, &path, &handler);
            }
            return;
        }

        // Gorilla Mux: r.HandleFunc("/path", handler).Methods("GET")
        // Go 1.22+ mux: mux.HandleFunc("GET /path", handler)
        if method_name == "HandleFunc" || method_name == "Handle" {
            if let Some((path, handler)) = extract_two_args(&args, src) {
                // Check for chained .Methods("GET") call
                let chained = detect_gorilla_method(call, src);
                let (embedded, clean_path) = parse_method_from_path(&path);
                let http_method = chained.unwrap_or(embedded);
                create_route_node(
                    buf,
                    project,
                    file,
                    call,
                    src,
                    http_method,
                    &clean_path,
                    &handler,
                );
            }
        }
    }
}

/// Parse Go 1.22+ route pattern: "GET /path" → ("GET", "/path"), "/path" → ("ANY", "/path")
fn parse_method_from_path(path: &str) -> (&'static str, String) {
    let trimmed = path.trim();
    if let Some(idx) = trimmed.find(' ') {
        let prefix = &trimmed[..idx];
        let rest = trimmed[idx..].trim().to_string();
        match prefix {
            "GET" => return ("GET", rest),
            "POST" => return ("POST", rest),
            "PUT" => return ("PUT", rest),
            "PATCH" => return ("PATCH", rest),
            "DELETE" => return ("DELETE", rest),
            "HEAD" => return ("HEAD", rest),
            "OPTIONS" => return ("OPTIONS", rest),
            _ => {}
        }
    }
    ("ANY", trimmed.to_string())
}

fn extract_two_args(args: &Node, src: &[u8]) -> Option<(String, String)> {
    let mut arg_nodes = Vec::new();
    for i in 0..args.child_count() {
        let c = args.child(i).unwrap();
        if c.kind() != "," && c.kind() != "(" && c.kind() != ")" {
            arg_nodes.push(c);
        }
    }
    if arg_nodes.len() >= 2 {
        let path = node_text(&arg_nodes[0], src).trim_matches('"').to_string();
        let handler = node_text(&arg_nodes[1], src).to_string();
        Some((path, handler))
    } else {
        None
    }
}

/// Detect .Methods("GET") chained call on Gorilla Mux.
fn detect_gorilla_method(call: &Node, src: &[u8]) -> Option<&'static str> {
    // The parent of this call_expression might be a selector in another call_expression
    let parent = call.parent()?;
    if parent.kind() == "selector_expression" {
        let field = parent.child_by_field_name("field")?;
        if node_text(&field, src) == "Methods" {
            let grandparent = parent.parent()?;
            if grandparent.kind() == "call_expression" {
                if let Some(args) = grandparent.child_by_field_name("arguments") {
                    for i in 0..args.child_count() {
                        let c = args.child(i).unwrap();
                        let text = node_text(&c, src).trim_matches('"');
                        match text {
                            "GET" => return Some("GET"),
                            "POST" => return Some("POST"),
                            "PUT" => return Some("PUT"),
                            "PATCH" => return Some("PATCH"),
                            "DELETE" => return Some("DELETE"),
                            _ => {}
                        }
                    }
                }
            }
        }
    }
    None
}

#[allow(clippy::too_many_arguments)]
fn create_route_node(
    buf: &mut GraphBuffer,
    project: &str,
    file: &DiscoveredFile,
    call: &Node,
    _src: &[u8],
    http_method: &str,
    path: &str,
    handler: &str,
) {
    let route_name = format!("{http_method} {path}");
    let route_qn = format!("{project}.route.{http_method}.{path}");
    let s = call.start_position().row as i32 + 1;
    let e = call.end_position().row as i32 + 1;

    // Clean handler name: "h.RegisterUser" -> "RegisterUser", "handler.New" -> "New"
    let handler_name = handler.rsplit('.').next().unwrap_or(handler);

    let props = serde_json::json!({
        "http_method": http_method,
        "path": path,
        "handler": handler_name,
    })
    .to_string();

    buf.add_node(
        "Route",
        &route_name,
        &route_qn,
        &file.rel_path,
        s,
        e,
        Some(props),
    );

    // HANDLES_ROUTE edge: handler method → route
    let handler_qn = format!("{project}.{handler_name}");
    buf.add_edge_by_qn(&handler_qn, &route_qn, "HANDLES_ROUTE", None);
}

// ── Interface satisfaction ─────────────────────────────

/// Detect Go interface satisfaction by comparing method sets.
/// Creates IMPLEMENTS edges for structs whose method set is a superset of an interface's.
pub fn pass_go_implements(buf: &mut GraphBuffer, files: &[&DiscoveredFile], project: &str) {
    // Collect interfaces: name -> list of method names
    let mut interfaces: Vec<(String, String, Vec<String>)> = Vec::new(); // (name, qn, methods)
                                                                         // Collect struct methods: receiver_type -> set of method names
    let mut struct_methods: std::collections::HashMap<String, std::collections::HashSet<String>> =
        std::collections::HashMap::new();
    // Track struct QNs
    let mut struct_qns: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();

    for f in files {
        if f.language != codryn_discover::Language::Go {
            continue;
        }
        let source = match std::fs::read_to_string(&f.abs_path) {
            Ok(s) => s,
            Err(_) => continue,
        };
        let mut parser = match make_parser() {
            Some(p) => p,
            None => continue,
        };
        let tree = match parser.parse(&source, None) {
            Some(t) => t,
            None => continue,
        };
        let src = source.as_bytes();
        let root = tree.root_node();

        for i in 0..root.child_count() {
            let child = root.child(i).unwrap();
            match child.kind() {
                "type_declaration" => {
                    for j in 0..child.child_count() {
                        let spec = child.child(j).unwrap();
                        if spec.kind() != "type_spec" {
                            continue;
                        }
                        let name = match spec.child_by_field_name("name") {
                            Some(n) => node_text(&n, src).to_string(),
                            None => continue,
                        };
                        let type_node = match spec.child_by_field_name("type") {
                            Some(t) => t,
                            None => continue,
                        };
                        let qn = fqn::fqn_compute(project, &f.rel_path, Some(&name));
                        if type_node.kind() == "interface_type" {
                            let methods = extract_interface_methods(&type_node, src);
                            if !methods.is_empty() {
                                interfaces.push((name, qn, methods));
                            }
                        } else if type_node.kind() == "struct_type" {
                            struct_qns.insert(name, qn);
                        }
                    }
                }
                "method_declaration" => {
                    if let Some(mname) = child.child_by_field_name("name") {
                        if let Some(recv) = receiver_type_name(&child, src) {
                            struct_methods
                                .entry(recv)
                                .or_default()
                                .insert(node_text(&mname, src).to_string());
                        }
                    }
                }
                _ => {}
            }
        }
    }

    // Match: for each interface, find structs whose method set is a superset
    for (iface_name, iface_qn, iface_methods) in &interfaces {
        for (struct_name, methods) in &struct_methods {
            if struct_name == iface_name {
                continue;
            }
            if iface_methods.iter().all(|m| methods.contains(m)) {
                let struct_qn = struct_qns
                    .get(struct_name)
                    .cloned()
                    .unwrap_or_else(|| format!("{project}.{struct_name}"));
                buf.add_edge_by_qn(&struct_qn, iface_qn, "IMPLEMENTS", None);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::Registry;
    use codryn_graph_buffer::GraphBuffer;

    fn parse_go(source: &str) -> (GraphBuffer, Registry) {
        use std::sync::atomic::{AtomicU32, Ordering};
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let id = COUNTER.fetch_add(1, Ordering::Relaxed);
        let tmp = format!("/tmp/codryn_go_test_{id}.go");
        let mut buf = GraphBuffer::new("p");
        let mut reg = Registry::new();
        let file = DiscoveredFile {
            abs_path: tmp.clone().into(),
            rel_path: "test.go".into(),
            language: codryn_discover::Language::Go,
        };
        std::fs::write(&tmp, source).unwrap();
        extract_go(&mut buf, &mut reg, "p", &file);
        let _ = std::fs::remove_file(&tmp);
        (buf, reg)
    }

    #[test]
    fn test_extract_struct_and_interface() {
        let (buf, reg) = parse_go(
            r#"package main

type Handler struct {
    svc *Service
}

type Reader interface {
    Read(p []byte) (int, error)
}
"#,
        );
        assert!(
            !reg.lookup("Handler").is_empty(),
            "Handler should be registered"
        );
        assert!(
            !reg.lookup("Reader").is_empty(),
            "Reader should be registered"
        );
        assert_eq!(reg.lookup("Handler")[0].label, "Class");
        assert_eq!(reg.lookup("Reader")[0].label, "Interface");
        assert!(buf.node_count() >= 3); // Handler + Reader + Module
    }

    #[test]
    fn test_extract_function_and_method() {
        let (buf, reg) = parse_go(
            r#"package main

type Server struct{}

func NewServer() *Server { return &Server{} }

func (s *Server) Start() {}
func (s Server) Stop() {}
"#,
        );
        assert!(!reg.lookup("NewServer").is_empty());
        assert_eq!(reg.lookup("NewServer")[0].label, "Function");
        assert!(!reg.lookup("Start").is_empty());
        assert_eq!(reg.lookup("Start")[0].label, "Method");
        assert!(!reg.lookup("Stop").is_empty());
        assert_eq!(reg.lookup("Stop")[0].label, "Method");
        // struct + 2 methods + 1 function + module = 5
        assert!(buf.node_count() >= 4);
        // CONTAINS edges: Server->Start, Server->Stop
        assert!(buf.edge_count() >= 2);
    }

    #[test]
    fn test_test_file_detection() {
        use std::sync::atomic::{AtomicU32, Ordering};
        static COUNTER: AtomicU32 = AtomicU32::new(100);
        let id = COUNTER.fetch_add(1, Ordering::Relaxed);
        let tmp = format!("/tmp/codryn_go_test_{id}_test.go");
        let mut buf = GraphBuffer::new("p");
        let mut reg = Registry::new();
        let file = DiscoveredFile {
            abs_path: tmp.clone().into(),
            rel_path: "handler/handler_test.go".into(),
            language: codryn_discover::Language::Go,
        };
        std::fs::write(
            &tmp,
            r#"package handler

import "testing"

func TestCreate(t *testing.T) {}
func BenchmarkCreate(b *testing.B) {}
func setup() {}
"#,
        )
        .unwrap();
        extract_go(&mut buf, &mut reg, "p", &file);
        assert!(!reg.lookup("TestCreate").is_empty());
        assert!(!reg.lookup("BenchmarkCreate").is_empty());
        assert!(!reg.lookup("setup").is_empty());
    }

    #[test]
    fn test_layer_classification() {
        use std::sync::atomic::{AtomicU32, Ordering};
        static COUNTER: AtomicU32 = AtomicU32::new(200);
        let id = COUNTER.fetch_add(1, Ordering::Relaxed);
        let tmp = format!("/tmp/codryn_go_test_{id}.go");
        let mut buf = GraphBuffer::new("p");
        let mut reg = Registry::new();
        let file = DiscoveredFile {
            abs_path: tmp.clone().into(),
            rel_path: "handler/handler.go".into(),
            language: codryn_discover::Language::Go,
        };
        std::fs::write(&tmp, "package handler\n\ntype Handler struct{}\n").unwrap();
        extract_go(&mut buf, &mut reg, "p", &file);
        assert!(!reg.lookup("Handler").is_empty());
    }
}
