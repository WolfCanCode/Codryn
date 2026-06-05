//! Kotlin AST walker.
//!
//! Extracts: class declarations, object declarations, data classes, interfaces,
//! function declarations, property declarations, annotations, inheritance.
//! Handles: visibility modifiers, abstract, open, data, sealed,
//! Spring Boot and Ktor annotations for framework detection.

use crate::{TsParam, TsSymbol};
use tree_sitter::{Node, TreeCursor};

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

/// Walk a Kotlin tree-sitter AST and extract symbols.
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
            "object_declaration" => {
                let doc = collect_preceding_comments(node, source);
                let annotations = collect_annotations(node, source);
                if let Some(mut sym) = extract_object(node, source) {
                    sym.docstring = doc.or(sym.docstring);
                    sym.decorators = annotations;
                    sym.parent_name = parent_name.map(String::from);
                    let name = sym.name.clone();
                    symbols.push(sym);
                    visit_children(cursor, source, symbols, Some(&name));
                }
            }
            "function_declaration" => {
                let doc = collect_preceding_comments(node, source);
                let annotations = collect_annotations(node, source);
                if let Some(mut sym) = extract_function(node, source, parent_name) {
                    sym.docstring = doc.or(sym.docstring);
                    sym.decorators = annotations.clone();
                    sym.is_test = annotations
                        .iter()
                        .any(|a| a == "Test" || a == "ParameterizedTest");
                    symbols.push(sym);
                }
                visit_children(cursor, source, symbols, parent_name);
            }
            "property_declaration" => {
                let annotations = collect_annotations(node, source);
                if let Some(mut sym) = extract_property(node, source, parent_name) {
                    sym.decorators = annotations;
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
// Class extraction (handles class, data class, sealed class, interface, enum)
// ---------------------------------------------------------------------------

fn extract_class(node: Node, source: &str) -> Option<TsSymbol> {
    let name = find_name(node, source)?;
    if name.is_empty() {
        return None;
    }

    let is_data = has_modifier_keyword(node, source, "data");
    let is_sealed = has_modifier_keyword(node, source, "sealed");
    let is_abstract = has_modifier_keyword(node, source, "abstract");
    let is_interface = is_interface_class(node, source);
    let is_enum = is_enum_class(node, source);
    let is_exported = !has_modifier_keyword(node, source, "private")
        && !has_modifier_keyword(node, source, "internal");

    let base_classes = extract_supertypes(node, source);
    let body = find_class_body(node);
    let body_text = body.map(|b| node_text(b, source));

    let label = if is_interface {
        "Interface"
    } else if is_enum {
        "Enum"
    } else {
        "Class"
    };

    let mut sig = String::new();
    if is_exported {
        // Kotlin doesn't require explicit public, but we note visibility
    }
    if is_abstract {
        sig.push_str("abstract ");
    }
    if is_sealed {
        sig.push_str("sealed ");
    }
    if is_data {
        sig.push_str("data ");
    }
    if is_interface {
        sig.push_str("interface ");
    } else if is_enum {
        sig.push_str("enum class ");
    } else {
        sig.push_str("class ");
    }
    sig.push_str(&name);
    if !base_classes.is_empty() {
        sig.push_str(" : ");
        sig.push_str(&base_classes.join(", "));
    }

    Some(TsSymbol {
        name,
        label: label.into(),
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
        is_abstract: is_abstract || is_interface,
        is_async: false,
        is_test: false,
        is_entry_point: false,
        body_text,
    })
}

// ---------------------------------------------------------------------------
// Object declaration extraction (companion object, singleton)
// ---------------------------------------------------------------------------

fn extract_object(node: Node, source: &str) -> Option<TsSymbol> {
    let name = find_name(node, source).unwrap_or_else(|| "companion".to_string());
    if name.is_empty() {
        return None;
    }

    let is_exported = !has_modifier_keyword(node, source, "private")
        && !has_modifier_keyword(node, source, "internal");
    let base_classes = extract_supertypes(node, source);
    let body = find_class_body(node);
    let body_text = body.map(|b| node_text(b, source));

    let mut sig = String::new();
    sig.push_str("object ");
    sig.push_str(&name);
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
        is_exported,
        is_abstract: false,
        is_async: false,
        is_test: false,
        is_entry_point: false,
        body_text,
    })
}

// ---------------------------------------------------------------------------
// Function extraction
// ---------------------------------------------------------------------------

fn extract_function(node: Node, source: &str, parent_name: Option<&str>) -> Option<TsSymbol> {
    let name = find_name(node, source)?;
    if name.is_empty() {
        return None;
    }

    let is_exported = !has_modifier_keyword(node, source, "private")
        && !has_modifier_keyword(node, source, "internal");
    let is_abstract = has_modifier_keyword(node, source, "abstract");
    let is_suspend = has_modifier_keyword(node, source, "suspend");

    let return_type = find_return_type(node, source);
    let params = extract_parameters(node, source);
    let body = find_function_body(node);
    let body_text = body.map(|b| node_text(b, source));

    let label = if parent_name.is_some() {
        "Method"
    } else {
        "Function"
    };

    let signature = build_function_signature(&name, &params, return_type.as_deref(), is_suspend);

    // Detect entry point: fun main(args: Array<String>) or fun main()
    let is_entry_point = parent_name.is_none() && name == "main";

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
        is_async: is_suspend,
        is_test: false,
        is_entry_point,
        body_text,
    })
}

// ---------------------------------------------------------------------------
// Property extraction
// ---------------------------------------------------------------------------

fn extract_property(node: Node, source: &str, parent_name: Option<&str>) -> Option<TsSymbol> {
    // Properties have a variable_declaration child with the name
    let var_decl = find_child_by_kind(node, "variable_declaration")?;
    let name_node = find_child_by_kind(var_decl, "simple_identifier")?;
    let name = node_text(name_node, source);
    if name.is_empty() {
        return None;
    }

    let is_exported = !has_modifier_keyword(node, source, "private")
        && !has_modifier_keyword(node, source, "internal");

    // Determine if val or var
    let is_val = has_child_with_text(node, source, "val");

    // Try to find type annotation
    let type_name = find_property_type(node, source);

    let mut sig = String::new();
    if is_val {
        sig.push_str("val ");
    } else {
        sig.push_str("var ");
    }
    sig.push_str(&name);
    if let Some(ref t) = type_name {
        sig.push_str(": ");
        sig.push_str(t);
    }

    Some(TsSymbol {
        name,
        label: "Constant".into(),
        start_line: node.start_position().row as i32 + 1,
        end_line: node.end_position().row as i32 + 1,
        parent_name: parent_name.map(String::from),
        signature: Some(sig),
        return_type: type_name,
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
    let params_node = match find_child_by_kind(node, "function_value_parameters") {
        Some(n) => n,
        None => return Vec::new(),
    };

    let mut params = Vec::new();
    for i in 0..params_node.named_child_count() {
        let child = match params_node.named_child(i) {
            Some(c) => c,
            None => continue,
        };
        if child.kind() == "parameter" {
            let pname = find_child_by_kind(child, "simple_identifier")
                .or_else(|| find_child_by_kind(child, "identifier"))
                .map(|n| node_text(n, source))
                .unwrap_or_default();
            let ptype = find_parameter_type(child, source);
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
// Inheritance / supertypes extraction
// ---------------------------------------------------------------------------

fn extract_supertypes(node: Node, source: &str) -> Vec<String> {
    let mut bases = Vec::new();
    // In tree-sitter-kotlin-ng, supertypes are in a `delegation_specifier` list
    if let Some(delegation) = find_child_by_kind(node, "delegation_specifiers") {
        for i in 0..delegation.named_child_count() {
            if let Some(child) = delegation.named_child(i) {
                // delegation_specifier contains user_type or constructor_invocation
                let text = extract_type_name_from_delegation(child, source);
                if !text.is_empty() {
                    bases.push(text);
                }
            }
        }
    }
    bases
}

fn extract_type_name_from_delegation(node: Node, source: &str) -> String {
    // delegation_specifier -> constructor_invocation -> user_type
    // or delegation_specifier -> user_type
    match node.kind() {
        "delegation_specifier" => {
            for i in 0..node.named_child_count() {
                if let Some(child) = node.named_child(i) {
                    let result = extract_type_name_from_delegation(child, source);
                    if !result.is_empty() {
                        return result;
                    }
                }
            }
            String::new()
        }
        "constructor_invocation" => {
            // First child is user_type
            if let Some(ut) = node.named_child(0) {
                if ut.kind() == "user_type" {
                    return extract_simple_type_name(ut, source);
                }
            }
            String::new()
        }
        "user_type" => extract_simple_type_name(node, source),
        "explicit_delegation" => {
            // explicit_delegation -> user_type
            for i in 0..node.named_child_count() {
                if let Some(child) = node.named_child(i) {
                    if child.kind() == "user_type" {
                        return extract_simple_type_name(child, source);
                    }
                }
            }
            String::new()
        }
        _ => {
            // Fallback: just get the text
            let text = node_text(node, source).trim().to_string();
            // Strip constructor args if present
            if let Some(idx) = text.find('(') {
                text[..idx].trim().to_string()
            } else {
                text
            }
        }
    }
}

fn extract_simple_type_name(node: Node, source: &str) -> String {
    // user_type -> type_identifier (simple_identifier)
    let text = node_text(node, source).trim().to_string();
    // Strip generic type parameters if present
    if let Some(idx) = text.find('<') {
        text[..idx].trim().to_string()
    } else {
        text
    }
}

// ---------------------------------------------------------------------------
// Modifier / keyword helpers
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

fn has_modifier_keyword(node: Node, source: &str, keyword: &str) -> bool {
    if let Some(mods) = find_modifiers_node(node) {
        for i in 0..mods.child_count() {
            let child = match mods.child(i) {
                Some(c) => c,
                None => continue,
            };
            // Modifiers can be visibility_modifier, inheritance_modifier, etc.
            let text = node_text(child, source);
            if text.trim() == keyword {
                return true;
            }
            // Check children of modifier nodes
            for j in 0..child.child_count() {
                if let Some(gc) = child.child(j) {
                    if node_text(gc, source).trim() == keyword {
                        return true;
                    }
                }
            }
        }
    }
    false
}

fn is_interface_class(node: Node, source: &str) -> bool {
    // In Kotlin, interface is a class_declaration with "interface" keyword
    for i in 0..node.child_count() {
        if let Some(child) = node.child(i) {
            if node_text(child, source).trim() == "interface" {
                return true;
            }
        }
    }
    false
}

fn is_enum_class(node: Node, source: &str) -> bool {
    // enum class has "enum" in modifiers
    has_modifier_keyword(node, source, "enum")
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
            if child.kind() == "annotation" {
                extract_annotation_name(child, source, &mut annotations);
            }
        }
    }

    annotations
}

fn extract_annotation_name(node: Node, source: &str, annotations: &mut Vec<String>) {
    // annotation -> user_type or constructor_invocation
    for i in 0..node.child_count() {
        let child = match node.child(i) {
            Some(c) => c,
            None => continue,
        };
        match child.kind() {
            "user_type" => {
                let text = node_text(child, source).trim().to_string();
                if !text.is_empty() {
                    annotations.push(text);
                }
            }
            "constructor_invocation" => {
                // First child is user_type
                if let Some(ut) = child.named_child(0) {
                    if ut.kind() == "user_type" {
                        let text = node_text(ut, source).trim().to_string();
                        if !text.is_empty() {
                            annotations.push(text);
                        }
                    }
                }
            }
            _ => {}
        }
    }
}

// ---------------------------------------------------------------------------
// Comment / KDoc extraction
// ---------------------------------------------------------------------------

fn collect_preceding_comments(node: Node, source: &str) -> Option<String> {
    let mut lines = Vec::new();
    let mut sibling = node.prev_sibling();

    while let Some(sib) = sibling {
        let kind = sib.kind();
        match kind {
            "multiline_comment" | "block_comment" => {
                let text = node_text(sib, source);
                // KDoc: /** ... */
                if text.starts_with("/**") {
                    let doc = parse_kdoc(&text);
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

/// Parse a KDoc comment block into clean text.
fn parse_kdoc(text: &str) -> String {
    let mut lines = Vec::new();
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed == "/**" || trimmed == "*/" {
            continue;
        }
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
// Return type extraction
// ---------------------------------------------------------------------------

fn find_return_type(node: Node, source: &str) -> Option<String> {
    // In Kotlin, return type comes after `:` following the parameters
    // Look for user_type or nullable_type after the colon that follows function_value_parameters
    let mut past_params = false;
    let mut past_colon = false;
    for i in 0..node.child_count() {
        let child = match node.child(i) {
            Some(c) => c,
            None => continue,
        };
        if child.kind() == "function_value_parameters" {
            past_params = true;
            continue;
        }
        if past_params && child.kind() == ":" {
            past_colon = true;
            continue;
        }
        if past_colon
            && (child.kind() == "user_type"
                || child.kind() == "nullable_type"
                || child.kind() == "type_identifier")
        {
            let text = node_text(child, source).trim().to_string();
            if !text.is_empty() {
                return Some(text);
            }
        }
        // Stop if we hit the function body
        if child.kind() == "function_body" {
            break;
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Property type extraction
// ---------------------------------------------------------------------------

fn find_property_type(node: Node, source: &str) -> Option<String> {
    // property_declaration -> variable_declaration -> ... : type
    // Look for user_type or nullable_type after `:` in the node
    let mut past_colon = false;
    for i in 0..node.child_count() {
        let child = match node.child(i) {
            Some(c) => c,
            None => continue,
        };
        if child.kind() == ":" {
            past_colon = true;
            continue;
        }
        if past_colon
            && (child.kind() == "user_type"
                || child.kind() == "nullable_type"
                || child.kind() == "type_identifier")
        {
            let text = node_text(child, source).trim().to_string();
            if !text.is_empty() {
                return Some(text);
            }
        }
        // Stop at assignment or property delegate
        if child.kind() == "=" || child.kind() == "property_delegate" {
            break;
        }
    }
    None
}

fn find_parameter_type(node: Node, source: &str) -> Option<String> {
    // parameter -> simple_identifier : user_type
    let mut past_colon = false;
    for i in 0..node.child_count() {
        let child = match node.child(i) {
            Some(c) => c,
            None => continue,
        };
        if child.kind() == ":" {
            past_colon = true;
            continue;
        }
        if past_colon
            && (child.kind() == "user_type"
                || child.kind() == "nullable_type"
                || child.kind() == "type_identifier")
        {
            let text = node_text(child, source).trim().to_string();
            if !text.is_empty() {
                return Some(text);
            }
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Signature building
// ---------------------------------------------------------------------------

fn build_function_signature(
    name: &str,
    params: &[TsParam],
    return_type: Option<&str>,
    is_suspend: bool,
) -> String {
    let mut sig = String::new();
    if is_suspend {
        sig.push_str("suspend ");
    }
    sig.push_str("fun ");
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
// Utility helpers
// ---------------------------------------------------------------------------

fn node_text(node: Node, source: &str) -> String {
    source.get(node.byte_range()).unwrap_or("").to_string()
}

fn find_name(node: Node, source: &str) -> Option<String> {
    for i in 0..node.child_count() {
        if let Some(child) = node.child(i) {
            match child.kind() {
                "type_identifier" | "simple_identifier" | "identifier" => {
                    let text = node_text(child, source);
                    if !text.is_empty() {
                        return Some(text);
                    }
                }
                _ => {}
            }
        }
    }
    None
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

fn find_class_body(node: Node) -> Option<Node> {
    find_child_by_kind(node, "class_body").or_else(|| find_child_by_kind(node, "enum_class_body"))
}

fn find_function_body(node: Node) -> Option<Node> {
    find_child_by_kind(node, "function_body")
}

fn has_child_with_text(node: Node, source: &str, text: &str) -> bool {
    for i in 0..node.child_count() {
        if let Some(child) = node.child(i) {
            if node_text(child, source).trim() == text {
                return true;
            }
        }
    }
    false
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use tree_sitter::Parser;

    fn parse_kotlin(source: &str) -> tree_sitter::Tree {
        let mut parser = Parser::new();
        let lang: tree_sitter::Language = tree_sitter_kotlin_ng::LANGUAGE.into();
        parser.set_language(&lang).unwrap();
        parser.parse(source, None).unwrap()
    }

    #[test]
    fn test_class_extraction() {
        let src = r#"
class UserService {
    fun doSomething() {}
}
"#;
        let tree = parse_kotlin(src);
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
    fn test_data_class() {
        let src = r#"
data class UserDto(
    val name: String,
    val email: String
)
"#;
        let tree = parse_kotlin(src);
        let symbols = walk_tree(&tree, src);

        let cls = symbols
            .iter()
            .find(|s| s.name == "UserDto" && s.label == "Class");
        assert!(cls.is_some(), "Data class UserDto not found");
        let cls = cls.unwrap();
        assert!(cls.signature.as_ref().unwrap().contains("data class"));
    }

    #[test]
    fn test_object_declaration() {
        let src = r#"
object DatabaseConfig {
    fun getConnection(): Connection {
        return pool.get()
    }
}
"#;
        let tree = parse_kotlin(src);
        let symbols = walk_tree(&tree, src);

        let obj = symbols
            .iter()
            .find(|s| s.name == "DatabaseConfig" && s.label == "Class");
        assert!(obj.is_some(), "Object DatabaseConfig not found");
        let obj = obj.unwrap();
        assert!(obj.signature.as_ref().unwrap().contains("object"));

        let method = symbols
            .iter()
            .find(|s| s.name == "getConnection" && s.label == "Method");
        assert!(method.is_some(), "Method getConnection not found");
        assert_eq!(
            method.unwrap().parent_name.as_deref(),
            Some("DatabaseConfig")
        );
    }

    #[test]
    fn test_interface_extraction() {
        let src = r#"
interface UserRepository {
    fun findById(id: Long): User?
    fun save(user: User): User
}
"#;
        let tree = parse_kotlin(src);
        let symbols = walk_tree(&tree, src);

        let iface = symbols
            .iter()
            .find(|s| s.name == "UserRepository" && s.label == "Interface");
        assert!(iface.is_some(), "Interface UserRepository not found");
        let iface = iface.unwrap();
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
    fn test_function_with_params_and_return_type() {
        let src = r#"
fun add(a: Int, b: Int): Int {
    return a + b
}
"#;
        let tree = parse_kotlin(src);
        let symbols = walk_tree(&tree, src);

        let f = symbols
            .iter()
            .find(|s| s.name == "add" && s.label == "Function");
        assert!(f.is_some(), "Function add not found");
        let f = f.unwrap();
        assert_eq!(f.parameters.len(), 2);
        assert_eq!(f.parameters[0].name, "a");
        assert_eq!(f.parameters[0].type_name.as_deref(), Some("Int"));
        assert_eq!(f.parameters[1].name, "b");
        assert_eq!(f.parameters[1].type_name.as_deref(), Some("Int"));
        assert_eq!(f.return_type.as_deref(), Some("Int"));
    }

    #[test]
    fn test_suspend_function() {
        let src = r#"
suspend fun fetchData(url: String): Response {
    return client.get(url)
}
"#;
        let tree = parse_kotlin(src);
        let symbols = walk_tree(&tree, src);

        let f = symbols
            .iter()
            .find(|s| s.name == "fetchData" && s.label == "Function");
        assert!(f.is_some(), "Function fetchData not found");
        let f = f.unwrap();
        assert!(f.is_async, "Expected is_async=true for suspend function");
        assert!(f.signature.as_ref().unwrap().contains("suspend"));
    }

    #[test]
    fn test_spring_annotations() {
        let src = r#"
@RestController
@RequestMapping("/api/users")
class UserController {

    @GetMapping("/{id}")
    fun getUser(@PathVariable id: Long): User {
        return userService.findById(id)
    }

    @PostMapping
    fun createUser(@RequestBody user: User): User {
        return userService.save(user)
    }
}
"#;
        let tree = parse_kotlin(src);
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
    fn test_ktor_annotations() {
        let src = r#"
@Location("/users")
class UserRoutes {
    @Location("/{id}")
    fun getUser(id: Int): User {
        return userService.findById(id)
    }
}
"#;
        let tree = parse_kotlin(src);
        let symbols = walk_tree(&tree, src);

        let cls = symbols
            .iter()
            .find(|s| s.name == "UserRoutes" && s.label == "Class");
        assert!(cls.is_some(), "Class UserRoutes not found");
        let cls = cls.unwrap();
        assert!(
            cls.decorators.iter().any(|d| d == "Location"),
            "Expected @Location annotation"
        );
    }

    #[test]
    fn test_inheritance() {
        let src = r#"
abstract class BaseService {
    abstract fun process(): Result
}

class OrderService : BaseService() {
    override fun process(): Result {
        return Result.success()
    }
}
"#;
        let tree = parse_kotlin(src);
        let symbols = walk_tree(&tree, src);

        let base = symbols
            .iter()
            .find(|s| s.name == "BaseService" && s.label == "Class");
        assert!(base.is_some(), "Class BaseService not found");
        assert!(base.unwrap().is_abstract);

        let order = symbols
            .iter()
            .find(|s| s.name == "OrderService" && s.label == "Class");
        assert!(order.is_some(), "Class OrderService not found");
        let order = order.unwrap();
        assert!(
            !order.base_classes.is_empty(),
            "Expected base classes for OrderService"
        );
        assert!(
            order.base_classes.iter().any(|b| b == "BaseService"),
            "Expected BaseService in base_classes"
        );
    }

    #[test]
    fn test_sealed_class() {
        let src = r#"
sealed class Result {
    data class Success(val data: String) : Result()
    data class Error(val message: String) : Result()
}
"#;
        let tree = parse_kotlin(src);
        let symbols = walk_tree(&tree, src);

        let sealed = symbols
            .iter()
            .find(|s| s.name == "Result" && s.label == "Class");
        assert!(sealed.is_some(), "Sealed class Result not found");
        let sealed = sealed.unwrap();
        assert!(sealed.signature.as_ref().unwrap().contains("sealed"));
    }

    #[test]
    fn test_entry_point_detection() {
        let src = r#"
fun main(args: Array<String>) {
    println("Hello")
}
"#;
        let tree = parse_kotlin(src);
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
    fn test_private_visibility() {
        let src = r#"
private class InternalHelper {
    private fun helperMethod() {}
    fun publicMethod() {}
}
"#;
        let tree = parse_kotlin(src);
        let symbols = walk_tree(&tree, src);

        let cls = symbols
            .iter()
            .find(|s| s.name == "InternalHelper" && s.label == "Class");
        assert!(cls.is_some());
        assert!(
            !cls.unwrap().is_exported,
            "Private class should not be exported"
        );

        let priv_method = symbols.iter().find(|s| s.name == "helperMethod");
        assert!(priv_method.is_some());
        assert!(
            !priv_method.unwrap().is_exported,
            "Private method should not be exported"
        );

        let pub_method = symbols.iter().find(|s| s.name == "publicMethod");
        assert!(pub_method.is_some());
        assert!(
            pub_method.unwrap().is_exported,
            "Public method should be exported"
        );
    }

    #[test]
    fn test_kdoc_extraction() {
        let src = r#"
/**
 * Represents a user in the system.
 * Contains user profile information.
 */
class User {
    /**
     * Get the user's full name.
     * @return the full name
     */
    fun getFullName(): String {
        return ""
    }
}
"#;
        let tree = parse_kotlin(src);
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
    fn test_enum_class() {
        let src = r#"
enum class Status {
    ACTIVE,
    INACTIVE,
    PENDING
}
"#;
        let tree = parse_kotlin(src);
        let symbols = walk_tree(&tree, src);

        let en = symbols
            .iter()
            .find(|s| s.name == "Status" && s.label == "Enum");
        assert!(en.is_some(), "Enum class Status not found");
        let en = en.unwrap();
        assert!(en.signature.as_ref().unwrap().contains("enum class"));
    }
}
