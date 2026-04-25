//! Rust AST walker.
//!
//! Recursively walks a tree-sitter AST and extracts symbols:
//! function items, struct items, enum items, trait items, impl items
//! (including methods within impl blocks).  Handles `pub`, `pub(crate)`,
//! `async`, generic parameters, doc comments (`///`), attributes
//! (`#[test]`, `#[tokio::test]`), and trait implementations
//! (`impl Trait for Struct`).

use crate::{TsParam, TsSymbol};
use tree_sitter::{Node, TreeCursor};

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

/// Walk a Rust tree-sitter AST and extract symbols.
pub fn walk_tree(tree: &tree_sitter::Tree, source: &str) -> Vec<TsSymbol> {
    let mut symbols = Vec::new();
    let mut cursor = tree.walk();
    visit_children(&mut cursor, source, &mut symbols, None);
    symbols
}

// ---------------------------------------------------------------------------
// Recursive visitor
// ---------------------------------------------------------------------------

/// Recursively visit children of the current cursor node.
///
/// `parent_name` carries the name of the enclosing impl/trait (if any) so
/// that methods get their `parent_name` set.
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
            "function_item" => {
                let attrs = collect_preceding_attributes(node, source);
                let doc = collect_preceding_doc_comments(node, source);
                if let Some(mut sym) = extract_function_item(node, source, parent_name) {
                    sym.decorators = attrs;
                    sym.docstring = doc.or(sym.docstring);
                    sym.is_test = is_rust_test(&sym.decorators);
                    sym.is_entry_point = is_rust_entry_point(&sym.name, &sym.decorators);
                    symbols.push(sym);
                }
                // Recurse for nested items inside function body
                visit_children(cursor, source, symbols, parent_name);
            }
            "struct_item" => {
                let attrs = collect_preceding_attributes(node, source);
                let doc = collect_preceding_doc_comments(node, source);
                if let Some(mut sym) = extract_struct_item(node, source) {
                    sym.decorators = attrs;
                    sym.docstring = doc.or(sym.docstring);
                    symbols.push(sym);
                }
            }
            "enum_item" => {
                let attrs = collect_preceding_attributes(node, source);
                let doc = collect_preceding_doc_comments(node, source);
                if let Some(mut sym) = extract_enum_item(node, source) {
                    sym.decorators = attrs;
                    sym.docstring = doc.or(sym.docstring);
                    symbols.push(sym);
                }
            }
            "trait_item" => {
                let attrs = collect_preceding_attributes(node, source);
                let doc = collect_preceding_doc_comments(node, source);
                if let Some(mut sym) = extract_trait_item(node, source) {
                    sym.decorators = attrs;
                    sym.docstring = doc.or(sym.docstring);
                    let trait_name = sym.name.clone();
                    symbols.push(sym);
                    // Recurse into trait body for method signatures
                    visit_children(cursor, source, symbols, Some(&trait_name));
                }
            }
            "impl_item" => {
                let attrs = collect_preceding_attributes(node, source);
                let doc = collect_preceding_doc_comments(node, source);
                if let Some(mut sym) = extract_impl_item(node, source) {
                    sym.decorators = attrs;
                    sym.docstring = doc.or(sym.docstring);
                    let impl_type_name = sym.name.clone();
                    symbols.push(sym);
                    // Recurse into impl body for methods
                    visit_children(cursor, source, symbols, Some(&impl_type_name));
                }
            }
            "function_signature_item" => {
                // Trait method signature (no body)
                let attrs = collect_preceding_attributes(node, source);
                let doc = collect_preceding_doc_comments(node, source);
                if let Some(mut sym) = extract_function_signature(node, source, parent_name) {
                    sym.decorators = attrs;
                    sym.docstring = doc.or(sym.docstring);
                    sym.is_test = is_rust_test(&sym.decorators);
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
// Function item extraction
// ---------------------------------------------------------------------------

/// Extract a `function_item` node: `fn foo(...) -> T { ... }`
fn extract_function_item(node: Node, source: &str, parent_name: Option<&str>) -> Option<TsSymbol> {
    let name_node = node.child_by_field_name("name")?;
    let name = node_text(name_node, source);
    if name.is_empty() {
        return None;
    }

    let is_pub = has_visibility(node);
    let is_async = has_child_kind(node, "async");
    let params = extract_parameters(node, source);
    let return_type = extract_return_type(node, source);
    let generics = extract_generic_params(node, source);
    let body = node.child_by_field_name("body");
    let body_text = body.map(|b| node_text(b, source));

    let label = if parent_name.is_some() {
        "Method"
    } else {
        "Function"
    };
    let signature = build_fn_signature(
        &name,
        &generics,
        &params,
        return_type.as_deref(),
        is_async,
        is_pub,
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
        is_exported: is_pub,
        is_abstract: false,
        is_async,
        is_test: false,        // set by caller after decorators are collected
        is_entry_point: false, // set by caller after decorators are collected
        body_text,
    })
}

// ---------------------------------------------------------------------------
// Function signature extraction (trait method without body)
// ---------------------------------------------------------------------------

/// Extract a `function_signature_item` node (trait method declaration).
fn extract_function_signature(
    node: Node,
    source: &str,
    parent_name: Option<&str>,
) -> Option<TsSymbol> {
    let name_node = node.child_by_field_name("name")?;
    let name = node_text(name_node, source);
    if name.is_empty() {
        return None;
    }

    let is_async = has_child_kind(node, "async");
    let params = extract_parameters(node, source);
    let return_type = extract_return_type(node, source);
    let generics = extract_generic_params(node, source);

    let signature = build_fn_signature(
        &name,
        &generics,
        &params,
        return_type.as_deref(),
        is_async,
        false,
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
        is_exported: false,
        is_abstract: true, // trait method without body is abstract
        is_async,
        is_test: false, // set by caller
        is_entry_point: false,
        body_text: None,
    })
}

// ---------------------------------------------------------------------------
// Struct extraction
// ---------------------------------------------------------------------------

/// Extract a `struct_item` node.
fn extract_struct_item(node: Node, source: &str) -> Option<TsSymbol> {
    let name_node = node.child_by_field_name("name")?;
    let name = node_text(name_node, source);
    if name.is_empty() {
        return None;
    }

    let is_pub = has_visibility(node);
    let generics = extract_generic_params(node, source);
    let body = node.child_by_field_name("body");
    let body_text = body.map(|b| node_text(b, source));

    let mut sig = String::new();
    if is_pub {
        sig.push_str("pub ");
    }
    sig.push_str("struct ");
    sig.push_str(&name);
    sig.push_str(&generics);

    Some(TsSymbol {
        name,
        label: "Class".into(), // Structs map to "Class" label in the graph
        start_line: node.start_position().row as i32 + 1,
        end_line: node.end_position().row as i32 + 1,
        parent_name: None,
        signature: Some(sig),
        return_type: None,
        parameters: Vec::new(),
        docstring: None,
        decorators: Vec::new(),
        base_classes: Vec::new(),
        is_exported: is_pub,
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

/// Extract an `enum_item` node.
fn extract_enum_item(node: Node, source: &str) -> Option<TsSymbol> {
    let name_node = node.child_by_field_name("name")?;
    let name = node_text(name_node, source);
    if name.is_empty() {
        return None;
    }

    let is_pub = has_visibility(node);
    let generics = extract_generic_params(node, source);
    let body = node.child_by_field_name("body");
    let body_text = body.map(|b| node_text(b, source));

    let mut sig = String::new();
    if is_pub {
        sig.push_str("pub ");
    }
    sig.push_str("enum ");
    sig.push_str(&name);
    sig.push_str(&generics);

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
        base_classes: Vec::new(),
        is_exported: is_pub,
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

/// Extract a `trait_item` node.
fn extract_trait_item(node: Node, source: &str) -> Option<TsSymbol> {
    let name_node = node.child_by_field_name("name")?;
    let name = node_text(name_node, source);
    if name.is_empty() {
        return None;
    }

    let is_pub = has_visibility(node);
    let generics = extract_generic_params(node, source);
    let body = node.child_by_field_name("body");
    let body_text = body.map(|b| node_text(b, source));

    // Extract super-traits from bounds: `trait Foo: Bar + Baz`
    let base_classes = extract_trait_bounds(node, source);

    let mut sig = String::new();
    if is_pub {
        sig.push_str("pub ");
    }
    sig.push_str("trait ");
    sig.push_str(&name);
    sig.push_str(&generics);
    if !base_classes.is_empty() {
        sig.push_str(": ");
        sig.push_str(&base_classes.join(" + "));
    }

    Some(TsSymbol {
        name,
        label: "Interface".into(), // Traits map to "Interface" label
        start_line: node.start_position().row as i32 + 1,
        end_line: node.end_position().row as i32 + 1,
        parent_name: None,
        signature: Some(sig),
        return_type: None,
        parameters: Vec::new(),
        docstring: None,
        decorators: Vec::new(),
        base_classes,
        is_exported: is_pub,
        is_abstract: true,
        is_async: false,
        is_test: false,
        is_entry_point: false,
        body_text,
    })
}

// ---------------------------------------------------------------------------
// Impl extraction
// ---------------------------------------------------------------------------

/// Extract an `impl_item` node: `impl Foo { ... }` or `impl Trait for Foo { ... }`
fn extract_impl_item(node: Node, source: &str) -> Option<TsSymbol> {
    // Determine the type being implemented and optionally the trait
    let (impl_type, trait_name) = extract_impl_names(node, source)?;

    let is_pub = has_visibility(node);
    let generics = extract_generic_params(node, source);
    let body = node.child_by_field_name("body");
    let body_text = body.map(|b| node_text(b, source));

    let base_classes = if let Some(ref t) = trait_name {
        vec![t.clone()]
    } else {
        Vec::new()
    };

    let mut sig = String::new();
    sig.push_str("impl");
    sig.push_str(&generics);
    sig.push(' ');
    if let Some(ref t) = trait_name {
        sig.push_str(t);
        sig.push_str(" for ");
    }
    sig.push_str(&impl_type);

    Some(TsSymbol {
        name: impl_type,
        label: "Impl".into(),
        start_line: node.start_position().row as i32 + 1,
        end_line: node.end_position().row as i32 + 1,
        parent_name: None,
        signature: Some(sig),
        return_type: None,
        parameters: Vec::new(),
        docstring: None,
        decorators: Vec::new(),
        base_classes,
        is_exported: is_pub,
        is_abstract: false,
        is_async: false,
        is_test: false,
        is_entry_point: false,
        body_text,
    })
}

/// Extract the type name and optional trait name from an `impl_item`.
///
/// For `impl Foo { ... }` → `("Foo", None)`
/// For `impl Trait for Foo { ... }` → `("Foo", Some("Trait"))`
fn extract_impl_names(node: Node, source: &str) -> Option<(String, Option<String>)> {
    // In tree-sitter-rust, impl_item has:
    //   - `type` field: the type being implemented
    //   - `trait` field: the trait (if this is a trait impl)
    let type_node = node.child_by_field_name("type")?;
    let impl_type = extract_type_name(type_node, source);
    if impl_type.is_empty() {
        return None;
    }

    let trait_name = node
        .child_by_field_name("trait")
        .map(|t| extract_type_name(t, source))
        .filter(|s| !s.is_empty());

    Some((impl_type, trait_name))
}

/// Extract a type name from a type node, handling `type_identifier`,
/// `generic_type`, `scoped_type_identifier`, etc.
fn extract_type_name(node: Node, source: &str) -> String {
    match node.kind() {
        "type_identifier" => node_text(node, source),
        "generic_type" => {
            // e.g. `Vec<T>` — take just the base name
            node.child_by_field_name("type")
                .map(|n| node_text(n, source))
                .unwrap_or_else(|| node_text(node, source))
        }
        "scoped_type_identifier" => node_text(node, source),
        _ => node_text(node, source),
    }
}

// ---------------------------------------------------------------------------
// Parameter extraction
// ---------------------------------------------------------------------------

/// Extract parameters from a `function_item` or `function_signature_item`.
fn extract_parameters(node: Node, source: &str) -> Vec<TsParam> {
    let params_node = match node.child_by_field_name("parameters") {
        Some(n) => n,
        None => return Vec::new(),
    };

    let mut params = Vec::new();
    for child in params_node.named_children(&mut params_node.walk()) {
        match child.kind() {
            "parameter" => {
                // `pattern: type`
                let pname = child
                    .child_by_field_name("pattern")
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
            "self_parameter" => {
                // `&self`, `&mut self`, `self`
                let text = node_text(child, source);
                params.push(TsParam {
                    name: text,
                    type_name: Some("Self".into()),
                });
            }
            _ => {}
        }
    }
    params
}

// ---------------------------------------------------------------------------
// Return type extraction
// ---------------------------------------------------------------------------

/// Extract the return type from a function item.
/// Rust uses `-> Type` syntax.
fn extract_return_type(node: Node, source: &str) -> Option<String> {
    node.child_by_field_name("return_type")
        .map(|rt| {
            let text = node_text(rt, source);
            let trimmed = text.trim();
            // Strip leading `->` if present
            if let Some(stripped) = trimmed.strip_prefix("->") {
                stripped.trim().to_string()
            } else {
                trimmed.to_string()
            }
        })
        .filter(|s| !s.is_empty())
}

// ---------------------------------------------------------------------------
// Generic parameter extraction
// ---------------------------------------------------------------------------

/// Extract generic type parameters, e.g. `<T, U: Clone>`.
/// Returns the full text including angle brackets, or empty string if none.
fn extract_generic_params(node: Node, source: &str) -> String {
    node.child_by_field_name("type_parameters")
        .map(|tp| node_text(tp, source))
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// Trait bounds extraction
// ---------------------------------------------------------------------------

/// Extract super-trait bounds from a trait item.
/// e.g. `trait Foo: Bar + Baz` → `["Bar", "Baz"]`
fn extract_trait_bounds(node: Node, source: &str) -> Vec<String> {
    let mut bounds = Vec::new();
    // In tree-sitter-rust, trait bounds are in a `trait_bounds` child
    // which contains `type_identifier` nodes separated by `+`
    for child in node.children(&mut node.walk()) {
        if child.kind() == "trait_bounds" {
            for bound_child in child.named_children(&mut child.walk()) {
                match bound_child.kind() {
                    "type_identifier" | "scoped_type_identifier" | "generic_type" => {
                        let name = node_text(bound_child, source);
                        if !name.is_empty() {
                            bounds.push(name);
                        }
                    }
                    _ => {}
                }
            }
        }
    }
    bounds
}

// ---------------------------------------------------------------------------
// Attribute extraction
// ---------------------------------------------------------------------------

/// Collect `#[...]` attributes that immediately precede the given node.
fn collect_preceding_attributes(node: Node, source: &str) -> Vec<String> {
    let mut attrs = Vec::new();
    let mut sibling = node.prev_sibling();
    while let Some(sib) = sibling {
        if sib.kind() == "attribute_item" {
            let text = node_text(sib, source).trim().to_string();
            attrs.push(text);
            sibling = sib.prev_sibling();
        } else if sib.kind() == "line_comment" {
            // Skip doc comments to keep looking for attributes
            sibling = sib.prev_sibling();
        } else {
            break;
        }
    }
    attrs.reverse(); // Restore original order
    attrs
}

// ---------------------------------------------------------------------------
// Doc comment extraction
// ---------------------------------------------------------------------------

/// Collect `///` doc comments that immediately precede the given node.
fn collect_preceding_doc_comments(node: Node, source: &str) -> Option<String> {
    let mut lines = Vec::new();
    let mut sibling = node.prev_sibling();
    while let Some(sib) = sibling {
        if sib.kind() == "line_comment" {
            let text = node_text(sib, source);
            let trimmed = text.trim();
            if let Some(doc) = trimmed.strip_prefix("///") {
                lines.push(doc.strip_prefix(' ').unwrap_or(doc).to_string());
                sibling = sib.prev_sibling();
            } else {
                break;
            }
        } else if sib.kind() == "attribute_item" {
            // Skip attributes to keep looking for doc comments
            sibling = sib.prev_sibling();
        } else {
            break;
        }
    }

    if lines.is_empty() {
        return None;
    }

    lines.reverse(); // Restore original order
    Some(lines.join("\n"))
}

// ---------------------------------------------------------------------------
// Visibility detection
// ---------------------------------------------------------------------------

/// Check if a node has a `visibility_modifier` child (`pub`, `pub(crate)`, etc.).
fn has_visibility(node: Node) -> bool {
    for child in node.children(&mut node.walk()) {
        if child.kind() == "visibility_modifier" {
            return true;
        }
    }
    false
}

// ---------------------------------------------------------------------------
// Signature building
// ---------------------------------------------------------------------------

fn build_fn_signature(
    name: &str,
    generics: &str,
    params: &[TsParam],
    return_type: Option<&str>,
    is_async: bool,
    is_pub: bool,
) -> String {
    let mut sig = String::new();
    if is_pub {
        sig.push_str("pub ");
    }
    if is_async {
        sig.push_str("async ");
    }
    sig.push_str("fn ");
    sig.push_str(name);
    sig.push_str(generics);
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
        sig.push_str(" -> ");
        sig.push_str(rt);
    }
    sig
}

// ---------------------------------------------------------------------------
// Utility
// ---------------------------------------------------------------------------

/// Get the UTF-8 text of a tree-sitter node.
fn node_text(node: Node, source: &str) -> String {
    source.get(node.byte_range()).unwrap_or("").to_string()
}

/// Check if a node has a direct child of the given kind.
fn has_child_kind(node: Node, kind: &str) -> bool {
    for child in node.children(&mut node.walk()) {
        if child.kind() == kind {
            return true;
        }
    }
    false
}

// ---------------------------------------------------------------------------
// is_test / is_entry_point detection
// ---------------------------------------------------------------------------

/// Check if a Rust function is a test based on its attributes.
/// Matches: `#[test]`, `#[tokio::test]`, `#[cfg(test)]`
fn is_rust_test(attrs: &[String]) -> bool {
    attrs.iter().any(|a| {
        a.contains("#[test]")
            || a.contains("tokio::test")
            || a.contains("cfg(test)")
            || a.contains("rstest")
    })
}

/// Check if a Rust function is an entry point.
/// Matches: `fn main()`, `#[tokio::main]`
fn is_rust_entry_point(name: &str, attrs: &[String]) -> bool {
    name == "main"
        || attrs
            .iter()
            .any(|a| a.contains("tokio::main") || a.contains("actix_web::main"))
}
