pub mod angular_adapter;
pub mod extraction;
pub mod lambda_cfn;
pub mod nextjs_routes;
pub mod jsx_framework;
pub mod vue_sfc;
pub mod passes;
pub mod registry;
pub mod spring_common;
pub mod spring_java;
pub mod spring_kotlin;

use anyhow::Result;
use codryn_discover::{discover_files, DiscoveredFile};
use codryn_foundation::fqn;
use codryn_graph_buffer::GraphBuffer;
use codryn_store::{FileHash, Project, Store};
use chrono::Utc;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

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
}

static INDEX_LOCK: Mutex<()> = Mutex::new(());

impl Pipeline {
    pub fn new(repo_path: &Path, db_path: &Path, mode: IndexMode) -> Self {
        Self {
            repo_path: repo_path.to_owned(),
            db_path: db_path.to_owned(),
            mode,
            cancelled: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn cancel_handle(&self) -> Arc<AtomicBool> {
        self.cancelled.clone()
    }

    pub fn project_name(&self) -> String {
        fqn::project_name_from_path(&self.repo_path.to_string_lossy())
    }

    pub fn run(&self) -> Result<()> {
        let _lock = INDEX_LOCK
            .lock()
            .map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        tracing::info!(repo = %self.repo_path.display(), "pipeline: start");

        let store = if self.db_path.to_string_lossy() == ":memory:" {
            Store::open_in_memory()?
        } else {
            std::fs::create_dir_all(&self.db_path)?;
            Store::open(&self.db_path.join("graph.db"))?
        };

        let project_name = self.project_name();
        let now = Utc::now().to_rfc3339();
        store.upsert_project(&Project {
            name: project_name.clone(),
            indexed_at: now,
            root_path: self.repo_path.to_string_lossy().into(),
        })?;

        // Phase 1: Discover files
        if self.is_cancelled() {
            return Ok(());
        }
        let files = discover_files(&self.repo_path)?;
        tracing::info!(count = files.len(), "pipeline: discovered files");

        // Phase 2: Compute file hashes for incremental indexing
        let old_hashes = store.get_file_hashes(&project_name)?;
        let old_map: HashMap<String, String> = old_hashes
            .into_iter()
            .map(|h| (h.rel_path, h.sha256))
            .collect();
        let (changed_files, new_hashes) = self.compute_changed(&files, &old_map)?;
        tracing::info!(changed = changed_files.len(), "pipeline: changed files");

        // Phase 3: Extract and build graph
        if self.is_cancelled() {
            return Ok(());
        }
        let mut buf = GraphBuffer::new(&project_name);

        // Pass 1: Structure nodes (Project, Folder, Module, File)
        passes::pass_structure(&mut buf, &project_name, &self.repo_path, &files);

        // Pass 2: Definitions (Function, Class, Method, Interface, etc.)
        // Always populate the registry from ALL files so call resolution works on reindex.
        // Only add nodes to the buffer for changed files to avoid duplicate inserts.
        if self.is_cancelled() {
            return Ok(());
        }
        let mut reg = registry::Registry::new();
        let changed_set: std::collections::HashSet<&str> =
            changed_files.iter().map(|f| f.rel_path.as_str()).collect();
        for f in &files {
            if self.is_cancelled() {
                break;
            }
            if changed_set.contains(f.rel_path.as_str()) {
                extraction::extract_file(&mut buf, &mut reg, &project_name, f);
            } else {
                extraction::register_file(&mut reg, &project_name, f);
            }
        }

        // Flush nodes only (edges will be rebuilt below)
        let edges_backup = buf.take_edges();
        buf.flush(&store)?;
        // Seed qn_to_id from all existing DB nodes so edge resolution works
        // even when no new nodes were inserted (incremental reindex, nothing changed).
        buf.seed_ids_from_store(&store)?;

        // Delete all edges BEFORE re-adding them (including CONTAINS from pass_structure)
        store.delete_project_edges(&project_name)?;
        // Clear FTS content for rebuild
        store.delete_project_code_fts(&project_name)?;

        // Re-add CONTAINS edges from pass_structure so they survive the delete
        buf.restore_edges(edges_backup);

        // Pass 3: Calls — always run on all files so edges are never missing on reindex
        if self.is_cancelled() {
            return Ok(());
        }
        passes::pass_calls(
            &mut buf,
            &reg,
            &files.iter().collect::<Vec<_>>(),
            &project_name,
        );
        buf.flush(&store)?;

        // Pass 4: Imports — same, run on all files
        if self.is_cancelled() {
            return Ok(());
        }
        passes::pass_imports(&mut buf, &files.iter().collect::<Vec<_>>(), &project_name);
        buf.flush(&store)?;

        // Pass 5: REST contract indexing (Route nodes + DTO edges)
        if self.is_cancelled() {
            return Ok(());
        }
        passes::pass_rest_contracts(
            &mut buf,
            &reg,
            &files.iter().collect::<Vec<_>>(),
            &project_name,
        );
        buf.flush(&store)?;

        // Pass 5b: Spring Boot routes (runs on ALL Java/Kotlin files)
        if self.is_cancelled() {
            return Ok(());
        }
        passes::pass_spring_routes(&mut buf, &files.iter().collect::<Vec<_>>(), &project_name);
        buf.flush(&store)?;

        // Pass 5c: SAM / CloudFormation Lambda HTTP events → Route nodes
        if self.is_cancelled() {
            return Ok(());
        }
        lambda_cfn::pass_lambda_cfn(
            &mut buf,
            &reg,
            &files.iter().collect::<Vec<_>>(),
            &project_name,
            &self.repo_path,
        );
        buf.flush(&store)?;

        // Pass 5c2: Serverless Framework v3/v4 (serverless.yml) → Route nodes
        if self.is_cancelled() {
            return Ok(());
        }
        lambda_cfn::pass_serverless_sls(
            &mut buf,
            &files.iter().collect::<Vec<_>>(),
            &project_name,
            &self.repo_path,
        );
        buf.flush(&store)?;

        // Pass 5d: Next.js App Router + Pages API routes
        if self.is_cancelled() {
            return Ok(());
        }
        nextjs_routes::pass_nextjs_routes(
            &mut buf,
            &reg,
            &files.iter().collect::<Vec<_>>(),
            &project_name,
        );
        buf.flush(&store)?;

        if self.is_cancelled() {
            return Ok(());
        }
        passes::pass_express_routes(
            &mut buf,
            &reg,
            &files.iter().collect::<Vec<_>>(),
            &project_name,
        );
        buf.flush(&store)?;

        if self.is_cancelled() {
            return Ok(());
        }
        // Fast mode skips semantic pass (INHERITS/IMPLEMENTS) for speed
        if self.mode == IndexMode::Full {
            passes::pass_semantic(&store, &project_name, &changed_files)?;
        }

        // Pass: Angular template awareness (RENDERS edges)
        if self.is_cancelled() {
            return Ok(());
        }
        passes::pass_angular_templates(
            &mut buf,
            &store,
            &files.iter().collect::<Vec<_>>(),
            &project_name,
        );
        buf.flush(&store)?;

        // Pass: Angular selectors, DI, inline templates
        if self.is_cancelled() {
            return Ok(());
        }
        angular_adapter::pass_angular(
            &mut buf,
            &store,
            &files.iter().collect::<Vec<_>>(),
            &project_name,
        );
        buf.flush(&store)?;
        angular_adapter::pass_angular_classify(&store, &project_name);

        // Pass: Vue SFC components + RENDERS
        if self.is_cancelled() {
            return Ok(());
        }
        vue_sfc::pass_vue_sfc(
            &mut buf,
            &store,
            &files.iter().collect::<Vec<_>>(),
            &project_name,
        );
        buf.flush(&store)?;

        // Pass: React / Solid JSX RENDERS + framework tagging
        if self.is_cancelled() {
            return Ok(());
        }
        jsx_framework::pass_jsx_framework(
            &mut buf,
            &store,
            &files.iter().collect::<Vec<_>>(),
            &project_name,
        );
        buf.flush(&store)?;
        jsx_framework::pass_jsx_framework_props(
            &store,
            &files.iter().collect::<Vec<_>>(),
            &project_name,
        );

        // Pass: Cross-project name-based auto-linking (MAPS_TO edges)
        if self.is_cancelled() {
            return Ok(());
        }
        passes::pass_cross_project_mapping(&mut buf, &store, &project_name);
        buf.flush(&store)?;

        // Update file hashes
        store.upsert_file_hash_batch(&new_hashes)?;

        // Mark files no longer on disk as deleted
        let live_paths: Vec<String> = files.iter().map(|f| f.rel_path.clone()).collect();
        let deleted = store
            .mark_deleted_files(&project_name, &live_paths)
            .unwrap_or(0);
        if deleted > 0 {
            tracing::info!(count = deleted, "pipeline: marked stale files as deleted");
        }

        tracing::info!(project = project_name, "pipeline: complete");
        Ok(())
    }

    fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Relaxed)
    }

    fn compute_changed<'a>(
        &self,
        files: &'a [DiscoveredFile],
        old_map: &HashMap<String, String>,
    ) -> Result<(Vec<&'a DiscoveredFile>, Vec<FileHash>)> {
        let project = self.project_name();
        let mut changed = Vec::new();
        let mut hashes = Vec::new();

        for f in files {
            let content = match std::fs::read(&f.abs_path) {
                Ok(c) => c,
                Err(_) => continue,
            };
            let hash = hex::encode(Sha256::digest(&content));
            let meta = std::fs::metadata(&f.abs_path).ok();
            let size = meta.as_ref().map(|m| m.len() as i64).unwrap_or(0);

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
        Ok((changed, hashes))
    }
}
