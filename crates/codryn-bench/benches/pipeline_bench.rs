//! Performance benchmarks for the codryn pipeline.
//!
//! Benchmarks cover:
//! - `bench_batch_insert`: INSERT...RETURNING throughput on 10k-node project
//! - `bench_qn_resolution`: Qualified name resolution on 180K-edge project (target <500ms)
//! - `bench_incremental_reindex`: 5 changed files in 1,600-file project (target <10s)
//! - `bench_page_size`: Comparing page_size 4096 vs 8192 on 10k-node project
//! - `bench_java_extraction`: 500+ Java file extraction (target <60s)
//!
//! Run with: `cargo bench -p codryn-bench`

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};

use codryn_bench::{generate_java_source, generate_test_nodes};
use codryn_discover::Language;
use codryn_graph_buffer::GraphBuffer;
use codryn_store::{FileHash, Node, Project, Store};

/// Benchmark: INSERT...RETURNING throughput on a 10k-node project.
///
/// Measures the rows/second throughput of batch node insertion using
/// the INSERT...RETURNING id optimization.
fn bench_batch_insert(c: &mut Criterion) {
    let mut group = c.benchmark_group("batch_insert");
    let batch_sizes: &[usize] = &[100, 500, 1000, 5000, 10000];

    for &size in batch_sizes {
        group.throughput(Throughput::Elements(size as u64));
        group.bench_with_input(BenchmarkId::from_parameter(size), &size, |b, &size| {
            b.iter_with_setup(
                || {
                    let store = Store::open_in_memory().unwrap();
                    store
                        .upsert_project(&Project {
                            name: "bench_project".to_string(),
                            indexed_at: "2024-01-01T00:00:00Z".to_string(),
                            root_path: "/bench".to_string(),
                        })
                        .unwrap();
                    store.enable_bulk_indexing_mode().unwrap();
                    let nodes = generate_test_nodes("bench_project", size);
                    (store, nodes)
                },
                |(store, nodes)| {
                    store.insert_nodes_batch(&nodes).unwrap();
                },
            );
        });
    }
    group.finish();
}

/// Benchmark: Qualified name resolution on a project with 180K edges.
///
/// Target: <500ms for full QN resolution pass.
/// Simulates the GraphBuffer flush where edges reference nodes by qualified name
/// and must be resolved to numeric IDs.
fn bench_qn_resolution(c: &mut Criterion) {
    let mut group = c.benchmark_group("qn_resolution");
    // Use a longer measurement time for this heavier benchmark
    group.measurement_time(std::time::Duration::from_secs(15));
    group.sample_size(10);

    // Setup: create a store with 10k nodes (simulating a large project)
    // and prepare 180K edge references by qualified name
    let node_count = 10_000;
    let edge_count = 180_000;

    group.bench_function("180k_edges", |b| {
        b.iter_with_setup(
            || {
                let store = Store::open_in_memory().unwrap();
                let project = "qn_bench";
                store
                    .upsert_project(&Project {
                        name: project.to_string(),
                        indexed_at: "2024-01-01T00:00:00Z".to_string(),
                        root_path: "/bench".to_string(),
                    })
                    .unwrap();
                store.enable_bulk_indexing_mode().unwrap();

                // Insert nodes
                let nodes = generate_test_nodes(project, node_count);
                store.insert_nodes_batch(&nodes).unwrap();
                store.disable_bulk_indexing_mode().unwrap();

                // Prepare a GraphBuffer with edges referencing nodes by QN
                let mut buf = GraphBuffer::new(project);
                // Seed the buffer with existing node IDs from the store
                buf.seed_ids_from_store(&store).unwrap();

                // Add edges by qualified name (simulating 180K edges)
                for i in 0..edge_count {
                    let src_idx = i % node_count;
                    let tgt_idx = (i * 7 + 3) % node_count; // pseudo-random target
                    let src_qn = format!(
                        "{}.src.module_{}.symbol_{}",
                        project,
                        src_idx / 100,
                        src_idx
                    );
                    let tgt_qn = format!(
                        "{}.src.module_{}.symbol_{}",
                        project,
                        tgt_idx / 100,
                        tgt_idx
                    );
                    buf.add_edge_by_qn(&src_qn, &tgt_qn, "CALLS", None);
                }

                (store, buf)
            },
            |(store, mut buf)| {
                // Flush resolves all QN-based edges to numeric IDs and writes to store
                buf.flush(&store).unwrap();
            },
        );
    });
    group.finish();
}

/// Benchmark: Incremental reindex with 5 changed files in a 1,600-file project.
///
/// Target: <10s for incremental reindex.
/// Simulates the scenario where a developer modifies 5 files and triggers reindex.
fn bench_incremental_reindex(c: &mut Criterion) {
    let mut group = c.benchmark_group("incremental_reindex");
    group.measurement_time(std::time::Duration::from_secs(20));
    group.sample_size(10);

    group.bench_function("5_changed_in_1600", |b| {
        b.iter_with_setup(
            || {
                let store = Store::open_in_memory().unwrap();
                let project = "incr_bench";
                store
                    .upsert_project(&Project {
                        name: project.to_string(),
                        indexed_at: "2024-01-01T00:00:00Z".to_string(),
                        root_path: "/bench".to_string(),
                    })
                    .unwrap();
                store.enable_bulk_indexing_mode().unwrap();

                // Simulate 1,600 files with ~10 symbols each = 16,000 nodes
                let total_files = 1_600;
                let symbols_per_file = 10;
                let total_nodes = total_files * symbols_per_file;
                let nodes = generate_test_nodes(project, total_nodes);
                store.insert_nodes_batch(&nodes).unwrap();

                // Record file hashes for all files
                let file_hashes: Vec<FileHash> = (0..total_files)
                    .map(|i| FileHash {
                        project: project.to_string(),
                        rel_path: format!("src/module_{}/file_{}.ts", i / 100, i / 10),
                        sha256: format!("hash_{:06}", i),
                        mtime_ns: 1_000_000_000 * i as i64,
                        size: 1024,
                    })
                    .collect();
                store.upsert_file_hash_batch(&file_hashes).unwrap();
                store.disable_bulk_indexing_mode().unwrap();

                // Identify 5 "changed" file paths
                let changed_files: Vec<String> = (0..5)
                    .map(|i| {
                        let file_idx = i * 320; // spread across the project
                        format!("src/module_{}/file_{}.ts", file_idx / 100, file_idx / 10)
                    })
                    .collect();

                (store, project.to_string(), changed_files)
            },
            |(store, project, changed_files)| {
                // Simulate incremental reindex: delete nodes for changed files, re-insert
                store.enable_bulk_indexing_mode().unwrap();

                let file_refs: Vec<&str> = changed_files.iter().map(|s| s.as_str()).collect();
                store.delete_nodes_for_files(&project, &file_refs).unwrap();

                // Re-insert updated nodes (simulating re-extraction)
                for file_path in &changed_files {
                    let new_nodes: Vec<Node> = (0..10)
                        .map(|i| Node {
                            id: 0,
                            project: project.clone(),
                            label: "Function".to_string(),
                            name: format!("updated_fn_{}", i),
                            qualified_name: format!("{}.{}.updated_fn_{}", project, file_path, i),
                            file_path: file_path.clone(),
                            start_line: i * 10,
                            end_line: i * 10 + 8,
                            properties_json: None,
                        })
                        .collect();
                    store.insert_nodes_batch(&new_nodes).unwrap();
                }
                store.disable_bulk_indexing_mode().unwrap();
            },
        );
    });
    group.finish();
}

/// Benchmark: SQLite page_size comparison (4096 vs 8192) on 10k-node project.
///
/// Measures total index time for a 10k-node project with different page sizes.
/// Uses file-backed stores to observe real I/O effects of page_size.
fn bench_page_size(c: &mut Criterion) {
    let mut group = c.benchmark_group("page_size");
    group.measurement_time(std::time::Duration::from_secs(10));
    group.sample_size(10);

    let node_count = 10_000;

    for page_size in [4096u32, 8192u32] {
        group.bench_with_input(
            BenchmarkId::from_parameter(page_size),
            &page_size,
            |b, &page_size| {
                b.iter_with_setup(
                    || {
                        // Create a temporary directory for the database
                        let tmp_dir = tempfile::tempdir().unwrap();
                        let db_path = tmp_dir.path().join("bench.db");

                        // Create the database with the desired page_size using rusqlite directly
                        {
                            let conn = rusqlite::Connection::open(&db_path).unwrap();
                            conn.execute_batch(&format!(
                                "PRAGMA page_size = {}; VACUUM;",
                                page_size
                            ))
                            .unwrap();
                        }

                        // Now open with Store (which will use the existing page_size)
                        let store = Store::open(&db_path).unwrap();
                        store
                            .upsert_project(&Project {
                                name: "page_bench".to_string(),
                                indexed_at: "2024-01-01T00:00:00Z".to_string(),
                                root_path: "/bench".to_string(),
                            })
                            .unwrap();

                        let nodes = generate_test_nodes("page_bench", node_count);
                        (store, nodes, tmp_dir)
                    },
                    |(store, nodes, _tmp_dir)| {
                        store.enable_bulk_indexing_mode().unwrap();
                        store.insert_nodes_batch(&nodes).unwrap();
                        store.disable_bulk_indexing_mode().unwrap();
                    },
                );
            },
        );
    }
    group.finish();
}

/// Benchmark: Java extraction on 500+ Java files.
///
/// Target: <60s for total extraction.
/// Generates synthetic Java source files and measures tree-sitter extraction throughput.
fn bench_java_extraction(c: &mut Criterion) {
    let mut group = c.benchmark_group("java_extraction");
    group.measurement_time(std::time::Duration::from_secs(15));
    group.sample_size(10);

    let file_count = 500;
    let methods_per_class = 10;

    group.throughput(Throughput::Elements(file_count as u64));
    group.bench_function("500_java_files", |b| {
        b.iter_with_setup(
            || {
                // Generate 500 Java source files
                let sources: Vec<(String, String)> = (0..file_count)
                    .map(|i| {
                        let class_name = format!("Service{}", i);
                        let source = generate_java_source(&class_name, methods_per_class);
                        (
                            format!("src/main/java/com/example/Service{}.java", i),
                            source,
                        )
                    })
                    .collect();
                sources
            },
            |sources| {
                // Extract symbols from all Java files using tree-sitter
                let mut total_symbols = 0;
                for (_path, source) in &sources {
                    if let Some(symbols) =
                        codryn_treesitter::extract_symbols(Language::Java, source)
                    {
                        total_symbols += symbols.len();
                    }
                }
                // Ensure we actually extracted something (prevents dead code elimination)
                assert!(total_symbols > 0, "Expected symbols from Java extraction");
            },
        );
    });
    group.finish();
}

criterion_group!(
    benches,
    bench_batch_insert,
    bench_qn_resolution,
    bench_incremental_reindex,
    bench_page_size,
    bench_java_extraction,
);
criterion_main!(benches);
