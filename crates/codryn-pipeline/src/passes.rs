use anyhow::Result;
use codryn_discover::{DiscoveredFile, Language};
use codryn_foundation::fqn;
use codryn_graph_buffer::GraphBuffer;
use codryn_store::{Node, Store};
use rayon::prelude::*;
use std::collections::HashMap;
use std::collections::HashSet;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::LazyLock;

use crate::registry::Registry;

/// Global cancellation token for parallel pass execution.
/// Set by the pipeline before running passes, checked by par_iter closures.
static PASS_CANCELLED: AtomicBool = AtomicBool::new(false);

/// Set the cancellation flag for parallel passes.
pub fn set_pass_cancelled(cancelled: bool) {
    PASS_CANCELLED.store(cancelled, Ordering::Relaxed);
}

/// Check if parallel passes should stop.
fn is_pass_cancelled() -> bool {
    PASS_CANCELLED.load(Ordering::Relaxed)
}

static CALL_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"\b(\w+)\s*\(").unwrap());

/// Mapping from bare specifier (e.g., "@myorg/pkg") to resolved module QN.
pub type PackageMap = HashMap<String, String>;

/// Parse all manifest files in the discovered file list and build a PackageMap.
pub fn pass_pkgmap(files: &[&DiscoveredFile], project: &str) -> PackageMap {
    let mut map = PackageMap::new();
    for f in files {
        let filename = f.rel_path.rsplit('/').next().unwrap_or(&f.rel_path);
        match filename {
            "package.json" => parse_package_json(&f.abs_path, project, &mut map),
            "go.mod" => parse_go_mod(&f.abs_path, project, &mut map),
            "Cargo.toml" => parse_cargo_toml(&f.abs_path, project, &mut map),
            "pyproject.toml" => parse_pyproject_toml(&f.abs_path, project, &mut map),
            "composer.json" => parse_composer_json(&f.abs_path, project, &mut map),
            "pubspec.yaml" => parse_pubspec_yaml(&f.abs_path, project, &mut map),
            "pom.xml" => parse_pom_xml(&f.abs_path, project, &mut map),
            "build.gradle" | "build.gradle.kts" => {
                parse_build_gradle(&f.abs_path, project, &mut map)
            }
            "mix.exs" => parse_mix_exs(&f.abs_path, project, &mut map),
            "setup.py" => parse_setup_py(&f.abs_path, project, &mut map),
            fname if fname.ends_with(".gemspec") => parse_gemspec(&f.abs_path, project, &mut map),
            _ => {}
        }
    }
    map
}

/// Parse package.json — extract dependencies, devDependencies, peerDependencies.
/// Maps each package name to `{project}.node_modules.{pkg_name}`.
fn parse_package_json(path: &Path, project: &str, map: &mut PackageMap) {
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) => {
            tracing::debug!(error = %e, path = %path.display(), "pass_pkgmap: failed to read package.json");
            return;
        }
    };
    let json: serde_json::Value = match serde_json::from_str(&content) {
        Ok(v) => v,
        Err(e) => {
            tracing::debug!(error = %e, path = %path.display(), "pass_pkgmap: failed to parse package.json");
            return;
        }
    };

    for section in &["dependencies", "devDependencies", "peerDependencies"] {
        if let Some(deps) = json.get(section).and_then(|v| v.as_object()) {
            for pkg_name in deps.keys() {
                let qn = format!("{}.node_modules.{}", project, pkg_name);
                map.insert(pkg_name.clone(), qn);
            }
        }
    }
}

/// Parse go.mod — extract `require` directives.
/// Maps each module path to `{project}.vendor.{module_path}`.
fn parse_go_mod(path: &Path, project: &str, map: &mut PackageMap) {
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) => {
            tracing::debug!(error = %e, path = %path.display(), "pass_pkgmap: failed to read go.mod");
            return;
        }
    };

    let mut in_require_block = false;
    for line in content.lines() {
        let trimmed = line.trim();

        if trimmed.starts_with("require (") || trimmed == "require (" {
            in_require_block = true;
            continue;
        }
        if in_require_block && trimmed == ")" {
            in_require_block = false;
            continue;
        }

        // Single-line require: `require github.com/foo/bar v1.2.3`
        if trimmed.starts_with("require ") && !trimmed.contains('(') {
            if let Some(module_path) = trimmed
                .strip_prefix("require ")
                .and_then(|rest| rest.split_whitespace().next())
            {
                let qn = format!("{}.vendor.{}", project, module_path);
                map.insert(module_path.to_string(), qn);
            }
            continue;
        }

        // Inside require block: `github.com/foo/bar v1.2.3`
        if in_require_block {
            if trimmed.starts_with("//") || trimmed.is_empty() {
                continue;
            }
            // Strip inline comments: `github.com/foo/bar v1.2.3 // indirect`
            let without_comment = trimmed.split("//").next().unwrap_or(trimmed).trim();
            if without_comment.is_empty() {
                continue;
            }
            if let Some(module_path) = without_comment.split_whitespace().next() {
                if module_path.contains('/') || module_path.contains('.') {
                    let qn = format!("{}.vendor.{}", project, module_path);
                    map.insert(module_path.to_string(), qn);
                }
            }
        }
    }
}

/// Parse Cargo.toml — extract [dependencies], [dev-dependencies], [build-dependencies].
/// Maps each crate name to `{project}.deps.{crate_name}`.
fn parse_cargo_toml(path: &Path, project: &str, map: &mut PackageMap) {
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) => {
            tracing::debug!(error = %e, path = %path.display(), "pass_pkgmap: failed to read Cargo.toml");
            return;
        }
    };
    let parsed: toml::Value = match content.parse() {
        Ok(v) => v,
        Err(e) => {
            tracing::debug!(error = %e, path = %path.display(), "pass_pkgmap: failed to parse Cargo.toml");
            return;
        }
    };

    for section in &["dependencies", "dev-dependencies", "build-dependencies"] {
        if let Some(deps) = parsed.get(section).and_then(|v| v.as_table()) {
            for crate_name in deps.keys() {
                let qn = format!("{}.deps.{}", project, crate_name);
                map.insert(crate_name.clone(), qn);
            }
        }
    }
}

/// Parse pyproject.toml — extract [project.dependencies] and [tool.poetry.dependencies].
/// Maps each package name to `{project}.site_packages.{pkg_name}`.
fn parse_pyproject_toml(path: &Path, project: &str, map: &mut PackageMap) {
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) => {
            tracing::debug!(error = %e, path = %path.display(), "pass_pkgmap: failed to read pyproject.toml");
            return;
        }
    };
    let parsed: toml::Value = match content.parse() {
        Ok(v) => v,
        Err(e) => {
            tracing::debug!(error = %e, path = %path.display(), "pass_pkgmap: failed to parse pyproject.toml");
            return;
        }
    };

    // [project.dependencies] — PEP 621 format: list of strings like "requests>=2.0"
    if let Some(deps) = parsed
        .get("project")
        .and_then(|p| p.get("dependencies"))
        .and_then(|d| d.as_array())
    {
        for dep in deps {
            if let Some(dep_str) = dep.as_str() {
                let pkg_name = extract_pep508_name(dep_str);
                if !pkg_name.is_empty() {
                    let qn = format!("{}.site_packages.{}", project, pkg_name);
                    map.insert(pkg_name.to_string(), qn);
                }
            }
        }
    }

    // [tool.poetry.dependencies] — table of package_name = version_spec
    if let Some(deps) = parsed
        .get("tool")
        .and_then(|t| t.get("poetry"))
        .and_then(|p| p.get("dependencies"))
        .and_then(|d| d.as_table())
    {
        for pkg_name in deps.keys() {
            // Skip python itself
            if pkg_name == "python" {
                continue;
            }
            let qn = format!("{}.site_packages.{}", project, pkg_name);
            map.insert(pkg_name.clone(), qn);
        }
    }
}

/// Extract the package name from a PEP 508 dependency string.
/// E.g., "requests>=2.0,<3" → "requests", "numpy[extra]" → "numpy"
fn extract_pep508_name(dep: &str) -> &str {
    let dep = dep.trim();
    // Package name ends at first version specifier, whitespace, semicolon, or bracket
    let end = dep
        .find(|c: char| {
            c == '>' || c == '<' || c == '=' || c == '!' || c == ';' || c == '[' || c == ' '
        })
        .unwrap_or(dep.len());
    &dep[..end]
}

// ── Additional manifest parsers ───────────────────────

/// Regex for pom.xml `<dependency>` blocks: extract groupId and artifactId.
static POM_DEP_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(
        r"(?s)<dependency>.*?<groupId>\s*([^<]+?)\s*</groupId>.*?<artifactId>\s*([^<]+?)\s*</artifactId>.*?</dependency>"
    ).unwrap()
});

/// Regex for build.gradle dependency declarations.
static GRADLE_DEP_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(
        r#"(?:implementation|api|compile|testImplementation|runtimeOnly|compileOnly)\s*[\('"]\s*([^'")]+)"#
    ).unwrap()
});

/// Regex for mix.exs `{:dep_name, ...}` tuples in the deps list.
static MIX_DEP_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"\{:(\w+),").unwrap());

/// Regex for gemspec `add_dependency` / `add_runtime_dependency` / `add_development_dependency`.
static GEMSPEC_DEP_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r#"add_(?:runtime_|development_)?dependency\s*[\(]\s*['"]([^'"]+)['"]"#)
        .unwrap()
});

/// Regex for setup.py `install_requires` list entries.
static SETUP_PY_DEP_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r#"['"]([A-Za-z0-9_][A-Za-z0-9_.\-]*)"#).unwrap());

/// Parse composer.json (PHP) — extract `require` and `require-dev` dependencies.
/// Maps each package name to `{project}.vendor.{pkg}`.
fn parse_composer_json(path: &Path, project: &str, map: &mut PackageMap) {
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) => {
            tracing::debug!(error = %e, path = %path.display(), "pass_pkgmap: failed to read composer.json");
            return;
        }
    };
    let json: serde_json::Value = match serde_json::from_str(&content) {
        Ok(v) => v,
        Err(e) => {
            tracing::debug!(error = %e, path = %path.display(), "pass_pkgmap: failed to parse composer.json");
            return;
        }
    };

    for section in &["require", "require-dev"] {
        if let Some(deps) = json.get(section).and_then(|v| v.as_object()) {
            for pkg_name in deps.keys() {
                // Skip php itself and ext-* extensions
                if pkg_name == "php" || pkg_name.starts_with("ext-") {
                    continue;
                }
                let qn = format!("{}.vendor.{}", project, pkg_name);
                map.insert(pkg_name.clone(), qn);
            }
        }
    }
}

/// Parse pubspec.yaml (Dart) — extract `dependencies` and `dev_dependencies`.
/// Maps each package name to `{project}.packages.{pkg}`.
/// Uses simple line-based parsing to avoid a YAML dependency.
fn parse_pubspec_yaml(path: &Path, project: &str, map: &mut PackageMap) {
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) => {
            tracing::debug!(error = %e, path = %path.display(), "pass_pkgmap: failed to read pubspec.yaml");
            return;
        }
    };

    let mut in_deps_section = false;
    let mut section_indent: Option<usize> = None;

    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }

        // Detect top-level section headers
        if !line.starts_with(' ') && !line.starts_with('\t') {
            in_deps_section = trimmed == "dependencies:" || trimmed == "dev_dependencies:";
            section_indent = None;
            continue;
        }

        if in_deps_section {
            let indent = line.len() - line.trim_start().len();
            // Set the expected indent for dependency entries
            if section_indent.is_none() {
                section_indent = Some(indent);
            }
            // If indent is less than or equal to 0, we've left the section
            if indent == 0 {
                in_deps_section = false;
                continue;
            }
            // Only process lines at the expected indent level (direct children)
            if Some(indent) == section_indent {
                // Extract package name from "pkg_name: version" or "pkg_name:"
                if let Some(pkg_name) = trimmed.split(':').next() {
                    let pkg_name = pkg_name.trim();
                    if !pkg_name.is_empty() && pkg_name != "flutter" && pkg_name != "flutter_test" {
                        let qn = format!("{}.packages.{}", project, pkg_name);
                        map.insert(pkg_name.to_string(), qn);
                    }
                }
            }
        }
    }
}

/// Parse pom.xml (Java/Maven) — extract `<dependency>` blocks.
/// Maps each dependency to `{project}.deps.{groupId}.{artifactId}`.
fn parse_pom_xml(path: &Path, project: &str, map: &mut PackageMap) {
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) => {
            tracing::debug!(error = %e, path = %path.display(), "pass_pkgmap: failed to read pom.xml");
            return;
        }
    };

    for caps in POM_DEP_RE.captures_iter(&content) {
        let group_id = caps.get(1).map(|m| m.as_str().trim()).unwrap_or("");
        let artifact_id = caps.get(2).map(|m| m.as_str().trim()).unwrap_or("");
        if !group_id.is_empty() && !artifact_id.is_empty() {
            let key = format!("{}:{}", group_id, artifact_id);
            let qn = format!("{}.deps.{}.{}", project, group_id, artifact_id);
            map.insert(key, qn);
        }
    }
}

/// Parse build.gradle / build.gradle.kts (Java/Gradle) — extract dependency declarations.
/// Maps each dependency to `{project}.deps.{dep}`.
fn parse_build_gradle(path: &Path, project: &str, map: &mut PackageMap) {
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) => {
            tracing::debug!(error = %e, path = %path.display(), "pass_pkgmap: failed to read build.gradle");
            return;
        }
    };

    for caps in GRADLE_DEP_RE.captures_iter(&content) {
        let dep_str = caps.get(1).map(|m| m.as_str().trim()).unwrap_or("");
        // Gradle deps are typically "group:artifact:version"
        let parts: Vec<&str> = dep_str.split(':').collect();
        if parts.len() >= 2 {
            let dep_name = format!("{}:{}", parts[0], parts[1]);
            let qn = format!("{}.deps.{}", project, dep_name);
            map.insert(dep_name, qn);
        } else if !dep_str.is_empty() {
            let qn = format!("{}.deps.{}", project, dep_str);
            map.insert(dep_str.to_string(), qn);
        }
    }
}

/// Parse mix.exs (Elixir) — extract `{:dep_name, ...}` tuples from the deps function.
/// Maps each dependency to `{project}.deps.{dep}`.
fn parse_mix_exs(path: &Path, project: &str, map: &mut PackageMap) {
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) => {
            tracing::debug!(error = %e, path = %path.display(), "pass_pkgmap: failed to read mix.exs");
            return;
        }
    };

    for caps in MIX_DEP_RE.captures_iter(&content) {
        if let Some(dep_name) = caps.get(1).map(|m| m.as_str()) {
            let qn = format!("{}.deps.{}", project, dep_name);
            map.insert(dep_name.to_string(), qn);
        }
    }
}

/// Parse *.gemspec (Ruby) — extract `add_dependency`, `add_runtime_dependency`,
/// `add_development_dependency` calls.
/// Maps each gem to `{project}.gems.{gem}`.
fn parse_gemspec(path: &Path, project: &str, map: &mut PackageMap) {
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) => {
            tracing::debug!(error = %e, path = %path.display(), "pass_pkgmap: failed to read gemspec");
            return;
        }
    };

    for caps in GEMSPEC_DEP_RE.captures_iter(&content) {
        if let Some(gem_name) = caps.get(1).map(|m| m.as_str()) {
            let qn = format!("{}.gems.{}", project, gem_name);
            map.insert(gem_name.to_string(), qn);
        }
    }
}

/// Parse setup.py (Python) — extract packages from `install_requires` list.
/// Maps each package to `{project}.site_packages.{pkg}`.
fn parse_setup_py(path: &Path, project: &str, map: &mut PackageMap) {
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) => {
            tracing::debug!(error = %e, path = %path.display(), "pass_pkgmap: failed to read setup.py");
            return;
        }
    };

    // Find the install_requires section
    let Some(start) = content.find("install_requires") else {
        return;
    };
    // Find the opening bracket
    let rest = &content[start..];
    let Some(bracket_start) = rest.find('[') else {
        return;
    };
    let rest = &rest[bracket_start..];
    let Some(bracket_end) = rest.find(']') else {
        return;
    };
    let requires_block = &rest[1..bracket_end];

    // Extract package names from quoted strings within the block
    for caps in SETUP_PY_DEP_RE.captures_iter(requires_block) {
        if let Some(dep_str) = caps.get(1).map(|m| m.as_str()) {
            let pkg_name = extract_pep508_name(dep_str);
            if !pkg_name.is_empty() {
                let qn = format!("{}.site_packages.{}", project, pkg_name);
                map.insert(pkg_name.to_string(), qn);
            }
        }
    }
}

/// Pass 1: Create structure nodes (Project, Folder, File).
pub fn pass_structure(
    buf: &mut GraphBuffer,
    project: &str,
    _repo_path: &Path,
    files: &[DiscoveredFile],
) {
    // Project node
    let proj_qn = project.to_owned();
    buf.add_node("Project", project, &proj_qn, "", 0, 0, None);

    // Collect unique directories
    let mut dirs = HashSet::new();
    for f in files {
        let mut dir = String::new();
        for part in f.rel_path.split('/') {
            if part.contains('.') && f.rel_path.ends_with(part) {
                break; // this is the filename
            }
            if !dir.is_empty() {
                dir.push('/');
            }
            dir.push_str(part);
            dirs.insert(dir.clone());
        }
    }

    for dir in &dirs {
        let folder_qn = fqn::fqn_folder(project, dir);
        let name = dir.rsplit('/').next().unwrap_or(dir);
        buf.add_node("Folder", name, &folder_qn, dir, 0, 0, None);
    }

    // File nodes + CONTAINS edges (Folder→File)
    for f in files {
        let file_qn = fqn::fqn_module(project, &f.rel_path);
        let name = Path::new(&f.rel_path)
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or(&f.rel_path);
        let props = serde_json::json!({ "language": f.language.name() }).to_string();
        buf.add_node("File", name, &file_qn, &f.rel_path, 0, 0, Some(props));

        // CONTAINS edge: parent folder → file
        let parent_dir = Path::new(&f.rel_path)
            .parent()
            .and_then(|p| p.to_str())
            .unwrap_or("");
        let parent_qn = if parent_dir.is_empty() {
            project.to_owned() // Project node
        } else {
            fqn::fqn_folder(project, parent_dir)
        };
        buf.add_edge_by_qn(&parent_qn, &file_qn, "CONTAINS", None);
    }

    // CONTAINS edges: Project → top-level folders, parent folder → child folder
    for dir in &dirs {
        let folder_qn = fqn::fqn_folder(project, dir);
        let parent_dir = Path::new(dir.as_str())
            .parent()
            .and_then(|p| p.to_str())
            .unwrap_or("");
        let parent_qn = if parent_dir.is_empty() {
            project.to_owned()
        } else {
            fqn::fqn_folder(project, parent_dir)
        };
        buf.add_edge_by_qn(&parent_qn, &folder_qn, "CONTAINS", None);
    }
}

/// Pass 3: Resolve call edges using the registry and Aho-Corasick.
/// When a `TypeRegistry` is provided, uses import context and receiver types
/// to disambiguate when multiple candidates match a call target name.
/// Falls back to name-based matching if type resolution fails.
pub fn pass_calls(buf: &mut GraphBuffer, reg: &Registry, files: &[&DiscoveredFile], project: &str) {
    pass_calls_with_types(buf, reg, None, files, project);
}

/// Type-aware variant of `pass_calls`. When `type_reg` is `Some`, uses it to
/// disambiguate ambiguous call targets by checking import relationships and
/// receiver types. Falls back to name-based matching otherwise.
pub fn pass_calls_with_types(
    buf: &mut GraphBuffer,
    reg: &Registry,
    type_reg: Option<&crate::registry::TypeRegistry>,
    files: &[&DiscoveredFile],
    project: &str,
) {
    if reg.is_empty() {
        return;
    }

    let names = reg.all_names();
    let ac = aho_corasick::AhoCorasick::builder()
        .ascii_case_insensitive(false)
        .build(&names);

    let ac = match ac {
        Ok(a) => a,
        Err(e) => {
            tracing::error!(
                patterns = names.len(),
                error = %e,
                "pass_calls: AhoCorasick build failed — no CALLS/USES edges will be created"
            );
            return;
        }
    };

    // Process files in parallel, collect (src_qn, tgt_qn, edge_type) tuples
    let edge_tuples: Vec<(String, String, String)> = files
        .par_iter()
        .flat_map(|f| {
            if is_pass_cancelled() {
                return vec![];
            }
            let source = match std::fs::read_to_string(&f.abs_path) {
                Ok(s) => s,
                Err(_) => return vec![],
            };

            let call_sites: HashSet<&str> = CALL_RE
                .find_iter(&source)
                .map(|m| m.as_str()[..m.as_str().len() - 1].trim())
                .collect();

            // Build byte-offset → line-number lookup
            let mut line_starts: Vec<usize> = vec![0];
            for (i, b) in source.bytes().enumerate() {
                if b == b'\n' {
                    line_starts.push(i + 1);
                }
            }

            // Get functions in this file for caller resolution
            let file_fns = reg.entries_for_file(&f.rel_path);

            let module_qn = fqn::fqn_module(project, &f.rel_path);
            let mut seen = HashSet::new();
            let mut edges = Vec::new();
            for mat in ac.find_iter(&source) {
                let name = &names[mat.pattern().as_usize()];
                if seen.contains(name.as_str()) {
                    continue;
                }
                seen.insert(name.as_str());

                // Convert byte offset to 1-based line number
                let line_num = (line_starts.partition_point(|&off| off <= mat.start())) as i32;

                // Find the containing function for this call site
                let caller_qn = file_fns
                    .iter()
                    .rev()
                    .find(|e| e.start_line <= line_num && e.end_line >= line_num)
                    .map(|e| e.qualified_name.as_str())
                    .unwrap_or(module_qn.as_str());

                let entries = reg.lookup(name);

                // Disambiguate when multiple candidates match
                let resolved = if entries.len() > 1 {
                    if let Some(tr) = type_reg {
                        disambiguate_call_target(tr, &f.rel_path, name, &entries, f.language)
                    } else {
                        None
                    }
                } else {
                    None
                };

                if let Some(entry) = resolved {
                    // Type resolution succeeded — emit a single edge
                    if entry.file_path != f.rel_path {
                        let edge_type = if call_sites.contains(name.as_str())
                            && matches!(entry.label.as_str(), "Function" | "Method")
                        {
                            "CALLS"
                        } else {
                            "USES"
                        };
                        edges.push((
                            caller_qn.to_owned(),
                            entry.qualified_name.clone(),
                            edge_type.to_owned(),
                        ));
                    }
                } else {
                    // Fallback: name-based matching (original behavior)
                    if entries.len() > 1 && type_reg.is_some() {
                        tracing::debug!(
                            name = name,
                            candidates = entries.len(),
                            file = f.rel_path.as_str(),
                            "pass_calls: type resolution failed, falling back to name-based matching"
                        );
                    }
                    for entry in entries {
                        if entry.file_path == f.rel_path {
                            continue;
                        }
                        let edge_type = if call_sites.contains(name.as_str())
                            && matches!(entry.label.as_str(), "Function" | "Method")
                        {
                            "CALLS"
                        } else {
                            "USES"
                        };
                        edges.push((
                            caller_qn.to_owned(),
                            entry.qualified_name.clone(),
                            edge_type.to_owned(),
                        ));
                    }
                }
            }
            edges
        })
        .collect();

    // Add edges to buffer serially (buffer is not Send)
    for (src, tgt, etype) in edge_tuples {
        buf.add_edge_by_qn(&src, &tgt, &etype, None);
    }
}

/// Disambiguate a call target when multiple registry entries match the same name.
///
/// Strategy:
/// 1. Prefer the entry whose file is directly imported by the caller's file.
/// 2. If a receiver type is available, prefer the entry whose qualified name
///    contains the receiver type (e.g., `MyClass.method`).
/// 3. If the target name is a known stdlib type for this language, skip it
///    (stdlib types don't get graph nodes).
/// 4. Return `None` to fall back to name-based matching.
fn disambiguate_call_target(
    type_reg: &crate::registry::TypeRegistry,
    caller_file: &str,
    target_name: &str,
    entries: &[crate::registry::RegistryEntry],
    lang: Language,
) -> Option<crate::registry::RegistryEntry> {
    // Skip stdlib types — they shouldn't create edges
    if crate::registry::is_stdlib_type(lang, target_name) {
        return None;
    }

    // Strategy 1: Prefer entries from files that the caller imports
    let imported: Vec<&crate::registry::RegistryEntry> = entries
        .iter()
        .filter(|e| type_reg.file_imports_from(caller_file, &e.file_path))
        .collect();

    if imported.len() == 1 {
        return Some(imported[0].clone());
    }

    // Strategy 2: Check if we have a receiver type for the target in this file
    // Look for `receiver.target_name(` pattern — the receiver's type might help
    if let Some(type_entry) = type_reg.resolve_type(caller_file, target_name) {
        let receiver_type = &type_entry.resolved_type;
        // Find entries whose qualified name contains the receiver type
        let by_receiver: Vec<&crate::registry::RegistryEntry> = entries
            .iter()
            .filter(|e| e.qualified_name.contains(receiver_type))
            .collect();
        if by_receiver.len() == 1 {
            return Some(by_receiver[0].clone());
        }
    }

    // If we narrowed to imported entries but still have multiple, return None
    // to fall back to name-based matching (emit edges to all candidates)
    None
}

/// Pass 4: Extract import edges.
pub fn pass_imports(buf: &mut GraphBuffer, files: &[&DiscoveredFile], project: &str) {
    pass_imports_with_pkgmap(buf, files, project, None, None);
}

/// Pass 4 (enhanced): Extract import edges with optional PackageMap for bare specifier resolution
/// and optional CompileCommandsMap for C/C++ `#include` resolution.
///
/// When a bare specifier is encountered and name-based resolution would produce a generic QN,
/// the PackageMap is consulted first. If the specifier is found in the map, the resolved QN
/// from the manifest is used instead. If not found, falls back to existing name-based resolution
/// and logs a debug message.
///
/// For C/C++ files, `#include` directives are parsed. If a CompileCommandsMap is provided and
/// the included header can't be resolved as a project-relative path, the file's include paths
/// from compile_commands.json are checked to find a matching header file.
pub fn pass_imports_with_pkgmap(
    buf: &mut GraphBuffer,
    files: &[&DiscoveredFile],
    project: &str,
    pkg_map: Option<&PackageMap>,
    cc_map: Option<&CompileCommandsMap>,
) {
    // Process files in parallel, collect (module_qn, target_qn) tuples
    let edge_tuples: Vec<(String, String)> = files
        .par_iter()
        .flat_map(|f| {
            if is_pass_cancelled() {
                return vec![];
            }
            let source = match std::fs::read_to_string(&f.abs_path) {
                Ok(s) => s,
                Err(_) => return vec![],
            };

            let module_qn = fqn::fqn_module(project, &f.rel_path);
            let mut edges = Vec::new();

            for line in source.lines() {
                let trimmed = line.trim();
                // Python: import X / from X import Y
                if trimmed.starts_with("import ") || trimmed.starts_with("from ") {
                    // Skip Java imports — handled below
                    if !trimmed.contains(';') {
                        if let Some(target) = parse_import_target(trimmed) {
                            let target_qn = resolve_bare_or_default(&target, pkg_map, || {
                                format!("{}.{}", project, target.replace('/', "."))
                            });
                            edges.push((module_qn.clone(), target_qn));
                        }
                    }
                }
                // JS/TS: import ... from '...'
                if trimmed.contains("from '")
                    || trimmed.contains("from \"")
                    || trimmed.contains("require(")
                {
                    if let Some(target) = parse_js_import(trimmed) {
                        // Check if this is a bare specifier (not a relative path)
                        let is_bare = is_bare_specifier(trimmed);
                        let target_qn = if is_bare {
                            // For bare specifiers, try PackageMap first
                            // The raw specifier is the original path before stripping ./
                            let raw_specifier = extract_raw_js_specifier(trimmed);
                            resolve_bare_or_default(
                                &raw_specifier.unwrap_or_else(|| target.clone()),
                                pkg_map,
                                || {
                                    if target.contains('/') {
                                        fqn::fqn_module(project, &format!("{}.ts", target))
                                    } else {
                                        format!("{}.{}", project, target)
                                    }
                                },
                            )
                        } else if target.contains('/') {
                            fqn::fqn_module(project, &format!("{}.ts", target))
                        } else {
                            format!("{}.{}", project, target)
                        };
                        edges.push((module_qn.clone(), target_qn));
                    }
                }
                // Rust: use crate::...
                if trimmed.starts_with("use ") {
                    if let Some(target) = parse_rust_use(trimmed) {
                        let target_qn = format!("{}.{}", project, target);
                        edges.push((module_qn.clone(), target_qn));
                    }
                }
                // Java/Kotlin: import com.example.Foo;
                if let Some(target) = parse_java_import(trimmed) {
                    let target_qn = format!("{}.{}", project, target);
                    edges.push((module_qn.clone(), target_qn));
                }
                // Go: import "path" or import ( "path" ... )
                if matches!(f.language, Language::Go) {
                    if let Some(target) = parse_go_import(trimmed) {
                        if !crate::go_common::is_stdlib_import(&target) {
                            // For Go, try PackageMap for the full module path
                            let target_qn = resolve_bare_or_default(&target, pkg_map, || {
                                let pkg = target.rsplit('/').next().unwrap_or(&target);
                                fqn::fqn_module(project, &format!("{pkg}/{pkg}.go"))
                            });
                            edges.push((module_qn.clone(), target_qn));
                        }
                    }
                }
                // C/C++/Objective-C: #include "header.h" or #include <header.h>
                if matches!(
                    f.language,
                    Language::C | Language::Cpp | Language::ObjectiveC
                ) {
                    if let Some(header) = parse_c_include(trimmed) {
                        let target_qn = resolve_c_include(&header, &f.rel_path, project, cc_map);
                        edges.push((module_qn.clone(), target_qn));
                    }
                }
            }
            edges
        })
        .collect();

    // Add edges to buffer serially
    for (src, tgt) in edge_tuples {
        buf.add_edge_by_qn(&src, &tgt, "IMPORTS", None);
    }
}

/// Resolve a bare specifier via the PackageMap, falling back to a default QN.
///
/// If the specifier is found in the PackageMap, returns the resolved QN.
/// Otherwise, calls `default_qn` to produce the fallback QN and logs a debug message.
fn resolve_bare_or_default(
    specifier: &str,
    pkg_map: Option<&PackageMap>,
    default_qn: impl FnOnce() -> String,
) -> String {
    if let Some(map) = pkg_map {
        if let Some(resolved_qn) = map.get(specifier) {
            return resolved_qn.clone();
        }
        // For scoped packages like @myorg/pkg/sub, also try the base package @myorg/pkg
        if specifier.contains('/') {
            let parts: Vec<&str> = specifier.splitn(3, '/').collect();
            // Handle @scope/name or @scope/name/subpath
            let base = if specifier.starts_with('@') && parts.len() >= 2 {
                format!("{}/{}", parts[0], parts[1])
            } else {
                parts[0].to_string()
            };
            if let Some(resolved_qn) = map.get(&base) {
                return resolved_qn.clone();
            }
        }
        tracing::debug!(specifier = %specifier, "bare import not resolved via manifest");
    }
    default_qn()
}

/// Check if a JS/TS import line contains a bare specifier (not a relative path).
fn is_bare_specifier(line: &str) -> bool {
    if let Some(raw) = extract_raw_js_specifier(line) {
        !raw.starts_with('.') && !raw.starts_with('/')
    } else {
        false
    }
}

/// Extract the raw specifier string from a JS/TS import/require line.
/// Returns the path as-is without stripping `./` or `../`.
fn extract_raw_js_specifier(line: &str) -> Option<String> {
    for delim in &["from '", "from \"", "require('", "require(\""] {
        if let Some(start) = line.find(delim) {
            let rest = &line[start + delim.len()..];
            let end_char = if delim.ends_with('\'') { '\'' } else { '"' };
            let end = rest.find(end_char)?;
            return Some(rest[..end].to_owned());
        }
    }
    None
}

fn parse_import_target(line: &str) -> Option<String> {
    // "from foo.bar import baz" -> "foo.bar"
    // "import foo.bar" -> "foo.bar"
    if let Some(rest) = line.strip_prefix("from ") {
        let end = rest.find(' ').unwrap_or(rest.len());
        return Some(rest[..end].to_owned());
    }
    if let Some(rest) = line.strip_prefix("import ") {
        let end = rest.find([' ', ',']).unwrap_or(rest.len());
        return Some(rest[..end].to_owned());
    }
    None
}

fn parse_js_import(line: &str) -> Option<String> {
    // Extract path from: from './foo' or require('./foo')
    for delim in &["from '", "from \"", "require('", "require(\""] {
        if let Some(start) = line.find(delim) {
            let rest = &line[start + delim.len()..];
            let end_char = if delim.ends_with('\'') { '\'' } else { '"' };
            let end = rest.find(end_char)?;
            let path = &rest[..end];
            // Strip leading ./ or ../
            let clean = path.trim_start_matches("./").trim_start_matches("../");
            return Some(clean.to_owned());
        }
    }
    None
}

fn parse_rust_use(line: &str) -> Option<String> {
    // "use crate::foo::bar;" -> "foo.bar"
    let rest = line.strip_prefix("use ")?.trim_end_matches(';').trim();
    let path = rest.strip_prefix("crate::").unwrap_or(rest);
    Some(path.replace("::", "."))
}

fn parse_java_import(line: &str) -> Option<String> {
    // "import com.example.Foo;" -> "Foo"
    // "import static com.example.Foo.bar;" -> "Foo"
    let rest = line.strip_prefix("import ")?.trim_end_matches(';').trim();
    let rest = rest.strip_prefix("static ").unwrap_or(rest).trim();
    // Get the last component (class name) — that's what we can resolve
    let class_name = rest.rsplit('.').next()?;
    if class_name.is_empty() || class_name == "*" {
        return None;
    }
    Some(class_name.to_owned())
}

/// Parse a Go import line. Handles: `import "path"`, `"path"`, `alias "path"`, `. "path"`.
fn parse_go_import(line: &str) -> Option<String> {
    // Single import: import "path"
    let s = line.strip_prefix("import ").unwrap_or(line).trim();
    // Extract quoted string (possibly after alias)
    let start = s.find('"')?;
    let rest = &s[start + 1..];
    let end = rest.find('"')?;
    let path = &rest[..end];
    if path.is_empty() {
        return None;
    }
    Some(path.to_owned())
}

/// Parse a C/C++ `#include` directive and return the header path.
/// Handles both `#include "header.h"` and `#include <header.h>`.
fn parse_c_include(line: &str) -> Option<String> {
    let trimmed = line.trim();
    let rest = trimmed.strip_prefix("#include")?;
    let rest = rest.trim();
    // #include "header.h"
    if let Some(inner) = rest.strip_prefix('"') {
        let end = inner.find('"')?;
        let path = &inner[..end];
        if !path.is_empty() {
            return Some(path.to_string());
        }
    }
    // #include <header.h>
    if let Some(inner) = rest.strip_prefix('<') {
        let end = inner.find('>')?;
        let path = &inner[..end];
        if !path.is_empty() {
            return Some(path.to_string());
        }
    }
    None
}

/// Resolve a C/C++ `#include` header path to a module QN.
///
/// First tries to resolve the header as a project-relative path. If that doesn't
/// look like a project file (no directory separator), checks the CompileCommandsMap
/// for the source file's include paths and tries to find the header under those paths.
/// Falls back to a generic QN based on the header path.
fn resolve_c_include(
    header: &str,
    source_rel_path: &str,
    project: &str,
    cc_map: Option<&CompileCommandsMap>,
) -> String {
    // If the header contains a directory separator, treat it as a project-relative path
    if header.contains('/') {
        return fqn::fqn_module(project, header);
    }

    // Try to resolve via compile_commands.json include paths
    if let Some(map) = cc_map {
        if let Some(ctx) = map.get(source_rel_path) {
            if let Some(inc_path) = ctx.include_paths.first() {
                // Build a project-relative path: include_path/header
                let candidate = format!("{}/{}", inc_path.trim_end_matches('/'), header);
                // Normalize: strip leading ./ if present
                let normalized = candidate.trim_start_matches("./").to_string();
                return fqn::fqn_module(project, &normalized);
            }
        }
    }

    // Fallback: resolve relative to the source file's directory
    let source_dir = Path::new(source_rel_path)
        .parent()
        .and_then(|p| p.to_str())
        .unwrap_or("");
    if source_dir.is_empty() {
        fqn::fqn_module(project, header)
    } else {
        fqn::fqn_module(project, &format!("{}/{}", source_dir, header))
    }
}

/// Pass 5: Semantic edges (INHERITS, IMPLEMENTS) from class declarations.
static EXTENDS_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(
        r"(?:class|interface)\s+\w+(?:<[^>]*>)?\s+extends\s+([\w,\s<>]+?)(?:\s+implements|\s*\{|$)",
    )
    .unwrap()
});
static IMPLEMENTS_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(
        r"class\s+\w+(?:<[^>]*>)?(?:\s+extends\s+[\w<>]+)?\s+implements\s+([\w,\s<>]+?)(?:\s*\{|$)",
    )
    .unwrap()
});
static CLASS_NAME_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"(?:class|interface)\s+(\w+)").unwrap());
static PY_INHERITS_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"^class\s+(\w+)\s*\(([^)]+)\)").unwrap());
static RUST_IMPL_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"^impl(?:<[^>]*>)?\s+(\w+)\s+for\s+(\w+)").unwrap());

pub fn pass_semantic(store: &Store, project: &str, files: &[&DiscoveredFile]) -> Result<()> {
    use codryn_graph_buffer::GraphBuffer;

    let mut buf = GraphBuffer::new(project);

    for f in files {
        let source = match std::fs::read_to_string(&f.abs_path) {
            Ok(s) => s,
            Err(_) => continue,
        };

        match f.language {
            Language::TypeScript | Language::Tsx | Language::JavaScript => {
                for line in source.lines() {
                    let trimmed = line.trim();
                    // class name
                    let class_name = CLASS_NAME_RE
                        .captures(trimmed)
                        .and_then(|c| c.get(1))
                        .map(|m| m.as_str().to_owned());

                    if let Some(ref cname) = class_name {
                        let src_qn = fqn::fqn_compute(project, &f.rel_path, Some(cname));
                        // extends
                        if let Some(caps) = EXTENDS_RE.captures(trimmed) {
                            for parent in caps.get(1).unwrap().as_str().split(',') {
                                let parent = parent.trim().split('<').next().unwrap_or("").trim();
                                if !parent.is_empty() {
                                    let tgt_qn = format!("{}.{}", project, parent);
                                    buf.add_edge_by_qn(&src_qn, &tgt_qn, "INHERITS", None);
                                }
                            }
                        }
                        // implements
                        if let Some(caps) = IMPLEMENTS_RE.captures(trimmed) {
                            for iface in caps.get(1).unwrap().as_str().split(',') {
                                let iface = iface.trim().split('<').next().unwrap_or("").trim();
                                if !iface.is_empty() {
                                    let tgt_qn = format!("{}.{}", project, iface);
                                    buf.add_edge_by_qn(&src_qn, &tgt_qn, "IMPLEMENTS", None);
                                }
                            }
                        }
                    }
                }
            }
            Language::Python | Language::Ruby => {
                for line in source.lines() {
                    if let Some(caps) = PY_INHERITS_RE.captures(line.trim()) {
                        let child = caps.get(1).unwrap().as_str();
                        let src_qn = fqn::fqn_compute(project, &f.rel_path, Some(child));
                        for parent in caps.get(2).unwrap().as_str().split(',') {
                            let parent = parent.trim();
                            if !parent.is_empty() && parent != "object" {
                                let tgt_qn = format!("{}.{}", project, parent);
                                buf.add_edge_by_qn(&src_qn, &tgt_qn, "INHERITS", None);
                            }
                        }
                    }
                }
            }
            Language::Rust => {
                for line in source.lines() {
                    if let Some(caps) = RUST_IMPL_RE.captures(line.trim()) {
                        let trait_name = caps.get(1).unwrap().as_str();
                        let struct_name = caps.get(2).unwrap().as_str();
                        let src_qn = fqn::fqn_compute(project, &f.rel_path, Some(struct_name));
                        let tgt_qn = format!("{}.{}", project, trait_name);
                        buf.add_edge_by_qn(&src_qn, &tgt_qn, "IMPLEMENTS", None);
                    }
                }
            }
            _ => {}
        }
    }

    buf.flush(store)?;
    Ok(())
}

// ── Usage References (non-call) ───────────────────────

/// Pass: Create USES edges for non-call references (variable, type, constant references).
/// Similar to `pass_calls` but only emits USES edges and filters out call sites.
pub fn pass_usages(
    buf: &mut GraphBuffer,
    reg: &Registry,
    files: &[&DiscoveredFile],
    project: &str,
) {
    if reg.is_empty() {
        return;
    }

    let names = reg.all_names();
    let ac = match aho_corasick::AhoCorasick::builder()
        .ascii_case_insensitive(false)
        .build(&names)
    {
        Ok(a) => a,
        Err(e) => {
            tracing::error!(error = %e, "pass_usages: AhoCorasick build failed");
            return;
        }
    };

    let edge_tuples: Vec<(String, String)> = files
        .par_iter()
        .flat_map(|f| {
            if is_pass_cancelled() {
                return vec![];
            }
            let source = match std::fs::read_to_string(&f.abs_path) {
                Ok(s) => s,
                Err(_) => return vec![],
            };

            // Collect call sites: names immediately followed by '('
            let call_sites: HashSet<&str> = CALL_RE
                .find_iter(&source)
                .map(|m| m.as_str()[..m.as_str().len() - 1].trim())
                .collect();

            // Build byte-offset → line-number lookup
            let mut line_starts: Vec<usize> = vec![0];
            for (i, b) in source.bytes().enumerate() {
                if b == b'\n' {
                    line_starts.push(i + 1);
                }
            }

            let file_fns = reg.entries_for_file(&f.rel_path);
            let module_qn = fqn::fqn_module(project, &f.rel_path);
            let mut seen = HashSet::new();
            let mut edges = Vec::new();

            for mat in ac.find_iter(&source) {
                let name = &names[mat.pattern().as_usize()];

                // Skip if this is a call site — pass_calls handles those
                if call_sites.contains(name.as_str()) {
                    continue;
                }

                // Check that the match is at a word boundary
                let start = mat.start();
                let end = mat.end();
                if start > 0 {
                    let prev = source.as_bytes()[start - 1];
                    if prev.is_ascii_alphanumeric() || prev == b'_' {
                        continue;
                    }
                }
                if end < source.len() {
                    let next = source.as_bytes()[end];
                    if next.is_ascii_alphanumeric() || next == b'_' {
                        continue;
                    }
                }

                let line_num = (line_starts.partition_point(|&off| off <= start)) as i32;

                let caller_qn = file_fns
                    .iter()
                    .rev()
                    .find(|e| e.start_line <= line_num && e.end_line >= line_num)
                    .map(|e| e.qualified_name.as_str())
                    .unwrap_or(module_qn.as_str());

                let entries = reg.lookup(name);
                for entry in entries {
                    // Skip self-references within the same file
                    if entry.file_path == f.rel_path {
                        continue;
                    }
                    let key = (caller_qn.to_owned(), entry.qualified_name.clone());
                    if seen.insert(key.clone()) {
                        edges.push(key);
                    }
                }
            }
            edges
        })
        .collect();

    for (src, tgt) in edge_tuples {
        buf.add_edge_by_qn(&src, &tgt, "USES", None);
    }
}

// ── Test Discovery ───────────────────────────────────

/// Returns true if the file path looks like a test file.
fn is_test_file(rel_path: &str) -> bool {
    let p = rel_path.to_lowercase();
    p.contains("/test/")
        || p.contains("/tests/")
        || p.contains("/__tests__/")
        || p.contains(".test.")
        || p.contains(".spec.")
        || p.contains("_test.")
        || p.ends_with("_test.go")
        || p.ends_with("_test.rs")
        || p.ends_with("_test.py")
}

/// Pass: Identify test files/functions, mark them with is_test, and create TESTS edges.
pub fn pass_tests(
    buf: &mut GraphBuffer,
    store: &Store,
    reg: &Registry,
    files: &[&DiscoveredFile],
    project: &str,
) {
    // Step 1: Identify test files and mark test functions with is_test property
    let test_file_paths: HashSet<String> = files
        .iter()
        .filter(|f| is_test_file(&f.rel_path))
        .map(|f| f.rel_path.clone())
        .collect();

    if test_file_paths.is_empty() {
        return;
    }

    // Step 2: For each test file, get its nodes and mark functions as is_test
    for test_path in &test_file_paths {
        let nodes = store
            .get_nodes_for_file(project, test_path)
            .unwrap_or_default();
        for node in &nodes {
            if !matches!(node.label.as_str(), "Function" | "Method") {
                continue;
            }
            // Mark as test if not already marked
            let already_test = node
                .properties_json
                .as_deref()
                .and_then(|p| serde_json::from_str::<serde_json::Value>(p).ok())
                .and_then(|v| v.get("is_test")?.as_bool())
                .unwrap_or(false);

            if !already_test {
                let mut props: serde_json::Value = node
                    .properties_json
                    .as_deref()
                    .and_then(|p| serde_json::from_str(p).ok())
                    .unwrap_or_else(|| serde_json::json!({}));
                props["is_test"] = serde_json::json!(true);
                let _ = store.update_node_properties(node.id, &props.to_string());
            }
        }
    }

    // Step 3: Create TESTS edges from test functions to the symbols they call
    // Use the registry to find what each test function references
    let names = reg.all_names();
    if names.is_empty() {
        return;
    }

    let ac = match aho_corasick::AhoCorasick::builder()
        .ascii_case_insensitive(false)
        .build(&names)
    {
        Ok(a) => a,
        Err(e) => {
            tracing::error!(error = %e, "pass_tests: AhoCorasick build failed");
            return;
        }
    };

    let edge_tuples: Vec<(String, String)> = files
        .par_iter()
        .filter(|f| test_file_paths.contains(&f.rel_path))
        .flat_map(|f| {
            if is_pass_cancelled() {
                return vec![];
            }
            let source = match std::fs::read_to_string(&f.abs_path) {
                Ok(s) => s,
                Err(_) => return vec![],
            };

            let mut line_starts: Vec<usize> = vec![0];
            for (i, b) in source.bytes().enumerate() {
                if b == b'\n' {
                    line_starts.push(i + 1);
                }
            }

            let file_fns = reg.entries_for_file(&f.rel_path);
            let module_qn = fqn::fqn_module(project, &f.rel_path);
            let mut seen = HashSet::new();
            let mut edges = Vec::new();

            for mat in ac.find_iter(&source) {
                let name = &names[mat.pattern().as_usize()];
                let line_num = (line_starts.partition_point(|&off| off <= mat.start())) as i32;

                let caller_qn = file_fns
                    .iter()
                    .rev()
                    .find(|e| e.start_line <= line_num && e.end_line >= line_num)
                    .map(|e| e.qualified_name.as_str())
                    .unwrap_or(module_qn.as_str());

                let entries = reg.lookup(name);
                for entry in entries {
                    // Only create TESTS edges to symbols in non-test files
                    if test_file_paths.contains(&entry.file_path) {
                        continue;
                    }
                    if entry.file_path == f.rel_path {
                        continue;
                    }
                    let key = (caller_qn.to_owned(), entry.qualified_name.clone());
                    if seen.insert(key.clone()) {
                        edges.push(key);
                    }
                }
            }
            edges
        })
        .collect();

    for (src, tgt) in edge_tuples {
        buf.add_edge_by_qn(&src, &tgt, "TESTS", None);
    }
}

// ── Environment Variable Scanning ────────────────────

static ENV_PATTERNS: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(
        r#"(?x)
        # JS/TS: process.env.VAR_NAME or process.env["VAR_NAME"] or process.env['VAR_NAME']
        process\.env\.([A-Za-z_][A-Za-z0-9_]*) |
        process\.env\["([A-Za-z_][A-Za-z0-9_]*)"\] |
        process\.env\['([A-Za-z_][A-Za-z0-9_]*)'\] |
        # Python: os.environ["VAR"] or os.environ['VAR'] or os.environ.get("VAR") or os.getenv("VAR")
        os\.environ\["([A-Za-z_][A-Za-z0-9_]*)"\] |
        os\.environ\['([A-Za-z_][A-Za-z0-9_]*)'\] |
        os\.environ\.get\("([A-Za-z_][A-Za-z0-9_]*)"\) |
        os\.environ\.get\('([A-Za-z_][A-Za-z0-9_]*)'\) |
        os\.getenv\("([A-Za-z_][A-Za-z0-9_]*)"\) |
        os\.getenv\('([A-Za-z_][A-Za-z0-9_]*)'\) |
        # Rust: std::env::var("VAR") or env::var("VAR")
        (?:std::)?env::var\("([A-Za-z_][A-Za-z0-9_]*)"\) |
        # Go: os.Getenv("VAR")
        os\.Getenv\("([A-Za-z_][A-Za-z0-9_]*)"\) |
        # Java: System.getenv("VAR")
        System\.getenv\("([A-Za-z_][A-Za-z0-9_]*)"\)
        "#,
    )
    .unwrap()
});

/// Pass: Scan for environment variable access patterns and create EnvVar nodes + READS_ENV edges.
pub fn pass_envscan(
    buf: &mut GraphBuffer,
    reg: &Registry,
    files: &[&DiscoveredFile],
    project: &str,
) {
    let mut env_vars_seen: HashSet<String> = HashSet::new();

    let edge_tuples: Vec<(String, String, String)> = files
        .par_iter()
        .flat_map(|f| {
            if is_pass_cancelled() {
                return vec![];
            }
            let source = match std::fs::read_to_string(&f.abs_path) {
                Ok(s) => s,
                Err(_) => return vec![],
            };

            let mut line_starts: Vec<usize> = vec![0];
            for (i, b) in source.bytes().enumerate() {
                if b == b'\n' {
                    line_starts.push(i + 1);
                }
            }

            let file_fns = reg.entries_for_file(&f.rel_path);
            let module_qn = fqn::fqn_module(project, &f.rel_path);
            let mut edges = Vec::new();

            for caps in ENV_PATTERNS.captures_iter(&source) {
                // Extract the env var name from whichever capture group matched
                let env_var = (1..=12).find_map(|i| caps.get(i).map(|m| m.as_str().to_owned()));

                let env_var = match env_var {
                    Some(v) if !v.is_empty() => v,
                    _ => continue,
                };

                let mat_start = caps.get(0).unwrap().start();
                let line_num = (line_starts.partition_point(|&off| off <= mat_start)) as i32;

                let caller_qn = file_fns
                    .iter()
                    .rev()
                    .find(|e| e.start_line <= line_num && e.end_line >= line_num)
                    .map(|e| e.qualified_name.as_str())
                    .unwrap_or(module_qn.as_str());

                let env_qn = format!("{}.envvar.{}", project, env_var);
                edges.push((caller_qn.to_owned(), env_var, env_qn));
            }
            edges
        })
        .collect::<Vec<_>>();

    // Create EnvVar nodes and READS_ENV edges serially
    for (caller_qn, env_var, env_qn) in &edge_tuples {
        if env_vars_seen.insert(env_qn.clone()) {
            buf.add_node("EnvVar", env_var, env_qn, "", 0, 0, None);
        }
        buf.add_edge_by_qn(caller_qn, env_qn, "READS_ENV", None);
    }
}

// ── REST Contract Indexing ────────────────────────────

/// Pass: Route nodes for REST controllers. Java/Kotlin now handled by AST extractors.
pub fn pass_rest_contracts(
    _buf: &mut GraphBuffer,
    _reg: &Registry,
    _files: &[&DiscoveredFile],
    _project: &str,
) {
    // Java/Kotlin routes are now handled by pass_spring_routes.
    // This pass is kept as a no-op for future non-Java/Kotlin REST frameworks.
}

/// Pass: Create Route nodes + HANDLES_ROUTE/ACCEPTS_DTO/RETURNS_DTO edges for Spring Boot controllers.
/// Runs on ALL Java/Kotlin files (not just changed) so edges survive reindex.
pub fn pass_spring_routes(buf: &mut GraphBuffer, files: &[&DiscoveredFile], project: &str) {
    for f in files {
        match f.language {
            Language::Java => {
                crate::spring_java::create_routes(buf, project, f);
            }
            Language::Kotlin => {
                crate::spring_kotlin::create_routes(buf, project, f);
            }
            _ => {}
        }
    }
}

// ── Angular Template Awareness ────────────────────────

static CUSTOM_ELEMENT_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"<([\w]+-[\w-]+)").unwrap());

/// Pass: Parse Angular .component.html files for custom element selectors and create RENDERS edges.
pub fn pass_angular_templates(
    buf: &mut GraphBuffer,
    store: &Store,
    files: &[&DiscoveredFile],
    project: &str,
) {
    for f in files {
        if !f.rel_path.ends_with(".component.html") {
            continue;
        }
        let source = match std::fs::read_to_string(&f.abs_path) {
            Ok(s) => s,
            Err(_) => continue,
        };

        // Resolve parent component: co-located .component.ts
        let ts_path = f.rel_path.replace(".component.html", ".component.ts");
        let parent_qn = fqn::fqn_module(project, &ts_path);
        // Find the actual class node in the TS file
        let parent_nodes = store
            .search_nodes_filtered(project, &ts_path, Some("Class"), 1)
            .unwrap_or_default();
        let parent_qn = parent_nodes
            .first()
            .map(|n| n.qualified_name.as_str())
            .unwrap_or(parent_qn.as_str());

        // Find all custom element tags
        let mut seen = HashSet::new();
        for caps in CUSTOM_ELEMENT_RE.captures_iter(&source) {
            let selector = caps.get(1).unwrap().as_str();
            if !seen.insert(selector.to_owned()) {
                continue;
            }
            // Look up the component with this selector
            if let Ok(Some(child)) = store.find_node_by_property(project, "selector", selector) {
                buf.add_edge_by_qn(parent_qn, &child.qualified_name, "RENDERS", None);
            }
        }
    }
}

// ── MinHash Similarity Detection ─────────────────────

/// Default minimum line count for similarity fingerprinting.
const SIMILARITY_MIN_LINES: i32 = 8;
/// Default similarity threshold for creating SIMILAR_TO edges.
const SIMILARITY_THRESHOLD: f64 = 0.7;
/// Maximum number of functions to compare for similarity (O(n²) guard).
const SIMILARITY_MAX_FUNCTIONS: usize = 2_000;

/// Pass: Compute MinHash fingerprints for functions/methods and create SIMILAR_TO edges
/// for pairs exceeding the similarity threshold. Skipped in Fast IndexMode.
pub fn pass_similarity(buf: &mut GraphBuffer, store: &Store, project: &str, repo_path: &Path) {
    let functions = store
        .get_nodes_by_label(project, "Function", 10_000)
        .unwrap_or_default();
    let methods = store
        .get_nodes_by_label(project, "Method", 10_000)
        .unwrap_or_default();

    // Filter to functions with enough lines
    let all: Vec<&codryn_store::Node> = functions
        .iter()
        .chain(methods.iter())
        .filter(|n| (n.end_line - n.start_line + 1) >= SIMILARITY_MIN_LINES)
        .filter(|n| !n.file_path.is_empty())
        .collect();

    if all.len() < 2 {
        return;
    }

    // Read source files and cache them to avoid re-reading
    let mut file_cache: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();
    for n in &all {
        if !file_cache.contains_key(&n.file_path) {
            let abs = repo_path.join(&n.file_path);
            if let Ok(content) = std::fs::read_to_string(&abs) {
                file_cache.insert(n.file_path.clone(), content);
            }
        }
    }

    // Extract function bodies and compute fingerprints in parallel
    let bodies: Vec<Option<String>> = all
        .iter()
        .map(|n| {
            let source = file_cache.get(&n.file_path)?;
            let lines: Vec<&str> = source.lines().collect();
            let start = (n.start_line - 1).max(0) as usize;
            let end = (n.end_line as usize).min(lines.len());
            if start >= end {
                return None;
            }
            Some(lines[start..end].join("\n"))
        })
        .collect();

    let fingerprints: Vec<Option<codryn_foundation::minhash::Fingerprint>> = bodies
        .par_iter()
        .map(|body| {
            let body = body.as_ref()?;
            let tokens = codryn_foundation::minhash::structural_tokens(body);
            if tokens.is_empty() {
                return None;
            }
            let token_refs: Vec<&str> = tokens.to_vec();
            Some(codryn_foundation::minhash::Fingerprint::from_tokens(
                &token_refs,
            ))
        })
        .collect();

    // Build index of nodes that have valid fingerprints
    let valid: Vec<(usize, &codryn_foundation::minhash::Fingerprint)> = fingerprints
        .iter()
        .enumerate()
        .filter_map(|(i, fp)| fp.as_ref().map(|f| (i, f)))
        .take(SIMILARITY_MAX_FUNCTIONS)
        .collect();

    tracing::info!(
        total = all.len(),
        fingerprinted = valid.len(),
        "pass_similarity: comparing function pairs"
    );

    if valid.len() > SIMILARITY_MAX_FUNCTIONS {
        tracing::warn!(
            count = valid.len(),
            max = SIMILARITY_MAX_FUNCTIONS,
            "pass_similarity: too many functions, truncated to cap"
        );
    }

    // Compare all pairs in parallel (O(n²) — bounded by SIMILARITY_MAX_FUNCTIONS)
    let edge_tuples: Vec<(String, String, String)> = valid
        .par_iter()
        .enumerate()
        .flat_map(|(i, &(idx_a, fp_a))| {
            let mut edges = Vec::new();
            for &(idx_b, fp_b) in &valid[(i + 1)..] {
                let sim = fp_a.similarity(fp_b);
                if sim >= SIMILARITY_THRESHOLD {
                    let props =
                        serde_json::json!({ "similarity": (sim * 1000.0).round() / 1000.0 })
                            .to_string();
                    edges.push((
                        all[idx_a].qualified_name.clone(),
                        all[idx_b].qualified_name.clone(),
                        props,
                    ));
                }
            }
            edges
        })
        .collect();

    for (src, tgt, props) in edge_tuples {
        buf.add_edge_by_qn(&src, &tgt, "SIMILAR_TO", Some(props));
    }
}

// ── Cross-Project Name-Based Auto-Linking ─────────────

/// Pass: Create MAPS_TO edges between classes/interfaces with the same name across linked projects.
pub fn pass_cross_project_mapping(buf: &mut GraphBuffer, store: &Store, project: &str) {
    let links = store.get_linked_projects(project).unwrap_or_default();
    for link in &links {
        let matches = store
            .find_matching_symbols_across_projects(project, &link.target_project)
            .unwrap_or_default();
        for (a, b) in &matches {
            buf.add_edge_by_qn(&a.qualified_name, &b.qualified_name, "MAPS_TO", None);
        }
    }
}

// ── Config File Linking ──────────────────────────────

/// Config file extensions we recognize.
const CONFIG_EXTENSIONS: &[&str] = &[".json", ".yaml", ".yml", ".toml", ".ini", ".env"];

/// Regex to extract top-level keys from JSON-like content: `"key":` patterns.
static JSON_KEY_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r#"^\s*"([A-Za-z_][A-Za-z0-9_]*)"\s*:"#).unwrap());

/// Regex to extract top-level keys from YAML content: `key:` at start of line.
static YAML_KEY_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r#"^([A-Za-z_][A-Za-z0-9_]*)\s*:"#).unwrap());

/// Regex to extract top-level keys from TOML content: `key =` or `[section]`.
static TOML_KEY_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r#"^([A-Za-z_][A-Za-z0-9_]*)\s*="#).unwrap());

/// Regex to extract keys from INI/.env content: `KEY=value`.
static INI_KEY_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r#"^([A-Za-z_][A-Za-z0-9_]*)\s*="#).unwrap());

fn is_config_file(rel_path: &str) -> bool {
    let lower = rel_path.to_lowercase();
    CONFIG_EXTENSIONS.iter().any(|ext| lower.ends_with(ext))
}

fn extract_config_keys(rel_path: &str, content: &str) -> Vec<String> {
    let lower = rel_path.to_lowercase();
    let re = if lower.ends_with(".json") {
        &*JSON_KEY_RE
    } else if lower.ends_with(".yaml") || lower.ends_with(".yml") {
        &*YAML_KEY_RE
    } else if lower.ends_with(".toml") {
        &*TOML_KEY_RE
    } else {
        // .ini, .env
        &*INI_KEY_RE
    };

    let mut keys = Vec::new();
    let mut seen = HashSet::new();
    for line in content.lines() {
        if let Some(caps) = re.captures(line) {
            let key = caps.get(1).unwrap().as_str().to_owned();
            if seen.insert(key.clone()) {
                keys.push(key);
            }
        }
    }
    keys
}

/// Pass: Identify config files, extract keys, and create CONFIG_LINKS edges
/// between config keys and code symbols that reference them.
pub fn pass_configlink(
    buf: &mut GraphBuffer,
    reg: &Registry,
    files: &[&DiscoveredFile],
    project: &str,
) {
    // Step 1: Find config files and extract keys
    let mut config_keys: Vec<(String, String)> = Vec::new(); // (key, config_file_qn)

    for f in files {
        if !is_config_file(&f.rel_path) {
            continue;
        }
        let content = match std::fs::read_to_string(&f.abs_path) {
            Ok(s) => s,
            Err(_) => continue,
        };

        let keys = extract_config_keys(&f.rel_path, &content);
        let file_qn = fqn::fqn_module(project, &f.rel_path);

        for key in keys {
            config_keys.push((key, file_qn.clone()));
        }
    }

    if config_keys.is_empty() {
        return;
    }

    // Step 2: For each config key, check if any registered symbol has the same name
    for (key, config_qn) in &config_keys {
        let entries = reg.lookup(key);
        for entry in entries {
            buf.add_edge_by_qn(config_qn, &entry.qualified_name, "CONFIG_LINKS", None);
        }
    }
}

// ── Enhanced Config Key Normalization ─────────────────

/// Normalize a config key for matching against code symbols.
///
/// Strips common prefixes/extensions, detects env var patterns (ALL_CAPS_WITH_UNDERSCORES),
/// splits camelCase identifiers, and splits on `_`, `.`, `-` delimiters.
/// Returns a list of lowercase tokens.
pub fn normalize_config_key(key: &str) -> Vec<String> {
    let mut tokens = Vec::new();

    // Strip common prefixes/extensions
    let stripped = key
        .trim_start_matches(".env.")
        .trim_start_matches("config.")
        .trim_start_matches("settings.")
        .trim_end_matches(".json")
        .trim_end_matches(".yaml")
        .trim_end_matches(".yml")
        .trim_end_matches(".toml");

    if stripped.is_empty() {
        return tokens;
    }

    // Detect ALL_CAPS_WITH_UNDERSCORES (env var pattern)
    if stripped
        .chars()
        .all(|c| c.is_uppercase() || c == '_' || c.is_ascii_digit())
        && stripped.chars().any(|c| c.is_alphabetic())
    {
        tokens.extend(
            stripped
                .split('_')
                .filter(|s| !s.is_empty())
                .map(|s| s.to_lowercase()),
        );
    } else {
        // Split camelCase and on delimiters
        let mut current = String::new();
        for ch in stripped.chars() {
            if ch == '_' || ch == '.' || ch == '-' {
                if !current.is_empty() {
                    tokens.push(current.to_lowercase());
                    current.clear();
                }
            } else if ch.is_uppercase() && !current.is_empty() {
                tokens.push(current.to_lowercase());
                current.clear();
                current.push(ch);
            } else {
                current.push(ch);
            }
        }
        if !current.is_empty() {
            tokens.push(current.to_lowercase());
        }
    }

    tokens
}

/// Compute token overlap ratio between normalized config tokens and a symbol name.
///
/// Returns the fraction of config tokens that appear as substrings in the symbol name.
/// A score > 0.5 indicates a likely match.
pub fn compute_match_score(config_tokens: &[String], symbol_name: &str) -> f64 {
    if config_tokens.is_empty() {
        return 0.0;
    }
    let name_lower = symbol_name.to_lowercase();
    let matching = config_tokens
        .iter()
        .filter(|t| name_lower.contains(t.as_str()))
        .count();
    matching as f64 / config_tokens.len() as f64
}

/// Enhanced config linking with normalized key matching.
///
/// Replaces the simple substring matching in `pass_configlink` with camelCase splitting,
/// env var detection, and prefix stripping. Creates CONFIG_LINKS edges with `config_key`
/// and `confidence` properties for matches with score > 0.5.
pub fn pass_configures(
    buf: &mut GraphBuffer,
    reg: &Registry,
    files: &[&DiscoveredFile],
    project: &str,
) {
    for f in files {
        if !is_config_file(&f.rel_path) {
            continue;
        }
        let content = match std::fs::read_to_string(&f.abs_path) {
            Ok(s) => s,
            Err(_) => continue,
        };

        let keys = extract_config_keys(&f.rel_path, &content);
        if keys.is_empty() {
            continue;
        }

        let all_names = reg.all_names();

        for key in &keys {
            let normalized_tokens = normalize_config_key(key);
            if normalized_tokens.is_empty() {
                continue;
            }

            for name in &all_names {
                let score = compute_match_score(&normalized_tokens, name);
                if score > 0.5 {
                    let entries = reg.lookup(name);
                    for entry in &entries {
                        let props = serde_json::json!({
                            "config_key": key,
                            "confidence": (score * 1000.0).round() / 1000.0,
                        })
                        .to_string();
                        let source_qn = fqn::fqn_compute(project, &f.rel_path, Some(key));
                        buf.add_edge_by_qn(
                            &source_qn,
                            &entry.qualified_name,
                            "CONFIG_LINKS",
                            Some(props),
                        );
                    }
                }
            }
        }
    }
}

// ── Enrichment Pass ──────────────────────────────────

/// Pass: Compute fan-in, fan-out, and centrality for each node and store as properties.
pub fn pass_enrichment(store: &Store, project: &str) -> Result<()> {
    let all_nodes = store.get_all_nodes(project)?;
    if all_nodes.is_empty() {
        return Ok(());
    }

    // Compute all degrees in two bulk SQL queries instead of 2×N individual ones
    let degrees = store.node_degrees_bulk(project)?;

    // Find max degree for normalization
    let max_degree: i32 = degrees
        .values()
        .map(|(fi, fo)| fi + fo)
        .max()
        .unwrap_or(1)
        .max(1);

    // Build batch of property updates
    let mut updates: Vec<(i64, String)> = Vec::with_capacity(all_nodes.len());
    for node in &all_nodes {
        let (fan_in, fan_out) = degrees.get(&node.id).copied().unwrap_or((0, 0));
        let centrality = (fan_in + fan_out) as f64 / max_degree as f64;

        // Merge with existing properties
        let mut props: serde_json::Value = node
            .properties_json
            .as_deref()
            .and_then(|p| serde_json::from_str(p).ok())
            .unwrap_or_else(|| serde_json::json!({}));

        props["fan_in"] = serde_json::json!(fan_in);
        props["fan_out"] = serde_json::json!(fan_out);
        props["centrality"] = serde_json::json!((centrality * 1000.0).round() / 1000.0);

        updates.push((node.id, props.to_string()));
    }

    store.update_node_properties_batch(&updates)?;

    Ok(())
}

// ── Generic Route Detection ──────────────────────────

/// Regex for Express.js routes: app.get('/path', ...) or router.post('/path', ...)
static EXPRESS_ROUTE_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(
        r#"(?:app|router)\.(get|post|put|patch|delete|options|head|all)\s*\(\s*['"]([^'"]+)['"]"#,
    )
    .unwrap()
});

/// Regex for FastAPI routes: @app.get("/path") or @router.post("/path")
static FASTAPI_ROUTE_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(
        r#"@(?:app|router)\.(get|post|put|patch|delete|options|head)\s*\(\s*['"]([^'"]+)['"]"#,
    )
    .unwrap()
});

/// Regex for Flask routes: @app.route("/path", methods=["GET"])
static FLASK_ROUTE_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(
        r#"@(?:app|blueprint|bp)\.route\s*\(\s*['"]([^'"]+)['"]\s*(?:,\s*methods\s*=\s*\[([^\]]*)\])?"#,
    )
    .unwrap()
});

/// Regex for Gin routes: r.GET("/path", handler) or r.POST("/path", handler)
static GIN_ROUTE_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(
        r#"(?:\w+)\.(GET|POST|PUT|PATCH|DELETE|OPTIONS|HEAD|Any)\s*\(\s*['"]([^'"]+)['"]"#,
    )
    .unwrap()
});

/// Pass: Detect HTTP route declarations in Express.js, FastAPI, Flask, and Gin,
/// and create Route nodes with HANDLES_ROUTE edges.
pub fn pass_route_nodes(
    buf: &mut GraphBuffer,
    reg: &Registry,
    files: &[&DiscoveredFile],
    project: &str,
) {
    for f in files {
        let source = match std::fs::read_to_string(&f.abs_path) {
            Ok(s) => s,
            Err(_) => continue,
        };

        let mut line_starts: Vec<usize> = vec![0];
        for (i, b) in source.bytes().enumerate() {
            if b == b'\n' {
                line_starts.push(i + 1);
            }
        }

        let file_fns = reg.entries_for_file(&f.rel_path);
        let module_qn = fqn::fqn_module(project, &f.rel_path);

        let mut routes_found: Vec<(String, String, i32)> = Vec::new(); // (method, path, line)

        match f.language {
            Language::JavaScript | Language::TypeScript | Language::Tsx => {
                for caps in EXPRESS_ROUTE_RE.captures_iter(&source) {
                    let method = caps.get(1).unwrap().as_str().to_uppercase();
                    let path = caps.get(2).unwrap().as_str().to_owned();
                    let offset = caps.get(0).unwrap().start();
                    let line = line_starts.partition_point(|&off| off <= offset) as i32;
                    routes_found.push((method, path, line));
                }
            }
            Language::Python => {
                // FastAPI
                for caps in FASTAPI_ROUTE_RE.captures_iter(&source) {
                    let method = caps.get(1).unwrap().as_str().to_uppercase();
                    let path = caps.get(2).unwrap().as_str().to_owned();
                    let offset = caps.get(0).unwrap().start();
                    let line = line_starts.partition_point(|&off| off <= offset) as i32;
                    routes_found.push((method, path, line));
                }
                // Flask
                for caps in FLASK_ROUTE_RE.captures_iter(&source) {
                    let path = caps.get(1).unwrap().as_str().to_owned();
                    let methods_str = caps.get(2).map(|m| m.as_str()).unwrap_or("GET");
                    let offset = caps.get(0).unwrap().start();
                    let line = line_starts.partition_point(|&off| off <= offset) as i32;
                    // Parse methods list
                    for method in methods_str.split(',') {
                        let m = method.trim().trim_matches(|c| c == '\'' || c == '"').trim();
                        if !m.is_empty() {
                            routes_found.push((m.to_uppercase(), path.clone(), line));
                        }
                    }
                }
            }
            Language::Go => {
                for caps in GIN_ROUTE_RE.captures_iter(&source) {
                    let method = caps.get(1).unwrap().as_str().to_uppercase();
                    let path = caps.get(2).unwrap().as_str().to_owned();
                    let offset = caps.get(0).unwrap().start();
                    let line = line_starts.partition_point(|&off| off <= offset) as i32;
                    routes_found.push((method, path, line));
                }
            }
            _ => continue,
        }

        // Create Route nodes and HANDLES_ROUTE edges
        for (method, path, line) in &routes_found {
            let route_name = format!("{} {}", method, path);
            let route_qn = format!("{}.route.{}.{}", project, method, path);
            let props = serde_json::json!({
                "http_method": method,
                "path": path,
            })
            .to_string();

            buf.add_node(
                "Route",
                &route_name,
                &route_qn,
                &f.rel_path,
                *line,
                *line,
                Some(props),
            );

            // Find the containing function for this route declaration
            let handler_qn = file_fns
                .iter()
                .rev()
                .find(|e| e.start_line <= *line && e.end_line >= *line)
                .map(|e| e.qualified_name.as_str())
                .unwrap_or(module_qn.as_str());

            buf.add_edge_by_qn(handler_qn, &route_qn, "HANDLES_ROUTE", None);
        }
    }
}

// ── Semantic Edges: Overrides and Delegation ─────────

/// Regex for @Override annotation (Java/Kotlin)
static OVERRIDE_ANNOTATION_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"@Override\b").unwrap());

/// Regex for `override` keyword in method declarations (C++, C#, Kotlin, TypeScript)
static OVERRIDE_KEYWORD_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"\boverride\s+(?:fun|func|def|async\s+)?\s*(\w+)").unwrap()
});

/// Regex for delegation: method body that just calls this.field.method() or self.field.method()
static DELEGATION_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"(?:return\s+)?(?:this|self)\s*\.\s*(\w+)\s*\.\s*(\w+)\s*\(").unwrap()
});

/// Pass: Detect method overrides and delegation patterns, creating OVERRIDES and DELEGATES_TO edges.
pub fn pass_semantic_edges(
    buf: &mut GraphBuffer,
    reg: &Registry,
    files: &[&DiscoveredFile],
    project: &str,
) {
    for f in files {
        let source = match std::fs::read_to_string(&f.abs_path) {
            Ok(s) => s,
            Err(_) => continue,
        };

        let mut line_starts: Vec<usize> = vec![0];
        for (i, b) in source.bytes().enumerate() {
            if b == b'\n' {
                line_starts.push(i + 1);
            }
        }

        let file_fns = reg.entries_for_file(&f.rel_path);
        let module_qn = fqn::fqn_module(project, &f.rel_path);

        // Detect @Override annotations
        for mat in OVERRIDE_ANNOTATION_RE.find_iter(&source) {
            let offset = mat.start();
            let line = line_starts.partition_point(|&off| off <= offset) as i32;

            // The overriding method is the function containing or immediately after @Override
            let overrider = file_fns
                .iter()
                .find(|e| e.start_line >= line && e.start_line <= line + 3)
                .or_else(|| {
                    file_fns
                        .iter()
                        .rev()
                        .find(|e| e.start_line <= line && e.end_line >= line)
                });

            if let Some(overrider) = overrider {
                // Look for a parent class method with the same name
                let entries = reg.lookup(overrider.qualified_name.rsplit('.').next().unwrap_or(""));
                for entry in entries {
                    if entry.qualified_name != overrider.qualified_name && entry.label == "Method" {
                        buf.add_edge_by_qn(
                            &overrider.qualified_name,
                            &entry.qualified_name,
                            "OVERRIDES",
                            None,
                        );
                    }
                }
            }
        }

        // Detect `override` keyword in method declarations
        for caps in OVERRIDE_KEYWORD_RE.captures_iter(&source) {
            let method_name = caps.get(1).unwrap().as_str();
            let offset = caps.get(0).unwrap().start();
            let line = line_starts.partition_point(|&off| off <= offset) as i32;

            let overrider = file_fns
                .iter()
                .find(|e| e.start_line <= line + 1 && e.end_line >= line);

            if let Some(overrider) = overrider {
                let entries = reg.lookup(method_name);
                for entry in entries {
                    if entry.qualified_name != overrider.qualified_name && entry.label == "Method" {
                        buf.add_edge_by_qn(
                            &overrider.qualified_name,
                            &entry.qualified_name,
                            "OVERRIDES",
                            None,
                        );
                    }
                }
            }
        }

        // Detect delegation patterns: this.field.method() or self.field.method()
        for caps in DELEGATION_RE.captures_iter(&source) {
            let _field_name = caps.get(1).unwrap().as_str();
            let delegated_method = caps.get(2).unwrap().as_str();
            let offset = caps.get(0).unwrap().start();
            let line = line_starts.partition_point(|&off| off <= offset) as i32;

            let delegator = file_fns
                .iter()
                .rev()
                .find(|e| e.start_line <= line && e.end_line >= line)
                .map(|e| e.qualified_name.as_str())
                .unwrap_or(module_qn.as_str());

            // Find the target method in the registry
            let entries = reg.lookup(delegated_method);
            for entry in entries {
                if entry.file_path != f.rel_path && entry.label == "Method" {
                    buf.add_edge_by_qn(delegator, &entry.qualified_name, "DELEGATES_TO", None);
                }
            }
        }
    }
}

// ── Channel / Event Detection ────────────────────────

/// Regex for emit patterns in JS/TS/Python:
///   emit("event"), socket.emit("event"), EventEmitter.emit("event"),
///   this.emit("event"), emitter.emit("event")
/// Captures the event name from the first string-literal argument.
static EMIT_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(
        r#"(?x)
        (?:\w+\.)?emit\s*\(\s*['"]([^'"]+)['"]\s*[,)]
        "#,
    )
    .unwrap()
});

/// Regex for listen patterns in JS/TS/Python:
///   on("event", ...), addEventListener("event", ...),
///   socket.on("event", ...), subscribe("event", ...)
static LISTEN_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(
        r#"(?x)
        (?:
            (?:\w+\.)?on\s*\(\s*['"]([^'"]+)['"]\s*[,)]  |
            (?:\w+\.)?addEventListener\s*\(\s*['"]([^'"]+)['"]\s*[,)]  |
            (?:\w+\.)?subscribe\s*\(\s*['"]([^'"]+)['"]\s*[,)]
        )
        "#,
    )
    .unwrap()
});

/// Regex for Redis pub/sub patterns:
///   publish("channel", ...), subscribe("channel")
///   client.publish("channel", ...), redis.subscribe("channel")
static REDIS_PUBSUB_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(
        r#"(?x)
        (?:\w+\.)?publish\s*\(\s*['"]([^'"]+)['"]\s*[,)]  |
        (?:\w+\.)?subscribe\s*\(\s*['"]([^'"]+)['"]\s*[,)]
        "#,
    )
    .unwrap()
});

/// Regex for RabbitMQ/AMQP patterns:
///   basicPublish("exchange", "routingKey", ...), channel.basicPublish(...)
///   basicConsume("queue", ...), channel.basicConsume(...)
static AMQP_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(
        r#"(?x)
        (?:\w+\.)?basicPublish\s*\(\s*['"]([^'"]+)['"]\s*[,)]  |
        (?:\w+\.)?basicConsume\s*\(\s*['"]([^'"]+)['"]\s*[,)]
        "#,
    )
    .unwrap()
});

/// Returns true if the event name looks dynamic (contains variable interpolation or is empty).
fn is_dynamic_event_name(name: &str) -> bool {
    name.is_empty()
        || name.contains("${")
        || name.contains("#{")
        || name.contains("{}")
        || name.contains("` +")
        || name.contains("+ `")
}

/// Pass: Detect event-driven communication patterns (emit/on, Redis pub/sub, RabbitMQ/AMQP)
/// and create Channel_Node nodes with EMITS/LISTENS edges.
///
/// Scans JS/TS/Python files for:
/// - Emit patterns: emit("event"), socket.emit("event"), EventEmitter.emit("event")
/// - Listen patterns: on("event"), addEventListener("event"), socket.on("event"), subscribe("event")
/// - Redis pub/sub: publish("channel"), subscribe("channel")
/// - RabbitMQ/AMQP: basicPublish("exchange"), basicConsume("queue")
///
/// Skips dynamic event names (variables, template literals) with a debug log.
pub fn pass_events(
    buf: &mut GraphBuffer,
    reg: &Registry,
    files: &[&DiscoveredFile],
    project: &str,
) {
    // Collect (caller_qn, event_name, edge_type) tuples in parallel
    let edge_tuples: Vec<(String, String, &str)> = files
        .par_iter()
        .flat_map(|f| {
            if is_pass_cancelled() {
                return vec![];
            }

            // Only scan JS/TS/Python files (plus any language for Redis/AMQP patterns)
            let scan_js_ts_py = matches!(
                f.language,
                Language::JavaScript | Language::TypeScript | Language::Tsx | Language::Python
            );

            // Skip files that have no relevant patterns at all
            if !scan_js_ts_py {
                // Still scan for Redis/AMQP patterns in other languages
                let source = match std::fs::read_to_string(&f.abs_path) {
                    Ok(s) => s,
                    Err(_) => return vec![],
                };
                // Quick check: does the file contain any relevant keywords?
                if !source.contains("basicPublish")
                    && !source.contains("basicConsume")
                    && !source.contains("publish(")
                    && !source.contains("subscribe(")
                {
                    return vec![];
                }
                return collect_event_edges(f, &source, reg, project, false);
            }

            let source = match std::fs::read_to_string(&f.abs_path) {
                Ok(s) => s,
                Err(_) => return vec![],
            };

            collect_event_edges(f, &source, reg, project, true)
        })
        .collect();

    // Create Channel_Node nodes and edges serially
    let mut channels_seen: HashSet<String> = HashSet::new();

    for (caller_qn, event_name, edge_type) in &edge_tuples {
        let channel_qn = format!("{}.channel.{}", project, event_name);

        if channels_seen.insert(channel_qn.clone()) {
            buf.add_node(
                "Channel",
                event_name,
                &channel_qn,
                "",
                0,
                0,
                Some(serde_json::json!({ "event_name": event_name }).to_string()),
            );
        }

        buf.add_edge_by_qn(caller_qn, &channel_qn, edge_type, None);
    }
}

/// Collect event edge tuples from a single file's source.
fn collect_event_edges(
    f: &DiscoveredFile,
    source: &str,
    reg: &Registry,
    project: &str,
    scan_emit_listen: bool,
) -> Vec<(String, String, &'static str)> {
    let mut line_starts: Vec<usize> = vec![0];
    for (i, b) in source.bytes().enumerate() {
        if b == b'\n' {
            line_starts.push(i + 1);
        }
    }

    let file_fns = reg.entries_for_file(&f.rel_path);
    let module_qn = fqn::fqn_module(project, &f.rel_path);
    let mut edges = Vec::new();

    // Helper closure to resolve the containing function for a byte offset
    let resolve_caller = |offset: usize| -> String {
        let line_num = line_starts.partition_point(|&off| off <= offset) as i32;
        file_fns
            .iter()
            .rev()
            .find(|e| e.start_line <= line_num && e.end_line >= line_num)
            .map(|e| e.qualified_name.clone())
            .unwrap_or_else(|| module_qn.clone())
    };

    if scan_emit_listen {
        // Emit patterns
        for caps in EMIT_RE.captures_iter(source) {
            let event_name = caps.get(1).unwrap().as_str();
            if is_dynamic_event_name(event_name) {
                tracing::debug!(
                    event = event_name,
                    file = f.rel_path.as_str(),
                    "pass_events: skipping dynamic event name in emit"
                );
                continue;
            }
            let offset = caps.get(0).unwrap().start();
            let caller_qn = resolve_caller(offset);
            edges.push((caller_qn, event_name.to_owned(), "EMITS"));
        }

        // Listen patterns
        for caps in LISTEN_RE.captures_iter(source) {
            // Extract event name from whichever capture group matched
            let event_name = (1..=3)
                .find_map(|i| caps.get(i).map(|m| m.as_str()))
                .unwrap_or("");
            if event_name.is_empty() || is_dynamic_event_name(event_name) {
                if !event_name.is_empty() {
                    tracing::debug!(
                        event = event_name,
                        file = f.rel_path.as_str(),
                        "pass_events: skipping dynamic event name in listener"
                    );
                }
                continue;
            }
            let offset = caps.get(0).unwrap().start();
            let caller_qn = resolve_caller(offset);
            edges.push((caller_qn, event_name.to_owned(), "LISTENS"));
        }
    }

    // Redis pub/sub patterns (all languages)
    for caps in REDIS_PUBSUB_RE.captures_iter(source) {
        let (event_name, edge_type) = if let Some(m) = caps.get(1) {
            (m.as_str(), "EMITS") // publish
        } else if let Some(m) = caps.get(2) {
            (m.as_str(), "LISTENS") // subscribe
        } else {
            continue;
        };
        if is_dynamic_event_name(event_name) {
            tracing::debug!(
                event = event_name,
                file = f.rel_path.as_str(),
                "pass_events: skipping dynamic event name in Redis pub/sub"
            );
            continue;
        }
        let offset = caps.get(0).unwrap().start();
        let caller_qn = resolve_caller(offset);
        edges.push((caller_qn, event_name.to_owned(), edge_type));
    }

    // RabbitMQ/AMQP patterns (all languages)
    for caps in AMQP_RE.captures_iter(source) {
        let (event_name, edge_type) = if let Some(m) = caps.get(1) {
            (m.as_str(), "EMITS") // basicPublish
        } else if let Some(m) = caps.get(2) {
            (m.as_str(), "LISTENS") // basicConsume
        } else {
            continue;
        };
        if is_dynamic_event_name(event_name) {
            tracing::debug!(
                event = event_name,
                file = f.rel_path.as_str(),
                "pass_events: skipping dynamic event name in AMQP"
            );
            continue;
        }
        let offset = caps.get(0).unwrap().start();
        let caller_qn = resolve_caller(offset);
        edges.push((caller_qn, event_name.to_owned(), edge_type));
    }

    edges
}

// ── Kubernetes Manifest Parsing ──────────────────────

/// Regex to extract `kind:` from a YAML document.
static K8S_KIND_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"(?m)^kind:\s*(\S+)").unwrap());

/// Regex to extract `metadata.name:` — matches `name:` indented under `metadata:`.
static K8S_NAME_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"(?m)^  name:\s*(\S+)").unwrap());

/// Regex to extract `metadata.namespace:`.
static K8S_NAMESPACE_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"(?m)^  namespace:\s*(\S+)").unwrap());

/// Regex to extract container image references: `image: <image>`.
static K8S_IMAGE_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r#"(?m)^\s+image:\s*['"]?([^\s'"]+)['"]?"#).unwrap());

/// Regex to extract `configMapRef.name` or `configMapKeyRef.name` references.
static K8S_CONFIGMAP_REF_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"(?m)(?:configMapRef|configMapKeyRef):\s*\n\s+name:\s*(\S+)").unwrap()
});

/// Regex to extract `secretRef.name` or `secretKeyRef.name` references.
static K8S_SECRET_REF_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"(?m)(?:secretRef|secretKeyRef):\s*\n\s+name:\s*(\S+)").unwrap()
});

/// Regex to extract labels block entries: `key: value` indented under `labels:`.
#[allow(dead_code)]
static K8S_LABELS_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"(?m)^\s{4}(\S+):\s*(\S+)").unwrap());

/// Known Kubernetes resource kinds we recognize.
const K8S_KINDS: &[&str] = &[
    "Deployment",
    "Service",
    "ConfigMap",
    "Secret",
    "Ingress",
    "StatefulSet",
    "DaemonSet",
    "Job",
    "CronJob",
    "Pod",
    "ReplicaSet",
    "Namespace",
    "ServiceAccount",
    "Role",
    "RoleBinding",
    "ClusterRole",
    "ClusterRoleBinding",
    "PersistentVolumeClaim",
    "PersistentVolume",
    "HorizontalPodAutoscaler",
    "NetworkPolicy",
];

/// Returns true if the file looks like a Kubernetes YAML manifest.
fn is_k8s_yaml(rel_path: &str, lang: Language) -> bool {
    matches!(lang, Language::Yaml) && {
        let lower = rel_path.to_lowercase();
        // Exclude kustomization files — handled by pass_kustomize
        !lower.ends_with("kustomization.yaml") && !lower.ends_with("kustomization.yml")
    }
}

/// Pass: Parse Kubernetes YAML manifests and create Resource nodes with metadata,
/// USES_IMAGE edges to Docker_Image nodes, and MOUNTS_CONFIG edges for ConfigMap/Secret refs.
pub fn pass_k8s(buf: &mut GraphBuffer, files: &[&DiscoveredFile], project: &str) {
    let mut images_seen: HashSet<String> = HashSet::new();

    for f in files {
        if !is_k8s_yaml(&f.rel_path, f.language) {
            continue;
        }

        let source = match std::fs::read_to_string(&f.abs_path) {
            Ok(s) => s,
            Err(_) => continue,
        };

        // Split on YAML document separators to handle multi-document files
        let documents: Vec<&str> = source.split("\n---").collect();

        for doc in documents {
            // Extract kind
            let kind = match K8S_KIND_RE.captures(doc) {
                Some(caps) => caps.get(1).unwrap().as_str().to_owned(),
                None => continue,
            };

            // Only process known K8s resource kinds
            if !K8S_KINDS.iter().any(|k| *k == kind) {
                continue;
            }

            // Extract metadata.name
            let resource_name = match K8S_NAME_RE.captures(doc) {
                Some(caps) => caps.get(1).unwrap().as_str().to_owned(),
                None => continue,
            };

            // Extract optional namespace
            let namespace = K8S_NAMESPACE_RE
                .captures(doc)
                .map(|caps| caps.get(1).unwrap().as_str().to_owned());

            // Build properties
            let mut props = serde_json::json!({
                "kind": kind,
                "resource_name": resource_name,
            });
            if let Some(ref ns) = namespace {
                props["namespace"] = serde_json::json!(ns);
            }

            // Extract labels (simple heuristic: lines with 4-space indent under labels:)
            if let Some(labels_start) = doc.find("labels:") {
                let labels_section = &doc[labels_start..];
                let mut labels = serde_json::Map::new();
                let mut first = true;
                for line in labels_section.lines() {
                    if first {
                        first = false;
                        continue; // skip the "labels:" line itself
                    }
                    let trimmed = line.trim();
                    if trimmed.is_empty() || (!line.starts_with("    ") && !line.starts_with("\t"))
                    {
                        break;
                    }
                    if let Some((k, v)) = trimmed.split_once(':') {
                        let k = k.trim();
                        let v = v.trim().trim_matches('"').trim_matches('\'');
                        if !k.is_empty() && !v.is_empty() {
                            labels.insert(k.to_owned(), serde_json::json!(v));
                        }
                    }
                }
                if !labels.is_empty() {
                    props["labels"] = serde_json::Value::Object(labels);
                }
            }

            let resource_qn = format!("{}.k8s.{}.{}", project, kind, resource_name);
            buf.add_node(
                "Resource",
                &resource_name,
                &resource_qn,
                &f.rel_path,
                0,
                0,
                Some(props.to_string()),
            );

            // Extract container images and create USES_IMAGE edges
            for caps in K8S_IMAGE_RE.captures_iter(doc) {
                let image = caps.get(1).unwrap().as_str();
                let image_qn = format!("{}.docker_image.{}", project, image);

                if images_seen.insert(image_qn.clone()) {
                    buf.add_node(
                        "Docker_Image",
                        image,
                        &image_qn,
                        &f.rel_path,
                        0,
                        0,
                        Some(serde_json::json!({ "image": image }).to_string()),
                    );
                }

                buf.add_edge_by_qn(&resource_qn, &image_qn, "USES_IMAGE", None);

                // Also store image in resource properties
                props["image"] = serde_json::json!(image);
            }

            // Extract ConfigMap references and create MOUNTS_CONFIG edges
            for caps in K8S_CONFIGMAP_REF_RE.captures_iter(doc) {
                let cm_name = caps.get(1).unwrap().as_str();
                let cm_qn = format!("{}.k8s.ConfigMap.{}", project, cm_name);
                buf.add_edge_by_qn(&resource_qn, &cm_qn, "MOUNTS_CONFIG", None);
            }

            // Extract Secret references and create MOUNTS_CONFIG edges
            for caps in K8S_SECRET_REF_RE.captures_iter(doc) {
                let secret_name = caps.get(1).unwrap().as_str();
                let secret_qn = format!("{}.k8s.Secret.{}", project, secret_name);
                buf.add_edge_by_qn(&resource_qn, &secret_qn, "MOUNTS_CONFIG", None);
            }
        }
    }
}

// ── Kustomize Parsing ────────────────────────────────

/// Regex to extract `resources:` list entries from kustomization.yaml.
#[allow(dead_code)]
static KUSTOMIZE_RESOURCE_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"(?m)^- (.+)$").unwrap());

/// Pass: Parse kustomization.yaml files and create Module nodes for overlays
/// with IMPORTS edges to base and overlay resources.
pub fn pass_kustomize(buf: &mut GraphBuffer, files: &[&DiscoveredFile], project: &str) {
    for f in files {
        if !matches!(f.language, Language::Kustomize) {
            continue;
        }

        let source = match std::fs::read_to_string(&f.abs_path) {
            Ok(s) => s,
            Err(_) => continue,
        };

        // Derive overlay name from directory
        let overlay_name = Path::new(&f.rel_path)
            .parent()
            .and_then(|p| p.file_name())
            .and_then(|n| n.to_str())
            .unwrap_or("kustomize");

        let module_qn = format!("{}.kustomize.{}", project, overlay_name);
        let props = serde_json::json!({
            "kind": "Kustomize",
            "overlay": overlay_name,
        });
        buf.add_node(
            "Module",
            overlay_name,
            &module_qn,
            &f.rel_path,
            0,
            0,
            Some(props.to_string()),
        );

        // Parse sections: resources, bases, patchesStrategicMerge, patchesJson6902, etc.
        // We look for list items under known section headers.
        let sections = [
            "resources:",
            "bases:",
            "patchesStrategicMerge:",
            "components:",
        ];
        let mut in_section = false;

        for line in source.lines() {
            let trimmed = line.trim();

            // Check if we're entering a known section
            if sections.contains(&trimmed) {
                in_section = true;
                continue;
            }

            // A non-indented, non-list line ends the current section
            if !trimmed.starts_with('-')
                && !trimmed.is_empty()
                && !line.starts_with(' ')
                && !line.starts_with('\t')
            {
                in_section = false;
                continue;
            }

            if in_section {
                if let Some(rest) = trimmed.strip_prefix("- ") {
                    let resource_path = rest.trim();
                    if resource_path.is_empty() {
                        continue;
                    }

                    // Resolve the resource path relative to the kustomization file's directory
                    let parent_dir = Path::new(&f.rel_path)
                        .parent()
                        .and_then(|p| p.to_str())
                        .unwrap_or("");
                    let resolved = if parent_dir.is_empty() {
                        resource_path.to_owned()
                    } else {
                        format!("{}/{}", parent_dir, resource_path)
                    };

                    // Create IMPORTS edge to the referenced resource
                    let target_qn = fqn::fqn_module(project, &resolved);
                    buf.add_edge_by_qn(&module_qn, &target_qn, "IMPORTS", None);
                }
            }
        }
    }
}

// ── Infrastructure Scanning: Dockerfiles and Helm ────

/// Regex for Dockerfile FROM directive: `FROM [--platform=...] <image> [AS <stage>]`
static DOCKERFILE_FROM_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"(?im)^FROM\s+(?:--platform=\S+\s+)?(\S+?)(?:\s+AS\s+(\S+))?$").unwrap()
});

/// Regex for Dockerfile COPY --from=<stage> directive
static DOCKERFILE_COPY_FROM_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"(?im)^COPY\s+--from=(\S+)\s+").unwrap());

/// Regex to detect Helm template syntax: `{{ ... }}`
static HELM_TEMPLATE_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"\{\{.*?\}\}").unwrap());

/// Pass: Parse Dockerfiles to create Docker_Image nodes with BUILDS_FROM/COPIES_FROM edges,
/// detect Helm chart templates, and create CONFIG_LINKS edges for config-env matches.
pub fn pass_infrascan(buf: &mut GraphBuffer, files: &[&DiscoveredFile], project: &str) {
    let mut images_seen: HashSet<String> = HashSet::new();

    for f in files {
        match f.language {
            Language::Dockerfile => {
                parse_dockerfile(buf, f, project, &mut images_seen);
            }
            Language::Yaml
                if f.rel_path.contains("/templates/") || f.rel_path.starts_with("templates/") =>
            {
                // Helm template: YAML file under templates/ with {{ }} syntax
                detect_helm_template(buf, f, project);
            }
            _ => {}
        }
    }
}

/// Parse a Dockerfile and create Docker_Image nodes, BUILDS_FROM edges for multi-stage builds,
/// and COPIES_FROM edges for COPY --from directives.
fn parse_dockerfile(
    buf: &mut GraphBuffer,
    f: &DiscoveredFile,
    project: &str,
    images_seen: &mut HashSet<String>,
) {
    let source = match std::fs::read_to_string(&f.abs_path) {
        Ok(s) => s,
        Err(_) => return,
    };

    // Track stages: stage_alias -> image_qn
    let mut stages: Vec<(Option<String>, String)> = Vec::new(); // (alias, image_qn)

    for caps in DOCKERFILE_FROM_RE.captures_iter(&source) {
        let image = caps.get(1).unwrap().as_str();
        let alias = caps.get(2).map(|m| m.as_str().to_owned());

        let image_qn = format!("{}.docker_image.{}", project, image);

        if images_seen.insert(image_qn.clone()) {
            buf.add_node(
                "Docker_Image",
                image,
                &image_qn,
                &f.rel_path,
                0,
                0,
                Some(serde_json::json!({ "image": image }).to_string()),
            );
        }

        // If this is not the first stage, create BUILDS_FROM edge from this image to the previous
        if !stages.is_empty() {
            let prev_qn = &stages.last().unwrap().1;
            buf.add_edge_by_qn(&image_qn, prev_qn, "BUILDS_FROM", None);
        }

        stages.push((alias, image_qn));
    }

    // Process COPY --from=<stage> directives
    for caps in DOCKERFILE_COPY_FROM_RE.captures_iter(&source) {
        let from_ref = caps.get(1).unwrap().as_str();

        // Resolve the stage reference: could be a stage alias or a numeric index
        let source_qn = if let Ok(idx) = from_ref.parse::<usize>() {
            stages.get(idx).map(|(_, qn)| qn.clone())
        } else {
            // Look up by alias
            stages
                .iter()
                .find(|(alias, _)| alias.as_deref() == Some(from_ref))
                .map(|(_, qn)| qn.clone())
        };

        if let Some(src_qn) = source_qn {
            // The COPIES_FROM edge goes from the current (last) stage to the referenced stage
            if let Some((_, current_qn)) = stages.last() {
                buf.add_edge_by_qn(current_qn, &src_qn, "COPIES_FROM", None);
            }
        }
    }
}

/// Detect Helm chart templates (YAML files in templates/ with {{ }} syntax)
/// and create Helm_Template nodes.
fn detect_helm_template(buf: &mut GraphBuffer, f: &DiscoveredFile, project: &str) {
    let source = match std::fs::read_to_string(&f.abs_path) {
        Ok(s) => s,
        Err(_) => return,
    };

    // Check if the file contains Helm template syntax
    if !HELM_TEMPLATE_RE.is_match(&source) {
        return;
    }

    let template_name = Path::new(&f.rel_path)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("template");

    let template_qn = format!("{}.helm_template.{}", project, template_name);
    let props = serde_json::json!({
        "kind": "Helm_Template",
        "template_file": f.rel_path,
    });

    buf.add_node(
        "Helm_Template",
        template_name,
        &template_qn,
        &f.rel_path,
        0,
        0,
        Some(props.to_string()),
    );
}

// ── Git History Integration ──────────────────────────
// Gated behind the `git-history` feature so these compile out when disabled.

#[cfg(feature = "git-history")]
mod git_passes {
    use codryn_graph_buffer::GraphBuffer;
    use codryn_store::Store;
    use std::collections::HashMap;
    use std::path::Path;

    /// Pass: Detect uncommitted changes (staged + unstaged) and create MODIFIED edges
    /// from changed file nodes to a GitDiff summary node.
    ///
    /// If the repo is not a git repository or git2 fails, logs a warning and returns.
    pub fn pass_gitdiff(buf: &mut GraphBuffer, project: &str, repo_path: &Path) {
        let repo = match git2::Repository::open(repo_path) {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!(error = %e, "pass_gitdiff: not a git repo or git2 failed, skipping");
                return;
            }
        };

        // Get the current index for diffing against the workdir
        let index = repo.index().ok();

        // Diff: index vs workdir (staged + unstaged changes)
        let diff = match repo.diff_index_to_workdir(
            index.as_ref(),
            Some(
                git2::DiffOptions::new()
                    .include_untracked(true)
                    .recurse_untracked_dirs(true),
            ),
        ) {
            Ok(d) => d,
            Err(e) => {
                tracing::warn!(error = %e, "pass_gitdiff: failed to compute diff, skipping");
                return;
            }
        };

        let mut changed_files: Vec<String> = Vec::new();
        diff.foreach(
            &mut |delta, _| {
                if let Some(path) = delta.new_file().path().and_then(|p| p.to_str()) {
                    changed_files.push(path.to_owned());
                } else if let Some(path) = delta.old_file().path().and_then(|p| p.to_str()) {
                    changed_files.push(path.to_owned());
                }
                true
            },
            None,
            None,
            None,
        )
        .ok();

        if changed_files.is_empty() {
            return;
        }

        // Create a GitDiff summary node
        let summary_qn = format!("{}.git_diff_summary", project);
        let props = serde_json::json!({
            "changed_file_count": changed_files.len(),
            "kind": "GitDiff",
        });
        buf.add_node(
            "GitDiff",
            "Uncommitted Changes",
            &summary_qn,
            "",
            0,
            0,
            Some(props.to_string()),
        );

        // Create MODIFIED edges from each changed file node to the summary
        for file_path in &changed_files {
            let file_qn = codryn_foundation::fqn::fqn_module(project, file_path);
            buf.add_edge_by_qn(&file_qn, &summary_qn, "MODIFIED", None);
        }

        tracing::info!(
            changed = changed_files.len(),
            "pass_gitdiff: detected uncommitted changes"
        );
    }

    /// Pass: Walk the last N commits, compute per-file change frequency, churn score,
    /// last commit info, recently_modified markers, and co-change edges.
    ///
    /// If the repo is not a git repository or git2 fails, logs a warning and returns.
    pub fn pass_githistory(
        buf: &mut GraphBuffer,
        store: &Store,
        project: &str,
        repo_path: &Path,
        max_commits: usize,
    ) {
        let repo = match git2::Repository::open(repo_path) {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!(error = %e, "pass_githistory: not a git repo or git2 failed, skipping");
                return;
            }
        };

        let mut revwalk = match repo.revwalk() {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!(error = %e, "pass_githistory: failed to create revwalk, skipping");
                return;
            }
        };

        if revwalk.push_head().is_err() {
            tracing::warn!("pass_githistory: no HEAD commit, skipping");
            return;
        }
        revwalk.set_sorting(git2::Sort::TIME).ok();

        // Per-file stats
        struct FileStats {
            change_frequency: u32,
            churn_score: u64, // additions + deletions
            last_commit_hash: String,
            last_commit_author: String,
        }

        let mut file_stats: HashMap<String, FileStats> = HashMap::new();
        // Per-file line ranges modified (to mark recently_modified on functions/classes)
        let mut file_modified_lines: HashMap<String, Vec<(i32, i32)>> = HashMap::new();
        // Co-change tracking: for each commit, which files were modified together
        let mut commit_file_sets: Vec<Vec<String>> = Vec::new();

        let mut commit_count = 0usize;

        for oid_result in revwalk {
            if commit_count >= max_commits {
                break;
            }
            if super::is_pass_cancelled() {
                return;
            }

            let oid = match oid_result {
                Ok(o) => o,
                Err(_) => continue,
            };

            let commit = match repo.find_commit(oid) {
                Ok(c) => c,
                Err(_) => continue,
            };

            let commit_hash = oid.to_string();
            let author_name = commit.author().name().unwrap_or("unknown").to_owned();

            let tree = match commit.tree() {
                Ok(t) => t,
                Err(_) => continue,
            };

            // Diff this commit against its first parent (or empty tree for root commit)
            let parent_tree = commit.parent(0).ok().and_then(|p| p.tree().ok());

            let diff = match repo.diff_tree_to_tree(parent_tree.as_ref(), Some(&tree), None) {
                Ok(d) => d,
                Err(_) => continue,
            };

            let mut files_in_commit: Vec<String> = Vec::new();

            // Collect per-file stats from this diff
            if let Ok(stats_result) = diff.stats() {
                let _ = stats_result; // We'll get per-file stats from deltas
            }

            // Iterate over deltas to get changed files
            let num_deltas = diff.deltas().len();
            for i in 0..num_deltas {
                let delta = diff.deltas().nth(i);
                if let Some(delta) = delta {
                    let file_path = delta
                        .new_file()
                        .path()
                        .and_then(|p| p.to_str())
                        .or_else(|| delta.old_file().path().and_then(|p| p.to_str()));

                    if let Some(path) = file_path {
                        let path = path.to_owned();
                        files_in_commit.push(path.clone());

                        let entry = file_stats.entry(path.clone()).or_insert(FileStats {
                            change_frequency: 0,
                            churn_score: 0,
                            last_commit_hash: String::new(),
                            last_commit_author: String::new(),
                        });
                        entry.change_frequency += 1;

                        // Store last commit info (first commit we see is the most recent due to TIME sort)
                        if entry.last_commit_hash.is_empty() {
                            entry.last_commit_hash = commit_hash.clone();
                            entry.last_commit_author = author_name.clone();
                        }
                    }
                }
            }

            // Compute per-file churn (additions + deletions) using patch hunks
            diff.foreach(
                &mut |_delta, _| true,
                None,
                Some(&mut |delta, hunk| {
                    let file_path = delta
                        .new_file()
                        .path()
                        .and_then(|p| p.to_str())
                        .or_else(|| delta.old_file().path().and_then(|p| p.to_str()));

                    if let Some(path) = file_path {
                        if let Some(entry) = file_stats.get_mut(path) {
                            entry.churn_score += hunk.new_lines() as u64 + hunk.old_lines() as u64;
                        }
                        // Track modified line ranges for recently_modified detection
                        let new_start = hunk.new_start() as i32;
                        let new_lines = hunk.new_lines() as i32;
                        if new_lines > 0 {
                            file_modified_lines
                                .entry(path.to_owned())
                                .or_default()
                                .push((new_start, new_start + new_lines - 1));
                        }
                    }
                    true
                }),
                None,
            )
            .ok();

            if !files_in_commit.is_empty() {
                commit_file_sets.push(files_in_commit);
            }

            commit_count += 1;
        }

        // Update File node properties with git stats
        let all_nodes = store.get_all_nodes(project).unwrap_or_default();
        let file_nodes: Vec<&codryn_store::Node> =
            all_nodes.iter().filter(|n| n.label == "File").collect();

        for node in &file_nodes {
            if let Some(stats) = file_stats.get(&node.file_path) {
                let mut props: serde_json::Value = node
                    .properties_json
                    .as_deref()
                    .and_then(|p| serde_json::from_str(p).ok())
                    .unwrap_or_else(|| serde_json::json!({}));

                props["change_frequency"] = serde_json::json!(stats.change_frequency);
                props["churn_score"] = serde_json::json!(stats.churn_score);
                props["last_commit_hash"] = serde_json::json!(stats.last_commit_hash);
                props["last_commit_author"] = serde_json::json!(stats.last_commit_author);

                let _ = store.update_node_properties(node.id, &props.to_string());
            }
        }

        // Mark functions/classes modified within the last N commits with recently_modified
        let symbol_nodes: Vec<&codryn_store::Node> = all_nodes
            .iter()
            .filter(|n| {
                matches!(
                    n.label.as_str(),
                    "Function" | "Method" | "Class" | "Interface"
                )
            })
            .collect();

        for node in &symbol_nodes {
            if let Some(ranges) = file_modified_lines.get(&node.file_path) {
                let overlaps = ranges
                    .iter()
                    .any(|(start, end)| node.start_line <= *end && node.end_line >= *start);
                if overlaps {
                    let mut props: serde_json::Value = node
                        .properties_json
                        .as_deref()
                        .and_then(|p| serde_json::from_str(p).ok())
                        .unwrap_or_else(|| serde_json::json!({}));

                    props["recently_modified"] = serde_json::json!(true);
                    let _ = store.update_node_properties(node.id, &props.to_string());
                }
            }
        }

        // Co-change edge detection: count how often each pair of files appears in the same commit
        let mut co_change_counts: HashMap<(String, String), u32> = HashMap::new();
        for files in &commit_file_sets {
            let mut sorted: Vec<&String> = files.iter().collect();
            sorted.sort();
            sorted.dedup();
            for i in 0..sorted.len() {
                for j in (i + 1)..sorted.len() {
                    let key = (sorted[i].clone(), sorted[j].clone());
                    *co_change_counts.entry(key).or_insert(0) += 1;
                }
            }
        }

        // Create CO_CHANGED edges for pairs with >= 3 co-changes
        let co_change_threshold = 3u32;
        for ((file_a, file_b), count) in &co_change_counts {
            if *count >= co_change_threshold {
                let qn_a = codryn_foundation::fqn::fqn_module(project, file_a);
                let qn_b = codryn_foundation::fqn::fqn_module(project, file_b);
                let props = serde_json::json!({ "co_change_count": count });
                buf.add_edge_by_qn(&qn_a, &qn_b, "CO_CHANGED", Some(props.to_string()));
            }
        }

        tracing::info!(
            files_tracked = file_stats.len(),
            commits_analyzed = commit_count,
            co_change_edges = co_change_counts
                .values()
                .filter(|c| **c >= co_change_threshold)
                .count(),
            "pass_githistory: complete"
        );
    }
}

// Re-export git pass functions at module level (feature-gated)
#[cfg(feature = "git-history")]
pub use git_passes::{pass_gitdiff, pass_githistory};

// ── Compile Commands Support ──────────────────────────

/// Per-file compile context extracted from compile_commands.json.
#[derive(Debug, Clone, Default)]
pub struct CompileContext {
    pub include_paths: Vec<String>,
    pub defines: Vec<(String, Option<String>)>, // (name, optional value)
    pub std_flag: Option<String>,               // e.g., "c++17", "c11"
}

/// Map from relative file path to its CompileContext.
pub type CompileCommandsMap = HashMap<String, CompileContext>;

/// Parse compile_commands.json and build a CompileCommandsMap.
///
/// Returns an empty map if the file doesn't exist or can't be parsed.
pub fn pass_compile_commands(repo_path: &Path) -> CompileCommandsMap {
    let cc_path = repo_path.join("compile_commands.json");
    if !cc_path.exists() {
        return CompileCommandsMap::new();
    }

    let content = match std::fs::read_to_string(&cc_path) {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(error = %e, "failed to read compile_commands.json");
            return CompileCommandsMap::new();
        }
    };

    let entries: Vec<serde_json::Value> = match serde_json::from_str(&content) {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(error = %e, "failed to parse compile_commands.json");
            return CompileCommandsMap::new();
        }
    };

    let mut map = CompileCommandsMap::new();
    for entry in &entries {
        let file = entry.get("file").and_then(|f| f.as_str()).unwrap_or("");
        if file.is_empty() {
            continue;
        }
        let rel_path = Path::new(file)
            .strip_prefix(repo_path)
            .unwrap_or(Path::new(file))
            .to_string_lossy()
            .replace('\\', "/");

        let args = extract_arguments(entry);
        let ctx = parse_compile_args(&args);
        map.insert(rel_path, ctx);
    }
    map
}

/// Extract compiler arguments from a compile_commands.json entry.
///
/// Prefers the `arguments` array field. Falls back to splitting the `command`
/// string on whitespace.
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

// ── Type Reference Extraction ─────────────────────────

/// Create TYPE_REF edges from functions/methods to the types they reference
/// in parameters and return types.
///
/// For each Function and Method node, extracts type names from the `parameters`
/// and `return_type` fields in the node's properties JSON. Types are resolved
/// against the Registry, and edges are created only when the referenced type
/// exists as a node and is not a standard library type.
pub fn pass_type_refs(buf: &mut GraphBuffer, reg: &Registry, store: &Store, project: &str) {
    let functions = store
        .get_nodes_by_label(project, "Function", 50_000)
        .unwrap_or_default();
    let methods = store
        .get_nodes_by_label(project, "Method", 50_000)
        .unwrap_or_default();

    let mut seen_edges: HashSet<(String, String)> = HashSet::new();

    for node in functions.iter().chain(methods.iter()) {
        let props: serde_json::Value = node
            .properties_json
            .as_deref()
            .and_then(|s| serde_json::from_str(s).ok())
            .unwrap_or_default();

        let lang = codryn_discover::detect_language(&node.file_path);

        // Extract type refs from parameters
        if let Some(params) = props.get("parameters").and_then(|p| p.as_array()) {
            for param in params {
                if let Some(type_name) = param.get("type").and_then(|t| t.as_str()) {
                    let base_type = strip_generic_params(type_name);
                    if base_type.is_empty() {
                        continue;
                    }
                    if crate::registry::is_stdlib_type(lang, base_type) {
                        continue;
                    }
                    if let Some(target_qn) = resolve_type_to_node(reg, base_type) {
                        let key = (node.qualified_name.clone(), target_qn.clone());
                        if seen_edges.insert(key) {
                            buf.add_edge_by_qn(&node.qualified_name, &target_qn, "TYPE_REF", None);
                        }
                    }
                }
            }
        }

        // Extract type ref from return type
        if let Some(ret_type) = props.get("return_type").and_then(|r| r.as_str()) {
            let base_type = strip_generic_params(ret_type);
            if base_type.is_empty() {
                continue;
            }
            if crate::registry::is_stdlib_type(lang, base_type) {
                continue;
            }
            if let Some(target_qn) = resolve_type_to_node(reg, base_type) {
                let key = (node.qualified_name.clone(), target_qn.clone());
                if seen_edges.insert(key) {
                    buf.add_edge_by_qn(&node.qualified_name, &target_qn, "TYPE_REF", None);
                }
            }
        }
    }
}

/// Strip generic type parameters from a type name.
/// E.g., "Vec<String>" → "Vec", "HashMap<K, V>" → "HashMap", "MyType" → "MyType"
fn strip_generic_params(type_name: &str) -> &str {
    type_name.split('<').next().unwrap_or(type_name).trim()
}

/// Resolve a type name to a node's qualified name via the Registry.
/// Prefers Class/Interface/Struct/Trait labels over other node types.
fn resolve_type_to_node(reg: &Registry, type_name: &str) -> Option<String> {
    let entries = reg.lookup(type_name);
    if entries.is_empty() {
        return None;
    }
    // Prefer Class/Interface/Struct/Trait labels
    entries
        .iter()
        .find(|e| matches!(e.label.as_str(), "Class" | "Interface" | "Struct" | "Trait"))
        .or_else(|| entries.first())
        .map(|e| e.qualified_name.clone())
}

// ── Service Pattern Classification ────────────────────────────────────────

/// Known HTTP client library identifiers.
const HTTP_CLIENT_LIBS: &[&str] = &[
    "requests",
    "axios",
    "httpx",
    "aiohttp",
    "urllib",
    "superagent",
    "node-fetch",
    "undici",
    "got",
    "fetch",
    "http.client",
    "reqwest",
    "hyper",
    "surf",
];

/// Known async broker library identifiers.
const ASYNC_BROKER_LIBS: &[&str] = &[
    "kafka", "amqp", "rabbitmq", "redis", "celery", "bull", "nats", "pulsar", "lapin", "rdkafka",
];

/// Known config library identifiers.
const CONFIG_LIBS: &[&str] = &[
    "dotenv",
    "config",
    "viper",
    "configparser",
    "pydantic_settings",
    "figment",
    "envy",
];

/// HTTP method suffixes for two-level matching.
const HTTP_METHOD_SUFFIXES: &[(&str, &str)] = &[
    ("get", "GET"),
    ("post", "POST"),
    ("put", "PUT"),
    ("delete", "DELETE"),
    ("patch", "PATCH"),
    ("head", "HEAD"),
    ("options", "OPTIONS"),
    ("request", "UNKNOWN"),
];

/// Classify CALLS edges by service pattern.
/// Adds typed edges (HTTP_CALLS, ASYNC_CALLS, CONFIGURES) alongside original CALLS.
/// The original CALLS edge is preserved — the typed edge is an additional relationship.
pub fn pass_service_patterns(buf: &mut GraphBuffer, store: &Store, project: &str) {
    let calls_edges = store
        .get_edges_by_type(project, "CALLS")
        .unwrap_or_default();
    for edge in &calls_edges {
        let target = match store.get_node_by_id(edge.target_id) {
            Ok(Some(n)) => n,
            _ => continue,
        };
        let qn_lower = target.qualified_name.to_lowercase();

        if let Some(lib) = HTTP_CLIENT_LIBS.iter().find(|l| qn_lower.contains(*l)) {
            let method = detect_http_method(&target.name);
            let props = serde_json::json!({
                "http_method": method,
                "library": *lib,
            })
            .to_string();
            buf.add_edge(edge.source_id, edge.target_id, "HTTP_CALLS", Some(props));
        } else if ASYNC_BROKER_LIBS.iter().any(|l| qn_lower.contains(l)) {
            buf.add_edge(edge.source_id, edge.target_id, "ASYNC_CALLS", None);
        } else if CONFIG_LIBS.iter().any(|l| qn_lower.contains(l)) {
            buf.add_edge(edge.source_id, edge.target_id, "CONFIGURES", None);
        }
    }
}

/// Detect the HTTP method from a method/function name by matching known suffixes.
pub fn detect_http_method(method_name: &str) -> &'static str {
    let lower = method_name.to_lowercase();
    HTTP_METHOD_SUFFIXES
        .iter()
        .find(|(suffix, _)| lower.ends_with(suffix) || lower.contains(suffix))
        .map(|(_, method)| *method)
        .unwrap_or("UNKNOWN")
}

/// Parse compiler arguments and extract include paths, defines, and std flags.
fn parse_compile_args(args: &[String]) -> CompileContext {
    let mut ctx = CompileContext::default();
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
                ctx.include_paths.push(path.to_string());
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
                    ctx.defines.push((name, value));
                }
            }
        } else if let Some(stripped) = arg.strip_prefix("-std=") {
            ctx.std_flag = Some(stripped.to_string());
        }
    }
    ctx
}

// ── Semantic Edge Pass v2: INHERITS, DECORATES, IMPLEMENTS ────────────────

/// Resolve a type name to a node's qualified name via the Registry.
/// Prefers Class/Interface/Trait/Struct/Protocol/Module labels.
/// Returns `(qualified_name, label)` so callers can distinguish INHERITS vs IMPLEMENTS.
fn resolve_type_target(reg: &Registry, type_name: &str) -> Option<(String, String)> {
    // Strip generic parameters: "List<String>" -> "List"
    let base_type = type_name.split('<').next().unwrap_or(type_name).trim();
    if base_type.is_empty() {
        return None;
    }
    let entries = reg.lookup(base_type);
    if entries.is_empty() {
        return None;
    }
    // Prefer Class/Interface/Struct/Trait/Protocol/Module labels
    let preferred = entries.iter().find(|e| {
        matches!(
            e.label.as_str(),
            "Class" | "Interface" | "Struct" | "Trait" | "Protocol" | "Module"
        )
    });
    let entry = preferred.or_else(|| entries.first())?;
    Some((entry.qualified_name.clone(), entry.label.clone()))
}

/// Strip `@` prefix and parentheses from a decorator name.
/// E.g., `@Component({...})` → `Component`, `@override` → `override`, `@Test` → `Test`
fn normalize_decorator_name(raw: &str) -> &str {
    let s = raw.strip_prefix('@').unwrap_or(raw);
    // Strip everything from the first `(` onward
    s.split('(').next().unwrap_or(s).trim()
}

/// Semantic edge pass v2: create INHERITS, DECORATES, and IMPLEMENTS edges
/// by reading `base_classes`, `decorators`, and impl/trait data from node properties.
///
/// This pass queries all nodes for the project from the Store, then:
/// - For nodes with `base_classes`: creates INHERITS or IMPLEMENTS edges depending
///   on whether the target resolves to a Class (INHERITS) or Interface/Trait (IMPLEMENTS).
/// - For Rust `Impl` nodes with `base_classes`: creates IMPLEMENTS edges (trait impl).
/// - For nodes with `decorators`: creates DECORATES edges from the decorator to the
///   decorated symbol.
///
/// Only creates edges when the target resolves to an existing node in the Registry.
pub fn pass_semantic_edges_v2(buf: &mut GraphBuffer, reg: &Registry, store: &Store, project: &str) {
    let all_nodes = store.get_all_nodes(project).unwrap_or_default();
    let mut seen_edges: HashSet<(String, String, String)> = HashSet::new();

    for node in &all_nodes {
        let props: serde_json::Value = node
            .properties_json
            .as_deref()
            .and_then(|s| serde_json::from_str(s).ok())
            .unwrap_or_default();

        // ── INHERITS / IMPLEMENTS from base_classes ──────────────────
        if let Some(bases) = props.get("base_classes").and_then(|b| b.as_array()) {
            for base_val in bases {
                let base_name = match base_val.as_str() {
                    Some(s) => s,
                    None => continue,
                };

                if let Some((target_qn, target_label)) = resolve_type_target(reg, base_name) {
                    // Determine edge type based on source and target labels
                    let edge_type = if node.label == "Impl" {
                        // Rust: `impl Trait for Struct` → IMPLEMENTS
                        "IMPLEMENTS"
                    } else if matches!(target_label.as_str(), "Interface" | "Trait" | "Protocol") {
                        "IMPLEMENTS"
                    } else {
                        "INHERITS"
                    };

                    let key = (
                        node.qualified_name.clone(),
                        target_qn.clone(),
                        edge_type.to_string(),
                    );
                    if seen_edges.insert(key) {
                        buf.add_edge_by_qn(&node.qualified_name, &target_qn, edge_type, None);
                    }
                }
            }
        }

        // ── DECORATES from decorators ────────────────────────────────
        if let Some(decorators) = props.get("decorators").and_then(|d| d.as_array()) {
            for dec_val in decorators {
                let raw_name = match dec_val.as_str() {
                    Some(s) => s,
                    None => continue,
                };

                let dec_name = normalize_decorator_name(raw_name);
                if dec_name.is_empty() {
                    continue;
                }

                // Try to resolve the decorator to a node in the registry
                if let Some((target_qn, _)) = resolve_type_target(reg, dec_name) {
                    let key = (
                        target_qn.clone(),
                        node.qualified_name.clone(),
                        "DECORATES".to_string(),
                    );
                    if seen_edges.insert(key) {
                        // Edge direction: decorator → decorated symbol
                        buf.add_edge_by_qn(&target_qn, &node.qualified_name, "DECORATES", None);
                    }
                }
            }
        }
    }
}

// ── Cross-Repo Intelligence ──────────────────────────

/// Normalize a route path pattern so that different parameter syntaxes match.
/// E.g., `/users/:id` and `/users/{id}` both become `/users/{param}`.
fn normalize_route_path(path: &str) -> String {
    static PARAM_COLON_RE: LazyLock<regex::Regex> =
        LazyLock::new(|| regex::Regex::new(r":(\w+)").unwrap());
    static PARAM_BRACE_RE: LazyLock<regex::Regex> =
        LazyLock::new(|| regex::Regex::new(r"\{(\w+)\}").unwrap());

    let normalized = PARAM_COLON_RE.replace_all(path, "{param}");
    let normalized = PARAM_BRACE_RE.replace_all(&normalized, "{param}");
    normalized.to_string()
}

/// Execute cross-repo intelligence pass for a project.
/// Only runs when the project has at least one linked project.
/// Detects cross-project communication patterns (HTTP routes, channels, async topics)
/// and creates typed CROSS_* edges between linked projects.
pub fn pass_cross_repo(buf: &mut GraphBuffer, store: &Store, project: &str) {
    let links = store.get_linked_projects(project).unwrap_or_default();
    if links.is_empty() {
        return;
    }

    // Remove stale CROSS_* edges for this project
    if let Err(e) = store.delete_edges_by_type_prefix(project, "CROSS_") {
        tracing::warn!(error = %e, "pass_cross_repo: failed to delete stale CROSS_ edges");
    }

    for link in &links {
        let linked_project = &link.target_project;

        // 1. Match HTTP routes
        match_cross_http_routes(buf, store, project, linked_project);

        // 2. Match channels
        match_cross_channels(buf, store, project, linked_project);

        // 3. Match async topics
        match_cross_async_topics(buf, store, project, linked_project);
    }
}

/// Match Route nodes across two projects by HTTP method + normalized path pattern.
/// Creates CROSS_HTTP edges bidirectionally.
fn match_cross_http_routes(buf: &mut GraphBuffer, store: &Store, project_a: &str, project_b: &str) {
    let routes_a = store
        .get_nodes_by_label(project_a, "Route", 10_000)
        .unwrap_or_default();
    let routes_b = store
        .get_nodes_by_label(project_b, "Route", 10_000)
        .unwrap_or_default();

    if routes_a.is_empty() || routes_b.is_empty() {
        return;
    }

    // Build a lookup map for project B routes: (method, normalized_path) -> Vec<Node>
    let mut b_route_map: HashMap<(String, String), Vec<&Node>> = HashMap::new();
    for route in &routes_b {
        if let Some(ref props_str) = route.properties_json {
            if let Ok(props) = serde_json::from_str::<serde_json::Value>(props_str) {
                let method = props
                    .get("http_method")
                    .and_then(|m| m.as_str())
                    .unwrap_or("")
                    .to_uppercase();
                let path = props.get("path").and_then(|p| p.as_str()).unwrap_or("");
                if !method.is_empty() && !path.is_empty() {
                    let normalized = normalize_route_path(path);
                    b_route_map
                        .entry((method, normalized))
                        .or_default()
                        .push(route);
                }
            }
        }
    }

    // Match project A routes against project B
    for route_a in &routes_a {
        if let Some(ref props_str) = route_a.properties_json {
            if let Ok(props) = serde_json::from_str::<serde_json::Value>(props_str) {
                let method = props
                    .get("http_method")
                    .and_then(|m| m.as_str())
                    .unwrap_or("")
                    .to_uppercase();
                let path = props.get("path").and_then(|p| p.as_str()).unwrap_or("");
                if method.is_empty() || path.is_empty() {
                    continue;
                }
                let normalized = normalize_route_path(path);
                let key = (method, normalized);

                if let Some(matching_routes) = b_route_map.get(&key) {
                    for route_b in matching_routes {
                        let props_json = serde_json::json!({
                            "source_project": project_a,
                            "target_project": project_b,
                            "match_type": "http_route",
                        })
                        .to_string();

                        // Bidirectional: A -> B and B -> A
                        buf.add_edge(
                            route_a.id,
                            route_b.id,
                            "CROSS_HTTP",
                            Some(props_json.clone()),
                        );
                        buf.add_edge(route_b.id, route_a.id, "CROSS_HTTP", Some(props_json));
                    }
                }
            }
        }
    }
}

/// Match Channel nodes across two projects by event name.
/// Creates CROSS_CHANNEL edges bidirectionally.
fn match_cross_channels(buf: &mut GraphBuffer, store: &Store, project_a: &str, project_b: &str) {
    let channels_a = store
        .get_nodes_by_label(project_a, "Channel", 10_000)
        .unwrap_or_default();
    let channels_b = store
        .get_nodes_by_label(project_b, "Channel", 10_000)
        .unwrap_or_default();

    if channels_a.is_empty() || channels_b.is_empty() {
        return;
    }

    // Build a lookup map for project B channels: event_name -> Vec<&Node>
    let mut b_channel_map: HashMap<String, Vec<&Node>> = HashMap::new();
    for ch in &channels_b {
        let event_name = extract_event_name(ch);
        if !event_name.is_empty() {
            b_channel_map.entry(event_name).or_default().push(ch);
        }
    }

    // Match project A channels against project B
    for ch_a in &channels_a {
        let event_name = extract_event_name(ch_a);
        if event_name.is_empty() {
            continue;
        }

        if let Some(matching_channels) = b_channel_map.get(&event_name) {
            for ch_b in matching_channels {
                let props_json = serde_json::json!({
                    "source_project": project_a,
                    "target_project": project_b,
                    "match_type": "channel",
                    "event_name": event_name,
                })
                .to_string();

                // Bidirectional
                buf.add_edge(ch_a.id, ch_b.id, "CROSS_CHANNEL", Some(props_json.clone()));
                buf.add_edge(ch_b.id, ch_a.id, "CROSS_CHANNEL", Some(props_json));
            }
        }
    }
}

/// Extract event name from a Channel node's properties or name.
fn extract_event_name(node: &codryn_store::Node) -> String {
    if let Some(ref props_str) = node.properties_json {
        if let Ok(props) = serde_json::from_str::<serde_json::Value>(props_str) {
            if let Some(name) = props.get("event_name").and_then(|n| n.as_str()) {
                return name.to_string();
            }
        }
    }
    // Fall back to node name
    node.name.clone()
}

/// Match nodes with async topic properties across two projects.
/// Looks for nodes with kafka_topic, rabbitmq_queue, or redis_channel properties.
/// Creates CROSS_ASYNC edges bidirectionally.
fn match_cross_async_topics(
    buf: &mut GraphBuffer,
    store: &Store,
    project_a: &str,
    project_b: &str,
) {
    // Collect nodes with async topic properties from both projects
    let topics_a = collect_async_topic_nodes(store, project_a);
    let topics_b = collect_async_topic_nodes(store, project_b);

    if topics_a.is_empty() || topics_b.is_empty() {
        return;
    }

    // Build a lookup map for project B: topic_name -> Vec<(node_id, topic_type)>
    let mut b_topic_map: HashMap<String, Vec<(i64, String)>> = HashMap::new();
    for (node_id, topic_name, topic_type) in &topics_b {
        b_topic_map
            .entry(topic_name.clone())
            .or_default()
            .push((*node_id, topic_type.clone()));
    }

    // Match project A topics against project B
    for (node_id_a, topic_name, topic_type_a) in &topics_a {
        if let Some(matches) = b_topic_map.get(topic_name) {
            for (node_id_b, topic_type_b) in matches {
                // Only match same topic type (kafka-kafka, rabbitmq-rabbitmq, etc.)
                if topic_type_a != topic_type_b {
                    continue;
                }

                let props_json = serde_json::json!({
                    "source_project": project_a,
                    "target_project": project_b,
                    "match_type": "async_topic",
                    "topic_name": topic_name,
                    "topic_type": topic_type_a,
                })
                .to_string();

                // Bidirectional
                buf.add_edge(
                    *node_id_a,
                    *node_id_b,
                    "CROSS_ASYNC",
                    Some(props_json.clone()),
                );
                buf.add_edge(*node_id_b, *node_id_a, "CROSS_ASYNC", Some(props_json));
            }
        }
    }
}

/// Collect nodes that have async topic properties (kafka_topic, rabbitmq_queue, redis_channel).
/// Returns Vec<(node_id, topic_name, topic_type)>.
fn collect_async_topic_nodes(store: &Store, project: &str) -> Vec<(i64, String, String)> {
    let mut results = Vec::new();

    // Check Channel nodes for async topic properties
    let channels = store
        .get_nodes_by_label(project, "Channel", 10_000)
        .unwrap_or_default();

    for node in &channels {
        if let Some(ref props_str) = node.properties_json {
            if let Ok(props) = serde_json::from_str::<serde_json::Value>(props_str) {
                for (key, topic_type) in &[
                    ("kafka_topic", "kafka"),
                    ("rabbitmq_queue", "rabbitmq"),
                    ("redis_channel", "redis"),
                ] {
                    if let Some(topic_name) = props.get(*key).and_then(|v| v.as_str()) {
                        results.push((node.id, topic_name.to_string(), topic_type.to_string()));
                    }
                }
            }
        }
    }

    // Also check Function/Method nodes that might have async topic properties
    for label in &["Function", "Method"] {
        let nodes = store
            .get_nodes_by_label(project, label, 50_000)
            .unwrap_or_default();
        for node in &nodes {
            if let Some(ref props_str) = node.properties_json {
                if let Ok(props) = serde_json::from_str::<serde_json::Value>(props_str) {
                    for (key, topic_type) in &[
                        ("kafka_topic", "kafka"),
                        ("rabbitmq_queue", "rabbitmq"),
                        ("redis_channel", "redis"),
                    ] {
                        if let Some(topic_name) = props.get(*key).and_then(|v| v.as_str()) {
                            results.push((node.id, topic_name.to_string(), topic_type.to_string()));
                        }
                    }
                }
            }
        }
    }

    results
}

// ── CI/CD Pipeline Parsing ────────────────────────────────────────

/// Regex for Jenkinsfile stage declarations: stage('name') or stage("name")
static JENKINSFILE_STAGE_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r#"stage\s*\(\s*['"]([^'"]+)['"]\s*\)"#).unwrap());

/// Detect which CI/CD system a file belongs to based on its relative path.
/// Returns `None` if the file is not a recognized CI/CD configuration file.
fn detect_ci_system(rel_path: &str) -> Option<&'static str> {
    let filename = rel_path.rsplit('/').next().unwrap_or(rel_path);

    if rel_path == ".gitlab-ci.yml" {
        return Some("gitlab");
    }
    if rel_path.starts_with(".github/workflows/")
        && (rel_path.ends_with(".yml") || rel_path.ends_with(".yaml"))
    {
        return Some("github");
    }
    if filename == "Jenkinsfile" || filename.ends_with(".jenkinsfile") {
        return Some("jenkins");
    }
    if rel_path == ".circleci/config.yml" {
        return Some("circleci");
    }
    if rel_path == "azure-pipelines.yml"
        || (rel_path.starts_with(".azure-pipelines/")
            && (rel_path.ends_with(".yml") || rel_path.ends_with(".yaml")))
    {
        return Some("azure");
    }
    if rel_path == "bitbucket-pipelines.yml" {
        return Some("bitbucket");
    }
    None
}

/// Derive a pipeline name from the file path.
fn pipeline_name_from_path(rel_path: &str) -> String {
    Path::new(rel_path)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("pipeline")
        .to_owned()
}

/// Detect deployment commands in a script line and return edge type labels.
fn detect_deploy_commands(line: &str) -> Vec<&'static str> {
    let mut edges = Vec::new();
    let trimmed = line.trim();
    if trimmed.contains("terraform apply") || trimmed.contains("terraform plan") {
        edges.push("DEPLOYS");
    }
    if trimmed.contains("kubectl apply") {
        edges.push("DEPLOYS");
    }
    if trimmed.contains("helm install") || trimmed.contains("helm upgrade") {
        edges.push("DEPLOYS");
    }
    if trimmed.contains("docker build") || trimmed.contains("docker push") {
        edges.push("BUILDS_IMAGE");
    }
    edges
}

/// Collect all script lines from a YAML value (handles both string and sequence).
fn collect_script_lines(val: &serde_yaml::Value) -> Vec<String> {
    match val {
        serde_yaml::Value::String(s) => s.lines().map(|l| l.to_owned()).collect(),
        serde_yaml::Value::Sequence(seq) => seq
            .iter()
            .filter_map(|v| v.as_str().map(|s| s.to_owned()))
            .collect(),
        _ => vec![],
    }
}

/// Get a one-line script summary from script lines.
fn script_summary(lines: &[String]) -> String {
    if lines.is_empty() {
        return String::new();
    }
    let first = lines[0].trim();
    if first.len() > 80 {
        format!("{}...", &first[..77])
    } else {
        first.to_owned()
    }
}

/// Create deployment/build edges for a job based on its script content.
fn create_deploy_edges(
    buf: &mut GraphBuffer,
    job_qn: &str,
    project: &str,
    script_lines: &[String],
) {
    let mut seen_edges: HashSet<&'static str> = HashSet::new();
    for line in script_lines {
        for edge_type in detect_deploy_commands(line) {
            if seen_edges.insert(edge_type) {
                // Create a generic target node for the deployment/build target
                let target_qn = if edge_type == "DEPLOYS" {
                    format!("{}.infra.deploy_target", project)
                } else {
                    format!("{}.infra.docker_target", project)
                };
                buf.add_edge_by_qn(job_qn, &target_qn, edge_type, None);
            }
        }
    }
}

/// Pass: Parse CI/CD pipeline files and create Pipeline, Stage, Job nodes
/// with BELONGS_TO_STAGE, NEXT_STAGE, DEPENDS_ON, DEPLOYS, BUILDS_IMAGE edges.
pub fn pass_pipelines(buf: &mut GraphBuffer, files: &[&DiscoveredFile], project: &str) {
    for f in files {
        if is_pass_cancelled() {
            return;
        }

        let ci_system = match detect_ci_system(&f.rel_path) {
            Some(s) => s,
            None => continue,
        };

        let source = match std::fs::read_to_string(&f.abs_path) {
            Ok(s) => s,
            Err(_) => continue,
        };

        match ci_system {
            "gitlab" => parse_gitlab_ci(buf, &source, &f.rel_path, project),
            "github" => parse_github_actions(buf, &source, &f.rel_path, project),
            "jenkins" => parse_jenkinsfile(buf, &source, &f.rel_path, project),
            "circleci" => parse_circleci(buf, &source, &f.rel_path, project),
            "azure" => parse_azure_pipelines(buf, &source, &f.rel_path, project),
            "bitbucket" => parse_bitbucket_pipelines(buf, &source, &f.rel_path, project),
            _ => {}
        }
    }
}

/// Parse a GitLab CI YAML file into Pipeline, Stage, and Job nodes.
fn parse_gitlab_ci(buf: &mut GraphBuffer, source: &str, rel_path: &str, project: &str) {
    let doc: serde_yaml::Value = match serde_yaml::from_str(source) {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(file = rel_path, error = %e, "pass_pipelines: failed to parse GitLab CI YAML");
            return;
        }
    };

    let mapping = match doc.as_mapping() {
        Some(m) => m,
        None => return,
    };

    let pipeline_name = pipeline_name_from_path(rel_path);
    let pipeline_qn = format!("{}.pipeline.gitlab.{}", project, pipeline_name);

    // Extract triggers from top-level keys (workflow:rules, only, etc.)
    let triggers: Vec<String> = Vec::new();
    let props = serde_json::json!({
        "ci_system": "gitlab",
        "triggers": triggers,
    });
    buf.add_node(
        "Pipeline",
        &pipeline_name,
        &pipeline_qn,
        rel_path,
        0,
        0,
        Some(props.to_string()),
    );

    // Extract stages
    let stages: Vec<String> = mapping
        .get(serde_yaml::Value::String("stages".into()))
        .and_then(|v| v.as_sequence())
        .map(|seq| {
            seq.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_owned()))
                .collect()
        })
        .unwrap_or_default();

    // Create Stage nodes and NEXT_STAGE edges
    let mut prev_stage_qn: Option<String> = None;
    for (i, stage_name) in stages.iter().enumerate() {
        let stage_qn = format!(
            "{}.pipeline.gitlab.{}.stage.{}",
            project, pipeline_name, stage_name
        );
        let stage_props = serde_json::json!({
            "pipeline_name": pipeline_name,
            "order": i,
        });
        buf.add_node(
            "Stage",
            stage_name,
            &stage_qn,
            rel_path,
            0,
            0,
            Some(stage_props.to_string()),
        );

        if let Some(ref prev_qn) = prev_stage_qn {
            buf.add_edge_by_qn(prev_qn, &stage_qn, "NEXT_STAGE", None);
        }
        prev_stage_qn = Some(stage_qn);
    }

    // GitLab CI reserved keys that are not jobs
    let reserved_keys: HashSet<&str> = [
        "stages",
        "variables",
        "image",
        "services",
        "before_script",
        "after_script",
        "cache",
        "include",
        "default",
        "workflow",
        "pages",
    ]
    .iter()
    .copied()
    .collect();

    // Extract jobs (top-level keys that are not reserved and not starting with '.')
    for (key, value) in mapping {
        let job_name = match key.as_str() {
            Some(s) => s,
            None => continue,
        };

        if reserved_keys.contains(job_name) || job_name.starts_with('.') {
            continue;
        }

        let job_map = match value.as_mapping() {
            Some(m) => m,
            None => continue,
        };

        let stage = job_map
            .get(serde_yaml::Value::String("stage".into()))
            .and_then(|v| v.as_str())
            .unwrap_or("test")
            .to_owned();

        let image = job_map
            .get(serde_yaml::Value::String("image".into()))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_owned();

        // Collect script lines from script, before_script, after_script
        let mut all_script_lines = Vec::new();
        for script_key in &["before_script", "script", "after_script"] {
            if let Some(val) = job_map.get(serde_yaml::Value::String((*script_key).into())) {
                all_script_lines.extend(collect_script_lines(val));
            }
        }

        let summary = script_summary(&all_script_lines);

        let job_qn = format!(
            "{}.pipeline.gitlab.{}.job.{}",
            project, pipeline_name, job_name
        );
        let job_props = serde_json::json!({
            "pipeline_name": pipeline_name,
            "stage": stage,
            "image": image,
            "script_summary": summary,
        });
        buf.add_node(
            "Job",
            job_name,
            &job_qn,
            rel_path,
            0,
            0,
            Some(job_props.to_string()),
        );

        // BELONGS_TO_STAGE edge
        let stage_qn = format!(
            "{}.pipeline.gitlab.{}.stage.{}",
            project, pipeline_name, stage
        );
        buf.add_edge_by_qn(&job_qn, &stage_qn, "BELONGS_TO_STAGE", None);

        // DEPENDS_ON edges from needs
        if let Some(needs) = job_map.get(serde_yaml::Value::String("needs".into())) {
            let need_names: Vec<String> = match needs {
                serde_yaml::Value::Sequence(seq) => seq
                    .iter()
                    .filter_map(|v| match v {
                        serde_yaml::Value::String(s) => Some(s.clone()),
                        serde_yaml::Value::Mapping(m) => m
                            .get(serde_yaml::Value::String("job".into()))
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_owned()),
                        _ => None,
                    })
                    .collect(),
                _ => vec![],
            };
            for need in &need_names {
                let need_qn = format!("{}.pipeline.gitlab.{}.job.{}", project, pipeline_name, need);
                buf.add_edge_by_qn(&job_qn, &need_qn, "DEPENDS_ON", None);
            }
        }

        // Deployment/build edges
        create_deploy_edges(buf, &job_qn, project, &all_script_lines);
    }

    // Extract synthetic jobs from `include:` components.
    // GitLab CI components typically inject jobs into specific stages.
    // We create placeholder Job nodes so the pipeline DAG isn't empty.
    if let Some(includes) = mapping.get(serde_yaml::Value::String("include".into())) {
        let include_list: Vec<&serde_yaml::Value> = match includes {
            serde_yaml::Value::Sequence(seq) => seq.iter().collect(),
            other => vec![other],
        };

        for inc in include_list {
            let inc_map = match inc.as_mapping() {
                Some(m) => m,
                None => continue,
            };

            // Extract component path: "component: code.example.com/org/ci/build-java-maven/maven@~latest"
            let component = inc_map
                .get(serde_yaml::Value::String("component".into()))
                .and_then(|v| v.as_str());
            let local = inc_map
                .get(serde_yaml::Value::String("local".into()))
                .and_then(|v| v.as_str());

            let (job_name, stage_hint) = if let Some(comp) = component {
                // Extract meaningful name from component path
                // e.g. "code.example.com/org/cicd/ci/build-java-maven/maven@~latest"
                //   → name: "maven", stage hint from path segment: "build-java-maven" → "build"
                let path_part = comp.split('@').next().unwrap_or(comp);
                let segments: Vec<&str> = path_part.split('/').collect();
                let name = segments.last().unwrap_or(&"included");
                let stage_hint = segments.iter().rev().skip(1).find_map(|seg| {
                    let lower = seg.to_lowercase();
                    for s in &stages {
                        if lower.contains(&s.to_lowercase()) {
                            return Some(s.clone());
                        }
                    }
                    None
                });
                (name.to_string(), stage_hint)
            } else if let Some(loc) = local {
                let name = Path::new(loc)
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("local-include")
                    .to_owned();
                (name, None)
            } else {
                continue;
            };

            // Try to infer stage from component inputs
            let input_stage = inc_map
                .get(serde_yaml::Value::String("inputs".into()))
                .and_then(|v| v.as_mapping())
                .and_then(|m| m.get(serde_yaml::Value::String("stage".into())))
                .and_then(|v| v.as_str())
                .map(|s| s.to_owned());

            let stage = input_stage
                .or(stage_hint)
                .unwrap_or_else(|| stages.first().cloned().unwrap_or_else(|| "test".to_owned()));

            let job_qn = format!(
                "{}.pipeline.gitlab.{}.job.{}",
                project, pipeline_name, job_name
            );
            let job_props = serde_json::json!({
                "pipeline_name": pipeline_name,
                "stage": stage,
                "image": "",
                "script_summary": format!("(from include: {})", component.or(local).unwrap_or("unknown")),
                "source": "include",
            });
            buf.add_node(
                "Job",
                &job_name,
                &job_qn,
                rel_path,
                0,
                0,
                Some(job_props.to_string()),
            );

            // BELONGS_TO_STAGE edge
            let stage_qn = format!(
                "{}.pipeline.gitlab.{}.stage.{}",
                project, pipeline_name, stage
            );
            buf.add_edge_by_qn(&job_qn, &stage_qn, "BELONGS_TO_STAGE", None);
        }
    }
}

/// Parse a GitHub Actions workflow YAML file into Pipeline and Job nodes.
fn parse_github_actions(buf: &mut GraphBuffer, source: &str, rel_path: &str, project: &str) {
    let doc: serde_yaml::Value = match serde_yaml::from_str(source) {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(file = rel_path, error = %e, "pass_pipelines: failed to parse GitHub Actions YAML");
            return;
        }
    };

    let mapping = match doc.as_mapping() {
        Some(m) => m,
        None => return,
    };

    // Pipeline name from the `name` field or file stem
    let pipeline_name = mapping
        .get(serde_yaml::Value::String("name".into()))
        .and_then(|v| v.as_str())
        .map(|s| s.to_owned())
        .unwrap_or_else(|| pipeline_name_from_path(rel_path));

    let pipeline_qn = format!("{}.pipeline.github.{}", project, pipeline_name);

    // Extract triggers from `on` key
    let triggers: Vec<String> = match mapping.get(serde_yaml::Value::String("on".into())) {
        Some(serde_yaml::Value::String(s)) => vec![s.clone()],
        Some(serde_yaml::Value::Sequence(seq)) => seq
            .iter()
            .filter_map(|v| v.as_str().map(|s| s.to_owned()))
            .collect(),
        Some(serde_yaml::Value::Mapping(m)) => m
            .keys()
            .filter_map(|k| k.as_str().map(|s| s.to_owned()))
            .collect(),
        _ => vec![],
    };
    // Also check for `true` key (YAML parses `on:` as boolean true)
    let triggers = if triggers.is_empty() {
        match mapping.get(serde_yaml::Value::Bool(true)) {
            Some(serde_yaml::Value::String(s)) => vec![s.clone()],
            Some(serde_yaml::Value::Sequence(seq)) => seq
                .iter()
                .filter_map(|v| v.as_str().map(|s| s.to_owned()))
                .collect(),
            Some(serde_yaml::Value::Mapping(m)) => m
                .keys()
                .filter_map(|k| k.as_str().map(|s| s.to_owned()))
                .collect(),
            _ => vec![],
        }
    } else {
        triggers
    };

    let props = serde_json::json!({
        "ci_system": "github",
        "triggers": triggers,
    });
    buf.add_node(
        "Pipeline",
        &pipeline_name,
        &pipeline_qn,
        rel_path,
        0,
        0,
        Some(props.to_string()),
    );

    // Extract jobs
    let jobs = match mapping.get(serde_yaml::Value::String("jobs".into())) {
        Some(serde_yaml::Value::Mapping(m)) => m,
        _ => return,
    };

    for (key, value) in jobs {
        let job_name = match key.as_str() {
            Some(s) => s,
            None => continue,
        };

        let job_map = match value.as_mapping() {
            Some(m) => m,
            None => continue,
        };

        let runs_on = job_map
            .get(serde_yaml::Value::String("runs-on".into()))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_owned();

        // Collect script lines from steps
        let mut all_script_lines = Vec::new();
        if let Some(steps) = job_map
            .get(serde_yaml::Value::String("steps".into()))
            .and_then(|v| v.as_sequence())
        {
            for step in steps {
                if let Some(run_val) = step
                    .as_mapping()
                    .and_then(|m| m.get(serde_yaml::Value::String("run".into())))
                {
                    all_script_lines.extend(collect_script_lines(run_val));
                }
            }
        }

        let summary = script_summary(&all_script_lines);

        let job_qn = format!(
            "{}.pipeline.github.{}.job.{}",
            project, pipeline_name, job_name
        );
        let job_props = serde_json::json!({
            "pipeline_name": pipeline_name,
            "stage": "",
            "image": runs_on,
            "script_summary": summary,
        });
        buf.add_node(
            "Job",
            job_name,
            &job_qn,
            rel_path,
            0,
            0,
            Some(job_props.to_string()),
        );

        // DEPENDS_ON edges from needs
        if let Some(needs) = job_map.get(serde_yaml::Value::String("needs".into())) {
            let need_names: Vec<String> = match needs {
                serde_yaml::Value::Sequence(seq) => seq
                    .iter()
                    .filter_map(|v| match v {
                        serde_yaml::Value::String(s) => Some(s.clone()),
                        serde_yaml::Value::Mapping(m) => m
                            .get(serde_yaml::Value::String("job".into()))
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_owned()),
                        _ => None,
                    })
                    .collect(),
                serde_yaml::Value::String(s) => vec![s.clone()],
                _ => vec![],
            };
            for need in &need_names {
                let need_qn = format!("{}.pipeline.github.{}.job.{}", project, pipeline_name, need);
                buf.add_edge_by_qn(&job_qn, &need_qn, "DEPENDS_ON", None);
            }
        }

        // Deployment/build edges
        create_deploy_edges(buf, &job_qn, project, &all_script_lines);
    }
}

/// Parse a Jenkinsfile using regex-based stage extraction.
fn parse_jenkinsfile(buf: &mut GraphBuffer, source: &str, rel_path: &str, project: &str) {
    let pipeline_name = pipeline_name_from_path(rel_path);
    let pipeline_qn = format!("{}.pipeline.jenkins.{}", project, pipeline_name);

    let props = serde_json::json!({
        "ci_system": "jenkins",
        "triggers": serde_json::Value::Array(vec![]),
    });
    buf.add_node(
        "Pipeline",
        &pipeline_name,
        &pipeline_qn,
        rel_path,
        0,
        0,
        Some(props.to_string()),
    );

    // Extract stages using regex
    let mut prev_stage_qn: Option<String> = None;
    for (i, caps) in JENKINSFILE_STAGE_RE.captures_iter(source).enumerate() {
        let stage_name = &caps[1];
        let stage_qn = format!(
            "{}.pipeline.jenkins.{}.stage.{}",
            project, pipeline_name, stage_name
        );
        let stage_props = serde_json::json!({
            "pipeline_name": pipeline_name,
            "order": i,
        });
        buf.add_node(
            "Stage",
            stage_name,
            &stage_qn,
            rel_path,
            0,
            0,
            Some(stage_props.to_string()),
        );

        if let Some(ref prev_qn) = prev_stage_qn {
            buf.add_edge_by_qn(prev_qn, &stage_qn, "NEXT_STAGE", None);
        }
        prev_stage_qn = Some(stage_qn);
    }
}

/// Parse a CircleCI config.yml file into Pipeline and Job nodes.
fn parse_circleci(buf: &mut GraphBuffer, source: &str, rel_path: &str, project: &str) {
    let doc: serde_yaml::Value = match serde_yaml::from_str(source) {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(file = rel_path, error = %e, "pass_pipelines: failed to parse CircleCI YAML");
            return;
        }
    };

    let mapping = match doc.as_mapping() {
        Some(m) => m,
        None => return,
    };

    let pipeline_name = pipeline_name_from_path(rel_path);
    let pipeline_qn = format!("{}.pipeline.circleci.{}", project, pipeline_name);

    let props = serde_json::json!({
        "ci_system": "circleci",
        "triggers": serde_json::Value::Array(vec![]),
    });
    buf.add_node(
        "Pipeline",
        &pipeline_name,
        &pipeline_qn,
        rel_path,
        0,
        0,
        Some(props.to_string()),
    );

    // Extract jobs
    let jobs = match mapping.get(serde_yaml::Value::String("jobs".into())) {
        Some(serde_yaml::Value::Mapping(m)) => m,
        _ => return,
    };

    for (key, value) in jobs {
        let job_name = match key.as_str() {
            Some(s) => s,
            None => continue,
        };

        let job_map = match value.as_mapping() {
            Some(m) => m,
            None => continue,
        };

        let image = job_map
            .get(serde_yaml::Value::String("docker".into()))
            .and_then(|v| v.as_sequence())
            .and_then(|seq| seq.first())
            .and_then(|v| v.as_mapping())
            .and_then(|m| m.get(serde_yaml::Value::String("image".into())))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_owned();

        // Collect script lines from steps
        let mut all_script_lines = Vec::new();
        if let Some(steps) = job_map
            .get(serde_yaml::Value::String("steps".into()))
            .and_then(|v| v.as_sequence())
        {
            for step in steps {
                if let Some(run_map) = step
                    .as_mapping()
                    .and_then(|m| m.get(serde_yaml::Value::String("run".into())))
                {
                    match run_map {
                        serde_yaml::Value::String(s) => {
                            all_script_lines.extend(s.lines().map(|l| l.to_owned()));
                        }
                        serde_yaml::Value::Mapping(m) => {
                            if let Some(cmd) = m.get(serde_yaml::Value::String("command".into())) {
                                all_script_lines.extend(collect_script_lines(cmd));
                            }
                        }
                        _ => {}
                    }
                }
            }
        }

        let summary = script_summary(&all_script_lines);

        let job_qn = format!(
            "{}.pipeline.circleci.{}.job.{}",
            project, pipeline_name, job_name
        );
        let job_props = serde_json::json!({
            "pipeline_name": pipeline_name,
            "stage": "",
            "image": image,
            "script_summary": summary,
        });
        buf.add_node(
            "Job",
            job_name,
            &job_qn,
            rel_path,
            0,
            0,
            Some(job_props.to_string()),
        );

        // Deployment/build edges
        create_deploy_edges(buf, &job_qn, project, &all_script_lines);
    }

    // Extract workflow dependencies (requires)
    if let Some(workflows) = mapping
        .get(serde_yaml::Value::String("workflows".into()))
        .and_then(|v| v.as_mapping())
    {
        for (_wf_key, wf_val) in workflows {
            if let Some(wf_jobs) = wf_val
                .as_mapping()
                .and_then(|m| m.get(serde_yaml::Value::String("jobs".into())))
                .and_then(|v| v.as_sequence())
            {
                for wf_job in wf_jobs {
                    if let Some(wf_job_map) = wf_job.as_mapping() {
                        for (jk, jv) in wf_job_map {
                            let jname = match jk.as_str() {
                                Some(s) => s,
                                None => continue,
                            };
                            if let Some(requires) = jv
                                .as_mapping()
                                .and_then(|m| m.get(serde_yaml::Value::String("requires".into())))
                                .and_then(|v| v.as_sequence())
                            {
                                let job_qn = format!(
                                    "{}.pipeline.circleci.{}.job.{}",
                                    project, pipeline_name, jname
                                );
                                for req in requires {
                                    if let Some(req_name) = req.as_str() {
                                        let req_qn = format!(
                                            "{}.pipeline.circleci.{}.job.{}",
                                            project, pipeline_name, req_name
                                        );
                                        buf.add_edge_by_qn(&job_qn, &req_qn, "DEPENDS_ON", None);
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Parse an Azure DevOps pipeline YAML file into Pipeline, Stage, and Job nodes.
fn parse_azure_pipelines(buf: &mut GraphBuffer, source: &str, rel_path: &str, project: &str) {
    let doc: serde_yaml::Value = match serde_yaml::from_str(source) {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(file = rel_path, error = %e, "pass_pipelines: failed to parse Azure DevOps YAML");
            return;
        }
    };

    let mapping = match doc.as_mapping() {
        Some(m) => m,
        None => return,
    };

    let pipeline_name = mapping
        .get(serde_yaml::Value::String("name".into()))
        .and_then(|v| v.as_str())
        .map(|s| s.to_owned())
        .unwrap_or_else(|| pipeline_name_from_path(rel_path));

    let pipeline_qn = format!("{}.pipeline.azure.{}", project, pipeline_name);

    // Extract triggers
    let triggers: Vec<String> = match mapping.get(serde_yaml::Value::String("trigger".into())) {
        Some(serde_yaml::Value::Sequence(seq)) => seq
            .iter()
            .filter_map(|v| v.as_str().map(|s| s.to_owned()))
            .collect(),
        Some(serde_yaml::Value::String(s)) => vec![s.clone()],
        _ => vec![],
    };

    let props = serde_json::json!({
        "ci_system": "azure",
        "triggers": triggers,
    });
    buf.add_node(
        "Pipeline",
        &pipeline_name,
        &pipeline_qn,
        rel_path,
        0,
        0,
        Some(props.to_string()),
    );

    // Extract stages
    if let Some(stages_seq) = mapping
        .get(serde_yaml::Value::String("stages".into()))
        .and_then(|v| v.as_sequence())
    {
        let mut prev_stage_qn: Option<String> = None;
        for (i, stage_val) in stages_seq.iter().enumerate() {
            let stage_map = match stage_val.as_mapping() {
                Some(m) => m,
                None => continue,
            };

            let stage_obj = match stage_map.get(serde_yaml::Value::String("stage".into())) {
                Some(serde_yaml::Value::String(s)) => s.clone(),
                _ => continue,
            };

            let stage_qn = format!(
                "{}.pipeline.azure.{}.stage.{}",
                project, pipeline_name, stage_obj
            );
            let stage_props = serde_json::json!({
                "pipeline_name": pipeline_name,
                "order": i,
            });
            buf.add_node(
                "Stage",
                &stage_obj,
                &stage_qn,
                rel_path,
                0,
                0,
                Some(stage_props.to_string()),
            );

            if let Some(ref prev_qn) = prev_stage_qn {
                buf.add_edge_by_qn(prev_qn, &stage_qn, "NEXT_STAGE", None);
            }
            prev_stage_qn = Some(stage_qn.clone());

            // Extract jobs within stage
            if let Some(jobs_seq) = stage_map
                .get(serde_yaml::Value::String("jobs".into()))
                .and_then(|v| v.as_sequence())
            {
                for job_val in jobs_seq {
                    let job_map = match job_val.as_mapping() {
                        Some(m) => m,
                        None => continue,
                    };

                    let job_name = match job_map
                        .get(serde_yaml::Value::String("job".into()))
                        .and_then(|v| v.as_str())
                    {
                        Some(s) => s.to_owned(),
                        None => continue,
                    };

                    let pool_image = job_map
                        .get(serde_yaml::Value::String("pool".into()))
                        .and_then(|v| v.as_mapping())
                        .and_then(|m| m.get(serde_yaml::Value::String("vmImage".into())))
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_owned();

                    // Collect script lines from steps
                    let mut all_script_lines = Vec::new();
                    if let Some(steps) = job_map
                        .get(serde_yaml::Value::String("steps".into()))
                        .and_then(|v| v.as_sequence())
                    {
                        for step in steps {
                            if let Some(script_val) = step
                                .as_mapping()
                                .and_then(|m| m.get(serde_yaml::Value::String("script".into())))
                            {
                                all_script_lines.extend(collect_script_lines(script_val));
                            }
                        }
                    }

                    let summary = script_summary(&all_script_lines);

                    let job_qn = format!(
                        "{}.pipeline.azure.{}.job.{}",
                        project, pipeline_name, job_name
                    );
                    let job_props = serde_json::json!({
                        "pipeline_name": pipeline_name,
                        "stage": stage_obj,
                        "image": pool_image,
                        "script_summary": summary,
                    });
                    buf.add_node(
                        "Job",
                        &job_name,
                        &job_qn,
                        rel_path,
                        0,
                        0,
                        Some(job_props.to_string()),
                    );

                    buf.add_edge_by_qn(&job_qn, &stage_qn, "BELONGS_TO_STAGE", None);

                    // DEPENDS_ON from dependsOn
                    if let Some(deps) = job_map
                        .get(serde_yaml::Value::String("dependsOn".into()))
                        .and_then(|v| v.as_sequence())
                    {
                        for dep in deps {
                            if let Some(dep_name) = dep.as_str() {
                                let dep_qn = format!(
                                    "{}.pipeline.azure.{}.job.{}",
                                    project, pipeline_name, dep_name
                                );
                                buf.add_edge_by_qn(&job_qn, &dep_qn, "DEPENDS_ON", None);
                            }
                        }
                    }

                    create_deploy_edges(buf, &job_qn, project, &all_script_lines);
                }
            }
        }
    }
}

/// Parse a Bitbucket Pipelines YAML file into Pipeline and Job nodes.
fn parse_bitbucket_pipelines(buf: &mut GraphBuffer, source: &str, rel_path: &str, project: &str) {
    let doc: serde_yaml::Value = match serde_yaml::from_str(source) {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(file = rel_path, error = %e, "pass_pipelines: failed to parse Bitbucket Pipelines YAML");
            return;
        }
    };

    let mapping = match doc.as_mapping() {
        Some(m) => m,
        None => return,
    };

    let pipeline_name = pipeline_name_from_path(rel_path);
    let pipeline_qn = format!("{}.pipeline.bitbucket.{}", project, pipeline_name);

    let props = serde_json::json!({
        "ci_system": "bitbucket",
        "triggers": serde_json::Value::Array(vec![]),
    });
    buf.add_node(
        "Pipeline",
        &pipeline_name,
        &pipeline_qn,
        rel_path,
        0,
        0,
        Some(props.to_string()),
    );

    // Bitbucket pipelines: pipelines.default is a list of steps
    let pipelines = match mapping.get(serde_yaml::Value::String("pipelines".into())) {
        Some(serde_yaml::Value::Mapping(m)) => m,
        _ => return,
    };

    // Process default pipeline and branch pipelines
    let mut step_index = 0usize;
    let process_steps =
        |steps: &serde_yaml::Sequence, buf: &mut GraphBuffer, step_index: &mut usize| {
            for step_val in steps {
                let step_map = match step_val
                    .as_mapping()
                    .and_then(|m| m.get(serde_yaml::Value::String("step".into())))
                    .and_then(|v| v.as_mapping())
                {
                    Some(m) => m,
                    None => continue,
                };

                let step_name = step_map
                    .get(serde_yaml::Value::String("name".into()))
                    .and_then(|v| v.as_str())
                    .unwrap_or(&format!("step-{}", step_index))
                    .to_owned();

                let image = step_map
                    .get(serde_yaml::Value::String("image".into()))
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_owned();

                let mut all_script_lines = Vec::new();
                if let Some(script) = step_map
                    .get(serde_yaml::Value::String("script".into()))
                    .and_then(|v| v.as_sequence())
                {
                    for line in script {
                        if let Some(s) = line.as_str() {
                            all_script_lines.push(s.to_owned());
                        }
                    }
                }

                let summary = script_summary(&all_script_lines);

                let job_qn = format!(
                    "{}.pipeline.bitbucket.{}.job.{}",
                    project, pipeline_name, step_name
                );
                let job_props = serde_json::json!({
                    "pipeline_name": pipeline_name,
                    "stage": "",
                    "image": image,
                    "script_summary": summary,
                });
                buf.add_node(
                    "Job",
                    &step_name,
                    &job_qn,
                    rel_path,
                    0,
                    0,
                    Some(job_props.to_string()),
                );

                create_deploy_edges(buf, &job_qn, project, &all_script_lines);
                *step_index += 1;
            }
        };

    // Process default pipeline
    if let Some(default_steps) = pipelines
        .get(serde_yaml::Value::String("default".into()))
        .and_then(|v| v.as_sequence())
    {
        process_steps(default_steps, buf, &mut step_index);
    }

    // Process branch pipelines
    if let Some(branches) = pipelines
        .get(serde_yaml::Value::String("branches".into()))
        .and_then(|v| v.as_mapping())
    {
        for (_branch_key, branch_val) in branches {
            if let Some(steps) = branch_val.as_sequence() {
                process_steps(steps, buf, &mut step_index);
            }
        }
    }
}

// ── IaC Parsing (Terraform, Helm) ──────────────────────────────────

/// Regex for Terraform resource blocks: `resource "type" "name" {`
static TF_RESOURCE_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r#"resource\s+"([^"]+)"\s+"([^"]+)"\s*\{"#).unwrap());

/// Regex for Terraform data blocks: `data "type" "name" {`
static TF_DATA_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r#"data\s+"([^"]+)"\s+"([^"]+)"\s*\{"#).unwrap());

/// Regex for Terraform module blocks: `module "name" {`
static TF_MODULE_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r#"module\s+"([^"]+)"\s*\{"#).unwrap());

/// Regex for Terraform cross-resource references: `type.name.attribute`
/// Matches patterns like `aws_subnet.main.id` or `data.aws_ami.ubuntu.id`
static TF_REF_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"\b([a-z][a-z0-9_]*)\.([a-z][a-z0-9_]*)\.([a-z][a-z0-9_]*)").unwrap()
});

/// Regex for Docker image references in Terraform (e.g., `image = "nginx:latest"`)
static TF_IMAGE_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r#"image\s*=\s*"([^"]+)""#).unwrap());

/// Pass: Parse Terraform .tf files and Helm Chart.yaml files,
/// creating Infra nodes with DEPENDS_ON and USES_IMAGE edges.
pub fn pass_iac(buf: &mut GraphBuffer, files: &[&DiscoveredFile], project: &str) {
    for f in files {
        if is_pass_cancelled() {
            return;
        }

        match f.language {
            Language::Hcl => {
                parse_terraform(buf, f, project);
            }
            Language::Yaml if is_helm_chart_yaml(&f.rel_path) => {
                parse_helm_chart(buf, f, project);
            }
            _ => {}
        }
    }
}

/// Check if a YAML file is a Helm Chart.yaml (not a template or values file).
fn is_helm_chart_yaml(rel_path: &str) -> bool {
    let filename = rel_path.rsplit('/').next().unwrap_or(rel_path);
    filename == "Chart.yaml" || filename == "Chart.yml"
}

/// Parse a Terraform .tf file and create Infra nodes with DEPENDS_ON edges.
fn parse_terraform(buf: &mut GraphBuffer, f: &DiscoveredFile, project: &str) {
    let source = match std::fs::read_to_string(&f.abs_path) {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!(file = f.rel_path.as_str(), error = %e, "pass_iac: failed to read Terraform file");
            return;
        }
    };

    // Collect all resource/data/module blocks and their QNs for cross-reference resolution
    let mut known_resources: HashMap<String, String> = HashMap::new(); // "type.name" -> qn

    // Parse resource blocks
    for caps in TF_RESOURCE_RE.captures_iter(&source) {
        let resource_type = caps.get(1).unwrap().as_str();
        let resource_name = caps.get(2).unwrap().as_str();
        let qn = format!("{}.terraform.{}.{}", project, resource_type, resource_name);
        let props = serde_json::json!({
            "infra_type": "terraform",
            "resource_type": resource_type,
            "resource_name": resource_name,
        });
        buf.add_node(
            "Infra",
            resource_name,
            &qn,
            &f.rel_path,
            0,
            0,
            Some(props.to_string()),
        );
        known_resources.insert(format!("{}.{}", resource_type, resource_name), qn);
    }

    // Parse data blocks
    for caps in TF_DATA_RE.captures_iter(&source) {
        let data_type = caps.get(1).unwrap().as_str();
        let data_name = caps.get(2).unwrap().as_str();
        let resource_type = format!("data.{}", data_type);
        let qn = format!("{}.terraform.data.{}.{}", project, data_type, data_name);
        let props = serde_json::json!({
            "infra_type": "terraform",
            "resource_type": resource_type,
            "resource_name": data_name,
        });
        buf.add_node(
            "Infra",
            data_name,
            &qn,
            &f.rel_path,
            0,
            0,
            Some(props.to_string()),
        );
        known_resources.insert(format!("{}.{}", data_type, data_name), qn);
    }

    // Parse module blocks
    for caps in TF_MODULE_RE.captures_iter(&source) {
        let module_name = caps.get(1).unwrap().as_str();
        let qn = format!("{}.terraform.module.{}", project, module_name);
        let props = serde_json::json!({
            "infra_type": "terraform",
            "resource_type": "module",
            "resource_name": module_name,
        });
        buf.add_node(
            "Infra",
            module_name,
            &qn,
            &f.rel_path,
            0,
            0,
            Some(props.to_string()),
        );
        known_resources.insert(format!("module.{}", module_name), qn);
    }

    // Detect cross-resource references and create DEPENDS_ON edges
    // We need to figure out which resource block each reference belongs to.
    // Strategy: scan the file line by line, track the current resource context,
    // and when we see a reference to another known resource, create an edge.
    detect_terraform_references(buf, &source, project, &known_resources);

    // Detect Docker image references and create USES_IMAGE edges
    for caps in TF_IMAGE_RE.captures_iter(&source) {
        let image = caps.get(1).unwrap().as_str();
        let _image_name = image.split(':').next().unwrap_or(image);
        // Try to match against a Dockerfile Docker_Image node
        let docker_qn = format!("{}.docker_image.{}", project, image);
        // Create USES_IMAGE from any resource in this file to the Docker image
        // We link from the first resource in the file as a reasonable heuristic
        if let Some(first_qn) = known_resources.values().next() {
            buf.add_edge_by_qn(first_qn, &docker_qn, "USES_IMAGE", None);
        }
    }
}

/// Detect Terraform cross-resource references and create DEPENDS_ON edges.
/// Scans the file to determine which resource block contains each reference,
/// then creates edges from the containing resource to the referenced resource.
fn detect_terraform_references(
    buf: &mut GraphBuffer,
    source: &str,
    _project: &str,
    known_resources: &HashMap<String, String>,
) {
    // Track current resource context by scanning for block openings
    let mut current_resource_qn: Option<String> = None;
    let mut brace_depth: i32 = 0;
    let mut seen_edges: HashSet<(String, String)> = HashSet::new();

    for line in source.lines() {
        let trimmed = line.trim();

        // Check if this line starts a new resource/data/module block
        if let Some(caps) = TF_RESOURCE_RE.captures(trimmed) {
            let rtype = caps.get(1).unwrap().as_str();
            let rname = caps.get(2).unwrap().as_str();
            let key = format!("{}.{}", rtype, rname);
            current_resource_qn = known_resources.get(&key).cloned();
            brace_depth = 1;
            continue;
        }
        if let Some(caps) = TF_DATA_RE.captures(trimmed) {
            let dtype = caps.get(1).unwrap().as_str();
            let dname = caps.get(2).unwrap().as_str();
            let key = format!("{}.{}", dtype, dname);
            current_resource_qn = known_resources.get(&key).cloned();
            brace_depth = 1;
            continue;
        }
        if let Some(caps) = TF_MODULE_RE.captures(trimmed) {
            let mname = caps.get(1).unwrap().as_str();
            let key = format!("module.{}", mname);
            current_resource_qn = known_resources.get(&key).cloned();
            brace_depth = 1;
            continue;
        }

        // Track brace depth
        for ch in trimmed.chars() {
            match ch {
                '{' => brace_depth += 1,
                '}' => {
                    brace_depth -= 1;
                    if brace_depth <= 0 {
                        current_resource_qn = None;
                        brace_depth = 0;
                    }
                }
                _ => {}
            }
        }

        // Skip if we're not inside a resource block
        let src_qn = match &current_resource_qn {
            Some(qn) => qn.clone(),
            None => continue,
        };

        // Skip comment lines
        if trimmed.starts_with('#') || trimmed.starts_with("//") {
            continue;
        }

        // Find cross-resource references in this line
        for caps in TF_REF_RE.captures_iter(line) {
            let ref_type = caps.get(1).unwrap().as_str();
            let ref_name = caps.get(2).unwrap().as_str();
            let ref_key = format!("{}.{}", ref_type, ref_name);

            if let Some(target_qn) = known_resources.get(&ref_key) {
                // Don't create self-referencing edges
                if *target_qn != src_qn {
                    let edge_key = (src_qn.clone(), target_qn.clone());
                    if seen_edges.insert(edge_key) {
                        buf.add_edge_by_qn(&src_qn, target_qn, "DEPENDS_ON", None);
                    }
                }
            }
        }
    }
}

/// Helm Chart.yaml deserialization helper
#[derive(serde::Deserialize)]
struct HelmChart {
    name: Option<String>,
    version: Option<String>,
    #[serde(rename = "appVersion")]
    app_version: Option<String>,
    dependencies: Option<Vec<HelmDependency>>,
}

#[derive(serde::Deserialize)]
struct HelmDependency {
    name: Option<String>,
    version: Option<String>,
    #[allow(dead_code)]
    repository: Option<String>,
    #[allow(dead_code)]
    condition: Option<String>,
}

/// Parse a Helm Chart.yaml file and create an Infra node with DEPENDS_ON edges.
fn parse_helm_chart(buf: &mut GraphBuffer, f: &DiscoveredFile, project: &str) {
    let source = match std::fs::read_to_string(&f.abs_path) {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!(file = f.rel_path.as_str(), error = %e, "pass_iac: failed to read Helm Chart.yaml");
            return;
        }
    };

    let chart: HelmChart = match serde_yaml::from_str(&source) {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(file = f.rel_path.as_str(), error = %e, "pass_iac: failed to parse Helm Chart.yaml");
            return;
        }
    };

    let chart_name = chart.name.unwrap_or_else(|| {
        // Derive name from directory
        Path::new(&f.rel_path)
            .parent()
            .and_then(|p| p.file_name())
            .and_then(|s| s.to_str())
            .unwrap_or("chart")
            .to_owned()
    });

    let version = chart.version.unwrap_or_default();
    let app_version = chart.app_version.unwrap_or_default();

    let chart_qn = format!("{}.helm.{}", project, chart_name);
    let props = serde_json::json!({
        "infra_type": "helm",
        "name": chart_name,
        "version": version,
        "appVersion": app_version,
    });

    buf.add_node(
        "Infra",
        &chart_name,
        &chart_qn,
        &f.rel_path,
        0,
        0,
        Some(props.to_string()),
    );

    // Create DEPENDS_ON edges for chart dependencies
    if let Some(deps) = chart.dependencies {
        for dep in &deps {
            let dep_name = match &dep.name {
                Some(n) => n.clone(),
                None => continue,
            };
            let dep_version = dep.version.as_deref().unwrap_or("");
            let dep_qn = format!("{}.helm.{}", project, dep_name);
            let dep_props = serde_json::json!({
                "infra_type": "helm",
                "name": dep_name,
                "version": dep_version,
                "appVersion": "",
            });

            // Create a node for the dependency chart
            buf.add_node(
                "Infra",
                &dep_name,
                &dep_qn,
                &f.rel_path,
                0,
                0,
                Some(dep_props.to_string()),
            );

            // DEPENDS_ON edge from parent chart to dependency
            buf.add_edge_by_qn(&chart_qn, &dep_qn, "DEPENDS_ON", None);
        }
    }

    // Detect Docker image references in values and create USES_IMAGE edges
    // Check if there's a values.yaml alongside Chart.yaml
    if let Some(parent) = f.abs_path.parent() {
        let values_path = parent.join("values.yaml");
        if let Ok(values_source) = std::fs::read_to_string(&values_path) {
            for caps in TF_IMAGE_RE.captures_iter(&values_source) {
                let image = caps.get(1).unwrap().as_str();
                let docker_qn = format!("{}.docker_image.{}", project, image);
                buf.add_edge_by_qn(&chart_qn, &docker_qn, "USES_IMAGE", None);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::Registry;
    use codryn_discover::{DiscoveredFile, Language};
    use codryn_graph_buffer::GraphBuffer;
    use codryn_store::{Node, Project, Store};

    /// Helper: create an in-memory store with a project.
    fn test_store(project: &str) -> Store {
        let s = Store::open_in_memory().unwrap();
        s.upsert_project(&Project {
            name: project.into(),
            indexed_at: "now".into(),
            root_path: "/tmp".into(),
        })
        .unwrap();
        s
    }

    /// Helper: write a file into a tempdir and return a DiscoveredFile.
    fn write_file(
        dir: &std::path::Path,
        rel_path: &str,
        content: &str,
        language: Language,
    ) -> DiscoveredFile {
        let abs = dir.join(rel_path);
        if let Some(parent) = abs.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(&abs, content).unwrap();
        DiscoveredFile {
            abs_path: abs,
            rel_path: rel_path.to_owned(),
            language,
        }
    }

    // ── pass_usages tests ────────────────────────────────

    #[test]
    fn test_pass_usages_creates_uses_edges() {
        let dir = tempfile::tempdir().unwrap();
        let project = "p";

        // File A defines a constant MY_CONST
        write_file(
            dir.path(),
            "src/constants.ts",
            "export const MY_CONST = 42;\n",
            Language::TypeScript,
        );
        // File B references MY_CONST but does NOT call it (no parentheses)
        let file_b = write_file(
            dir.path(),
            "src/app.ts",
            "import { MY_CONST } from './constants';\nconst x = MY_CONST + 1;\n",
            Language::TypeScript,
        );

        let mut reg = Registry::new();
        reg.register(
            "MY_CONST",
            &format!("{}.src.constants.MY_CONST", project),
            "src/constants.ts",
            "Variable",
            1,
            1,
        );

        let mut buf = GraphBuffer::new(project);
        let files: Vec<&DiscoveredFile> = vec![&file_b];
        pass_usages(&mut buf, &reg, &files, project);

        assert!(
            buf.edge_count() > 0,
            "pass_usages should create USES edges for non-call references"
        );
    }

    #[test]
    fn test_pass_usages_skips_call_sites() {
        let dir = tempfile::tempdir().unwrap();
        let project = "p";

        // File A defines a function doStuff
        write_file(
            dir.path(),
            "src/utils.ts",
            "export function doStuff() { return 1; }\n",
            Language::TypeScript,
        );
        // File B calls doStuff() — this should NOT produce a USES edge
        let file_b = write_file(
            dir.path(),
            "src/main.ts",
            "import { doStuff } from './utils';\nconst result = doStuff();\n",
            Language::TypeScript,
        );

        let mut reg = Registry::new();
        reg.register(
            "doStuff",
            &format!("{}.src.utils.doStuff", project),
            "src/utils.ts",
            "Function",
            1,
            1,
        );

        let mut buf = GraphBuffer::new(project);
        let files: Vec<&DiscoveredFile> = vec![&file_b];
        pass_usages(&mut buf, &reg, &files, project);

        assert_eq!(
            buf.edge_count(),
            0,
            "pass_usages should NOT create edges for call sites (function calls)"
        );
    }

    #[test]
    fn test_pass_usages_empty_registry() {
        let dir = tempfile::tempdir().unwrap();
        let project = "p";

        let file = write_file(
            dir.path(),
            "src/app.ts",
            "const x = 1;\n",
            Language::TypeScript,
        );

        let reg = Registry::new();
        let mut buf = GraphBuffer::new(project);
        let files: Vec<&DiscoveredFile> = vec![&file];
        pass_usages(&mut buf, &reg, &files, project);

        assert_eq!(
            buf.edge_count(),
            0,
            "empty registry should produce no edges"
        );
    }

    // ── pass_tests tests ─────────────────────────────────

    #[test]
    fn test_pass_tests_marks_test_functions() {
        let dir = tempfile::tempdir().unwrap();
        let project = "p";
        let store = test_store(project);

        // Insert a function node in a test file
        store
            .insert_node(&Node {
                id: 0,
                project: project.into(),
                label: "Function".into(),
                name: "test_add".into(),
                qualified_name: format!("{}.src.__tests__.math.test.test_add", project),
                file_path: "src/__tests__/math.test.ts".into(),
                start_line: 1,
                end_line: 5,
                properties_json: None,
            })
            .unwrap();
        let test_file = write_file(
            dir.path(),
            "src/__tests__/math.test.ts",
            "import { add } from '../math';\nfunction test_add() {\n  expect(add(1, 2)).toBe(3);\n}\n",
            Language::TypeScript,
        );

        // Register the production symbol
        let mut reg = Registry::new();
        reg.register(
            "add",
            &format!("{}.src.math.add", project),
            "src/math.ts",
            "Function",
            1,
            10,
        );
        // Register the test function too so entries_for_file works
        reg.register(
            "test_add",
            &format!("{}.src.__tests__.math.test.test_add", project),
            "src/__tests__/math.test.ts",
            "Function",
            2,
            4,
        );

        let mut buf = GraphBuffer::new(project);
        let files: Vec<&DiscoveredFile> = vec![&test_file];
        pass_tests(&mut buf, &store, &reg, &files, project);

        // Verify the test function was marked with is_test
        let nodes = store
            .get_nodes_for_file(project, "src/__tests__/math.test.ts")
            .unwrap();
        let test_node = nodes.iter().find(|n| n.name == "test_add").unwrap();
        let props: serde_json::Value =
            serde_json::from_str(test_node.properties_json.as_deref().unwrap_or("{}")).unwrap();
        assert_eq!(
            props.get("is_test").and_then(|v| v.as_bool()),
            Some(true),
            "test function should be marked with is_test"
        );

        // Verify TESTS edges were created
        assert!(
            buf.edge_count() > 0,
            "pass_tests should create TESTS edges from test functions to tested symbols"
        );
    }

    #[test]
    fn test_pass_tests_ignores_non_test_files() {
        let dir = tempfile::tempdir().unwrap();
        let project = "p";
        let store = test_store(project);

        // A regular (non-test) file
        let file = write_file(
            dir.path(),
            "src/math.ts",
            "export function add(a: number, b: number) { return a + b; }\n",
            Language::TypeScript,
        );

        let mut reg = Registry::new();
        reg.register(
            "add",
            &format!("{}.src.math.add", project),
            "src/math.ts",
            "Function",
            1,
            1,
        );

        let mut buf = GraphBuffer::new(project);
        let files: Vec<&DiscoveredFile> = vec![&file];
        pass_tests(&mut buf, &store, &reg, &files, project);

        assert_eq!(
            buf.edge_count(),
            0,
            "pass_tests should not create edges for non-test files"
        );
    }

    #[test]
    fn test_is_test_file_patterns() {
        assert!(is_test_file("src/__tests__/foo.ts"));
        assert!(is_test_file("src/foo.test.ts"));
        assert!(is_test_file("src/foo.spec.ts"));
        assert!(is_test_file("src/tests/test_main.py"));
        assert!(is_test_file("pkg/handler_test.go"));
        assert!(is_test_file("src/lib_test.rs"));
        assert!(!is_test_file("src/main.ts"));
        assert!(!is_test_file("src/utils.py"));
    }

    // ── pass_envscan tests ───────────────────────────────

    #[test]
    fn test_pass_envscan_js_process_env() {
        let dir = tempfile::tempdir().unwrap();
        let project = "p";

        let file = write_file(
            dir.path(),
            "src/config.ts",
            "const port = process.env.PORT;\nconst host = process.env.HOST;\n",
            Language::TypeScript,
        );

        let reg = Registry::new();
        let mut buf = GraphBuffer::new(project);
        let files: Vec<&DiscoveredFile> = vec![&file];
        pass_envscan(&mut buf, &reg, &files, project);

        // Should create 2 EnvVar nodes (PORT, HOST) and 2 READS_ENV edges
        assert!(
            buf.node_count() >= 2,
            "should create EnvVar nodes for PORT and HOST, got {}",
            buf.node_count()
        );
        assert!(
            buf.edge_count() >= 2,
            "should create READS_ENV edges, got {}",
            buf.edge_count()
        );
    }

    #[test]
    fn test_pass_envscan_python_os_environ() {
        let dir = tempfile::tempdir().unwrap();
        let project = "p";

        let file = write_file(
            dir.path(),
            "src/config.py",
            "import os\ndb_url = os.environ[\"DATABASE_URL\"]\nsecret = os.getenv(\"SECRET_KEY\")\n",
            Language::Python,
        );

        let reg = Registry::new();
        let mut buf = GraphBuffer::new(project);
        let files: Vec<&DiscoveredFile> = vec![&file];
        pass_envscan(&mut buf, &reg, &files, project);

        assert!(
            buf.node_count() >= 2,
            "should create EnvVar nodes for DATABASE_URL and SECRET_KEY, got {}",
            buf.node_count()
        );
        assert!(
            buf.edge_count() >= 2,
            "should create READS_ENV edges, got {}",
            buf.edge_count()
        );
    }

    #[test]
    fn test_pass_envscan_rust_env_var() {
        let dir = tempfile::tempdir().unwrap();
        let project = "p";

        let file = write_file(
            dir.path(),
            "src/main.rs",
            "fn main() {\n    let key = std::env::var(\"API_KEY\").unwrap();\n}\n",
            Language::Rust,
        );

        let reg = Registry::new();
        let mut buf = GraphBuffer::new(project);
        let files: Vec<&DiscoveredFile> = vec![&file];
        pass_envscan(&mut buf, &reg, &files, project);

        assert!(
            buf.node_count() >= 1,
            "should create EnvVar node for API_KEY, got {}",
            buf.node_count()
        );
        assert!(
            buf.edge_count() >= 1,
            "should create READS_ENV edge, got {}",
            buf.edge_count()
        );
    }

    #[test]
    fn test_pass_envscan_deduplicates_env_vars() {
        let dir = tempfile::tempdir().unwrap();
        let project = "p";

        // Same env var referenced twice in different files
        let file_a = write_file(
            dir.path(),
            "src/a.ts",
            "const x = process.env.PORT;\n",
            Language::TypeScript,
        );
        let file_b = write_file(
            dir.path(),
            "src/b.ts",
            "const y = process.env.PORT;\n",
            Language::TypeScript,
        );

        let reg = Registry::new();
        let mut buf = GraphBuffer::new(project);
        let files: Vec<&DiscoveredFile> = vec![&file_a, &file_b];
        pass_envscan(&mut buf, &reg, &files, project);

        // Should create only 1 EnvVar node for PORT, but 2 READS_ENV edges
        assert_eq!(
            buf.node_count(),
            1,
            "should create exactly 1 EnvVar node for PORT"
        );
        assert_eq!(
            buf.edge_count(),
            2,
            "should create 2 READS_ENV edges (one per file)"
        );
    }

    #[test]
    fn test_pass_envscan_no_env_vars() {
        let dir = tempfile::tempdir().unwrap();
        let project = "p";

        let file = write_file(
            dir.path(),
            "src/app.ts",
            "const x = 42;\nconsole.log(x);\n",
            Language::TypeScript,
        );

        let reg = Registry::new();
        let mut buf = GraphBuffer::new(project);
        let files: Vec<&DiscoveredFile> = vec![&file];
        pass_envscan(&mut buf, &reg, &files, project);

        assert_eq!(buf.node_count(), 0, "no env vars should produce no nodes");
        assert_eq!(buf.edge_count(), 0, "no env vars should produce no edges");
    }

    // ── pass_configlink tests ────────────────────────────

    #[test]
    fn test_pass_configlink_env_file_creates_edges() {
        let dir = tempfile::tempdir().unwrap();
        let project = "p";

        // Create a .env config file with keys
        let config_file = write_file(
            dir.path(),
            "config/.env",
            "PORT=3000\nDATABASE_URL=postgres://localhost\n",
            Language::Unknown,
        );

        // Create a source file that references PORT
        let _src_file = write_file(
            dir.path(),
            "src/app.ts",
            "const port = process.env.PORT;\n",
            Language::TypeScript,
        );

        // Register a symbol named PORT so the config key matches
        let mut reg = Registry::new();
        reg.register(
            "PORT",
            &format!("{}.src.app.PORT", project),
            "src/app.ts",
            "Variable",
            1,
            1,
        );

        let mut buf = GraphBuffer::new(project);
        let files: Vec<&DiscoveredFile> = vec![&config_file];
        pass_configlink(&mut buf, &reg, &files, project);

        assert!(
            buf.edge_count() > 0,
            "pass_configlink should create CONFIG_LINKS edges when config keys match registered symbols"
        );
    }

    #[test]
    fn test_pass_configlink_json_file_creates_edges() {
        let dir = tempfile::tempdir().unwrap();
        let project = "p";

        let config_file = write_file(
            dir.path(),
            "config/settings.json",
            "{\n  \"apiKey\": \"abc123\",\n  \"timeout\": 5000\n}\n",
            Language::Unknown,
        );

        let mut reg = Registry::new();
        reg.register(
            "apiKey",
            &format!("{}.src.config.apiKey", project),
            "src/config.ts",
            "Variable",
            1,
            1,
        );

        let mut buf = GraphBuffer::new(project);
        let files: Vec<&DiscoveredFile> = vec![&config_file];
        pass_configlink(&mut buf, &reg, &files, project);

        assert!(
            buf.edge_count() > 0,
            "pass_configlink should create CONFIG_LINKS edges for JSON config keys"
        );
    }

    #[test]
    fn test_pass_configlink_no_matching_symbols() {
        let dir = tempfile::tempdir().unwrap();
        let project = "p";

        let config_file = write_file(
            dir.path(),
            "config/.env",
            "SOME_RANDOM_KEY=value\n",
            Language::Unknown,
        );

        // Registry has no symbol named SOME_RANDOM_KEY
        let reg = Registry::new();
        let mut buf = GraphBuffer::new(project);
        let files: Vec<&DiscoveredFile> = vec![&config_file];
        pass_configlink(&mut buf, &reg, &files, project);

        assert_eq!(
            buf.edge_count(),
            0,
            "pass_configlink should create no edges when no symbols match config keys"
        );
    }

    #[test]
    fn test_pass_configlink_skips_non_config_files() {
        let dir = tempfile::tempdir().unwrap();
        let project = "p";

        let src_file = write_file(
            dir.path(),
            "src/app.ts",
            "const PORT = 3000;\n",
            Language::TypeScript,
        );

        let mut reg = Registry::new();
        reg.register(
            "PORT",
            &format!("{}.src.app.PORT", project),
            "src/app.ts",
            "Variable",
            1,
            1,
        );

        let mut buf = GraphBuffer::new(project);
        let files: Vec<&DiscoveredFile> = vec![&src_file];
        pass_configlink(&mut buf, &reg, &files, project);

        assert_eq!(
            buf.edge_count(),
            0,
            "pass_configlink should skip non-config files"
        );
    }

    // ── normalize_config_key + compute_match_score + pass_configures tests ──

    #[test]
    fn test_normalize_config_key_camel_case() {
        let tokens = normalize_config_key("databaseUrl");
        assert_eq!(tokens, vec!["database", "url"]);
    }

    #[test]
    fn test_normalize_config_key_env_var() {
        let tokens = normalize_config_key("DATABASE_URL");
        assert_eq!(tokens, vec!["database", "url"]);
    }

    #[test]
    fn test_normalize_config_key_prefix_stripping() {
        let tokens = normalize_config_key("config.databaseUrl");
        assert_eq!(tokens, vec!["database", "url"]);

        let tokens2 = normalize_config_key("settings.apiKey");
        assert_eq!(tokens2, vec!["api", "key"]);

        let tokens3 = normalize_config_key(".env.SECRET_KEY");
        assert_eq!(tokens3, vec!["secret", "key"]);
    }

    #[test]
    fn test_normalize_config_key_extension_stripping() {
        let tokens = normalize_config_key("databaseUrl.json");
        assert_eq!(tokens, vec!["database", "url"]);

        let tokens2 = normalize_config_key("databaseUrl.yaml");
        assert_eq!(tokens2, vec!["database", "url"]);

        let tokens3 = normalize_config_key("databaseUrl.yml");
        assert_eq!(tokens3, vec!["database", "url"]);

        let tokens4 = normalize_config_key("databaseUrl.toml");
        assert_eq!(tokens4, vec!["database", "url"]);
    }

    #[test]
    fn test_normalize_config_key_delimiter_splitting() {
        let tokens = normalize_config_key("database_url");
        assert_eq!(tokens, vec!["database", "url"]);

        let tokens2 = normalize_config_key("database.url");
        assert_eq!(tokens2, vec!["database", "url"]);

        let tokens3 = normalize_config_key("database-url");
        assert_eq!(tokens3, vec!["database", "url"]);
    }

    #[test]
    fn test_normalize_config_key_empty_and_prefix_only() {
        let tokens = normalize_config_key("");
        assert!(tokens.is_empty());

        let tokens2 = normalize_config_key("config.");
        assert!(tokens2.is_empty());
    }

    #[test]
    fn test_compute_match_score_full_match() {
        let tokens = vec!["database".to_string(), "url".to_string()];
        let score = compute_match_score(&tokens, "databaseUrl");
        assert!(
            (score - 1.0).abs() < f64::EPSILON,
            "full match should be 1.0"
        );
    }

    #[test]
    fn test_compute_match_score_partial_match() {
        let tokens = vec!["database".to_string(), "url".to_string()];
        let score = compute_match_score(&tokens, "database");
        assert!(
            (score - 0.5).abs() < f64::EPSILON,
            "partial match should be 0.5"
        );
    }

    #[test]
    fn test_compute_match_score_no_match() {
        let tokens = vec!["database".to_string(), "url".to_string()];
        let score = compute_match_score(&tokens, "serverPort");
        assert!((score - 0.0).abs() < f64::EPSILON, "no match should be 0.0");
    }

    #[test]
    fn test_compute_match_score_empty_tokens() {
        let tokens: Vec<String> = vec![];
        let score = compute_match_score(&tokens, "anything");
        assert!(
            (score - 0.0).abs() < f64::EPSILON,
            "empty tokens should be 0.0"
        );
    }

    #[test]
    fn test_pass_configures_env_file_creates_edges() {
        let dir = tempfile::tempdir().unwrap();
        let project = "p";

        // Create a .env config file with keys
        let config_file = write_file(
            dir.path(),
            "config/.env",
            "DATABASE_URL=postgres://localhost\n",
            Language::Unknown,
        );

        // Register a symbol named databaseUrl so the normalized config key matches
        let mut reg = Registry::new();
        reg.register(
            "databaseUrl",
            &format!("{}.src.config.databaseUrl", project),
            "src/config.ts",
            "Variable",
            1,
            1,
        );

        let mut buf = GraphBuffer::new(project);
        let files: Vec<&DiscoveredFile> = vec![&config_file];
        pass_configures(&mut buf, &reg, &files, project);

        assert!(
            buf.edge_count() > 0,
            "pass_configures should create CONFIG_LINKS edges when normalized config keys match symbols"
        );
    }

    #[test]
    fn test_pass_configures_no_match_below_threshold() {
        let dir = tempfile::tempdir().unwrap();
        let project = "p";

        // Config key with 3 tokens: "max", "retry", "count"
        let config_file = write_file(
            dir.path(),
            "config/.env",
            "MAX_RETRY_COUNT=5\n",
            Language::Unknown,
        );

        // Symbol name only matches 1 of 3 tokens (score = 0.33, below 0.5)
        let mut reg = Registry::new();
        reg.register(
            "max",
            &format!("{}.src.app.max", project),
            "src/app.ts",
            "Function",
            1,
            1,
        );

        let mut buf = GraphBuffer::new(project);
        let files: Vec<&DiscoveredFile> = vec![&config_file];
        pass_configures(&mut buf, &reg, &files, project);

        assert_eq!(
            buf.edge_count(),
            0,
            "pass_configures should not create edges when match score is below 0.5"
        );
    }

    #[test]
    fn test_pass_configures_skips_non_config_files() {
        let dir = tempfile::tempdir().unwrap();
        let project = "p";

        let src_file = write_file(
            dir.path(),
            "src/app.ts",
            "const DATABASE_URL = 'test';\n",
            Language::TypeScript,
        );

        let mut reg = Registry::new();
        reg.register(
            "databaseUrl",
            &format!("{}.src.config.databaseUrl", project),
            "src/config.ts",
            "Variable",
            1,
            1,
        );

        let mut buf = GraphBuffer::new(project);
        let files: Vec<&DiscoveredFile> = vec![&src_file];
        pass_configures(&mut buf, &reg, &files, project);

        assert_eq!(
            buf.edge_count(),
            0,
            "pass_configures should skip non-config files"
        );
    }

    // ── pass_enrichment tests ────────────────────────────

    #[test]
    fn test_pass_enrichment_sets_fan_in_fan_out_centrality() {
        let project = "p";
        let store = test_store(project);

        // Insert nodes
        let id_a = store
            .insert_node(&Node {
                id: 0,
                project: project.into(),
                label: "Function".into(),
                name: "a".into(),
                qualified_name: format!("{}.a", project),
                file_path: "src/a.ts".into(),
                start_line: 1,
                end_line: 10,
                properties_json: None,
            })
            .unwrap();
        let id_b = store
            .insert_node(&Node {
                id: 0,
                project: project.into(),
                label: "Function".into(),
                name: "b".into(),
                qualified_name: format!("{}.b", project),
                file_path: "src/b.ts".into(),
                start_line: 1,
                end_line: 10,
                properties_json: None,
            })
            .unwrap();
        let id_c = store
            .insert_node(&Node {
                id: 0,
                project: project.into(),
                label: "Function".into(),
                name: "c".into(),
                qualified_name: format!("{}.c", project),
                file_path: "src/c.ts".into(),
                start_line: 1,
                end_line: 10,
                properties_json: None,
            })
            .unwrap();

        // a -> b, a -> c, c -> b  (b has fan_in=2, a has fan_out=2)
        store
            .insert_edge(&codryn_store::Edge {
                id: 0,
                project: project.into(),
                source_id: id_a,
                target_id: id_b,
                edge_type: "CALLS".into(),
                properties_json: None,
            })
            .unwrap();
        store
            .insert_edge(&codryn_store::Edge {
                id: 0,
                project: project.into(),
                source_id: id_a,
                target_id: id_c,
                edge_type: "CALLS".into(),
                properties_json: None,
            })
            .unwrap();
        store
            .insert_edge(&codryn_store::Edge {
                id: 0,
                project: project.into(),
                source_id: id_c,
                target_id: id_b,
                edge_type: "CALLS".into(),
                properties_json: None,
            })
            .unwrap();

        pass_enrichment(&store, project).unwrap();

        // Verify properties on node a (fan_in=0, fan_out=2)
        let nodes = store.get_all_nodes(project).unwrap();
        let node_a = nodes.iter().find(|n| n.name == "a").unwrap();
        let props_a: serde_json::Value =
            serde_json::from_str(node_a.properties_json.as_deref().unwrap()).unwrap();
        assert_eq!(props_a["fan_in"], 0);
        assert_eq!(props_a["fan_out"], 2);
        assert!(props_a["centrality"].as_f64().unwrap() > 0.0);

        // Verify properties on node b (fan_in=2, fan_out=0)
        let node_b = nodes.iter().find(|n| n.name == "b").unwrap();
        let props_b: serde_json::Value =
            serde_json::from_str(node_b.properties_json.as_deref().unwrap()).unwrap();
        assert_eq!(props_b["fan_in"], 2);
        assert_eq!(props_b["fan_out"], 0);
        assert!(props_b["centrality"].as_f64().unwrap() > 0.0);
    }

    #[test]
    fn test_pass_enrichment_empty_project() {
        let project = "empty";
        let store = test_store(project);

        // Should not panic on empty project
        pass_enrichment(&store, project).unwrap();
    }

    // ── pass_route_nodes tests ───────────────────────────

    #[test]
    fn test_pass_route_nodes_express() {
        let dir = tempfile::tempdir().unwrap();
        let project = "p";

        let file = write_file(
            dir.path(),
            "src/routes.ts",
            "import express from 'express';\nconst app = express();\napp.get('/users', getUsers);\napp.post('/users', createUser);\n",
            Language::TypeScript,
        );

        let mut reg = Registry::new();
        reg.register(
            "getUsers",
            &format!("{}.src.routes.getUsers", project),
            "src/routes.ts",
            "Function",
            3,
            3,
        );

        let mut buf = GraphBuffer::new(project);
        let files: Vec<&DiscoveredFile> = vec![&file];
        pass_route_nodes(&mut buf, &reg, &files, project);

        // Should create 2 Route nodes and 2 HANDLES_ROUTE edges
        assert!(
            buf.node_count() >= 2,
            "should create Route nodes for GET /users and POST /users, got {}",
            buf.node_count()
        );
        assert!(
            buf.edge_count() >= 2,
            "should create HANDLES_ROUTE edges, got {}",
            buf.edge_count()
        );
    }

    #[test]
    fn test_pass_route_nodes_fastapi() {
        let dir = tempfile::tempdir().unwrap();
        let project = "p";

        let file = write_file(
            dir.path(),
            "src/main.py",
            "from fastapi import FastAPI\napp = FastAPI()\n\n@app.get(\"/items\")\ndef list_items():\n    return []\n\n@app.post(\"/items\")\ndef create_item():\n    pass\n",
            Language::Python,
        );

        let mut reg = Registry::new();
        reg.register(
            "list_items",
            &format!("{}.src.main.list_items", project),
            "src/main.py",
            "Function",
            5,
            6,
        );

        let mut buf = GraphBuffer::new(project);
        let files: Vec<&DiscoveredFile> = vec![&file];
        pass_route_nodes(&mut buf, &reg, &files, project);

        assert!(
            buf.node_count() >= 2,
            "should create Route nodes for FastAPI routes, got {}",
            buf.node_count()
        );
        assert!(
            buf.edge_count() >= 2,
            "should create HANDLES_ROUTE edges for FastAPI routes, got {}",
            buf.edge_count()
        );
    }

    #[test]
    fn test_pass_route_nodes_no_routes() {
        let dir = tempfile::tempdir().unwrap();
        let project = "p";

        let file = write_file(
            dir.path(),
            "src/utils.ts",
            "export function add(a: number, b: number) { return a + b; }\n",
            Language::TypeScript,
        );

        let reg = Registry::new();
        let mut buf = GraphBuffer::new(project);
        let files: Vec<&DiscoveredFile> = vec![&file];
        pass_route_nodes(&mut buf, &reg, &files, project);

        assert_eq!(buf.node_count(), 0, "no routes should produce no nodes");
        assert_eq!(buf.edge_count(), 0, "no routes should produce no edges");
    }

    // ── pass_semantic_edges tests ────────────────────────

    #[test]
    fn test_pass_semantic_edges_override_annotation() {
        let dir = tempfile::tempdir().unwrap();
        let project = "p";

        let file = write_file(
            dir.path(),
            "src/Service.java",
            "public class ChildService extends BaseService {\n    @Override\n    public void process() {\n        // child impl\n    }\n}\n",
            Language::Java,
        );

        let mut reg = Registry::new();
        // Register the overriding method
        reg.register(
            "process",
            &format!("{}.src.Service.ChildService.process", project),
            "src/Service.java",
            "Method",
            3,
            5,
        );
        // Register the parent method with the same short name
        reg.register(
            "process",
            &format!("{}.src.BaseService.process", project),
            "src/BaseService.java",
            "Method",
            5,
            10,
        );

        let mut buf = GraphBuffer::new(project);
        let files: Vec<&DiscoveredFile> = vec![&file];
        pass_semantic_edges(&mut buf, &reg, &files, project);

        assert!(
            buf.edge_count() > 0,
            "pass_semantic_edges should create OVERRIDES edges for @Override methods"
        );
    }

    #[test]
    fn test_pass_semantic_edges_delegation_pattern() {
        let dir = tempfile::tempdir().unwrap();
        let project = "p";

        let file = write_file(
            dir.path(),
            "src/proxy.ts",
            "class Proxy {\n    private delegate: Service;\n    handle() {\n        return this.delegate.execute();\n    }\n}\n",
            Language::TypeScript,
        );

        let mut reg = Registry::new();
        reg.register(
            "handle",
            &format!("{}.src.proxy.Proxy.handle", project),
            "src/proxy.ts",
            "Method",
            3,
            5,
        );
        // Register the delegated method in another file
        reg.register(
            "execute",
            &format!("{}.src.service.Service.execute", project),
            "src/service.ts",
            "Method",
            1,
            10,
        );

        let mut buf = GraphBuffer::new(project);
        let files: Vec<&DiscoveredFile> = vec![&file];
        pass_semantic_edges(&mut buf, &reg, &files, project);

        assert!(
            buf.edge_count() > 0,
            "pass_semantic_edges should create DELEGATES_TO edges for delegation patterns"
        );
    }

    #[test]
    fn test_pass_semantic_edges_no_patterns() {
        let dir = tempfile::tempdir().unwrap();
        let project = "p";

        let file = write_file(
            dir.path(),
            "src/plain.ts",
            "function add(a: number, b: number) {\n    return a + b;\n}\n",
            Language::TypeScript,
        );

        let reg = Registry::new();
        let mut buf = GraphBuffer::new(project);
        let files: Vec<&DiscoveredFile> = vec![&file];
        pass_semantic_edges(&mut buf, &reg, &files, project);

        assert_eq!(
            buf.edge_count(),
            0,
            "no override/delegation patterns should produce no edges"
        );
    }

    // ── pass_similarity tests ────────────────────────────

    #[test]
    fn test_pass_similarity_identical_functions() {
        let dir = tempfile::tempdir().unwrap();
        let project = "p";

        // Two identical function bodies (>= SIMILARITY_MIN_LINES each)
        let body = "\
function compute(input) {
    let result = 0;
    for (let i = 0; i < input.length; i++) {
        if (input[i] > 0) {
            result += input[i] * 2;
        } else {
            result -= input[i];
        }
    }
    return result;
}";
        let src_a = format!("{}\n", body);
        let src_b = format!("{}\n", body);

        write_file(dir.path(), "src/a.ts", &src_a, Language::TypeScript);
        write_file(dir.path(), "src/b.ts", &src_b, Language::TypeScript);

        let store = Store::open_in_memory().unwrap();
        store
            .upsert_project(&Project {
                name: project.into(),
                indexed_at: "now".into(),
                root_path: dir.path().to_string_lossy().into(),
            })
            .unwrap();

        store
            .insert_node(&Node {
                id: 0,
                project: project.into(),
                label: "Function".into(),
                name: "compute".into(),
                qualified_name: format!("{}.src.a.compute", project),
                file_path: "src/a.ts".into(),
                start_line: 1,
                end_line: 11,
                properties_json: None,
            })
            .unwrap();
        store
            .insert_node(&Node {
                id: 0,
                project: project.into(),
                label: "Function".into(),
                name: "compute".into(),
                qualified_name: format!("{}.src.b.compute", project),
                file_path: "src/b.ts".into(),
                start_line: 1,
                end_line: 11,
                properties_json: None,
            })
            .unwrap();

        let mut buf = GraphBuffer::new(project);
        pass_similarity(&mut buf, &store, project, dir.path());

        assert!(
            buf.edge_count() > 0,
            "identical functions should produce a SIMILAR_TO edge"
        );
    }

    #[test]
    fn test_pass_similarity_unrelated_functions() {
        let dir = tempfile::tempdir().unwrap();
        let project = "p";

        // Two completely different function bodies
        let src_a = "\
function processData(items) {
    let total = 0;
    for (let i = 0; i < items.length; i++) {
        if (items[i].active) {
            total += items[i].value;
        } else {
            total -= items[i].penalty;
        }
    }
    return total;
}
";
        let src_b = "\
class DatabaseConnection {
    constructor(host, port) {
        this.host = host;
        this.port = port;
        this.connected = false;
        this.retryCount = 0;
        this.maxRetries = 5;
        this.timeout = 3000;
    }
    connect() {
        this.connected = true;
    }
}
";

        write_file(dir.path(), "src/a.ts", src_a, Language::TypeScript);
        write_file(dir.path(), "src/b.ts", src_b, Language::TypeScript);

        let store = Store::open_in_memory().unwrap();
        store
            .upsert_project(&Project {
                name: project.into(),
                indexed_at: "now".into(),
                root_path: dir.path().to_string_lossy().into(),
            })
            .unwrap();

        store
            .insert_node(&Node {
                id: 0,
                project: project.into(),
                label: "Function".into(),
                name: "processData".into(),
                qualified_name: format!("{}.src.a.processData", project),
                file_path: "src/a.ts".into(),
                start_line: 1,
                end_line: 11,
                properties_json: None,
            })
            .unwrap();
        store
            .insert_node(&Node {
                id: 0,
                project: project.into(),
                label: "Function".into(),
                name: "DatabaseConnection".into(),
                qualified_name: format!("{}.src.b.DatabaseConnection", project),
                file_path: "src/b.ts".into(),
                start_line: 1,
                end_line: 13,
                properties_json: None,
            })
            .unwrap();

        let mut buf = GraphBuffer::new(project);
        pass_similarity(&mut buf, &store, project, dir.path());

        assert_eq!(
            buf.edge_count(),
            0,
            "unrelated functions should not produce SIMILAR_TO edges"
        );
    }

    #[test]
    fn test_pass_similarity_skips_short_functions() {
        let dir = tempfile::tempdir().unwrap();
        let project = "p";

        // Two identical but short functions (< SIMILARITY_MIN_LINES = 8)
        let short_body = "\
function add(a, b) {
    return a + b;
}
";
        write_file(dir.path(), "src/a.ts", short_body, Language::TypeScript);
        write_file(dir.path(), "src/b.ts", short_body, Language::TypeScript);

        let store = Store::open_in_memory().unwrap();
        store
            .upsert_project(&Project {
                name: project.into(),
                indexed_at: "now".into(),
                root_path: dir.path().to_string_lossy().into(),
            })
            .unwrap();

        // Both functions span only 3 lines — below MIN_LINES threshold of 8
        store
            .insert_node(&Node {
                id: 0,
                project: project.into(),
                label: "Function".into(),
                name: "add".into(),
                qualified_name: format!("{}.src.a.add", project),
                file_path: "src/a.ts".into(),
                start_line: 1,
                end_line: 3,
                properties_json: None,
            })
            .unwrap();
        store
            .insert_node(&Node {
                id: 0,
                project: project.into(),
                label: "Function".into(),
                name: "add".into(),
                qualified_name: format!("{}.src.b.add", project),
                file_path: "src/b.ts".into(),
                start_line: 1,
                end_line: 3,
                properties_json: None,
            })
            .unwrap();

        let mut buf = GraphBuffer::new(project);
        pass_similarity(&mut buf, &store, project, dir.path());

        assert_eq!(
            buf.edge_count(),
            0,
            "functions shorter than MIN_LINES should be skipped"
        );
    }

    // ── parallel vs sequential extraction tests ──────────

    #[test]
    fn test_parallel_extraction_matches_sequential_typescript() {
        let dir = tempfile::tempdir().unwrap();
        let project = "p";

        let ts_src = "\
export function greet(name: string): string {
    return `Hello, ${name}!`;
}

export class UserService {
    private users: string[] = [];

    addUser(name: string): void {
        this.users.push(name);
    }

    getUsers(): string[] {
        return this.users;
    }
}

export interface Config {
    host: string;
    port: number;
}
";
        let file = write_file(dir.path(), "src/app.ts", ts_src, Language::TypeScript);

        // Sequential extraction
        let mut seq_buf = GraphBuffer::new(project);
        let mut seq_reg = Registry::new();
        crate::extraction::extract_file(&mut seq_buf, &mut seq_reg, project, &file);

        // Parallel extraction
        let par_result = crate::extraction::extract_file_parallel(project, &file);
        assert!(
            par_result.is_some(),
            "parallel extraction should return Some for TypeScript"
        );
        let par_result = par_result.unwrap();

        // Apply parallel result to a buffer for comparison
        let mut par_buf = GraphBuffer::new(project);
        let mut par_reg = Registry::new();
        par_result.apply(&mut par_buf, &mut par_reg);

        // Both should produce the same number of nodes
        assert_eq!(
            seq_buf.node_count(),
            par_buf.node_count(),
            "sequential and parallel extraction should produce the same number of nodes for TypeScript"
        );

        // Both registries should have the same entries
        let seq_names = seq_reg.all_names();
        let par_names = par_reg.all_names();
        let mut seq_sorted = seq_names.clone();
        seq_sorted.sort();
        let mut par_sorted = par_names.clone();
        par_sorted.sort();
        assert_eq!(
            seq_sorted, par_sorted,
            "sequential and parallel extraction should register the same symbol names"
        );
    }

    #[test]
    fn test_parallel_extraction_matches_sequential_python() {
        let dir = tempfile::tempdir().unwrap();
        let project = "p";

        let py_src = "\
class Calculator:
    def add(self, a: int, b: int) -> int:
        return a + b

    def subtract(self, a: int, b: int) -> int:
        return a - b

def factorial(n: int) -> int:
    if n <= 1:
        return 1
    return n * factorial(n - 1)
";
        let file = write_file(dir.path(), "src/calc.py", py_src, Language::Python);

        // Sequential
        let mut seq_buf = GraphBuffer::new(project);
        let mut seq_reg = Registry::new();
        crate::extraction::extract_file(&mut seq_buf, &mut seq_reg, project, &file);

        // Parallel
        let par_result = crate::extraction::extract_file_parallel(project, &file);
        assert!(
            par_result.is_some(),
            "parallel extraction should return Some for Python"
        );
        let par_result = par_result.unwrap();

        let mut par_buf = GraphBuffer::new(project);
        let mut par_reg = Registry::new();
        par_result.apply(&mut par_buf, &mut par_reg);

        assert_eq!(
            seq_buf.node_count(),
            par_buf.node_count(),
            "sequential and parallel extraction should produce the same number of nodes for Python"
        );

        let mut seq_names: Vec<_> = seq_reg.all_names();
        seq_names.sort();
        let mut par_names: Vec<_> = par_reg.all_names();
        par_names.sort();
        assert_eq!(seq_names, par_names, "same symbol names for Python");
    }

    #[test]
    fn test_parallel_extraction_matches_sequential_rust() {
        let dir = tempfile::tempdir().unwrap();
        let project = "p";

        let rs_src = "\
pub struct Config {
    pub host: String,
    pub port: u16,
}

pub fn parse_config(input: &str) -> Config {
    Config {
        host: input.to_string(),
        port: 8080,
    }
}

pub trait Handler {
    fn handle(&self, req: &str) -> String;
}
";
        let file = write_file(dir.path(), "src/config.rs", rs_src, Language::Rust);

        // Sequential
        let mut seq_buf = GraphBuffer::new(project);
        let mut seq_reg = Registry::new();
        crate::extraction::extract_file(&mut seq_buf, &mut seq_reg, project, &file);

        // Parallel
        let par_result = crate::extraction::extract_file_parallel(project, &file);
        assert!(
            par_result.is_some(),
            "parallel extraction should return Some for Rust"
        );
        let par_result = par_result.unwrap();

        let mut par_buf = GraphBuffer::new(project);
        let mut par_reg = Registry::new();
        par_result.apply(&mut par_buf, &mut par_reg);

        assert_eq!(
            seq_buf.node_count(),
            par_buf.node_count(),
            "sequential and parallel extraction should produce the same number of nodes for Rust"
        );

        let mut seq_names: Vec<_> = seq_reg.all_names();
        seq_names.sort();
        let mut par_names: Vec<_> = par_reg.all_names();
        par_names.sort();
        assert_eq!(seq_names, par_names, "same symbol names for Rust");
    }

    #[test]
    fn test_parallel_extraction_returns_none_for_java() {
        let dir = tempfile::tempdir().unwrap();
        let project = "p";

        let java_src = "\
public class Main {
    public static void main(String[] args) {
        System.out.println(\"Hello\");
    }
}
";
        let file = write_file(dir.path(), "src/Main.java", java_src, Language::Java);

        let result = crate::extraction::extract_file_parallel(project, &file);
        assert!(
            result.is_none(),
            "parallel extraction should return None for Java (falls back to serial)"
        );
    }

    #[test]
    fn test_parallel_extraction_result_contains_expected_data() {
        let dir = tempfile::tempdir().unwrap();
        let project = "p";

        let ts_src = "\
export function hello(name: string): string {
    return `Hello, ${name}!`;
}

export class Greeter {
    greet(): string {
        return hello('world');
    }
}
";
        let file = write_file(dir.path(), "src/greeter.ts", ts_src, Language::TypeScript);

        let result = crate::extraction::extract_file_parallel(project, &file);
        assert!(result.is_some());
        let result = result.unwrap();

        // Should have nodes (functions, classes, module)
        assert!(
            !result.nodes.is_empty(),
            "ExtractionResult should contain nodes"
        );

        // Should have registry entries
        assert!(
            !result.registry_entries.is_empty(),
            "ExtractionResult should contain registry entries"
        );

        // Check that we have a Module node
        assert!(
            result.nodes.iter().any(|n| n.label == "Module"),
            "ExtractionResult should contain a Module node"
        );

        // Check that node fields are populated
        for node in &result.nodes {
            assert!(!node.name.is_empty(), "node name should not be empty");
            assert!(
                !node.qualified_name.is_empty(),
                "node qualified_name should not be empty"
            );
            assert!(
                !node.file_path.is_empty(),
                "node file_path should not be empty"
            );
            assert!(node.start_line >= 1, "node start_line should be >= 1");
            assert!(
                node.end_line >= node.start_line,
                "node end_line should be >= start_line"
            );
        }

        // Check code snippets are populated for non-Module nodes
        assert!(
            !result.code_snippets.is_empty(),
            "ExtractionResult should contain code snippets for FTS"
        );
    }

    #[test]
    fn test_parallel_extraction_multi_file_consistency() {
        let dir = tempfile::tempdir().unwrap();
        let project = "p";

        let ts_src = "export function fetchData(url: string): Promise<any> {\n    return fetch(url).then(r => r.json());\n}\n";
        let py_src =
            "def process(data: list) -> dict:\n    return {item: len(item) for item in data}\n";
        let rs_src = "pub fn compute(x: i32, y: i32) -> i32 {\n    x * y + 1\n}\n";

        let ts_file = write_file(dir.path(), "src/api.ts", ts_src, Language::TypeScript);
        let py_file = write_file(dir.path(), "src/process.py", py_src, Language::Python);
        let rs_file = write_file(dir.path(), "src/math.rs", rs_src, Language::Rust);

        let files = vec![&ts_file, &py_file, &rs_file];

        for file in &files {
            // Sequential
            let mut seq_buf = GraphBuffer::new(project);
            let mut seq_reg = Registry::new();
            crate::extraction::extract_file(&mut seq_buf, &mut seq_reg, project, file);

            // Parallel
            let par_result = crate::extraction::extract_file_parallel(project, file);
            assert!(
                par_result.is_some(),
                "parallel should return Some for {:?}",
                file.language
            );
            let par_result = par_result.unwrap();

            let mut par_buf = GraphBuffer::new(project);
            let mut par_reg = Registry::new();
            par_result.apply(&mut par_buf, &mut par_reg);

            assert_eq!(
                seq_buf.node_count(),
                par_buf.node_count(),
                "node count mismatch for {:?} file {}",
                file.language,
                file.rel_path
            );

            let mut seq_names: Vec<_> = seq_reg.all_names();
            seq_names.sort();
            let mut par_names: Vec<_> = par_reg.all_names();
            par_names.sort();
            assert_eq!(
                seq_names, par_names,
                "registry name mismatch for {:?} file {}",
                file.language, file.rel_path
            );
        }
    }

    // ── pass_events tests ────────────────────────────────

    #[test]
    fn test_pass_events_socket_emit_creates_channel_and_emits_edge() {
        let dir = tempfile::tempdir().unwrap();
        let project = "p";

        let file = write_file(
            dir.path(),
            "src/server.ts",
            "function sendUpdate() {\n    socket.emit('user:updated', data);\n}\n",
            Language::TypeScript,
        );

        let mut reg = Registry::new();
        reg.register(
            "sendUpdate",
            &format!("{}.src.server.sendUpdate", project),
            "src/server.ts",
            "Function",
            1,
            3,
        );

        let mut buf = GraphBuffer::new(project);
        let files: Vec<&DiscoveredFile> = vec![&file];
        pass_events(&mut buf, &reg, &files, project);

        assert_eq!(
            buf.node_count(),
            1,
            "should create 1 Channel node for 'user:updated'"
        );
        assert_eq!(
            buf.edge_count(),
            1,
            "should create 1 EMITS edge from sendUpdate to Channel node"
        );
    }

    #[test]
    fn test_pass_events_socket_on_creates_channel_and_listens_edge() {
        let dir = tempfile::tempdir().unwrap();
        let project = "p";

        let file = write_file(
            dir.path(),
            "src/client.ts",
            "function setupListeners() {\n    socket.on('user:updated', handleUpdate);\n}\n",
            Language::TypeScript,
        );

        let mut reg = Registry::new();
        reg.register(
            "setupListeners",
            &format!("{}.src.client.setupListeners", project),
            "src/client.ts",
            "Function",
            1,
            3,
        );

        let mut buf = GraphBuffer::new(project);
        let files: Vec<&DiscoveredFile> = vec![&file];
        pass_events(&mut buf, &reg, &files, project);

        assert_eq!(
            buf.node_count(),
            1,
            "should create 1 Channel node for 'user:updated'"
        );
        assert_eq!(
            buf.edge_count(),
            1,
            "should create 1 LISTENS edge from setupListeners to Channel node"
        );
    }

    #[test]
    fn test_pass_events_same_event_different_files_shares_channel_node() {
        let dir = tempfile::tempdir().unwrap();
        let project = "p";

        // File A emits the event
        let file_a = write_file(
            dir.path(),
            "src/emitter.ts",
            "function notify() {\n    socket.emit('order:placed', order);\n}\n",
            Language::TypeScript,
        );
        // File B listens for the same event
        let file_b = write_file(
            dir.path(),
            "src/handler.ts",
            "function listen() {\n    socket.on('order:placed', processOrder);\n}\n",
            Language::TypeScript,
        );

        let mut reg = Registry::new();
        reg.register(
            "notify",
            &format!("{}.src.emitter.notify", project),
            "src/emitter.ts",
            "Function",
            1,
            3,
        );
        reg.register(
            "listen",
            &format!("{}.src.handler.listen", project),
            "src/handler.ts",
            "Function",
            1,
            3,
        );

        let mut buf = GraphBuffer::new(project);
        let files: Vec<&DiscoveredFile> = vec![&file_a, &file_b];
        pass_events(&mut buf, &reg, &files, project);

        // Only 1 Channel node for 'order:placed', but 2 edges (EMITS + LISTENS)
        assert_eq!(
            buf.node_count(),
            1,
            "same event name should produce only 1 Channel node"
        );
        assert_eq!(
            buf.edge_count(),
            2,
            "should create 1 EMITS + 1 LISTENS edge to the shared Channel node"
        );
    }

    #[test]
    fn test_pass_events_redis_publish_subscribe() {
        let dir = tempfile::tempdir().unwrap();
        let project = "p";

        let file = write_file(
            dir.path(),
            "src/pubsub.ts",
            "function pub() {\n    client.publish('notifications', msg);\n}\nfunction sub() {\n    client.subscribe('notifications');\n}\n",
            Language::TypeScript,
        );

        let mut reg = Registry::new();
        reg.register(
            "pub",
            &format!("{}.src.pubsub.pub", project),
            "src/pubsub.ts",
            "Function",
            1,
            3,
        );
        reg.register(
            "sub",
            &format!("{}.src.pubsub.sub", project),
            "src/pubsub.ts",
            "Function",
            4,
            6,
        );

        let mut buf = GraphBuffer::new(project);
        let files: Vec<&DiscoveredFile> = vec![&file];
        pass_events(&mut buf, &reg, &files, project);

        // 1 Channel node for 'notifications'
        assert_eq!(
            buf.node_count(),
            1,
            "Redis pub/sub should create 1 Channel node for 'notifications'"
        );
        // publish matches EMITS via REDIS_PUBSUB_RE, subscribe matches LISTENS via
        // both LISTEN_RE and REDIS_PUBSUB_RE, so at least 2 edges (EMITS + LISTENS)
        assert!(
            buf.edge_count() >= 2,
            "Redis pub/sub should create at least EMITS + LISTENS edges, got {}",
            buf.edge_count()
        );
    }

    #[test]
    fn test_pass_events_dynamic_event_names_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let project = "p";

        let file = write_file(
            dir.path(),
            "src/dynamic.ts",
            "function emitDynamic() {\n    socket.emit('${eventName}', data);\n    socket.emit('valid-event', data);\n}\n",
            Language::TypeScript,
        );

        let mut reg = Registry::new();
        reg.register(
            "emitDynamic",
            &format!("{}.src.dynamic.emitDynamic", project),
            "src/dynamic.ts",
            "Function",
            1,
            4,
        );

        let mut buf = GraphBuffer::new(project);
        let files: Vec<&DiscoveredFile> = vec![&file];
        pass_events(&mut buf, &reg, &files, project);

        // Dynamic '${eventName}' should be skipped, only 'valid-event' should be detected
        assert_eq!(
            buf.node_count(),
            1,
            "dynamic event names should be skipped, only 'valid-event' should create a Channel node"
        );
        assert_eq!(
            buf.edge_count(),
            1,
            "only 1 EMITS edge for the static event name"
        );
    }

    #[test]
    fn test_pass_events_no_event_patterns_produces_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let project = "p";

        let file = write_file(
            dir.path(),
            "src/utils.ts",
            "export function add(a: number, b: number) {\n    return a + b;\n}\n",
            Language::TypeScript,
        );

        let reg = Registry::new();
        let mut buf = GraphBuffer::new(project);
        let files: Vec<&DiscoveredFile> = vec![&file];
        pass_events(&mut buf, &reg, &files, project);

        assert_eq!(
            buf.node_count(),
            0,
            "files without event patterns should produce no Channel nodes"
        );
        assert_eq!(
            buf.edge_count(),
            0,
            "files without event patterns should produce no edges"
        );
    }

    // ── pass_k8s tests ───────────────────────────────────

    #[test]
    fn test_pass_k8s_deployment_with_image() {
        let dir = tempfile::tempdir().unwrap();
        let project = "p";

        let yaml = "\
apiVersion: apps/v1
kind: Deployment
metadata:
  name: my-app
  namespace: production
spec:
  replicas: 3
  template:
    spec:
      containers:
        - name: app
          image: nginx:1.25
";
        let file = write_file(dir.path(), "k8s/deploy.yaml", yaml, Language::Yaml);

        let mut buf = GraphBuffer::new(project);
        let files: Vec<&DiscoveredFile> = vec![&file];
        pass_k8s(&mut buf, &files, project);

        // Should create a Resource node for the Deployment and a Docker_Image node
        assert!(
            buf.node_count() >= 2,
            "should create Resource + Docker_Image nodes, got {}",
            buf.node_count()
        );
        // Should create a USES_IMAGE edge
        assert!(
            buf.edge_count() >= 1,
            "should create USES_IMAGE edge, got {}",
            buf.edge_count()
        );
    }

    #[test]
    fn test_pass_k8s_configmap_creates_resource_node() {
        let dir = tempfile::tempdir().unwrap();
        let project = "p";

        let yaml = "\
apiVersion: v1
kind: ConfigMap
metadata:
  name: app-config
data:
  DATABASE_URL: postgres://localhost/db
";
        let file = write_file(dir.path(), "k8s/configmap.yaml", yaml, Language::Yaml);

        let mut buf = GraphBuffer::new(project);
        let files: Vec<&DiscoveredFile> = vec![&file];
        pass_k8s(&mut buf, &files, project);

        // Should create a Resource node for the ConfigMap
        assert_eq!(
            buf.node_count(),
            1,
            "should create 1 Resource node for ConfigMap"
        );
    }

    // ── pass_kustomize tests ─────────────────────────────

    #[test]
    fn test_pass_kustomize_creates_module_and_imports() {
        let dir = tempfile::tempdir().unwrap();
        let project = "p";

        let yaml = "\
apiVersion: kustomize.config.k8s.io/v1beta1
kind: Kustomization
resources:
  - deployment.yaml
  - service.yaml
";
        let file = write_file(
            dir.path(),
            "overlays/prod/kustomization.yaml",
            yaml,
            Language::Kustomize,
        );

        let mut buf = GraphBuffer::new(project);
        let files: Vec<&DiscoveredFile> = vec![&file];
        pass_kustomize(&mut buf, &files, project);

        // Should create a Module node for the overlay
        assert_eq!(
            buf.node_count(),
            1,
            "should create 1 Module node for kustomize overlay"
        );
        // Should create 2 IMPORTS edges (one for each resource)
        assert_eq!(
            buf.edge_count(),
            2,
            "should create 2 IMPORTS edges for resources"
        );
    }

    // ── pass_infrascan tests ─────────────────────────────

    #[test]
    fn test_pass_infrascan_dockerfile_creates_image_node() {
        let dir = tempfile::tempdir().unwrap();
        let project = "p";

        let dockerfile = "FROM node:18-alpine\nWORKDIR /app\nCOPY . .\nRUN npm install\n";
        let file = write_file(dir.path(), "Dockerfile", dockerfile, Language::Dockerfile);

        let mut buf = GraphBuffer::new(project);
        let files: Vec<&DiscoveredFile> = vec![&file];
        pass_infrascan(&mut buf, &files, project);

        // Should create a Docker_Image node for node:18-alpine
        assert_eq!(buf.node_count(), 1, "should create 1 Docker_Image node");
    }

    #[test]
    fn test_pass_infrascan_multistage_dockerfile_builds_from() {
        let dir = tempfile::tempdir().unwrap();
        let project = "p";

        let dockerfile = "\
FROM node:18-alpine AS builder
WORKDIR /app
COPY . .
RUN npm run build

FROM nginx:alpine AS runtime
COPY --from=builder /app/dist /usr/share/nginx/html
";
        let file = write_file(dir.path(), "Dockerfile", dockerfile, Language::Dockerfile);

        let mut buf = GraphBuffer::new(project);
        let files: Vec<&DiscoveredFile> = vec![&file];
        pass_infrascan(&mut buf, &files, project);

        // Should create 2 Docker_Image nodes (node:18-alpine, nginx:alpine)
        assert_eq!(
            buf.node_count(),
            2,
            "should create 2 Docker_Image nodes for multi-stage build"
        );
        // Should create BUILDS_FROM edge (runtime -> builder) and COPIES_FROM edge
        assert!(
            buf.edge_count() >= 1,
            "should create BUILDS_FROM/COPIES_FROM edges, got {}",
            buf.edge_count()
        );
    }

    #[test]
    fn test_infra_passes_no_infra_files_produces_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let project = "p";

        let file = write_file(
            dir.path(),
            "src/app.ts",
            "export function main() {}\n",
            Language::TypeScript,
        );

        let mut k8s_buf = GraphBuffer::new(project);
        let mut kust_buf = GraphBuffer::new(project);
        let mut infra_buf = GraphBuffer::new(project);
        let files: Vec<&DiscoveredFile> = vec![&file];

        pass_k8s(&mut k8s_buf, &files, project);
        pass_kustomize(&mut kust_buf, &files, project);
        pass_infrascan(&mut infra_buf, &files, project);

        assert_eq!(k8s_buf.node_count(), 0, "no YAML files → no k8s nodes");
        assert_eq!(k8s_buf.edge_count(), 0, "no YAML files → no k8s edges");
        assert_eq!(kust_buf.node_count(), 0, "no kustomization → no nodes");
        assert_eq!(kust_buf.edge_count(), 0, "no kustomization → no edges");
        assert_eq!(infra_buf.node_count(), 0, "no Dockerfiles → no nodes");
        assert_eq!(infra_buf.edge_count(), 0, "no Dockerfiles → no edges");
    }

    // ── manifest parsing tests ─────────────────────────

    #[test]
    fn test_parse_package_json_dependencies() {
        let dir = tempfile::tempdir().unwrap();
        let project = "proj";
        let content = r#"{
            "dependencies": { "express": "^4.0.0", "@myorg/utils": "1.0.0" },
            "devDependencies": { "jest": "^29.0.0" },
            "peerDependencies": { "react": "^18.0.0" }
        }"#;
        let file = write_file(dir.path(), "package.json", content, Language::Json);
        let files: Vec<&DiscoveredFile> = vec![&file];
        let map = pass_pkgmap(&files, project);

        assert_eq!(map.get("express").unwrap(), "proj.node_modules.express");
        assert_eq!(
            map.get("@myorg/utils").unwrap(),
            "proj.node_modules.@myorg/utils"
        );
        assert_eq!(map.get("jest").unwrap(), "proj.node_modules.jest");
        assert_eq!(map.get("react").unwrap(), "proj.node_modules.react");
        assert_eq!(map.len(), 4);
    }

    #[test]
    fn test_parse_go_mod_require_directives() {
        let dir = tempfile::tempdir().unwrap();
        let project = "proj";
        let content = "\
module example.com/myapp

go 1.21

require (
\tgithub.com/gin-gonic/gin v1.9.1
\tgithub.com/stretchr/testify v1.8.4 // indirect
)

require golang.org/x/text v0.14.0
";
        let file = write_file(dir.path(), "go.mod", content, Language::Go);
        let files: Vec<&DiscoveredFile> = vec![&file];
        let map = pass_pkgmap(&files, project);

        assert_eq!(
            map.get("github.com/gin-gonic/gin").unwrap(),
            "proj.vendor.github.com/gin-gonic/gin"
        );
        assert_eq!(
            map.get("github.com/stretchr/testify").unwrap(),
            "proj.vendor.github.com/stretchr/testify"
        );
        assert_eq!(
            map.get("golang.org/x/text").unwrap(),
            "proj.vendor.golang.org/x/text"
        );
        assert_eq!(map.len(), 3);
    }

    #[test]
    fn test_parse_cargo_toml_dependencies() {
        let dir = tempfile::tempdir().unwrap();
        let project = "proj";
        let content = r#"
[package]
name = "myapp"
version = "0.1.0"

[dependencies]
serde = "1.0"
tokio = { version = "1", features = ["full"] }

[dev-dependencies]
tempfile = "3"

[build-dependencies]
cc = "1.0"
"#;
        let file = write_file(dir.path(), "Cargo.toml", content, Language::Rust);
        let files: Vec<&DiscoveredFile> = vec![&file];
        let map = pass_pkgmap(&files, project);

        assert_eq!(map.get("serde").unwrap(), "proj.deps.serde");
        assert_eq!(map.get("tokio").unwrap(), "proj.deps.tokio");
        assert_eq!(map.get("tempfile").unwrap(), "proj.deps.tempfile");
        assert_eq!(map.get("cc").unwrap(), "proj.deps.cc");
        assert_eq!(map.len(), 4);
    }

    #[test]
    fn test_parse_pyproject_toml_pep621() {
        let dir = tempfile::tempdir().unwrap();
        let project = "proj";
        let content = r#"
[project]
name = "myapp"
dependencies = [
    "requests>=2.28",
    "numpy[extra]",
    "flask",
]
"#;
        let file = write_file(dir.path(), "pyproject.toml", content, Language::Python);
        let files: Vec<&DiscoveredFile> = vec![&file];
        let map = pass_pkgmap(&files, project);

        assert_eq!(map.get("requests").unwrap(), "proj.site_packages.requests");
        assert_eq!(map.get("numpy").unwrap(), "proj.site_packages.numpy");
        assert_eq!(map.get("flask").unwrap(), "proj.site_packages.flask");
        assert_eq!(map.len(), 3);
    }

    #[test]
    fn test_parse_pyproject_toml_poetry() {
        let dir = tempfile::tempdir().unwrap();
        let project = "proj";
        let content = r#"
[tool.poetry]
name = "myapp"

[tool.poetry.dependencies]
python = "^3.11"
django = "^4.2"
celery = "^5.3"
"#;
        let file = write_file(dir.path(), "pyproject.toml", content, Language::Python);
        let files: Vec<&DiscoveredFile> = vec![&file];
        let map = pass_pkgmap(&files, project);

        // python should be skipped
        assert!(!map.contains_key("python"));
        assert_eq!(map.get("django").unwrap(), "proj.site_packages.django");
        assert_eq!(map.get("celery").unwrap(), "proj.site_packages.celery");
        assert_eq!(map.len(), 2);
    }

    #[test]
    fn test_parse_composer_json() {
        let dir = tempfile::tempdir().unwrap();
        let project = "proj";
        let content = r#"{
            "require": {
                "php": "^8.1",
                "laravel/framework": "^10.0",
                "ext-json": "*"
            },
            "require-dev": {
                "phpunit/phpunit": "^10.0"
            }
        }"#;
        let file = write_file(dir.path(), "composer.json", content, Language::Php);
        let files: Vec<&DiscoveredFile> = vec![&file];
        let map = pass_pkgmap(&files, project);

        // php and ext-* should be skipped
        assert!(!map.contains_key("php"));
        assert!(!map.contains_key("ext-json"));
        assert_eq!(
            map.get("laravel/framework").unwrap(),
            "proj.vendor.laravel/framework"
        );
        assert_eq!(
            map.get("phpunit/phpunit").unwrap(),
            "proj.vendor.phpunit/phpunit"
        );
        assert_eq!(map.len(), 2);
    }

    #[test]
    fn test_parse_pubspec_yaml() {
        let dir = tempfile::tempdir().unwrap();
        let project = "proj";
        let content = "\
name: my_app
dependencies:
  http: ^0.13.0
  provider: ^6.0.0
dev_dependencies:
  test: ^1.24.0
";
        let file = write_file(dir.path(), "pubspec.yaml", content, Language::Dart);
        let files: Vec<&DiscoveredFile> = vec![&file];
        let map = pass_pkgmap(&files, project);

        assert_eq!(map.get("http").unwrap(), "proj.packages.http");
        assert_eq!(map.get("provider").unwrap(), "proj.packages.provider");
        assert_eq!(map.get("test").unwrap(), "proj.packages.test");
        assert_eq!(map.len(), 3);
    }

    #[test]
    fn test_parse_pom_xml() {
        let dir = tempfile::tempdir().unwrap();
        let project = "proj";
        let content = r#"<?xml version="1.0"?>
<project>
  <dependencies>
    <dependency>
      <groupId>org.springframework.boot</groupId>
      <artifactId>spring-boot-starter-web</artifactId>
    </dependency>
    <dependency>
      <groupId>junit</groupId>
      <artifactId>junit</artifactId>
    </dependency>
  </dependencies>
</project>"#;
        let file = write_file(dir.path(), "pom.xml", content, Language::Java);
        let files: Vec<&DiscoveredFile> = vec![&file];
        let map = pass_pkgmap(&files, project);

        assert_eq!(
            map.get("org.springframework.boot:spring-boot-starter-web")
                .unwrap(),
            "proj.deps.org.springframework.boot.spring-boot-starter-web"
        );
        assert_eq!(map.get("junit:junit").unwrap(), "proj.deps.junit.junit");
        assert_eq!(map.len(), 2);
    }

    #[test]
    fn test_parse_build_gradle() {
        let dir = tempfile::tempdir().unwrap();
        let project = "proj";
        let content = r#"
dependencies {
    implementation 'org.springframework:spring-core:5.3.0'
    testImplementation 'junit:junit:4.13'
}
"#;
        let file = write_file(dir.path(), "build.gradle", content, Language::Java);
        let files: Vec<&DiscoveredFile> = vec![&file];
        let map = pass_pkgmap(&files, project);

        assert!(map.contains_key("org.springframework:spring-core"));
        assert!(map.contains_key("junit:junit"));
        assert_eq!(map.len(), 2);
    }

    #[test]
    fn test_parse_mix_exs() {
        let dir = tempfile::tempdir().unwrap();
        let project = "proj";
        let content = r#"
defmodule MyApp.MixProject do
  defp deps do
    [
      {:phoenix, "~> 1.7"},
      {:ecto, "~> 3.10"},
    ]
  end
end
"#;
        let file = write_file(dir.path(), "mix.exs", content, Language::Elixir);
        let files: Vec<&DiscoveredFile> = vec![&file];
        let map = pass_pkgmap(&files, project);

        assert_eq!(map.get("phoenix").unwrap(), "proj.deps.phoenix");
        assert_eq!(map.get("ecto").unwrap(), "proj.deps.ecto");
        assert_eq!(map.len(), 2);
    }

    #[test]
    fn test_parse_gemspec() {
        let dir = tempfile::tempdir().unwrap();
        let project = "proj";
        let content = r#"
Gem::Specification.new do |s|
  s.name = "mygem"
  s.add_runtime_dependency('rails', '~> 7.0')
  s.add_development_dependency('rspec', '~> 3.12')
end
"#;
        let file = write_file(dir.path(), "mygem.gemspec", content, Language::Ruby);
        let files: Vec<&DiscoveredFile> = vec![&file];
        let map = pass_pkgmap(&files, project);

        assert_eq!(map.get("rails").unwrap(), "proj.gems.rails");
        assert_eq!(map.get("rspec").unwrap(), "proj.gems.rspec");
        assert_eq!(map.len(), 2);
    }

    #[test]
    fn test_parse_setup_py() {
        let dir = tempfile::tempdir().unwrap();
        let project = "proj";
        let content = r#"
from setuptools import setup
setup(
    name="myapp",
    install_requires=[
        'requests>=2.28',
        'click',
    ],
)
"#;
        let file = write_file(dir.path(), "setup.py", content, Language::Python);
        let files: Vec<&DiscoveredFile> = vec![&file];
        let map = pass_pkgmap(&files, project);

        assert_eq!(map.get("requests").unwrap(), "proj.site_packages.requests");
        assert_eq!(map.get("click").unwrap(), "proj.site_packages.click");
        assert_eq!(map.len(), 2);
    }

    #[test]
    fn test_pass_pkgmap_multiple_manifests() {
        let dir = tempfile::tempdir().unwrap();
        let project = "proj";

        let pkg_json = write_file(
            dir.path(),
            "package.json",
            r#"{"dependencies": {"lodash": "^4.0"}}"#,
            Language::Json,
        );
        let cargo = write_file(
            dir.path(),
            "Cargo.toml",
            "[dependencies]\nserde = \"1.0\"\n",
            Language::Rust,
        );
        let files: Vec<&DiscoveredFile> = vec![&pkg_json, &cargo];
        let map = pass_pkgmap(&files, project);

        assert_eq!(map.get("lodash").unwrap(), "proj.node_modules.lodash");
        assert_eq!(map.get("serde").unwrap(), "proj.deps.serde");
        assert_eq!(map.len(), 2);
    }

    #[test]
    fn test_pass_pkgmap_invalid_json_skips_gracefully() {
        let dir = tempfile::tempdir().unwrap();
        let project = "proj";
        let file = write_file(
            dir.path(),
            "package.json",
            "not valid json{{{",
            Language::Json,
        );
        let files: Vec<&DiscoveredFile> = vec![&file];
        let map = pass_pkgmap(&files, project);

        assert!(map.is_empty(), "invalid JSON should produce empty map");
    }

    // ── resolve_bare_or_default tests ────────────────────

    #[test]
    fn test_resolve_bare_or_default_with_pkgmap() {
        let mut pkg_map = PackageMap::new();
        pkg_map.insert(
            "express".to_string(),
            "proj.node_modules.express".to_string(),
        );
        pkg_map.insert(
            "@myorg/utils".to_string(),
            "proj.node_modules.@myorg/utils".to_string(),
        );

        // Direct match
        let result = resolve_bare_or_default("express", Some(&pkg_map), || "fallback".to_string());
        assert_eq!(result, "proj.node_modules.express");

        // Scoped package match
        let result =
            resolve_bare_or_default("@myorg/utils", Some(&pkg_map), || "fallback".to_string());
        assert_eq!(result, "proj.node_modules.@myorg/utils");

        // Scoped package subpath — should resolve to base package
        let result = resolve_bare_or_default("@myorg/utils/sub/path", Some(&pkg_map), || {
            "fallback".to_string()
        });
        assert_eq!(result, "proj.node_modules.@myorg/utils");
    }

    #[test]
    fn test_resolve_bare_or_default_fallback() {
        let pkg_map = PackageMap::new();

        // Specifier not in map — should fall back to default
        let result =
            resolve_bare_or_default("unknown-pkg", Some(&pkg_map), || "default.qn".to_string());
        assert_eq!(result, "default.qn");
    }

    #[test]
    fn test_resolve_bare_or_default_no_pkgmap() {
        // No PackageMap at all — should use default
        let result = resolve_bare_or_default("express", None, || "default.qn".to_string());
        assert_eq!(result, "default.qn");
    }

    // ── compile commands tests ──────────────────────────

    #[test]
    fn test_compile_commands_arguments_array_format() {
        let dir = tempfile::tempdir().unwrap();
        let cc = serde_json::json!([
            {
                "directory": dir.path().to_string_lossy().to_string(),
                "file": dir.path().join("src/main.c").to_string_lossy().to_string(),
                "arguments": ["gcc", "-Iinclude", "-I", "vendor/lib", "-DNDEBUG", "-DVERSION=2", "-std=c11", "-o", "main.o", "src/main.c"]
            }
        ]);
        std::fs::write(
            dir.path().join("compile_commands.json"),
            serde_json::to_string(&cc).unwrap(),
        )
        .unwrap();

        let map = pass_compile_commands(dir.path());
        assert_eq!(map.len(), 1);

        let ctx = map.get("src/main.c").unwrap();
        assert_eq!(ctx.include_paths, vec!["include", "vendor/lib"]);
        assert_eq!(ctx.defines.len(), 2);
        assert_eq!(ctx.defines[0], ("NDEBUG".to_string(), None));
        assert_eq!(
            ctx.defines[1],
            ("VERSION".to_string(), Some("2".to_string()))
        );
        assert_eq!(ctx.std_flag.as_deref(), Some("c11"));
    }

    #[test]
    fn test_compile_commands_command_string_format() {
        let dir = tempfile::tempdir().unwrap();
        let cc = serde_json::json!([
            {
                "directory": dir.path().to_string_lossy().to_string(),
                "file": dir.path().join("lib.cpp").to_string_lossy().to_string(),
                "command": "g++ -I/usr/local/include -DUSE_SSL -std=c++17 -c lib.cpp"
            }
        ]);
        std::fs::write(
            dir.path().join("compile_commands.json"),
            serde_json::to_string(&cc).unwrap(),
        )
        .unwrap();

        let map = pass_compile_commands(dir.path());
        assert_eq!(map.len(), 1);

        let ctx = map.get("lib.cpp").unwrap();
        assert_eq!(ctx.include_paths, vec!["/usr/local/include"]);
        assert_eq!(ctx.defines, vec![("USE_SSL".to_string(), None)]);
        assert_eq!(ctx.std_flag.as_deref(), Some("c++17"));
    }

    #[test]
    fn test_compile_commands_flag_extraction() {
        // Test parse_compile_args directly for thorough flag coverage
        let args: Vec<String> = vec![
            "clang",
            "-I",
            "path/a",
            "-Ipath/b",
            "-DFOO",
            "-D",
            "BAR=hello",
            "-DBAZ=1",
            "-std=c99",
            "-Wall",
            "-O2",
        ]
        .into_iter()
        .map(String::from)
        .collect();

        let ctx = parse_compile_args(&args);
        assert_eq!(ctx.include_paths, vec!["path/a", "path/b"]);
        assert_eq!(ctx.defines.len(), 3);
        assert_eq!(ctx.defines[0], ("FOO".to_string(), None));
        assert_eq!(
            ctx.defines[1],
            ("BAR".to_string(), Some("hello".to_string()))
        );
        assert_eq!(ctx.defines[2], ("BAZ".to_string(), Some("1".to_string())));
        assert_eq!(ctx.std_flag.as_deref(), Some("c99"));
    }

    #[test]
    fn test_compile_commands_missing_file() {
        let dir = tempfile::tempdir().unwrap();
        // No compile_commands.json created
        let map = pass_compile_commands(dir.path());
        assert!(
            map.is_empty(),
            "missing compile_commands.json should return empty map"
        );
    }

    // ── pass_gitdiff / pass_githistory tests ─────────────

    #[cfg(feature = "git-history")]
    mod git_history_tests {
        use super::*;

        /// Helper: create a git repo, add a file, and make an initial commit.
        /// Returns the repo and the tempdir (must keep tempdir alive).
        fn init_repo_with_commit(
            dir: &std::path::Path,
            files: &[(&str, &str)],
            message: &str,
        ) -> git2::Repository {
            let repo = git2::Repository::init(dir).unwrap();

            // Configure a dummy author for commits
            let mut config = repo.config().unwrap();
            config.set_str("user.name", "Test User").unwrap();
            config.set_str("user.email", "test@example.com").unwrap();
            drop(config);

            for (rel_path, content) in files {
                let abs = dir.join(rel_path);
                if let Some(parent) = abs.parent() {
                    std::fs::create_dir_all(parent).unwrap();
                }
                std::fs::write(&abs, content).unwrap();
            }

            // Stage all files
            let mut index = repo.index().unwrap();
            index
                .add_all(["*"].iter(), git2::IndexAddOption::DEFAULT, None)
                .unwrap();
            index.write().unwrap();
            let tree_oid = index.write_tree().unwrap();
            drop(index);

            {
                let tree = repo.find_tree(tree_oid).unwrap();
                let sig = repo.signature().unwrap();
                repo.commit(Some("HEAD"), &sig, &sig, message, &tree, &[])
                    .unwrap();
            }

            repo
        }

        /// Helper: add another commit on top of an existing repo.
        fn add_commit(
            repo: &git2::Repository,
            dir: &std::path::Path,
            files: &[(&str, &str)],
            message: &str,
        ) {
            for (rel_path, content) in files {
                let abs = dir.join(rel_path);
                if let Some(parent) = abs.parent() {
                    std::fs::create_dir_all(parent).unwrap();
                }
                std::fs::write(&abs, content).unwrap();
            }

            let mut index = repo.index().unwrap();
            index
                .add_all(["*"].iter(), git2::IndexAddOption::DEFAULT, None)
                .unwrap();
            index.write().unwrap();
            let tree_oid = index.write_tree().unwrap();
            drop(index);

            let tree = repo.find_tree(tree_oid).unwrap();
            let sig = repo.signature().unwrap();
            let parent = repo.head().unwrap().peel_to_commit().unwrap();

            repo.commit(Some("HEAD"), &sig, &sig, message, &tree, &[&parent])
                .unwrap();
        }

        #[test]
        fn test_pass_gitdiff_uncommitted_changes_creates_modified_edges() {
            let dir = tempfile::tempdir().unwrap();
            let project = "p";

            // Create repo with initial commit
            let _repo =
                init_repo_with_commit(dir.path(), &[("src/main.rs", "fn main() {}\n")], "initial");

            // Make an uncommitted change
            std::fs::write(
                dir.path().join("src/main.rs"),
                "fn main() { println!(\"hi\"); }\n",
            )
            .unwrap();

            let mut buf = GraphBuffer::new(project);
            super::super::pass_gitdiff(&mut buf, project, dir.path());

            // Should create a GitDiff summary node + MODIFIED edge
            assert!(
                buf.node_count() >= 1,
                "pass_gitdiff should create a GitDiff summary node, got {}",
                buf.node_count()
            );
            assert!(
                buf.edge_count() >= 1,
                "pass_gitdiff should create MODIFIED edges for uncommitted changes, got {}",
                buf.edge_count()
            );
        }

        #[test]
        fn test_pass_gitdiff_non_git_directory_skips_gracefully() {
            let dir = tempfile::tempdir().unwrap();
            let project = "p";

            // No git init — just a plain directory
            std::fs::write(dir.path().join("file.txt"), "hello").unwrap();

            let mut buf = GraphBuffer::new(project);
            super::super::pass_gitdiff(&mut buf, project, dir.path());

            // Should produce nothing and not panic
            assert_eq!(
                buf.node_count(),
                0,
                "pass_gitdiff on non-git dir should produce no nodes"
            );
            assert_eq!(
                buf.edge_count(),
                0,
                "pass_gitdiff on non-git dir should produce no edges"
            );
        }

        #[test]
        fn test_pass_githistory_change_frequency_and_churn() {
            let dir = tempfile::tempdir().unwrap();
            let project = "p";

            // Create repo with initial commit
            let repo = init_repo_with_commit(
                dir.path(),
                &[("src/a.rs", "fn a() {}\n"), ("src/b.rs", "fn b() {}\n")],
                "initial commit",
            );

            // Second commit: modify a.rs
            add_commit(
                &repo,
                dir.path(),
                &[("src/a.rs", "fn a() { println!(\"v2\"); }\n")],
                "update a",
            );

            // Third commit: modify a.rs again
            add_commit(
                &repo,
                dir.path(),
                &[("src/a.rs", "fn a() { println!(\"v3\"); }\n")],
                "update a again",
            );

            // Set up store with File nodes
            let store = test_store(project);
            store
                .insert_node(&Node {
                    id: 0,
                    project: project.into(),
                    label: "File".into(),
                    name: "a.rs".into(),
                    qualified_name: format!("{}.src.a", project),
                    file_path: "src/a.rs".into(),
                    start_line: 0,
                    end_line: 0,
                    properties_json: None,
                })
                .unwrap();
            store
                .insert_node(&Node {
                    id: 0,
                    project: project.into(),
                    label: "File".into(),
                    name: "b.rs".into(),
                    qualified_name: format!("{}.src.b", project),
                    file_path: "src/b.rs".into(),
                    start_line: 0,
                    end_line: 0,
                    properties_json: None,
                })
                .unwrap();

            let mut buf = GraphBuffer::new(project);
            super::super::pass_githistory(&mut buf, &store, project, dir.path(), 100);

            // Verify change_frequency on a.rs (modified in all 3 commits)
            let nodes = store.get_all_nodes(project).unwrap();
            let node_a = nodes
                .iter()
                .find(|n| n.label == "File" && n.file_path == "src/a.rs")
                .unwrap();
            let props_a: serde_json::Value =
                serde_json::from_str(node_a.properties_json.as_deref().unwrap_or("{}")).unwrap();
            let freq_a = props_a["change_frequency"].as_u64().unwrap_or(0);
            assert!(
                freq_a >= 2,
                "a.rs should have change_frequency >= 2 (modified in multiple commits), got {}",
                freq_a
            );

            // Verify churn_score is set on a.rs
            let churn_a = props_a["churn_score"].as_u64().unwrap_or(0);
            assert!(
                churn_a > 0,
                "a.rs should have a positive churn_score, got {}",
                churn_a
            );

            // Verify b.rs has change_frequency of 1 (only initial commit)
            let node_b = nodes
                .iter()
                .find(|n| n.label == "File" && n.file_path == "src/b.rs")
                .unwrap();
            let props_b: serde_json::Value =
                serde_json::from_str(node_b.properties_json.as_deref().unwrap_or("{}")).unwrap();
            let freq_b = props_b["change_frequency"].as_u64().unwrap_or(0);
            assert_eq!(
                freq_b, 1,
                "b.rs should have change_frequency of 1 (only initial commit)"
            );
        }

        #[test]
        fn test_pass_githistory_co_change_edges() {
            let dir = tempfile::tempdir().unwrap();
            let project = "p";

            // Create repo with initial commit containing two files
            let repo = init_repo_with_commit(
                dir.path(),
                &[("src/a.rs", "fn a() {}\n"), ("src/b.rs", "fn b() {}\n")],
                "initial",
            );

            // Commits 2-4: modify both a.rs and b.rs together (need >= 3 co-changes)
            for i in 2..=4 {
                add_commit(
                    &repo,
                    dir.path(),
                    &[
                        ("src/a.rs", &format!("fn a() {{ /* v{} */ }}\n", i)),
                        ("src/b.rs", &format!("fn b() {{ /* v{} */ }}\n", i)),
                    ],
                    &format!("update both files v{}", i),
                );
            }

            let store = test_store(project);
            store
                .insert_node(&Node {
                    id: 0,
                    project: project.into(),
                    label: "File".into(),
                    name: "a.rs".into(),
                    qualified_name: format!("{}.src.a", project),
                    file_path: "src/a.rs".into(),
                    start_line: 0,
                    end_line: 0,
                    properties_json: None,
                })
                .unwrap();
            store
                .insert_node(&Node {
                    id: 0,
                    project: project.into(),
                    label: "File".into(),
                    name: "b.rs".into(),
                    qualified_name: format!("{}.src.b", project),
                    file_path: "src/b.rs".into(),
                    start_line: 0,
                    end_line: 0,
                    properties_json: None,
                })
                .unwrap();

            let mut buf = GraphBuffer::new(project);
            super::super::pass_githistory(&mut buf, &store, project, dir.path(), 100);

            // a.rs and b.rs are modified together in >= 3 commits (initial + 3 updates = 4)
            // so a CO_CHANGED edge should be created
            assert!(
                buf.edge_count() >= 1,
                "pass_githistory should create CO_CHANGED edges for files frequently modified together, got {}",
                buf.edge_count()
            );
        }

        #[test]
        fn test_pass_githistory_non_git_directory_skips_gracefully() {
            let dir = tempfile::tempdir().unwrap();
            let project = "p";

            // No git init
            std::fs::write(dir.path().join("file.txt"), "hello").unwrap();

            let store = test_store(project);
            let mut buf = GraphBuffer::new(project);
            super::super::pass_githistory(&mut buf, &store, project, dir.path(), 100);

            // Should produce nothing and not panic
            assert_eq!(
                buf.node_count(),
                0,
                "pass_githistory on non-git dir should produce no nodes"
            );
            assert_eq!(
                buf.edge_count(),
                0,
                "pass_githistory on non-git dir should produce no edges"
            );
        }
    }

    // ── pass_type_refs tests ─────────────────────────────

    #[test]
    fn test_type_ref_edge_for_function_parameter() {
        let project = "p";
        let store = test_store(project);

        // Insert a Class node that the function parameter references
        store
            .insert_node(&Node {
                id: 0,
                project: project.into(),
                label: "Class".into(),
                name: "UserService".into(),
                qualified_name: format!("{}.src.services.UserService", project),
                file_path: "src/services.ts".into(),
                start_line: 1,
                end_line: 20,
                properties_json: None,
            })
            .unwrap();

        // Insert a Function node with a parameter typed as UserService
        let params_json = serde_json::json!([
            {"name": "svc", "type": "UserService"}
        ]);
        let props = serde_json::json!({
            "parameters": params_json,
        });
        store
            .insert_node(&Node {
                id: 0,
                project: project.into(),
                label: "Function".into(),
                name: "handleRequest".into(),
                qualified_name: format!("{}.src.handler.handleRequest", project),
                file_path: "src/handler.ts".into(),
                start_line: 1,
                end_line: 10,
                properties_json: Some(props.to_string()),
            })
            .unwrap();

        // Register UserService in the registry
        let mut reg = Registry::new();
        reg.register(
            "UserService",
            &format!("{}.src.services.UserService", project),
            "src/services.ts",
            "Class",
            1,
            20,
        );

        let mut buf = GraphBuffer::new(project);
        pass_type_refs(&mut buf, &reg, &store, project);

        assert!(
            buf.edge_count() >= 1,
            "pass_type_refs should create a TYPE_REF edge for parameter type, got {}",
            buf.edge_count()
        );
    }

    #[test]
    fn test_type_ref_edge_for_return_type() {
        let project = "p";
        let store = test_store(project);

        // Insert a Class node for the return type
        store
            .insert_node(&Node {
                id: 0,
                project: project.into(),
                label: "Class".into(),
                name: "Order".into(),
                qualified_name: format!("{}.src.models.Order", project),
                file_path: "src/models.ts".into(),
                start_line: 1,
                end_line: 15,
                properties_json: None,
            })
            .unwrap();

        // Insert a Function node with a return type of Order
        let props = serde_json::json!({
            "return_type": "Order",
        });
        store
            .insert_node(&Node {
                id: 0,
                project: project.into(),
                label: "Function".into(),
                name: "getOrder".into(),
                qualified_name: format!("{}.src.api.getOrder", project),
                file_path: "src/api.ts".into(),
                start_line: 1,
                end_line: 10,
                properties_json: Some(props.to_string()),
            })
            .unwrap();

        let mut reg = Registry::new();
        reg.register(
            "Order",
            &format!("{}.src.models.Order", project),
            "src/models.ts",
            "Class",
            1,
            15,
        );

        let mut buf = GraphBuffer::new(project);
        pass_type_refs(&mut buf, &reg, &store, project);

        assert!(
            buf.edge_count() >= 1,
            "pass_type_refs should create a TYPE_REF edge for return type, got {}",
            buf.edge_count()
        );
    }

    #[test]
    fn test_type_ref_skips_stdlib_types() {
        let project = "p";
        let store = test_store(project);

        // Insert a Function node with stdlib parameter types
        let params_json = serde_json::json!([
            {"name": "name", "type": "String"},
            {"name": "count", "type": "i32"}
        ]);
        let props = serde_json::json!({
            "parameters": params_json,
            "return_type": "Vec",
        });
        store
            .insert_node(&Node {
                id: 0,
                project: project.into(),
                label: "Function".into(),
                name: "process".into(),
                qualified_name: format!("{}.src.lib.process", project),
                file_path: "src/lib.rs".into(),
                start_line: 1,
                end_line: 10,
                properties_json: Some(props.to_string()),
            })
            .unwrap();

        let reg = Registry::new();
        let mut buf = GraphBuffer::new(project);
        pass_type_refs(&mut buf, &reg, &store, project);

        assert_eq!(
            buf.edge_count(),
            0,
            "pass_type_refs should not create TYPE_REF edges for stdlib types"
        );
    }

    #[test]
    fn test_type_ref_skips_nonexistent_types() {
        let project = "p";
        let store = test_store(project);

        // Insert a Function node referencing a type that doesn't exist in the registry
        let params_json = serde_json::json!([
            {"name": "svc", "type": "NonExistentService"}
        ]);
        let props = serde_json::json!({
            "parameters": params_json,
            "return_type": "AlsoDoesNotExist",
        });
        store
            .insert_node(&Node {
                id: 0,
                project: project.into(),
                label: "Function".into(),
                name: "doStuff".into(),
                qualified_name: format!("{}.src.app.doStuff", project),
                file_path: "src/app.ts".into(),
                start_line: 1,
                end_line: 10,
                properties_json: Some(props.to_string()),
            })
            .unwrap();

        // Empty registry — no types registered
        let reg = Registry::new();
        let mut buf = GraphBuffer::new(project);
        pass_type_refs(&mut buf, &reg, &store, project);

        assert_eq!(
            buf.edge_count(),
            0,
            "pass_type_refs should not create TYPE_REF edges for types not in the registry"
        );
    }

    #[test]
    fn test_strip_generic_params() {
        assert_eq!(strip_generic_params("Vec<String>"), "Vec");
        assert_eq!(strip_generic_params("HashMap<K, V>"), "HashMap");
        assert_eq!(strip_generic_params("MyType"), "MyType");
        assert_eq!(strip_generic_params("Option<Vec<u8>>"), "Option");
    }

    #[test]
    fn test_resolve_type_to_node_prefers_class() {
        let mut reg = Registry::new();
        // Register both a Function and a Class with the same name
        reg.register("Foo", "p.src.foo_fn.Foo", "src/foo_fn.ts", "Function", 1, 5);
        reg.register("Foo", "p.src.foo_cls.Foo", "src/foo_cls.ts", "Class", 1, 20);

        let result = resolve_type_to_node(&reg, "Foo");
        assert_eq!(result, Some("p.src.foo_cls.Foo".to_string()));
    }

    // ── pass_service_patterns tests ─────────────────────────

    fn setup_service_pattern_store() -> (Store, i64, i64) {
        let store = Store::open_in_memory().unwrap();
        store
            .upsert_project(&codryn_store::Project {
                name: "p".into(),
                indexed_at: "now".into(),
                root_path: "/tmp".into(),
            })
            .unwrap();
        let caller_id = store
            .insert_node(&codryn_store::Node {
                id: 0,
                project: "p".into(),
                label: "Function".into(),
                name: "doWork".into(),
                qualified_name: "p.src.doWork".into(),
                file_path: "src/main.ts".into(),
                start_line: 1,
                end_line: 10,
                properties_json: None,
            })
            .unwrap();
        (store, caller_id, 0) // target_id will be set per test
    }

    #[test]
    fn test_service_patterns_http_client_detection() {
        let (store, caller_id, _) = setup_service_pattern_store();
        let target_id = store
            .insert_node(&codryn_store::Node {
                id: 0,
                project: "p".into(),
                label: "Function".into(),
                name: "get".into(),
                qualified_name: "p.node_modules.axios.get".into(),
                file_path: "node_modules/axios/index.js".into(),
                start_line: 1,
                end_line: 5,
                properties_json: None,
            })
            .unwrap();
        store
            .insert_edge(&codryn_store::Edge {
                id: 0,
                project: "p".into(),
                source_id: caller_id,
                target_id,
                edge_type: "CALLS".into(),
                properties_json: None,
            })
            .unwrap();

        let mut buf = GraphBuffer::new("p");
        pass_service_patterns(&mut buf, &store, "p");

        assert!(
            buf.edge_count() >= 1,
            "pass_service_patterns should create an HTTP_CALLS edge for axios, got {}",
            buf.edge_count()
        );
    }

    #[test]
    fn test_service_patterns_http_method_extraction() {
        assert_eq!(detect_http_method("get"), "GET");
        assert_eq!(detect_http_method("post"), "POST");
        assert_eq!(detect_http_method("sendPost"), "POST");
        assert_eq!(detect_http_method("httpGet"), "GET");
        assert_eq!(detect_http_method("doPut"), "PUT");
        assert_eq!(detect_http_method("executeDelete"), "DELETE");
        assert_eq!(detect_http_method("sendPatch"), "PATCH");
        assert_eq!(detect_http_method("doSomething"), "UNKNOWN");
    }

    #[test]
    fn test_service_patterns_async_broker_detection() {
        let (store, caller_id, _) = setup_service_pattern_store();
        let target_id = store
            .insert_node(&codryn_store::Node {
                id: 0,
                project: "p".into(),
                label: "Function".into(),
                name: "produce".into(),
                qualified_name: "p.deps.rdkafka.produce".into(),
                file_path: "deps/rdkafka/lib.rs".into(),
                start_line: 1,
                end_line: 5,
                properties_json: None,
            })
            .unwrap();
        store
            .insert_edge(&codryn_store::Edge {
                id: 0,
                project: "p".into(),
                source_id: caller_id,
                target_id,
                edge_type: "CALLS".into(),
                properties_json: None,
            })
            .unwrap();

        let mut buf = GraphBuffer::new("p");
        pass_service_patterns(&mut buf, &store, "p");

        assert!(
            buf.edge_count() >= 1,
            "pass_service_patterns should create an ASYNC_CALLS edge for kafka, got {}",
            buf.edge_count()
        );
    }

    #[test]
    fn test_service_patterns_config_lib_detection() {
        let (store, caller_id, _) = setup_service_pattern_store();
        let target_id = store
            .insert_node(&codryn_store::Node {
                id: 0,
                project: "p".into(),
                label: "Function".into(),
                name: "load".into(),
                qualified_name: "p.site_packages.dotenv.load".into(),
                file_path: "site_packages/dotenv/__init__.py".into(),
                start_line: 1,
                end_line: 5,
                properties_json: None,
            })
            .unwrap();
        store
            .insert_edge(&codryn_store::Edge {
                id: 0,
                project: "p".into(),
                source_id: caller_id,
                target_id,
                edge_type: "CALLS".into(),
                properties_json: None,
            })
            .unwrap();

        let mut buf = GraphBuffer::new("p");
        pass_service_patterns(&mut buf, &store, "p");

        assert!(
            buf.edge_count() >= 1,
            "pass_service_patterns should create a CONFIGURES edge for dotenv, got {}",
            buf.edge_count()
        );
    }

    #[test]
    fn test_service_patterns_preserves_original_calls_edge() {
        let (store, caller_id, _) = setup_service_pattern_store();
        let target_id = store
            .insert_node(&codryn_store::Node {
                id: 0,
                project: "p".into(),
                label: "Function".into(),
                name: "post".into(),
                qualified_name: "p.node_modules.axios.post".into(),
                file_path: "node_modules/axios/index.js".into(),
                start_line: 1,
                end_line: 5,
                properties_json: None,
            })
            .unwrap();
        store
            .insert_edge(&codryn_store::Edge {
                id: 0,
                project: "p".into(),
                source_id: caller_id,
                target_id,
                edge_type: "CALLS".into(),
                properties_json: None,
            })
            .unwrap();

        let mut buf = GraphBuffer::new("p");
        pass_service_patterns(&mut buf, &store, "p");

        // The original CALLS edge should still exist in the store (we only add, never remove)
        let calls = store.get_edges_by_type("p", "CALLS").unwrap();
        assert_eq!(
            calls.len(),
            1,
            "original CALLS edge should be preserved after service pattern classification"
        );

        // And a new HTTP_CALLS edge should be added to the buffer
        assert!(
            buf.edge_count() >= 1,
            "an HTTP_CALLS edge should be added alongside the original CALLS edge"
        );
    }

    // ── pass_semantic_edges_v2 tests ────────────────────────

    /// Helper: set up a store with nodes for semantic edge v2 tests.
    fn setup_semantic_v2_store() -> (Store, Registry) {
        let store = Store::open_in_memory().unwrap();
        store
            .upsert_project(&codryn_store::Project {
                name: "p".into(),
                indexed_at: "now".into(),
                root_path: "/tmp".into(),
            })
            .unwrap();
        let reg = Registry::new();
        (store, reg)
    }

    #[test]
    fn test_semantic_v2_inherits_edge_for_class_inheritance() {
        let (store, mut reg) = setup_semantic_v2_store();

        // Insert parent class node
        let parent_id = store
            .insert_node(&codryn_store::Node {
                id: 0,
                project: "p".into(),
                label: "Class".into(),
                name: "Animal".into(),
                qualified_name: "p.src.models.Animal".into(),
                file_path: "src/models.py".into(),
                start_line: 1,
                end_line: 10,
                properties_json: None,
            })
            .unwrap();

        // Insert child class node with base_classes
        let child_id = store
            .insert_node(&codryn_store::Node {
                id: 0,
                project: "p".into(),
                label: "Class".into(),
                name: "Dog".into(),
                qualified_name: "p.src.models.Dog".into(),
                file_path: "src/models.py".into(),
                start_line: 12,
                end_line: 20,
                properties_json: Some(serde_json::json!({"base_classes": ["Animal"]}).to_string()),
            })
            .unwrap();

        // Register parent in registry
        reg.register(
            "Animal",
            "p.src.models.Animal",
            "src/models.py",
            "Class",
            1,
            10,
        );

        let mut buf = GraphBuffer::new("p");
        buf.seed_ids_from_store(&store).unwrap();
        pass_semantic_edges_v2(&mut buf, &reg, &store, "p");

        assert!(
            buf.edge_count() >= 1,
            "pass_semantic_edges_v2 should create INHERITS edge for class inheritance"
        );

        // Flush and verify the edge type
        buf.flush(&store).unwrap();
        let edges = store.get_edges_by_type("p", "INHERITS").unwrap();
        assert_eq!(edges.len(), 1, "should have exactly one INHERITS edge");
        assert_eq!(edges[0].source_id, child_id);
        assert_eq!(edges[0].target_id, parent_id);
    }

    #[test]
    fn test_semantic_v2_implements_edge_for_interface() {
        let (store, mut reg) = setup_semantic_v2_store();

        // Insert interface node
        let iface_id = store
            .insert_node(&codryn_store::Node {
                id: 0,
                project: "p".into(),
                label: "Interface".into(),
                name: "Serializable".into(),
                qualified_name: "p.src.interfaces.Serializable".into(),
                file_path: "src/interfaces.ts".into(),
                start_line: 1,
                end_line: 5,
                properties_json: None,
            })
            .unwrap();

        // Insert class that implements the interface
        let class_id = store
            .insert_node(&codryn_store::Node {
                id: 0,
                project: "p".into(),
                label: "Class".into(),
                name: "User".into(),
                qualified_name: "p.src.models.User".into(),
                file_path: "src/models.ts".into(),
                start_line: 1,
                end_line: 20,
                properties_json: Some(
                    serde_json::json!({"base_classes": ["Serializable"]}).to_string(),
                ),
            })
            .unwrap();

        // Register interface in registry
        reg.register(
            "Serializable",
            "p.src.interfaces.Serializable",
            "src/interfaces.ts",
            "Interface",
            1,
            5,
        );

        let mut buf = GraphBuffer::new("p");
        buf.seed_ids_from_store(&store).unwrap();
        pass_semantic_edges_v2(&mut buf, &reg, &store, "p");

        assert!(
            buf.edge_count() >= 1,
            "pass_semantic_edges_v2 should create IMPLEMENTS edge for interface implementation"
        );

        buf.flush(&store).unwrap();
        let edges = store.get_edges_by_type("p", "IMPLEMENTS").unwrap();
        assert_eq!(edges.len(), 1, "should have exactly one IMPLEMENTS edge");
        assert_eq!(edges[0].source_id, class_id);
        assert_eq!(edges[0].target_id, iface_id);
    }

    #[test]
    fn test_semantic_v2_decorates_edge() {
        let (store, mut reg) = setup_semantic_v2_store();

        // Insert decorator function node
        let dec_id = store
            .insert_node(&codryn_store::Node {
                id: 0,
                project: "p".into(),
                label: "Function".into(),
                name: "log_calls".into(),
                qualified_name: "p.src.decorators.log_calls".into(),
                file_path: "src/decorators.py".into(),
                start_line: 1,
                end_line: 10,
                properties_json: None,
            })
            .unwrap();

        // Insert decorated function node
        let func_id = store
            .insert_node(&codryn_store::Node {
                id: 0,
                project: "p".into(),
                label: "Function".into(),
                name: "process".into(),
                qualified_name: "p.src.service.process".into(),
                file_path: "src/service.py".into(),
                start_line: 1,
                end_line: 15,
                properties_json: Some(
                    serde_json::json!({"decorators": ["@log_calls"]}).to_string(),
                ),
            })
            .unwrap();

        // Register decorator in registry
        reg.register(
            "log_calls",
            "p.src.decorators.log_calls",
            "src/decorators.py",
            "Function",
            1,
            10,
        );

        let mut buf = GraphBuffer::new("p");
        buf.seed_ids_from_store(&store).unwrap();
        pass_semantic_edges_v2(&mut buf, &reg, &store, "p");

        assert!(
            buf.edge_count() >= 1,
            "pass_semantic_edges_v2 should create DECORATES edge"
        );

        buf.flush(&store).unwrap();
        let edges = store.get_edges_by_type("p", "DECORATES").unwrap();
        assert_eq!(edges.len(), 1, "should have exactly one DECORATES edge");
        // DECORATES: decorator → decorated symbol
        assert_eq!(edges[0].source_id, dec_id);
        assert_eq!(edges[0].target_id, func_id);
    }

    #[test]
    fn test_semantic_v2_unresolvable_targets_no_edges() {
        let (store, reg) = setup_semantic_v2_store();

        // Insert a class with base_classes that don't exist in the registry
        store
            .insert_node(&codryn_store::Node {
                id: 0,
                project: "p".into(),
                label: "Class".into(),
                name: "Orphan".into(),
                qualified_name: "p.src.orphan.Orphan".into(),
                file_path: "src/orphan.py".into(),
                start_line: 1,
                end_line: 10,
                properties_json: Some(
                    serde_json::json!({
                        "base_classes": ["NonExistentBase"],
                        "decorators": ["@non_existent_decorator"]
                    })
                    .to_string(),
                ),
            })
            .unwrap();

        let mut buf = GraphBuffer::new("p");
        buf.seed_ids_from_store(&store).unwrap();
        pass_semantic_edges_v2(&mut buf, &reg, &store, "p");

        assert_eq!(
            buf.edge_count(),
            0,
            "pass_semantic_edges_v2 should not create edges for unresolvable targets"
        );
    }

    #[test]
    fn test_semantic_v2_rust_impl_trait_creates_implements() {
        let (store, mut reg) = setup_semantic_v2_store();

        // Insert trait (Interface) node
        let trait_id = store
            .insert_node(&codryn_store::Node {
                id: 0,
                project: "p".into(),
                label: "Interface".into(),
                name: "Display".into(),
                qualified_name: "p.src.traits.Display".into(),
                file_path: "src/traits.rs".into(),
                start_line: 1,
                end_line: 5,
                properties_json: None,
            })
            .unwrap();

        // Insert Rust impl node (impl Display for MyStruct)
        let impl_id = store
            .insert_node(&codryn_store::Node {
                id: 0,
                project: "p".into(),
                label: "Impl".into(),
                name: "MyStruct".into(),
                qualified_name: "p.src.my_struct.impl_Display_for_MyStruct".into(),
                file_path: "src/my_struct.rs".into(),
                start_line: 10,
                end_line: 20,
                properties_json: Some(serde_json::json!({"base_classes": ["Display"]}).to_string()),
            })
            .unwrap();

        // Register trait in registry
        reg.register(
            "Display",
            "p.src.traits.Display",
            "src/traits.rs",
            "Interface",
            1,
            5,
        );

        let mut buf = GraphBuffer::new("p");
        buf.seed_ids_from_store(&store).unwrap();
        pass_semantic_edges_v2(&mut buf, &reg, &store, "p");

        assert!(
            buf.edge_count() >= 1,
            "pass_semantic_edges_v2 should create IMPLEMENTS edge for Rust impl Trait"
        );

        buf.flush(&store).unwrap();
        let edges = store.get_edges_by_type("p", "IMPLEMENTS").unwrap();
        assert_eq!(edges.len(), 1, "should have exactly one IMPLEMENTS edge");
        assert_eq!(edges[0].source_id, impl_id);
        assert_eq!(edges[0].target_id, trait_id);
    }

    #[test]
    fn test_normalize_decorator_name() {
        assert_eq!(normalize_decorator_name("@Component"), "Component");
        assert_eq!(normalize_decorator_name("@Component({})"), "Component");
        assert_eq!(normalize_decorator_name("@override"), "override");
        assert_eq!(normalize_decorator_name("Test"), "Test");
        assert_eq!(
            normalize_decorator_name("@log_calls(level=DEBUG)"),
            "log_calls"
        );
    }

    #[test]
    fn test_resolve_type_target_prefers_class_interface() {
        let mut reg = Registry::new();
        reg.register("Foo", "p.Foo", "foo.ts", "Function", 1, 5);
        reg.register("Foo", "p.models.Foo", "models.ts", "Class", 1, 10);

        let result = resolve_type_target(&reg, "Foo");
        assert!(result.is_some());
        let (qn, label) = result.unwrap();
        assert_eq!(qn, "p.models.Foo");
        assert_eq!(label, "Class");
    }

    #[test]
    fn test_resolve_type_target_strips_generics() {
        let mut reg = Registry::new();
        reg.register(
            "List",
            "p.collections.List",
            "collections.ts",
            "Class",
            1,
            10,
        );

        let result = resolve_type_target(&reg, "List<String>");
        assert!(result.is_some());
        assert_eq!(result.unwrap().0, "p.collections.List");
    }

    #[test]
    fn test_resolve_type_target_returns_none_for_unknown() {
        let reg = Registry::new();
        assert!(resolve_type_target(&reg, "NonExistent").is_none());
    }

    // ── Cross-Repo Intelligence Tests ────────────────────

    /// Helper: set up two linked projects in an in-memory store.
    fn setup_cross_repo_store() -> codryn_store::Store {
        let store = codryn_store::Store::open_in_memory().unwrap();
        store
            .upsert_project(&codryn_store::Project {
                name: "frontend".into(),
                indexed_at: "now".into(),
                root_path: "/fe".into(),
            })
            .unwrap();
        store
            .upsert_project(&codryn_store::Project {
                name: "backend".into(),
                indexed_at: "now".into(),
                root_path: "/be".into(),
            })
            .unwrap();
        store.link_projects("frontend", "backend").unwrap();
        store
    }

    #[test]
    fn test_cross_repo_http_route_matching() {
        let store = setup_cross_repo_store();

        // Add Route node to frontend
        let fe_route_id = store
            .insert_node(&codryn_store::Node {
                id: 0,
                project: "frontend".into(),
                label: "Route".into(),
                name: "GET /api/users/:id".into(),
                qualified_name: "frontend.route.GET./api/users/:id".into(),
                file_path: "src/api.ts".into(),
                start_line: 1,
                end_line: 10,
                properties_json: Some(
                    serde_json::json!({"http_method": "GET", "path": "/api/users/:id"}).to_string(),
                ),
            })
            .unwrap();

        // Add matching Route node to backend (different param syntax)
        let be_route_id = store
            .insert_node(&codryn_store::Node {
                id: 0,
                project: "backend".into(),
                label: "Route".into(),
                name: "GET /api/users/{id}".into(),
                qualified_name: "backend.route.GET./api/users/{id}".into(),
                file_path: "src/controller.java".into(),
                start_line: 1,
                end_line: 10,
                properties_json: Some(
                    serde_json::json!({"http_method": "GET", "path": "/api/users/{id}"})
                        .to_string(),
                ),
            })
            .unwrap();

        let mut buf = GraphBuffer::new("frontend");
        pass_cross_repo(&mut buf, &store, "frontend");

        // Should create bidirectional CROSS_HTTP edges
        assert_eq!(
            buf.edge_count(),
            2,
            "should create 2 bidirectional CROSS_HTTP edges"
        );

        buf.flush(&store).unwrap();
        let edges = store.get_edges_by_type("frontend", "CROSS_HTTP").unwrap();
        assert_eq!(edges.len(), 2, "should have 2 CROSS_HTTP edges");

        // Verify bidirectionality
        let has_fe_to_be = edges
            .iter()
            .any(|e| e.source_id == fe_route_id && e.target_id == be_route_id);
        let has_be_to_fe = edges
            .iter()
            .any(|e| e.source_id == be_route_id && e.target_id == fe_route_id);
        assert!(has_fe_to_be, "should have edge from frontend to backend");
        assert!(has_be_to_fe, "should have edge from backend to frontend");
    }

    #[test]
    fn test_cross_repo_channel_matching() {
        let store = setup_cross_repo_store();

        // Add Channel node to frontend
        let fe_ch_id = store
            .insert_node(&codryn_store::Node {
                id: 0,
                project: "frontend".into(),
                label: "Channel".into(),
                name: "user:updated".into(),
                qualified_name: "frontend.channel.user:updated".into(),
                file_path: "src/events.ts".into(),
                start_line: 1,
                end_line: 1,
                properties_json: Some(
                    serde_json::json!({"event_name": "user:updated"}).to_string(),
                ),
            })
            .unwrap();

        // Add matching Channel node to backend
        let be_ch_id = store
            .insert_node(&codryn_store::Node {
                id: 0,
                project: "backend".into(),
                label: "Channel".into(),
                name: "user:updated".into(),
                qualified_name: "backend.channel.user:updated".into(),
                file_path: "src/events.java".into(),
                start_line: 1,
                end_line: 1,
                properties_json: Some(
                    serde_json::json!({"event_name": "user:updated"}).to_string(),
                ),
            })
            .unwrap();

        let mut buf = GraphBuffer::new("frontend");
        pass_cross_repo(&mut buf, &store, "frontend");

        assert_eq!(
            buf.edge_count(),
            2,
            "should create 2 bidirectional CROSS_CHANNEL edges"
        );

        buf.flush(&store).unwrap();
        let edges = store
            .get_edges_by_type("frontend", "CROSS_CHANNEL")
            .unwrap();
        assert_eq!(edges.len(), 2);

        let has_fe_to_be = edges
            .iter()
            .any(|e| e.source_id == fe_ch_id && e.target_id == be_ch_id);
        let has_be_to_fe = edges
            .iter()
            .any(|e| e.source_id == be_ch_id && e.target_id == fe_ch_id);
        assert!(
            has_fe_to_be,
            "should have channel edge from frontend to backend"
        );
        assert!(
            has_be_to_fe,
            "should have channel edge from backend to frontend"
        );
    }

    #[test]
    fn test_cross_repo_async_topic_matching() {
        let store = setup_cross_repo_store();

        // Add Channel node with kafka_topic to frontend (use a different name than event_name
        // to avoid double-matching via match_cross_channels)
        let fe_id = store
            .insert_node(&codryn_store::Node {
                id: 0,
                project: "frontend".into(),
                label: "Channel".into(),
                name: "kafka-order-events".into(),
                qualified_name: "frontend.channel.kafka-order-events".into(),
                file_path: "src/kafka.ts".into(),
                start_line: 1,
                end_line: 1,
                properties_json: Some(
                    serde_json::json!({"kafka_topic": "order-events"}).to_string(),
                ),
            })
            .unwrap();

        // Add matching Channel node with kafka_topic to backend (different name)
        let be_id = store
            .insert_node(&codryn_store::Node {
                id: 0,
                project: "backend".into(),
                label: "Channel".into(),
                name: "kafka-order-events-consumer".into(),
                qualified_name: "backend.channel.kafka-order-events-consumer".into(),
                file_path: "src/kafka.java".into(),
                start_line: 1,
                end_line: 1,
                properties_json: Some(
                    serde_json::json!({"kafka_topic": "order-events"}).to_string(),
                ),
            })
            .unwrap();

        let mut buf = GraphBuffer::new("frontend");
        pass_cross_repo(&mut buf, &store, "frontend");

        assert_eq!(
            buf.edge_count(),
            2,
            "should create 2 bidirectional CROSS_ASYNC edges"
        );

        buf.flush(&store).unwrap();
        let edges = store.get_edges_by_type("frontend", "CROSS_ASYNC").unwrap();
        assert_eq!(edges.len(), 2);

        let has_fe_to_be = edges
            .iter()
            .any(|e| e.source_id == fe_id && e.target_id == be_id);
        let has_be_to_fe = edges
            .iter()
            .any(|e| e.source_id == be_id && e.target_id == fe_id);
        assert!(
            has_fe_to_be,
            "should have async edge from frontend to backend"
        );
        assert!(
            has_be_to_fe,
            "should have async edge from backend to frontend"
        );
    }

    #[test]
    fn test_cross_repo_bidirectional_edges() {
        let store = setup_cross_repo_store();

        // Add Route nodes to both projects
        let fe_id = store
            .insert_node(&codryn_store::Node {
                id: 0,
                project: "frontend".into(),
                label: "Route".into(),
                name: "POST /api/orders".into(),
                qualified_name: "frontend.route.POST./api/orders".into(),
                file_path: "src/api.ts".into(),
                start_line: 1,
                end_line: 1,
                properties_json: Some(
                    serde_json::json!({"http_method": "POST", "path": "/api/orders"}).to_string(),
                ),
            })
            .unwrap();

        let be_id = store
            .insert_node(&codryn_store::Node {
                id: 0,
                project: "backend".into(),
                label: "Route".into(),
                name: "POST /api/orders".into(),
                qualified_name: "backend.route.POST./api/orders".into(),
                file_path: "src/controller.java".into(),
                start_line: 1,
                end_line: 1,
                properties_json: Some(
                    serde_json::json!({"http_method": "POST", "path": "/api/orders"}).to_string(),
                ),
            })
            .unwrap();

        // Run from frontend perspective — creates edges under "frontend" project
        let mut buf_fe = GraphBuffer::new("frontend");
        pass_cross_repo(&mut buf_fe, &store, "frontend");
        buf_fe.flush(&store).unwrap();

        // The pass creates bidirectional edges (A->B and B->A) in a single run
        let fe_edges = store.get_edges_by_type("frontend", "CROSS_HTTP").unwrap();

        assert_eq!(
            fe_edges.len(),
            2,
            "should have 2 CROSS_HTTP edges (bidirectional) from frontend run"
        );

        // Verify both directions exist
        let has_fe_to_be = fe_edges
            .iter()
            .any(|e| e.source_id == fe_id && e.target_id == be_id);
        let has_be_to_fe = fe_edges
            .iter()
            .any(|e| e.source_id == be_id && e.target_id == fe_id);
        assert!(has_fe_to_be, "should have edge from frontend to backend");
        assert!(has_be_to_fe, "should have edge from backend to frontend");
    }

    #[test]
    fn test_cross_repo_stale_edge_cleanup() {
        let store = setup_cross_repo_store();

        // Add Route nodes
        store
            .insert_node(&codryn_store::Node {
                id: 0,
                project: "frontend".into(),
                label: "Route".into(),
                name: "GET /api/health".into(),
                qualified_name: "frontend.route.GET./api/health".into(),
                file_path: "src/api.ts".into(),
                start_line: 1,
                end_line: 1,
                properties_json: Some(
                    serde_json::json!({"http_method": "GET", "path": "/api/health"}).to_string(),
                ),
            })
            .unwrap();

        store
            .insert_node(&codryn_store::Node {
                id: 0,
                project: "backend".into(),
                label: "Route".into(),
                name: "GET /api/health".into(),
                qualified_name: "backend.route.GET./api/health".into(),
                file_path: "src/controller.java".into(),
                start_line: 1,
                end_line: 1,
                properties_json: Some(
                    serde_json::json!({"http_method": "GET", "path": "/api/health"}).to_string(),
                ),
            })
            .unwrap();

        // First run: creates CROSS_HTTP edges
        let mut buf = GraphBuffer::new("frontend");
        pass_cross_repo(&mut buf, &store, "frontend");
        buf.flush(&store).unwrap();

        let edges_before = store.get_edges_by_type("frontend", "CROSS_HTTP").unwrap();
        assert_eq!(edges_before.len(), 2, "should have 2 edges after first run");

        // Second run: should clean up stale edges and recreate
        let mut buf2 = GraphBuffer::new("frontend");
        pass_cross_repo(&mut buf2, &store, "frontend");
        buf2.flush(&store).unwrap();

        let edges_after = store.get_edges_by_type("frontend", "CROSS_HTTP").unwrap();
        assert_eq!(
            edges_after.len(),
            2,
            "should still have 2 edges after re-run (stale cleaned + new created)"
        );
    }

    #[test]
    fn test_cross_repo_skips_when_no_links() {
        let store = codryn_store::Store::open_in_memory().unwrap();
        store
            .upsert_project(&codryn_store::Project {
                name: "solo".into(),
                indexed_at: "now".into(),
                root_path: "/solo".into(),
            })
            .unwrap();

        // Add a Route node
        store
            .insert_node(&codryn_store::Node {
                id: 0,
                project: "solo".into(),
                label: "Route".into(),
                name: "GET /api/test".into(),
                qualified_name: "solo.route.GET./api/test".into(),
                file_path: "src/api.ts".into(),
                start_line: 1,
                end_line: 1,
                properties_json: Some(
                    serde_json::json!({"http_method": "GET", "path": "/api/test"}).to_string(),
                ),
            })
            .unwrap();

        let mut buf = GraphBuffer::new("solo");
        pass_cross_repo(&mut buf, &store, "solo");

        assert_eq!(
            buf.edge_count(),
            0,
            "should create no edges when project has no links"
        );
    }

    #[test]
    fn test_normalize_route_path() {
        assert_eq!(normalize_route_path("/users/:id"), "/users/{param}");
        assert_eq!(normalize_route_path("/users/{id}"), "/users/{param}");
        assert_eq!(
            normalize_route_path("/api/v1/orders/:orderId/items/:itemId"),
            "/api/v1/orders/{param}/items/{param}"
        );
        assert_eq!(normalize_route_path("/api/health"), "/api/health");
    }

    #[test]
    fn test_delete_edges_by_type_prefix() {
        let store = codryn_store::Store::open_in_memory().unwrap();
        store
            .upsert_project(&codryn_store::Project {
                name: "p".into(),
                indexed_at: "now".into(),
                root_path: "/p".into(),
            })
            .unwrap();

        let id1 = store
            .insert_node(&codryn_store::Node {
                id: 0,
                project: "p".into(),
                label: "Function".into(),
                name: "a".into(),
                qualified_name: "p.a".into(),
                file_path: "".into(),
                start_line: 0,
                end_line: 0,
                properties_json: None,
            })
            .unwrap();

        let id2 = store
            .insert_node(&codryn_store::Node {
                id: 0,
                project: "p".into(),
                label: "Function".into(),
                name: "b".into(),
                qualified_name: "p.b".into(),
                file_path: "".into(),
                start_line: 0,
                end_line: 0,
                properties_json: None,
            })
            .unwrap();

        // Insert CROSS_HTTP and CALLS edges
        store
            .insert_edge(&codryn_store::Edge {
                id: 0,
                project: "p".into(),
                source_id: id1,
                target_id: id2,
                edge_type: "CROSS_HTTP".into(),
                properties_json: None,
            })
            .unwrap();
        store
            .insert_edge(&codryn_store::Edge {
                id: 0,
                project: "p".into(),
                source_id: id1,
                target_id: id2,
                edge_type: "CROSS_CHANNEL".into(),
                properties_json: None,
            })
            .unwrap();
        store
            .insert_edge(&codryn_store::Edge {
                id: 0,
                project: "p".into(),
                source_id: id1,
                target_id: id2,
                edge_type: "CALLS".into(),
                properties_json: None,
            })
            .unwrap();

        // Delete CROSS_* edges
        let deleted = store.delete_edges_by_type_prefix("p", "CROSS_").unwrap();
        assert_eq!(deleted, 2, "should delete 2 CROSS_* edges");

        // CALLS edge should remain
        let calls = store.get_edges_by_type("p", "CALLS").unwrap();
        assert_eq!(calls.len(), 1, "CALLS edge should not be deleted");

        // CROSS_* edges should be gone
        let cross_http = store.get_edges_by_type("p", "CROSS_HTTP").unwrap();
        assert!(cross_http.is_empty(), "CROSS_HTTP edges should be deleted");
    }
}
