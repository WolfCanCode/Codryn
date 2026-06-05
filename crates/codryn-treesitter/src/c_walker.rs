//! C / C++ AST walker.
//!
//! Recursively walks a tree-sitter AST and extracts symbols:
//! function definitions, struct specifiers, class specifiers (C++),
//! namespace definitions (C++), enum specifiers.
//! Handles class methods, virtual/override, templates.

use crate::{TsParam, TsSymbol};
use tree_sitter::{Node, TreeCursor};

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

/// Walk a C/C++ tree-sitter AST and extract symbols.
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
                if let Some(mut sym) = extract_function_definition(node, source, parent_name) {
                    sym.docstring = doc.or(sym.docstring);
                    symbols.push(sym);
                }
                visit_children(cursor, source, symbols, parent_name);
            }
            "declaration" => {
                // Could be a function declaration (prototype) — extract if it has a function declarator
                let doc = collect_preceding_comments(node, source);
                if let Some(mut sym) = extract_function_declaration(node, source, parent_name) {
                    sym.docstring = doc.or(sym.docstring);
                    symbols.push(sym);
                }
                visit_children(cursor, source, symbols, parent_name);
            }
            "struct_specifier" | "class_specifier" => {
                let doc = collect_preceding_comments(node, source);
                if let Some(mut sym) = extract_class_or_struct(node, source, kind) {
                    sym.docstring = doc.or(sym.docstring);
                    let name = sym.name.clone();
                    symbols.push(sym);
                    // Recurse into body for methods
                    visit_children(cursor, source, symbols, Some(&name));
                }
            }
            "enum_specifier" => {
                let doc = collect_preceding_comments(node, source);
                if let Some(mut sym) = extract_enum(node, source) {
                    sym.docstring = doc.or(sym.docstring);
                    symbols.push(sym);
                }
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
            "template_declaration" => {
                // Template wraps a function/class — recurse to find the inner declaration
                visit_children(cursor, source, symbols, parent_name);
            }
            "field_declaration" => {
                // Could be a method declaration inside a class body
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
// Function definition extraction
// ---------------------------------------------------------------------------

fn extract_function_definition(
    node: Node,
    source: &str,
    parent_name: Option<&str>,
) -> Option<TsSymbol> {
    let declarator = node.child_by_field_name("declarator")?;
    let (name, params) = extract_declarator_name_and_params(declarator, source)?;
    if name.is_empty() {
        return None;
    }

    let return_type = node
        .child_by_field_name("type")
        .map(|t| node_text(t, source).trim().to_string())
        .filter(|s| !s.is_empty());

    let is_virtual = has_specifier(node, source, "virtual");
    let body = node.child_by_field_name("body");
    let body_text = body.map(|b| node_text(b, source));

    let label = if parent_name.is_some() {
        "Method"
    } else {
        "Function"
    };
    let signature = build_c_signature(return_type.as_deref(), &name, &params, is_virtual);

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
// Function declaration (prototype) extraction
// ---------------------------------------------------------------------------

fn extract_function_declaration(
    node: Node,
    source: &str,
    parent_name: Option<&str>,
) -> Option<TsSymbol> {
    let declarator = node.child_by_field_name("declarator")?;
    // Only extract if it's a function declarator (has parameter list)
    if declarator.kind() != "function_declarator" {
        return None;
    }
    let (name, params) = extract_declarator_name_and_params(declarator, source)?;
    if name.is_empty() {
        return None;
    }

    let return_type = node
        .child_by_field_name("type")
        .map(|t| node_text(t, source).trim().to_string())
        .filter(|s| !s.is_empty());

    let is_virtual = has_specifier(node, source, "virtual");
    let is_abstract = node_text(node, source).contains("= 0");

    let label = if parent_name.is_some() {
        "Method"
    } else {
        "Function"
    };
    let signature = build_c_signature(return_type.as_deref(), &name, &params, is_virtual);

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
        is_abstract,
        is_async: false,
        is_test: false,
        is_entry_point: false,
        body_text: None,
    })
}

// ---------------------------------------------------------------------------
// Class / struct extraction
// ---------------------------------------------------------------------------

fn extract_class_or_struct(node: Node, source: &str, kind: &str) -> Option<TsSymbol> {
    let name_node = node.child_by_field_name("name")?;
    let name = node_text(name_node, source);
    if name.is_empty() {
        return None;
    }

    let base_classes = extract_base_class_clause(node, source);
    let body = node.child_by_field_name("body");
    let body_text = body.map(|b| node_text(b, source));

    let keyword = if kind == "class_specifier" {
        "class"
    } else {
        "struct"
    };
    let mut sig = format!("{} {}", keyword, name);
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

    let body = node.child_by_field_name("body");
    let body_text = body.map(|b| node_text(b, source));

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
        body_text,
    })
}

// ---------------------------------------------------------------------------
// Declarator helpers
// ---------------------------------------------------------------------------

/// Recursively unwrap a declarator to find the function name and parameters.
/// Handles: `function_declarator`, `pointer_declarator`, `qualified_identifier`, etc.
fn extract_declarator_name_and_params(node: Node, source: &str) -> Option<(String, Vec<TsParam>)> {
    match node.kind() {
        "function_declarator" => {
            let name_part = node.child_by_field_name("declarator")?;
            let name = extract_simple_name(name_part, source);
            let params = extract_parameter_list(node, source);
            Some((name, params))
        }
        "pointer_declarator" => {
            let inner = node.child_by_field_name("declarator")?;
            extract_declarator_name_and_params(inner, source)
        }
        "parenthesized_declarator" => {
            let inner = node.named_child(0)?;
            extract_declarator_name_and_params(inner, source)
        }
        "reference_declarator" => {
            let inner = node.named_child(0)?;
            extract_declarator_name_and_params(inner, source)
        }
        _ => {
            let name = extract_simple_name(node, source);
            if !name.is_empty() {
                Some((name, Vec::new()))
            } else {
                None
            }
        }
    }
}

/// Extract a simple name from an identifier or qualified_identifier node.
fn extract_simple_name(node: Node, source: &str) -> String {
    match node.kind() {
        "identifier" | "field_identifier" | "type_identifier" => node_text(node, source),
        "qualified_identifier" | "scoped_identifier" => {
            // e.g. MyClass::method — take the full qualified name
            node_text(node, source)
        }
        "destructor_name" => node_text(node, source),
        "template_function" => {
            // e.g. foo<T> — take just the name part
            node.child_by_field_name("name")
                .map(|n| node_text(n, source))
                .unwrap_or_else(|| node_text(node, source))
        }
        _ => node_text(node, source),
    }
}

// ---------------------------------------------------------------------------
// Parameter extraction
// ---------------------------------------------------------------------------

fn extract_parameter_list(node: Node, source: &str) -> Vec<TsParam> {
    let params_node = match node.child_by_field_name("parameters") {
        Some(n) => n,
        None => return Vec::new(),
    };

    let mut params = Vec::new();
    for child in params_node.named_children(&mut params_node.walk()) {
        match child.kind() {
            "parameter_declaration" => {
                let ptype = child
                    .child_by_field_name("type")
                    .map(|t| node_text(t, source));
                let pname = child
                    .child_by_field_name("declarator")
                    .map(|d| extract_simple_name(d, source))
                    .unwrap_or_default();
                params.push(TsParam {
                    name: if pname.is_empty() { "_".into() } else { pname },
                    type_name: ptype,
                });
            }
            "optional_parameter_declaration" => {
                let ptype = child
                    .child_by_field_name("type")
                    .map(|t| node_text(t, source));
                let pname = child
                    .child_by_field_name("declarator")
                    .map(|d| extract_simple_name(d, source))
                    .unwrap_or_default();
                params.push(TsParam {
                    name: if pname.is_empty() { "_".into() } else { pname },
                    type_name: ptype,
                });
            }
            "variadic_parameter_declaration" => {
                params.push(TsParam {
                    name: "...".into(),
                    type_name: None,
                });
            }
            _ => {}
        }
    }
    params
}

// ---------------------------------------------------------------------------
// Base class extraction
// ---------------------------------------------------------------------------

fn extract_base_class_clause(node: Node, source: &str) -> Vec<String> {
    let mut bases = Vec::new();
    for child in node.children(&mut node.walk()) {
        if child.kind() == "base_class_clause" {
            for inner in child.named_children(&mut child.walk()) {
                let text = node_text(inner, source).trim().to_string();
                // Strip access specifiers like "public ", "private ", "protected "
                let cleaned = text
                    .strip_prefix("public ")
                    .or_else(|| text.strip_prefix("private "))
                    .or_else(|| text.strip_prefix("protected "))
                    .unwrap_or(&text)
                    .to_string();
                if !cleaned.is_empty() {
                    bases.push(cleaned);
                }
            }
        }
    }
    bases
}

// ---------------------------------------------------------------------------
// Specifier detection
// ---------------------------------------------------------------------------

fn has_specifier(node: Node, source: &str, keyword: &str) -> bool {
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
            // Collect C-style doc comments: /** ... */, /// ..., //! ...
            if trimmed.starts_with("///")
                || trimmed.starts_with("//!")
                || trimmed.starts_with("/**")
            {
                lines.push(clean_c_comment(trimmed));
                sibling = sib.prev_sibling();
            } else if trimmed.starts_with("//") {
                lines.push(
                    trimmed
                        .strip_prefix("//")
                        .unwrap_or(trimmed)
                        .trim()
                        .to_string(),
                );
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

fn clean_c_comment(text: &str) -> String {
    if let Some(s) = text.strip_prefix("///") {
        return s.strip_prefix(' ').unwrap_or(s).to_string();
    }
    if let Some(s) = text.strip_prefix("//!") {
        return s.strip_prefix(' ').unwrap_or(s).to_string();
    }
    if text.starts_with("/**") {
        let s = text.strip_prefix("/**").unwrap_or(text);
        let s = s.strip_suffix("*/").unwrap_or(s);
        return s.trim().to_string();
    }
    text.to_string()
}

// ---------------------------------------------------------------------------
// Signature building
// ---------------------------------------------------------------------------

fn build_c_signature(
    return_type: Option<&str>,
    name: &str,
    params: &[TsParam],
    is_virtual: bool,
) -> String {
    let mut sig = String::new();
    if is_virtual {
        sig.push_str("virtual ");
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
