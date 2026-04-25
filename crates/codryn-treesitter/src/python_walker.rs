//! Python AST walker.
//!
//! Recursively walks a tree-sitter AST and extracts symbols:
//! function definitions, async function definitions, class definitions,
//! decorated definitions, lambda expressions.  Handles nested classes,
//! inner functions, decorators, base classes, docstrings (first string
//! expression in body), type annotations on parameters and return types.

use crate::{TsParam, TsSymbol};
use tree_sitter::{Node, TreeCursor};

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

/// Walk a Python tree-sitter AST and extract symbols.
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
/// `parent_class` carries the name of the enclosing class (if any) so that
/// methods get their `parent_name` set.
fn visit_children(
    cursor: &mut TreeCursor,
    source: &str,
    symbols: &mut Vec<TsSymbol>,
    parent_class: Option<&str>,
) {
    if !cursor.goto_first_child() {
        return;
    }
    loop {
        let node = cursor.node();
        let kind = node.kind();

        match kind {
            "decorated_definition" => {
                handle_decorated_definition(cursor, source, symbols, parent_class);
            }
            "function_definition" => {
                if let Some(sym) = extract_function(node, source, parent_class, false, &[]) {
                    symbols.push(sym);
                }
                // Recurse into body for nested functions
                visit_children(cursor, source, symbols, parent_class);
            }
            "class_definition" => {
                handle_class(node, source, symbols, parent_class, &[]);
            }
            "expression_statement" => {
                // Check for lambda assignments: `foo = lambda x: x + 1`
                handle_lambda_assignment(node, source, symbols, parent_class);
                visit_children(cursor, source, symbols, parent_class);
            }
            _ => {
                visit_children(cursor, source, symbols, parent_class);
            }
        }

        if !cursor.goto_next_sibling() {
            break;
        }
    }
    cursor.goto_parent();
}

// ---------------------------------------------------------------------------
// Decorated definition handling
// ---------------------------------------------------------------------------

/// Handle a `decorated_definition` node which wraps a function or class with
/// one or more `@decorator` lines.
fn handle_decorated_definition(
    cursor: &mut TreeCursor,
    source: &str,
    symbols: &mut Vec<TsSymbol>,
    parent_class: Option<&str>,
) {
    let dec_node = cursor.node();
    let decorators = collect_decorators(dec_node, source);

    // The actual definition is the last named child (function_definition or class_definition)
    for child in dec_node.named_children(&mut dec_node.walk()) {
        match child.kind() {
            "function_definition" => {
                let is_async = false;
                if let Some(sym) =
                    extract_function(child, source, parent_class, is_async, &decorators)
                {
                    symbols.push(sym);
                }
                // Recurse into body for nested definitions
                visit_children(cursor, source, symbols, parent_class);
            }
            "class_definition" => {
                handle_class(child, source, symbols, parent_class, &decorators);
            }
            _ => {}
        }
    }
}

// ---------------------------------------------------------------------------
// Class handling
// ---------------------------------------------------------------------------

/// Extract a class and recurse into its body for methods and nested classes.
fn handle_class(
    node: Node,
    source: &str,
    symbols: &mut Vec<TsSymbol>,
    parent_class: Option<&str>,
    decorators: &[String],
) {
    if let Some(sym) = extract_class(node, source, parent_class, decorators) {
        let class_name = sym.name.clone();
        symbols.push(sym);

        // Recurse into class body with class_name as parent
        if let Some(body) = node.child_by_field_name("body") {
            let mut body_cursor = body.walk();
            visit_children(&mut body_cursor, source, symbols, Some(&class_name));
        }
    }
}

// ---------------------------------------------------------------------------
// Function extraction
// ---------------------------------------------------------------------------

/// Extract a `function_definition` node.
fn extract_function(
    node: Node,
    source: &str,
    parent_class: Option<&str>,
    _is_async_hint: bool,
    decorators: &[String],
) -> Option<TsSymbol> {
    let name_node = node.child_by_field_name("name")?;
    let name = node_text(name_node, source);
    if name.is_empty() {
        return None;
    }

    // Detect async: check if the parent or a preceding sibling is `async`
    let is_async = is_async_function(node, source);

    let params = extract_parameters(node, source);
    let return_type = extract_return_type(node, source);
    let body = node.child_by_field_name("body");
    let body_text = body.map(|b| node_text(b, source));
    let docstring = body.and_then(|b| extract_docstring(b, source));

    let is_abstract = decorators.iter().any(|d| d.contains("abstractmethod"));

    // Detect test functions: name starts with test_ or has pytest decorators
    let is_test = name.starts_with("test_")
        || decorators
            .iter()
            .any(|d| d.contains("pytest.mark") || d.contains("pytest.fixture"));

    let label = if parent_class.is_some() {
        "Method"
    } else {
        "Function"
    };
    let signature = build_function_signature(&name, &params, return_type.as_deref(), is_async);

    Some(TsSymbol {
        name,
        label: label.into(),
        start_line: node.start_position().row as i32 + 1,
        end_line: node.end_position().row as i32 + 1,
        parent_name: parent_class.map(String::from),
        signature: Some(signature),
        return_type,
        parameters: params,
        docstring,
        decorators: decorators.to_vec(),
        base_classes: Vec::new(),
        is_exported: false,
        is_abstract,
        is_async,
        is_test,
        is_entry_point: false,
        body_text,
    })
}

// ---------------------------------------------------------------------------
// Class extraction
// ---------------------------------------------------------------------------

/// Extract a `class_definition` node.
fn extract_class(
    node: Node,
    source: &str,
    parent_class: Option<&str>,
    decorators: &[String],
) -> Option<TsSymbol> {
    let name_node = node.child_by_field_name("name")?;
    let name = node_text(name_node, source);
    if name.is_empty() {
        return None;
    }

    let base_classes = extract_base_classes(node, source);
    let body = node.child_by_field_name("body");
    let body_text = body.map(|b| node_text(b, source));
    let docstring = body.and_then(|b| extract_docstring(b, source));

    let is_abstract = decorators
        .iter()
        .any(|d| d.contains("ABC") || d.contains("abstractmethod"))
        || base_classes
            .iter()
            .any(|b| b == "ABC" || b.contains("ABCMeta"));

    // Detect test classes: name starts with Test
    let is_test = name.starts_with("Test");

    let mut sig = String::from("class ");
    sig.push_str(&name);
    if !base_classes.is_empty() {
        sig.push('(');
        sig.push_str(&base_classes.join(", "));
        sig.push(')');
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
        docstring,
        decorators: decorators.to_vec(),
        base_classes,
        is_exported: false,
        is_abstract,
        is_async: false,
        is_test,
        is_entry_point: false,
        body_text,
    })
}

// ---------------------------------------------------------------------------
// Lambda assignment handling
// ---------------------------------------------------------------------------

/// Check for `name = lambda ...` patterns in expression statements.
fn handle_lambda_assignment(
    node: Node,
    source: &str,
    symbols: &mut Vec<TsSymbol>,
    parent_class: Option<&str>,
) {
    // expression_statement -> assignment -> right = lambda
    for child in node.named_children(&mut node.walk()) {
        if child.kind() == "assignment" {
            let left = child.child_by_field_name("left");
            let right = child.child_by_field_name("right");
            if let (Some(l), Some(r)) = (left, right) {
                if r.kind() == "lambda" {
                    let name = node_text(l, source);
                    if !name.is_empty() {
                        if let Some(sym) = extract_lambda(r, source, &name, parent_class) {
                            symbols.push(sym);
                        }
                    }
                }
            }
        }
    }
}

/// Extract a lambda expression given a name from its assignment.
fn extract_lambda(
    node: Node,
    source: &str,
    name: &str,
    parent_class: Option<&str>,
) -> Option<TsSymbol> {
    let params = extract_lambda_parameters(node, source);
    let body_text = node
        .child_by_field_name("body")
        .map(|b| node_text(b, source));
    let signature = build_function_signature(name, &params, None, false);

    let label = if parent_class.is_some() {
        "Method"
    } else {
        "Function"
    };

    Some(TsSymbol {
        name: name.to_string(),
        label: label.into(),
        start_line: node.start_position().row as i32 + 1,
        end_line: node.end_position().row as i32 + 1,
        parent_name: parent_class.map(String::from),
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
// Parameter extraction
// ---------------------------------------------------------------------------

/// Extract parameters from a function_definition node.
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
            "typed_parameter" => {
                // name: type
                let pname = child
                    .child_by_field_name("name")
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
            "default_parameter" => {
                // name=value
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
            "typed_default_parameter" => {
                // name: type = value
                let pname = child
                    .child_by_field_name("name")
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
            "list_splat_pattern" => {
                // *args
                let inner = child.named_child(0);
                let pname = if let Some(inner) = inner {
                    format!("*{}", node_text(inner, source))
                } else {
                    "*".to_string()
                };
                params.push(TsParam {
                    name: pname,
                    type_name: None,
                });
            }
            "dictionary_splat_pattern" => {
                // **kwargs
                let inner = child.named_child(0);
                let pname = if let Some(inner) = inner {
                    format!("**{}", node_text(inner, source))
                } else {
                    "**".to_string()
                };
                params.push(TsParam {
                    name: pname,
                    type_name: None,
                });
            }
            "tuple_pattern" => {
                // Destructured parameter
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

/// Extract parameters from a lambda node.
fn extract_lambda_parameters(node: Node, source: &str) -> Vec<TsParam> {
    // Lambda parameters are stored in a `lambda_parameters` child
    let mut params = Vec::new();
    for child in node.named_children(&mut node.walk()) {
        if child.kind() == "lambda_parameters" {
            for param in child.named_children(&mut child.walk()) {
                match param.kind() {
                    "identifier" => {
                        let pname = node_text(param, source);
                        if !pname.is_empty() {
                            params.push(TsParam {
                                name: pname,
                                type_name: None,
                            });
                        }
                    }
                    "default_parameter" => {
                        let pname = param
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
                    "list_splat_pattern" => {
                        let inner = param.named_child(0);
                        let pname = if let Some(inner) = inner {
                            format!("*{}", node_text(inner, source))
                        } else {
                            "*".to_string()
                        };
                        params.push(TsParam {
                            name: pname,
                            type_name: None,
                        });
                    }
                    "dictionary_splat_pattern" => {
                        let inner = param.named_child(0);
                        let pname = if let Some(inner) = inner {
                            format!("**{}", node_text(inner, source))
                        } else {
                            "**".to_string()
                        };
                        params.push(TsParam {
                            name: pname,
                            type_name: None,
                        });
                    }
                    _ => {}
                }
            }
            break;
        }
    }
    params
}

// ---------------------------------------------------------------------------
// Return type extraction
// ---------------------------------------------------------------------------

/// Extract the return type annotation from a function_definition node.
/// Python uses `-> type` syntax.
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
// Base class extraction
// ---------------------------------------------------------------------------

/// Extract base classes from a class_definition's argument_list.
/// e.g. `class Foo(Base, Mixin):` → `["Base", "Mixin"]`
fn extract_base_classes(node: Node, source: &str) -> Vec<String> {
    let mut bases = Vec::new();

    // In tree-sitter-python, base classes are in the `superclasses` field
    // which is an `argument_list` node
    if let Some(args) = node.child_by_field_name("superclasses") {
        for child in args.named_children(&mut args.walk()) {
            match child.kind() {
                "identifier" | "attribute" => {
                    let name = node_text(child, source);
                    if !name.is_empty() {
                        bases.push(name);
                    }
                }
                "keyword_argument" => {
                    // e.g. metaclass=ABCMeta — skip these
                }
                _ => {
                    // Could be a subscript like Generic[T] — take the text
                    let name = node_text(child, source);
                    if !name.is_empty() {
                        bases.push(name);
                    }
                }
            }
        }
    }

    bases
}

// ---------------------------------------------------------------------------
// Decorator extraction
// ---------------------------------------------------------------------------

/// Collect decorators from a `decorated_definition` node.
fn collect_decorators(node: Node, source: &str) -> Vec<String> {
    let mut decorators = Vec::new();
    for child in node.named_children(&mut node.walk()) {
        if child.kind() == "decorator" {
            let text = node_text(child, source).trim().to_string();
            decorators.push(text);
        }
    }
    decorators
}

// ---------------------------------------------------------------------------
// Docstring extraction
// ---------------------------------------------------------------------------

/// Extract a docstring from the first statement in a function/class body.
/// In Python, a docstring is the first `expression_statement` containing a
/// `string` node in the body block.
fn extract_docstring(body: Node, source: &str) -> Option<String> {
    // The body is a `block` node; look at its first named child
    let first = body.named_child(0)?;
    if first.kind() != "expression_statement" {
        return None;
    }
    let expr = first.named_child(0)?;
    if expr.kind() == "string" || expr.kind() == "concatenated_string" {
        let raw = node_text(expr, source);
        return Some(clean_docstring(&raw));
    }
    None
}

/// Clean up a Python docstring: strip triple-quote delimiters and normalize.
fn clean_docstring(raw: &str) -> String {
    let s = raw.trim();
    // Strip triple-quote delimiters (""" or ''')
    let s = s
        .strip_prefix("\"\"\"")
        .or_else(|| s.strip_prefix("'''"))
        .unwrap_or(s);
    let s = s
        .strip_suffix("\"\"\"")
        .or_else(|| s.strip_suffix("'''"))
        .unwrap_or(s);
    // Also handle single-quoted strings used as docstrings
    let s = s
        .strip_prefix('"')
        .or_else(|| s.strip_prefix('\''))
        .unwrap_or(s);
    let s = s
        .strip_suffix('"')
        .or_else(|| s.strip_suffix('\''))
        .unwrap_or(s);

    // Normalize: strip common leading whitespace
    let lines: Vec<&str> = s.lines().collect();
    if lines.is_empty() {
        return String::new();
    }

    // Find minimum indentation (ignoring empty lines and first line)
    let min_indent = lines
        .iter()
        .skip(1)
        .filter(|l| !l.trim().is_empty())
        .map(|l| l.len() - l.trim_start().len())
        .min()
        .unwrap_or(0);

    let mut result = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        if i == 0 {
            result.push(line.trim());
        } else if line.trim().is_empty() {
            result.push("");
        } else if line.len() > min_indent {
            result.push(&line[min_indent..]);
        } else {
            result.push(line.trim());
        }
    }

    // Trim leading/trailing empty lines
    while result.first() == Some(&"") {
        result.remove(0);
    }
    while result.last() == Some(&"") {
        result.pop();
    }

    result.join("\n")
}

// ---------------------------------------------------------------------------
// Async detection
// ---------------------------------------------------------------------------

/// Check if a function_definition is async.
/// In tree-sitter-python, `async def` is represented as a function_definition
/// with the parent or a preceding keyword being `async`.
fn is_async_function(node: Node, source: &str) -> bool {
    // Check if there's a preceding `async` keyword sibling
    if let Some(prev) = node.prev_sibling() {
        if prev.kind() == "async" || node_text(prev, source) == "async" {
            return true;
        }
    }
    // Also check parent: in some grammar versions, async wraps the function
    if let Some(parent) = node.parent() {
        if parent.kind() == "decorated_definition" {
            // Check siblings within the decorated_definition
            for child in parent.children(&mut parent.walk()) {
                if child.kind() == "async"
                    || (child.kind() == "function_definition" && child.id() == node.id())
                {
                    // Found the function; check if async keyword precedes it
                    break;
                }
                if node_text(child, source) == "async" {
                    return true;
                }
            }
        }
    }
    false
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
    sig.push_str("def ");
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
