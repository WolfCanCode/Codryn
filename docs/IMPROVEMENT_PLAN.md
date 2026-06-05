# Improvement Plan

Audit of the full codryn codebase — Rust backend and React frontend — produced via the codryn knowledge graph and deep code review. Organised into nine tracks: **Faster**, **Better**, **Looks Good**, **Core Parity**, **Agent-First**, **Reliability & Operations**, **Developer Experience & Distribution**, **Data Quality & Intelligence**, and **Installation & Activation UX**.

Track 4 expanded MCP tool surface to **46 tools**, pipeline passes, and language coverage including Tier 1–3 walkers.

Track 5 adds agent-first features designed specifically to reduce agent tool calls, context window waste, and wrong-turn decisions. All milestones complete as of v0.4.3, including semantic search, OpenAPI generation, API surface diff, graph diff, dependency freshness, and staleness scoring.

> **Status as of v0.4.3:** Tracks 1–6, 8, and 9 are fully complete (Track 9 has 2 remaining items: `index_repository` auto-write trigger and per-workspace `.codryn.toml` override). Track 7 (Developer Experience & Distribution) remains planned.

> Status key: `[ ]` not started · `[~]` in progress · `[x]` done

---

## Track 1 — Faster (Performance)

### 1.1 Batch Insert N+1 in SQLite Store

`codryn-store/src/lib.rs` — `insert_nodes_batch` runs **2N statements** per batch (INSERT then SELECT id per row). SQLite 3.35+ supports `INSERT … RETURNING id`, cutting round-trips in half. Alternatively, build a `qualified_name → id` map from one bulk SELECT before the inserts.

- [x] Replace INSERT + SELECT pattern with `INSERT … RETURNING id`
- [ ] Benchmark before/after on a 10k-node project

### 1.2 Blocking I/O on the Async Runtime

MCP handlers in `codryn-mcp/src/lib.rs` are `async` but call `pipeline.run()` synchronously, blocking the Tokio runtime during full disk + CPU work. Same for `std::fs::read` and `Command::new("git")`.

- [x] Wrap `pipeline.run()` in `tokio::task::spawn_blocking`
- [x] Audit all `std::fs` and `Command` calls inside async fns; move to spawn_blocking

### 1.3 Parallel File Processing in Pipeline

`codryn-pipeline` runs all passes sequentially on a single thread. No `rayon` in the workspace. File-level parallel processing would dramatically speed up indexing.

- [x] Add `rayon` to workspace dependencies
- [x] Parallelise `pass_calls`, `pass_imports`, `pass_semantic` at the file level
- [x] Gate parallelism behind a configurable thread count (default = num_cpus)

### 1.4 Eliminate Duplicate File Reads

`compute_changed` reads every file for SHA-256 hashing; later passes read the same files again. A shared file-content cache between hash and parse passes would save I/O.

- [x] Introduce a `FileCache` (path → Arc<String>) populated during `compute_changed`
- [x] Pass it through to extraction and all passes
- [x] Evict after pipeline run completes

### 1.5 ~~Canvas Hit-Testing Performance~~ → Optimized Graph Library

Replaced hand-rolled d3-force + Canvas 2D rendering with `force-graph` (vasturiano). Built-in quadtree hit-testing, viewport culling, node labels, directional arrows, node dragging, and handles 75k+ elements.

- [x] Replace manual d3-force + Canvas 2D with `force-graph` library
- [x] Built-in quadtree hit-testing (O(log n) instead of O(n) per mouse-move)
- [x] Built-in node labels, directional arrows, curved links, node drag

### 1.6 Store Connection Reuse

`get_store()` in `codryn-mcp/src/lib.rs` opens a **new** `Store` connection on every call while also caching one in a mutex — double work.

- [x] Refactor to always use the cached connection
- [x] Remove the redundant per-call open

### 1.7 Regex Recompilation

`pass_semantic` in `passes.rs` rebuilds regex patterns on every pipeline run.

- [x] Move regexes to `std::sync::LazyLock` (or `once_cell::sync::Lazy`) constants
- [x] Verify no runtime-dependent patterns that actually need dynamic compilation

### 1.8 Similarity Detection O(n²) Guard

`pass_similarity` compared all function pairs in a serial O(n²) loop with no cap. On large projects (thousands of functions), this caused multi-minute freezes.

- [x] Cap similarity comparison at 2,000 functions (`SIMILARITY_MAX_FUNCTIONS`)
- [x] Raise minimum line threshold from 5 to 8 (`SIMILARITY_MIN_LINES`)
- [x] Parallelize comparisons with rayon `par_iter`

### 1.9 Batch Enrichment Queries

`pass_enrichment` called `node_degree()` (2 SQL queries) per node, then `update_node_properties()` per node — ~44K individual SQL round-trips on a 14K-node project.

- [x] Add `node_degrees_bulk()` — 2 bulk `GROUP BY` queries for all fan-in/fan-out
- [x] Add `update_node_properties_batch()` — single transaction for all property updates
- [x] Rewrite `pass_enrichment` to use batch methods

### 1.10 Tree-sitter Error Tolerance

`extract_symbols` rejected the entire parse tree when `has_error()` was true, causing hundreds of files to fall back to regex with `warn!` log spam. Tree-sitter is designed for error recovery.

- [x] Accept partial parse trees (remove `return None` on `has_error()`)
- [x] Downgrade log from `warn!` to `debug!`

### 1.11 Reduce Flush Round-Trips (25 → 3–4)

`Pipeline::run()` calls `buf.flush(&store)` **25 times** during a single index. Each flush opens a transaction, inserts nodes, resolves QNs (including suffix-match fallback queries), inserts edges, and commits. On the reference project (23K nodes, 180K edges), this is the dominant bottleneck — each flush does thousands of individual SQL statements.

- [x] Consolidate passes into 3–4 flush phases: (1) structure + definitions, (2) all edge passes, (3) infra/pipeline passes, (4) enrichment
- [x] Accumulate edges across `pass_calls`, `pass_imports`, `pass_rest_contracts`, `pass_spring_routes`, `pass_go_routes`, `pass_angular`, `pass_vue` into a single buffer before flushing
- [x] Move the 4 concurrent passes (configlink, routes, semantic_edges, events) into the same edge-accumulation phase
- [x] Merge the 5 infra passes (k8s, kustomize, infrascan, pipelines, iac) into a single flush
- [ ] Benchmark: target 60% reduction in total index time on 20K+ node projects

### 1.12 Batch QN Resolution in GraphBuffer::flush()

`GraphBuffer::flush()` resolves unresolved QNs one-by-one via `find_node_by_qn()` and `find_nodes_by_qn_suffix()`. On a 180K-edge project, this means tens of thousands of individual SELECT queries during edge resolution.

- [x] Collect all unresolved QNs into a single `WHERE qualified_name IN (...)` batch query
- [x] For suffix matches, use a single `WHERE qualified_name LIKE '%.' || ?` with a temp table of suffixes
- [x] Pre-populate `qn_to_id` from the full node table once before edge resolution (already done via `seed_ids_from_store`, but called too late for some passes)
- [ ] Benchmark: target <500ms for QN resolution on 180K edges

### 1.13 Skip Unchanged Passes on Incremental Reindex

The pipeline already computes `changed_files` but then runs **every pass on ALL files** (not just changed ones). `pass_calls`, `pass_imports`, and framework passes re-scan every file even when only 5 files changed. The comment in the code says "always run on all files so edges are never missing on reindex" — but this is overly conservative.

- [x] For `pass_calls`: only scan changed files + files that import changed files (1-hop dependents)
- [x] For `pass_imports`: only scan changed files (imports are file-local)
- [x] For framework passes (Angular, Vue, Spring, Go): only scan changed files
- [x] Keep full-scan behavior for `pass_semantic` and `pass_enrichment` (they need global context)
- [x] Add a `--full` flag to force full re-scan when needed
- [ ] Benchmark: target <10s incremental reindex when 5 files changed in a 1,600-file project

### 1.14 Lazy Type Registry Population

Type assignment extraction (`extract_type_assigns` + `analyze_scope`) runs on **every file** serially, even in Fast mode. For the reference project with 1,686 files, this is a significant sequential bottleneck.

- [x] Parallelize type extraction with rayon (it's currently a serial `for f in &files` loop)
- [x] In Fast mode, skip type extraction entirely (it only benefits `pass_calls` disambiguation)
- [ ] Cache type registry results per-file and only re-extract for changed files
- [ ] Benchmark: measure type extraction time in isolation on the reference project

### 1.15 SQLite Write Optimizations

The current SQLite configuration uses WAL mode and 64MB cache, but several additional optimizations would help for bulk writes during indexing.

- [x] Set `PRAGMA temp_store = MEMORY` — keep temp tables in RAM during indexing
- [x] Set `PRAGMA mmap_size = 268435456` (256MB) — memory-mapped I/O for reads during indexing
- [x] Use `BEGIN IMMEDIATE` instead of `BEGIN DEFERRED` for write transactions to avoid BUSY retries
- [x] Disable `PRAGMA foreign_keys` during bulk indexing (re-enable after) — saves constraint checks per row
- [ ] Consider `PRAGMA page_size = 8192` (vs default 4096) for better I/O alignment on modern SSDs
- [x] Add `PRAGMA wal_autocheckpoint = 10000` during indexing to reduce checkpoint frequency
- [ ] Benchmark: measure total index time before/after on the reference project

### 1.16 Java/Kotlin/Go Serial Extraction Bottleneck

Java, Kotlin, and Go files are extracted **serially** because their extractors "mutate buf/reg directly." On the reference project (which is primarily C but has Go tooling), this is less impactful, but on Spring Boot or Go monorepos it's a major bottleneck.

- [x] Refactor Java/Kotlin/Go extractors to use the same `ExtractionResult` pattern as other languages
- [x] This enables parallel extraction via `extract_file_parallel` for all languages
- [x] Eliminate the serial `for f in &files` loop for Java/Kotlin/Go
- [ ] Benchmark: measure extraction time on a Spring Boot project with 500+ Java files

### 1.17 Progress Reporting During Indexing

5–10 minute indexing with no feedback is a poor experience. Agents and users have no idea if it's stuck or making progress.

- [x] Add progress callbacks to `Pipeline::run()`: file discovery %, extraction %, pass N/M, flush %
- [ ] Expose progress via the MCP `index_repository` response (streaming or polling)
- [ ] Add estimated time remaining based on files/second throughput
- [x] Log pass-level timing: "pass_calls: 45s, pass_imports: 12s, flush: 23s" for profiling

---

## Track 2 — Better (Functionality & Robustness)

### 2.1 Complete the Cypher Executor

The parser supports more features than the executor runs.

- [x] Wire `ORDER BY` + `LIMIT` execution (parsed but ignored today)
- [x] Execute all `patterns[]`, not just `patterns[0]`
- [x] Add `OR` / `NOT` support in WHERE clauses
- [x] Parameterise label/type filters in SQL to eliminate injection risk
- [x] Add variable-length path support `()-[*1..N]->()`

### 2.2 Implement Stubbed MCP Tools

Three tools are advertised but non-functional.

- [x] `manage_adr` — back with a `decisions` table in the store, or remove from the tool list
- [x] `ingest_traces` — write trace edges to the graph (CALLS / ASYNC_CALLS from runtime data)
- [x] `trace_call_path` — replace substring JSON search with real BFS/DFS on edges, parameterised queries, configurable depth limit

### 2.3 New High-Value MCP Tools

Tools that AI agents would use frequently but are missing today.

- [x] `find_symbol` — fast ranked symbol lookup by name or qualified name, with match type and score
- [x] `get_symbol_details` — full symbol context in one call: callers, callees, imports, inheritance, optional snippet
- [x] `find_references` — find all references to a symbol via graph edges, grouped by file or symbol
- [x] `impact_analysis` — blast radius analysis: direct/indirect dependents, affected files, risk level
- [x] `explain_index_result` — debug why a file or symbol is missing/incomplete in the index
- [x] `sample_graph` — return a bounded set of high-centrality nodes for orientation
- [x] `search_code` — with configurable context lines around matches

### 2.4 Fix Zero IMPORTS / INHERITS / IMPLEMENTS Edges

The graph schema shows **0 edges** for these types despite the UI and Cypher engine supporting them. The extraction or pass logic is likely broken or not wired.

- [x] Add a test project with known imports/inheritance and verify edges are created
- [x] Debug `pass_imports` and inheritance extraction in `extraction.rs`
- [x] Add regression tests for each edge type

### 2.5 Error Handling Overhaul

- [x] Replace `rows.filter_map(|r| r.ok())` with proper error propagation (at least log + count)
- [x] Add `.context()` to store and pipeline error paths for debuggability
- [x] Introduce `thiserror` enums for public crate APIs (it's declared but unused)
- [x] Audit and replace `unwrap()` in non-test code with `?` or explicit error handling
- [x] Surface graph-loading errors in the frontend (show `sdx-notification`)

### 2.6 Clean Up Dead Code & Stubs

- [x] Remove or implement pass stubs: `pass_tests`, `pass_routes`, `pass_infrascan`, `pass_envscan`, `pass_gitdiff`
- [x] Wire `IndexMode` to pipeline logic or remove `#[allow(dead_code)]`
- [x] ~~Remove unused `tree-sitter` dependency~~ — now used for Java/Kotlin AST extraction
- [x] Remove `goBack()` from `graph.component.ts` (never called from template)

### 2.7 Test Coverage

- [x] Pipeline: unit tests for `pass_calls`, `pass_imports`, `pass_semantic`, `extraction`
- [x] Cypher executor: test each clause type (MATCH, WHERE, RETURN, ORDER BY, LIMIT)
- [x] MCP tools: integration tests for index → query round-trip
- [x] Graph buffer: test flush → store consistency
- [x] Add CI with GitHub Actions (`cargo test`, `cargo clippy`, `ng test`)

### 2.8 Angular Subscription Leak

`graph.component.ts` — `route.queryParams.subscribe` in `ngOnInit` without cleanup.

- [x] Use `takeUntilDestroyed()` or convert to `toSignal(route.queryParams)`

### 2.9 Agent Feedback: Extraction Depth & Edge Quality

Feedback from real agent sessions revealed the graph was too shallow for practical use.

**A. start_line == end_line for all symbols**

- [x] Brace-counting end-line detection in `extract_file` for brace-based languages
- [x] Indentation-based end detection for Python/Ruby/Elixir
- [x] Increase default `snippet_lines` from 20 to ~~50~~ (now caps at 150 using full AST range)

**B. CALLS edges from Module, not Function nodes**

- [x] Added `start_line`/`end_line` to `RegistryEntry`
- [x] `pass_calls` resolves calling function via byte-offset→line-number + line range lookup
- [x] Falls back to Module QN for top-level code

**C. search_graph requires exact symbol names**

- [x] Multi-word queries split into AND-ed `LIKE` tokens with `COLLATE NOCASE`
- [x] Applied to both `search_nodes` and `search_nodes_filtered`
- [x] Consider SQLite FTS5 for full-text search

**D. Cross-project auto-linking**

- [x] Add `include_linked` to `find_symbol` and `find_references`

**E. Framework-aware indexing (Angular, Spring Boot)**

- [x] Extract methods inside classes
- [x] Spring Boot: tree-sitter AST extraction for Java/Kotlin with route, DTO, and layer classification
- [x] Angular: selector nodes, constructor DI graph (INJECTS), inline template RENDERS, layer classification
- [x] Vue: component name extraction, selector indexing, composable DI (INJECTS), template RENDERS
- [x] Ginkgo/Gomega: BDD spec extraction (`Describe`, `Context`, `It`, `BeforeEach`) as test Function nodes with nested hierarchy
- [ ] Parse route configs, `@Input`/`@Output` decorators, React hooks

### 2.10 Agent Experience & Diagnostics

Feedback from real agent sessions revealed gaps that caused agents to get empty results without understanding why.

**A. AhoCorasick silent failure on large projects**

- [x] Removed `ContiguousNFA` constraint from `pass_calls`; auto-selects optimal strategy
- [x] Fixes 0 CALLS/USES edges on projects with 1500+ symbols (e.g., Spring Boot backends)

**B. `index_status` diagnostics for agents**

- [x] Returns `warnings` array when: nodes > 10 but edges = 0, no CALLS/USES edges, Routes exist but no HANDLES_ROUTE edges
- [x] Add warning when no IMPORTS edges exist
- [x] Add warning when project has `.vue` files but no Selector nodes (Vue adapter didn't run)

**C. Go 1.22+ route method detection**

- [x] `parse_method_from_path` extracts HTTP method from embedded path strings (`"GET /users"`)
- [x] Applied to `http.HandleFunc`, `http.Handle`, and `mux.HandleFunc` selector patterns

**D. Dashboard visibility fixes**

- [x] Added gray (#616161) background for `[data-method="ANY"]` route badges
- [x] Added Vue, Ginkgo, Gomega framework badges with devicon icons and CSS classes

**E. Sudo-free install**

- [x] `install.sh` and `Makefile` check `/usr/local/bin` writability before using sudo
- [x] `codesign` tries without sudo first, falls back to sudo

### 2.11 Agent Experience — DTO Resolution, Auto-Linking, Class Details, Angular Architecture

Feedback from real agent sessions revealed four friction points that required extra tool calls.

**A. find_routes returns null DTOs**

- [x] `GraphBuffer::flush()` picks best match (exact name) when suffix-matching finds multiple candidates
- [x] Route nodes store `request_dto_type` / `response_dto_type` in properties during extraction (Java + Kotlin)
- [x] `find_routes` falls back to Route properties when `ACCEPTS_DTO`/`RETURNS_DTO` edges are missing

**B. Projects not auto-linked**

- [x] `index_repository` calls `suggest_project_links` after indexing and auto-links pairs scoring ≥ 0.5
- [x] Auto-linked projects returned in response under `"auto_linked"` field

**C. get_symbol_details minimal for classes**

- [x] `get_symbol_details` includes `"members"` array for Class/Interface (methods with annotations, return types, line ranges)
- [x] Class snippet cap reduced to 50 lines (methods still 150) since members summary provides structural overview

**D. Angular/TS empty architecture layers**

- [x] `pass_angular` sets `"decorator"` and `"layer"` properties on Angular class nodes directly
- [x] `get_architecture` now classifies Angular classes via node-layer first pass (component, service, module, etc.)

---

## Track 3 — Looks Good (UI/UX & Visual Polish)

### 3.1 Dark Mode

~~Canvas hardcodes `#ffffff` background. Node/edge colors are hardcoded Material palette, not SDX tokens.~~

Cosmos renders on a dark background by default. The graph canvas now uses `#0a0a1a`.

- [x] Graph canvas uses dark background (cosmos default)
- [x] Extract sidebar/overlay colors into CSS custom properties for full dark mode
- [x] Add a theme toggle in the header (persist choice in `localStorage`)

### 3.2 Search UX — Highlight Instead of Hide

Current search removes non-matching nodes entirely, losing graph context.

- [x] Add a search mode toggle: **filter** (current) vs **highlight**
- [x] In highlight mode: dim non-matching nodes/edges (alpha) but keep them visible
- [x] Show a match list panel with click-to-focus

### 3.3 Missing Graph Explorer Features

| Feature | Priority | Notes |
|---------|----------|-------|
| ~~Node drag to reposition~~ | ~~High~~ | ✅ Built-in with `force-graph` `enableNodeDrag` |
| Export to PNG | Medium | ✅ `canvas.toDataURL()` with a toolbar button |
| Export to JSON | Medium | ✅ Dump visible subgraph as JSON download |
| Shortest path between two nodes | Medium | ✅ Pick source + target, highlight path |
| Alternative layouts (dagre, hierarchy, circle) | Medium | ✅ Force-directed, DAG, and circle layout options |
| Deep link to node (`?nodeId=`) | Medium | ✅ Focus + open popover on load |
| Lasso / multi-select | Low | ✅ Shift+drag to select a region |
| Community / cluster coloring | Low | ✅ Label propagation algorithm, color clusters |
| Level-of-detail rendering | Low | `force-graph` `nodeCanvasObject` already handles zoom-based label visibility |

### 3.4 Design System Consistency

- [x] Replace custom project cards with `sdx-card` (or wrap with SDX tokens only)
- [x] Replace native `<button class="cypher-chip">` with `sdx-button` in Control
- [x] Use SDX cancel token instead of hardcoded `#d32f2f` on destructive buttons
- [x] Replace emoji in simulation banner ("Neurons connecting…") with an `sdx-icon` + proper label

### 3.5 Responsive & Mobile

- [x] Add a collapsible sidebar (hamburger toggle on narrow viewports)
- [x] Replace fixed `260px` sidebar width with responsive breakpoints
- [x] Add touch pinch-zoom support on the canvas

### 3.6 Accessibility

- [x] Add keyboard navigation for the graph (arrow keys to move between neighbors, Enter to select)
- [x] Add ARIA labels on toolbar buttons and the canvas region
- [x] Provide a screen-reader summary of the visible graph (node/edge counts, selected node details)

---

## Track 4 — Core Parity (Gaps vs C codryn)

Cross-project audit of the codryn codebase. MCP tool surface is at **46 tools**, pipeline passes expanded, and language coverage includes Tier 1 deep walkers (Java, Kotlin, Dart, Lua, Haskell) plus Tier 2 & 3 regex walkers. All parity items are complete as of v0.4.3.

### 4.1 Tree-sitter Walker Expansion

The baseline ships **100+ tree-sitter grammars** with full AST extraction. The Rust port has **14 walkers** (Bash, C, C++, C#, Elixir, JS, PHP, Python, Ruby, Rust, Scala, Swift, TS/TSX) plus dedicated adapters for Java, Kotlin, and Go. All other languages fall back to regex extraction, producing shallower graphs.

**Tier 1 — High-demand languages (most requested by agents):**

- [x] Go walker (replace/augment dedicated `go_adapter.rs` with tree-sitter for deeper extraction)
- [x] Java walker (replace/augment `spring_java.rs` regex with tree-sitter AST)
- [x] Kotlin walker (replace/augment `spring_kotlin.rs` regex with tree-sitter AST)
- [x] Dart walker (`tree-sitter-dart` crate)
- [x] Lua walker (`tree-sitter-lua` crate)
- [x] Haskell walker (`tree-sitter-haskell` crate)

**Tier 2 — Commonly indexed languages:**

- [x] Julia walker (regex-fallback)
- [x] Zig walker (regex-fallback)
- [x] Nim walker (regex-fallback)
- [x] OCaml walker (regex-fallback)
- [x] Perl walker (regex-fallback)
- [x] R walker (regex-fallback)
- [x] Clojure walker (regex-fallback)
- [x] Erlang walker (regex-fallback)
- [x] F# walker (regex-fallback)

**Tier 3 — Long tail (baseline had grammars; codryn detects but does not extract yet):**

- [x] HCL/Terraform (regex-fallback)
- [x] Protobuf (regex-fallback)
- [x] GraphQL (regex-fallback)
- [x] SQL (regex-fallback)
- [x] Fortran, COBOL, Ada, Pascal, Odin, Crystal, GDScript, Gleam, Elm, Nix, Markdown, YAML, JSON, HTML, CSS, SCSS, Svelte, Vue (SFC parsing), Dockerfile, Makefile, CMake

### 4.2 Incremental Indexing

The baseline has `pipeline_incremental.c` — re-indexes only changed files by comparing file hashes against the stored index. The Rust port now supports incremental indexing.

- [x] Store per-file content hash in the graph during indexing
- [x] On re-index, compute hashes for all discovered files and diff against stored hashes
- [x] Only re-extract and re-process changed/added/deleted files
- [x] Surgically remove stale nodes/edges for deleted or changed files before re-inserting
- [x] Benchmark: target <2s incremental re-index on a 10k-file project with 5 changed files

### 4.3 Git History Enrichment (`pass_githistory`)

The baseline runs `git log` to enrich nodes with commit frequency, last-modified dates, and contributor counts. This enables hotspot detection (frequently changed files/functions).

- [x] Run `git log --format` to collect per-file: commit count, last commit date, unique author count
- [x] Store as node properties: `git_commits`, `git_last_modified`, `git_authors`
- [x] Expose in `get_symbol_details` and `get_file_overview` responses
- [x] Add `git_hotspots` query or integrate into `sample_graph` sorting

### 4.4 Usage/Reference Pass (`pass_usages`)

The baseline has a dedicated `pass_usages` that creates USES edges for non-call references (type annotations, variable declarations, constant references). The Rust port only creates USES edges as a fallback in `pass_calls` when a match isn't a function call.

- [x] Dedicated pass that scans for type annotations, variable type references, and constant usage
- [x] Create USES edges from the referencing symbol to the referenced type/constant
- [x] Distinguish from CALLS edges for cleaner `find_references` results

### 4.5 Config Linking (`pass_configlink`)

The baseline links configuration files (`.env`, `application.yml`, `config.json`, etc.) to the code that reads them via environment variable names or config keys.

- [x] Detect config files by name/extension patterns
- [x] Extract config keys from YAML, JSON, TOML, .env, .properties files
- [x] Match config keys against `process.env`, `os.environ`, `@Value`, `viper.Get` patterns in code
- [x] Create CONFIGURES edges from config file nodes to consuming code nodes

### 4.6 Kubernetes Manifest Pass (`pass_k8s`)

The baseline parses Kubernetes manifests (Deployments, Services, ConfigMaps, Ingress) and links them to the application code they deploy.

- [x] Parse K8s YAML manifests for resource metadata (name, kind, namespace, image, ports)
- [x] Create Infrastructure nodes with `infra_type: "kubernetes"` and resource properties
- [x] Link Deployment/Pod specs to Docker images and service names
- [x] Link ConfigMap/Secret references to `pass_configlink` config nodes

### 4.7 Environment Variable Scanning (`pass_envscan`)

The baseline extracts all environment variable accesses across languages and creates a unified view.

- [x] Detect `process.env.X`, `os.environ["X"]`, `os.Getenv("X")`, `System.getenv("X")`, `ENV["X"]` patterns
- [x] Create EnvVar nodes or annotate accessing functions with `env_vars` property
- [x] Cross-reference with `.env` files and K8s ConfigMaps from `pass_configlink`/`pass_k8s`

### 4.8 Git Diff Pass (`pass_gitdiff`)

The baseline enriches the graph with uncommitted change information at the node level, not just file level.

- [x] Run `git diff --name-only HEAD` to get changed files (already done in `detect_changes`)
- [x] Map changed files to affected graph nodes (functions/classes in those files)
- [x] Annotate affected nodes with `has_uncommitted_changes: true`
- [x] Combine with `impact_analysis` to show blast radius of uncommitted work

### 4.9 Channel & Message Queue Extraction

The baseline's `extract_channels.c` detects Go channel operations, message queue publish/subscribe patterns, and event emitters.

- [x] Detect Go channel `make(chan T)`, `ch <- val`, `<-ch` patterns
- [x] Detect message queue patterns: RabbitMQ publish/consume, Kafka producer/consumer, Redis pub/sub
- [x] Detect event emitter patterns: Node.js `EventEmitter.emit`/`.on`, Python signals
- [x] Create SENDS_TO / RECEIVES_FROM edges between producer and consumer functions

### 4.10 Type Assignment & Reference Extraction

The baseline has `extract_type_assigns.c` and `extract_type_refs.c` for building a richer type graph beyond what tree-sitter walkers provide.

- [x] Extract variable type assignments (`let x: Type = ...`, `x = Type()`)
- [x] Extract type references in function signatures, generics, and type aliases
- [x] Feed into `TypeRegistry` for better `pass_calls` disambiguation
- [x] Create TYPE_OF edges from variables to their types

### 4.11 C/C++ Preprocessor Integration

The baseline has `preprocessor.cpp` (using `simplecpp`) for resolving `#include` chains, `#define` macros, and conditional compilation. The Rust port has basic `compile_commands.json` support but no preprocessor.

- [x] Extract `#define` macros as Constant nodes and `#include "..."` as INCLUDES edges
- [x] Integrate a full C preprocessor (e.g., `cc` crate or custom) for `#include` chain resolution
- [x] Resolve `#define` constants and macros for better symbol extraction
- [x] Use `compile_commands.json` include paths for system header resolution
- [x] Handle conditional compilation (`#ifdef`) to avoid indexing dead code paths

### 4.12 Cross-Repository Linking (`pass_cross_repo`)

The baseline has `pass_cross_repo.c` for detecting cross-repository dependencies at the pipeline level (not just the MCP `link_project` tool).

- [x] During indexing, detect references to external packages that match other indexed projects
- [x] Auto-create cross-project CALLS/IMPORTS edges when both sides are indexed
- [x] Use `PackageMap` to resolve `node_modules`, `vendor`, `site_packages` references to linked projects

### 4.13 FastAPI Dependency Injection

The baseline has `pass_fastapi_depends` for extracting FastAPI's `Depends()` injection graph.

- [x] Detect `Depends(function_name)` in FastAPI route handler parameters
- [x] Create INJECTS edges from the dependency function to the route handler
- [x] Chain nested `Depends()` calls into a dependency tree
- [x] Annotate route nodes with their dependency chain

### 4.14 Decorator/Annotation Tag Pass

The baseline's `pass_decorator_tags` (in `pass_enrichment.c`) extracts and normalises decorator/annotation metadata across languages.

- [x] Extract Python `@decorator`, Java/Kotlin `@Annotation`, TypeScript decorators
- [x] Store normalised decorator names in node properties (`decorators: ["Component", "Injectable"]`)
- [x] Enable Cypher queries like `MATCH (n) WHERE n.decorators CONTAINS 'Controller'`
- [x] Already partially done for Angular/Spring — generalised to all languages

---

## Track 5 — Agent-First Features (Beyond Origin)

Features that neither the baseline nor any current tool provides, designed specifically to reduce agent tool calls, context window waste, and wrong-turn decisions. These are informed by real agent session patterns where the graph has the data but the agent can't access it efficiently. All items complete as of v0.4.3.

### 5.1 Semantic Code Search (Embedding-Based)

Agents frequently search for concepts ("error handling logic", "authentication middleware") but `search_graph` only matches symbol names and `search_code` only matches literal strings. Neither finds semantically related code.

- [x] Generate embeddings for function/class docstrings and signatures during indexing
- [x] Store embeddings in a vector column or sidecar SQLite table
- [x] Add `semantic_search` MCP tool: takes a natural language query, returns ranked symbols by cosine similarity
- [x] Use a lightweight local model (e.g., `all-MiniLM-L6-v2`) for embedding generation
- [x] Fall back to `search_graph` when embeddings aren't available-MiniLM-L6-v2` via `ort` / ONNX Runtime) — no API calls
- [ ] Fall back to `search_graph` when embeddings aren't available

### 5.2 Change Impact Preview (`what_if`)

Agents call `impact_analysis` to see blast radius, but they can't preview what would break if they changed a function's signature or removed a parameter. This causes agents to make changes and then discover breakage.

- [x] Add `what_if` MCP tool: takes a symbol + proposed change type (rename, remove, change_signature, move_file)
- [x] For rename: return all call sites that need updating, grouped by file
- [x] For remove: return all direct callers that would break, plus transitive dependents
- [x] For change_signature: return all call sites with current argument patterns
- [x] For move_file: return all import statements that need updating
- [x] Include a `fix_plan` field with concrete file:line edits the agent can apply

### 5.3 Codebase Summary / Onboarding Brief

When an agent first encounters a project, it wastes 3–5 tool calls just orienting: `get_architecture`, `find_entrypoints`, `sample_graph`, `find_routes`. A single "tell me about this project" call would save significant context.

- [x] Add `get_project_summary` MCP tool: returns a structured onboarding brief in one call
- [x] Include: language breakdown, framework detection, architecture layers, top-10 high-centrality symbols, route count, test coverage estimate, entry points, linked projects
- [x] Include: detected patterns (monorepo, microservice, MVC, event-driven, etc.)
- [x] Include: suggested first reads (README, main entry, config files)
- [x] Cache the summary and invalidate on re-index

### 5.4 Dead Code Detection

Agents frequently ask "is this function used anywhere?" and have to call `find_references` per symbol. A bulk dead code report would let agents confidently remove unused code or flag it in reviews.

- [x] Add `find_dead_code` MCP tool: returns symbols with zero incoming CALLS/USES/IMPORTS edges
- [x] Filter out: entry points, test functions, exported API surfaces, framework lifecycle hooks
- [x] Group by file for easy cleanup
- [x] Include confidence level (high = truly unreachable, medium = only referenced via reflection/dynamic dispatch)
- [x] Support scope filter (directory, module, or full project)

### 5.5 Dependency Graph & Circular Dependency Detection

Agents working on refactoring need to understand module-level dependency structure. Currently they'd need multiple Cypher queries to piece this together.

- [x] Add `get_dependency_graph` MCP tool: returns module-to-module IMPORTS edges as an adjacency list
- [x] Detect and flag circular dependencies (A→B→C→A)
- [x] Return topological sort order for safe refactoring sequence
- [x] Support granularity levels: file, folder/module, package
- [x] Include edge weights (number of imports between modules)

### 5.6 Smart Diff Review (`review_changes`)

After an agent makes changes, it has no way to validate those changes against the graph. A review tool would catch issues before the human sees them.

- [x] Add `review_changes` MCP tool: takes a list of changed files (or reads from `git diff`)
- [x] Cross-reference changes against the graph: are all callers of modified functions updated?
- [x] Detect: broken imports (file moved but importers not updated), missing test updates, signature mismatches
- [x] Return a structured review with severity levels (error, warning, info)
- [x] Integrate with `what_if` for each changed symbol

### 5.7 Pattern Detection & Anti-Pattern Flagging

Agents making architectural decisions benefit from knowing existing patterns. Currently they have to infer patterns from reading code.

- [x] Add `detect_patterns` MCP tool: scans the graph for common architectural patterns
- [x] Detect: MVC layers, repository pattern, service layer, factory pattern, singleton, observer/pub-sub
- [x] Detect anti-patterns: god classes (high fan-in + fan-out), circular dependencies, deep inheritance chains, shotgun surgery candidates (one change requires touching many files)
- [x] Return pattern instances with confidence scores and involved symbols
- [x] Include recommendations ("Consider extracting interface for class X with 47 dependents")

### 5.8 Test Gap Analysis

Agents writing tests need to know what's already covered and what's missing. Currently they call `find_tests_for_target` per symbol.

- [x] Add `test_coverage_map` MCP tool: returns a project-wide map of symbols → their test files
- [x] Identify untested public functions/methods (no test file references them)
- [x] Rank untested symbols by risk: high fan-in symbols without tests are highest priority
- [x] Group by module for systematic test writing
- [x] Include: test-to-source ratio per module, modules with zero test coverage

### 5.9 Multi-Hop Context Gathering (`get_context_for_task`)

The most common agent pattern is: "I need to modify function X, give me everything I need to know." This currently takes 3–4 calls (`get_symbol_details` + `find_references` + `get_code_snippet` for each caller). A single task-oriented context call would collapse this.

- [x] Add `get_context_for_task` MCP tool: takes a symbol + task type (modify, debug, test, document)
- [x] For `modify`: return symbol details + all callers with their snippets + related tests + import chain
- [x] For `debug`: return symbol details + call chain (callers and callees 2 levels deep) + recent git changes
- [x] For `test`: return symbol details + existing tests + similar tested functions as examples + dependencies to mock
- [x] For `document`: return symbol details + all public API consumers + usage examples from tests
- [x] Cap total response size to avoid context window overflow (configurable, default 8K tokens)

### 5.10 Stale Index Detection & Auto-Refresh

Agents get wrong results when the index is stale but don't know it. The auto-indexer checks timestamps but doesn't communicate staleness to the agent.

- [x] Add `staleness_score` to every query response: percentage of indexed files that have changed on disk since last index
- [x] When staleness > 20%, include a `"warning": "index may be stale"` in responses
- [x] Add `freshness_check` MCP tool: returns per-file staleness without triggering a full re-index
- [x] Auto-trigger incremental re-index (4.2) when staleness > threshold and query involves changed files
- [x] Track which query results might be affected by stale files

### 5.11 Natural Language to Cypher

Agents sometimes need complex graph queries but struggle to write correct Cypher. A natural language interface would make the graph accessible without Cypher knowledge.

- [x] Add `ask_graph` MCP tool: takes a natural language question about the codebase
- [x] Translate to Cypher using the graph schema as context (node labels, edge types, property names)
- [x] Use a template-based approach for common patterns: "who calls X", "what imports Y", "show me all controllers"
- [x] Fall back to `search_graph` for questions that don't map to graph queries
- [x] Return both the Cypher query (for learning) and the results

### 5.12 Batch Symbol Resolution

Agents frequently need details on multiple symbols (e.g., all methods of a class, all route handlers). Currently this requires N sequential `get_symbol_details` calls, wasting round-trips.

- [x] Add `get_symbols_batch` MCP tool: takes an array of symbol names or qualified names
- [x] Return details for all symbols in a single response
- [x] Support filter: "all methods of class X", "all functions in file Y", "all symbols matching pattern Z"
- [x] Include relationship edges between the returned symbols (internal call graph)
- [x] Cap at 50 symbols per request to keep response size manageable

### 5.13 Refactoring Plan Generator

When agents need to do large refactors (extract module, split class, move function), they currently plan manually by reading code. A graph-aware planner would produce safer refactoring steps.

- [x] Add `plan_refactoring` MCP tool: takes a refactoring type + target
- [x] Supported refactoring types: `extract_module`, `split_class`, `move_function`, `inline_function`, `extract_interface`
- [x] Return an ordered list of steps with file:line edits, respecting dependency order
- [x] Flag risks: circular dependency introduction, public API breakage, test breakage
- [x] Include rollback information (what to undo if a step fails)

### 5.14 Conversation-Aware Query Cache

Agents in a single session often query the same symbols repeatedly (e.g., checking a function's callers, then its details, then its callers again after a change). Caching would reduce latency and token waste.

- [x] Add session-scoped query cache keyed by (tool_name, project, args_hash)
- [x] Return cached results with `"cached": true` flag when the index hasn't changed
- [x] Invalidate cache entries when: re-index completes, `detect_changes` shows affected files
- [x] Track cache hit rate in `diagnostics` output
- [x] Add `clear_cache` MCP tool for explicit invalidation

### 5.15 Error Chain Tracing

When agents debug errors, they need to trace how exceptions propagate through the call chain. The graph has the call edges but no tool surfaces this as an error-flow view.

- [x] Add `trace_error_flow` MCP tool: takes a function that throws/raises and traces all callers that don't catch
- [x] Detect try/catch, try/except, Result/Option handling patterns per language
- [x] Return the "uncaught chain": functions where the error propagates without handling
- [x] Identify the first handler in each call path
- [x] Useful for: "if this function throws, what breaks?"

### 5.16 API Surface Documentation

Agents writing documentation or building integrations need to know the public API surface. Currently they'd need to query for exported symbols across all files.

- [x] Add `get_api_surface` MCP tool: returns all exported/public symbols grouped by module
- [x] Include: function signatures, parameter types, return types, docstrings
- [x] Filter by: module path, symbol type (functions only, classes only), decorator (`@api`, `@public`)
- [x] Generate OpenAPI-compatible schema for HTTP route handlers
- [x] Support diff mode: "what changed in the API since last index"

---

## Suggested Priority Order

Start with quick wins that compound, then move to the bigger features.

| Phase | Items | Effort |
|-------|-------|--------|
| **Phase 1 — Quick wins** | 1.6, 1.7, 2.6, 2.8, 2.5 (frontend error surfacing) | ✅ Done |
| **Phase 2 — Core performance** | 1.1, 1.2, 1.4, 1.8, 1.9, 1.10 | ✅ Done |
| **Phase 2b — Index speed** | 1.11 (flush consolidation), 1.12 (batch QN), 1.15 (SQLite tuning), 1.17 (progress) | ✅ Done |
| **Phase 2c — Incremental speed** | 1.13 (skip unchanged), 1.14 (lazy types), 1.16 (parallel Java/Go) | ✅ Done |
| **Phase 3 — Cypher & tools** | 2.1, 2.2, 2.4 | ✅ Done |
| **Phase 3b — Framework support** | 2.9E (Vue, Ginkgo/Gomega), 2.10 (agent diagnostics, Go routes) | ✅ Done |
| **Phase 3c — Agent experience** | 2.11 (DTO resolution, auto-linking, class details, Angular arch) | ✅ Done |
| **Phase 4 — UI polish** | 3.1, 3.2, 3.4, 3.3 (node drag + export + layouts + path + lasso + community) | ✅ Done |
| **Phase 5 — Scale & robustness** | 1.3, 2.3, 2.5, 2.7, CI | ✅ Done |
| **Phase 6 — Advanced UX** | 3.3 (layouts, path, clusters), 3.5, 3.6 | ✅ Done |
| **Phase 7 — Origin parity (high)** | 4.1 Tier 1 (Go walker), 4.3 (git history), 4.11 (C/C++ preprocessor), 4.13 (FastAPI) | ✅ Done |
| **Phase 7b — Agent-first (high)** | 5.3 (summary), 5.9 (context), 5.12 (batch), 5.10 (staleness) | ✅ Done |
| **Phase 7c — Install UX** | 9.1 (interactive install), 9.2 (workspace activation), 9.3 (lite steering), 9.5 (mcp.json mgmt) | ✅ Done |
| **Phase 8 — Origin parity (medium)** | 4.4 (usages), 4.5 (configlink), 4.6 (k8s), 4.7 (envscan) | ✅ Done |
| **Phase 8b — Agent-first (medium)** | 5.2 (what-if), 5.4 (dead code), 5.5 (deps), 5.6 (review), 5.8 (test gaps) | ✅ Done |
| **Phase 9 — Origin parity (depth)** | 4.1 Tier 1+2+3 walkers, 4.8–4.10, 4.12, 4.14 | ✅ Done |
| **Phase 9b — Agent-first (advanced)** | 5.1 (semantic), 5.7 (patterns), 5.11 (NL→Cypher), 5.13–5.16 | ✅ Done |
| **Phase 9c — Install UX (extended)** | 9.4 (CLI-first mode), 9.6 (uninstall) | ✅ Done |
| **Phase 10 — Reliability** | 6.1 (shutdown), 6.2 (pool), 6.4 (logging), 6.5 (health), 6.6 (recovery) | ✅ Done |
| **Phase 10b — Ops tooling** | 6.3 (memory), 6.7 (backup), 6.8 (rate limit) | ✅ Done |
| **Phase 11 — Core Intelligence** | 8.3 (confidence), 8.1 (validation), 8.2 (dedup), index runs, 8.4 (snapshots) | ✅ Done |
| **Phase 11b — Metrics & CLI** | 8.5 (complexity), 8.6 (doc coverage), 8.7 (deps), CLI UX, query improvements | ✅ Done |
| **Phase 12 — Distribution** _(deferred)_ | 7.1 (releases), 7.2 (Homebrew/cargo), 7.3 (Docker), 7.5 (plugins), 7.6 (LSP), 7.7 (VS Code) | Deferred |

The MCP server has no signal handling — a SIGTERM during indexing can leave the SQLite database in a partially-written state (uncommitted WAL frames, incomplete flush phases).

- [x] Register SIGTERM/SIGINT handlers via `tokio::signal` or `ctrlc` crate
- [x] On signal: stop accepting new MCP requests, drain in-flight handlers
- [x] Wait for any active `Pipeline::run()` to complete its current flush phase (with 30s timeout)
- [x] Force-exit after timeout with a warning log
- [x] Ensure all SQLite transactions are committed or rolled back before exit

### 6.2 Connection Pooling for SQLite

`codryn-mcp/src/lib.rs` guards the `Store` behind a single `Arc<Mutex<Store>>`. Every MCP tool call — including read-only queries — contends on this lock. SQLite WAL mode supports concurrent readers with a single writer.

- [x] Replace `Arc<Mutex<Store>>` with a connection pool (e.g., `r2d2-sqlite` or custom pool)
- [x] Configure pool: 4 reader connections + 1 writer connection (configurable)
- [x] Read-only tools (`find_symbol`, `search_graph`, `get_symbol_details`) use reader connections
- [x] Write tools (`index_repository`, `manage_adr`, `ingest_traces`) acquire the writer
- [x] Add pool metrics to `diagnostics` output: active/idle connections, wait time

### 6.3 Memory Pressure Management During Indexing

On large projects (50K+ files), the `FileCache`, `GraphBuffer`, and `TypeRegistry` can collectively consume several GB. No backpressure mechanism exists — the process just grows until OOM-killed.

- [x] Add a configurable memory limit (default: 2GB, via `CODRYN_MAX_MEMORY_MB` env var or config)
- [x] Monitor RSS via `/proc/self/statm` (Linux) or `mach_task_info` (macOS) during pipeline runs
- [x] When usage exceeds 80% of limit: flush `GraphBuffer` early, evict `FileCache` LRU entries
- [x] Log memory high-water-mark at end of each index run
- [x] Add `memory_usage_mb` to `index_repository` response

### 6.4 Structured Logging with Configurable Levels

Current logging uses `tracing` with `env-filter` but defaults to unstructured text on stderr. No JSON output for log aggregation, and per-module filtering requires knowing internal crate names.

- [x] Add `CODRYN_LOG_FORMAT=json` support via `tracing-subscriber`'s JSON layer
- [x] Include structured fields: timestamp (ISO 8601), level, module path, span context, duration for timed operations
- [x] Document per-module filter syntax in README (e.g., `CODRYN_LOG_LEVEL=codryn_pipeline=debug,codryn_store=warn`)
- [x] Add request-scoped span for each MCP tool call (tool name, project, duration)
- [x] Default to `info` level with human-readable format when no env vars are set

### 6.5 Health Check Endpoint

No way to determine if the MCP server is alive and functional without sending a real tool call. Monitoring systems need a lightweight probe.

- [x] Add `health_check` MCP tool: returns uptime, indexed project count, store status, active index runs
- [x] Response time target: <100ms (no heavy queries)
- [x] Include `store_ok: bool` (can open and query the database)
- [x] Include `version` field for deployment tracking
- [x] Optionally expose as HTTP endpoint on the UI dashboard port (`/health`)

### 6.6 Crash Recovery (Resume Interrupted Indexing)

If the server crashes mid-index (power loss, OOM kill, segfault in tree-sitter), the next index must start from scratch. On a 10-minute index, this wastes significant time.

- [x] Record checkpoint in a `_index_progress` table: project, phase name, phase index, timestamp, files_processed
- [x] On startup, check for incomplete checkpoints
- [x] Offer resume: skip completed phases, roll back partial phase data, restart from interrupted phase
- [x] If partial flush left orphan nodes/edges, clean them up before resuming
- [x] Add `--resume` flag to `index_repository` tool and CLI `index` command

### 6.7 Database Backup & Restore

The graph database (`graph.db`) can grow to hundreds of MB. No built-in way to back it up safely (copying while the server writes risks corruption) or restore from backup.

- [x] Add `codryn backup [--output path]` CLI command using SQLite online backup API
- [x] Backup creates a consistent snapshot without blocking reads
- [x] Add `codryn restore <backup-file>` CLI command (requires server to be stopped)
- [ ] Add `--compress` flag for zstd-compressed backups
- [ ] Include backup metadata: creation time, source version, project count, total nodes/edges

### 6.8 Rate Limiting for Expensive Queries

A misbehaving agent (or infinite loop) can saturate the server with expensive operations like `trace_call_path` (BFS), `impact_analysis` (multi-hop traversal), or `query_graph` (arbitrary Cypher).

- [x] Track per-tool execution time; classify queries >500ms as "expensive"
- [x] Implement sliding window rate limit: max 10 expensive queries per 60s per session
- [x] Exempt `index_repository`, `health_check`, and `detect_changes` from limits
- [x] Return structured error with `retry_after_seconds` when limit is hit
- [ ] Add rate limit stats to `diagnostics` output
- [x] Make thresholds configurable via config file (6.9)

---

## Track 7 — Developer Experience & Distribution

Making the tool easy to install, configure, and extend. Currently requires building from source on macOS (the install script only handles macOS). These items expand reach to all platforms and improve the day-to-day developer workflow.

### 7.1 Cross-Platform Binary Releases

The `install.sh` script builds from source and only works on macOS. No pre-built binaries exist for any platform.

- [ ] Add GitHub Actions release workflow triggered on `v*` tags
- [ ] Build matrix: macOS (aarch64 + x86_64), Linux (x86_64 + aarch64), Windows (x86_64)
- [ ] Produce artifacts: binary, SHA-256 checksum, changelog excerpt from `CHANGELOG.md`
- [ ] Run `cargo test` on each platform before publishing
- [ ] Use `cross` or `cargo-zigbuild` for Linux cross-compilation from CI
- [ ] Sign macOS binaries with ad-hoc signature (or Apple Developer ID if available)

### 7.2 Homebrew Formula / Cargo Install Support

No package manager integration — users must clone and build manually.

- [ ] Publish to crates.io as `codryn` (or `codryn`) for `cargo install` support
- [ ] Create a Homebrew tap (`homebrew-codryn`) with a formula pointing to GitHub Release binaries
- [ ] Auto-update the formula on new releases via CI (update SHA and version in formula)
- [ ] Add `cargo binstall` metadata for binary installation without compilation
- [ ] Document installation methods in README (Homebrew, cargo install, binary download, Docker)

### 7.3 Docker Image

No containerized deployment option. Users running in Docker-based dev environments or CI can't use the tool without building a custom image.

- [ ] Multi-stage Dockerfile: builder (Rust + dependencies) → runtime (minimal debian-slim or alpine)
- [ ] Runtime image includes: binary, tree-sitter grammars (bundled), git (for `detect_changes`)
- [ ] Expose: stdio for MCP transport, port 3000 for UI dashboard
- [ ] Volume mount at `/data` for persistent graph storage
- [ ] Publish to `ghcr.io` on each release
- [ ] Add `docker-compose.yml` example for local development

### 7.4 Configuration File Support (TOML)

All configuration is via environment variables or CLI flags. No persistent configuration file for server settings.

- [x] Read `~/.config/codryn/config.toml` on startup (XDG-compliant path)
- [x] Support settings: `store_path`, `log_level`, `log_format`, `pool_size`, `staleness_threshold_secs`, `max_memory_mb`, `rate_limit.*`, `ui_port`
- [x] Environment variables override config file values (env takes precedence)
- [ ] Add `codryn config init` CLI command to generate a default config file with comments
- [x] Validate config on load; log warnings for unknown keys, use defaults for invalid values
- [ ] Support project-level `.codryn.toml` for per-project overrides (e.g., custom ignore patterns)

### 7.5 Plugin/Extension System for Custom Passes

Adding a new indexing pass requires modifying `codryn-pipeline` source code. No way for users to add domain-specific extraction without forking.

- [ ] Define a stable plugin ABI: `extern "C"` functions for `register()`, `run_pass()`, `cleanup()`
- [ ] Plugin receives: file list, read-only Store access, GraphBuffer for adding nodes/edges
- [ ] Load plugins from `~/.config/codryn/plugins/` (or configured directory)
- [ ] Plugin metadata: name, version, phase (after-extraction, after-edges, after-enrichment), dependencies
- [ ] Catch panics in plugin code; log error and continue with remaining passes
- [ ] Document plugin API with an example plugin (e.g., custom TODO/FIXME extraction)

### 7.6 LSP Integration

The knowledge graph is only accessible via MCP tools. IDE users without MCP support can't benefit from graph-powered navigation.

- [ ] Add optional LSP server mode (`codryn lsp --port 9257`)
- [ ] Implement `textDocument/definition`: resolve symbol via graph, return file:line location
- [ ] Implement `textDocument/references`: return all CALLS/IMPORTS/USES edges as locations
- [ ] Implement `workspace/symbol`: graph-powered symbol search with ranking
- [ ] Implement `textDocument/hover`: show symbol details (callers count, complexity, layer)
- [ ] Auto-start LSP alongside MCP server when configured

### 7.7 VS Code Extension

No IDE integration beyond MCP. A VS Code extension would make the graph accessible to developers who don't use AI agents.

- [ ] Tree view panel: project architecture (modules → classes → functions)
- [ ] Click symbol → show details panel (callers, callees, imports, complexity)
- [ ] Command palette: `CBM: Find Symbol`, `CBM: Impact Analysis`, `CBM: Find References`
- [ ] CodeLens annotations: show caller count above functions
- [ ] Status bar: index freshness indicator, click to re-index
- [ ] Connect to running MCP server via stdio or TCP socket

### 7.8 CLI Improvements

The CLI (`codryn-cli`) has basic `install`, `update`, `doctor`, `version` commands but no interactive mode, no progress bars, and plain text output.

- [ ] Add progress bars for `index` command using `indicatif` crate (files/s, current phase, ETA)
- [ ] Add `codryn query` subcommand: interactive REPL for Cypher queries with syntax highlighting
- [ ] Add `codryn tools` subcommand: run any MCP tool from the command line with JSON output
- [ ] Colored table output for `list-projects`, `find-symbol`, `find-references` (using `comfy-table` or `tabled`)
- [ ] Add `--json` flag to all commands for machine-readable output
- [ ] Generate shell completions: `codryn completions bash/zsh/fish`
- [ ] Add `codryn stats` command: project summary (nodes, edges, languages, last indexed)

---

## Track 8 — Data Quality & Intelligence

Improving the accuracy, consistency, and richness of the knowledge graph. The current graph is structurally correct but lacks quality signals, historical context, and self-healing capabilities.

### 8.1 Graph Consistency Validation

No mechanism to detect structural issues in the graph. Interrupted indexes, bugs in passes, or schema migrations can leave orphan nodes, dangling edges, or duplicate entries.

- [x] Add `codryn validate` CLI command (and `validate_graph` MCP tool)
- [x] Check for: orphan nodes (zero edges), dangling edges (source/target node missing), duplicate qualified names within a project
- [x] Check for: nodes missing required properties (label, qualified_name, file_path), edges with invalid types
- [x] Report: issue type, count, example node/edge IDs, suggested fix
- [x] Add `--fix` flag: remove dangling edges, merge duplicates, add missing properties with defaults
- [x] Run validation automatically after each index (log warnings only, don't auto-fix)

### 8.2 Duplicate Node Detection and Merging

Re-indexing can create duplicate nodes when qualified name resolution changes between runs (e.g., a file is renamed but the old node persists). The `UPSERT` logic handles exact matches but not near-duplicates.

- [x] Detect duplicates: same `qualified_name` + same `project_id` but different `node_id`
- [x] Detect near-duplicates: same `name` + same `file_path` + same `label` (likely renamed)
- [x] Merge strategy: keep the node with the most recent `indexed_at`, redirect all edges to survivor
- [x] Log merged nodes for audit trail
- [x] Add `deduplicate` method to Store, callable from CLI and post-index hook

### 8.3 Confidence Scoring on Edges

All edges are treated equally, but AST-derived edges (from tree-sitter walkers) are far more reliable than regex-derived edges (from `pass_calls` pattern matching or `pass_semantic`). Agents have no way to filter by reliability.

- [x] Add `confidence` column to edges table (REAL, 0.0–1.0)
- [x] Scoring rules:
  - Tree-sitter AST extraction (walker-derived): 0.95
  - Dedicated adapter (Spring, Angular, Vue, Go): 0.85
  - AhoCorasick pattern match (`pass_calls`): 0.70
  - Regex pattern match (`pass_semantic`, `pass_rest_contracts`): 0.60
  - Heuristic/fallback: 0.40
- [x] Add `min_confidence` parameter to `find_references`, `impact_analysis`, `trace_call_path`
- [x] Include `confidence` in edge results for `query_graph` Cypher responses
- [x] Default: return all edges (no filter); agents can opt into high-confidence-only mode

### 8.4 Historical Graph Snapshots (Diff Between Index Runs)

No way to see how the codebase structure evolved. Each index overwrites the previous state completely.

- [x] Add `_snapshots` table: project_id, timestamp, total_nodes, total_edges, per-label counts, content_hash
- [x] Record a snapshot at the end of each successful Index_Run
- [x] Add `codryn diff [--from <timestamp>] [--to <timestamp>]` CLI command
- [x] Diff output: added nodes/edges, removed nodes/edges, modified nodes (property changes)
- [x] Retain last 10 snapshots per project (configurable via `snapshot_retention` in config)
- [x] Add `get_graph_diff` MCP tool for agents to detect structural drift

### 8.5 Code Complexity Metrics Integration

Function nodes have `line_count` but no complexity metrics. Agents identifying refactoring candidates need cyclomatic/cognitive complexity to prioritize.

- [x] Compute cyclomatic complexity during tree-sitter extraction (count: if, else if, while, for, case, &&, ||, catch, ternary)
- [x] Compute cognitive complexity using Sonar's algorithm (nesting-aware, increment for breaks in linear flow)
- [x] Store as node properties: `cyclomatic_complexity` (int), `cognitive_complexity` (int)
- [x] Languages supported: all tree-sitter walker languages (14+); skip for regex-extracted functions
- [x] Add complexity to `get_symbol_details` response
- [x] Add `--sort-by complexity` to `sample_graph` and `find_dead_code`
- [x] Add `codryn complexity [--threshold 15]` CLI command to list high-complexity functions

### 8.6 Documentation Coverage Scoring

No visibility into which public symbols have documentation. Agents writing docs don't know where to start.

- [x] During extraction, detect doc comments: `///` (Rust), `/**` (Java/JS/TS), `#` (Python), `""" """` (Python docstrings)
- [x] Store `has_docs: bool` and `doc_lines: int` on symbol nodes
- [x] Add `codryn doc-coverage [--module path]` CLI command: percentage of public symbols with docs, grouped by module
- [x] Flag modules below 50% coverage
- [x] Add `doc_coverage` field to `get_architecture` response (per-module percentage)
- [x] Add `--undocumented` filter to `get_api_surface` (Track 5.16) when implemented

### 8.7 Dependency Freshness Checking

No awareness of whether project dependencies are up-to-date. Agents and developers must manually check each package manager.

- [x] Add `codryn deps [--check]` CLI command
- [x] Parse manifests: `Cargo.toml`, `package.json`, `go.mod`, `requirements.txt`, `pyproject.toml`, `pom.xml`, `build.gradle`
- [x] For `--check`: query registry APIs (crates.io, npm, pkg.go.dev, PyPI, Maven Central) for latest versions
- [x] Categorize: up-to-date, patch available, minor available, major available, deprecated/yanked
- [x] Output as table (default) or JSON (`--json`)
- [x] Cache registry responses for 1 hour to avoid repeated API calls
- [x] If offline: report declared versions only, skip freshness check

---

## Track 9 — Installation & Activation UX

The current `install` command takes an opinionated "install everything everywhere" approach — global steering files, skill files, and mcp.json modifications across all detected IDEs. This is too aggressive as a default. Users doing design/architecture work (not always code repos) get irrelevant steering injected into every conversation, wasting context tokens. The setup should respect user choice about where and when to activate.

### 9.1 Interactive Install Flow

The `codryn install` command currently auto-installs globally without asking. It should be interactive with sensible defaults.

- [x] Prompt: "Where to install?" → global / workspace-only / both (default: workspace-only)
- [x] Prompt: "Which IDEs to configure?" → auto-detect installed IDEs, let user pick (checkboxes)
- [x] Prompt: "Install steering/skill files?" → yes / no / workspace-only (default: workspace-only)
- [x] Prompt: "Steering intensity?" → full (current MANDATORY tone) / lite (available-if-needed) / none
- [x] Add `--non-interactive` flag that uses defaults for CI/scripting
- [x] Add `--dry-run` flag to preview what would be installed without writing files
- [x] Store user preferences in `~/.config/codryn/install-preferences.toml` for future `codryn update` runs

### 9.2 Workspace-Level Activation (Default)

Instead of global steering that fires on every conversation regardless of context, the default should be workspace-level activation that only applies when working in an indexed project.

- [x] Default install target: workspace steering in the current project (not global-only)
- [x] Only install global steering when user explicitly opts in
- [ ] `index_repository` auto-writes workspace steering on first index: when a project is indexed for the first time, write the codryn steering file into the workspace (the trigger for "this project has a knowledge graph" becoming true)
- [x] Add `codryn activate` command: installs steering + mcp.json in the current workspace
- [x] Add `codryn deactivate` command: removes steering + mcp.json from the current workspace
- [x] `codryn activate --global` / `codryn deactivate --global` for explicit global management
- [x] Track activation state per-workspace in `~/.config/codryn/workspaces.toml`

### 9.3 Lite Steering Mode

The current steering file uses MANDATORY/MUST language that forces the agent to use graph tools for every code lookup. This wastes tokens when the user is doing non-code work (design docs, architecture discussions, etc.). A "lite" mode would make the tools available without forcing their use.

- [x] Create two steering templates: `full` (current behavior) and `lite`
- [x] Lite template: ~10 lines, states "codryn tools are available for code discovery" without MANDATORY directives
- [x] Lite template omits: analytics requirements, tool-ordering rules, "DO NOT use grep" restrictions
- [x] Default to `lite` for global installs, `full` for workspace installs in indexed projects
- [x] Add `codryn steering --mode lite|full` to switch an existing installation
- [ ] Allow per-workspace override: `.codryn.toml` with `steering_mode = "lite"` or `"full"`

### 9.4 CLI-First Mode (Token-Saving Alternative)

The MCP server + steering file approach keeps a persistent connection and instructs the agent to use graph tools for every code lookup. A CLI-first approach would let the agent invoke `codryn` commands only when it decides graph queries are useful — saving tokens by not loading steering instructions into every conversation.

- [x] Add `codryn query <tool-name> [--json] [args...]` CLI command: runs any MCP tool as a one-shot CLI call
- [x] Examples: `codryn query find-symbol OrderHandler`, `codryn query impact-analysis --name OrderHandler`
- [x] Output: JSON by default (machine-readable for agents), table format with `--table`
- [x] No persistent process needed — starts, queries the SQLite store, exits
- [x] Document as alternative to MCP server for agents that support shell tool calls
- [x] Add `codryn install --mode cli` that skips mcp.json configuration entirely, only installs the binary
- [x] Steering file for CLI mode: minimal, just documents available `codryn query` commands

### 9.5 Selective mcp.json Management

The current install modifies `mcp.json` for all detected IDEs without asking. Users with existing MCP configurations may not want automatic modifications.

- [x] Before modifying any `mcp.json`, show the proposed change and ask for confirmation
- [x] Support `--skip-mcp-config` flag to install steering/skills without touching mcp.json
- [x] Add `codryn mcp-config show` to display what would be added to mcp.json (for manual copy-paste)
- [x] Add `codryn mcp-config add [--ide <target>]` for targeted agent configuration
- [x] Add `codryn mcp-config remove [--ide <target>]` to cleanly remove the server entry
- [x] Never overwrite existing mcp.json entries — merge or warn on conflicts

### 9.6 Uninstall / Clean Removal

No `codryn uninstall` command exists. Users who want to remove the tool must manually hunt down steering files, skill files, and mcp.json entries across multiple locations.

- [x] Add `codryn uninstall` command: removes all installed artifacts (steering, skills, mcp.json entries)
- [x] Show what will be removed and ask for confirmation
- [x] Support `--keep-data` flag to preserve the graph database while removing IDE integration
- [x] Support `--workspace-only` to remove from current workspace without touching global config
- [x] Log all removed files for audit

---

## Updated Priority Order

| Phase | Items | Effort |
|-------|-------|--------|
| **Phase 1 — Quick wins** | 1.6, 1.7, 2.6, 2.8, 2.5 (frontend error surfacing) | ✅ Done |
| **Phase 2 — Core performance** | 1.1, 1.2, 1.4, 1.8, 1.9, 1.10 | ✅ Done |
| **Phase 2b — Index speed** | 1.11 (flush consolidation), 1.12 (batch QN), 1.15 (SQLite tuning), 1.17 (progress) | ✅ Done |
| **Phase 2c — Incremental speed** | 1.13 (skip unchanged), 1.14 (lazy types), 1.16 (parallel Java/Go) | ✅ Done |
| **Phase 3 — Cypher & tools** | 2.1, 2.2, 2.4 | ✅ Done |
| **Phase 3b — Framework support** | 2.9E (Vue, Ginkgo/Gomega), 2.10 (agent diagnostics, Go routes) | ✅ Done |
| **Phase 3c — Agent experience** | 2.11 (DTO resolution, auto-linking, class details, Angular arch) | ✅ Done |
| **Phase 4 — UI polish** | 3.1, 3.2, 3.4, 3.3 (node drag + export + layouts + path + lasso + community) | ✅ Done |
| **Phase 5 — Scale & robustness** | 1.3, 2.3, 2.5, 2.7, CI | ✅ Done |
| **Phase 6 — Advanced UX** | 3.3 (layouts, path, clusters), 3.5, 3.6 | ✅ Done |
| **Phase 7 — Origin parity (high)** | 4.1 Tier 1 (Go walker), 4.3 (git history), 4.11 (C/C++ preprocessor), 4.13 (FastAPI) | ✅ Done |
| **Phase 7b — Agent-first (high)** | 5.3 (summary), 5.9 (context), 5.12 (batch), 5.10 (staleness) | ✅ Done |
| **Phase 7c — Install UX** | 9.1 (interactive install), 9.2 (workspace activation), 9.3 (lite steering), 9.5 (mcp.json mgmt) | ✅ Done |
| **Phase 8 — Origin parity (medium)** | 4.4 (usages), 4.5 (configlink), 4.6 (k8s), 4.7 (envscan) | ✅ Done |
| **Phase 8b — Agent-first (medium)** | 5.2 (what-if), 5.4 (dead code), 5.5 (deps), 5.6 (review), 5.8 (test gaps) | ✅ Done |
| **Phase 9 — Origin parity (depth)** | 4.1 Tier 1+2+3 walkers, 4.8–4.10, 4.12, 4.14 | ✅ Done |
| **Phase 9b — Agent-first (advanced)** | 5.1 (semantic), 5.7 (patterns), 5.11 (NL→Cypher), 5.13–5.16 | ✅ Done |
| **Phase 9c — Install UX (extended)** | 9.4 (CLI-first mode), 9.6 (uninstall) | ✅ Done |
| **Phase 10 — Reliability** | 6.1 (shutdown), 6.2 (pool), 6.4 (logging), 6.5 (health), 6.6 (recovery) | ✅ Done |
| **Phase 10b — Ops tooling** | 6.3 (memory), 6.7 (backup), 6.8 (rate limit) | ✅ Done |
| **Phase 11 — Core Intelligence** | 8.3 (confidence), 8.1 (validation), 8.2 (dedup), index runs, 8.4 (snapshots) | ✅ Done |
| **Phase 11b — Metrics & CLI** | 8.5 (complexity), 8.6 (doc coverage), 8.7 (deps), CLI UX, query improvements | ✅ Done |
| **Phase 12 — Distribution** _(deferred)_ | 7.1 (releases), 7.2 (Homebrew/cargo), 7.3 (Docker), 7.5 (plugins), 7.6 (LSP), 7.7 (VS Code) | Deferred |

