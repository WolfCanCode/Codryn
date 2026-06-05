use anyhow::{Context, Result};
use chrono::Utc;
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::Store;

/// Monotonic counter to ensure unique IDs even within the same millisecond.
static RUN_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Status of an index run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum IndexRunStatus {
    Running,
    Completed,
    Failed,
    Canceled,
}

impl fmt::Display for IndexRunStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            IndexRunStatus::Running => write!(f, "Running"),
            IndexRunStatus::Completed => write!(f, "Completed"),
            IndexRunStatus::Failed => write!(f, "Failed"),
            IndexRunStatus::Canceled => write!(f, "Canceled"),
        }
    }
}

impl FromStr for IndexRunStatus {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self> {
        match s {
            "Running" => Ok(IndexRunStatus::Running),
            "Completed" => Ok(IndexRunStatus::Completed),
            "Failed" => Ok(IndexRunStatus::Failed),
            "Canceled" => Ok(IndexRunStatus::Canceled),
            other => anyhow::bail!("unknown IndexRunStatus: {}", other),
        }
    }
}

/// Record of a single index run for a project.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexRun {
    pub id: String,
    pub project: String,
    pub mode: String,
    pub status: IndexRunStatus,
    pub git_commit: Option<String>,
    pub started_at: String,
    pub completed_at: Option<String>,
    pub node_count: i64,
    pub edge_count: i64,
    pub error: Option<String>,
}

impl Store {
    /// Start a new index run for a project. Returns the created `IndexRun`.
    pub fn start_index_run(
        &self,
        project: &str,
        mode: &str,
        git_commit: Option<&str>,
    ) -> Result<IndexRun> {
        let now = Utc::now();
        let started_at = now.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string();
        // Generate a unique id: project + timestamp millis + monotonic counter
        let seq = RUN_COUNTER.fetch_add(1, Ordering::Relaxed);
        let id = format!("{}-{}-{}", project, now.timestamp_millis(), seq);

        self.conn
            .execute(
                "INSERT INTO _index_runs \
                 (id, project, mode, status, git_commit, started_at, completed_at, node_count, edge_count, error) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, NULL, 0, 0, NULL)",
                rusqlite::params![
                    id,
                    project,
                    mode,
                    IndexRunStatus::Running.to_string(),
                    git_commit,
                    started_at,
                ],
            )
            .context("failed to insert index run")?;

        Ok(IndexRun {
            id,
            project: project.to_string(),
            mode: mode.to_string(),
            status: IndexRunStatus::Running,
            git_commit: git_commit.map(|s| s.to_string()),
            started_at,
            completed_at: None,
            node_count: 0,
            edge_count: 0,
            error: None,
        })
    }

    /// Mark an index run as completed with final node/edge counts.
    pub fn complete_index_run(&self, run_id: &str, node_count: i64, edge_count: i64) -> Result<()> {
        let completed_at = Utc::now().format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string();
        let rows = self
            .conn
            .execute(
                "UPDATE _index_runs \
                 SET status = ?1, completed_at = ?2, node_count = ?3, edge_count = ?4 \
                 WHERE id = ?5",
                rusqlite::params![
                    IndexRunStatus::Completed.to_string(),
                    completed_at,
                    node_count,
                    edge_count,
                    run_id,
                ],
            )
            .context("failed to complete index run")?;
        if rows == 0 {
            anyhow::bail!("index run not found: {}", run_id);
        }
        Ok(())
    }

    /// Mark an index run as failed with an error message.
    pub fn fail_index_run(&self, run_id: &str, error: &str) -> Result<()> {
        let completed_at = Utc::now().format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string();
        let rows = self
            .conn
            .execute(
                "UPDATE _index_runs \
                 SET status = ?1, completed_at = ?2, error = ?3 \
                 WHERE id = ?4",
                rusqlite::params![
                    IndexRunStatus::Failed.to_string(),
                    completed_at,
                    error,
                    run_id,
                ],
            )
            .context("failed to fail index run")?;
        if rows == 0 {
            anyhow::bail!("index run not found: {}", run_id);
        }
        Ok(())
    }

    /// Mark an index run as canceled.
    pub fn cancel_index_run(&self, run_id: &str) -> Result<()> {
        let completed_at = Utc::now().format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string();
        let rows = self
            .conn
            .execute(
                "UPDATE _index_runs \
                 SET status = ?1, completed_at = ?2 \
                 WHERE id = ?3",
                rusqlite::params![IndexRunStatus::Canceled.to_string(), completed_at, run_id,],
            )
            .context("failed to cancel index run")?;
        if rows == 0 {
            anyhow::bail!("index run not found: {}", run_id);
        }
        Ok(())
    }

    /// List recent index runs for a project, ordered by started_at DESC.
    pub fn list_index_runs(&self, project: &str, limit: usize) -> Result<Vec<IndexRun>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, project, mode, status, git_commit, started_at, completed_at, \
                    node_count, edge_count, error \
             FROM _index_runs \
             WHERE project = ?1 \
             ORDER BY started_at DESC \
             LIMIT ?2",
        )?;

        let runs = stmt
            .query_map(rusqlite::params![project, limit as i64], |row| {
                let status_str: String = row.get(3)?;
                let status =
                    IndexRunStatus::from_str(&status_str).unwrap_or(IndexRunStatus::Failed);
                Ok(IndexRun {
                    id: row.get(0)?,
                    project: row.get(1)?,
                    mode: row.get(2)?,
                    status,
                    git_commit: row.get(4)?,
                    started_at: row.get(5)?,
                    completed_at: row.get(6)?,
                    node_count: row.get(7)?,
                    edge_count: row.get(8)?,
                    error: row.get(9)?,
                })
            })
            .context("failed to query index runs")?
            .collect::<rusqlite::Result<Vec<_>>>()
            .context("failed to collect index runs")?;

        Ok(runs)
    }

    /// Cancel all index runs with status "Running" for a project.
    /// Used during crash recovery to clean up stale running runs.
    pub fn cancel_running_index_runs(&self, project: &str) -> Result<usize> {
        let completed_at = Utc::now().format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string();
        let rows = self
            .conn
            .execute(
                "UPDATE _index_runs \
                 SET status = ?1, completed_at = ?2 \
                 WHERE project = ?3 AND status = ?4",
                rusqlite::params![
                    IndexRunStatus::Canceled.to_string(),
                    completed_at,
                    project,
                    IndexRunStatus::Running.to_string(),
                ],
            )
            .context("failed to cancel running index runs")?;
        Ok(rows)
    }

    /// Get a single index run by id.
    pub fn get_index_run(&self, run_id: &str) -> Result<Option<IndexRun>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, project, mode, status, git_commit, started_at, completed_at, \
                    node_count, edge_count, error \
             FROM _index_runs \
             WHERE id = ?1",
        )?;

        let result = stmt
            .query_row(rusqlite::params![run_id], |row| {
                let status_str: String = row.get(3)?;
                let status =
                    IndexRunStatus::from_str(&status_str).unwrap_or(IndexRunStatus::Failed);
                Ok(IndexRun {
                    id: row.get(0)?,
                    project: row.get(1)?,
                    mode: row.get(2)?,
                    status,
                    git_commit: row.get(4)?,
                    started_at: row.get(5)?,
                    completed_at: row.get(6)?,
                    node_count: row.get(7)?,
                    edge_count: row.get(8)?,
                    error: row.get(9)?,
                })
            })
            .optional()
            .context("failed to get index run")?;

        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Store;

    fn test_store() -> Store {
        Store::open_in_memory().unwrap()
    }

    #[test]
    fn test_start_index_run_creates_running_run() {
        let store = test_store();
        let run = store.start_index_run("myproject", "full", None).unwrap();

        assert_eq!(run.project, "myproject");
        assert_eq!(run.mode, "full");
        assert_eq!(run.status, IndexRunStatus::Running);
        assert!(run.git_commit.is_none());
        assert!(run.completed_at.is_none());
        assert_eq!(run.node_count, 0);
        assert_eq!(run.edge_count, 0);
        assert!(run.error.is_none());
        assert!(!run.id.is_empty());
    }

    #[test]
    fn test_start_index_run_with_git_commit() {
        let store = test_store();
        let run = store
            .start_index_run("proj", "fast", Some("abc123"))
            .unwrap();

        assert_eq!(run.git_commit, Some("abc123".to_string()));
    }

    #[test]
    fn test_complete_index_run() {
        let store = test_store();
        let run = store.start_index_run("proj", "full", None).unwrap();

        store.complete_index_run(&run.id, 100, 200).unwrap();

        let updated = store.get_index_run(&run.id).unwrap().unwrap();
        assert_eq!(updated.status, IndexRunStatus::Completed);
        assert_eq!(updated.node_count, 100);
        assert_eq!(updated.edge_count, 200);
        assert!(updated.completed_at.is_some());
        assert!(updated.error.is_none());
    }

    #[test]
    fn test_fail_index_run() {
        let store = test_store();
        let run = store.start_index_run("proj", "full", None).unwrap();

        store.fail_index_run(&run.id, "out of memory").unwrap();

        let updated = store.get_index_run(&run.id).unwrap().unwrap();
        assert_eq!(updated.status, IndexRunStatus::Failed);
        assert_eq!(updated.error, Some("out of memory".to_string()));
        assert!(updated.completed_at.is_some());
    }

    #[test]
    fn test_cancel_index_run() {
        let store = test_store();
        let run = store.start_index_run("proj", "full", None).unwrap();

        store.cancel_index_run(&run.id).unwrap();

        let updated = store.get_index_run(&run.id).unwrap().unwrap();
        assert_eq!(updated.status, IndexRunStatus::Canceled);
        assert!(updated.completed_at.is_some());
    }

    #[test]
    fn test_complete_nonexistent_run_errors() {
        let store = test_store();
        let result = store.complete_index_run("nonexistent-id", 0, 0);
        assert!(result.is_err());
    }

    #[test]
    fn test_fail_nonexistent_run_errors() {
        let store = test_store();
        let result = store.fail_index_run("nonexistent-id", "error");
        assert!(result.is_err());
    }

    #[test]
    fn test_cancel_nonexistent_run_errors() {
        let store = test_store();
        let result = store.cancel_index_run("nonexistent-id");
        assert!(result.is_err());
    }

    #[test]
    fn test_list_index_runs_returns_most_recent_first() {
        let store = test_store();

        // Create 3 runs with slight delays to ensure ordering
        let run1 = store.start_index_run("proj", "full", None).unwrap();
        // Simulate different timestamps by directly inserting with known timestamps
        store
            .conn
            .execute(
                "UPDATE _index_runs SET started_at = '2025-01-01T10:00:00.000Z' WHERE id = ?1",
                rusqlite::params![run1.id],
            )
            .unwrap();

        let run2 = store.start_index_run("proj", "fast", None).unwrap();
        store
            .conn
            .execute(
                "UPDATE _index_runs SET started_at = '2025-01-02T10:00:00.000Z' WHERE id = ?1",
                rusqlite::params![run2.id],
            )
            .unwrap();

        let run3 = store.start_index_run("proj", "full", None).unwrap();
        store
            .conn
            .execute(
                "UPDATE _index_runs SET started_at = '2025-01-03T10:00:00.000Z' WHERE id = ?1",
                rusqlite::params![run3.id],
            )
            .unwrap();

        let runs = store.list_index_runs("proj", 10).unwrap();
        assert_eq!(runs.len(), 3);
        // Most recent first
        assert_eq!(runs[0].id, run3.id);
        assert_eq!(runs[1].id, run2.id);
        assert_eq!(runs[2].id, run1.id);
    }

    #[test]
    fn test_list_index_runs_respects_limit() {
        let store = test_store();

        for _ in 0..5 {
            store.start_index_run("proj", "full", None).unwrap();
        }

        let runs = store.list_index_runs("proj", 3).unwrap();
        assert_eq!(runs.len(), 3);
    }

    #[test]
    fn test_list_index_runs_filters_by_project() {
        let store = test_store();

        store.start_index_run("proj_a", "full", None).unwrap();
        store.start_index_run("proj_a", "fast", None).unwrap();
        store.start_index_run("proj_b", "full", None).unwrap();

        let runs_a = store.list_index_runs("proj_a", 10).unwrap();
        let runs_b = store.list_index_runs("proj_b", 10).unwrap();

        assert_eq!(runs_a.len(), 2);
        assert_eq!(runs_b.len(), 1);
    }

    #[test]
    fn test_list_index_runs_empty_project() {
        let store = test_store();
        let runs = store.list_index_runs("nonexistent", 10).unwrap();
        assert!(runs.is_empty());
    }

    #[test]
    fn test_cancel_running_index_runs() {
        let store = test_store();

        let run1 = store.start_index_run("proj", "full", None).unwrap();
        let run2 = store.start_index_run("proj", "fast", None).unwrap();
        let run3 = store.start_index_run("proj", "full", None).unwrap();

        // Complete run3 so it should NOT be canceled
        store.complete_index_run(&run3.id, 50, 100).unwrap();

        let canceled = store.cancel_running_index_runs("proj").unwrap();
        assert_eq!(canceled, 2); // run1 and run2 were Running

        let updated1 = store.get_index_run(&run1.id).unwrap().unwrap();
        let updated2 = store.get_index_run(&run2.id).unwrap().unwrap();
        let updated3 = store.get_index_run(&run3.id).unwrap().unwrap();

        assert_eq!(updated1.status, IndexRunStatus::Canceled);
        assert_eq!(updated2.status, IndexRunStatus::Canceled);
        assert_eq!(updated3.status, IndexRunStatus::Completed); // unchanged
    }

    #[test]
    fn test_cancel_running_index_runs_only_affects_target_project() {
        let store = test_store();

        let run_a = store.start_index_run("proj_a", "full", None).unwrap();
        let run_b = store.start_index_run("proj_b", "full", None).unwrap();

        store.cancel_running_index_runs("proj_a").unwrap();

        let updated_a = store.get_index_run(&run_a.id).unwrap().unwrap();
        let updated_b = store.get_index_run(&run_b.id).unwrap().unwrap();

        assert_eq!(updated_a.status, IndexRunStatus::Canceled);
        assert_eq!(updated_b.status, IndexRunStatus::Running); // unaffected
    }

    #[test]
    fn test_cancel_running_index_runs_no_running_is_noop() {
        let store = test_store();
        let canceled = store.cancel_running_index_runs("proj").unwrap();
        assert_eq!(canceled, 0);
    }

    #[test]
    fn test_index_run_status_display_and_from_str() {
        for status in &[
            IndexRunStatus::Running,
            IndexRunStatus::Completed,
            IndexRunStatus::Failed,
            IndexRunStatus::Canceled,
        ] {
            let s = status.to_string();
            let parsed: IndexRunStatus = s.parse().unwrap();
            assert_eq!(&parsed, status);
        }
    }

    #[test]
    fn test_index_run_status_from_str_invalid() {
        let result = IndexRunStatus::from_str("Unknown");
        assert!(result.is_err());
    }
}
