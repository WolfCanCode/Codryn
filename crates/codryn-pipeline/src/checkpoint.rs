//! Checkpoint system for resuming interrupted indexing runs.
//!
//! This module provides functions to detect incomplete indexing checkpoints,
//! validate their age, clean up partial phase artifacts, and determine
//! whether a pipeline run should resume from a previous checkpoint.

use chrono::{DateTime, Utc};
use codryn_store::Store;

use crate::{phases, ResumeInfo};

/// Default maximum age (in days) for a checkpoint to be considered valid for resume.
pub const DEFAULT_MAX_AGE_DAYS: u64 = 7;

/// Check whether an interrupted indexing run should be resumed.
///
/// Returns `Some(ResumeInfo)` if an incomplete checkpoint exists that is:
/// - Not older than `max_age_days` (default 7 days)
/// - Not corrupted (valid phase name, parseable timestamp)
///
/// Returns `None` if:
/// - No checkpoint exists (logs info message)
/// - Checkpoint is too old (discarded with warning)
/// - Checkpoint is corrupted (discarded with warning, falls back to full index)
pub fn should_resume(store: &Store, project: &str) -> Option<ResumeInfo> {
    should_resume_with_max_age(store, project, DEFAULT_MAX_AGE_DAYS)
}

/// Check whether an interrupted indexing run should be resumed, with a configurable max age.
///
/// This is the inner implementation that accepts a custom `max_age_days` parameter
/// for testability.
pub fn should_resume_with_max_age(
    store: &Store,
    project: &str,
    max_age_days: u64,
) -> Option<ResumeInfo> {
    // Query for incomplete checkpoint
    let checkpoint = match store.get_incomplete_checkpoint(project) {
        Ok(Some(cp)) => cp,
        Ok(None) => {
            tracing::info!(
                project = project,
                "checkpoint: no incomplete checkpoint found, starting fresh index"
            );
            return None;
        }
        Err(e) => {
            tracing::warn!(
                project = project,
                error = %e,
                "checkpoint: failed to query checkpoint (possible corruption), falling back to full index"
            );
            return None;
        }
    };

    // Validate the checkpoint is not corrupted
    if !is_checkpoint_data_valid(&checkpoint.phase, &checkpoint.started_at) {
        tracing::warn!(
            project = project,
            phase = %checkpoint.phase,
            started_at = %checkpoint.started_at,
            "checkpoint: corrupted checkpoint detected (invalid phase or timestamp), discarding and falling back to full index"
        );
        // Attempt to clear the corrupted checkpoint
        if let Err(e) = store.clear_checkpoint(project) {
            tracing::warn!(
                project = project,
                error = %e,
                "checkpoint: failed to clear corrupted checkpoint"
            );
        }
        return None;
    }

    // Validate checkpoint age
    if !is_checkpoint_valid(store, project, max_age_days) {
        tracing::warn!(
            project = project,
            phase = %checkpoint.phase,
            started_at = %checkpoint.started_at,
            max_age_days = max_age_days,
            "checkpoint: checkpoint is older than {} days, discarding and falling back to full index",
            max_age_days
        );
        // Clear the stale checkpoint
        if let Err(e) = store.clear_checkpoint(project) {
            tracing::warn!(
                project = project,
                error = %e,
                "checkpoint: failed to clear stale checkpoint"
            );
        }
        return None;
    }

    // Checkpoint is valid — build ResumeInfo
    tracing::info!(
        project = project,
        phase = %checkpoint.phase,
        phase_index = checkpoint.phase_index,
        files_processed = checkpoint.files_processed,
        started_at = %checkpoint.started_at,
        "checkpoint: valid incomplete checkpoint found, will resume from interrupted phase"
    );

    Some(ResumeInfo {
        project: checkpoint.project,
        interrupted_phase: checkpoint.phase,
        phase_index: checkpoint.phase_index,
        files_processed: checkpoint.files_processed,
        started_at: checkpoint.started_at,
    })
}

/// Validate that a checkpoint's age does not exceed the maximum allowed days.
///
/// Returns `true` if the checkpoint is within the allowed age, `false` if it's too old
/// or if no incomplete checkpoint exists.
///
/// A checkpoint older than `max_age_days` is considered stale and should be discarded
/// because the repository state may have changed significantly since the checkpoint
/// was created.
pub fn is_checkpoint_valid(store: &Store, project: &str, max_age_days: u64) -> bool {
    let checkpoint = match store.get_incomplete_checkpoint(project) {
        Ok(Some(cp)) => cp,
        Ok(None) => return false,
        Err(e) => {
            tracing::warn!(
                project = project,
                error = %e,
                "checkpoint: failed to query checkpoint for age validation"
            );
            return false;
        }
    };

    is_timestamp_within_age(&checkpoint.started_at, max_age_days)
}

/// Clean up partial phase artifacts from an incomplete indexing phase.
///
/// When a phase is interrupted mid-execution, it may leave behind partial data
/// (orphan nodes, incomplete edges). This function removes those artifacts so
/// the phase can be cleanly re-run.
///
/// The cleanup strategy depends on the phase:
/// - `extraction`: Deletes all non-structural nodes (keeps Project, Folder, File) and all edges
/// - `phase2_edges`: Deletes all edges
/// - `phase3_semantic`: Deletes all edges (semantic edges are layered on top of phase 2)
/// - `phase4_infrastructure`: Deletes all edges
/// - `phase5_enrichment`: Deletes all edges
pub fn cleanup_partial_phase(store: &Store, project: &str, phase: &str) {
    tracing::info!(
        project = project,
        phase = phase,
        "checkpoint: cleaning up partial artifacts from interrupted phase"
    );

    let result = match phase {
        phases::EXTRACTION => {
            // Extraction writes nodes — delete all non-structure nodes and edges
            // to allow a clean re-extraction
            let edge_result = store.delete_project_edges(project);
            let node_result = store.conn().execute(
                "DELETE FROM nodes WHERE project = ?1 AND label NOT IN ('Project', 'Folder', 'File')",
                rusqlite::params![project],
            );
            match (edge_result, node_result) {
                (Ok(_), Ok(count)) => {
                    tracing::info!(
                        project = project,
                        phase = phase,
                        nodes_deleted = count,
                        "checkpoint: cleaned up extraction artifacts"
                    );
                    Ok(())
                }
                (Err(e), _) => Err(e),
                (_, Err(e)) => Err(anyhow::anyhow!("failed to delete partial nodes: {}", e)),
            }
        }
        phases::PHASE2_EDGES
        | phases::PHASE3_SEMANTIC
        | phases::PHASE4_INFRASTRUCTURE
        | phases::PHASE5_ENRICHMENT => {
            // These phases write edges — delete all edges to allow clean re-run
            store.delete_project_edges(project)
        }
        unknown => {
            tracing::warn!(
                project = project,
                phase = unknown,
                "checkpoint: unknown phase for cleanup, deleting all edges as fallback"
            );
            store.delete_project_edges(project)
        }
    };

    if let Err(e) = result {
        tracing::warn!(
            project = project,
            phase = phase,
            error = %e,
            "checkpoint: failed to clean up partial phase artifacts"
        );
    }
}

/// Determine the first incomplete phase index, skipping completed phases.
///
/// Queries all checkpoints for the project and returns the phase index of the
/// first phase that is not marked as completed. If all phases are completed,
/// returns `None` (indicating a fresh run should start).
pub fn first_incomplete_phase(store: &Store, project: &str) -> Option<u32> {
    let all_phases = phases::all();

    for (idx, &phase_name) in all_phases.iter().enumerate() {
        let is_complete = is_phase_completed(store, project, phase_name);
        if !is_complete {
            return Some(idx as u32);
        }
    }

    // All phases completed — no resume needed
    None
}

/// Check if a specific phase has been completed for a project.
fn is_phase_completed(store: &Store, project: &str, phase: &str) -> bool {
    let result = store.conn().query_row(
        "SELECT completed FROM _index_progress WHERE project = ?1 AND phase = ?2",
        rusqlite::params![project, phase],
        |row| row.get::<_, i32>(0),
    );

    match result {
        Ok(completed) => completed != 0,
        Err(_) => false, // No record means not completed
    }
}

// --- Internal helpers ---

/// Validate that a checkpoint's data is not corrupted.
///
/// Checks:
/// - Phase name is a known pipeline phase
/// - Timestamp is a valid RFC 3339 date-time string
fn is_checkpoint_data_valid(phase: &str, started_at: &str) -> bool {
    // Validate phase name
    let valid_phases = phases::all();
    if !valid_phases.contains(&phase) {
        return false;
    }

    // Validate timestamp is parseable
    started_at.parse::<DateTime<Utc>>().is_ok() || DateTime::parse_from_rfc3339(started_at).is_ok()
}

/// Check if a timestamp string is within the allowed age in days.
fn is_timestamp_within_age(started_at: &str, max_age_days: u64) -> bool {
    let parsed = match DateTime::parse_from_rfc3339(started_at) {
        Ok(dt) => dt.with_timezone(&Utc),
        Err(_) => {
            // Try parsing as a plain UTC datetime
            match started_at.parse::<DateTime<Utc>>() {
                Ok(dt) => dt,
                Err(_) => return false, // Unparseable timestamp = invalid
            }
        }
    };

    let now = Utc::now();
    let age = now.signed_duration_since(parsed);
    let max_age = chrono::Duration::days(max_age_days as i64);

    age <= max_age
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Duration, Utc};
    use codryn_store::{IndexCheckpoint, Store};

    fn test_store() -> Store {
        Store::open_in_memory().unwrap()
    }

    #[test]
    fn test_should_resume_returns_none_when_no_checkpoint() {
        let store = test_store();
        let result = should_resume(&store, "nonexistent_project");
        assert!(result.is_none());
    }

    #[test]
    fn test_should_resume_returns_info_for_valid_incomplete_checkpoint() {
        let store = test_store();
        let now = Utc::now().to_rfc3339();
        store
            .save_checkpoint(&IndexCheckpoint {
                project: "myproject".into(),
                phase: "phase2_edges".into(),
                phase_index: 1,
                files_processed: 50,
                started_at: now.clone(),
                completed: false,
                run_id: None,
            })
            .unwrap();

        let result = should_resume(&store, "myproject");
        assert!(result.is_some());
        let info = result.unwrap();
        assert_eq!(info.project, "myproject");
        assert_eq!(info.interrupted_phase, "phase2_edges");
        assert_eq!(info.phase_index, 1);
        assert_eq!(info.files_processed, 50);
    }

    #[test]
    fn test_should_resume_returns_none_for_completed_checkpoint() {
        let store = test_store();
        let now = Utc::now().to_rfc3339();
        store
            .save_checkpoint(&IndexCheckpoint {
                project: "myproject".into(),
                phase: "extraction".into(),
                phase_index: 0,
                files_processed: 100,
                started_at: now,
                completed: true,
                run_id: None,
            })
            .unwrap();

        let result = should_resume(&store, "myproject");
        assert!(result.is_none());
    }

    #[test]
    fn test_should_resume_returns_none_for_stale_checkpoint() {
        let store = test_store();
        // Create a checkpoint that is 10 days old
        let old_time = (Utc::now() - Duration::days(10)).to_rfc3339();
        store
            .save_checkpoint(&IndexCheckpoint {
                project: "myproject".into(),
                phase: "phase2_edges".into(),
                phase_index: 1,
                files_processed: 50,
                started_at: old_time,
                completed: false,
                run_id: None,
            })
            .unwrap();

        let result = should_resume(&store, "myproject");
        assert!(result.is_none());
    }

    #[test]
    fn test_should_resume_returns_none_for_corrupted_phase() {
        let store = test_store();
        let now = Utc::now().to_rfc3339();
        // Manually insert a checkpoint with an invalid phase name
        store
            .conn()
            .execute(
                "INSERT OR REPLACE INTO _index_progress (project, phase, phase_index, files_processed, started_at, completed) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                rusqlite::params!["myproject", "invalid_phase_xyz", 99, 10, now, 0],
            )
            .unwrap();

        let result = should_resume(&store, "myproject");
        assert!(result.is_none());
    }

    #[test]
    fn test_should_resume_returns_none_for_corrupted_timestamp() {
        let store = test_store();
        // Manually insert a checkpoint with an invalid timestamp
        store
            .conn()
            .execute(
                "INSERT OR REPLACE INTO _index_progress (project, phase, phase_index, files_processed, started_at, completed) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                rusqlite::params!["myproject", "extraction", 0, 10, "not-a-date", 0],
            )
            .unwrap();

        let result = should_resume(&store, "myproject");
        assert!(result.is_none());
    }

    #[test]
    fn test_is_checkpoint_valid_returns_true_for_recent() {
        let store = test_store();
        let now = Utc::now().to_rfc3339();
        store
            .save_checkpoint(&IndexCheckpoint {
                project: "myproject".into(),
                phase: "extraction".into(),
                phase_index: 0,
                files_processed: 10,
                started_at: now,
                completed: false,
                run_id: None,
            })
            .unwrap();

        assert!(is_checkpoint_valid(&store, "myproject", 7));
    }

    #[test]
    fn test_is_checkpoint_valid_returns_false_for_old() {
        let store = test_store();
        let old_time = (Utc::now() - Duration::days(8)).to_rfc3339();
        store
            .save_checkpoint(&IndexCheckpoint {
                project: "myproject".into(),
                phase: "extraction".into(),
                phase_index: 0,
                files_processed: 10,
                started_at: old_time,
                completed: false,
                run_id: None,
            })
            .unwrap();

        assert!(!is_checkpoint_valid(&store, "myproject", 7));
    }

    #[test]
    fn test_is_checkpoint_valid_returns_false_when_no_checkpoint() {
        let store = test_store();
        assert!(!is_checkpoint_valid(&store, "nonexistent", 7));
    }

    #[test]
    fn test_cleanup_partial_phase_extraction() {
        let store = test_store();
        // Insert a project record first (FK constraint)
        store
            .conn()
            .execute(
                "INSERT INTO projects (name, indexed_at, root_path) VALUES ('proj', '2024-01-01', '/tmp/proj')",
                [],
            )
            .unwrap();
        // Insert some nodes and edges
        store
            .conn()
            .execute(
                "INSERT INTO nodes (project, label, name, qualified_name, file_path, start_line, end_line) \
                 VALUES ('proj', 'Function', 'foo', 'proj::foo', 'src/main.rs', 1, 10)",
                [],
            )
            .unwrap();
        store
            .conn()
            .execute(
                "INSERT INTO nodes (project, label, name, qualified_name, file_path, start_line, end_line) \
                 VALUES ('proj', 'File', 'main.rs', 'proj::main.rs', 'src/main.rs', 0, 0)",
                [],
            )
            .unwrap();

        cleanup_partial_phase(&store, "proj", "extraction");

        // File node should remain, Function node should be deleted
        let count: i64 = store
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM nodes WHERE project = 'proj'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 1); // Only the File node remains

        let label: String = store
            .conn()
            .query_row(
                "SELECT label FROM nodes WHERE project = 'proj'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(label, "File");
    }

    #[test]
    fn test_cleanup_partial_phase_edges() {
        let store = test_store();
        // Insert a project record first (FK constraint)
        store
            .conn()
            .execute(
                "INSERT INTO projects (name, indexed_at, root_path) VALUES ('proj', '2024-01-01', '/tmp/proj')",
                [],
            )
            .unwrap();
        // Insert nodes and an edge
        store
            .conn()
            .execute(
                "INSERT INTO nodes (id, project, label, name, qualified_name, file_path, start_line, end_line) \
                 VALUES (1, 'proj', 'Function', 'foo', 'proj::foo', 'src/main.rs', 1, 10)",
                [],
            )
            .unwrap();
        store
            .conn()
            .execute(
                "INSERT INTO nodes (id, project, label, name, qualified_name, file_path, start_line, end_line) \
                 VALUES (2, 'proj', 'Function', 'bar', 'proj::bar', 'src/main.rs', 11, 20)",
                [],
            )
            .unwrap();
        store
            .conn()
            .execute(
                "INSERT INTO edges (project, source_id, target_id, type) VALUES ('proj', 1, 2, 'CALLS')",
                [],
            )
            .unwrap();

        cleanup_partial_phase(&store, "proj", "phase2_edges");

        // Edges should be deleted, nodes should remain
        let edge_count: i64 = store
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM edges WHERE project = 'proj'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(edge_count, 0);

        let node_count: i64 = store
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM nodes WHERE project = 'proj'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(node_count, 2);
    }

    #[test]
    fn test_first_incomplete_phase_all_completed() {
        let store = test_store();
        let now = Utc::now().to_rfc3339();

        for (idx, &phase) in phases::all().iter().enumerate() {
            store
                .save_checkpoint(&IndexCheckpoint {
                    project: "proj".into(),
                    phase: phase.into(),
                    phase_index: idx as u32,
                    files_processed: 100,
                    started_at: now.clone(),
                    completed: true,
                    run_id: None,
                })
                .unwrap();
        }

        assert_eq!(first_incomplete_phase(&store, "proj"), None);
    }

    #[test]
    fn test_first_incomplete_phase_finds_first_gap() {
        let store = test_store();
        let now = Utc::now().to_rfc3339();

        // Mark extraction as completed
        store
            .save_checkpoint(&IndexCheckpoint {
                project: "proj".into(),
                phase: "extraction".into(),
                phase_index: 0,
                files_processed: 100,
                started_at: now.clone(),
                completed: true,
                run_id: None,
            })
            .unwrap();

        // phase2_edges is not completed (no record)
        let result = first_incomplete_phase(&store, "proj");
        assert_eq!(result, Some(1)); // phase2_edges is index 1
    }

    #[test]
    fn test_is_checkpoint_data_valid_known_phases() {
        let now = Utc::now().to_rfc3339();
        assert!(is_checkpoint_data_valid("extraction", &now));
        assert!(is_checkpoint_data_valid("phase2_edges", &now));
        assert!(is_checkpoint_data_valid("phase3_semantic", &now));
        assert!(is_checkpoint_data_valid("phase4_infrastructure", &now));
        assert!(is_checkpoint_data_valid("phase5_enrichment", &now));
    }

    #[test]
    fn test_is_checkpoint_data_valid_rejects_unknown_phase() {
        let now = Utc::now().to_rfc3339();
        assert!(!is_checkpoint_data_valid("unknown_phase", &now));
        assert!(!is_checkpoint_data_valid("", &now));
    }

    #[test]
    fn test_is_checkpoint_data_valid_rejects_bad_timestamp() {
        assert!(!is_checkpoint_data_valid("extraction", "not-a-date"));
        assert!(!is_checkpoint_data_valid("extraction", ""));
        assert!(!is_checkpoint_data_valid("extraction", "2024-13-45"));
    }

    #[test]
    fn test_is_timestamp_within_age() {
        let now = Utc::now().to_rfc3339();
        assert!(is_timestamp_within_age(&now, 7));

        let three_days_ago = (Utc::now() - Duration::days(3)).to_rfc3339();
        assert!(is_timestamp_within_age(&three_days_ago, 7));

        let eight_days_ago = (Utc::now() - Duration::days(8)).to_rfc3339();
        assert!(!is_timestamp_within_age(&eight_days_ago, 7));
    }

    #[test]
    fn test_should_resume_with_custom_max_age() {
        let store = test_store();
        let three_days_ago = (Utc::now() - Duration::days(3)).to_rfc3339();

        // Test 1: With max age of 5 days, a 3-day-old checkpoint should resume
        store
            .save_checkpoint(&IndexCheckpoint {
                project: "myproject".into(),
                phase: "phase2_edges".into(),
                phase_index: 1,
                files_processed: 50,
                started_at: three_days_ago.clone(),
                completed: false,
                run_id: None,
            })
            .unwrap();
        assert!(should_resume_with_max_age(&store, "myproject", 5).is_some());

        // Test 2: With max age of 2 days, a 3-day-old checkpoint should NOT resume
        // Re-insert the checkpoint since the previous call may have cleared it
        store
            .save_checkpoint(&IndexCheckpoint {
                project: "myproject2".into(),
                phase: "phase2_edges".into(),
                phase_index: 1,
                files_processed: 50,
                started_at: three_days_ago,
                completed: false,
                run_id: None,
            })
            .unwrap();
        assert!(should_resume_with_max_age(&store, "myproject2", 2).is_none());
    }
}
