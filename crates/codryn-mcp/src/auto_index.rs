use chrono::Utc;
use codryn_pipeline::{IndexMode, Pipeline};
use codryn_store::Store;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Default staleness threshold: 1 hour.
const DEFAULT_STALENESS_SECS: u64 = 3600;

/// Tracks auto-indexing state per project and triggers background re-indexing
/// when a project's index is detected as stale.
#[derive(Debug, Clone)]
pub struct AutoIndexer {
    store_path: PathBuf,
    staleness_threshold: Duration,
    /// Projects currently being re-indexed (prevents concurrent reindex).
    in_progress: Arc<Mutex<HashMap<String, Instant>>>,
}

impl AutoIndexer {
    pub fn new(store_path: &Path) -> Self {
        Self {
            store_path: store_path.to_owned(),
            staleness_threshold: Duration::from_secs(DEFAULT_STALENESS_SECS),
            in_progress: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn set_staleness_threshold(&mut self, secs: u64) {
        self.staleness_threshold = Duration::from_secs(secs);
    }

    /// Check if a project is stale and trigger background re-index if needed.
    /// Returns immediately — re-indexing happens on a background thread.
    pub fn check_and_reindex(&self, project_name: &str, root_path: &str) {
        // Check if already in progress
        {
            let guard = self.in_progress.lock().unwrap();
            if guard.contains_key(project_name) {
                return;
            }
        }

        // Check staleness by opening the store and reading indexed_at
        let store = match open_store(&self.store_path) {
            Some(s) => s,
            None => return,
        };
        let project = match store.get_project(project_name) {
            Ok(Some(p)) => p,
            _ => return,
        };

        if !is_stale(&project.indexed_at, self.staleness_threshold) {
            return;
        }

        // Always use Full mode for auto-reindex to prevent data loss.
        // Fast mode's delete-then-reinsert pattern is unsafe when multiple
        // processes trigger concurrent reindexes. Full mode is safe because
        // it deletes all edges first, then rebuilds from scratch.
        let mode = IndexMode::Full;

        // Mark as in-progress and spawn background thread
        {
            let mut guard = self.in_progress.lock().unwrap();
            guard.insert(project_name.to_string(), Instant::now());
        }

        let project_name = project_name.to_string();
        let root_path = PathBuf::from(root_path);
        let store_path = self.store_path.clone();
        let in_progress = self.in_progress.clone();

        std::thread::spawn(move || {
            tracing::info!(project = %project_name, mode = ?mode, "auto-index: starting background reindex");
            let pipeline = Pipeline::new(&root_path, &store_path, mode);
            match pipeline.run() {
                Ok(()) => {
                    tracing::info!(project = %project_name, "auto-index: reindex complete");
                }
                Err(e) => {
                    tracing::error!(
                        project = %project_name,
                        error = %e,
                        "auto-index: reindex failed"
                    );
                }
            }
            // Remove from in-progress regardless of success/failure
            let mut guard = in_progress.lock().unwrap();
            guard.remove(&project_name);
        });
    }

    /// Check if a project is currently being re-indexed.
    #[cfg(test)]
    pub fn is_in_progress(&self, project_name: &str) -> bool {
        let guard = self.in_progress.lock().unwrap();
        guard.contains_key(project_name)
    }

    /// Return the number of currently active index runs.
    pub fn active_index_runs(&self) -> usize {
        let guard = self.in_progress.lock().unwrap();
        guard.len()
    }
}

/// Parse the indexed_at timestamp and check if it's older than the threshold.
pub fn is_stale(indexed_at: &str, threshold: Duration) -> bool {
    let indexed_ts = chrono::DateTime::parse_from_rfc3339(indexed_at)
        .ok()
        .map(|dt| dt.timestamp() as u64)
        .unwrap_or(0);
    let now = Utc::now().timestamp() as u64;
    let age = Duration::from_secs(now.saturating_sub(indexed_ts));
    age >= threshold
}

/// Open the store, returning None on failure.
fn open_store(store_path: &Path) -> Option<Store> {
    if store_path.to_string_lossy() == ":memory:" {
        Store::open_in_memory().ok()
    } else {
        std::fs::create_dir_all(store_path).ok()?;
        Store::open(&store_path.join("graph.db")).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use codryn_store::Project;
    use std::time::Duration;

    fn test_store_with_project(indexed_at: &str) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::TempDir::new().unwrap();
        let store_path = dir.path().to_path_buf();
        let db_path = dir.path().join("graph.db");
        let store = Store::open(&db_path).unwrap();
        store
            .upsert_project(&Project {
                name: "test-project".into(),
                indexed_at: indexed_at.into(),
                root_path: "/tmp/test".into(),
            })
            .unwrap();
        (dir, store_path)
    }

    #[test]
    fn test_staleness_detection_stale() {
        // Project indexed 2 hours ago with 1-hour threshold should be stale
        let two_hours_ago = (Utc::now() - chrono::Duration::hours(2)).to_rfc3339();
        assert!(is_stale(&two_hours_ago, Duration::from_secs(3600)));
    }

    #[test]
    fn test_staleness_detection_fresh() {
        // Project indexed 10 minutes ago with 1-hour threshold should NOT be stale
        let ten_min_ago = (Utc::now() - chrono::Duration::minutes(10)).to_rfc3339();
        assert!(!is_stale(&ten_min_ago, Duration::from_secs(3600)));
    }

    #[test]
    fn test_staleness_detection_custom_threshold() {
        // Project indexed 5 minutes ago with 1-minute threshold should be stale
        let five_min_ago = (Utc::now() - chrono::Duration::minutes(5)).to_rfc3339();
        assert!(is_stale(&five_min_ago, Duration::from_secs(60)));
    }

    #[test]
    fn test_staleness_detection_invalid_timestamp() {
        // Invalid timestamp should be treated as stale (epoch 0)
        assert!(is_stale("not-a-timestamp", Duration::from_secs(3600)));
    }

    #[test]
    fn test_staleness_detection_exact_boundary() {
        // Project indexed exactly at threshold should be stale (>= comparison)
        let exactly_one_hour_ago = (Utc::now() - chrono::Duration::hours(1)).to_rfc3339();
        assert!(is_stale(&exactly_one_hour_ago, Duration::from_secs(3600)));
    }

    #[test]
    fn test_concurrent_reindex_prevention() {
        let two_hours_ago = (Utc::now() - chrono::Duration::hours(2)).to_rfc3339();
        let (_dir, store_path) = test_store_with_project(&two_hours_ago);

        let indexer = AutoIndexer::new(&store_path);

        // Manually insert into in_progress to simulate an ongoing reindex
        {
            let mut guard = indexer.in_progress.lock().unwrap();
            guard.insert("test-project".to_string(), Instant::now());
        }

        // check_and_reindex should return immediately without spawning another thread
        indexer.check_and_reindex("test-project", "/tmp/test");

        // Still only one entry in in_progress (no duplicate)
        let guard = indexer.in_progress.lock().unwrap();
        assert!(guard.contains_key("test-project"));
        assert_eq!(guard.len(), 1);
    }

    #[test]
    fn test_fresh_project_no_reindex() {
        let recent = Utc::now().to_rfc3339();
        let (_dir, store_path) = test_store_with_project(&recent);

        let indexer = AutoIndexer::new(&store_path);
        indexer.check_and_reindex("test-project", "/tmp/test");

        // Should NOT be in progress since the project is fresh
        // Give a tiny moment for the check to complete
        std::thread::sleep(Duration::from_millis(50));
        assert!(!indexer.is_in_progress("test-project"));
    }

    #[test]
    fn test_nonexistent_project_no_reindex() {
        let (_dir, store_path) = test_store_with_project(&Utc::now().to_rfc3339());

        let indexer = AutoIndexer::new(&store_path);
        // Try to reindex a project that doesn't exist
        indexer.check_and_reindex("nonexistent", "/tmp/nope");

        std::thread::sleep(Duration::from_millis(50));
        assert!(!indexer.is_in_progress("nonexistent"));
    }

    #[test]
    fn test_set_staleness_threshold() {
        let mut indexer = AutoIndexer::new(Path::new("/tmp"));
        assert_eq!(indexer.staleness_threshold, Duration::from_secs(3600));

        indexer.set_staleness_threshold(120);
        assert_eq!(indexer.staleness_threshold, Duration::from_secs(120));
    }

    #[test]
    fn test_non_blocking_check() {
        // Verify that check_and_reindex returns immediately even for stale projects
        let two_hours_ago = (Utc::now() - chrono::Duration::hours(2)).to_rfc3339();
        let (_dir, store_path) = test_store_with_project(&two_hours_ago);

        let indexer = AutoIndexer::new(&store_path);
        let start = Instant::now();
        // This should return immediately (non-blocking)
        // The actual reindex will fail because /tmp/test doesn't exist, but that's fine
        indexer.check_and_reindex("test-project", "/tmp/test");
        let elapsed = start.elapsed();

        // Should complete in well under 1 second (just spawns a thread)
        assert!(
            elapsed < Duration::from_secs(1),
            "check_and_reindex took too long: {:?}",
            elapsed
        );
    }
}
