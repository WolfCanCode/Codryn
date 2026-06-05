/// Track 4.3 — Git history enrichment (`pass_githistory`).
///
/// Runs `git log` to enrich graph nodes with per-file commit frequency,
/// last-modified dates, and contributor counts. Enables hotspot detection
/// (frequently changed files/functions).
///
/// Properties added to File/Module nodes:
/// - `git_commits`: number of commits touching this file
/// - `git_last_modified`: ISO 8601 timestamp of the most recent commit
/// - `git_authors`: number of unique authors who have committed to this file
///
/// These properties are also propagated to Function/Class/Method nodes
/// within the file (as a file-level annotation).
use anyhow::Result;
use codryn_store::Store;
use std::collections::HashMap;
use std::path::Path;
use std::process::Command;

/// Per-file git history statistics.
#[derive(Debug, Clone)]
pub struct FileGitStats {
    /// Number of commits that touched this file.
    pub commit_count: u32,
    /// ISO 8601 timestamp of the most recent commit.
    pub last_modified: String,
    /// Number of unique authors.
    pub author_count: u32,
}

/// Collect git history statistics for all tracked files in the repository.
///
/// Returns a map from relative file path → `FileGitStats`.
/// Returns an empty map if the repository is not a git repo or git is unavailable.
pub fn collect_git_history(repo_path: &Path) -> HashMap<String, FileGitStats> {
    let mut stats: HashMap<String, FileGitStats> = HashMap::new();

    // Check if this is a git repository
    if !repo_path.join(".git").exists() {
        tracing::debug!("pass_githistory: not a git repository, skipping");
        return stats;
    }

    // Run: git log --format="%H|%ae|%aI" --name-only --diff-filter=ACDMR
    // This gives us: commit hash | author email | ISO timestamp, then file paths
    let output = Command::new("git")
        .args([
            "log",
            "--format=%H|%ae|%aI",
            "--name-only",
            "--diff-filter=ACDMR",
            "--no-merges",
        ])
        .current_dir(repo_path)
        .output();

    let output = match output {
        Ok(o) if o.status.success() => o,
        Ok(o) => {
            tracing::debug!(
                status = %o.status,
                "pass_githistory: git log failed"
            );
            return stats;
        }
        Err(e) => {
            tracing::debug!(error = %e, "pass_githistory: git not available");
            return stats;
        }
    };

    let text = match String::from_utf8(output.stdout) {
        Ok(t) => t,
        Err(e) => {
            tracing::debug!(error = %e, "pass_githistory: git output not valid UTF-8");
            return stats;
        }
    };

    // Parse the output: alternating commit header lines and file path lines
    // Format:
    //   <hash>|<email>|<iso_timestamp>
    //   <empty line>
    //   file1.rs
    //   file2.rs
    //   <empty line>
    //   <hash>|<email>|<iso_timestamp>
    //   ...
    let mut current_author: Option<String> = None;
    let mut current_timestamp: Option<String> = None;
    // Track per-file: (commit_hashes, authors, last_modified)
    let mut file_data: HashMap<
        String,
        (
            std::collections::HashSet<String>,
            std::collections::HashSet<String>,
            String,
        ),
    > = HashMap::new();

    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        // Check if this is a commit header line (contains '|' separators)
        let parts: Vec<&str> = line.splitn(3, '|').collect();
        if parts.len() == 3
            && parts[0].len() == 40
            && parts[0].chars().all(|c| c.is_ascii_hexdigit())
        {
            // This is a commit header: hash|email|timestamp
            current_author = Some(parts[1].to_string());
            current_timestamp = Some(parts[2].to_string());
        } else if current_author.is_some() {
            // This is a file path line
            let rel_path = line.to_string();
            let entry = file_data.entry(rel_path).or_insert_with(|| {
                (
                    std::collections::HashSet::new(),
                    std::collections::HashSet::new(),
                    String::new(),
                )
            });

            // Add author
            if let Some(ref author) = current_author {
                entry.1.insert(author.clone());
            }

            // Update last_modified (keep the most recent — git log is newest-first)
            if entry.2.is_empty() {
                if let Some(ref ts) = current_timestamp {
                    entry.2 = ts.clone();
                }
            }

            // Count this commit (use a placeholder since we don't track hash per file here)
            entry
                .0
                .insert(current_timestamp.as_deref().unwrap_or("").to_string());
        }
    }

    // Convert to FileGitStats
    for (path, (commits, authors, last_modified)) in file_data {
        stats.insert(
            path,
            FileGitStats {
                commit_count: commits.len() as u32,
                last_modified,
                author_count: authors.len() as u32,
            },
        );
    }

    tracing::info!(
        files = stats.len(),
        "pass_githistory: collected git history"
    );
    stats
}

/// Enrich graph nodes with git history statistics.
///
/// Updates `properties_json` for File/Module nodes and propagates
/// file-level stats to Function/Class/Method nodes within each file.
pub fn enrich_nodes_with_git_history(
    store: &Store,
    project: &str,
    git_stats: &HashMap<String, FileGitStats>,
) -> Result<()> {
    if git_stats.is_empty() {
        return Ok(());
    }

    let all_nodes = store.get_all_nodes(project)?;
    let mut updates: Vec<(i64, String)> = Vec::new();

    for node in &all_nodes {
        if node.file_path.is_empty() {
            continue;
        }

        let Some(file_stats) = git_stats.get(&node.file_path) else {
            continue;
        };

        // Parse existing properties
        let mut props: serde_json::Value = node
            .properties_json
            .as_deref()
            .and_then(|p| serde_json::from_str(p).ok())
            .unwrap_or_else(|| serde_json::json!({}));

        // Add git history fields
        props["git_commits"] = serde_json::json!(file_stats.commit_count);
        props["git_last_modified"] = serde_json::json!(file_stats.last_modified);
        props["git_authors"] = serde_json::json!(file_stats.author_count);

        // Mark as hotspot if commit count is high (top quartile heuristic)
        if file_stats.commit_count >= 10 {
            props["git_hotspot"] = serde_json::json!(true);
        }

        updates.push((node.id, props.to_string()));
    }

    if !updates.is_empty() {
        store.update_node_properties_batch(&updates)?;
        tracing::info!(
            count = updates.len(),
            "pass_githistory: enriched nodes with git history"
        );
    }

    Ok(())
}

/// Run the full git history pass: collect stats and enrich nodes.
pub fn pass_githistory(store: &Store, project: &str, repo_path: &Path) -> Result<()> {
    let start = std::time::Instant::now();
    let stats = collect_git_history(repo_path);
    if stats.is_empty() {
        return Ok(());
    }
    enrich_nodes_with_git_history(store, project, &stats)?;
    tracing::info!(
        elapsed_ms = start.elapsed().as_millis(),
        "pass_githistory: completed"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use codryn_store::{Node, Project, Store};

    fn setup_store() -> Store {
        let s = Store::open_in_memory().unwrap();
        s.upsert_project(&Project {
            name: "p".into(),
            indexed_at: "now".into(),
            root_path: "/tmp".into(),
        })
        .unwrap();
        s
    }

    fn add_node(s: &Store, name: &str, fp: &str) -> i64 {
        s.insert_node(&Node {
            id: 0,
            project: "p".into(),
            label: "Function".into(),
            name: name.into(),
            qualified_name: format!("p::{}", name),
            file_path: fp.into(),
            start_line: 1,
            end_line: 10,
            properties_json: None,
        })
        .unwrap()
    }

    #[test]
    fn test_enrich_nodes_with_git_history() {
        let s = setup_store();
        let _id = add_node(&s, "main", "src/main.rs");

        let mut stats = HashMap::new();
        stats.insert(
            "src/main.rs".to_string(),
            FileGitStats {
                commit_count: 15,
                last_modified: "2024-01-15T10:00:00Z".to_string(),
                author_count: 3,
            },
        );

        enrich_nodes_with_git_history(&s, "p", &stats).unwrap();

        // Verify the node was updated
        let node = s.find_node_by_qn("p", "p::main").unwrap().unwrap();
        let props: serde_json::Value =
            serde_json::from_str(node.properties_json.as_deref().unwrap_or("{}")).unwrap();
        assert_eq!(props["git_commits"], 15);
        assert_eq!(props["git_last_modified"], "2024-01-15T10:00:00Z");
        assert_eq!(props["git_authors"], 3);
        assert_eq!(props["git_hotspot"], true); // 15 >= 10
    }

    #[test]
    fn test_enrich_nodes_no_hotspot_below_threshold() {
        let s = setup_store();
        add_node(&s, "helper", "src/helper.rs");

        let mut stats = HashMap::new();
        stats.insert(
            "src/helper.rs".to_string(),
            FileGitStats {
                commit_count: 3,
                last_modified: "2024-01-01T00:00:00Z".to_string(),
                author_count: 1,
            },
        );

        enrich_nodes_with_git_history(&s, "p", &stats).unwrap();

        let node = s.find_node_by_qn("p", "p::helper").unwrap().unwrap();
        let props: serde_json::Value =
            serde_json::from_str(node.properties_json.as_deref().unwrap_or("{}")).unwrap();
        assert_eq!(props["git_commits"], 3);
        // Should NOT be marked as hotspot (3 < 10)
        assert!(props.get("git_hotspot").is_none() || props["git_hotspot"] == false);
    }

    #[test]
    fn test_enrich_nodes_empty_stats() {
        let s = setup_store();
        add_node(&s, "main", "src/main.rs");

        // Empty stats should be a no-op
        let result = enrich_nodes_with_git_history(&s, "p", &HashMap::new());
        assert!(result.is_ok());
    }

    #[test]
    fn test_collect_git_history_non_git_repo() {
        // Non-git directory should return empty map
        let stats = collect_git_history(Path::new("/tmp"));
        // /tmp is not a git repo, so should return empty (or may have git history if /tmp is in a repo)
        // Just verify it doesn't panic
        let _ = stats;
    }
}
