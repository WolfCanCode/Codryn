use codryn_store::{Node, Project, Store};
use proptest::prelude::*;

/// **Validates: Requirements 12.1**
/// Property 9: Hotspot Sort Order
///
/// For any set of nodes with `git_commits` property values, sorting by hotspot
/// SHALL produce a sequence where each node's `git_commits` count is greater than
/// or equal to the next node's count (descending order), and nodes without
/// `git_commits` data SHALL be excluded from results.
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

/// Strategy for generating a git_commits value (positive integer).
fn git_commits_strategy() -> impl Strategy<Value = i64> {
    1i64..1000
}

/// Strategy for a label value.
fn label_strategy() -> impl Strategy<Value = String> {
    prop_oneof![
        Just("Function".to_string()),
        Just("Class".to_string()),
        Just("Method".to_string()),
    ]
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(100))]

    /// Property 9: Results are sorted in descending order by git_commits.
    #[test]
    fn hotspot_results_sorted_descending(
        git_commits_values in prop::collection::vec(git_commits_strategy(), 1..20),
        labels in prop::collection::vec(label_strategy(), 20),
    ) {
        let store = test_store();
        setup_project(&store);

        // Insert nodes with varying git_commits values
        for (i, commits) in git_commits_values.iter().enumerate() {
            let label = &labels[i % labels.len()];
            let props = format!(
                r#"{{"git_commits": {}, "git_authors": 1, "git_last_modified": "2024-01-01T00:00:00Z"}}"#,
                commits
            );
            store.insert_node(&Node {
                id: 0,
                project: "p".into(),
                label: label.clone(),
                name: format!("func_{}", i),
                qualified_name: format!("p.func_{}", i),
                file_path: "src/main.rs".into(),
                start_line: 1,
                end_line: 10,
                properties_json: Some(props),
            }).unwrap();
        }

        let results = store.query_hotspots("p", 100).unwrap();

        // All results must be sorted in descending order by git_commits
        for window in results.windows(2) {
            prop_assert!(
                window[0].git_commits >= window[1].git_commits,
                "Results not sorted descending: {} < {}",
                window[0].git_commits,
                window[1].git_commits
            );
        }
    }

    /// Property 9: Nodes without git_commits data are excluded from results.
    #[test]
    fn hotspot_excludes_nodes_without_git_data(
        num_with_git in 1usize..10,
        num_without_git in 1usize..10,
    ) {
        let store = test_store();
        setup_project(&store);

        // Insert nodes WITH git data
        for i in 0..num_with_git {
            let props = format!(
                r#"{{"git_commits": {}, "git_authors": 1, "git_last_modified": "2024-01-01T00:00:00Z"}}"#,
                (i + 1) * 5
            );
            store.insert_node(&Node {
                id: 0,
                project: "p".into(),
                label: "Function".into(),
                name: format!("with_git_{}", i),
                qualified_name: format!("p.with_git_{}", i),
                file_path: "src/main.rs".into(),
                start_line: 1,
                end_line: 10,
                properties_json: Some(props),
            }).unwrap();
        }

        // Insert nodes WITHOUT git data (various forms)
        for i in 0..num_without_git {
            let props = match i % 3 {
                0 => r#"{"cyclomatic_complexity": 5}"#.to_string(),
                1 => r#"{"git_commits": 0, "git_authors": 0}"#.to_string(),
                _ => r#"{}"#.to_string(),
            };
            store.insert_node(&Node {
                id: 0,
                project: "p".into(),
                label: "Function".into(),
                name: format!("without_git_{}", i),
                qualified_name: format!("p.without_git_{}", i),
                file_path: "src/main.rs".into(),
                start_line: 1,
                end_line: 10,
                properties_json: Some(props),
            }).unwrap();
        }

        let results = store.query_hotspots("p", 100).unwrap();

        // No result should have git_commits <= 0
        for row in &results {
            prop_assert!(
                row.git_commits > 0,
                "Result has git_commits <= 0: {} with git_commits={}",
                row.name,
                row.git_commits
            );
        }

        // Count of results should equal count of nodes with valid git_commits > 0
        prop_assert_eq!(
            results.len(),
            num_with_git,
            "Expected {} results (nodes with git data), got {}",
            num_with_git,
            results.len()
        );
    }

    /// Property 9: Result count respects the limit parameter.
    #[test]
    fn hotspot_respects_limit(
        num_nodes in 5usize..20,
        limit in 1i64..10,
    ) {
        let store = test_store();
        setup_project(&store);

        // Insert nodes all with valid git data
        for i in 0..num_nodes {
            let props = format!(
                r#"{{"git_commits": {}, "git_authors": 1, "git_last_modified": "2024-01-01T00:00:00Z"}}"#,
                (i + 1) * 3
            );
            store.insert_node(&Node {
                id: 0,
                project: "p".into(),
                label: "Function".into(),
                name: format!("func_{}", i),
                qualified_name: format!("p.func_{}", i),
                file_path: "src/main.rs".into(),
                start_line: 1,
                end_line: 10,
                properties_json: Some(props),
            }).unwrap();
        }

        let results = store.query_hotspots("p", limit).unwrap();

        // Result count should be min(num_nodes, limit)
        let expected_count = std::cmp::min(num_nodes, limit as usize);
        prop_assert_eq!(
            results.len(),
            expected_count,
            "Expected {} results (min of {} nodes and {} limit), got {}",
            expected_count,
            num_nodes,
            limit,
            results.len()
        );

        // Results should still be sorted descending
        for window in results.windows(2) {
            prop_assert!(
                window[0].git_commits >= window[1].git_commits,
                "Results not sorted descending after limit: {} < {}",
                window[0].git_commits,
                window[1].git_commits
            );
        }
    }
}
