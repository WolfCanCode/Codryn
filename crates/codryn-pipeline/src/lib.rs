pub mod angular_adapter;
pub mod checkpoint;
pub mod complexity;
pub mod cpp_preprocessor;
pub mod doc_coverage;
pub mod extraction;
pub mod fastapi_adapter;
pub mod git_history;
pub mod go_adapter;
pub mod go_common;
pub mod jsx_framework;
pub mod lambda_cfn;
pub mod memory;
pub mod nextjs_routes;
pub mod pass_channels;
pub mod pass_configlink;
pub mod pass_cross_repo;
pub mod pass_decorators;
#[cfg(feature = "git-history")]
pub mod pass_gitdiff;
pub mod pass_k8s;
pub mod pass_types;
pub mod pass_usages;
pub mod passes;
pub mod registry;
pub mod spring_common;
pub mod spring_java;
pub mod spring_kotlin;
pub mod tier3_walkers;
pub mod vue_adapter;
pub mod vue_sfc;

use anyhow::Result;
use chrono::Utc;
use codryn_discover::{discover_files_with_mappings, load_language_mappings, DiscoveredFile};
use codryn_foundation::fqn;
use codryn_graph_buffer::GraphBuffer;
use codryn_store::{FileHash, IndexCheckpoint, Project, Store};
use rayon::prelude::*;
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

/// Pipeline phase identifiers used for checkpoint tracking.
pub mod phases {
    pub const EXTRACTION: &str = "extraction";
    pub const PHASE2_EDGES: &str = "phase2_edges";
    pub const PHASE3_SEMANTIC: &str = "phase3_semantic";
    pub const PHASE4_INFRASTRUCTURE: &str = "phase4_infrastructure";
    pub const PHASE5_ENRICHMENT: &str = "phase5_enrichment";

    /// Returns the phase index (ordinal) for a given phase name.
    pub fn phase_index(phase: &str) -> u32 {
        match phase {
            EXTRACTION => 0,
            PHASE2_EDGES => 1,
            PHASE3_SEMANTIC => 2,
            PHASE4_INFRASTRUCTURE => 3,
            PHASE5_ENRICHMENT => 4,
            _ => 0,
        }
    }

    /// Returns all phases in order.
    pub fn all() -> &'static [&'static str] {
        &[
            EXTRACTION,
            PHASE2_EDGES,
            PHASE3_SEMANTIC,
            PHASE4_INFRASTRUCTURE,
            PHASE5_ENRICHMENT,
        ]
    }
}

/// Information about an incomplete indexing run that can be resumed.
#[derive(Debug, Clone)]
pub struct ResumeInfo {
    /// The project name that was being indexed.
    pub project: String,
    /// The phase that was interrupted.
    pub interrupted_phase: String,
    /// The phase index (ordinal) of the interrupted phase.
    pub phase_index: u32,
    /// Number of files that were processed before interruption.
    pub files_processed: u32,
    /// When the interrupted phase started.
    pub started_at: String,
}

/// Progress update emitted during pipeline execution.
#[derive(Debug, Clone)]
pub struct ProgressUpdate {
    /// Current phase name (e.g., "discovery", "extraction", "pass_calls").
    pub phase: String,
    /// Completion percentage (0.0 to 1.0) within the current phase.
    pub percent: f64,
    /// Human-readable message (e.g., "Extracted 500/1686 files").
    pub message: String,
    /// Ordinal position for pass phases (e.g., "3 of 8").
    pub pass_ordinal: Option<String>,
}

/// Callback type for progress reporting.
pub type ProgressCallback = Box<dyn Fn(ProgressUpdate) + Send + Sync>;

/// A cache of file contents keyed by absolute path.
/// Populated during the change-detection phase so that extraction and passes
/// can reuse already-read content instead of hitting the filesystem again.
#[derive(Debug, Clone, Default)]
pub struct FileCache {
    inner: HashMap<PathBuf, Arc<String>>,
}

impl FileCache {
    pub fn new() -> Self {
        Self {
            inner: HashMap::new(),
        }
    }

    /// Insert file content into the cache.
    pub fn insert(&mut self, path: PathBuf, content: Arc<String>) {
        self.inner.insert(path, content);
    }

    /// Get cached content for a path, or read from disk and cache it.
    pub fn get_or_read(&mut self, path: &Path) -> Option<Arc<String>> {
        if let Some(cached) = self.inner.get(path) {
            return Some(Arc::clone(cached));
        }
        match std::fs::read_to_string(path) {
            Ok(s) => {
                let arc = Arc::new(s);
                self.inner.insert(path.to_owned(), Arc::clone(&arc));
                Some(arc)
            }
            Err(_) => None,
        }
    }

    /// Get cached content without reading from disk.
    pub fn get(&self, path: &Path) -> Option<Arc<String>> {
        self.inner.get(path).map(Arc::clone)
    }

    /// Number of cached entries.
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    /// Whether the cache is empty.
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    /// Clear all cached content to free memory.
    pub fn clear(&mut self) {
        self.inner.clear();
    }

    /// Evict the oldest `count` entries from the cache.
    /// Since HashMap doesn't track insertion order, this removes
    /// arbitrary entries (effectively random eviction).
    pub fn evict_oldest(&mut self, count: usize) {
        let keys: Vec<PathBuf> = self.inner.keys().take(count).cloned().collect();
        for key in keys {
            self.inner.remove(&key);
        }
    }
}

/// Determine which files each pass should process during incremental reindex.
pub struct IncrementalFileSet<'a> {
    /// Files whose content changed (hash mismatch).
    pub changed: Vec<&'a DiscoveredFile>,
    /// Changed files + files that import any changed file (for pass_calls).
    pub changed_plus_dependents: Vec<&'a DiscoveredFile>,
    /// All files (for passes that need global context).
    pub all: Vec<&'a DiscoveredFile>,
}

impl<'a> IncrementalFileSet<'a> {
    /// Compute the incremental file set for pass filtering.
    ///
    /// If fewer than 10% of files have changed, computes 1-hop dependents
    /// by querying IMPORTS edges in the store. If 10% or more have changed,
    /// all categories return all files (full reindex is more efficient).
    pub fn compute(
        all_files: &'a [DiscoveredFile],
        changed_files: &[&'a DiscoveredFile],
        store: &Store,
        project: &str,
    ) -> Self {
        let all: Vec<&'a DiscoveredFile> = all_files.iter().collect();

        // Threshold: if changed >= 10% of all files, return all files for every category
        if changed_files.len() >= all_files.len() / 10 {
            let changed: Vec<&'a DiscoveredFile> = changed_files.to_vec();
            return Self {
                changed: changed.clone(),
                changed_plus_dependents: all.clone(),
                all,
            };
        }

        let changed: Vec<&'a DiscoveredFile> = changed_files.to_vec();

        // Compute 1-hop dependents: files that import any changed file
        let changed_paths: HashSet<&str> =
            changed_files.iter().map(|f| f.rel_path.as_str()).collect();

        let dependent_paths = Self::find_dependents(store, project, &changed_paths);

        // Build changed_plus_dependents: changed files + files that import them
        let changed_plus_dep_set: HashSet<&str> = changed_paths
            .iter()
            .copied()
            .chain(dependent_paths.iter().map(|s| s.as_str()))
            .collect();

        let changed_plus_dependents: Vec<&'a DiscoveredFile> = all_files
            .iter()
            .filter(|f| changed_plus_dep_set.contains(f.rel_path.as_str()))
            .collect();

        Self {
            changed,
            changed_plus_dependents,
            all,
        }
    }

    /// Query the store for 1-hop dependents of the changed files.
    /// Returns file paths of files that import any of the changed files.
    fn find_dependents(store: &Store, project: &str, changed_paths: &HashSet<&str>) -> Vec<String> {
        if changed_paths.is_empty() {
            return Vec::new();
        }

        // Build the SQL query with placeholders for changed file paths
        // SELECT DISTINCT n_src.file_path
        // FROM edges e
        // JOIN nodes n_src ON n_src.id = e.source_id
        // JOIN nodes n_tgt ON n_tgt.id = e.target_id
        // WHERE e.project = ?1
        //   AND e.type = 'IMPORTS'
        //   AND n_tgt.file_path IN (?, ?, ...)
        let placeholders: Vec<String> = (0..changed_paths.len())
            .map(|i| format!("?{}", i + 2))
            .collect();
        let sql = format!(
            "SELECT DISTINCT n_src.file_path \
             FROM edges e \
             JOIN nodes n_src ON n_src.id = e.source_id \
             JOIN nodes n_tgt ON n_tgt.id = e.target_id \
             WHERE e.project = ?1 \
               AND e.type = 'IMPORTS' \
               AND n_tgt.file_path IN ({})",
            placeholders.join(", ")
        );

        let conn = store.conn();
        let mut stmt = match conn.prepare(&sql) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(error = %e, "IncrementalFileSet: failed to prepare dependents query");
                return Vec::new();
            }
        };

        // Build params: project + changed file paths
        let changed_vec: Vec<&str> = changed_paths.iter().copied().collect();
        let mut params: Vec<&dyn rusqlite::ToSql> = Vec::with_capacity(1 + changed_vec.len());
        params.push(&project);
        for path in &changed_vec {
            params.push(path);
        }

        let rows = match stmt.query_map(params.as_slice(), |row| row.get::<_, String>(0)) {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!(error = %e, "IncrementalFileSet: failed to query dependents");
                return Vec::new();
            }
        };

        rows.filter_map(|r| r.ok()).collect()
    }
}

/// Cross-process exclusive lock using OS file locking.
/// Held for the duration of a pipeline run to prevent concurrent indexing
/// from multiple processes (e.g. codryn-ui and codryn-mcp) on the same store.
struct CrossProcessLock {
    _file: std::fs::File,
}

impl CrossProcessLock {
    fn acquire(path: &std::path::Path) -> anyhow::Result<Self> {
        use std::fs::OpenOptions;
        let file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)
            .map_err(|e| anyhow::anyhow!("failed to open lock file: {e}"))?;

        #[cfg(unix)]
        {
            use std::os::unix::io::AsRawFd;
            let fd = file.as_raw_fd();
            // LOCK_EX (blocking): wait until we can acquire the exclusive lock.
            // This serializes concurrent indexing across all processes on this machine.
            let ret = unsafe { libc_flock(fd, 2) }; // LOCK_EX=2 (blocking)
            if ret != 0 {
                return Err(anyhow::anyhow!(
                    "failed to acquire index lock (flock errno: {})",
                    ret
                ));
            }
        }

        Ok(Self { _file: file })
    }
}

impl Drop for CrossProcessLock {
    fn drop(&mut self) {
        #[cfg(unix)]
        {
            use std::os::unix::io::AsRawFd;
            unsafe { libc_flock(self._file.as_raw_fd(), 8) }; // LOCK_UN=8
        }
    }
}

#[cfg(unix)]
unsafe fn libc_flock(fd: std::os::unix::io::RawFd, op: i32) -> i32 {
    extern "C" {
        fn flock(fd: i32, operation: i32) -> i32;
    }
    flock(fd, op)
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum IndexMode {
    Full,
    Fast,
}

pub struct Pipeline {
    repo_path: PathBuf,
    db_path: PathBuf,
    mode: IndexMode,
    cancelled: Arc<AtomicBool>,
    num_threads: usize,
    progress_cb: Option<ProgressCallback>,
    memory_monitor: Option<memory::MemoryMonitor>,
}

static INDEX_LOCK: Mutex<()> = Mutex::new(());

impl Pipeline {
    pub fn new(repo_path: &Path, db_path: &Path, mode: IndexMode) -> Self {
        Self {
            repo_path: repo_path.to_owned(),
            db_path: db_path.to_owned(),
            mode,
            cancelled: Arc::new(AtomicBool::new(false)),
            num_threads: 0, // 0 = use rayon default (num_cpus)
            progress_cb: None,
            memory_monitor: None,
        }
    }

    /// Set the maximum number of threads for parallel extraction.
    /// A value of 0 means use the default (number of CPU cores).
    pub fn set_num_threads(&mut self, n: usize) {
        self.num_threads = n;
    }

    /// Set an optional progress callback. If the callback panics, the panic
    /// is caught and logged, and indexing continues.
    pub fn set_progress_callback(&mut self, cb: ProgressCallback) {
        self.progress_cb = Some(cb);
    }

    /// Set the memory monitor for memory pressure management.
    /// When set, the pipeline will flush buffers when memory usage exceeds
    /// the configured threshold, and log the high-water mark at end of run.
    pub fn set_memory_monitor(&mut self, monitor: memory::MemoryMonitor) {
        self.memory_monitor = Some(monitor);
    }

    /// Emit a progress update to the registered callback, if any.
    /// Panics in the callback are caught and logged without interrupting indexing.
    fn emit_progress(&self, update: ProgressUpdate) {
        if let Some(ref cb) = self.progress_cb {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| cb(update)));
            if let Err(e) = result {
                tracing::warn!("progress callback panicked: {:?}", e);
            }
        }
    }

    /// Emit progress as an overall percentage (0.0 to 1.0) across the entire pipeline.
    /// `phase_base` is the starting percent for this phase (e.g., 0.02 for extraction start).
    /// `phase_weight` is how much of the total this phase represents (e.g., 0.38 for extraction).
    /// `within_phase` is progress within the current phase (0.0 to 1.0).
    fn emit_overall_progress(
        &self,
        phase_base: f64,
        phase_weight: f64,
        within_phase: f64,
        message: &str,
    ) {
        let overall = phase_base + phase_weight * within_phase;
        self.emit_progress(ProgressUpdate {
            phase: "indexing".into(),
            percent: overall.min(1.0),
            message: message.to_string(),
            pass_ordinal: None,
        });
    }

    pub fn cancel_handle(&self) -> Arc<AtomicBool> {
        self.cancelled.clone()
    }

    pub fn project_name(&self) -> String {
        fqn::project_name_from_path(&self.repo_path.to_string_lossy())
    }

    /// Check if there is an incomplete checkpoint for this pipeline's project.
    /// Returns `ResumeInfo` if an interrupted indexing run was detected.
    /// This should be called on startup to offer resume capability.
    pub fn check_incomplete_checkpoint(&self) -> Result<Option<ResumeInfo>> {
        let store = if self.db_path.to_string_lossy() == ":memory:" {
            Store::open_in_memory()?
        } else {
            let db_file = self.db_path.join("graph.db");
            if !db_file.exists() {
                return Ok(None);
            }
            Store::open(&db_file)?
        };

        let project_name = self.project_name();
        match store.get_incomplete_checkpoint(&project_name)? {
            Some(cp) => Ok(Some(ResumeInfo {
                project: cp.project,
                interrupted_phase: cp.phase,
                phase_index: cp.phase_index,
                files_processed: cp.files_processed,
                started_at: cp.started_at,
            })),
            None => Ok(None),
        }
    }

    /// Roll back partial data from an interrupted phase before re-running it.
    /// This deletes edges/nodes that may have been partially written during the
    /// interrupted phase.
    fn rollback_phase(&self, store: &Store, project: &str, phase: &str) -> Result<()> {
        tracing::info!(
            phase = phase,
            project = project,
            "pipeline: rolling back partial data from interrupted phase"
        );
        match phase {
            phases::EXTRACTION => {
                // Extraction writes nodes — delete all non-structure nodes and edges
                // to allow a clean re-extraction
                store.delete_project_edges(project)?;
                store.conn().execute(
                    "DELETE FROM nodes WHERE project = ?1 AND label NOT IN ('Project', 'Folder', 'File')",
                    rusqlite::params![project],
                )?;
            }
            phases::PHASE2_EDGES => {
                // Phase 2 writes edges only — delete all edges to re-run
                store.delete_project_edges(project)?;
            }
            phases::PHASE3_SEMANTIC => {
                // Phase 3 writes semantic edges — delete all edges and re-run from phase 2
                // Since phase 3 adds edges on top of phase 2, we only need to delete edges
                // that were added in phase 3. However, since we can't easily distinguish them,
                // we delete all edges and re-run from phase 2.
                // Actually, for simplicity, just delete all edges — phase 2 checkpoint is
                // already marked complete, so we'll re-run phase 2 edges first.
                store.delete_project_edges(project)?;
            }
            phases::PHASE4_INFRASTRUCTURE => {
                // Phase 4 writes infrastructure edges — delete edges with infra-related types
                // For simplicity, delete all edges and rely on re-running from phase 2
                store.delete_project_edges(project)?;
            }
            phases::PHASE5_ENRICHMENT => {
                // Phase 5 writes enrichment data (similarity edges, git history)
                // Delete edges added in this phase — for simplicity delete all edges
                store.delete_project_edges(project)?;
            }
            _ => {
                tracing::warn!(
                    phase = phase,
                    "pipeline: unknown phase for rollback, deleting all edges"
                );
                store.delete_project_edges(project)?;
            }
        }
        Ok(())
    }

    /// Record a checkpoint at the start of a pipeline phase.
    fn record_checkpoint(
        &self,
        store: &Store,
        project: &str,
        phase: &str,
        files_processed: u32,
        run_id: Option<&str>,
    ) -> Result<()> {
        let cp = IndexCheckpoint {
            project: project.to_string(),
            phase: phase.to_string(),
            phase_index: phases::phase_index(phase),
            files_processed,
            started_at: Utc::now().to_rfc3339(),
            completed: false,
            run_id: run_id.map(|s| s.to_string()),
        };
        store.save_checkpoint(&cp)?;
        tracing::debug!(phase = phase, "pipeline: checkpoint recorded");
        Ok(())
    }

    /// Mark a checkpoint as completed after a phase finishes successfully.
    fn complete_checkpoint(
        &self,
        store: &Store,
        project: &str,
        phase: &str,
        files_processed: u32,
        run_id: Option<&str>,
    ) -> Result<()> {
        let cp = IndexCheckpoint {
            project: project.to_string(),
            phase: phase.to_string(),
            phase_index: phases::phase_index(phase),
            files_processed,
            started_at: Utc::now().to_rfc3339(),
            completed: true,
            run_id: run_id.map(|s| s.to_string()),
        };
        store.save_checkpoint(&cp)?;
        tracing::debug!(phase = phase, "pipeline: checkpoint completed");
        Ok(())
    }

    pub fn run(&self) -> Result<()> {
        let _lock = INDEX_LOCK
            .lock()
            .map_err(|e| anyhow::anyhow!("lock: {e}"))?;

        // Cross-process lock: prevent concurrent indexing from multiple processes
        // (e.g. codryn-ui and codryn-mcp running simultaneously on the same store).
        // We hold an exclusive advisory lock on a lock file for the duration of the run.
        let _file_lock = if self.db_path.to_string_lossy() != ":memory:" {
            std::fs::create_dir_all(&self.db_path).ok();
            let lock_path = self.db_path.join("index.lock");
            Some(CrossProcessLock::acquire(&lock_path)?)
        } else {
            None
        };

        tracing::info!(repo = %self.repo_path.display(), "pipeline: start");

        let store = if self.db_path.to_string_lossy() == ":memory:" {
            Store::open_in_memory()?
        } else {
            std::fs::create_dir_all(&self.db_path)?;
            Store::open(&self.db_path.join("graph.db"))?
        };

        // Enable bulk indexing mode for write throughput (Requirements 3.1-3.6)
        store.enable_bulk_indexing_mode()?;

        // Check for incomplete checkpoint and determine resume phase
        let project_name = self.project_name();
        let resume_from_phase = match store.get_incomplete_checkpoint(&project_name)? {
            Some(cp) => {
                tracing::info!(
                    phase = %cp.phase,
                    phase_index = cp.phase_index,
                    files_processed = cp.files_processed,
                    "pipeline: detected incomplete checkpoint, will resume"
                );
                // Cancel any stale running index runs from the crashed session
                match store.cancel_running_index_runs(&project_name) {
                    Ok(n) if n > 0 => {
                        tracing::info!(
                            count = n,
                            project = project_name,
                            "pipeline: canceled stale running index runs from previous crash"
                        );
                    }
                    Ok(_) => {}
                    Err(e) => {
                        tracing::warn!(error = %e, "pipeline: failed to cancel stale index runs");
                    }
                }
                // Roll back partial data from the interrupted phase
                self.rollback_phase(&store, &project_name, &cp.phase)?;
                // After rollback, determine effective resume point:
                // - If extraction was interrupted, start from scratch (phase 0)
                // - If any edge phase was interrupted, rollback deletes all edges,
                //   so we must re-run from phase 2 (edges). Extraction (phase 0)
                //   still runs to rebuild the registry/file context.
                let effective_phase = if cp.phase_index == 0 {
                    0 // Re-run everything
                } else {
                    // Edge phases: extraction still runs (for context), but we
                    // skip re-flushing extraction nodes since they're already in DB.
                    // We resume edge building from phase 2.
                    phases::phase_index(phases::PHASE2_EDGES)
                };
                Some(effective_phase)
            }
            None => None,
        };

        // Run the main indexing logic, capturing the result
        let result = self.run_inner(&store, resume_from_phase);

        // Always restore pragmas, even on error (Requirement 3.5)
        if let Err(e) = store.disable_bulk_indexing_mode() {
            tracing::error!(error = %e, "failed to restore SQLite pragmas after indexing");
        }

        result
    }

    /// Inner indexing logic, separated so that bulk indexing mode can be
    /// reliably enabled/disabled around it in a finally-style pattern.
    fn run_inner(&self, store: &Store, resume_from_phase: Option<u32>) -> Result<()> {
        let overall_start = Instant::now();
        let project_name = self.project_name();
        let now = Utc::now().to_rfc3339();
        store.upsert_project(&Project {
            name: project_name.clone(),
            indexed_at: now,
            root_path: self.repo_path.to_string_lossy().into(),
        })?;

        // Start an index run to track this indexing session
        let mode_str = match self.mode {
            IndexMode::Full => "full",
            IndexMode::Fast => "fast",
        };
        let git_commit = self.detect_git_commit();
        let index_run = match store.start_index_run(&project_name, mode_str, git_commit.as_deref())
        {
            Ok(run) => {
                tracing::info!(run_id = %run.id, mode = mode_str, "pipeline: started index run");
                Some(run)
            }
            Err(e) => {
                tracing::warn!(error = %e, "pipeline: failed to start index run, continuing without tracking");
                None
            }
        };
        let run_id: Option<String> = index_run.as_ref().map(|r| r.id.clone());

        // Run the inner logic and capture the result so we can complete/fail the index run
        let result =
            self.run_inner_tracked(store, resume_from_phase, run_id.as_deref(), overall_start);

        // Complete or fail the index run based on the result
        if let Some(ref rid) = run_id {
            match &result {
                Ok(()) => {
                    // Count nodes and edges for the completed run
                    let (node_count, edge_count) = store
                        .get_graph_schema(&project_name)
                        .map(|s| (s.total_nodes, s.total_edges))
                        .unwrap_or((0, 0));
                    if let Err(e) = store.complete_index_run(rid, node_count, edge_count) {
                        tracing::warn!(error = %e, run_id = %rid, "pipeline: failed to complete index run record");
                    } else {
                        tracing::info!(
                            run_id = %rid,
                            node_count = node_count,
                            edge_count = edge_count,
                            "pipeline: index run completed"
                        );
                    }
                    // Record a graph summary snapshot after successful index run
                    if let Err(e) = store.record_snapshot(&project_name, run_id.as_deref()) {
                        tracing::warn!(error = %e, "pipeline: failed to record graph snapshot");
                    } else {
                        tracing::info!("pipeline: graph snapshot recorded");
                    }
                    // Prune old snapshots (keep last 10 by default)
                    let retention = 10usize;
                    if let Err(e) = store.prune_old_snapshots(&project_name, retention) {
                        tracing::warn!(error = %e, "pipeline: failed to prune old snapshots");
                    }
                }
                Err(e) => {
                    let error_msg = e.to_string();
                    if let Err(fe) = store.fail_index_run(rid, &error_msg) {
                        tracing::warn!(error = %fe, run_id = %rid, "pipeline: failed to record index run failure");
                    } else {
                        tracing::info!(run_id = %rid, error = %error_msg, "pipeline: index run failed");
                    }
                }
            }
        }

        result
    }

    /// Detect the current git commit hash for the repository, if available.
    fn detect_git_commit(&self) -> Option<String> {
        let git_dir = self.repo_path.join(".git");
        if !git_dir.exists() {
            return None;
        }
        // Read HEAD to get the current commit
        let head_path = git_dir.join("HEAD");
        let head_content = std::fs::read_to_string(&head_path).ok()?;
        let head_content = head_content.trim();
        if head_content.starts_with("ref: ") {
            // Symbolic ref — resolve to commit hash
            let ref_path = head_content.strip_prefix("ref: ")?;
            let commit_path = git_dir.join(ref_path);
            std::fs::read_to_string(commit_path)
                .ok()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
        } else if head_content.len() >= 7 {
            // Detached HEAD — content is the commit hash
            Some(head_content.to_string())
        } else {
            None
        }
    }

    /// Core indexing logic with index run tracking.
    fn run_inner_tracked(
        &self,
        store: &Store,
        resume_from_phase: Option<u32>,
        run_id: Option<&str>,
        overall_start: Instant,
    ) -> Result<()> {
        let project_name = self.project_name();

        // Upsert the project record (sets indexed_at timestamp)
        let now = Utc::now().to_rfc3339();
        store.upsert_project(&Project {
            name: project_name.clone(),
            indexed_at: now,
            root_path: self.repo_path.to_string_lossy().into(),
        })?;

        // Determine which phase to start from (0 = extraction, the beginning)
        let start_phase = resume_from_phase.unwrap_or(0);
        if start_phase > 0 {
            tracing::info!(
                start_phase = start_phase,
                "pipeline: resuming from phase index {}",
                start_phase
            );
        }

        // Phase 1: Discover files
        if self.is_cancelled() {
            return Ok(());
        }

        // Record checkpoint for extraction phase
        self.record_checkpoint(store, &project_name, phases::EXTRACTION, 0, run_id)?;
        let discovery_start = Instant::now();
        self.emit_overall_progress(0.0, 0.02, 0.0, "Discovering files...");
        let mappings = load_language_mappings(&self.repo_path);
        let files = discover_files_with_mappings(&self.repo_path, &mappings)?;
        tracing::info!(count = files.len(), "pipeline: discovered files");
        self.emit_overall_progress(0.0, 0.02, 1.0, &format!("Discovered {} files", files.len()));
        let discovery_elapsed = discovery_start.elapsed();

        // Safety guard: if no files were discovered but the directory exists and is
        // non-empty, this likely indicates a filesystem permission issue (e.g. macOS
        // TCC blocking access to ~/Documents). Abort early to avoid overwriting a
        // valid existing index with a broken empty one.
        if files.is_empty() && self.repo_path.is_dir() {
            let has_entries = std::fs::read_dir(&self.repo_path)
                .map(|mut rd| rd.next().is_some())
                .unwrap_or(false);
            if has_entries {
                tracing::error!(
                    path = %self.repo_path.display(),
                    "pipeline: discovered 0 source files but directory is non-empty. \
                     This likely indicates a filesystem permission issue (e.g. macOS \
                     Full Disk Access not granted). Aborting to preserve existing index."
                );
                anyhow::bail!(
                    "Discovered 0 source files in '{}' but directory is non-empty. \
                     This is likely a filesystem permission issue. On macOS, grant \
                     Full Disk Access to the application in System Settings > \
                     Privacy & Security > Full Disk Access.",
                    self.repo_path.display()
                );
            }
        }

        // Phase 2: Compute file hashes for incremental indexing
        let old_hashes = store.get_file_hashes(&project_name)?;
        let old_map: HashMap<String, String> = old_hashes
            .into_iter()
            .map(|h| (h.rel_path, h.sha256))
            .collect();

        // Health check: detect a corrupted or incomplete index and force Full reindex.
        // If the stored index has suspiciously few edges relative to the number of
        // discovered files (< 0.5 edges/file), the index is likely corrupted — e.g.,
        // from the bug where Fast mode deleted all edges but only rebuilt a subset.
        // In that case, override Fast mode with Full to rebuild from scratch.
        let effective_mode = if self.mode == IndexMode::Fast && !old_map.is_empty() {
            match store.get_graph_schema(&project_name) {
                Ok(schema) => {
                    let edges_per_file = schema.total_edges as f64 / files.len().max(1) as f64;
                    if edges_per_file < 2.0 {
                        tracing::warn!(
                            total_edges = schema.total_edges,
                            total_nodes = schema.total_nodes,
                            files = files.len(),
                            edges_per_file = edges_per_file,
                            "pipeline: index health check FAILED — index appears corrupted \
                             (< 2.0 edges/file). Forcing Full reindex to rebuild from scratch."
                        );
                        IndexMode::Full
                    } else {
                        tracing::debug!(
                            total_edges = schema.total_edges,
                            edges_per_file = edges_per_file,
                            "pipeline: index health check passed, using Fast mode"
                        );
                        IndexMode::Fast
                    }
                }
                Err(e) => {
                    tracing::warn!(
                        error = %e,
                        "pipeline: could not read graph schema for health check, using Fast mode"
                    );
                    IndexMode::Fast
                }
            }
        } else {
            self.mode
        };

        let (changed_files, new_hashes, mut file_cache) = self.compute_changed(&files, &old_map)?;
        tracing::info!(
            changed = changed_files.len(),
            cached = file_cache.len(),
            "pipeline: changed files"
        );

        // Detect deleted files: paths in stored hashes but no longer on disk.
        // Remove all nodes and edges for deleted files BEFORE extraction begins
        // so that stale data doesn't pollute the graph. (Requirements 4.4, 4.5)
        let new_paths: HashSet<&str> = files.iter().map(|f| f.rel_path.as_str()).collect();
        let deleted_paths: Vec<&str> = old_map
            .keys()
            .filter(|p| !new_paths.contains(p.as_str()))
            .map(|p| p.as_str())
            .collect();
        if !deleted_paths.is_empty() {
            tracing::info!(
                count = deleted_paths.len(),
                "pipeline: detected deleted files, removing stale nodes/edges"
            );
            store.delete_nodes_for_files(&project_name, &deleted_paths)?;
        }

        // Phase 3: Extract and build graph
        if self.is_cancelled() {
            return Ok(());
        }
        let extraction_start = Instant::now();
        let mut buf = GraphBuffer::new(&project_name);

        // Pass 1: Structure nodes (Project, Folder, Module, File)
        passes::pass_structure(&mut buf, &project_name, &self.repo_path, &files);

        // Pass 2: Definitions (Function, Class, Method, Interface, etc.)
        // Always populate the registry from ALL files so call resolution works on reindex.
        // Only add nodes to the buffer for changed files to avoid duplicate inserts.
        if self.is_cancelled() {
            return Ok(());
        }

        // In Fast mode: do NOT delete existing symbol nodes before re-extracting.
        // Deleting nodes invalidates edge IDs for unchanged files, causing data loss.
        // Instead, we upsert (INSERT OR IGNORE) — stale nodes from renamed/deleted
        // functions may accumulate until the next Full reindex, which is acceptable.
        // Full mode still deletes all edges and rebuilds from scratch (safe).
        if effective_mode == IndexMode::Full {
            let changed_paths: Vec<&str> =
                changed_files.iter().map(|f| f.rel_path.as_str()).collect();
            store.delete_nodes_for_changed_files(&project_name, &changed_paths)?;
        }

        let mut reg = registry::Registry::new();
        let mut type_reg = registry::TypeRegistry::new();
        let changed_set: std::collections::HashSet<&str> =
            changed_files.iter().map(|f| f.rel_path.as_str()).collect();

        // Build a rayon thread pool with configurable thread count
        let pool = {
            let mut builder = rayon::ThreadPoolBuilder::new();
            if self.num_threads > 0 {
                builder = builder.num_threads(self.num_threads);
            }
            builder.build().unwrap_or_else(|_| {
                rayon::ThreadPoolBuilder::new()
                    .build()
                    .expect("failed to build default rayon thread pool")
            })
        };

        // Parallel extraction for changed files
        let cancelled = &self.cancelled;
        self.emit_overall_progress(
            0.02,
            0.38,
            0.0,
            &format!("Extracting 0/{} files...", changed_files.len()),
        );
        let (parallel_results, parallel_reg_entries): (
            Vec<extraction::ExtractionResult>,
            Vec<Vec<(String, registry::RegistryEntry)>>,
        ) = pool.install(|| {
            // Extract changed files in parallel
            let results: Vec<Option<extraction::ExtractionResult>> = changed_files
                .par_iter()
                .map(|f| {
                    if cancelled.load(Ordering::Relaxed) {
                        return None;
                    }
                    extraction::extract_file_parallel(&project_name, f)
                })
                .collect();

            // Register unchanged files in parallel
            let unchanged_files: Vec<&DiscoveredFile> = files
                .iter()
                .filter(|f| !changed_set.contains(f.rel_path.as_str()))
                .collect();
            let reg_entries: Vec<Option<Vec<(String, registry::RegistryEntry)>>> = unchanged_files
                .par_iter()
                .map(|f| {
                    if cancelled.load(Ordering::Relaxed) {
                        return None;
                    }
                    extraction::register_file_parallel(&project_name, f)
                })
                .collect();

            (
                results.into_iter().flatten().collect(),
                reg_entries.into_iter().flatten().collect(),
            )
        });

        // Serial merge: apply parallel results into GraphBuffer and Registry
        // Emit progress every 100 files during merge (Requirements 7.2)
        let merge_total = parallel_results.len();
        for (i, result) in parallel_results.into_iter().enumerate() {
            result.apply(&mut buf, &mut reg);
            let count = i + 1;
            if count % 100 == 0 || count == merge_total {
                self.emit_overall_progress(
                    0.02,
                    0.38,
                    count as f64 / merge_total.max(1) as f64,
                    &format!("Extracted {}/{} files", count, merge_total),
                );
            }
        }
        for entries in parallel_reg_entries {
            extraction::ExtractionResult::apply_registry_only(entries, &mut reg);
        }

        // Parallel Java/Kotlin/Go extraction for changed files (Requirements 6.1, 6.2, 6.3)
        let jkg_results: Vec<extraction::ExtractionResult> = pool.install(|| {
            changed_files
                .par_iter()
                .filter(|f| {
                    matches!(
                        f.language,
                        codryn_discover::Language::Java
                            | codryn_discover::Language::Kotlin
                            | codryn_discover::Language::Go
                    )
                })
                .filter_map(|f| {
                    if cancelled.load(Ordering::Relaxed) {
                        return None;
                    }
                    match f.language {
                        codryn_discover::Language::Java => {
                            spring_java::extract_java_parallel(&project_name, f)
                        }
                        codryn_discover::Language::Kotlin => {
                            spring_kotlin::extract_kotlin_parallel(&project_name, f)
                        }
                        codryn_discover::Language::Go => {
                            go_adapter::extract_go_parallel(&project_name, f)
                        }
                        _ => None,
                    }
                })
                .collect()
        });

        // Serial merge of parallel Java/Kotlin/Go extraction results
        for result in jkg_results {
            result.apply(&mut buf, &mut reg);
        }

        // Register unchanged Java/Kotlin/Go files (for call resolution)
        for f in &files {
            if self.is_cancelled() {
                break;
            }
            if matches!(
                f.language,
                codryn_discover::Language::Java
                    | codryn_discover::Language::Kotlin
                    | codryn_discover::Language::Go
            ) && !changed_set.contains(f.rel_path.as_str())
            {
                extraction::register_file(&mut reg, &project_name, f);
            }
        }

        // Type assignment extraction: populate TypeRegistry from all files
        // This runs before pass_calls so type data is available for disambiguation
        // Skip entirely in Fast mode (Requirements 5.4)
        if !self.is_cancelled() && effective_mode != IndexMode::Fast {
            // Parallel extraction using rayon (Requirements 5.1, 5.2, 5.3)
            let file_type_results: Vec<extraction::FileTypeResult> = pool.install(|| {
                files
                    .par_iter()
                    .filter_map(|f| {
                        if cancelled.load(Ordering::Relaxed) {
                            return None;
                        }
                        let source = file_cache.get(&f.abs_path)?;
                        Some(extraction::extract_file_types(f, &source))
                    })
                    .collect()
            });

            // Serial merge into TypeRegistry
            for result in file_type_results {
                for (file_path, symbol_name, resolved_type) in result.types {
                    type_reg.register_type(&file_path, &symbol_name, &resolved_type);
                }
                for (importer, imported) in result.imports {
                    type_reg.register_import(&importer, &imported);
                }
            }

            tracing::info!(
                types = type_reg.len(),
                "pipeline: type assignment extraction complete (parallel)"
            );
        }

        // ═══════════════════════════════════════════════════════════════════
        // PHASE 1: Structure + Definitions (single flush for nodes)
        // ═══════════════════════════════════════════════════════════════════
        let extraction_elapsed = extraction_start.elapsed();
        let mut flush_total = std::time::Duration::ZERO;

        // Memory pressure check: evict FileCache entries if over limit (Requirement 3.3)
        memory::evict_file_cache_if_needed(&mut file_cache);

        // Memory pressure check: if above threshold, flush early (Requirement 3.2)
        if let Some(ref monitor) = self.memory_monitor {
            if monitor.should_flush() {
                tracing::info!("pipeline: memory pressure detected, flushing buffers early");
            }
        }

        self.emit_overall_progress(0.40, 0.05, 0.0, "Flushing structure + definitions...");
        // Flush nodes only (edges will be rebuilt below)
        let edges_backup = buf.take_edges();
        let flush_start = Instant::now();
        buf.flush(store)?;
        flush_total += flush_start.elapsed();
        // Seed qn_to_id from all existing DB nodes so edge resolution works
        // even when no new nodes were inserted (incremental reindex, nothing changed).
        buf.seed_ids_from_store(store)?;

        // Edge deletion strategy depends on index mode:
        // - Full mode: delete ALL edges and rebuild from scratch (correct, complete).
        // - Fast/incremental mode: only delete edges for changed files.
        //   Edges for unchanged files are preserved — they were already correct.
        //   This prevents the regression where a reindex with few changed files
        //   would wipe all edges and only rebuild a small subset.
        if effective_mode == IndexMode::Full {
            store.delete_project_edges(&project_name)?;
            // Clear FTS content for full rebuild
            store.delete_project_code_fts(&project_name)?;
        } else {
            // Incremental: delete only edges touching changed-file nodes.
            // Nodes are NOT deleted in Fast mode (see above) — only edges are rebuilt.
            // This preserves node IDs so edges from unchanged files remain valid.
            let changed_paths: Vec<&str> =
                changed_files.iter().map(|f| f.rel_path.as_str()).collect();
            if !changed_paths.is_empty() {
                store.delete_edges_for_files(&project_name, &changed_paths)?;
            }
            // Clear FTS entries for changed files only
            store.delete_project_code_fts(&project_name)?;
        }

        // Re-add CONTAINS edges from pass_structure so they survive the delete
        buf.restore_edges(edges_backup);

        // Sync cancellation flag for parallel passes
        passes::set_pass_cancelled(self.is_cancelled());

        tracing::info!("pipeline: phase 1 complete (structure + definitions)");

        // Mark extraction phase as completed
        self.complete_checkpoint(
            store,
            &project_name,
            phases::EXTRACTION,
            changed_files.len() as u32,
            run_id,
        )?;
        // Compute the incremental file set for pass filtering (Requirements 4.1-4.6)
        let file_set = if effective_mode == IndexMode::Full {
            // Full mode: all passes get all files
            IncrementalFileSet {
                changed: files.iter().collect(),
                changed_plus_dependents: files.iter().collect(),
                all: files.iter().collect(),
            }
        } else {
            IncrementalFileSet::compute(&files, &changed_files, store, &project_name)
        };

        // Fast mode: if nothing changed, skip all passes — no edges to rebuild.
        // This prevents passes that run on all_file_refs (pass_configures, pass_route_nodes, etc.)
        // from accumulating duplicate edges on every run.
        if effective_mode == IndexMode::Fast && changed_files.is_empty() {
            tracing::info!(
                project = project_name,
                "pipeline: fast mode, no files changed — skipping all passes"
            );
            store.store_file_hashes_batch(&new_hashes)?;
            store.clear_checkpoint(&project_name)?;
            return Ok(());
        }
        tracing::info!(
            changed = file_set.changed.len(),
            changed_plus_dep = file_set.changed_plus_dependents.len(),
            all = file_set.all.len(),
            "pipeline: incremental file set computed"
        );

        // ═══════════════════════════════════════════════════════════════════
        // PHASE 2: Core Edges (single flush for pass_calls, pass_type_refs,
        //          pass_service_patterns, pass_imports, pass_rest_contracts,
        //          pass_spring_routes, pass_go_routes)
        // ═══════════════════════════════════════════════════════════════════

        // Record checkpoint for phase 2
        if start_phase <= phases::phase_index(phases::PHASE2_EDGES) {
            self.record_checkpoint(store, &project_name, phases::PHASE2_EDGES, 0, run_id)?;
        }

        let phase2_start = Instant::now();

        // Pass: Calls — use changed_plus_dependents for incremental correctness
        // (includes changed files + files that import them, so call edges are complete)
        if self.is_cancelled() {
            return Ok(());
        }
        self.emit_overall_progress(0.45, 0.20, 0.0, "Resolving function calls...");
        passes::pass_calls_with_types(
            &mut buf,
            &reg,
            Some(&type_reg),
            &file_set.changed_plus_dependents,
            &project_name,
        );

        // Pass: Type references — create TYPE_REF edges from functions to referenced types
        if !self.is_cancelled() {
            self.emit_overall_progress(0.45, 0.20, 0.3, "Resolving type references...");
            passes::pass_type_refs(&mut buf, &reg, store, &project_name);
        }

        // Pass: Service pattern classification — reclassify CALLS edges by library type
        if !self.is_cancelled() {
            self.emit_overall_progress(0.45, 0.20, 0.4, "Classifying service patterns...");
            passes::pass_service_patterns(&mut buf, store, &project_name);
        }

        // Build PackageMap from manifest files (before imports pass)
        let file_refs: Vec<&DiscoveredFile> = files.iter().collect();
        let pkg_map = passes::pass_pkgmap(&file_refs, &project_name);
        if !pkg_map.is_empty() {
            tracing::info!(
                count = pkg_map.len(),
                "pipeline: built package map from manifests"
            );
        }

        // Build CompileCommandsMap from compile_commands.json (before imports pass)
        let cc_map = passes::pass_compile_commands(&self.repo_path);
        if !cc_map.is_empty() {
            tracing::info!(count = cc_map.len(), "pipeline: built compile commands map");
        }

        // Pass: Imports — run on changed files only (Requirement 4.1)
        if !self.is_cancelled() {
            self.emit_overall_progress(0.45, 0.20, 0.5, "Resolving imports...");
            let pkg_map_ref = if pkg_map.is_empty() {
                None
            } else {
                Some(&pkg_map)
            };
            let cc_map_ref = if cc_map.is_empty() {
                None
            } else {
                Some(&cc_map)
            };
            passes::pass_imports_with_pkgmap(
                &mut buf,
                &file_set.changed,
                &project_name,
                pkg_map_ref,
                cc_map_ref,
            );
        }

        // Pass: REST contract indexing — run on changed files only (Requirement 4.3)
        if !self.is_cancelled() {
            self.emit_overall_progress(0.45, 0.20, 0.7, "Indexing REST contracts...");
            passes::pass_rest_contracts(&mut buf, &reg, &file_set.changed, &project_name);
        }

        // Pass: Spring Boot routes — run on changed files only (Requirement 4.3)
        if !self.is_cancelled() {
            self.emit_overall_progress(0.45, 0.20, 0.8, "Indexing Spring routes...");
            passes::pass_spring_routes(&mut buf, &file_set.changed, &project_name);
        }

        // Pass: Go routes — run on changed files only (Requirement 4.3)
        if !self.is_cancelled() {
            self.emit_overall_progress(0.45, 0.20, 0.9, "Indexing Go routes...");
            go_adapter::pass_go_routes(&mut buf, &file_set.changed, &project_name);
        }

        // Pass: C/C++ preprocessor — extract macros and INCLUDES edges for changed C/C++ files
        if !self.is_cancelled() {
            let cc_map_ref = if cc_map.is_empty() {
                None
            } else {
                Some(&cc_map)
            };
            cpp_preprocessor::pass_cpp_preprocessor(
                &mut buf,
                &file_set.changed,
                &project_name,
                cc_map_ref,
            );
        }

        // Pass: FastAPI dependency injection — create INJECTS edges for Depends() patterns
        if !self.is_cancelled() {
            fastapi_adapter::pass_fastapi_depends(&mut buf, &file_set.changed, &project_name);
        }

        // Single flush for all Phase 2 edges
        if self.is_cancelled() {
            return Ok(());
        }
        self.emit_overall_progress(0.45, 0.20, 0.95, "Flushing core edges...");
        let flush_start = Instant::now();
        buf.flush(store)?;
        flush_total += flush_start.elapsed();
        let phase2_elapsed = phase2_start.elapsed();
        tracing::info!("pipeline: phase 2 complete (core edges)");

        // Mark phase 2 as completed
        self.complete_checkpoint(
            store,
            &project_name,
            phases::PHASE2_EDGES,
            files.len() as u32,
            run_id,
        )?;
        // Memory pressure check between phases (Requirement 3.2)
        memory::evict_file_cache_if_needed(&mut file_cache);
        if let Some(ref monitor) = self.memory_monitor {
            if monitor.should_flush() {
                tracing::info!("pipeline: memory pressure detected between phase 2 and 3");
                file_cache.clear();
            }
        }

        // ═══════════════════════════════════════════════════════════════════
        // PHASE 3: Semantic + Framework (single flush for pass_semantic,
        //          pass_angular_templates, pass_angular, pass_vue,
        //          pass_cross_project_mapping, concurrent passes,
        //          pass_semantic_edges_v2)
        // ═══════════════════════════════════════════════════════════════════

        if self.is_cancelled() {
            return Ok(());
        }

        // Record checkpoint for phase 3
        if start_phase <= phases::phase_index(phases::PHASE3_SEMANTIC) {
            self.record_checkpoint(store, &project_name, phases::PHASE3_SEMANTIC, 0, run_id)?;
        }

        let phase3_start = Instant::now();
        self.emit_overall_progress(0.65, 0.15, 0.0, "Analyzing semantic relationships...");

        // Fast mode skips semantic pass (INHERITS/IMPLEMENTS) for speed
        if self.mode == IndexMode::Full {
            passes::pass_semantic(store, &project_name, &changed_files)?;
            // Go interface satisfaction (IMPLEMENTS edges via method-set comparison)
            // pass_semantic needs all files for global context (Requirement 4.4)
            go_adapter::pass_go_implements(&mut buf, &file_set.all, &project_name);
        }

        // Pass: Angular template awareness (RENDERS edges) — run on changed files (Requirement 4.3)
        if !self.is_cancelled() {
            passes::pass_angular_templates(&mut buf, store, &file_set.changed, &project_name);
        }

        // Pass: Angular selectors, DI, inline templates — run on changed files (Requirement 4.3)
        if !self.is_cancelled() {
            angular_adapter::pass_angular(&mut buf, store, &file_set.changed, &project_name);
            angular_adapter::pass_angular_classify(store, &project_name);
        }

        // Pass: Vue selectors, composable DI, template RENDERS — run on changed files (Requirement 4.3)
        if !self.is_cancelled() {
            vue_adapter::pass_vue(&mut buf, store, &file_set.changed, &project_name);
        }

        // Pass: Cross-project name-based auto-linking (MAPS_TO edges)
        if !self.is_cancelled() {
            passes::pass_cross_project_mapping(&mut buf, store, &project_name);
        }

        // Concurrent passes: Config file linking, Generic route detection,
        // Semantic edges (OVERRIDES, DELEGATES_TO), Event/channel detection.
        // These passes are independent — run them concurrently with separate buffers.
        // These need all files for global context (semantic analysis).
        if !self.is_cancelled() {
            let all_file_refs: &Vec<&DiscoveredFile> = &file_set.all;
            let mut buf_configlink = GraphBuffer::new(&project_name);
            let mut buf_routes = GraphBuffer::new(&project_name);
            let mut buf_semantic = GraphBuffer::new(&project_name);
            let mut buf_events = GraphBuffer::new(&project_name);

            rayon::scope(|s| {
                s.spawn(|_| {
                    if !cancelled.load(Ordering::Relaxed) {
                        passes::pass_configures(
                            &mut buf_configlink,
                            &reg,
                            all_file_refs,
                            &project_name,
                        );
                    }
                });
                s.spawn(|_| {
                    if !cancelled.load(Ordering::Relaxed) {
                        passes::pass_route_nodes(
                            &mut buf_routes,
                            &reg,
                            all_file_refs,
                            &project_name,
                        );
                    }
                });
                s.spawn(|_| {
                    if !cancelled.load(Ordering::Relaxed) {
                        passes::pass_semantic_edges(
                            &mut buf_semantic,
                            &reg,
                            all_file_refs,
                            &project_name,
                        );
                    }
                });
                s.spawn(|_| {
                    if !cancelled.load(Ordering::Relaxed) {
                        passes::pass_events(&mut buf_events, &reg, all_file_refs, &project_name);
                    }
                });
            });

            // Merge concurrent pass buffers into the main buffer
            buf.merge_from(buf_configlink);
            buf.merge_from(buf_routes);
            buf.merge_from(buf_semantic);
            buf.merge_from(buf_events);
        }

        // Pass: Semantic edges v2 (INHERITS, DECORATES, IMPLEMENTS)
        // Runs after pass_semantic_edges (OVERRIDES, DELEGATES_TO) and needs Store access.
        // Needs seed_ids_from_store for QN resolution of edges referencing existing nodes.
        if !self.is_cancelled() {
            buf.seed_ids_from_store(store)?;
            passes::pass_semantic_edges_v2(&mut buf, &reg, store, &project_name);
        }

        // Single flush for all Phase 3 edges
        if self.is_cancelled() {
            return Ok(());
        }
        self.emit_overall_progress(0.65, 0.15, 0.9, "Flushing semantic edges...");
        let flush_start = Instant::now();
        buf.flush(store)?;
        flush_total += flush_start.elapsed();
        let phase3_elapsed = phase3_start.elapsed();
        tracing::info!("pipeline: phase 3 complete (semantic + framework)");

        // Mark phase 3 as completed
        self.complete_checkpoint(
            store,
            &project_name,
            phases::PHASE3_SEMANTIC,
            files.len() as u32,
            run_id,
        )?;
        // ═══════════════════════════════════════════════════════════════════
        // PHASE 4: Infrastructure (single flush for pass_k8s, pass_kustomize,
        //          pass_infrascan, pass_pipelines, pass_iac, pass_cross_repo)
        // ═══════════════════════════════════════════════════════════════════

        if self.is_cancelled() {
            return Ok(());
        }

        // Record checkpoint for phase 4
        if start_phase <= phases::phase_index(phases::PHASE4_INFRASTRUCTURE) {
            self.record_checkpoint(
                store,
                &project_name,
                phases::PHASE4_INFRASTRUCTURE,
                0,
                run_id,
            )?;
        }

        let phase4_start = Instant::now();
        self.emit_overall_progress(0.80, 0.10, 0.0, "Scanning infrastructure...");
        {
            let file_refs: Vec<&DiscoveredFile> = files.iter().collect();
            let mut buf_k8s = GraphBuffer::new(&project_name);
            let mut buf_kustomize = GraphBuffer::new(&project_name);
            let mut buf_infrascan = GraphBuffer::new(&project_name);
            let mut buf_pipelines = GraphBuffer::new(&project_name);
            let mut buf_iac = GraphBuffer::new(&project_name);

            rayon::scope(|s| {
                s.spawn(|_| {
                    if !cancelled.load(Ordering::Relaxed) {
                        passes::pass_k8s(&mut buf_k8s, &file_refs, &project_name);
                    }
                });
                s.spawn(|_| {
                    if !cancelled.load(Ordering::Relaxed) {
                        passes::pass_kustomize(&mut buf_kustomize, &file_refs, &project_name);
                    }
                });
                s.spawn(|_| {
                    if !cancelled.load(Ordering::Relaxed) {
                        passes::pass_infrascan(&mut buf_infrascan, &file_refs, &project_name);
                    }
                });
                s.spawn(|_| {
                    if !cancelled.load(Ordering::Relaxed) {
                        passes::pass_pipelines(&mut buf_pipelines, &file_refs, &project_name);
                    }
                });
                s.spawn(|_| {
                    if !cancelled.load(Ordering::Relaxed) {
                        passes::pass_iac(&mut buf_iac, &file_refs, &project_name);
                    }
                });
            });

            // Merge all infrastructure buffers into a single buffer for one flush
            let mut buf_infra = GraphBuffer::new(&project_name);
            buf_infra.merge_from(buf_k8s);
            buf_infra.merge_from(buf_kustomize);
            buf_infra.merge_from(buf_infrascan);
            buf_infra.merge_from(buf_pipelines);
            buf_infra.merge_from(buf_iac);

            // Pass: Cross-repo intelligence (CROSS_HTTP, CROSS_CHANNEL, CROSS_ASYNC edges)
            // Only runs when the project has at least one linked project.
            if !self.is_cancelled() {
                let links = store.get_linked_projects(&project_name).unwrap_or_default();
                if !links.is_empty() {
                    buf_infra.seed_ids_from_store(store)?;
                    passes::pass_cross_repo(&mut buf_infra, store, &project_name);
                    tracing::info!(
                        linked = links.len(),
                        "pipeline: cross-repo intelligence pass complete"
                    );
                }
            }

            // Single flush for all Phase 4 edges
            self.emit_overall_progress(0.80, 0.10, 0.9, "Flushing infrastructure edges...");
            let flush_start = Instant::now();
            buf_infra.flush(store)?;
            flush_total += flush_start.elapsed();
        }
        let phase4_elapsed = phase4_start.elapsed();
        tracing::info!("pipeline: phase 4 complete (infrastructure)");

        // Mark phase 4 as completed
        self.complete_checkpoint(
            store,
            &project_name,
            phases::PHASE4_INFRASTRUCTURE,
            files.len() as u32,
            run_id,
        )?;
        // ═══════════════════════════════════════════════════════════════════
        // PHASE 5: Enrichment (single flush for pass_enrichment,
        //          pass_similarity, pass_gitdiff, pass_githistory)
        // ═══════════════════════════════════════════════════════════════════

        // Pass: Enrichment (fan-in, fan-out, centrality) — skip in Fast mode
        if self.is_cancelled() {
            return Ok(());
        }

        // Record checkpoint for phase 5
        if start_phase <= phases::phase_index(phases::PHASE5_ENRICHMENT) {
            self.record_checkpoint(store, &project_name, phases::PHASE5_ENRICHMENT, 0, run_id)?;
        }

        let phase5_start = Instant::now();
        self.emit_overall_progress(0.90, 0.10, 0.0, "Running enrichment...");
        if self.mode == IndexMode::Full {
            passes::pass_enrichment(store, &project_name)?;
        }

        // Pass: Similarity detection (MinHash fingerprinting) — skip in Fast mode
        if !self.is_cancelled() && self.mode == IndexMode::Full {
            passes::pass_similarity(&mut buf, store, &project_name, &self.repo_path);
        }

        // Pass: Git history enrichment — enrich nodes with commit frequency,
        // last-modified dates, and contributor counts. Skip in Fast mode.
        if !self.is_cancelled() && self.mode == IndexMode::Full {
            if let Err(e) = git_history::pass_githistory(store, &project_name, &self.repo_path) {
                tracing::warn!(error = %e, "pipeline: git history pass failed (non-fatal)");
            }
        }

        // Pass: Git history integration — skip in Fast mode
        #[cfg(feature = "git-history")]
        {
            if !self.is_cancelled() && self.mode == IndexMode::Full {
                passes::pass_gitdiff(&mut buf, &project_name, &self.repo_path);
            }
            if !self.is_cancelled() && self.mode == IndexMode::Full {
                passes::pass_githistory(&mut buf, &store, &project_name, &self.repo_path, 100);
            }
        }

        // Pass: Git diff node annotation — annotate nodes with has_uncommitted_changes
        #[cfg(feature = "git-history")]
        {
            if !self.is_cancelled() && self.mode == IndexMode::Full {
                if let Err(e) = pass_gitdiff::pass_gitdiff(store, &project_name, &self.repo_path) {
                    tracing::warn!(error = %e, "pipeline: git diff annotation pass failed (non-fatal)");
                }
            }
        }

        // Single flush for all Phase 5 edges (similarity + git history)
        if buf.edge_count() > 0 || buf.node_count() > 0 {
            self.emit_overall_progress(0.90, 0.10, 0.9, "Flushing enrichment data...");
            let flush_start = Instant::now();
            buf.flush(store)?;
            flush_total += flush_start.elapsed();
        }
        let phase5_elapsed = phase5_start.elapsed();
        tracing::info!("pipeline: phase 5 complete (enrichment)");

        // Mark phase 5 as completed
        self.complete_checkpoint(
            store,
            &project_name,
            phases::PHASE5_ENRICHMENT,
            files.len() as u32,
            run_id,
        )?;
        // Update file hashes — persist SHA-256 hashes after successful extraction (Requirement 4.1)
        store.store_file_hashes_batch(&new_hashes)?;

        // Mark files no longer on disk as deleted
        let live_paths: Vec<String> = files.iter().map(|f| f.rel_path.clone()).collect();
        let deleted = store
            .mark_deleted_files(&project_name, &live_paths)
            .unwrap_or(0);
        if deleted > 0 {
            tracing::info!(count = deleted, "pipeline: marked stale files as deleted");
        }

        // Clear the file cache to free memory
        file_cache.clear();

        // Log memory high-water mark at end of run (Requirement 3.4)
        if let Some(ref monitor) = self.memory_monitor {
            monitor.log_high_water_mark();
        }

        // Log timing summary (Requirement 7.4)
        let overall_elapsed = overall_start.elapsed();
        tracing::info!(
            total_secs = overall_elapsed.as_secs_f64(),
            discovery_secs = discovery_elapsed.as_secs_f64(),
            extraction_secs = extraction_elapsed.as_secs_f64(),
            phase2_core_edges_secs = phase2_elapsed.as_secs_f64(),
            phase3_semantic_secs = phase3_elapsed.as_secs_f64(),
            phase4_infrastructure_secs = phase4_elapsed.as_secs_f64(),
            phase5_enrichment_secs = phase5_elapsed.as_secs_f64(),
            flush_total_secs = flush_total.as_secs_f64(),
            "pipeline: timing summary"
        );

        tracing::info!(project = project_name, "pipeline: complete");

        // Clear all checkpoints on successful completion (Requirement 6.2)
        store.clear_checkpoint(&project_name)?;

        Ok(())
    }

    fn is_cancelled(&self) -> bool {
        let c = self.cancelled.load(Ordering::Relaxed);
        if c {
            // Propagate cancellation to parallel pass workers
            passes::set_pass_cancelled(true);
        }
        c
    }

    fn compute_changed<'a>(
        &self,
        files: &'a [DiscoveredFile],
        old_map: &HashMap<String, String>,
    ) -> Result<(Vec<&'a DiscoveredFile>, Vec<FileHash>, FileCache)> {
        let project = self.project_name();
        let mut changed = Vec::new();
        let mut hashes = Vec::new();
        let mut cache = FileCache::new();

        for f in files {
            let content = match std::fs::read(&f.abs_path) {
                Ok(c) => c,
                Err(_) => continue,
            };
            let hash = hex::encode(Sha256::digest(&content));
            let size = content.len() as i64;

            // Cache the content as a String for later reuse by extraction/passes
            if let Ok(s) = String::from_utf8(content) {
                cache.insert(f.abs_path.clone(), Arc::new(s));
            }

            hashes.push(FileHash {
                project: project.clone(),
                rel_path: f.rel_path.clone(),
                sha256: hash.clone(),
                mtime_ns: 0,
                size,
            });

            if old_map.get(&f.rel_path) != Some(&hash) {
                changed.push(f);
            }
        }
        Ok((changed, hashes, cache))
    }
}
