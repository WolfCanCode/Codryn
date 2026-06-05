//! Java AST walker.
//!
//! Extracts: class declarations, interface declarations, enum declarations,
//! method declarations, constructor declarations, fields, annotations, inheritance.
//! Handles: access modifiers, abstract, static, Spring Boot annotations,
//! `@Test` annotation for test detection.

use crate::{TsParam, TsSymbol};
use tree_sitter::{Node, TreeCursor};

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

/// Walk a Java tree-sitter AST and extract symbols.
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
            "class_declaration" => {
                let doc = collect_preceding_comments(node, source);
                let annotations = collect_annotations(node, source);
                if let Some(mut sym) = extract_class(node, source) {
                    sym.docstring = doc.or(sym.docstring);
                    sym.decorators = annotations;
                    sym.parent_name = parent_name.map(String::from);
                    let name = sym.name.clone();
                    symbols.push(sym);
                    visit_children(cursor, source, symbols, Some(&name));
                }
            }
            "interface_declaration" => {
                let doc = collect_preceding_comments(node, source);
                let annotations = collect_annotations(node, source);
                if let Some(mut sym) = extract_interface(node, source) {
                    sym.docstring = doc.or(sym.docstring);
                    sym.decorators = annotations;
                    sym.parent_name = parent_name.map(String::from);
                    let name = sym.name.clone();
                    symbols.push(sym);
                    visit_children(cursor, source, symbols, Some(&name));
                }
            }
            "enum_declaration" => {
                let doc = collect_preceding_comments(node, source);
                let annotations = collect_annotations(node, source);
                if let Some(mut sym) = extract_enum(node, source) {
                    sym.docstring = doc.or(sym.docstring);
                    sym.decorators = annotations;
                    sym.parent_name = parent_name.map(String::from);
                    let name = sym.name.clone();
                    symbols.push(sym);
                    visit_children(cursor, source, symbols, Some(&name));
                }
            }
            "method_declaration" => {
                let doc = collect_preceding_comments(node, source);
                let annotations = collect_annotations(node, source);
                if let Some(mut sym) = extract_method(node, source, parent_name) {
                    sym.docstring = doc.or(sym.docstring);
                    sym.decorators = annotations.clone();
                    sym.is_test = annotations
                        .iter()
                        .any(|a| a == "Test" || a == "ParameterizedTest");
                    symbols.push(sym);
                }
                visit_children(cursor, source, symbols, parent_name);
            }
            "constructor_declaration" => {
                let doc = collect_preceding_comments(node, source);
                let annotations = collect_annotations(node, source);
                if let Some(mut sym) = extract_constructor(node, source, parent_name) {
                    sym.docstring = doc.or(sym.docstring);
                    sym.decorators = annotations;
                    symbols.push(sym);
                }
                visit_children(cursor, source, symbols, parent_name);
            }
            "field_declaration" => {
                // Extract fields as symbols for completeness
                let annotations = collect_annotations(node, source);
                if let Some(mut sym) = extract_field(node, source, parent_name) {
                    sym.decorators = annotations;
                    symbols.push(sym);
                }
            }
            "annotation_type_declaration" => {
                let doc = collect_preceding_comments(node, source);
                if let Some(mut sym) = extract_annotation_type(node, source) {
                    sym.docstring = doc.or(sym.docstring);
                    sym.parent_name = parent_name.map(String::from);
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
// Class extraction
// ---------------------------------------------------------------------------

fn extract_class(node: Node, source: &str) -> Option<TsSymbol> {
    let name_node = node.child_by_field_name("name")?;
    let name = node_text(name_node, source);
    if name.is_empty() {
        return None;
    }

    let is_exported = has_modifier(node, source, "public");
    let is_abstract = has_modifier(node, source, "abstract");
    let base_classes = extract_superclass_and_interfaces(node, source);
    let body = node.child_by_field_name("body");
    let body_text = body.map(|b| node_text(b, source));

    let mut sig = String::new();
    if is_exported {
        sig.push_str("public ");
    }
    if is_abstract {
        sig.push_str("abstract ");
    }
    sig.push_str("class ");
    sig.push_str(&name);
    if !base_classes.is_empty() {
        sig.push_str(" extends/implements ");
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
    let base_classes = extract_extends_interfaces(node, source);
    let body = node.child_by_field_name("body");
    let body_text = body.map(|b| node_text(b, source));

    let mut sig = String::new();
    if is_exported {
        sig.push_str("public ");
    }
    sig.push_str("interface ");
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
    let base_classes = extract_implements(node, source);
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
        base_classes,
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
    let is_static = has_modifier(node, source, "static");

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

    let signature = build_method_signature(
        return_type.as_deref(),
        &name,
        &params,
        is_exported,
        is_abstract,
        is_static,
    );

    // Detect entry point: public static void main(String[] args)
    let is_entry_point =
        is_exported && is_static && name == "main" && return_type.as_deref() == Some("void");

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
        is_async: false,
        is_test: false,
        is_entry_point,
        body_text,
    })
}

// ---------------------------------------------------------------------------
// Constructor extraction
// ---------------------------------------------------------------------------

fn extract_constructor(node: Node, source: &str, parent_name: Option<&str>) -> Option<TsSymbol> {
    let name_node = node.child_by_field_name("name")?;
    let name = node_text(name_node, source);
    if name.is_empty() {
        return None;
    }

    let is_exported = has_modifier(node, source, "public");
    let params = extract_parameters(node, source);
    let body = node.child_by_field_name("body");
    let body_text = body.map(|b| node_text(b, source));

    let mut sig = String::new();
    if is_exported {
        sig.push_str("public ");
    }
    sig.push_str(&name);
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

    Some(TsSymbol {
        name,
        label: "Method".into(),
        start_line: node.start_position().row as i32 + 1,
        end_line: node.end_position().row as i32 + 1,
        parent_name: parent_name.map(String::from),
        signature: Some(sig),
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
// Field extraction
// ---------------------------------------------------------------------------

fn extract_field(node: Node, source: &str, parent_name: Option<&str>) -> Option<TsSymbol> {
    // Fields have a type and declarator(s)
    let type_node = node.child_by_field_name("type")?;
    let type_name = node_text(type_node, source).trim().to_string();

    // Find the variable declarator to get the field name
    let declarator = node.child_by_field_name("declarator")?;
    let name_node = declarator.child_by_field_name("name")?;
    let name = node_text(name_node, source);
    if name.is_empty() {
        return None;
    }

    let is_exported = has_modifier(node, source, "public");
    let is_static = has_modifier(node, source, "static");

    let mut sig = String::new();
    if is_exported {
        sig.push_str("public ");
    }
    if is_static {
        sig.push_str("static ");
    }
    sig.push_str(&type_name);
    sig.push(' ');
    sig.push_str(&name);

    Some(TsSymbol {
        name,
        label: "Constant".into(), // Fields are stored as constants
        start_line: node.start_position().row as i32 + 1,
        end_line: node.end_position().row as i32 + 1,
        parent_name: parent_name.map(String::from),
        signature: Some(sig),
        return_type: Some(type_name),
        parameters: Vec::new(),
        docstring: None,
        decorators: Vec::new(),
        base_classes: Vec::new(),
        is_exported,
        is_abstract: false,
        is_async: false,
        is_test: false,
        is_entry_point: false,
        body_text: None,
    })
}

// ---------------------------------------------------------------------------
// Annotation type declaration extraction
// ---------------------------------------------------------------------------

fn extract_annotation_type(node: Node, source: &str) -> Option<TsSymbol> {
    let name_node = node.child_by_field_name("name")?;
    let name = node_text(name_node, source);
    if name.is_empty() {
        return None;
    }

    let is_exported = has_modifier(node, source, "public");

    Some(TsSymbol {
        name: name.clone(),
        label: "Interface".into(),
        start_line: node.start_position().row as i32 + 1,
        end_line: node.end_position().row as i32 + 1,
        parent_name: None,
        signature: Some(format!("@interface {}", name)),
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
    for i in 0..params_node.named_child_count() {
        let child = match params_node.named_child(i) {
            Some(c) => c,
            None => continue,
        };
        if child.kind() == "formal_parameter" || child.kind() == "spread_parameter" {
            let ptype = child
                .child_by_field_name("type")
                .map(|t| node_text(t, source).trim().to_string());
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
// Inheritance extraction
// ---------------------------------------------------------------------------

/// Extract superclass and implemented interfaces from a class declaration.
fn extract_superclass_and_interfaces(node: Node, source: &str) -> Vec<String> {
    let mut bases = Vec::new();

    // superclass: `extends Foo` — the superclass node wraps a type_identifier
    if let Some(sc) = node.child_by_field_name("superclass") {
        // Find the type_identifier or generic_type inside the superclass node
        for i in 0..sc.named_child_count() {
            if let Some(child) = sc.named_child(i) {
                if child.kind() == "type_identifier" || child.kind() == "generic_type" {
                    let text = node_text(child, source).trim().to_string();
                    if !text.is_empty() {
                        bases.push(text);
                    }
                }
            }
        }
    }

    // interfaces: `implements Foo, Bar` — contains a type_list
    if let Some(ifaces) = node.child_by_field_name("interfaces") {
        extract_type_list_from_super_interfaces(ifaces, source, &mut bases);
    }

    bases
}

/// Extract extended interfaces from an interface declaration.
fn extract_extends_interfaces(node: Node, source: &str) -> Vec<String> {
    let mut bases = Vec::new();
    // In tree-sitter-java, interface extends uses `extends_interfaces` node (no field name)
    for i in 0..node.child_count() {
        let child = match node.child(i) {
            Some(c) => c,
            None => continue,
        };
        if child.kind() == "extends_interfaces" || child.kind() == "super_interfaces" {
            extract_type_list_from_super_interfaces(child, source, &mut bases);
        }
    }
    bases
}

/// Extract implemented interfaces from an enum declaration.
fn extract_implements(node: Node, source: &str) -> Vec<String> {
    let mut bases = Vec::new();
    if let Some(ifaces) = node.child_by_field_name("interfaces") {
        extract_type_list_from_super_interfaces(ifaces, source, &mut bases);
    }
    // Also check by iterating children for super_interfaces without field name
    if bases.is_empty() {
        for i in 0..node.child_count() {
            let child = match node.child(i) {
                Some(c) => c,
                None => continue,
            };
            if child.kind() == "super_interfaces" {
                extract_type_list_from_super_interfaces(child, source, &mut bases);
            }
        }
    }
    bases
}

/// Extract type names from a super_interfaces or extends_interfaces node.
/// These contain a `type_list` child with type_identifier children.
fn extract_type_list_from_super_interfaces(node: Node, source: &str, bases: &mut Vec<String>) {
    for i in 0..node.named_child_count() {
        let child = match node.named_child(i) {
            Some(c) => c,
            None => continue,
        };
        if child.kind() == "type_list" {
            extract_type_list(child, source, bases);
        } else if child.kind() == "type_identifier" || child.kind() == "generic_type" {
            let text = node_text(child, source).trim().to_string();
            if !text.is_empty() {
                bases.push(text);
            }
        }
    }
}

/// Extract type names from a type_list node (used in implements/extends clauses).
fn extract_type_list(node: Node, source: &str, bases: &mut Vec<String>) {
    for i in 0..node.named_child_count() {
        let child = match node.named_child(i) {
            Some(c) => c,
            None => continue,
        };
        let text = node_text(child, source).trim().to_string();
        if !text.is_empty() {
            bases.push(text);
        }
    }
}

// ---------------------------------------------------------------------------
// Modifier helpers
// ---------------------------------------------------------------------------

fn find_modifiers_node(node: Node) -> Option<Node> {
    for i in 0..node.child_count() {
        if let Some(child) = node.child(i) {
            if child.kind() == "modifiers" {
                return Some(child);
            }
        }
    }
    None
}

fn has_modifier(node: Node, source: &str, keyword: &str) -> bool {
    if let Some(mods) = find_modifiers_node(node) {
        for i in 0..mods.child_count() {
            let child = match mods.child(i) {
                Some(c) => c,
                None => continue,
            };
            if child.kind() == keyword {
                return true;
            }
            // Some modifiers are just text nodes
            let text = node_text(child, source);
            if text.trim() == keyword {
                return true;
            }
        }
    }
    false
}

// ---------------------------------------------------------------------------
// Annotation extraction
// ---------------------------------------------------------------------------

fn collect_annotations(node: Node, source: &str) -> Vec<String> {
    let mut annotations = Vec::new();

    if let Some(mods) = find_modifiers_node(node) {
        for i in 0..mods.child_count() {
            let child = match mods.child(i) {
                Some(c) => c,
                None => continue,
            };
            match child.kind() {
                "marker_annotation" | "annotation" => {
                    if let Some(name_node) = child.child_by_field_name("name") {
                        let ann_name = node_text(name_node, source);
                        if !ann_name.is_empty() {
                            annotations.push(ann_name);
                        }
                    }
                }
                _ => {}
            }
        }
    }

    annotations
}

// ---------------------------------------------------------------------------
// Comment / Javadoc extraction
// ---------------------------------------------------------------------------

fn collect_preceding_comments(node: Node, source: &str) -> Option<String> {
    let mut lines = Vec::new();
    let mut sibling = node.prev_sibling();

    while let Some(sib) = sibling {
        let kind = sib.kind();
        match kind {
            "block_comment" => {
                let text = node_text(sib, source);
                // Javadoc: /** ... */
                if text.starts_with("/**") {
                    let doc = parse_javadoc(&text);
                    if !doc.is_empty() {
                        lines.push(doc);
                    }
                }
                sibling = sib.prev_sibling();
            }
            "line_comment" => {
                let text = node_text(sib, source);
                let trimmed = text.trim();
                if let Some(s) = trimmed.strip_prefix("//") {
                    lines.push(s.strip_prefix(' ').unwrap_or(s).to_string());
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

/// Parse a Javadoc comment block into clean text.
fn parse_javadoc(text: &str) -> String {
    let mut lines = Vec::new();
    for line in text.lines() {
        let trimmed = line.trim();
        // Skip opening /** and closing */
        if trimmed == "/**" || trimmed == "*/" {
            continue;
        }
        // Strip leading * from Javadoc lines
        let content = if let Some(s) = trimmed.strip_prefix("*") {
            s.strip_prefix(' ').unwrap_or(s)
        } else if let Some(s) = trimmed.strip_prefix("/**") {
            s.strip_prefix(' ').unwrap_or(s)
        } else {
            trimmed
        };
        if !content.is_empty() {
            lines.push(content.to_string());
        }
    }
    lines.join("\n")
}

// ---------------------------------------------------------------------------
// Signature building
// ---------------------------------------------------------------------------

fn build_method_signature(
    return_type: Option<&str>,
    name: &str,
    params: &[TsParam],
    is_public: bool,
    is_abstract: bool,
    is_static: bool,
) -> String {
    let mut sig = String::new();
    if is_public {
        sig.push_str("public ");
    }
    if is_abstract {
        sig.push_str("abstract ");
    }
    if is_static {
        sig.push_str("static ");
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

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use tree_sitter::Parser;

    fn parse_java(source: &str) -> tree_sitter::Tree {
        let mut parser = Parser::new();
        let lang: tree_sitter::Language = tree_sitter_java::LANGUAGE.into();
        parser.set_language(&lang).unwrap();
        parser.parse(source, None).unwrap()
    }

    #[test]
    fn test_class_extraction() {
        let src = r#"
public class UserService {
    public void doSomething() {}
}
"#;
        let tree = parse_java(src);
        let symbols = walk_tree(&tree, src);

        let cls = symbols
            .iter()
            .find(|s| s.name == "UserService" && s.label == "Class");
        assert!(cls.is_some(), "Class UserService not found");
        let cls = cls.unwrap();
        assert!(cls.is_exported);
        assert!(!cls.is_abstract);
    }

    #[test]
    fn test_abstract_class_with_inheritance() {
        let src = r#"
public abstract class BaseController extends AbstractHandler implements Serializable {
    public abstract void handle();
}
"#;
        let tree = parse_java(src);
        let symbols = walk_tree(&tree, src);

        let cls = symbols
            .iter()
            .find(|s| s.name == "BaseController" && s.label == "Class");
        assert!(cls.is_some(), "Class BaseController not found");
        let cls = cls.unwrap();
        assert!(cls.is_exported);
        assert!(cls.is_abstract);
        assert!(!cls.base_classes.is_empty(), "Expected base classes");
    }

    #[test]
    fn test_interface_extraction() {
        let src = r#"
public interface UserRepository {
    User findById(Long id);
    void save(User user);
}
"#;
        let tree = parse_java(src);
        let symbols = walk_tree(&tree, src);

        let iface = symbols
            .iter()
            .find(|s| s.name == "UserRepository" && s.label == "Interface");
        assert!(iface.is_some(), "Interface UserRepository not found");
        let iface = iface.unwrap();
        assert!(iface.is_exported);
        assert!(iface.is_abstract);

        let methods: Vec<_> = symbols
            .iter()
            .filter(|s| s.label == "Method" && s.parent_name.as_deref() == Some("UserRepository"))
            .collect();
        assert!(
            methods.len() >= 2,
            "Expected at least 2 methods, got {}",
            methods.len()
        );
    }

    #[test]
    fn test_enum_extraction() {
        let src = r#"
public enum Status {
    ACTIVE,
    INACTIVE,
    PENDING;

    public String display() {
        return name().toLowerCase();
    }
}
"#;
        let tree = parse_java(src);
        let symbols = walk_tree(&tree, src);

        let en = symbols
            .iter()
            .find(|s| s.name == "Status" && s.label == "Enum");
        assert!(en.is_some(), "Enum Status not found");
        let en = en.unwrap();
        assert!(en.is_exported);
    }

    #[test]
    fn test_method_extraction_with_params() {
        let src = r#"
public class Calculator {
    public int add(int a, int b) {
        return a + b;
    }

    private static double multiply(double x, double y) {
        return x * y;
    }
}
"#;
        let tree = parse_java(src);
        let symbols = walk_tree(&tree, src);

        let add = symbols
            .iter()
            .find(|s| s.name == "add" && s.label == "Method");
        assert!(add.is_some(), "Method add not found");
        let add = add.unwrap();
        assert!(add.is_exported);
        assert_eq!(add.parameters.len(), 2);
        assert_eq!(add.parameters[0].name, "a");
        assert_eq!(add.parameters[0].type_name.as_deref(), Some("int"));
        assert_eq!(add.return_type.as_deref(), Some("int"));
        assert_eq!(add.parent_name.as_deref(), Some("Calculator"));

        let mul = symbols
            .iter()
            .find(|s| s.name == "multiply" && s.label == "Method");
        assert!(mul.is_some(), "Method multiply not found");
        let mul = mul.unwrap();
        assert!(!mul.is_exported); // private
    }

    #[test]
    fn test_constructor_extraction() {
        let src = r#"
public class Person {
    private String name;

    public Person(String name) {
        this.name = name;
    }
}
"#;
        let tree = parse_java(src);
        let symbols = walk_tree(&tree, src);

        let ctor = symbols
            .iter()
            .find(|s| s.name == "Person" && s.label == "Method" && s.parameters.len() == 1);
        assert!(ctor.is_some(), "Constructor Person not found");
        let ctor = ctor.unwrap();
        assert!(ctor.is_exported);
        assert_eq!(ctor.parameters[0].name, "name");
        assert_eq!(ctor.parameters[0].type_name.as_deref(), Some("String"));
    }

    #[test]
    fn test_spring_annotations() {
        let src = r#"
@RestController
@RequestMapping("/api/users")
public class UserController {

    @GetMapping("/{id}")
    public User getUser(@PathVariable Long id) {
        return null;
    }

    @PostMapping
    public User createUser(@RequestBody User user) {
        return null;
    }
}
"#;
        let tree = parse_java(src);
        let symbols = walk_tree(&tree, src);

        let cls = symbols
            .iter()
            .find(|s| s.name == "UserController" && s.label == "Class");
        assert!(cls.is_some(), "Class UserController not found");
        let cls = cls.unwrap();
        assert!(
            cls.decorators.iter().any(|d| d == "RestController"),
            "Expected @RestController"
        );
        assert!(
            cls.decorators.iter().any(|d| d == "RequestMapping"),
            "Expected @RequestMapping"
        );

        let get_method = symbols
            .iter()
            .find(|s| s.name == "getUser" && s.label == "Method");
        assert!(get_method.is_some(), "Method getUser not found");
        let get_method = get_method.unwrap();
        assert!(
            get_method.decorators.iter().any(|d| d == "GetMapping"),
            "Expected @GetMapping"
        );

        let post_method = symbols
            .iter()
            .find(|s| s.name == "createUser" && s.label == "Method");
        assert!(post_method.is_some(), "Method createUser not found");
        let post_method = post_method.unwrap();
        assert!(
            post_method.decorators.iter().any(|d| d == "PostMapping"),
            "Expected @PostMapping"
        );
    }

    #[test]
    fn test_service_annotation() {
        let src = r#"
@Service
public class OrderService {
    @Autowired
    private OrderRepository repository;

    @Transactional
    public Order processOrder(Order order) {
        return repository.save(order);
    }
}
"#;
        let tree = parse_java(src);
        let symbols = walk_tree(&tree, src);

        let cls = symbols
            .iter()
            .find(|s| s.name == "OrderService" && s.label == "Class");
        assert!(cls.is_some(), "Class OrderService not found");
        let cls = cls.unwrap();
        assert!(
            cls.decorators.iter().any(|d| d == "Service"),
            "Expected @Service"
        );

        let method = symbols
            .iter()
            .find(|s| s.name == "processOrder" && s.label == "Method");
        assert!(method.is_some(), "Method processOrder not found");
        let method = method.unwrap();
        assert!(
            method.decorators.iter().any(|d| d == "Transactional"),
            "Expected @Transactional"
        );
    }

    #[test]
    fn test_test_annotation() {
        let src = r#"
public class UserServiceTest {
    @Test
    public void testCreateUser() {
        // test body
    }

    @ParameterizedTest
    public void testWithParams(String input) {
        // test body
    }
}
"#;
        let tree = parse_java(src);
        let symbols = walk_tree(&tree, src);

        let test_method = symbols
            .iter()
            .find(|s| s.name == "testCreateUser" && s.label == "Method");
        assert!(test_method.is_some(), "Method testCreateUser not found");
        assert!(
            test_method.unwrap().is_test,
            "Expected is_test=true for @Test method"
        );

        let param_test = symbols
            .iter()
            .find(|s| s.name == "testWithParams" && s.label == "Method");
        assert!(param_test.is_some(), "Method testWithParams not found");
        assert!(
            param_test.unwrap().is_test,
            "Expected is_test=true for @ParameterizedTest method"
        );
    }

    #[test]
    fn test_entry_point_detection() {
        let src = r#"
public class Main {
    public static void main(String[] args) {
        System.out.println("Hello");
    }
}
"#;
        let tree = parse_java(src);
        let symbols = walk_tree(&tree, src);

        let main = symbols
            .iter()
            .find(|s| s.name == "main" && s.label == "Method");
        assert!(main.is_some(), "Method main not found");
        assert!(
            main.unwrap().is_entry_point,
            "Expected is_entry_point=true for main"
        );
    }

    #[test]
    fn test_javadoc_extraction() {
        let src = r#"
/**
 * Represents a user in the system.
 * Contains user profile information.
 */
public class User {
    /**
     * Get the user's full name.
     * @return the full name
     */
    public String getFullName() {
        return "";
    }
}
"#;
        let tree = parse_java(src);
        let symbols = walk_tree(&tree, src);

        let cls = symbols
            .iter()
            .find(|s| s.name == "User" && s.label == "Class");
        assert!(cls.is_some(), "Class User not found");
        let doc = cls
            .unwrap()
            .docstring
            .as_ref()
            .expect("Expected docstring on User");
        assert!(
            doc.contains("Represents a user"),
            "Docstring should contain class description"
        );

        let method = symbols
            .iter()
            .find(|s| s.name == "getFullName" && s.label == "Method");
        assert!(method.is_some(), "Method getFullName not found");
        let doc = method
            .unwrap()
            .docstring
            .as_ref()
            .expect("Expected docstring on getFullName");
        assert!(
            doc.contains("Get the user's full name"),
            "Docstring should contain method description"
        );
    }

    #[test]
    fn test_repository_annotation() {
        let src = r#"
@Repository
public interface UserRepository {
    User findByEmail(String email);
}
"#;
        let tree = parse_java(src);
        let symbols = walk_tree(&tree, src);

        let iface = symbols
            .iter()
            .find(|s| s.name == "UserRepository" && s.label == "Interface");
        assert!(iface.is_some(), "Interface UserRepository not found");
        let iface = iface.unwrap();
        assert!(
            iface.decorators.iter().any(|d| d == "Repository"),
            "Expected @Repository"
        );
    }

    #[test]
    fn test_is_exported_based_on_public() {
        let src = r#"
class PackagePrivateClass {
    void packageMethod() {}
    public void publicMethod() {}
}
"#;
        let tree = parse_java(src);
        let symbols = walk_tree(&tree, src);

        let cls = symbols
            .iter()
            .find(|s| s.name == "PackagePrivateClass" && s.label == "Class");
        assert!(cls.is_some());
        assert!(
            !cls.unwrap().is_exported,
            "Package-private class should not be exported"
        );

        let pkg_method = symbols.iter().find(|s| s.name == "packageMethod");
        assert!(pkg_method.is_some());
        assert!(
            !pkg_method.unwrap().is_exported,
            "Package-private method should not be exported"
        );

        let pub_method = symbols.iter().find(|s| s.name == "publicMethod");
        assert!(pub_method.is_some());
        assert!(
            pub_method.unwrap().is_exported,
            "Public method should be exported"
        );
    }
}
