//! Pipeline pass: Usage and Reference detection.
//!
//! Scans source files for type annotations, variable declarations with explicit types,
//! and named constant references. Creates USES edges from the nearest enclosing symbol
//! (function, class, or module) to the referenced type or constant node.
//!
//! Requirements: 13.1, 13.2, 13.3, 13.4, 13.5

use std::collections::HashSet;
use std::sync::LazyLock;

use codryn_discover::{DiscoveredFile, Language};
use codryn_foundation::fqn;
use codryn_graph_buffer::{EdgeSource, GraphBuffer};
use rayon::prelude::*;

use crate::registry::Registry;
use crate::FileCache;

// ── Type annotation patterns ──────────────────────────────────────────────

/// TypeScript/JavaScript: `x: MyType`, `x: MyType[]`, `x: MyType<T>`
static TS_TYPE_ANNOTATION_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r":\s*([A-Z][A-Za-z0-9_]*)(?:\s*[<\[\]|&,;)\s=])").unwrap());

/// Python: `x: MyType`, `-> MyType`, `x: Optional[MyType]`, `x: List[MyType]`
static PY_TYPE_ANNOTATION_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"(?::\s*|->\s*)([A-Z][A-Za-z0-9_]*)(?:\s*[<\[\]=,):;\s]|$)").unwrap()
});

/// Rust: `x: MyType`, `-> MyType`, `Vec<MyType>`, `&MyType`
static RS_TYPE_ANNOTATION_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(
        r"(?::\s*&?(?:mut\s+)?|->?\s*&?(?:mut\s+)?)([A-Z][A-Za-z0-9_]*)(?:\s*[<>,;)\s{])",
    )
    .unwrap()
});

/// Java/Kotlin/C#: `MyType x`, `MyType<T> x`, `final MyType x`
static JAVA_TYPE_DECL_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(
        r"(?:(?:final|const|var|val|let|private|public|protected|static|readonly)\s+)*([A-Z][A-Za-z0-9_]*)\s*(?:<[^>]*>)?\s+[a-z_]\w*\s*[=;,)]"
    ).unwrap()
});

/// Go: `var x MyType`, `x MyType` (in function params/struct fields)
static GO_TYPE_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"\b[a-z_]\w*\s+\*?([A-Z][A-Za-z0-9_]*)(?:\s*[,;)\s{])").unwrap()
});

/// Generic type parameter extraction: `List<MyType>`, `Map<K, MyType>`, `Vec<MyType>`
static GENERIC_INNER_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"<\s*(?:[^<>]*,\s*)*([A-Z][A-Za-z0-9_]*)(?:\s*[,>])").unwrap()
});

/// Constant reference patterns: `const X`, `final X`, `val X` (uppercase identifiers)
static CONST_REF_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"\b([A-Z][A-Z0-9_]{2,})\b").unwrap());

// ── Main pass entry point ─────────────────────────────────────────────────

/// Scan source files for type annotations and constant references, creating USES edges.
///
/// For each type annotation or constant reference found:
/// 1. Determine the nearest enclosing symbol (function, class, or module)
/// 2. Resolve the referenced type/constant name via the Registry
/// 3. Create a USES edge from the enclosing symbol to the referenced node
///
/// Skips unresolved references gracefully (Requirement 13.5).
pub fn pass_usages(
    buf: &mut GraphBuffer,
    files: &[&DiscoveredFile],
    file_cache: &FileCache,
    project: &str,
    reg: &Registry,
) {
    if reg.is_empty() {
        return;
    }

    // Process files in parallel, collect (src_qn, tgt_qn) tuples
    let edge_tuples: Vec<(String, String)> = files
        .par_iter()
        .flat_map(|f| {
            // Read file content from cache or disk
            let source = if let Some(cached) = file_cache.get(&f.abs_path) {
                cached
            } else {
                match std::fs::read_to_string(&f.abs_path) {
                    Ok(s) => std::sync::Arc::new(s),
                    Err(_) => return vec![],
                }
            };

            extract_usages_from_file(f, &source, project, reg)
        })
        .collect();

    // Add edges to buffer serially (buffer is not Send)
    let mut seen: HashSet<(String, String)> = HashSet::new();
    for (src, tgt) in edge_tuples {
        // Deduplicate edges per (source, target) pair
        if seen.insert((src.clone(), tgt.clone())) {
            buf.add_edge_with_confidence(&src, &tgt, "USES", EdgeSource::RegexMatch, None);
        }
    }
}

/// Extract usage edges from a single file.
/// Returns a list of (source_qn, target_qn) pairs.
fn extract_usages_from_file(
    f: &DiscoveredFile,
    source: &str,
    project: &str,
    reg: &Registry,
) -> Vec<(String, String)> {
    let mut edges = Vec::new();
    let module_qn = fqn::fqn_module(project, &f.rel_path);

    // Build line-offset lookup for determining enclosing symbol
    let line_starts = build_line_starts(source);

    // Get all symbols in this file for enclosing-symbol resolution
    let file_symbols = reg.entries_for_file(&f.rel_path);

    // Extract type annotations based on language
    let type_refs = extract_type_references(source, f.language);

    for (byte_offset, type_name) in &type_refs {
        // Skip standard library types
        if crate::registry::is_stdlib_type(f.language, type_name) {
            continue;
        }

        // Resolve the type name to a node in the registry
        let target_entries = reg.lookup(type_name);
        if target_entries.is_empty() {
            // Requirement 13.5: skip unresolved type references gracefully
            continue;
        }

        // Prefer Class/Interface/Struct/Trait labels for type resolution
        let target_qn = target_entries
            .iter()
            .find(|e| {
                matches!(
                    e.label.as_str(),
                    "Class" | "Interface" | "Struct" | "Trait" | "Enum"
                )
            })
            .or_else(|| target_entries.first())
            .map(|e| e.qualified_name.clone());

        let Some(target_qn) = target_qn else {
            continue;
        };

        // Determine the enclosing symbol at this byte offset
        let line_num = byte_offset_to_line(&line_starts, *byte_offset);
        let enclosing_qn = find_enclosing_symbol(&file_symbols, line_num, &module_qn);

        // Don't create self-referential edges
        if enclosing_qn == target_qn {
            continue;
        }

        edges.push((enclosing_qn, target_qn));
    }

    // Extract constant references
    let const_refs = extract_constant_references(source, f.language);

    for (byte_offset, const_name) in &const_refs {
        let target_entries = reg.lookup(const_name);
        if target_entries.is_empty() {
            // Requirement 13.5: skip unresolved references gracefully
            continue;
        }

        // For constants, prefer entries with matching file or "Constant" label
        let target_qn = target_entries
            .iter()
            .find(|e| e.label == "Constant" || e.label == "Variable")
            .or_else(|| target_entries.first())
            .map(|e| e.qualified_name.clone());

        let Some(target_qn) = target_qn else {
            continue;
        };

        // Determine the enclosing symbol
        let line_num = byte_offset_to_line(&line_starts, *byte_offset);
        let enclosing_qn = find_enclosing_symbol(&file_symbols, line_num, &module_qn);

        // Don't create self-referential edges or edges to same-file constants
        // (those are already handled by pass_calls)
        if enclosing_qn == target_qn {
            continue;
        }

        // Only create USES edges for constants defined in other files
        let is_same_file = target_entries.iter().any(|e| e.file_path == f.rel_path);
        if is_same_file {
            continue;
        }

        edges.push((enclosing_qn, target_qn));
    }

    edges
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
///
/// - If the line is inside a function/method, returns that function's QN.
/// - Otherwise, returns the module QN (for module-scope declarations).
///
/// This handles both Requirement 13.2 (module-scope) and 13.3 (function-scope).
fn find_enclosing_symbol(
    file_symbols: &[crate::registry::RegistryEntry],
    line_num: i32,
    module_qn: &str,
) -> String {
    // Find the innermost enclosing symbol (prefer functions/methods over classes)
    let mut best: Option<&crate::registry::RegistryEntry> = None;

    for entry in file_symbols {
        if entry.start_line <= line_num && entry.end_line >= line_num {
            // Prefer the most specific (innermost) enclosing symbol
            match best {
                None => best = Some(entry),
                Some(current) => {
                    // Prefer functions/methods over classes for enclosing context
                    let entry_is_fn = matches!(entry.label.as_str(), "Function" | "Method");
                    let current_is_fn = matches!(current.label.as_str(), "Function" | "Method");

                    if entry_is_fn && !current_is_fn {
                        best = Some(entry);
                    } else if entry_is_fn == current_is_fn {
                        // Both same type — prefer the one with narrower range
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

/// Extract type references from source code based on language.
/// Returns (byte_offset, type_name) pairs.
fn extract_type_references(source: &str, lang: Language) -> Vec<(usize, String)> {
    let mut refs = Vec::new();

    match lang {
        Language::TypeScript | Language::Tsx | Language::JavaScript => {
            for cap in TS_TYPE_ANNOTATION_RE.captures_iter(source) {
                if let Some(m) = cap.get(1) {
                    refs.push((m.start(), m.as_str().to_owned()));
                }
            }
            // Also extract generic inner types
            for cap in GENERIC_INNER_RE.captures_iter(source) {
                if let Some(m) = cap.get(1) {
                    refs.push((m.start(), m.as_str().to_owned()));
                }
            }
        }
        Language::Python => {
            for cap in PY_TYPE_ANNOTATION_RE.captures_iter(source) {
                if let Some(m) = cap.get(1) {
                    refs.push((m.start(), m.as_str().to_owned()));
                }
            }
            for cap in GENERIC_INNER_RE.captures_iter(source) {
                if let Some(m) = cap.get(1) {
                    refs.push((m.start(), m.as_str().to_owned()));
                }
            }
        }
        Language::Rust => {
            for cap in RS_TYPE_ANNOTATION_RE.captures_iter(source) {
                if let Some(m) = cap.get(1) {
                    refs.push((m.start(), m.as_str().to_owned()));
                }
            }
            for cap in GENERIC_INNER_RE.captures_iter(source) {
                if let Some(m) = cap.get(1) {
                    refs.push((m.start(), m.as_str().to_owned()));
                }
            }
        }
        Language::Java | Language::Kotlin | Language::CSharp => {
            // Java-style: `MyType x =` declarations
            for cap in JAVA_TYPE_DECL_RE.captures_iter(source) {
                if let Some(m) = cap.get(1) {
                    refs.push((m.start(), m.as_str().to_owned()));
                }
            }
            // Also match colon-style annotations (Kotlin: `x: MyType`)
            for cap in TS_TYPE_ANNOTATION_RE.captures_iter(source) {
                if let Some(m) = cap.get(1) {
                    refs.push((m.start(), m.as_str().to_owned()));
                }
            }
            for cap in GENERIC_INNER_RE.captures_iter(source) {
                if let Some(m) = cap.get(1) {
                    refs.push((m.start(), m.as_str().to_owned()));
                }
            }
        }
        Language::Go => {
            for cap in GO_TYPE_RE.captures_iter(source) {
                if let Some(m) = cap.get(1) {
                    refs.push((m.start(), m.as_str().to_owned()));
                }
            }
        }
        Language::Dart | Language::Swift => {
            // Dart/Swift use colon-style annotations like TypeScript
            for cap in TS_TYPE_ANNOTATION_RE.captures_iter(source) {
                if let Some(m) = cap.get(1) {
                    refs.push((m.start(), m.as_str().to_owned()));
                }
            }
            for cap in GENERIC_INNER_RE.captures_iter(source) {
                if let Some(m) = cap.get(1) {
                    refs.push((m.start(), m.as_str().to_owned()));
                }
            }
        }
        Language::C | Language::Cpp => {
            // C/C++ uses Java-style `Type var` declarations
            for cap in JAVA_TYPE_DECL_RE.captures_iter(source) {
                if let Some(m) = cap.get(1) {
                    refs.push((m.start(), m.as_str().to_owned()));
                }
            }
        }
        _ => {
            // For unsupported languages, try both patterns as a best-effort
            for cap in TS_TYPE_ANNOTATION_RE.captures_iter(source) {
                if let Some(m) = cap.get(1) {
                    refs.push((m.start(), m.as_str().to_owned()));
                }
            }
            for cap in JAVA_TYPE_DECL_RE.captures_iter(source) {
                if let Some(m) = cap.get(1) {
                    refs.push((m.start(), m.as_str().to_owned()));
                }
            }
        }
    }

    refs
}

/// Extract named constant references from source code.
/// Constants are identified as ALL_CAPS identifiers (at least 3 chars).
/// Returns (byte_offset, constant_name) pairs.
fn extract_constant_references(source: &str, lang: Language) -> Vec<(usize, String)> {
    // Skip languages where ALL_CAPS is not a constant convention
    if matches!(
        lang,
        Language::Sql | Language::Css | Language::Scss | Language::Html
    ) {
        return Vec::new();
    }

    let mut refs = Vec::new();

    for cap in CONST_REF_RE.captures_iter(source) {
        if let Some(m) = cap.get(1) {
            let name = m.as_str();
            // Skip common non-constant ALL_CAPS patterns
            if is_common_keyword(name) {
                continue;
            }
            refs.push((m.start(), name.to_owned()));
        }
    }

    refs
}

/// Check if an ALL_CAPS identifier is a common keyword/macro that shouldn't
/// be treated as a constant reference.
fn is_common_keyword(name: &str) -> bool {
    matches!(
        name,
        "NULL"
            | "TRUE"
            | "FALSE"
            | "NONE"
            | "SELF"
            | "THIS"
            | "TODO"
            | "FIXME"
            | "NOTE"
            | "HACK"
            | "XXX"
            | "EOF"
            | "NAN"
            | "INF"
            | "OK"
            | "ERR"
            | "GET"
            | "POST"
            | "PUT"
            | "DELETE"
            | "PATCH"
            | "HEAD"
            | "OPTIONS"
            | "HTTP"
            | "HTTPS"
            | "SQL"
            | "HTML"
            | "CSS"
            | "JSON"
            | "XML"
            | "UTF"
            | "ASCII"
            | "API"
            | "URL"
            | "URI"
            | "ENV"
    )
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
        assert_eq!(byte_offset_to_line(&starts, 0), 1); // first char of line 1
        assert_eq!(byte_offset_to_line(&starts, 5), 1); // last char of line 1
        assert_eq!(byte_offset_to_line(&starts, 6), 2); // first char of line 2
        assert_eq!(byte_offset_to_line(&starts, 12), 3); // first char of line 3
    }

    #[test]
    fn test_extract_type_references_typescript() {
        let source = "const x: MyService = new MyService();\nfunction foo(bar: UserModel): ResponseType {\n}";
        let refs = extract_type_references(source, Language::TypeScript);
        let names: Vec<&str> = refs.iter().map(|(_, n)| n.as_str()).collect();
        assert!(names.contains(&"MyService"));
        assert!(names.contains(&"UserModel"));
        assert!(names.contains(&"ResponseType"));
    }

    #[test]
    fn test_extract_type_references_python() {
        let source = "def process(item: OrderItem) -> OrderResult:\n    pass";
        let refs = extract_type_references(source, Language::Python);
        let names: Vec<&str> = refs.iter().map(|(_, n)| n.as_str()).collect();
        assert!(names.contains(&"OrderItem"));
        assert!(names.contains(&"OrderResult"));
    }

    #[test]
    fn test_extract_type_references_java() {
        let source = "private UserService userService = new UserService();\nfinal OrderRepository repo = getRepo();";
        let refs = extract_type_references(source, Language::Java);
        let names: Vec<&str> = refs.iter().map(|(_, n)| n.as_str()).collect();
        assert!(names.contains(&"UserService"));
        assert!(names.contains(&"OrderRepository"));
    }

    #[test]
    fn test_extract_type_references_rust() {
        let source = "fn process(item: &OrderItem) -> Result<Response> {\n    let svc: MyService = get();\n}";
        let refs = extract_type_references(source, Language::Rust);
        let names: Vec<&str> = refs.iter().map(|(_, n)| n.as_str()).collect();
        assert!(names.contains(&"OrderItem"));
        assert!(names.contains(&"MyService"));
    }

    #[test]
    fn test_extract_type_references_go() {
        let source = "func process(item *OrderItem) {\n    var svc MyService\n}";
        let refs = extract_type_references(source, Language::Go);
        let names: Vec<&str> = refs.iter().map(|(_, n)| n.as_str()).collect();
        assert!(names.contains(&"OrderItem"));
        assert!(names.contains(&"MyService"));
    }

    #[test]
    fn test_extract_constant_references() {
        let source = "let timeout = MAX_TIMEOUT;\nif (status == HTTP_OK) { return DEFAULT_VALUE; }";
        let refs = extract_constant_references(source, Language::TypeScript);
        let names: Vec<&str> = refs.iter().map(|(_, n)| n.as_str()).collect();
        assert!(names.contains(&"MAX_TIMEOUT"));
        assert!(names.contains(&"HTTP_OK"));
        assert!(names.contains(&"DEFAULT_VALUE"));
    }

    #[test]
    fn test_extract_constant_references_skips_keywords() {
        let source = "if (x == NULL || y == TRUE) { return FALSE; }";
        let refs = extract_constant_references(source, Language::TypeScript);
        let names: Vec<&str> = refs.iter().map(|(_, n)| n.as_str()).collect();
        assert!(!names.contains(&"NULL"));
        assert!(!names.contains(&"TRUE"));
        assert!(!names.contains(&"FALSE"));
    }

    #[test]
    fn test_extract_constant_references_skips_sql() {
        let source = "SELECT MAX_VALUE FROM config WHERE STATUS = ACTIVE_STATUS";
        let refs = extract_constant_references(source, Language::Sql);
        assert!(refs.is_empty());
    }

    #[test]
    fn test_is_common_keyword() {
        assert!(is_common_keyword("NULL"));
        assert!(is_common_keyword("TRUE"));
        assert!(is_common_keyword("TODO"));
        assert!(!is_common_keyword("MAX_RETRIES"));
        assert!(!is_common_keyword("DEFAULT_TIMEOUT"));
    }

    #[test]
    fn test_find_enclosing_symbol_in_function() {
        let symbols = vec![
            crate::registry::RegistryEntry {
                qualified_name: "proj.module.MyClass".to_owned(),
                file_path: "src/module.ts".to_owned(),
                label: "Class".to_owned(),
                start_line: 1,
                end_line: 50,
            },
            crate::registry::RegistryEntry {
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
    fn test_generic_inner_type_extraction() {
        let source = "const items: List<OrderItem> = [];\nconst map: Map<String, UserModel> = {};";
        let refs = extract_type_references(source, Language::TypeScript);
        let names: Vec<&str> = refs.iter().map(|(_, n)| n.as_str()).collect();
        assert!(names.contains(&"OrderItem"));
        assert!(names.contains(&"UserModel"));
    }
}
