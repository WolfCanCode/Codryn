//! Go AST walker.
//!
//! Extracts symbols from Go source files using tree-sitter:
//! - Function declarations (`func foo(...)`)
//! - Method declarations (`func (r *Receiver) foo(...)`)
//! - Type declarations (struct, interface, type alias)
//! - Constants and variables (top-level)
//!
//! This walker provides the unified `TsSymbol` interface used by the rest of
//! the pipeline, complementing the dedicated `go_adapter.rs` which handles
//! route extraction and Go-specific graph edges.

use crate::{TsParam, TsSymbol};
use tree_sitter::Node;

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

pub fn walk_tree(tree: &tree_sitter::Tree, source: &str) -> Vec<TsSymbol> {
    let mut symbols = Vec::new();
    let src = source.as_bytes();
    let root = tree.root_node();

    for i in 0..root.child_count() {
        let child = root.child(i).unwrap();
        match child.kind() {
            "function_declaration" => {
                if let Some(sym) = extract_function(&child, src, None) {
                    symbols.push(sym);
                }
            }
            "method_declaration" => {
                if let Some(sym) = extract_method(&child, src) {
                    symbols.push(sym);
                }
            }
            "type_declaration" => {
                extract_type_decl(&child, src, &mut symbols);
            }
            "const_declaration" | "var_declaration" => {
                // Top-level constants and variables
                extract_const_var(&child, src, &mut symbols);
            }
            _ => {}
        }
    }

    symbols
}

// ---------------------------------------------------------------------------
// Function extraction
// ---------------------------------------------------------------------------

fn extract_function(node: &Node, src: &[u8], parent_name: Option<&str>) -> Option<TsSymbol> {
    let name_node = node.child_by_field_name("name")?;
    let name = node_text(name_node, src).to_string();
    if name.is_empty() {
        return None;
    }

    let params = extract_params(node, src);
    let return_type = extract_return_type(node, src);
    let body = node.child_by_field_name("body");
    let body_text = body.map(|b| node_text(b, src));
    let doc = collect_doc_comment(node, src);

    let is_exported = name
        .chars()
        .next()
        .map(|c| c.is_uppercase())
        .unwrap_or(false);
    let is_test =
        name.starts_with("Test") || name.starts_with("Benchmark") || name.starts_with("Example");
    let is_entry_point = name == "main";

    let label = if parent_name.is_some() {
        "Method"
    } else {
        "Function"
    };

    let sig = build_signature(&name, &params, return_type.as_deref(), false);

    Some(TsSymbol {
        name,
        label: label.into(),
        start_line: node.start_position().row as i32 + 1,
        end_line: node.end_position().row as i32 + 1,
        parent_name: parent_name.map(String::from),
        signature: Some(sig),
        return_type,
        parameters: params,
        docstring: doc,
        decorators: Vec::new(),
        base_classes: Vec::new(),
        is_exported,
        is_abstract: false,
        is_async: false,
        is_test,
        is_entry_point,
        body_text: body_text.map(String::from),
    })
}

// ---------------------------------------------------------------------------
// Method extraction
// ---------------------------------------------------------------------------

fn extract_method(node: &Node, src: &[u8]) -> Option<TsSymbol> {
    let name_node = node.child_by_field_name("name")?;
    let name = node_text(name_node, src).to_string();
    if name.is_empty() {
        return None;
    }

    // Extract receiver type
    let receiver_type = extract_receiver_type(node, src);
    let params = extract_params(node, src);
    let return_type = extract_return_type(node, src);
    let body = node.child_by_field_name("body");
    let body_text = body.map(|b| node_text(b, src));
    let doc = collect_doc_comment(node, src);

    let is_exported = name
        .chars()
        .next()
        .map(|c| c.is_uppercase())
        .unwrap_or(false);
    let sig = build_signature(&name, &params, return_type.as_deref(), false);

    Some(TsSymbol {
        name,
        label: "Method".into(),
        start_line: node.start_position().row as i32 + 1,
        end_line: node.end_position().row as i32 + 1,
        parent_name: receiver_type,
        signature: Some(sig),
        return_type,
        parameters: params,
        docstring: doc,
        decorators: Vec::new(),
        base_classes: Vec::new(),
        is_exported,
        is_abstract: false,
        is_async: false,
        is_test: false,
        is_entry_point: false,
        body_text: body_text.map(String::from),
    })
}

fn extract_receiver_type(node: &Node, src: &[u8]) -> Option<String> {
    let receiver = node.child_by_field_name("receiver")?;
    // receiver is a parameter_list with one parameter_declaration
    for i in 0..receiver.child_count() {
        let param = receiver.child(i)?;
        if param.kind() == "parameter_declaration" {
            if let Some(typ) = param.child_by_field_name("type") {
                let text = node_text(typ, src);
                // Strip pointer: *Handler -> Handler
                return Some(text.trim_start_matches('*').to_string());
            }
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Type declaration extraction
// ---------------------------------------------------------------------------

fn extract_type_decl(node: &Node, src: &[u8], symbols: &mut Vec<TsSymbol>) {
    // type_declaration can contain multiple type_spec children
    for i in 0..node.child_count() {
        let child = node.child(i).unwrap();
        if child.kind() == "type_spec" {
            if let Some(sym) = extract_type_spec(&child, src) {
                symbols.push(sym);
            }
        }
    }
}

fn extract_type_spec(node: &Node, src: &[u8]) -> Option<TsSymbol> {
    let name_node = node.child_by_field_name("name")?;
    let name = node_text(name_node, src).to_string();
    if name.is_empty() {
        return None;
    }

    let type_node = node.child_by_field_name("type")?;
    let type_kind = type_node.kind();
    let doc = collect_doc_comment(node, src);

    let (label, base_classes, is_abstract) = match type_kind {
        "struct_type" => ("Class", vec![], false),
        "interface_type" => {
            // Extract interface methods as base_classes (for IMPLEMENTS edges)
            let methods = extract_interface_methods(&type_node, src);
            ("Interface", methods, true)
        }
        _ => ("Class", vec![], false), // type alias, etc.
    };

    let is_exported = name
        .chars()
        .next()
        .map(|c| c.is_uppercase())
        .unwrap_or(false);
    let body_text = Some(node_text(type_node, src).to_string());

    Some(TsSymbol {
        name,
        label: label.into(),
        start_line: node.start_position().row as i32 + 1,
        end_line: node.end_position().row as i32 + 1,
        parent_name: None,
        signature: None,
        return_type: None,
        parameters: Vec::new(),
        docstring: doc,
        decorators: Vec::new(),
        base_classes,
        is_exported,
        is_abstract,
        is_async: false,
        is_test: false,
        is_entry_point: false,
        body_text,
    })
}

fn extract_interface_methods(interface_node: &Node, src: &[u8]) -> Vec<String> {
    let mut methods = Vec::new();
    for i in 0..interface_node.child_count() {
        let child = interface_node.child(i).unwrap();
        if child.kind() == "method_elem" || child.kind() == "method_spec" {
            if let Some(name_node) = child.child_by_field_name("name") {
                let name = node_text(name_node, src);
                if !name.is_empty() {
                    methods.push(name.to_string());
                }
            }
        }
    }
    methods
}

// ---------------------------------------------------------------------------
// Const/var extraction
// ---------------------------------------------------------------------------

fn extract_const_var(node: &Node, src: &[u8], symbols: &mut Vec<TsSymbol>) {
    let label = if node.kind() == "const_declaration" {
        "Constant"
    } else {
        "Variable"
    };

    // const_declaration / var_declaration contain const_spec / var_spec children
    let spec_kind = if node.kind() == "const_declaration" {
        "const_spec"
    } else {
        "var_spec"
    };

    for i in 0..node.child_count() {
        let child = node.child(i).unwrap();
        if child.kind() == spec_kind {
            if let Some(name_node) = child.child_by_field_name("name") {
                let name = node_text(name_node, src);
                if name.is_empty() {
                    continue;
                }
                let is_exported = name
                    .chars()
                    .next()
                    .map(|c| c.is_uppercase())
                    .unwrap_or(false);
                if !is_exported {
                    continue; // Skip unexported constants/variables
                }
                symbols.push(TsSymbol {
                    name: name.to_string(),
                    label: label.into(),
                    start_line: child.start_position().row as i32 + 1,
                    end_line: child.end_position().row as i32 + 1,
                    parent_name: None,
                    signature: None,
                    return_type: None,
                    parameters: Vec::new(),
                    docstring: None,
                    decorators: Vec::new(),
                    base_classes: Vec::new(),
                    is_exported: true,
                    is_abstract: false,
                    is_async: false,
                    is_test: false,
                    is_entry_point: false,
                    body_text: None,
                });
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Parameter extraction
// ---------------------------------------------------------------------------

fn extract_params(node: &Node, src: &[u8]) -> Vec<TsParam> {
    let params_node = match node.child_by_field_name("parameters") {
        Some(n) => n,
        None => return Vec::new(),
    };

    let mut params = Vec::new();
    for i in 0..params_node.child_count() {
        let child = params_node.child(i).unwrap();
        if child.kind() == "parameter_declaration" {
            // Go: `name type` or `name1, name2 type`
            let type_node = child.child_by_field_name("type");
            let type_name = type_node.map(|t| node_text(t, src).to_string());

            // Names can be multiple identifiers
            for j in 0..child.child_count() {
                let name_child = child.child(j).unwrap();
                if name_child.kind() == "identifier" {
                    let name = node_text(name_child, src);
                    if !name.is_empty() {
                        params.push(TsParam {
                            name: name.to_string(),
                            type_name: type_name.clone(),
                        });
                    }
                }
            }
        } else if child.kind() == "variadic_parameter_declaration" {
            let type_node = child.child_by_field_name("type");
            let type_name = type_node.map(|t| format!("...{}", node_text(t, src)));
            if let Some(name_node) = child.child_by_field_name("name") {
                let name = node_text(name_node, src);
                if !name.is_empty() {
                    params.push(TsParam {
                        name: name.to_string(),
                        type_name,
                    });
                }
            }
        }
    }
    params
}

// ---------------------------------------------------------------------------
// Return type extraction
// ---------------------------------------------------------------------------

fn extract_return_type(node: &Node, src: &[u8]) -> Option<String> {
    node.child_by_field_name("result")
        .map(|r| {
            let text = node_text(r, src);
            text.trim().to_string()
        })
        .filter(|s| !s.is_empty())
}

// ---------------------------------------------------------------------------
// Doc comment extraction
// ---------------------------------------------------------------------------

fn collect_doc_comment(node: &Node, src: &[u8]) -> Option<String> {
    let mut lines = Vec::new();
    let mut sibling = node.prev_sibling();
    while let Some(sib) = sibling {
        if sib.kind() == "comment" {
            let text = node_text(sib, src);
            let trimmed = text.trim();
            if let Some(doc) = trimmed.strip_prefix("//") {
                lines.push(doc.strip_prefix(' ').unwrap_or(doc).to_string());
                sibling = sib.prev_sibling();
            } else {
                break;
            }
        } else {
            break;
        }
    }
    if lines.is_empty() {
        return None;
    }
    lines.reverse();
    Some(lines.join("\n"))
}

// ---------------------------------------------------------------------------
// Signature building
// ---------------------------------------------------------------------------

fn build_signature(
    name: &str,
    params: &[TsParam],
    return_type: Option<&str>,
    _is_async: bool,
) -> String {
    let mut sig = String::from("func ");
    sig.push_str(name);
    sig.push('(');
    let param_strs: Vec<String> = params
        .iter()
        .map(|p| {
            if let Some(ref t) = p.type_name {
                format!("{} {}", p.name, t)
            } else {
                p.name.clone()
            }
        })
        .collect();
    sig.push_str(&param_strs.join(", "));
    sig.push(')');
    if let Some(rt) = return_type {
        sig.push(' ');
        sig.push_str(rt);
    }
    sig
}

// ---------------------------------------------------------------------------
// Utility
// ---------------------------------------------------------------------------

fn node_text<'a>(node: Node, src: &'a [u8]) -> &'a str {
    node.utf8_text(src).unwrap_or("")
}

#[cfg(test)]
mod tests {

    use crate::extract_symbols;
    use codryn_discover::Language;

    #[test]
    fn go_function_extraction() {
        let src = r#"
// Add adds two integers.
func Add(a int, b int) int {
    return a + b
}
"#;
        let syms = extract_symbols(Language::Go, src).unwrap();
        let f = syms
            .iter()
            .find(|s| s.name == "Add")
            .expect("Add not found");
        assert_eq!(f.label, "Function");
        assert!(f.is_exported);
        assert_eq!(f.parameters.len(), 2);
        assert_eq!(f.return_type.as_deref(), Some("int"));
        assert!(f.docstring.as_ref().unwrap().contains("Add adds"));
    }

    #[test]
    fn go_method_extraction() {
        let src = r#"
type Handler struct{}

func (h *Handler) ServeHTTP(w http.ResponseWriter, r *http.Request) {
}
"#;
        let syms = extract_symbols(Language::Go, src).unwrap();
        let m = syms
            .iter()
            .find(|s| s.name == "ServeHTTP")
            .expect("ServeHTTP not found");
        assert_eq!(m.label, "Method");
        assert_eq!(m.parent_name.as_deref(), Some("Handler"));
    }

    #[test]
    fn go_struct_extraction() {
        let src = r#"
// User represents a user.
type User struct {
    Name string
    Age  int
}
"#;
        let syms = extract_symbols(Language::Go, src).unwrap();
        let s = syms
            .iter()
            .find(|s| s.name == "User" && s.label == "Class")
            .expect("User struct not found");
        assert!(s.is_exported);
    }

    #[test]
    fn go_interface_extraction() {
        let src = r#"
type Writer interface {
    Write(p []byte) (n int, err error)
}
"#;
        let syms = extract_symbols(Language::Go, src).unwrap();
        let iface = syms
            .iter()
            .find(|s| s.name == "Writer" && s.label == "Interface")
            .expect("Writer interface not found");
        assert!(iface.is_abstract);
    }

    #[test]
    fn go_unexported_function_not_exported() {
        let src = r#"
func helper() string {
    return "help"
}
"#;
        let syms = extract_symbols(Language::Go, src).unwrap();
        let f = syms
            .iter()
            .find(|s| s.name == "helper")
            .expect("helper not found");
        assert!(!f.is_exported);
    }

    #[test]
    fn go_test_function_detected() {
        let src = r#"
func TestAdd(t *testing.T) {
    // test
}
"#;
        let syms = extract_symbols(Language::Go, src).unwrap();
        let f = syms
            .iter()
            .find(|s| s.name == "TestAdd")
            .expect("TestAdd not found");
        assert!(f.is_test);
    }

    #[test]
    fn go_main_is_entry_point() {
        let src = r#"
func main() {
    // entry
}
"#;
        let syms = extract_symbols(Language::Go, src).unwrap();
        let f = syms
            .iter()
            .find(|s| s.name == "main")
            .expect("main not found");
        assert!(f.is_entry_point);
    }
}
