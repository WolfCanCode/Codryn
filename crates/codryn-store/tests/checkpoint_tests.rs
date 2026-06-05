use codryn_store::{IndexCheckpoint, Store};

fn test_store() -> Store {
    Store::open_in_memory().unwrap()
}

#[test]
fn test_save_checkpoint() {
    let store = test_store();
    let cp = IndexCheckpoint {
        project: "myproject".into(),
        phase: "extraction".into(),
        phase_index: 0,
        files_processed: 42,
        started_at: "2025-01-15T10:00:00Z".into(),
        completed: false,
        run_id: None,
    };
    store.save_checkpoint(&cp).unwrap();

    // Verify we can retrieve it
    let result = store.get_incomplete_checkpoint("myproject").unwrap();
    assert!(result.is_some());
    let retrieved = result.unwrap();
    assert_eq!(retrieved.project, "myproject");
    assert_eq!(retrieved.phase, "extraction");
    assert_eq!(retrieved.phase_index, 0);
    assert_eq!(retrieved.files_processed, 42);
    assert_eq!(retrieved.started_at, "2025-01-15T10:00:00Z");
    assert!(!retrieved.completed);
}

#[test]
fn test_save_checkpoint_upsert() {
    let store = test_store();
    let cp = IndexCheckpoint {
        project: "proj".into(),
        phase: "extraction".into(),
        phase_index: 0,
        files_processed: 10,
        started_at: "2025-01-15T10:00:00Z".into(),
        completed: false,
        run_id: None,
    };
    store.save_checkpoint(&cp).unwrap();

    // Update the same checkpoint with more files processed
    let cp_updated = IndexCheckpoint {
        project: "proj".into(),
        phase: "extraction".into(),
        phase_index: 0,
        files_processed: 50,
        started_at: "2025-01-15T10:00:00Z".into(),
        completed: false,
        run_id: None,
    };
    store.save_checkpoint(&cp_updated).unwrap();

    let result = store.get_incomplete_checkpoint("proj").unwrap().unwrap();
    assert_eq!(result.files_processed, 50);
}

#[test]
fn test_get_incomplete_checkpoint_returns_none_when_all_completed() {
    let store = test_store();
    let cp = IndexCheckpoint {
        project: "proj".into(),
        phase: "extraction".into(),
        phase_index: 0,
        files_processed: 100,
        started_at: "2025-01-15T10:00:00Z".into(),
        completed: true,
        run_id: None,
    };
    store.save_checkpoint(&cp).unwrap();

    let result = store.get_incomplete_checkpoint("proj").unwrap();
    assert!(result.is_none());
}

#[test]
fn test_get_incomplete_checkpoint_returns_highest_phase_index() {
    let store = test_store();

    // Phase 0 completed
    store
        .save_checkpoint(&IndexCheckpoint {
            project: "proj".into(),
            phase: "extraction".into(),
            phase_index: 0,
            files_processed: 100,
            started_at: "2025-01-15T10:00:00Z".into(),
            completed: true,
            run_id: None,
        })
        .unwrap();

    // Phase 1 incomplete
    store
        .save_checkpoint(&IndexCheckpoint {
            project: "proj".into(),
            phase: "phase2_edges".into(),
            phase_index: 1,
            files_processed: 30,
            started_at: "2025-01-15T10:01:00Z".into(),
            completed: false,
            run_id: None,
        })
        .unwrap();

    // Phase 2 incomplete (higher index)
    store
        .save_checkpoint(&IndexCheckpoint {
            project: "proj".into(),
            phase: "phase3_semantic".into(),
            phase_index: 2,
            files_processed: 5,
            started_at: "2025-01-15T10:02:00Z".into(),
            completed: false,
            run_id: None,
        })
        .unwrap();

    let result = store.get_incomplete_checkpoint("proj").unwrap().unwrap();
    assert_eq!(result.phase, "phase3_semantic");
    assert_eq!(result.phase_index, 2);
}

#[test]
fn test_get_incomplete_checkpoint_no_data() {
    let store = test_store();
    let result = store.get_incomplete_checkpoint("nonexistent").unwrap();
    assert!(result.is_none());
}

#[test]
fn test_clear_checkpoint() {
    let store = test_store();

    // Save multiple checkpoints for the same project
    store
        .save_checkpoint(&IndexCheckpoint {
            project: "proj".into(),
            phase: "extraction".into(),
            phase_index: 0,
            files_processed: 100,
            started_at: "2025-01-15T10:00:00Z".into(),
            completed: true,
            run_id: None,
        })
        .unwrap();
    store
        .save_checkpoint(&IndexCheckpoint {
            project: "proj".into(),
            phase: "phase2_edges".into(),
            phase_index: 1,
            files_processed: 50,
            started_at: "2025-01-15T10:01:00Z".into(),
            completed: false,
            run_id: None,
        })
        .unwrap();

    // Clear all checkpoints for the project
    store.clear_checkpoint("proj").unwrap();

    // Verify nothing remains
    let result = store.get_incomplete_checkpoint("proj").unwrap();
    assert!(result.is_none());
}

#[test]
fn test_clear_checkpoint_does_not_affect_other_projects() {
    let store = test_store();

    store
        .save_checkpoint(&IndexCheckpoint {
            project: "proj_a".into(),
            phase: "extraction".into(),
            phase_index: 0,
            files_processed: 10,
            started_at: "2025-01-15T10:00:00Z".into(),
            completed: false,
            run_id: None,
        })
        .unwrap();
    store
        .save_checkpoint(&IndexCheckpoint {
            project: "proj_b".into(),
            phase: "extraction".into(),
            phase_index: 0,
            files_processed: 20,
            started_at: "2025-01-15T10:00:00Z".into(),
            completed: false,
            run_id: None,
        })
        .unwrap();

    // Clear only proj_a
    store.clear_checkpoint("proj_a").unwrap();

    // proj_a should be gone
    assert!(store.get_incomplete_checkpoint("proj_a").unwrap().is_none());
    // proj_b should still exist
    let result = store.get_incomplete_checkpoint("proj_b").unwrap().unwrap();
    assert_eq!(result.project, "proj_b");
    assert_eq!(result.files_processed, 20);
}

#[test]
fn test_clear_checkpoint_on_empty_is_noop() {
    let store = test_store();
    // Should not error when there's nothing to clear
    store.clear_checkpoint("nonexistent").unwrap();
}

#[test]
fn test_checkpoint_stores_and_retrieves_run_id() {
    let store = test_store();
    let cp = IndexCheckpoint {
        project: "proj".into(),
        phase: "extraction".into(),
        phase_index: 0,
        files_processed: 10,
        started_at: "2025-01-15T10:00:00Z".into(),
        completed: false,
        run_id: Some("proj-1234567890-0".into()),
    };
    store.save_checkpoint(&cp).unwrap();

    let result = store.get_incomplete_checkpoint("proj").unwrap().unwrap();
    assert_eq!(result.run_id, Some("proj-1234567890-0".into()));
}

#[test]
fn test_checkpoint_run_id_none_is_stored_as_null() {
    let store = test_store();
    let cp = IndexCheckpoint {
        project: "proj".into(),
        phase: "extraction".into(),
        phase_index: 0,
        files_processed: 10,
        started_at: "2025-01-15T10:00:00Z".into(),
        completed: false,
        run_id: None,
    };
    store.save_checkpoint(&cp).unwrap();

    let result = store.get_incomplete_checkpoint("proj").unwrap().unwrap();
    assert_eq!(result.run_id, None);
}
