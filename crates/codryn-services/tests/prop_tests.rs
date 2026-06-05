use codryn_services::pipeline::{
    DagEdge, InfraResource, JobInfo, PipelineDag, PipelineInfo, StageInfo,
};
use proptest::prelude::*;

// ── Strategies ────────────────────────────────────────────────────────

/// Generate a non-empty alphanumeric identifier string.
fn ident_strategy() -> impl Strategy<Value = String> {
    "[a-z][a-z0-9_-]{0,19}"
}

/// Generate a file path string.
fn file_path_strategy() -> impl Strategy<Value = String> {
    "[a-z]{1,8}(/[a-z]{1,8}){0,3}\\.[a-z]{1,5}"
}

/// Generate a CI system name.
fn ci_system_strategy() -> impl Strategy<Value = String> {
    prop_oneof![
        Just("gitlab".to_string()),
        Just("github".to_string()),
        Just("jenkins".to_string()),
        Just("circleci".to_string()),
        Just("azure".to_string()),
        Just("bitbucket".to_string()),
    ]
}

/// Generate a trigger event name.
fn trigger_strategy() -> impl Strategy<Value = String> {
    prop_oneof![
        Just("push".to_string()),
        Just("pull_request".to_string()),
        Just("schedule".to_string()),
        Just("merge_request".to_string()),
        Just("tag".to_string()),
    ]
}

/// Generate a valid PipelineInfo.
fn pipeline_info_strategy() -> impl Strategy<Value = PipelineInfo> {
    (
        ident_strategy(),
        file_path_strategy(),
        ci_system_strategy(),
        prop::collection::vec(trigger_strategy(), 0..5),
    )
        .prop_map(|(name, file_path, ci_system, triggers)| PipelineInfo {
            name,
            file_path,
            ci_system,
            triggers,
        })
}

/// Generate a valid StageInfo.
fn stage_info_strategy() -> impl Strategy<Value = StageInfo> {
    (ident_strategy(), 0usize..100).prop_map(|(name, order)| StageInfo { name, order })
}

/// Generate a valid JobInfo.
fn job_info_strategy() -> impl Strategy<Value = JobInfo> {
    (
        ident_strategy(),
        ident_strategy(),
        prop::option::of(ident_strategy()),
        prop::collection::vec(ident_strategy(), 0..5),
    )
        .prop_map(|(name, stage, image, dependencies)| JobInfo {
            name,
            stage,
            image,
            dependencies,
        })
}

/// Generate a valid edge type.
fn edge_type_strategy() -> impl Strategy<Value = String> {
    prop_oneof![
        Just("DEPENDS_ON".to_string()),
        Just("BELONGS_TO_STAGE".to_string()),
        Just("NEXT_STAGE".to_string()),
        Just("DEPLOYS".to_string()),
        Just("BUILDS_IMAGE".to_string()),
    ]
}

/// Generate a valid DagEdge.
fn dag_edge_strategy() -> impl Strategy<Value = DagEdge> {
    (ident_strategy(), ident_strategy(), edge_type_strategy()).prop_map(
        |(source, target, edge_type)| DagEdge {
            source,
            target,
            edge_type,
        },
    )
}

/// Generate a valid PipelineDag.
fn pipeline_dag_strategy() -> impl Strategy<Value = PipelineDag> {
    (
        pipeline_info_strategy(),
        prop::collection::vec(stage_info_strategy(), 0..8),
        prop::collection::vec(job_info_strategy(), 0..10),
        prop::collection::vec(dag_edge_strategy(), 0..15),
    )
        .prop_map(|(pipeline, stages, jobs, edges)| PipelineDag {
            pipeline,
            stages,
            jobs,
            edges,
        })
}

/// Generate a simple JSON value suitable for InfraResource.properties.
fn simple_json_value_strategy() -> impl Strategy<Value = serde_json::Value> {
    let leaf = prop_oneof![
        ident_strategy().prop_map(serde_json::Value::String),
        (0i64..10000).prop_map(|n| serde_json::Value::Number(serde_json::Number::from(n))),
        any::<bool>().prop_map(serde_json::Value::Bool),
        Just(serde_json::Value::Null),
    ];

    prop::collection::hash_map(ident_strategy(), leaf, 0..5)
        .prop_map(|map| serde_json::Value::Object(map.into_iter().collect()))
}

/// Generate a valid InfraResource.
fn infra_resource_strategy() -> impl Strategy<Value = InfraResource> {
    (
        ident_strategy(),
        ident_strategy(),
        prop_oneof![
            Just("terraform".to_string()),
            Just("helm".to_string()),
            Just("kubernetes".to_string()),
            Just("docker".to_string()),
        ],
        file_path_strategy(),
        simple_json_value_strategy(),
    )
        .prop_map(
            |(name, resource_type, kind, file_path, properties)| InfraResource {
                name,
                resource_type,
                kind,
                file_path,
                properties,
            },
        )
}

// ═══════════════════════════════════════════════════════════════════════
// Feature: devops-pipeline-support, Property 16: Pipeline DAG serialization round-trip
// ═══════════════════════════════════════════════════════════════════════

/// **Validates: Requirements 12.2**
mod property16_pipeline_dag_round_trip {
    use super::*;

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(100))]

        #[test]
        fn serialize_deserialize_produces_equivalent_pipeline_dag(
            dag in pipeline_dag_strategy()
        ) {
            let json_str = serde_json::to_string(&dag).expect("serialization should succeed");
            let deserialized: PipelineDag =
                serde_json::from_str(&json_str).expect("deserialization should succeed");
            prop_assert_eq!(
                &deserialized, &dag,
                "round-trip failed: deserialize(serialize(dag)) != dag"
            );
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Feature: devops-pipeline-support, Property 17: Infrastructure resource serialization round-trip
// ═══════════════════════════════════════════════════════════════════════

/// **Validates: Requirements 12.4**
mod property17_infra_resource_round_trip {
    use super::*;

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(100))]

        #[test]
        fn serialize_deserialize_produces_equivalent_infra_resources(
            resources in prop::collection::vec(infra_resource_strategy(), 0..10)
        ) {
            let json_str = serde_json::to_string(&resources).expect("serialization should succeed");
            let deserialized: Vec<InfraResource> =
                serde_json::from_str(&json_str).expect("deserialization should succeed");
            prop_assert_eq!(
                &deserialized, &resources,
                "round-trip failed: deserialize(serialize(resources)) != resources"
            );
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Feature: devops-pipeline-support, Property 13: Topological sort produces valid ordering
// ═══════════════════════════════════════════════════════════════════════

/// **Validates: Requirements 8.2**
mod property13_topological_sort_valid_ordering {
    use super::*;
    use codryn_services::pipeline::PipelineService;
    use codryn_store::{Edge, Node, Project, Store};

    /// Strategy: generate an acyclic DAG of N jobs (2..=8) with random DEPENDS_ON
    /// edges only from higher-indexed to lower-indexed jobs (guaranteeing acyclicity).
    /// Returns (num_jobs, edges_as_index_pairs) where each edge is (source_idx, target_idx)
    /// meaning job[source_idx] DEPENDS_ON job[target_idx].
    fn acyclic_dag_strategy() -> impl Strategy<Value = (usize, Vec<(usize, usize)>)> {
        (2usize..=8).prop_flat_map(|n| {
            // Generate a subset of valid edges: source > target (higher depends on lower)
            let all_possible: Vec<(usize, usize)> =
                (0..n).flat_map(|s| (0..s).map(move |t| (s, t))).collect();
            let max_edges = all_possible.len();
            prop::sample::subsequence(all_possible, 0..=max_edges).prop_map(move |edges| (n, edges))
        })
    }

    fn setup_acyclic_pipeline(n: usize, edges: &[(usize, usize)]) -> (Store, String, String) {
        let store = Store::open_in_memory().unwrap();
        let project = "p";
        let pipeline_name = "CI";

        store
            .upsert_project(&Project {
                name: project.into(),
                indexed_at: "now".into(),
                root_path: "/tmp".into(),
            })
            .unwrap();

        // Insert Pipeline node
        store
            .insert_node(&Node {
                id: 0,
                project: project.into(),
                label: "Pipeline".into(),
                name: pipeline_name.into(),
                qualified_name: format!("{}.pipeline.gitlab.{}", project, pipeline_name),
                file_path: ".gitlab-ci.yml".into(),
                start_line: 0,
                end_line: 0,
                properties_json: Some(r#"{"ci_system":"gitlab","triggers":[]}"#.into()),
            })
            .unwrap();

        // Insert Job nodes: job_0, job_1, ..., job_{n-1}
        let mut job_ids = Vec::new();
        for i in 0..n {
            let job_name = format!("job_{}", i);
            let id = store
                .insert_node(&Node {
                    id: 0,
                    project: project.into(),
                    label: "Job".into(),
                    name: job_name.clone(),
                    qualified_name: format!(
                        "{}.pipeline.gitlab.{}.job.{}",
                        project, pipeline_name, job_name
                    ),
                    file_path: ".gitlab-ci.yml".into(),
                    start_line: 0,
                    end_line: 0,
                    properties_json: Some(format!(
                        r#"{{"pipeline_name":"{}","stage":"default"}}"#,
                        pipeline_name
                    )),
                })
                .unwrap();
            job_ids.push(id);
        }

        // Insert DEPENDS_ON edges
        for &(src_idx, tgt_idx) in edges {
            store
                .insert_edge(&Edge {
                    id: 0,
                    project: project.into(),
                    source_id: job_ids[src_idx],
                    target_id: job_ids[tgt_idx],
                    edge_type: "DEPENDS_ON".into(),
                    properties_json: None,
                })
                .unwrap();
        }

        (store, project.to_string(), pipeline_name.to_string())
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(100))]

        #[test]
        fn topological_sort_respects_depends_on_ordering(
            (n, edges) in acyclic_dag_strategy()
        ) {
            let (store, project, pipeline_name) = setup_acyclic_pipeline(n, &edges);
            let dag = PipelineService::get_pipeline_dag(&store, &project, &pipeline_name)
                .expect("acyclic DAG should not error");

            // Build position map: job name -> index in sorted output
            let position: std::collections::HashMap<&str, usize> = dag
                .jobs
                .iter()
                .enumerate()
                .map(|(i, j)| (j.name.as_str(), i))
                .collect();

            // For every DEPENDS_ON edge (A→B), B must appear before A
            for &(src_idx, tgt_idx) in &edges {
                let src_name = format!("job_{}", src_idx);
                let tgt_name = format!("job_{}", tgt_idx);
                let src_pos = position.get(src_name.as_str());
                let tgt_pos = position.get(tgt_name.as_str());
                if let (Some(&sp), Some(&tp)) = (src_pos, tgt_pos) {
                    prop_assert!(
                        tp < sp,
                        "DEPENDS_ON edge {} -> {} violated: {} at pos {}, {} at pos {}",
                        src_name, tgt_name, tgt_name, tp, src_name, sp
                    );
                }
            }
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Feature: devops-pipeline-support, Property 14: Cycle detection
// ═══════════════════════════════════════════════════════════════════════

/// **Validates: Requirements 8.4**
mod property14_cycle_detection {
    use super::*;
    use codryn_services::pipeline::PipelineService;
    use codryn_store::{Edge, Node, Project, Store};

    /// Strategy for acyclic DAGs (reuse same approach: edges only from higher to lower index).
    fn acyclic_dag_strategy() -> impl Strategy<Value = (usize, Vec<(usize, usize)>)> {
        (2usize..=6).prop_flat_map(|n| {
            let all_possible: Vec<(usize, usize)> =
                (0..n).flat_map(|s| (0..s).map(move |t| (s, t))).collect();
            let max_edges = all_possible.len();
            prop::sample::subsequence(all_possible, 0..=max_edges).prop_map(move |edges| (n, edges))
        })
    }

    /// Strategy for cyclic DAGs: pick two distinct nodes and create a cycle between them
    /// (A depends on B AND B depends on A), plus optional extra acyclic edges.
    fn cyclic_dag_strategy() -> impl Strategy<Value = (usize, Vec<(usize, usize)>)> {
        (2usize..=6).prop_flat_map(|n| {
            // Pick two distinct nodes to form a guaranteed cycle
            let cycle_pair = (0..n, 0..n).prop_filter("need distinct nodes", |&(a, b)| a != b);

            // Optional extra acyclic edges (higher -> lower only)
            let forward_possible: Vec<(usize, usize)> =
                (0..n).flat_map(|s| (0..s).map(move |t| (s, t))).collect();
            let max_fwd = forward_possible.len();
            let extra = prop::sample::subsequence(forward_possible, 0..=max_fwd);

            (cycle_pair, extra).prop_map(move |((a, b), mut edges)| {
                // Ensure both directions exist to form a cycle: a→b and b→a
                if !edges.contains(&(a, b)) {
                    edges.push((a, b));
                }
                if !edges.contains(&(b, a)) {
                    edges.push((b, a));
                }
                (n, edges)
            })
        })
    }

    fn setup_pipeline_with_edges(n: usize, edges: &[(usize, usize)]) -> (Store, String, String) {
        let store = Store::open_in_memory().unwrap();
        let project = "p";
        let pipeline_name = "CI";

        store
            .upsert_project(&Project {
                name: project.into(),
                indexed_at: "now".into(),
                root_path: "/tmp".into(),
            })
            .unwrap();

        store
            .insert_node(&Node {
                id: 0,
                project: project.into(),
                label: "Pipeline".into(),
                name: pipeline_name.into(),
                qualified_name: format!("{}.pipeline.gitlab.{}", project, pipeline_name),
                file_path: ".gitlab-ci.yml".into(),
                start_line: 0,
                end_line: 0,
                properties_json: Some(r#"{"ci_system":"gitlab","triggers":[]}"#.into()),
            })
            .unwrap();

        let mut job_ids = Vec::new();
        for i in 0..n {
            let job_name = format!("job_{}", i);
            let id = store
                .insert_node(&Node {
                    id: 0,
                    project: project.into(),
                    label: "Job".into(),
                    name: job_name.clone(),
                    qualified_name: format!(
                        "{}.pipeline.gitlab.{}.job.{}",
                        project, pipeline_name, job_name
                    ),
                    file_path: ".gitlab-ci.yml".into(),
                    start_line: 0,
                    end_line: 0,
                    properties_json: Some(format!(
                        r#"{{"pipeline_name":"{}","stage":"default"}}"#,
                        pipeline_name
                    )),
                })
                .unwrap();
            job_ids.push(id);
        }

        for &(src_idx, tgt_idx) in edges {
            store
                .insert_edge(&Edge {
                    id: 0,
                    project: project.into(),
                    source_id: job_ids[src_idx],
                    target_id: job_ids[tgt_idx],
                    edge_type: "DEPENDS_ON".into(),
                    properties_json: None,
                })
                .unwrap();
        }

        (store, project.to_string(), pipeline_name.to_string())
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(100))]

        #[test]
        fn acyclic_dag_does_not_return_cycle_error(
            (n, edges) in acyclic_dag_strategy()
        ) {
            let (store, project, pipeline_name) = setup_pipeline_with_edges(n, &edges);
            let result = PipelineService::get_pipeline_dag(&store, &project, &pipeline_name);
            prop_assert!(
                result.is_ok(),
                "Acyclic DAG with {} jobs should not return error, got: {:?}",
                n,
                result.err()
            );
        }

        #[test]
        fn cyclic_dag_returns_error(
            (n, edges) in cyclic_dag_strategy()
        ) {
            let (store, project, pipeline_name) = setup_pipeline_with_edges(n, &edges);
            let result = PipelineService::get_pipeline_dag(&store, &project, &pipeline_name);
            prop_assert!(
                result.is_err(),
                "Cyclic DAG with {} jobs and edges {:?} should return error",
                n,
                edges
            );
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Feature: devops-pipeline-support, Property 15: Infrastructure type filter correctness
// ═══════════════════════════════════════════════════════════════════════

/// **Validates: Requirements 8.3, 9.4**
mod property15_infrastructure_type_filter {
    use super::*;
    use codryn_services::pipeline::PipelineService;
    use codryn_store::{Node, Project, Store};

    /// The set of infra types we generate.
    const INFRA_TYPES: &[&str] = &["terraform", "helm", "kubernetes", "docker"];

    /// Strategy: generate a list of (name_suffix, infra_type) pairs representing Infra nodes.
    fn infra_nodes_strategy() -> impl Strategy<Value = (Vec<(usize, String)>, String)> {
        // Generate 1..=15 infra nodes with random types
        let nodes =
            prop::collection::vec((0usize..1000, prop::sample::select(INFRA_TYPES)), 1..=15)
                .prop_map(|pairs| {
                    pairs
                        .into_iter()
                        .enumerate()
                        .map(|(i, (_, t))| (i, t.to_string()))
                        .collect::<Vec<_>>()
                });

        // Pick a filter type
        let filter = prop::sample::select(INFRA_TYPES).prop_map(|s| s.to_string());

        (nodes, filter)
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(100))]

        #[test]
        fn filter_returns_only_and_all_matching_nodes(
            (infra_nodes, filter_type) in infra_nodes_strategy()
        ) {
            let store = Store::open_in_memory().unwrap();
            let project = "p";

            store
                .upsert_project(&Project {
                    name: project.into(),
                    indexed_at: "now".into(),
                    root_path: "/tmp".into(),
                })
                .unwrap();

            // Insert Infra nodes with varying types
            for (i, infra_type) in &infra_nodes {
                let name = format!("resource_{}", i);
                store
                    .insert_node(&Node {
                        id: 0,
                        project: project.into(),
                        label: "Infra".into(),
                        name: name.clone(),
                        qualified_name: format!("{}.infra.{}", project, name),
                        file_path: format!("infra/{}.tf", name),
                        start_line: 0,
                        end_line: 0,
                        properties_json: Some(format!(
                            r#"{{"infra_type":"{}","resource_type":"some_type"}}"#,
                            infra_type
                        )),
                    })
                    .unwrap();
            }

            // Call list_infrastructure with the filter
            let filtered = PipelineService::list_infrastructure(&store, project, Some(&filter_type))
                .expect("list_infrastructure should succeed");

            // Compute expected: all nodes whose infra_type matches the filter
            let expected_names: std::collections::HashSet<String> = infra_nodes
                .iter()
                .filter(|(_, t)| t == &filter_type)
                .map(|(i, _)| format!("resource_{}", i))
                .collect();

            let actual_names: std::collections::HashSet<String> = filtered
                .iter()
                .map(|r| r.name.clone())
                .collect();

            // All returned nodes must match the filter type
            for r in &filtered {
                prop_assert_eq!(
                    &r.kind, &filter_type,
                    "Returned resource '{}' has kind '{}', expected '{}'",
                    r.name, r.kind, filter_type
                );
            }

            // All matching nodes must be returned
            prop_assert_eq!(
                &actual_names, &expected_names,
                "Mismatch: actual={:?}, expected={:?}",
                &actual_names, &expected_names
            );
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Property: Dead code zero-reference invariant
// ═══════════════════════════════════════════════════════════════════════

mod property_dead_code_zero_reference {
    use codryn_services::dead_code::find_dead_code;
    use codryn_store::{Edge, Node, Project, Store};
    use proptest::prelude::*;

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(30))]

        #[test]
        fn nodes_without_incoming_edges_appear_in_dead_code(
            count in 1usize..=5,
        ) {
            let store = Store::open_in_memory().unwrap();
            store.upsert_project(&Project {
                name: "p".into(), indexed_at: "now".into(), root_path: "/tmp".into(),
            }).unwrap();

            for i in 0..count {
                store.insert_node(&Node {
                    id: 0, project: "p".into(), label: "Function".into(),
                    name: format!("orphan_{}", i),
                    qualified_name: format!("p.orphan_{}", i),
                    file_path: "src/lib.rs".into(),
                    start_line: i as i32, end_line: i as i32,
                    properties_json: None,
                }).unwrap();
            }

            let results = find_dead_code(&store, "p", None, None).unwrap();
            let names: Vec<_> = results.iter().map(|r| r.symbol.as_str()).collect();
            for i in 0..count {
                let expected = format!("orphan_{}", i);
                prop_assert!(names.contains(&expected.as_str()),
                    "Expected '{}' in dead code results", expected);
            }
        }

        #[test]
        fn nodes_with_incoming_edges_excluded_from_dead_code(
            count in 1usize..=5,
        ) {
            let store = Store::open_in_memory().unwrap();
            store.upsert_project(&Project {
                name: "p".into(), indexed_at: "now".into(), root_path: "/tmp".into(),
            }).unwrap();

            let mut ids = Vec::new();
            for i in 0..count {
                let id = store.insert_node(&Node {
                    id: 0, project: "p".into(), label: "Function".into(),
                    name: format!("used_{}", i),
                    qualified_name: format!("p.used_{}", i),
                    file_path: "src/lib.rs".into(),
                    start_line: i as i32, end_line: i as i32,
                    properties_json: None,
                }).unwrap();
                ids.push(id);
            }
            // Add a caller node
            let caller_id = store.insert_node(&Node {
                id: 0, project: "p".into(), label: "Function".into(),
                name: "caller".into(), qualified_name: "p.caller".into(),
                file_path: "src/main.rs".into(),
                start_line: 0, end_line: 0, properties_json: None,
            }).unwrap();

            for &target_id in &ids {
                store.insert_edge(&Edge {
                    id: 0, project: "p".into(),
                    source_id: caller_id, target_id,
                    edge_type: "CALLS".into(), properties_json: None,
                }).unwrap();
            }

            let results = find_dead_code(&store, "p", None, None).unwrap();
            let names: Vec<_> = results.iter().map(|r| r.symbol.as_str()).collect();
            for i in 0..count {
                let name = format!("used_{}", i);
                prop_assert!(!names.contains(&name.as_str()),
                    "'{}' should NOT appear in dead code", name);
            }
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Property 15: Staleness Score Computation
// **Validates: Requirements 24.1, 24.2**
//
// For any project with T total indexed files where C files have content
// hashes differing from disk, the staleness_score SHALL equal C/T (within
// floating-point precision), and when staleness_score > 0.20, a warning
// field SHALL be present in the response.
// ═══════════════════════════════════════════════════════════════════════

mod property_staleness_score {
    use codryn_services::staleness::{build_annotation, compute_staleness, warning_threshold};
    use codryn_store::{FileHash, Project, Store};
    use proptest::prelude::*;
    use sha2::{Digest, Sha256};
    use tempfile::TempDir;

    /// Helper to create a project in the store.
    fn setup_project(store: &Store, project: &str, root_path: &str) {
        store
            .upsert_project(&Project {
                name: project.to_string(),
                indexed_at: "2024-01-01T00:00:00Z".to_string(),
                root_path: root_path.to_string(),
            })
            .unwrap();
    }

    /// Strategy to generate a total file count (small enough to avoid 500ms timeout).
    fn total_files_strategy() -> impl Strategy<Value = usize> {
        1usize..=50
    }

    /// Strategy to generate a changed file count as a percentage of total.
    fn changed_pct_strategy() -> impl Strategy<Value = u8> {
        0u8..=100
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(200))]

        /// Property 15a: staleness_score = C/T within floating-point precision.
        ///
        /// For any T total files and C changed files (C <= T), the computed
        /// staleness score must equal C/T within f64 precision.
        #[test]
        fn staleness_score_equals_c_over_t(
            total in total_files_strategy(),
            changed_pct in changed_pct_strategy(),
        ) {
            let changed = ((total as f64 * changed_pct as f64 / 100.0).round() as usize).min(total);

            // Set up a temp directory with `total` files, `changed` of which are stale
            let dir = TempDir::new().unwrap();
            let store = Store::open_in_memory().unwrap();
            setup_project(&store, "test_project", dir.path().to_str().unwrap());

            let mut file_hashes = Vec::new();
            for i in 0..total {
                let rel_path = format!("src/file_{}.rs", i);
                let abs_path = dir.path().join(&rel_path);

                // Create parent directories
                if let Some(parent) = abs_path.parent() {
                    std::fs::create_dir_all(parent).unwrap();
                }

                if i < changed {
                    // Stale file: write content that differs from stored hash
                    let content = format!("// modified content {}", i);
                    std::fs::write(&abs_path, &content).unwrap();

                    // Store a DIFFERENT hash (so the file appears stale)
                    let fake_hash = hex::encode(Sha256::digest(
                        format!("original content {}", i).as_bytes(),
                    ));
                    file_hashes.push(FileHash {
                        project: "test_project".to_string(),
                        rel_path,
                        sha256: fake_hash,
                        mtime_ns: 0, // Force mtime mismatch to trigger hash comparison
                        size: content.len() as i64,
                    });
                } else {
                    // Fresh file: write content that matches stored hash
                    let content = format!("// original content {}", i);
                    std::fs::write(&abs_path, &content).unwrap();

                    let real_hash = hex::encode(Sha256::digest(content.as_bytes()));
                    file_hashes.push(FileHash {
                        project: "test_project".to_string(),
                        rel_path,
                        sha256: real_hash,
                        mtime_ns: 0, // Force mtime mismatch to trigger hash comparison
                        size: content.len() as i64,
                    });
                }
            }

            store.store_file_hashes_batch(&file_hashes).unwrap();

            // Compute staleness using the real function
            let report = compute_staleness(
                &store,
                "test_project",
                dir.path(),
                None,
            ).unwrap();

            // The implementation has a 500ms timeout; if it times out it extrapolates.
            // When all files were checked (no extrapolation), score must equal C/T exactly.
            // When extrapolation occurred, we verify weaker bounds.
            let expected_score = changed as f64 / total as f64;
            if report.changed_files == changed {
                // No extrapolation — exact equality within f64 precision
                prop_assert!(
                    (report.score - expected_score).abs() < 1e-10,
                    "staleness_score {} != expected C/T = {}/{} = {}",
                    report.score, changed, total, expected_score
                );
            } else {
                // Extrapolation occurred due to timeout — verify score is bounded [0, 1]
                prop_assert!(
                    report.score >= 0.0 && report.score <= 1.0,
                    "staleness_score {} out of bounds [0, 1] (extrapolated)",
                    report.score
                );
            }
        }

        /// Property 15b: warning field presence based on threshold.
        ///
        /// When staleness_score > 0.20, a warning field SHALL be present.
        /// When staleness_score <= 0.20, no warning field SHALL be present.
        #[test]
        fn warning_present_iff_score_above_threshold(
            total in total_files_strategy(),
            changed_pct in changed_pct_strategy(),
        ) {
            let changed = ((total as f64 * changed_pct as f64 / 100.0).round() as usize).min(total);
            let score = changed as f64 / total as f64;

            let annotation = build_annotation(score);

            // Verify: score is preserved in annotation
            prop_assert!(
                (annotation.staleness_score - score).abs() < 1e-10,
                "annotation score {} != computed score {}",
                annotation.staleness_score, score
            );

            // Verify: warning present iff score > 0.20
            let threshold = warning_threshold();
            if score > threshold {
                prop_assert!(
                    annotation.warning.is_some(),
                    "Expected warning for score {} > threshold {}, but got None",
                    score, threshold
                );
            } else {
                prop_assert!(
                    annotation.warning.is_none(),
                    "Expected no warning for score {} <= threshold {}, but got: {:?}",
                    score, threshold, annotation.warning
                );
            }
        }

        /// Property 15c: zero total files yields zero score with no warning.
        ///
        /// Edge case: when T = 0, score must be 0.0 and no warning present.
        #[test]
        fn zero_total_files_gives_zero_score_no_warning(_seed in 0u32..100) {
            let store = Store::open_in_memory().unwrap();
            let dir = TempDir::new().unwrap();
            setup_project(&store, "empty_project", dir.path().to_str().unwrap());

            // No file hashes stored — empty project
            let report = compute_staleness(
                &store,
                "empty_project",
                dir.path(),
                None,
            ).unwrap();

            prop_assert_eq!(report.score, 0.0);
            prop_assert_eq!(report.total_files, 0);
            prop_assert_eq!(report.changed_files, 0);

            let annotation = build_annotation(report.score);
            prop_assert!(annotation.warning.is_none());
        }

        /// Property 15d: score is always in [0.0, 1.0] range.
        ///
        /// For any valid inputs, the staleness score must be bounded.
        #[test]
        fn score_bounded_zero_to_one(
            total in total_files_strategy(),
            changed_pct in changed_pct_strategy(),
        ) {
            let changed = ((total as f64 * changed_pct as f64 / 100.0).round() as usize).min(total);

            let dir = TempDir::new().unwrap();
            let store = Store::open_in_memory().unwrap();
            setup_project(&store, "bounded_project", dir.path().to_str().unwrap());

            let mut file_hashes = Vec::new();
            for i in 0..total {
                let rel_path = format!("src/file_{}.rs", i);
                let abs_path = dir.path().join(&rel_path);
                if let Some(parent) = abs_path.parent() {
                    std::fs::create_dir_all(parent).unwrap();
                }

                let content = format!("// content {}", i);
                std::fs::write(&abs_path, &content).unwrap();

                let hash = if i < changed {
                    // Stale: store wrong hash
                    hex::encode(Sha256::digest(b"wrong"))
                } else {
                    // Fresh: store correct hash
                    hex::encode(Sha256::digest(content.as_bytes()))
                };

                file_hashes.push(FileHash {
                    project: "bounded_project".to_string(),
                    rel_path,
                    sha256: hash,
                    mtime_ns: 0,
                    size: content.len() as i64,
                });
            }

            store.store_file_hashes_batch(&file_hashes).unwrap();

            let report = compute_staleness(
                &store,
                "bounded_project",
                dir.path(),
                None,
            ).unwrap();

            prop_assert!(report.score >= 0.0, "score {} < 0.0", report.score);
            prop_assert!(report.score <= 1.0, "score {} > 1.0", report.score);
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Property: Dependency graph edge faithfulness
// ═══════════════════════════════════════════════════════════════════════

mod property_dependency_graph_edges {
    use codryn_services::dependency_graph::{get_dependency_graph, Granularity};
    use codryn_store::{Edge, Node, Project, Store};
    use proptest::prelude::*;

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(30))]

        #[test]
        fn all_imports_edges_appear_in_dependency_graph(
            edge_count in 1usize..=5,
        ) {
            let store = Store::open_in_memory().unwrap();
            store.upsert_project(&Project {
                name: "p".into(), indexed_at: "now".into(), root_path: "/tmp".into(),
            }).unwrap();

            let mut expected_edges = Vec::new();
            for i in 0..edge_count {
                let src_file = format!("src/mod_{}.rs", i);
                let tgt_file = format!("src/dep_{}.rs", i);

                let src_id = store.insert_node(&Node {
                    id: 0, project: "p".into(), label: "Function".into(),
                    name: format!("fn_{}", i),
                    qualified_name: format!("p.fn_{}", i),
                    file_path: src_file.clone(),
                    start_line: 0, end_line: 0, properties_json: None,
                }).unwrap();

                let tgt_id = store.insert_node(&Node {
                    id: 0, project: "p".into(), label: "Module".into(),
                    name: format!("dep_{}", i),
                    qualified_name: format!("p.dep_{}", i),
                    file_path: tgt_file.clone(),
                    start_line: 0, end_line: 0, properties_json: None,
                }).unwrap();

                store.insert_edge(&Edge {
                    id: 0, project: "p".into(),
                    source_id: src_id, target_id: tgt_id,
                    edge_type: "IMPORTS".into(), properties_json: None,
                }).unwrap();

                expected_edges.push((src_file, tgt_file));
            }

            let result = get_dependency_graph(&store, "p", Granularity::File, None, false).unwrap();

            for (from, to) in &expected_edges {
                let found = result.edges.iter().any(|e| &e.from == from && &e.to == to);
                prop_assert!(found,
                    "Edge {} -> {} not found in dependency graph", from, to);
            }
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Property 16: NL-to-Cypher Template Matching
// ═══════════════════════════════════════════════════════════════════════

/// **Validates: Requirement 15.2**
/// Questions matching known templates must produce Cypher containing the entity name.
mod property16_nl_to_cypher_template_matching {
    use codryn_services::nl_to_cypher::NLToCypherService;
    use codryn_store::{Project, Store};
    use proptest::prelude::*;

    fn setup_store(project: &str) -> Store {
        let store = Store::open_in_memory().unwrap();
        store
            .upsert_project(&Project {
                name: project.to_string(),
                indexed_at: "2024-01-01T00:00:00Z".to_string(),
                root_path: "/tmp".to_string(),
            })
            .unwrap();
        store
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(30))]

        #[test]
        fn who_calls_template_contains_entity(
            entity in "[a-z][a-z0-9]{2,15}",
            project in "[a-z]{3,10}",
        ) {
            let store = setup_store(&project);
            let question = format!("who calls {}", entity);
            let result = NLToCypherService::translate_and_execute(&store, &project, &question).unwrap();
            prop_assert!(
                result.cypher.contains(&entity),
                "Cypher '{}' should contain entity '{}'", result.cypher, entity
            );
            prop_assert_eq!(result.matched_template.as_deref(), Some("who_calls"));
        }

        #[test]
        fn what_imports_template_contains_entity(
            entity in "[a-z][a-z0-9]{2,15}",
            project in "[a-z]{3,10}",
        ) {
            let store = setup_store(&project);
            let question = format!("what imports {}", entity);
            let result = NLToCypherService::translate_and_execute(&store, &project, &question).unwrap();
            prop_assert!(
                result.cypher.contains(&entity),
                "Cypher '{}' should contain entity '{}'", result.cypher, entity
            );
            prop_assert_eq!(result.matched_template.as_deref(), Some("who_imports"));
        }

        #[test]
        fn inheritance_template_contains_entity(
            entity in "[a-z][a-z0-9]{2,15}",
            project in "[a-z]{3,10}",
        ) {
            let store = setup_store(&project);
            let question = format!("inheritance of {}", entity);
            let result = NLToCypherService::translate_and_execute(&store, &project, &question).unwrap();
            prop_assert!(
                result.cypher.contains(&entity),
                "Cypher '{}' should contain entity '{}'", result.cypher, entity
            );
            prop_assert_eq!(result.matched_template.as_deref(), Some("inheritance"));
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Property: API Surface Export Filter
// ═══════════════════════════════════════════════════════════════════════

/// Only exported symbols appear in get_api_surface results.
mod property_api_surface_export_filter {
    use codryn_services::api_surface::APISurfaceService;
    use codryn_store::{Node, Project, Store};
    use proptest::prelude::*;

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(30))]

        #[test]
        fn only_exported_symbols_returned(
            project in "[a-z]{3,10}",
            n_exported in 1usize..5,
            n_unexported in 1usize..5,
        ) {
            let store = Store::open_in_memory().unwrap();
            store.upsert_project(&Project {
                name: project.clone(),
                indexed_at: "2024-01-01T00:00:00Z".to_string(),
                root_path: "/tmp".to_string(),
            }).unwrap();

            // Insert exported nodes
            for i in 0..n_exported {
                store.insert_node(&Node {
                    id: 0, project: project.clone(), label: "Function".into(),
                    name: format!("exported{}", i),
                    qualified_name: format!("{}::exported{}", project, i),
                    file_path: "src/lib.rs".into(),
                    start_line: 1, end_line: 10,
                    properties_json: Some(r#"{"is_exported":true}"#.into()),
                }).unwrap();
            }
            // Insert unexported nodes
            for i in 0..n_unexported {
                store.insert_node(&Node {
                    id: 0, project: project.clone(), label: "Function".into(),
                    name: format!("unexported{}", i),
                    qualified_name: format!("{}::unexported{}", project, i),
                    file_path: "src/lib.rs".into(),
                    start_line: 1, end_line: 10,
                    properties_json: Some(r#"{"is_exported":false}"#.into()),
                }).unwrap();
            }

            let result = APISurfaceService::get_api_surface(&store, &project, None, None, 100, false).unwrap();
            prop_assert_eq!(result.symbols.len(), n_exported, "only exported symbols should be returned");
            for sym in &result.symbols {
                prop_assert!(sym.name.starts_with("exported"), "unexpected symbol: {}", sym.name);
            }
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Property: Test Gap Coverage Ratio
// ═══════════════════════════════════════════════════════════════════════

/// Nodes with TESTS edges are counted as tested; total >= tested always.
mod property_test_gap_coverage {
    use codryn_services::test_gap::TestGapService;
    use codryn_store::{Edge, Node, Project, Store};
    use proptest::prelude::*;

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(30))]

        #[test]
        fn coverage_total_gte_tested(
            n_tested in 1usize..5,
            n_untested in 1usize..5,
        ) {
            let store = Store::open_in_memory().unwrap();
            let project = "p";
            store.upsert_project(&Project {
                name: project.into(), indexed_at: "now".into(), root_path: "/tmp".into(),
            }).unwrap();

            let mut ids = Vec::new();
            let total = n_tested + n_untested;

            // Insert all nodes (non-test file path)
            for i in 0..total {
                let id = store.insert_node(&Node {
                    id: 0, project: project.into(), label: "Function".into(),
                    name: format!("fn_{}", i),
                    qualified_name: format!("p.fn_{}", i),
                    file_path: "src/lib.rs".into(),
                    start_line: i as i32, end_line: i as i32,
                    properties_json: None,
                }).unwrap();
                ids.push(id);
            }

            // Insert a test node and TESTS edges for the first n_tested
            let test_id = store.insert_node(&Node {
                id: 0, project: project.into(), label: "Function".into(),
                name: "test_fn".into(),
                qualified_name: "p.test_fn".into(),
                file_path: "tests/test_lib.rs".into(),
                start_line: 0, end_line: 0, properties_json: None,
            }).unwrap();

            for &target_id in &ids[..n_tested] {
                store.insert_edge(&Edge {
                    id: 0, project: project.into(),
                    source_id: test_id, target_id,
                    edge_type: "TESTS".into(), properties_json: None,
                }).unwrap();
            }

            let result = TestGapService::test_coverage_map(&store, project, None, false, 100).unwrap();

            // module_coverage should show total >= tested
            let total_symbols: usize = result.module_coverage.iter().map(|m| m.total).sum();
            let tested_symbols: usize = result.module_coverage.iter().map(|m| m.tested).sum();
            prop_assert!(total_symbols >= tested_symbols,
                "total {} must be >= tested {}", total_symbols, tested_symbols);
            // The tested count should match n_tested (only src/lib.rs nodes counted)
            prop_assert_eq!(tested_symbols, n_tested,
                "tested count should match n_tested");
            // Untested list should have n_untested entries
            prop_assert_eq!(result.untested.len(), n_untested,
                "untested count should match n_untested");
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Property 18: Semver Freshness Categorization
// **Validates: Requirements 33.2**
//
// For any pair of semantic version strings (declared, latest), the
// categorization SHALL be: "up-to-date" when they are equal,
// "patch-available" when only patch differs, "minor-available" when
// minor differs (with any patch), "major-available" when major differs,
// and "deprecated" when the registry marks the package as deprecated.
// ═══════════════════════════════════════════════════════════════════════

mod property18_semver_freshness_categorization {
    use codryn_services::dep_freshness::{categorize, FreshnessCategory};
    use proptest::prelude::*;

    /// Strategy to generate a valid semver component (0..=99).
    fn semver_component() -> impl Strategy<Value = u32> {
        0u32..100
    }

    /// Strategy to generate a semver version string "major.minor.patch".
    fn semver_strategy() -> impl Strategy<Value = (u32, u32, u32)> {
        (semver_component(), semver_component(), semver_component())
    }

    /// Format a semver tuple as a version string.
    fn fmt_ver(major: u32, minor: u32, patch: u32) -> String {
        format!("{}.{}.{}", major, minor, patch)
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(200))]

        /// Property 18a: Equal versions → UpToDate.
        ///
        /// When declared == latest, the category must be UpToDate.
        #[test]
        fn equal_versions_are_up_to_date(
            (major, minor, patch) in semver_strategy()
        ) {
            let version = fmt_ver(major, minor, patch);
            let result = categorize(&version, &version);
            prop_assert_eq!(
                result,
                FreshnessCategory::UpToDate,
                "Equal versions {} should be UpToDate",
                version
            );
        }

        /// Property 18b: Same major+minor, higher patch → PatchAvailable.
        ///
        /// When only the patch component differs (latest > declared),
        /// the category must be PatchAvailable.
        #[test]
        fn same_major_minor_different_patch_is_patch_available(
            (major, minor, patch_declared) in semver_strategy(),
            patch_increment in 1u32..50,
        ) {
            let patch_latest = patch_declared + patch_increment;
            let declared = fmt_ver(major, minor, patch_declared);
            let latest = fmt_ver(major, minor, patch_latest);
            let result = categorize(&declared, &latest);
            prop_assert_eq!(
                result,
                FreshnessCategory::PatchAvailable,
                "Versions {} vs {} should be PatchAvailable",
                declared, latest
            );
        }

        /// Property 18c: Same major, different minor → MinorAvailable.
        ///
        /// When the major is the same but minor differs (latest > declared),
        /// the category must be MinorAvailable regardless of patch values.
        #[test]
        fn same_major_different_minor_is_minor_available(
            major in semver_component(),
            minor_declared in 0u32..50,
            minor_increment in 1u32..50,
            patch_declared in semver_component(),
            patch_latest in semver_component(),
        ) {
            let minor_latest = minor_declared + minor_increment;
            let declared = fmt_ver(major, minor_declared, patch_declared);
            let latest = fmt_ver(major, minor_latest, patch_latest);
            let result = categorize(&declared, &latest);
            prop_assert_eq!(
                result,
                FreshnessCategory::MinorAvailable,
                "Versions {} vs {} should be MinorAvailable",
                declared, latest
            );
        }

        /// Property 18d: Different major → MajorAvailable.
        ///
        /// When the major version differs (latest > declared),
        /// the category must be MajorAvailable regardless of minor/patch.
        #[test]
        fn different_major_is_major_available(
            major_declared in 0u32..50,
            major_increment in 1u32..50,
            minor_declared in semver_component(),
            minor_latest in semver_component(),
            patch_declared in semver_component(),
            patch_latest in semver_component(),
        ) {
            let major_latest = major_declared + major_increment;
            let declared = fmt_ver(major_declared, minor_declared, patch_declared);
            let latest = fmt_ver(major_latest, minor_latest, patch_latest);
            let result = categorize(&declared, &latest);
            prop_assert_eq!(
                result,
                FreshnessCategory::MajorAvailable,
                "Versions {} vs {} should be MajorAvailable",
                declared, latest
            );
        }

        /// Property 18e: Declared >= latest → UpToDate.
        ///
        /// When the declared version is greater than or equal to the latest,
        /// the category must be UpToDate (not outdated).
        #[test]
        fn declared_gte_latest_is_up_to_date(
            (major_latest, minor_latest, patch_latest) in semver_strategy(),
            major_bump in 0u32..10,
            minor_bump in 0u32..10,
            patch_bump in 0u32..10,
        ) {
            // Ensure declared >= latest by adding non-negative bumps
            let major_declared = major_latest + major_bump;
            let minor_declared = if major_bump > 0 { minor_latest } else { minor_latest + minor_bump };
            let patch_declared = if major_bump > 0 || minor_bump > 0 {
                patch_latest
            } else {
                patch_latest + patch_bump
            };

            let declared = fmt_ver(major_declared, minor_declared, patch_declared);
            let latest = fmt_ver(major_latest, minor_latest, patch_latest);
            let result = categorize(&declared, &latest);
            prop_assert_eq!(
                result,
                FreshnessCategory::UpToDate,
                "Declared {} >= latest {} should be UpToDate",
                declared, latest
            );
        }
    }
}
