use codryn_discover::{DiscoveredFile, Language};
use codryn_foundation::fqn;
use codryn_graph_buffer::GraphBuffer;
use regex::Regex;

use crate::registry::Registry;

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
        _ => {}
    }

    let source = match std::fs::read_to_string(&file.abs_path) {
        Ok(s) => s,
        Err(_) => return,
    };

    let is_ts = matches!(
        file.language,
        Language::TypeScript | Language::Tsx | Language::JavaScript
    );
    let is_java = matches!(file.language, Language::Java | Language::Kotlin);
    let patterns = get_patterns(file.language);
    let lines: Vec<&str> = source.lines().collect();
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
        _ => {}
    }

    let source = match std::fs::read_to_string(&file.abs_path) {
        Ok(s) => s,
        Err(_) => return,
    };
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
            // Next.js route handlers (before generic `function`)
            (
                re(r"^export\s+(?:async\s+)?function\s+(GET|POST|PUT|PATCH|DELETE|HEAD|OPTIONS)\b"),
                "Function",
            ),
            // AWS Lambda-style handlers
            (re(r"^export\s+const\s+(handler)\b"), "Function"),
            (re(r"^export\s+async\s+function\s+(handler)\b"), "Function"),
            (re(r"^\s*exports\.(handler)\s*="), "Function"),
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
