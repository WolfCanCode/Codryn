//! Lua AST walker.
//!
//! Extracts: function declarations (both `function name()` and `local function name()` forms),
//! table constructors with method definitions, and module patterns.
//! Handles: nested functions, method syntax (`function t:method()`), module return patterns.

use crate::{TsParam, TsSymbol};
use tree_sitter::Node;

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

/// Walk a Lua tree-sitter AST and extract symbols.
pub fn walk_tree(tree: &tree_sitter::Tree, source: &str) -> Vec<TsSymbol> {
    let mut symbols = Vec::new();
    let root = tree.root_node();
    visit_children(root, source, &mut symbols, None);
    symbols
}

// ---------------------------------------------------------------------------
// Recursive visitor
// ---------------------------------------------------------------------------

fn visit_children(
    node: Node,
    source: &str,
    symbols: &mut Vec<TsSymbol>,
    parent_name: Option<&str>,
) {
    for i in 0..node.child_count() {
        let child = match node.child(i) {
            Some(c) => c,
            None => continue,
        };
        let kind = child.kind();

        match kind {
            "function_declaration" => {
                if let Some(sym) = extract_function_declaration(child, source, parent_name) {
                    symbols.push(sym);
                }
                // Recurse into the function body for nested definitions
                if let Some(body) = find_child_by_kind(child, "block") {
                    visit_children(body, source, symbols, parent_name);
                }
            }
            "variable_declaration" => {
                // `local M = {}` or `local f = function() end`
                extract_from_variable_declaration(child, source, symbols, parent_name);
                // Recurse into children for nested structures
                visit_children(child, source, symbols, parent_name);
            }
            "assignment_statement" => {
                // `M.foo = function() end` (top-level, not inside variable_declaration)
                // Only handle if parent is chunk/block (not inside variable_declaration)
                if node.kind() != "variable_declaration" {
                    extract_from_assignment(child, source, symbols, parent_name);
                }
                visit_children(child, source, symbols, parent_name);
            }
            _ => {
                visit_children(child, source, symbols, parent_name);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Function declaration extraction
// ---------------------------------------------------------------------------

/// Extract a function_declaration node.
/// In tree-sitter-lua, both `function name()` and `local function name()` produce
/// a `function_declaration` node. The local variant has a `local` keyword child.
fn extract_function_declaration(
    node: Node,
    source: &str,
    parent_name: Option<&str>,
) -> Option<TsSymbol> {
    let is_local = has_child_kind(node, "local");

    // The function name can be:
    // - identifier (simple name)
    // - dot_index_expression (Table.method)
    // - method_index_expression (Table:method)
    let (raw_name, _name_node) = find_function_name(node, source)?;
    if raw_name.is_empty() {
        return None;
    }

    let (name, effective_parent, label) = parse_function_name(&raw_name, parent_name);

    let params = extract_parameters_from_node(node, source);
    let body_text = find_child_by_kind(node, "block").map(|b| node_text(b, source));
    let docstring = collect_preceding_comments(node, source);

    let signature = if is_local {
        format!("local function {}", build_param_sig(&raw_name, &params))
    } else {
        format!("function {}", build_param_sig(&raw_name, &params))
    };

    // is_exported: not local, and not a private name (starting with _)
    let is_exported = !is_local && !name.starts_with('_');

    Some(TsSymbol {
        name,
        label: label.into(),
        start_line: node.start_position().row as i32 + 1,
        end_line: node.end_position().row as i32 + 1,
        parent_name: effective_parent,
        signature: Some(signature),
        return_type: None,
        parameters: params,
        docstring,
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
// Variable declaration extraction (module tables, function assignments)
// ---------------------------------------------------------------------------

fn extract_from_variable_declaration(
    node: Node,
    source: &str,
    symbols: &mut Vec<TsSymbol>,
    parent_name: Option<&str>,
) {
    // Structure: variable_declaration -> local, assignment_statement
    // assignment_statement -> variable_list, =, expression_list
    let assignment = match find_child_by_kind(node, "assignment_statement") {
        Some(a) => a,
        None => return,
    };

    let var_list = find_child_by_kind(assignment, "variable_list");
    let expr_list = find_child_by_kind(assignment, "expression_list");

    let (var_list, expr_list) = match (var_list, expr_list) {
        (Some(v), Some(e)) => (v, e),
        _ => return,
    };

    // Get variable names and their corresponding values
    let var_count = var_list.named_child_count();
    let val_count = expr_list.named_child_count();

    for idx in 0..var_count {
        let var_node = match var_list.named_child(idx) {
            Some(v) => v,
            None => continue,
        };
        let var_name = node_text(var_node, source);
        if var_name.is_empty() {
            continue;
        }

        let val_node = if idx < val_count {
            expr_list.named_child(idx)
        } else {
            None
        };

        if let Some(val) = val_node {
            match val.kind() {
                "table_constructor" => {
                    // Module pattern: `local M = {}`
                    let docstring = collect_preceding_comments(node, source);
                    symbols.push(TsSymbol {
                        name: var_name.clone(),
                        label: "Module".into(),
                        start_line: node.start_position().row as i32 + 1,
                        end_line: node.end_position().row as i32 + 1,
                        parent_name: parent_name.map(String::from),
                        signature: Some(format!("local {} = {{}}", var_name)),
                        return_type: None,
                        parameters: Vec::new(),
                        docstring,
                        decorators: Vec::new(),
                        base_classes: Vec::new(),
                        is_exported: false, // local declarations are not exported
                        is_abstract: false,
                        is_async: false,
                        is_test: false,
                        is_entry_point: false,
                        body_text: Some(node_text(val, source)),
                    });

                    // Extract methods defined inside the table constructor
                    extract_table_methods(val, source, symbols, &var_name);
                }
                "function_definition" => {
                    // Function assignment: `local f = function(...) end`
                    let params = extract_parameters_from_node(val, source);
                    let body_text = find_child_by_kind(val, "block").map(|b| node_text(b, source));
                    let signature =
                        format!("local function {}", build_param_sig(&var_name, &params));
                    let docstring = collect_preceding_comments(node, source);

                    symbols.push(TsSymbol {
                        name: var_name,
                        label: "Function".into(),
                        start_line: node.start_position().row as i32 + 1,
                        end_line: node.end_position().row as i32 + 1,
                        parent_name: parent_name.map(String::from),
                        signature: Some(signature),
                        return_type: None,
                        parameters: params,
                        docstring,
                        decorators: Vec::new(),
                        base_classes: Vec::new(),
                        is_exported: false,
                        is_abstract: false,
                        is_async: false,
                        is_test: false,
                        is_entry_point: false,
                        body_text,
                    });
                }
                _ => {}
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Assignment statement extraction (non-local: `M.foo = function() end`)
// ---------------------------------------------------------------------------

fn extract_from_assignment(
    node: Node,
    source: &str,
    symbols: &mut Vec<TsSymbol>,
    parent_name: Option<&str>,
) {
    let var_list = find_child_by_kind(node, "variable_list");
    let expr_list = find_child_by_kind(node, "expression_list");

    let (var_list, expr_list) = match (var_list, expr_list) {
        (Some(v), Some(e)) => (v, e),
        _ => return,
    };

    let var_count = var_list.named_child_count();
    let val_count = expr_list.named_child_count();

    for idx in 0..var_count {
        let var_node = match var_list.named_child(idx) {
            Some(v) => v,
            None => continue,
        };
        let var_name = node_text(var_node, source);
        if var_name.is_empty() {
            continue;
        }

        let val_node = if idx < val_count {
            expr_list.named_child(idx)
        } else {
            None
        };

        if let Some(val) = val_node {
            match val.kind() {
                "table_constructor" => {
                    // Global module pattern: `M = {}`
                    let docstring = collect_preceding_comments(node, source);
                    symbols.push(TsSymbol {
                        name: var_name.clone(),
                        label: "Module".into(),
                        start_line: node.start_position().row as i32 + 1,
                        end_line: node.end_position().row as i32 + 1,
                        parent_name: parent_name.map(String::from),
                        signature: Some(format!("{} = {{}}", var_name)),
                        return_type: None,
                        parameters: Vec::new(),
                        docstring,
                        decorators: Vec::new(),
                        base_classes: Vec::new(),
                        is_exported: true,
                        is_abstract: false,
                        is_async: false,
                        is_test: false,
                        is_entry_point: false,
                        body_text: Some(node_text(val, source)),
                    });

                    extract_table_methods(val, source, symbols, &var_name);
                }
                "function_definition" => {
                    // Function assignment: `M.foo = function(...) end`
                    let (name, effective_parent, label) =
                        parse_function_name(&var_name, parent_name);
                    let params = extract_parameters_from_node(val, source);
                    let body_text = find_child_by_kind(val, "block").map(|b| node_text(b, source));
                    let signature = format!("function {}", build_param_sig(&var_name, &params));
                    let docstring = collect_preceding_comments(node, source);

                    symbols.push(TsSymbol {
                        name,
                        label: label.into(),
                        start_line: node.start_position().row as i32 + 1,
                        end_line: node.end_position().row as i32 + 1,
                        parent_name: effective_parent,
                        signature: Some(signature),
                        return_type: None,
                        parameters: params,
                        docstring,
                        decorators: Vec::new(),
                        base_classes: Vec::new(),
                        is_exported: parent_name.is_none(),
                        is_abstract: false,
                        is_async: false,
                        is_test: false,
                        is_entry_point: false,
                        body_text,
                    });
                }
                _ => {}
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Table method extraction
// ---------------------------------------------------------------------------

fn extract_table_methods(
    table_node: Node,
    source: &str,
    symbols: &mut Vec<TsSymbol>,
    table_name: &str,
) {
    // Look for field definitions inside the table that have function values
    for i in 0..table_node.named_child_count() {
        if let Some(field) = table_node.named_child(i) {
            if field.kind() == "field" {
                // field has a name and value
                let field_name = field
                    .child_by_field_name("name")
                    .map(|n| node_text(n, source));
                let field_value = field.child_by_field_name("value");

                if let (Some(name), Some(val_node)) = (field_name, field_value) {
                    if !name.is_empty() && val_node.kind() == "function_definition" {
                        let params = extract_parameters_from_node(val_node, source);
                        let body_text =
                            find_child_by_kind(val_node, "block").map(|b| node_text(b, source));
                        let sig = format!("{}.{}", table_name, build_param_sig(&name, &params));

                        symbols.push(TsSymbol {
                            name,
                            label: "Method".into(),
                            start_line: field.start_position().row as i32 + 1,
                            end_line: field.end_position().row as i32 + 1,
                            parent_name: Some(table_name.to_string()),
                            signature: Some(sig),
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
                        });
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Parameter extraction
// ---------------------------------------------------------------------------

fn extract_parameters_from_node(node: Node, source: &str) -> Vec<TsParam> {
    let params_node = match find_child_by_kind(node, "parameters") {
        Some(n) => n,
        None => return Vec::new(),
    };

    let mut params = Vec::new();
    for i in 0..params_node.named_child_count() {
        if let Some(child) = params_node.named_child(i) {
            let name = node_text(child, source);
            // Skip `self` parameter (implicit in : syntax)
            if !name.is_empty() && name != "self" {
                params.push(TsParam {
                    name,
                    type_name: None, // Lua is dynamically typed
                });
            }
        }
    }
    params
}

// ---------------------------------------------------------------------------
// Name parsing helpers
// ---------------------------------------------------------------------------

/// Find the function name from a function_declaration node.
/// Returns the raw name string and the name node.
fn find_function_name<'a>(node: Node<'a>, source: &str) -> Option<(String, Node<'a>)> {
    // Look for identifier, dot_index_expression, or method_index_expression children
    for i in 0..node.child_count() {
        if let Some(child) = node.child(i) {
            match child.kind() {
                "identifier" => {
                    let text = node_text(child, source);
                    if !text.is_empty() && text != "function" && text != "local" {
                        return Some((text, child));
                    }
                }
                "dot_index_expression" | "method_index_expression" => {
                    let text = node_text(child, source);
                    if !text.is_empty() {
                        return Some((text, child));
                    }
                }
                _ => {}
            }
        }
    }
    None
}

/// Parse a Lua function name that may contain `.` or `:` separators.
/// Returns (simple_name, parent_name, label).
fn parse_function_name(
    raw_name: &str,
    default_parent: Option<&str>,
) -> (String, Option<String>, &'static str) {
    if let Some(colon_pos) = raw_name.rfind(':') {
        // Method syntax: `function Table:method()`
        let parent = raw_name[..colon_pos].to_string();
        let method = raw_name[colon_pos + 1..].to_string();
        (method, Some(parent), "Method")
    } else if let Some(dot_pos) = raw_name.rfind('.') {
        // Dot syntax: `function Table.method()`
        let parent = raw_name[..dot_pos].to_string();
        let method = raw_name[dot_pos + 1..].to_string();
        (method, Some(parent), "Method")
    } else {
        // Simple name
        (
            raw_name.to_string(),
            default_parent.map(String::from),
            "Function",
        )
    }
}

// ---------------------------------------------------------------------------
// Comment extraction
// ---------------------------------------------------------------------------

fn collect_preceding_comments(node: Node, source: &str) -> Option<String> {
    let mut lines = Vec::new();
    let mut sibling = node.prev_sibling();

    while let Some(sib) = sibling {
        let kind = sib.kind();
        match kind {
            "comment" => {
                let text = node_text(sib, source);
                let trimmed = text.trim();
                // Lua comments start with --
                if let Some(content) = trimmed.strip_prefix("--") {
                    // Strip additional leading dashes (e.g., --- for LuaDoc)
                    let content = content.strip_prefix('-').unwrap_or(content);
                    let content = content.strip_prefix(' ').unwrap_or(content);
                    lines.push(content.to_string());
                }
                sibling = sib.prev_sibling();
            }
            _ => break,
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

fn build_param_sig(name: &str, params: &[TsParam]) -> String {
    let param_str: Vec<&str> = params.iter().map(|p| p.name.as_str()).collect();
    format!("{}({})", name, param_str.join(", "))
}

// ---------------------------------------------------------------------------
// Utility
// ---------------------------------------------------------------------------

fn node_text(node: Node, source: &str) -> String {
    source.get(node.byte_range()).unwrap_or("").to_string()
}

fn has_child_kind(node: Node, kind: &str) -> bool {
    for i in 0..node.child_count() {
        if let Some(child) = node.child(i) {
            if child.kind() == kind {
                return true;
            }
        }
    }
    false
}

fn find_child_by_kind<'a>(node: Node<'a>, kind: &str) -> Option<Node<'a>> {
    for i in 0..node.child_count() {
        if let Some(child) = node.child(i) {
            if child.kind() == kind {
                return Some(child);
            }
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use tree_sitter::Parser;

    fn parse_lua(source: &str) -> tree_sitter::Tree {
        let mut parser = Parser::new();
        let lang: tree_sitter::Language = tree_sitter_lua::LANGUAGE.into();
        parser.set_language(&lang).unwrap();
        parser.parse(source, None).unwrap()
    }

    #[test]
    fn test_global_function() {
        let src = r#"
function greet(name)
    return "Hello, " .. name
end
"#;
        let tree = parse_lua(src);
        let symbols = walk_tree(&tree, src);

        let f = symbols
            .iter()
            .find(|s| s.name == "greet" && s.label == "Function");
        assert!(
            f.is_some(),
            "Function greet not found. Got: {:?}",
            symbols
                .iter()
                .map(|s| (&s.name, &s.label, s.start_line))
                .collect::<Vec<_>>()
        );
        let f = f.unwrap();
        assert!(f.is_exported);
        assert_eq!(f.parameters.len(), 1);
        assert_eq!(f.parameters[0].name, "name");
        assert!(f
            .signature
            .as_ref()
            .unwrap()
            .contains("function greet(name)"));
    }

    #[test]
    fn test_local_function() {
        let src = r#"
local function helper(x, y)
    return x + y
end
"#;
        let tree = parse_lua(src);
        let symbols = walk_tree(&tree, src);

        let f = symbols
            .iter()
            .find(|s| s.name == "helper" && s.label == "Function");
        assert!(
            f.is_some(),
            "Local function helper not found. Got: {:?}",
            symbols
                .iter()
                .map(|s| (&s.name, &s.label, s.start_line))
                .collect::<Vec<_>>()
        );
        let f = f.unwrap();
        assert!(!f.is_exported); // local functions are not exported
        assert_eq!(f.parameters.len(), 2);
        assert_eq!(f.parameters[0].name, "x");
        assert_eq!(f.parameters[1].name, "y");
        assert!(f
            .signature
            .as_ref()
            .unwrap()
            .contains("local function helper(x, y)"));
    }

    #[test]
    fn test_method_colon_syntax() {
        let src = r#"
function MyClass:init(name)
    self.name = name
end

function MyClass:getName()
    return self.name
end
"#;
        let tree = parse_lua(src);
        let symbols = walk_tree(&tree, src);

        let init = symbols
            .iter()
            .find(|s| s.name == "init" && s.label == "Method");
        assert!(
            init.is_some(),
            "Method init not found. Got: {:?}",
            symbols
                .iter()
                .map(|s| (&s.name, &s.label, s.start_line))
                .collect::<Vec<_>>()
        );
        let init = init.unwrap();
        assert_eq!(init.parent_name.as_deref(), Some("MyClass"));
        assert_eq!(init.parameters.len(), 1);
        assert_eq!(init.parameters[0].name, "name");

        let get_name = symbols
            .iter()
            .find(|s| s.name == "getName" && s.label == "Method");
        assert!(get_name.is_some(), "Method getName not found");
        assert_eq!(get_name.unwrap().parent_name.as_deref(), Some("MyClass"));
    }

    #[test]
    fn test_method_dot_syntax() {
        let src = r#"
function Utils.calculate(a, b)
    return a * b
end
"#;
        let tree = parse_lua(src);
        let symbols = walk_tree(&tree, src);

        let calc = symbols
            .iter()
            .find(|s| s.name == "calculate" && s.label == "Method");
        assert!(
            calc.is_some(),
            "Method calculate not found. Got: {:?}",
            symbols
                .iter()
                .map(|s| (&s.name, &s.label, s.start_line))
                .collect::<Vec<_>>()
        );
        let calc = calc.unwrap();
        assert_eq!(calc.parent_name.as_deref(), Some("Utils"));
        assert_eq!(calc.parameters.len(), 2);
    }

    #[test]
    fn test_module_pattern() {
        let src = r#"
local M = {}

function M.setup(config)
    M.config = config
end

function M:run()
    print("running")
end

return M
"#;
        let tree = parse_lua(src);
        let symbols = walk_tree(&tree, src);

        // Should find the module table
        let module = symbols
            .iter()
            .find(|s| s.name == "M" && s.label == "Module");
        assert!(
            module.is_some(),
            "Module M not found. Got: {:?}",
            symbols
                .iter()
                .map(|s| (&s.name, &s.label, s.start_line))
                .collect::<Vec<_>>()
        );
        let module = module.unwrap();
        assert!(!module.is_exported); // local module

        // Should find methods on the module
        let setup = symbols
            .iter()
            .find(|s| s.name == "setup" && s.label == "Method");
        assert!(setup.is_some(), "Method setup not found");
        assert_eq!(setup.unwrap().parent_name.as_deref(), Some("M"));

        let run = symbols
            .iter()
            .find(|s| s.name == "run" && s.label == "Method");
        assert!(run.is_some(), "Method run not found");
        assert_eq!(run.unwrap().parent_name.as_deref(), Some("M"));
    }

    #[test]
    fn test_table_constructor_with_methods() {
        let src = r#"
local Widget = {
    draw = function(self)
        print("drawing")
    end,
    resize = function(self, w, h)
        self.width = w
        self.height = h
    end
}
"#;
        let tree = parse_lua(src);
        let symbols = walk_tree(&tree, src);

        // Should find the module/table
        let widget = symbols
            .iter()
            .find(|s| s.name == "Widget" && s.label == "Module");
        assert!(
            widget.is_some(),
            "Module Widget not found. Got: {:?}",
            symbols
                .iter()
                .map(|s| (&s.name, &s.label, s.start_line))
                .collect::<Vec<_>>()
        );

        // Should find methods inside the table
        let draw = symbols
            .iter()
            .find(|s| s.name == "draw" && s.label == "Method");
        assert!(
            draw.is_some(),
            "Method draw not found. Got: {:?}",
            symbols
                .iter()
                .map(|s| (&s.name, &s.label, s.start_line))
                .collect::<Vec<_>>()
        );
        assert_eq!(draw.unwrap().parent_name.as_deref(), Some("Widget"));

        let resize = symbols
            .iter()
            .find(|s| s.name == "resize" && s.label == "Method");
        assert!(resize.is_some(), "Method resize not found");
        assert_eq!(resize.unwrap().parent_name.as_deref(), Some("Widget"));
    }

    #[test]
    fn test_comment_extraction() {
        let src = r#"
--- Calculate the sum of two numbers.
--- @param a number
--- @param b number
function add(a, b)
    return a + b
end
"#;
        let tree = parse_lua(src);
        let symbols = walk_tree(&tree, src);

        let f = symbols.iter().find(|s| s.name == "add");
        assert!(f.is_some(), "Function add not found");
        let f = f.unwrap();
        let doc = f.docstring.as_ref();
        assert!(doc.is_some(), "Expected docstring on add");
        assert!(doc.unwrap().contains("Calculate the sum"));
    }

    #[test]
    fn test_walker_output_invariants() {
        let src = r#"
function globalFn(x) return x end
local function localFn(a, b) return a + b end
function Obj:method() end
function Obj.staticMethod(x) return x end
local M = {}
function M.init() end
"#;
        let tree = parse_lua(src);
        let symbols = walk_tree(&tree, src);

        assert!(!symbols.is_empty(), "Expected at least one symbol");

        // All symbols should have valid invariants
        for sym in &symbols {
            assert!(!sym.name.is_empty(), "Symbol name should not be empty");
            assert!(
                [
                    "Function",
                    "Method",
                    "Class",
                    "Interface",
                    "Module",
                    "Impl",
                    "Enum",
                    "Constant"
                ]
                .contains(&sym.label.as_str()),
                "Invalid label '{}' for symbol '{}'",
                sym.label,
                sym.name
            );
            assert!(
                sym.start_line > 0,
                "start_line should be > 0 for {}",
                sym.name
            );
            assert!(sym.end_line > 0, "end_line should be > 0 for {}", sym.name);
            assert!(
                sym.start_line <= sym.end_line,
                "start_line ({}) should be <= end_line ({}) for {}",
                sym.start_line,
                sym.end_line,
                sym.name
            );
        }
    }
}
