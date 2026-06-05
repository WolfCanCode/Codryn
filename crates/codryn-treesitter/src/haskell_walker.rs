//! Haskell AST walker.
//!
//! Extracts: module declarations, function definitions (with type signatures),
//! type declarations (data, newtype, type synonyms), type class declarations,
//! type class instances, and data constructors.

use crate::{TsParam, TsSymbol};
use tree_sitter::Node;

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

/// Walk a Haskell tree-sitter AST and extract symbols.
pub fn walk_tree(tree: &tree_sitter::Tree, source: &str) -> Vec<TsSymbol> {
    let mut symbols = Vec::new();
    let root = tree.root_node();

    // Extract module declaration from header
    extract_module_header(root, source, &mut symbols);

    // Walk declarations
    for i in 0..root.child_count() {
        let child = match root.child(i) {
            Some(c) => c,
            None => continue,
        };
        if child.kind() == "declarations" {
            visit_declarations(child, source, &mut symbols);
        }
    }

    symbols
}

// ---------------------------------------------------------------------------
// Module header extraction
// ---------------------------------------------------------------------------

fn extract_module_header(root: Node, source: &str, symbols: &mut Vec<TsSymbol>) {
    for i in 0..root.child_count() {
        let child = match root.child(i) {
            Some(c) => c,
            None => continue,
        };
        if child.kind() == "header" {
            // header contains: module keyword, module name node, where keyword
            for hi in 0..child.child_count() {
                let hc = match child.child(hi) {
                    Some(c) => c,
                    None => continue,
                };
                if hc.kind() == "module" && hc.child_count() > 0 {
                    // This is the module name node (contains module_id children)
                    let module_name = node_text(hc, source);
                    if !module_name.is_empty() && module_name != "module" {
                        symbols.push(TsSymbol {
                            name: module_name.clone(),
                            label: "Module".into(),
                            start_line: child.start_position().row as i32 + 1,
                            end_line: child.end_position().row as i32 + 1,
                            parent_name: None,
                            signature: Some(format!("module {} where", module_name)),
                            return_type: None,
                            parameters: Vec::new(),
                            docstring: None,
                            decorators: Vec::new(),
                            base_classes: Vec::new(),
                            is_exported: true,
                            is_abstract: false,
                            is_async: false,
                            is_test: false,
                            is_entry_point: false,
                            body_text: None,
                        });
                        return;
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Declarations visitor
// ---------------------------------------------------------------------------

fn visit_declarations(node: Node, source: &str, symbols: &mut Vec<TsSymbol>) {
    // Collect type signatures so we can attach them to function definitions
    let mut signatures: Vec<(String, String)> = Vec::new();

    // First pass: collect all type signatures
    for i in 0..node.child_count() {
        let child = match node.child(i) {
            Some(c) => c,
            None => continue,
        };
        if child.kind() == "signature" {
            if let Some((name, sig_text)) = extract_type_signature(child, source) {
                signatures.push((name, sig_text));
            }
        }
    }

    // Also look for haddock comments that are siblings of the declarations node
    // (they appear at the root level, before the declarations node)
    let haddock_before_decls = collect_root_haddocks(node, source);

    // Second pass: extract declarations
    for i in 0..node.child_count() {
        let child = match node.child(i) {
            Some(c) => c,
            None => continue,
        };
        match child.kind() {
            "function" => {
                extract_function(
                    child,
                    source,
                    symbols,
                    &signatures,
                    None,
                    &haddock_before_decls,
                );
            }
            "bind" => {
                // `bind` is used for functions with no parameters (e.g., `main = ...`)
                extract_bind(child, source, symbols, &signatures, &haddock_before_decls);
            }
            "data_type" => {
                extract_data_type(child, source, symbols);
            }
            "class" => {
                extract_type_class(child, source, symbols);
            }
            "instance" => {
                extract_instance(child, source, symbols);
            }
            "type_synomym" => {
                // Note: tree-sitter-haskell has a typo in the node kind
                extract_type_synonym(child, source, symbols);
            }
            "newtype" => {
                extract_newtype(child, source, symbols);
            }
            _ => {}
        }
    }
}

/// Collect haddock comments that appear before the declarations node at root level.
/// Returns a map of first-declaration-line -> haddock text.
fn collect_root_haddocks(decl_node: Node, source: &str) -> Vec<(i32, String)> {
    let mut haddocks = Vec::new();
    // The haddock is a sibling of the declarations node in the root
    if let Some(parent) = decl_node.parent() {
        for i in 0..parent.child_count() {
            let child = match parent.child(i) {
                Some(c) => c,
                None => continue,
            };
            if child.kind() == "haddock" || child.kind() == "comment" {
                let text = node_text(child, source);
                let trimmed = text.trim();
                let content = if let Some(c) = trimmed.strip_prefix("-- |") {
                    c.trim().to_string()
                } else if let Some(c) = trimmed.strip_prefix("-- ^") {
                    c.trim().to_string()
                } else {
                    continue;
                };
                // The haddock applies to the next declaration after it
                // Store the line number of the haddock
                let haddock_line = child.end_position().row as i32 + 1;
                haddocks.push((haddock_line, content));
            }
        }
    }
    haddocks
}

// ---------------------------------------------------------------------------
// Type signature extraction
// ---------------------------------------------------------------------------

fn extract_type_signature(node: Node, source: &str) -> Option<(String, String)> {
    // signature: variable :: type
    let mut name = String::new();

    for i in 0..node.child_count() {
        let child = match node.child(i) {
            Some(c) => c,
            None => continue,
        };
        if child.kind() == "variable" && name.is_empty() {
            name = node_text(child, source);
        }
    }

    if name.is_empty() {
        return None;
    }

    let sig_text = node_text(node, source);
    Some((name, sig_text))
}

// ---------------------------------------------------------------------------
// Function extraction
// ---------------------------------------------------------------------------

fn extract_function(
    node: Node,
    source: &str,
    symbols: &mut Vec<TsSymbol>,
    signatures: &[(String, String)],
    parent_name: Option<&str>,
    root_haddocks: &[(i32, String)],
) {
    // function: variable patterns match
    let mut name = String::new();
    let mut params = Vec::new();

    for i in 0..node.child_count() {
        let child = match node.child(i) {
            Some(c) => c,
            None => continue,
        };
        match child.kind() {
            "variable" if name.is_empty() => {
                name = node_text(child, source);
            }
            "patterns" => {
                params = extract_patterns_as_params(child, source);
            }
            _ => {}
        }
    }

    if name.is_empty() {
        return;
    }

    // Check if we already have this function (multiple equations for same function)
    // Only emit the first equation
    if symbols.iter().any(|s| {
        s.name == name && s.label == "Function" && s.parent_name == parent_name.map(String::from)
    }) {
        return;
    }

    // Find matching type signature
    let signature = signatures
        .iter()
        .find(|(sig_name, _)| sig_name == &name)
        .map(|(_, sig_text)| sig_text.clone());

    // Extract return type from signature if available
    let return_type = signature.as_ref().and_then(|sig| extract_return_type(sig));

    // Try to get haddock from preceding siblings first, then from root haddocks
    let docstring = collect_preceding_haddock(node, source)
        .or_else(|| find_root_haddock_for_node(node, source, root_haddocks));

    let body_text = Some(node_text(node, source));

    let label = if parent_name.is_some() {
        "Method"
    } else {
        "Function"
    };
    let is_entry_point = name == "main" && parent_name.is_none();

    symbols.push(TsSymbol {
        name,
        label: label.into(),
        start_line: node.start_position().row as i32 + 1,
        end_line: node.end_position().row as i32 + 1,
        parent_name: parent_name.map(String::from),
        signature,
        return_type,
        parameters: params,
        docstring,
        decorators: Vec::new(),
        base_classes: Vec::new(),
        is_exported: true, // Haskell exports are controlled by module export list
        is_abstract: false,
        is_async: false,
        is_test: false,
        is_entry_point,
        body_text,
    });
}

// ---------------------------------------------------------------------------
// Bind extraction (functions with no parameters)
// ---------------------------------------------------------------------------

fn extract_bind(
    node: Node,
    source: &str,
    symbols: &mut Vec<TsSymbol>,
    signatures: &[(String, String)],
    root_haddocks: &[(i32, String)],
) {
    // bind: variable match
    let mut name = String::new();

    for i in 0..node.child_count() {
        let child = match node.child(i) {
            Some(c) => c,
            None => continue,
        };
        if child.kind() == "variable" && name.is_empty() {
            name = node_text(child, source);
        }
    }

    if name.is_empty() {
        return;
    }

    // Check if we already have this function
    if symbols
        .iter()
        .any(|s| s.name == name && s.label == "Function")
    {
        return;
    }

    // Find matching type signature
    let signature = signatures
        .iter()
        .find(|(sig_name, _)| sig_name == &name)
        .map(|(_, sig_text)| sig_text.clone());

    let return_type = signature.as_ref().and_then(|sig| extract_return_type(sig));

    let docstring = collect_preceding_haddock(node, source)
        .or_else(|| find_root_haddock_for_node(node, source, root_haddocks));

    let body_text = Some(node_text(node, source));
    let is_entry_point = name == "main";

    symbols.push(TsSymbol {
        name,
        label: "Function".into(),
        start_line: node.start_position().row as i32 + 1,
        end_line: node.end_position().row as i32 + 1,
        parent_name: None,
        signature,
        return_type,
        parameters: Vec::new(),
        docstring,
        decorators: Vec::new(),
        base_classes: Vec::new(),
        is_exported: true,
        is_abstract: false,
        is_async: false,
        is_test: false,
        is_entry_point,
        body_text,
    });
}

// ---------------------------------------------------------------------------
// Data type extraction
// ---------------------------------------------------------------------------

fn extract_data_type(node: Node, source: &str, symbols: &mut Vec<TsSymbol>) {
    let mut name = String::new();
    let mut constructors: Vec<String> = Vec::new();

    for i in 0..node.child_count() {
        let child = match node.child(i) {
            Some(c) => c,
            None => continue,
        };
        match child.kind() {
            "name" if name.is_empty() => {
                name = node_text(child, source);
            }
            "data_constructors" => {
                extract_data_constructors(child, source, &mut constructors);
            }
            _ => {}
        }
    }

    if name.is_empty() {
        return;
    }

    let sig_text = node_text(node, source);
    let docstring = collect_preceding_haddock(node, source);

    // Emit the data type itself as a Class (type-level construct)
    symbols.push(TsSymbol {
        name: name.clone(),
        label: "Class".into(),
        start_line: node.start_position().row as i32 + 1,
        end_line: node.end_position().row as i32 + 1,
        parent_name: None,
        signature: Some(sig_text),
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
        body_text: Some(node_text(node, source)),
    });

    // Emit each data constructor as a Constant
    for ctor in constructors {
        symbols.push(TsSymbol {
            name: ctor.clone(),
            label: "Constant".into(),
            start_line: node.start_position().row as i32 + 1,
            end_line: node.end_position().row as i32 + 1,
            parent_name: Some(name.clone()),
            signature: Some(format!("{} (constructor of {})", ctor, name)),
            return_type: Some(name.clone()),
            parameters: Vec::new(),
            docstring: None,
            decorators: Vec::new(),
            base_classes: Vec::new(),
            is_exported: true,
            is_abstract: false,
            is_async: false,
            is_test: false,
            is_entry_point: false,
            body_text: None,
        });
    }
}

fn extract_data_constructors(node: Node, source: &str, constructors: &mut Vec<String>) {
    for i in 0..node.child_count() {
        let child = match node.child(i) {
            Some(c) => c,
            None => continue,
        };
        if child.kind() == "data_constructor" {
            // data_constructor -> prefix -> constructor
            if let Some(ctor_name) = find_constructor_name(child, source) {
                constructors.push(ctor_name);
            }
        }
    }
}

fn find_constructor_name(node: Node, source: &str) -> Option<String> {
    for i in 0..node.child_count() {
        let child = match node.child(i) {
            Some(c) => c,
            None => continue,
        };
        match child.kind() {
            "prefix" => {
                // prefix -> constructor
                for j in 0..child.child_count() {
                    if let Some(gc) = child.child(j) {
                        if gc.kind() == "constructor" {
                            return Some(node_text(gc, source));
                        }
                    }
                }
            }
            "constructor" => {
                return Some(node_text(child, source));
            }
            _ => {}
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Type class extraction
// ---------------------------------------------------------------------------

fn extract_type_class(node: Node, source: &str, symbols: &mut Vec<TsSymbol>) {
    let mut name = String::new();
    let mut base_classes = Vec::new();

    for i in 0..node.child_count() {
        let child = match node.child(i) {
            Some(c) => c,
            None => continue,
        };
        match child.kind() {
            "name"
                if name.is_empty() => {
                    name = node_text(child, source);
                }
            "context" => {
                // Extract superclass constraints
                extract_context_classes(child, source, &mut base_classes);
            }
            "class_declarations"
                // Extract method signatures from the class body
                if !name.is_empty() => {
                    extract_class_methods(child, source, symbols, &name);
                }
            _ => {}
        }
    }

    if name.is_empty() {
        return;
    }

    let sig_text = node_text(node, source);
    let docstring = collect_preceding_haddock(node, source);

    symbols.push(TsSymbol {
        name,
        label: "Interface".into(),
        start_line: node.start_position().row as i32 + 1,
        end_line: node.end_position().row as i32 + 1,
        parent_name: None,
        signature: Some(sig_text),
        return_type: None,
        parameters: Vec::new(),
        docstring,
        decorators: Vec::new(),
        base_classes,
        is_exported: true,
        is_abstract: true,
        is_async: false,
        is_test: false,
        is_entry_point: false,
        body_text: Some(node_text(node, source)),
    });
}

fn extract_context_classes(node: Node, source: &str, base_classes: &mut Vec<String>) {
    // context -> apply (ClassName var) =>
    for i in 0..node.child_count() {
        let child = match node.child(i) {
            Some(c) => c,
            None => continue,
        };
        if child.kind() == "apply" {
            // First child of apply is the class name
            if let Some(first) = child.child(0) {
                if first.kind() == "name" {
                    base_classes.push(node_text(first, source));
                }
            }
        } else if child.kind() == "name" {
            base_classes.push(node_text(child, source));
        }
    }
}

fn extract_class_methods(node: Node, source: &str, symbols: &mut Vec<TsSymbol>, class_name: &str) {
    // class_declarations contains signature nodes
    for i in 0..node.child_count() {
        let child = match node.child(i) {
            Some(c) => c,
            None => continue,
        };
        if child.kind() == "signature" {
            let mut method_name = String::new();
            for j in 0..child.child_count() {
                let gc = match child.child(j) {
                    Some(c) => c,
                    None => continue,
                };
                if gc.kind() == "variable" && method_name.is_empty() {
                    method_name = node_text(gc, source);
                }
            }

            if !method_name.is_empty() {
                let sig_text = node_text(child, source);
                let return_type = extract_return_type(&sig_text);

                symbols.push(TsSymbol {
                    name: method_name,
                    label: "Method".into(),
                    start_line: child.start_position().row as i32 + 1,
                    end_line: child.end_position().row as i32 + 1,
                    parent_name: Some(class_name.to_string()),
                    signature: Some(sig_text),
                    return_type,
                    parameters: Vec::new(),
                    docstring: None,
                    decorators: Vec::new(),
                    base_classes: Vec::new(),
                    is_exported: true,
                    is_abstract: true,
                    is_async: false,
                    is_test: false,
                    is_entry_point: false,
                    body_text: None,
                });
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Instance extraction
// ---------------------------------------------------------------------------

fn extract_instance(node: Node, source: &str, symbols: &mut Vec<TsSymbol>) {
    let mut class_name = String::new();
    let mut type_name = String::new();

    for i in 0..node.child_count() {
        let child = match node.child(i) {
            Some(c) => c,
            None => continue,
        };
        match child.kind() {
            "name" if class_name.is_empty() => {
                class_name = node_text(child, source);
            }
            "type_patterns" => {
                // Get the type being instanced
                for j in 0..child.child_count() {
                    if let Some(gc) = child.child(j) {
                        if gc.kind() == "name" && type_name.is_empty() {
                            type_name = node_text(gc, source);
                        }
                    }
                }
            }
            _ => {}
        }
    }

    if class_name.is_empty() || type_name.is_empty() {
        return;
    }

    let instance_name = format!("{} {}", class_name, type_name);
    let sig_text = node_text(node, source);

    symbols.push(TsSymbol {
        name: instance_name,
        label: "Impl".into(),
        start_line: node.start_position().row as i32 + 1,
        end_line: node.end_position().row as i32 + 1,
        parent_name: None,
        signature: Some(format!("instance {} {}", class_name, type_name)),
        return_type: None,
        parameters: Vec::new(),
        docstring: None,
        decorators: Vec::new(),
        base_classes: vec![class_name],
        is_exported: true,
        is_abstract: false,
        is_async: false,
        is_test: false,
        is_entry_point: false,
        body_text: Some(sig_text),
    });
}

// ---------------------------------------------------------------------------
// Type synonym extraction
// ---------------------------------------------------------------------------

fn extract_type_synonym(node: Node, source: &str, symbols: &mut Vec<TsSymbol>) {
    let mut name = String::new();

    for i in 0..node.child_count() {
        let child = match node.child(i) {
            Some(c) => c,
            None => continue,
        };
        // First "name" child after "type" keyword is the type synonym name
        if child.kind() == "name" && name.is_empty() {
            name = node_text(child, source);
        }
    }

    if name.is_empty() {
        return;
    }

    let sig_text = node_text(node, source);
    let docstring = collect_preceding_haddock(node, source);

    symbols.push(TsSymbol {
        name,
        label: "Class".into(),
        start_line: node.start_position().row as i32 + 1,
        end_line: node.end_position().row as i32 + 1,
        parent_name: None,
        signature: Some(sig_text),
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
        body_text: None,
    });
}

// ---------------------------------------------------------------------------
// Newtype extraction
// ---------------------------------------------------------------------------

fn extract_newtype(node: Node, source: &str, symbols: &mut Vec<TsSymbol>) {
    let mut name = String::new();
    let mut ctor_name = String::new();

    for i in 0..node.child_count() {
        let child = match node.child(i) {
            Some(c) => c,
            None => continue,
        };
        match child.kind() {
            "name" if name.is_empty() => {
                name = node_text(child, source);
            }
            "newtype_constructor" => {
                // newtype_constructor -> constructor
                for j in 0..child.child_count() {
                    if let Some(gc) = child.child(j) {
                        if gc.kind() == "constructor" && ctor_name.is_empty() {
                            ctor_name = node_text(gc, source);
                        }
                    }
                }
            }
            _ => {}
        }
    }

    if name.is_empty() {
        return;
    }

    let sig_text = node_text(node, source);
    let docstring = collect_preceding_haddock(node, source);

    // Emit the newtype as a Class
    symbols.push(TsSymbol {
        name: name.clone(),
        label: "Class".into(),
        start_line: node.start_position().row as i32 + 1,
        end_line: node.end_position().row as i32 + 1,
        parent_name: None,
        signature: Some(sig_text),
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
        body_text: Some(node_text(node, source)),
    });

    // Emit the constructor as a Constant
    if !ctor_name.is_empty() {
        symbols.push(TsSymbol {
            name: ctor_name.clone(),
            label: "Constant".into(),
            start_line: node.start_position().row as i32 + 1,
            end_line: node.end_position().row as i32 + 1,
            parent_name: Some(name.clone()),
            signature: Some(format!("{} (constructor of {})", ctor_name, name)),
            return_type: Some(name),
            parameters: Vec::new(),
            docstring: None,
            decorators: Vec::new(),
            base_classes: Vec::new(),
            is_exported: true,
            is_abstract: false,
            is_async: false,
            is_test: false,
            is_entry_point: false,
            body_text: None,
        });
    }
}

// ---------------------------------------------------------------------------
// Parameter extraction from patterns
// ---------------------------------------------------------------------------

fn extract_patterns_as_params(node: Node, source: &str) -> Vec<TsParam> {
    let mut params = Vec::new();
    for i in 0..node.child_count() {
        let child = match node.child(i) {
            Some(c) => c,
            None => continue,
        };
        match child.kind() {
            "variable" => {
                let name = node_text(child, source);
                if !name.is_empty() {
                    params.push(TsParam {
                        name,
                        type_name: None,
                    });
                }
            }
            "constructor" | "literal" | "wildcard" => {
                // Pattern matching arguments (e.g., `show Red = ...`)
                let name = node_text(child, source);
                if !name.is_empty() {
                    params.push(TsParam {
                        name,
                        type_name: None,
                    });
                }
            }
            _ => {
                // For complex patterns, use the text representation
                let name = node_text(child, source);
                if !name.is_empty() {
                    params.push(TsParam {
                        name,
                        type_name: None,
                    });
                }
            }
        }
    }
    params
}

// ---------------------------------------------------------------------------
// Return type extraction from type signature
// ---------------------------------------------------------------------------

fn extract_return_type(sig: &str) -> Option<String> {
    // Extract the last type in a function signature: `name :: A -> B -> ReturnType`
    let parts: Vec<&str> = sig.splitn(2, "::").collect();
    if parts.len() < 2 {
        return None;
    }
    let type_part = parts[1].trim();
    // The return type is the last segment after the final `->`
    let segments: Vec<&str> = type_part.split("->").collect();
    segments.last().map(|s| s.trim().to_string())
}

// ---------------------------------------------------------------------------
// Haddock comment extraction
// ---------------------------------------------------------------------------

fn collect_preceding_haddock(node: Node, source: &str) -> Option<String> {
    let mut lines = Vec::new();
    let mut sibling = node.prev_sibling();

    while let Some(sib) = sibling {
        let kind = sib.kind();
        match kind {
            "haddock" | "comment" => {
                let text = node_text(sib, source);
                let trimmed = text.trim();
                // Haddock: -- | or -- ^
                if let Some(content) = trimmed.strip_prefix("-- |") {
                    lines.push(content.trim().to_string());
                } else if let Some(content) = trimmed.strip_prefix("-- ^") {
                    lines.push(content.trim().to_string());
                } else if let Some(content) = trimmed.strip_prefix("--") {
                    lines.push(content.trim().to_string());
                }
                sibling = sib.prev_sibling();
            }
            "signature" => {
                // Type signatures precede functions; check for haddock before the signature
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

/// Find a root-level haddock comment that applies to a given node.
/// Root haddocks are siblings of the `declarations` node and apply to the first
/// declaration that follows them (matched by proximity of line numbers).
fn find_root_haddock_for_node(
    node: Node,
    source: &str,
    root_haddocks: &[(i32, String)],
) -> Option<String> {
    // The node's start line (or its preceding signature's start line)
    let mut target_line = node.start_position().row as i32 + 1;

    // If there's a preceding signature, use its line as the target
    if let Some(prev) = node.prev_sibling() {
        if prev.kind() == "signature" {
            target_line = prev.start_position().row as i32 + 1;
        }
    }

    // Find a haddock whose end line is just before the target line
    // (allowing for the signature to be between them)
    for (haddock_end_line, content) in root_haddocks {
        // The haddock should be on the line just before the signature/function
        if *haddock_end_line == target_line - 1 || *haddock_end_line == target_line {
            return Some(content.clone());
        }
    }

    // Also check if there's a haddock in the imports section that's a sibling
    // of the declarations node
    let _ = source; // suppress unused warning
    None
}

// ---------------------------------------------------------------------------
// Utility
// ---------------------------------------------------------------------------

fn node_text(node: Node, source: &str) -> String {
    source.get(node.byte_range()).unwrap_or("").to_string()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use tree_sitter::Parser;

    fn parse_haskell(source: &str) -> tree_sitter::Tree {
        let mut parser = Parser::new();
        let lang: tree_sitter::Language = tree_sitter_haskell::LANGUAGE.into();
        parser.set_language(&lang).unwrap();
        parser.parse(source, None).unwrap()
    }

    #[test]
    fn test_module_declaration() {
        let src = "module Data.List.Utils where\n\nfoo = 1\n";
        let tree = parse_haskell(src);
        let symbols = walk_tree(&tree, src);

        let module = symbols
            .iter()
            .find(|s| s.name == "Data.List.Utils" && s.label == "Module");
        assert!(
            module.is_some(),
            "Module Data.List.Utils not found. Got: {:?}",
            symbols
                .iter()
                .map(|s| (&s.name, &s.label))
                .collect::<Vec<_>>()
        );
        let module = module.unwrap();
        assert!(module.is_exported);
        assert!(module
            .signature
            .as_ref()
            .unwrap()
            .contains("module Data.List.Utils where"));
    }

    #[test]
    fn test_function_with_signature() {
        let src = r#"module Main where

add :: Int -> Int -> Int
add x y = x + y
"#;
        let tree = parse_haskell(src);
        let symbols = walk_tree(&tree, src);

        let f = symbols
            .iter()
            .find(|s| s.name == "add" && s.label == "Function");
        assert!(
            f.is_some(),
            "Function add not found. Got: {:?}",
            symbols
                .iter()
                .map(|s| (&s.name, &s.label))
                .collect::<Vec<_>>()
        );
        let f = f.unwrap();
        assert!(f.is_exported);
        assert_eq!(f.return_type.as_deref(), Some("Int"));
        assert!(f
            .signature
            .as_ref()
            .unwrap()
            .contains("add :: Int -> Int -> Int"));
        assert_eq!(f.parameters.len(), 2);
        assert_eq!(f.parameters[0].name, "x");
        assert_eq!(f.parameters[1].name, "y");
    }

    #[test]
    fn test_function_without_signature() {
        let src = r#"module Main where

greet name = "Hello, " ++ name
"#;
        let tree = parse_haskell(src);
        let symbols = walk_tree(&tree, src);

        let f = symbols
            .iter()
            .find(|s| s.name == "greet" && s.label == "Function");
        assert!(
            f.is_some(),
            "Function greet not found. Got: {:?}",
            symbols
                .iter()
                .map(|s| (&s.name, &s.label))
                .collect::<Vec<_>>()
        );
        let f = f.unwrap();
        assert_eq!(f.parameters.len(), 1);
        assert_eq!(f.parameters[0].name, "name");
        assert!(f.signature.is_none()); // No type signature
    }

    #[test]
    fn test_data_type_with_constructors() {
        let src = r#"module Main where

data Color = Red | Green | Blue
"#;
        let tree = parse_haskell(src);
        let symbols = walk_tree(&tree, src);

        // Data type itself
        let dt = symbols
            .iter()
            .find(|s| s.name == "Color" && s.label == "Class");
        assert!(
            dt.is_some(),
            "Data type Color not found. Got: {:?}",
            symbols
                .iter()
                .map(|s| (&s.name, &s.label))
                .collect::<Vec<_>>()
        );

        // Data constructors
        let red = symbols
            .iter()
            .find(|s| s.name == "Red" && s.label == "Constant");
        assert!(red.is_some(), "Constructor Red not found");
        assert_eq!(red.unwrap().parent_name.as_deref(), Some("Color"));
        assert_eq!(red.unwrap().return_type.as_deref(), Some("Color"));

        let green = symbols
            .iter()
            .find(|s| s.name == "Green" && s.label == "Constant");
        assert!(green.is_some(), "Constructor Green not found");

        let blue = symbols
            .iter()
            .find(|s| s.name == "Blue" && s.label == "Constant");
        assert!(blue.is_some(), "Constructor Blue not found");
    }

    #[test]
    fn test_type_class() {
        let src = r#"module Main where

class Eq a => Ord a where
  compare :: a -> a -> Ordering
  (<) :: a -> a -> Bool
"#;
        let tree = parse_haskell(src);
        let symbols = walk_tree(&tree, src);

        let cls = symbols
            .iter()
            .find(|s| s.name == "Ord" && s.label == "Interface");
        assert!(
            cls.is_some(),
            "Type class Ord not found. Got: {:?}",
            symbols
                .iter()
                .map(|s| (&s.name, &s.label))
                .collect::<Vec<_>>()
        );
        let cls = cls.unwrap();
        assert!(cls.is_abstract);
        assert!(cls.base_classes.contains(&"Eq".to_string()));

        // Class method
        let compare = symbols
            .iter()
            .find(|s| s.name == "compare" && s.label == "Method");
        assert!(compare.is_some(), "Method compare not found");
        assert_eq!(compare.unwrap().parent_name.as_deref(), Some("Ord"));
        assert!(compare.unwrap().is_abstract);
    }

    #[test]
    fn test_type_class_instance() {
        let src = r#"module Main where

instance Show Color where
  show Red = "Red"
  show Green = "Green"
  show Blue = "Blue"
"#;
        let tree = parse_haskell(src);
        let symbols = walk_tree(&tree, src);

        let inst = symbols
            .iter()
            .find(|s| s.name == "Show Color" && s.label == "Impl");
        assert!(
            inst.is_some(),
            "Instance Show Color not found. Got: {:?}",
            symbols
                .iter()
                .map(|s| (&s.name, &s.label))
                .collect::<Vec<_>>()
        );
        let inst = inst.unwrap();
        assert!(inst.base_classes.contains(&"Show".to_string()));
        assert!(inst
            .signature
            .as_ref()
            .unwrap()
            .contains("instance Show Color"));
    }

    #[test]
    fn test_type_synonym() {
        let src = r#"module Main where

type Name = String
"#;
        let tree = parse_haskell(src);
        let symbols = walk_tree(&tree, src);

        let ts = symbols
            .iter()
            .find(|s| s.name == "Name" && s.label == "Class");
        assert!(
            ts.is_some(),
            "Type synonym Name not found. Got: {:?}",
            symbols
                .iter()
                .map(|s| (&s.name, &s.label))
                .collect::<Vec<_>>()
        );
        assert!(ts
            .unwrap()
            .signature
            .as_ref()
            .unwrap()
            .contains("type Name = String"));
    }

    #[test]
    fn test_newtype() {
        let src = r#"module Main where

newtype Wrapper a = Wrapper { unwrap :: a }
"#;
        let tree = parse_haskell(src);
        let symbols = walk_tree(&tree, src);

        let nt = symbols
            .iter()
            .find(|s| s.name == "Wrapper" && s.label == "Class");
        assert!(
            nt.is_some(),
            "Newtype Wrapper not found. Got: {:?}",
            symbols
                .iter()
                .map(|s| (&s.name, &s.label))
                .collect::<Vec<_>>()
        );

        // Constructor
        let ctor = symbols
            .iter()
            .find(|s| s.name == "Wrapper" && s.label == "Constant");
        assert!(ctor.is_some(), "Constructor Wrapper not found");
        assert_eq!(ctor.unwrap().parent_name.as_deref(), Some("Wrapper"));
    }

    #[test]
    fn test_main_is_entry_point() {
        let src = r#"module Main where

main :: IO ()
main = putStrLn "Hello"
"#;
        let tree = parse_haskell(src);
        let symbols = walk_tree(&tree, src);

        let main_fn = symbols
            .iter()
            .find(|s| s.name == "main" && s.label == "Function");
        assert!(main_fn.is_some(), "Function main not found");
        assert!(main_fn.unwrap().is_entry_point);
    }

    #[test]
    fn test_multiple_function_equations() {
        let src = r#"module Main where

factorial :: Int -> Int
factorial 0 = 1
factorial n = n * factorial (n - 1)
"#;
        let tree = parse_haskell(src);
        let symbols = walk_tree(&tree, src);

        // Should only emit one symbol for factorial (not one per equation)
        let factorials: Vec<_> = symbols
            .iter()
            .filter(|s| s.name == "factorial" && s.label == "Function")
            .collect();
        assert_eq!(
            factorials.len(),
            1,
            "Expected exactly 1 factorial symbol, got {}",
            factorials.len()
        );
        assert_eq!(factorials[0].return_type.as_deref(), Some("Int"));
    }

    #[test]
    fn test_haddock_comment() {
        let src = r#"module Main where

-- | Add two numbers together.
add :: Int -> Int -> Int
add x y = x + y
"#;
        let tree = parse_haskell(src);
        let symbols = walk_tree(&tree, src);

        let f = symbols
            .iter()
            .find(|s| s.name == "add" && s.label == "Function");
        assert!(f.is_some(), "Function add not found");
        let doc = f.unwrap().docstring.as_ref();
        assert!(doc.is_some(), "Expected haddock docstring on add");
        assert!(
            doc.unwrap().contains("Add two numbers"),
            "Docstring should contain 'Add two numbers', got: {:?}",
            doc
        );
    }

    #[test]
    fn test_walker_output_invariants() {
        let src = r#"module Data.Example where

-- | A simple function
add :: Int -> Int -> Int
add x y = x + y

data Shape = Circle Double | Rectangle Double Double

class Drawable a where
  draw :: a -> IO ()

instance Drawable Shape where
  draw (Circle r) = putStrLn "circle"
  draw (Rectangle w h) = putStrLn "rect"

type Radius = Double

newtype Age = Age Int
"#;
        let tree = parse_haskell(src);
        let symbols = walk_tree(&tree, src);

        assert!(!symbols.is_empty(), "Expected at least one symbol");

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
