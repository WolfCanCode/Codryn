use codryn_store::{MetadataFilter, Node, Project, Store};

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

/// Seed the store with a variety of nodes for filtering tests.
fn seed_nodes(store: &Store) {
    setup_project(store);

    // test function, exported, complexity 2
    insert_node_with_props(
        store,
        "test_login",
        "Function",
        r#"{"is_test":true,"is_exported":true,"complexity":2}"#,
    );
    // non-test function, exported, complexity 8
    insert_node_with_props(
        store,
        "handle_request",
        "Function",
        r#"{"is_test":false,"is_exported":true,"complexity":8}"#,
    );
    // non-test method, not exported, complexity 15
    insert_node_with_props(
        store,
        "process_data",
        "Method",
        r#"{"is_test":false,"is_exported":false,"complexity":15}"#,
    );
    // test method, not exported, complexity 3
    insert_node_with_props(
        store,
        "test_process",
        "Method",
        r#"{"is_test":true,"is_exported":false,"complexity":3}"#,
    );
    // entry point function, exported, complexity 1
    insert_node_with_props(
        store,
        "main",
        "Function",
        r#"{"is_test":false,"is_exported":true,"is_entry_point":true,"complexity":1}"#,
    );
    // class node, no test/exported metadata
    insert_node_with_props(store, "UserService", "Class", r#"{"complexity":5}"#);
}

#[test]
fn test_filter_by_is_test_true() {
    let store = test_store();
    seed_nodes(&store);

    let results = store
        .get_nodes_by_metadata(
            "p",
            &MetadataFilter {
                is_test: Some(true),
                ..Default::default()
            },
            100,
        )
        .unwrap();

    assert_eq!(results.len(), 2);
    let names: Vec<&str> = results.iter().map(|n| n.name.as_str()).collect();
    assert!(names.contains(&"test_login"));
    assert!(names.contains(&"test_process"));
}

#[test]
fn test_filter_by_is_test_false() {
    let store = test_store();
    seed_nodes(&store);

    let results = store
        .get_nodes_by_metadata(
            "p",
            &MetadataFilter {
                is_test: Some(false),
                ..Default::default()
            },
            100,
        )
        .unwrap();

    assert_eq!(results.len(), 3);
    let names: Vec<&str> = results.iter().map(|n| n.name.as_str()).collect();
    assert!(names.contains(&"handle_request"));
    assert!(names.contains(&"process_data"));
    assert!(names.contains(&"main"));
}

#[test]
fn test_filter_by_is_exported_true() {
    let store = test_store();
    seed_nodes(&store);

    let results = store
        .get_nodes_by_metadata(
            "p",
            &MetadataFilter {
                is_exported: Some(true),
                ..Default::default()
            },
            100,
        )
        .unwrap();

    assert_eq!(results.len(), 3);
    let names: Vec<&str> = results.iter().map(|n| n.name.as_str()).collect();
    assert!(names.contains(&"test_login"));
    assert!(names.contains(&"handle_request"));
    assert!(names.contains(&"main"));
}

#[test]
fn test_filter_by_is_exported_false() {
    let store = test_store();
    seed_nodes(&store);

    let results = store
        .get_nodes_by_metadata(
            "p",
            &MetadataFilter {
                is_exported: Some(false),
                ..Default::default()
            },
            100,
        )
        .unwrap();

    assert_eq!(results.len(), 2);
    let names: Vec<&str> = results.iter().map(|n| n.name.as_str()).collect();
    assert!(names.contains(&"process_data"));
    assert!(names.contains(&"test_process"));
}

#[test]
fn test_filter_by_min_complexity() {
    let store = test_store();
    seed_nodes(&store);

    let results = store
        .get_nodes_by_metadata(
            "p",
            &MetadataFilter {
                min_complexity: Some(5),
                ..Default::default()
            },
            100,
        )
        .unwrap();

    assert_eq!(results.len(), 3);
    let names: Vec<&str> = results.iter().map(|n| n.name.as_str()).collect();
    assert!(names.contains(&"handle_request")); // complexity 8
    assert!(names.contains(&"process_data")); // complexity 15
    assert!(names.contains(&"UserService")); // complexity 5
}

#[test]
fn test_filter_by_high_complexity() {
    let store = test_store();
    seed_nodes(&store);

    let results = store
        .get_nodes_by_metadata(
            "p",
            &MetadataFilter {
                min_complexity: Some(10),
                ..Default::default()
            },
            100,
        )
        .unwrap();

    assert_eq!(results.len(), 1);
    assert_eq!(results[0].name, "process_data");
}

#[test]
fn test_filter_by_label() {
    let store = test_store();
    seed_nodes(&store);

    let results = store
        .get_nodes_by_metadata(
            "p",
            &MetadataFilter {
                label: Some("Method".into()),
                ..Default::default()
            },
            100,
        )
        .unwrap();

    assert_eq!(results.len(), 2);
    for node in &results {
        assert_eq!(node.label, "Method");
    }
}

#[test]
fn test_filter_by_label_class() {
    let store = test_store();
    seed_nodes(&store);

    let results = store
        .get_nodes_by_metadata(
            "p",
            &MetadataFilter {
                label: Some("Class".into()),
                ..Default::default()
            },
            100,
        )
        .unwrap();

    assert_eq!(results.len(), 1);
    assert_eq!(results[0].name, "UserService");
}

#[test]
fn test_combined_is_test_and_label() {
    let store = test_store();
    seed_nodes(&store);

    let results = store
        .get_nodes_by_metadata(
            "p",
            &MetadataFilter {
                is_test: Some(true),
                label: Some("Function".into()),
                ..Default::default()
            },
            100,
        )
        .unwrap();

    assert_eq!(results.len(), 1);
    assert_eq!(results[0].name, "test_login");
}

#[test]
fn test_combined_exported_and_min_complexity() {
    let store = test_store();
    seed_nodes(&store);

    let results = store
        .get_nodes_by_metadata(
            "p",
            &MetadataFilter {
                is_exported: Some(true),
                min_complexity: Some(5),
                ..Default::default()
            },
            100,
        )
        .unwrap();

    assert_eq!(results.len(), 1);
    assert_eq!(results[0].name, "handle_request");
}

#[test]
fn test_combined_not_test_and_label_and_complexity() {
    let store = test_store();
    seed_nodes(&store);

    let results = store
        .get_nodes_by_metadata(
            "p",
            &MetadataFilter {
                is_test: Some(false),
                label: Some("Function".into()),
                min_complexity: Some(5),
                ..Default::default()
            },
            100,
        )
        .unwrap();

    assert_eq!(results.len(), 1);
    assert_eq!(results[0].name, "handle_request");
}

#[test]
fn test_filter_by_is_entry_point() {
    let store = test_store();
    seed_nodes(&store);

    let results = store
        .get_nodes_by_metadata(
            "p",
            &MetadataFilter {
                is_entry_point: Some(true),
                ..Default::default()
            },
            100,
        )
        .unwrap();

    assert_eq!(results.len(), 1);
    assert_eq!(results[0].name, "main");
}

#[test]
fn test_no_filters_returns_all() {
    let store = test_store();
    seed_nodes(&store);

    let results = store
        .get_nodes_by_metadata("p", &MetadataFilter::default(), 100)
        .unwrap();

    assert_eq!(results.len(), 6);
}

#[test]
fn test_limit_respected() {
    let store = test_store();
    seed_nodes(&store);

    let results = store
        .get_nodes_by_metadata("p", &MetadataFilter::default(), 2)
        .unwrap();

    assert_eq!(results.len(), 2);
}

#[test]
fn test_no_matching_results() {
    let store = test_store();
    seed_nodes(&store);

    let results = store
        .get_nodes_by_metadata(
            "p",
            &MetadataFilter {
                min_complexity: Some(100),
                ..Default::default()
            },
            100,
        )
        .unwrap();

    assert!(results.is_empty());
}

#[test]
fn test_all_filters_combined() {
    let store = test_store();
    seed_nodes(&store);

    // Only main matches: not test, exported, entry point, Function, complexity >= 1
    let results = store
        .get_nodes_by_metadata(
            "p",
            &MetadataFilter {
                is_test: Some(false),
                is_exported: Some(true),
                is_entry_point: Some(true),
                min_complexity: Some(1),
                label: Some("Function".into()),
            },
            100,
        )
        .unwrap();

    assert_eq!(results.len(), 1);
    assert_eq!(results[0].name, "main");
}
