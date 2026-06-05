//! Benchmark utilities for the codryn pipeline.
//!
//! This crate provides Criterion.rs benchmarks for critical performance paths:
//! - Batch INSERT...RETURNING throughput
//! - Qualified name resolution
//! - Incremental reindex
//! - SQLite page_size comparison
//! - Java extraction throughput

pub mod fixtures;

use anyhow::Result;
use codryn_store::{Node, Project, Store};

/// Generate a synthetic project with `n` nodes for benchmarking.
pub fn generate_test_nodes(project: &str, n: usize) -> Vec<Node> {
    (0..n)
        .map(|i| Node {
            id: 0,
            project: project.to_string(),
            label: if i % 3 == 0 {
                "Class".to_string()
            } else {
                "Function".to_string()
            },
            name: format!("symbol_{}", i),
            qualified_name: format!("{}.src.module_{}.symbol_{}", project, i / 100, i),
            file_path: format!("src/module_{}/file_{}.ts", i / 100, i / 10),
            start_line: (i * 10) as i32,
            end_line: (i * 10 + 8) as i32,
            properties_json: Some(format!(
                r#"{{"cyclomatic_complexity": {}, "cognitive_complexity": {}}}"#,
                (i % 10) + 1,
                (i % 5)
            )),
        })
        .collect()
}

/// Create a store with a project and `n` nodes already inserted.
/// Returns the store and the list of (qualified_name, id) pairs.
pub fn setup_store_with_nodes(n: usize) -> Result<(Store, Vec<(String, i64)>)> {
    let store = Store::open_in_memory()?;
    let project = "bench_project";
    store.upsert_project(&Project {
        name: project.to_string(),
        indexed_at: chrono::Utc::now().to_rfc3339(),
        root_path: "/bench".to_string(),
    })?;
    store.enable_bulk_indexing_mode()?;

    let nodes = generate_test_nodes(project, n);
    let mut all_ids = Vec::with_capacity(n);

    // Insert in batches of 500 to avoid overly large transactions
    for chunk in nodes.chunks(500) {
        let ids = store.insert_nodes_batch(chunk)?;
        all_ids.extend(ids);
    }

    store.disable_bulk_indexing_mode()?;
    Ok((store, all_ids))
}

/// Generate synthetic Java source code for extraction benchmarking.
pub fn generate_java_source(class_name: &str, method_count: usize) -> String {
    let mut source = String::with_capacity(method_count * 400);
    source.push_str("package com.example.app;\n\n");
    source.push_str("import java.util.List;\n");
    source.push_str("import java.util.Map;\n");
    source.push_str("import org.springframework.stereotype.Service;\n\n");
    source.push_str("@Service\n");
    source.push_str(&format!("public class {} {{\n\n", class_name));

    for i in 0..method_count {
        source.push_str(&format!(
            "    public String method{i}(int param{i}, String arg{i}) {{\n",
        ));
        source.push_str(&format!("        if (param{i} > 0) {{\n",));
        source.push_str(&format!(
            "            for (int j = 0; j < param{i}; j++) {{\n",
        ));
        source.push_str("                if (j % 2 == 0) {\n");
        source.push_str(&format!(
            "                    System.out.println(arg{i});\n",
        ));
        source.push_str("                } else {\n");
        source.push_str("                    return \"result_\" + j;\n");
        source.push_str("                }\n");
        source.push_str("            }\n");
        source.push_str("        }\n");
        source.push_str(&format!("        return arg{i} + \"_processed\";\n",));
        source.push_str("    }\n\n");
    }

    source.push_str("}\n");
    source
}
