use codryn_discover::DiscoveredFile;
use codryn_foundation::fqn;
use codryn_graph_buffer::GraphBuffer;
use tree_sitter::{Node, Parser};

use crate::registry::Registry;
use crate::spring_common::*;

fn make_parser() -> Option<Parser> {
    let mut parser = Parser::new();
    let lang: tree_sitter::Language = tree_sitter_kotlin_ng::LANGUAGE.into();
    parser.set_language(&lang).ok()?;
    Some(parser)
}

pub fn extract_kotlin(
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
    walk_top_level(buf, reg, project, file, src, &tree.root_node());

    let module_qn = fqn::fqn_module(project, &file.rel_path);
    let module_name = std::path::Path::new(&file.rel_path)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(&file.rel_path);
    let lines = source.lines().count();
    buf.add_node(
        "Module",
        module_name,
        &module_qn,
        &file.rel_path,
        1,
        lines as i32,
        None,
    );
}

pub fn register_kotlin(reg: &mut Registry, project: &str, file: &DiscoveredFile) {
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
    walk_top_level_register(reg, project, file, src, &tree.root_node());
}

fn walk_top_level(
    buf: &mut GraphBuffer,
    reg: &mut Registry,
    project: &str,
    file: &DiscoveredFile,
    src: &[u8],
    node: &Node,
) {
    for i in 0..node.child_count() {
        let child = node.child(i).unwrap();
        match child.kind() {
            "class_declaration" | "object_declaration" => {
                extract_class(buf, reg, project, file, src, &child);
            }
            "function_declaration" => {
                extract_function(buf, reg, project, file, src, &child, "", false, None);
            }
            _ => {
                // Recurse into source_file children
                if child.kind() == "source_file" {
                    walk_top_level(buf, reg, project, file, src, &child);
                }
            }
        }
    }
}

fn walk_top_level_register(
    reg: &mut Registry,
    project: &str,
    file: &DiscoveredFile,
    src: &[u8],
    node: &Node,
) {
    for i in 0..node.child_count() {
        let child = node.child(i).unwrap();
        match child.kind() {
            "class_declaration" | "object_declaration" => {
                register_class(reg, project, file, src, &child);
            }
            "function_declaration" => {
                if let Some(name) = find_name(&child, src) {
                    let s = child.start_position().row as i32 + 1;
                    let e = child.end_position().row as i32 + 1;
                    let qn = fqn::fqn_compute(project, &file.rel_path, Some(&name));
                    reg.register(&name, &qn, &file.rel_path, "Function", s, e);
                }
            }
            _ => {}
        }
    }
}

// ── Helpers ───────────────────────────────────────────

fn node_text<'a>(node: &Node, src: &'a [u8]) -> &'a str {
    node.utf8_text(src).unwrap_or("")
}

/// Find the name of a Kotlin class/function declaration.
fn find_name(node: &Node, src: &[u8]) -> Option<String> {
    for i in 0..node.child_count() {
        let c = node.child(i).unwrap();
        match c.kind() {
            "type_identifier" | "simple_identifier" | "identifier" => {
                return Some(node_text(&c, src).to_string())
            }
            _ => {}
        }
    }
    None
}

fn get_annotations(node: &Node, src: &[u8]) -> Vec<String> {
    let mut anns = Vec::new();
    for i in 0..node.child_count() {
        let c = node.child(i).unwrap();
        if c.kind() == "modifiers" {
            for j in 0..c.child_count() {
                let mc = c.child(j).unwrap();
                if mc.kind() == "annotation" {
                    // Kotlin annotations: annotation -> user_type or constructor_invocation
                    for k in 0..mc.child_count() {
                        let ac = mc.child(k).unwrap();
                        match ac.kind() {
                            "user_type" => {
                                anns.push(node_text(&ac, src).to_string());
                            }
                            "constructor_invocation" => {
                                // First child is user_type
                                if let Some(ut) = ac.child(0) {
                                    if ut.kind() == "user_type" {
                                        anns.push(node_text(&ut, src).to_string());
                                    }
                                }
                            }
                            _ => {}
                        }
                    }
                }
            }
        }
    }
    anns
}

fn is_data_class(node: &Node, src: &[u8]) -> bool {
    for i in 0..node.child_count() {
        let c = node.child(i).unwrap();
        if c.kind() == "modifiers" {
            for j in 0..c.child_count() {
                let mc = c.child(j).unwrap();
                if node_text(&mc, src) == "data" {
                    return true;
                }
            }
        }
    }
    false
}

fn get_annotation_string_arg(node: &Node, src: &[u8], ann_name: &str) -> Option<String> {
    for i in 0..node.child_count() {
        let c = node.child(i).unwrap();
        if c.kind() == "modifiers" {
            for j in 0..c.child_count() {
                let mc = c.child(j).unwrap();
                if mc.kind() == "annotation" {
                    let text = node_text(&mc, src);
                    if text.contains(ann_name) {
                        return extract_annotation_string(text);
                    }
                }
            }
        }
    }
    None
}

fn find_return_type(node: &Node, src: &[u8]) -> Option<String> {
    // Return type is user_type after `:` which comes after function_value_parameters
    let mut past_colon = false;
    for i in 0..node.child_count() {
        let c = node.child(i).unwrap();
        if c.kind() == ":" {
            past_colon = true;
            continue;
        }
        if past_colon && (c.kind() == "user_type" || c.kind() == "nullable_type") {
            return Some(node_text(&c, src).to_string());
        }
        // Reset if we hit function_body without finding type
        if c.kind() == "function_body" {
            break;
        }
    }
    None
}

fn find_class_body<'a>(node: &'a Node<'a>) -> Option<Node<'a>> {
    for i in 0..node.child_count() {
        let c = node.child(i).unwrap();
        if c.kind() == "class_body" || c.kind() == "enum_class_body" {
            return Some(c);
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
    let name = match find_name(node, src) {
        Some(n) => n,
        None => return,
    };
    let annotations = get_annotations(node, src);
    let ann_refs: Vec<&str> = annotations.iter().map(|s| s.as_str()).collect();
    let data = is_data_class(node, src);
    let layer = if data {
        Some("dto")
    } else {
        classify_layer(&name, &ann_refs, &file.rel_path)
    };
    let is_controller = annotations
        .iter()
        .any(|a| SPRING_CONTROLLER_ANNOTATIONS.contains(&a.as_str()));

    let base_path = get_annotation_string_arg(node, src, "RequestMapping").unwrap_or_default();

    let label = "Class";

    let mut props = serde_json::Map::new();
    if !annotations.is_empty() {
        props.insert("annotations".into(), serde_json::json!(annotations));
    }
    if let Some(l) = layer {
        props.insert("layer".into(), serde_json::json!(l));
    }
    if data {
        props.insert("data_class".into(), serde_json::json!(true));
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
    buf.add_code_content(&qn, node_text(node, src));

    if let Some(body) = find_class_body(node) {
        for j in 0..body.child_count() {
            let child = body.child(j).unwrap();
            if child.kind() == "function_declaration" {
                extract_function(
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
}

fn register_class(
    reg: &mut Registry,
    project: &str,
    file: &DiscoveredFile,
    src: &[u8],
    node: &Node,
) {
    let name = match find_name(node, src) {
        Some(n) => n,
        None => return,
    };
    let start = node.start_position().row as i32 + 1;
    let end = node.end_position().row as i32 + 1;
    let qn = fqn::fqn_compute(project, &file.rel_path, Some(&name));
    reg.register(&name, &qn, &file.rel_path, "Class", start, end);

    if let Some(body) = find_class_body(node) {
        for j in 0..body.child_count() {
            let child = body.child(j).unwrap();
            if child.kind() == "function_declaration" {
                if let Some(fname) = find_name(&child, src) {
                    let s = child.start_position().row as i32 + 1;
                    let e = child.end_position().row as i32 + 1;
                    let fqn = fqn::fqn_compute(project, &file.rel_path, Some(&fname));
                    reg.register(&fname, &fqn, &file.rel_path, "Method", s, e);
                }
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn extract_function(
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
    let name = match find_name(node, src) {
        Some(n) => n,
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

    let return_type = find_return_type(node, src).unwrap_or_default();
    if !return_type.is_empty() {
        props.insert("return_type".into(), serde_json::json!(return_type));
    }

    let start = node.start_position().row as i32 + 1;
    let end = node.end_position().row as i32 + 1;
    let qn = fqn::fqn_compute(project, &file.rel_path, Some(&name));
    let label = if class_base_path.is_empty() && class_layer.is_none() {
        "Function"
    } else {
        "Method"
    };

    let props_json = if props.is_empty() {
        None
    } else {
        Some(serde_json::Value::Object(props).to_string())
    };
    buf.add_node(label, &name, &qn, &file.rel_path, start, end, props_json);
    reg.register(&name, &qn, &file.rel_path, label, start, end);
    buf.add_code_content(&qn, node_text(node, src));

    // Route creation
    if is_controller {
        let mut http_method: Option<String> = None;
        let mut method_path = String::new();

        for ann in &annotations {
            if let Some(m) = resolve_http_method(ann) {
                http_method = Some(m.to_string());
                if let Some(p) = get_annotation_string_arg(node, src, ann) {
                    method_path = p;
                }
                break;
            }
            if ann == "RequestMapping" {
                let args = get_annotation_string_arg(node, src, "RequestMapping");
                let full_text = node_text(node, src);
                http_method = Some(request_mapping_method(full_text).to_string());
                if let Some(p) = args {
                    method_path = p;
                }
                break;
            }
        }

        if let Some(hm) = http_method {
            let full_path = combine_paths(class_base_path, &method_path);
            let route_name = format!("{hm} {full_path}");
            let route_qn = format!("{project}.route.{hm}.{full_path}");
            let mut route_props =
                serde_json::json!({"http_method": hm, "path": full_path, "method_name": name});

            // Request body DTO — scan function parameters for @RequestBody
            for i in 0..node.child_count() {
                let c = node.child(i).unwrap();
                if c.kind() == "function_value_parameters" {
                    let mut next_has_request_body = false;
                    for j in 0..c.child_count() {
                        let param = c.child(j).unwrap();
                        if param.kind() == "parameter_modifiers" {
                            let text = node_text(&param, src);
                            next_has_request_body = text.contains("RequestBody");
                            continue;
                        }
                        if param.kind() == "parameter" {
                            let has_rb = next_has_request_body
                                || node_text(&param, src).contains("RequestBody");
                            next_has_request_body = false;
                            if has_rb {
                                if let Some(ptype) = find_param_type(&param, src) {
                                    let dto = unwrap_generic_type(&ptype);
                                    let dto = dto.rsplit('.').next().unwrap_or(dto).trim();
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
            }

            // Response DTO
            let ret = unwrap_generic_type(&return_type);
            let ret = ret.rsplit('.').next().unwrap_or(ret).trim();
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

fn find_param_type(param: &Node, src: &[u8]) -> Option<String> {
    // In Kotlin, parameter type is after `:` — look for user_type
    for i in 0..param.child_count() {
        let c = param.child(i).unwrap();
        if c.kind() == "user_type" || c.kind() == "nullable_type" {
            return Some(node_text(&c, src).to_string());
        }
    }
    None
}

/// Create Route nodes + edges only (for pass_spring_routes).
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
        if child.kind() == "class_declaration" || child.kind() == "object_declaration" {
            let anns = get_annotations(&child, src);
            if !anns
                .iter()
                .any(|a| SPRING_CONTROLLER_ANNOTATIONS.contains(&a.as_str()))
            {
                continue;
            }
            let base_path =
                get_annotation_string_arg(&child, src, "RequestMapping").unwrap_or_default();
            if let Some(body) = find_class_body(&child) {
                for j in 0..body.child_count() {
                    let m = body.child(j).unwrap();
                    if m.kind() == "function_declaration" {
                        let fname = match find_name(&m, src) {
                            Some(n) => n,
                            None => continue,
                        };
                        let manns = get_annotations(&m, src);
                        let mut hm = None;
                        let mut mpath = String::new();
                        for ann in &manns {
                            if let Some(method) = resolve_http_method(ann) {
                                hm = Some(method.to_string());
                                mpath = get_annotation_string_arg(&m, src, ann).unwrap_or_default();
                                break;
                            }
                            if ann == "RequestMapping" {
                                hm = Some(request_mapping_method(node_text(&m, src)).to_string());
                                mpath = get_annotation_string_arg(&m, src, "RequestMapping")
                                    .unwrap_or_default();
                                break;
                            }
                        }
                        let hm = match hm {
                            Some(h) => h,
                            None => continue,
                        };
                        let full_path = combine_paths(&base_path, &mpath);
                        let route_qn = format!("{project}.route.{hm}.{full_path}");
                        let method_qn = fqn::fqn_compute(project, &file.rel_path, Some(&fname));
                        let s = m.start_position().row as i32 + 1;
                        let e = m.end_position().row as i32 + 1;
                        let mut props = serde_json::json!({"http_method": hm, "path": full_path, "method_name": fname});
                        // Request DTO
                        for ci in 0..m.child_count() {
                            let c = m.child(ci).unwrap();
                            if c.kind() == "function_value_parameters" {
                                let mut next_rb = false;
                                for pj in 0..c.child_count() {
                                    let p = c.child(pj).unwrap();
                                    if p.kind() == "parameter_modifiers" {
                                        next_rb = node_text(&p, src).contains("RequestBody");
                                        continue;
                                    }
                                    if p.kind() == "parameter" && next_rb {
                                        if let Some(pt) = find_param_type(&p, src) {
                                            let dto = unwrap_generic_type(&pt);
                                            let dto = dto.rsplit('.').next().unwrap_or(dto).trim();
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
                                        next_rb = false;
                                    }
                                }
                            }
                        }
                        // Response DTO
                        let ret_type = find_return_type(&m, src).unwrap_or_default();
                        let ret = unwrap_generic_type(&ret_type);
                        let ret = ret.rsplit('.').next().unwrap_or(ret).trim();
                        if is_dto_candidate(ret) {
                            props["response_dto_type"] = serde_json::json!(ret);
                            buf.add_edge_by_qn(
                                &route_qn,
                                &format!("{project}.{ret}"),
                                "RETURNS_DTO",
                                None,
                            );
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
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::Registry;
    use codryn_graph_buffer::GraphBuffer;

    fn parse_kotlin(source: &str) -> (GraphBuffer, Registry) {
        let mut buf = GraphBuffer::new("p");
        let mut reg = Registry::new();
        let mut parser = make_parser().unwrap();
        let tree = parser.parse(source, None).unwrap();
        let src = source.as_bytes();
        walk_top_level(
            &mut buf,
            &mut reg,
            "p",
            &fake_file(),
            src,
            &tree.root_node(),
        );
        (buf, reg)
    }

    fn fake_file() -> DiscoveredFile {
        DiscoveredFile {
            abs_path: "/tmp/UserController.kt".into(),
            rel_path: "src/controller/UserController.kt".into(),
            language: codryn_discover::Language::Kotlin,
        }
    }

    #[test]
    fn test_kotlin_controller() {
        let (buf, reg) = parse_kotlin(
            r#"
@RestController
@RequestMapping("/api")
class UserController {
    @GetMapping("/users")
    fun getUsers(): List<UserDto> {
        return emptyList()
    }
}
"#,
        );
        assert!(!reg.lookup("UserController").is_empty());
        assert!(!reg.lookup("getUsers").is_empty());
        // class + method + route = 3 nodes
        assert!(
            buf.node_count() >= 3,
            "expected >=3 nodes, got {}",
            buf.node_count()
        );
    }

    #[test]
    fn test_kotlin_data_class() {
        let (buf, reg) = parse_kotlin(
            r#"
data class UserDto(
    val name: String,
    val email: String
)
"#,
        );
        assert!(!reg.lookup("UserDto").is_empty());
        assert!(buf.node_count() >= 1);
    }

    #[test]
    fn test_kotlin_post_with_body() {
        let (buf, _) = parse_kotlin(
            r#"
@RestController
@RequestMapping("/api")
class UserController {
    @PostMapping("/users")
    fun createUser(@RequestBody req: CreateUserRequest): ResponseEntity<UserDto> {
        return ResponseEntity.ok(UserDto())
    }
}
"#,
        );
        assert!(
            buf.node_count() >= 3,
            "expected >=3 nodes, got {}",
            buf.node_count()
        );
        assert!(
            buf.edge_count() >= 3,
            "expected >=3 edges, got {}",
            buf.edge_count()
        );
    }
}
