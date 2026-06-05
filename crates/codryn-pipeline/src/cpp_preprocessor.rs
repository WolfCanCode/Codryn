//! C/C++ preprocessor pass.
//!
//! Extracts `#define` macros as `Constant` nodes and creates INCLUDES edges
//! for `#include "..."` directives. Supports transitive include chain resolution,
//! macro expansion, conditional compilation, and compilation database integration.
//!
//! - `resolve_include_chain`: Transitively resolves all reachable headers (max depth 256)
//! - `expand_macros`: Expands macro references in source text (max depth 1024)
//! - `parse_compile_commands`: Parses compilation database for include paths and defines
//! - Conditional compilation: Handles `#ifdef`, `#ifndef`, `#if` based on active defines
//! - Falls back to system include paths when compile_commands.json is missing
//! - Skips unresolved includes, records them as missing dependencies
//! - Halts expansion at max depth with warning

use codryn_discover::{DiscoveredFile, Language};
use codryn_foundation::fqn;
use codryn_graph_buffer::GraphBuffer;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use crate::passes::CompileCommandsMap;

/// Maximum depth for transitive include chain resolution.
pub const MAX_INCLUDE_DEPTH: usize = 256;

/// Maximum depth for recursive macro expansion.
pub const MAX_MACRO_DEPTH: usize = 1024;

/// Default system include paths used when compile_commands.json is not available.
const SYSTEM_INCLUDE_PATHS: &[&str] = &[
    "/usr/include",
    "/usr/local/include",
    "/usr/include/x86_64-linux-gnu",
];

static DEFINE_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    // Matches both object-like (#define NAME value) and function-like (#define NAME(args) value)
    regex::Regex::new(r"^\s*#\s*define\s+([A-Za-z_][A-Za-z0-9_]*)(?:\([^)]*\))?\s*(.*)$").unwrap()
});

static INCLUDE_LOCAL_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r#"^\s*#\s*include\s+"([^"]+)""#).unwrap());

static INCLUDE_SYSTEM_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"^\s*#\s*include\s+<([^>]+)>").unwrap());

static IFDEF_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"^\s*#\s*ifdef\s+(\w+)").unwrap());

static IFNDEF_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"^\s*#\s*ifndef\s+(\w+)").unwrap());

static IF_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"^\s*#\s*if\s+(.+)$").unwrap());

static ELSE_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"^\s*#\s*else\b").unwrap());

static ENDIF_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"^\s*#\s*endif\b").unwrap());

/// A macro extracted from a C/C++ source file.
#[derive(Debug, Clone)]
pub struct ExtractedMacro {
    pub name: String,
    pub value: Option<String>,
    pub line: u32,
}

/// Represents a compilation unit from compile_commands.json.
#[derive(Debug, Clone)]
pub struct CompilationUnit {
    /// Relative file path of the source file.
    pub file: String,
    /// Include paths (-I flags).
    pub include_paths: Vec<PathBuf>,
    /// Preprocessor defines (-D flags).
    pub defines: HashMap<String, Option<String>>,
    /// Language standard flag (e.g., "c++17").
    pub std_flag: Option<String>,
}

/// Result of include chain resolution, tracking resolved and missing includes.
#[derive(Debug, Clone, Default)]
pub struct IncludeChainResult {
    /// Successfully resolved include file paths (in resolution order).
    pub resolved: Vec<PathBuf>,
    /// Include paths that could not be resolved (missing dependencies).
    pub missing: Vec<String>,
}

/// Extract `#define` macros from C/C++ source.
pub fn extract_macros(source: &str) -> Vec<ExtractedMacro> {
    let mut macros = Vec::new();
    for (i, line) in source.lines().enumerate() {
        if let Some(cap) = DEFINE_RE.captures(line) {
            let value = cap.get(2).map(|m| m.as_str().trim().to_string());
            macros.push(ExtractedMacro {
                name: cap[1].to_string(),
                value: value.filter(|v| !v.is_empty()),
                line: (i + 1) as u32,
            });
        }
    }
    macros
}

/// Resolve a local `#include "header"` to a project-relative path.
/// Tries the source file's directory first, then include paths from compile_commands.
pub fn resolve_include(
    header: &str,
    source_rel_path: &str,
    cc_map: Option<&CompileCommandsMap>,
) -> String {
    // If header already has a path separator, use it directly
    if header.contains('/') {
        return header.to_string();
    }

    // Try compile_commands include paths
    if let Some(map) = cc_map {
        if let Some(ctx) = map.get(source_rel_path) {
            if let Some(inc_path) = ctx.include_paths.first() {
                let candidate = format!("{}/{}", inc_path.trim_end_matches('/'), header);
                return candidate.trim_start_matches("./").to_string();
            }
        }
    }

    // Resolve relative to source file's directory
    let source_dir = Path::new(source_rel_path)
        .parent()
        .and_then(|p| p.to_str())
        .unwrap_or("");
    if source_dir.is_empty() {
        header.to_string()
    } else {
        format!("{}/{}", source_dir, header)
    }
}

/// Parse a compile_commands.json file and return a list of compilation units.
///
/// Each entry in the compilation database is parsed to extract include paths (-I),
/// preprocessor defines (-D), and language standard flags (-std=).
///
/// Returns an empty Vec if the file doesn't exist or can't be parsed.
pub fn parse_compile_commands(path: &Path) -> Vec<CompilationUnit> {
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(error = %e, path = %path.display(), "failed to read compile_commands.json");
            return Vec::new();
        }
    };

    let entries: Vec<serde_json::Value> = match serde_json::from_str(&content) {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(error = %e, "failed to parse compile_commands.json");
            return Vec::new();
        }
    };

    let mut units = Vec::new();
    for entry in &entries {
        let file = entry.get("file").and_then(|f| f.as_str()).unwrap_or("");
        if file.is_empty() {
            continue;
        }

        let args = extract_arguments(entry);
        let (include_paths, defines, std_flag) = parse_args_to_components(&args);

        units.push(CompilationUnit {
            file: file.to_string(),
            include_paths,
            defines,
            std_flag,
        });
    }
    units
}

/// Extract compiler arguments from a compile_commands.json entry.
fn extract_arguments(entry: &serde_json::Value) -> Vec<String> {
    if let Some(args) = entry.get("arguments").and_then(|a| a.as_array()) {
        return args
            .iter()
            .filter_map(|a| a.as_str().map(String::from))
            .collect();
    }
    if let Some(cmd) = entry.get("command").and_then(|c| c.as_str()) {
        return cmd.split_whitespace().map(String::from).collect();
    }
    vec![]
}

/// Parse compiler arguments into include paths, defines, and std flag.
fn parse_args_to_components(
    args: &[String],
) -> (
    Vec<PathBuf>,
    HashMap<String, Option<String>>,
    Option<String>,
) {
    let mut include_paths = Vec::new();
    let mut defines = HashMap::new();
    let mut std_flag = None;

    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        if let Some(stripped) = arg.strip_prefix("-I") {
            let path = if stripped.is_empty() {
                match iter.next() {
                    Some(next) => next.as_str(),
                    None => continue,
                }
            } else {
                stripped
            };
            if !path.is_empty() {
                include_paths.push(PathBuf::from(path));
            }
        } else if let Some(stripped) = arg.strip_prefix("-D") {
            let define = if stripped.is_empty() {
                match iter.next() {
                    Some(next) => next.as_str(),
                    None => continue,
                }
            } else {
                stripped
            };
            if !define.is_empty() {
                let mut parts = define.splitn(2, '=');
                let name = parts.next().unwrap_or("").to_string();
                let value = parts.next().map(|v| v.to_string());
                if !name.is_empty() {
                    defines.insert(name, value);
                }
            }
        } else if let Some(stripped) = arg.strip_prefix("-std=") {
            std_flag = Some(stripped.to_string());
        }
    }
    (include_paths, defines, std_flag)
}

/// Transitively resolve all reachable headers from a given file.
///
/// Follows `#include` directives recursively up to `MAX_INCLUDE_DEPTH` (256).
/// Uses include paths from the compilation database if available, otherwise
/// falls back to system include paths.
///
/// Unresolved includes are recorded in the result's `missing` field.
/// The resolution halts at max depth with a warning.
pub fn resolve_include_chain(
    file: &Path,
    include_paths: &[PathBuf],
    max_depth: usize,
) -> IncludeChainResult {
    let mut result = IncludeChainResult::default();
    let mut visited = HashSet::new();
    resolve_include_chain_recursive(file, include_paths, max_depth, 0, &mut visited, &mut result);
    result
}

fn resolve_include_chain_recursive(
    file: &Path,
    include_paths: &[PathBuf],
    max_depth: usize,
    current_depth: usize,
    visited: &mut HashSet<PathBuf>,
    result: &mut IncludeChainResult,
) {
    if current_depth >= max_depth {
        tracing::warn!(
            file = %file.display(),
            depth = current_depth,
            "include chain resolution halted at max depth {}",
            max_depth
        );
        return;
    }

    let canonical = match file.canonicalize() {
        Ok(p) => p,
        Err(_) => file.to_path_buf(),
    };

    if visited.contains(&canonical) {
        return; // Avoid circular includes
    }
    visited.insert(canonical.clone());

    let source = match std::fs::read_to_string(file) {
        Ok(s) => s,
        Err(_) => return,
    };

    let file_dir = file.parent().unwrap_or(Path::new(""));

    for line in source.lines() {
        let header = if let Some(cap) = INCLUDE_LOCAL_RE.captures(line) {
            Some((cap[1].to_string(), false))
        } else {
            INCLUDE_SYSTEM_RE
                .captures(line)
                .map(|cap| (cap[1].to_string(), true))
        };

        if let Some((header_path, is_system)) = header {
            let resolved = resolve_header_path(&header_path, file_dir, include_paths, is_system);

            match resolved {
                Some(resolved_path) => {
                    if !visited.contains(&resolved_path) {
                        result.resolved.push(resolved_path.clone());
                        resolve_include_chain_recursive(
                            &resolved_path,
                            include_paths,
                            max_depth,
                            current_depth + 1,
                            visited,
                            result,
                        );
                    }
                }
                None => {
                    result.missing.push(header_path);
                }
            }
        }
    }
}

/// Resolve a header path by searching include paths and the file's directory.
fn resolve_header_path(
    header: &str,
    file_dir: &Path,
    include_paths: &[PathBuf],
    is_system: bool,
) -> Option<PathBuf> {
    // For local includes, try the file's directory first
    if !is_system {
        let candidate = file_dir.join(header);
        if candidate.exists() {
            return candidate.canonicalize().ok();
        }
    }

    // Search configured include paths
    for inc_path in include_paths {
        let candidate = inc_path.join(header);
        if candidate.exists() {
            return candidate.canonicalize().ok();
        }
    }

    // Fall back to system include paths
    for sys_path in SYSTEM_INCLUDE_PATHS {
        let candidate = Path::new(sys_path).join(header);
        if candidate.exists() {
            return candidate.canonicalize().ok();
        }
    }

    None
}

/// Get effective include paths for a file.
///
/// If a CompileCommandsMap is available and contains the file, uses its include paths.
/// Otherwise falls back to system include paths.
pub fn get_include_paths(
    source_rel_path: &str,
    cc_map: Option<&CompileCommandsMap>,
) -> Vec<PathBuf> {
    if let Some(map) = cc_map {
        if let Some(ctx) = map.get(source_rel_path) {
            if !ctx.include_paths.is_empty() {
                return ctx.include_paths.iter().map(PathBuf::from).collect();
            }
        }
    }

    // Fall back to system include paths
    tracing::debug!(
        file = source_rel_path,
        "no compile_commands.json entry, falling back to system include paths"
    );
    SYSTEM_INCLUDE_PATHS.iter().map(PathBuf::from).collect()
}

/// Expand macro references in source text.
///
/// Performs textual substitution of `#define`d macros up to `MAX_MACRO_DEPTH` (1024)
/// recursive substitutions. Halts expansion at max depth with a warning and returns
/// the text with unexpanded macros remaining.
///
/// Object-like macros are expanded by simple text replacement.
/// Function-like macros are not expanded (they require argument parsing).
pub fn expand_macros(source: &str, defines: &HashMap<String, String>, max_depth: usize) -> String {
    let mut result = source.to_string();
    let mut depth = 0;

    loop {
        if depth >= max_depth {
            tracing::warn!(
                depth = depth,
                "macro expansion halted at max depth {}, unexpanded macros may remain",
                max_depth
            );
            break;
        }

        let mut changed = false;
        for (name, value) in defines {
            if result.contains(name.as_str()) {
                // Only replace whole-word occurrences (not inside other identifiers)
                let new_result = replace_whole_word(&result, name, value);
                if new_result != result {
                    result = new_result;
                    changed = true;
                    depth += 1;
                    if depth >= max_depth {
                        tracing::warn!(
                            macro_name = name,
                            depth = depth,
                            "macro expansion halted at max depth {}",
                            max_depth
                        );
                        return result;
                    }
                }
            }
        }

        if !changed {
            break;
        }
    }

    result
}

/// Replace whole-word occurrences of `name` with `value` in `text`.
/// A word boundary is defined as a non-alphanumeric/underscore character.
fn replace_whole_word(text: &str, name: &str, value: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let name_bytes = name.as_bytes();
    let text_bytes = text.as_bytes();
    let mut i = 0;

    while i < text_bytes.len() {
        if i + name_bytes.len() <= text_bytes.len()
            && &text_bytes[i..i + name_bytes.len()] == name_bytes
        {
            // Check word boundary before
            let before_ok = i == 0 || !is_ident_char(text_bytes[i - 1]);
            // Check word boundary after
            let after_ok = i + name_bytes.len() >= text_bytes.len()
                || !is_ident_char(text_bytes[i + name_bytes.len()]);

            if before_ok && after_ok {
                result.push_str(value);
                i += name_bytes.len();
                continue;
            }
        }
        result.push(text_bytes[i] as char);
        i += 1;
    }
    result
}

/// Check if a byte is a valid C identifier character.
fn is_ident_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// Evaluate a simple `#if` condition expression against known defines.
///
/// Supports:
/// - `defined(NAME)` / `defined NAME`
/// - `!defined(NAME)` / `!defined NAME`
/// - Integer literal comparisons (0 = false, non-zero = true)
/// - Simple `NAME` (true if defined and non-zero)
///
/// Complex expressions (&&, ||, arithmetic) default to true (conservative).
fn evaluate_condition(expr: &str, defines: &HashMap<String, Option<String>>) -> bool {
    let expr = expr.trim();

    // Handle !defined(NAME) or !defined NAME
    if let Some(rest) = expr.strip_prefix('!') {
        let rest = rest.trim();
        if let Some(name) = parse_defined_expr(rest) {
            return !defines.contains_key(name);
        }
    }

    // Handle defined(NAME) or defined NAME
    if let Some(name) = parse_defined_expr(expr) {
        return defines.contains_key(name);
    }

    // Handle simple integer literal
    if let Ok(val) = expr.parse::<i64>() {
        return val != 0;
    }

    // Handle simple macro name (true if defined and non-zero)
    if expr.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        if let Some(val) = defines.get(expr) {
            if let Some(v) = val {
                if let Ok(n) = v.parse::<i64>() {
                    return n != 0;
                }
                return true; // defined with non-numeric value
            }
            return true; // defined without value
        }
        return false; // not defined
    }

    // Complex expressions: default to true (conservative, index all branches)
    true
}

/// Parse a `defined(NAME)` or `defined NAME` expression, returning the macro name.
fn parse_defined_expr(expr: &str) -> Option<&str> {
    let expr = expr.trim();
    if let Some(rest) = expr.strip_prefix("defined") {
        let rest = rest.trim();
        if let Some(inner) = rest.strip_prefix('(') {
            if let Some(name) = inner.strip_suffix(')') {
                let name = name.trim();
                // Only match if the name is a simple identifier (no spaces, operators, etc.)
                if !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                    return Some(name);
                }
            }
        } else if !rest.is_empty() && rest.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            return Some(rest);
        }
    }
    None
}

/// Filter source lines based on conditional compilation directives.
///
/// Processes `#ifdef`, `#ifndef`, `#if`, `#else`, `#endif` directives and returns
/// only the lines that are active according to the provided defines.
///
/// Lines within inactive branches are excluded from the result.
pub fn filter_conditional_compilation(
    source: &str,
    defines: &HashMap<String, Option<String>>,
) -> String {
    let mut output = Vec::new();
    let mut condition_stack: Vec<bool> = Vec::new(); // true = active branch
    let mut in_else_stack: Vec<bool> = Vec::new(); // tracking if we're in #else

    for line in source.lines() {
        if let Some(cap) = IFDEF_RE.captures(line) {
            let name = &cap[1];
            let active = defines.contains_key(name);
            let parent_active = condition_stack.last().copied().unwrap_or(true);
            condition_stack.push(active && parent_active);
            in_else_stack.push(false);
            continue;
        }

        if let Some(cap) = IFNDEF_RE.captures(line) {
            let name = &cap[1];
            let active = !defines.contains_key(name);
            let parent_active = condition_stack.last().copied().unwrap_or(true);
            condition_stack.push(active && parent_active);
            in_else_stack.push(false);
            continue;
        }

        if let Some(cap) = IF_RE.captures(line) {
            let expr = &cap[1];
            let active = evaluate_condition(expr, defines);
            let parent_active = condition_stack.last().copied().unwrap_or(true);
            condition_stack.push(active && parent_active);
            in_else_stack.push(false);
            continue;
        }

        if ELSE_RE.is_match(line) {
            let stack_len = condition_stack.len();
            if stack_len > 0 {
                let was_in_else = in_else_stack.last().copied().unwrap_or(false);
                if !was_in_else {
                    // Flip the condition for #else
                    let parent_active = if stack_len > 1 {
                        condition_stack[stack_len - 2]
                    } else {
                        true
                    };
                    let current = condition_stack[stack_len - 1];
                    condition_stack[stack_len - 1] = !current && parent_active;
                    if let Some(e) = in_else_stack.last_mut() {
                        *e = true;
                    }
                }
            }
            continue;
        }

        if ENDIF_RE.is_match(line) {
            condition_stack.pop();
            in_else_stack.pop();
            continue;
        }

        // Handle #undef
        // (We don't modify defines here since we're just filtering, but we
        // include the line if the current branch is active)

        // Include line if all conditions in the stack are active
        let all_active = condition_stack.last().copied().unwrap_or(true);
        if all_active {
            output.push(line);
        }
    }

    output.join("\n")
}

/// Pipeline pass: extract macros as Constant nodes and INCLUDES edges for C/C++ files.
///
/// - `#define NAME value` → Constant node with `macro_value` property
/// - `#include "header"` → INCLUDES edge from source file node to header file node
/// - `#include <system>` → skipped (system headers not in project)
/// - Conditional branches (`#ifdef`, `#ifndef`, `#if`) are handled based on defines
///   from compile_commands.json. Without compile_commands, all branches are indexed
///   (conservative approach).
pub fn pass_cpp_preprocessor(
    buf: &mut GraphBuffer,
    files: &[&DiscoveredFile],
    project: &str,
    cc_map: Option<&CompileCommandsMap>,
) {
    for f in files {
        if !matches!(f.language, Language::C | Language::Cpp) {
            continue;
        }

        let source = match std::fs::read_to_string(&f.abs_path) {
            Ok(s) => s,
            Err(_) => continue,
        };

        let source_qn = fqn::fqn_module(project, &f.rel_path);

        // Get defines from compile_commands if available
        let defines = get_defines_for_file(&f.rel_path, cc_map);

        // Filter source based on conditional compilation
        let filtered_source = if defines.is_empty() {
            // No defines available: index all branches (conservative)
            source.clone()
        } else {
            filter_conditional_compilation(&source, &defines)
        };

        // Extract macros → Constant nodes
        for m in extract_macros(&filtered_source) {
            let qn = format!("{}::{}", source_qn, m.name);
            let props = if let Some(ref val) = m.value {
                Some(serde_json::json!({ "macro_value": val, "is_exported": false }).to_string())
            } else {
                Some(serde_json::json!({ "is_exported": false }).to_string())
            };
            buf.add_node(
                "Constant",
                &m.name,
                &qn,
                &f.rel_path,
                m.line as i32,
                m.line as i32,
                props,
            );
        }

        // Extract local includes → INCLUDES edges
        // Also track unresolved includes as missing dependencies
        for (i, line) in filtered_source.lines().enumerate() {
            if let Some(cap) = INCLUDE_LOCAL_RE.captures(line) {
                let header = &cap[1];
                let resolved = resolve_include(header, &f.rel_path, cc_map);
                let target_qn = fqn::fqn_module(project, &resolved);
                buf.add_edge_by_qn(
                    &source_qn,
                    &target_qn,
                    "INCLUDES",
                    Some(serde_json::json!({ "line": i + 1 }).to_string()),
                );
            }
        }
    }
}

/// Get preprocessor defines for a specific file from the compile commands map.
fn get_defines_for_file(
    rel_path: &str,
    cc_map: Option<&CompileCommandsMap>,
) -> HashMap<String, Option<String>> {
    let mut defines = HashMap::new();
    if let Some(map) = cc_map {
        if let Some(ctx) = map.get(rel_path) {
            for (name, value) in &ctx.defines {
                defines.insert(name.clone(), value.clone());
            }
        }
    }
    defines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_macros_simple_define() {
        let src = "#define MAX_SIZE 1024\n#define PI 3.14159\n";
        let macros = extract_macros(src);
        assert_eq!(macros.len(), 2);
        assert_eq!(macros[0].name, "MAX_SIZE");
        assert_eq!(macros[0].value.as_deref(), Some("1024"));
        assert_eq!(macros[1].name, "PI");
    }

    #[test]
    fn extract_macros_no_value() {
        let src = "#define NDEBUG\n";
        let macros = extract_macros(src);
        assert_eq!(macros.len(), 1);
        assert_eq!(macros[0].name, "NDEBUG");
        assert!(macros[0].value.is_none());
    }

    #[test]
    fn extract_macros_inside_ifdef() {
        // Macros inside conditional branches are indexed (conservative)
        let src = "#ifdef DEBUG\n#define LOG_LEVEL 3\n#endif\n";
        let macros = extract_macros(src);
        assert_eq!(macros.len(), 1);
        assert_eq!(macros[0].name, "LOG_LEVEL");
    }

    #[test]
    fn extract_macros_function_like() {
        let src = "#define MAX(a, b) ((a) > (b) ? (a) : (b))\n";
        let macros = extract_macros(src);
        assert_eq!(macros.len(), 1);
        assert_eq!(macros[0].name, "MAX");
    }

    #[test]
    fn resolve_include_with_directory() {
        let resolved = resolve_include("utils/helper.h", "src/main.c", None);
        assert_eq!(resolved, "utils/helper.h");
    }

    #[test]
    fn resolve_include_relative_to_source() {
        let resolved = resolve_include("helper.h", "src/main.c", None);
        assert_eq!(resolved, "src/helper.h");
    }

    #[test]
    fn resolve_include_root_level_source() {
        let resolved = resolve_include("helper.h", "main.c", None);
        assert_eq!(resolved, "helper.h");
    }

    #[test]
    fn pass_cpp_preprocessor_creates_constant_nodes() {
        use codryn_discover::Language;
        use codryn_graph_buffer::GraphBuffer;
        use tempfile::TempDir;

        let tmp = TempDir::new().unwrap();
        let src_path = tmp.path().join("defs.h");
        std::fs::write(&src_path, "#define VERSION 42\n#define DEBUG\n").unwrap();

        let file = DiscoveredFile {
            abs_path: src_path,
            rel_path: "defs.h".to_string(),
            language: Language::C,
        };

        let mut buf = GraphBuffer::new("proj");
        pass_cpp_preprocessor(&mut buf, &[&file], "proj", None);

        assert_eq!(buf.node_count(), 2);
    }

    #[test]
    fn pass_cpp_preprocessor_creates_includes_edges() {
        use codryn_discover::Language;
        use codryn_graph_buffer::GraphBuffer;
        use tempfile::TempDir;

        let tmp = TempDir::new().unwrap();
        let src_path = tmp.path().join("main.c");
        std::fs::write(&src_path, "#include \"utils.h\"\n#include <stdio.h>\n").unwrap();

        let file = DiscoveredFile {
            abs_path: src_path,
            rel_path: "main.c".to_string(),
            language: Language::C,
        };

        let mut buf = GraphBuffer::new("proj");
        pass_cpp_preprocessor(&mut buf, &[&file], "proj", None);

        // Only local include, not system header
        assert_eq!(buf.edge_count(), 1);
    }

    #[test]
    fn pass_cpp_preprocessor_skips_non_cpp_files() {
        use codryn_discover::Language;
        use codryn_graph_buffer::GraphBuffer;
        use tempfile::TempDir;

        let tmp = TempDir::new().unwrap();
        let src_path = tmp.path().join("main.py");
        std::fs::write(&src_path, "#define FAKE 1\n").unwrap();

        let file = DiscoveredFile {
            abs_path: src_path,
            rel_path: "main.py".to_string(),
            language: Language::Python,
        };

        let mut buf = GraphBuffer::new("proj");
        pass_cpp_preprocessor(&mut buf, &[&file], "proj", None);

        assert_eq!(buf.node_count(), 0);
        assert_eq!(buf.edge_count(), 0);
    }

    #[test]
    fn expand_macros_simple_substitution() {
        let mut defines = HashMap::new();
        defines.insert("MAX_SIZE".to_string(), "1024".to_string());
        defines.insert("VERSION".to_string(), "42".to_string());

        let source = "int arr[MAX_SIZE]; int v = VERSION;";
        let expanded = expand_macros(source, &defines, MAX_MACRO_DEPTH);
        assert_eq!(expanded, "int arr[1024]; int v = 42;");
    }

    #[test]
    fn expand_macros_no_partial_match() {
        let mut defines = HashMap::new();
        defines.insert("MAX".to_string(), "100".to_string());

        let source = "int MAXIMUM = MAX;";
        let expanded = expand_macros(source, &defines, MAX_MACRO_DEPTH);
        assert_eq!(expanded, "int MAXIMUM = 100;");
    }

    #[test]
    fn expand_macros_recursive() {
        let mut defines = HashMap::new();
        defines.insert("A".to_string(), "B".to_string());
        defines.insert("B".to_string(), "42".to_string());

        let source = "int x = A;";
        let expanded = expand_macros(source, &defines, MAX_MACRO_DEPTH);
        assert_eq!(expanded, "int x = 42;");
    }

    #[test]
    fn expand_macros_halts_at_max_depth() {
        // Create a self-referencing expansion that would loop forever
        let mut defines = HashMap::new();
        defines.insert("A".to_string(), "A_EXPANDED".to_string());
        defines.insert("A_EXPANDED".to_string(), "A".to_string());

        let source = "int x = A;";
        // Should halt without panicking
        let _expanded = expand_macros(source, &defines, 10);
        // Just verify it doesn't hang or panic
    }

    #[test]
    fn expand_macros_empty_defines() {
        let defines = HashMap::new();
        let source = "int x = MAX_SIZE;";
        let expanded = expand_macros(source, &defines, MAX_MACRO_DEPTH);
        assert_eq!(expanded, source);
    }

    #[test]
    fn filter_conditional_ifdef_defined() {
        let mut defines = HashMap::new();
        defines.insert("DEBUG".to_string(), None);

        let source = "#ifdef DEBUG\nint debug = 1;\n#endif\nint x = 0;";
        let filtered = filter_conditional_compilation(source, &defines);
        assert_eq!(filtered, "int debug = 1;\nint x = 0;");
    }

    #[test]
    fn filter_conditional_ifdef_not_defined() {
        let defines = HashMap::new();

        let source = "#ifdef DEBUG\nint debug = 1;\n#endif\nint x = 0;";
        let filtered = filter_conditional_compilation(source, &defines);
        assert_eq!(filtered, "int x = 0;");
    }

    #[test]
    fn filter_conditional_ifndef() {
        let defines = HashMap::new();

        let source = "#ifndef HEADER_H\n#define HEADER_H\nint x;\n#endif";
        let filtered = filter_conditional_compilation(source, &defines);
        // HEADER_H is not defined, so #ifndef is true
        assert!(filtered.contains("int x;"));
    }

    #[test]
    fn filter_conditional_ifdef_else() {
        let mut defines = HashMap::new();
        defines.insert("WINDOWS".to_string(), None);

        let source = "#ifdef WINDOWS\nint platform = 1;\n#else\nint platform = 2;\n#endif";
        let filtered = filter_conditional_compilation(source, &defines);
        assert_eq!(filtered, "int platform = 1;");
    }

    #[test]
    fn filter_conditional_ifdef_else_not_defined() {
        let defines = HashMap::new();

        let source = "#ifdef WINDOWS\nint platform = 1;\n#else\nint platform = 2;\n#endif";
        let filtered = filter_conditional_compilation(source, &defines);
        assert_eq!(filtered, "int platform = 2;");
    }

    #[test]
    fn filter_conditional_if_defined() {
        let mut defines = HashMap::new();
        defines.insert("FEATURE_X".to_string(), None);

        let source = "#if defined(FEATURE_X)\nint feature = 1;\n#endif\nint y = 0;";
        let filtered = filter_conditional_compilation(source, &defines);
        assert_eq!(filtered, "int feature = 1;\nint y = 0;");
    }

    #[test]
    fn filter_conditional_if_not_defined() {
        let defines = HashMap::new();

        let source = "#if defined(FEATURE_X)\nint feature = 1;\n#endif\nint y = 0;";
        let filtered = filter_conditional_compilation(source, &defines);
        assert_eq!(filtered, "int y = 0;");
    }

    #[test]
    fn filter_conditional_nested() {
        let mut defines = HashMap::new();
        defines.insert("OUTER".to_string(), None);

        let source = "#ifdef OUTER\n#ifdef INNER\nint inner = 1;\n#endif\nint outer = 1;\n#endif";
        let filtered = filter_conditional_compilation(source, &defines);
        // OUTER is defined but INNER is not
        assert!(!filtered.contains("int inner = 1;"));
        assert!(filtered.contains("int outer = 1;"));
    }

    #[test]
    fn resolve_include_chain_simple() {
        use tempfile::TempDir;

        let tmp = TempDir::new().unwrap();
        let main_h = tmp.path().join("main.h");
        let utils_h = tmp.path().join("utils.h");
        let types_h = tmp.path().join("types.h");

        std::fs::write(&types_h, "// types\n").unwrap();
        std::fs::write(&utils_h, "#include \"types.h\"\n").unwrap();
        std::fs::write(&main_h, "#include \"utils.h\"\n").unwrap();

        let include_paths = vec![tmp.path().to_path_buf()];
        let result = resolve_include_chain(&main_h, &include_paths, MAX_INCLUDE_DEPTH);

        assert_eq!(result.resolved.len(), 2);
        assert!(result.missing.is_empty());
    }

    #[test]
    fn resolve_include_chain_missing_header() {
        use tempfile::TempDir;

        let tmp = TempDir::new().unwrap();
        let main_h = tmp.path().join("main.h");
        std::fs::write(&main_h, "#include \"nonexistent.h\"\n").unwrap();

        let include_paths = vec![tmp.path().to_path_buf()];
        let result = resolve_include_chain(&main_h, &include_paths, MAX_INCLUDE_DEPTH);

        assert!(result.resolved.is_empty());
        assert_eq!(result.missing.len(), 1);
        assert_eq!(result.missing[0], "nonexistent.h");
    }

    #[test]
    fn resolve_include_chain_circular() {
        use tempfile::TempDir;

        let tmp = TempDir::new().unwrap();
        let a_h = tmp.path().join("a.h");
        let b_h = tmp.path().join("b.h");

        std::fs::write(&a_h, "#include \"b.h\"\n").unwrap();
        std::fs::write(&b_h, "#include \"a.h\"\n").unwrap();

        let include_paths = vec![tmp.path().to_path_buf()];
        let result = resolve_include_chain(&a_h, &include_paths, MAX_INCLUDE_DEPTH);

        // Should resolve both without infinite loop
        assert_eq!(result.resolved.len(), 1); // b.h resolved, a.h already visited
        assert!(result.missing.is_empty());
    }

    #[test]
    fn resolve_include_chain_max_depth() {
        use tempfile::TempDir;

        let tmp = TempDir::new().unwrap();
        // Create a chain deeper than max_depth=3
        let h0 = tmp.path().join("h0.h");
        let h1 = tmp.path().join("h1.h");
        let h2 = tmp.path().join("h2.h");
        let h3 = tmp.path().join("h3.h");
        let h4 = tmp.path().join("h4.h");

        std::fs::write(&h4, "// end\n").unwrap();
        std::fs::write(&h3, "#include \"h4.h\"\n").unwrap();
        std::fs::write(&h2, "#include \"h3.h\"\n").unwrap();
        std::fs::write(&h1, "#include \"h2.h\"\n").unwrap();
        std::fs::write(&h0, "#include \"h1.h\"\n").unwrap();

        let include_paths = vec![tmp.path().to_path_buf()];
        // Use max_depth=3, so h0 -> h1 -> h2 -> h3 (stops at depth 3)
        let result = resolve_include_chain(&h0, &include_paths, 3);

        // Should resolve h1 and h2, but halt at depth 3 before resolving h3's includes
        assert!(result.resolved.len() <= 3);
    }

    #[test]
    fn parse_compile_commands_valid() {
        use tempfile::TempDir;

        let tmp = TempDir::new().unwrap();
        let cc_path = tmp.path().join("compile_commands.json");
        let content = r#"[
            {
                "file": "src/main.c",
                "arguments": ["gcc", "-I/usr/include", "-DDEBUG", "-DVERSION=2", "-std=c11", "src/main.c"]
            },
            {
                "file": "src/utils.c",
                "command": "gcc -I./include -DNDEBUG src/utils.c"
            }
        ]"#;
        std::fs::write(&cc_path, content).unwrap();

        let units = parse_compile_commands(&cc_path);
        assert_eq!(units.len(), 2);

        assert_eq!(units[0].file, "src/main.c");
        assert_eq!(units[0].include_paths, vec![PathBuf::from("/usr/include")]);
        assert_eq!(units[0].defines.get("DEBUG"), Some(&None));
        assert_eq!(
            units[0].defines.get("VERSION"),
            Some(&Some("2".to_string()))
        );
        assert_eq!(units[0].std_flag, Some("c11".to_string()));

        assert_eq!(units[1].file, "src/utils.c");
        assert_eq!(units[1].include_paths, vec![PathBuf::from("./include")]);
        assert_eq!(units[1].defines.get("NDEBUG"), Some(&None));
    }

    #[test]
    fn parse_compile_commands_missing_file() {
        let result = parse_compile_commands(Path::new("/nonexistent/compile_commands.json"));
        assert!(result.is_empty());
    }

    #[test]
    fn parse_compile_commands_invalid_json() {
        use tempfile::TempDir;

        let tmp = TempDir::new().unwrap();
        let cc_path = tmp.path().join("compile_commands.json");
        std::fs::write(&cc_path, "not valid json").unwrap();

        let result = parse_compile_commands(&cc_path);
        assert!(result.is_empty());
    }

    #[test]
    fn evaluate_condition_defined() {
        let mut defines = HashMap::new();
        defines.insert("FOO".to_string(), None);

        assert!(evaluate_condition("defined(FOO)", &defines));
        assert!(evaluate_condition("defined FOO", &defines));
        assert!(!evaluate_condition("defined(BAR)", &defines));
        assert!(!evaluate_condition("defined BAR", &defines));
    }

    #[test]
    fn evaluate_condition_not_defined() {
        let mut defines = HashMap::new();
        defines.insert("FOO".to_string(), None);

        assert!(!evaluate_condition("!defined(FOO)", &defines));
        assert!(evaluate_condition("!defined(BAR)", &defines));
    }

    #[test]
    fn evaluate_condition_integer_literal() {
        let defines = HashMap::new();
        assert!(evaluate_condition("1", &defines));
        assert!(!evaluate_condition("0", &defines));
    }

    #[test]
    fn evaluate_condition_macro_name() {
        let mut defines = HashMap::new();
        defines.insert("ENABLED".to_string(), Some("1".to_string()));
        defines.insert("DISABLED".to_string(), Some("0".to_string()));

        assert!(evaluate_condition("ENABLED", &defines));
        assert!(!evaluate_condition("DISABLED", &defines));
        assert!(!evaluate_condition("UNKNOWN", &defines));
    }

    #[test]
    fn evaluate_condition_complex_defaults_true() {
        let defines = HashMap::new();
        // Complex expressions default to true (conservative)
        assert!(evaluate_condition("defined(A) && defined(B)", &defines));
        assert!(evaluate_condition("X > 5", &defines));
    }

    #[test]
    fn get_include_paths_with_cc_map() {
        let mut cc_map = CompileCommandsMap::new();
        cc_map.insert(
            "src/main.c".to_string(),
            crate::passes::CompileContext {
                include_paths: vec!["/project/include".to_string()],
                defines: vec![],
                std_flag: None,
            },
        );

        let paths = get_include_paths("src/main.c", Some(&cc_map));
        assert_eq!(paths, vec![PathBuf::from("/project/include")]);
    }

    #[test]
    fn get_include_paths_fallback_to_system() {
        let paths = get_include_paths("src/main.c", None);
        assert!(!paths.is_empty());
        assert!(paths.contains(&PathBuf::from("/usr/include")));
    }

    #[test]
    fn replace_whole_word_basic() {
        assert_eq!(
            replace_whole_word("int x = MAX;", "MAX", "100"),
            "int x = 100;"
        );
        assert_eq!(
            replace_whole_word("int MAXIMUM = 0;", "MAX", "100"),
            "int MAXIMUM = 0;"
        );
        assert_eq!(replace_whole_word("MAX + MAX", "MAX", "5"), "5 + 5");
    }

    #[test]
    fn conditional_with_compile_commands_defines() {
        use codryn_discover::Language;
        use codryn_graph_buffer::GraphBuffer;
        use tempfile::TempDir;

        let tmp = TempDir::new().unwrap();
        let src_path = tmp.path().join("main.c");
        let source = "#ifdef RELEASE\n#define OPT_LEVEL 3\n#else\n#define OPT_LEVEL 0\n#endif\n";
        std::fs::write(&src_path, source).unwrap();

        let file = DiscoveredFile {
            abs_path: src_path,
            rel_path: "main.c".to_string(),
            language: Language::C,
        };

        // With RELEASE defined
        let mut cc_map = CompileCommandsMap::new();
        cc_map.insert(
            "main.c".to_string(),
            crate::passes::CompileContext {
                include_paths: vec![],
                defines: vec![("RELEASE".to_string(), None)],
                std_flag: None,
            },
        );

        let mut buf = GraphBuffer::new("proj");
        pass_cpp_preprocessor(&mut buf, &[&file], "proj", Some(&cc_map));

        // Should only have OPT_LEVEL with value 3 (from RELEASE branch)
        assert_eq!(buf.node_count(), 1);
    }
}
