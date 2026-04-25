//! Elixir AST walker.
//!
//! Extracts: def/defp function definitions, defmodule module definitions,
//! defmacro/defmacrop macro definitions, defprotocol, defimpl.
//! Handles: module nesting, @doc/@moduledoc attributes, @spec type specs.

use crate::{TsParam, TsSymbol};
use tree_sitter::{Node, TreeCursor};

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

/// Walk an Elixir tree-sitter AST and extract symbols.
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
            "call" => {
                // In tree-sitter-elixir, `def`, `defmodule`, `defmacro` etc.
                // are all represented as `call` nodes.
                handle_call(cursor, node, source, symbols, parent_name);
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
// Call node dispatch
// ---------------------------------------------------------------------------

fn handle_call(
    cursor: &mut TreeCursor,
    node: Node,
    source: &str,
    symbols: &mut Vec<TsSymbol>,
    parent_name: Option<&str>,
) {
    let target = match node.child_by_field_name("target") {
        Some(t) => node_text(t, source),
        None => {
            visit_children(cursor, source, symbols, parent_name);
            return;
        }
    };

    match target.as_str() {
        "defmodule" => {
            let doc = collect_preceding_doc(node, source);
            if let Some(mut sym) = extract_defmodule(node, source) {
                sym.docstring = doc.or(sym.docstring);
                let name = sym.name.clone();
                symbols.push(sym);
                visit_children(cursor, source, symbols, Some(&name));
            }
        }
        "def" | "defp" => {
            let doc = collect_preceding_doc(node, source);
            if let Some(mut sym) = extract_def(node, source, parent_name, &target) {
                sym.docstring = doc.or(sym.docstring);
                symbols.push(sym);
            }
            visit_children(cursor, source, symbols, parent_name);
        }
        "defmacro" | "defmacrop" => {
            let doc = collect_preceding_doc(node, source);
            if let Some(mut sym) = extract_def(node, source, parent_name, &target) {
                sym.docstring = doc.or(sym.docstring);
                sym.decorators.push(target.clone());
                symbols.push(sym);
            }
            visit_children(cursor, source, symbols, parent_name);
        }
        "defprotocol" => {
            let doc = collect_preceding_doc(node, source);
            if let Some(mut sym) = extract_defprotocol(node, source) {
                sym.docstring = doc.or(sym.docstring);
                let name = sym.name.clone();
                symbols.push(sym);
                visit_children(cursor, source, symbols, Some(&name));
            }
        }
        "defimpl" => {
            // Implementation of a protocol — recurse into body
            visit_children(cursor, source, symbols, parent_name);
        }
        _ => {
            visit_children(cursor, source, symbols, parent_name);
        }
    }
}

// ---------------------------------------------------------------------------
// defmodule extraction
// ---------------------------------------------------------------------------

fn extract_defmodule(node: Node, source: &str) -> Option<TsSymbol> {
    let args = node.child_by_field_name("arguments")?;
    let first_arg = args.named_child(0)?;
    let name = node_text(first_arg, source).trim().to_string();
    if name.is_empty() {
        return None;
    }

    let body_text = extract_do_block_text(node, source);

    Some(TsSymbol {
        name: name.clone(),
        label: "Module".into(),
        start_line: node.start_position().row as i32 + 1,
        end_line: node.end_position().row as i32 + 1,
        parent_name: None,
        signature: Some(format!("defmodule {}", name)),
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
// def/defp extraction
// ---------------------------------------------------------------------------

fn extract_def(
    node: Node,
    source: &str,
    parent_name: Option<&str>,
    def_kind: &str,
) -> Option<TsSymbol> {
    let args = node.child_by_field_name("arguments")?;
    let first_arg = args.named_child(0)?;

    // The first argument to def/defp is either:
    // - A call node: `def foo(a, b)` → call with target=foo, arguments=(a,b)
    // - An identifier: `def foo` (no params)
    let (name, params) = match first_arg.kind() {
        "call" => {
            let fn_name = first_arg
                .child_by_field_name("target")
                .map(|t| node_text(t, source))
                .unwrap_or_default();
            let fn_params = extract_call_params(first_arg, source);
            (fn_name, fn_params)
        }
        "identifier" => (node_text(first_arg, source), Vec::new()),
        "binary_operator" => {
            // Pattern like `def foo(a) when is_integer(a)`
            let left = first_arg.child(0);
            if let Some(left) = left {
                if left.kind() == "call" {
                    let fn_name = left
                        .child_by_field_name("target")
                        .map(|t| node_text(t, source))
                        .unwrap_or_default();
                    let fn_params = extract_call_params(left, source);
                    (fn_name, fn_params)
                } else {
                    (node_text(left, source), Vec::new())
                }
            } else {
                return None;
            }
        }
        _ => return None,
    };

    if name.is_empty() {
        return None;
    }

    let is_exported = def_kind == "def" || def_kind == "defmacro";
    let body_text = extract_do_block_text(node, source);

    let label = if parent_name.is_some() {
        "Method"
    } else {
        "Function"
    };
    let signature = build_elixir_signature(def_kind, &name, &params);

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
        is_exported,
        is_abstract: false,
        is_async: false,
        is_test: false,
        is_entry_point: false,
        body_text,
    })
}

// ---------------------------------------------------------------------------
// defprotocol extraction
// ---------------------------------------------------------------------------

fn extract_defprotocol(node: Node, source: &str) -> Option<TsSymbol> {
    let args = node.child_by_field_name("arguments")?;
    let first_arg = args.named_child(0)?;
    let name = node_text(first_arg, source).trim().to_string();
    if name.is_empty() {
        return None;
    }

    let body_text = extract_do_block_text(node, source);

    Some(TsSymbol {
        name: name.clone(),
        label: "Interface".into(),
        start_line: node.start_position().row as i32 + 1,
        end_line: node.end_position().row as i32 + 1,
        parent_name: None,
        signature: Some(format!("defprotocol {}", name)),
        return_type: None,
        parameters: Vec::new(),
        docstring: None,
        decorators: Vec::new(),
        base_classes: Vec::new(),
        is_exported: true,
        is_abstract: true,
        is_async: false,
        is_test: false,
        is_entry_point: false,
        body_text,
    })
}

// ---------------------------------------------------------------------------
// Parameter extraction from call arguments
// ---------------------------------------------------------------------------

fn extract_call_params(call_node: Node, source: &str) -> Vec<TsParam> {
    let args = match call_node.child_by_field_name("arguments") {
        Some(a) => a,
        None => return Vec::new(),
    };

    let mut params = Vec::new();
    for child in args.named_children(&mut args.walk()) {
        let text = node_text(child, source).trim().to_string();
        if !text.is_empty() {
            // Elixir params don't have types in the signature
            params.push(TsParam {
                name: text,
                type_name: None,
            });
        }
    }
    params
}

// ---------------------------------------------------------------------------
// Do-block body extraction
// ---------------------------------------------------------------------------

fn extract_do_block_text(node: Node, source: &str) -> Option<String> {
    for child in node.named_children(&mut node.walk()) {
        if child.kind() == "do_block" {
            return Some(node_text(child, source));
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Doc attribute extraction
// ---------------------------------------------------------------------------

fn collect_preceding_doc(node: Node, source: &str) -> Option<String> {
    // In Elixir, @doc and @moduledoc precede the definition
    let mut sibling = node.prev_sibling();
    while let Some(sib) = sibling {
        if sib.kind() == "call" {
            let target_text = sib
                .child_by_field_name("target")
                .map(|t| node_text(t, source))
                .unwrap_or_default();
            if target_text == "@doc" || target_text == "@moduledoc" {
                // Extract the string argument
                if let Some(args) = sib.child_by_field_name("arguments") {
                    if let Some(first) = args.named_child(0) {
                        let text = node_text(first, source);
                        let cleaned = text
                            .trim()
                            .trim_matches('"')
                            .trim_start_matches("\"\"\"")
                            .trim_end_matches("\"\"\"")
                            .trim()
                            .to_string();
                        if !cleaned.is_empty() {
                            return Some(cleaned);
                        }
                    }
                }
            }
            // Keep looking for @doc before other calls
            sibling = sib.prev_sibling();
        } else if sib.kind() == "comment" {
            let text = node_text(sib, source);
            let trimmed = text.trim();
            if let Some(s) = trimmed.strip_prefix('#') {
                return Some(s.strip_prefix(' ').unwrap_or(s).to_string());
            }
            sibling = sib.prev_sibling();
        } else if sib.kind() == "unary_operator" {
            // @doc, @moduledoc, @spec are unary_operator nodes in some grammar versions
            let text = node_text(sib, source);
            if text.contains("@doc") || text.contains("@moduledoc") {
                // Try to extract the string content
                let cleaned = text.trim().to_string();
                if !cleaned.is_empty() {
                    return Some(cleaned);
                }
            }
            sibling = sib.prev_sibling();
        } else {
            break;
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Signature building
// ---------------------------------------------------------------------------

fn build_elixir_signature(def_kind: &str, name: &str, params: &[TsParam]) -> String {
    let mut sig = String::from(def_kind);
    sig.push(' ');
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
