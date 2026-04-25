//! Swift AST walker.
//!
//! Extracts: function declarations, class declarations, struct declarations,
//! protocol declarations, enum declarations, extension declarations.
//! Handles: access modifiers, async/throws, generics, protocol conformance.

use crate::{TsParam, TsSymbol};
use tree_sitter::{Node, TreeCursor};

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

/// Walk a Swift tree-sitter AST and extract symbols.
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
            "function_declaration" => {
                let doc = collect_preceding_comments(node, source);
                if let Some(mut sym) = extract_function(node, source, parent_name) {
                    sym.docstring = doc.or(sym.docstring);
                    symbols.push(sym);
                }
                visit_children(cursor, source, symbols, parent_name);
            }
            "class_declaration" => {
                let doc = collect_preceding_comments(node, source);
                if let Some(mut sym) = extract_class_like(node, source, "class") {
                    sym.docstring = doc.or(sym.docstring);
                    let name = sym.name.clone();
                    symbols.push(sym);
                    visit_children(cursor, source, symbols, Some(&name));
                }
            }
            "struct_declaration" => {
                // tree-sitter-swift doesn't have struct_declaration; structs may appear
                // as class_declaration with "struct" keyword. Handle if present.
                let doc = collect_preceding_comments(node, source);
                if let Some(mut sym) = extract_class_like(node, source, "struct") {
                    sym.docstring = doc.or(sym.docstring);
                    let name = sym.name.clone();
                    symbols.push(sym);
                    visit_children(cursor, source, symbols, Some(&name));
                }
            }
            "protocol_declaration" => {
                let doc = collect_preceding_comments(node, source);
                if let Some(mut sym) = extract_protocol(node, source) {
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
                    let name = sym.name.clone();
                    symbols.push(sym);
                    visit_children(cursor, source, symbols, Some(&name));
                }
            }
            "extension_declaration" => {
                // Extensions add methods to existing types
                let ext_name = find_child_name(node, source);
                if let Some(ref n) = ext_name {
                    visit_children(cursor, source, symbols, Some(n));
                } else {
                    visit_children(cursor, source, symbols, parent_name);
                }
            }
            "init_declaration" => {
                let doc = collect_preceding_comments(node, source);
                if let Some(mut sym) = extract_init(node, source, parent_name) {
                    sym.docstring = doc.or(sym.docstring);
                    symbols.push(sym);
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

    let is_exported =
        has_modifier_keyword(node, source, "public") || has_modifier_keyword(node, source, "open");
    let is_async = node_text(node, source).contains(" async ");
    let params = extract_parameters(node, source);
    let return_type = extract_return_type(node, source);
    let body = node.child_by_field_name("body");
    let body_text = body.map(|b| node_text(b, source));

    let label = if parent_name.is_some() {
        "Method"
    } else {
        "Function"
    };
    let signature = build_swift_signature(&name, &params, return_type.as_deref(), is_async);

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
        is_exported,
        is_abstract: false,
        is_async,
        is_test: false,
        is_entry_point: false,
        body_text,
    })
}

// ---------------------------------------------------------------------------
// Init (constructor) extraction
// ---------------------------------------------------------------------------

fn extract_init(node: Node, source: &str, parent_name: Option<&str>) -> Option<TsSymbol> {
    let params = extract_parameters(node, source);
    let body = node.child_by_field_name("body");
    let body_text = body.map(|b| node_text(b, source));

    let signature = build_swift_signature("init", &params, None, false);

    Some(TsSymbol {
        name: "init".into(),
        label: "Method".into(),
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
// Class / struct extraction
// ---------------------------------------------------------------------------

fn extract_class_like(node: Node, source: &str, keyword: &str) -> Option<TsSymbol> {
    let name_node = node.child_by_field_name("name")?;
    let name = node_text(name_node, source);
    if name.is_empty() {
        return None;
    }

    let is_exported =
        has_modifier_keyword(node, source, "public") || has_modifier_keyword(node, source, "open");
    let base_classes = extract_inheritance_clause(node, source);
    let body = node.child_by_field_name("body");
    let body_text = body.map(|b| node_text(b, source));

    let mut sig = String::new();
    if is_exported {
        sig.push_str("public ");
    }
    sig.push_str(keyword);
    sig.push(' ');
    sig.push_str(&name);
    if !base_classes.is_empty() {
        sig.push_str(": ");
        sig.push_str(&base_classes.join(", "));
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
        is_exported,
        is_abstract: false,
        is_async: false,
        is_test: false,
        is_entry_point: false,
        body_text,
    })
}

// ---------------------------------------------------------------------------
// Protocol extraction
// ---------------------------------------------------------------------------

fn extract_protocol(node: Node, source: &str) -> Option<TsSymbol> {
    let name_node = node.child_by_field_name("name")?;
    let name = node_text(name_node, source);
    if name.is_empty() {
        return None;
    }

    let is_exported = has_modifier_keyword(node, source, "public");
    let base_classes = extract_inheritance_clause(node, source);
    let body = node.child_by_field_name("body");
    let body_text = body.map(|b| node_text(b, source));

    let mut sig = format!("protocol {}", name);
    if !base_classes.is_empty() {
        sig.push_str(": ");
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
        is_exported,
        is_abstract: true,
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

    let is_exported = has_modifier_keyword(node, source, "public");
    let base_classes = extract_inheritance_clause(node, source);
    let body = node.child_by_field_name("body");
    let body_text = body.map(|b| node_text(b, source));

    let mut sig = format!("enum {}", name);
    if !base_classes.is_empty() {
        sig.push_str(": ");
        sig.push_str(&base_classes.join(", "));
    }

    Some(TsSymbol {
        name,
        label: "Enum".into(),
        start_line: node.start_position().row as i32 + 1,
        end_line: node.end_position().row as i32 + 1,
        parent_name: None,
        signature: Some(sig),
        return_type: None,
        parameters: Vec::new(),
        docstring: None,
        decorators: Vec::new(),
        base_classes,
        is_exported,
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
    // In tree-sitter-swift, parameters are in a parameter_clause or parameter child
    let mut params = Vec::new();
    for child in node.named_children(&mut node.walk()) {
        if child.kind() == "parameter" {
            let pname = child
                .child_by_field_name("name")
                .or_else(|| child.child_by_field_name("external_name"))
                .map(|n| node_text(n, source))
                .unwrap_or_default();
            let ptype = child
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
                .strip_prefix("->")
                .unwrap_or(trimmed)
                .trim()
                .to_string()
        })
        .filter(|s| !s.is_empty())
}

// ---------------------------------------------------------------------------
// Inheritance clause extraction
// ---------------------------------------------------------------------------

fn extract_inheritance_clause(node: Node, source: &str) -> Vec<String> {
    let mut bases = Vec::new();
    for child in node.children(&mut node.walk()) {
        if child.kind() == "type_constraints"
            || child.kind() == "inheritance_specifier"
            || child.kind() == "type_identifier"
        {
            // Collect type identifiers from inheritance
            let text = node_text(child, source).trim().to_string();
            if !text.is_empty() && text != ":" {
                bases.push(text);
            }
        }
    }
    bases
}

// ---------------------------------------------------------------------------
// Modifier helpers
// ---------------------------------------------------------------------------

fn has_modifier_keyword(node: Node, source: &str, keyword: &str) -> bool {
    for child in node.children(&mut node.walk()) {
        if (child.kind() == "modifiers" || child.kind() == "modifier")
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
        if sib.kind() == "comment" || sib.kind() == "multiline_comment" {
            let text = node_text(sib, source);
            let trimmed = text.trim();
            if let Some(s) = trimmed.strip_prefix("///") {
                lines.push(s.strip_prefix(' ').unwrap_or(s).to_string());
            } else if let Some(s) = trimmed.strip_prefix("//") {
                lines.push(s.strip_prefix(' ').unwrap_or(s).to_string());
            } else if trimmed.starts_with("/**") {
                let cleaned = trimmed
                    .strip_prefix("/**")
                    .unwrap_or(trimmed)
                    .strip_suffix("*/")
                    .unwrap_or(trimmed)
                    .trim()
                    .to_string();
                lines.push(cleaned);
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

fn build_swift_signature(
    name: &str,
    params: &[TsParam],
    return_type: Option<&str>,
    is_async: bool,
) -> String {
    let mut sig = String::from("func ");
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
    if is_async {
        sig.push_str(" async");
    }
    if let Some(rt) = return_type {
        sig.push_str(" -> ");
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

fn find_child_name(node: Node, source: &str) -> Option<String> {
    node.child_by_field_name("name")
        .map(|n| node_text(n, source))
        .filter(|s| !s.is_empty())
}
