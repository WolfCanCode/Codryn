//! Cross-Repository Linking Pass
//!
//! Matches import paths in source files against linked project namespaces.
//! Creates cross-project IMPORTS edges with `cross_project: true` tag.
//! Handles re-indexing by adding new edges, removing stale edges, and updating
//! moved targets. Skips unmatched imports gracefully.
//!
//! Requirements: 21.1, 21.2, 21.3, 21.4

use codryn_discover::DiscoveredFile;
use codryn_foundation::fqn;
use codryn_graph_buffer::{EdgeSource, GraphBuffer};
use codryn_store::Store;
use std::collections::HashSet;

/// A namespace registered for a linked project.
/// The namespace is the project name used as a prefix for qualified names.
#[derive(Debug, Clone)]
pub struct ProjectNamespace {
    /// The project identifier (e.g., "backend-api").
    pub project_name: String,
    /// Namespace prefixes that identify imports belonging to this project.
    /// These are derived from the project's top-level module/package names.
    pub prefixes: Vec<String>,
}

/// A detected cross-project import reference.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct CrossRepoImport {
    /// Qualified name of the importing symbol (source).
    source_qn: String,
    /// Qualified name of the target in the linked project.
    target_qn: String,
    /// The linked project that owns the target.
    target_project: String,
}

/// Cross-Repository Linking Pass.
///
/// 1. Retrieves linked projects from the store
/// 2. Builds namespace prefixes from each linked project's exported symbols
/// 3. Scans source files for import statements
/// 4. Matches import paths against linked project namespaces
/// 5. Creates cross-project IMPORTS edges with `cross_project: true` and source project tag
/// 6. Removes stale cross-project edges no longer present in source
/// 7. Skips unmatched imports gracefully
pub fn pass_cross_repo(
    buf: &mut GraphBuffer,
    store: &Store,
    files: &[&DiscoveredFile],
    project: &str,
) {
    // Step 1: Get linked projects
    let links = store.get_linked_projects(project).unwrap_or_default();
    if links.is_empty() {
        tracing::debug!("pass_cross_repo: no linked projects, skipping");
        return;
    }

    // Step 2: Build namespace map from linked projects.
    // For each linked project, collect its top-level module/package names as namespace prefixes.
    let namespaces = build_namespace_map(store, &links);
    if namespaces.is_empty() {
        tracing::debug!("pass_cross_repo: no namespace prefixes found in linked projects");
        return;
    }

    // Step 3: Scan source files for imports and match against namespaces
    let new_imports = detect_cross_repo_imports(files, project, &namespaces);

    // Step 4: Get existing cross-project IMPORTS edges for this project (for re-indexing)
    let existing_edges = get_existing_cross_project_edges(store, project);

    // Step 5: Compute delta — add new, remove stale
    let new_set: HashSet<(String, String)> = new_imports
        .iter()
        .map(|i| (i.source_qn.clone(), i.target_qn.clone()))
        .collect();

    let existing_set: HashSet<(String, String)> = existing_edges
        .iter()
        .map(|e| (e.source_qn.clone(), e.target_qn.clone()))
        .collect();

    // Edges to add: in new but not in existing
    let to_add: Vec<&CrossRepoImport> = new_imports
        .iter()
        .filter(|i| !existing_set.contains(&(i.source_qn.clone(), i.target_qn.clone())))
        .collect();

    // Edges to remove: in existing but not in new (stale)
    let stale_edge_ids: Vec<i64> = existing_edges
        .iter()
        .filter(|e| !new_set.contains(&(e.source_qn.clone(), e.target_qn.clone())))
        .map(|e| e.edge_id)
        .collect();

    // Step 6: Remove stale cross-project edges
    if !stale_edge_ids.is_empty() {
        remove_stale_cross_project_edges(store, &stale_edge_ids);
    }

    // Step 7: Add new cross-project IMPORTS edges
    for import in &to_add {
        let props = serde_json::json!({
            "cross_project": true,
            "source_project": project,
            "target_project": import.target_project,
        })
        .to_string();

        buf.add_edge_with_confidence(
            &import.source_qn,
            &import.target_qn,
            "IMPORTS",
            EdgeSource::ImportResolver,
            Some(props),
        );
    }

    tracing::info!(
        linked_projects = links.len(),
        new_edges = to_add.len(),
        removed_edges = stale_edge_ids.len(),
        total_imports_detected = new_imports.len(),
        "pass_cross_repo: complete"
    );
}

/// Build a namespace map from linked projects.
/// For each linked project, derives namespace prefixes from its top-level nodes.
fn build_namespace_map(store: &Store, links: &[codryn_store::ProjectLink]) -> Vec<ProjectNamespace> {
    let mut namespaces = Vec::new();

    for link in links {
        let target_project = &link.target_project;

        // Get all nodes from the linked project to derive namespace prefixes
        let nodes = store.get_all_nodes(target_project).unwrap_or_default();

        // Collect unique top-level package/module prefixes from qualified names.
        // A prefix is the first segment after the project name in the qualified name.
        // E.g., for "backend-api.com.example.service.UserService", the prefix is "com.example".
        let mut prefixes: HashSet<String> = HashSet::new();

        for node in &nodes {
            // Skip File/Folder/Module nodes — we want actual code symbols
            if node.label == "File" || node.label == "Folder" {
                continue;
            }

            // Extract the namespace prefix from the qualified name.
            // The QN format is: project.path.segments.name
            // We want the path segments (without project prefix and final name).
            let qn = &node.qualified_name;
            if let Some(stripped) = qn.strip_prefix(&format!("{}.", target_project)) {
                // Get the first 1-2 segments as a namespace prefix
                let parts: Vec<&str> = stripped.split('.').collect();
                if parts.len() >= 2 {
                    // Use first segment as a prefix (package/module name)
                    prefixes.insert(parts[0].to_owned());
                    // Also use first two segments for more specific matching
                    prefixes.insert(format!("{}.{}", parts[0], parts[1]));
                } else if !parts.is_empty() {
                    prefixes.insert(parts[0].to_owned());
                }
            }
        }

        if !prefixes.is_empty() {
            namespaces.push(ProjectNamespace {
                project_name: target_project.clone(),
                prefixes: prefixes.into_iter().collect(),
            });
        }
    }

    namespaces
}

/// Scan source files for import statements and match them against linked project namespaces.
fn detect_cross_repo_imports(
    files: &[&DiscoveredFile],
    project: &str,
    namespaces: &[ProjectNamespace],
) -> Vec<CrossRepoImport> {
    let mut imports = Vec::new();

    for f in files {
        let source = match std::fs::read_to_string(&f.abs_path) {
            Ok(s) => s,
            Err(_) => continue,
        };

        let module_qn = fqn::fqn_module(project, &f.rel_path);

        for line in source.lines() {
            let trimmed = line.trim();

            // Extract import path from various language patterns
            let import_path = extract_import_path(trimmed, f.language);
            if let Some(path) = import_path {
                // Try to match against linked project namespaces
                if let Some((target_project, target_qn)) = match_namespace(&path, namespaces) {
                    imports.push(CrossRepoImport {
                        source_qn: module_qn.clone(),
                        target_qn: format!("{}.{}", target_project, target_qn),
                        target_project: target_project.to_owned(),
                    });
                }
            }
        }
    }

    // Deduplicate
    let mut seen = HashSet::new();
    imports.retain(|i| seen.insert(i.clone()));

    imports
}

/// Extract the import path from a source line based on language.
fn extract_import_path(line: &str, language: codryn_discover::Language) -> Option<String> {
    use codryn_discover::Language;

    match language {
        Language::Python => extract_python_import(line),
        Language::JavaScript | Language::TypeScript | Language::Tsx => extract_js_import(line),
        Language::Rust => extract_rust_import(line),
        Language::Java | Language::Kotlin => extract_java_import(line),
        Language::Go => extract_go_import(line),
        Language::Cpp | Language::C => extract_c_import(line),
        _ => None,
    }
}

/// Extract import path from Python import statements.
fn extract_python_import(line: &str) -> Option<String> {
    if line.starts_with("from ") {
        // from package.module import something
        let rest = line.strip_prefix("from ")?;
        let module = rest.split_whitespace().next()?;
        if module.starts_with('.') {
            return None; // relative import
        }
        Some(module.to_owned())
    } else if line.starts_with("import ") {
        // import package.module
        let rest = line.strip_prefix("import ")?;
        let module = rest.split_whitespace().next()?;
        let module = module.trim_end_matches(',');
        if module.starts_with('.') {
            return None;
        }
        Some(module.to_owned())
    } else {
        None
    }
}

/// Extract import path from JS/TS import statements.
fn extract_js_import(line: &str) -> Option<String> {
    // import ... from 'path' or import ... from "path"
    // require('path') or require("path")
    let path = if line.contains("from '") {
        line.split("from '").nth(1)?.split('\'').next()
    } else if line.contains("from \"") {
        line.split("from \"").nth(1)?.split('"').next()
    } else if line.contains("require('") {
        line.split("require('").nth(1)?.split('\'').next()
    } else if line.contains("require(\"") {
        line.split("require(\"").nth(1)?.split('"').next()
    } else {
        None
    }?;

    // Skip relative imports
    if path.starts_with('.') || path.starts_with('/') {
        return None;
    }

    Some(path.to_owned())
}

/// Extract import path from Rust use statements.
fn extract_rust_import(line: &str) -> Option<String> {
    if !line.starts_with("use ") {
        return None;
    }
    let rest = line.strip_prefix("use ")?;
    let path = rest.trim_end_matches(';').split('{').next()?.trim();
    // Skip crate-local imports
    if path.starts_with("crate::") || path.starts_with("self::") || path.starts_with("super::") {
        return None;
    }
    // Convert :: to . for matching
    Some(path.replace("::", "."))
}

/// Extract import path from Java/Kotlin import statements.
fn extract_java_import(line: &str) -> Option<String> {
    if !line.starts_with("import ") {
        return None;
    }
    let rest = line.strip_prefix("import ")?;
    // Skip static imports prefix
    let rest = rest.strip_prefix("static ").unwrap_or(rest);
    let path = rest.trim_end_matches(';').trim();
    if path.is_empty() {
        return None;
    }
    Some(path.to_owned())
}

/// Extract import path from Go import statements.
fn extract_go_import(line: &str) -> Option<String> {
    // import "path" or "path" inside import block
    let path = if line.contains('"') {
        let start = line.find('"')? + 1;
        let end = line[start..].find('"')? + start;
        &line[start..end]
    } else {
        return None;
    };

    // Skip standard library imports (no dots in path typically)
    if !path.contains('.') && !path.contains('/') {
        return None;
    }

    Some(path.to_owned())
}

/// Extract import path from C/C++ #include directives.
fn extract_c_import(line: &str) -> Option<String> {
    if !line.starts_with("#include") {
        return None;
    }
    // #include "path" or #include <path>
    let path = if line.contains('"') {
        let start = line.find('"')? + 1;
        let end = line[start..].find('"')? + start;
        &line[start..end]
    } else if line.contains('<') {
        let start = line.find('<')? + 1;
        let end = line[start..].find('>')? + start;
        &line[start..end]
    } else {
        return None;
    };

    // Skip relative includes
    if path.starts_with("./") || path.starts_with("../") {
        return None;
    }

    Some(path.replace('/', ".").replace(".hpp", "").replace(".h", ""))
}

/// Match an import path against linked project namespaces.
/// Returns (target_project_name, resolved_qn_suffix) if matched.
fn match_namespace<'a>(
    import_path: &str,
    namespaces: &'a [ProjectNamespace],
) -> Option<(&'a str, String)> {
    // Normalize the import path: replace / with . for uniform matching
    let normalized = import_path.replace('/', ".").replace("::", ".");

    // Try to match against each namespace's prefixes (longest prefix first)
    let mut best_match: Option<(&str, String, usize)> = None;

    for ns in namespaces {
        for prefix in &ns.prefixes {
            if normalized.starts_with(prefix) || normalized == *prefix {
                let prefix_len = prefix.len();
                // Prefer longer (more specific) prefix matches
                if best_match
                    .as_ref()
                    .is_none_or(|(_, _, len)| prefix_len > *len)
                {
                    // The target QN is the full normalized import path
                    best_match = Some((&ns.project_name, normalized.clone(), prefix_len));
                }
            }
        }
    }

    best_match.map(|(project, qn, _)| (project, qn))
}

/// An existing cross-project edge with its database ID for efficient deletion.
struct ExistingCrossEdge {
    /// Database edge ID.
    edge_id: i64,
    /// Source qualified name.
    source_qn: String,
    /// Target qualified name.
    target_qn: String,
}

/// Get existing cross-project IMPORTS edges for this project.
/// Returns edges with their IDs for efficient stale edge removal.
fn get_existing_cross_project_edges(store: &Store, project: &str) -> Vec<ExistingCrossEdge> {
    // Query edges with cross_project: true in properties
    let edges = match store.get_edges_by_type(project, "IMPORTS") {
        Ok(e) => e,
        Err(e) => {
            tracing::debug!(
                error = %e,
                "pass_cross_repo: failed to query existing cross-project edges"
            );
            return Vec::new();
        }
    };

    let mut result = Vec::new();
    for edge in &edges {
        // Check if this edge has cross_project: true in properties
        if let Some(ref props_str) = edge.properties_json {
            if let Ok(props) = serde_json::from_str::<serde_json::Value>(props_str) {
                if props.get("cross_project").and_then(|v| v.as_bool()) == Some(true) {
                    // Resolve source and target IDs back to qualified names
                    if let (Some(src_qn), Some(tgt_qn)) = (
                        resolve_node_qn(store, edge.source_id),
                        resolve_node_qn(store, edge.target_id),
                    ) {
                        result.push(ExistingCrossEdge {
                            edge_id: edge.id,
                            source_qn: src_qn,
                            target_qn: tgt_qn,
                        });
                    }
                }
            }
        }
    }

    result
}

/// Resolve a node ID to its qualified name.
fn resolve_node_qn(store: &Store, node_id: i64) -> Option<String> {
    store
        .get_node_by_id(node_id)
        .ok()
        .flatten()
        .map(|n| n.qualified_name)
}

/// Remove stale cross-project edges by their database IDs.
fn remove_stale_cross_project_edges(store: &Store, edge_ids: &[i64]) {
    use rusqlite::params;
    let conn = store.conn();
    for &edge_id in edge_ids {
        if let Err(e) = conn.execute("DELETE FROM edges WHERE id = ?1", params![edge_id]) {
            tracing::debug!(
                error = %e,
                edge_id = edge_id,
                "pass_cross_repo: failed to remove stale cross-project edge"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_python_import() {
        assert_eq!(
            extract_python_import("from django.db import models"),
            Some("django.db".to_owned())
        );
        assert_eq!(
            extract_python_import("import requests"),
            Some("requests".to_owned())
        );
        assert_eq!(extract_python_import("from .local import something"), None);
        assert_eq!(extract_python_import("x = 1"), None);
    }

    #[test]
    fn test_extract_js_import() {
        assert_eq!(
            extract_js_import("import { foo } from '@myorg/shared-lib'"),
            Some("@myorg/shared-lib".to_owned())
        );
        assert_eq!(
            extract_js_import("import utils from \"lodash\""),
            Some("lodash".to_owned())
        );
        assert_eq!(extract_js_import("import { bar } from './local'"), None);
        assert_eq!(
            extract_js_import("const x = require('express')"),
            Some("express".to_owned())
        );
    }

    #[test]
    fn test_extract_rust_import() {
        assert_eq!(
            extract_rust_import("use serde::Serialize;"),
            Some("serde.Serialize".to_owned())
        );
        assert_eq!(extract_rust_import("use crate::models::User;"), None);
        assert_eq!(extract_rust_import("use self::inner::Foo;"), None);
    }

    #[test]
    fn test_extract_java_import() {
        assert_eq!(
            extract_java_import("import com.example.service.UserService;"),
            Some("com.example.service.UserService".to_owned())
        );
        assert_eq!(
            extract_java_import("import static org.junit.Assert.assertEquals;"),
            Some("org.junit.Assert.assertEquals".to_owned())
        );
        assert_eq!(extract_java_import("class Foo {}"), None);
    }

    #[test]
    fn test_extract_go_import() {
        assert_eq!(
            extract_go_import("\"github.com/myorg/shared/pkg\""),
            Some("github.com/myorg/shared/pkg".to_owned())
        );
        assert_eq!(extract_go_import("\"fmt\""), None);
    }

    #[test]
    fn test_extract_c_import() {
        assert_eq!(
            extract_c_import("#include \"shared/types.h\""),
            Some("shared.types".to_owned())
        );
        assert_eq!(
            extract_c_import("#include <mylib/core.hpp>"),
            Some("mylib.core".to_owned())
        );
        assert_eq!(extract_c_import("#include \"./local.h\""), None);
    }

    #[test]
    fn test_match_namespace_finds_best_match() {
        let namespaces = vec![
            ProjectNamespace {
                project_name: "backend".to_owned(),
                prefixes: vec!["com.example".to_owned(), "com".to_owned()],
            },
            ProjectNamespace {
                project_name: "shared-lib".to_owned(),
                prefixes: vec!["com.example.shared".to_owned()],
            },
        ];

        // Should match shared-lib (longer prefix)
        let result = match_namespace("com.example.shared.Utils", &namespaces);
        assert_eq!(
            result,
            Some(("shared-lib", "com.example.shared.Utils".to_owned()))
        );

        // Should match backend (com.example prefix)
        let result = match_namespace("com.example.service.UserService", &namespaces);
        assert_eq!(
            result,
            Some(("backend", "com.example.service.UserService".to_owned()))
        );

        // No match
        let result = match_namespace("org.other.Foo", &namespaces);
        assert_eq!(result, None);
    }

    #[test]
    fn test_match_namespace_normalizes_paths() {
        let namespaces = vec![ProjectNamespace {
            project_name: "shared".to_owned(),
            prefixes: vec!["github.com.myorg.shared".to_owned()],
        }];

        // Go-style path with slashes should be normalized
        let result = match_namespace("github.com/myorg/shared/pkg", &namespaces);
        assert_eq!(
            result,
            Some(("shared", "github.com.myorg.shared.pkg".to_owned()))
        );
    }

    #[test]
    fn test_match_namespace_skips_unmatched() {
        let namespaces = vec![ProjectNamespace {
            project_name: "backend".to_owned(),
            prefixes: vec!["com.myapp".to_owned()],
        }];

        // Unmatched import should return None (Requirement 21.4)
        let result = match_namespace("org.external.library", &namespaces);
        assert_eq!(result, None);
    }

    #[test]
    fn test_cross_repo_import_deduplication() {
        let imports = vec![
            CrossRepoImport {
                source_qn: "proj.src.main".to_owned(),
                target_qn: "backend.com.example.User".to_owned(),
                target_project: "backend".to_owned(),
            },
            CrossRepoImport {
                source_qn: "proj.src.main".to_owned(),
                target_qn: "backend.com.example.User".to_owned(),
                target_project: "backend".to_owned(),
            },
        ];

        let mut seen = HashSet::new();
        let deduped: Vec<_> = imports
            .into_iter()
            .filter(|i| seen.insert(i.clone()))
            .collect();
        assert_eq!(deduped.len(), 1);
    }
}
