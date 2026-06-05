//! PHP AST walker.
//!
//! Extracts: function definitions, class declarations, interface declarations,
//! trait declarations, method declarations, enum declarations (PHP 8.1+).
//! Handles: visibility modifiers, abstract, static, return types, doc comments.

use crate::{TsParam, TsSymbol};
use tree_sitter::{Node, TreeCursor};

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

/// Walk a PHP tree-sitter AST and extract symbols.
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
            "class_declaration" => {
                let doc = collect_preceding_comments(node, source);
                if let Some(mut sym) = extract_class(node, source) {
                    sym.docstring = doc.or(sym.docstring);
                    let name = sym.name.clone();
                    symbols.push(sym);
                    visit_children(cursor, source, symbols, Some(&name));
                }
            }
            "interface_declaration" => {
                let doc = collect_preceding_comments(node, source);
                if let Some(mut sym) = extract_interface(node, source) {
                    sym.docstring = doc.or(sym.docstring);
                    let name = sym.name.clone();
                    symbols.push(sym);
                    visit_children(cursor, source, symbols, Some(&name));
                }
            }
            "trait_declaration" => {
                let doc = collect_preceding_comments(node, source);
                if let Some(mut sym) = extract_trait(node, source) {
                    sym.docstring = doc.or(sym.docstring);
                    let name = sym.name.clone();
                    symbols.push(sym);
                    visit_children(cursor, source, symbols, Some(&name));
                }
            }
            "enum_declaration" => {
                let doc = collect_preceding_comments(node, source);
                if let Some(mut sym) = extract_enum(node, source) {
                    sym.docstring = doc.or(sym.docstring);
                    symbols.push(sym);
                }
            }
            "method_declaration" => {
                let doc = collect_preceding_comments(node, source);
                if let Some(mut sym) = extract_method(node, source, parent_name) {
                    sym.docstring = doc.or(sym.docstring);
                    symbols.push(sym);
                }
                visit_children(cursor, source, symbols, parent_name);
            }
            "namespace_definition" => {
                let doc = collect_preceding_comments(node, source);
                if let Some(mut sym) = extract_namespace(node, source) {
                    sym.docstring = doc.or(sym.docstring);
                    let ns_name = sym.name.clone();
                    symbols.push(sym);
                    visit_children(cursor, source, symbols, Some(&ns_name));
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
    let signature = build_php_signature(None, return_type.as_deref(), &name, &params, false, false);

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
// Method extraction
// ---------------------------------------------------------------------------

fn extract_method(node: Node, source: &str, parent_name: Option<&str>) -> Option<TsSymbol> {
    let name_node = node.child_by_field_name("name")?;
    let name = node_text(name_node, source);
    if name.is_empty() {
        return None;
    }

    let visibility = get_visibility(node, source);
    let is_abstract = has_modifier_text(node, source, "abstract");
    let is_static = has_modifier_text(node, source, "static");
    let is_exported = visibility.as_deref() == Some("public");

    let params = extract_parameters(node, source);
    let return_type = extract_return_type(node, source);
    let body = node.child_by_field_name("body");
    let body_text = body.map(|b| node_text(b, source));

    let signature = build_php_signature(
        visibility.as_deref(),
        return_type.as_deref(),
        &name,
        &params,
        is_abstract,
        is_static,
    );

    Some(TsSymbol {
        name,
        label: "Method".into(),
        start_line: node.start_position().row as i32 + 1,
        end_line: node.end_position().row as i32 + 1,
        parent_name: parent_name.map(String::from),
        signature: Some(signature),
        return_type,
        parameters: params,
        docstring: None,
        decorators: Vec::new(),
        base_classes: Vec::new(),
        is_exported,
        is_abstract,
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

    let is_abstract = has_modifier_text(node, source, "abstract");
    let base_classes = extract_base_clause(node, source);
    let interfaces = extract_implements_clause(node, source);
    let mut all_bases = base_classes;
    all_bases.extend(interfaces);

    let body = node.child_by_field_name("body");
    let body_text = body.map(|b| node_text(b, source));

    let mut sig = String::new();
    if is_abstract {
        sig.push_str("abstract ");
    }
    sig.push_str("class ");
    sig.push_str(&name);
    if !all_bases.is_empty() {
        sig.push_str(" extends/implements ");
        sig.push_str(&all_bases.join(", "));
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
        base_classes: all_bases,
        is_exported: false,
        is_abstract,
        is_async: false,
        is_test: false,
        is_entry_point: false,
        body_text,
    })
}

// ---------------------------------------------------------------------------
// Interface extraction
// ---------------------------------------------------------------------------

fn extract_interface(node: Node, source: &str) -> Option<TsSymbol> {
    let name_node = node.child_by_field_name("name")?;
    let name = node_text(name_node, source);
    if name.is_empty() {
        return None;
    }

    let base_classes = extract_base_clause(node, source);
    let body = node.child_by_field_name("body");
    let body_text = body.map(|b| node_text(b, source));

    let mut sig = format!("interface {}", name);
    if !base_classes.is_empty() {
        sig.push_str(" extends ");
        sig.push_str(&base_classes.join(", "));
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
// Trait extraction
// ---------------------------------------------------------------------------

fn extract_trait(node: Node, source: &str) -> Option<TsSymbol> {
    let name_node = node.child_by_field_name("name")?;
    let name = node_text(name_node, source);
    if name.is_empty() {
        return None;
    }

    let body = node.child_by_field_name("body");
    let body_text = body.map(|b| node_text(b, source));

    Some(TsSymbol {
        name: name.clone(),
        label: "Interface".into(), // Traits map to Interface label
        start_line: node.start_position().row as i32 + 1,
        end_line: node.end_position().row as i32 + 1,
        parent_name: None,
        signature: Some(format!("trait {}", name)),
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
// Enum extraction
// ---------------------------------------------------------------------------

fn extract_enum(node: Node, source: &str) -> Option<TsSymbol> {
    let name_node = node.child_by_field_name("name")?;
    let name = node_text(name_node, source);
    if name.is_empty() {
        return None;
    }

    let body = node.child_by_field_name("body");
    let body_text = body.map(|b| node_text(b, source));

    Some(TsSymbol {
        name: name.clone(),
        label: "Enum".into(),
        start_line: node.start_position().row as i32 + 1,
        end_line: node.end_position().row as i32 + 1,
        parent_name: None,
        signature: Some(format!("enum {}", name)),
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
// Namespace extraction
// ---------------------------------------------------------------------------

fn extract_namespace(node: Node, source: &str) -> Option<TsSymbol> {
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
        signature: Some(format!("namespace {}", name)),
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
    let params_node = match node.child_by_field_name("parameters") {
        Some(n) => n,
        None => return Vec::new(),
    };

    let mut params = Vec::new();
    for child in params_node.named_children(&mut params_node.walk()) {
        if child.kind() == "simple_parameter" || child.kind() == "property_promotion_parameter" {
            let ptype = child
                .child_by_field_name("type")
                .map(|t| node_text(t, source));
            let pname = child
                .child_by_field_name("name")
                .map(|n| node_text(n, source))
                .unwrap_or_default();
            if !pname.is_empty() {
                params.push(TsParam {
                    name: pname,
                    type_name: ptype,
                });
            }
        } else if child.kind() == "variadic_parameter" {
            let pname = child
                .child_by_field_name("name")
                .map(|n| format!("...{}", node_text(n, source)))
                .unwrap_or_else(|| "...".into());
            let ptype = child
                .child_by_field_name("type")
                .map(|t| node_text(t, source));
            params.push(TsParam {
                name: pname,
                type_name: ptype,
            });
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
// Base class / implements extraction
// ---------------------------------------------------------------------------

fn extract_base_clause(node: Node, source: &str) -> Vec<String> {
    let mut bases = Vec::new();
    for child in node.children(&mut node.walk()) {
        if child.kind() == "base_clause" {
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

fn extract_implements_clause(node: Node, source: &str) -> Vec<String> {
    let mut ifaces = Vec::new();
    for child in node.children(&mut node.walk()) {
        if child.kind() == "class_interface_clause" {
            for inner in child.named_children(&mut child.walk()) {
                let text = node_text(inner, source).trim().to_string();
                if !text.is_empty() {
                    ifaces.push(text);
                }
            }
        }
    }
    ifaces
}

// ---------------------------------------------------------------------------
// Modifier helpers
// ---------------------------------------------------------------------------

fn get_visibility(node: Node, source: &str) -> Option<String> {
    for child in node.children(&mut node.walk()) {
        let text = node_text(child, source);
        let t = text.trim();
        if t == "public" || t == "private" || t == "protected" {
            return Some(t.to_string());
        }
        if child.kind() == "visibility_modifier" {
            return Some(t.to_string());
        }
    }
    None
}

fn has_modifier_text(node: Node, source: &str, keyword: &str) -> bool {
    for child in node.children(&mut node.walk()) {
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
        if sib.kind() == "comment" {
            let text = node_text(sib, source);
            let trimmed = text.trim();
            if trimmed.starts_with("/**") {
                // PHPDoc block comment
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
            } else if let Some(s) = trimmed.strip_prefix('#') {
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

fn build_php_signature(
    visibility: Option<&str>,
    return_type: Option<&str>,
    name: &str,
    params: &[TsParam],
    is_abstract: bool,
    is_static: bool,
) -> String {
    let mut sig = String::new();
    if let Some(vis) = visibility {
        sig.push_str(vis);
        sig.push(' ');
    }
    if is_abstract {
        sig.push_str("abstract ");
    }
    if is_static {
        sig.push_str("static ");
    }
    sig.push_str("function ");
    sig.push_str(name);
    sig.push('(');
    let param_strs: Vec<String> = params
        .iter()
        .map(|p| {
            if let Some(ref t) = p.type_name {
                format!("{} {}", t, p.name)
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
