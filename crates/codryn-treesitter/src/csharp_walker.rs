//! C# AST walker.
//!
//! Extracts: class declarations, struct declarations, interface declarations,
//! enum declarations, method declarations, namespace declarations.
//! Handles: access modifiers, async, abstract, virtual, override, base classes,
//! XML doc comments, attributes/decorators.

use crate::{TsParam, TsSymbol};
use tree_sitter::{Node, TreeCursor};

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

/// Walk a C# tree-sitter AST and extract symbols.
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
            "class_declaration" | "struct_declaration" | "record_declaration" => {
                let doc = collect_preceding_comments(node, source);
                let attrs = collect_attributes(node, source);
                if let Some(mut sym) = extract_class_like(node, source, kind) {
                    sym.docstring = doc.or(sym.docstring);
                    sym.decorators = attrs;
                    let name = sym.name.clone();
                    symbols.push(sym);
                    visit_children(cursor, source, symbols, Some(&name));
                }
            }
            "interface_declaration" => {
                let doc = collect_preceding_comments(node, source);
                let attrs = collect_attributes(node, source);
                if let Some(mut sym) = extract_interface(node, source) {
                    sym.docstring = doc.or(sym.docstring);
                    sym.decorators = attrs;
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
            "method_declaration" | "constructor_declaration" => {
                let doc = collect_preceding_comments(node, source);
                let attrs = collect_attributes(node, source);
                if let Some(mut sym) = extract_method(node, source, parent_name) {
                    sym.docstring = doc.or(sym.docstring);
                    sym.decorators = attrs;
                    symbols.push(sym);
                }
                visit_children(cursor, source, symbols, parent_name);
            }
            "namespace_declaration" | "file_scoped_namespace_declaration" => {
                let doc = collect_preceding_comments(node, source);
                if let Some(mut sym) = extract_namespace(node, source) {
                    sym.docstring = doc.or(sym.docstring);
                    let ns_name = sym.name.clone();
                    symbols.push(sym);
                    visit_children(cursor, source, symbols, Some(&ns_name));
                }
            }
            "property_declaration" => {
                // Skip properties for now, just recurse
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
// Class / struct / record extraction
// ---------------------------------------------------------------------------

fn extract_class_like(node: Node, source: &str, kind: &str) -> Option<TsSymbol> {
    let name_node = node.child_by_field_name("name")?;
    let name = node_text(name_node, source);
    if name.is_empty() {
        return None;
    }

    let is_exported = has_modifier(node, source, "public");
    let is_abstract = has_modifier(node, source, "abstract");
    let base_classes = extract_base_list(node, source);
    let body = node.child_by_field_name("body");
    let body_text = body.map(|b| node_text(b, source));

    let keyword = match kind {
        "struct_declaration" => "struct",
        "record_declaration" => "record",
        _ => "class",
    };
    let mut sig = String::new();
    if is_exported {
        sig.push_str("public ");
    }
    if is_abstract {
        sig.push_str("abstract ");
    }
    sig.push_str(keyword);
    sig.push(' ');
    sig.push_str(&name);
    if !base_classes.is_empty() {
        sig.push_str(" : ");
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

    let is_exported = has_modifier(node, source, "public");
    let base_classes = extract_base_list(node, source);
    let body = node.child_by_field_name("body");
    let body_text = body.map(|b| node_text(b, source));

    let mut sig = String::new();
    if is_exported {
        sig.push_str("public ");
    }
    sig.push_str("interface ");
    sig.push_str(&name);
    if !base_classes.is_empty() {
        sig.push_str(" : ");
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

    let is_exported = has_modifier(node, source, "public");
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
        is_exported,
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

    let is_exported = has_modifier(node, source, "public");
    let is_abstract = has_modifier(node, source, "abstract");
    let is_async = has_modifier(node, source, "async");

    let return_type = node
        .child_by_field_name("type")
        .map(|t| node_text(t, source).trim().to_string())
        .filter(|s| !s.is_empty());

    let params = extract_parameters(node, source);
    let body = node.child_by_field_name("body");
    let body_text = body.map(|b| node_text(b, source));

    let label = if parent_name.is_some() {
        "Method"
    } else {
        "Function"
    };
    let signature = build_csharp_signature(
        return_type.as_deref(),
        &name,
        &params,
        is_exported,
        is_abstract,
        is_async,
    );

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
        is_abstract,
        is_async,
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
        if child.kind() == "parameter" {
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
        }
    }
    params
}

// ---------------------------------------------------------------------------
// Base list extraction
// ---------------------------------------------------------------------------

fn extract_base_list(node: Node, source: &str) -> Vec<String> {
    let mut bases = Vec::new();
    for child in node.children(&mut node.walk()) {
        if child.kind() == "base_list" {
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
// Modifier / attribute helpers
// ---------------------------------------------------------------------------

fn has_modifier(node: Node, source: &str, keyword: &str) -> bool {
    for child in node.children(&mut node.walk()) {
        if (child.kind() == "modifier" || child.kind() == keyword)
            && node_text(child, source).trim() == keyword
        {
            return true;
        }
    }
    false
}

fn collect_attributes(node: Node, source: &str) -> Vec<String> {
    let mut attrs = Vec::new();
    let mut sibling = node.prev_sibling();
    while let Some(sib) = sibling {
        if sib.kind() == "attribute_list" {
            attrs.push(node_text(sib, source).trim().to_string());
            sibling = sib.prev_sibling();
        } else {
            break;
        }
    }
    attrs.reverse();
    attrs
}

// ---------------------------------------------------------------------------
// Comment extraction
// ---------------------------------------------------------------------------

fn collect_preceding_comments(node: Node, source: &str) -> Option<String> {
    let mut lines = Vec::new();
    let mut sibling = node.prev_sibling();
    while let Some(sib) = sibling {
        let kind = sib.kind();
        if kind == "comment" {
            let text = node_text(sib, source);
            let trimmed = text.trim();
            if let Some(s) = trimmed.strip_prefix("///") {
                lines.push(s.strip_prefix(' ').unwrap_or(s).to_string());
            } else if let Some(s) = trimmed.strip_prefix("//") {
                lines.push(s.strip_prefix(' ').unwrap_or(s).to_string());
            }
            sibling = sib.prev_sibling();
        } else if kind == "attribute_list" {
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

fn build_csharp_signature(
    return_type: Option<&str>,
    name: &str,
    params: &[TsParam],
    is_public: bool,
    is_abstract: bool,
    is_async: bool,
) -> String {
    let mut sig = String::new();
    if is_public {
        sig.push_str("public ");
    }
    if is_abstract {
        sig.push_str("abstract ");
    }
    if is_async {
        sig.push_str("async ");
    }
    if let Some(rt) = return_type {
        sig.push_str(rt);
        sig.push(' ');
    }
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
    sig
}

// ---------------------------------------------------------------------------
// Utility
// ---------------------------------------------------------------------------

fn node_text(node: Node, source: &str) -> String {
    source.get(node.byte_range()).unwrap_or("").to_string()
}
