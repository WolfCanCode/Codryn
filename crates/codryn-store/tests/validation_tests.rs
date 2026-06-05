use codryn_store::{Edge, Node, Project, Store};

// ── Helpers ───────────────────────────────────────────────────────────────────

fn test_store() -> Store {
    Store::open_in_memory().unwrap()
}

fn setup_project(s: &Store) {
    s.upsert_project(&Project {
        name: "p".into(),
        indexed_at: "now".into(),
        root_path: "/tmp".into(),
    })
    .unwrap();
}

fn make_node(project: &str, name: &str, qn: &str, file_path: &str) -> Node {
    Node {
        id: 0,
        project: project.into(),
        label: "Function".into(),
        name: name.into(),
        qualified_name: qn.into(),
        file_path: file_path.into(),
        start_line: 1,
        end_line: 10,
        properties_json: None,
    }
}

fn make_edge(project: &str, source_id: i64, target_id: i64) -> Edge {
    Edge {
        id: 0,
        project: project.into(),
        source_id,
        target_id,
        edge_type: "CALLS".into(),
        properties_json: None,
    }
}

/// Insert a node bypassing the UNIQUE(project, qualified_name) constraint.
///
/// The nodes table has an inline UNIQUE constraint that cannot be dropped at
/// runtime. To simulate a corrupted graph with duplicate QNs (which the
/// validator must detect), we recreate the table without the constraint,
/// insert the duplicate row, then restore the original schema.
///
/// This is intentionally test-only and uses PRAGMA writable_schema.
fn insert_duplicate_node(s: &Store, project: &str, name: &str, qn: &str, file_path: &str) {
    s.conn()
        .execute_batch(&format!(
            "PRAGMA writable_schema = ON;\
             CREATE TABLE IF NOT EXISTS _nodes_tmp (\
               id INTEGER PRIMARY KEY AUTOINCREMENT,\
               project TEXT NOT NULL,\
               label TEXT NOT NULL,\
               name TEXT NOT NULL,\
               qualified_name TEXT NOT NULL,\
               file_path TEXT DEFAULT '',\
               start_line INTEGER DEFAULT 0,\
               end_line INTEGER DEFAULT 0,\
               properties TEXT DEFAULT '{{}}'\
             );\
             INSERT INTO _nodes_tmp SELECT * FROM nodes;\
             INSERT INTO _nodes_tmp \
               (project, label, name, qualified_name, file_path, start_line, end_line, properties) \
               VALUES ('{project}', 'Function', '{name}', '{qn}', '{file_path}', 1, 10, '{{}}');\
             DROP TABLE nodes;\
             ALTER TABLE _nodes_tmp RENAME TO nodes;\
             PRAGMA writable_schema = OFF;"
        ))
        .unwrap();
}

/// Set a node's properties column to an invalid JSON string.
///
/// The expression index `idx_nodes_properties_complexity` uses
/// `json_extract(properties, '$.complexity')`, which SQLite evaluates on every
/// UPDATE. Storing malformed JSON would normally fail because of this index.
/// We drop the expression indexes before the update, simulating a graph that
/// was corrupted outside of normal write paths. The indexes are not recreated
/// because SQLite would fail to build them over the now-invalid row.
fn set_invalid_properties(s: &Store, node_id: i64, bad_json: &str) {
    s.conn()
        .execute_batch(
            "DROP INDEX IF EXISTS idx_nodes_properties_test;\
             DROP INDEX IF EXISTS idx_nodes_properties_exported;\
             DROP INDEX IF EXISTS idx_nodes_properties_complexity;",
        )
        .unwrap();
    s.conn()
        .execute(
            "UPDATE nodes SET properties = ?1 WHERE id = ?2",
            rusqlite::params![bad_json, node_id],
        )
        .unwrap();
    // Note: expression indexes are intentionally not recreated here because
    // SQLite would fail to build them over the row with invalid JSON.
    // The validation logic does not depend on these indexes.
}

// ── Test 1: Clean graph → total_issues = 0 ───────────────────────────────────

#[test]
fn clean_graph_has_no_issues() {
    let s = test_store();
    setup_project(&s);

    let id1 = s
        .insert_node(&make_node("p", "foo", "p.foo", "src/foo.rs"))
        .unwrap();
    let id2 = s
        .insert_node(&make_node("p", "bar", "p.bar", "src/bar.rs"))
        .unwrap();
    s.insert_edge(&make_edge("p", id1, id2)).unwrap();

    let report = s.validate_graph("p").unwrap();
    assert_eq!(report.total_issues, 0, "clean graph should have no issues");
    assert!(report.dangling_edges.is_empty());
    assert!(report.orphan_nodes.is_empty());
    assert!(report.duplicate_qns.is_empty());
    assert!(report.missing_properties.is_empty());
    assert!(report.invalid_properties_json.is_empty());
    assert!(report.self_loops.is_empty());
    assert!(report.cross_project_edges.is_empty());
}

// ── Test 2: Dangling edge — source_id doesn't exist ──────────────────────────

#[test]
fn dangling_edge_missing_source_detected() {
    let s = test_store();
    setup_project(&s);

    let id1 = s
        .insert_node(&make_node("p", "foo", "p.foo", "src/foo.rs"))
        .unwrap();

    // Insert edge with a non-existent source_id using raw SQL (FK off in bulk mode)
    s.enable_bulk_indexing_mode().unwrap();
    s.conn()
        .execute(
            "INSERT INTO edges (project, source_id, target_id, type, properties) \
             VALUES ('p', 9999, ?1, 'CALLS', '{}')",
            rusqlite::params![id1],
        )
        .unwrap();
    s.disable_bulk_indexing_mode().unwrap();

    let report = s.validate_graph("p").unwrap();
    assert!(
        !report.dangling_edges.is_empty(),
        "should detect dangling edge with missing source"
    );
    assert!(report.total_issues > 0);
}

// ── Test 3: Dangling edge — target_id doesn't exist ──────────────────────────

#[test]
fn dangling_edge_missing_target_detected() {
    let s = test_store();
    setup_project(&s);

    let id1 = s
        .insert_node(&make_node("p", "foo", "p.foo", "src/foo.rs"))
        .unwrap();

    // Insert edge with a non-existent target_id using raw SQL
    s.enable_bulk_indexing_mode().unwrap();
    s.conn()
        .execute(
            "INSERT INTO edges (project, source_id, target_id, type, properties) \
             VALUES ('p', ?1, 9999, 'CALLS', '{}')",
            rusqlite::params![id1],
        )
        .unwrap();
    s.disable_bulk_indexing_mode().unwrap();

    let report = s.validate_graph("p").unwrap();
    assert!(
        !report.dangling_edges.is_empty(),
        "should detect dangling edge with missing target"
    );
    assert!(report.total_issues > 0);
}

// ── Test 4: Orphan node (no edges) ───────────────────────────────────────────

#[test]
fn orphan_node_detected() {
    let s = test_store();
    setup_project(&s);

    let id1 = s
        .insert_node(&make_node("p", "foo", "p.foo", "src/foo.rs"))
        .unwrap();
    let id2 = s
        .insert_node(&make_node("p", "bar", "p.bar", "src/bar.rs"))
        .unwrap();
    let _orphan_id = s
        .insert_node(&make_node("p", "orphan", "p.orphan", "src/orphan.rs"))
        .unwrap();

    s.insert_edge(&make_edge("p", id1, id2)).unwrap();

    let report = s.validate_graph("p").unwrap();
    assert_eq!(
        report.orphan_nodes.len(),
        1,
        "should detect exactly one orphan node"
    );
    assert!(report.total_issues > 0);
}

#[test]
fn no_orphan_when_all_nodes_have_edges() {
    let s = test_store();
    setup_project(&s);

    let id1 = s
        .insert_node(&make_node("p", "foo", "p.foo", "src/foo.rs"))
        .unwrap();
    let id2 = s
        .insert_node(&make_node("p", "bar", "p.bar", "src/bar.rs"))
        .unwrap();
    s.insert_edge(&make_edge("p", id1, id2)).unwrap();

    let report = s.validate_graph("p").unwrap();
    assert!(
        report.orphan_nodes.is_empty(),
        "no orphans when all nodes have edges"
    );
}

// ── Test 5: Duplicate qualified names ────────────────────────────────────────

#[test]
fn duplicate_qualified_names_detected() {
    let s = test_store();
    setup_project(&s);

    // Insert first node normally
    s.insert_node(&make_node("p", "foo", "p.foo", "src/foo.rs"))
        .unwrap();

    // Insert a second node with the same QN, bypassing the UNIQUE constraint
    // to simulate a corrupted graph state.
    insert_duplicate_node(&s, "p", "foo", "p.foo", "src/foo_copy.rs");

    let report = s.validate_graph("p").unwrap();
    assert_eq!(
        report.duplicate_qns.len(),
        1,
        "should detect one duplicate QN group"
    );
    let (qn, ids) = &report.duplicate_qns[0];
    assert_eq!(qn, "p.foo");
    assert_eq!(ids.len(), 2, "duplicate group should contain 2 node IDs");
    // total_issues counts duplicates as (ids.len() - 1) per group = 1
    assert!(report.total_issues > 0);
}

// ── Test 6: Node with empty name ─────────────────────────────────────────────

#[test]
fn missing_name_detected() {
    let s = test_store();
    setup_project(&s);

    // Insert a node with empty name via raw SQL
    s.conn()
        .execute(
            "INSERT INTO nodes \
             (project, label, name, qualified_name, file_path, start_line, end_line, properties) \
             VALUES ('p', 'Function', '', 'p.empty_name', 'src/x.rs', 1, 10, '{}')",
            [],
        )
        .unwrap();

    let report = s.validate_graph("p").unwrap();
    let name_issues: Vec<_> = report
        .missing_properties
        .iter()
        .filter(|(_, field)| field == "name")
        .collect();
    assert_eq!(name_issues.len(), 1, "should detect empty name");
    assert!(report.total_issues > 0);
}

// ── Test 7: Node with empty qualified_name ────────────────────────────────────

#[test]
fn missing_qualified_name_detected() {
    let s = test_store();
    setup_project(&s);

    s.conn()
        .execute(
            "INSERT INTO nodes \
             (project, label, name, qualified_name, file_path, start_line, end_line, properties) \
             VALUES ('p', 'Function', 'foo', '', 'src/x.rs', 1, 10, '{}')",
            [],
        )
        .unwrap();

    let report = s.validate_graph("p").unwrap();
    let qn_issues: Vec<_> = report
        .missing_properties
        .iter()
        .filter(|(_, field)| field == "qualified_name")
        .collect();
    assert_eq!(qn_issues.len(), 1, "should detect empty qualified_name");
    assert!(report.total_issues > 0);
}

// ── Test 8: Node with empty file_path ────────────────────────────────────────

#[test]
fn missing_file_path_detected() {
    let s = test_store();
    setup_project(&s);

    s.insert_node(&Node {
        id: 0,
        project: "p".into(),
        label: "Function".into(),
        name: "foo".into(),
        qualified_name: "p.foo".into(),
        file_path: "".into(), // empty file_path
        start_line: 1,
        end_line: 10,
        properties_json: None,
    })
    .unwrap();

    let report = s.validate_graph("p").unwrap();
    let fp_issues: Vec<_> = report
        .missing_properties
        .iter()
        .filter(|(_, field)| field == "file_path")
        .collect();
    assert_eq!(fp_issues.len(), 1, "should detect empty file_path");
    assert!(report.total_issues > 0);
}

// ── Test 9: Node with invalid JSON in properties ──────────────────────────────

#[test]
fn invalid_properties_json_detected() {
    let s = test_store();
    setup_project(&s);

    let id = s
        .insert_node(&make_node("p", "foo", "p.foo", "src/foo.rs"))
        .unwrap();

    // Corrupt the properties column with invalid JSON.
    // We must drop expression indexes first because SQLite evaluates them on UPDATE.
    set_invalid_properties(&s, id, "not-valid-json");

    let report = s.validate_graph("p").unwrap();
    assert!(
        report.invalid_properties_json.contains(&id),
        "should detect node with invalid properties JSON"
    );
    assert!(report.total_issues > 0);
}

#[test]
fn valid_properties_json_not_flagged() {
    let s = test_store();
    setup_project(&s);

    let id1 = s
        .insert_node(&Node {
            id: 0,
            project: "p".into(),
            label: "Function".into(),
            name: "foo".into(),
            qualified_name: "p.foo".into(),
            file_path: "src/foo.rs".into(),
            start_line: 1,
            end_line: 10,
            properties_json: Some(r#"{"cyclomatic_complexity": 3}"#.into()),
        })
        .unwrap();
    let id2 = s
        .insert_node(&make_node("p", "bar", "p.bar", "src/bar.rs"))
        .unwrap();
    s.insert_edge(&make_edge("p", id1, id2)).unwrap();

    let report = s.validate_graph("p").unwrap();
    assert!(
        !report.invalid_properties_json.contains(&id1),
        "valid JSON properties should not be flagged"
    );
}

// ── Test 10: Self-loop edge (source_id == target_id) ─────────────────────────

#[test]
fn self_loop_detected() {
    let s = test_store();
    setup_project(&s);

    let id = s
        .insert_node(&make_node("p", "foo", "p.foo", "src/foo.rs"))
        .unwrap();

    // Insert a self-loop edge directly (the schema doesn't prevent self-loops)
    s.conn()
        .execute(
            "INSERT INTO edges (project, source_id, target_id, type, properties) \
             VALUES ('p', ?1, ?1, 'CALLS', '{}')",
            rusqlite::params![id],
        )
        .unwrap();

    let report = s.validate_graph("p").unwrap();
    assert!(
        !report.self_loops.is_empty(),
        "should detect self-loop edge"
    );
    assert!(report.total_issues > 0);
}

// ── Test 11: Cross-project edge ───────────────────────────────────────────────

#[test]
fn cross_project_edge_detected() {
    let s = test_store();
    setup_project(&s);
    s.upsert_project(&Project {
        name: "q".into(),
        indexed_at: "now".into(),
        root_path: "/tmp/q".into(),
    })
    .unwrap();

    let id_p = s
        .insert_node(&make_node("p", "foo", "p.foo", "src/foo.rs"))
        .unwrap();
    let id_q = s
        .insert_node(&make_node("q", "bar", "q.bar", "src/bar.rs"))
        .unwrap();

    // Insert an edge in project "p" that references a node from project "q"
    s.enable_bulk_indexing_mode().unwrap();
    s.conn()
        .execute(
            "INSERT INTO edges (project, source_id, target_id, type, properties) \
             VALUES ('p', ?1, ?2, 'CALLS', '{}')",
            rusqlite::params![id_p, id_q],
        )
        .unwrap();
    s.disable_bulk_indexing_mode().unwrap();

    let report = s.validate_graph("p").unwrap();
    assert!(
        !report.cross_project_edges.is_empty(),
        "should detect cross-project edge"
    );
    assert!(report.total_issues > 0);
}

// ── Test 12: fix_safe removes dangling edges ──────────────────────────────────

#[test]
fn fix_safe_removes_dangling_edges() {
    let s = test_store();
    setup_project(&s);

    let id1 = s
        .insert_node(&make_node("p", "foo", "p.foo", "src/foo.rs"))
        .unwrap();

    // Insert a dangling edge (target doesn't exist)
    s.enable_bulk_indexing_mode().unwrap();
    s.conn()
        .execute(
            "INSERT INTO edges (project, source_id, target_id, type, properties) \
             VALUES ('p', ?1, 9999, 'CALLS', '{}')",
            rusqlite::params![id1],
        )
        .unwrap();
    s.disable_bulk_indexing_mode().unwrap();

    let report_before = s.validate_graph("p").unwrap();
    assert!(
        !report_before.dangling_edges.is_empty(),
        "dangling edge should be present before fix"
    );

    let fixes = s.fix_safe("p").unwrap();
    assert!(fixes > 0, "fix_safe should report at least one fix");

    let report_after = s.validate_graph("p").unwrap();
    assert!(
        report_after.dangling_edges.is_empty(),
        "dangling edges should be removed after fix_safe"
    );
}

// ── Test 13: fix_safe nullifies invalid properties_json ──────────────────────

#[test]
fn fix_safe_nullifies_invalid_properties_json() {
    let s = test_store();
    setup_project(&s);

    let id = s
        .insert_node(&make_node("p", "foo", "p.foo", "src/foo.rs"))
        .unwrap();

    // Corrupt the properties (drop expression indexes first)
    set_invalid_properties(&s, id, "not-valid-json");

    let report_before = s.validate_graph("p").unwrap();
    assert!(
        report_before.invalid_properties_json.contains(&id),
        "invalid JSON should be detected before fix"
    );

    let fixes = s.fix_safe("p").unwrap();
    assert!(fixes > 0, "fix_safe should report at least one fix");

    let report_after = s.validate_graph("p").unwrap();
    assert!(
        !report_after.invalid_properties_json.contains(&id),
        "invalid properties_json should be nullified after fix_safe"
    );
}

// ── Test 14: fix_safe does NOT merge duplicate nodes ─────────────────────────

#[test]
fn fix_safe_does_not_merge_duplicate_nodes() {
    let s = test_store();
    setup_project(&s);

    // Insert first node normally, then a duplicate bypassing the UNIQUE constraint
    s.insert_node(&make_node("p", "foo", "p.foo", "src/foo.rs"))
        .unwrap();
    insert_duplicate_node(&s, "p", "foo", "p.foo", "src/foo_dup.rs");

    let report_before = s.validate_graph("p").unwrap();
    assert_eq!(
        report_before.duplicate_qns.len(),
        1,
        "should detect duplicate QNs before fix_safe"
    );

    s.fix_safe("p").unwrap();

    // Duplicates should still be present — fix_safe does NOT merge them
    let report_after = s.validate_graph("p").unwrap();
    assert_eq!(
        report_after.duplicate_qns.len(),
        1,
        "fix_safe must NOT merge duplicate nodes — duplicates should still be present"
    );
    let (_, ids) = &report_after.duplicate_qns[0];
    assert_eq!(
        ids.len(),
        2,
        "both duplicate nodes should still exist after fix_safe"
    );
}

// ── Test 15: Multiple issues in one graph ────────────────────────────────────

#[test]
fn multiple_issues_all_detected() {
    let s = test_store();
    setup_project(&s);

    // 1. Insert a node with empty file_path (missing_properties)
    let id_no_fp = s
        .insert_node(&Node {
            id: 0,
            project: "p".into(),
            label: "Function".into(),
            name: "no_fp".into(),
            qualified_name: "p.no_fp".into(),
            file_path: "".into(),
            start_line: 1,
            end_line: 5,
            properties_json: None,
        })
        .unwrap();

    // 2. Insert a node with invalid JSON properties
    let id_bad_json = s
        .insert_node(&make_node("p", "bad_json", "p.bad_json", "src/bad.rs"))
        .unwrap();
    set_invalid_properties(&s, id_bad_json, "{invalid");

    // 3. Insert a self-loop
    let id_loop = s
        .insert_node(&make_node("p", "loopy", "p.loopy", "src/loop.rs"))
        .unwrap();
    s.conn()
        .execute(
            "INSERT INTO edges (project, source_id, target_id, type, properties) \
             VALUES ('p', ?1, ?1, 'CALLS', '{}')",
            rusqlite::params![id_loop],
        )
        .unwrap();

    // 4. Insert a dangling edge (missing target)
    s.enable_bulk_indexing_mode().unwrap();
    s.conn()
        .execute(
            "INSERT INTO edges (project, source_id, target_id, type, properties) \
             VALUES ('p', ?1, 99999, 'CALLS', '{}')",
            rusqlite::params![id_no_fp],
        )
        .unwrap();
    s.disable_bulk_indexing_mode().unwrap();

    // 5. Insert an orphan node (no edges)
    let _orphan = s
        .insert_node(&make_node("p", "orphan", "p.orphan", "src/orphan.rs"))
        .unwrap();

    let report = s.validate_graph("p").unwrap();

    assert!(
        !report.missing_properties.is_empty(),
        "should detect missing file_path"
    );
    assert!(
        !report.invalid_properties_json.is_empty(),
        "should detect invalid JSON"
    );
    assert!(!report.self_loops.is_empty(), "should detect self-loop");
    assert!(
        !report.dangling_edges.is_empty(),
        "should detect dangling edge"
    );
    assert!(!report.orphan_nodes.is_empty(), "should detect orphan node");
    assert!(
        report.total_issues >= 5,
        "total_issues should reflect all detected problems, got {}",
        report.total_issues
    );
}
