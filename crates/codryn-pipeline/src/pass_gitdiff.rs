//! Git Diff Pass
//!
//! Maps uncommitted file changes to graph nodes by file_path and annotates
//! matched nodes with `has_uncommitted_changes: true`. Skips files with no
//! corresponding graph nodes and handles non-git repos gracefully.
//!
//! Requirements: 17.1, 17.2, 17.3, 17.4, 17.5

use anyhow::Result;
use codryn_store::Store;
use std::path::Path;

/// Run the git diff pass: detect uncommitted changes and annotate graph nodes.
///
/// This pass:
/// 1. Opens the git repository at `repo_path`
/// 2. Computes the diff between HEAD and the working directory (staged + unstaged)
/// 3. For each changed file, queries the store for nodes with matching `file_path`
/// 4. Annotates matched nodes with `has_uncommitted_changes: true` in their properties
/// 5. Skips files that have no corresponding graph nodes
///
/// If the directory is not a git repository or git operations fail, the pass
/// logs a warning and returns Ok(()) without modifying any nodes.
pub fn pass_gitdiff(store: &Store, project: &str, repo_path: &Path) -> Result<()> {
    // Attempt to open the git repository
    let repo = match git2::Repository::open(repo_path) {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(
                error = %e,
                path = %repo_path.display(),
                "pass_gitdiff: not a git repository or git2 failed, skipping"
            );
            return Ok(());
        }
    };

    // Get the diff between HEAD and the working directory
    let changed_files = match get_uncommitted_changes(&repo) {
        Ok(files) => files,
        Err(e) => {
            tracing::warn!(
                error = %e,
                "pass_gitdiff: failed to compute uncommitted changes, skipping"
            );
            return Ok(());
        }
    };

    if changed_files.is_empty() {
        tracing::debug!("pass_gitdiff: no uncommitted changes detected");
        return Ok(());
    }

    tracing::info!(
        changed_files = changed_files.len(),
        "pass_gitdiff: processing uncommitted changes"
    );

    // Collect node property updates: (node_id, updated_properties_json)
    let mut updates: Vec<(i64, String)> = Vec::new();
    let mut annotated_count = 0usize;
    let mut skipped_count = 0usize;

    for file_path in &changed_files {
        // Query the store for nodes whose definition resides in this file
        let nodes = match store.get_nodes_for_file(project, file_path) {
            Ok(n) => n,
            Err(e) => {
                tracing::debug!(
                    error = %e,
                    file = file_path,
                    "pass_gitdiff: failed to query nodes for file, skipping"
                );
                skipped_count += 1;
                continue;
            }
        };

        if nodes.is_empty() {
            // No graph nodes for this file — skip without error (Req 17.3)
            skipped_count += 1;
            continue;
        }

        // Annotate each node with has_uncommitted_changes: true
        for node in &nodes {
            let updated_props = annotate_properties(node.properties_json.as_deref());
            updates.push((node.id, updated_props));
            annotated_count += 1;
        }
    }

    // Batch update all annotated nodes in a single transaction
    if !updates.is_empty() {
        store.update_node_properties_batch(&updates)?;
    }

    tracing::info!(
        annotated = annotated_count,
        skipped = skipped_count,
        "pass_gitdiff: completed node annotation"
    );

    Ok(())
}

/// Get the list of uncommitted file changes (staged + unstaged + untracked).
///
/// Uses git2 to diff HEAD against the working directory. Returns relative
/// file paths of all changed files.
fn get_uncommitted_changes(repo: &git2::Repository) -> Result<Vec<String>> {
    // Get HEAD tree for diffing (handle empty repos with no commits)
    let head_tree = repo.head().ok().and_then(|h| h.peel_to_tree().ok());

    // Diff HEAD tree against the working directory (includes staged + unstaged)
    let mut diff_opts = git2::DiffOptions::new();
    diff_opts
        .include_untracked(true)
        .recurse_untracked_dirs(true);

    let diff = repo.diff_tree_to_workdir_with_index(head_tree.as_ref(), Some(&mut diff_opts))?;

    let mut changed_files: Vec<String> = Vec::new();

    diff.foreach(
        &mut |delta, _| {
            // Prefer new_file path (for additions/modifications), fall back to old_file (for deletions)
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
    )?;

    Ok(changed_files)
}

/// Annotate a node's properties JSON with `has_uncommitted_changes: true`.
///
/// If the node already has properties, merges the new field into the existing
/// JSON object. If properties are empty/null, creates a new JSON object.
fn annotate_properties(existing_props: Option<&str>) -> String {
    match existing_props {
        Some(json_str) if !json_str.is_empty() => {
            // Parse existing properties and add the annotation
            match serde_json::from_str::<serde_json::Value>(json_str) {
                Ok(mut obj) => {
                    if let Some(map) = obj.as_object_mut() {
                        map.insert(
                            "has_uncommitted_changes".to_string(),
                            serde_json::Value::Bool(true),
                        );
                    }
                    serde_json::to_string(&obj)
                        .unwrap_or_else(|_| r#"{"has_uncommitted_changes":true}"#.to_string())
                }
                Err(_) => {
                    // If existing properties aren't valid JSON, create fresh
                    r#"{"has_uncommitted_changes":true}"#.to_string()
                }
            }
        }
        _ => {
            // No existing properties — create new JSON object
            r#"{"has_uncommitted_changes":true}"#.to_string()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_annotate_properties_empty() {
        let result = annotate_properties(None);
        assert_eq!(result, r#"{"has_uncommitted_changes":true}"#);
    }

    #[test]
    fn test_annotate_properties_empty_string() {
        let result = annotate_properties(Some(""));
        assert_eq!(result, r#"{"has_uncommitted_changes":true}"#);
    }

    #[test]
    fn test_annotate_properties_existing_json() {
        let result = annotate_properties(Some(r#"{"language":"rust","complexity":5}"#));
        let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(parsed["has_uncommitted_changes"], true);
        assert_eq!(parsed["language"], "rust");
        assert_eq!(parsed["complexity"], 5);
    }

    #[test]
    fn test_annotate_properties_already_annotated() {
        let result =
            annotate_properties(Some(r#"{"has_uncommitted_changes":false,"name":"test"}"#));
        let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(parsed["has_uncommitted_changes"], true);
        assert_eq!(parsed["name"], "test");
    }

    #[test]
    fn test_annotate_properties_invalid_json() {
        let result = annotate_properties(Some("not valid json"));
        assert_eq!(result, r#"{"has_uncommitted_changes":true}"#);
    }
}
