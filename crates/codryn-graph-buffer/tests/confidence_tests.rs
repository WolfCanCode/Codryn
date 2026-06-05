/// Unit tests for confidence scoring (Task 12.3)
///
/// Covers:
/// 1. Each EdgeSource variant returns the correct confidence value
/// 2. add_edge() backward compatibility — defaults to AstNameMatch confidence
/// 3. add_edge_with_confidence() stores the correct confidence
/// 4. min_confidence filtering works correctly
/// 5. confidence and edge_source fields appear in query responses
use codryn_graph_buffer::{EdgeSource, GraphBuffer};
use codryn_store::{Edge, Node, Project, Store};

fn test_store() -> Store {
    Store::open_in_memory().unwrap()
}

fn setup_project(store: &Store) {
    store
        .upsert_project(&Project {
            name: "test".into(),
            indexed_at: "now".into(),
            root_path: "/test".into(),
        })
        .unwrap();
}

fn make_node(project: &str, name: &str, qn: &str) -> Node {
    Node {
        id: 0,
        project: project.to_owned(),
        label: "Function".into(),
        name: name.to_owned(),
        qualified_name: qn.to_owned(),
        file_path: "src/lib.rs".into(),
        start_line: 1,
        end_line: 10,
        properties_json: None,
    }
}

// ── 1. EdgeSource confidence values ──────────────────────────────────────────

#[test]
fn test_edge_source_compiler_index_confidence() {
    assert_eq!(EdgeSource::CompilerIndex.confidence(), 0.98);
}

#[test]
fn test_edge_source_external_lsp_confidence() {
    assert_eq!(EdgeSource::ExternalLsp.confidence(), 0.95);
}

#[test]
fn test_edge_source_ast_structural_confidence() {
    assert_eq!(EdgeSource::AstStructural.confidence(), 0.90);
}

#[test]
fn test_edge_source_dedicated_adapter_confidence() {
    assert_eq!(EdgeSource::DedicatedAdapter.confidence(), 0.85);
}

#[test]
fn test_edge_source_import_resolver_confidence() {
    assert_eq!(EdgeSource::ImportResolver.confidence(), 0.82);
}

#[test]
fn test_edge_source_ast_name_match_confidence() {
    assert_eq!(EdgeSource::AstNameMatch.confidence(), 0.60);
}

#[test]
fn test_edge_source_aho_corasick_match_confidence() {
    assert_eq!(EdgeSource::AhoCorasickMatch.confidence(), 0.55);
}

#[test]
fn test_edge_source_regex_match_confidence() {
    assert_eq!(EdgeSource::RegexMatch.confidence(), 0.45);
}

#[test]
fn test_edge_source_heuristic_confidence() {
    assert_eq!(EdgeSource::Heuristic.confidence(), 0.30);
}

/// All 9 variants are covered and confidence values are in (0, 1].
#[test]
fn test_all_edge_source_confidences_in_valid_range() {
    let variants = [
        EdgeSource::CompilerIndex,
        EdgeSource::ExternalLsp,
        EdgeSource::AstStructural,
        EdgeSource::DedicatedAdapter,
        EdgeSource::ImportResolver,
        EdgeSource::AstNameMatch,
        EdgeSource::AhoCorasickMatch,
        EdgeSource::RegexMatch,
        EdgeSource::Heuristic,
    ];
    for v in variants {
        let c = v.confidence();
        assert!(
            c > 0.0 && c <= 1.0,
            "{:?} confidence {} is not in (0, 1]",
            v,
            c
        );
    }
}

/// Confidence values are strictly ordered as documented in the design.
#[test]
fn test_edge_source_confidence_ordering() {
    assert!(EdgeSource::CompilerIndex.confidence() > EdgeSource::ExternalLsp.confidence());
    assert!(EdgeSource::ExternalLsp.confidence() > EdgeSource::AstStructural.confidence());
    assert!(EdgeSource::AstStructural.confidence() > EdgeSource::DedicatedAdapter.confidence());
    assert!(EdgeSource::DedicatedAdapter.confidence() > EdgeSource::ImportResolver.confidence());
    assert!(EdgeSource::ImportResolver.confidence() > EdgeSource::AstNameMatch.confidence());
    assert!(EdgeSource::AstNameMatch.confidence() > EdgeSource::AhoCorasickMatch.confidence());
    assert!(EdgeSource::AhoCorasickMatch.confidence() > EdgeSource::RegexMatch.confidence());
    assert!(EdgeSource::RegexMatch.confidence() > EdgeSource::Heuristic.confidence());
}

/// EdgeSource::as_str() returns the expected string for each variant.
#[test]
fn test_edge_source_as_str() {
    assert_eq!(EdgeSource::AstStructural.as_str(), "AstStructural");
    assert_eq!(EdgeSource::AstNameMatch.as_str(), "AstNameMatch");
    assert_eq!(EdgeSource::ImportResolver.as_str(), "ImportResolver");
    assert_eq!(EdgeSource::DedicatedAdapter.as_str(), "DedicatedAdapter");
    assert_eq!(EdgeSource::ExternalLsp.as_str(), "ExternalLsp");
    assert_eq!(EdgeSource::CompilerIndex.as_str(), "CompilerIndex");
    assert_eq!(EdgeSource::AhoCorasickMatch.as_str(), "AhoCorasickMatch");
    assert_eq!(EdgeSource::RegexMatch.as_str(), "RegexMatch");
    assert_eq!(EdgeSource::Heuristic.as_str(), "Heuristic");
}

// ── 2. add_edge() backward compatibility ─────────────────────────────────────

/// add_edge() with no confidence metadata stores an edge without _confidence
/// in properties_json (backward-compatible: no confidence columns set).
/// After flush, the edge is stored and the store has the correct edge count.
#[test]
fn test_add_edge_backward_compatible_stores_edge() {
    let store = test_store();
    setup_project(&store);

    let src_id = store
        .insert_node(&make_node("test", "caller", "test.caller"))
        .unwrap();
    let tgt_id = store
        .insert_node(&make_node("test", "callee", "test.callee"))
        .unwrap();

    let mut buf = GraphBuffer::new("test");
    buf.add_edge(src_id, tgt_id, "CALLS", None);
    buf.flush(&store).unwrap();

    let schema = store.get_graph_schema("test").unwrap();
    assert_eq!(schema.total_edges, 1);
}

/// add_edge() does not embed _confidence in properties_json — the edge is
/// stored without a confidence value (NULL in the DB column).
#[test]
fn test_add_edge_no_confidence_in_properties() {
    let store = test_store();
    setup_project(&store);

    let src_id = store
        .insert_node(&make_node("test", "a", "test.a"))
        .unwrap();
    let tgt_id = store
        .insert_node(&make_node("test", "b", "test.b"))
        .unwrap();

    let mut buf = GraphBuffer::new("test");
    buf.add_edge(src_id, tgt_id, "CALLS", None);
    buf.flush(&store).unwrap();

    // Query the raw edge to check confidence column is NULL
    let conn = store.conn();
    let (confidence, edge_source): (Option<f64>, Option<String>) = conn
        .query_row(
            "SELECT confidence, edge_source FROM edges WHERE project = 'test'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();

    assert!(
        confidence.is_none(),
        "add_edge() should not set confidence, got {:?}",
        confidence
    );
    assert!(
        edge_source.is_none(),
        "add_edge() should not set edge_source, got {:?}",
        edge_source
    );
}

// ── 3. add_edge_with_confidence() stores correct confidence ──────────────────

/// add_edge_with_confidence() with AstStructural stores confidence=0.90
/// and edge_source="AstStructural" in the DB.
#[test]
fn test_add_edge_with_confidence_ast_structural() {
    let store = test_store();
    setup_project(&store);

    let mut buf = GraphBuffer::new("test");
    buf.add_node(
        "Function",
        "parent",
        "test.parent",
        "src/lib.rs",
        1,
        10,
        None,
    );
    buf.add_node(
        "Function",
        "child",
        "test.child",
        "src/lib.rs",
        12,
        20,
        None,
    );
    buf.add_edge_with_confidence(
        "test.parent",
        "test.child",
        "CONTAINS",
        EdgeSource::AstStructural,
        None,
    );
    buf.flush(&store).unwrap();

    let conn = store.conn();
    let (confidence, edge_source): (Option<f64>, Option<String>) = conn
        .query_row(
            "SELECT confidence, edge_source FROM edges WHERE project = 'test' AND type = 'CONTAINS'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();

    assert_eq!(confidence, Some(0.90));
    assert_eq!(edge_source.as_deref(), Some("AstStructural"));
}

/// add_edge_with_confidence() with CompilerIndex stores confidence=0.98.
#[test]
fn test_add_edge_with_confidence_compiler_index() {
    let store = test_store();
    setup_project(&store);

    let mut buf = GraphBuffer::new("test");
    buf.add_node("Function", "src_fn", "test.src_fn", "src/a.rs", 1, 5, None);
    buf.add_node("Function", "tgt_fn", "test.tgt_fn", "src/b.rs", 1, 5, None);
    buf.add_edge_with_confidence(
        "test.src_fn",
        "test.tgt_fn",
        "CALLS",
        EdgeSource::CompilerIndex,
        None,
    );
    buf.flush(&store).unwrap();

    let conn = store.conn();
    let (confidence, edge_source): (Option<f64>, Option<String>) = conn
        .query_row(
            "SELECT confidence, edge_source FROM edges WHERE project = 'test' AND type = 'CALLS'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();

    assert_eq!(confidence, Some(0.98));
    assert_eq!(edge_source.as_deref(), Some("CompilerIndex"));
}

/// add_edge_with_confidence() with Heuristic stores confidence=0.30.
#[test]
fn test_add_edge_with_confidence_heuristic() {
    let store = test_store();
    setup_project(&store);

    let mut buf = GraphBuffer::new("test");
    buf.add_node("Function", "fn_a", "test.fn_a", "src/a.rs", 1, 5, None);
    buf.add_node("Function", "fn_b", "test.fn_b", "src/b.rs", 1, 5, None);
    buf.add_edge_with_confidence(
        "test.fn_a",
        "test.fn_b",
        "CALLS",
        EdgeSource::Heuristic,
        None,
    );
    buf.flush(&store).unwrap();

    let conn = store.conn();
    let (confidence, edge_source): (Option<f64>, Option<String>) = conn
        .query_row(
            "SELECT confidence, edge_source FROM edges WHERE project = 'test' AND type = 'CALLS'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();

    assert_eq!(confidence, Some(0.30));
    assert_eq!(edge_source.as_deref(), Some("Heuristic"));
}

/// add_edge_with_source() (ID-based variant) stores the correct confidence.
#[test]
fn test_add_edge_with_source_stores_confidence() {
    let store = test_store();
    setup_project(&store);

    let src_id = store
        .insert_node(&make_node("test", "caller", "test.caller"))
        .unwrap();
    let tgt_id = store
        .insert_node(&make_node("test", "callee", "test.callee"))
        .unwrap();

    let mut buf = GraphBuffer::new("test");
    buf.add_edge_with_source(src_id, tgt_id, "CALLS", EdgeSource::ImportResolver, None);
    buf.flush(&store).unwrap();

    let conn = store.conn();
    let (confidence, edge_source): (Option<f64>, Option<String>) = conn
        .query_row(
            "SELECT confidence, edge_source FROM edges WHERE project = 'test'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();

    assert_eq!(confidence, Some(0.82));
    assert_eq!(edge_source.as_deref(), Some("ImportResolver"));
}

/// Extra properties_json passed to add_edge_with_confidence() are preserved
/// after flush (internal _confidence/_edge_source fields are stripped).
#[test]
fn test_add_edge_with_confidence_preserves_user_properties() {
    let store = test_store();
    setup_project(&store);

    let mut buf = GraphBuffer::new("test");
    buf.add_node("Function", "fn_x", "test.fn_x", "src/x.rs", 1, 5, None);
    buf.add_node("Function", "fn_y", "test.fn_y", "src/y.rs", 1, 5, None);
    buf.add_edge_with_confidence(
        "test.fn_x",
        "test.fn_y",
        "CALLS",
        EdgeSource::DedicatedAdapter,
        Some(r#"{"call_site": "line_42"}"#.to_owned()),
    );
    buf.flush(&store).unwrap();

    let conn = store.conn();
    let (confidence, edge_source, props): (Option<f64>, Option<String>, String) = conn
        .query_row(
            "SELECT confidence, edge_source, properties FROM edges WHERE project = 'test'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();

    assert_eq!(confidence, Some(0.85));
    assert_eq!(edge_source.as_deref(), Some("DedicatedAdapter"));
    // Internal fields should not leak into the stored properties column
    assert!(
        !props.contains("_confidence"),
        "properties should not contain _confidence"
    );
    assert!(
        !props.contains("_edge_source"),
        "properties should not contain _edge_source"
    );
}

// ── 4. min_confidence filtering ───────────────────────────────────────────────

/// Helper: insert two nodes and two edges with different confidence levels,
/// then return (store, high_conf_node_id, low_conf_node_id, target_node_id).
fn setup_confidence_graph() -> (Store, i64, i64, i64) {
    let store = test_store();
    setup_project(&store);

    let target_id = store
        .insert_node(&make_node("test", "target", "test.target"))
        .unwrap();
    let high_caller_id = store
        .insert_node(&make_node("test", "high_caller", "test.high_caller"))
        .unwrap();
    let low_caller_id = store
        .insert_node(&make_node("test", "low_caller", "test.low_caller"))
        .unwrap();

    // High-confidence edge: AstStructural (0.90)
    store
        .insert_edge(&Edge {
            id: 0,
            project: "test".into(),
            source_id: high_caller_id,
            target_id,
            edge_type: "CALLS".into(),
            properties_json: Some(r#"{"_confidence":0.90,"_edge_source":"AstStructural"}"#.into()),
        })
        .unwrap();

    // Low-confidence edge: Heuristic (0.30)
    store
        .insert_edge(&Edge {
            id: 0,
            project: "test".into(),
            source_id: low_caller_id,
            target_id,
            edge_type: "CALLS".into(),
            properties_json: Some(r#"{"_confidence":0.30,"_edge_source":"Heuristic"}"#.into()),
        })
        .unwrap();

    (store, high_caller_id, low_caller_id, target_id)
}

/// incoming_references_with_confidence(min_confidence=0.80) returns only the
/// high-confidence edge and excludes the low-confidence one.
#[test]
fn test_min_confidence_filters_low_confidence_edges() {
    let (store, _high_id, _low_id, target_id) = setup_confidence_graph();

    let refs = store
        .incoming_references_with_confidence(target_id, None, 100, Some(0.80))
        .unwrap();

    assert_eq!(refs.len(), 1, "expected 1 reference above 0.80 threshold");
    assert_eq!(refs[0].0.name, "high_caller");
}

/// incoming_references_with_confidence(min_confidence=None) returns all edges
/// regardless of confidence.
#[test]
fn test_no_min_confidence_returns_all_edges() {
    let (store, _high_id, _low_id, target_id) = setup_confidence_graph();

    let refs = store
        .incoming_references_with_confidence(target_id, None, 100, None)
        .unwrap();

    assert_eq!(refs.len(), 2, "expected both edges when no min_confidence");
}

/// incoming_references_with_confidence(min_confidence=0.0) returns all edges
/// (threshold of 0.0 means nothing is filtered).
#[test]
fn test_min_confidence_zero_returns_all_edges() {
    let (store, _high_id, _low_id, target_id) = setup_confidence_graph();

    let refs = store
        .incoming_references_with_confidence(target_id, None, 100, Some(0.0))
        .unwrap();

    assert_eq!(refs.len(), 2, "min_confidence=0.0 should return all edges");
}

/// incoming_references_with_confidence(min_confidence=1.0) returns no edges
/// when none have confidence >= 1.0.
#[test]
fn test_min_confidence_one_returns_no_edges() {
    let (store, _high_id, _low_id, target_id) = setup_confidence_graph();

    let refs = store
        .incoming_references_with_confidence(target_id, None, 100, Some(1.0))
        .unwrap();

    assert!(
        refs.is_empty(),
        "min_confidence=1.0 should return no edges (none have confidence=1.0)"
    );
}

/// impact_bfs_with_confidence(min_confidence=0.80) excludes low-confidence edges
/// from the BFS traversal.
#[test]
fn test_impact_bfs_min_confidence_filters_edges() {
    let (store, _high_id, _low_id, target_id) = setup_confidence_graph();

    let (direct, all, _files) = store
        .impact_bfs_with_confidence(target_id, 3, 100, Some(0.80))
        .unwrap();

    assert_eq!(direct.len(), 1, "expected 1 direct dependent above 0.80");
    assert_eq!(all.len(), 1, "expected 1 total dependent above 0.80");
    assert_eq!(direct[0].name, "high_caller");
}

/// impact_bfs_with_confidence(min_confidence=None) returns all dependents.
#[test]
fn test_impact_bfs_no_min_confidence_returns_all() {
    let (store, _high_id, _low_id, target_id) = setup_confidence_graph();

    let (direct, all, _files) = store
        .impact_bfs_with_confidence(target_id, 3, 100, None)
        .unwrap();

    assert_eq!(
        direct.len(),
        2,
        "expected 2 direct dependents with no filter"
    );
    assert_eq!(all.len(), 2);
}

/// Edges with NULL confidence are always included regardless of min_confidence
/// (NULL means "unknown", not "zero confidence").
#[test]
fn test_null_confidence_edges_always_included() {
    let store = test_store();
    setup_project(&store);

    let target_id = store
        .insert_node(&make_node("test", "target", "test.target"))
        .unwrap();
    let caller_id = store
        .insert_node(&make_node("test", "caller", "test.caller"))
        .unwrap();

    // Insert edge with NULL confidence (no properties_json)
    store
        .insert_edge(&Edge {
            id: 0,
            project: "test".into(),
            source_id: caller_id,
            target_id,
            edge_type: "CALLS".into(),
            properties_json: None,
        })
        .unwrap();

    // Even with a high min_confidence, NULL-confidence edges should be included
    let refs = store
        .incoming_references_with_confidence(target_id, None, 100, Some(0.99))
        .unwrap();

    assert_eq!(
        refs.len(),
        1,
        "NULL-confidence edges should be included regardless of min_confidence"
    );
}

// ── 5. confidence and edge_source in query responses ─────────────────────────

/// incoming_references_detailed() returns confidence and edge_source fields.
#[test]
fn test_incoming_references_detailed_includes_confidence_fields() {
    let (store, _high_id, _low_id, target_id) = setup_confidence_graph();

    let refs = store
        .incoming_references_detailed(target_id, None, 100, None)
        .unwrap();

    assert_eq!(refs.len(), 2);

    // Find the high-confidence reference
    let high_ref = refs
        .iter()
        .find(|(n, _, _, _)| n.name == "high_caller")
        .expect("high_caller should be in results");

    assert_eq!(
        high_ref.2,
        Some(0.90),
        "high_caller edge should have confidence=0.90"
    );
    assert_eq!(
        high_ref.3.as_deref(),
        Some("AstStructural"),
        "high_caller edge should have edge_source=AstStructural"
    );

    // Find the low-confidence reference
    let low_ref = refs
        .iter()
        .find(|(n, _, _, _)| n.name == "low_caller")
        .expect("low_caller should be in results");

    assert_eq!(
        low_ref.2,
        Some(0.30),
        "low_caller edge should have confidence=0.30"
    );
    assert_eq!(
        low_ref.3.as_deref(),
        Some("Heuristic"),
        "low_caller edge should have edge_source=Heuristic"
    );
}

/// incoming_references_detailed() with min_confidence filter returns only
/// matching edges, and those edges carry the correct confidence metadata.
#[test]
fn test_incoming_references_detailed_with_min_confidence() {
    let (store, _high_id, _low_id, target_id) = setup_confidence_graph();

    let refs = store
        .incoming_references_detailed(target_id, None, 100, Some(0.80))
        .unwrap();

    assert_eq!(refs.len(), 1);
    let (node, edge_type, confidence, edge_source) = &refs[0];
    assert_eq!(node.name, "high_caller");
    assert_eq!(edge_type, "CALLS");
    assert_eq!(*confidence, Some(0.90));
    assert_eq!(edge_source.as_deref(), Some("AstStructural"));
}

/// direct_dependents_with_confidence() returns confidence metadata.
#[test]
fn test_direct_dependents_with_confidence_returns_metadata() {
    let (store, _high_id, _low_id, target_id) = setup_confidence_graph();

    let deps = store
        .direct_dependents_with_confidence(target_id, 100, None)
        .unwrap();

    assert_eq!(deps.len(), 2);

    // All results should have confidence and edge_source populated
    for (node, _edge_type, confidence, edge_source) in &deps {
        assert!(
            confidence.is_some(),
            "node {} should have confidence set",
            node.name
        );
        assert!(
            edge_source.is_some(),
            "node {} should have edge_source set",
            node.name
        );
    }
}

/// Edges inserted via GraphBuffer::add_edge_with_confidence() have their
/// confidence and edge_source correctly readable via incoming_references_detailed().
#[test]
fn test_graph_buffer_confidence_readable_via_store_query() {
    let store = test_store();
    setup_project(&store);

    let mut buf = GraphBuffer::new("test");
    buf.add_node(
        "Function",
        "caller_fn",
        "test.caller_fn",
        "src/a.rs",
        1,
        5,
        None,
    );
    buf.add_node(
        "Function",
        "callee_fn",
        "test.callee_fn",
        "src/b.rs",
        1,
        5,
        None,
    );
    buf.add_edge_with_confidence(
        "test.caller_fn",
        "test.callee_fn",
        "CALLS",
        EdgeSource::ExternalLsp,
        None,
    );
    buf.flush(&store).unwrap();

    // Find the callee node id
    let callee = store
        .find_node_by_qn("test", "test.callee_fn")
        .unwrap()
        .expect("callee_fn should exist");

    let refs = store
        .incoming_references_detailed(callee.id, None, 100, None)
        .unwrap();

    assert_eq!(refs.len(), 1);
    let (node, _edge_type, confidence, edge_source) = &refs[0];
    assert_eq!(node.name, "caller_fn");
    assert_eq!(*confidence, Some(0.95));
    assert_eq!(edge_source.as_deref(), Some("ExternalLsp"));
}

/// Multiple edges with different EdgeSource variants are all stored correctly
/// and can be queried with min_confidence filtering.
#[test]
fn test_multiple_edge_sources_stored_and_filtered() {
    let store = test_store();
    setup_project(&store);

    let target_id = store
        .insert_node(&make_node("test", "target", "test.target"))
        .unwrap();

    // Insert edges for each EdgeSource variant
    let sources = [
        (EdgeSource::CompilerIndex, 0.98),
        (EdgeSource::ExternalLsp, 0.95),
        (EdgeSource::AstStructural, 0.90),
        (EdgeSource::DedicatedAdapter, 0.85),
        (EdgeSource::ImportResolver, 0.82),
        (EdgeSource::AstNameMatch, 0.60),
        (EdgeSource::AhoCorasickMatch, 0.55),
        (EdgeSource::RegexMatch, 0.45),
        (EdgeSource::Heuristic, 0.30),
    ];

    for (i, (source, _expected_conf)) in sources.iter().enumerate() {
        let caller_id = store
            .insert_node(&make_node(
                "test",
                &format!("caller_{}", i),
                &format!("test.caller_{}", i),
            ))
            .unwrap();
        store
            .insert_edge(&Edge {
                id: 0,
                project: "test".into(),
                source_id: caller_id,
                target_id,
                edge_type: "CALLS".into(),
                properties_json: Some(format!(
                    r#"{{"_confidence":{},"_edge_source":"{}"}}"#,
                    source.confidence(),
                    source.as_str()
                )),
            })
            .unwrap();
    }

    // All 9 edges should be returned with no filter
    let all_refs = store
        .incoming_references_with_confidence(target_id, None, 100, None)
        .unwrap();
    assert_eq!(all_refs.len(), 9, "expected all 9 edges");

    // Only edges with confidence >= 0.82 (ImportResolver and above = 5 variants)
    let high_refs = store
        .incoming_references_with_confidence(target_id, None, 100, Some(0.82))
        .unwrap();
    assert_eq!(
        high_refs.len(),
        5,
        "expected 5 edges with confidence >= 0.82 (CompilerIndex, ExternalLsp, AstStructural, DedicatedAdapter, ImportResolver)"
    );

    // Only edges with confidence >= 0.90 (AstStructural and above = 3 variants)
    let structural_refs = store
        .incoming_references_with_confidence(target_id, None, 100, Some(0.90))
        .unwrap();
    assert_eq!(
        structural_refs.len(),
        3,
        "expected 3 edges with confidence >= 0.90 (CompilerIndex, ExternalLsp, AstStructural)"
    );
}
