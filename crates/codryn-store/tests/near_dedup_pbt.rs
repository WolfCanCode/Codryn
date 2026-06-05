use codryn_store::{Edge, Node, Project, Store};
use proptest::prelude::*;

/// **Validates: Requirements 30.1, 30.2, 30.3**
/// Property 16: Near-Duplicate Detection and Merge
///
/// For any set of nodes sharing the same name, file_path, and label but different
/// node_ids, the deduplication pass SHALL detect them as a group, keep the node with
/// the most recent indexed_at timestamp as survivor, and redirect all edges from
/// discarded nodes to the survivor without creating self-referential edges.
fn test_store() -> Store {
    Store::open_in_memory().unwrap()
}

fn setup_project(store: &Store) {
    store
        .upsert_project(&Project {
            name: "p".into(),
            indexed_at: "2025-01-01T00:00:00Z".into(),
            root_path: "/tmp".into(),
        })
        .unwrap();
}

/// Strategy for generating a node name (short alphabetic string).
fn name_strategy() -> impl Strategy<Value = String> {
    "[a-z]{3,8}"
}

/// Strategy for generating a file path.
fn file_path_strategy() -> impl Strategy<Value = String> {
    prop_oneof![
        Just("src/a.rs".to_string()),
        Just("src/b.rs".to_string()),
        Just("src/c.rs".to_string()),
        Just("src/lib.rs".to_string()),
        Just("src/main.rs".to_string()),
    ]
}

/// Strategy for generating a label.
fn label_strategy() -> impl Strategy<Value = String> {
    prop_oneof![
        Just("Function".to_string()),
        Just("Class".to_string()),
        Just("Method".to_string()),
        Just("Module".to_string()),
    ]
}

/// Strategy for generating an edge type.
fn edge_type_strategy() -> impl Strategy<Value = String> {
    prop_oneof![
        Just("CALLS".to_string()),
        Just("IMPORTS".to_string()),
        Just("USES".to_string()),
    ]
}

/// Insert a near-duplicate node (same name, file_path, label but different qualified_name).
fn insert_near_dup_node(
    store: &Store,
    name: &str,
    file_path: &str,
    label: &str,
    qn_suffix: usize,
) -> i64 {
    store
        .insert_node(&Node {
            id: 0,
            project: "p".into(),
            label: label.into(),
            name: name.into(),
            qualified_name: format!("p::{}::{}_{}", file_path, name, qn_suffix),
            file_path: file_path.into(),
            start_line: 1,
            end_line: 10,
            properties_json: None,
        })
        .unwrap()
}

/// Insert a unique non-duplicate node (used as edge endpoints).
fn insert_unique_node(store: &Store, suffix: usize) -> i64 {
    store
        .insert_node(&Node {
            id: 0,
            project: "p".into(),
            label: "Function".into(),
            name: format!("unique_{}", suffix),
            qualified_name: format!("p::unique_{}", suffix),
            file_path: format!("src/unique_{}.rs", suffix),
            start_line: 1,
            end_line: 5,
            properties_json: None,
        })
        .unwrap()
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(100))]

    /// Property 16.1: All nodes with same (name, file_path, label) are detected as a group.
    ///
    /// For any N >= 2 nodes sharing the same (name, file_path, label), detect_near_duplicates
    /// SHALL return exactly one group containing all N nodes.
    #[test]
    fn near_duplicates_detected_as_group(
        name in name_strategy(),
        file_path in file_path_strategy(),
        label in label_strategy(),
        dup_count in 2usize..6,
    ) {
        let store = test_store();
        setup_project(&store);

        // Insert dup_count near-duplicate nodes
        let mut node_ids: Vec<i64> = Vec::new();
        for i in 0..dup_count {
            let id = insert_near_dup_node(&store, &name, &file_path, &label, i);
            node_ids.push(id);
        }

        let groups = store.detect_near_duplicates("p").unwrap();

        // Exactly one group should be detected
        prop_assert_eq!(
            groups.len(), 1,
            "Expected 1 group, got {}. name={}, file_path={}, label={}",
            groups.len(), name, file_path, label
        );

        let group = &groups[0];

        // Group should match the shared attributes
        prop_assert_eq!(&group.name, &name);
        prop_assert_eq!(&group.file_path, &file_path);
        prop_assert_eq!(&group.label, &label);

        // Total nodes in group = survivor + discarded = dup_count
        let total_in_group = 1 + group.discarded_ids.len();
        prop_assert_eq!(
            total_in_group, dup_count,
            "Expected {} nodes in group, got {}",
            dup_count, total_in_group
        );

        // All node_ids should be accounted for (either survivor or discarded)
        let mut all_group_ids: Vec<i64> = group.discarded_ids.clone();
        all_group_ids.push(group.survivor_id);
        all_group_ids.sort();
        let mut expected_ids = node_ids.clone();
        expected_ids.sort();
        prop_assert_eq!(all_group_ids, expected_ids);
    }

    /// Property 16.2: The survivor is the node with the smallest ID.
    ///
    /// Since all nodes in the same project share indexed_at, the tie-breaker
    /// (smallest node_id) determines the survivor.
    #[test]
    fn survivor_is_smallest_id(
        name in name_strategy(),
        file_path in file_path_strategy(),
        label in label_strategy(),
        dup_count in 2usize..6,
    ) {
        let store = test_store();
        setup_project(&store);

        let mut node_ids: Vec<i64> = Vec::new();
        for i in 0..dup_count {
            let id = insert_near_dup_node(&store, &name, &file_path, &label, i);
            node_ids.push(id);
        }

        let groups = store.detect_near_duplicates("p").unwrap();
        prop_assert_eq!(groups.len(), 1);

        let group = &groups[0];
        let smallest_id = *node_ids.iter().min().unwrap();

        prop_assert_eq!(
            group.survivor_id, smallest_id,
            "Survivor should be smallest ID ({}), got {}",
            smallest_id, group.survivor_id
        );
    }

    /// Property 16.3: After merge, all edges from discarded nodes are redirected to survivor.
    ///
    /// For any set of edges pointing to/from discarded nodes, after merge_near_duplicates,
    /// those edges should now reference the survivor node.
    #[test]
    fn edges_redirected_to_survivor_after_merge(
        name in name_strategy(),
        file_path in file_path_strategy(),
        label in label_strategy(),
        dup_count in 2usize..5,
        num_callers in 1usize..4,
        edge_types in prop::collection::vec(edge_type_strategy(), 1..4),
    ) {
        let store = test_store();
        setup_project(&store);

        // Insert near-duplicate nodes
        let mut dup_ids: Vec<i64> = Vec::new();
        for i in 0..dup_count {
            let id = insert_near_dup_node(&store, &name, &file_path, &label, i);
            dup_ids.push(id);
        }

        // Insert unique caller nodes
        let mut caller_ids: Vec<i64> = Vec::new();
        for i in 0..num_callers {
            let id = insert_unique_node(&store, i);
            caller_ids.push(id);
        }

        // Create edges from callers to discarded nodes (not the survivor)
        let survivor_id = *dup_ids.iter().min().unwrap();
        let discarded_ids: Vec<i64> = dup_ids.iter().copied().filter(|&id| id != survivor_id).collect();

        let mut expected_edges: Vec<(i64, String)> = Vec::new(); // (caller_id, edge_type)
        for (i, &caller_id) in caller_ids.iter().enumerate() {
            let target = discarded_ids[i % discarded_ids.len()];
            let edge_type = &edge_types[i % edge_types.len()];
            store.insert_edge(&Edge {
                id: 0,
                project: "p".into(),
                source_id: caller_id,
                target_id: target,
                edge_type: edge_type.clone(),
                properties_json: None,
            }).unwrap();
            expected_edges.push((caller_id, edge_type.clone()));
        }

        // Perform merge
        let merge_count = store.merge_near_duplicates("p").unwrap();
        prop_assert_eq!(merge_count, 1);

        // After merge, all edges should point to the survivor
        let edges = store.get_edges("p", 1000).unwrap();
        for edge in &edges {
            // No edge should reference any discarded node
            prop_assert!(
                !discarded_ids.contains(&edge.source_id),
                "Edge source_id {} references a discarded node",
                edge.source_id
            );
            prop_assert!(
                !discarded_ids.contains(&edge.target_id),
                "Edge target_id {} references a discarded node",
                edge.target_id
            );
        }

        // All expected edges should now target the survivor
        for (caller_id, edge_type) in &expected_edges {
            let matching = edges.iter().filter(|e| {
                e.source_id == *caller_id && e.target_id == survivor_id && e.edge_type == *edge_type
            }).count();
            prop_assert!(
                matching >= 1,
                "Expected edge from caller {} to survivor {} with type {}, not found",
                caller_id, survivor_id, edge_type
            );
        }
    }

    /// Property 16.4: No self-referential edges exist after merge.
    ///
    /// After merge_near_duplicates, no edge should have source_id == target_id
    /// on the survivor node.
    #[test]
    fn no_self_referential_edges_after_merge(
        name in name_strategy(),
        file_path in file_path_strategy(),
        label in label_strategy(),
        dup_count in 2usize..5,
        // Whether to create cross-edges between duplicates (which become self-refs after merge)
        create_cross_edges in prop::bool::ANY,
        num_cross_edges in 1usize..4,
    ) {
        let store = test_store();
        setup_project(&store);

        // Insert near-duplicate nodes
        let mut dup_ids: Vec<i64> = Vec::new();
        for i in 0..dup_count {
            let id = insert_near_dup_node(&store, &name, &file_path, &label, i);
            dup_ids.push(id);
        }

        let survivor_id = *dup_ids.iter().min().unwrap();

        // Optionally create edges between duplicate nodes (these would become self-referential)
        if create_cross_edges && dup_ids.len() >= 2 {
            for i in 0..num_cross_edges.min(dup_ids.len() - 1) {
                let src = dup_ids[i];
                let tgt = dup_ids[(i + 1) % dup_ids.len()];
                if src != tgt {
                    store.insert_edge(&Edge {
                        id: 0,
                        project: "p".into(),
                        source_id: src,
                        target_id: tgt,
                        edge_type: "CALLS".into(),
                        properties_json: None,
                    }).unwrap();
                }
            }
        }

        // Also add some edges from survivor to discarded (would become self-ref)
        if dup_ids.len() >= 2 {
            let discarded = dup_ids.iter().copied().find(|&id| id != survivor_id).unwrap();
            store.insert_edge(&Edge {
                id: 0,
                project: "p".into(),
                source_id: survivor_id,
                target_id: discarded,
                edge_type: "IMPORTS".into(),
                properties_json: None,
            }).unwrap();
        }

        // Perform merge
        store.merge_near_duplicates("p").unwrap();

        // Verify: no self-referential edges exist
        let edges = store.get_edges("p", 1000).unwrap();
        for edge in &edges {
            prop_assert!(
                edge.source_id != edge.target_id,
                "Self-referential edge found: source_id={} == target_id={} (edge_type={})",
                edge.source_id, edge.target_id, edge.edge_type
            );
        }
    }

    /// Property 16.5: Discarded nodes are removed from the store after merge.
    ///
    /// After merge_near_duplicates, only the survivor node should remain from
    /// the duplicate group.
    #[test]
    fn discarded_nodes_removed_after_merge(
        name in name_strategy(),
        file_path in file_path_strategy(),
        label in label_strategy(),
        dup_count in 2usize..6,
        num_unique in 0usize..4,
    ) {
        let store = test_store();
        setup_project(&store);

        // Insert near-duplicate nodes
        let mut dup_ids: Vec<i64> = Vec::new();
        for i in 0..dup_count {
            let id = insert_near_dup_node(&store, &name, &file_path, &label, i);
            dup_ids.push(id);
        }

        // Insert some unique (non-duplicate) nodes
        for i in 0..num_unique {
            insert_unique_node(&store, i);
        }

        let survivor_id = *dup_ids.iter().min().unwrap();

        // Perform merge
        store.merge_near_duplicates("p").unwrap();

        // Verify: only survivor + unique nodes remain
        let schema = store.get_graph_schema("p").unwrap();
        let expected_nodes = 1 + num_unique as i64; // 1 survivor + unique nodes
        prop_assert_eq!(
            schema.total_nodes, expected_nodes,
            "Expected {} nodes (1 survivor + {} unique), got {}",
            expected_nodes, num_unique, schema.total_nodes
        );

        // Verify survivor still exists
        let survivor_exists: bool = store.conn().query_row(
            "SELECT COUNT(*) > 0 FROM nodes WHERE id = ?1",
            rusqlite::params![survivor_id],
            |row| row.get(0),
        ).unwrap();
        prop_assert!(survivor_exists, "Survivor node {} should still exist", survivor_id);

        // Verify discarded nodes are gone
        let discarded_ids: Vec<i64> = dup_ids.iter().copied().filter(|&id| id != survivor_id).collect();
        for &discarded_id in &discarded_ids {
            let exists: bool = store.conn().query_row(
                "SELECT COUNT(*) > 0 FROM nodes WHERE id = ?1",
                rusqlite::params![discarded_id],
                |row| row.get(0),
            ).unwrap();
            prop_assert!(!exists, "Discarded node {} should have been removed", discarded_id);
        }
    }
}
