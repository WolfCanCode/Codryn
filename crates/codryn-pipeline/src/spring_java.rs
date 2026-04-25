use codryn_discover::DiscoveredFile;
use codryn_foundation::fqn;
use codryn_graph_buffer::GraphBuffer;
use tree_sitter::{Node, Parser};

use crate::registry::Registry;
use crate::spring_common::*;

fn make_parser() -> Option<Parser> {
    let mut parser = Parser::new();
    let lang: tree_sitter::Language = tree_sitter_java::LANGUAGE.into();
    parser.set_language(&lang).ok()?;
    Some(parser)
}

/// Extract definitions from a Java file using tree-sitter AST.
pub fn extract_java(
    buf: &mut GraphBuffer,
    reg: &mut Registry,
    project: &str,
    file: &DiscoveredFile,
) {
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

    for i in 0..root.child_count() {
        let child = root.child(i).unwrap();
        match child.kind() {
            "class_declaration" | "interface_declaration" | "enum_declaration" => {
                extract_class(buf, reg, project, file, src, &child);
            }
            _ => {}
        }
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
pub fn register_java(reg: &mut Registry, project: &str, file: &DiscoveredFile) {
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
            "class_declaration" | "interface_declaration" | "enum_declaration" => {
                register_class(reg, project, file, src, &child);
            }
            _ => {}
        }
    }
}

// ── Helpers ───────────────────────────────────────────

fn node_text<'a>(node: &Node, src: &'a [u8]) -> &'a str {
    node.utf8_text(src).unwrap_or("")
}

fn get_annotations(node: &Node, src: &[u8]) -> Vec<String> {
    let mut anns = Vec::new();
    // modifiers can be a field or a child depending on node type
    let mods = node.child_by_field_name("modifiers").or_else(|| {
        (0..node.child_count()).find_map(|i| {
            let c = node.child(i).unwrap();
            if c.kind() == "modifiers" {
                Some(c)
            } else {
                None
            }
        })
    });
    if let Some(mods) = mods {
        for j in 0..mods.child_count() {
            let c = mods.child(j).unwrap();
            match c.kind() {
                "marker_annotation" | "annotation" => {
                    if let Some(name) = c.child_by_field_name("name") {
                        anns.push(node_text(&name, src).to_string());
                    }
                }
                _ => {}
            }
        }
    }
    anns
}

fn find_modifiers<'a>(node: &'a Node<'a>) -> Option<Node<'a>> {
    node.child_by_field_name("modifiers").or_else(|| {
        (0..node.child_count()).find_map(|i| {
            let c = node.child(i).unwrap();
            if c.kind() == "modifiers" {
                Some(c)
            } else {
                None
            }
        })
    })
}

fn get_annotation_args_text(node: &Node, src: &[u8], ann_name: &str) -> Option<String> {
    let mods = find_modifiers(node)?;
    for j in 0..mods.child_count() {
        let c = mods.child(j).unwrap();
        if c.kind() == "annotation" {
            if let Some(name) = c.child_by_field_name("name") {
                if node_text(&name, src) == ann_name {
                    if let Some(args) = c.child_by_field_name("arguments") {
                        return Some(node_text(&args, src).to_string());
                    }
                }
            }
        }
    }
    None
}

fn extract_class(
    buf: &mut GraphBuffer,
    reg: &mut Registry,
    project: &str,
    file: &DiscoveredFile,
    src: &[u8],
    node: &Node,
) {
    let name = match node.child_by_field_name("name") {
        Some(n) => node_text(&n, src).to_string(),
        None => return,
    };
    let annotations = get_annotations(node, src);
    let ann_refs: Vec<&str> = annotations.iter().map(|s| s.as_str()).collect();
    let layer = classify_layer(&name, &ann_refs, &file.rel_path);
    let label = match node.kind() {
        "interface_declaration" => "Interface",
        "enum_declaration" => "Class",
        _ => "Class",
    };

    // Detect base path from @RequestMapping
    let base_path = get_annotation_args_text(node, src, "RequestMapping")
        .and_then(|t| extract_annotation_string(&t))
        .unwrap_or_default();

    let is_controller = annotations
        .iter()
        .any(|a| SPRING_CONTROLLER_ANNOTATIONS.contains(&a.as_str()));

    let mut props = serde_json::Map::new();
    if !annotations.is_empty() {
        props.insert("annotations".into(), serde_json::json!(annotations));
    }
    if let Some(l) = layer {
        props.insert("layer".into(), serde_json::json!(l));
    }
    if !base_path.is_empty() {
        props.insert("base_path".into(), serde_json::json!(base_path));
    }
    let props_json = if props.is_empty() {
        None
    } else {
        Some(serde_json::Value::Object(props).to_string())
    };

    let start = node.start_position().row as i32 + 1;
    let end = node.end_position().row as i32 + 1;
    let qn = fqn::fqn_compute(project, &file.rel_path, Some(&name));

    buf.add_node(label, &name, &qn, &file.rel_path, start, end, props_json);
    reg.register(&name, &qn, &file.rel_path, label, start, end);

    // FTS content
    let content = node_text(node, src);
    buf.add_code_content(&qn, content);

    // Methods inside class body
    if let Some(body) = node.child_by_field_name("body") {
        for j in 0..body.child_count() {
            let child = body.child(j).unwrap();
            if child.kind() == "method_declaration" {
                extract_method(
                    buf,
                    reg,
                    project,
                    file,
                    src,
                    &child,
                    &base_path,
                    is_controller,
                    layer,
                );
            }
        }
    }

    // INHERITS / IMPLEMENTS edges
    if let Some(sc) = node.child_by_field_name("superclass") {
        // superclass node wraps the type
        for k in 0..sc.child_count() {
            let t = sc.child(k).unwrap();
            if t.kind() == "type_identifier" || t.kind() == "generic_type" {
                let parent = first_type_name(&t, src);
                if !parent.is_empty() {
                    let tgt = format!("{project}.{parent}");
                    buf.add_edge_by_qn(&qn, &tgt, "INHERITS", None);
                }
            }
        }
    }
    if let Some(si) = node.child_by_field_name("interfaces") {
        extract_type_list(si, src).iter().for_each(|iface| {
            let tgt = format!("{project}.{iface}");
            buf.add_edge_by_qn(&qn, &tgt, "IMPLEMENTS", None);
        });
    }
}

fn register_class(
    reg: &mut Registry,
    project: &str,
    file: &DiscoveredFile,
    src: &[u8],
    node: &Node,
) {
    let name = match node.child_by_field_name("name") {
        Some(n) => node_text(&n, src).to_string(),
        None => return,
    };
    let label = if node.kind() == "interface_declaration" {
        "Interface"
    } else {
        "Class"
    };
    let start = node.start_position().row as i32 + 1;
    let end = node.end_position().row as i32 + 1;
    let qn = fqn::fqn_compute(project, &file.rel_path, Some(&name));
    reg.register(&name, &qn, &file.rel_path, label, start, end);

    if let Some(body) = node.child_by_field_name("body") {
        for j in 0..body.child_count() {
            let child = body.child(j).unwrap();
            if child.kind() == "method_declaration" {
                if let Some(mn) = child.child_by_field_name("name") {
                    let mname = node_text(&mn, src).to_string();
                    let ms = child.start_position().row as i32 + 1;
                    let me = child.end_position().row as i32 + 1;
                    let mqn = fqn::fqn_compute(project, &file.rel_path, Some(&mname));
                    reg.register(&mname, &mqn, &file.rel_path, "Method", ms, me);
                }
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn extract_method(
    buf: &mut GraphBuffer,
    reg: &mut Registry,
    project: &str,
    file: &DiscoveredFile,
    src: &[u8],
    node: &Node,
    class_base_path: &str,
    is_controller: bool,
    class_layer: Option<&str>,
) {
    let name = match node.child_by_field_name("name") {
        Some(n) => node_text(&n, src).to_string(),
        None => return,
    };
    let annotations = get_annotations(node, src);
    let ann_refs: Vec<&str> = annotations.iter().map(|s| s.as_str()).collect();
    let layer = classify_layer(&name, &ann_refs, &file.rel_path).or(class_layer);

    let mut props = serde_json::Map::new();
    if !annotations.is_empty() {
        props.insert("annotations".into(), serde_json::json!(annotations));
    }
    if let Some(l) = layer {
        props.insert("layer".into(), serde_json::json!(l));
    }

    let start = node.start_position().row as i32 + 1;
    let end = node.end_position().row as i32 + 1;
    let qn = fqn::fqn_compute(project, &file.rel_path, Some(&name));

    // Detect mapping annotation for route creation
    let mut http_method: Option<String> = None;
    let mut method_path = String::new();

    for ann in &annotations {
        if let Some(m) = resolve_http_method(ann) {
            http_method = Some(m.to_string());
            if let Some(args) = get_annotation_args_text(node, src, ann) {
                if let Some(p) = extract_annotation_string(&args) {
                    method_path = p;
                }
            }
            break;
        }
        if ann == "RequestMapping" {
            if let Some(args) = get_annotation_args_text(node, src, "RequestMapping") {
                let args_text = args.to_string();
                http_method = Some(request_mapping_method(&args_text).to_string());
                if let Some(p) = extract_annotation_string(&args_text) {
                    method_path = p;
                }
            } else {
                http_method = Some("GET".into());
            }
            break;
        }
    }

    // Return type
    let return_type = node
        .child_by_field_name("type")
        .map(|t| node_text(&t, src).to_string())
        .unwrap_or_default();

    if !return_type.is_empty() {
        props.insert("return_type".into(), serde_json::json!(return_type));
    }

    let props_json = if props.is_empty() {
        None
    } else {
        Some(serde_json::Value::Object(props).to_string())
    };
    buf.add_node("Method", &name, &qn, &file.rel_path, start, end, props_json);
    reg.register(&name, &qn, &file.rel_path, "Method", start, end);

    // FTS
    let content = node_text(node, src);
    buf.add_code_content(&qn, content);

    // Route creation
    if is_controller {
        if let Some(hm) = http_method {
            let full_path = combine_paths(class_base_path, &method_path);
            let route_name = format!("{hm} {full_path}");
            let route_qn = format!("{project}.route.{hm}.{full_path}");
            let mut route_props =
                serde_json::json!({"http_method": hm, "path": full_path, "method_name": name});

            // DTO edges + store type names as fallback properties
            // Request body
            if let Some(params) = node.child_by_field_name("parameters") {
                for k in 0..params.child_count() {
                    let param = params.child(k).unwrap();
                    if param.kind() == "formal_parameter" || param.kind() == "spread_parameter" {
                        let param_anns = get_param_annotations(&param, src);
                        if param_anns.iter().any(|a| a == "RequestBody") {
                            if let Some(ptype) = param.child_by_field_name("type") {
                                let type_text = node_text(&ptype, src);
                                let dto = unwrap_generic_type(type_text);
                                let dto = first_simple_name(dto);
                                if is_dto_candidate(dto) {
                                    route_props["request_dto_type"] = serde_json::json!(dto);
                                    let dto_qn = format!("{project}.{dto}");
                                    buf.add_edge_by_qn(&route_qn, &dto_qn, "ACCEPTS_DTO", None);
                                }
                            }
                        }
                    }
                }
            }

            // Response DTO
            let ret = unwrap_generic_type(&return_type);
            let ret = first_simple_name(ret);
            if is_dto_candidate(ret) {
                route_props["response_dto_type"] = serde_json::json!(ret);
                let ret_qn = format!("{project}.{ret}");
                buf.add_edge_by_qn(&route_qn, &ret_qn, "RETURNS_DTO", None);
            }

            buf.add_node(
                "Route",
                &route_name,
                &route_qn,
                &file.rel_path,
                start,
                end,
                Some(route_props.to_string()),
            );
            buf.add_edge_by_qn(&qn, &route_qn, "HANDLES_ROUTE", None);
        }
    }
}

fn get_param_annotations(param: &Node, src: &[u8]) -> Vec<String> {
    let mut anns = Vec::new();
    for i in 0..param.child_count() {
        let c = param.child(i).unwrap();
        match c.kind() {
            "marker_annotation" => {
                if let Some(name) = c.child_by_field_name("name") {
                    anns.push(node_text(&name, src).to_string());
                }
            }
            "annotation" => {
                if let Some(name) = c.child_by_field_name("name") {
                    anns.push(node_text(&name, src).to_string());
                }
            }
            "modifiers" => {
                for j in 0..c.child_count() {
                    let mc = c.child(j).unwrap();
                    if mc.kind() == "marker_annotation" || mc.kind() == "annotation" {
                        if let Some(name) = mc.child_by_field_name("name") {
                            anns.push(node_text(&name, src).to_string());
                        }
                    }
                }
            }
            _ => {}
        }
    }
    anns
}

fn first_type_name(node: &Node, src: &[u8]) -> String {
    if node.kind() == "type_identifier" {
        return node_text(node, src).to_string();
    }
    if node.kind() == "generic_type" {
        for i in 0..node.child_count() {
            let c = node.child(i).unwrap();
            if c.kind() == "type_identifier" {
                return node_text(&c, src).to_string();
            }
        }
    }
    node_text(node, src).to_string()
}

fn extract_type_list(node: Node, src: &[u8]) -> Vec<String> {
    let mut types = Vec::new();
    for i in 0..node.child_count() {
        let c = node.child(i).unwrap();
        if c.kind() == "type_identifier" || c.kind() == "generic_type" {
            types.push(first_type_name(&c, src));
        }
        // type_list wraps types
        if c.kind() == "type_list" {
            for j in 0..c.child_count() {
                let t = c.child(j).unwrap();
                if t.kind() == "type_identifier" || t.kind() == "generic_type" {
                    types.push(first_type_name(&t, src));
                }
            }
        }
    }
    types
}

/// Extract first simple name from a potentially qualified type like "com.example.UserDto" → "UserDto"
fn first_simple_name(s: &str) -> &str {
    let s = s.split('<').next().unwrap_or(s);
    s.rsplit('.').next().unwrap_or(s).trim()
}

/// Create Route nodes + edges only (for pass_spring_routes). Parses the file fresh.
pub fn create_routes(buf: &mut GraphBuffer, project: &str, file: &DiscoveredFile) {
    let source = match std::fs::read_to_string(&file.abs_path) {
        Ok(s) => s,
        Err(_) => return,
    };
    let mut parser = match make_parser() {
        Some(p) => p,
        None => return,
    };
    let tree = match parser.parse(&source, None) {
        Some(t) => t,
        None => return,
    };
    let src = source.as_bytes();
    for i in 0..tree.root_node().child_count() {
        let child = tree.root_node().child(i).unwrap();
        if child.kind() == "class_declaration" {
            let anns = get_annotations(&child, src);
            if !anns
                .iter()
                .any(|a| SPRING_CONTROLLER_ANNOTATIONS.contains(&a.as_str()))
            {
                continue;
            }
            let base_path = get_annotation_args_text(&child, src, "RequestMapping")
                .and_then(|t| extract_annotation_string(&t))
                .unwrap_or_default();
            if let Some(body) = child.child_by_field_name("body") {
                for j in 0..body.child_count() {
                    let m = body.child(j).unwrap();
                    if m.kind() == "method_declaration" {
                        create_route_for_method(buf, project, file, src, &m, &base_path);
                    }
                }
            }
        }
    }
}

fn create_route_for_method(
    buf: &mut GraphBuffer,
    project: &str,
    file: &DiscoveredFile,
    src: &[u8],
    node: &Node,
    base_path: &str,
) {
    let mname = match node.child_by_field_name("name") {
        Some(n) => node_text(&n, src).to_string(),
        None => return,
    };
    let anns = get_annotations(node, src);
    let (hm, mpath) = match find_mapping(&anns, node, src) {
        Some(v) => v,
        None => return,
    };
    let full_path = combine_paths(base_path, &mpath);
    let route_qn = format!("{project}.route.{hm}.{full_path}");
    let method_qn = fqn::fqn_compute(project, &file.rel_path, Some(&mname));
    let s = node.start_position().row as i32 + 1;
    let e = node.end_position().row as i32 + 1;
    let mut props = serde_json::json!({"http_method": hm, "path": full_path, "method_name": mname});
    // Request DTO
    if let Some(params) = node.child_by_field_name("parameters") {
        for k in 0..params.child_count() {
            let p = params.child(k).unwrap();
            if p.kind() == "formal_parameter"
                && get_param_annotations(&p, src)
                    .iter()
                    .any(|a| a == "RequestBody")
            {
                if let Some(pt) = p.child_by_field_name("type") {
                    let dto = first_simple_name(unwrap_generic_type(node_text(&pt, src)));
                    if is_dto_candidate(dto) {
                        props["request_dto_type"] = serde_json::json!(dto);
                        buf.add_edge_by_qn(
                            &route_qn,
                            &format!("{project}.{dto}"),
                            "ACCEPTS_DTO",
                            None,
                        );
                    }
                }
            }
        }
    }
    // Response DTO
    if let Some(rt) = node.child_by_field_name("type") {
        let ret = first_simple_name(unwrap_generic_type(node_text(&rt, src)));
        if is_dto_candidate(ret) {
            props["response_dto_type"] = serde_json::json!(ret);
            buf.add_edge_by_qn(&route_qn, &format!("{project}.{ret}"), "RETURNS_DTO", None);
        }
    }
    buf.add_node(
        "Route",
        &format!("{hm} {full_path}"),
        &route_qn,
        &file.rel_path,
        s,
        e,
        Some(props.to_string()),
    );
    buf.add_edge_by_qn(&method_qn, &route_qn, "HANDLES_ROUTE", None);
}

fn find_mapping(anns: &[String], node: &Node, src: &[u8]) -> Option<(String, String)> {
    for ann in anns {
        if let Some(m) = resolve_http_method(ann) {
            let path = get_annotation_args_text(node, src, ann)
                .and_then(|t| extract_annotation_string(&t))
                .unwrap_or_default();
            return Some((m.to_string(), path));
        }
        if ann == "RequestMapping" {
            let args = get_annotation_args_text(node, src, "RequestMapping").unwrap_or_default();
            let method = request_mapping_method(&args).to_string();
            let path = extract_annotation_string(&args).unwrap_or_default();
            return Some((method, path));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::Registry;
    use codryn_graph_buffer::GraphBuffer;

    fn parse_java(source: &str) -> (GraphBuffer, Registry) {
        let mut buf = GraphBuffer::new("p");
        let mut reg = Registry::new();
        let mut parser = make_parser().unwrap();
        let tree = parser.parse(source, None).unwrap();
        let src = source.as_bytes();
        let root = tree.root_node();
        for i in 0..root.child_count() {
            let child = root.child(i).unwrap();
            match child.kind() {
                "class_declaration" | "interface_declaration" | "enum_declaration" => {
                    extract_class(&mut buf, &mut reg, "p", &fake_file(), src, &child);
                }
                _ => {}
            }
        }
        (buf, reg)
    }

    fn fake_file() -> DiscoveredFile {
        DiscoveredFile {
            abs_path: "/tmp/UserController.java".into(),
            rel_path: "src/controller/UserController.java".into(),
            language: codryn_discover::Language::Java,
        }
    }

    #[test]
    fn test_extract_controller_class() {
        let (buf, reg) = parse_java(
            r#"
@RestController
@RequestMapping("/api")
public class UserController {
    @GetMapping("/users")
    public List<UserDto> getUsers() {
        return null;
    }
}
"#,
        );
        assert!(
            buf.node_count() >= 2,
            "should have class + method nodes, got {}",
            buf.node_count()
        );
        assert!(!reg.lookup("UserController").is_empty());
        assert!(!reg.lookup("getUsers").is_empty());
    }

    #[test]
    fn test_route_and_dto_edges() {
        let (buf, _) = parse_java(
            r#"
@RestController
@RequestMapping("/api")
public class UserController {
    @PostMapping("/users")
    public ResponseEntity<UserDto> createUser(@RequestBody CreateUserRequest req) {
        return null;
    }
}
"#,
        );
        // Should have: class, method, route = 3 nodes minimum
        assert!(
            buf.node_count() >= 3,
            "expected >=3 nodes, got {}",
            buf.node_count()
        );
        // Should have edges: HANDLES_ROUTE, ACCEPTS_DTO, RETURNS_DTO
        assert!(
            buf.edge_count() >= 3,
            "expected >=3 edges, got {}",
            buf.edge_count()
        );
    }

    #[test]
    fn test_patch_route() {
        let (buf, _) = parse_java(
            r#"
@RestController
@RequestMapping("/v1")
public class OrderController {
    @PatchMapping("/orders/{id}")
    public OrderDto update(@RequestBody UpdateOrderRequest body) {
        return null;
    }
}
"#,
        );
        assert!(buf.node_count() >= 3);
        assert!(buf.edge_count() >= 3);
    }

    #[test]
    fn test_layer_classification() {
        let (buf, _) = parse_java(
            r#"
@Service
public class UserService {
    public void doStuff() {}
}
"#,
        );
        // Service class should have layer property
        assert!(buf.node_count() >= 2);
    }

    #[test]
    fn test_interface_extraction() {
        let (buf, reg) = parse_java(
            r#"
public interface UserRepository extends JpaRepository<User, Long> {
    List<User> findByName(String name);
}
"#,
        );
        assert!(!reg.lookup("UserRepository").is_empty());
        assert!(buf.node_count() >= 1);
    }
}
