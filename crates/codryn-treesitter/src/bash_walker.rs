//! Bash AST walker.
//!
//! Extracts: function definitions.
//! Bash is relatively simple — the main construct is function definitions.
//! Handles both `function foo { ... }` and `foo() { ... }` syntax.

use crate::{TsParam, TsSymbol};
use tree_sitter::{Node, TreeCursor};

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

/// Walk a Bash tree-sitter AST and extract symbols.
pub fn walk_tree(tree: &tree_sitter::Tree, source: &str) -> Vec<TsSymbol> {
    let mut symbols = Vec::new();
    let mut cursor = tree.walk();
    visit_children(&mut cursor, source, &mut symbols);
    symbols
}

// ---------------------------------------------------------------------------
// Recursive visitor
// ---------------------------------------------------------------------------

fn visit_children(cursor: &mut TreeCursor, source: &str, symbols: &mut Vec<TsSymbol>) {
    if !cursor.goto_first_child() {
        return;
    }
    loop {
        let node = cursor.node();
        let kind = node.kind();

        match kind {
            "function_definition" => {
                let doc = collect_preceding_comments(node, source);
                if let Some(mut sym) = extract_function(node, source) {
                    sym.docstring = doc.or(sym.docstring);
                    symbols.push(sym);
                }
                // Recurse for nested functions (rare but possible)
                visit_children(cursor, source, symbols);
            }
            _ => {
                visit_children(cursor, source, symbols);
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

fn extract_function(node: Node, source: &str) -> Option<TsSymbol> {
    let name_node = node.child_by_field_name("name")?;
    let name = node_text(name_node, source);
    if name.is_empty() {
        return None;
    }

    let body = node.child_by_field_name("body");
    let body_text = body.map(|b| node_text(b, source));

    // Bash functions don't have declared parameters — they use $1, $2, etc.
    // We can try to detect positional parameter usage in the body.
    let params = body_text
        .as_deref()
        .map(extract_positional_params)
        .unwrap_or_default();

    let signature = format!("function {}", name);

    Some(TsSymbol {
        name,
        label: "Function".into(),
        start_line: node.start_position().row as i32 + 1,
        end_line: node.end_position().row as i32 + 1,
        parent_name: None,
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
// Positional parameter detection
// ---------------------------------------------------------------------------

/// Detect positional parameters ($1, $2, ...) used in a function body.
fn extract_positional_params(body: &str) -> Vec<TsParam> {
    let mut max_param = 0u32;
    for i in 1..=9 {
        let pattern = format!("${}", i);
        let pattern_braced = format!("${{{}}}", i);
        if (body.contains(&pattern) || body.contains(&pattern_braced)) && i > max_param {
            max_param = i;
        }
    }

    (1..=max_param)
        .map(|i| TsParam {
            name: format!("${}", i),
            type_name: None,
        })
        .collect()
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
// Utility
// ---------------------------------------------------------------------------

fn node_text(node: Node, source: &str) -> String {
    source.get(node.byte_range()).unwrap_or("").to_string()
}
