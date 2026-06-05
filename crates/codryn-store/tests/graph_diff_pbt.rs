use codryn_store::{Edge, Node, Project, Store};
use proptest::prelude::*;

/// **Validates: Requirements 31.1, 31.5**
/// Property 17: Graph Diff Delta Computation
///
/// For any two snapshots of the same project, the graph diff SHALL correctly compute
/// node_delta as (snapshot2.total_nodes - snapshot1.total_nodes), edge_delta as
/// (snapshot2.total_edges - snapshot1.total_edges), and per-label/per-edge-type count
/// changes that sum to the respective deltas.
fn test_store() -> Store {
    Store::open_in_memory().unwrap()
}

fn setup_project(store: &Store, project: &str) {
    store
        .upsert_project(&Project {
            name: project.into(),
            indexed_at: "2025-01-01T00:00:00Z".into(),
            root_path: "/tmp".into(),
        })
        .unwrap();
}

fn insert_node(store: &Store, project: &str, label: &str, name: &str) -> i64 {
    store
        .insert_node(&Node {
            id: 0,
            project: project.into(),
            label: label.into(),
            name: name.into(),
            qualified_name: format!("{}.{}", project, name),
            file_path: "src/lib.rs".into(),
            start_line: 1,
            end_line: 10,
            properties_json: None,
        })
        .unwrap()
}

fn insert_edge(store: &Store, project: &str, source_id: i64, target_id: i64, edge_type: &str) {
    store
        .insert_edge(&Edge {
            id: 0,
            project: project.into(),
            source_id,
            target_id,
            edge_type: edge_type.into(),
            properties_json: None,
        })
        .unwrap();
}

/// Strategy for generating a node label.
fn label_strategy() -> impl Strategy<Value = String> {
    prop_oneof![
        Just("Function".to_string()),
        Just("Class".to_string()),
        Just("Method".to_string()),
        Just("Module".to_string()),
        Just("Interface".to_string()),
    ]
}

/// Strategy for generating an edge type.
fn edge_type_strategy() -> impl Strategy<Value = String> {
    prop_oneof![
        Just("CALLS".to_string()),
        Just("IMPORTS".to_string()),
        Just("USES".to_string()),
        Just("TYPE_OF".to_string()),
    ]
}

/// Strategy for generating a set of nodes as (label, name) pairs.
fn nodes_strategy(max_count: usize) -> impl Strategy<Value = Vec<(String, String)>> {
    prop::collection::vec((label_strategy(), "[a-z]{3,8}"), 0..max_count)
}

/// Strategy for generating edges as (source_idx, target_idx, edge_type) triples.
/// Indices are relative to the node list.
#[allow(dead_code)]
fn edges_strategy(node_count: usize) -> impl Strategy<Value = Vec<(usize, usize, String)>> {
    if node_count < 2 {
        return prop::collection::vec((0usize..1, 0usize..1, edge_type_strategy()), 0..0).boxed();
    }
    prop::collection::vec(
        (0..node_count, 0..node_count, edge_type_strategy()),
        0..node_count.min(15),
    )
    .prop_filter("no self-edges", |edges| {
        edges.iter().all(|(s, t, _)| s != t)
    })
    .boxed()
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(100))]

    /// Property 17: node_delta equals snapshot2.total_nodes - snapshot1.total_nodes
    /// and edge_delta equals snapshot2.total_edges - snapshot1.total_edges.
    #[test]
    fn diff_deltas_match_snapshot_totals(
        nodes1 in nodes_strategy(15),
        nodes2_extra in nodes_strategy(10),
    ) {
        let store = test_store();
        setup_project(&store, "p");

        // Insert initial nodes for snapshot 1
        let mut node_ids: Vec<i64> = Vec::new();
        for (i, (label, name)) in nodes1.iter().enumerate() {
            let unique_name = format!("{}_{}", name, i);
            let id = insert_node(&store, "p", label, &unique_name);
            node_ids.push(id);
        }

        // Add some edges for snapshot 1 (between existing nodes)
        if node_ids.len() >= 2 {
            for i in 0..node_ids.len().saturating_sub(1).min(5) {
                insert_edge(&store, "p", node_ids[i], node_ids[i + 1], "CALLS");
            }
        }

        let snap1 = store.record_snapshot("p", None).unwrap();

        // Add extra nodes for snapshot 2
        for (i, (label, name)) in nodes2_extra.iter().enumerate() {
            let unique_name = format!("{}_extra_{}", name, i);
            let id = insert_node(&store, "p", label, &unique_name);
            node_ids.push(id);
        }

        // Add more edges for snapshot 2
        if node_ids.len() >= 2 {
            let start = nodes1.len().max(1);
            for i in start..node_ids.len().saturating_sub(1).min(start + 5) {
                insert_edge(&store, "p", node_ids[i], node_ids[0], "IMPORTS");
            }
        }

        let snap2 = store.record_snapshot("p", None).unwrap();

        let diff = store.diff_snapshots(snap1.id, snap2.id).unwrap();

        // Property: node_delta = snapshot2.total_nodes - snapshot1.total_nodes
        prop_assert_eq!(
            diff.node_delta,
            snap2.total_nodes - snap1.total_nodes,
            "node_delta mismatch: got {}, expected {} - {} = {}",
            diff.node_delta,
            snap2.total_nodes,
            snap1.total_nodes,
            snap2.total_nodes - snap1.total_nodes
        );

        // Property: edge_delta = snapshot2.total_edges - snapshot1.total_edges
        prop_assert_eq!(
            diff.edge_delta,
            snap2.total_edges - snap1.total_edges,
            "edge_delta mismatch: got {}, expected {} - {} = {}",
            diff.edge_delta,
            snap2.total_edges,
            snap1.total_edges,
            snap2.total_edges - snap1.total_edges
        );
    }

    /// Property 17: Per-label count changes sum to node_delta.
    #[test]
    fn label_changes_sum_to_node_delta(
        nodes1 in nodes_strategy(15),
        nodes2_extra in nodes_strategy(10),
        nodes_to_remove in 0usize..5,
    ) {
        let store = test_store();
        setup_project(&store, "p");

        // Insert initial nodes for snapshot 1
        let mut node_ids: Vec<i64> = Vec::new();
        for (i, (label, name)) in nodes1.iter().enumerate() {
            let unique_name = format!("{}_{}", name, i);
            let id = insert_node(&store, "p", label, &unique_name);
            node_ids.push(id);
        }

        let snap1 = store.record_snapshot("p", None).unwrap();

        // Remove some nodes (simulate deletion between snapshots)
        let remove_count = nodes_to_remove.min(node_ids.len());
        for id in node_ids.iter().take(remove_count) {
            store.conn().execute(
                "DELETE FROM nodes WHERE id = ?1",
                rusqlite::params![id],
            ).unwrap();
        }

        // Add extra nodes for snapshot 2
        for (i, (label, name)) in nodes2_extra.iter().enumerate() {
            let unique_name = format!("{}_extra_{}", name, i);
            insert_node(&store, "p", label, &unique_name);
        }

        let snap2 = store.record_snapshot("p", None).unwrap();

        let diff = store.diff_snapshots(snap1.id, snap2.id).unwrap();

        // Property: sum of all per-label changes must equal node_delta
        let label_changes_sum: i64 = diff.label_changes.values().sum();
        prop_assert_eq!(
            label_changes_sum,
            diff.node_delta,
            "Sum of label_changes ({}) does not equal node_delta ({}). label_changes: {:?}",
            label_changes_sum,
            diff.node_delta,
            diff.label_changes
        );
    }

    /// Property 17: Per-edge-type count changes sum to edge_delta.
    #[test]
    fn edge_type_changes_sum_to_edge_delta(
        num_nodes in 3usize..12,
        edge_types1 in prop::collection::vec(edge_type_strategy(), 0..10),
        edge_types2 in prop::collection::vec(edge_type_strategy(), 0..10),
        edges_to_remove in 0usize..5,
    ) {
        let store = test_store();
        setup_project(&store, "p");

        // Insert nodes
        let mut node_ids: Vec<i64> = Vec::new();
        for i in 0..num_nodes {
            let id = insert_node(&store, "p", "Function", &format!("func_{}", i));
            node_ids.push(id);
        }

        // Insert edges for snapshot 1
        let mut edge_ids: Vec<i64> = Vec::new();
        for (i, edge_type) in edge_types1.iter().enumerate() {
            let src = i % node_ids.len();
            let tgt = (i + 1) % node_ids.len();
            if src != tgt {
                insert_edge(&store, "p", node_ids[src], node_ids[tgt], edge_type);
                // Get the last inserted edge ID
                let eid: i64 = store.conn().query_row(
                    "SELECT MAX(id) FROM edges WHERE project = 'p'",
                    [],
                    |row| row.get(0),
                ).unwrap();
                edge_ids.push(eid);
            }
        }

        let snap1 = store.record_snapshot("p", None).unwrap();

        // Remove some edges
        let remove_count = edges_to_remove.min(edge_ids.len());
        for id in edge_ids.iter().take(remove_count) {
            store.conn().execute(
                "DELETE FROM edges WHERE id = ?1",
                rusqlite::params![id],
            ).unwrap();
        }

        // Add new edges for snapshot 2
        for (i, edge_type) in edge_types2.iter().enumerate() {
            let src = (i + 2) % node_ids.len();
            let tgt = (i + 3) % node_ids.len();
            if src != tgt {
                insert_edge(&store, "p", node_ids[src], node_ids[tgt], edge_type);
            }
        }

        let snap2 = store.record_snapshot("p", None).unwrap();

        let diff = store.diff_snapshots(snap1.id, snap2.id).unwrap();

        // Property: sum of all per-edge-type changes must equal edge_delta
        let edge_type_changes_sum: i64 = diff.edge_type_changes.values().sum();
        prop_assert_eq!(
            edge_type_changes_sum,
            diff.edge_delta,
            "Sum of edge_type_changes ({}) does not equal edge_delta ({}). edge_type_changes: {:?}",
            edge_type_changes_sum,
            diff.edge_delta,
            diff.edge_type_changes
        );
    }

    /// Property 17: All four properties hold simultaneously for arbitrary graph mutations.
    #[test]
    fn all_diff_properties_hold_together(
        labels1 in prop::collection::vec(label_strategy(), 1..12),
        labels2 in prop::collection::vec(label_strategy(), 0..8),
        edge_types1 in prop::collection::vec(edge_type_strategy(), 0..8),
        edge_types2 in prop::collection::vec(edge_type_strategy(), 0..8),
        remove_nodes in 0usize..4,
        remove_edges in 0usize..4,
    ) {
        let store = test_store();
        setup_project(&store, "p");

        // Build snapshot 1
        let mut node_ids: Vec<i64> = Vec::new();
        for (i, label) in labels1.iter().enumerate() {
            let id = insert_node(&store, "p", label, &format!("n1_{}", i));
            node_ids.push(id);
        }

        let mut edge_ids: Vec<i64> = Vec::new();
        for (i, edge_type) in edge_types1.iter().enumerate() {
            if node_ids.len() >= 2 {
                let src = i % node_ids.len();
                let tgt = (i + 1) % node_ids.len();
                if src != tgt {
                    insert_edge(&store, "p", node_ids[src], node_ids[tgt], edge_type);
                    let eid: i64 = store.conn().query_row(
                        "SELECT MAX(id) FROM edges WHERE project = 'p'",
                        [],
                        |row| row.get(0),
                    ).unwrap();
                    edge_ids.push(eid);
                }
            }
        }

        let snap1 = store.record_snapshot("p", None).unwrap();

        // Mutate: remove some nodes and their edges
        let nodes_to_remove = remove_nodes.min(node_ids.len());
        for id in node_ids.iter().take(nodes_to_remove) {
            // Delete edges referencing this node first
            store.conn().execute(
                "DELETE FROM edges WHERE project = 'p' AND (source_id = ?1 OR target_id = ?1)",
                rusqlite::params![id],
            ).unwrap();
            store.conn().execute(
                "DELETE FROM nodes WHERE id = ?1",
                rusqlite::params![id],
            ).unwrap();
        }

        // Remove some additional edges
        // Re-query remaining edge IDs since some may have been deleted with nodes
        let remaining_edge_ids: Vec<i64> = {
            let mut stmt = store.conn().prepare(
                "SELECT id FROM edges WHERE project = 'p'"
            ).unwrap();
            stmt.query_map([], |row| row.get(0))
                .unwrap()
                .collect::<rusqlite::Result<Vec<_>>>()
                .unwrap()
        };
        let edges_to_remove_count = remove_edges.min(remaining_edge_ids.len());
        for id in remaining_edge_ids.iter().take(edges_to_remove_count) {
            store.conn().execute(
                "DELETE FROM edges WHERE id = ?1",
                rusqlite::params![id],
            ).unwrap();
        }

        // Add new nodes
        let mut new_node_ids: Vec<i64> = Vec::new();
        for (i, label) in labels2.iter().enumerate() {
            let id = insert_node(&store, "p", label, &format!("n2_{}", i));
            new_node_ids.push(id);
        }

        // Add new edges (using all remaining + new nodes)
        let all_current_node_ids: Vec<i64> = {
            let mut stmt = store.conn().prepare(
                "SELECT id FROM nodes WHERE project = 'p'"
            ).unwrap();
            stmt.query_map([], |row| row.get(0))
                .unwrap()
                .collect::<rusqlite::Result<Vec<_>>>()
                .unwrap()
        };

        for (i, edge_type) in edge_types2.iter().enumerate() {
            if all_current_node_ids.len() >= 2 {
                let src = i % all_current_node_ids.len();
                let tgt = (i + 1) % all_current_node_ids.len();
                if src != tgt {
                    insert_edge(
                        &store, "p",
                        all_current_node_ids[src],
                        all_current_node_ids[tgt],
                        edge_type,
                    );
                }
            }
        }

        let snap2 = store.record_snapshot("p", None).unwrap();

        let diff = store.diff_snapshots(snap1.id, snap2.id).unwrap();

        // Property 1: node_delta = snapshot2.total_nodes - snapshot1.total_nodes
        prop_assert_eq!(
            diff.node_delta,
            snap2.total_nodes - snap1.total_nodes,
            "node_delta mismatch"
        );

        // Property 2: edge_delta = snapshot2.total_edges - snapshot1.total_edges
        prop_assert_eq!(
            diff.edge_delta,
            snap2.total_edges - snap1.total_edges,
            "edge_delta mismatch"
        );

        // Property 3: sum of label_changes = node_delta
        let label_sum: i64 = diff.label_changes.values().sum();
        prop_assert_eq!(
            label_sum,
            diff.node_delta,
            "label_changes sum ({}) != node_delta ({})",
            label_sum,
            diff.node_delta
        );

        // Property 4: sum of edge_type_changes = edge_delta
        let edge_type_sum: i64 = diff.edge_type_changes.values().sum();
        prop_assert_eq!(
            edge_type_sum,
            diff.edge_delta,
            "edge_type_changes sum ({}) != edge_delta ({})",
            edge_type_sum,
            diff.edge_delta
        );
    }
}
