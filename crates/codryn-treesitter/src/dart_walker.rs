//! Dart AST walker.
//!
//! Extracts: class declarations, mixin declarations, extension declarations,
//! top-level functions, methods, factory constructors, constructors, annotations.
//! Handles: abstract classes, inheritance (extends/implements/with), async functions.

use crate::{TsParam, TsSymbol};
use tree_sitter::Node;

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

/// Walk a Dart tree-sitter AST and extract symbols.
pub fn walk_tree(tree: &tree_sitter::Tree, source: &str) -> Vec<TsSymbol> {
    let mut symbols = Vec::new();
    let root = tree.root_node();
    visit_top_level(root, source, &mut symbols);
    symbols
}

// ---------------------------------------------------------------------------
// Top-level visitor
// ---------------------------------------------------------------------------

fn visit_top_level(node: Node, source: &str, symbols: &mut Vec<TsSymbol>) {
    let child_count = node.child_count();
    let mut i = 0;
    while i < child_count {
        let child = match node.child(i) {
            Some(c) => c,
            None => {
                i += 1;
                continue;
            }
        };
        let kind = child.kind();

        match kind {
            "class_declaration" => {
                extract_class_declaration(child, source, symbols, None);
            }
            "mixin_declaration" => {
                extract_mixin_declaration(child, source, symbols);
            }
            "extension_declaration" => {
                extract_extension_declaration(child, source, symbols);
            }
            "function_signature" => {
                // Top-level function: function_signature followed by function_body
                let body_node = node.child(i + 1);
                let annotations = collect_preceding_annotations(node, i, source);
                let doc = collect_preceding_doc_comments(node, i, source);
                if let Some(mut sym) =
                    extract_function_from_signature(child, body_node, source, None)
                {
                    sym.decorators = annotations;
                    sym.docstring = doc;
                    sym.is_entry_point = sym.name == "main";
                    symbols.push(sym);
                }
                // Skip the function_body
                if body_node.map(|n| n.kind()) == Some("function_body") {
                    i += 1;
                }
            }
            _ => {}
        }
        i += 1;
    }
}

// ---------------------------------------------------------------------------
// Class extraction
// ---------------------------------------------------------------------------

fn extract_class_declaration(
    node: Node,
    source: &str,
    symbols: &mut Vec<TsSymbol>,
    parent_name: Option<&str>,
) {
    let mut name = String::new();
    let mut is_abstract = false;
    let mut base_classes = Vec::new();
    let mut annotations = Vec::new();
    let mut class_body_node: Option<Node> = None;

    for ci in 0..node.child_count() {
        let child = match node.child(ci) {
            Some(c) => c,
            None => continue,
        };
        match child.kind() {
            "abstract" => is_abstract = true,
            "identifier" if name.is_empty() => {
                name = node_text(child, source);
            }
            "superclass" => {
                // superclass -> extends type_identifier
                if let Some(type_id) = find_child_by_kind(child, "type_identifier") {
                    base_classes.push(node_text(type_id, source));
                }
            }
            "interfaces" => {
                // interfaces -> implements type_identifier [, type_identifier]*
                for ii in 0..child.child_count() {
                    if let Some(ic) = child.child(ii) {
                        if ic.kind() == "type_identifier" {
                            base_classes.push(node_text(ic, source));
                        }
                    }
                }
            }
            "mixins" => {
                // mixins -> with type_identifier [, type_identifier]*
                for ii in 0..child.child_count() {
                    if let Some(ic) = child.child(ii) {
                        if ic.kind() == "type_identifier" {
                            base_classes.push(node_text(ic, source));
                        }
                    }
                }
            }
            "annotation" => {
                if let Some(ann_name) = extract_annotation_name(child, source) {
                    annotations.push(ann_name);
                }
            }
            "class_body" => {
                class_body_node = Some(child);
            }
            _ => {}
        }
    }

    if name.is_empty() {
        return;
    }

    let body_text = class_body_node.map(|b| node_text(b, source));

    // Build signature
    let mut sig = String::new();
    if is_abstract {
        sig.push_str("abstract ");
    }
    sig.push_str("class ");
    sig.push_str(&name);
    if !base_classes.is_empty() {
        sig.push_str(" : ");
        sig.push_str(&base_classes.join(", "));
    }

    symbols.push(TsSymbol {
        name: name.clone(),
        label: "Class".into(),
        start_line: node.start_position().row as i32 + 1,
        end_line: node.end_position().row as i32 + 1,
        parent_name: parent_name.map(String::from),
        signature: Some(sig),
        return_type: None,
        parameters: Vec::new(),
        docstring: None,
        decorators: annotations,
        base_classes,
        is_exported: !name.starts_with('_'),
        is_abstract,
        is_async: false,
        is_test: false,
        is_entry_point: false,
        body_text,
    });

    // Extract members from class body
    if let Some(body) = class_body_node {
        extract_class_members(body, source, symbols, &name);
    }
}

// ---------------------------------------------------------------------------
// Mixin extraction
// ---------------------------------------------------------------------------

fn extract_mixin_declaration(node: Node, source: &str, symbols: &mut Vec<TsSymbol>) {
    let mut name = String::new();
    let mut base_classes = Vec::new();
    let mut annotations = Vec::new();
    let mut class_body_node: Option<Node> = None;

    for ci in 0..node.child_count() {
        let child = match node.child(ci) {
            Some(c) => c,
            None => continue,
        };
        match child.kind() {
            "identifier" if name.is_empty() => {
                name = node_text(child, source);
            }
            "superclass" | "interfaces" | "mixins" => {
                for ii in 0..child.child_count() {
                    if let Some(ic) = child.child(ii) {
                        if ic.kind() == "type_identifier" {
                            base_classes.push(node_text(ic, source));
                        }
                    }
                }
            }
            "annotation" => {
                if let Some(ann_name) = extract_annotation_name(child, source) {
                    annotations.push(ann_name);
                }
            }
            "class_body" => {
                class_body_node = Some(child);
            }
            _ => {}
        }
    }

    if name.is_empty() {
        return;
    }

    let body_text = class_body_node.map(|b| node_text(b, source));

    let mut sig = String::from("mixin ");
    sig.push_str(&name);
    if !base_classes.is_empty() {
        sig.push_str(" on ");
        sig.push_str(&base_classes.join(", "));
    }

    symbols.push(TsSymbol {
        name: name.clone(),
        label: "Class".into(),
        start_line: node.start_position().row as i32 + 1,
        end_line: node.end_position().row as i32 + 1,
        parent_name: None,
        signature: Some(sig),
        return_type: None,
        parameters: Vec::new(),
        docstring: None,
        decorators: annotations,
        base_classes,
        is_exported: !name.starts_with('_'),
        is_abstract: false,
        is_async: false,
        is_test: false,
        is_entry_point: false,
        body_text,
    });

    // Extract members
    if let Some(body) = class_body_node {
        extract_class_members(body, source, symbols, &name);
    }
}

// ---------------------------------------------------------------------------
// Extension extraction
// ---------------------------------------------------------------------------

fn extract_extension_declaration(node: Node, source: &str, symbols: &mut Vec<TsSymbol>) {
    let mut name = String::new();
    let mut on_type = String::new();
    let mut class_body_node: Option<Node> = None;

    for ci in 0..node.child_count() {
        let child = match node.child(ci) {
            Some(c) => c,
            None => continue,
        };
        match child.kind() {
            "identifier" if name.is_empty() => {
                name = node_text(child, source);
            }
            "type_identifier" if on_type.is_empty() => {
                on_type = node_text(child, source);
            }
            "class_body" | "extension_body" => {
                class_body_node = Some(child);
            }
            _ => {}
        }
    }

    // Extensions can be unnamed
    if name.is_empty() {
        name = format!(
            "extension on {}",
            if on_type.is_empty() {
                "unknown"
            } else {
                &on_type
            }
        );
    }

    let body_text = class_body_node.map(|b| node_text(b, source));

    let mut sig = String::from("extension ");
    sig.push_str(&name);
    if !on_type.is_empty() {
        sig.push_str(" on ");
        sig.push_str(&on_type);
    }

    symbols.push(TsSymbol {
        name: name.clone(),
        label: "Class".into(),
        start_line: node.start_position().row as i32 + 1,
        end_line: node.end_position().row as i32 + 1,
        parent_name: None,
        signature: Some(sig),
        return_type: None,
        parameters: Vec::new(),
        docstring: None,
        decorators: Vec::new(),
        base_classes: if on_type.is_empty() {
            Vec::new()
        } else {
            vec![on_type]
        },
        is_exported: !name.starts_with('_'),
        is_abstract: false,
        is_async: false,
        is_test: false,
        is_entry_point: false,
        body_text,
    });

    // Extract members
    if let Some(body) = class_body_node {
        extract_class_members(body, source, symbols, &name);
    }
}

// ---------------------------------------------------------------------------
// Class member extraction
// ---------------------------------------------------------------------------

fn extract_class_members(body: Node, source: &str, symbols: &mut Vec<TsSymbol>, class_name: &str) {
    for ci in 0..body.child_count() {
        let child = match body.child(ci) {
            Some(c) => c,
            None => continue,
        };
        if child.kind() != "class_member" {
            continue;
        }
        extract_class_member(child, source, symbols, class_name);
    }
}

fn extract_class_member(
    member_node: Node,
    source: &str,
    symbols: &mut Vec<TsSymbol>,
    class_name: &str,
) {
    // Collect annotations on this member
    let mut annotations = Vec::new();
    let mut method_sig_node: Option<Node> = None;
    let mut function_body_node: Option<Node> = None;
    let mut declaration_node: Option<Node> = None;

    for ci in 0..member_node.child_count() {
        let child = match member_node.child(ci) {
            Some(c) => c,
            None => continue,
        };
        match child.kind() {
            "annotation" => {
                if let Some(ann_name) = extract_annotation_name(child, source) {
                    annotations.push(ann_name);
                }
            }
            "method_signature" => {
                method_sig_node = Some(child);
            }
            "function_body" => {
                function_body_node = Some(child);
            }
            "declaration" => {
                declaration_node = Some(child);
            }
            _ => {}
        }
    }

    // Case 1: method_signature + function_body (regular method or factory constructor)
    if let Some(method_sig) = method_sig_node {
        extract_method_or_factory(
            method_sig,
            function_body_node,
            member_node,
            source,
            symbols,
            class_name,
            &annotations,
        );
        return;
    }

    // Case 2: declaration (constructor_signature or abstract method via function_signature)
    if let Some(decl) = declaration_node {
        extract_from_declaration(decl, member_node, source, symbols, class_name, &annotations);
    }
}

fn extract_method_or_factory(
    method_sig: Node,
    body_node: Option<Node>,
    member_node: Node,
    source: &str,
    symbols: &mut Vec<TsSymbol>,
    class_name: &str,
    annotations: &[String],
) {
    // method_signature can contain:
    // - function_signature (regular method)
    // - factory_constructor_signature (factory constructor)
    for ci in 0..method_sig.child_count() {
        let child = match method_sig.child(ci) {
            Some(c) => c,
            None => continue,
        };
        match child.kind() {
            "function_signature" => {
                if let Some(mut sym) =
                    extract_function_from_signature(child, body_node, source, Some(class_name))
                {
                    sym.decorators = annotations.to_vec();
                    sym.is_test = annotations.iter().any(|a| a == "test" || a == "Test");
                    // Use member_node for line range (includes annotations)
                    sym.start_line = member_node.start_position().row as i32 + 1;
                    sym.end_line = member_node.end_position().row as i32 + 1;
                    symbols.push(sym);
                }
            }
            "factory_constructor_signature" => {
                let mut name = String::new();
                let mut params = Vec::new();

                for fi in 0..child.child_count() {
                    let fc = match child.child(fi) {
                        Some(c) => c,
                        None => continue,
                    };
                    match fc.kind() {
                        "identifier" if name.is_empty() => {
                            name = node_text(fc, source);
                        }
                        "formal_parameter_list" => {
                            params = extract_params_from_list(fc, source);
                        }
                        _ => {}
                    }
                }

                if !name.is_empty() {
                    let body_text = body_node.map(|b| node_text(b, source));
                    let sig = format!("factory {}({})", name, format_params(&params));

                    symbols.push(TsSymbol {
                        name: format!("{}.factory", name),
                        label: "Method".into(),
                        start_line: member_node.start_position().row as i32 + 1,
                        end_line: member_node.end_position().row as i32 + 1,
                        parent_name: Some(class_name.to_string()),
                        signature: Some(sig),
                        return_type: Some(name),
                        parameters: params,
                        docstring: None,
                        decorators: annotations.to_vec(),
                        base_classes: Vec::new(),
                        is_exported: true,
                        is_abstract: false,
                        is_async: false,
                        is_test: false,
                        is_entry_point: false,
                        body_text,
                    });
                }
            }
            _ => {}
        }
    }
}

fn extract_from_declaration(
    decl: Node,
    member_node: Node,
    source: &str,
    symbols: &mut Vec<TsSymbol>,
    class_name: &str,
    annotations: &[String],
) {
    // declaration can contain:
    // - constructor_signature (named or unnamed constructor)
    // - function_signature (abstract method declaration without body)
    for ci in 0..decl.child_count() {
        let child = match decl.child(ci) {
            Some(c) => c,
            None => continue,
        };
        match child.kind() {
            "constructor_signature" => {
                let mut ctor_name = String::new();
                let mut named_part = String::new();
                let mut params = Vec::new();

                for fi in 0..child.child_count() {
                    let fc = match child.child(fi) {
                        Some(c) => c,
                        None => continue,
                    };
                    match fc.kind() {
                        "identifier" => {
                            if ctor_name.is_empty() {
                                ctor_name = node_text(fc, source);
                            } else {
                                named_part = node_text(fc, source);
                            }
                        }
                        "formal_parameter_list" => {
                            params = extract_params_from_list(fc, source);
                        }
                        _ => {}
                    }
                }

                if !ctor_name.is_empty() {
                    let display_name = if named_part.is_empty() {
                        ctor_name.clone()
                    } else {
                        format!("{}.{}", ctor_name, named_part)
                    };
                    let sig = format!("{}({})", display_name, format_params(&params));

                    symbols.push(TsSymbol {
                        name: display_name,
                        label: "Method".into(),
                        start_line: member_node.start_position().row as i32 + 1,
                        end_line: member_node.end_position().row as i32 + 1,
                        parent_name: Some(class_name.to_string()),
                        signature: Some(sig),
                        return_type: Some(ctor_name),
                        parameters: params,
                        docstring: None,
                        decorators: annotations.to_vec(),
                        base_classes: Vec::new(),
                        is_exported: !named_part.starts_with('_'),
                        is_abstract: false,
                        is_async: false,
                        is_test: false,
                        is_entry_point: false,
                        body_text: None,
                    });
                }
            }
            "function_signature" => {
                // Abstract method (no body, just declaration with semicolon)
                if let Some(mut sym) =
                    extract_function_from_signature(child, None, source, Some(class_name))
                {
                    sym.decorators = annotations.to_vec();
                    sym.is_abstract = true;
                    sym.start_line = member_node.start_position().row as i32 + 1;
                    sym.end_line = member_node.end_position().row as i32 + 1;
                    symbols.push(sym);
                }
            }
            _ => {}
        }
    }
}

// ---------------------------------------------------------------------------
// Function/method extraction from function_signature node
// ---------------------------------------------------------------------------

fn extract_function_from_signature(
    sig_node: Node,
    body_node: Option<Node>,
    source: &str,
    parent_name: Option<&str>,
) -> Option<TsSymbol> {
    let mut name = String::new();
    let mut return_type: Option<String> = None;
    let mut params = Vec::new();

    for ci in 0..sig_node.child_count() {
        let child = match sig_node.child(ci) {
            Some(c) => c,
            None => continue,
        };
        match child.kind() {
            "identifier" if name.is_empty() => {
                name = node_text(child, source);
            }
            "type_identifier" if return_type.is_none() => {
                let mut rt = node_text(child, source);
                // Check for type_arguments following
                if let Some(next) = sig_node.child(ci + 1) {
                    if next.kind() == "type_arguments" {
                        rt.push_str(&node_text(next, source));
                    }
                }
                return_type = Some(rt);
            }
            "void_type" => {
                return_type = Some("void".to_string());
            }
            "formal_parameter_list" => {
                params = extract_params_from_list(child, source);
            }
            _ => {}
        }
    }

    if name.is_empty() {
        return None;
    }

    // Detect async from function_body
    let is_async = body_node
        .map(|b| {
            for bi in 0..b.child_count() {
                if let Some(bc) = b.child(bi) {
                    if bc.kind() == "async" || bc.kind() == "async*" {
                        return true;
                    }
                }
            }
            false
        })
        .unwrap_or(false);

    let body_text = body_node.map(|b| node_text(b, source));

    let label = if parent_name.is_some() {
        "Method"
    } else {
        "Function"
    };

    let sig = build_function_signature(&name, &params, return_type.as_deref(), is_async);

    let is_exported = !name.starts_with('_');

    Some(TsSymbol {
        name,
        label: label.into(),
        start_line: sig_node.start_position().row as i32 + 1,
        end_line: body_node
            .map(|b| b.end_position().row as i32 + 1)
            .unwrap_or(sig_node.end_position().row as i32 + 1),
        parent_name: parent_name.map(String::from),
        signature: Some(sig),
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
// Parameter extraction
// ---------------------------------------------------------------------------

fn extract_params_from_list(list_node: Node, source: &str) -> Vec<TsParam> {
    let mut params = Vec::new();
    for ci in 0..list_node.child_count() {
        let child = match list_node.child(ci) {
            Some(c) => c,
            None => continue,
        };
        if child.kind() == "formal_parameter" {
            if let Some(param) = extract_single_param(child, source) {
                params.push(param);
            }
        }
        // Handle optional positional and named parameters
        if child.kind() == "optional_formal_parameters" {
            for oi in 0..child.child_count() {
                if let Some(oc) = child.child(oi) {
                    if oc.kind() == "formal_parameter" {
                        if let Some(param) = extract_single_param(oc, source) {
                            params.push(param);
                        }
                    }
                }
            }
        }
    }
    params
}

fn extract_single_param(param_node: Node, source: &str) -> Option<TsParam> {
    // formal_parameter can contain:
    // - type_identifier + identifier (typed param)
    // - identifier only (untyped param)
    // - constructor_param (this.name)
    let mut type_name: Option<String> = None;
    let mut name = String::new();

    for ci in 0..param_node.child_count() {
        let child = match param_node.child(ci) {
            Some(c) => c,
            None => continue,
        };
        match child.kind() {
            "type_identifier" => {
                type_name = Some(node_text(child, source));
            }
            "identifier" => {
                name = node_text(child, source);
            }
            "constructor_param" => {
                // this.name pattern
                for pi in 0..child.child_count() {
                    if let Some(pc) = child.child(pi) {
                        if pc.kind() == "identifier" {
                            name = node_text(pc, source);
                        }
                    }
                }
            }
            _ => {}
        }
    }

    if name.is_empty() {
        return None;
    }

    Some(TsParam { name, type_name })
}

// ---------------------------------------------------------------------------
// Annotation extraction
// ---------------------------------------------------------------------------

fn extract_annotation_name(node: Node, source: &str) -> Option<String> {
    // annotation -> @ identifier [arguments]
    for ci in 0..node.child_count() {
        let child = match node.child(ci) {
            Some(c) => c,
            None => continue,
        };
        if child.kind() == "identifier" {
            let text = node_text(child, source);
            if !text.is_empty() {
                return Some(text);
            }
        }
    }
    None
}

/// Collect annotations that precede a node at index `idx` in the parent.
fn collect_preceding_annotations(parent: Node, idx: usize, source: &str) -> Vec<String> {
    let mut annotations = Vec::new();
    let mut j = idx;
    while j > 0 {
        j -= 1;
        let prev = match parent.child(j) {
            Some(c) => c,
            None => break,
        };
        if prev.kind() == "annotation" {
            if let Some(ann_name) = extract_annotation_name(prev, source) {
                annotations.push(ann_name);
            }
        } else if prev.kind() == "comment" {
            // Skip comments, they might be between annotations
            continue;
        } else {
            break;
        }
    }
    annotations.reverse();
    annotations
}

// ---------------------------------------------------------------------------
// Doc comment extraction
// ---------------------------------------------------------------------------

/// Collect `///` doc comments preceding a node at index `idx` in the parent.
fn collect_preceding_doc_comments(parent: Node, idx: usize, source: &str) -> Option<String> {
    let mut lines = Vec::new();
    let mut j = idx;
    while j > 0 {
        j -= 1;
        let prev = match parent.child(j) {
            Some(c) => c,
            None => break,
        };
        if prev.kind() == "comment" {
            let text = node_text(prev, source);
            let trimmed = text.trim();
            if let Some(doc) = trimmed.strip_prefix("///") {
                lines.push(doc.strip_prefix(' ').unwrap_or(doc).to_string());
            } else {
                break;
            }
        } else if prev.kind() == "annotation" {
            // Annotations can appear between doc comments and the declaration
            continue;
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

fn build_function_signature(
    name: &str,
    params: &[TsParam],
    return_type: Option<&str>,
    is_async: bool,
) -> String {
    let mut sig = String::new();
    if let Some(rt) = return_type {
        sig.push_str(rt);
        sig.push(' ');
    }
    sig.push_str(name);
    sig.push('(');
    sig.push_str(&format_params(params));
    sig.push(')');
    if is_async {
        sig.push_str(" async");
    }
    sig
}

fn format_params(params: &[TsParam]) -> String {
    params
        .iter()
        .map(|p| {
            if let Some(ref t) = p.type_name {
                format!("{} {}", t, p.name)
            } else {
                p.name.clone()
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

// ---------------------------------------------------------------------------
// Utility helpers
// ---------------------------------------------------------------------------

fn node_text(node: Node, source: &str) -> String {
    source.get(node.byte_range()).unwrap_or("").to_string()
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

    fn parse_dart(source: &str) -> tree_sitter::Tree {
        let mut parser = Parser::new();
        let lang: tree_sitter::Language = tree_sitter_dart::LANGUAGE.into();
        parser.set_language(&lang).unwrap();
        parser.parse(source, None).unwrap()
    }

    #[test]
    fn test_class_extraction() {
        let src = r#"
class UserService {
  void doSomething() {}
}
"#;
        let tree = parse_dart(src);
        let symbols = walk_tree(&tree, src);

        let cls = symbols
            .iter()
            .find(|s| s.name == "UserService" && s.label == "Class");
        assert!(cls.is_some(), "Class UserService not found");
        let cls = cls.unwrap();
        assert!(cls.is_exported);
        assert!(!cls.is_abstract);
        assert!(cls
            .signature
            .as_ref()
            .unwrap()
            .contains("class UserService"));
    }

    #[test]
    fn test_abstract_class() {
        let src = r#"
abstract class Shape {
  double area();
}
"#;
        let tree = parse_dart(src);
        let symbols = walk_tree(&tree, src);

        let cls = symbols
            .iter()
            .find(|s| s.name == "Shape" && s.label == "Class");
        assert!(cls.is_some(), "Abstract class Shape not found");
        let cls = cls.unwrap();
        assert!(cls.is_abstract);
        assert!(cls.signature.as_ref().unwrap().contains("abstract class"));
    }

    #[test]
    fn test_class_with_inheritance() {
        let src = r#"
class Circle extends Shape implements Drawable {
  double radius;
  Circle(this.radius);

  @override
  double area() => 3.14 * radius * radius;
}
"#;
        let tree = parse_dart(src);
        let symbols = walk_tree(&tree, src);

        let cls = symbols
            .iter()
            .find(|s| s.name == "Circle" && s.label == "Class");
        assert!(cls.is_some(), "Class Circle not found");
        let cls = cls.unwrap();
        assert!(cls.base_classes.contains(&"Shape".to_string()));
        assert!(cls.base_classes.contains(&"Drawable".to_string()));
    }

    #[test]
    fn test_mixin_extraction() {
        let src = r#"
mixin Swimming {
  void swim() {
    print('swimming');
  }
}
"#;
        let tree = parse_dart(src);
        let symbols = walk_tree(&tree, src);

        let mixin = symbols
            .iter()
            .find(|s| s.name == "Swimming" && s.label == "Class");
        assert!(mixin.is_some(), "Mixin Swimming not found");
        let mixin = mixin.unwrap();
        assert!(mixin.signature.as_ref().unwrap().contains("mixin Swimming"));

        let method = symbols
            .iter()
            .find(|s| s.name == "swim" && s.label == "Method");
        assert!(method.is_some(), "Method swim not found");
        assert_eq!(method.unwrap().parent_name.as_deref(), Some("Swimming"));
    }

    #[test]
    fn test_extension_extraction() {
        let src = r#"
extension StringExt on String {
  String capitalize() {
    return '${this[0].toUpperCase()}${substring(1)}';
  }
}
"#;
        let tree = parse_dart(src);
        let symbols = walk_tree(&tree, src);

        let ext = symbols
            .iter()
            .find(|s| s.name == "StringExt" && s.label == "Class");
        assert!(ext.is_some(), "Extension StringExt not found");
        let ext = ext.unwrap();
        assert!(ext
            .signature
            .as_ref()
            .unwrap()
            .contains("extension StringExt on String"));
        assert!(ext.base_classes.contains(&"String".to_string()));

        let method = symbols
            .iter()
            .find(|s| s.name == "capitalize" && s.label == "Method");
        assert!(method.is_some(), "Method capitalize not found");
        assert_eq!(method.unwrap().parent_name.as_deref(), Some("StringExt"));
    }

    #[test]
    fn test_factory_constructor() {
        let src = r#"
class Logger {
  static final Logger _instance = Logger._internal();

  factory Logger() {
    return _instance;
  }

  Logger._internal();
}
"#;
        let tree = parse_dart(src);
        let symbols = walk_tree(&tree, src);

        let cls = symbols
            .iter()
            .find(|s| s.name == "Logger" && s.label == "Class");
        assert!(cls.is_some(), "Class Logger not found");

        let factory = symbols
            .iter()
            .find(|s| s.name == "Logger.factory" && s.label == "Method");
        assert!(factory.is_some(), "Factory constructor not found");
        let factory = factory.unwrap();
        assert_eq!(factory.parent_name.as_deref(), Some("Logger"));
        assert!(factory
            .signature
            .as_ref()
            .unwrap()
            .contains("factory Logger"));

        let named_ctor = symbols
            .iter()
            .find(|s| s.name == "Logger._internal" && s.label == "Method");
        assert!(
            named_ctor.is_some(),
            "Named constructor Logger._internal not found"
        );
    }

    #[test]
    fn test_top_level_function() {
        let src = r#"
int add(int a, int b) {
  return a + b;
}
"#;
        let tree = parse_dart(src);
        let symbols = walk_tree(&tree, src);

        let f = symbols
            .iter()
            .find(|s| s.name == "add" && s.label == "Function");
        assert!(f.is_some(), "Function add not found");
        let f = f.unwrap();
        assert_eq!(f.parameters.len(), 2);
        assert_eq!(f.parameters[0].name, "a");
        assert_eq!(f.parameters[0].type_name.as_deref(), Some("int"));
        assert_eq!(f.parameters[1].name, "b");
        assert_eq!(f.parameters[1].type_name.as_deref(), Some("int"));
        assert_eq!(f.return_type.as_deref(), Some("int"));
    }

    #[test]
    fn test_async_function() {
        let src = r#"
Future<String> fetchData(String url) async {
  return '';
}
"#;
        let tree = parse_dart(src);
        let symbols = walk_tree(&tree, src);

        let f = symbols
            .iter()
            .find(|s| s.name == "fetchData" && s.label == "Function");
        assert!(f.is_some(), "Function fetchData not found");
        let f = f.unwrap();
        assert!(f.is_async, "Expected is_async=true for async function");
        assert!(f.return_type.as_ref().unwrap().contains("Future"));
    }

    #[test]
    fn test_annotations() {
        let src = r#"
@deprecated
class OldWidget {}
"#;
        let tree = parse_dart(src);
        let symbols = walk_tree(&tree, src);

        let cls = symbols
            .iter()
            .find(|s| s.name == "OldWidget" && s.label == "Class");
        assert!(cls.is_some(), "Class OldWidget not found");
        let cls = cls.unwrap();
        assert!(
            cls.decorators.iter().any(|d| d == "deprecated"),
            "Expected @deprecated annotation"
        );
    }

    #[test]
    fn test_method_annotations() {
        let src = r#"
class MyWidget {
  @override
  void build() {}
}
"#;
        let tree = parse_dart(src);
        let symbols = walk_tree(&tree, src);

        let method = symbols
            .iter()
            .find(|s| s.name == "build" && s.label == "Method");
        assert!(method.is_some(), "Method build not found");
        let method = method.unwrap();
        assert!(
            method.decorators.iter().any(|d| d == "override"),
            "Expected @override annotation"
        );
    }

    #[test]
    fn test_doc_comments() {
        let src = r#"
/// Adds two numbers.
/// Returns the sum.
int add(int a, int b) {
  return a + b;
}
"#;
        let tree = parse_dart(src);
        let symbols = walk_tree(&tree, src);

        let f = symbols
            .iter()
            .find(|s| s.name == "add" && s.label == "Function");
        assert!(f.is_some(), "Function add not found");
        let doc = f.unwrap().docstring.as_ref().expect("Expected docstring");
        assert!(
            doc.contains("Adds two numbers"),
            "Docstring should contain description"
        );
        assert!(
            doc.contains("Returns the sum"),
            "Docstring should contain second line"
        );
    }

    #[test]
    fn test_private_visibility() {
        let src = r#"
class _InternalHelper {
  void _privateMethod() {}
  void publicMethod() {}
}
"#;
        let tree = parse_dart(src);
        let symbols = walk_tree(&tree, src);

        let cls = symbols
            .iter()
            .find(|s| s.name == "_InternalHelper" && s.label == "Class");
        assert!(cls.is_some(), "Class _InternalHelper not found");
        assert!(
            !cls.unwrap().is_exported,
            "Private class should not be exported"
        );
    }

    #[test]
    fn test_entry_point() {
        let src = r#"
void main() {
  print('Hello');
}
"#;
        let tree = parse_dart(src);
        let symbols = walk_tree(&tree, src);

        let main = symbols
            .iter()
            .find(|s| s.name == "main" && s.label == "Function");
        assert!(main.is_some(), "Function main not found");
        assert!(
            main.unwrap().is_entry_point,
            "Expected is_entry_point=true for main"
        );
    }

    #[test]
    fn test_constructor_extraction() {
        let src = r#"
class Animal {
  String name;
  Animal(this.name);
  void speak() {}
}
"#;
        let tree = parse_dart(src);
        let symbols = walk_tree(&tree, src);

        let ctor = symbols
            .iter()
            .find(|s| s.name == "Animal" && s.label == "Method");
        assert!(ctor.is_some(), "Constructor Animal not found");
        let ctor = ctor.unwrap();
        assert_eq!(ctor.parent_name.as_deref(), Some("Animal"));
        assert_eq!(ctor.parameters.len(), 1);
        assert_eq!(ctor.parameters[0].name, "name");
    }
}
