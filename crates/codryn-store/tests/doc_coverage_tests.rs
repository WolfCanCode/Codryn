use codryn_store::{Node, Project, Store};

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

/// Insert a symbol node with optional `has_docs` property.
fn insert_symbol(
    s: &Store,
    project: &str,
    label: &str,
    name: &str,
    qn: &str,
    file_path: &str,
    has_docs: Option<bool>,
) -> i64 {
    let props = has_docs.map(|v| format!(r#"{{"has_docs":{}}}"#, v));
    s.insert_node(&Node {
        id: 0,
        project: project.into(),
        label: label.into(),
        name: name.into(),
        qualified_name: qn.into(),
        file_path: file_path.into(),
        start_line: 1,
        end_line: 10,
        properties_json: props,
    })
    .unwrap()
}

// ── Coverage percentage calculation ──────────────────────────────────────────

#[test]
fn coverage_pct_all_documented() {
    let s = test_store();
    setup_project(&s);

    insert_symbol(&s, "p", "Function", "a", "p.a", "src/lib.rs", Some(true));
    insert_symbol(&s, "p", "Function", "b", "p.b", "src/lib.rs", Some(true));
    insert_symbol(&s, "p", "Function", "c", "p.c", "src/lib.rs", Some(true));

    let rows = s.query_doc_coverage("p", None).unwrap();
    assert_eq!(rows.len(), 1);
    let row = &rows[0];
    assert_eq!(row.total_symbols, 3);
    assert_eq!(row.documented_symbols, 3);
    assert!((row.coverage_pct - 100.0).abs() < 0.001);
    assert!(!row.needs_attention);
}

#[test]
fn coverage_pct_none_documented() {
    let s = test_store();
    setup_project(&s);

    insert_symbol(&s, "p", "Function", "a", "p.a", "src/lib.rs", Some(false));
    insert_symbol(&s, "p", "Function", "b", "p.b", "src/lib.rs", Some(false));

    let rows = s.query_doc_coverage("p", None).unwrap();
    assert_eq!(rows.len(), 1);
    let row = &rows[0];
    assert_eq!(row.total_symbols, 2);
    assert_eq!(row.documented_symbols, 0);
    assert!((row.coverage_pct - 0.0).abs() < 0.001);
    assert!(row.needs_attention);
}

#[test]
fn coverage_pct_partial_documented() {
    let s = test_store();
    setup_project(&s);

    // 2 documented, 3 undocumented = 40%
    insert_symbol(&s, "p", "Function", "a", "p.a", "src/lib.rs", Some(true));
    insert_symbol(&s, "p", "Function", "b", "p.b", "src/lib.rs", Some(true));
    insert_symbol(&s, "p", "Function", "c", "p.c", "src/lib.rs", Some(false));
    insert_symbol(&s, "p", "Function", "d", "p.d", "src/lib.rs", Some(false));
    insert_symbol(&s, "p", "Function", "e", "p.e", "src/lib.rs", Some(false));

    let rows = s.query_doc_coverage("p", None).unwrap();
    assert_eq!(rows.len(), 1);
    let row = &rows[0];
    assert_eq!(row.total_symbols, 5);
    assert_eq!(row.documented_symbols, 2);
    assert!((row.coverage_pct - 40.0).abs() < 0.001);
    assert!(row.needs_attention); // 40% < 50%
}

#[test]
fn coverage_pct_exactly_50_percent_does_not_need_attention() {
    let s = test_store();
    setup_project(&s);

    insert_symbol(&s, "p", "Function", "a", "p.a", "src/lib.rs", Some(true));
    insert_symbol(&s, "p", "Function", "b", "p.b", "src/lib.rs", Some(false));

    let rows = s.query_doc_coverage("p", None).unwrap();
    assert_eq!(rows.len(), 1);
    let row = &rows[0];
    assert!((row.coverage_pct - 50.0).abs() < 0.001);
    assert!(!row.needs_attention); // exactly 50% is NOT flagged
}

#[test]
fn coverage_pct_no_has_docs_property_counts_as_undocumented() {
    let s = test_store();
    setup_project(&s);

    // Node with no properties_json at all — should count as undocumented
    insert_symbol(&s, "p", "Function", "a", "p.a", "src/lib.rs", None);

    let rows = s.query_doc_coverage("p", None).unwrap();
    assert_eq!(rows.len(), 1);
    let row = &rows[0];
    assert_eq!(row.total_symbols, 1);
    assert_eq!(row.documented_symbols, 0);
    assert!((row.coverage_pct - 0.0).abs() < 0.001);
    assert!(row.needs_attention);
}

// ── Module grouping ───────────────────────────────────────────────────────────

#[test]
fn module_grouping_two_files() {
    let s = test_store();
    setup_project(&s);

    // Module A: 2 documented, 0 undocumented = 100%
    insert_symbol(
        &s,
        "p",
        "Function",
        "a1",
        "p.a1",
        "src/module_a.rs",
        Some(true),
    );
    insert_symbol(
        &s,
        "p",
        "Function",
        "a2",
        "p.a2",
        "src/module_a.rs",
        Some(true),
    );

    // Module B: 0 documented, 2 undocumented = 0%
    insert_symbol(
        &s,
        "p",
        "Function",
        "b1",
        "p.b1",
        "src/module_b.rs",
        Some(false),
    );
    insert_symbol(
        &s,
        "p",
        "Function",
        "b2",
        "p.b2",
        "src/module_b.rs",
        Some(false),
    );

    let rows = s.query_doc_coverage("p", None).unwrap();
    assert_eq!(rows.len(), 2, "should have one row per module");

    // Results are ordered by coverage ASC, so module_b (0%) comes first
    let module_b = rows.iter().find(|r| r.module.contains("module_b")).unwrap();
    let module_a = rows.iter().find(|r| r.module.contains("module_a")).unwrap();

    assert_eq!(module_a.total_symbols, 2);
    assert_eq!(module_a.documented_symbols, 2);
    assert!((module_a.coverage_pct - 100.0).abs() < 0.001);
    assert!(!module_a.needs_attention);

    assert_eq!(module_b.total_symbols, 2);
    assert_eq!(module_b.documented_symbols, 0);
    assert!((module_b.coverage_pct - 0.0).abs() < 0.001);
    assert!(module_b.needs_attention);
}

#[test]
fn module_grouping_three_files_ordered_by_coverage_asc() {
    let s = test_store();
    setup_project(&s);

    // low.rs: 0/2 = 0%
    insert_symbol(&s, "p", "Function", "l1", "p.l1", "src/low.rs", Some(false));
    insert_symbol(&s, "p", "Function", "l2", "p.l2", "src/low.rs", Some(false));

    // mid.rs: 1/2 = 50%
    insert_symbol(&s, "p", "Function", "m1", "p.m1", "src/mid.rs", Some(true));
    insert_symbol(&s, "p", "Function", "m2", "p.m2", "src/mid.rs", Some(false));

    // high.rs: 2/2 = 100%
    insert_symbol(&s, "p", "Function", "h1", "p.h1", "src/high.rs", Some(true));
    insert_symbol(&s, "p", "Function", "h2", "p.h2", "src/high.rs", Some(true));

    let rows = s.query_doc_coverage("p", None).unwrap();
    assert_eq!(rows.len(), 3);

    // First row should be the least-documented module (low.rs at 0%)
    assert!(rows[0].module.contains("low.rs"));
    assert!((rows[0].coverage_pct - 0.0).abs() < 0.001);

    // Last row should be the most-documented module (high.rs at 100%)
    assert!(rows[2].module.contains("high.rs"));
    assert!((rows[2].coverage_pct - 100.0).abs() < 0.001);
}

#[test]
fn module_filter_restricts_to_matching_files() {
    let s = test_store();
    setup_project(&s);

    insert_symbol(&s, "p", "Function", "a", "p.a", "src/api.rs", Some(true));
    insert_symbol(
        &s,
        "p",
        "Function",
        "b",
        "p.b",
        "src/internal.rs",
        Some(false),
    );

    // Filter to only "api" files
    let rows = s.query_doc_coverage("p", Some("api")).unwrap();
    assert_eq!(rows.len(), 1);
    assert!(rows[0].module.contains("api.rs"));
}

#[test]
fn non_public_labels_excluded_from_coverage() {
    let s = test_store();
    setup_project(&s);

    // Only Function, Method, Class, Interface are counted
    insert_symbol(&s, "p", "Function", "f", "p.f", "src/lib.rs", Some(true));
    // Module and File labels should be excluded
    insert_symbol(&s, "p", "Module", "m", "p.m", "src/lib.rs", Some(false));
    insert_symbol(&s, "p", "File", "file", "p.file", "src/lib.rs", Some(false));

    let rows = s.query_doc_coverage("p", None).unwrap();
    assert_eq!(rows.len(), 1);
    // Only the Function node is counted
    assert_eq!(rows[0].total_symbols, 1);
    assert_eq!(rows[0].documented_symbols, 1);
}

#[test]
fn empty_project_returns_no_rows() {
    let s = test_store();
    setup_project(&s);

    let rows = s.query_doc_coverage("p", None).unwrap();
    assert!(rows.is_empty());
}

#[test]
fn class_and_interface_labels_included() {
    let s = test_store();
    setup_project(&s);

    insert_symbol(
        &s,
        "p",
        "Class",
        "MyClass",
        "p.MyClass",
        "src/lib.rs",
        Some(true),
    );
    insert_symbol(
        &s,
        "p",
        "Interface",
        "MyIface",
        "p.MyIface",
        "src/lib.rs",
        Some(false),
    );
    insert_symbol(
        &s,
        "p",
        "Method",
        "myMethod",
        "p.myMethod",
        "src/lib.rs",
        Some(true),
    );

    let rows = s.query_doc_coverage("p", None).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].total_symbols, 3);
    assert_eq!(rows[0].documented_symbols, 2);
    assert!((rows[0].coverage_pct - (2.0 / 3.0 * 100.0)).abs() < 0.1);
}
