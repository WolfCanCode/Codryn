//! Ruby AST walker.
//!
//! Extracts: method definitions, class definitions, module definitions,
//! singleton methods (self.method).
//! Handles: nested classes/modules, access modifiers inferred from context.

use crate::{TsParam, TsSymbol};
use tree_sitter::{Node, TreeCursor};

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

/// Walk a Ruby tree-sitter AST and extract symbols.
pub fn walk_tree(tree: &tree_sitter::Tree, source: &str) -> Vec<TsSymbol> {
    let mut symbols = Vec::new();
    let mut cursor = tree.walk();
    visit_children(&mut cursor, source, &mut symbols, None);
    symbols
}

// ---------------------------------------------------------------------------
// Recursive visitor
// ---------------------------------------------------------------------------

fn visit_children(
    cursor: &mut TreeCursor,
    source: &str,
    symbols: &mut Vec<TsSymbol>,
    parent_name: Option<&str>,
) {
    if !cursor.goto_first_child() {
        return;
    }
    loop {
        let node = cursor.node();
        let kind = node.kind();

        match kind {
            "method" => {
                let doc = collect_preceding_comments(node, source);
                if let Some(mut sym) = extract_method(node, source, parent_name) {
                    sym.docstring = doc.or(sym.docstring);
                    symbols.push(sym);
                }
                visit_children(cursor, source, symbols, parent_name);
            }
            "singleton_method" => {
                let doc = collect_preceding_comments(node, source);
                if let Some(mut sym) = extract_singleton_method(node, source, parent_name) {
                    sym.docstring = doc.or(sym.docstring);
                    symbols.push(sym);
                }
                visit_children(cursor, source, symbols, parent_name);
            }
            "class" => {
                let doc = collect_preceding_comments(node, source);
                if let Some(mut sym) = extract_class(node, source) {
                    sym.docstring = doc.or(sym.docstring);
                    let name = sym.name.clone();
                    symbols.push(sym);
                    visit_children(cursor, source, symbols, Some(&name));
                }
            }
            "module" => {
                let doc = collect_preceding_comments(node, source);
                if let Some(mut sym) = extract_module(node, source) {
                    sym.docstring = doc.or(sym.docstring);
                    let name = sym.name.clone();
                    symbols.push(sym);
                    visit_children(cursor, source, symbols, Some(&name));
                }
            }
            _ => {
                visit_children(cursor, source, symbols, parent_name);
            }
        }

        if !cursor.goto_next_sibling() {
            break;
        }
    }
    cursor.goto_parent();
}

// ---------------------------------------------------------------------------
// Method extraction
// ---------------------------------------------------------------------------

fn extract_method(node: Node, source: &str, parent_name: Option<&str>) -> Option<TsSymbol> {
    let name_node = node.child_by_field_name("name")?;
    let name = node_text(name_node, source);
    if name.is_empty() {
        return None;
    }

    let params = extract_parameters(node, source);
    let body = node.child_by_field_name("body");
    let body_text = body.map(|b| node_text(b, source));

    let label = if parent_name.is_some() {
        "Method"
    } else {
        "Function"
    };
    let signature = build_ruby_signature(&name, &params);

    Some(TsSymbol {
        name,
        label: label.into(),
        start_line: node.start_position().row as i32 + 1,
        end_line: node.end_position().row as i32 + 1,
        parent_name: parent_name.map(String::from),
        signature: Some(signature),
        return_type: None,
        parameters: params,
        docstring: None,
        decorators: Vec::new(),
        base_classes: Vec::new(),
        is_exported: false,
        is_abstract: false,
        is_async: false,
        is_test: false,
        is_entry_point: false,
        body_text,
    })
}

// ---------------------------------------------------------------------------
// Singleton method extraction (self.method)
// ---------------------------------------------------------------------------

fn extract_singleton_method(
    node: Node,
    source: &str,
    parent_name: Option<&str>,
) -> Option<TsSymbol> {
    let name_node = node.child_by_field_name("name")?;
    let name = node_text(name_node, source);
    if name.is_empty() {
        return None;
    }

    let params = extract_parameters(node, source);
    let body = node.child_by_field_name("body");
    let body_text = body.map(|b| node_text(b, source));

    let label = if parent_name.is_some() {
        "Method"
    } else {
        "Function"
    };
    let sig_name = format!("self.{}", name);
    let signature = build_ruby_signature(&sig_name, &params);

    Some(TsSymbol {
        name,
        label: label.into(),
        start_line: node.start_position().row as i32 + 1,
        end_line: node.end_position().row as i32 + 1,
        parent_name: parent_name.map(String::from),
        signature: Some(signature),
        return_type: None,
        parameters: params,
        docstring: None,
        decorators: Vec::new(),
        base_classes: Vec::new(),
        is_exported: true, // singleton methods are class-level (public)
        is_abstract: false,
        is_async: false,
        is_test: false,
        is_entry_point: false,
        body_text,
    })
}

// ---------------------------------------------------------------------------
// Class extraction
// ---------------------------------------------------------------------------

fn extract_class(node: Node, source: &str) -> Option<TsSymbol> {
    let name_node = node.child_by_field_name("name")?;
    let name = node_text(name_node, source);
    if name.is_empty() {
        return None;
    }

    let base_classes = node
        .child_by_field_name("superclass")
        .map(|sc| vec![node_text(sc, source)])
        .unwrap_or_default();

    let body = node.child_by_field_name("body");
    let body_text = body.map(|b| node_text(b, source));

    let mut sig = format!("class {}", name);
    if let Some(base) = base_classes.first() {
        sig.push_str(" < ");
        sig.push_str(base);
    }

    Some(TsSymbol {
        name,
        label: "Class".into(),
        start_line: node.start_position().row as i32 + 1,
        end_line: node.end_position().row as i32 + 1,
        parent_name: None,
        signature: Some(sig),
        return_type: None,
        parameters: Vec::new(),
        docstring: None,
        decorators: Vec::new(),
        base_classes,
        is_exported: false,
        is_abstract: false,
        is_async: false,
        is_test: false,
        is_entry_point: false,
        body_text,
    })
}

// ---------------------------------------------------------------------------
// Module extraction
// ---------------------------------------------------------------------------

fn extract_module(node: Node, source: &str) -> Option<TsSymbol> {
    let name_node = node.child_by_field_name("name")?;
    let name = node_text(name_node, source);
    if name.is_empty() {
        return None;
    }

    let body = node.child_by_field_name("body");
    let body_text = body.map(|b| node_text(b, source));

    Some(TsSymbol {
        name: name.clone(),
        label: "Module".into(),
        start_line: node.start_position().row as i32 + 1,
        end_line: node.end_position().row as i32 + 1,
        parent_name: None,
        signature: Some(format!("module {}", name)),
        return_type: None,
        parameters: Vec::new(),
        docstring: None,
        decorators: Vec::new(),
        base_classes: Vec::new(),
        is_exported: false,
        is_abstract: false,
        is_async: false,
        is_test: false,
        is_entry_point: false,
        body_text,
    })
}

// ---------------------------------------------------------------------------
// Parameter extraction
// ---------------------------------------------------------------------------

fn extract_parameters(node: Node, source: &str) -> Vec<TsParam> {
    let params_node = match node.child_by_field_name("parameters") {
        Some(n) => n,
        None => return Vec::new(),
    };

    let mut params = Vec::new();
    for child in params_node.named_children(&mut params_node.walk()) {
        match child.kind() {
            "identifier" => {
                let pname = node_text(child, source);
                if !pname.is_empty() {
                    params.push(TsParam {
                        name: pname,
                        type_name: None,
                    });
                }
            }
            "optional_parameter" => {
                let pname = child
                    .child_by_field_name("name")
                    .map(|n| node_text(n, source))
                    .unwrap_or_default();
                if !pname.is_empty() {
                    params.push(TsParam {
                        name: pname,
                        type_name: None,
                    });
                }
            }
            "splat_parameter" => {
                let pname = child
                    .child_by_field_name("name")
                    .map(|n| format!("*{}", node_text(n, source)))
                    .unwrap_or_else(|| "*".into());
                params.push(TsParam {
                    name: pname,
                    type_name: None,
                });
            }
            "hash_splat_parameter" => {
                let pname = child
                    .child_by_field_name("name")
                    .map(|n| format!("**{}", node_text(n, source)))
                    .unwrap_or_else(|| "**".into());
                params.push(TsParam {
                    name: pname,
                    type_name: None,
                });
            }
            "block_parameter" => {
                let pname = child
                    .child_by_field_name("name")
                    .map(|n| format!("&{}", node_text(n, source)))
                    .unwrap_or_else(|| "&".into());
                params.push(TsParam {
                    name: pname,
                    type_name: None,
                });
            }
            "keyword_parameter" => {
                let pname = child
                    .child_by_field_name("name")
                    .map(|n| format!("{}:", node_text(n, source)))
                    .unwrap_or_default();
                if !pname.is_empty() {
                    params.push(TsParam {
                        name: pname,
                        type_name: None,
                    });
                }
            }
            _ => {}
        }
    }
    params
}

// ---------------------------------------------------------------------------
// Comment extraction
// ---------------------------------------------------------------------------

fn collect_preceding_comments(node: Node, source: &str) -> Option<String> {
    let mut lines = Vec::new();
    let mut sibling = node.prev_sibling();
    while let Some(sib) = sibling {
        if sib.kind() == "comment" {
            let text = node_text(sib, source);
            let trimmed = text.trim();
            if let Some(s) = trimmed.strip_prefix('#') {
                lines.push(s.strip_prefix(' ').unwrap_or(s).to_string());
            }
            sibling = sib.prev_sibling();
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

fn build_ruby_signature(name: &str, params: &[TsParam]) -> String {
    let mut sig = String::from("def ");
    sig.push_str(name);
    if !params.is_empty() {
        sig.push('(');
        let param_strs: Vec<&str> = params.iter().map(|p| p.name.as_str()).collect();
        sig.push_str(&param_strs.join(", "));
        sig.push(')');
    }
    sig
}

// ---------------------------------------------------------------------------
// Utility
// ---------------------------------------------------------------------------

fn node_text(node: Node, source: &str) -> String {
    source.get(node.byte_range()).unwrap_or("").to_string()
}
