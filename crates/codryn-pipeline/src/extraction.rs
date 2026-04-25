use codryn_discover::{DiscoveredFile, Language};
use codryn_foundation::fqn;
use codryn_graph_buffer::GraphBuffer;
use codryn_treesitter::TsSymbol;
use regex::Regex;

use crate::registry::{is_stdlib_type, Registry, RegistryEntry, TypeRegistry};

/// Holds the result of extracting a single file, without mutating shared state.
/// Nodes, registry entries, and code snippets are collected here and merged later.
#[derive(Debug, Clone)]
pub struct ExtractionResult {
    /// Nodes to add to the GraphBuffer: (label, name, qualified_name, file_path, start_line, end_line, properties_json)
    pub nodes: Vec<ExtractionNode>,
    /// Registry entries to merge into the Registry: (short_name, entry)
    pub registry_entries: Vec<(String, RegistryEntry)>,
    /// Code snippets for FTS indexing: (qualified_name, content)
    pub code_snippets: Vec<(String, String)>,
}

/// A node extracted from a file, ready to be added to a GraphBuffer.
#[derive(Debug, Clone)]
pub struct ExtractionNode {
    pub label: String,
    pub name: String,
    pub qualified_name: String,
    pub file_path: String,
    pub start_line: i32,
    pub end_line: i32,
    pub properties_json: Option<String>,
}

impl ExtractionResult {
    fn new() -> Self {
        Self {
            nodes: Vec::new(),
            registry_entries: Vec::new(),
            code_snippets: Vec::new(),
        }
    }

    /// Merge this result into a GraphBuffer and Registry.
    pub fn apply(self, buf: &mut GraphBuffer, reg: &mut Registry) {
        for node in self.nodes {
            buf.add_node(
                &node.label,
                &node.name,
                &node.qualified_name,
                &node.file_path,
                node.start_line,
                node.end_line,
                node.properties_json,
            );
        }
        for (name, entry) in &self.registry_entries {
            reg.register(
                name,
                &entry.qualified_name,
                &entry.file_path,
                &entry.label,
                entry.start_line,
                entry.end_line,
            );
        }
        for (qn, content) in &self.code_snippets {
            buf.add_code_content(qn, content);
        }
    }

    /// Merge only registry entries (for unchanged files that only need registration).
    pub fn apply_registry_only(entries: Vec<(String, RegistryEntry)>, reg: &mut Registry) {
        for (name, entry) in &entries {
            reg.register(
                name,
                &entry.qualified_name,
                &entry.file_path,
                &entry.label,
                entry.start_line,
                entry.end_line,
            );
        }
    }
}

/// Extract type assignments from tree-sitter symbols and register in TypeRegistry.
/// Called during the extraction phase for each file.
/// Registers function return types and parameter types, skipping stdlib types.
pub fn extract_type_assigns(
    type_reg: &mut TypeRegistry,
    file_path: &str,
    symbols: &[TsSymbol],
    lang: Language,
) {
    for sym in symbols {
        // Register function return types
        if let Some(ref ret_type) = sym.return_type {
            if !is_stdlib_type(lang, ret_type) {
                type_reg.register_type(file_path, &format!("{}::return", sym.name), ret_type);
            }
        }
        // Register parameter types
        for param in &sym.parameters {
            if let Some(ref type_name) = param.type_name {
                if !is_stdlib_type(lang, type_name) {
                    type_reg.register_type(
                        file_path,
                        &format!("{}::{}", sym.name, param.name),
                        type_name,
                    );
                }
            }
        }
    }
}

/// Extract definitions from a file without mutating shared state.
/// Returns `None` for Java/Kotlin/Go (which require their own AST extractors that
/// mutate GraphBuffer/Registry directly) — the caller should fall back to serial
/// `extract_file` for those.
///
/// This is safe to call from multiple threads in parallel via `rayon::par_iter()`.
pub fn extract_file_parallel(project: &str, file: &DiscoveredFile) -> Option<ExtractionResult> {
    // Java/Kotlin/Go have dedicated AST extractors that mutate buf/reg directly.
    // Return None so the caller falls back to serial extraction for these.
    match file.language {
        Language::Java | Language::Kotlin | Language::Go => return None,
        _ => {}
    }

    let source = match std::fs::read_to_string(&file.abs_path) {
        Ok(s) => s,
        Err(_) => return Some(ExtractionResult::new()),
    };

    let lines: Vec<&str> = source.lines().collect();

    // Try tree-sitter first
    if let Some(symbols) = codryn_treesitter::extract_symbols(file.language, &source) {
        let mut result = ExtractionResult::new();
        for sym in &symbols {
            let qn = if let Some(ref parent) = sym.parent_name {
                fqn::fqn_compute(
                    project,
                    &file.rel_path,
                    Some(&format!("{}.{}", parent, sym.name)),
                )
            } else {
                fqn::fqn_compute(project, &file.rel_path, Some(&sym.name))
            };
            let props = build_metadata_json(sym, &lines, &file.rel_path);
            result.nodes.push(ExtractionNode {
                label: sym.label.clone(),
                name: sym.name.clone(),
                qualified_name: qn.clone(),
                file_path: file.rel_path.clone(),
                start_line: sym.start_line,
                end_line: sym.end_line,
                properties_json: props,
            });
            result.registry_entries.push((
                sym.name.clone(),
                RegistryEntry {
                    qualified_name: qn.clone(),
                    file_path: file.rel_path.clone(),
                    label: sym.label.clone(),
                    start_line: sym.start_line,
                    end_line: sym.end_line,
                },
            ));
            // Code content for FTS
            let start_idx = (sym.start_line - 1).max(0) as usize;
            let end_idx = (sym.end_line as usize).min(lines.len());
            if end_idx > start_idx {
                let content = lines[start_idx..end_idx].join("\n");
                result.code_snippets.push((qn, content));
            }
        }
        // Module node
        let module_qn = fqn::fqn_module(project, &file.rel_path);
        let module_name = std::path::Path::new(&file.rel_path)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or(&file.rel_path);
        result.nodes.push(ExtractionNode {
            label: "Module".to_owned(),
            name: module_name.to_owned(),
            qualified_name: module_qn,
            file_path: file.rel_path.clone(),
            start_line: 1,
            end_line: lines.len() as i32,
            properties_json: None,
        });
        return Some(result);
    }

    // Fall through to regex extraction
    let mut result = ExtractionResult::new();
    let patterns = get_patterns(file.language);
    for (i, &line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        let line_num = (i + 1) as i32;
        for (pat, label) in &patterns {
            if let Some(caps) = pat.captures(trimmed) {
                if let Some(name) = caps.get(1) {
                    let name = name.as_str();
                    if matches!(
                        name,
                        "if" | "for"
                            | "while"
                            | "switch"
                            | "catch"
                            | "return"
                            | "new"
                            | "typeof"
                            | "instanceof"
                    ) {
                        continue;
                    }
                    if name.len() > 1 && !name.starts_with('_') || label == &"Class" {
                        let qn = fqn::fqn_compute(project, &file.rel_path, Some(name));
                        let end = compute_end_line(&lines, i, file.language);
                        result.nodes.push(ExtractionNode {
                            label: label.to_string(),
                            name: name.to_owned(),
                            qualified_name: qn.clone(),
                            file_path: file.rel_path.clone(),
                            start_line: line_num,
                            end_line: end,
                            properties_json: None,
                        });
                        result.registry_entries.push((
                            name.to_owned(),
                            RegistryEntry {
                                qualified_name: qn.clone(),
                                file_path: file.rel_path.clone(),
                                label: label.to_string(),
                                start_line: line_num,
                                end_line: end,
                            },
                        ));
                        // Code content for FTS
                        let end_idx = (end as usize).min(lines.len());
                        if end_idx > i {
                            let content = lines[i..end_idx].join("\n");
                            result.code_snippets.push((qn, content));
                        }
                    }
                }
            }
        }
    }
    // Module node
    let module_qn = fqn::fqn_module(project, &file.rel_path);
    let module_name = std::path::Path::new(&file.rel_path)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(&file.rel_path);
    result.nodes.push(ExtractionNode {
        label: "Module".to_owned(),
        name: module_name.to_owned(),
        qualified_name: module_qn,
        file_path: file.rel_path.clone(),
        start_line: 1,
        end_line: lines.len() as i32,
        properties_json: None,
    });
    Some(result)
}

/// Extract registry entries from a file without mutating shared state (for unchanged files).
/// Returns `None` for Java/Kotlin/Go — caller should fall back to serial `register_file`.
pub fn register_file_parallel(
    project: &str,
    file: &DiscoveredFile,
) -> Option<Vec<(String, RegistryEntry)>> {
    match file.language {
        Language::Java | Language::Kotlin | Language::Go => return None,
        _ => {}
    }

    let source = match std::fs::read_to_string(&file.abs_path) {
        Ok(s) => s,
        Err(_) => return Some(Vec::new()),
    };

    // Try tree-sitter first
    if let Some(symbols) = codryn_treesitter::extract_symbols(file.language, &source) {
        let mut entries = Vec::new();
        for sym in &symbols {
            let qn = if let Some(ref parent) = sym.parent_name {
                fqn::fqn_compute(
                    project,
                    &file.rel_path,
                    Some(&format!("{}.{}", parent, sym.name)),
                )
            } else {
                fqn::fqn_compute(project, &file.rel_path, Some(&sym.name))
            };
            entries.push((
                sym.name.clone(),
                RegistryEntry {
                    qualified_name: qn,
                    file_path: file.rel_path.clone(),
                    label: sym.label.clone(),
                    start_line: sym.start_line,
                    end_line: sym.end_line,
                },
            ));
        }
        return Some(entries);
    }

    // Regex fallback
    let patterns = get_patterns(file.language);
    let lines: Vec<&str> = source.lines().collect();
    let mut entries = Vec::new();
    for (i, &line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        for (pat, label) in &patterns {
            if let Some(caps) = pat.captures(trimmed) {
                if let Some(name) = caps.get(1) {
                    let name = name.as_str();
                    if matches!(
                        name,
                        "if" | "for"
                            | "while"
                            | "switch"
                            | "catch"
                            | "return"
                            | "new"
                            | "typeof"
                            | "instanceof"
                    ) {
                        continue;
                    }
                    if name.len() > 1 && !name.starts_with('_') || label == &"Class" {
                        let qn = fqn::fqn_compute(project, &file.rel_path, Some(name));
                        let line_num = (i + 1) as i32;
                        let end = compute_end_line(&lines, i, file.language);
                        entries.push((
                            name.to_owned(),
                            RegistryEntry {
                                qualified_name: qn,
                                file_path: file.rel_path.clone(),
                                label: label.to_string(),
                                start_line: line_num,
                                end_line: end,
                            },
                        ));
                    }
                }
            }
        }
    }
    Some(entries)
}

static ANGULAR_DECORATOR: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
fn angular_decorator_re() -> &'static Regex {
    ANGULAR_DECORATOR.get_or_init(|| {
        Regex::new(r"^@(Component|Injectable|Pipe|Directive|NgModule|Guard|Resolver|Interceptor)")
            .unwrap()
    })
}

static JAVA_ANNOTATION: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
fn java_annotation_re() -> &'static Regex {
    JAVA_ANNOTATION.get_or_init(|| {
        Regex::new(r#"^@(GetMapping|PostMapping|PutMapping|PatchMapping|DeleteMapping|RequestMapping)\s*(?:\(\s*(?:value\s*=\s*)?(?:"([^"]*)")?[^)]*\))?"#).unwrap()
    })
}

static JAVA_CLASS_ANNOTATION: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
fn java_class_annotation_re() -> &'static Regex {
    JAVA_CLASS_ANNOTATION.get_or_init(|| {
        Regex::new(r#"^@(RestController|Controller|Service|Repository|RequestMapping)\s*(?:\(\s*(?:value\s*=\s*)?(?:"([^"]*)")?[^)]*\))?"#).unwrap()
    })
}

fn annotation_to_http_method(annotation: &str) -> &'static str {
    match annotation {
        "GetMapping" => "GET",
        "PostMapping" => "POST",
        "PutMapping" => "PUT",
        "PatchMapping" => "PATCH",
        "DeleteMapping" => "DELETE",
        _ => "",
    }
}

/// Build a metadata JSON string from a tree-sitter extracted symbol.
/// Returns `None` if the symbol has no meaningful metadata to store.
fn build_metadata_json(sym: &TsSymbol, _source_lines: &[&str], rel_path: &str) -> Option<String> {
    let mut m = serde_json::Map::new();

    if let Some(ref sig) = sym.signature {
        m.insert("signature".into(), serde_json::json!(sig));
    }
    if let Some(ref rt) = sym.return_type {
        m.insert("return_type".into(), serde_json::json!(rt));
    }
    if !sym.parameters.is_empty() {
        let params: Vec<serde_json::Value> = sym
            .parameters
            .iter()
            .map(|p| {
                serde_json::json!({
                    "name": p.name,
                    "type": p.type_name,
                })
            })
            .collect();
        m.insert("parameters".into(), serde_json::json!(params));
    }
    if let Some(ref doc) = sym.docstring {
        m.insert("docstring".into(), serde_json::json!(doc));
    }
    if !sym.decorators.is_empty() {
        m.insert("decorators".into(), serde_json::json!(sym.decorators));
    }
    if !sym.base_classes.is_empty() {
        m.insert("base_classes".into(), serde_json::json!(sym.base_classes));
    }
    m.insert("is_exported".into(), serde_json::json!(sym.is_exported));
    m.insert("is_abstract".into(), serde_json::json!(sym.is_abstract));

    // Use is_test from walker (already set for Rust/Python), or derive from decorators/name
    let is_test_by_file = rel_path.contains("__tests__/")
        || rel_path.contains("__tests__\\")
        || rel_path.contains(".test.")
        || rel_path.contains(".spec.")
        || rel_path.contains("_test.");
    let is_test = sym.is_test
        || is_test_by_file
        || sym.decorators.iter().any(|d| {
            d.contains("test")
                || d.contains("Test")
                || d.contains("pytest")
                || d.contains("tokio::test")
        })
        || sym.name.starts_with("test_")
        || sym.name.starts_with("Test");
    m.insert("is_test".into(), serde_json::json!(is_test));

    // Derive is_entry_point from walker field or name/decorator patterns
    let is_entry_point = sym.is_entry_point
        || sym.name == "main"
        || (sym.label == "Function"
            && sym.body_text.as_ref().is_some_and(|body| {
                body.contains("app.listen(")
                    || body.contains("createServer(")
                    || body.contains("server.listen(")
                    || body.contains(".listen(")
                    || body.contains("process.argv")
                    || body.contains("commander")
                    || body.contains("yargs")
            }));
    m.insert("is_entry_point".into(), serde_json::json!(is_entry_point));

    // Compute complexity from body text
    if let Some(ref body) = sym.body_text {
        let complexity = codryn_foundation::complexity::cyclomatic_complexity(body);
        m.insert("complexity".into(), serde_json::json!(complexity));
    }

    // Line count
    let line_count = (sym.end_line - sym.start_line + 1).max(1);
    m.insert("line_count".into(), serde_json::json!(line_count));

    if m.is_empty() {
        None
    } else {
        Some(serde_json::Value::Object(m).to_string())
    }
}

/// Extract definitions from a file using regex patterns (fallback when tree-sitter unavailable).
pub fn extract_file(
    buf: &mut GraphBuffer,
    reg: &mut Registry,
    project: &str,
    file: &DiscoveredFile,
) {
    // Dispatch Java/Kotlin to tree-sitter AST extractors
    match file.language {
        Language::Java => {
            crate::spring_java::extract_java(buf, reg, project, file);
            return;
        }
        Language::Kotlin => {
            crate::spring_kotlin::extract_kotlin(buf, reg, project, file);
            return;
        }
        Language::Go => {
            crate::go_adapter::extract_go(buf, reg, project, file);
            return;
        }
        _ => {}
    }

    let source = match std::fs::read_to_string(&file.abs_path) {
        Ok(s) => s,
        Err(_) => return,
    };

    // Try tree-sitter first for supported languages
    let lines: Vec<&str> = source.lines().collect();
    if let Some(symbols) = codryn_treesitter::extract_symbols(file.language, &source) {
        for sym in &symbols {
            let qn = if let Some(ref parent) = sym.parent_name {
                fqn::fqn_compute(
                    project,
                    &file.rel_path,
                    Some(&format!("{}.{}", parent, sym.name)),
                )
            } else {
                fqn::fqn_compute(project, &file.rel_path, Some(&sym.name))
            };
            let props = build_metadata_json(sym, &lines, &file.rel_path);
            buf.add_node(
                &sym.label,
                &sym.name,
                &qn,
                &file.rel_path,
                sym.start_line,
                sym.end_line,
                props,
            );
            reg.register(
                &sym.name,
                &qn,
                &file.rel_path,
                &sym.label,
                sym.start_line,
                sym.end_line,
            );
            // Index code content for FTS
            let start_idx = (sym.start_line - 1).max(0) as usize;
            let end_idx = (sym.end_line as usize).min(lines.len());
            if end_idx > start_idx {
                let content = lines[start_idx..end_idx].join("\n");
                buf.add_code_content(&qn, &content);
            }
        }
        // Module node
        let module_qn = fqn::fqn_module(project, &file.rel_path);
        let module_name = std::path::Path::new(&file.rel_path)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or(&file.rel_path);
        buf.add_node(
            "Module",
            module_name,
            &module_qn,
            &file.rel_path,
            1,
            lines.len() as i32,
            None,
        );
        return; // Tree-sitter succeeded, skip regex
    }

    // Fall through to regex extraction if tree-sitter returned None

    let is_ts = matches!(
        file.language,
        Language::TypeScript | Language::Tsx | Language::JavaScript
    );
    let is_java = matches!(file.language, Language::Java | Language::Kotlin);
    let patterns = get_patterns(file.language);
    let mut pending_decorator: Option<String> = None;
    let mut decorator_depth: i32 = 0; // track open parens inside decorator
    let mut pending_selector: Option<String> = None;
    let mut pending_template_url: Option<String> = None;
    // Java annotation tracking
    let mut pending_java_annotation: Option<String> = None;
    let mut pending_java_path: Option<String> = None;
    let mut pending_java_http_method: Option<String> = None;
    let mut java_annotation_depth: i32 = 0;
    // Class-level annotation state for Java
    let mut pending_class_annotations: Vec<String> = Vec::new();
    let mut pending_class_base_path: Option<String> = None;

    for (i, &line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        let line_num = (i + 1) as i32;

        // Track Angular decorators — apply to the next class
        if is_ts {
            if let Some(caps) = angular_decorator_re().captures(trimmed) {
                pending_decorator = Some(caps.get(1).unwrap().as_str().to_owned());
                decorator_depth = trimmed.chars().filter(|&c| c == '(').count() as i32
                    - trimmed.chars().filter(|&c| c == ')').count() as i32;
                continue;
            }
            if pending_decorator.is_some() && decorator_depth > 0 {
                decorator_depth += trimmed.chars().filter(|&c| c == '(').count() as i32
                    - trimmed.chars().filter(|&c| c == ')').count() as i32;
                // Capture selector and templateUrl from @Component decorator body
                if let Some(start) = trimmed.find("selector:") {
                    let rest = &trimmed[start..];
                    if let Some(q1) = rest.find('\'') {
                        let after = &rest[q1 + 1..];
                        if let Some(q2) = after.find('\'') {
                            pending_selector = Some(after[..q2].to_owned());
                        }
                    }
                }
                if let Some(start) = trimmed.find("templateUrl:") {
                    let rest = &trimmed[start..];
                    if let Some(q1) = rest.find('\'') {
                        let after = &rest[q1 + 1..];
                        if let Some(q2) = after.find('\'') {
                            pending_template_url = Some(after[..q2].to_owned());
                        }
                    }
                }
                continue;
            }
        }

        // Track Java/Kotlin annotations
        if is_java {
            // Class-level annotations: @RestController, @Controller, @RequestMapping
            if let Some(caps) = java_class_annotation_re().captures(trimmed) {
                let ann = caps.get(1).unwrap().as_str();
                pending_class_annotations.push(ann.to_owned());
                if let Some(path) = caps.get(2) {
                    pending_class_base_path = Some(path.as_str().to_owned());
                }
                continue;
            }
            // Method-level annotations: @GetMapping, @PostMapping, etc.
            if let Some(caps) = java_annotation_re().captures(trimmed) {
                let ann = caps.get(1).unwrap().as_str();
                pending_java_http_method = Some(annotation_to_http_method(ann).to_owned());
                pending_java_annotation = Some(ann.to_owned());
                pending_java_path = caps.get(2).map(|m| m.as_str().to_owned());
                java_annotation_depth = trimmed.chars().filter(|&c| c == '(').count() as i32
                    - trimmed.chars().filter(|&c| c == ')').count() as i32;
                continue;
            }
            // Multi-line annotation body
            if pending_java_annotation.is_some() && java_annotation_depth > 0 {
                java_annotation_depth += trimmed.chars().filter(|&c| c == '(').count() as i32
                    - trimmed.chars().filter(|&c| c == ')').count() as i32;
                continue;
            }
        }

        for (pat, label) in &patterns {
            if let Some(caps) = pat.captures(trimmed) {
                if let Some(name) = caps.get(1) {
                    let name = name.as_str();
                    if matches!(
                        name,
                        "if" | "for"
                            | "while"
                            | "switch"
                            | "catch"
                            | "return"
                            | "new"
                            | "typeof"
                            | "instanceof"
                    ) {
                        continue;
                    }
                    if name.len() > 1 && !name.starts_with('_') || label == &"Class" {
                        let (effective_label, props) = if is_java {
                            if *label == "Class" {
                                // Attach class-level annotations
                                let props = if !pending_class_annotations.is_empty() {
                                    let mut p = serde_json::Map::new();
                                    p.insert(
                                        "annotations".into(),
                                        serde_json::json!(pending_class_annotations),
                                    );
                                    if let Some(ref bp) = pending_class_base_path {
                                        p.insert("base_path".into(), serde_json::json!(bp));
                                    }
                                    Some(serde_json::Value::Object(p).to_string())
                                } else {
                                    None
                                };
                                // Don't clear — class annotations stay for methods
                                ("Class", props)
                            } else if pending_java_annotation.is_some() {
                                // Method with REST annotation
                                let mut p = serde_json::Map::new();
                                if let Some(ann) = pending_java_annotation.take() {
                                    p.insert("annotation".into(), serde_json::json!(ann));
                                }
                                if let Some(path) = pending_java_path.take() {
                                    p.insert("path".into(), serde_json::json!(path));
                                }
                                if let Some(hm) = pending_java_http_method.take() {
                                    if !hm.is_empty() {
                                        p.insert("http_method".into(), serde_json::json!(hm));
                                    }
                                }
                                ("Method", Some(serde_json::Value::Object(p).to_string()))
                            } else {
                                ("Method", None)
                            }
                        } else if is_ts {
                            let effective_label = if *label == "Class" {
                                if let Some(ref dec) = pending_decorator {
                                    match dec.as_str() {
                                        "Component" | "Injectable" | "Pipe" | "Directive" => {
                                            "Class"
                                        }
                                        _ => label,
                                    }
                                } else {
                                    label
                                }
                            } else {
                                label
                            };
                            let props = pending_decorator.take().map(|d| {
                                let mut p = serde_json::Map::new();
                                p.insert("decorator".into(), serde_json::json!(d));
                                if let Some(sel) = pending_selector.take() {
                                    p.insert("selector".into(), serde_json::json!(sel));
                                }
                                if let Some(tpl) = pending_template_url.take() {
                                    p.insert("templateUrl".into(), serde_json::json!(tpl));
                                }
                                serde_json::Value::Object(p).to_string()
                            });
                            (effective_label, props)
                        } else {
                            (*label, None)
                        };
                        let qn = fqn::fqn_compute(project, &file.rel_path, Some(name));
                        let end = compute_end_line(&lines, i, file.language);
                        buf.add_node(
                            effective_label,
                            name,
                            &qn,
                            &file.rel_path,
                            line_num,
                            end,
                            props,
                        );
                        reg.register(name, &qn, &file.rel_path, effective_label, line_num, end);
                        // Index code content for FTS
                        let end_idx = (end as usize).min(lines.len());
                        let start_idx = i;
                        if end_idx > start_idx {
                            let content = lines[start_idx..end_idx].join("\n");
                            buf.add_code_content(&qn, &content);
                        }
                    }
                }
            }
        }
        // Clear pending TS decorator
        if is_ts
            && pending_decorator.is_some()
            && !trimmed.is_empty()
            && !trimmed.starts_with('@')
            && !trimmed.starts_with("//")
            && !trimmed.contains("class ")
        {
            pending_decorator = None;
        }
        // Clear pending Java annotation if line is not blank/comment/annotation and not a method
        if is_java
            && pending_java_annotation.is_some()
            && java_annotation_depth <= 0
            && !trimmed.is_empty()
            && !trimmed.starts_with('@')
            && !trimmed.starts_with("//")
            && !trimmed.starts_with("/*")
            && !trimmed.starts_with("*")
        {
            // The annotation was consumed by the pattern match above, or it's stale
            pending_java_annotation = None;
            pending_java_path = None;
            pending_java_http_method = None;
        }
    }

    // Module node
    let module_qn = fqn::fqn_module(project, &file.rel_path);
    let module_name = std::path::Path::new(&file.rel_path)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(&file.rel_path);
    buf.add_node(
        "Module",
        module_name,
        &module_qn,
        &file.rel_path,
        1,
        lines.len() as i32,
        None,
    );
}

/// Register definitions from a file into the registry without adding nodes to the buffer.
/// Used for unchanged files during incremental reindex so call resolution still works.
pub fn register_file(reg: &mut Registry, project: &str, file: &DiscoveredFile) {
    // Dispatch Java/Kotlin to tree-sitter AST extractors
    match file.language {
        Language::Java => {
            crate::spring_java::register_java(reg, project, file);
            return;
        }
        Language::Kotlin => {
            crate::spring_kotlin::register_kotlin(reg, project, file);
            return;
        }
        Language::Go => {
            crate::go_adapter::register_go(reg, project, file);
            return;
        }
        _ => {}
    }

    let source = match std::fs::read_to_string(&file.abs_path) {
        Ok(s) => s,
        Err(_) => return,
    };

    // Try tree-sitter first for supported languages
    if let Some(symbols) = codryn_treesitter::extract_symbols(file.language, &source) {
        for sym in &symbols {
            let qn = if let Some(ref parent) = sym.parent_name {
                fqn::fqn_compute(
                    project,
                    &file.rel_path,
                    Some(&format!("{}.{}", parent, sym.name)),
                )
            } else {
                fqn::fqn_compute(project, &file.rel_path, Some(&sym.name))
            };
            reg.register(
                &sym.name,
                &qn,
                &file.rel_path,
                &sym.label,
                sym.start_line,
                sym.end_line,
            );
        }
        return; // Tree-sitter succeeded, skip regex
    }

    // Fall through to regex extraction if tree-sitter returned None
    let patterns = get_patterns(file.language);
    let lines: Vec<&str> = source.lines().collect();
    for (i, &line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        for (pat, label) in &patterns {
            if let Some(caps) = pat.captures(trimmed) {
                if let Some(name) = caps.get(1) {
                    let name = name.as_str();
                    if matches!(
                        name,
                        "if" | "for"
                            | "while"
                            | "switch"
                            | "catch"
                            | "return"
                            | "new"
                            | "typeof"
                            | "instanceof"
                    ) {
                        continue;
                    }
                    if name.len() > 1 && !name.starts_with('_') || label == &"Class" {
                        let qn = fqn::fqn_compute(project, &file.rel_path, Some(name));
                        let line_num = (i + 1) as i32;
                        let end = compute_end_line(&lines, i, file.language);
                        reg.register(name, &qn, &file.rel_path, label, line_num, end);
                    }
                }
            }
        }
    }
}

fn get_patterns(lang: Language) -> Vec<(Regex, &'static str)> {
    match lang {
        Language::Python => vec![
            (re(r"^def\s+(\w+)\s*\("), "Function"),
            (re(r"^class\s+(\w+)"), "Class"),
        ],
        Language::JavaScript | Language::TypeScript | Language::Tsx => vec![
            // Named functions
            (
                re(r"^(?:export\s+)?(?:default\s+)?(?:async\s+)?function\s*\*?\s*(\w+)"),
                "Function",
            ),
            // Arrow functions: const foo = (...) => / const foo = async (...) =>
            (
                re(
                    r"^(?:export\s+)?(?:const|let|var)\s+(\w+)\s*=\s*(?:async\s+)?(?:<[^>]*>\s*)?\(",
                ),
                "Function",
            ),
            // Arrow functions: const foo = async arg =>
            (
                re(r"^(?:export\s+)?(?:const|let|var)\s+(\w+)\s*=\s*(?:async\s+)?\w+\s*=>"),
                "Function",
            ),
            // Classes and abstract classes
            (re(r"^(?:export\s+)?(?:abstract\s+)?class\s+(\w+)"), "Class"),
            // Interfaces
            (re(r"^(?:export\s+)?interface\s+(\w+)"), "Interface"),
            // Type aliases
            (re(r"^(?:export\s+)?type\s+(\w+)\s*[=<]"), "Interface"),
            // Enums
            (re(r"^(?:export\s+)?(?:const\s+)?enum\s+(\w+)"), "Class"),
            // Class methods with access modifiers
            (
                re(
                    r"^(?:(?:public|private|protected|static|async|override|abstract)\s+)+(\w+)\s*(?:<[^>]*>)?\s*\(",
                ),
                "Function",
            ),
            // Bare methods: methodName() or methodName<T>() — indented, no keyword prefix
            // Must be indented (not at column 0) to avoid matching top-level calls
            (
                re(r"^  +(\w+)\s*(?:<[^>]*>)?\s*\([^)]*\)\s*(?::\s*\w[\w<>\[\]|&\s]*?)?\s*\{"),
                "Function",
            ),
        ],
        Language::Rust => vec![
            (
                re(r"^pub(?:\(crate\))?\s+(?:async\s+)?fn\s+(\w+)"),
                "Function",
            ),
            (re(r"^(?:pub\s+)?(?:async\s+)?fn\s+(\w+)"), "Function"),
            (re(r"^(?:pub\s+)?struct\s+(\w+)"), "Class"),
            (re(r"^(?:pub\s+)?trait\s+(\w+)"), "Interface"),
            (re(r"^(?:pub\s+)?enum\s+(\w+)"), "Class"),
            (re(r"^impl(?:<[^>]*>)?\s+(\w+)"), "Class"),
        ],
        Language::Go => vec![
            (re(r"^func\s+(?:\([^)]+\)\s+)?(\w+)\s*\("), "Function"),
            (re(r"^type\s+(\w+)\s+struct"), "Class"),
            (re(r"^type\s+(\w+)\s+interface"), "Interface"),
        ],
        Language::Java | Language::Kotlin => vec![
            (
                re(
                    r"(?:public|private|protected|static|final|abstract|synchronized|native|\s)+\s+(?:\w+(?:<[^>]*>)?)\s+(\w+)\s*\(",
                ),
                "Method",
            ),
            (re(r"(?:public\s+)?(?:abstract\s+)?class\s+(\w+)"), "Class"),
            (re(r"(?:public\s+)?interface\s+(\w+)"), "Interface"),
            (re(r"(?:public\s+)?enum\s+(\w+)"), "Class"),
        ],
        Language::CSharp => vec![
            (
                re(r"(?:public|private|protected|internal|static|\s)+\s+\w+\s+(\w+)\s*\("),
                "Function",
            ),
            (re(r"(?:public\s+)?class\s+(\w+)"), "Class"),
            (re(r"(?:public\s+)?interface\s+(\w+)"), "Interface"),
        ],
        Language::Cpp | Language::C => vec![
            (re(r"^\w[\w\s\*]*\s+(\w+)\s*\([^;]*$"), "Function"),
            (re(r"^(?:class|struct)\s+(\w+)"), "Class"),
        ],
        Language::Ruby => vec![
            (re(r"^\s*def\s+(\w+)"), "Function"),
            (re(r"^\s*class\s+(\w+)"), "Class"),
            (re(r"^\s*module\s+(\w+)"), "Class"),
        ],
        Language::Php => vec![
            (
                re(r"(?:public|private|protected|static|\s)+\s*function\s+(\w+)"),
                "Function",
            ),
            (re(r"^class\s+(\w+)"), "Class"),
            (re(r"^interface\s+(\w+)"), "Interface"),
        ],
        Language::Swift => vec![
            (re(r"^\s*func\s+(\w+)"), "Function"),
            (re(r"^\s*class\s+(\w+)"), "Class"),
            (re(r"^\s*protocol\s+(\w+)"), "Interface"),
            (re(r"^\s*struct\s+(\w+)"), "Class"),
        ],
        Language::Elixir => vec![
            (re(r"^\s*def\s+(\w+)"), "Function"),
            (re(r"^\s*defmodule\s+(\w+)"), "Class"),
        ],
        Language::Scala => vec![
            (re(r"^\s*def\s+(\w+)"), "Function"),
            (re(r"^\s*class\s+(\w+)"), "Class"),
            (re(r"^\s*trait\s+(\w+)"), "Interface"),
            (re(r"^\s*object\s+(\w+)"), "Class"),
        ],
        _ => vec![
            // Generic fallback: match common function/class patterns
            (re(r"(?:function|def|fn|func)\s+(\w+)"), "Function"),
            (re(r"(?:class|struct|type)\s+(\w+)"), "Class"),
        ],
    }
}

fn compute_end_line(lines: &[&str], start_idx: usize, lang: Language) -> i32 {
    let total = lines.len();
    if start_idx >= total {
        return (start_idx + 1) as i32;
    }
    let uses_indent = matches!(lang, Language::Python | Language::Ruby | Language::Elixir);
    if uses_indent {
        let base = lines[start_idx].len() - lines[start_idx].trim_start().len();
        let mut last = start_idx;
        for (j, &line) in lines.iter().enumerate().skip(start_idx + 1) {
            if line.trim().is_empty() {
                continue;
            }
            let indent = line.len() - line.trim_start().len();
            if indent <= base {
                break;
            }
            last = j;
        }
        (last + 1) as i32
    } else {
        let mut depth: i32 = 0;
        let mut found_open = false;
        for (j, &line) in lines.iter().enumerate().skip(start_idx) {
            for ch in line.chars() {
                if ch == '{' {
                    depth += 1;
                    found_open = true;
                } else if ch == '}' {
                    depth -= 1;
                    if found_open && depth <= 0 {
                        return (j + 1) as i32;
                    }
                }
            }
        }
        (start_idx + 1) as i32
    }
}

fn re(pattern: &str) -> Regex {
    Regex::new(pattern).unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::TypeRegistry;
    use codryn_treesitter::{TsParam, TsSymbol};

    fn make_symbol(
        name: &str,
        return_type: Option<&str>,
        params: Vec<(&str, Option<&str>)>,
    ) -> TsSymbol {
        TsSymbol {
            name: name.to_string(),
            label: "Function".to_string(),
            start_line: 1,
            end_line: 10,
            parent_name: None,
            signature: None,
            return_type: return_type.map(|s| s.to_string()),
            parameters: params
                .into_iter()
                .map(|(n, t)| TsParam {
                    name: n.to_string(),
                    type_name: t.map(|s| s.to_string()),
                })
                .collect(),
            docstring: None,
            decorators: vec![],
            base_classes: vec![],
            is_exported: false,
            is_abstract: false,
            is_async: false,
            is_test: false,
            is_entry_point: false,
            body_text: None,
        }
    }

    #[test]
    fn test_extract_type_assigns_return_type() {
        let mut type_reg = TypeRegistry::new();
        let symbols = vec![make_symbol("process", Some("MyResult"), vec![])];
        extract_type_assigns(&mut type_reg, "src/lib.rs", &symbols, Language::Rust);

        let entry = type_reg.lookup_type("src/lib.rs", "process::return");
        assert!(entry.is_some());
        assert_eq!(entry.unwrap().resolved_type, "MyResult");
    }

    #[test]
    fn test_extract_type_assigns_param_types() {
        let mut type_reg = TypeRegistry::new();
        let symbols = vec![make_symbol(
            "handle",
            None,
            vec![("req", Some("HttpRequest")), ("ctx", Some("AppContext"))],
        )];
        extract_type_assigns(&mut type_reg, "src/handler.rs", &symbols, Language::Rust);

        let req_entry = type_reg.lookup_type("src/handler.rs", "handle::req");
        assert!(req_entry.is_some());
        assert_eq!(req_entry.unwrap().resolved_type, "HttpRequest");

        let ctx_entry = type_reg.lookup_type("src/handler.rs", "handle::ctx");
        assert!(ctx_entry.is_some());
        assert_eq!(ctx_entry.unwrap().resolved_type, "AppContext");
    }

    #[test]
    fn test_extract_type_assigns_skips_stdlib_rust() {
        let mut type_reg = TypeRegistry::new();
        let symbols = vec![make_symbol(
            "get_items",
            Some("Vec"),
            vec![("name", Some("String"))],
        )];
        extract_type_assigns(&mut type_reg, "src/lib.rs", &symbols, Language::Rust);

        // Vec and String are stdlib types — should be skipped
        assert!(type_reg
            .lookup_type("src/lib.rs", "get_items::return")
            .is_none());
        assert!(type_reg
            .lookup_type("src/lib.rs", "get_items::name")
            .is_none());
    }

    #[test]
    fn test_extract_type_assigns_skips_stdlib_go() {
        let mut type_reg = TypeRegistry::new();
        let symbols = vec![make_symbol(
            "serve",
            Some("error"),
            vec![("ctx", Some("Context"))],
        )];
        extract_type_assigns(&mut type_reg, "main.go", &symbols, Language::Go);

        // error and Context are Go stdlib types
        assert!(type_reg.lookup_type("main.go", "serve::return").is_none());
        assert!(type_reg.lookup_type("main.go", "serve::ctx").is_none());
    }

    #[test]
    fn test_extract_type_assigns_skips_stdlib_typescript() {
        let mut type_reg = TypeRegistry::new();
        let symbols = vec![make_symbol(
            "fetchData",
            Some("Promise"),
            vec![("url", Some("string"))],
        )];
        extract_type_assigns(&mut type_reg, "src/api.ts", &symbols, Language::TypeScript);

        assert!(type_reg
            .lookup_type("src/api.ts", "fetchData::return")
            .is_none());
        assert!(type_reg
            .lookup_type("src/api.ts", "fetchData::url")
            .is_none());
    }

    #[test]
    fn test_extract_type_assigns_skips_stdlib_python() {
        let mut type_reg = TypeRegistry::new();
        let symbols = vec![make_symbol(
            "process",
            Some("dict"),
            vec![("items", Some("list"))],
        )];
        extract_type_assigns(&mut type_reg, "app.py", &symbols, Language::Python);

        assert!(type_reg.lookup_type("app.py", "process::return").is_none());
        assert!(type_reg.lookup_type("app.py", "process::items").is_none());
    }

    #[test]
    fn test_extract_type_assigns_keeps_custom_types() {
        let mut type_reg = TypeRegistry::new();
        let symbols = vec![make_symbol(
            "create_order",
            Some("OrderResult"),
            vec![("req", Some("OrderRequest")), ("id", Some("i32"))],
        )];
        extract_type_assigns(&mut type_reg, "src/orders.rs", &symbols, Language::Rust);

        // Custom types should be registered
        assert!(type_reg
            .lookup_type("src/orders.rs", "create_order::return")
            .is_some());
        assert!(type_reg
            .lookup_type("src/orders.rs", "create_order::req")
            .is_some());
        // i32 is stdlib — should be skipped
        assert!(type_reg
            .lookup_type("src/orders.rs", "create_order::id")
            .is_none());
    }

    #[test]
    fn test_extract_type_assigns_params_without_types() {
        let mut type_reg = TypeRegistry::new();
        let symbols = vec![make_symbol("greet", None, vec![("name", None)])];
        extract_type_assigns(&mut type_reg, "app.js", &symbols, Language::JavaScript);

        // No types to register
        assert!(type_reg.is_empty());
    }
}
