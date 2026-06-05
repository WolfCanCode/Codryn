use codryn_store::{Node, Project, Store};

fn test_store() -> Store {
    Store::open_in_memory().unwrap()
}

fn setup_project(store: &Store) {
    store
        .upsert_project(&Project {
            name: "p".into(),
            indexed_at: "now".into(),
            root_path: "/tmp".into(),
        })
        .unwrap();
}

fn insert_node_with_props(store: &Store, name: &str, label: &str, props: &str) -> i64 {
    store
        .insert_node(&Node {
            id: 0,
            project: "p".into(),
            label: label.into(),
            name: name.into(),
            qualified_name: format!("p.{}", name),
            file_path: "src/main.rs".into(),
            start_line: 1,
            end_line: 10,
            properties_json: Some(props.into()),
        })
        .unwrap()
}

#[test]
fn test_query_hotspots_returns_nodes_sorted_by_git_commits_desc() {
    let store = test_store();
    setup_project(&store);

    // Insert nodes with varying git_commits
    insert_node_with_props(
        &store,
        "low_activity",
        "Function",
        r#"{"git_commits": 3, "git_authors": 1, "git_last_modified": "2024-01-01T00:00:00Z"}"#,
    );
    insert_node_with_props(
        &store,
        "high_activity",
        "Function",
        r#"{"git_commits": 50, "git_authors": 5, "git_last_modified": "2024-06-15T10:00:00Z"}"#,
    );
    insert_node_with_props(
        &store,
        "medium_activity",
        "Class",
        r#"{"git_commits": 20, "git_authors": 3, "git_last_modified": "2024-03-10T08:30:00Z"}"#,
    );

    let results = store.query_hotspots("p", 10).unwrap();

    assert_eq!(results.len(), 3);
    // Should be sorted by git_commits descending
    assert_eq!(results[0].name, "high_activity");
    assert_eq!(results[0].git_commits, 50);
    assert_eq!(results[0].git_authors, 5);
    assert_eq!(results[0].git_last_modified, "2024-06-15T10:00:00Z");

    assert_eq!(results[1].name, "medium_activity");
    assert_eq!(results[1].git_commits, 20);
    assert_eq!(results[1].git_authors, 3);

    assert_eq!(results[2].name, "low_activity");
    assert_eq!(results[2].git_commits, 3);
    assert_eq!(results[2].git_authors, 1);
}

#[test]
fn test_query_hotspots_excludes_nodes_without_git_data() {
    let store = test_store();
    setup_project(&store);

    // Node with git data
    insert_node_with_props(
        &store,
        "with_git",
        "Function",
        r#"{"git_commits": 10, "git_authors": 2, "git_last_modified": "2024-01-15T10:00:00Z"}"#,
    );
    // Node without git data
    insert_node_with_props(
        &store,
        "without_git",
        "Function",
        r#"{"cyclomatic_complexity": 5}"#,
    );
    // Node with git_commits = 0 (should be excluded)
    insert_node_with_props(
        &store,
        "zero_commits",
        "Function",
        r#"{"git_commits": 0, "git_authors": 0, "git_last_modified": ""}"#,
    );

    let results = store.query_hotspots("p", 10).unwrap();

    assert_eq!(results.len(), 1);
    assert_eq!(results[0].name, "with_git");
}

#[test]
fn test_query_hotspots_returns_empty_when_no_git_data() {
    let store = test_store();
    setup_project(&store);

    // Only nodes without git data
    insert_node_with_props(
        &store,
        "no_git_1",
        "Function",
        r#"{"cyclomatic_complexity": 5}"#,
    );
    insert_node_with_props(&store, "no_git_2", "Class", r#"{"is_exported": true}"#);

    let results = store.query_hotspots("p", 10).unwrap();

    assert_eq!(results.len(), 0);
}

#[test]
fn test_query_hotspots_respects_limit() {
    let store = test_store();
    setup_project(&store);

    for i in 1..=10 {
        insert_node_with_props(
            &store,
            &format!("func_{}", i),
            "Function",
            &format!(
                r#"{{"git_commits": {}, "git_authors": 1, "git_last_modified": "2024-01-01T00:00:00Z"}}"#,
                i * 5
            ),
        );
    }

    let results = store.query_hotspots("p", 3).unwrap();

    assert_eq!(results.len(), 3);
    // Top 3 by git_commits desc: 50, 45, 40
    assert_eq!(results[0].git_commits, 50);
    assert_eq!(results[1].git_commits, 45);
    assert_eq!(results[2].git_commits, 40);
}

#[test]
fn test_query_hotspots_includes_label_field() {
    let store = test_store();
    setup_project(&store);

    insert_node_with_props(
        &store,
        "my_class",
        "Class",
        r#"{"git_commits": 15, "git_authors": 2, "git_last_modified": "2024-02-01T00:00:00Z"}"#,
    );
    insert_node_with_props(
        &store,
        "my_func",
        "Function",
        r#"{"git_commits": 25, "git_authors": 4, "git_last_modified": "2024-03-01T00:00:00Z"}"#,
    );

    let results = store.query_hotspots("p", 10).unwrap();

    assert_eq!(results[0].label, "Function");
    assert_eq!(results[1].label, "Class");
}
