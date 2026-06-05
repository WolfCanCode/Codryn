//! Tier 3 regex-based extractors for programming languages that lack tree-sitter grammars.
//!
//! These extractors use regex patterns to identify top-level functions, modules, and type
//! definitions in languages like Fortran, COBOL, Ada, Pascal, Odin, Crystal, GDScript,
//! Gleam, Elm, and Nix.

use codryn_discover::Language;
use codryn_foundation::fqn;
use regex::Regex;
use std::sync::LazyLock;

use crate::extraction::ExtractionNode;

/// Extract top-level definitions from Tier 3 programming languages using regex patterns.
///
/// Returns a vector of `ExtractionNode` for each recognized definition (functions,
/// modules, type definitions). Returns an empty vector if no definitions are found.
pub fn extract_tier3_programming(
    source: &str,
    file_path: &str,
    project: &str,
    lang: Language,
) -> Vec<ExtractionNode> {
    let patterns = get_tier3_patterns(lang);
    if patterns.is_empty() {
        return Vec::new();
    }

    let lines: Vec<&str> = source.lines().collect();
    let mut nodes = Vec::new();

    for (i, &line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        let line_num = (i + 1) as i32;

        for (pat, label) in &patterns {
            if let Some(caps) = pat.captures(trimmed) {
                if let Some(name_match) = caps.get(1) {
                    let name = name_match.as_str();
                    // Skip empty names or very short noise
                    if name.is_empty() {
                        continue;
                    }

                    let qn = fqn::fqn_compute(project, file_path, Some(name));
                    let end_line = compute_end_line_tier3(&lines, i, lang);

                    nodes.push(ExtractionNode {
                        label: label.to_string(),
                        name: name.to_owned(),
                        qualified_name: qn,
                        file_path: file_path.to_owned(),
                        start_line: line_num,
                        end_line,
                        properties_json: None,
                    });
                    break; // Only match first pattern per line
                }
            }
        }
    }

    nodes
}

/// Get regex patterns for a specific Tier 3 programming language.
/// Each pattern returns (Regex, label) where label is "Function", "Class", or "Module".
fn get_tier3_patterns(lang: Language) -> Vec<(&'static Regex, &'static str)> {
    match lang {
        Language::Fortran => fortran_patterns(),
        Language::Cobol => cobol_patterns(),
        Language::Ada => ada_patterns(),
        Language::Pascal => pascal_patterns(),
        Language::Odin => odin_patterns(),
        Language::Crystal => crystal_patterns(),
        Language::GDScript => gdscript_patterns(),
        Language::Gleam => gleam_patterns(),
        Language::Elm => elm_patterns(),
        Language::Nix => nix_patterns(),
        _ => Vec::new(),
    }
}

// ── Fortran ──────────────────────────────────────────────────────────────────

static FORTRAN_SUBROUTINE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^\s*(?:recursive\s+)?subroutine\s+(\w+)").unwrap());
static FORTRAN_FUNCTION: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^\s*(?:\w+\s+)?function\s+(\w+)").unwrap());
static FORTRAN_MODULE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^\s*module\s+(\w+)").unwrap());
static FORTRAN_TYPE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^\s*type(?:\s*,\s*\w+)*\s*::\s*(\w+)").unwrap());
static FORTRAN_PROGRAM: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^\s*program\s+(\w+)").unwrap());

fn fortran_patterns() -> Vec<(&'static Regex, &'static str)> {
    vec![
        (&FORTRAN_SUBROUTINE, "Function"),
        (&FORTRAN_FUNCTION, "Function"),
        (&FORTRAN_MODULE, "Module"),
        (&FORTRAN_TYPE, "Class"),
        (&FORTRAN_PROGRAM, "Module"),
    ]
}

// ── COBOL ────────────────────────────────────────────────────────────────────

static COBOL_DIVISION: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^\s*(\w[\w-]*)\s+DIVISION").unwrap());
static COBOL_SECTION: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^\s*(\w[\w-]*)\s+SECTION\s*\.").unwrap());
static COBOL_PARAGRAPH: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^\s{0,3}(\w[\w-]*)\s*\.\s*$").unwrap());
static COBOL_PROGRAM_ID: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^\s*PROGRAM-ID\.\s*(\w[\w-]*)").unwrap());

fn cobol_patterns() -> Vec<(&'static Regex, &'static str)> {
    vec![
        (&COBOL_PROGRAM_ID, "Module"),
        (&COBOL_DIVISION, "Module"),
        (&COBOL_SECTION, "Function"),
        (&COBOL_PARAGRAPH, "Function"),
    ]
}

// ── Ada ──────────────────────────────────────────────────────────────────────

static ADA_PROCEDURE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^\s*(?:overriding\s+)?procedure\s+(\w+)").unwrap());
static ADA_FUNCTION: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^\s*(?:overriding\s+)?function\s+(\w+)").unwrap());
static ADA_PACKAGE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^\s*package\s+(?:body\s+)?(\w[\w.]*)").unwrap());
static ADA_TYPE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^\s*type\s+(\w+)\s+is").unwrap());
static ADA_TASK: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^\s*task\s+(?:body\s+)?(\w+)").unwrap());

fn ada_patterns() -> Vec<(&'static Regex, &'static str)> {
    vec![
        (&ADA_PROCEDURE, "Function"),
        (&ADA_FUNCTION, "Function"),
        (&ADA_PACKAGE, "Module"),
        (&ADA_TYPE, "Class"),
        (&ADA_TASK, "Function"),
    ]
}

// ── Pascal ───────────────────────────────────────────────────────────────────

static PASCAL_PROCEDURE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^\s*procedure\s+(\w+)").unwrap());
static PASCAL_FUNCTION: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^\s*function\s+(\w+)").unwrap());
static PASCAL_UNIT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^\s*unit\s+(\w+)").unwrap());
static PASCAL_PROGRAM: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^\s*program\s+(\w+)").unwrap());
static PASCAL_TYPE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^\s*(\w+)\s*=\s*(?:class|record|object|interface)").unwrap());

fn pascal_patterns() -> Vec<(&'static Regex, &'static str)> {
    vec![
        (&PASCAL_PROCEDURE, "Function"),
        (&PASCAL_FUNCTION, "Function"),
        (&PASCAL_UNIT, "Module"),
        (&PASCAL_PROGRAM, "Module"),
        (&PASCAL_TYPE, "Class"),
    ]
}

// ── Odin ─────────────────────────────────────────────────────────────────────

static ODIN_PROC: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\s*(\w+)\s*::\s*proc").unwrap());
static ODIN_STRUCT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*(\w+)\s*::\s*struct").unwrap());
static ODIN_ENUM: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\s*(\w+)\s*::\s*enum").unwrap());
static ODIN_UNION: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*(\w+)\s*::\s*union").unwrap());
static ODIN_PACKAGE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*package\s+(\w+)").unwrap());

fn odin_patterns() -> Vec<(&'static Regex, &'static str)> {
    vec![
        (&ODIN_PACKAGE, "Module"),
        (&ODIN_PROC, "Function"),
        (&ODIN_STRUCT, "Class"),
        (&ODIN_ENUM, "Class"),
        (&ODIN_UNION, "Class"),
    ]
}

// ── Crystal ──────────────────────────────────────────────────────────────────

static CRYSTAL_DEF: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\s*(?:private\s+|protected\s+)?def\s+(?:self\.)?(\w+)").unwrap()
});
static CRYSTAL_CLASS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*(?:abstract\s+)?class\s+(\w+)").unwrap());
static CRYSTAL_MODULE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*module\s+(\w+)").unwrap());
static CRYSTAL_STRUCT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*(?:abstract\s+)?struct\s+(\w+)").unwrap());
static CRYSTAL_ENUM: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\s*enum\s+(\w+)").unwrap());

fn crystal_patterns() -> Vec<(&'static Regex, &'static str)> {
    vec![
        (&CRYSTAL_MODULE, "Module"),
        (&CRYSTAL_CLASS, "Class"),
        (&CRYSTAL_STRUCT, "Class"),
        (&CRYSTAL_ENUM, "Class"),
        (&CRYSTAL_DEF, "Function"),
    ]
}

// ── GDScript ─────────────────────────────────────────────────────────────────

static GDSCRIPT_FUNC: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*(?:static\s+)?func\s+(\w+)").unwrap());
static GDSCRIPT_CLASS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*class\s+(\w+)").unwrap());
static GDSCRIPT_CLASS_NAME: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*class_name\s+(\w+)").unwrap());
static GDSCRIPT_SIGNAL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*signal\s+(\w+)").unwrap());
static GDSCRIPT_ENUM: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\s*enum\s+(\w+)").unwrap());

fn gdscript_patterns() -> Vec<(&'static Regex, &'static str)> {
    vec![
        (&GDSCRIPT_CLASS_NAME, "Class"),
        (&GDSCRIPT_CLASS, "Class"),
        (&GDSCRIPT_ENUM, "Class"),
        (&GDSCRIPT_FUNC, "Function"),
        (&GDSCRIPT_SIGNAL, "Function"),
    ]
}

// ── Gleam ────────────────────────────────────────────────────────────────────

static GLEAM_PUB_FN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*pub\s+fn\s+(\w+)").unwrap());
static GLEAM_FN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\s*fn\s+(\w+)").unwrap());
static GLEAM_PUB_TYPE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*pub\s+type\s+(\w+)").unwrap());
static GLEAM_TYPE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\s*type\s+(\w+)").unwrap());
static GLEAM_PUB_OPAQUE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*pub\s+opaque\s+type\s+(\w+)").unwrap());

fn gleam_patterns() -> Vec<(&'static Regex, &'static str)> {
    vec![
        (&GLEAM_PUB_OPAQUE, "Class"),
        (&GLEAM_PUB_TYPE, "Class"),
        (&GLEAM_TYPE, "Class"),
        (&GLEAM_PUB_FN, "Function"),
        (&GLEAM_FN, "Function"),
    ]
}

// ── Elm ──────────────────────────────────────────────────────────────────────

static ELM_MODULE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*(?:port\s+)?module\s+([\w.]+)").unwrap());
static ELM_TYPE_ALIAS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*type\s+alias\s+(\w+)").unwrap());
static ELM_TYPE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\s*type\s+(\w+)").unwrap());
// Elm top-level function: starts at column 0, name followed by arguments or type annotation
static ELM_FUNCTION: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^([a-z]\w*)\s+(?::|[^=]*=)").unwrap());
static ELM_PORT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\s*port\s+(\w+)\s*:").unwrap());

fn elm_patterns() -> Vec<(&'static Regex, &'static str)> {
    vec![
        (&ELM_MODULE, "Module"),
        (&ELM_TYPE_ALIAS, "Class"),
        (&ELM_TYPE, "Class"),
        (&ELM_PORT, "Function"),
        (&ELM_FUNCTION, "Function"),
    ]
}

// ── Nix ──────────────────────────────────────────────────────────────────────

// Nix attribute set bindings: name = ...;
static NIX_BINDING: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\s*(\w+)\s*=\s*(?:\{|let|rec|import|builtins|lib\.|pkgs\.|fetchurl|mkDerivation|buildPythonPackage|stdenv)").unwrap()
});
// Nix function definitions: name = { ... }: or name = args:
static NIX_FUNCTION: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*(\w+)\s*=\s*(?:\{[^}]*\}|[\w]+)\s*:").unwrap());
// Nix inherit statements are not definitions, skip them
// Nix let bindings at top level
static NIX_LET_BINDING: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\s*(\w+)\s*=\s*(?:mkDerivation|buildPythonPackage|fetchFromGitHub|callPackage)")
        .unwrap()
});

fn nix_patterns() -> Vec<(&'static Regex, &'static str)> {
    vec![
        (&NIX_FUNCTION, "Function"),
        (&NIX_LET_BINDING, "Function"),
        (&NIX_BINDING, "Function"),
    ]
}

// ── End-line computation ─────────────────────────────────────────────────────

/// Compute the end line for a definition starting at `start_idx`.
/// Uses language-appropriate heuristics: indent-based for Python-like languages,
/// brace-counting for C-like, and keyword-based for others.
fn compute_end_line_tier3(lines: &[&str], start_idx: usize, lang: Language) -> i32 {
    let total = lines.len();
    if start_idx >= total {
        return (start_idx + 1) as i32;
    }

    match lang {
        // Indent-based languages (GDScript, Nix partially)
        Language::GDScript => {
            let base_indent = lines[start_idx].len() - lines[start_idx].trim_start().len();
            let mut last = start_idx;
            for (j, &line) in lines.iter().enumerate().skip(start_idx + 1) {
                if line.trim().is_empty() {
                    continue;
                }
                let indent = line.len() - line.trim_start().len();
                if indent <= base_indent {
                    break;
                }
                last = j;
            }
            (last + 1) as i32
        }
        // Keyword-based end (Fortran, Ada, Pascal, COBOL, Elm)
        Language::Fortran => find_end_keyword(
            lines,
            start_idx,
            &[
                "end",
                "endsubroutine",
                "endfunction",
                "endmodule",
                "endprogram",
                "end subroutine",
                "end function",
                "end module",
                "end program",
                "end type",
            ],
        ),
        Language::Ada => find_end_keyword(lines, start_idx, &["end;", "end "]),
        Language::Pascal => {
            // Pascal uses begin/end blocks
            brace_like_end(lines, start_idx, "begin", "end")
        }
        Language::Cobol => {
            // COBOL paragraphs end at the next paragraph or section
            let mut last = start_idx;
            for (j, &line) in lines.iter().enumerate().skip(start_idx + 1) {
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }
                // Next paragraph/section/division starts
                if COBOL_PARAGRAPH.is_match(trimmed)
                    || COBOL_SECTION.is_match(trimmed)
                    || COBOL_DIVISION.is_match(trimmed)
                {
                    break;
                }
                last = j;
            }
            (last + 1) as i32
        }
        // Brace-based languages (Odin, Crystal, Gleam)
        Language::Odin | Language::Crystal | Language::Gleam => {
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
        // Elm: indent-based (definitions at column 0, body is indented)
        Language::Elm => {
            let mut last = start_idx;
            for (j, &line) in lines.iter().enumerate().skip(start_idx + 1) {
                if line.trim().is_empty() {
                    continue;
                }
                // Next top-level definition starts at column 0
                let first_char = line.chars().next().unwrap_or(' ');
                if !first_char.is_whitespace() {
                    break;
                }
                last = j;
            }
            (last + 1) as i32
        }
        // Nix: brace-based
        Language::Nix => {
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
                // Also check for semicolons ending simple bindings
                if !found_open && line.contains(';') {
                    return (j + 1) as i32;
                }
            }
            (start_idx + 1) as i32
        }
        _ => (start_idx + 1) as i32,
    }
}

/// Find end line by scanning for a keyword-based terminator (case-insensitive).
fn find_end_keyword(lines: &[&str], start_idx: usize, keywords: &[&str]) -> i32 {
    for (j, &line) in lines.iter().enumerate().skip(start_idx + 1) {
        let lower = line.trim().to_lowercase();
        for kw in keywords {
            if lower.starts_with(kw) {
                return (j + 1) as i32;
            }
        }
    }
    // If no end keyword found, return start + 1
    (start_idx + 1) as i32
}

/// Find end line using begin/end keyword pairs (Pascal-style).
fn brace_like_end(lines: &[&str], start_idx: usize, open: &str, close: &str) -> i32 {
    let mut depth: i32 = 0;
    let mut found_open = false;
    for (j, &line) in lines.iter().enumerate().skip(start_idx) {
        let lower = line.to_lowercase();
        // Count occurrences of open/close keywords
        for word in lower.split_whitespace() {
            if word == open || word.starts_with(&format!("{open};")) {
                depth += 1;
                found_open = true;
            } else if word == close || word == format!("{close};") || word == format!("{close}.") {
                depth -= 1;
                if found_open && depth <= 0 {
                    return (j + 1) as i32;
                }
            }
        }
    }
    (start_idx + 1) as i32
}

// ══════════════════════════════════════════════════════════════════════════════
// Markup and config file extractors
// ══════════════════════════════════════════════════════════════════════════════

// ── Markdown ─────────────────────────────────────────────────────────────────

static MD_HEADING: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(#{1,3})\s+(.+)$").unwrap());

// ── YAML ─────────────────────────────────────────────────────────────────────

static YAML_TOP_KEY: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^([a-zA-Z_][\w.-]*)\s*:").unwrap());

// ── JSON ─────────────────────────────────────────────────────────────────────

static JSON_TOP_KEY: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"^\s*"([^"]+)"\s*:"#).unwrap());

// ── HTML ─────────────────────────────────────────────────────────────────────

static HTML_ID: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"id\s*=\s*["']([^"']+)["']"#).unwrap());

// ── CSS/SCSS ─────────────────────────────────────────────────────────────────

static CSS_SELECTOR: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*([.#][\w-]+(?:\s*[,>+~]\s*[.#]?[\w-]+)*)\s*\{").unwrap());

/// Extract structural elements from markup and config files.
///
/// - Markdown: headings (levels 1–3)
/// - YAML: top-level keys (depth 1)
/// - JSON: top-level object keys
/// - HTML: elements with id attributes
/// - CSS/SCSS: class selectors and id selectors
///
/// Returns a vector of `ExtractionNode` for each recognized structural element.
pub fn extract_markup(
    source: &str,
    file_path: &str,
    project: &str,
    lang: Language,
) -> Vec<ExtractionNode> {
    match lang {
        Language::Markdown => extract_markdown(source, file_path, project),
        Language::Yaml => extract_yaml(source, file_path, project),
        Language::Json => extract_json(source, file_path, project),
        Language::Html => extract_html(source, file_path, project),
        Language::Css | Language::Scss => extract_css(source, file_path, project),
        _ => Vec::new(),
    }
}

/// Extract headings (levels 1–3) from Markdown files.
fn extract_markdown(source: &str, file_path: &str, project: &str) -> Vec<ExtractionNode> {
    let lines: Vec<&str> = source.lines().collect();
    let mut nodes = Vec::new();

    for (i, &line) in lines.iter().enumerate() {
        if let Some(caps) = MD_HEADING.captures(line) {
            let level = caps.get(1).unwrap().as_str().len();
            if level > 3 {
                continue;
            }
            let name = caps.get(2).unwrap().as_str().trim().to_owned();
            if name.is_empty() {
                continue;
            }

            let line_num = (i + 1) as i32;
            let end_line = compute_markdown_end_line(&lines, i, level);
            let qn = fqn::fqn_compute(project, file_path, Some(&name));

            let label = match level {
                1 => "Module",
                _ => "Function",
            };

            nodes.push(ExtractionNode {
                label: label.to_string(),
                name,
                qualified_name: qn,
                file_path: file_path.to_owned(),
                start_line: line_num,
                end_line,
                properties_json: None,
            });
        }
    }

    nodes
}

/// Compute end line for a Markdown heading: extends until the next heading of same or higher level.
fn compute_markdown_end_line(lines: &[&str], start_idx: usize, level: usize) -> i32 {
    let mut last = start_idx;
    for (j, &line) in lines.iter().enumerate().skip(start_idx + 1) {
        if let Some(caps) = MD_HEADING.captures(line) {
            let next_level = caps.get(1).unwrap().as_str().len();
            if next_level <= level {
                break;
            }
        }
        last = j;
    }
    (last + 1) as i32
}

/// Extract top-level keys from YAML files (depth 1 only).
fn extract_yaml(source: &str, file_path: &str, project: &str) -> Vec<ExtractionNode> {
    let lines: Vec<&str> = source.lines().collect();
    let mut nodes = Vec::new();

    for (i, &line) in lines.iter().enumerate() {
        // Skip comments and empty lines
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') || trimmed == "---" || trimmed == "..." {
            continue;
        }

        // Only match keys at column 0 (no leading whitespace) for top-level
        if line.starts_with(' ') || line.starts_with('\t') {
            continue;
        }

        if let Some(caps) = YAML_TOP_KEY.captures(line) {
            let name = caps.get(1).unwrap().as_str().to_owned();
            if name.is_empty() {
                continue;
            }

            let line_num = (i + 1) as i32;
            let end_line = compute_yaml_end_line(&lines, i);
            let qn = fqn::fqn_compute(project, file_path, Some(&name));

            nodes.push(ExtractionNode {
                label: "Function".to_string(),
                name,
                qualified_name: qn,
                file_path: file_path.to_owned(),
                start_line: line_num,
                end_line,
                properties_json: None,
            });
        }
    }

    nodes
}

/// Compute end line for a YAML top-level key: extends until the next top-level key.
fn compute_yaml_end_line(lines: &[&str], start_idx: usize) -> i32 {
    let mut last = start_idx;
    for (j, &line) in lines.iter().enumerate().skip(start_idx + 1) {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        // Next top-level key starts at column 0
        if !line.starts_with(' ') && !line.starts_with('\t') && YAML_TOP_KEY.is_match(line) {
            break;
        }
        last = j;
    }
    (last + 1) as i32
}

/// Extract top-level object keys from JSON files.
fn extract_json(source: &str, file_path: &str, project: &str) -> Vec<ExtractionNode> {
    let lines: Vec<&str> = source.lines().collect();
    let mut nodes = Vec::new();

    // Track depth line by line
    let mut current_depth: i32 = 0;
    for (i, &line) in lines.iter().enumerate() {
        let depth_at_start = current_depth;

        // Update depth based on this line's braces
        for ch in line.chars() {
            match ch {
                '{' | '[' => current_depth += 1,
                '}' | ']' => current_depth -= 1,
                _ => {}
            }
        }

        // Only match keys at depth 1 (inside the top-level object)
        if depth_at_start == 1 {
            if let Some(caps) = JSON_TOP_KEY.captures(line) {
                let name = caps.get(1).unwrap().as_str().to_owned();
                if name.is_empty() {
                    continue;
                }

                let line_num = (i + 1) as i32;
                let qn = fqn::fqn_compute(project, file_path, Some(&name));

                // End line: scan forward until we return to depth 1 or lower
                let end_line = compute_json_end_line(&lines, i, current_depth);

                nodes.push(ExtractionNode {
                    label: "Function".to_string(),
                    name,
                    qualified_name: qn,
                    file_path: file_path.to_owned(),
                    start_line: line_num,
                    end_line,
                    properties_json: None,
                });
            }
        }
    }

    nodes
}

/// Compute end line for a JSON top-level key's value.
fn compute_json_end_line(lines: &[&str], start_idx: usize, depth_after_start: i32) -> i32 {
    let mut depth = depth_after_start;
    for (j, &line) in lines.iter().enumerate().skip(start_idx + 1) {
        for ch in line.chars() {
            match ch {
                '{' | '[' => depth += 1,
                '}' | ']' => depth -= 1,
                _ => {}
            }
        }
        // When we return to depth 1 or below, the value has ended
        if depth <= 1 {
            return (j + 1) as i32;
        }
    }
    (start_idx + 1) as i32
}

/// Extract elements with id attributes from HTML files.
fn extract_html(source: &str, file_path: &str, project: &str) -> Vec<ExtractionNode> {
    let lines: Vec<&str> = source.lines().collect();
    let mut nodes = Vec::new();

    for (i, &line) in lines.iter().enumerate() {
        // Find all id attributes on this line
        for caps in HTML_ID.captures_iter(line) {
            let name = caps.get(1).unwrap().as_str().to_owned();
            if name.is_empty() {
                continue;
            }

            let line_num = (i + 1) as i32;
            let qn = fqn::fqn_compute(project, file_path, Some(&name));

            nodes.push(ExtractionNode {
                label: "Function".to_string(),
                name,
                qualified_name: qn,
                file_path: file_path.to_owned(),
                start_line: line_num,
                end_line: line_num,
                properties_json: None,
            });
        }
    }

    nodes
}

/// Extract class selectors and id selectors from CSS/SCSS files.
fn extract_css(source: &str, file_path: &str, project: &str) -> Vec<ExtractionNode> {
    let lines: Vec<&str> = source.lines().collect();
    let mut nodes = Vec::new();
    let mut brace_depth: i32 = 0;

    for (i, &line) in lines.iter().enumerate() {
        let trimmed = line.trim();

        // Skip comments
        if trimmed.starts_with("//") || trimmed.starts_with("/*") {
            continue;
        }

        // Only match selectors at top level (not nested inside other rules)
        if brace_depth == 0 {
            if let Some(caps) = CSS_SELECTOR.captures(line) {
                let name = caps.get(1).unwrap().as_str().trim().to_owned();
                if name.is_empty() {
                    continue;
                }

                let line_num = (i + 1) as i32;
                let qn = fqn::fqn_compute(project, file_path, Some(&name));
                let end_line = compute_css_end_line(&lines, i);

                nodes.push(ExtractionNode {
                    label: "Function".to_string(),
                    name,
                    qualified_name: qn,
                    file_path: file_path.to_owned(),
                    start_line: line_num,
                    end_line,
                    properties_json: None,
                });
            }
        }

        // Track brace depth
        for ch in trimmed.chars() {
            match ch {
                '{' => brace_depth += 1,
                '}' => brace_depth -= 1,
                _ => {}
            }
        }
    }

    nodes
}

/// Compute end line for a CSS rule block by counting braces.
fn compute_css_end_line(lines: &[&str], start_idx: usize) -> i32 {
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

// ══════════════════════════════════════════════════════════════════════════════
// Build/Infrastructure File Extractors
// ══════════════════════════════════════════════════════════════════════════════

// ── Dockerfile ───────────────────────────────────────────────────────────────

/// Matches FROM instructions with optional AS alias: `FROM image:tag AS stage_name`
static DOCKERFILE_FROM: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^\s*FROM\s+(\S+)(?:\s+AS\s+(\w+))?").unwrap());

// ── Makefile ─────────────────────────────────────────────────────────────────

/// Matches Makefile targets: `target_name:` (not variable assignments)
/// We match `name:` and then exclude `:=` and `::=` in the extraction logic.
static MAKEFILE_TARGET: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^([a-zA-Z_][\w.-]*)\s*:").unwrap());

// ── CMake ────────────────────────────────────────────────────────────────────

/// Matches CMake function definitions: `function(name ...)`
static CMAKE_FUNCTION: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^\s*function\s*\(\s*(\w+)").unwrap());

/// Matches CMake macro definitions: `macro(name ...)`
static CMAKE_MACRO: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^\s*macro\s*\(\s*(\w+)").unwrap());

/// Matches CMake project declarations: `project(name ...)`
static CMAKE_PROJECT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^\s*project\s*\(\s*(\w+)").unwrap());

/// Extract definitions from build/infrastructure files.
///
/// Supports:
/// - **Dockerfile**: FROM stages (named stages via AS, or image name)
/// - **Makefile**: targets (lines matching `target:`)
/// - **CMake**: functions, macros, and project declarations
///
/// The `ext` parameter determines which parser to use:
/// - `"dockerfile"` or files named `Dockerfile*` → Dockerfile parser
/// - `"makefile"` or files named `Makefile*`, `GNUmakefile` → Makefile parser
/// - `"cmake"` or `".cmake"` or `"CMakeLists.txt"` → CMake parser
pub fn extract_build_infra(
    source: &str,
    file_path: &str,
    project: &str,
    ext: &str,
) -> Vec<ExtractionNode> {
    let ext_lower = ext.to_lowercase();
    let file_lower = file_path.to_lowercase();

    if ext_lower == "dockerfile" || file_lower.contains("dockerfile") {
        extract_dockerfile(source, file_path, project)
    } else if ext_lower == "makefile"
        || ext_lower == "mk"
        || file_lower.ends_with("makefile")
        || file_lower.ends_with("gnumakefile")
        || file_lower.ends_with(".mk")
    {
        extract_makefile(source, file_path, project)
    } else if ext_lower == "cmake"
        || file_lower.ends_with(".cmake")
        || file_lower.ends_with("cmakelists.txt")
    {
        extract_cmake(source, file_path, project)
    } else {
        Vec::new()
    }
}

/// Extract FROM stages from a Dockerfile.
///
/// Named stages (FROM ... AS name) use the stage alias as the node name.
/// Unnamed stages use the image name (without tag) as the node name.
fn extract_dockerfile(source: &str, file_path: &str, project: &str) -> Vec<ExtractionNode> {
    let lines: Vec<&str> = source.lines().collect();
    let mut nodes = Vec::new();

    for (i, &line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        let line_num = (i + 1) as i32;

        if let Some(caps) = DOCKERFILE_FROM.captures(trimmed) {
            // Prefer the AS alias (group 2), fall back to image name (group 1)
            let name = if let Some(alias) = caps.get(2) {
                alias.as_str().to_owned()
            } else {
                // Use image name without tag/digest
                let image = caps.get(1).unwrap().as_str();
                image
                    .split(':')
                    .next()
                    .unwrap_or(image)
                    .split('@')
                    .next()
                    .unwrap_or(image)
                    .to_owned()
            };

            if name.is_empty() {
                continue;
            }

            // Compute end line: scan until next FROM or end of file
            let end_line = compute_end_line_dockerfile(&lines, i);

            let qn = fqn::fqn_compute(project, file_path, Some(&name));
            nodes.push(ExtractionNode {
                label: "Function".to_string(), // Stages are build steps
                name,
                qualified_name: qn,
                file_path: file_path.to_owned(),
                start_line: line_num,
                end_line,
                properties_json: None,
            });
        }
    }

    nodes
}

/// Compute end line for a Dockerfile FROM stage: ends at the next FROM or EOF.
fn compute_end_line_dockerfile(lines: &[&str], start_idx: usize) -> i32 {
    for (j, &line) in lines.iter().enumerate().skip(start_idx + 1) {
        let trimmed = line.trim();
        if DOCKERFILE_FROM.is_match(trimmed) {
            // End at the line before the next FROM
            return j as i32;
        }
    }
    // No next FROM found, stage extends to end of file
    lines.len() as i32
}

/// Extract targets from a Makefile.
fn extract_makefile(source: &str, file_path: &str, project: &str) -> Vec<ExtractionNode> {
    let lines: Vec<&str> = source.lines().collect();
    let mut nodes = Vec::new();

    for (i, &line) in lines.iter().enumerate() {
        let line_num = (i + 1) as i32;

        // Skip lines starting with tab (recipe lines) or comments
        if line.starts_with('\t') || line.trim_start().starts_with('#') {
            continue;
        }

        // Skip variable assignments: NAME := value, NAME ::= value, NAME ?= value
        if line.contains(":=") || line.contains("?=") || line.contains("+=") {
            continue;
        }

        if let Some(caps) = MAKEFILE_TARGET.captures(line) {
            if let Some(name_match) = caps.get(1) {
                let name = name_match.as_str();

                // Skip internal/pattern targets (starting with dot, except known ones)
                if name.starts_with('.') {
                    continue;
                }

                let end_line = compute_end_line_makefile(&lines, i);
                let qn = fqn::fqn_compute(project, file_path, Some(name));

                nodes.push(ExtractionNode {
                    label: "Function".to_string(), // Targets are build actions
                    name: name.to_owned(),
                    qualified_name: qn,
                    file_path: file_path.to_owned(),
                    start_line: line_num,
                    end_line,
                    properties_json: None,
                });
            }
        }
    }

    nodes
}

/// Compute end line for a Makefile target: ends at the next non-indented, non-empty line
/// that is a new target or variable assignment.
fn compute_end_line_makefile(lines: &[&str], start_idx: usize) -> i32 {
    let mut last = start_idx;
    for (j, &line) in lines.iter().enumerate().skip(start_idx + 1) {
        if line.trim().is_empty() {
            continue;
        }
        // Recipe lines start with tab
        if line.starts_with('\t') {
            last = j;
            continue;
        }
        // Next target or variable — stop
        break;
    }
    (last + 1) as i32
}

/// Extract functions, macros, and project declarations from CMake files.
fn extract_cmake(source: &str, file_path: &str, project: &str) -> Vec<ExtractionNode> {
    let lines: Vec<&str> = source.lines().collect();
    let mut nodes = Vec::new();

    for (i, &line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        let line_num = (i + 1) as i32;

        // Check function
        if let Some(caps) = CMAKE_FUNCTION.captures(trimmed) {
            if let Some(name_match) = caps.get(1) {
                let name = name_match.as_str();
                let end_line = find_cmake_end(&lines, i, "endfunction");
                let qn = fqn::fqn_compute(project, file_path, Some(name));
                nodes.push(ExtractionNode {
                    label: "Function".to_string(),
                    name: name.to_owned(),
                    qualified_name: qn,
                    file_path: file_path.to_owned(),
                    start_line: line_num,
                    end_line,
                    properties_json: None,
                });
                continue;
            }
        }

        // Check macro
        if let Some(caps) = CMAKE_MACRO.captures(trimmed) {
            if let Some(name_match) = caps.get(1) {
                let name = name_match.as_str();
                let end_line = find_cmake_end(&lines, i, "endmacro");
                let qn = fqn::fqn_compute(project, file_path, Some(name));
                nodes.push(ExtractionNode {
                    label: "Function".to_string(), // Macros are callable like functions
                    name: name.to_owned(),
                    qualified_name: qn,
                    file_path: file_path.to_owned(),
                    start_line: line_num,
                    end_line,
                    properties_json: None,
                });
                continue;
            }
        }

        // Check project
        if let Some(caps) = CMAKE_PROJECT.captures(trimmed) {
            if let Some(name_match) = caps.get(1) {
                let name = name_match.as_str();
                let qn = fqn::fqn_compute(project, file_path, Some(name));
                nodes.push(ExtractionNode {
                    label: "Module".to_string(), // Project is a top-level module
                    name: name.to_owned(),
                    qualified_name: qn,
                    file_path: file_path.to_owned(),
                    start_line: line_num,
                    end_line: line_num, // project() is typically a single line
                    properties_json: None,
                });
            }
        }
    }

    nodes
}

/// Find the end line for a CMake function/macro by scanning for the matching end keyword.
fn find_cmake_end(lines: &[&str], start_idx: usize, end_keyword: &str) -> i32 {
    for (j, &line) in lines.iter().enumerate().skip(start_idx + 1) {
        let lower = line.trim().to_lowercase();
        if lower.starts_with(end_keyword) {
            return (j + 1) as i32;
        }
    }
    // If no end keyword found, return start line
    (start_idx + 1) as i32
}

// ══════════════════════════════════════════════════════════════════════════════
// Single-File Component (SFC) Extractors — Svelte & Vue
// ══════════════════════════════════════════════════════════════════════════════

// ── Script block detection ───────────────────────────────────────────────────

/// Matches opening <script> tags (with optional attributes like lang="ts", setup, context="module")
static SCRIPT_OPEN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(?i)^\s*<script[^>]*>"#).unwrap());

/// Matches closing </script> tags
static SCRIPT_CLOSE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)^\s*</script>").unwrap());

// ── Svelte patterns ──────────────────────────────────────────────────────────

/// Svelte exported variable: `export let name` or `export const name`
static SVELTE_EXPORT_VAR: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*export\s+(?:let|const|var)\s+(\w+)").unwrap());

/// Svelte function declaration: `function name(` or `export function name(`
static SVELTE_FUNCTION: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*(?:export\s+)?function\s+(\w+)").unwrap());

/// Svelte arrow/const function: `const name = (` or `export const name = (`
static SVELTE_CONST_FN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\s*(?:export\s+)?const\s+(\w+)\s*=\s*(?:\([^)]*\)\s*=>|\w+\s*=>|async\s*\()")
        .unwrap()
});

// ── Vue patterns ─────────────────────────────────────────────────────────────

/// Vue Options API: `name: 'ComponentName'` or `name: "ComponentName"`
static VUE_NAME_OPTION: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"^\s*name\s*:\s*['"](\w+)['"]"#).unwrap());

/// Vue Composition API defineComponent: `export default defineComponent({`
#[allow(dead_code)]
static VUE_DEFINE_COMPONENT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*export\s+default\s+defineComponent\s*\(").unwrap());

/// Vue <script setup> function: `function name(` or `const name = (`
static VUE_SETUP_FUNCTION: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*(?:export\s+)?function\s+(\w+)").unwrap());

/// Vue <script setup> const/let: `const name =` or `let name =`
static VUE_SETUP_VAR: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*(?:export\s+)?(?:const|let|var)\s+(\w+)\s*=").unwrap());

/// Vue defineProps/defineEmits/defineExpose
static VUE_DEFINE_MACRO: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"^\s*(?:const\s+\w+\s*=\s*)?(defineProps|defineEmits|defineExpose|defineSlots)\s*[<(]",
    )
    .unwrap()
});

// ── Template tag references ──────────────────────────────────────────────────

/// Matches component tag references in templates: `<ComponentName` or `<component-name`
/// Excludes standard HTML tags and built-in elements.
static TEMPLATE_COMPONENT_TAG: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"<([A-Z][\w-]*)[\s/>]").unwrap());

/// Matches kebab-case component tags that are likely custom components (contain a hyphen)
static TEMPLATE_KEBAB_TAG: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"<([a-z][\w]*-[\w-]*)[\s/>]").unwrap());

/// Extract definitions from Svelte or Vue single-file components.
///
/// Extracts:
/// - Component name (from filename or explicit name option)
/// - Exported functions and variables from the script block
/// - Component tag references used in the template section
///
/// The `ext` parameter should be `"svelte"` or `"vue"`.
pub fn extract_sfc(source: &str, file_path: &str, project: &str, ext: &str) -> Vec<ExtractionNode> {
    let ext_lower = ext.to_lowercase();
    match ext_lower.as_str() {
        "svelte" => extract_svelte(source, file_path, project),
        "vue" => extract_vue(source, file_path, project),
        _ => Vec::new(),
    }
}

/// Extract definitions from a Svelte single-file component.
fn extract_svelte(source: &str, file_path: &str, project: &str) -> Vec<ExtractionNode> {
    let lines: Vec<&str> = source.lines().collect();
    let mut nodes = Vec::new();

    // Extract component name from filename
    let component_name = component_name_from_path(file_path);
    let comp_qn = fqn::fqn_compute(project, file_path, Some(&component_name));
    nodes.push(ExtractionNode {
        label: "Class".to_string(),
        name: component_name,
        qualified_name: comp_qn,
        file_path: file_path.to_owned(),
        start_line: 1,
        end_line: lines.len() as i32,
        properties_json: None,
    });

    // Find script blocks and extract exports/functions
    let mut in_script = false;
    let mut in_template = false;

    for (i, &line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        let line_num = (i + 1) as i32;

        // Track template sections for component tag extraction
        if trimmed.starts_with("<template")
            || (!trimmed.starts_with("<script")
                && !trimmed.starts_with("<style")
                && !in_script
                && !trimmed.starts_with("</"))
        {
            // In Svelte, template is the default content (not wrapped in <template>)
            // Everything outside <script> and <style> is template
            if !in_script {
                in_template = true;
            }
        }

        // Detect script block boundaries
        if SCRIPT_OPEN.is_match(trimmed) {
            in_script = true;
            in_template = false;
            continue;
        }
        if SCRIPT_CLOSE.is_match(trimmed) {
            in_script = false;
            continue;
        }

        if trimmed.starts_with("<style") {
            in_template = false;
            in_script = false;
        }

        // Extract from script block
        if in_script {
            // Exported variables (props in Svelte)
            if let Some(caps) = SVELTE_EXPORT_VAR.captures(trimmed) {
                let name = caps.get(1).unwrap().as_str();
                let qn = fqn::fqn_compute(project, file_path, Some(name));
                nodes.push(ExtractionNode {
                    label: "Function".to_string(),
                    name: name.to_owned(),
                    qualified_name: qn,
                    file_path: file_path.to_owned(),
                    start_line: line_num,
                    end_line: line_num,
                    properties_json: None,
                });
                continue;
            }

            // Function declarations
            if let Some(caps) = SVELTE_FUNCTION.captures(trimmed) {
                let name = caps.get(1).unwrap().as_str();
                let end_line = compute_brace_end(&lines, i);
                let qn = fqn::fqn_compute(project, file_path, Some(name));
                nodes.push(ExtractionNode {
                    label: "Function".to_string(),
                    name: name.to_owned(),
                    qualified_name: qn,
                    file_path: file_path.to_owned(),
                    start_line: line_num,
                    end_line,
                    properties_json: None,
                });
                continue;
            }

            // Arrow function / const function
            if let Some(caps) = SVELTE_CONST_FN.captures(trimmed) {
                let name = caps.get(1).unwrap().as_str();
                let end_line = compute_brace_end(&lines, i);
                let qn = fqn::fqn_compute(project, file_path, Some(name));
                nodes.push(ExtractionNode {
                    label: "Function".to_string(),
                    name: name.to_owned(),
                    qualified_name: qn,
                    file_path: file_path.to_owned(),
                    start_line: line_num,
                    end_line,
                    properties_json: None,
                });
                continue;
            }
        }

        // Extract component tag references from template
        if in_template {
            extract_template_tags(trimmed, line_num, file_path, project, &mut nodes);
        }
    }

    nodes
}

/// Extract definitions from a Vue single-file component.
fn extract_vue(source: &str, file_path: &str, project: &str) -> Vec<ExtractionNode> {
    let lines: Vec<&str> = source.lines().collect();
    let mut nodes = Vec::new();

    // Extract component name from filename (default)
    let component_name = component_name_from_path(file_path);

    // Check for explicit name in script
    let mut explicit_name: Option<String> = None;
    let mut is_script_setup = false;

    // First pass: detect script type and explicit name
    let mut in_script = false;
    for &line in &lines {
        let trimmed = line.trim();
        if trimmed.contains("<script") && trimmed.contains("setup") {
            is_script_setup = true;
        }
        if SCRIPT_OPEN.is_match(trimmed) {
            in_script = true;
            continue;
        }
        if SCRIPT_CLOSE.is_match(trimmed) {
            in_script = false;
            continue;
        }
        if in_script {
            if let Some(caps) = VUE_NAME_OPTION.captures(trimmed) {
                explicit_name = Some(caps.get(1).unwrap().as_str().to_owned());
            }
        }
    }

    let final_name = explicit_name.unwrap_or(component_name);
    let comp_qn = fqn::fqn_compute(project, file_path, Some(&final_name));
    nodes.push(ExtractionNode {
        label: "Class".to_string(),
        name: final_name,
        qualified_name: comp_qn,
        file_path: file_path.to_owned(),
        start_line: 1,
        end_line: lines.len() as i32,
        properties_json: None,
    });

    // Second pass: extract functions/variables and template tags
    in_script = false;
    let mut in_template = false;

    for (i, &line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        let line_num = (i + 1) as i32;

        // Track template section
        if trimmed.starts_with("<template") {
            in_template = true;
            continue;
        }
        if trimmed.starts_with("</template") {
            in_template = false;
            continue;
        }

        // Track script section
        if SCRIPT_OPEN.is_match(trimmed) {
            in_script = true;
            continue;
        }
        if SCRIPT_CLOSE.is_match(trimmed) {
            in_script = false;
            continue;
        }

        // Extract from script block
        if in_script {
            if is_script_setup {
                // <script setup> mode: extract top-level declarations
                // Vue define macros (defineProps, defineEmits, etc.)
                if let Some(caps) = VUE_DEFINE_MACRO.captures(trimmed) {
                    let name = caps.get(1).unwrap().as_str();
                    let qn = fqn::fqn_compute(project, file_path, Some(name));
                    nodes.push(ExtractionNode {
                        label: "Function".to_string(),
                        name: name.to_owned(),
                        qualified_name: qn,
                        file_path: file_path.to_owned(),
                        start_line: line_num,
                        end_line: line_num,
                        properties_json: None,
                    });
                    continue;
                }

                // Function declarations
                if let Some(caps) = VUE_SETUP_FUNCTION.captures(trimmed) {
                    let name = caps.get(1).unwrap().as_str();
                    let end_line = compute_brace_end(&lines, i);
                    let qn = fqn::fqn_compute(project, file_path, Some(name));
                    nodes.push(ExtractionNode {
                        label: "Function".to_string(),
                        name: name.to_owned(),
                        qualified_name: qn,
                        file_path: file_path.to_owned(),
                        start_line: line_num,
                        end_line,
                        properties_json: None,
                    });
                    continue;
                }

                // Const/let variable declarations (reactive state, composables, etc.)
                if let Some(caps) = VUE_SETUP_VAR.captures(trimmed) {
                    let name = caps.get(1).unwrap().as_str();
                    // Skip common non-meaningful names
                    if name == "props" || name == "emit" || name == "slots" {
                        continue;
                    }
                    let end_line = compute_statement_end(&lines, i);
                    let qn = fqn::fqn_compute(project, file_path, Some(name));
                    nodes.push(ExtractionNode {
                        label: "Function".to_string(),
                        name: name.to_owned(),
                        qualified_name: qn,
                        file_path: file_path.to_owned(),
                        start_line: line_num,
                        end_line,
                        properties_json: None,
                    });
                    continue;
                }
            } else {
                // Options API or Composition API with defineComponent
                // Extract exported functions
                if let Some(caps) = VUE_SETUP_FUNCTION.captures(trimmed) {
                    let name = caps.get(1).unwrap().as_str();
                    let end_line = compute_brace_end(&lines, i);
                    let qn = fqn::fqn_compute(project, file_path, Some(name));
                    nodes.push(ExtractionNode {
                        label: "Function".to_string(),
                        name: name.to_owned(),
                        qualified_name: qn,
                        file_path: file_path.to_owned(),
                        start_line: line_num,
                        end_line,
                        properties_json: None,
                    });
                    continue;
                }
            }
        }

        // Extract component tag references from template
        if in_template {
            extract_template_tags(trimmed, line_num, file_path, project, &mut nodes);
        }
    }

    nodes
}

/// Derive component name from file path.
/// E.g., "src/components/MyButton.svelte" → "MyButton"
fn component_name_from_path(file_path: &str) -> String {
    let path = std::path::Path::new(file_path);
    path.file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("Component")
        .to_owned()
}

/// Extract component tag references from a template line.
/// Adds nodes for PascalCase and kebab-case custom component tags.
fn extract_template_tags(
    line: &str,
    line_num: i32,
    file_path: &str,
    project: &str,
    nodes: &mut Vec<ExtractionNode>,
) {
    // PascalCase component tags (e.g., <MyComponent>)
    for caps in TEMPLATE_COMPONENT_TAG.captures_iter(line) {
        let tag_name = caps.get(1).unwrap().as_str();
        // Skip common built-in/HTML-like tags
        if is_builtin_tag(tag_name) {
            continue;
        }
        // Avoid duplicates within the same file
        if nodes
            .iter()
            .any(|n| n.name == tag_name && n.label == "Module")
        {
            continue;
        }
        let qn = fqn::fqn_compute(project, file_path, Some(tag_name));
        nodes.push(ExtractionNode {
            label: "Module".to_string(), // Component references are module-level imports
            name: tag_name.to_owned(),
            qualified_name: qn,
            file_path: file_path.to_owned(),
            start_line: line_num,
            end_line: line_num,
            properties_json: None,
        });
    }

    // Kebab-case component tags (e.g., <my-component>)
    for caps in TEMPLATE_KEBAB_TAG.captures_iter(line) {
        let tag_name = caps.get(1).unwrap().as_str();
        // Avoid duplicates
        if nodes
            .iter()
            .any(|n| n.name == tag_name && n.label == "Module")
        {
            continue;
        }
        let qn = fqn::fqn_compute(project, file_path, Some(tag_name));
        nodes.push(ExtractionNode {
            label: "Module".to_string(),
            name: tag_name.to_owned(),
            qualified_name: qn,
            file_path: file_path.to_owned(),
            start_line: line_num,
            end_line: line_num,
            properties_json: None,
        });
    }
}

/// Check if a PascalCase tag name is a built-in HTML/SVG element or framework directive.
fn is_builtin_tag(tag: &str) -> bool {
    matches!(
        tag,
        "Teleport" | "Transition" | "TransitionGroup" | "KeepAlive" | "Suspense"
        | "Component" | "Slot" | "Fragment"
        // SVG elements that start with uppercase
        | "SVG" | "Symbol"
    )
}

/// Compute end line for a brace-delimited block starting at or after `start_idx`.
fn compute_brace_end(lines: &[&str], start_idx: usize) -> i32 {
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
    // If no matching brace found, return start + 1
    (start_idx + 1) as i32
}

/// Compute end line for a statement (ends at semicolon or next line without continuation).
fn compute_statement_end(lines: &[&str], start_idx: usize) -> i32 {
    let mut depth: i32 = 0;
    for (j, &line) in lines.iter().enumerate().skip(start_idx) {
        for ch in line.chars() {
            match ch {
                '(' | '[' | '{' => depth += 1,
                ')' | ']' | '}' => depth -= 1,
                _ => {}
            }
        }
        // Statement ends when we're back at depth 0 and line doesn't end with continuation
        if depth <= 0 && j > start_idx {
            return (j + 1) as i32;
        }
        // Also end at semicolons when balanced
        if depth == 0 && line.contains(';') {
            return (j + 1) as i32;
        }
    }
    (start_idx + 1) as i32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fortran_extraction() {
        let source = r#"
program main
  implicit none
  call hello()
end program main

module math_utils
  implicit none
contains
  subroutine add(a, b, result)
    integer, intent(in) :: a, b
    integer, intent(out) :: result
    result = a + b
  end subroutine add

  recursive function factorial(n) result(res)
    integer, intent(in) :: n
    integer :: res
    if (n <= 1) then
      res = 1
    else
      res = n * factorial(n - 1)
    end if
  end function factorial
end module math_utils

type, public :: Point
  real :: x, y
end type Point
"#;
        let nodes = extract_tier3_programming(source, "math.f90", "proj", Language::Fortran);
        let names: Vec<&str> = nodes.iter().map(|n| n.name.as_str()).collect();
        assert!(names.contains(&"main"), "should find program main");
        assert!(
            names.contains(&"math_utils"),
            "should find module math_utils"
        );
        assert!(names.contains(&"add"), "should find subroutine add");
        assert!(
            names.contains(&"factorial"),
            "should find function factorial"
        );
        assert!(names.contains(&"Point"), "should find type Point");
    }

    #[test]
    fn test_cobol_extraction() {
        let source = r#"
       IDENTIFICATION DIVISION.
       PROGRAM-ID. HELLO-WORLD.
       DATA DIVISION.
       PROCEDURE DIVISION.
       MAIN-LOGIC SECTION.
       DISPLAY-HELLO.
           DISPLAY "Hello, World!".
           STOP RUN.
"#;
        let nodes = extract_tier3_programming(source, "hello.cbl", "proj", Language::Cobol);
        let names: Vec<&str> = nodes.iter().map(|n| n.name.as_str()).collect();
        assert!(names.contains(&"HELLO-WORLD"), "should find PROGRAM-ID");
        assert!(names.contains(&"MAIN-LOGIC"), "should find section");
    }

    #[test]
    fn test_ada_extraction() {
        let source = r#"
package body Math_Utils is
   type Vector is record
      X, Y : Float;
   end record;

   procedure Add(A, B : in Integer; Result : out Integer) is
   begin
      Result := A + B;
   end;

   function Factorial(N : Integer) return Integer is
   begin
      if N <= 1 then
         return 1;
      else
         return N * Factorial(N - 1);
      end if;
   end;
end Math_Utils;
"#;
        let nodes = extract_tier3_programming(source, "math.adb", "proj", Language::Ada);
        let names: Vec<&str> = nodes.iter().map(|n| n.name.as_str()).collect();
        assert!(names.contains(&"Math_Utils"), "should find package");
        assert!(names.contains(&"Vector"), "should find type");
        assert!(names.contains(&"Add"), "should find procedure");
        assert!(names.contains(&"Factorial"), "should find function");
    }

    #[test]
    fn test_pascal_extraction() {
        let source = r#"
unit MathUtils;

interface

type
  TPoint = class
    X, Y: Integer;
  end;

procedure Add(A, B: Integer; var Result: Integer);
function Factorial(N: Integer): Integer;

implementation

procedure Add(A, B: Integer; var Result: Integer);
begin
  Result := A + B;
end;

function Factorial(N: Integer): Integer;
begin
  if N <= 1 then
    Result := 1
  else
    Result := N * Factorial(N - 1);
end;

end.
"#;
        let nodes = extract_tier3_programming(source, "math.pas", "proj", Language::Pascal);
        let names: Vec<&str> = nodes.iter().map(|n| n.name.as_str()).collect();
        assert!(names.contains(&"MathUtils"), "should find unit");
        assert!(names.contains(&"TPoint"), "should find type");
        assert!(names.contains(&"Add"), "should find procedure");
        assert!(names.contains(&"Factorial"), "should find function");
    }

    #[test]
    fn test_odin_extraction() {
        let source = r#"
package main

Vector2 :: struct {
    x, y: f32,
}

Direction :: enum {
    North,
    South,
    East,
    West,
}

add :: proc(a, b: int) -> int {
    return a + b
}

main :: proc() {
    fmt.println("Hello, Odin!")
}
"#;
        let nodes = extract_tier3_programming(source, "main.odin", "proj", Language::Odin);
        let names: Vec<&str> = nodes.iter().map(|n| n.name.as_str()).collect();
        assert!(names.contains(&"main"), "should find package or proc main");
        assert!(names.contains(&"Vector2"), "should find struct");
        assert!(names.contains(&"Direction"), "should find enum");
        assert!(names.contains(&"add"), "should find proc");
    }

    #[test]
    fn test_crystal_extraction() {
        let source = r#"
module MathUtils
  abstract class Shape
    abstract def area : Float64
  end

  class Circle < Shape
    def initialize(@radius : Float64)
    end

    def area : Float64
      Math::PI * @radius ** 2
    end
  end

  enum Color
    Red
    Green
    Blue
  end

  def self.add(a : Int32, b : Int32) : Int32
    a + b
  end
end
"#;
        let nodes = extract_tier3_programming(source, "math.cr", "proj", Language::Crystal);
        let names: Vec<&str> = nodes.iter().map(|n| n.name.as_str()).collect();
        assert!(names.contains(&"MathUtils"), "should find module");
        assert!(names.contains(&"Shape"), "should find abstract class");
        assert!(names.contains(&"Circle"), "should find class");
        assert!(names.contains(&"Color"), "should find enum");
        assert!(names.contains(&"area"), "should find def");
    }

    #[test]
    fn test_gdscript_extraction() {
        let source = r#"
class_name Player
extends CharacterBody2D

signal health_changed(new_health)

enum State {
    IDLE,
    RUNNING,
    JUMPING,
}

func _ready():
    pass

func move(direction: Vector2) -> void:
    velocity = direction * speed
    move_and_slide()

static func create_player() -> Player:
    return Player.new()
"#;
        let nodes = extract_tier3_programming(source, "player.gd", "proj", Language::GDScript);
        let names: Vec<&str> = nodes.iter().map(|n| n.name.as_str()).collect();
        assert!(names.contains(&"Player"), "should find class_name");
        assert!(names.contains(&"health_changed"), "should find signal");
        assert!(names.contains(&"State"), "should find enum");
        assert!(names.contains(&"_ready"), "should find func");
        assert!(names.contains(&"move"), "should find func");
        assert!(names.contains(&"create_player"), "should find static func");
    }

    #[test]
    fn test_gleam_extraction() {
        let source = r#"
pub type User {
  User(name: String, age: Int)
}

pub opaque type Password {
  Password(hash: String)
}

type Internal {
  Internal(value: Int)
}

pub fn greet(user: User) -> String {
  string.concat(["Hello, ", user.name, "!"])
}

fn helper() -> Int {
  42
}
"#;
        let nodes = extract_tier3_programming(source, "app.gleam", "proj", Language::Gleam);
        let names: Vec<&str> = nodes.iter().map(|n| n.name.as_str()).collect();
        assert!(names.contains(&"User"), "should find pub type");
        assert!(names.contains(&"Password"), "should find pub opaque type");
        assert!(names.contains(&"Internal"), "should find type");
        assert!(names.contains(&"greet"), "should find pub fn");
        assert!(names.contains(&"helper"), "should find fn");
    }

    #[test]
    fn test_elm_extraction() {
        let source = r#"
module Main exposing (main)

type alias Model =
    { count : Int
    , name : String
    }

type Msg
    = Increment
    | Decrement
    | Reset

port sendMessage : String -> Cmd msg

update : Msg -> Model -> Model
update msg model =
    case msg of
        Increment ->
            { model | count = model.count + 1 }
        Decrement ->
            { model | count = model.count - 1 }
        Reset ->
            { model | count = 0 }

view : Model -> Html Msg
view model =
    div [] [ text (String.fromInt model.count) ]
"#;
        let nodes = extract_tier3_programming(source, "Main.elm", "proj", Language::Elm);
        let names: Vec<&str> = nodes.iter().map(|n| n.name.as_str()).collect();
        assert!(names.contains(&"Main"), "should find module");
        assert!(names.contains(&"Model"), "should find type alias");
        assert!(names.contains(&"Msg"), "should find type");
        assert!(names.contains(&"sendMessage"), "should find port");
        assert!(names.contains(&"update"), "should find function");
        assert!(names.contains(&"view"), "should find function");
    }

    #[test]
    fn test_nix_extraction() {
        let source = r#"
{ pkgs, lib, ... }:

let
  myPackage = pkgs.callPackage ./package.nix {};
in
{
  buildApp = { stdenv, fetchurl }:
    stdenv.mkDerivation {
      name = "my-app";
      src = fetchurl {
        url = "https://example.com/app.tar.gz";
      };
    };

  helper = x: x + 1;
}
"#;
        let nodes = extract_tier3_programming(source, "default.nix", "proj", Language::Nix);
        // Nix extraction should find at least some bindings
        assert!(
            !nodes.is_empty(),
            "should find at least one definition in Nix"
        );
        // Check that all nodes have valid properties
        for node in &nodes {
            assert!(!node.name.is_empty(), "node name should not be empty");
            assert!(node.start_line > 0, "start_line should be > 0");
            assert!(
                node.start_line <= node.end_line,
                "start_line should be <= end_line"
            );
        }
    }

    #[test]
    fn test_empty_file_produces_no_nodes() {
        let source = "";
        let nodes = extract_tier3_programming(source, "empty.f90", "proj", Language::Fortran);
        assert!(nodes.is_empty(), "empty file should produce no nodes");
    }

    #[test]
    fn test_no_definitions_produces_no_nodes() {
        let source = "! This is just a comment\n! No definitions here\n";
        let nodes = extract_tier3_programming(source, "comments.f90", "proj", Language::Fortran);
        assert!(
            nodes.is_empty(),
            "file with only comments should produce no nodes"
        );
    }

    #[test]
    fn test_unsupported_language_produces_no_nodes() {
        let source = "fn main() { println!(\"hello\"); }";
        let nodes = extract_tier3_programming(source, "main.rs", "proj", Language::Rust);
        assert!(
            nodes.is_empty(),
            "unsupported language should produce no nodes"
        );
    }

    #[test]
    fn test_node_properties_are_valid() {
        let source = "pub fn greet(name: String) -> String {\n  \"Hello\"\n}\n";
        let nodes = extract_tier3_programming(source, "app.gleam", "proj", Language::Gleam);
        for node in &nodes {
            assert!(!node.name.is_empty(), "name should not be empty");
            assert!(!node.label.is_empty(), "label should not be empty");
            assert!(
                node.start_line > 0,
                "start_line should be positive: got {}",
                node.start_line
            );
            assert!(
                node.start_line <= node.end_line,
                "start_line ({}) should be <= end_line ({})",
                node.start_line,
                node.end_line
            );
            assert!(
                !node.qualified_name.is_empty(),
                "qualified_name should not be empty"
            );
            assert_eq!(node.file_path, "app.gleam");
        }
    }

    // ── Build/Infra extractor tests ──────────────────────────────────────────

    #[test]
    fn test_dockerfile_extraction_named_stages() {
        let source = r#"
FROM rust:1.75 AS builder
RUN cargo build --release

FROM debian:bookworm-slim AS runtime
COPY --from=builder /app/target/release/myapp /usr/local/bin/
CMD ["myapp"]
"#;
        let nodes = extract_build_infra(source, "Dockerfile", "proj", "dockerfile");
        let names: Vec<&str> = nodes.iter().map(|n| n.name.as_str()).collect();
        assert_eq!(names.len(), 2, "should find 2 FROM stages");
        assert!(names.contains(&"builder"), "should find stage 'builder'");
        assert!(names.contains(&"runtime"), "should find stage 'runtime'");

        // Verify line numbers
        let builder = nodes.iter().find(|n| n.name == "builder").unwrap();
        assert_eq!(builder.start_line, 2);
        assert_eq!(builder.label, "Function");
    }

    #[test]
    fn test_dockerfile_extraction_unnamed_stages() {
        let source = r#"FROM node:18-alpine
RUN npm install
COPY . .

FROM nginx:latest
COPY --from=0 /app/dist /usr/share/nginx/html
"#;
        let nodes = extract_build_infra(source, "Dockerfile", "proj", "dockerfile");
        let names: Vec<&str> = nodes.iter().map(|n| n.name.as_str()).collect();
        assert_eq!(names.len(), 2, "should find 2 FROM stages");
        assert!(
            names.contains(&"node"),
            "should find image 'node' (without tag)"
        );
        assert!(
            names.contains(&"nginx"),
            "should find image 'nginx' (without tag)"
        );
    }

    #[test]
    fn test_dockerfile_extraction_with_digest() {
        let source = "FROM ubuntu@sha256:abc123\nRUN apt-get update\n";
        let nodes = extract_build_infra(source, "Dockerfile.prod", "proj", "dockerfile");
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].name, "ubuntu");
    }

    #[test]
    fn test_makefile_extraction() {
        let source = r#"
CC = gcc
CFLAGS = -Wall -O2

all: build test
	@echo "Done"

build:
	$(CC) $(CFLAGS) -o app main.c

test: build
	./run_tests.sh

clean:
	rm -f app *.o

install: build
	cp app /usr/local/bin/
"#;
        let nodes = extract_build_infra(source, "Makefile", "proj", "makefile");
        let names: Vec<&str> = nodes.iter().map(|n| n.name.as_str()).collect();
        assert!(names.contains(&"all"), "should find target 'all'");
        assert!(names.contains(&"build"), "should find target 'build'");
        assert!(names.contains(&"test"), "should find target 'test'");
        assert!(names.contains(&"clean"), "should find target 'clean'");
        assert!(names.contains(&"install"), "should find target 'install'");
        assert_eq!(names.len(), 5, "should find exactly 5 targets");

        // Verify all nodes have Function label
        for node in &nodes {
            assert_eq!(node.label, "Function");
        }
    }

    #[test]
    fn test_makefile_skips_phony_and_internal() {
        let source = ".PHONY: all clean\n.SUFFIXES:\n\nall:\n\t@echo hi\n";
        let nodes = extract_build_infra(source, "Makefile", "proj", "makefile");
        let names: Vec<&str> = nodes.iter().map(|n| n.name.as_str()).collect();
        assert!(names.contains(&"all"), "should find target 'all'");
        assert!(!names.contains(&".PHONY"), "should skip .PHONY");
        assert!(!names.contains(&".SUFFIXES"), "should skip .SUFFIXES");
    }

    #[test]
    fn test_cmake_extraction() {
        let source = r#"
cmake_minimum_required(VERSION 3.20)
project(MyApp VERSION 1.0.0 LANGUAGES CXX)

function(setup_compiler_flags target)
  target_compile_options(${target} PRIVATE -Wall -Wextra)
endfunction()

macro(add_test_suite name)
  add_executable(${name}_test ${ARGN})
  target_link_libraries(${name}_test PRIVATE GTest::gtest_main)
  add_test(NAME ${name} COMMAND ${name}_test)
endmacro()

add_executable(myapp src/main.cpp)
"#;
        let nodes = extract_build_infra(source, "CMakeLists.txt", "proj", "cmake");
        let names: Vec<&str> = nodes.iter().map(|n| n.name.as_str()).collect();
        assert!(names.contains(&"MyApp"), "should find project 'MyApp'");
        assert!(
            names.contains(&"setup_compiler_flags"),
            "should find function 'setup_compiler_flags'"
        );
        assert!(
            names.contains(&"add_test_suite"),
            "should find macro 'add_test_suite'"
        );
        assert_eq!(names.len(), 3, "should find exactly 3 definitions");

        // Verify labels
        let project_node = nodes.iter().find(|n| n.name == "MyApp").unwrap();
        assert_eq!(project_node.label, "Module");

        let func_node = nodes
            .iter()
            .find(|n| n.name == "setup_compiler_flags")
            .unwrap();
        assert_eq!(func_node.label, "Function");
        // Function should span from function() to endfunction()
        assert!(
            func_node.end_line > func_node.start_line,
            "function should span multiple lines"
        );
    }

    #[test]
    fn test_cmake_end_line_computation() {
        let source =
            "function(my_func arg1 arg2)\n  message(STATUS \"hello\")\n  set(X 1)\nendfunction()\n";
        let nodes = extract_build_infra(source, "CMakeLists.txt", "proj", "cmake");
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].name, "my_func");
        assert_eq!(nodes[0].start_line, 1);
        assert_eq!(nodes[0].end_line, 4); // endfunction() is on line 4
    }

    #[test]
    fn test_build_infra_empty_file() {
        let nodes = extract_build_infra("", "Dockerfile", "proj", "dockerfile");
        assert!(nodes.is_empty(), "empty Dockerfile should produce no nodes");

        let nodes = extract_build_infra("", "Makefile", "proj", "makefile");
        assert!(nodes.is_empty(), "empty Makefile should produce no nodes");

        let nodes = extract_build_infra("", "CMakeLists.txt", "proj", "cmake");
        assert!(nodes.is_empty(), "empty CMake file should produce no nodes");
    }

    #[test]
    fn test_build_infra_unsupported_ext() {
        let nodes = extract_build_infra("some content", "file.txt", "proj", "txt");
        assert!(
            nodes.is_empty(),
            "unsupported extension should produce no nodes"
        );
    }

    #[test]
    fn test_build_infra_file_path_detection() {
        // Should detect Dockerfile by file path even with different ext
        let source = "FROM alpine:3.18\nRUN echo hello\n";
        let nodes = extract_build_infra(source, "docker/Dockerfile.prod", "proj", "");
        assert_eq!(nodes.len(), 1, "should detect Dockerfile by path");
        assert_eq!(nodes[0].name, "alpine");

        // Should detect Makefile by path
        let source = "build:\n\techo building\n";
        let nodes = extract_build_infra(source, "src/Makefile", "proj", "");
        assert_eq!(nodes.len(), 1, "should detect Makefile by path");

        // Should detect CMake by path
        let source = "project(Foo)\n";
        let nodes = extract_build_infra(source, "build/CMakeLists.txt", "proj", "");
        assert_eq!(nodes.len(), 1, "should detect CMakeLists.txt by path");
    }

    #[test]
    fn test_build_infra_node_properties_valid() {
        let source = "FROM rust:1.75 AS builder\nRUN cargo build\n";
        let nodes = extract_build_infra(source, "Dockerfile", "proj", "dockerfile");
        for node in &nodes {
            assert!(!node.name.is_empty(), "name should not be empty");
            assert!(!node.label.is_empty(), "label should not be empty");
            assert!(node.start_line > 0, "start_line should be > 0");
            assert!(node.start_line <= node.end_line, "start_line <= end_line");
            assert!(!node.qualified_name.is_empty(), "qn should not be empty");
            assert_eq!(node.file_path, "Dockerfile");
        }
    }

    // ── Markup extractor tests ───────────────────────────────────────────────

    #[test]
    fn test_markdown_heading_extraction() {
        let source = "# Introduction\n\nSome text here.\n\n## Getting Started\n\nMore text.\n\n### Installation\n\nInstall steps.\n\n## Usage\n\nUsage info.\n";
        let nodes = extract_markup(source, "README.md", "proj", Language::Markdown);
        let names: Vec<&str> = nodes.iter().map(|n| n.name.as_str()).collect();
        assert!(names.contains(&"Introduction"), "should find h1");
        assert!(names.contains(&"Getting Started"), "should find h2");
        assert!(names.contains(&"Installation"), "should find h3");
        assert!(names.contains(&"Usage"), "should find second h2");
        assert_eq!(nodes.len(), 4, "should find exactly 4 headings");

        // H1 should be labeled Module
        let intro = nodes.iter().find(|n| n.name == "Introduction").unwrap();
        assert_eq!(intro.label, "Module");

        // H2/H3 should be labeled Function
        let getting_started = nodes.iter().find(|n| n.name == "Getting Started").unwrap();
        assert_eq!(getting_started.label, "Function");
    }

    #[test]
    fn test_markdown_skips_h4_and_deeper() {
        let source = "# Title\n\n#### Deep heading\n\n##### Even deeper\n";
        let nodes = extract_markup(source, "doc.md", "proj", Language::Markdown);
        assert_eq!(nodes.len(), 1, "should only find h1, skip h4+");
        assert_eq!(nodes[0].name, "Title");
    }

    #[test]
    fn test_markdown_end_lines() {
        let source =
            "# Section 1\n\nContent line 1\nContent line 2\n\n# Section 2\n\nMore content\n";
        let nodes = extract_markup(source, "doc.md", "proj", Language::Markdown);
        assert_eq!(nodes.len(), 2);
        // Section 1 should end before Section 2 starts
        let s1 = &nodes[0];
        assert_eq!(s1.start_line, 1);
        assert!(s1.end_line >= 4, "Section 1 should span its content");
        assert!(s1.end_line < 6, "Section 1 should end before Section 2");
    }

    #[test]
    fn test_yaml_top_level_keys() {
        let source = "---\nname: my-app\nversion: 1.0.0\ndependencies:\n  express: ^4.0.0\n  lodash: ^4.17.0\nscripts:\n  start: node index.js\n";
        let nodes = extract_markup(source, "package.yaml", "proj", Language::Yaml);
        let names: Vec<&str> = nodes.iter().map(|n| n.name.as_str()).collect();
        assert!(names.contains(&"name"), "should find 'name' key");
        assert!(names.contains(&"version"), "should find 'version' key");
        assert!(
            names.contains(&"dependencies"),
            "should find 'dependencies' key"
        );
        assert!(names.contains(&"scripts"), "should find 'scripts' key");
        assert_eq!(nodes.len(), 4, "should find exactly 4 top-level keys");
    }

    #[test]
    fn test_yaml_skips_nested_keys() {
        let source =
            "server:\n  host: localhost\n  port: 8080\ndatabase:\n  url: postgres://localhost\n";
        let nodes = extract_markup(source, "config.yml", "proj", Language::Yaml);
        let names: Vec<&str> = nodes.iter().map(|n| n.name.as_str()).collect();
        assert!(names.contains(&"server"), "should find 'server'");
        assert!(names.contains(&"database"), "should find 'database'");
        assert!(!names.contains(&"host"), "should NOT find nested 'host'");
        assert!(!names.contains(&"port"), "should NOT find nested 'port'");
        assert_eq!(nodes.len(), 2);
    }

    #[test]
    fn test_yaml_skips_comments_and_separators() {
        let source =
            "---\n# This is a comment\nkey1: value1\n# Another comment\nkey2: value2\n...\n";
        let nodes = extract_markup(source, "data.yml", "proj", Language::Yaml);
        let names: Vec<&str> = nodes.iter().map(|n| n.name.as_str()).collect();
        assert_eq!(nodes.len(), 2);
        assert!(names.contains(&"key1"));
        assert!(names.contains(&"key2"));
    }

    #[test]
    fn test_json_top_level_keys() {
        let source = "{\n  \"name\": \"my-app\",\n  \"version\": \"1.0.0\",\n  \"dependencies\": {\n    \"express\": \"^4.0.0\"\n  },\n  \"scripts\": {\n    \"start\": \"node index.js\"\n  }\n}\n";
        let nodes = extract_markup(source, "package.json", "proj", Language::Json);
        let names: Vec<&str> = nodes.iter().map(|n| n.name.as_str()).collect();
        assert!(names.contains(&"name"), "should find 'name'");
        assert!(names.contains(&"version"), "should find 'version'");
        assert!(
            names.contains(&"dependencies"),
            "should find 'dependencies'"
        );
        assert!(names.contains(&"scripts"), "should find 'scripts'");
        assert_eq!(nodes.len(), 4, "should find exactly 4 top-level keys");
    }

    #[test]
    fn test_json_skips_nested_keys() {
        let source = "{\n  \"outer\": {\n    \"inner\": \"value\"\n  }\n}\n";
        let nodes = extract_markup(source, "data.json", "proj", Language::Json);
        let names: Vec<&str> = nodes.iter().map(|n| n.name.as_str()).collect();
        assert!(names.contains(&"outer"), "should find 'outer'");
        assert!(!names.contains(&"inner"), "should NOT find nested 'inner'");
        assert_eq!(nodes.len(), 1);
    }

    #[test]
    fn test_html_id_extraction() {
        let source = "<html>\n<body>\n  <div id=\"main-content\">\n    <h1 id=\"title\">Hello</h1>\n    <section id=\"features\">\n      <p>Content</p>\n    </section>\n  </div>\n</body>\n</html>\n";
        let nodes = extract_markup(source, "index.html", "proj", Language::Html);
        let names: Vec<&str> = nodes.iter().map(|n| n.name.as_str()).collect();
        assert!(
            names.contains(&"main-content"),
            "should find id='main-content'"
        );
        assert!(names.contains(&"title"), "should find id='title'");
        assert!(names.contains(&"features"), "should find id='features'");
        assert_eq!(nodes.len(), 3);
    }

    #[test]
    fn test_html_multiple_ids_on_same_line() {
        let source = "<div id=\"a\"><span id=\"b\">text</span></div>\n";
        let nodes = extract_markup(source, "page.html", "proj", Language::Html);
        let names: Vec<&str> = nodes.iter().map(|n| n.name.as_str()).collect();
        assert!(names.contains(&"a"));
        assert!(names.contains(&"b"));
        assert_eq!(nodes.len(), 2);
    }

    #[test]
    fn test_css_selector_extraction() {
        let source = ".container {\n  width: 100%;\n}\n\n#header {\n  background: blue;\n}\n\n.btn-primary {\n  color: white;\n}\n";
        let nodes = extract_markup(source, "styles.css", "proj", Language::Css);
        let names: Vec<&str> = nodes.iter().map(|n| n.name.as_str()).collect();
        assert!(names.contains(&".container"), "should find .container");
        assert!(names.contains(&"#header"), "should find #header");
        assert!(names.contains(&".btn-primary"), "should find .btn-primary");
        assert_eq!(nodes.len(), 3);
    }

    #[test]
    fn test_scss_selector_extraction() {
        let source = ".wrapper {\n  padding: 16px;\n}\n\n#sidebar {\n  width: 250px;\n}\n";
        let nodes = extract_markup(source, "app.scss", "proj", Language::Scss);
        let names: Vec<&str> = nodes.iter().map(|n| n.name.as_str()).collect();
        assert!(names.contains(&".wrapper"), "should find .wrapper");
        assert!(names.contains(&"#sidebar"), "should find #sidebar");
        assert_eq!(nodes.len(), 2);
    }

    #[test]
    fn test_css_skips_nested_selectors() {
        let source = ".parent {\n  color: red;\n  .child {\n    color: blue;\n  }\n}\n";
        let nodes = extract_markup(source, "nested.scss", "proj", Language::Scss);
        // Only the top-level .parent should be extracted
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].name, ".parent");
    }

    #[test]
    fn test_css_end_lines() {
        let source =
            ".first {\n  color: red;\n  font-size: 14px;\n}\n\n.second {\n  color: blue;\n}\n";
        let nodes = extract_markup(source, "styles.css", "proj", Language::Css);
        assert_eq!(nodes.len(), 2);
        let first = &nodes[0];
        assert_eq!(first.start_line, 1);
        assert_eq!(first.end_line, 4); // closing brace on line 4
    }

    #[test]
    fn test_markup_empty_file() {
        let source = "";
        let nodes = extract_markup(source, "empty.md", "proj", Language::Markdown);
        assert!(nodes.is_empty());
    }

    #[test]
    fn test_markup_unsupported_language() {
        let source = "fn main() {}";
        let nodes = extract_markup(source, "main.rs", "proj", Language::Rust);
        assert!(nodes.is_empty());
    }

    #[test]
    fn test_markup_node_properties_valid() {
        let source = "# Title\n\nContent\n\n## Section\n\nMore content\n";
        let nodes = extract_markup(source, "doc.md", "proj", Language::Markdown);
        for node in &nodes {
            assert!(!node.name.is_empty(), "name should not be empty");
            assert!(!node.label.is_empty(), "label should not be empty");
            assert!(node.start_line > 0, "start_line should be > 0");
            assert!(
                node.start_line <= node.end_line,
                "start_line ({}) <= end_line ({})",
                node.start_line,
                node.end_line
            );
            assert!(!node.qualified_name.is_empty());
            assert_eq!(node.file_path, "doc.md");
        }
    }

    // ── SFC extractor tests ──────────────────────────────────────────────────

    #[test]
    fn test_svelte_extraction() {
        let source = r#"<script>
  export let count = 0;
  export let name;

  function increment() {
    count += 1;
  }

  const reset = () => {
    count = 0;
  };
</script>

<button on:click={increment}>
  {count}
</button>

<ChildComponent />
<my-widget />
"#;
        let nodes = extract_sfc(source, "src/Counter.svelte", "proj", "svelte");
        let names: Vec<&str> = nodes.iter().map(|n| n.name.as_str()).collect();

        // Component name from filename
        assert!(
            names.contains(&"Counter"),
            "should extract component name from filename"
        );

        // Exported variables
        assert!(names.contains(&"count"), "should find exported let count");
        assert!(names.contains(&"name"), "should find exported let name");

        // Functions
        assert!(
            names.contains(&"increment"),
            "should find function increment"
        );
        assert!(
            names.contains(&"reset"),
            "should find const arrow function reset"
        );

        // Template component references
        assert!(
            names.contains(&"ChildComponent"),
            "should find PascalCase component tag"
        );
        assert!(
            names.contains(&"my-widget"),
            "should find kebab-case component tag"
        );
    }

    #[test]
    fn test_vue_script_setup_extraction() {
        let source = r#"<template>
  <div>
    <h1>{{ title }}</h1>
    <UserCard />
    <app-footer />
  </div>
</template>

<script setup lang="ts">
import { ref } from 'vue'

const title = ref('Hello')
const count = ref(0)

function handleClick() {
  count.value++
}

defineProps<{
  msg: string
}>()

defineEmits(['update'])
</script>

<style scoped>
.title { color: red; }
</style>
"#;
        let nodes = extract_sfc(source, "src/components/MyPage.vue", "proj", "vue");
        let names: Vec<&str> = nodes.iter().map(|n| n.name.as_str()).collect();

        // Component name from filename
        assert!(
            names.contains(&"MyPage"),
            "should extract component name from filename"
        );

        // Script setup variables
        assert!(names.contains(&"title"), "should find const title");
        assert!(names.contains(&"count"), "should find const count");

        // Functions
        assert!(
            names.contains(&"handleClick"),
            "should find function handleClick"
        );

        // Define macros
        assert!(names.contains(&"defineProps"), "should find defineProps");
        assert!(names.contains(&"defineEmits"), "should find defineEmits");

        // Template component references
        assert!(
            names.contains(&"UserCard"),
            "should find PascalCase component tag"
        );
        assert!(
            names.contains(&"app-footer"),
            "should find kebab-case component tag"
        );
    }

    #[test]
    fn test_vue_options_api_extraction() {
        let source = r#"<template>
  <div>
    <HeaderNav />
  </div>
</template>

<script>
export default {
  name: 'AppLayout',
  methods: {
    fetchData() {
      return fetch('/api/data')
    }
  }
}
</script>
"#;
        let nodes = extract_sfc(source, "src/AppLayout.vue", "proj", "vue");
        let names: Vec<&str> = nodes.iter().map(|n| n.name.as_str()).collect();

        // Explicit name from options
        assert!(
            names.contains(&"AppLayout"),
            "should find explicit component name"
        );

        // Template component references
        assert!(
            names.contains(&"HeaderNav"),
            "should find HeaderNav component tag"
        );
    }

    #[test]
    fn test_sfc_empty_file() {
        let source = "";
        let nodes = extract_sfc(source, "Empty.svelte", "proj", "svelte");
        // Should still produce the component name node
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].name, "Empty");
    }

    #[test]
    fn test_sfc_unsupported_ext() {
        let source = "<div>hello</div>";
        let nodes = extract_sfc(source, "file.html", "proj", "html");
        assert!(nodes.is_empty(), "unsupported ext should produce no nodes");
    }

    #[test]
    fn test_sfc_node_properties_valid() {
        let source = "<script>\nexport let value = 0;\n</script>\n<p>{value}</p>\n";
        let nodes = extract_sfc(source, "src/Widget.svelte", "proj", "svelte");
        for node in &nodes {
            assert!(!node.name.is_empty(), "name should not be empty");
            assert!(!node.label.is_empty(), "label should not be empty");
            assert!(node.start_line > 0, "start_line should be > 0");
            assert!(
                node.start_line <= node.end_line,
                "start_line ({}) <= end_line ({})",
                node.start_line,
                node.end_line
            );
            assert!(!node.qualified_name.is_empty(), "qn should not be empty");
            assert_eq!(node.file_path, "src/Widget.svelte");
        }
    }

    #[test]
    fn test_svelte_no_script_block() {
        let source = "<h1>Hello World</h1>\n<MyComponent />\n";
        let nodes = extract_sfc(source, "src/Hello.svelte", "proj", "svelte");
        let names: Vec<&str> = nodes.iter().map(|n| n.name.as_str()).collect();
        assert!(names.contains(&"Hello"), "should extract component name");
        assert!(
            names.contains(&"MyComponent"),
            "should find template component tag"
        );
    }
}
