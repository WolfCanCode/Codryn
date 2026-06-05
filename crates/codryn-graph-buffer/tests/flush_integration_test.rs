//! Integration test for GraphBuffer flush round-trip.
//!
//! Tests that N buffered nodes + M buffered edges (including QN-based edges)
//! are correctly flushed to the Store, with all nodes retrievable by qualified
//! name and all edges having resolved source/target IDs > 0.
//!
//! **Validates: Requirements 2.2**

use codryn_graph_buffer::{EdgeSource, GraphBuffer};
use codryn_store::{Project, Store};

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

/// Test that N buffered nodes are all retrievable by qualified name after flush.
/// Uses a mix of labels (Function, Class, Method, Module) to simulate real usage.
#[test]
fn test_flush_nodes_retrievable_by_qualified_name() {
    let store = test_store();
    let project = "flush_test";
    setup_project(&store, project);

    let mut buf = GraphBuffer::new(project);

    // Buffer N = 10 nodes with distinct qualified names
    let node_specs: Vec<(&str, &str, &str, &str)> = vec![
        ("Function", "main", "flush_test.src.main", "src/main.ts"),
        (
            "Class",
            "UserService",
            "flush_test.src.services.UserService",
            "src/services/user.ts",
        ),
        (
            "Method",
            "getUser",
            "flush_test.src.services.UserService.getUser",
            "src/services/user.ts",
        ),
        (
            "Method",
            "createUser",
            "flush_test.src.services.UserService.createUser",
            "src/services/user.ts",
        ),
        (
            "Class",
            "UserRepository",
            "flush_test.src.repo.UserRepository",
            "src/repo/user_repo.ts",
        ),
        (
            "Method",
            "findById",
            "flush_test.src.repo.UserRepository.findById",
            "src/repo/user_repo.ts",
        ),
        (
            "Function",
            "handleRequest",
            "flush_test.src.handler.handleRequest",
            "src/handler.ts",
        ),
        ("Module", "logger", "flush_test.src.logger", "src/logger.ts"),
        (
            "Function",
            "log",
            "flush_test.src.logger.log",
            "src/logger.ts",
        ),
        (
            "Function",
            "formatError",
            "flush_test.src.utils.formatError",
            "src/utils.ts",
        ),
    ];

    let n = node_specs.len();

    for (i, (label, name, qn, file_path)) in node_specs.iter().enumerate() {
        buf.add_node(
            label,
            name,
            qn,
            file_path,
            (i as i32) + 1,
            (i as i32) + 10,
            None,
        );
    }

    assert_eq!(buf.node_count(), n);

    // Flush to store
    buf.flush(&store).unwrap();

    // Verify all N nodes are retrievable by qualified name
    for (_label, _name, qn, _file_path) in &node_specs {
        let node = store
            .find_node_by_qn(project, qn)
            .unwrap_or_else(|e| panic!("Failed to query node '{}': {}", qn, e));
        assert!(
            node.is_some(),
            "Node with qualified_name '{}' should be retrievable after flush",
            qn
        );
        let node = node.unwrap();
        assert!(
            node.id > 0,
            "Node '{}' should have id > 0, got {}",
            qn,
            node.id
        );
        assert_eq!(node.qualified_name, *qn);
    }
}

/// Test that M buffered edges (including QN-based edges requiring resolution)
/// all have resolved source and target IDs > 0 after flush.
#[test]
fn test_flush_edges_have_resolved_ids() {
    let store = test_store();
    let project = "flush_edges_test";
    setup_project(&store, project);

    let mut buf = GraphBuffer::new(project);

    // Add nodes that edges will reference
    buf.add_node(
        "Function",
        "main",
        "flush_edges_test.main",
        "src/main.ts",
        1,
        20,
        None,
    );
    buf.add_node(
        "Class",
        "UserService",
        "flush_edges_test.UserService",
        "src/user.ts",
        1,
        50,
        None,
    );
    buf.add_node(
        "Method",
        "getUser",
        "flush_edges_test.UserService.getUser",
        "src/user.ts",
        5,
        15,
        None,
    );
    buf.add_node(
        "Method",
        "createUser",
        "flush_edges_test.UserService.createUser",
        "src/user.ts",
        17,
        30,
        None,
    );
    buf.add_node(
        "Module",
        "logger",
        "flush_edges_test.logger",
        "src/logger.ts",
        1,
        10,
        None,
    );

    // Add QN-based edges (source_id/target_id = 0, resolved at flush time)
    buf.add_edge_by_qn(
        "flush_edges_test.main",
        "flush_edges_test.UserService.getUser",
        "CALLS",
        None,
    );
    buf.add_edge_by_qn(
        "flush_edges_test.main",
        "flush_edges_test.UserService.createUser",
        "CALLS",
        None,
    );
    buf.add_edge_by_qn(
        "flush_edges_test.main",
        "flush_edges_test.logger",
        "IMPORTS",
        None,
    );
    buf.add_edge_by_qn(
        "flush_edges_test.UserService",
        "flush_edges_test.UserService.getUser",
        "CONTAINS",
        None,
    );
    buf.add_edge_by_qn(
        "flush_edges_test.UserService",
        "flush_edges_test.UserService.createUser",
        "CONTAINS",
        None,
    );

    let m = buf.edge_count();
    assert_eq!(m, 5);

    // Flush to store
    buf.flush(&store).unwrap();

    // Verify all M edges have resolved source_id and target_id > 0
    let edges = store.get_edges(project, 100).unwrap();
    assert_eq!(
        edges.len(),
        m,
        "Expected {} edges in store after flush, got {}",
        m,
        edges.len()
    );

    for edge in &edges {
        assert!(
            edge.source_id > 0,
            "Edge (type={}) should have source_id > 0, got {}",
            edge.edge_type,
            edge.source_id
        );
        assert!(
            edge.target_id > 0,
            "Edge (type={}) should have target_id > 0, got {}",
            edge.edge_type,
            edge.target_id
        );
    }
}

/// Combined test: N nodes + M edges (mix of QN-based and ID-based edges)
/// verifying the full flush round-trip.
#[test]
fn test_flush_combined_nodes_and_edges_round_trip() {
    let store = test_store();
    let project = "combined_test";
    setup_project(&store, project);

    let mut buf = GraphBuffer::new(project);

    // Add N = 7 nodes
    let qualified_names = vec![
        (
            "Function",
            "entrypoint",
            "combined_test.entrypoint",
            "src/index.ts",
        ),
        (
            "Class",
            "OrderService",
            "combined_test.OrderService",
            "src/order.ts",
        ),
        (
            "Method",
            "placeOrder",
            "combined_test.OrderService.placeOrder",
            "src/order.ts",
        ),
        (
            "Method",
            "cancelOrder",
            "combined_test.OrderService.cancelOrder",
            "src/order.ts",
        ),
        (
            "Class",
            "PaymentGateway",
            "combined_test.PaymentGateway",
            "src/payment.ts",
        ),
        (
            "Method",
            "charge",
            "combined_test.PaymentGateway.charge",
            "src/payment.ts",
        ),
        ("Module", "config", "combined_test.config", "src/config.ts"),
    ];

    let n = qualified_names.len();

    for (i, (label, name, qn, file_path)) in qualified_names.iter().enumerate() {
        buf.add_node(
            label,
            name,
            qn,
            file_path,
            (i as i32) + 1,
            (i as i32) + 15,
            None,
        );
    }

    // Add M = 6 edges: mix of QN-based and edges with confidence
    // QN-based edges
    buf.add_edge_by_qn(
        "combined_test.entrypoint",
        "combined_test.OrderService.placeOrder",
        "CALLS",
        None,
    );
    buf.add_edge_by_qn(
        "combined_test.OrderService.placeOrder",
        "combined_test.PaymentGateway.charge",
        "CALLS",
        None,
    );
    buf.add_edge_by_qn(
        "combined_test.entrypoint",
        "combined_test.config",
        "IMPORTS",
        None,
    );

    // QN-based edges with confidence metadata
    buf.add_edge_with_confidence(
        "combined_test.OrderService",
        "combined_test.OrderService.placeOrder",
        "CONTAINS",
        EdgeSource::AstStructural,
        None,
    );
    buf.add_edge_with_confidence(
        "combined_test.OrderService",
        "combined_test.OrderService.cancelOrder",
        "CONTAINS",
        EdgeSource::AstStructural,
        None,
    );
    buf.add_edge_with_confidence(
        "combined_test.PaymentGateway",
        "combined_test.PaymentGateway.charge",
        "CONTAINS",
        EdgeSource::AstStructural,
        None,
    );

    let m = buf.edge_count();
    assert_eq!(m, 6);
    assert_eq!(buf.node_count(), n);

    // Flush
    buf.flush(&store).unwrap();

    // Verify all N nodes retrievable by qualified name
    for (_label, _name, qn, _file_path) in &qualified_names {
        let node = store.find_node_by_qn(project, qn).unwrap();
        assert!(node.is_some(), "Node '{}' should exist after flush", qn);
        let node = node.unwrap();
        assert!(node.id > 0, "Node '{}' should have id > 0", qn);
    }

    // Verify all M edges have resolved IDs > 0
    let edges = store.get_edges(project, 100).unwrap();
    assert_eq!(edges.len(), m, "Expected {} edges, got {}", m, edges.len());

    for edge in &edges {
        assert!(
            edge.source_id > 0,
            "Edge '{}' source_id should be > 0, got {}",
            edge.edge_type,
            edge.source_id
        );
        assert!(
            edge.target_id > 0,
            "Edge '{}' target_id should be > 0, got {}",
            edge.edge_type,
            edge.target_id
        );
    }
}

/// Test that edges referencing pre-existing nodes in the store (not in the current buffer)
/// are correctly resolved via store lookup during flush.
#[test]
fn test_flush_resolves_edges_to_preexisting_store_nodes() {
    let store = test_store();
    let project = "preexist_test";
    setup_project(&store, project);

    // Pre-insert some nodes directly into the store (simulating a previous indexing run)
    let mut setup_buf = GraphBuffer::new(project);
    setup_buf.add_node(
        "Class",
        "Database",
        "preexist_test.Database",
        "src/db.ts",
        1,
        30,
        None,
    );
    setup_buf.add_node(
        "Method",
        "query",
        "preexist_test.Database.query",
        "src/db.ts",
        5,
        20,
        None,
    );
    setup_buf.flush(&store).unwrap();

    // Now create a new buffer with new nodes that reference the pre-existing ones
    let mut buf = GraphBuffer::new(project);
    buf.seed_ids_from_store(&store).unwrap();

    buf.add_node(
        "Function",
        "fetchUsers",
        "preexist_test.fetchUsers",
        "src/api.ts",
        1,
        10,
        None,
    );
    buf.add_node(
        "Function",
        "fetchOrders",
        "preexist_test.fetchOrders",
        "src/api.ts",
        12,
        25,
        None,
    );

    // Add edges from new nodes to pre-existing nodes
    buf.add_edge_by_qn(
        "preexist_test.fetchUsers",
        "preexist_test.Database.query",
        "CALLS",
        None,
    );
    buf.add_edge_by_qn(
        "preexist_test.fetchOrders",
        "preexist_test.Database.query",
        "CALLS",
        None,
    );

    let m = buf.edge_count();
    assert_eq!(m, 2);

    buf.flush(&store).unwrap();

    // Verify edges resolved correctly
    let edges = store.get_edges_by_type(project, "CALLS").unwrap();
    assert_eq!(edges.len(), 2, "Expected 2 CALLS edges");

    for edge in &edges {
        assert!(
            edge.source_id > 0,
            "CALLS edge source_id should be > 0, got {}",
            edge.source_id
        );
        assert!(
            edge.target_id > 0,
            "CALLS edge target_id should be > 0, got {}",
            edge.target_id
        );

        // Verify target is the pre-existing Database.query node
        let target_node = store
            .find_node_by_qn(project, "preexist_test.Database.query")
            .unwrap()
            .unwrap();
        assert_eq!(edge.target_id, target_node.id);
    }
}

/// Test that edges with unresolvable QNs are dropped (not stored with id=0).
#[test]
fn test_flush_drops_unresolvable_edges() {
    let store = test_store();
    let project = "unresolvable_test";
    setup_project(&store, project);

    let mut buf = GraphBuffer::new(project);

    buf.add_node(
        "Function",
        "caller",
        "unresolvable_test.caller",
        "src/a.ts",
        1,
        5,
        None,
    );

    // Add an edge to a non-existent target
    buf.add_edge_by_qn(
        "unresolvable_test.caller",
        "unresolvable_test.nonexistent_target",
        "CALLS",
        None,
    );

    // Also add a valid edge
    buf.add_node(
        "Function",
        "callee",
        "unresolvable_test.callee",
        "src/b.ts",
        1,
        5,
        None,
    );
    buf.add_edge_by_qn(
        "unresolvable_test.caller",
        "unresolvable_test.callee",
        "CALLS",
        None,
    );

    buf.flush(&store).unwrap();

    // Only the valid edge should be stored
    let edges = store.get_edges(project, 100).unwrap();
    assert_eq!(
        edges.len(),
        1,
        "Only resolvable edges should be stored, got {}",
        edges.len()
    );
    assert!(edges[0].source_id > 0);
    assert!(edges[0].target_id > 0);
}

/// Test flush with a larger number of nodes and edges to verify scalability.
#[test]
fn test_flush_many_nodes_and_edges() {
    let store = test_store();
    let project = "scale_test";
    setup_project(&store, project);

    let mut buf = GraphBuffer::new(project);

    let n = 50;
    let mut qns = Vec::new();

    // Add N = 50 nodes
    for i in 0..n {
        let qn = format!("scale_test.mod{}.fn_{}", i / 10, i);
        buf.add_node(
            "Function",
            &format!("fn_{}", i),
            &qn,
            &format!("src/mod{}/file_{}.ts", i / 10, i),
            1,
            10 + i as i32,
            None,
        );
        qns.push(qn);
    }

    // Add M edges: each function calls the next one (chain)
    let m = n - 1;
    for i in 0..m {
        buf.add_edge_by_qn(&qns[i], &qns[i + 1], "CALLS", None);
    }

    assert_eq!(buf.node_count(), n);
    assert_eq!(buf.edge_count(), m);

    buf.flush(&store).unwrap();

    // Verify all N nodes retrievable
    for qn in &qns {
        let node = store.find_node_by_qn(project, qn).unwrap();
        assert!(node.is_some(), "Node '{}' should exist", qn);
        assert!(node.unwrap().id > 0);
    }

    // Verify all M edges have resolved IDs
    let edges = store.get_edges(project, 200).unwrap();
    assert_eq!(edges.len(), m, "Expected {} edges, got {}", m, edges.len());

    for edge in &edges {
        assert!(edge.source_id > 0, "Edge source_id should be > 0");
        assert!(edge.target_id > 0, "Edge target_id should be > 0");
    }
}
