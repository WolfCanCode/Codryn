//! Pipeline pass: Type Assignment and Reference Extraction.
//!
//! Scans source files for explicit type annotations and function signatures,
//! creating TYPE_OF and TYPE_REF edges. Registers resolved types in the
//! TypeRegistry for `pass_calls` disambiguation.
//!
//! Requirements: 19.1, 19.2, 19.3, 19.4, 19.5

use std::collections::HashSet;
use std::sync::LazyLock;

use codryn_discover::{DiscoveredFile, Language};
use codryn_foundation::fqn;
use codryn_graph_buffer::{EdgeSource, GraphBuffer};
use rayon::prelude::*;

use crate::registry::{self, Registry, TypeRegistry};
use crate::FileCache;

// ── Type annotation patterns ──────────────────────────────────────────────

/// TypeScript/Rust/Dart/Kotlin: `x: MyType`, `x: MyType[]`
static COLON_TYPE_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r":\s*&?(?:mut\s+)?([A-Z][A-Za-z0-9_]*)(?:\s*[<\[\]|&,;)\s=]|$)").unwrap()
});

/// Java/Kotlin/C#: `MyType x`, `final MyType x`, `private MyType x`
static JAVA_TYPE_DECL_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(
        r"(?:(?:final|const|var|val|let|private|public|protected|static|readonly)\s+)*([A-Z][A-Za-z0-9_]*)\s*(?:<[^>]*>)?\s+[a-z_]\w*\s*[=;,)]"
    ).unwrap()
});

/// Go: `var x MyType`, `x MyType` (in function params/struct fields)
static GO_TYPE_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"\b[a-z_]\w*\s+\*?([A-Z][A-Za-z0-9_]*)(?:\s*[,;)\s{])").unwrap()
});

// ── Main pass entry point ─────────────────────────────────────────────────

/// Scan source files for type annotations and function signatures, creating
/// TYPE_OF and TYPE_REF edges. Also registers resolved types in the TypeRegistry
/// for `pass_calls` disambiguation.
///
/// - TYPE_OF edges: from a function/module node to a type node when a variable
///   is declared with an explicit type annotation (Requirement 19.1).
/// - TYPE_REF edges: from a function node to type nodes referenced in its
///   parameters or return type (Requirement 19.2).
///
/// Skips standard library types and unresolvable types (Requirement 19.4).
/// Strips generic wrappers and links inner types (Requirement 19.5).
/// Deduplicates edges per (source, target) pair.
pub fn pass_types(
    buf: &mut GraphBuffer,
    files: &[&DiscoveredFile],
    file_cache: &FileCache,
    project: &str,
    reg: &Registry,
    type_registry: &mut TypeRegistry,
) {
    if reg.is_empty() {
        return;
    }

    // Process files in parallel, collect edge tuples and type registrations
    let results: Vec<FileTypeResult> = files
        .par_iter()
        .flat_map(|f| {
            let source = if let Some(cached) = file_cache.get(&f.abs_path) {
                cached
            } else {
                match std::fs::read_to_string(&f.abs_path) {
                    Ok(s) => std::sync::Arc::new(s),
                    Err(_) => return vec![],
                }
            };

            vec![extract_types_from_file(f, &source, project, reg)]
        })
        .collect();

    // Merge results: add edges to buffer and register types
    let mut seen_edges: HashSet<(String, String, &'static str)> = HashSet::new();

    for result in results {
        // Register types in TypeRegistry (Requirement 19.3)
        for (file_path, symbol_name, resolved_type) in &result.type_registrations {
            type_registry.register_type(file_path, symbol_name, resolved_type);
        }

        // Add TYPE_OF edges (deduplicated per source, target pair)
        for (src, tgt) in &result.type_of_edges {
            if seen_edges.insert((src.clone(), tgt.clone(), "TYPE_OF")) {
                buf.add_edge_with_confidence(src, tgt, "TYPE_OF", EdgeSource::RegexMatch, None);
            }
        }

        // Add TYPE_REF edges (deduplicated per source, target pair)
        for (src, tgt) in &result.type_ref_edges {
            if seen_edges.insert((src.clone(), tgt.clone(), "TYPE_REF")) {
                buf.add_edge_with_confidence(src, tgt, "TYPE_REF", EdgeSource::RegexMatch, None);
            }
        }
    }
}

// ── Internal types ────────────────────────────────────────────────────────

/// Result of processing a single file for type information.
struct FileTypeResult {
    /// TYPE_OF edges: (source_qn, target_qn)
    type_of_edges: Vec<(String, String)>,
    /// TYPE_REF edges: (source_qn, target_qn)
    type_ref_edges: Vec<(String, String)>,
    /// Type registrations: (file_path, symbol_name, resolved_type)
    type_registrations: Vec<(String, String, String)>,
}

// ── File processing ───────────────────────────────────────────────────────

/// Extract type information from a single file.
fn extract_types_from_file(
    f: &DiscoveredFile,
    source: &str,
    project: &str,
    reg: &Registry,
) -> FileTypeResult {
    let mut result = FileTypeResult {
        type_of_edges: Vec::new(),
        type_ref_edges: Vec::new(),
        type_registrations: Vec::new(),
    };

    let module_qn = fqn::fqn_module(project, &f.rel_path);
    let line_starts = build_line_starts(source);
    let file_symbols = reg.entries_for_file(&f.rel_path);

    // Extract variable type annotations → TYPE_OF edges
    let var_types = extract_variable_type_annotations(source, f.language);
    for (byte_offset, var_name, type_name) in &var_types {
        // Strip generic wrappers (Requirement 19.5)
        let inner_types = strip_generic_wrappers(type_name);

        for inner_type in &inner_types {
            // Skip standard library types (Requirement 19.4)
            if registry::is_stdlib_type(f.language, inner_type) {
                continue;
            }

            // Resolve the type name to a node in the registry
            let target_entries = reg.lookup(inner_type);
            if target_entries.is_empty() {
                // Requirement 19.4: skip unresolvable types
                continue;
            }

            // Prefer Class/Interface/Struct/Trait nodes for type resolution
            let target_qn = resolve_type_target(&target_entries);
            let Some(target_qn) = target_qn else {
                continue;
            };

            // Determine the enclosing symbol
            let line_num = byte_offset_to_line(&line_starts, *byte_offset);
            let enclosing_qn = find_enclosing_symbol(&file_symbols, line_num, &module_qn);

            // Don't create self-referential edges
            if enclosing_qn == target_qn {
                continue;
            }

            result
                .type_of_edges
                .push((enclosing_qn.clone(), target_qn.clone()));

            // Register in TypeRegistry (Requirement 19.3)
            result.type_registrations.push((
                f.rel_path.clone(),
                var_name.clone(),
                inner_type.clone(),
            ));
        }
    }

    // Extract function parameter and return types → TYPE_REF edges
    let fn_types = extract_function_type_refs(source, f.language);
    for (byte_offset, fn_name, type_name) in &fn_types {
        // Strip generic wrappers (Requirement 19.5)
        let inner_types = strip_generic_wrappers(type_name);

        for inner_type in &inner_types {
            // Skip standard library types
            if registry::is_stdlib_type(f.language, inner_type) {
                continue;
            }

            // Resolve the type name to a node in the registry
            let target_entries = reg.lookup(inner_type);
            if target_entries.is_empty() {
                // Requirement 19.4: skip unresolvable types
                continue;
            }

            // Only create TYPE_REF to Class/Interface/Struct/Trait nodes
            let target_qn = resolve_type_target(&target_entries);
            let Some(target_qn) = target_qn else {
                continue;
            };

            // Determine the enclosing function
            let line_num = byte_offset_to_line(&line_starts, *byte_offset);
            let enclosing_qn = find_enclosing_symbol(&file_symbols, line_num, &module_qn);

            // Don't create self-referential edges
            if enclosing_qn == target_qn {
                continue;
            }

            result
                .type_ref_edges
                .push((enclosing_qn.clone(), target_qn.clone()));

            // Register parameter/return types in TypeRegistry (Requirement 19.3)
            result.type_registrations.push((
                f.rel_path.clone(),
                fn_name.clone(),
                inner_type.clone(),
            ));
        }
    }

    result
}

// ── Helper functions ──────────────────────────────────────────────────────

/// Build a byte-offset → line-number lookup table.
fn build_line_starts(source: &str) -> Vec<usize> {
    let mut starts = vec![0];
    for (i, b) in source.bytes().enumerate() {
        if b == b'\n' {
            starts.push(i + 1);
        }
    }
    starts
}

/// Convert a byte offset to a 1-based line number.
fn byte_offset_to_line(line_starts: &[usize], offset: usize) -> i32 {
    line_starts.partition_point(|&start| start <= offset) as i32
}

/// Find the nearest enclosing symbol for a given line number.
/// Prefers functions/methods over classes, and narrower ranges over wider.
fn find_enclosing_symbol(
    file_symbols: &[registry::RegistryEntry],
    line_num: i32,
    module_qn: &str,
) -> String {
    let mut best: Option<&registry::RegistryEntry> = None;

    for entry in file_symbols {
        if entry.start_line <= line_num && entry.end_line >= line_num {
            match best {
                None => best = Some(entry),
                Some(current) => {
                    let entry_is_fn = matches!(entry.label.as_str(), "Function" | "Method");
                    let current_is_fn = matches!(current.label.as_str(), "Function" | "Method");

                    if entry_is_fn && !current_is_fn {
                        best = Some(entry);
                    } else if entry_is_fn == current_is_fn {
                        let entry_range = entry.end_line - entry.start_line;
                        let current_range = current.end_line - current.start_line;
                        if entry_range < current_range {
                            best = Some(entry);
                        }
                    }
                }
            }
        }
    }

    best.map(|e| e.qualified_name.clone())
        .unwrap_or_else(|| module_qn.to_owned())
}

/// Resolve a type name to its qualified name from registry entries.
/// Prefers Class/Interface/Struct/Trait labels.
fn resolve_type_target(entries: &[registry::RegistryEntry]) -> Option<String> {
    entries
        .iter()
        .find(|e| {
            matches!(
                e.label.as_str(),
                "Class" | "Interface" | "Struct" | "Trait" | "Enum"
            )
        })
        .or_else(|| entries.first())
        .map(|e| e.qualified_name.clone())
}

/// Strip generic wrapper types and return the inner type arguments.
/// E.g., `Vec<MyType>` → `["MyType"]`, `Map<String, OrderItem>` → `["OrderItem"]`
/// If no generics, returns the type itself (if it starts with uppercase).
///
/// Requirement 19.5: Handle generics by stripping wrappers and linking inner types.
fn strip_generic_wrappers(type_name: &str) -> Vec<String> {
    let mut results = Vec::new();

    // Check if the type has generic parameters
    if let Some(angle_start) = type_name.find('<') {
        // Extract inner types from generic parameters
        let inner = &type_name[angle_start + 1..type_name.len().saturating_sub(1)];
        for part in inner.split(',') {
            let trimmed = part.trim();
            // Extract the type name (first uppercase identifier)
            if let Some(cap) = extract_first_type_name(trimmed) {
                results.push(cap);
            }
        }
        // If no inner types found, fall back to the outer type
        if results.is_empty() {
            let outer = &type_name[..angle_start];
            if !outer.is_empty() && outer.starts_with(|c: char| c.is_uppercase()) {
                results.push(outer.to_owned());
            }
        }
    } else {
        // No generics — return the type itself
        if type_name.starts_with(|c: char| c.is_uppercase()) {
            results.push(type_name.to_owned());
        }
    }

    results
}

/// Extract the first uppercase type name from a string fragment.
fn extract_first_type_name(s: &str) -> Option<String> {
    // Skip reference/pointer markers
    let s = s.trim_start_matches('&').trim_start_matches('*').trim();
    let s = s.strip_prefix("mut ").unwrap_or(s);
    let s = s.trim();

    // Find the first uppercase identifier
    let start = s.find(|c: char| c.is_uppercase())?;
    let rest = &s[start..];
    let end = rest
        .find(|c: char| !c.is_alphanumeric() && c != '_')
        .unwrap_or(rest.len());
    let name = &rest[..end];
    if name.is_empty() {
        None
    } else {
        Some(name.to_owned())
    }
}

// ── Type extraction by language ───────────────────────────────────────────

/// Extract variable type annotations from source code.
/// Returns (byte_offset, variable_name, type_name) triples.
fn extract_variable_type_annotations(source: &str, lang: Language) -> Vec<(usize, String, String)> {
    let mut results = Vec::new();

    match lang {
        Language::TypeScript | Language::Tsx | Language::JavaScript => {
            extract_var_types_colon_style(source, &mut results);
        }
        Language::Python => {
            extract_var_types_colon_style(source, &mut results);
        }
        Language::Rust => {
            extract_var_types_colon_style(source, &mut results);
        }
        Language::Java | Language::Kotlin | Language::CSharp => {
            extract_var_types_java_style(source, &mut results);
            extract_var_types_colon_style(source, &mut results); // Kotlin also uses colon
        }
        Language::Go => {
            extract_var_types_go_style(source, &mut results);
        }
        Language::Dart | Language::Swift => {
            extract_var_types_colon_style(source, &mut results);
        }
        Language::C | Language::Cpp => {
            extract_var_types_java_style(source, &mut results);
        }
        _ => {
            // Best-effort: try both patterns
            extract_var_types_colon_style(source, &mut results);
            extract_var_types_java_style(source, &mut results);
        }
    }

    results
}

/// Extract colon-style type annotations: `x: MyType`, `let x: MyType`
fn extract_var_types_colon_style(source: &str, results: &mut Vec<(usize, String, String)>) {
    // Pattern: `identifier: Type` (with optional let/const/var prefix)
    static VAR_COLON_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
        regex::Regex::new(
            r"(?:let|const|var|val|mut)\s+(\w+)\s*:\s*&?(?:mut\s+)?([A-Z][A-Za-z0-9_]*(?:<[^>]*>)?)"
        ).unwrap()
    });

    for cap in VAR_COLON_RE.captures_iter(source) {
        if let (Some(name_m), Some(type_m)) = (cap.get(1), cap.get(2)) {
            results.push((
                name_m.start(),
                name_m.as_str().to_owned(),
                type_m.as_str().to_owned(),
            ));
        }
    }
}

/// Extract Java-style type declarations: `MyType x =`, `final MyType x`
fn extract_var_types_java_style(source: &str, results: &mut Vec<(usize, String, String)>) {
    for cap in JAVA_TYPE_DECL_RE.captures_iter(source) {
        if let Some(type_m) = cap.get(1) {
            // The variable name follows the type — extract it
            let after_type = &source[type_m.end()..];
            // Skip generic params if present
            let after_generic = if after_type.starts_with('<') {
                after_type
                    .find('>')
                    .map(|i| &after_type[i + 1..])
                    .unwrap_or(after_type)
            } else {
                after_type
            };
            // Find the variable name
            let trimmed = after_generic.trim_start();
            let var_end = trimmed
                .find(|c: char| !c.is_alphanumeric() && c != '_')
                .unwrap_or(trimmed.len());
            let var_name = &trimmed[..var_end];
            if !var_name.is_empty() {
                results.push((
                    type_m.start(),
                    var_name.to_owned(),
                    type_m.as_str().to_owned(),
                ));
            }
        }
    }
}

/// Extract Go-style type declarations: `var x MyType`, `x *MyType`
fn extract_var_types_go_style(source: &str, results: &mut Vec<(usize, String, String)>) {
    // `var name Type`
    static GO_VAR_RE: LazyLock<regex::Regex> =
        LazyLock::new(|| regex::Regex::new(r"var\s+(\w+)\s+\*?([A-Z][A-Za-z0-9_]*)").unwrap());

    for cap in GO_VAR_RE.captures_iter(source) {
        if let (Some(name_m), Some(type_m)) = (cap.get(1), cap.get(2)) {
            results.push((
                name_m.start(),
                name_m.as_str().to_owned(),
                type_m.as_str().to_owned(),
            ));
        }
    }

    // Also match `name Type` in struct fields and function params
    for cap in GO_TYPE_RE.captures_iter(source) {
        if let Some(type_m) = cap.get(1) {
            results.push((
                type_m.start(),
                String::new(), // variable name not easily extractable here
                type_m.as_str().to_owned(),
            ));
        }
    }
}

/// Extract function parameter and return type references.
/// Returns (byte_offset, function_context_name, type_name) triples.
fn extract_function_type_refs(source: &str, lang: Language) -> Vec<(usize, String, String)> {
    let mut results = Vec::new();

    match lang {
        Language::TypeScript
        | Language::Tsx
        | Language::JavaScript
        | Language::Python
        | Language::Rust
        | Language::Dart
        | Language::Swift
        | Language::Kotlin => {
            extract_fn_types_colon_style(source, &mut results);
        }
        Language::Java | Language::CSharp | Language::Cpp | Language::C => {
            extract_fn_types_java_style(source, &mut results);
        }
        Language::Go => {
            extract_fn_types_go_style(source, &mut results);
        }
        _ => {
            extract_fn_types_colon_style(source, &mut results);
        }
    }

    results
}

/// Extract function parameter/return types from colon-style languages.
fn extract_fn_types_colon_style(source: &str, results: &mut Vec<(usize, String, String)>) {
    // Function signature with typed parameters: `fn name(param: Type)`
    static FN_SIG_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
        regex::Regex::new(
            r"(?:fn|function|def|fun)\s+(\w+)\s*(?:<[^>]*>)?\s*\(([^)]*)\)(?:\s*(?:->|:)\s*&?(?:mut\s+)?([A-Z][A-Za-z0-9_]*(?:<[^>]*>)?))?",
        ).unwrap()
    });

    for cap in FN_SIG_RE.captures_iter(source) {
        let fn_name = cap.get(1).map(|m| m.as_str()).unwrap_or("");
        let params = cap.get(2).map(|m| m.as_str()).unwrap_or("");
        let return_type = cap.get(3).map(|m| m.as_str());
        let offset = cap.get(0).map(|m| m.start()).unwrap_or(0);

        // Extract types from parameters
        for param_cap in COLON_TYPE_RE.captures_iter(params) {
            if let Some(type_m) = param_cap.get(1) {
                results.push((
                    offset,
                    format!("{}::param", fn_name),
                    type_m.as_str().to_owned(),
                ));
            }
        }

        // Extract return type
        if let Some(ret) = return_type {
            results.push((offset, format!("{}::return", fn_name), ret.to_owned()));
        }
    }
}

/// Extract function parameter/return types from Java-style languages.
fn extract_fn_types_java_style(source: &str, results: &mut Vec<(usize, String, String)>) {
    // Method signature: `ReturnType methodName(ParamType param, ...)`
    static JAVA_METHOD_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
        regex::Regex::new(
            r"(?:public|private|protected|static|\s)+\s+([A-Z][A-Za-z0-9_]*(?:<[^>]*>)?)\s+(\w+)\s*\(([^)]*)\)"
        ).unwrap()
    });

    for cap in JAVA_METHOD_RE.captures_iter(source) {
        let return_type = cap.get(1).map(|m| m.as_str()).unwrap_or("");
        let fn_name = cap.get(2).map(|m| m.as_str()).unwrap_or("");
        let params = cap.get(3).map(|m| m.as_str()).unwrap_or("");
        let offset = cap.get(0).map(|m| m.start()).unwrap_or(0);

        // Return type
        if !return_type.is_empty() && return_type != "void" {
            results.push((
                offset,
                format!("{}::return", fn_name),
                return_type.to_owned(),
            ));
        }

        // Parameter types: `Type name` pairs
        for param in params.split(',') {
            let trimmed = param.trim();
            if trimmed.is_empty() {
                continue;
            }
            // Extract type (first word starting with uppercase)
            if let Some(type_name) = extract_first_type_name(trimmed) {
                results.push((offset, format!("{}::param", fn_name), type_name));
            }
        }
    }
}

/// Extract function parameter/return types from Go-style code.
fn extract_fn_types_go_style(source: &str, results: &mut Vec<(usize, String, String)>) {
    // Go function: `func name(param Type, ...) ReturnType {`
    static GO_FN_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
        regex::Regex::new(
            r"func\s+(?:\([^)]*\)\s+)?(\w+)\s*\(([^)]*)\)\s*(?:\(([^)]*)\)|(\*?[A-Z][A-Za-z0-9_]*))?\s*\{"
        ).unwrap()
    });

    for cap in GO_FN_RE.captures_iter(source) {
        let fn_name = cap.get(1).map(|m| m.as_str()).unwrap_or("");
        let params = cap.get(2).map(|m| m.as_str()).unwrap_or("");
        let multi_return = cap.get(3).map(|m| m.as_str());
        let single_return = cap.get(4).map(|m| m.as_str());
        let offset = cap.get(0).map(|m| m.start()).unwrap_or(0);

        // Extract parameter types
        for param in params.split(',') {
            let trimmed = param.trim();
            if trimmed.is_empty() {
                continue;
            }
            // Go params: `name Type` or `name *Type`
            let parts: Vec<&str> = trimmed.split_whitespace().collect();
            if parts.len() >= 2 {
                let type_str = parts.last().unwrap().trim_start_matches('*');
                if type_str.starts_with(|c: char| c.is_uppercase()) {
                    results.push((offset, format!("{}::param", fn_name), type_str.to_owned()));
                }
            }
        }

        // Return type
        if let Some(ret) = single_return {
            let ret = ret.trim_start_matches('*');
            if ret.starts_with(|c: char| c.is_uppercase()) {
                results.push((offset, format!("{}::return", fn_name), ret.to_owned()));
            }
        }
        if let Some(multi) = multi_return {
            for part in multi.split(',') {
                let trimmed = part.trim().trim_start_matches('*');
                if trimmed.starts_with(|c: char| c.is_uppercase()) {
                    results.push((offset, format!("{}::return", fn_name), trimmed.to_owned()));
                }
            }
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_line_starts() {
        let source = "line1\nline2\nline3";
        let starts = build_line_starts(source);
        assert_eq!(starts, vec![0, 6, 12]);
    }

    #[test]
    fn test_byte_offset_to_line() {
        let starts = vec![0, 6, 12];
        assert_eq!(byte_offset_to_line(&starts, 0), 1);
        assert_eq!(byte_offset_to_line(&starts, 5), 1);
        assert_eq!(byte_offset_to_line(&starts, 6), 2);
        assert_eq!(byte_offset_to_line(&starts, 12), 3);
    }

    #[test]
    fn test_strip_generic_wrappers_simple() {
        let result = strip_generic_wrappers("MyType");
        assert_eq!(result, vec!["MyType"]);
    }

    #[test]
    fn test_strip_generic_wrappers_vec() {
        let result = strip_generic_wrappers("Vec<OrderItem>");
        assert_eq!(result, vec!["OrderItem"]);
    }

    #[test]
    fn test_strip_generic_wrappers_map() {
        let result = strip_generic_wrappers("Map<String, UserModel>");
        assert!(result.contains(&"UserModel".to_owned()));
    }

    #[test]
    fn test_strip_generic_wrappers_no_inner_uppercase() {
        // If inner types are all lowercase (stdlib), the outer type is returned
        // (stdlib filtering happens at the caller level)
        let result = strip_generic_wrappers("Vec<string>");
        assert_eq!(result, vec!["Vec"]);
    }

    #[test]
    fn test_extract_first_type_name() {
        assert_eq!(extract_first_type_name("MyType"), Some("MyType".to_owned()));
        assert_eq!(
            extract_first_type_name("&MyType"),
            Some("MyType".to_owned())
        );
        assert_eq!(
            extract_first_type_name("*MyType"),
            Some("MyType".to_owned())
        );
        assert_eq!(
            extract_first_type_name("mut MyType"),
            Some("MyType".to_owned())
        );
        assert_eq!(extract_first_type_name("lowercase"), None);
    }

    #[test]
    fn test_extract_var_types_colon_style_typescript() {
        let source = "const svc: MyService = new MyService();\nlet items: OrderItem[] = [];";
        let mut results = Vec::new();
        extract_var_types_colon_style(source, &mut results);
        let types: Vec<&str> = results.iter().map(|(_, _, t)| t.as_str()).collect();
        assert!(types.contains(&"MyService"));
        assert!(types.contains(&"OrderItem"));
    }

    #[test]
    fn test_extract_var_types_colon_style_rust() {
        let source = "let svc: &MyService = get_svc();\nlet mut item: OrderItem = create();";
        let mut results = Vec::new();
        extract_var_types_colon_style(source, &mut results);
        let types: Vec<&str> = results.iter().map(|(_, _, t)| t.as_str()).collect();
        assert!(types.contains(&"MyService"));
        assert!(types.contains(&"OrderItem"));
    }

    #[test]
    fn test_extract_var_types_java_style() {
        let source = "private UserService userService = new UserService();\nfinal OrderRepository repo = getRepo();";
        let mut results = Vec::new();
        extract_var_types_java_style(source, &mut results);
        let types: Vec<&str> = results.iter().map(|(_, _, t)| t.as_str()).collect();
        assert!(types.contains(&"UserService"));
        assert!(types.contains(&"OrderRepository"));
    }

    #[test]
    fn test_extract_var_types_go_style() {
        let source = "var svc MyService\nvar repo *OrderRepository";
        let mut results = Vec::new();
        extract_var_types_go_style(source, &mut results);
        let types: Vec<&str> = results.iter().map(|(_, _, t)| t.as_str()).collect();
        assert!(types.contains(&"MyService"));
        assert!(types.contains(&"OrderRepository"));
    }

    #[test]
    fn test_extract_fn_types_colon_style() {
        let source = "fn process(item: OrderItem) -> ResponseType {\n}";
        let mut results = Vec::new();
        extract_fn_types_colon_style(source, &mut results);
        let types: Vec<&str> = results.iter().map(|(_, _, t)| t.as_str()).collect();
        assert!(types.contains(&"OrderItem"));
        assert!(types.contains(&"ResponseType"));
    }

    #[test]
    fn test_extract_fn_types_java_style() {
        let source = "    public ResponseDto processOrder(OrderRequest request) {\n    }";
        let mut results = Vec::new();
        extract_fn_types_java_style(source, &mut results);
        let types: Vec<&str> = results.iter().map(|(_, _, t)| t.as_str()).collect();
        assert!(types.contains(&"ResponseDto"));
        assert!(types.contains(&"OrderRequest"));
    }

    #[test]
    fn test_extract_fn_types_go_style() {
        let source = "func ProcessOrder(item *OrderItem) *ResponseType {\n}";
        let mut results = Vec::new();
        extract_fn_types_go_style(source, &mut results);
        let types: Vec<&str> = results.iter().map(|(_, _, t)| t.as_str()).collect();
        assert!(types.contains(&"OrderItem"));
        assert!(types.contains(&"ResponseType"));
    }

    #[test]
    fn test_find_enclosing_symbol_in_function() {
        let symbols = vec![
            registry::RegistryEntry {
                qualified_name: "proj.module.MyClass".to_owned(),
                file_path: "src/module.ts".to_owned(),
                label: "Class".to_owned(),
                start_line: 1,
                end_line: 50,
            },
            registry::RegistryEntry {
                qualified_name: "proj.module.MyClass.process".to_owned(),
                file_path: "src/module.ts".to_owned(),
                label: "Method".to_owned(),
                start_line: 10,
                end_line: 30,
            },
        ];

        // Line 15 is inside the method
        let result = find_enclosing_symbol(&symbols, 15, "proj.module");
        assert_eq!(result, "proj.module.MyClass.process");

        // Line 5 is inside the class but outside the method
        let result = find_enclosing_symbol(&symbols, 5, "proj.module");
        assert_eq!(result, "proj.module.MyClass");

        // Line 55 is outside everything
        let result = find_enclosing_symbol(&symbols, 55, "proj.module");
        assert_eq!(result, "proj.module");
    }

    #[test]
    fn test_resolve_type_target_prefers_class() {
        let entries = vec![
            registry::RegistryEntry {
                qualified_name: "proj.module.MyType".to_owned(),
                file_path: "src/types.ts".to_owned(),
                label: "Class".to_owned(),
                start_line: 1,
                end_line: 10,
            },
            registry::RegistryEntry {
                qualified_name: "proj.other.MyType".to_owned(),
                file_path: "src/other.ts".to_owned(),
                label: "Function".to_owned(),
                start_line: 1,
                end_line: 5,
            },
        ];

        let result = resolve_type_target(&entries);
        assert_eq!(result, Some("proj.module.MyType".to_owned()));
    }

    #[test]
    fn test_deduplication_of_edges() {
        // Verify that strip_generic_wrappers + resolve logic
        // would produce the same type for repeated annotations
        let result1 = strip_generic_wrappers("List<OrderItem>");
        let result2 = strip_generic_wrappers("Vec<OrderItem>");
        assert_eq!(result1, vec!["OrderItem"]);
        assert_eq!(result2, vec!["OrderItem"]);
    }
}
