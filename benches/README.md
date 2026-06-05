# Performance Benchmarks

The benchmark harness lives in `crates/codryn-bench/benches/pipeline_bench.rs`.

## Running Benchmarks

```bash
# Run all benchmarks
cargo bench -p codryn-bench

# Run a specific benchmark group
cargo bench -p codryn-bench --bench pipeline_bench -- "batch_insert"
cargo bench -p codryn-bench --bench pipeline_bench -- "qn_resolution"
cargo bench -p codryn-bench --bench pipeline_bench -- "incremental_reindex"
cargo bench -p codryn-bench --bench pipeline_bench -- "page_size"
cargo bench -p codryn-bench --bench pipeline_bench -- "java_extraction"
```

## Benchmark Targets

| Benchmark | Description | Target |
|-----------|-------------|--------|
| `batch_insert` | INSERT...RETURNING throughput on 10k-node project | Measure rows/sec |
| `qn_resolution` | Qualified name resolution on 180K-edge project | <500ms |
| `incremental_reindex` | 5 changed files in 1,600-file project | <10s |
| `page_size` | Compare page_size 4096 vs 8192 on 10k-node project | Report difference |
| `java_extraction` | 500+ Java file tree-sitter extraction | <60s |

## Results

Results are stored in `target/criterion/` with HTML reports.
