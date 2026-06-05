use anyhow::Result;
use codryn_store::Store;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::path::Path;
use std::time::Duration;

/// Threshold above which a warning is included in query responses.
const WARNING_THRESHOLD: f64 = 0.20;

/// Threshold above which an incremental re-index is triggered.
const REINDEX_THRESHOLD: f64 = 0.50;

/// Maximum time allowed for staleness computation (requirement 24.5).
const STALENESS_COMPUTATION_TIMEOUT: Duration = Duration::from_millis(500);

/// Maximum time allowed for triggered re-index (requirement 24.3).
const REINDEX_TIMEOUT: Duration = Duration::from_secs(30);

/// Report from computing staleness for a project.
#[derive(Debug, Clone, Serialize)]
pub struct StalenessReport {
    pub project: String,
    pub score: f64,
    pub total_files: usize,
    pub changed_files: usize,
    pub stale_files: Vec<String>,
    pub should_reindex: bool,
}

/// Annotation to be added to MCP query responses.
#[derive(Debug, Clone, Serialize)]
pub struct StalenessAnnotation {
    pub staleness_score: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub warning: Option<String>,
}

/// Compute the staleness score for a project.
///
/// The score is the ratio of stale files (files whose content hash differs
/// from disk) to total indexed files. Uses file modification timestamps
/// for fast comparison (within 500ms budget), falling back to full hash
/// comparison only for files whose mtime has changed.
pub fn compute_staleness(
    store: &Store,
    project: &str,
    repo_root: &Path,
    scope: Option<&str>,
) -> Result<StalenessReport> {
    let hashes = store.get_file_hashes(project)?;
    let filtered: Vec<_> = match scope {
        Some(s) => hashes
            .into_iter()
            .filter(|h| h.rel_path.starts_with(s))
            .collect(),
        None => hashes,
    };

    let total_files = filtered.len();
    let mut changed_files = 0usize;
    let mut stale_files = Vec::new();

    let deadline = std::time::Instant::now() + STALENESS_COMPUTATION_TIMEOUT;
    let mut checked_count = 0usize;

    for fh in &filtered {
        // If we've exceeded the 500ms budget, stop checking and extrapolate
        if std::time::Instant::now() >= deadline {
            break;
        }

        let abs = repo_root.join(&fh.rel_path);
        let is_stale = check_file_freshness(fh.mtime_ns, &fh.sha256, &abs);
        if is_stale {
            changed_files += 1;
            stale_files.push(fh.rel_path.clone());
        }
        checked_count += 1;
    }

    // If we timed out before checking all files, extrapolate the staleness ratio
    if checked_count < total_files && checked_count > 0 {
        let ratio = changed_files as f64 / checked_count as f64;
        let estimated_changed = (ratio * total_files as f64).round() as usize;
        changed_files = estimated_changed;
    }

    let score = if total_files == 0 {
        0.0
    } else {
        changed_files as f64 / total_files as f64
    };

    Ok(StalenessReport {
        project: project.to_string(),
        score,
        total_files,
        changed_files,
        stale_files,
        should_reindex: score > REINDEX_THRESHOLD,
    })
}

/// Compute just the staleness score (lightweight version for query annotation).
/// Returns (score, set of stale file paths).
pub fn compute_score(store: &Store, project: &str, repo_root: &Path) -> (f64, HashSet<String>) {
    let hashes = match store.get_file_hashes(project) {
        Ok(h) => h,
        Err(_) => return (0.0, HashSet::new()),
    };

    let total_files = hashes.len();
    if total_files == 0 {
        return (0.0, HashSet::new());
    }

    let mut stale_set = HashSet::new();
    let deadline = std::time::Instant::now() + STALENESS_COMPUTATION_TIMEOUT;

    for fh in &hashes {
        if std::time::Instant::now() >= deadline {
            break;
        }

        let abs = repo_root.join(&fh.rel_path);
        if check_file_freshness(fh.mtime_ns, &fh.sha256, &abs) {
            stale_set.insert(fh.rel_path.clone());
        }
    }

    let score = stale_set.len() as f64 / total_files as f64;
    (score, stale_set)
}

/// Check if a file is stale by comparing modification timestamps first,
/// then falling back to full hash comparison only when mtime differs.
///
/// Returns `true` if the file is stale (content has changed or file is missing).
fn check_file_freshness(stored_mtime_ns: i64, stored_hash: &str, file_path: &Path) -> bool {
    let metadata = match std::fs::metadata(file_path) {
        Ok(m) => m,
        Err(_) => return true, // file missing = stale
    };

    // Compare modification timestamps first (fast path)
    let current_mtime_ns = metadata_mtime_ns(&metadata);
    if current_mtime_ns == stored_mtime_ns {
        // Timestamp unchanged — file is fresh
        return false;
    }

    // Timestamp changed — fall back to full hash comparison
    let content = match std::fs::read(file_path) {
        Ok(c) => c,
        Err(_) => return true, // can't read = treat as stale
    };
    let hash = hex::encode(Sha256::digest(&content));
    hash != stored_hash
}

/// Extract modification time in nanoseconds from file metadata.
#[cfg(unix)]
fn metadata_mtime_ns(metadata: &std::fs::Metadata) -> i64 {
    use std::os::unix::fs::MetadataExt;
    let secs = metadata.mtime();
    let nsecs = metadata.mtime_nsec();
    secs * 1_000_000_000 + nsecs
}

#[cfg(not(unix))]
fn metadata_mtime_ns(metadata: &std::fs::Metadata) -> i64 {
    metadata
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_nanos() as i64)
        .unwrap_or(0)
}

/// Build a staleness annotation for MCP query responses.
///
/// Returns a `StalenessAnnotation` with the score and an optional warning
/// when the score exceeds the warning threshold (0.20).
pub fn build_annotation(score: f64) -> StalenessAnnotation {
    let warning = if score > WARNING_THRESHOLD {
        Some(format!(
            "Index may be stale ({:.0}% of files have changed on disk)",
            score * 100.0
        ))
    } else {
        None
    };
    StalenessAnnotation {
        staleness_score: score,
        warning,
    }
}

/// Annotate individual query result items with `potentially_stale: true`
/// for items whose file_path is in the set of stale files.
pub fn annotate_result(result: &mut serde_json::Value, stale_files: &HashSet<String>) {
    if let Some(file_path) = result.get("file_path").and_then(|v| v.as_str()) {
        if stale_files.contains(file_path) {
            result.as_object_mut().map(|obj| {
                obj.insert(
                    "potentially_stale".to_string(),
                    serde_json::Value::Bool(true),
                )
            });
        }
    }
}

/// Annotate a list of result items with staleness information.
pub fn annotate_results(results: &mut [serde_json::Value], stale_files: &HashSet<String>) {
    for item in results.iter_mut() {
        annotate_result(item, stale_files);
    }
}

/// Check if an incremental re-index should be triggered based on staleness.
///
/// Returns `true` if staleness exceeds the reindex threshold (0.50) and
/// at least one result item references a stale file.
pub fn should_trigger_reindex(
    score: f64,
    result_file_paths: &[&str],
    stale_files: &HashSet<String>,
) -> bool {
    if score <= REINDEX_THRESHOLD {
        return false;
    }
    // At least one result must reference a stale file
    result_file_paths.iter().any(|fp| stale_files.contains(*fp))
}

/// Returns the reindex timeout duration (30 seconds).
pub fn reindex_timeout() -> Duration {
    REINDEX_TIMEOUT
}

/// Returns the warning threshold.
pub fn warning_threshold() -> f64 {
    WARNING_THRESHOLD
}

/// Returns the reindex threshold.
pub fn reindex_threshold() -> f64 {
    REINDEX_THRESHOLD
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn test_build_annotation_below_threshold() {
        let ann = build_annotation(0.10);
        assert_eq!(ann.staleness_score, 0.10);
        assert!(ann.warning.is_none());
    }

    #[test]
    fn test_build_annotation_above_threshold() {
        let ann = build_annotation(0.30);
        assert_eq!(ann.staleness_score, 0.30);
        assert!(ann.warning.is_some());
        assert!(ann.warning.unwrap().contains("stale"));
    }

    #[test]
    fn test_build_annotation_at_threshold() {
        // Exactly at 0.20 should NOT trigger warning (> not >=)
        let ann = build_annotation(0.20);
        assert!(ann.warning.is_none());
    }

    #[test]
    fn test_annotate_result_stale_file() {
        let mut stale = HashSet::new();
        stale.insert("src/main.rs".to_string());

        let mut result = serde_json::json!({
            "name": "main",
            "file_path": "src/main.rs"
        });
        annotate_result(&mut result, &stale);
        assert_eq!(result["potentially_stale"], true);
    }

    #[test]
    fn test_annotate_result_fresh_file() {
        let mut stale = HashSet::new();
        stale.insert("src/other.rs".to_string());

        let mut result = serde_json::json!({
            "name": "main",
            "file_path": "src/main.rs"
        });
        annotate_result(&mut result, &stale);
        assert!(result.get("potentially_stale").is_none());
    }

    #[test]
    fn test_annotate_results_mixed() {
        let mut stale = HashSet::new();
        stale.insert("src/stale.rs".to_string());

        let mut results = vec![
            serde_json::json!({"name": "a", "file_path": "src/stale.rs"}),
            serde_json::json!({"name": "b", "file_path": "src/fresh.rs"}),
            serde_json::json!({"name": "c", "file_path": "src/stale.rs"}),
        ];
        annotate_results(&mut results, &stale);

        assert_eq!(results[0]["potentially_stale"], true);
        assert!(results[1].get("potentially_stale").is_none());
        assert_eq!(results[2]["potentially_stale"], true);
    }

    #[test]
    fn test_should_trigger_reindex_above_threshold_with_stale_result() {
        let mut stale = HashSet::new();
        stale.insert("src/main.rs".to_string());

        assert!(should_trigger_reindex(0.60, &["src/main.rs"], &stale));
    }

    #[test]
    fn test_should_trigger_reindex_above_threshold_no_stale_result() {
        let mut stale = HashSet::new();
        stale.insert("src/other.rs".to_string());

        // Score is high but no result references a stale file
        assert!(!should_trigger_reindex(0.60, &["src/main.rs"], &stale));
    }

    #[test]
    fn test_should_trigger_reindex_below_threshold() {
        let mut stale = HashSet::new();
        stale.insert("src/main.rs".to_string());

        assert!(!should_trigger_reindex(0.30, &["src/main.rs"], &stale));
    }

    #[test]
    fn test_should_trigger_reindex_at_threshold() {
        let mut stale = HashSet::new();
        stale.insert("src/main.rs".to_string());

        // Exactly at 0.50 should NOT trigger (> not >=)
        assert!(!should_trigger_reindex(0.50, &["src/main.rs"], &stale));
    }

    #[test]
    fn test_compute_score_empty_project() {
        // With an in-memory store that has no file hashes, score should be 0.0
        let store = codryn_store::Store::open_in_memory().unwrap();
        let (score, stale) = compute_score(&store, "nonexistent", Path::new("/tmp"));
        assert_eq!(score, 0.0);
        assert!(stale.is_empty());
    }

    #[test]
    fn test_check_file_freshness_missing_file() {
        // Non-existent file should be considered stale
        assert!(check_file_freshness(
            0,
            "abc123",
            Path::new("/nonexistent/path/file.rs")
        ));
    }

    #[test]
    fn test_check_file_freshness_matching_mtime() {
        // Create a temp file and check with matching mtime
        let dir = tempfile::TempDir::new().unwrap();
        let file_path = dir.path().join("test.txt");
        std::fs::write(&file_path, "hello").unwrap();

        let metadata = std::fs::metadata(&file_path).unwrap();
        let mtime = metadata_mtime_ns(&metadata);

        // With matching mtime, file should be fresh regardless of hash
        assert!(!check_file_freshness(mtime, "wrong_hash", &file_path));
    }

    #[test]
    fn test_check_file_freshness_different_mtime_same_content() {
        // Create a temp file
        let dir = tempfile::TempDir::new().unwrap();
        let file_path = dir.path().join("test.txt");
        std::fs::write(&file_path, "hello").unwrap();

        let content_hash = hex::encode(Sha256::digest(b"hello"));

        // Different mtime but same hash — file is fresh
        assert!(!check_file_freshness(0, &content_hash, &file_path));
    }

    #[test]
    fn test_check_file_freshness_different_mtime_different_content() {
        // Create a temp file
        let dir = tempfile::TempDir::new().unwrap();
        let file_path = dir.path().join("test.txt");
        std::fs::write(&file_path, "hello").unwrap();

        // Different mtime and different hash — file is stale
        assert!(check_file_freshness(0, "wrong_hash", &file_path));
    }
}
