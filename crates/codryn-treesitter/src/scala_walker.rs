//! Scala AST walker.
//!
//! Extracts: function definitions, class definitions, object definitions,
//! trait definitions, val/var definitions (when they are function-like).
//! Handles: access modifiers, case classes, abstract, sealed, extends/with.

use crate::{TsParam, TsSymbol};
use tree_sitter::{Node, TreeCursor};

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

/// Walk a Scala tree-sitter AST and extract symbols.
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
            "function_definition" => {
                let doc = collect_preceding_comments(node, source);
                if let Some(mut sym) = extract_function(node, source, parent_name) {
                    sym.docstring = doc.or(sym.docstring);
                    symbols.push(sym);
                }
                visit_children(cursor, source, symbols, parent_name);
            }
            "class_definition" => {
                let doc = collect_preceding_comments(node, source);
                if let Some(mut sym) = extract_class(node, source) {
                    sym.docstring = doc.or(sym.docstring);
                    let name = sym.name.clone();
                    symbols.push(sym);
                    visit_children(cursor, source, symbols, Some(&name));
                }
            }
            "object_definition" => {
                let doc = collect_preceding_comments(node, source);
                if let Some(mut sym) = extract_object(node, source) {
                    sym.docstring = doc.or(sym.docstring);
                    let name = sym.name.clone();
                    symbols.push(sym);
                    visit_children(cursor, source, symbols, Some(&name));
                }
            }
            "trait_definition" => {
                let doc = collect_preceding_comments(node, source);
                if let Some(mut sym) = extract_trait(node, source) {
                    sym.docstring = doc.or(sym.docstring);
                    let name = sym.name.clone();
                    symbols.push(sym);
                    visit_children(cursor, source, symbols, Some(&name));
                }
            }
            "package_clause" => {
                if let Some(sym) = extract_package(node, source) {
                    symbols.push(sym);
                }
                visit_children(cursor, source, symbols, parent_name);
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
// Function extraction
// ---------------------------------------------------------------------------

fn extract_function(node: Node, source: &str, parent_name: Option<&str>) -> Option<TsSymbol> {
    let name_node = node.child_by_field_name("name")?;
    let name = node_text(name_node, source);
    if name.is_empty() {
        return None;
    }

    let params = extract_parameters(node, source);
    let return_type = extract_return_type(node, source);
    let body = node.child_by_field_name("body");
    let body_text = body.map(|b| node_text(b, source));

    let label = if parent_name.is_some() {
        "Method"
    } else {
        "Function"
    };
    let signature = build_scala_signature(&name, &params, return_type.as_deref());

    Some(TsSymbol {
        name,
        label: label.into(),
        start_line: node.start_position().row as i32 + 1,
        end_line: node.end_position().row as i32 + 1,
        parent_name: parent_name.map(String::from),
        signature: Some(signature),
        return_type,
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
// Class extraction
// ---------------------------------------------------------------------------

fn extract_class(node: Node, source: &str) -> Option<TsSymbol> {
    let name_node = node.child_by_field_name("name")?;
    let name = node_text(name_node, source);
    if name.is_empty() {
        return None;
    }

    let is_abstract = has_modifier(node, source, "abstract");
    let is_case = has_modifier(node, source, "case");
    let base_classes = extract_extends_clause(node, source);
    let body = node.child_by_field_name("body");
    let body_text = body.map(|b| node_text(b, source));

    let mut sig = String::new();
    if is_abstract {
        sig.push_str("abstract ");
    }
    if is_case {
        sig.push_str("case ");
    }
    sig.push_str("class ");
    sig.push_str(&name);
    if !base_classes.is_empty() {
        sig.push_str(" extends ");
        sig.push_str(&base_classes.join(" with "));
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
        is_abstract,
        is_async: false,
        is_test: false,
        is_entry_point: false,
        body_text,
    })
}

// ---------------------------------------------------------------------------
// Object extraction (Scala singleton)
// ---------------------------------------------------------------------------

fn extract_object(node: Node, source: &str) -> Option<TsSymbol> {
    let name_node = node.child_by_field_name("name")?;
    let name = node_text(name_node, source);
    if name.is_empty() {
        return None;
    }

    let base_classes = extract_extends_clause(node, source);
    let body = node.child_by_field_name("body");
    let body_text = body.map(|b| node_text(b, source));

    let mut sig = format!("object {}", name);
    if !base_classes.is_empty() {
        sig.push_str(" extends ");
        sig.push_str(&base_classes.join(" with "));
    }

    Some(TsSymbol {
        name,
        label: "Class".into(), // Objects are singleton classes
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
// Trait extraction
// ---------------------------------------------------------------------------

fn extract_trait(node: Node, source: &str) -> Option<TsSymbol> {
    let name_node = node.child_by_field_name("name")?;
    let name = node_text(name_node, source);
    if name.is_empty() {
        return None;
    }

    let is_sealed = has_modifier(node, source, "sealed");
    let base_classes = extract_extends_clause(node, source);
    let body = node.child_by_field_name("body");
    let body_text = body.map(|b| node_text(b, source));

    let mut sig = String::new();
    if is_sealed {
        sig.push_str("sealed ");
    }
    sig.push_str("trait ");
    sig.push_str(&name);
    if !base_classes.is_empty() {
        sig.push_str(" extends ");
        sig.push_str(&base_classes.join(" with "));
    }

    Some(TsSymbol {
        name,
        label: "Interface".into(),
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
        is_abstract: true,
        is_async: false,
        is_test: false,
        is_entry_point: false,
        body_text,
    })
}

// ---------------------------------------------------------------------------
// Package extraction
// ---------------------------------------------------------------------------

fn extract_package(node: Node, source: &str) -> Option<TsSymbol> {
    let name_node = node.child_by_field_name("name")?;
    let name = node_text(name_node, source);
    if name.is_empty() {
        return None;
    }

    Some(TsSymbol {
        name: name.clone(),
        label: "Module".into(),
        start_line: node.start_position().row as i32 + 1,
        end_line: node.end_position().row as i32 + 1,
        parent_name: None,
        signature: Some(format!("package {}", name)),
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
        body_text: None,
    })
}

// ---------------------------------------------------------------------------
// Parameter extraction
// ---------------------------------------------------------------------------

fn extract_parameters(node: Node, source: &str) -> Vec<TsParam> {
    let mut params = Vec::new();
    // Scala functions have class_parameters or parameters children
    for child in node.named_children(&mut node.walk()) {
        if child.kind() == "parameters" || child.kind() == "class_parameters" {
            for param in child.named_children(&mut child.walk()) {
                if param.kind() == "parameter" || param.kind() == "class_parameter" {
                    let pname = param
                        .child_by_field_name("name")
                        .map(|n| node_text(n, source))
                        .unwrap_or_default();
                    let ptype = param
                        .child_by_field_name("type")
                        .map(|t| node_text(t, source));
                    if !pname.is_empty() {
                        params.push(TsParam {
                            name: pname,
                            type_name: ptype,
                        });
                    }
                }
            }
        }
    }
    params
}

// ---------------------------------------------------------------------------
// Return type extraction
// ---------------------------------------------------------------------------

fn extract_return_type(node: Node, source: &str) -> Option<String> {
    node.child_by_field_name("return_type")
        .map(|rt| {
            let text = node_text(rt, source);
            let trimmed = text.trim();
            trimmed
                .strip_prefix(':')
                .unwrap_or(trimmed)
                .trim()
                .to_string()
        })
        .filter(|s| !s.is_empty())
}

// ---------------------------------------------------------------------------
// Extends clause extraction
// ---------------------------------------------------------------------------

fn extract_extends_clause(node: Node, source: &str) -> Vec<String> {
    let mut bases = Vec::new();
    for child in node.children(&mut node.walk()) {
        if child.kind() == "extends_clause" {
            for inner in child.named_children(&mut child.walk()) {
                let text = node_text(inner, source).trim().to_string();
                if !text.is_empty() {
                    bases.push(text);
                }
            }
        }
    }
    bases
}

// ---------------------------------------------------------------------------
// Modifier helpers
// ---------------------------------------------------------------------------

fn has_modifier(node: Node, source: &str, keyword: &str) -> bool {
    for child in node.children(&mut node.walk()) {
        if (child.kind() == "modifiers" || child.kind() == "annotation")
            && node_text(child, source).contains(keyword)
        {
            return true;
        }
        if node_text(child, source).trim() == keyword {
            return true;
        }
    }
    false
}

// ---------------------------------------------------------------------------
// Comment extraction
// ---------------------------------------------------------------------------

fn collect_preceding_comments(node: Node, source: &str) -> Option<String> {
    let mut lines = Vec::new();
    let mut sibling = node.prev_sibling();
    while let Some(sib) = sibling {
        if sib.kind() == "comment" || sib.kind() == "block_comment" {
            let text = node_text(sib, source);
            let trimmed = text.trim();
            if trimmed.starts_with("/**") {
                let cleaned = trimmed
                    .strip_prefix("/**")
                    .unwrap_or(trimmed)
                    .strip_suffix("*/")
                    .unwrap_or(trimmed)
                    .trim()
                    .to_string();
                lines.push(cleaned);
            } else if let Some(s) = trimmed.strip_prefix("//") {
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

fn build_scala_signature(name: &str, params: &[TsParam], return_type: Option<&str>) -> String {
    let mut sig = String::from("def ");
    sig.push_str(name);
    sig.push('(');
    let param_strs: Vec<String> = params
        .iter()
        .map(|p| {
            if let Some(ref t) = p.type_name {
                format!("{}: {}", p.name, t)
            } else {
                p.name.clone()
            }
        })
        .collect();
    sig.push_str(&param_strs.join(", "));
    sig.push(')');
    if let Some(rt) = return_type {
        sig.push_str(": ");
        sig.push_str(rt);
    }
    sig
}

// ---------------------------------------------------------------------------
// Utility
// ---------------------------------------------------------------------------

fn node_text(node: Node, source: &str) -> String {
    source.get(node.byte_range()).unwrap_or("").to_string()
}
