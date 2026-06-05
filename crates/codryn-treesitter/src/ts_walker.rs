//! TypeScript / JavaScript / TSX AST walker.
//!
//! Recursively walks a tree-sitter AST and extracts symbols:
//! function declarations, arrow functions, classes, methods, interfaces,
//! type aliases, and enums.  Handles nested classes, inner functions,
//! closures, async/abstract modifiers, decorators, base classes, export
//! status, JSDoc comments, signatures, parameters with types, and return
//! types.

use crate::{TsParam, TsSymbol};
use tree_sitter::{Node, TreeCursor};

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

/// Walk a TypeScript/JavaScript/TSX tree-sitter AST and extract symbols.
pub fn walk_tree(tree: &tree_sitter::Tree, source: &str) -> Vec<TsSymbol> {
    let mut symbols = Vec::new();
    let mut cursor = tree.walk();
    visit_children(&mut cursor, source, &mut symbols, None, false);
    symbols
}

// ---------------------------------------------------------------------------
// Recursive visitor
// ---------------------------------------------------------------------------

/// Recursively visit children of the current cursor node.
///
/// `parent_class` carries the name of the enclosing class (if any) so that
/// methods get their `parent_name` set.
/// `is_exported` is propagated from an enclosing `export_statement`.
fn visit_children(
    cursor: &mut TreeCursor,
    source: &str,
    symbols: &mut Vec<TsSymbol>,
    parent_class: Option<&str>,
    is_exported: bool,
) {
    if !cursor.goto_first_child() {
        return;
    }
    loop {
        let node = cursor.node();
        let kind = node.kind();

        match kind {
            "export_statement" => {
                handle_export_statement(cursor, source, symbols, parent_class);
            }
            "function_declaration" => {
                let jsdoc = preceding_jsdoc(node, source);
                let decorators = collect_preceding_decorators(node, source);
                if let Some(sym) =
                    extract_function_declaration(node, source, parent_class, is_exported)
                {
                    let mut sym = sym;
                    sym.docstring = jsdoc.or(sym.docstring);
                    sym.decorators = if decorators.is_empty() {
                        sym.decorators
                    } else {
                        decorators
                    };
                    symbols.push(sym);
                }
                // Visit body for nested functions
                visit_children(cursor, source, symbols, parent_class, false);
            }
            "class_declaration" => {
                let jsdoc = preceding_jsdoc(node, source);
                let decorators = collect_preceding_decorators(node, source);
                if let Some(mut sym) =
                    extract_class_declaration(node, source, parent_class, is_exported)
                {
                    sym.docstring = jsdoc.or(sym.docstring);
                    sym.decorators = if decorators.is_empty() {
                        sym.decorators
                    } else {
                        decorators
                    };
                    let class_name = sym.name.clone();
                    symbols.push(sym);
                    // Recurse into class body with class_name as parent
                    visit_children(cursor, source, symbols, Some(&class_name), false);
                }
            }
            "abstract_class_declaration" => {
                let jsdoc = preceding_jsdoc(node, source);
                let decorators = collect_preceding_decorators(node, source);
                if let Some(mut sym) =
                    extract_class_declaration(node, source, parent_class, is_exported)
                {
                    sym.is_abstract = true;
                    sym.docstring = jsdoc.or(sym.docstring);
                    sym.decorators = if decorators.is_empty() {
                        sym.decorators
                    } else {
                        decorators
                    };
                    let class_name = sym.name.clone();
                    symbols.push(sym);
                    visit_children(cursor, source, symbols, Some(&class_name), false);
                }
            }
            "method_definition" => {
                let jsdoc = preceding_jsdoc(node, source);
                let decorators = collect_preceding_decorators(node, source);
                if let Some(mut sym) = extract_method_definition(node, source, parent_class) {
                    sym.docstring = jsdoc.or(sym.docstring);
                    sym.decorators = if decorators.is_empty() {
                        sym.decorators
                    } else {
                        decorators
                    };
                    symbols.push(sym);
                }
                // Visit body for nested functions
                visit_children(cursor, source, symbols, parent_class, false);
            }
            "interface_declaration" => {
                let jsdoc = preceding_jsdoc(node, source);
                if let Some(mut sym) = extract_interface_declaration(node, source, is_exported) {
                    sym.docstring = jsdoc.or(sym.docstring);
                    symbols.push(sym);
                }
            }
            "type_alias_declaration" => {
                let jsdoc = preceding_jsdoc(node, source);
                if let Some(mut sym) = extract_type_alias(node, source, is_exported) {
                    sym.docstring = jsdoc.or(sym.docstring);
                    symbols.push(sym);
                }
            }
            "enum_declaration" => {
                let jsdoc = preceding_jsdoc(node, source);
                if let Some(mut sym) = extract_enum_declaration(node, source, is_exported) {
                    sym.docstring = jsdoc.or(sym.docstring);
                    symbols.push(sym);
                }
            }
            // Arrow functions assigned to variables:
            //   const foo = async () => { ... }
            //   let bar = () => expr
            "lexical_declaration" | "variable_declaration" => {
                handle_variable_declaration(node, source, symbols, parent_class, is_exported);
                // Don't recurse further – we already inspected declarators
            }
            // Catch arrow functions that appear as expression_statement
            // (e.g. module.exports = () => {})
            "expression_statement" => {
                handle_expression_statement(node, source, symbols, parent_class, is_exported);
            }
            _ => {
                // Recurse into other nodes (e.g. statement_block, program)
                visit_children(cursor, source, symbols, parent_class, is_exported);
            }
        }

        if !cursor.goto_next_sibling() {
            break;
        }
    }
    cursor.goto_parent();
}

// ---------------------------------------------------------------------------
// Export statement handling
// ---------------------------------------------------------------------------

fn handle_export_statement(
    cursor: &mut TreeCursor,
    source: &str,
    symbols: &mut Vec<TsSymbol>,
    parent_class: Option<&str>,
) {
    let export_node = cursor.node();
    let is_default = export_node
        .children(&mut export_node.walk())
        .any(|c| c.kind() == "default");

    // Walk children of the export_statement with is_exported = true
    if cursor.goto_first_child() {
        loop {
            let child = cursor.node();
            let kind = child.kind();
            match kind {
                "function_declaration" => {
                    let jsdoc = preceding_jsdoc(export_node, source);
                    let decorators = collect_preceding_decorators(export_node, source);
                    if let Some(mut sym) =
                        extract_function_declaration(child, source, parent_class, true)
                    {
                        sym.docstring = jsdoc.or(sym.docstring);
                        sym.decorators = if decorators.is_empty() {
                            sym.decorators
                        } else {
                            decorators
                        };
                        symbols.push(sym);
                    }
                    visit_children(cursor, source, symbols, parent_class, false);
                }
                "class_declaration" | "abstract_class_declaration" => {
                    let jsdoc = preceding_jsdoc(export_node, source);
                    let decorators = collect_preceding_decorators(export_node, source);
                    if let Some(mut sym) =
                        extract_class_declaration(child, source, parent_class, true)
                    {
                        if kind == "abstract_class_declaration" {
                            sym.is_abstract = true;
                        }
                        sym.docstring = jsdoc.or(sym.docstring);
                        sym.decorators = if decorators.is_empty() {
                            sym.decorators
                        } else {
                            decorators
                        };
                        let class_name = sym.name.clone();
                        symbols.push(sym);
                        visit_children(cursor, source, symbols, Some(&class_name), false);
                    }
                }
                "interface_declaration" => {
                    let jsdoc = preceding_jsdoc(export_node, source);
                    if let Some(mut sym) = extract_interface_declaration(child, source, true) {
                        sym.docstring = jsdoc.or(sym.docstring);
                        symbols.push(sym);
                    }
                }
                "type_alias_declaration" => {
                    let jsdoc = preceding_jsdoc(export_node, source);
                    if let Some(mut sym) = extract_type_alias(child, source, true) {
                        sym.docstring = jsdoc.or(sym.docstring);
                        symbols.push(sym);
                    }
                }
                "enum_declaration" => {
                    let jsdoc = preceding_jsdoc(export_node, source);
                    if let Some(mut sym) = extract_enum_declaration(child, source, true) {
                        sym.docstring = jsdoc.or(sym.docstring);
                        symbols.push(sym);
                    }
                }
                "lexical_declaration" | "variable_declaration" => {
                    handle_variable_declaration(child, source, symbols, parent_class, true);
                }
                _ => {
                    // e.g. `export default function() {}` — anonymous default export
                    if kind == "function" || kind == "arrow_function" {
                        let name = if is_default {
                            "default".to_string()
                        } else {
                            String::new()
                        };
                        if !name.is_empty() {
                            if let Some(mut sym) = extract_arrow_or_anon_function(
                                child,
                                source,
                                &name,
                                parent_class,
                                true,
                            ) {
                                sym.docstring = preceding_jsdoc(export_node, source);
                                symbols.push(sym);
                            }
                        }
                    }
                }
            }
            if !cursor.goto_next_sibling() {
                break;
            }
        }
        cursor.goto_parent();
    }
}

// ---------------------------------------------------------------------------
// Variable declaration → arrow function extraction
// ---------------------------------------------------------------------------

fn handle_variable_declaration(
    node: Node,
    source: &str,
    symbols: &mut Vec<TsSymbol>,
    parent_class: Option<&str>,
    is_exported: bool,
) {
    for child in node.children(&mut node.walk()) {
        if child.kind() == "variable_declarator" {
            let name_node = child.child_by_field_name("name");
            let value_node = child.child_by_field_name("value");
            if let (Some(name_n), Some(val_n)) = (name_node, value_node) {
                let val_kind = val_n.kind();
                if val_kind == "arrow_function"
                    || val_kind == "function"
                    || val_kind == "function_expression"
                {
                    let name = node_text(name_n, source);
                    let jsdoc = preceding_jsdoc(node, source);
                    if let Some(mut sym) = extract_arrow_or_anon_function(
                        val_n,
                        source,
                        &name,
                        parent_class,
                        is_exported,
                    ) {
                        sym.docstring = jsdoc.or(sym.docstring);
                        symbols.push(sym);
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Expression statement → assignment arrow functions
// ---------------------------------------------------------------------------

fn handle_expression_statement(
    node: Node,
    source: &str,
    symbols: &mut Vec<TsSymbol>,
    parent_class: Option<&str>,
    is_exported: bool,
) {
    // e.g. module.exports = () => {}  or  exports.foo = function() {}
    for child in node.children(&mut node.walk()) {
        if child.kind() == "assignment_expression" {
            let right = child.child_by_field_name("right");
            if let Some(r) = right {
                let rk = r.kind();
                if rk == "arrow_function" || rk == "function" || rk == "function_expression" {
                    let left = child.child_by_field_name("left");
                    if let Some(l) = left {
                        let name = extract_assignment_name(l, source);
                        if !name.is_empty() {
                            let jsdoc = preceding_jsdoc(node, source);
                            if let Some(mut sym) = extract_arrow_or_anon_function(
                                r,
                                source,
                                &name,
                                parent_class,
                                is_exported,
                            ) {
                                sym.docstring = jsdoc.or(sym.docstring);
                                symbols.push(sym);
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Try to extract a meaningful name from the left side of an assignment.
fn extract_assignment_name(node: Node, source: &str) -> String {
    match node.kind() {
        "identifier" => node_text(node, source),
        "member_expression" => {
            // e.g. module.exports or exports.foo → take the last property
            if let Some(prop) = node.child_by_field_name("property") {
                node_text(prop, source)
            } else {
                node_text(node, source)
            }
        }
        _ => node_text(node, source),
    }
}

// ---------------------------------------------------------------------------
// Symbol extractors
// ---------------------------------------------------------------------------

/// Extract a `function_declaration` node.
fn extract_function_declaration(
    node: Node,
    source: &str,
    parent_class: Option<&str>,
    is_exported: bool,
) -> Option<TsSymbol> {
    let name_node = node.child_by_field_name("name")?;
    let name = node_text(name_node, source);
    if name.is_empty() {
        return None;
    }

    let is_async = node.children(&mut node.walk()).any(|c| c.kind() == "async");

    let params = extract_parameters(node, source);
    let return_type = extract_return_type(node, source);
    let body = node.child_by_field_name("body");
    let body_text = body.map(|b| node_text(b, source));
    let signature = build_function_signature(&name, &params, return_type.as_deref(), is_async);

    Some(TsSymbol {
        name,
        label: "Function".into(),
        start_line: node.start_position().row as i32 + 1,
        end_line: node.end_position().row as i32 + 1,
        parent_name: parent_class.map(String::from),
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

/// Extract an arrow function or anonymous function expression given a name.
fn extract_arrow_or_anon_function(
    node: Node,
    source: &str,
    name: &str,
    parent_class: Option<&str>,
    is_exported: bool,
) -> Option<TsSymbol> {
    if name.is_empty() {
        return None;
    }

    let is_async = node.children(&mut node.walk()).any(|c| c.kind() == "async");

    let params = extract_parameters(node, source);
    let return_type = extract_return_type(node, source);
    let body = node.child_by_field_name("body");
    let body_text = body.map(|b| node_text(b, source));
    let signature = build_function_signature(name, &params, return_type.as_deref(), is_async);

    Some(TsSymbol {
        name: name.to_string(),
        label: "Function".into(),
        start_line: node.start_position().row as i32 + 1,
        end_line: node.end_position().row as i32 + 1,
        parent_name: parent_class.map(String::from),
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

/// Extract a `class_declaration` (or `abstract_class_declaration`) node.
fn extract_class_declaration(
    node: Node,
    source: &str,
    parent_class: Option<&str>,
    is_exported: bool,
) -> Option<TsSymbol> {
    let name_node = node.child_by_field_name("name")?;
    let name = node_text(name_node, source);
    if name.is_empty() {
        return None;
    }

    let is_abstract = node.kind() == "abstract_class_declaration"
        || node
            .children(&mut node.walk())
            .any(|c| c.kind() == "abstract");

    let base_classes = extract_heritage(node, source);
    let decorators = collect_child_decorators(node, source);
    let body = node.child_by_field_name("body");
    let body_text = body.map(|b| node_text(b, source));

    let mut sig = String::new();
    if is_abstract {
        sig.push_str("abstract ");
    }
    sig.push_str("class ");
    sig.push_str(&name);
    if !base_classes.is_empty() {
        sig.push_str(" extends ");
        sig.push_str(&base_classes.join(", "));
    }

    Some(TsSymbol {
        name,
        label: "Class".into(),
        start_line: node.start_position().row as i32 + 1,
        end_line: node.end_position().row as i32 + 1,
        parent_name: parent_class.map(String::from),
        signature: Some(sig),
        return_type: None,
        parameters: Vec::new(),
        docstring: None,
        decorators,
        base_classes,
        is_exported,
        is_abstract,
        is_async: false,
        is_test: false,
        is_entry_point: false,
        body_text,
    })
}

/// Extract a `method_definition` node.
fn extract_method_definition(
    node: Node,
    source: &str,
    parent_class: Option<&str>,
) -> Option<TsSymbol> {
    let name_node = node.child_by_field_name("name")?;
    let name = node_text(name_node, source);
    if name.is_empty() {
        return None;
    }

    let is_async = node.children(&mut node.walk()).any(|c| c.kind() == "async");
    let is_abstract = node
        .children(&mut node.walk())
        .any(|c| node_text(c, source) == "abstract");

    // Check for static, get, set keywords
    let is_static = node
        .children(&mut node.walk())
        .any(|c| c.kind() == "static");
    let is_getter = node.children(&mut node.walk()).any(|c| c.kind() == "get");
    let is_setter = node.children(&mut node.walk()).any(|c| c.kind() == "set");

    let params = extract_parameters(node, source);
    let return_type = extract_return_type(node, source);
    let body = node.child_by_field_name("body");
    let body_text = body.map(|b| node_text(b, source));

    let mut sig = String::new();
    if is_static {
        sig.push_str("static ");
    }
    if is_async {
        sig.push_str("async ");
    }
    if is_getter {
        sig.push_str("get ");
    }
    if is_setter {
        sig.push_str("set ");
    }
    sig.push_str(&name);
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
    if let Some(ref rt) = return_type {
        sig.push_str(": ");
        sig.push_str(rt);
    }

    Some(TsSymbol {
        name,
        label: "Method".into(),
        start_line: node.start_position().row as i32 + 1,
        end_line: node.end_position().row as i32 + 1,
        parent_name: parent_class.map(String::from),
        signature: Some(sig),
        return_type,
        parameters: params,
        docstring: None,
        decorators: Vec::new(),
        base_classes: Vec::new(),
        is_exported: false,
        is_abstract,
        is_async,
        is_test: false,
        is_entry_point: false,
        body_text,
    })
}

/// Extract an `interface_declaration` node.
fn extract_interface_declaration(node: Node, source: &str, is_exported: bool) -> Option<TsSymbol> {
    let name_node = node.child_by_field_name("name")?;
    let name = node_text(name_node, source);
    if name.is_empty() {
        return None;
    }

    let base_classes = extract_extends_clause(node, source);
    let body = node.child_by_field_name("body");
    let body_text = body.map(|b| node_text(b, source));

    let mut sig = String::from("interface ");
    sig.push_str(&name);
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
        is_exported,
        is_abstract: false,
        is_async: false,
        is_test: false,
        is_entry_point: false,
        body_text,
    })
}

/// Extract a `type_alias_declaration` node.
fn extract_type_alias(node: Node, source: &str, is_exported: bool) -> Option<TsSymbol> {
    let name_node = node.child_by_field_name("name")?;
    let name = node_text(name_node, source);
    if name.is_empty() {
        return None;
    }

    let value_node = node.child_by_field_name("value");
    let body_text = value_node.map(|v| node_text(v, source));

    let sig = format!(
        "type {}",
        node_text(node, source).lines().next().unwrap_or(&name)
    );

    Some(TsSymbol {
        name,
        label: "TypeAlias".into(),
        start_line: node.start_position().row as i32 + 1,
        end_line: node.end_position().row as i32 + 1,
        parent_name: None,
        signature: Some(sig),
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

/// Extract an `enum_declaration` node.
fn extract_enum_declaration(node: Node, source: &str, is_exported: bool) -> Option<TsSymbol> {
    let name_node = node.child_by_field_name("name")?;
    let name = node_text(name_node, source);
    if name.is_empty() {
        return None;
    }

    let body = node.child_by_field_name("body");
    let body_text = body.map(|b| node_text(b, source));

    let sig = format!("enum {}", name);

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

/// Extract parameters from a function/method/arrow node.
fn extract_parameters(node: Node, source: &str) -> Vec<TsParam> {
    let params_node = node.child_by_field_name("parameters");
    let params_node = match params_node {
        Some(n) => n,
        None => return Vec::new(),
    };

    let mut params = Vec::new();
    for child in params_node.children(&mut params_node.walk()) {
        match child.kind() {
            "required_parameter" | "optional_parameter" => {
                let pname = child
                    .child_by_field_name("pattern")
                    .or_else(|| child.child_by_field_name("name"))
                    .map(|n| node_text(n, source))
                    .unwrap_or_default();
                let ptype = child
                    .child_by_field_name("type")
                    .map(|t| extract_type_annotation(t, source));
                if !pname.is_empty() {
                    params.push(TsParam {
                        name: pname,
                        type_name: ptype,
                    });
                }
            }
            // Plain JS parameters (identifier, assignment_pattern, rest_pattern, etc.)
            "identifier" => {
                let pname = node_text(child, source);
                if !pname.is_empty() {
                    params.push(TsParam {
                        name: pname,
                        type_name: None,
                    });
                }
            }
            "assignment_pattern" => {
                if let Some(left) = child.child_by_field_name("left") {
                    let pname = node_text(left, source);
                    if !pname.is_empty() {
                        params.push(TsParam {
                            name: pname,
                            type_name: None,
                        });
                    }
                }
            }
            "rest_pattern" | "rest_element" => {
                // ...args
                let inner = child.named_child(0);
                if let Some(inner) = inner {
                    let pname = format!("...{}", node_text(inner, source));
                    params.push(TsParam {
                        name: pname,
                        type_name: None,
                    });
                }
            }
            "object_pattern" | "array_pattern" => {
                // Destructured parameter — use the full text as name
                let pname = node_text(child, source);
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

/// Extract the return type annotation from a function/method node.
fn extract_return_type(node: Node, source: &str) -> Option<String> {
    node.child_by_field_name("return_type")
        .map(|rt| extract_type_annotation(rt, source))
}

/// Extract the text of a type annotation node, stripping the leading `:` if present.
fn extract_type_annotation(node: Node, source: &str) -> String {
    let text = node_text(node, source);
    let trimmed = text.trim();
    if let Some(stripped) = trimmed.strip_prefix(':') {
        stripped.trim().to_string()
    } else {
        trimmed.to_string()
    }
}

// ---------------------------------------------------------------------------
// Heritage (extends / implements) extraction
// ---------------------------------------------------------------------------

/// Extract base classes from `extends` and `implements` clauses on a class.
fn extract_heritage(node: Node, source: &str) -> Vec<String> {
    let mut bases = Vec::new();

    for child in node.children(&mut node.walk()) {
        match child.kind() {
            // TypeScript: class_heritage contains extends_clause and implements_clause
            "class_heritage" => {
                for hc in child.children(&mut child.walk()) {
                    match hc.kind() {
                        "extends_clause" | "implements_clause" => {
                            collect_type_names_from_clause(hc, source, &mut bases);
                        }
                        _ => {}
                    }
                }
            }
            // Direct extends_clause / implements_clause (some grammar versions)
            "extends_clause" | "implements_clause" => {
                collect_type_names_from_clause(child, source, &mut bases);
            }
            _ => {}
        }
    }
    bases
}

/// Extract type names from an extends/implements clause on an interface.
fn extract_extends_clause(node: Node, source: &str) -> Vec<String> {
    let mut bases = Vec::new();
    for child in node.children(&mut node.walk()) {
        if child.kind() == "extends_type_clause" || child.kind() == "extends_clause" {
            collect_type_names_from_clause(child, source, &mut bases);
        }
    }
    bases
}

/// Collect type identifier names from an extends/implements clause node.
fn collect_type_names_from_clause(clause: Node, source: &str, out: &mut Vec<String>) {
    for child in clause.named_children(&mut clause.walk()) {
        match child.kind() {
            "type_identifier" | "identifier" | "nested_type_identifier" => {
                let name = node_text(child, source);
                if !name.is_empty() {
                    out.push(name);
                }
            }
            "generic_type" => {
                // e.g. Base<T> — take the type name without generics
                if let Some(name_node) = child.child_by_field_name("name") {
                    let name = node_text(name_node, source);
                    if !name.is_empty() {
                        out.push(name);
                    }
                }
            }
            _ => {}
        }
    }
}

// ---------------------------------------------------------------------------
// Decorator extraction
// ---------------------------------------------------------------------------

/// Collect decorators that appear as preceding siblings of a node.
fn collect_preceding_decorators(node: Node, source: &str) -> Vec<String> {
    let mut decorators = Vec::new();
    let mut sibling = node.prev_sibling();
    while let Some(s) = sibling {
        if s.kind() == "decorator" {
            let text = node_text(s, source).trim().to_string();
            decorators.push(text);
            sibling = s.prev_sibling();
        } else {
            break;
        }
    }
    decorators.reverse();
    decorators
}

/// Collect decorators that are direct children of a node (e.g. inside a class body).
fn collect_child_decorators(node: Node, source: &str) -> Vec<String> {
    let mut decorators = Vec::new();
    for child in node.children(&mut node.walk()) {
        if child.kind() == "decorator" {
            let text = node_text(child, source).trim().to_string();
            decorators.push(text);
        }
    }
    decorators
}

// ---------------------------------------------------------------------------
// JSDoc extraction
// ---------------------------------------------------------------------------

/// Look for a JSDoc comment (`/** ... */`) immediately preceding the given node.
fn preceding_jsdoc(node: Node, source: &str) -> Option<String> {
    let mut sibling = node.prev_sibling();
    // Skip over decorator nodes to find the comment
    while let Some(s) = sibling {
        if s.kind() == "decorator" {
            sibling = s.prev_sibling();
            continue;
        }
        break;
    }
    let s = sibling?;
    if s.kind() == "comment" {
        let text = node_text(s, source);
        if text.starts_with("/**") {
            return Some(clean_jsdoc(&text));
        }
    }
    None
}

/// Clean up a JSDoc comment: strip `/**`, `*/`, and leading `*` on each line.
fn clean_jsdoc(raw: &str) -> String {
    let stripped = raw
        .strip_prefix("/**")
        .unwrap_or(raw)
        .strip_suffix("*/")
        .unwrap_or(raw);
    stripped
        .lines()
        .map(|line| {
            let trimmed = line.trim();
            if let Some(rest) = trimmed.strip_prefix('*') {
                rest.trim()
            } else {
                trimmed
            }
        })
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

// ---------------------------------------------------------------------------
// Signature building
// ---------------------------------------------------------------------------

fn build_function_signature(
    name: &str,
    params: &[TsParam],
    return_type: Option<&str>,
    is_async: bool,
) -> String {
    let mut sig = String::new();
    if is_async {
        sig.push_str("async ");
    }
    sig.push_str("function ");
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

/// Get the UTF-8 text of a tree-sitter node.
fn node_text(node: Node, source: &str) -> String {
    source.get(node.byte_range()).unwrap_or("").to_string()
}
