pub mod angular_adapter;
pub mod extraction;
pub mod go_adapter;
pub mod go_common;
pub mod jsx_framework;
pub mod lambda_cfn;
pub mod nextjs_routes;
pub mod passes;
pub mod registry;
pub mod spring_common;
pub mod spring_java;
pub mod spring_kotlin;
pub mod vue_adapter;
pub mod vue_sfc;

use anyhow::Result;
use chrono::Utc;
use codryn_discover::{discover_files_with_mappings, load_language_mappings, DiscoveredFile};
use codryn_foundation::fqn;
use codryn_graph_buffer::GraphBuffer;
use codryn_store::{FileHash, Project, Store};
use rayon::prelude::*;
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
    num_threads: usize,
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
        }
    }

    /// Set the maximum number of threads for parallel extraction.
    /// A value of 0 means use the default (number of CPU cores).
    pub fn set_num_threads(&mut self, n: usize) {
        self.num_threads = n;
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
        let mappings = load_language_mappings(&self.repo_path);
        let files = discover_files_with_mappings(&self.repo_path, &mappings)?;
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

        // Delete existing symbol nodes for changed files BEFORE re-extracting.
        // This prevents stale nodes from accumulating when functions are renamed or deleted
        // within a file that still exists. File and Folder nodes are preserved.
        {
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
            builder
                .build()
                .unwrap_or_else(|_| rayon::ThreadPoolBuilder::new().build().unwrap())
        };

        // Parallel extraction for changed files
        let cancelled = &self.cancelled;
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
        for result in parallel_results {
            result.apply(&mut buf, &mut reg);
        }
        for entries in parallel_reg_entries {
            extraction::ExtractionResult::apply_registry_only(entries, &mut reg);
        }

        // Handle Java/Kotlin/Go files serially (their extractors mutate buf/reg directly)
        for f in &files {
            if self.is_cancelled() {
                break;
            }
            if matches!(
                f.language,
                codryn_discover::Language::Java
                    | codryn_discover::Language::Kotlin
                    | codryn_discover::Language::Go
            ) {
                if changed_set.contains(f.rel_path.as_str()) {
                    extraction::extract_file(&mut buf, &mut reg, &project_name, f);
                } else {
                    extraction::register_file(&mut reg, &project_name, f);
                }
            }
        }

        // Type assignment extraction: populate TypeRegistry from all files
        // This runs before pass_calls so type data is available for disambiguation
        if !self.is_cancelled() {
            for f in &files {
                let source = match std::fs::read_to_string(&f.abs_path) {
                    Ok(s) => s,
                    Err(_) => continue,
                };
                // Extract type assignments from tree-sitter symbols
                if let Some(symbols) = codryn_treesitter::extract_symbols(f.language, &source) {
                    extraction::extract_type_assigns(
                        &mut type_reg,
                        &f.rel_path,
                        &symbols,
                        f.language,
                    );
                }
                // Analyze scope for variable type annotations
                registry::analyze_scope(&mut type_reg, &f.rel_path, &source, f.language);
            }
            tracing::info!(
                types = type_reg.len(),
                "pipeline: type assignment extraction complete"
            );
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

        // Sync cancellation flag for parallel passes
        passes::set_pass_cancelled(self.is_cancelled());

        // Pass 3: Calls — always run on all files so edges are never missing on reindex
        if self.is_cancelled() {
            return Ok(());
        }
        passes::pass_calls_with_types(
            &mut buf,
            &reg,
            Some(&type_reg),
            &files.iter().collect::<Vec<_>>(),
            &project_name,
        );
        buf.flush(&store)?;

        // Pass 3b: Type references — create TYPE_REF edges from functions to referenced types
        if !self.is_cancelled() {
            passes::pass_type_refs(&mut buf, &reg, &store, &project_name);
            buf.flush(&store)?;
        }

        // Pass 3c: Service pattern classification — reclassify CALLS edges by library type
        if !self.is_cancelled() {
            passes::pass_service_patterns(&mut buf, &store, &project_name);
            buf.flush(&store)?;
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

        // Pass 4: Imports — same, run on all files; use PackageMap for bare specifier resolution
        // and CompileCommandsMap for C/C++ #include resolution
        if self.is_cancelled() {
            return Ok(());
        }
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
            &file_refs,
            &project_name,
            pkg_map_ref,
            cc_map_ref,
        );
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

        // Pass 5c: SAM / CloudFormation Lambda HTTP events and Serverless Framework routes
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

        // Pass 5e: Go routes (net/http + frameworks)
        if self.is_cancelled() {
            return Ok(());
        }
        go_adapter::pass_go_routes(&mut buf, &files.iter().collect::<Vec<_>>(), &project_name);
        buf.flush(&store)?;

        if self.is_cancelled() {
            return Ok(());
        }
        // Fast mode skips semantic pass (INHERITS/IMPLEMENTS) for speed
        if self.mode == IndexMode::Full {
            passes::pass_semantic(&store, &project_name, &changed_files)?;
            // Go interface satisfaction (IMPLEMENTS edges via method-set comparison)
            go_adapter::pass_go_implements(
                &mut buf,
                &files.iter().collect::<Vec<_>>(),
                &project_name,
            );
            buf.flush(&store)?;
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

        // Pass: Vue selectors, composable DI, template RENDERS
        if self.is_cancelled() {
            return Ok(());
        }
        vue_adapter::pass_vue(
            &mut buf,
            &store,
            &files.iter().collect::<Vec<_>>(),
            &project_name,
        );
        buf.flush(&store)?;

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

        // Pass: Config file linking (CONFIG_LINKS edges)
        // Pass: Generic route detection (Express, FastAPI, Flask, Gin)
        // Pass: Semantic edges (OVERRIDES, DELEGATES_TO)
        // Pass: Event/channel detection (EMITS, LISTENS)
        // These passes are independent — run them concurrently with separate buffers.
        if self.is_cancelled() {
            return Ok(());
        }
        {
            let file_refs: Vec<&DiscoveredFile> = files.iter().collect();
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
                            &file_refs,
                            &project_name,
                        );
                    }
                });
                s.spawn(|_| {
                    if !cancelled.load(Ordering::Relaxed) {
                        passes::pass_route_nodes(&mut buf_routes, &reg, &file_refs, &project_name);
                    }
                });
                s.spawn(|_| {
                    if !cancelled.load(Ordering::Relaxed) {
                        passes::pass_semantic_edges(
                            &mut buf_semantic,
                            &reg,
                            &file_refs,
                            &project_name,
                        );
                    }
                });
                s.spawn(|_| {
                    if !cancelled.load(Ordering::Relaxed) {
                        passes::pass_events(&mut buf_events, &reg, &file_refs, &project_name);
                    }
                });
            });

            // Merge results serially
            buf_configlink.flush(&store)?;
            buf_routes.flush(&store)?;
            buf_semantic.flush(&store)?;
            buf_events.flush(&store)?;
        }

        // Pass: Semantic edges v2 (INHERITS, DECORATES, IMPLEMENTS)
        // Runs after pass_semantic_edges (OVERRIDES, DELEGATES_TO) and needs Store access.
        if !self.is_cancelled() {
            let mut buf_semantic_v2 = GraphBuffer::new(&project_name);
            buf_semantic_v2.seed_ids_from_store(&store)?;
            passes::pass_semantic_edges_v2(&mut buf_semantic_v2, &reg, &store, &project_name);
            buf_semantic_v2.flush(&store)?;
        }

        // Pass: Infrastructure — K8s manifests, Kustomize, Dockerfiles/Helm, CI/CD pipelines, IaC
        if self.is_cancelled() {
            return Ok(());
        }
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

            buf_k8s.flush(&store)?;
            buf_kustomize.flush(&store)?;
            buf_infrascan.flush(&store)?;
            buf_pipelines.flush(&store)?;
            buf_iac.flush(&store)?;
        }

        // Pass: Cross-repo intelligence (CROSS_HTTP, CROSS_CHANNEL, CROSS_ASYNC edges)
        // Only runs when the project has at least one linked project.
        if !self.is_cancelled() {
            let links = store.get_linked_projects(&project_name).unwrap_or_default();
            if !links.is_empty() {
                let mut buf_cross = GraphBuffer::new(&project_name);
                buf_cross.seed_ids_from_store(&store)?;
                passes::pass_cross_repo(&mut buf_cross, &store, &project_name);
                buf_cross.flush(&store)?;
                tracing::info!(
                    linked = links.len(),
                    "pipeline: cross-repo intelligence pass complete"
                );
            }
        }

        // Pass: Enrichment (fan-in, fan-out, centrality) — skip in Fast mode
        if self.is_cancelled() {
            return Ok(());
        }
        if self.mode == IndexMode::Full {
            passes::pass_enrichment(&store, &project_name)?;
        }

        // Pass: Similarity detection (MinHash fingerprinting) — skip in Fast mode
        if self.is_cancelled() {
            return Ok(());
        }
        if self.mode == IndexMode::Full {
            passes::pass_similarity(&mut buf, &store, &project_name, &self.repo_path);
            buf.flush(&store)?;
        }

        // Pass: Git history integration — skip in Fast mode
        #[cfg(feature = "git-history")]
        {
            if !self.is_cancelled() && self.mode == IndexMode::Full {
                passes::pass_gitdiff(&mut buf, &project_name, &self.repo_path);
                buf.flush(&store)?;
            }
            if !self.is_cancelled() && self.mode == IndexMode::Full {
                passes::pass_githistory(&mut buf, &store, &project_name, &self.repo_path, 100);
                buf.flush(&store)?;
            }
        }

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
