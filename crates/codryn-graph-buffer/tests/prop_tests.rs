use codryn_graph_buffer::{EdgeSource, GraphBuffer};
use codryn_store::{Node, Project, Store};
use proptest::prelude::*;

fn test_store() -> Store {
    Store::open_in_memory().unwrap()
}

fn setup_project(store: &Store, name: &str) {
    store
        .upsert_project(&Project {
            name: name.into(),
            indexed_at: "2024-01-01T00:00:00Z".into(),
            root_path: "/test".into(),
        })
        .unwrap();
}

/// **Validates: Requirements 2.3**
/// Property 2 (index-speed-optimization): Seed IDs Completeness
/// For any set of nodes stored in the database for a project, after calling
/// `seed_ids_from_store`, every node's qualified name SHALL be present in the
/// `qn_to_id` map with the correct node ID.
mod property2_seed_ids_completeness {
    use super::*;

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(100))]
        #[test]
        fn seed_ids_contains_all_nodes(
            node_count in 5usize..30,
            label_indices in prop::collection::vec(0usize..5, 5..30),
        ) {
            let labels = ["Function".to_string(),
                "Class".to_string(),
                "Method".to_string(),
                "Module".to_string(),
                "Interface".to_string()];

            // Clamp label_indices to actual node_count
            let node_count = node_count.min(label_indices.len());

            let store = test_store();
            store.upsert_project(&Project {
                name: "test_proj".into(),
                indexed_at: "now".into(),
                root_path: "/test".into(),
            }).unwrap();

            // Generate unique nodes with distinct qualified names
            let mut nodes = Vec::new();
            for (i, label_idx_raw) in label_indices.iter().enumerate().take(node_count) {
                let label_idx = label_idx_raw % labels.len();
                nodes.push(Node {
                    id: 0,
                    project: "test_proj".into(),
                    label: labels[label_idx].clone(),
                    name: format!("symbol_{}", i),
                    qualified_name: format!("test_proj.src.mod{}.symbol_{}", i / 5, i),
                    file_path: format!("src/file_{}.ts", i),
                    start_line: 1,
                    end_line: 10 + (i as i32),
                    properties_json: None,
                });
            }

            // Insert nodes into the store
            let insert_results = store.insert_nodes_batch(&nodes).unwrap();
            prop_assert_eq!(
                insert_results.len(), node_count,
                "insert_nodes_batch should return {} results, got {}",
                node_count, insert_results.len()
            );

            // Build expected mapping: qualified_name -> id
            let expected: std::collections::HashMap<String, i64> = insert_results
                .iter()
                .map(|(qn, id)| (qn.clone(), *id))
                .collect();

            // Create a new GraphBuffer and seed IDs from the store
            let mut buf = GraphBuffer::new("test_proj");
            buf.seed_ids_from_store(&store).unwrap();

            // Verify every inserted node's qualified name is present with correct ID
            for (qn, expected_id) in &expected {
                let actual_id = buf.get_node_id(qn);
                prop_assert_eq!(
                    actual_id, Some(*expected_id),
                    "seed_ids_from_store missing or wrong ID for QN '{}': expected Some({}), got {:?}",
                    qn, expected_id, actual_id
                );
            }

            // Verify the total count matches (no extra entries beyond what was inserted)
            // We check that every node we inserted is present - completeness
            let mut found_count = 0;
            for node in &nodes {
                if buf.get_node_id(&node.qualified_name).is_some() {
                    found_count += 1;
                }
            }
            prop_assert_eq!(
                found_count, node_count,
                "expected {} nodes in qn_to_id, but only found {}",
                node_count, found_count
            );
        }
    }
}

/// **Validates: Requirements 2.2**
/// Property 1 (GraphBuffer Flush Round-Trip):
/// For any set of N valid nodes and M valid edges (including QN-based edges
/// requiring resolution) buffered in a GraphBuffer, flushing to the Store and
/// then querying back by qualified name SHALL return exactly N nodes with
/// matching qualified names, and all M edges SHALL have resolved source and
/// target IDs greater than 0.
mod property1_flush_round_trip {
    use super::*;

    /// Strategy to generate a valid node label.
    fn arb_label() -> impl Strategy<Value = String> {
        prop_oneof![
            Just("Function".to_string()),
            Just("Class".to_string()),
            Just("Method".to_string()),
            Just("Module".to_string()),
            Just("Interface".to_string()),
        ]
    }

    /// Strategy to generate a valid edge type.
    fn arb_edge_type() -> impl Strategy<Value = String> {
        prop_oneof![
            Just("CALLS".to_string()),
            Just("IMPORTS".to_string()),
            Just("CONTAINS".to_string()),
            Just("USES".to_string()),
        ]
    }

    /// Strategy to generate an EdgeSource variant.
    fn arb_edge_source() -> impl Strategy<Value = EdgeSource> {
        prop_oneof![
            Just(EdgeSource::AstStructural),
            Just(EdgeSource::AstNameMatch),
            Just(EdgeSource::ImportResolver),
            Just(EdgeSource::DedicatedAdapter),
            Just(EdgeSource::ExternalLsp),
            Just(EdgeSource::CompilerIndex),
            Just(EdgeSource::AhoCorasickMatch),
            Just(EdgeSource::RegexMatch),
            Just(EdgeSource::Heuristic),
        ]
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(50))]

        /// For any N nodes (1..50) and M edges connecting those nodes via QN
        /// resolution, flushing to the store and querying back SHALL return
        /// exactly N nodes with matching qualified names, and all M edges SHALL
        /// have resolved source and target IDs > 0.
        #[test]
        fn flush_round_trip_preserves_nodes_and_resolves_edges(
            node_count in 1usize..50,
            labels in prop::collection::vec(arb_label(), 1..50),
            edge_pairs in prop::collection::vec((0usize..50, 0usize..50, arb_edge_type(), arb_edge_source()), 0..50),
        ) {
            // Clamp to actual generated sizes
            let n = node_count.min(labels.len());

            let store = test_store();
            let project = "prop_flush_test";
            setup_project(&store, project);

            let mut buf = GraphBuffer::new(project);

            // Generate N nodes with unique qualified names
            let mut qualified_names: Vec<String> = Vec::with_capacity(n);
            for (i, label) in labels.iter().enumerate().take(n) {
                let name = format!("symbol_{}", i);
                let qn = format!("{}.mod{}.symbol_{}", project, i / 5, i);
                let file_path = format!("src/mod{}/file_{}.rs", i / 5, i);
                let start_line = (i as i32) + 1;
                let end_line = start_line + 10;

                buf.add_node(label, &name, &qn, &file_path, start_line, end_line, None);
                qualified_names.push(qn);
            }

            prop_assert_eq!(buf.node_count(), n);

            // Generate M edges between valid node pairs (QN-based, requiring resolution)
            // Filter edge_pairs to only include valid indices and non-self-referencing edges
            let valid_edges: Vec<_> = edge_pairs
                .iter()
                .filter(|(src_idx, tgt_idx, _, _)| {
                    let src = *src_idx % n;
                    let tgt = *tgt_idx % n;
                    src != tgt // no self-referential edges
                })
                .collect();

            // Deduplicate edges by (source_qn, target_qn) to avoid duplicate edge issues
            let mut seen_pairs = std::collections::HashSet::new();
            let mut unique_edges = Vec::new();
            for (src_idx, tgt_idx, edge_type, edge_source) in &valid_edges {
                let src = *src_idx % n;
                let tgt = *tgt_idx % n;
                let pair = (src, tgt, edge_type.clone());
                if seen_pairs.insert(pair) {
                    unique_edges.push((src, tgt, edge_type.clone(), *edge_source));
                }
            }

            let m = unique_edges.len();

            for (src, tgt, edge_type, edge_source) in &unique_edges {
                buf.add_edge_with_confidence(
                    &qualified_names[*src],
                    &qualified_names[*tgt],
                    edge_type,
                    *edge_source,
                    None,
                );
            }

            prop_assert_eq!(buf.edge_count(), m);

            // Flush to store
            buf.flush(&store).expect("flush should succeed");

            // PROPERTY: All N nodes are retrievable by qualified name
            for qn in &qualified_names {
                let node = store
                    .find_node_by_qn(project, qn)
                    .unwrap_or_else(|e| panic!("Failed to query node '{}': {}", qn, e));
                prop_assert!(
                    node.is_some(),
                    "Node with qualified_name '{}' should be retrievable after flush",
                    qn
                );
                let node = node.unwrap();
                prop_assert!(
                    node.id > 0,
                    "Node '{}' should have id > 0, got {}",
                    qn,
                    node.id
                );
                prop_assert_eq!(
                    &node.qualified_name, qn,
                    "Node qualified_name mismatch: expected '{}', got '{}'",
                    qn, node.qualified_name
                );
            }

            // PROPERTY: All M edges have resolved source and target IDs > 0
            let edges = store.get_edges(project, (m as i32) + 10).unwrap();
            prop_assert_eq!(
                edges.len(), m,
                "Expected {} edges in store after flush, got {}",
                m, edges.len()
            );

            for edge in &edges {
                prop_assert!(
                    edge.source_id > 0,
                    "Edge (type={}) should have source_id > 0, got {}",
                    edge.edge_type,
                    edge.source_id
                );
                prop_assert!(
                    edge.target_id > 0,
                    "Edge (type={}) should have target_id > 0, got {}",
                    edge.edge_type,
                    edge.target_id
                );
            }
        }
    }
}
