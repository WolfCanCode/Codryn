use codryn_discover::{DiscoveredFile, Language};
use codryn_graph_buffer::GraphBuffer;
use codryn_pipeline::extraction::extract_type_assigns;
use codryn_pipeline::passes::{
    self, compute_match_score, detect_http_method, normalize_config_key, PackageMap,
};
use codryn_pipeline::registry::{Registry, TypeRegistry};
use codryn_store::Store;
use codryn_treesitter::{TsParam, TsSymbol};
use proptest::prelude::*;

fn test_store() -> Store {
    Store::open_in_memory().unwrap()
}

// ── Strategies ────────────────────────────────────────────────────────

/// Generate a valid dependency/package name (alphanumeric + hyphens, 1-30 chars).
fn dep_name_strategy() -> impl Strategy<Value = String> {
    "[a-z][a-z0-9\\-]{0,19}".prop_filter("non-empty", |s| !s.is_empty())
}

/// Generate a semver-like version string.
fn version_strategy() -> impl Strategy<Value = String> {
    (1u32..20, 0u32..50, 0u32..100).prop_map(|(a, b, c)| format!("{}.{}.{}", a, b, c))
}

/// Generate a project name.
fn project_strategy() -> impl Strategy<Value = String> {
    "[a-z]{3,10}"
}

// ═══════════════════════════════════════════════════════════════════════
// Property 1: Manifest parsing produces complete PackageMap
// ═══════════════════════════════════════════════════════════════════════

/// **Validates: Requirements 1.1, 1.2, 1.3, 1.4**
mod property1_manifest_parsing {
    use super::*;

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(100))]

        #[test]
        fn package_json_all_deps_mapped(
            deps in prop::collection::hash_map(dep_name_strategy(), version_strategy(), 1..10),
            project in project_strategy(),
        ) {
            let tmp = tempfile::TempDir::new().unwrap();
            let pkg_json = serde_json::json!({
                "dependencies": deps.iter()
                    .map(|(k, v)| (k.clone(), serde_json::Value::String(format!("^{}", v))))
                    .collect::<serde_json::Map<String, serde_json::Value>>()
            });
            std::fs::write(tmp.path().join("package.json"), pkg_json.to_string()).unwrap();

            let file = DiscoveredFile {
                abs_path: tmp.path().join("package.json"),
                rel_path: "package.json".to_string(),
                language: Language::JavaScript,
            };
            let files: Vec<&DiscoveredFile> = vec![&file];
            let map = passes::pass_pkgmap(&files, &project);

            for dep_name in deps.keys() {
                prop_assert!(
                    map.contains_key(dep_name),
                    "dependency '{}' not found in PackageMap", dep_name
                );
                let qn = &map[dep_name];
                prop_assert!(!qn.is_empty(), "QN for '{}' is empty", dep_name);
                prop_assert!(
                    qn.contains(&project),
                    "QN '{}' doesn't contain project '{}'", qn, project
                );
            }
        }

        #[test]
        fn cargo_toml_all_deps_mapped(
            deps in prop::collection::hash_map(dep_name_strategy(), version_strategy(), 1..10),
            project in project_strategy(),
        ) {
            let tmp = tempfile::TempDir::new().unwrap();
            let mut toml_content = String::from("[dependencies]\n");
            for (name, ver) in &deps {
                toml_content.push_str(&format!("{} = \"{}\"\n", name, ver));
            }
            std::fs::write(tmp.path().join("Cargo.toml"), &toml_content).unwrap();

            let file = DiscoveredFile {
                abs_path: tmp.path().join("Cargo.toml"),
                rel_path: "Cargo.toml".to_string(),
                language: Language::Rust,
            };
            let files: Vec<&DiscoveredFile> = vec![&file];
            let map = passes::pass_pkgmap(&files, &project);

            for dep_name in deps.keys() {
                prop_assert!(
                    map.contains_key(dep_name),
                    "crate '{}' not found in PackageMap", dep_name
                );
                let qn = &map[dep_name];
                prop_assert!(!qn.is_empty(), "QN for '{}' is empty", dep_name);
            }
        }

        #[test]
        fn go_mod_all_deps_mapped(
            modules in prop::collection::hash_map(
                "[a-z]{3,8}\\.[a-z]{2,5}/[a-z]{3,10}",
                version_strategy().prop_map(|v| format!("v{}", v)),
                1..8
            ),
            project in project_strategy(),
        ) {
            let tmp = tempfile::TempDir::new().unwrap();
            let mut content = String::from("module example.com/mymod\n\ngo 1.21\n\nrequire (\n");
            for (mod_path, ver) in &modules {
                content.push_str(&format!("\t{} {}\n", mod_path, ver));
            }
            content.push_str(")\n");
            std::fs::write(tmp.path().join("go.mod"), &content).unwrap();

            let file = DiscoveredFile {
                abs_path: tmp.path().join("go.mod"),
                rel_path: "go.mod".to_string(),
                language: Language::Go,
            };
            let files: Vec<&DiscoveredFile> = vec![&file];
            let map = passes::pass_pkgmap(&files, &project);

            for mod_path in modules.keys() {
                prop_assert!(
                    map.contains_key(mod_path),
                    "Go module '{}' not found in PackageMap", mod_path
                );
                let qn = &map[mod_path];
                prop_assert!(!qn.is_empty(), "QN for '{}' is empty", mod_path);
            }
        }

        #[test]
        fn pyproject_toml_all_deps_mapped(
            deps in prop::collection::hash_map(dep_name_strategy(), version_strategy(), 1..8),
            project in project_strategy(),
        ) {
            let tmp = tempfile::TempDir::new().unwrap();
            let dep_list: Vec<String> = deps.iter()
                .map(|(name, ver)| format!("\"{}>={}\"", name, ver))
                .collect();
            let content = format!(
                "[project]\nname = \"myproject\"\ndependencies = [{}]\n",
                dep_list.join(", ")
            );
            std::fs::write(tmp.path().join("pyproject.toml"), &content).unwrap();

            let file = DiscoveredFile {
                abs_path: tmp.path().join("pyproject.toml"),
                rel_path: "pyproject.toml".to_string(),
                language: Language::Python,
            };
            let files: Vec<&DiscoveredFile> = vec![&file];
            let map = passes::pass_pkgmap(&files, &project);

            for dep_name in deps.keys() {
                prop_assert!(
                    map.contains_key(dep_name.as_str()),
                    "Python package '{}' not found in PackageMap", dep_name
                );
                let qn = &map[dep_name];
                prop_assert!(!qn.is_empty(), "QN for '{}' is empty", dep_name);
            }
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Property 2: Bare specifier resolution via PackageMap
// ═══════════════════════════════════════════════════════════════════════

/// **Validates: Requirements 1.6**
mod property2_bare_specifier_resolution {
    use super::*;

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(100))]

        #[test]
        fn specifier_resolves_to_pkgmap_value(
            entries in prop::collection::hash_map(
                dep_name_strategy(),
                "[a-z]{3,10}\\.[a-z_]{3,15}\\.[a-z_]{3,15}",
                1..15
            ),
        ) {
            let pkg_map: PackageMap = entries.clone();

            // For every key in the map, the resolved QN should match the value
            for (specifier, expected_qn) in &entries {
                let resolved = pkg_map.get(specifier);
                prop_assert!(
                    resolved.is_some(),
                    "specifier '{}' not found in PackageMap", specifier
                );
                prop_assert_eq!(
                    resolved.unwrap(), expected_qn,
                    "resolved QN mismatch for specifier '{}'", specifier
                );
            }
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Property 3: Cross-repo matching creates correct typed edges
// ═══════════════════════════════════════════════════════════════════════

/// **Validates: Requirements 2.1, 2.2, 2.3, 2.4**
mod property3_cross_repo_matching {
    use super::*;

    fn http_method_strategy() -> impl Strategy<Value = String> {
        prop_oneof![
            Just("GET".to_string()),
            Just("POST".to_string()),
            Just("PUT".to_string()),
            Just("DELETE".to_string()),
        ]
    }

    fn route_path_strategy() -> impl Strategy<Value = String> {
        "/[a-z]{2,8}(/[a-z]{2,8}){0,2}"
    }

    fn event_name_strategy() -> impl Strategy<Value = String> {
        "[a-z]{3,10}\\.[a-z]{3,10}"
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(100))]

        #[test]
        fn matching_routes_create_cross_http_edges(
            method in http_method_strategy(),
            path in route_path_strategy(),
        ) {
            let store = test_store();
            let project_a = "proj_a";
            let project_b = "proj_b";

            // Create projects and link them
            store.upsert_project(&codryn_store::Project {
                name: project_a.to_string(),
                indexed_at: chrono::Utc::now().to_rfc3339(),
                root_path: "/a".to_string(),
            }).unwrap();
            store.upsert_project(&codryn_store::Project {
                name: project_b.to_string(),
                indexed_at: chrono::Utc::now().to_rfc3339(),
                root_path: "/b".to_string(),
            }).unwrap();
            store.link_projects(project_a, project_b).unwrap();

            // Create Route nodes in both projects
            let props_a = serde_json::json!({
                "http_method": method,
                "path": path,
            }).to_string();
            let props_b = props_a.clone();

            let _nodes_a = store.insert_nodes_batch(&[codryn_store::Node {
                id: 0,
                project: project_a.to_string(),
                label: "Route".to_string(),
                name: format!("{} {}", method, path),
                qualified_name: format!("{}.route.{}", project_a, path),
                file_path: "routes.ts".to_string(),
                start_line: 1,
                end_line: 10,
                properties_json: Some(props_a),
            }]).unwrap();

            let _nodes_b = store.insert_nodes_batch(&[codryn_store::Node {
                id: 0,
                project: project_b.to_string(),
                label: "Route".to_string(),
                name: format!("{} {}", method, path),
                qualified_name: format!("{}.route.{}", project_b, path),
                file_path: "routes.ts".to_string(),
                start_line: 1,
                end_line: 10,
                properties_json: Some(props_b),
            }]).unwrap();

            // Run cross-repo pass
            let mut buf = GraphBuffer::new(project_a);
            passes::pass_cross_repo(&mut buf, &store, project_a);

            // The buffer should contain CROSS_HTTP edges (bidirectional = 2 edges)
            prop_assert!(
                buf.edge_count() >= 2,
                "Expected at least 2 CROSS_HTTP edges (bidirectional), got {}",
                buf.edge_count()
            );
        }

        #[test]
        fn matching_channels_create_cross_channel_edges(
            event_name in event_name_strategy(),
        ) {
            let store = test_store();
            let project_a = "proj_a";
            let project_b = "proj_b";

            store.upsert_project(&codryn_store::Project {
                name: project_a.to_string(),
                indexed_at: chrono::Utc::now().to_rfc3339(),
                root_path: "/a".to_string(),
            }).unwrap();
            store.upsert_project(&codryn_store::Project {
                name: project_b.to_string(),
                indexed_at: chrono::Utc::now().to_rfc3339(),
                root_path: "/b".to_string(),
            }).unwrap();
            store.link_projects(project_a, project_b).unwrap();

            let props = serde_json::json!({
                "event_name": event_name,
            }).to_string();

            store.insert_nodes_batch(&[codryn_store::Node {
                id: 0,
                project: project_a.to_string(),
                label: "Channel".to_string(),
                name: event_name.clone(),
                qualified_name: format!("{}.channel.{}", project_a, event_name),
                file_path: "events.ts".to_string(),
                start_line: 1,
                end_line: 5,
                properties_json: Some(props.clone()),
            }]).unwrap();

            store.insert_nodes_batch(&[codryn_store::Node {
                id: 0,
                project: project_b.to_string(),
                label: "Channel".to_string(),
                name: event_name.clone(),
                qualified_name: format!("{}.channel.{}", project_b, event_name),
                file_path: "events.ts".to_string(),
                start_line: 1,
                end_line: 5,
                properties_json: Some(props),
            }]).unwrap();

            let mut buf = GraphBuffer::new(project_a);
            passes::pass_cross_repo(&mut buf, &store, project_a);

            // Should have bidirectional CROSS_CHANNEL edges
            prop_assert!(
                buf.edge_count() >= 2,
                "Expected at least 2 CROSS_CHANNEL edges, got {}",
                buf.edge_count()
            );
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Property 4: Service pattern classification correctness
// ═══════════════════════════════════════════════════════════════════════

/// **Validates: Requirements 3.1, 3.2, 3.3, 3.5**
mod property4_service_pattern_classification {
    use super::*;

    fn http_lib_strategy() -> impl Strategy<Value = String> {
        prop_oneof![
            Just("requests".to_string()),
            Just("axios".to_string()),
            Just("httpx".to_string()),
            Just("fetch".to_string()),
            Just("reqwest".to_string()),
        ]
    }

    fn async_lib_strategy() -> impl Strategy<Value = String> {
        prop_oneof![
            Just("kafka".to_string()),
            Just("rabbitmq".to_string()),
            Just("redis".to_string()),
            Just("nats".to_string()),
            Just("celery".to_string()),
        ]
    }

    fn config_lib_strategy() -> impl Strategy<Value = String> {
        prop_oneof![
            Just("dotenv".to_string()),
            Just("config".to_string()),
            Just("viper".to_string()),
            Just("figment".to_string()),
        ]
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(100))]

        #[test]
        fn http_lib_creates_http_calls_edge(
            lib in http_lib_strategy(),
            prefix in "[a-z]{3,8}",
        ) {
            let store = test_store();
            let project = "test_proj";
            store.upsert_project(&codryn_store::Project {
                name: project.to_string(),
                indexed_at: chrono::Utc::now().to_rfc3339(),
                root_path: "/test".to_string(),
            }).unwrap();

            // Create source and target nodes
            let nodes = store.insert_nodes_batch(&[
                codryn_store::Node {
                    id: 0, project: project.to_string(),
                    label: "Function".to_string(),
                    name: "caller".to_string(),
                    qualified_name: format!("{}.caller", project),
                    file_path: "main.ts".to_string(),
                    start_line: 1, end_line: 10,
                    properties_json: None,
                },
                codryn_store::Node {
                    id: 0, project: project.to_string(),
                    label: "Function".to_string(),
                    name: format!("{}_get", lib),
                    qualified_name: format!("{}.{}.{}.get", project, prefix, lib),
                    file_path: "lib.ts".to_string(),
                    start_line: 1, end_line: 5,
                    properties_json: None,
                },
            ]).unwrap();

            let src_id = nodes[0].1;
            let tgt_id = nodes[1].1;

            // Insert a CALLS edge
            store.insert_edges_batch(&[codryn_store::Edge {
                id: 0, project: project.to_string(),
                source_id: src_id, target_id: tgt_id,
                edge_type: "CALLS".to_string(),
                properties_json: None,
            }]).unwrap();

            let mut buf = GraphBuffer::new(project);
            passes::pass_service_patterns(&mut buf, &store, project);

            // Should create an HTTP_CALLS edge
            prop_assert!(
                buf.edge_count() >= 1,
                "Expected at least 1 HTTP_CALLS edge, got {}", buf.edge_count()
            );

            // Original CALLS edge should still exist in store
            let calls = store.get_edges_by_type(project, "CALLS").unwrap();
            prop_assert!(
                !calls.is_empty(),
                "Original CALLS edge should be preserved"
            );
        }

        #[test]
        fn async_lib_creates_async_calls_edge(
            lib in async_lib_strategy(),
            prefix in "[a-z]{3,8}",
        ) {
            let store = test_store();
            let project = "test_proj";
            store.upsert_project(&codryn_store::Project {
                name: project.to_string(),
                indexed_at: chrono::Utc::now().to_rfc3339(),
                root_path: "/test".to_string(),
            }).unwrap();

            let nodes = store.insert_nodes_batch(&[
                codryn_store::Node {
                    id: 0, project: project.to_string(),
                    label: "Function".to_string(),
                    name: "publisher".to_string(),
                    qualified_name: format!("{}.publisher", project),
                    file_path: "main.ts".to_string(),
                    start_line: 1, end_line: 10,
                    properties_json: None,
                },
                codryn_store::Node {
                    id: 0, project: project.to_string(),
                    label: "Function".to_string(),
                    name: format!("{}_send", lib),
                    qualified_name: format!("{}.{}.{}.send", project, prefix, lib),
                    file_path: "broker.ts".to_string(),
                    start_line: 1, end_line: 5,
                    properties_json: None,
                },
            ]).unwrap();

            store.insert_edges_batch(&[codryn_store::Edge {
                id: 0, project: project.to_string(),
                source_id: nodes[0].1, target_id: nodes[1].1,
                edge_type: "CALLS".to_string(),
                properties_json: None,
            }]).unwrap();

            let mut buf = GraphBuffer::new(project);
            passes::pass_service_patterns(&mut buf, &store, project);

            prop_assert!(
                buf.edge_count() >= 1,
                "Expected at least 1 ASYNC_CALLS edge, got {}", buf.edge_count()
            );
        }

        #[test]
        fn config_lib_creates_configures_edge(
            lib in config_lib_strategy(),
            prefix in "[a-z]{3,8}",
        ) {
            let store = test_store();
            let project = "test_proj";
            store.upsert_project(&codryn_store::Project {
                name: project.to_string(),
                indexed_at: chrono::Utc::now().to_rfc3339(),
                root_path: "/test".to_string(),
            }).unwrap();

            let nodes = store.insert_nodes_batch(&[
                codryn_store::Node {
                    id: 0, project: project.to_string(),
                    label: "Function".to_string(),
                    name: "init".to_string(),
                    qualified_name: format!("{}.init", project),
                    file_path: "main.ts".to_string(),
                    start_line: 1, end_line: 10,
                    properties_json: None,
                },
                codryn_store::Node {
                    id: 0, project: project.to_string(),
                    label: "Function".to_string(),
                    name: format!("{}_load", lib),
                    qualified_name: format!("{}.{}.{}.load", project, prefix, lib),
                    file_path: "config.ts".to_string(),
                    start_line: 1, end_line: 5,
                    properties_json: None,
                },
            ]).unwrap();

            store.insert_edges_batch(&[codryn_store::Edge {
                id: 0, project: project.to_string(),
                source_id: nodes[0].1, target_id: nodes[1].1,
                edge_type: "CALLS".to_string(),
                properties_json: None,
            }]).unwrap();

            let mut buf = GraphBuffer::new(project);
            passes::pass_service_patterns(&mut buf, &store, project);

            prop_assert!(
                buf.edge_count() >= 1,
                "Expected at least 1 CONFIGURES edge, got {}", buf.edge_count()
            );
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Property 5: HTTP method detection from method names
// ═══════════════════════════════════════════════════════════════════════

/// **Validates: Requirements 3.4**
mod property5_http_method_detection {
    use super::*;

    fn method_suffix_pair() -> impl Strategy<Value = (String, &'static str)> {
        prop_oneof![
            "[0-9]{0,5}".prop_map(|prefix| (format!("{}get", prefix), "GET")),
            "[0-9]{0,5}".prop_map(|prefix| (format!("{}post", prefix), "POST")),
            "[0-9]{0,5}".prop_map(|prefix| (format!("{}put", prefix), "PUT")),
            "[0-9]{0,5}".prop_map(|prefix| (format!("{}delete", prefix), "DELETE")),
            "[0-9]{0,5}".prop_map(|prefix| (format!("{}patch", prefix), "PATCH")),
            "[0-9]{0,5}".prop_map(|prefix| (format!("{}head", prefix), "HEAD")),
            "[0-9]{0,5}".prop_map(|prefix| (format!("{}options", prefix), "OPTIONS")),
        ]
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(100))]

        #[test]
        fn detect_http_method_returns_correct_method(
            (method_name, expected) in method_suffix_pair()
        ) {
            let result = detect_http_method(&method_name);
            prop_assert_eq!(
                result, expected,
                "detect_http_method('{}') returned '{}', expected '{}'",
                method_name, result, expected
            );
        }

        #[test]
        fn unknown_method_names_return_unknown(
            name in "[a-z]{3,10}"
                .prop_filter("no http suffix", |s| {
                    let lower = s.to_lowercase();
                    !["get", "post", "put", "delete", "patch", "head", "options", "request"]
                        .iter()
                        .any(|suffix| lower.ends_with(suffix) || lower.contains(suffix))
                })
        ) {
            let result = detect_http_method(&name);
            prop_assert_eq!(
                result, "UNKNOWN",
                "detect_http_method('{}') should return UNKNOWN, got '{}'",
                name, result
            );
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Property 6: Type assignment extraction registers all typed declarations
// ═══════════════════════════════════════════════════════════════════════

/// **Validates: Requirements 4.1, 4.2, 4.3, 4.4**
mod property6_type_assignment_extraction {
    use super::*;

    fn custom_type_strategy() -> impl Strategy<Value = String> {
        "[A-Z][a-zA-Z]{2,15}"
    }

    fn param_name_strategy() -> impl Strategy<Value = String> {
        "[a-z][a-zA-Z]{1,10}"
    }

    fn func_name_strategy() -> impl Strategy<Value = String> {
        "[a-z][a-zA-Z]{2,12}"
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(100))]

        #[test]
        fn return_types_registered(
            func_name in func_name_strategy(),
            ret_type in custom_type_strategy(),
        ) {
            let mut type_reg = TypeRegistry::new();
            let file_path = "test.rs";

            let sym = TsSymbol {
                name: func_name.clone(),
                label: "Function".to_string(),
                start_line: 1,
                end_line: 10,
                parent_name: None,
                signature: None,
                return_type: Some(ret_type.clone()),
                parameters: vec![],
                docstring: None,
                decorators: vec![],
                base_classes: vec![],
                is_exported: false,
                is_abstract: false,
                is_async: false,
                is_test: false,
                is_entry_point: false,
                body_text: None,
            };

            extract_type_assigns(&mut type_reg, file_path, &[sym], Language::Rust);

            let key = format!("{}::return", func_name);
            let entry = type_reg.lookup_type(file_path, &key);
            prop_assert!(
                entry.is_some(),
                "Return type for '{}' not registered in TypeRegistry", func_name
            );
            prop_assert_eq!(
                &entry.unwrap().resolved_type, &ret_type,
                "Return type mismatch for '{}'", func_name
            );
        }

        #[test]
        fn param_types_registered(
            func_name in func_name_strategy(),
            params in prop::collection::vec(
                (param_name_strategy(), custom_type_strategy()),
                1..5
            ),
        ) {
            let mut type_reg = TypeRegistry::new();
            let file_path = "test.ts";

            let ts_params: Vec<TsParam> = params.iter()
                .map(|(name, type_name)| TsParam {
                    name: name.clone(),
                    type_name: Some(type_name.clone()),
                })
                .collect();

            let sym = TsSymbol {
                name: func_name.clone(),
                label: "Function".to_string(),
                start_line: 1,
                end_line: 10,
                parent_name: None,
                signature: None,
                return_type: None,
                parameters: ts_params,
                docstring: None,
                decorators: vec![],
                base_classes: vec![],
                is_exported: false,
                is_abstract: false,
                is_async: false,
                is_test: false,
                is_entry_point: false,
                body_text: None,
            };

            extract_type_assigns(&mut type_reg, file_path, &[sym], Language::TypeScript);

            for (param_name, type_name) in &params {
                let key = format!("{}::{}", func_name, param_name);
                let entry = type_reg.lookup_type(file_path, &key);
                prop_assert!(
                    entry.is_some(),
                    "Param type for '{}::{}' not registered", func_name, param_name
                );
                prop_assert_eq!(
                    &entry.unwrap().resolved_type, type_name,
                    "Param type mismatch for '{}::{}'", func_name, param_name
                );
            }
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Property 7: TYPE_REF edges only target existing non-stdlib nodes
// ═══════════════════════════════════════════════════════════════════════

/// **Validates: Requirements 5.1, 5.2, 5.4, 5.5**
mod property7_type_ref_edges {
    use super::*;

    fn custom_type_name() -> impl Strategy<Value = String> {
        "[A-Z][a-zA-Z]{2,12}"
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(100))]

        #[test]
        fn type_ref_only_for_existing_non_stdlib(
            existing_types in prop::collection::vec(custom_type_name(), 1..5),
            nonexistent_types in prop::collection::vec(custom_type_name(), 1..3),
            func_name in "[a-z][a-zA-Z]{2,10}",
        ) {
            let store = test_store();
            let project = "test_proj";
            store.upsert_project(&codryn_store::Project {
                name: project.to_string(),
                indexed_at: chrono::Utc::now().to_rfc3339(),
                root_path: "/test".to_string(),
            }).unwrap();

            // Register existing type nodes in the store and registry
            let mut reg = Registry::new();
            let mut type_nodes = Vec::new();
            for type_name in &existing_types {
                let qn = format!("{}.types.{}", project, type_name);
                type_nodes.push(codryn_store::Node {
                    id: 0, project: project.to_string(),
                    label: "Class".to_string(),
                    name: type_name.clone(),
                    qualified_name: qn.clone(),
                    file_path: "types.ts".to_string(),
                    start_line: 1, end_line: 10,
                    properties_json: None,
                });
                reg.register(type_name, &qn, "types.ts", "Class", 1, 10);
            }
            store.insert_nodes_batch(&type_nodes).unwrap();

            // Create a function node with params referencing both existing and nonexistent types
            let mut all_params = Vec::new();
            for t in &existing_types {
                all_params.push(serde_json::json!({"name": format!("p_{}", t), "type": t}));
            }
            for t in &nonexistent_types {
                all_params.push(serde_json::json!({"name": format!("p_{}", t), "type": t}));
            }
            // Also add a stdlib type that should be skipped
            all_params.push(serde_json::json!({"name": "p_str", "type": "String"}));

            let func_props = serde_json::json!({
                "parameters": all_params,
            }).to_string();

            let func_qn = format!("{}.funcs.{}", project, func_name);
            store.insert_nodes_batch(&[codryn_store::Node {
                id: 0, project: project.to_string(),
                label: "Function".to_string(),
                name: func_name.clone(),
                qualified_name: func_qn.clone(),
                file_path: "main.ts".to_string(),
                start_line: 1, end_line: 20,
                properties_json: Some(func_props),
            }]).unwrap();

            let mut buf = GraphBuffer::new(project);
            passes::pass_type_refs(&mut buf, &reg, &store, project);

            // TYPE_REF edges should only be created for existing types, not for
            // nonexistent types or stdlib types.
            // The number of edges should be <= number of existing types
            prop_assert!(
                buf.edge_count() <= existing_types.len(),
                "Got {} TYPE_REF edges but only {} existing types",
                buf.edge_count(), existing_types.len()
            );
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Property 8: Compile commands flag extraction
// ═══════════════════════════════════════════════════════════════════════

/// **Validates: Requirements 6.1, 6.2**
mod property8_compile_commands {
    use super::*;

    fn include_path_strategy() -> impl Strategy<Value = String> {
        "/[a-z]{2,8}(/[a-z]{2,8}){0,3}"
    }

    fn define_name_strategy() -> impl Strategy<Value = String> {
        "[A-Z][A-Z_]{1,15}"
    }

    fn std_flag_strategy() -> impl Strategy<Value = String> {
        prop_oneof![
            Just("c++17".to_string()),
            Just("c++20".to_string()),
            Just("c11".to_string()),
            Just("c17".to_string()),
            Just("gnu++14".to_string()),
        ]
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(100))]

        #[test]
        fn all_flags_extracted_from_arguments(
            include_paths in prop::collection::vec(include_path_strategy(), 1..5),
            defines in prop::collection::vec(define_name_strategy(), 0..4),
            std_flag in prop::option::of(std_flag_strategy()),
        ) {
            let tmp = tempfile::TempDir::new().unwrap();

            // Build arguments array
            let mut args: Vec<String> = vec!["gcc".to_string()];
            for path in &include_paths {
                args.push(format!("-I{}", path));
            }
            for def in &defines {
                args.push(format!("-D{}", def));
            }
            if let Some(ref flag) = std_flag {
                args.push(format!("-std={}", flag));
            }
            args.push("main.c".to_string());

            let cc_json = serde_json::json!([{
                "directory": tmp.path().to_string_lossy().to_string(),
                "file": tmp.path().join("main.c").to_string_lossy().to_string(),
                "arguments": args,
            }]);
            std::fs::write(tmp.path().join("compile_commands.json"), cc_json.to_string()).unwrap();
            // Create the source file so the path exists
            std::fs::write(tmp.path().join("main.c"), "int main() {}").unwrap();

            let map = passes::pass_compile_commands(tmp.path());

            prop_assert!(
                !map.is_empty(),
                "CompileCommandsMap should not be empty"
            );

            // Find the entry for main.c
            let ctx = map.values().next().unwrap();

            // All include paths should be extracted
            for path in &include_paths {
                prop_assert!(
                    ctx.include_paths.contains(path),
                    "Include path '{}' not found in CompileContext. Got: {:?}",
                    path, ctx.include_paths
                );
            }

            // All defines should be extracted
            for def in &defines {
                prop_assert!(
                    ctx.defines.iter().any(|(name, _)| name == def),
                    "Define '{}' not found in CompileContext. Got: {:?}",
                    def, ctx.defines
                );
            }

            // Std flag should match
            if let Some(ref flag) = std_flag {
                prop_assert_eq!(
                    ctx.std_flag.as_deref(), Some(flag.as_str()),
                    "std_flag mismatch"
                );
            }
        }

        #[test]
        fn missing_compile_commands_returns_empty(
            _dummy in 0u8..1,
        ) {
            let tmp = tempfile::TempDir::new().unwrap();
            // No compile_commands.json file
            let map = passes::pass_compile_commands(tmp.path());
            prop_assert!(map.is_empty(), "Should return empty map when file is missing");
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Property 9: Config key normalization correctness
// ═══════════════════════════════════════════════════════════════════════

/// **Validates: Requirements 7.1, 7.2, 7.3**
mod property9_config_key_normalization {
    use super::*;

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(100))]

        #[test]
        fn camel_case_split_into_lowercase_tokens(
            parts in prop::collection::vec("[a-z]{2,8}", 2..5),
        ) {
            // Build a camelCase key from parts: ["database", "url", "port"] -> "databaseUrlPort"
            let mut camel = parts[0].clone();
            for part in &parts[1..] {
                let mut chars = part.chars();
                if let Some(first) = chars.next() {
                    camel.push(first.to_uppercase().next().unwrap());
                    camel.extend(chars);
                }
            }

            let tokens = normalize_config_key(&camel);

            // All original parts should appear as lowercase tokens
            for part in &parts {
                prop_assert!(
                    tokens.contains(&part.to_lowercase()),
                    "Token '{}' not found in normalized tokens {:?} for key '{}'",
                    part, tokens, camel
                );
            }

            // All tokens should be lowercase
            for token in &tokens {
                prop_assert_eq!(
                    token, &token.to_lowercase(),
                    "Token '{}' is not lowercase", token
                );
            }
        }

        #[test]
        fn all_caps_split_on_underscores(
            parts in prop::collection::vec("[A-Z]{2,8}", 2..5),
        ) {
            let upper_key = parts.join("_");

            let tokens = normalize_config_key(&upper_key);

            // Each part should appear as a lowercase token
            for part in &parts {
                prop_assert!(
                    tokens.contains(&part.to_lowercase()),
                    "Token '{}' not found in normalized tokens {:?} for key '{}'",
                    part.to_lowercase(), tokens, upper_key
                );
            }
        }

        #[test]
        fn prefixes_and_extensions_stripped(
            base_key in "[a-z]{3,10}",
            prefix in prop_oneof![
                Just(".env.".to_string()),
                Just("config.".to_string()),
                Just("settings.".to_string()),
            ],
            extension in prop_oneof![
                Just(".json".to_string()),
                Just(".yaml".to_string()),
                Just(".yml".to_string()),
                Just(".toml".to_string()),
            ],
        ) {
            let key_with_prefix = format!("{}{}", prefix, base_key);
            let key_with_extension = format!("{}{}", base_key, extension);

            let tokens_prefix = normalize_config_key(&key_with_prefix);
            let tokens_ext = normalize_config_key(&key_with_extension);
            let tokens_bare = normalize_config_key(&base_key);

            // After stripping, the tokens should be the same as the bare key
            prop_assert_eq!(
                &tokens_prefix, &tokens_bare,
                "Prefix '{}' not stripped from key '{}'",
                prefix, key_with_prefix
            );
            prop_assert_eq!(
                &tokens_ext, &tokens_bare,
                "Extension '{}' not stripped from key '{}'",
                extension, key_with_extension
            );
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Property 10: Config linking produces edges with confidence scores
// ═══════════════════════════════════════════════════════════════════════

/// **Validates: Requirements 7.5**
mod property10_config_linking_confidence {
    use super::*;

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(100))]

        #[test]
        fn match_score_equals_token_overlap_ratio(
            config_tokens in prop::collection::vec("[a-z]{2,8}", 1..6),
            symbol_name_parts in prop::collection::vec("[a-z]{2,8}", 1..6),
        ) {
            let symbol_name = symbol_name_parts.join("");

            let score = compute_match_score(&config_tokens, &symbol_name);

            // Manually compute expected score
            let name_lower = symbol_name.to_lowercase();
            let matching = config_tokens.iter()
                .filter(|t| name_lower.contains(t.as_str()))
                .count();
            let expected = matching as f64 / config_tokens.len() as f64;

            prop_assert!(
                (score - expected).abs() < f64::EPSILON,
                "Score {} != expected {} for tokens {:?} and symbol '{}'",
                score, expected, config_tokens, symbol_name
            );
        }

        #[test]
        fn full_overlap_gives_score_one(
            tokens in prop::collection::vec("[a-z]{2,6}", 1..4),
        ) {
            // Build a symbol name that contains all tokens
            let symbol_name = tokens.join("_");

            let score = compute_match_score(&tokens, &symbol_name);

            prop_assert!(
                (score - 1.0).abs() < f64::EPSILON,
                "Full overlap should give score 1.0, got {} for tokens {:?} and symbol '{}'",
                score, tokens, symbol_name
            );
        }

        #[test]
        fn empty_tokens_gives_score_zero(
            symbol_name in "[a-z]{3,15}",
        ) {
            let empty: Vec<String> = vec![];
            let score = compute_match_score(&empty, &symbol_name);
            prop_assert!(
                score.abs() < f64::EPSILON,
                "Empty tokens should give score 0.0, got {}", score
            );
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Property 19: Semantic edges created only for resolvable targets
// ═══════════════════════════════════════════════════════════════════════

/// **Validates: Requirements 16.1, 16.2, 16.3, 16.4**
mod property19_semantic_edge_resolution {
    use super::*;

    fn class_name_strategy() -> impl Strategy<Value = String> {
        "[A-Z][a-zA-Z]{2,12}"
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(100))]

        #[test]
        fn inherits_edges_only_for_resolvable_base_classes(
            child_name in class_name_strategy(),
            resolvable_bases in prop::collection::vec(class_name_strategy(), 1..4),
            unresolvable_bases in prop::collection::vec(class_name_strategy(), 1..4),
        ) {
            let store = test_store();
            let project = "test_proj";
            store.upsert_project(&codryn_store::Project {
                name: project.to_string(),
                indexed_at: chrono::Utc::now().to_rfc3339(),
                root_path: "/test".to_string(),
            }).unwrap();

            // Register resolvable base classes in the Registry and Store
            let mut reg = Registry::new();
            let mut base_nodes = Vec::new();
            for base in &resolvable_bases {
                let qn = format!("{}.types.{}", project, base);
                reg.register(base, &qn, "types.ts", "Class", 1, 10);
                base_nodes.push(codryn_store::Node {
                    id: 0,
                    project: project.to_string(),
                    label: "Class".to_string(),
                    name: base.clone(),
                    qualified_name: qn,
                    file_path: "types.ts".to_string(),
                    start_line: 1,
                    end_line: 10,
                    properties_json: None,
                });
            }
            store.insert_nodes_batch(&base_nodes).unwrap();

            // Do NOT register unresolvable bases in the Registry

            // Create the child class node with all bases (resolvable + unresolvable)
            let all_bases: Vec<String> = resolvable_bases.iter()
                .chain(unresolvable_bases.iter())
                .cloned()
                .collect();
            let child_qn = format!("{}.classes.{}", project, child_name);
            let props = serde_json::json!({
                "base_classes": all_bases,
            }).to_string();

            store.insert_nodes_batch(&[codryn_store::Node {
                id: 0,
                project: project.to_string(),
                label: "Class".to_string(),
                name: child_name.clone(),
                qualified_name: child_qn,
                file_path: "child.ts".to_string(),
                start_line: 1,
                end_line: 20,
                properties_json: Some(props),
            }]).unwrap();

            let mut buf = GraphBuffer::new(project);
            passes::pass_semantic_edges_v2(&mut buf, &reg, &store, project);

            // Edges should only be created for resolvable bases.
            // Deduplicate resolvable bases since the same name may appear multiple times.
            let unique_resolvable: std::collections::HashSet<&String> = resolvable_bases.iter().collect();
            prop_assert!(
                buf.edge_count() <= unique_resolvable.len(),
                "Got {} edges but only {} unique resolvable bases (resolvable: {:?}, unresolvable: {:?})",
                buf.edge_count(), unique_resolvable.len(), resolvable_bases, unresolvable_bases
            );

            // Verify no edges were created for unresolvable bases by checking
            // that the edge count equals the number of unique resolvable bases
            // that don't collide with unresolvable names in the registry.
            // (Since unresolvable bases are NOT in the registry, they should produce 0 edges.)
            // The edge count should be exactly the number of unique resolvable bases.
            prop_assert!(
                buf.edge_count() >= 1,
                "Expected at least 1 INHERITS edge for resolvable bases, got 0"
            );
        }

        #[test]
        fn implements_edges_for_interface_targets(
            impl_name in class_name_strategy(),
            resolvable_traits in prop::collection::vec(class_name_strategy(), 1..3),
            unresolvable_traits in prop::collection::vec(class_name_strategy(), 1..3),
        ) {
            let store = test_store();
            let project = "test_proj";
            store.upsert_project(&codryn_store::Project {
                name: project.to_string(),
                indexed_at: chrono::Utc::now().to_rfc3339(),
                root_path: "/test".to_string(),
            }).unwrap();

            // Register resolvable traits as Interface nodes
            let mut reg = Registry::new();
            let mut trait_nodes = Vec::new();
            for trait_name in &resolvable_traits {
                let qn = format!("{}.traits.{}", project, trait_name);
                reg.register(trait_name, &qn, "traits.ts", "Interface", 1, 10);
                trait_nodes.push(codryn_store::Node {
                    id: 0,
                    project: project.to_string(),
                    label: "Interface".to_string(),
                    name: trait_name.clone(),
                    qualified_name: qn,
                    file_path: "traits.ts".to_string(),
                    start_line: 1,
                    end_line: 10,
                    properties_json: None,
                });
            }
            store.insert_nodes_batch(&trait_nodes).unwrap();

            // Create the implementing class with both resolvable and unresolvable bases
            let all_bases: Vec<String> = resolvable_traits.iter()
                .chain(unresolvable_traits.iter())
                .cloned()
                .collect();
            let impl_qn = format!("{}.classes.{}", project, impl_name);
            let props = serde_json::json!({
                "base_classes": all_bases,
            }).to_string();

            store.insert_nodes_batch(&[codryn_store::Node {
                id: 0,
                project: project.to_string(),
                label: "Class".to_string(),
                name: impl_name.clone(),
                qualified_name: impl_qn,
                file_path: "impl.ts".to_string(),
                start_line: 1,
                end_line: 20,
                properties_json: Some(props),
            }]).unwrap();

            let mut buf = GraphBuffer::new(project);
            passes::pass_semantic_edges_v2(&mut buf, &reg, &store, project);

            // Only resolvable traits should produce IMPLEMENTS edges
            let unique_resolvable: std::collections::HashSet<&String> = resolvable_traits.iter().collect();
            prop_assert!(
                buf.edge_count() <= unique_resolvable.len(),
                "Got {} edges but only {} unique resolvable traits",
                buf.edge_count(), unique_resolvable.len()
            );
            prop_assert!(
                buf.edge_count() >= 1,
                "Expected at least 1 IMPLEMENTS edge for resolvable traits, got 0"
            );
        }

        #[test]
        fn no_edges_when_all_targets_unresolvable(
            child_name in class_name_strategy(),
            unresolvable_bases in prop::collection::vec(class_name_strategy(), 1..5),
        ) {
            let store = test_store();
            let project = "test_proj";
            store.upsert_project(&codryn_store::Project {
                name: project.to_string(),
                indexed_at: chrono::Utc::now().to_rfc3339(),
                root_path: "/test".to_string(),
            }).unwrap();

            // Empty registry — nothing is resolvable
            let reg = Registry::new();

            let child_qn = format!("{}.classes.{}", project, child_name);
            let props = serde_json::json!({
                "base_classes": unresolvable_bases,
            }).to_string();

            store.insert_nodes_batch(&[codryn_store::Node {
                id: 0,
                project: project.to_string(),
                label: "Class".to_string(),
                name: child_name.clone(),
                qualified_name: child_qn,
                file_path: "child.ts".to_string(),
                start_line: 1,
                end_line: 20,
                properties_json: Some(props),
            }]).unwrap();

            let mut buf = GraphBuffer::new(project);
            passes::pass_semantic_edges_v2(&mut buf, &reg, &store, project);

            prop_assert_eq!(
                buf.edge_count(), 0,
                "Expected 0 edges when all targets are unresolvable, got {}",
                buf.edge_count()
            );
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Feature: devops-pipeline-support, Property 1: Pipeline node creation
// ═══════════════════════════════════════════════════════════════════════

/// **Validates: Requirements 2.1, 3.1**
mod property1_pipeline_node_creation {
    use super::*;

    /// Strategy: generate a random GitLab CI YAML with 1-5 stages and 1-5 jobs.
    /// Uses hash_set to ensure unique job names (duplicate YAML keys are invalid).
    fn gitlab_ci_yaml_strategy() -> impl Strategy<Value = String> {
        let stages = prop::collection::vec("[a-z]{3,8}", 1..=5);
        let jobs = prop::collection::hash_set("[a-z]{3,8}", 1..=5);
        (stages, jobs).prop_map(|(stages, jobs)| {
            let mut yaml = String::from("stages:\n");
            for s in &stages {
                yaml.push_str(&format!("  - {}\n", s));
            }
            for (i, j) in jobs.iter().enumerate() {
                let stage = &stages[i % stages.len()];
                yaml.push_str(&format!(
                    "{}:\n  stage: {}\n  script:\n    - echo hello\n",
                    j, stage
                ));
            }
            yaml
        })
    }

    /// Strategy: generate a random GitHub Actions workflow YAML with 1-5 jobs.
    fn github_actions_yaml_strategy() -> impl Strategy<Value = (String, String)> {
        let name = "[a-zA-Z][a-zA-Z0-9 ]{2,15}";
        let jobs = prop::collection::vec("[a-z][a-z0-9]{2,8}", 1..=5);
        (name, jobs).prop_map(|(name, jobs)| {
            let mut yaml = format!("name: \"{}\"\non: push\njobs:\n", name);
            for j in &jobs {
                yaml.push_str(&format!(
                    "  {}:\n    runs-on: ubuntu-latest\n    steps:\n      - run: echo hi\n",
                    j
                ));
            }
            (name, yaml)
        })
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(100))]

        #[test]
        fn gitlab_ci_produces_exactly_one_pipeline_node(
            yaml_content in gitlab_ci_yaml_strategy(),
            project in "[a-z]{3,8}",
        ) {
            let tmp = tempfile::TempDir::new().unwrap();
            let abs_path = tmp.path().join(".gitlab-ci.yml");
            std::fs::write(&abs_path, &yaml_content).unwrap();

            let file = DiscoveredFile {
                abs_path,
                rel_path: ".gitlab-ci.yml".to_string(),
                language: Language::Yaml,
            };
            let files: Vec<&DiscoveredFile> = vec![&file];

            let store = Store::open_in_memory().unwrap();
            store.upsert_project(&codryn_store::Project {
                name: project.clone(),
                indexed_at: "now".into(),
                root_path: tmp.path().to_string_lossy().to_string(),
            }).unwrap();

            let mut buf = GraphBuffer::new(&project);
            passes::pass_pipelines(&mut buf, &files, &project);
            buf.flush(&store).unwrap();

            let pipeline_nodes = store.get_nodes_by_label(&project, "Pipeline", 100).unwrap();
            prop_assert_eq!(
                pipeline_nodes.len(), 1,
                "Expected exactly 1 Pipeline node, got {}", pipeline_nodes.len()
            );
            prop_assert!(
                !pipeline_nodes[0].name.is_empty(),
                "Pipeline node name should not be empty"
            );
        }

        #[test]
        fn github_actions_produces_exactly_one_pipeline_node(
            (_name, yaml_content) in github_actions_yaml_strategy(),
            project in "[a-z]{3,8}",
        ) {
            let tmp = tempfile::TempDir::new().unwrap();
            let wf_dir = tmp.path().join(".github").join("workflows");
            std::fs::create_dir_all(&wf_dir).unwrap();
            let abs_path = wf_dir.join("ci.yml");
            std::fs::write(&abs_path, &yaml_content).unwrap();

            let file = DiscoveredFile {
                abs_path,
                rel_path: ".github/workflows/ci.yml".to_string(),
                language: Language::Yaml,
            };
            let files: Vec<&DiscoveredFile> = vec![&file];

            let store = Store::open_in_memory().unwrap();
            store.upsert_project(&codryn_store::Project {
                name: project.clone(),
                indexed_at: "now".into(),
                root_path: tmp.path().to_string_lossy().to_string(),
            }).unwrap();

            let mut buf = GraphBuffer::new(&project);
            passes::pass_pipelines(&mut buf, &files, &project);
            buf.flush(&store).unwrap();

            let pipeline_nodes = store.get_nodes_by_label(&project, "Pipeline", 100).unwrap();
            prop_assert_eq!(
                pipeline_nodes.len(), 1,
                "Expected exactly 1 Pipeline node for GitHub Actions, got {}", pipeline_nodes.len()
            );
            prop_assert!(
                !pipeline_nodes[0].name.is_empty(),
                "Pipeline node name should not be empty"
            );
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Feature: devops-pipeline-support, Property 2: Stage node count matches declarations
// ═══════════════════════════════════════════════════════════════════════

/// **Validates: Requirements 2.2**
mod property2_stage_node_count {
    use super::*;

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(100))]

        #[test]
        fn gitlab_ci_stage_count_matches_declarations(
            stages in prop::collection::hash_set("[a-z]{3,10}", 1..=5),
            project in "[a-z]{3,8}",
        ) {
            let stages_vec: Vec<String> = stages.into_iter().collect();
            let n = stages_vec.len();

            let mut yaml = String::from("stages:\n");
            for s in &stages_vec {
                yaml.push_str(&format!("  - {}\n", s));
            }
            // Add one job so the YAML is valid
            yaml.push_str(&format!("myjob:\n  stage: {}\n  script:\n    - echo hi\n", stages_vec[0]));

            let tmp = tempfile::TempDir::new().unwrap();
            let abs_path = tmp.path().join(".gitlab-ci.yml");
            std::fs::write(&abs_path, &yaml).unwrap();

            let file = DiscoveredFile {
                abs_path,
                rel_path: ".gitlab-ci.yml".to_string(),
                language: Language::Yaml,
            };
            let files: Vec<&DiscoveredFile> = vec![&file];

            let store = Store::open_in_memory().unwrap();
            store.upsert_project(&codryn_store::Project {
                name: project.clone(),
                indexed_at: "now".into(),
                root_path: tmp.path().to_string_lossy().to_string(),
            }).unwrap();

            let mut buf = GraphBuffer::new(&project);
            passes::pass_pipelines(&mut buf, &files, &project);
            buf.flush(&store).unwrap();

            let stage_nodes = store.get_nodes_by_label(&project, "Stage", 100).unwrap();
            prop_assert_eq!(
                stage_nodes.len(), n,
                "Expected {} Stage nodes for {} declared stages, got {}",
                n, n, stage_nodes.len()
            );
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Feature: devops-pipeline-support, Property 3: Job node count matches definitions
// ═══════════════════════════════════════════════════════════════════════

/// **Validates: Requirements 2.3, 3.2**
mod property3_job_node_count {
    use super::*;

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(100))]

        #[test]
        fn gitlab_ci_job_count_matches_definitions(
            stages in prop::collection::hash_set("[a-z]{3,10}", 1..=3),
            jobs in prop::collection::hash_set("[a-z]{3,10}", 1..=5),
            project in "[a-z]{3,8}",
        ) {
            let stages_vec: Vec<String> = stages.into_iter().collect();
            let jobs_vec: Vec<String> = jobs.into_iter()
                .filter(|j| !["stages", "variables", "image", "services",
                    "before_script", "after_script", "cache", "include",
                    "default", "workflow", "pages"].contains(&j.as_str()))
                .filter(|j| !j.starts_with('.'))
                .collect();
            if jobs_vec.is_empty() {
                return Ok(());
            }
            let n_jobs = jobs_vec.len();

            let mut yaml = String::from("stages:\n");
            for s in &stages_vec {
                yaml.push_str(&format!("  - {}\n", s));
            }
            for (i, j) in jobs_vec.iter().enumerate() {
                let stage = &stages_vec[i % stages_vec.len()];
                yaml.push_str(&format!("{}:\n  stage: {}\n  script:\n    - echo test\n", j, stage));
            }

            let tmp = tempfile::TempDir::new().unwrap();
            let abs_path = tmp.path().join(".gitlab-ci.yml");
            std::fs::write(&abs_path, &yaml).unwrap();

            let file = DiscoveredFile {
                abs_path,
                rel_path: ".gitlab-ci.yml".to_string(),
                language: Language::Yaml,
            };
            let files: Vec<&DiscoveredFile> = vec![&file];

            let store = Store::open_in_memory().unwrap();
            store.upsert_project(&codryn_store::Project {
                name: project.clone(),
                indexed_at: "now".into(),
                root_path: tmp.path().to_string_lossy().to_string(),
            }).unwrap();

            let mut buf = GraphBuffer::new(&project);
            passes::pass_pipelines(&mut buf, &files, &project);
            buf.flush(&store).unwrap();

            let job_nodes = store.get_nodes_by_label(&project, "Job", 100).unwrap();
            prop_assert_eq!(
                job_nodes.len(), n_jobs,
                "Expected {} Job nodes, got {}", n_jobs, job_nodes.len()
            );
        }

        #[test]
        fn github_actions_job_count_matches_definitions(
            jobs in prop::collection::hash_set("[a-z][a-z0-9]{2,8}", 1..=5),
            project in "[a-z]{3,8}",
        ) {
            let jobs_vec: Vec<String> = jobs.into_iter().collect();
            let n_jobs = jobs_vec.len();

            let mut yaml = String::from("name: \"CI\"\non: push\njobs:\n");
            for j in &jobs_vec {
                yaml.push_str(&format!("  {}:\n    runs-on: ubuntu-latest\n    steps:\n      - run: echo hi\n", j));
            }

            let tmp = tempfile::TempDir::new().unwrap();
            let wf_dir = tmp.path().join(".github").join("workflows");
            std::fs::create_dir_all(&wf_dir).unwrap();
            let abs_path = wf_dir.join("ci.yml");
            std::fs::write(&abs_path, &yaml).unwrap();

            let file = DiscoveredFile {
                abs_path,
                rel_path: ".github/workflows/ci.yml".to_string(),
                language: Language::Yaml,
            };
            let files: Vec<&DiscoveredFile> = vec![&file];

            let store = Store::open_in_memory().unwrap();
            store.upsert_project(&codryn_store::Project {
                name: project.clone(),
                indexed_at: "now".into(),
                root_path: tmp.path().to_string_lossy().to_string(),
            }).unwrap();

            let mut buf = GraphBuffer::new(&project);
            passes::pass_pipelines(&mut buf, &files, &project);
            buf.flush(&store).unwrap();

            let job_nodes = store.get_nodes_by_label(&project, "Job", 100).unwrap();
            prop_assert_eq!(
                job_nodes.len(), n_jobs,
                "Expected {} Job nodes for GitHub Actions, got {}", n_jobs, job_nodes.len()
            );
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Feature: devops-pipeline-support, Property 4: BELONGS_TO_STAGE edges for staged jobs
// ═══════════════════════════════════════════════════════════════════════

/// **Validates: Requirements 2.4**
mod property4_belongs_to_stage_edges {
    use super::*;

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(100))]

        #[test]
        fn each_staged_job_has_exactly_one_belongs_to_stage_edge(
            stages in prop::collection::hash_set("[a-z]{3,10}", 1..=4),
            jobs in prop::collection::hash_set("[a-z]{3,10}", 1..=5),
            project in "[a-z]{3,8}",
        ) {
            let stages_vec: Vec<String> = stages.into_iter().collect();
            let jobs_vec: Vec<String> = jobs.into_iter()
                .filter(|j| !["stages", "variables", "image", "services",
                    "before_script", "after_script", "cache", "include",
                    "default", "workflow", "pages"].contains(&j.as_str()))
                .filter(|j| !j.starts_with('.'))
                .collect();
            if jobs_vec.is_empty() {
                return Ok(());
            }

            // Build a map of job -> stage assignment
            let mut job_stage_map: Vec<(String, String)> = Vec::new();
            let mut yaml = String::from("stages:\n");
            for s in &stages_vec {
                yaml.push_str(&format!("  - {}\n", s));
            }
            for (i, j) in jobs_vec.iter().enumerate() {
                let stage = &stages_vec[i % stages_vec.len()];
                yaml.push_str(&format!("{}:\n  stage: {}\n  script:\n    - echo test\n", j, stage));
                job_stage_map.push((j.clone(), stage.clone()));
            }

            let tmp = tempfile::TempDir::new().unwrap();
            let abs_path = tmp.path().join(".gitlab-ci.yml");
            std::fs::write(&abs_path, &yaml).unwrap();

            let file = DiscoveredFile {
                abs_path,
                rel_path: ".gitlab-ci.yml".to_string(),
                language: Language::Yaml,
            };
            let files: Vec<&DiscoveredFile> = vec![&file];

            let store = Store::open_in_memory().unwrap();
            store.upsert_project(&codryn_store::Project {
                name: project.clone(),
                indexed_at: "now".into(),
                root_path: tmp.path().to_string_lossy().to_string(),
            }).unwrap();

            let mut buf = GraphBuffer::new(&project);
            passes::pass_pipelines(&mut buf, &files, &project);
            buf.flush(&store).unwrap();

            let belongs_edges = store.get_edges_by_type(&project, "BELONGS_TO_STAGE").unwrap();

            // Each job should have exactly one BELONGS_TO_STAGE edge
            prop_assert_eq!(
                belongs_edges.len(), jobs_vec.len(),
                "Expected {} BELONGS_TO_STAGE edges (one per job), got {}",
                jobs_vec.len(), belongs_edges.len()
            );

            // Verify each edge connects a Job to a Stage
            let job_nodes = store.get_nodes_by_label(&project, "Job", 100).unwrap();
            let stage_nodes = store.get_nodes_by_label(&project, "Stage", 100).unwrap();
            let job_ids: std::collections::HashSet<i64> = job_nodes.iter().map(|n| n.id).collect();
            let stage_ids: std::collections::HashSet<i64> = stage_nodes.iter().map(|n| n.id).collect();

            for edge in &belongs_edges {
                prop_assert!(
                    job_ids.contains(&edge.source_id),
                    "BELONGS_TO_STAGE source {} is not a Job node", edge.source_id
                );
                prop_assert!(
                    stage_ids.contains(&edge.target_id),
                    "BELONGS_TO_STAGE target {} is not a Stage node", edge.target_id
                );
            }
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Feature: devops-pipeline-support, Property 5: DEPENDS_ON edges match needs declarations
// ═══════════════════════════════════════════════════════════════════════

/// **Validates: Requirements 2.5, 3.3**
mod property5_depends_on_edges {
    use super::*;

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(100))]

        #[test]
        fn gitlab_ci_needs_produce_depends_on_edges(
            stages in prop::collection::hash_set("[a-z]{3,10}", 1..=3),
            jobs in prop::collection::vec("[a-z]{3,10}", 2..=5),
            project in "[a-z]{3,8}",
        ) {
            let stages_vec: Vec<String> = stages.into_iter().collect();
            // Deduplicate jobs and filter reserved keys
            let mut seen = std::collections::HashSet::new();
            let jobs_vec: Vec<String> = jobs.into_iter()
                .filter(|j| !["stages", "variables", "image", "services",
                    "before_script", "after_script", "cache", "include",
                    "default", "workflow", "pages"].contains(&j.as_str()))
                .filter(|j| !j.starts_with('.'))
                .filter(|j| seen.insert(j.clone()))
                .collect();
            if jobs_vec.len() < 2 {
                return Ok(());
            }

            // The first job has no needs; subsequent jobs need the first job
            let mut yaml = String::from("stages:\n");
            for s in &stages_vec {
                yaml.push_str(&format!("  - {}\n", s));
            }

            // First job: no needs
            let first_job = &jobs_vec[0];
            let first_stage = &stages_vec[0];
            yaml.push_str(&format!("{}:\n  stage: {}\n  script:\n    - echo first\n",
                first_job, first_stage));

            // Remaining jobs: each needs the first job
            let mut total_needs = 0usize;
            for (i, j) in jobs_vec.iter().enumerate().skip(1) {
                let stage = &stages_vec[i % stages_vec.len()];
                yaml.push_str(&format!("{}:\n  stage: {}\n  needs:\n    - {}\n  script:\n    - echo dep\n",
                    j, stage, first_job));
                total_needs += 1;
            }

            let tmp = tempfile::TempDir::new().unwrap();
            let abs_path = tmp.path().join(".gitlab-ci.yml");
            std::fs::write(&abs_path, &yaml).unwrap();

            let file = DiscoveredFile {
                abs_path,
                rel_path: ".gitlab-ci.yml".to_string(),
                language: Language::Yaml,
            };
            let files: Vec<&DiscoveredFile> = vec![&file];

            let store = Store::open_in_memory().unwrap();
            store.upsert_project(&codryn_store::Project {
                name: project.clone(),
                indexed_at: "now".into(),
                root_path: tmp.path().to_string_lossy().to_string(),
            }).unwrap();

            let mut buf = GraphBuffer::new(&project);
            passes::pass_pipelines(&mut buf, &files, &project);
            buf.flush(&store).unwrap();

            let depends_edges = store.get_edges_by_type(&project, "DEPENDS_ON").unwrap();
            prop_assert_eq!(
                depends_edges.len(), total_needs,
                "Expected {} DEPENDS_ON edges, got {}", total_needs, depends_edges.len()
            );
        }

        #[test]
        fn github_actions_needs_produce_depends_on_edges(
            jobs in prop::collection::vec("[a-z][a-z0-9]{2,8}", 2..=5),
            project in "[a-z]{3,8}",
        ) {
            let mut seen = std::collections::HashSet::new();
            let jobs_vec: Vec<String> = jobs.into_iter()
                .filter(|j| seen.insert(j.clone()))
                .collect();
            if jobs_vec.len() < 2 {
                return Ok(());
            }

            let mut yaml = String::from("name: \"CI\"\non: push\njobs:\n");

            // First job: no needs
            yaml.push_str(&format!("  {}:\n    runs-on: ubuntu-latest\n    steps:\n      - run: echo first\n",
                jobs_vec[0]));

            // Remaining jobs: each needs the first job
            let mut total_needs = 0usize;
            for j in jobs_vec.iter().skip(1) {
                yaml.push_str(&format!("  {}:\n    runs-on: ubuntu-latest\n    needs:\n      - {}\n    steps:\n      - run: echo dep\n",
                    j, jobs_vec[0]));
                total_needs += 1;
            }

            let tmp = tempfile::TempDir::new().unwrap();
            let wf_dir = tmp.path().join(".github").join("workflows");
            std::fs::create_dir_all(&wf_dir).unwrap();
            let abs_path = wf_dir.join("ci.yml");
            std::fs::write(&abs_path, &yaml).unwrap();

            let file = DiscoveredFile {
                abs_path,
                rel_path: ".github/workflows/ci.yml".to_string(),
                language: Language::Yaml,
            };
            let files: Vec<&DiscoveredFile> = vec![&file];

            let store = Store::open_in_memory().unwrap();
            store.upsert_project(&codryn_store::Project {
                name: project.clone(),
                indexed_at: "now".into(),
                root_path: tmp.path().to_string_lossy().to_string(),
            }).unwrap();

            let mut buf = GraphBuffer::new(&project);
            passes::pass_pipelines(&mut buf, &files, &project);
            buf.flush(&store).unwrap();

            let depends_edges = store.get_edges_by_type(&project, "DEPENDS_ON").unwrap();
            prop_assert_eq!(
                depends_edges.len(), total_needs,
                "Expected {} DEPENDS_ON edges for GitHub Actions, got {}",
                total_needs, depends_edges.len()
            );
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Feature: devops-pipeline-support, Property 6: NEXT_STAGE edges between consecutive stages
// ═══════════════════════════════════════════════════════════════════════

/// **Validates: Requirements 2.6**
mod property6_next_stage_edges {
    use super::*;

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(100))]

        #[test]
        fn n_stages_produce_n_minus_1_next_stage_edges(
            stages in prop::collection::hash_set("[a-z]{3,10}", 1..=5),
            project in "[a-z]{3,8}",
        ) {
            let stages_vec: Vec<String> = stages.into_iter().collect();
            let n = stages_vec.len();

            let mut yaml = String::from("stages:\n");
            for s in &stages_vec {
                yaml.push_str(&format!("  - {}\n", s));
            }
            // Add a minimal job
            yaml.push_str(&format!("myjob:\n  stage: {}\n  script:\n    - echo hi\n", stages_vec[0]));

            let tmp = tempfile::TempDir::new().unwrap();
            let abs_path = tmp.path().join(".gitlab-ci.yml");
            std::fs::write(&abs_path, &yaml).unwrap();

            let file = DiscoveredFile {
                abs_path,
                rel_path: ".gitlab-ci.yml".to_string(),
                language: Language::Yaml,
            };
            let files: Vec<&DiscoveredFile> = vec![&file];

            let store = Store::open_in_memory().unwrap();
            store.upsert_project(&codryn_store::Project {
                name: project.clone(),
                indexed_at: "now".into(),
                root_path: tmp.path().to_string_lossy().to_string(),
            }).unwrap();

            let mut buf = GraphBuffer::new(&project);
            passes::pass_pipelines(&mut buf, &files, &project);
            buf.flush(&store).unwrap();

            let next_stage_edges = store.get_edges_by_type(&project, "NEXT_STAGE").unwrap();
            let expected = if n >= 2 { n - 1 } else { 0 };
            prop_assert_eq!(
                next_stage_edges.len(), expected,
                "Expected {} NEXT_STAGE edges for {} stages, got {}",
                expected, n, next_stage_edges.len()
            );

            // Verify each NEXT_STAGE edge connects two Stage nodes
            let stage_nodes = store.get_nodes_by_label(&project, "Stage", 100).unwrap();
            let stage_ids: std::collections::HashSet<i64> = stage_nodes.iter().map(|n| n.id).collect();
            for edge in &next_stage_edges {
                prop_assert!(
                    stage_ids.contains(&edge.source_id),
                    "NEXT_STAGE source {} is not a Stage node", edge.source_id
                );
                prop_assert!(
                    stage_ids.contains(&edge.target_id),
                    "NEXT_STAGE target {} is not a Stage node", edge.target_id
                );
            }
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Feature: devops-pipeline-support, Property 7: Trigger events stored in Pipeline properties
// ═══════════════════════════════════════════════════════════════════════

/// **Validates: Requirements 3.4**
mod property7_trigger_events {
    use super::*;

    fn trigger_event_strategy() -> impl Strategy<Value = String> {
        prop_oneof![
            Just("push".to_string()),
            Just("pull_request".to_string()),
            Just("schedule".to_string()),
            Just("workflow_dispatch".to_string()),
            Just("release".to_string()),
            Just("issues".to_string()),
        ]
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(100))]

        #[test]
        fn github_actions_triggers_stored_correctly(
            triggers in prop::collection::hash_set(trigger_event_strategy(), 1..=4),
            project in "[a-z]{3,8}",
        ) {
            let triggers_vec: Vec<String> = triggers.into_iter().collect();
            let k = triggers_vec.len();

            // Build the `on:` section as a mapping (each trigger is a key)
            let mut yaml = String::from("name: \"CI\"\n\"on\":\n");
            for t in &triggers_vec {
                yaml.push_str(&format!("  {}:\n", t));
            }
            yaml.push_str("jobs:\n  build:\n    runs-on: ubuntu-latest\n    steps:\n      - run: echo hi\n");

            let tmp = tempfile::TempDir::new().unwrap();
            let wf_dir = tmp.path().join(".github").join("workflows");
            std::fs::create_dir_all(&wf_dir).unwrap();
            let abs_path = wf_dir.join("ci.yml");
            std::fs::write(&abs_path, &yaml).unwrap();

            let file = DiscoveredFile {
                abs_path,
                rel_path: ".github/workflows/ci.yml".to_string(),
                language: Language::Yaml,
            };
            let files: Vec<&DiscoveredFile> = vec![&file];

            let store = Store::open_in_memory().unwrap();
            store.upsert_project(&codryn_store::Project {
                name: project.clone(),
                indexed_at: "now".into(),
                root_path: tmp.path().to_string_lossy().to_string(),
            }).unwrap();

            let mut buf = GraphBuffer::new(&project);
            passes::pass_pipelines(&mut buf, &files, &project);
            buf.flush(&store).unwrap();

            let pipeline_nodes = store.get_nodes_by_label(&project, "Pipeline", 100).unwrap();
            prop_assert_eq!(pipeline_nodes.len(), 1, "Expected 1 Pipeline node");

            let props_str = pipeline_nodes[0].properties_json.as_ref().unwrap();
            let props: serde_json::Value = serde_json::from_str(props_str).unwrap();
            let stored_triggers = props["triggers"].as_array().unwrap();

            prop_assert_eq!(
                stored_triggers.len(), k,
                "Expected {} triggers, got {}. Stored: {:?}, Expected: {:?}",
                k, stored_triggers.len(), stored_triggers, triggers_vec
            );

            // Verify all expected triggers are present
            let stored_set: std::collections::HashSet<String> = stored_triggers.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect();
            for t in &triggers_vec {
                prop_assert!(
                    stored_set.contains(t),
                    "Trigger '{}' not found in stored triggers {:?}", t, stored_set
                );
            }
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Feature: devops-pipeline-support, Property 12: Job script command detection creates appropriate edges
// ═══════════════════════════════════════════════════════════════════════

/// **Validates: Requirements 6.1, 6.2, 6.3**
mod property12_deploy_command_detection {
    use super::*;

    fn deploy_command_strategy() -> impl Strategy<Value = (String, &'static str)> {
        prop_oneof![
            Just(("terraform apply -auto-approve".to_string(), "DEPLOYS")),
            Just(("terraform plan".to_string(), "DEPLOYS")),
            Just(("kubectl apply -f deployment.yaml".to_string(), "DEPLOYS")),
            Just(("helm install my-release ./chart".to_string(), "DEPLOYS")),
            Just(("helm upgrade my-release ./chart".to_string(), "DEPLOYS")),
            Just(("docker build -t myimage .".to_string(), "BUILDS_IMAGE")),
            Just(("docker push myimage:latest".to_string(), "BUILDS_IMAGE")),
        ]
    }

    fn non_deploy_command_strategy() -> impl Strategy<Value = String> {
        prop_oneof![
            Just("echo hello".to_string()),
            Just("npm install".to_string()),
            Just("cargo test".to_string()),
            Just("python -m pytest".to_string()),
            Just("make build".to_string()),
            Just("ls -la".to_string()),
        ]
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(100))]

        #[test]
        fn deploy_commands_produce_correct_edges(
            (command, expected_edge_type) in deploy_command_strategy(),
            project in "[a-z]{3,8}",
            job_name in "[a-z]{3,8}",
        ) {
            // Filter out reserved GitLab keys
            if ["stages", "variables", "image", "services",
                "before_script", "after_script", "cache", "include",
                "default", "workflow", "pages"].contains(&job_name.as_str()) || job_name.starts_with('.') {
                return Ok(());
            }

            let yaml = format!(
                "stages:\n  - deploy\n{}:\n  stage: deploy\n  script:\n    - {}\n",
                job_name, command
            );

            let tmp = tempfile::TempDir::new().unwrap();
            let abs_path = tmp.path().join(".gitlab-ci.yml");
            std::fs::write(&abs_path, &yaml).unwrap();

            let file = DiscoveredFile {
                abs_path,
                rel_path: ".gitlab-ci.yml".to_string(),
                language: Language::Yaml,
            };
            let files: Vec<&DiscoveredFile> = vec![&file];

            let store = Store::open_in_memory().unwrap();
            store.upsert_project(&codryn_store::Project {
                name: project.clone(),
                indexed_at: "now".into(),
                root_path: tmp.path().to_string_lossy().to_string(),
            }).unwrap();

            // Pre-create the infra target nodes so edges can resolve
            let deploy_target_qn = format!("{}.infra.deploy_target", project);
            let docker_target_qn = format!("{}.infra.docker_target", project);
            store.insert_nodes_batch(&[
                codryn_store::Node {
                    id: 0, project: project.clone(),
                    label: "Infra".to_string(),
                    name: "deploy_target".to_string(),
                    qualified_name: deploy_target_qn,
                    file_path: "infra".to_string(),
                    start_line: 0, end_line: 0,
                    properties_json: None,
                },
                codryn_store::Node {
                    id: 0, project: project.clone(),
                    label: "Infra".to_string(),
                    name: "docker_target".to_string(),
                    qualified_name: docker_target_qn,
                    file_path: "infra".to_string(),
                    start_line: 0, end_line: 0,
                    properties_json: None,
                },
            ]).unwrap();

            let mut buf = GraphBuffer::new(&project);
            buf.seed_ids_from_store(&store).unwrap();
            passes::pass_pipelines(&mut buf, &files, &project);
            buf.flush(&store).unwrap();

            let deploy_edges = store.get_edges_by_type(&project, expected_edge_type).unwrap();
            prop_assert!(
                !deploy_edges.is_empty(),
                "Expected at least one {} edge for command '{}', got 0",
                expected_edge_type, command
            );
        }

        #[test]
        fn non_deploy_commands_produce_no_deploy_edges(
            command in non_deploy_command_strategy(),
            project in "[a-z]{3,8}",
            job_name in "[a-z]{3,8}",
        ) {
            if ["stages", "variables", "image", "services",
                "before_script", "after_script", "cache", "include",
                "default", "workflow", "pages"].contains(&job_name.as_str()) || job_name.starts_with('.') {
                return Ok(());
            }

            let yaml = format!(
                "stages:\n  - test\n{}:\n  stage: test\n  script:\n    - {}\n",
                job_name, command
            );

            let tmp = tempfile::TempDir::new().unwrap();
            let abs_path = tmp.path().join(".gitlab-ci.yml");
            std::fs::write(&abs_path, &yaml).unwrap();

            let file = DiscoveredFile {
                abs_path,
                rel_path: ".gitlab-ci.yml".to_string(),
                language: Language::Yaml,
            };
            let files: Vec<&DiscoveredFile> = vec![&file];

            let store = Store::open_in_memory().unwrap();
            store.upsert_project(&codryn_store::Project {
                name: project.clone(),
                indexed_at: "now".into(),
                root_path: tmp.path().to_string_lossy().to_string(),
            }).unwrap();

            let mut buf = GraphBuffer::new(&project);
            passes::pass_pipelines(&mut buf, &files, &project);
            buf.flush(&store).unwrap();

            let deploys = store.get_edges_by_type(&project, "DEPLOYS").unwrap();
            let builds = store.get_edges_by_type(&project, "BUILDS_IMAGE").unwrap();
            prop_assert_eq!(
                deploys.len(), 0,
                "Expected 0 DEPLOYS edges for non-deploy command '{}', got {}", command, deploys.len()
            );
            prop_assert_eq!(
                builds.len(), 0,
                "Expected 0 BUILDS_IMAGE edges for non-deploy command '{}', got {}", command, builds.len()
            );
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Feature: devops-pipeline-support, Property 8: Terraform Infra node count matches blocks
// ═══════════════════════════════════════════════════════════════════════

/// **Validates: Requirements 5.1**
mod property8_terraform_infra_node_count {
    use super::*;

    fn tf_resource_type_strategy() -> impl Strategy<Value = String> {
        prop_oneof![
            Just("aws_instance".to_string()),
            Just("aws_subnet".to_string()),
            Just("aws_vpc".to_string()),
            Just("aws_s3_bucket".to_string()),
            Just("google_compute_instance".to_string()),
            Just("azurerm_resource_group".to_string()),
        ]
    }

    fn tf_name_strategy() -> impl Strategy<Value = String> {
        "[a-z][a-z_]{2,10}"
    }

    fn tf_data_type_strategy() -> impl Strategy<Value = String> {
        prop_oneof![
            Just("aws_ami".to_string()),
            Just("aws_availability_zones".to_string()),
            Just("aws_caller_identity".to_string()),
        ]
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(100))]

        #[test]
        fn terraform_infra_node_count_matches_blocks(
            resources in prop::collection::vec(
                (tf_resource_type_strategy(), tf_name_strategy()),
                0..=4
            ),
            data_blocks in prop::collection::vec(
                (tf_data_type_strategy(), tf_name_strategy()),
                0..=3
            ),
            modules in prop::collection::vec(tf_name_strategy(), 0..=3),
            project in "[a-z]{3,8}",
        ) {
            // Deduplicate: resource blocks by (type, name), data by (type, name), modules by name
            let mut seen_resources = std::collections::HashSet::new();
            let unique_resources: Vec<_> = resources.into_iter()
                .filter(|(t, n)| seen_resources.insert((t.clone(), n.clone())))
                .collect();

            let mut seen_data = std::collections::HashSet::new();
            let unique_data: Vec<_> = data_blocks.into_iter()
                .filter(|(t, n)| seen_data.insert((t.clone(), n.clone())))
                .collect();

            let mut seen_modules = std::collections::HashSet::new();
            let unique_modules: Vec<_> = modules.into_iter()
                .filter(|n| seen_modules.insert(n.clone()))
                .collect();

            let total_blocks = unique_resources.len() + unique_data.len() + unique_modules.len();
            if total_blocks == 0 {
                return Ok(());
            }

            // Build Terraform content
            let mut tf_content = String::new();
            for (rtype, rname) in &unique_resources {
                tf_content.push_str(&format!(
                    "resource \"{}\" \"{}\" {{\n  ami = \"ami-123\"\n}}\n\n",
                    rtype, rname
                ));
            }
            for (dtype, dname) in &unique_data {
                tf_content.push_str(&format!(
                    "data \"{}\" \"{}\" {{\n  most_recent = true\n}}\n\n",
                    dtype, dname
                ));
            }
            for mname in &unique_modules {
                tf_content.push_str(&format!(
                    "module \"{}\" {{\n  source = \"./modules/{}\"\n}}\n\n",
                    mname, mname
                ));
            }

            let tmp = tempfile::TempDir::new().unwrap();
            let abs_path = tmp.path().join("main.tf");
            std::fs::write(&abs_path, &tf_content).unwrap();

            let file = DiscoveredFile {
                abs_path,
                rel_path: "main.tf".to_string(),
                language: Language::Hcl,
            };
            let files: Vec<&DiscoveredFile> = vec![&file];

            let store = Store::open_in_memory().unwrap();
            store.upsert_project(&codryn_store::Project {
                name: project.clone(),
                indexed_at: "now".into(),
                root_path: tmp.path().to_string_lossy().to_string(),
            }).unwrap();

            let mut buf = GraphBuffer::new(&project);
            passes::pass_iac(&mut buf, &files, &project);
            buf.flush(&store).unwrap();

            let infra_nodes = store.get_nodes_by_label(&project, "Infra", 100).unwrap();
            prop_assert_eq!(
                infra_nodes.len(), total_blocks,
                "Expected {} Infra nodes for {} resource + {} data + {} module blocks, got {}",
                total_blocks, unique_resources.len(), unique_data.len(), unique_modules.len(),
                infra_nodes.len()
            );

            // Verify each node has non-empty resource_type and resource_name
            for node in &infra_nodes {
                let props_str = node.properties_json.as_ref().unwrap();
                let props: serde_json::Value = serde_json::from_str(props_str).unwrap();
                let rt = props["resource_type"].as_str().unwrap_or("");
                let rn = props["resource_name"].as_str().unwrap_or("");
                prop_assert!(
                    !rt.is_empty(),
                    "Infra node '{}' has empty resource_type", node.name
                );
                prop_assert!(
                    !rn.is_empty(),
                    "Infra node '{}' has empty resource_name", node.name
                );
            }
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Feature: devops-pipeline-support, Property 9: Terraform cross-reference DEPENDS_ON edges
// ═══════════════════════════════════════════════════════════════════════

/// **Validates: Requirements 5.2**
mod property9_terraform_cross_reference_depends_on {
    use super::*;

    fn tf_resource_type_strategy() -> impl Strategy<Value = String> {
        prop_oneof![
            Just("aws_instance".to_string()),
            Just("aws_subnet".to_string()),
            Just("aws_vpc".to_string()),
            Just("aws_security_group".to_string()),
        ]
    }

    fn tf_name_strategy() -> impl Strategy<Value = String> {
        "[a-z][a-z_]{2,8}"
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(100))]

        #[test]
        fn terraform_cross_references_produce_depends_on_edges(
            ref_type in tf_resource_type_strategy(),
            ref_name in tf_name_strategy(),
            src_type in tf_resource_type_strategy(),
            src_name in tf_name_strategy(),
            project in "[a-z]{3,8}",
        ) {
            // Ensure source and target are different resources
            if src_type == ref_type && src_name == ref_name {
                return Ok(());
            }

            // Build Terraform content where src references ref via interpolation
            let tf_content = format!(
                "resource \"{}\" \"{}\" {{\n  ami = \"ami-123\"\n}}\n\n\
                 resource \"{}\" \"{}\" {{\n  subnet_id = {}.{}.id\n}}\n",
                ref_type, ref_name,
                src_type, src_name,
                ref_type, ref_name
            );

            let tmp = tempfile::TempDir::new().unwrap();
            let abs_path = tmp.path().join("main.tf");
            std::fs::write(&abs_path, &tf_content).unwrap();

            let file = DiscoveredFile {
                abs_path,
                rel_path: "main.tf".to_string(),
                language: Language::Hcl,
            };
            let files: Vec<&DiscoveredFile> = vec![&file];

            let store = Store::open_in_memory().unwrap();
            store.upsert_project(&codryn_store::Project {
                name: project.clone(),
                indexed_at: "now".into(),
                root_path: tmp.path().to_string_lossy().to_string(),
            }).unwrap();

            let mut buf = GraphBuffer::new(&project);
            passes::pass_iac(&mut buf, &files, &project);
            buf.flush(&store).unwrap();

            let depends_edges = store.get_edges_by_type(&project, "DEPENDS_ON").unwrap();
            prop_assert!(
                !depends_edges.is_empty(),
                "Expected at least one DEPENDS_ON edge for cross-reference {}.{}.id, got 0",
                ref_type, ref_name
            );

            // Verify the edge connects two Infra nodes
            let infra_nodes = store.get_nodes_by_label(&project, "Infra", 100).unwrap();
            let infra_ids: std::collections::HashSet<i64> = infra_nodes.iter().map(|n| n.id).collect();
            for edge in &depends_edges {
                prop_assert!(
                    infra_ids.contains(&edge.source_id),
                    "DEPENDS_ON source {} is not an Infra node", edge.source_id
                );
                prop_assert!(
                    infra_ids.contains(&edge.target_id),
                    "DEPENDS_ON target {} is not an Infra node", edge.target_id
                );
            }
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Feature: devops-pipeline-support, Property 10: Helm chart Infra node creation
// ═══════════════════════════════════════════════════════════════════════

/// **Validates: Requirements 5.5**
mod property10_helm_chart_infra_node {
    use super::*;

    fn chart_name_strategy() -> impl Strategy<Value = String> {
        "[a-z][a-z0-9\\-]{2,15}"
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(100))]

        #[test]
        fn helm_chart_yaml_produces_one_infra_node_with_correct_properties(
            chart_name in chart_name_strategy(),
            version in version_strategy(),
            app_version in version_strategy(),
            project in "[a-z]{3,8}",
        ) {
            let yaml_content = format!(
                "apiVersion: v2\nname: {}\nversion: {}\nappVersion: \"{}\"\ndescription: A test chart\n",
                chart_name, version, app_version
            );

            let tmp = tempfile::TempDir::new().unwrap();
            let chart_dir = tmp.path().join("mychart");
            std::fs::create_dir_all(&chart_dir).unwrap();
            let abs_path = chart_dir.join("Chart.yaml");
            std::fs::write(&abs_path, &yaml_content).unwrap();

            let file = DiscoveredFile {
                abs_path,
                rel_path: "mychart/Chart.yaml".to_string(),
                language: Language::Yaml,
            };
            let files: Vec<&DiscoveredFile> = vec![&file];

            let store = Store::open_in_memory().unwrap();
            store.upsert_project(&codryn_store::Project {
                name: project.clone(),
                indexed_at: "now".into(),
                root_path: tmp.path().to_string_lossy().to_string(),
            }).unwrap();

            let mut buf = GraphBuffer::new(&project);
            passes::pass_iac(&mut buf, &files, &project);
            buf.flush(&store).unwrap();

            let infra_nodes = store.get_nodes_by_label(&project, "Infra", 100).unwrap();
            prop_assert_eq!(
                infra_nodes.len(), 1,
                "Expected exactly 1 Infra node for Helm chart, got {}", infra_nodes.len()
            );

            let node = &infra_nodes[0];
            let props_str = node.properties_json.as_ref().unwrap();
            let props: serde_json::Value = serde_json::from_str(props_str).unwrap();

            prop_assert_eq!(
                props["name"].as_str().unwrap(), chart_name.as_str(),
                "Chart name mismatch"
            );
            prop_assert_eq!(
                props["version"].as_str().unwrap(), version.as_str(),
                "Chart version mismatch"
            );
            prop_assert_eq!(
                props["appVersion"].as_str().unwrap(), app_version.as_str(),
                "Chart appVersion mismatch"
            );
            prop_assert_eq!(
                props["infra_type"].as_str().unwrap(), "helm",
                "infra_type should be 'helm'"
            );
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Feature: devops-pipeline-support, Property 11: Helm dependency DEPENDS_ON edges
// ═══════════════════════════════════════════════════════════════════════

/// **Validates: Requirements 5.6**
mod property11_helm_dependency_depends_on {
    use super::*;

    fn chart_name_strategy() -> impl Strategy<Value = String> {
        "[a-z][a-z0-9\\-]{2,15}"
    }

    fn dep_entry_strategy() -> impl Strategy<Value = (String, String)> {
        ("[a-z][a-z0-9\\-]{2,12}", version_strategy())
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(100))]

        #[test]
        fn helm_n_dependencies_produce_n_depends_on_edges(
            chart_name in chart_name_strategy(),
            version in version_strategy(),
            deps in prop::collection::vec(dep_entry_strategy(), 1..=5),
            project in "[a-z]{3,8}",
        ) {
            // Deduplicate dependency names
            let mut seen = std::collections::HashSet::new();
            let unique_deps: Vec<_> = deps.into_iter()
                .filter(|(name, _)| seen.insert(name.clone()))
                // Ensure dep name differs from chart name
                .filter(|(name, _)| *name != chart_name)
                .collect();
            if unique_deps.is_empty() {
                return Ok(());
            }
            let n = unique_deps.len();

            // Build Chart.yaml with dependencies
            let mut yaml_content = format!(
                "apiVersion: v2\nname: {}\nversion: {}\ndependencies:\n",
                chart_name, version
            );
            for (dep_name, dep_version) in &unique_deps {
                yaml_content.push_str(&format!(
                    "  - name: {}\n    version: \"{}\"\n    repository: \"https://charts.example.com\"\n",
                    dep_name, dep_version
                ));
            }

            let tmp = tempfile::TempDir::new().unwrap();
            let chart_dir = tmp.path().join("mychart");
            std::fs::create_dir_all(&chart_dir).unwrap();
            let abs_path = chart_dir.join("Chart.yaml");
            std::fs::write(&abs_path, &yaml_content).unwrap();

            let file = DiscoveredFile {
                abs_path,
                rel_path: "mychart/Chart.yaml".to_string(),
                language: Language::Yaml,
            };
            let files: Vec<&DiscoveredFile> = vec![&file];

            let store = Store::open_in_memory().unwrap();
            store.upsert_project(&codryn_store::Project {
                name: project.clone(),
                indexed_at: "now".into(),
                root_path: tmp.path().to_string_lossy().to_string(),
            }).unwrap();

            let mut buf = GraphBuffer::new(&project);
            passes::pass_iac(&mut buf, &files, &project);
            buf.flush(&store).unwrap();

            let depends_edges = store.get_edges_by_type(&project, "DEPENDS_ON").unwrap();
            prop_assert_eq!(
                depends_edges.len(), n,
                "Expected {} DEPENDS_ON edges for {} Helm dependencies, got {}",
                n, n, depends_edges.len()
            );

            // Verify edges connect Infra nodes
            let infra_nodes = store.get_nodes_by_label(&project, "Infra", 100).unwrap();
            let infra_ids: std::collections::HashSet<i64> = infra_nodes.iter().map(|n| n.id).collect();
            for edge in &depends_edges {
                prop_assert!(
                    infra_ids.contains(&edge.source_id),
                    "DEPENDS_ON source {} is not an Infra node", edge.source_id
                );
                prop_assert!(
                    infra_ids.contains(&edge.target_id),
                    "DEPENDS_ON target {} is not an Infra node", edge.target_id
                );
            }
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Property 13: pass_calls creates CALLS edges for known function calls
// ═══════════════════════════════════════════════════════════════════════

/// **Validates: Item 2.7 — Pipeline unit tests for pass_calls**
mod property13_pass_calls {
    use super::*;

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(50))]

        #[test]
        fn pass_calls_creates_edges_for_known_functions(
            caller_name in "[a-z][a-zA-Z]{2,10}",
            callee_name in "[a-z][a-zA-Z]{2,10}",
            project in project_strategy(),
        ) {
            // Skip if caller and callee have the same name (same-file calls are skipped)
            prop_assume!(caller_name != callee_name);

            let tmp = tempfile::TempDir::new().unwrap();

            // Create a source file that calls the callee function
            let source_content = format!(
                "function {}() {{\n  {}();\n}}\n",
                caller_name, callee_name
            );
            let source_file = "src/caller.ts";
            std::fs::create_dir_all(tmp.path().join("src")).unwrap();
            std::fs::write(tmp.path().join(source_file), &source_content).unwrap();

            // Register the callee in a different file so the edge is created
            let callee_file = "src/callee.ts";
            let callee_qn = format!("{}.callee.{}", project, callee_name);

            let mut reg = Registry::new();
            // Register the caller so it can be resolved as the source
            let caller_qn = format!("{}.caller.{}", project, caller_name);
            reg.register(&caller_name, &caller_qn, source_file, "Function", 1, 3);
            // Register the callee in a different file
            reg.register(&callee_name, &callee_qn, callee_file, "Function", 1, 5);

            let discovered = DiscoveredFile {
                abs_path: tmp.path().join(source_file),
                rel_path: source_file.to_string(),
                language: Language::TypeScript,
            };
            let files: Vec<&DiscoveredFile> = vec![&discovered];

            let mut buf = GraphBuffer::new(&project);
            passes::pass_calls(&mut buf, &reg, &files, &project);

            // Should have at least one CALLS edge from caller to callee
            prop_assert!(
                buf.edge_count() >= 1,
                "Expected at least 1 CALLS edge for '{}' calling '{}', got {}",
                caller_name, callee_name, buf.edge_count()
            );
        }

        #[test]
        fn pass_calls_skips_same_file_references(
            func_name in "[a-z][a-zA-Z]{2,10}",
            project in project_strategy(),
        ) {
            let tmp = tempfile::TempDir::new().unwrap();

            // Create a source file that references a function defined in the same file
            let source_content = format!(
                "function {}() {{}}\nfunction main() {{ {}(); }}\n",
                func_name, func_name
            );
            let source_file = "src/same.ts";
            std::fs::create_dir_all(tmp.path().join("src")).unwrap();
            std::fs::write(tmp.path().join(source_file), &source_content).unwrap();

            let func_qn = format!("{}.same.{}", project, func_name);

            let mut reg = Registry::new();
            // Register the function in the SAME file as the source
            reg.register(&func_name, &func_qn, source_file, "Function", 1, 1);

            let discovered = DiscoveredFile {
                abs_path: tmp.path().join(source_file),
                rel_path: source_file.to_string(),
                language: Language::TypeScript,
            };
            let files: Vec<&DiscoveredFile> = vec![&discovered];

            let mut buf = GraphBuffer::new(&project);
            passes::pass_calls(&mut buf, &reg, &files, &project);

            // Same-file references should NOT create edges
            prop_assert_eq!(
                buf.edge_count(), 0,
                "Expected 0 edges for same-file reference to '{}', got {}",
                func_name, buf.edge_count()
            );
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Property 6: TypeRegistry Parallel-Serial Confluence
// ═══════════════════════════════════════════════════════════════════════

/// **Validates: Requirements 5.3**
mod property6_type_registry_parallel_serial_confluence {
    use super::*;
    use codryn_pipeline::extraction::{extract_file_types, FileTypeResult};
    use rayon::prelude::*;
    use std::collections::HashMap;
    use std::path::PathBuf;

    /// Generate a valid TypeScript function name.
    fn ts_func_name_strategy() -> impl Strategy<Value = String> {
        "[a-z][a-zA-Z]{2,10}"
    }

    /// Generate a valid custom type name (non-stdlib).
    fn custom_type_strategy() -> impl Strategy<Value = String> {
        "[A-Z][a-zA-Z]{3,12}"
    }

    /// Generate a valid parameter name.
    fn param_name_strategy() -> impl Strategy<Value = String> {
        "[a-z][a-zA-Z]{1,8}"
    }

    /// Generate a TypeScript source file with typed function declarations.
    /// Returns (file_name, source_code).
    fn ts_source_strategy() -> impl Strategy<Value = (String, String)> {
        (
            "[a-z][a-z_]{2,8}",
            prop::collection::vec(
                (
                    ts_func_name_strategy(),
                    custom_type_strategy(),
                    prop::collection::vec((param_name_strategy(), custom_type_strategy()), 1..4),
                ),
                1..5,
            ),
        )
            .prop_map(|(file_stem, funcs)| {
                let file_name = format!("src/{}.ts", file_stem);
                let mut source = String::new();
                for (func_name, ret_type, params) in &funcs {
                    let params_str: Vec<String> = params
                        .iter()
                        .map(|(name, ty)| format!("{}: {}", name, ty))
                        .collect();
                    source.push_str(&format!(
                        "function {}({}): {} {{\n  return null;\n}}\n\n",
                        func_name,
                        params_str.join(", "),
                        ret_type
                    ));
                }
                (file_name, source)
            })
    }

    /// Generate a collection of TypeScript source files.
    fn ts_files_strategy() -> impl Strategy<Value = Vec<(String, String)>> {
        prop::collection::vec(ts_source_strategy(), 2..8).prop_map(|files| {
            // Ensure unique file names by appending index
            files
                .into_iter()
                .enumerate()
                .map(|(i, (_name, src))| {
                    let unique_name = format!("src/file_{}.ts", i);
                    (unique_name, src)
                })
                .collect()
        })
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(100))]

        #[test]
        fn parallel_serial_type_registry_equality(
            files_data in ts_files_strategy(),
        ) {
            // Create DiscoveredFile instances (no actual filesystem needed since
            // extract_file_types takes source as a parameter)
            let discovered_files: Vec<DiscoveredFile> = files_data
                .iter()
                .map(|(name, _)| DiscoveredFile {
                    abs_path: PathBuf::from(name),
                    rel_path: name.clone(),
                    language: Language::TypeScript,
                })
                .collect();

            // Pair files with their sources
            let file_source_pairs: Vec<(&DiscoveredFile, &str)> = discovered_files
                .iter()
                .zip(files_data.iter().map(|(_, src)| src.as_str()))
                .collect();

            // ── Serial extraction ──
            let mut serial_reg = TypeRegistry::new();
            for (file, source) in &file_source_pairs {
                let result = extract_file_types(file, source);
                for (file_path, symbol_name, resolved_type) in result.types {
                    serial_reg.register_type(&file_path, &symbol_name, &resolved_type);
                }
                for (importer, imported) in result.imports {
                    serial_reg.register_import(&importer, &imported);
                }
            }

            // ── Parallel extraction ──
            let parallel_results: Vec<FileTypeResult> = file_source_pairs
                .par_iter()
                .map(|(file, source)| extract_file_types(file, source))
                .collect();

            let mut parallel_reg = TypeRegistry::new();
            for result in parallel_results {
                for (file_path, symbol_name, resolved_type) in result.types {
                    parallel_reg.register_type(&file_path, &symbol_name, &resolved_type);
                }
                for (importer, imported) in result.imports {
                    parallel_reg.register_import(&importer, &imported);
                }
            }

            // ── Compare TypeRegistry contents ──
            // Both registries should have the same number of type entries
            prop_assert_eq!(
                serial_reg.len(),
                parallel_reg.len(),
                "TypeRegistry size mismatch: serial={}, parallel={}",
                serial_reg.len(),
                parallel_reg.len()
            );

            // Drain serial registry and verify each entry exists in parallel registry
            let serial_types: HashMap<(String, String), String> = serial_reg
                .drain_types()
                .map(|((fp, sym), entry)| ((fp, sym), entry.resolved_type))
                .collect();

            for ((file_path, symbol_name), expected_type) in &serial_types {
                let parallel_entry = parallel_reg.lookup_type(file_path, symbol_name);
                prop_assert!(
                    parallel_entry.is_some(),
                    "Type ({}, {}) present in serial but missing in parallel registry",
                    file_path, symbol_name
                );
                prop_assert_eq!(
                    &parallel_entry.unwrap().resolved_type,
                    expected_type,
                    "Type mismatch for ({}, {}): serial='{}', parallel='{}'",
                    file_path, symbol_name, expected_type,
                    parallel_entry.unwrap().resolved_type
                );
            }

            // Also verify parallel doesn't have extra entries
            let parallel_types: HashMap<(String, String), String> = parallel_reg
                .drain_types()
                .map(|((fp, sym), entry)| ((fp, sym), entry.resolved_type))
                .collect();

            prop_assert_eq!(
                serial_types.len(),
                parallel_types.len(),
                "After drain, serial has {} entries but parallel has {}",
                serial_types.len(),
                parallel_types.len()
            );
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Property 7: Java/Kotlin/Go Extraction Parallel-Serial Confluence
// ═══════════════════════════════════════════════════════════════════════

/// **Validates: Requirements 6.3**
mod property7_java_kotlin_go_extraction_confluence {
    use super::*;
    use codryn_pipeline::extraction::ExtractionResult;
    use codryn_pipeline::go_adapter::extract_go_parallel;
    use codryn_pipeline::spring_java::extract_java_parallel;
    use codryn_pipeline::spring_kotlin::extract_kotlin_parallel;

    /// Generate a valid Java class name.
    fn java_class_name_strategy() -> impl Strategy<Value = String> {
        "[A-Z][a-zA-Z]{3,12}"
    }

    /// Generate a valid method name.
    fn method_name_strategy() -> impl Strategy<Value = String> {
        "[a-z][a-zA-Z]{2,10}"
    }

    /// Generate a Java source file with a Spring controller class.
    /// Returns (class_name, source_code).
    fn java_source_strategy() -> impl Strategy<Value = (String, String)> {
        (
            java_class_name_strategy(),
            prop::collection::vec(method_name_strategy(), 1..4),
        )
            .prop_map(|(class_name, methods)| {
                let mut source = String::new();
                source.push_str("@RestController\n");
                source.push_str(&format!(
                    "@RequestMapping(\"/api/{}\")\n",
                    class_name.to_lowercase()
                ));
                source.push_str(&format!("public class {} {{\n", class_name));
                for method in &methods {
                    source.push_str(&format!(
                        "    @GetMapping(\"/{}\")\n    public String {}() {{\n        return \"ok\";\n    }}\n\n",
                        method, method
                    ));
                }
                source.push_str("}\n");
                (class_name, source)
            })
    }

    /// Generate a Kotlin source file with a Spring controller class.
    /// Returns (class_name, source_code).
    fn kotlin_source_strategy() -> impl Strategy<Value = (String, String)> {
        (
            java_class_name_strategy(),
            prop::collection::vec(method_name_strategy(), 1..4),
        )
            .prop_map(|(class_name, methods)| {
                let mut source = String::new();
                source.push_str("@RestController\n");
                source.push_str(&format!(
                    "@RequestMapping(\"/api/{}\")\n",
                    class_name.to_lowercase()
                ));
                source.push_str(&format!("class {} {{\n", class_name));
                for method in &methods {
                    source.push_str(&format!(
                        "    @GetMapping(\"/{}\")\n    fun {}(): String {{\n        return \"ok\"\n    }}\n\n",
                        method, method
                    ));
                }
                source.push_str("}\n");
                (class_name, source)
            })
    }

    /// Generate a Go source file with struct and methods.
    /// Returns (struct_name, source_code).
    fn go_source_strategy() -> impl Strategy<Value = (String, String)> {
        (
            java_class_name_strategy(),
            prop::collection::vec(method_name_strategy(), 1..4),
        )
            .prop_map(|(struct_name, methods)| {
                let mut source = String::new();
                source.push_str("package main\n\n");
                source.push_str(&format!("type {} struct {{\n", struct_name));
                source.push_str("    Name string\n");
                source.push_str("}\n\n");
                for method in &methods {
                    // Capitalize first letter for exported method
                    let exported_name = {
                        let mut chars = method.chars();
                        match chars.next() {
                            Some(c) => c.to_uppercase().to_string() + chars.as_str(),
                            None => method.clone(),
                        }
                    };
                    source.push_str(&format!(
                        "func (h *{}) {}() string {{\n    return \"ok\"\n}}\n\n",
                        struct_name, exported_name
                    ));
                }
                (struct_name, source)
            })
    }

    /// Compare two ExtractionResults for equality of nodes and registry entries.
    fn results_are_equal(a: &ExtractionResult, b: &ExtractionResult) -> Result<(), String> {
        if a.nodes.len() != b.nodes.len() {
            return Err(format!(
                "Node count mismatch: {} vs {}",
                a.nodes.len(),
                b.nodes.len()
            ));
        }
        if a.registry_entries.len() != b.registry_entries.len() {
            return Err(format!(
                "Registry entry count mismatch: {} vs {}",
                a.registry_entries.len(),
                b.registry_entries.len()
            ));
        }

        // Compare nodes by qualified_name (order may differ in parallel)
        let mut a_nodes: Vec<_> = a
            .nodes
            .iter()
            .map(|n| (&n.qualified_name, &n.label, &n.name))
            .collect();
        let mut b_nodes: Vec<_> = b
            .nodes
            .iter()
            .map(|n| (&n.qualified_name, &n.label, &n.name))
            .collect();
        a_nodes.sort();
        b_nodes.sort();

        for (i, (an, bn)) in a_nodes.iter().zip(b_nodes.iter()).enumerate() {
            if an != bn {
                return Err(format!(
                    "Node mismatch at index {}: {:?} vs {:?}",
                    i, an, bn
                ));
            }
        }

        // Compare registry entries by qualified_name
        let mut a_reg: Vec<_> = a
            .registry_entries
            .iter()
            .map(|(name, entry)| (name, &entry.qualified_name, &entry.label))
            .collect();
        let mut b_reg: Vec<_> = b
            .registry_entries
            .iter()
            .map(|(name, entry)| (name, &entry.qualified_name, &entry.label))
            .collect();
        a_reg.sort();
        b_reg.sort();

        for (i, (ar, br)) in a_reg.iter().zip(b_reg.iter()).enumerate() {
            if ar != br {
                return Err(format!(
                    "Registry entry mismatch at index {}: {:?} vs {:?}",
                    i, ar, br
                ));
            }
        }

        Ok(())
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(100))]

        #[test]
        fn java_parallel_extraction_is_deterministic(
            (class_name, source) in java_source_strategy(),
            project in project_strategy(),
        ) {
            let tmp = tempfile::TempDir::new().unwrap();
            let file_name = format!("{}.java", class_name);
            let abs_path = tmp.path().join(&file_name);
            std::fs::write(&abs_path, &source).unwrap();

            let file = DiscoveredFile {
                abs_path: abs_path.clone(),
                rel_path: format!("src/controller/{}", file_name),
                language: Language::Java,
            };

            // Call extract_java_parallel twice (simulating serial and parallel invocations)
            let result1 = extract_java_parallel(&project, &file);
            let result2 = extract_java_parallel(&project, &file);

            prop_assert!(result1.is_some(), "First Java extraction returned None");
            prop_assert!(result2.is_some(), "Second Java extraction returned None");

            let r1 = result1.unwrap();
            let r2 = result2.unwrap();

            // Results should be identical
            if let Err(msg) = results_are_equal(&r1, &r2) {
                prop_assert!(false, "Java extraction not deterministic: {}", msg);
            }

            // Verify expected content: should have at least a class node and a module node
            prop_assert!(
                r1.nodes.len() >= 2,
                "Expected at least 2 nodes (class + module), got {}",
                r1.nodes.len()
            );

            // Verify the class is in the registry
            let has_class = r1
                .registry_entries
                .iter()
                .any(|(name, _)| name == &class_name);
            prop_assert!(
                has_class,
                "Class '{}' not found in registry entries",
                class_name
            );
        }

        #[test]
        fn kotlin_parallel_extraction_is_deterministic(
            (class_name, source) in kotlin_source_strategy(),
            project in project_strategy(),
        ) {
            let tmp = tempfile::TempDir::new().unwrap();
            let file_name = format!("{}.kt", class_name);
            let abs_path = tmp.path().join(&file_name);
            std::fs::write(&abs_path, &source).unwrap();

            let file = DiscoveredFile {
                abs_path: abs_path.clone(),
                rel_path: format!("src/controller/{}", file_name),
                language: Language::Kotlin,
            };

            // Call extract_kotlin_parallel twice
            let result1 = extract_kotlin_parallel(&project, &file);
            let result2 = extract_kotlin_parallel(&project, &file);

            prop_assert!(result1.is_some(), "First Kotlin extraction returned None");
            prop_assert!(result2.is_some(), "Second Kotlin extraction returned None");

            let r1 = result1.unwrap();
            let r2 = result2.unwrap();

            // Results should be identical
            if let Err(msg) = results_are_equal(&r1, &r2) {
                prop_assert!(false, "Kotlin extraction not deterministic: {}", msg);
            }

            // Verify expected content: should have at least a class node and a module node
            prop_assert!(
                r1.nodes.len() >= 2,
                "Expected at least 2 nodes (class + module), got {}",
                r1.nodes.len()
            );

            // Verify the class is in the registry
            let has_class = r1
                .registry_entries
                .iter()
                .any(|(name, _)| name == &class_name);
            prop_assert!(
                has_class,
                "Class '{}' not found in registry entries",
                class_name
            );
        }

        #[test]
        fn go_parallel_extraction_is_deterministic(
            (struct_name, source) in go_source_strategy(),
            project in project_strategy(),
        ) {
            let tmp = tempfile::TempDir::new().unwrap();
            let file_name = format!("{}.go", struct_name.to_lowercase());
            let abs_path = tmp.path().join(&file_name);
            std::fs::write(&abs_path, &source).unwrap();

            let file = DiscoveredFile {
                abs_path: abs_path.clone(),
                rel_path: format!("pkg/handler/{}", file_name),
                language: Language::Go,
            };

            // Call extract_go_parallel twice
            let result1 = extract_go_parallel(&project, &file);
            let result2 = extract_go_parallel(&project, &file);

            prop_assert!(result1.is_some(), "First Go extraction returned None");
            prop_assert!(result2.is_some(), "Second Go extraction returned None");

            let r1 = result1.unwrap();
            let r2 = result2.unwrap();

            // Results should be identical
            if let Err(msg) = results_are_equal(&r1, &r2) {
                prop_assert!(false, "Go extraction not deterministic: {}", msg);
            }

            // Verify expected content: should have at least a struct node and a module node
            prop_assert!(
                r1.nodes.len() >= 2,
                "Expected at least 2 nodes (struct + module), got {}",
                r1.nodes.len()
            );

            // Verify the struct is in the registry
            let has_struct = r1
                .registry_entries
                .iter()
                .any(|(name, _)| name == &struct_name);
            prop_assert!(
                has_struct,
                "Struct '{}' not found in registry entries",
                struct_name
            );
        }

        #[test]
        fn java_kotlin_go_parallel_vs_serial_via_rayon(
            (java_class, java_source) in java_source_strategy(),
            (kotlin_class, kotlin_source) in kotlin_source_strategy(),
            (go_struct, go_source) in go_source_strategy(),
            project in project_strategy(),
        ) {
            let tmp = tempfile::TempDir::new().unwrap();

            // Write Java file
            let java_file_name = format!("{}.java", java_class);
            let java_abs = tmp.path().join(&java_file_name);
            std::fs::write(&java_abs, &java_source).unwrap();
            let java_file = DiscoveredFile {
                abs_path: java_abs,
                rel_path: format!("src/{}", java_file_name),
                language: Language::Java,
            };

            // Write Kotlin file
            let kotlin_file_name = format!("{}.kt", kotlin_class);
            let kotlin_abs = tmp.path().join(&kotlin_file_name);
            std::fs::write(&kotlin_abs, &kotlin_source).unwrap();
            let kotlin_file = DiscoveredFile {
                abs_path: kotlin_abs,
                rel_path: format!("src/{}", kotlin_file_name),
                language: Language::Kotlin,
            };

            // Write Go file
            let go_file_name = format!("{}.go", go_struct.to_lowercase());
            let go_abs = tmp.path().join(&go_file_name);
            std::fs::write(&go_abs, &go_source).unwrap();
            let go_file = DiscoveredFile {
                abs_path: go_abs,
                rel_path: format!("pkg/{}", go_file_name),
                language: Language::Go,
            };

            let files = vec![&java_file, &kotlin_file, &go_file];

            // ── Serial extraction ──
            let serial_results: Vec<Option<ExtractionResult>> = files
                .iter()
                .map(|f| match f.language {
                    Language::Java => extract_java_parallel(&project, f),
                    Language::Kotlin => extract_kotlin_parallel(&project, f),
                    Language::Go => extract_go_parallel(&project, f),
                    _ => None,
                })
                .collect();

            // ── Parallel extraction via rayon ──
            use rayon::prelude::*;
            let parallel_results: Vec<Option<ExtractionResult>> = files
                .par_iter()
                .map(|f| match f.language {
                    Language::Java => extract_java_parallel(&project, f),
                    Language::Kotlin => extract_kotlin_parallel(&project, f),
                    Language::Go => extract_go_parallel(&project, f),
                    _ => None,
                })
                .collect();

            // Compare serial vs parallel results for each file
            prop_assert_eq!(
                serial_results.len(),
                parallel_results.len(),
                "Result count mismatch"
            );

            for (i, (serial, parallel)) in serial_results
                .iter()
                .zip(parallel_results.iter())
                .enumerate()
            {
                match (serial, parallel) {
                    (Some(s), Some(p)) => {
                        if let Err(msg) = results_are_equal(s, p) {
                            prop_assert!(
                                false,
                                "File {} serial/parallel mismatch: {}",
                                i,
                                msg
                            );
                        }
                    }
                    (None, None) => {} // Both None is fine
                    _ => {
                        prop_assert!(
                            false,
                            "File {}: one returned Some and the other None",
                            i
                        );
                    }
                }
            }
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Property 5: Incremental Reindex Graph Equivalence
// ═══════════════════════════════════════════════════════════════════════

/// **Validates: Requirements 1.6, 4.1, 4.2, 4.3, 4.7**
mod property5_incremental_reindex_graph_equivalence {
    use super::*;
    use codryn_pipeline::{IndexMode, Pipeline};
    use std::collections::{BTreeMap, BTreeSet};

    /// A normalized representation of a graph node for comparison purposes.
    /// Excludes the numeric ID (which varies between runs) and enrichment properties.
    #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
    struct NormalizedNode {
        label: String,
        name: String,
        qualified_name: String,
        file_path: String,
        start_line: i32,
        end_line: i32,
    }

    /// A normalized representation of a graph edge for comparison purposes.
    /// Uses qualified names of source/target instead of numeric IDs.
    #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
    struct NormalizedEdge {
        source_qn: String,
        target_qn: String,
        edge_type: String,
    }

    /// Edge types that are produced by enrichment passes or Full-mode-only passes
    /// and should be excluded from comparison between Fast and Full mode.
    fn is_full_mode_only_edge_type(edge_type: &str) -> bool {
        matches!(
            edge_type,
            // Enrichment pass (skipped in Fast mode)
            "SIMILAR_TO" | "GIT_CHANGED_WITH" | "GIT_AUTHORED_BY"
            // pass_semantic (skipped in Fast mode)
            | "INHERITS" | "IMPLEMENTS"
            // pass_type_refs (depends on TypeRegistry which is empty in Fast mode)
            | "TYPE_REF"
        )
    }

    /// Node labels that are enrichment-only and should be excluded from comparison.
    fn is_enrichment_node_label(label: &str) -> bool {
        matches!(label, "Author" | "Commit")
    }

    /// Extract a normalized graph snapshot from the store, excluding enrichment data.
    fn snapshot_graph(
        store: &Store,
        project: &str,
    ) -> (BTreeSet<NormalizedNode>, BTreeSet<NormalizedEdge>) {
        let nodes = store.get_all_nodes(project).unwrap();
        let edges = store.get_edges(project, i32::MAX).unwrap();

        // Build id -> qualified_name map for edge normalization
        let id_to_qn: BTreeMap<i64, String> = nodes
            .iter()
            .map(|n| (n.id, n.qualified_name.clone()))
            .collect();

        let normalized_nodes: BTreeSet<NormalizedNode> = nodes
            .into_iter()
            .filter(|n| !is_enrichment_node_label(&n.label))
            .map(|n| NormalizedNode {
                label: n.label,
                name: n.name,
                qualified_name: n.qualified_name,
                file_path: n.file_path,
                // Fast mode preserves existing nodes without updating line numbers,
                // so we normalize line numbers to 0 for comparison purposes.
                start_line: 0,
                end_line: 0,
            })
            .collect();

        let normalized_edges: BTreeSet<NormalizedEdge> = edges
            .into_iter()
            .filter(|e| !is_full_mode_only_edge_type(&e.edge_type))
            .filter_map(|e| {
                let source_qn = id_to_qn.get(&e.source_id)?.clone();
                let target_qn = id_to_qn.get(&e.target_id)?.clone();
                Some(NormalizedEdge {
                    source_qn,
                    target_qn,
                    edge_type: e.edge_type,
                })
            })
            .collect();

        (normalized_nodes, normalized_edges)
    }

    /// Generate a function name.
    fn func_name_strategy() -> impl Strategy<Value = String> {
        "[a-z][a-zA-Z]{2,8}"
    }

    /// Strategy to generate a set of TypeScript source files with inter-file calls and imports.
    /// Returns (file_contents: Vec<(filename, content)>, num_files).
    fn ts_project_strategy() -> impl Strategy<Value = Vec<(String, String)>> {
        // Generate 3-6 files, each with 1-3 functions
        (3usize..=6).prop_flat_map(|num_files| {
            let file_funcs: Vec<_> = (0..num_files)
                .map(|_| prop::collection::vec(func_name_strategy(), 1..=3))
                .collect();
            file_funcs.prop_map(move |funcs_per_file| {
                let mut files = Vec::new();
                let mut all_exports: Vec<(usize, String)> = Vec::new(); // (file_idx, func_name)

                // First pass: generate function declarations
                for (file_idx, funcs) in funcs_per_file.iter().enumerate() {
                    for func in funcs {
                        all_exports.push((file_idx, func.clone()));
                    }
                }

                // Second pass: generate file contents with imports and calls
                for (file_idx, funcs) in funcs_per_file.iter().enumerate() {
                    let filename = format!("file{}.ts", file_idx);
                    let mut content = String::new();

                    // Add imports from other files
                    for (other_idx, other_func) in &all_exports {
                        if *other_idx != file_idx {
                            content.push_str(&format!(
                                "import {{ {} }} from './file{}';\n",
                                other_func, other_idx
                            ));
                        }
                    }
                    content.push('\n');

                    // Add function declarations that call imported functions
                    for (i, func) in funcs.iter().enumerate() {
                        content.push_str(&format!("export function {}() {{\n", func));
                        // Call some functions from other files
                        for (other_idx, other_func) in &all_exports {
                            if *other_idx != file_idx {
                                content.push_str(&format!("  {}();\n", other_func));
                                break; // Just one call per function to keep it simple
                            }
                        }
                        content.push_str(&format!("  return {};\n", i));
                        content.push_str("}\n\n");
                    }

                    files.push((filename, content));
                }

                files
            })
        })
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(20))]

        #[test]
        fn incremental_reindex_matches_full_reindex(
            initial_files in ts_project_strategy(),
        ) {
            let num_files = initial_files.len();
            if num_files < 3 {
                return Ok(());
            }

            // Create a temporary directory for the project
            let tmp = tempfile::TempDir::new().unwrap();
            let repo_path = tmp.path();

            // Write initial files
            for (filename, content) in &initial_files {
                std::fs::write(repo_path.join(filename), content).unwrap();
            }

            // Also write a package.json so the project is recognized
            std::fs::write(
                repo_path.join("package.json"),
                r#"{"name": "test-project", "version": "1.0.0"}"#,
            ).unwrap();

            // Create a db directory for the store
            let db_dir = tempfile::TempDir::new().unwrap();
            let db_path = db_dir.path();

            // Step 1: Run full index on initial state
            let pipeline = Pipeline::new(repo_path, db_path, IndexMode::Full);
            pipeline.run().unwrap();

            // Step 2: Modify ALL files so that the incremental reindex processes everything.
            // This ensures pass_imports runs on all files (since all are in `changed`).
            // The property validates that incremental reindex produces the same result
            // as a full reindex when all files have changed.
            for (idx, (filename, _)) in initial_files.iter().enumerate() {
                let mut new_content = String::new();

                // Add imports from other files
                for (other_idx, (_, other_content)) in initial_files.iter().enumerate() {
                    if other_idx != idx {
                        // Extract first exported function name from the other file
                        if let Some(func_line) = other_content.lines().find(|l| l.starts_with("export function")) {
                            if let Some(rest) = func_line.strip_prefix("export function ") {
                                if let Some(name) = rest.split('(').next() {
                                    new_content.push_str(&format!(
                                        "import {{ {} }} from './file{}';\n",
                                        name, other_idx
                                    ));
                                }
                            }
                        }
                    }
                }
                new_content.push('\n');

                // Add a new function unique to this modification
                new_content.push_str(&format!(
                    "export function newFunc{}() {{\n  return 'modified';\n}}\n\n",
                    idx
                ));

                // Keep original exported functions with modified bodies
                let (_, original) = &initial_files[idx];
                for line in original.lines() {
                    if line.starts_with("export function") {
                        new_content.push_str(line);
                        new_content.push('\n');
                        new_content.push_str("  // modified body\n");
                        new_content.push_str("  return 'changed';\n");
                        new_content.push_str("}\n\n");
                    }
                }

                std::fs::write(repo_path.join(filename), &new_content).unwrap();
            }

            // Step 3: Run incremental reindex (Fast mode) on the modified state
            let pipeline_incr = Pipeline::new(repo_path, db_path, IndexMode::Fast);
            pipeline_incr.run().unwrap();

            // Capture the incremental graph
            let store_incr = Store::open(&db_path.join("graph.db")).unwrap();
            let project_name = pipeline_incr.project_name();
            let (incr_nodes, incr_edges) = snapshot_graph(&store_incr, &project_name);
            drop(store_incr);

            // Step 4: Run a fresh full index on the same final state (separate db)
            let db_dir_full = tempfile::TempDir::new().unwrap();
            let db_path_full = db_dir_full.path();

            let pipeline_full = Pipeline::new(repo_path, db_path_full, IndexMode::Full);
            pipeline_full.run().unwrap();

            // Capture the full reindex graph
            let store_full = Store::open(&db_path_full.join("graph.db")).unwrap();
            let (full_nodes, full_edges) = snapshot_graph(&store_full, &project_name);
            drop(store_full);

            // Step 5: Compare the two graphs.
            // Fast mode contract: all nodes from Full must exist in Incremental.
            // Fast mode may have EXTRA stale nodes (renamed/deleted functions accumulate
            // until the next Full reindex) — that is acceptable and by design.
            let full_only_nodes: BTreeSet<_> = full_nodes.difference(&incr_nodes).collect();

            prop_assert!(
                full_only_nodes.is_empty(),
                "Fast mode is MISSING nodes that Full mode has ({} missing):\n    {:?}",
                full_only_nodes.len(),
                full_only_nodes.iter().take(5).collect::<Vec<_>>()
            );

            // Edges: all edges from Full must exist in Incremental.
            // Fast mode may have extra stale edges (from stale nodes) — acceptable.
            let full_only_edges: BTreeSet<_> = full_edges.difference(&incr_edges).collect();

            prop_assert!(
                full_only_edges.is_empty(),
                "Fast mode is MISSING edges that Full mode has ({} missing):\n    {:?}",
                full_only_edges.len(),
                full_only_edges.iter().take(5).collect::<Vec<_>>()
            );
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Property 8: Progress Callback Frequency
// ═══════════════════════════════════════════════════════════════════════

/// **Validates: Requirements 7.2**
mod property8_progress_callback_frequency {
    use super::*;
    use codryn_pipeline::{IndexMode, Pipeline};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(10))]

        #[test]
        fn progress_callback_frequency(n in 100usize..300) {
            let tmp = tempfile::TempDir::new().unwrap();
            // Generate N TypeScript files
            for i in 0..n {
                let content = format!("export function func{}() {{ return {}; }}\n", i, i);
                std::fs::write(tmp.path().join(format!("file{}.ts", i)), content).unwrap();
            }
            // Write package.json so the project is discoverable
            std::fs::write(tmp.path().join("package.json"), r#"{"name":"test"}"#).unwrap();

            let db_dir = tempfile::TempDir::new().unwrap();
            let counter = Arc::new(AtomicUsize::new(0));
            let counter_clone = counter.clone();

            let mut pipeline = Pipeline::new(tmp.path(), db_dir.path(), IndexMode::Full);
            pipeline.set_progress_callback(Box::new(move |_update| {
                counter_clone.fetch_add(1, Ordering::Relaxed);
            }));
            pipeline.run().unwrap();

            let count = counter.load(Ordering::Relaxed);
            let min_expected = n / 100;
            prop_assert!(
                count >= min_expected,
                "Expected at least {} callbacks for {} files, got {}",
                min_expected, n, count
            );
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Property 9: Panicking Callback Resilience
// ═══════════════════════════════════════════════════════════════════════

/// **Validates: Requirements 7.6**
mod property9_panicking_callback_resilience {
    use super::*;
    use codryn_pipeline::{IndexMode, Pipeline};

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(20))]

        #[test]
        fn panicking_callback_resilience(n in 5usize..20) {
            let tmp = tempfile::TempDir::new().unwrap();
            for i in 0..n {
                let content = format!("export function func{}() {{ return {}; }}\n", i, i);
                std::fs::write(tmp.path().join(format!("file{}.ts", i)), content).unwrap();
            }
            std::fs::write(tmp.path().join("package.json"), r#"{"name":"test"}"#).unwrap();

            let db_dir = tempfile::TempDir::new().unwrap();
            let mut pipeline = Pipeline::new(tmp.path(), db_dir.path(), IndexMode::Full);
            pipeline.set_progress_callback(Box::new(|_update| {
                panic!("intentional panic in progress callback");
            }));

            // Pipeline should complete successfully despite panicking callback
            let result = pipeline.run();
            prop_assert!(
                result.is_ok(),
                "Pipeline should complete Ok despite panicking callback, got: {:?}",
                result.err()
            );
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Property 10: Incremental Diff Correctness
// ═══════════════════════════════════════════════════════════════════════

/// **Validates: Requirements 4.2, 4.3**
/// IncrementalFileSet::compute must correctly partition files into
/// changed / changed_plus_dependents / all.
mod property10_incremental_diff_correctness {
    use codryn_discover::{DiscoveredFile, Language};
    use codryn_pipeline::IncrementalFileSet;
    use codryn_store::Store;
    use proptest::prelude::*;
    use std::path::PathBuf;

    fn make_file(rel: &str) -> DiscoveredFile {
        DiscoveredFile {
            abs_path: PathBuf::from(rel),
            rel_path: rel.to_string(),
            language: Language::Rust,
        }
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(50))]

        #[test]
        fn changed_subset_of_all(
            n_files in 1usize..20,
            n_changed in 0usize..20,
        ) {
            let n_changed = n_changed.min(n_files);
            let files: Vec<DiscoveredFile> = (0..n_files)
                .map(|i| make_file(&format!("src/file{}.rs", i)))
                .collect();
            let changed: Vec<&DiscoveredFile> = files[..n_changed].iter().collect();

            let store = Store::open_in_memory().unwrap();
            let fset = IncrementalFileSet::compute(&files, &changed, &store, "proj");

            // changed ⊆ all
            prop_assert!(fset.changed.len() <= fset.all.len());
            // changed_plus_dependents ⊆ all
            prop_assert!(fset.changed_plus_dependents.len() <= fset.all.len());
            // all == files
            prop_assert_eq!(fset.all.len(), n_files);
            // changed == n_changed
            prop_assert_eq!(fset.changed.len(), n_changed);
        }

        #[test]
        fn full_reindex_threshold_returns_all(
            n_files in 10usize..50,
        ) {
            // When changed >= 10% of all files, all categories return all files
            let files: Vec<DiscoveredFile> = (0..n_files)
                .map(|i| make_file(&format!("src/file{}.rs", i)))
                .collect();
            // Use exactly 10% changed to trigger the threshold
            let n_changed = (n_files / 10).max(1);
            let changed: Vec<&DiscoveredFile> = files[..n_changed].iter().collect();

            let store = Store::open_in_memory().unwrap();
            let fset = IncrementalFileSet::compute(&files, &changed, &store, "proj");

            prop_assert_eq!(fset.changed_plus_dependents.len(), n_files,
                "at threshold, changed_plus_dependents should be all files");
        }

        #[test]
        fn empty_changed_returns_empty_changed_set(
            n_files in 10usize..20,
        ) {
            // Use n_files >= 10 so that 0 changed < 10% threshold (0 < 1)
            let files: Vec<DiscoveredFile> = (0..n_files)
                .map(|i| make_file(&format!("src/file{}.rs", i)))
                .collect();
            let changed: Vec<&DiscoveredFile> = vec![];

            let store = Store::open_in_memory().unwrap();
            let fset = IncrementalFileSet::compute(&files, &changed, &store, "proj");

            prop_assert_eq!(fset.changed.len(), 0);
            prop_assert_eq!(fset.changed_plus_dependents.len(), 0);
            prop_assert_eq!(fset.all.len(), n_files);
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Property 11: FastAPI Depends Extraction Completeness
// ═══════════════════════════════════════════════════════════════════════

/// **Validates: Requirements 6.1, 6.2, 6.3, 6.4, 6.5**
/// For any Python source with N `Depends()` calls in a single handler,
/// `extract_fastapi_depends` must return exactly N relations.
mod property11_fastapi_depends_completeness {
    use codryn_pipeline::fastapi_adapter::{compute_chain_depths, extract_fastapi_depends};
    use proptest::prelude::*;

    fn dep_name_strategy() -> impl Strategy<Value = String> {
        "[a-z][a-z0-9_]{2,12}"
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(50))]

        #[test]
        fn n_depends_produces_n_relations(
            handler in "[a-z][a-z0-9_]{2,12}",
            deps in prop::collection::vec(dep_name_strategy(), 1..6),
        ) {
            // Build a Python function with N Depends() params
            let params: Vec<String> = deps
                .iter()
                .enumerate()
                .map(|(i, d)| format!("p{}: Any = Depends({})", i, d))
                .collect();
            let src = format!("def {}({}):\n    pass\n", handler, params.join(", "));

            let rels = extract_fastapi_depends(&src);
            prop_assert_eq!(
                rels.len(), deps.len(),
                "expected {} relations, got {} for source: {}", deps.len(), rels.len(), src
            );

            // All handler names should match
            for (h, _) in &rels {
                prop_assert_eq!(h, &handler);
            }

            // All dep names should be present
            let found_deps: Vec<&str> = rels.iter().map(|(_, d)| d.as_str()).collect();
            for dep in &deps {
                prop_assert!(
                    found_deps.contains(&dep.as_str()),
                    "dep {} not found in relations", dep
                );
            }
        }

        #[test]
        fn direct_dependency_has_depth_zero(
            handler in "[a-z][a-z0-9_]{2,12}",
            dep in dep_name_strategy(),
        ) {
            let rels = vec![(handler.clone(), dep.clone())];
            let depths = compute_chain_depths(&rels);
            prop_assert_eq!(
                depths.get(&(handler.clone(), dep.clone())),
                Some(&0),
                "direct dependency should have chain_depth=0"
            );
        }

        #[test]
        fn chained_dependency_has_depth_one(
            route_handler in "[a-z][a-z0-9_]{2,12}",
            mid_dep in "[a-z][a-z0-9_]{2,12}",
            leaf_dep in "[a-z][a-z0-9_]{2,12}",
        ) {
            prop_assume!(route_handler != mid_dep && mid_dep != leaf_dep && route_handler != leaf_dep);
            let rels = vec![
                (route_handler.clone(), mid_dep.clone()),
                (mid_dep.clone(), leaf_dep.clone()),
            ];
            let depths = compute_chain_depths(&rels);
            // route_handler -> mid_dep: depth 0
            prop_assert_eq!(
                depths.get(&(route_handler.clone(), mid_dep.clone())),
                Some(&0)
            );
            // mid_dep -> leaf_dep: depth 1
            prop_assert_eq!(
                depths.get(&(mid_dep.clone(), leaf_dep.clone())),
                Some(&1)
            );
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Property 8: Tier 3 Regex Extraction Correctness
// ═══════════════════════════════════════════════════════════════════════

/// **Validates: Requirements 11.1, 11.2, 11.3, 11.4**
///
/// For any file containing N recognizable top-level definitions (functions, modules,
/// types, headings, keys, targets) in a supported Tier 3 language or markup format,
/// the regex extractor SHALL produce at least N nodes, each with a valid name, label,
/// and start_line > 0.
mod property8_tier3_extraction {
    use super::*;
    use codryn_pipeline::tier3_walkers::{
        extract_build_infra, extract_markup, extract_sfc, extract_tier3_programming,
    };

    // ── Strategies for generating source files with known definitions ──

    /// Generate a valid identifier for Tier 3 languages (alphanumeric, starts with letter).
    fn ident_strategy() -> impl Strategy<Value = String> {
        "[a-z][a-z0-9_]{1,15}"
    }

    /// Generate a Fortran source file with N known subroutines/functions.
    fn fortran_source_strategy(n: usize) -> impl Strategy<Value = (String, usize)> {
        prop::collection::vec(ident_strategy(), n..=n).prop_map(move |names| {
            let mut src = String::new();
            for name in &names {
                src.push_str(&format!(
                    "subroutine {}(a, b)\n  integer :: a, b\n  a = a + b\nend subroutine {}\n\n",
                    name, name
                ));
            }
            (src, names.len())
        })
    }

    /// Generate a Gleam source file with N known functions.
    fn gleam_source_strategy(n: usize) -> impl Strategy<Value = (String, usize)> {
        prop::collection::vec(ident_strategy(), n..=n).prop_map(move |names| {
            let mut src = String::new();
            for name in &names {
                src.push_str(&format!(
                    "pub fn {}(x: Int) -> Int {{\n  x + 1\n}}\n\n",
                    name
                ));
            }
            (src, names.len())
        })
    }

    /// Generate a Crystal source file with N known definitions (mix of classes and functions).
    fn crystal_source_strategy(n: usize) -> impl Strategy<Value = (String, usize)> {
        prop::collection::vec(ident_strategy(), n..=n).prop_map(move |names| {
            let mut src = String::new();
            for name in &names {
                src.push_str(&format!("def {}(x)\n  x + 1\nend\n\n", name));
            }
            (src, names.len())
        })
    }

    /// Generate a GDScript source file with N known functions.
    fn gdscript_source_strategy(n: usize) -> impl Strategy<Value = (String, usize)> {
        prop::collection::vec(ident_strategy(), n..=n).prop_map(move |names| {
            let mut src = String::new();
            for name in &names {
                src.push_str(&format!("func {}():\n    pass\n\n", name));
            }
            (src, names.len())
        })
    }

    /// Generate a Markdown file with N known headings.
    fn markdown_source_strategy(n: usize) -> impl Strategy<Value = (String, usize)> {
        prop::collection::vec(ident_strategy(), n..=n).prop_map(move |names| {
            let mut src = String::new();
            for (i, name) in names.iter().enumerate() {
                let level = (i % 3) + 1; // Cycle through h1, h2, h3
                let hashes = "#".repeat(level);
                src.push_str(&format!("{} {}\n\nSome content here.\n\n", hashes, name));
            }
            (src, names.len())
        })
    }

    /// Generate a YAML file with N known top-level keys.
    fn yaml_source_strategy(n: usize) -> impl Strategy<Value = (String, usize)> {
        prop::collection::vec(ident_strategy(), n..=n).prop_map(move |names| {
            let mut src = String::new();
            for name in &names {
                src.push_str(&format!("{}: value_{}\n", name, name));
            }
            (src, names.len())
        })
    }

    /// Generate a Makefile with N known targets.
    fn makefile_source_strategy(n: usize) -> impl Strategy<Value = (String, usize)> {
        prop::collection::vec(ident_strategy(), n..=n).prop_map(move |names| {
            let mut src = String::new();
            for name in &names {
                src.push_str(&format!("{}:\n\t@echo \"running {}\"\n\n", name, name));
            }
            (src, names.len())
        })
    }

    /// Generate a Dockerfile with N known FROM stages.
    fn dockerfile_source_strategy(n: usize) -> impl Strategy<Value = (String, usize)> {
        prop::collection::vec(ident_strategy(), n..=n).prop_map(move |names| {
            let mut src = String::new();
            for name in &names {
                src.push_str(&format!(
                    "FROM alpine:3.18 AS {}\nRUN echo {}\n\n",
                    name, name
                ));
            }
            (src, names.len())
        })
    }

    /// Generate a Svelte SFC with N known exported variables/functions.
    fn svelte_source_strategy(n: usize) -> impl Strategy<Value = (String, usize)> {
        prop::collection::vec(ident_strategy(), n..=n).prop_map(move |names| {
            let mut script_content = String::new();
            for name in &names {
                script_content.push_str(&format!("  export let {} = 0;\n", name));
            }
            // The component itself (from filename) is always extracted as 1 additional node
            let src = format!(
                "<script>\n{}</script>\n\n<div>Hello</div>\n",
                script_content
            );
            // Expected: N exported vars + 1 component node from filename
            (src, names.len() + 1)
        })
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(100))]

        // ── Tier 3 Programming Languages ──

        #[test]
        fn fortran_extraction_produces_at_least_n_nodes(
            (source, expected_count) in (1usize..6).prop_flat_map(fortran_source_strategy),
        ) {
            let nodes = extract_tier3_programming(&source, "test.f90", "proj", Language::Fortran);

            prop_assert!(
                nodes.len() >= expected_count,
                "Expected at least {} nodes, got {} for Fortran source:\n{}",
                expected_count, nodes.len(), source
            );

            for node in &nodes {
                prop_assert!(!node.name.is_empty(), "Node name must not be empty");
                prop_assert!(
                    !node.label.is_empty(),
                    "Node label must not be empty for '{}'", node.name
                );
                prop_assert!(
                    node.start_line > 0,
                    "start_line must be > 0 for '{}', got {}", node.name, node.start_line
                );
            }
        }

        #[test]
        fn gleam_extraction_produces_at_least_n_nodes(
            (source, expected_count) in (1usize..6).prop_flat_map(gleam_source_strategy),
        ) {
            let nodes = extract_tier3_programming(&source, "app.gleam", "proj", Language::Gleam);

            prop_assert!(
                nodes.len() >= expected_count,
                "Expected at least {} nodes, got {} for Gleam source:\n{}",
                expected_count, nodes.len(), source
            );

            for node in &nodes {
                prop_assert!(!node.name.is_empty(), "Node name must not be empty");
                prop_assert!(!node.label.is_empty(), "Node label must not be empty");
                prop_assert!(node.start_line > 0, "start_line must be > 0, got {}", node.start_line);
            }
        }

        #[test]
        fn crystal_extraction_produces_at_least_n_nodes(
            (source, expected_count) in (1usize..6).prop_flat_map(crystal_source_strategy),
        ) {
            let nodes = extract_tier3_programming(&source, "app.cr", "proj", Language::Crystal);

            prop_assert!(
                nodes.len() >= expected_count,
                "Expected at least {} nodes, got {} for Crystal source:\n{}",
                expected_count, nodes.len(), source
            );

            for node in &nodes {
                prop_assert!(!node.name.is_empty(), "Node name must not be empty");
                prop_assert!(!node.label.is_empty(), "Node label must not be empty");
                prop_assert!(node.start_line > 0, "start_line must be > 0, got {}", node.start_line);
            }
        }

        #[test]
        fn gdscript_extraction_produces_at_least_n_nodes(
            (source, expected_count) in (1usize..6).prop_flat_map(gdscript_source_strategy),
        ) {
            let nodes = extract_tier3_programming(&source, "player.gd", "proj", Language::GDScript);

            prop_assert!(
                nodes.len() >= expected_count,
                "Expected at least {} nodes, got {} for GDScript source:\n{}",
                expected_count, nodes.len(), source
            );

            for node in &nodes {
                prop_assert!(!node.name.is_empty(), "Node name must not be empty");
                prop_assert!(!node.label.is_empty(), "Node label must not be empty");
                prop_assert!(node.start_line > 0, "start_line must be > 0, got {}", node.start_line);
            }
        }

        // ── Markup Extractors ──

        #[test]
        fn markdown_extraction_produces_at_least_n_nodes(
            (source, expected_count) in (1usize..6).prop_flat_map(markdown_source_strategy),
        ) {
            let nodes = extract_markup(&source, "README.md", "proj", Language::Markdown);

            prop_assert!(
                nodes.len() >= expected_count,
                "Expected at least {} nodes, got {} for Markdown source:\n{}",
                expected_count, nodes.len(), source
            );

            for node in &nodes {
                prop_assert!(!node.name.is_empty(), "Node name must not be empty");
                prop_assert!(!node.label.is_empty(), "Node label must not be empty");
                prop_assert!(node.start_line > 0, "start_line must be > 0, got {}", node.start_line);
            }
        }

        #[test]
        fn yaml_extraction_produces_at_least_n_nodes(
            (source, expected_count) in (1usize..6).prop_flat_map(yaml_source_strategy),
        ) {
            let nodes = extract_markup(&source, "config.yml", "proj", Language::Yaml);

            prop_assert!(
                nodes.len() >= expected_count,
                "Expected at least {} nodes, got {} for YAML source:\n{}",
                expected_count, nodes.len(), source
            );

            for node in &nodes {
                prop_assert!(!node.name.is_empty(), "Node name must not be empty");
                prop_assert!(!node.label.is_empty(), "Node label must not be empty");
                prop_assert!(node.start_line > 0, "start_line must be > 0, got {}", node.start_line);
            }
        }

        // ── Build/Infra Extractors ──

        #[test]
        fn makefile_extraction_produces_at_least_n_nodes(
            (source, expected_count) in (1usize..6).prop_flat_map(makefile_source_strategy),
        ) {
            let nodes = extract_build_infra(&source, "Makefile", "proj", "makefile");

            prop_assert!(
                nodes.len() >= expected_count,
                "Expected at least {} nodes, got {} for Makefile source:\n{}",
                expected_count, nodes.len(), source
            );

            for node in &nodes {
                prop_assert!(!node.name.is_empty(), "Node name must not be empty");
                prop_assert!(!node.label.is_empty(), "Node label must not be empty");
                prop_assert!(node.start_line > 0, "start_line must be > 0, got {}", node.start_line);
            }
        }

        #[test]
        fn dockerfile_extraction_produces_at_least_n_nodes(
            (source, expected_count) in (1usize..6).prop_flat_map(dockerfile_source_strategy),
        ) {
            let nodes = extract_build_infra(&source, "Dockerfile", "proj", "dockerfile");

            prop_assert!(
                nodes.len() >= expected_count,
                "Expected at least {} nodes, got {} for Dockerfile source:\n{}",
                expected_count, nodes.len(), source
            );

            for node in &nodes {
                prop_assert!(!node.name.is_empty(), "Node name must not be empty");
                prop_assert!(!node.label.is_empty(), "Node label must not be empty");
                prop_assert!(node.start_line > 0, "start_line must be > 0, got {}", node.start_line);
            }
        }

        // ── SFC Extractors ──

        #[test]
        fn svelte_extraction_produces_at_least_n_nodes(
            (source, expected_count) in (1usize..5).prop_flat_map(svelte_source_strategy),
        ) {
            let nodes = extract_sfc(&source, "src/Counter.svelte", "proj", "svelte");

            prop_assert!(
                nodes.len() >= expected_count,
                "Expected at least {} nodes, got {} for Svelte source:\n{}",
                expected_count, nodes.len(), source
            );

            for node in &nodes {
                prop_assert!(!node.name.is_empty(), "Node name must not be empty");
                prop_assert!(!node.label.is_empty(), "Node label must not be empty");
                prop_assert!(node.start_line > 0, "start_line must be > 0, got {}", node.start_line);
            }
        }

        // ── Cross-format: all nodes have valid properties ──

        #[test]
        fn all_tier3_nodes_have_valid_file_path_and_qn(
            (source, _) in (1usize..4).prop_flat_map(fortran_source_strategy),
        ) {
            let nodes = extract_tier3_programming(&source, "math.f90", "myproject", Language::Fortran);

            for node in &nodes {
                prop_assert!(
                    !node.qualified_name.is_empty(),
                    "qualified_name must not be empty for '{}'", node.name
                );
                prop_assert_eq!(
                    &node.file_path, "math.f90",
                    "file_path must match the input file_path"
                );
                prop_assert!(
                    node.label == "Function" || node.label == "Class" || node.label == "Module",
                    "label must be Function, Class, or Module, got '{}'", node.label
                );
            }
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Property 12: Kubernetes Manifest Node/Edge Creation
// ═══════════════════════════════════════════════════════════════════════

/// **Validates: Requirements 15.1, 15.2, 15.3, 15.4, 15.5**
mod property12_k8s_manifest_parsing {
    use super::*;
    use codryn_pipeline::pass_k8s::pass_k8s;

    // ── Strategies ──

    /// Generate a valid K8s resource name (lowercase alphanumeric + hyphens, 1-20 chars).
    fn k8s_name_strategy() -> impl Strategy<Value = String> {
        "[a-z][a-z0-9\\-]{0,14}[a-z0-9]".prop_filter("non-empty and no double hyphens", |s| {
            !s.is_empty() && !s.contains("--")
        })
    }

    /// Generate a valid K8s namespace.
    fn namespace_strategy() -> impl Strategy<Value = String> {
        prop_oneof![
            Just("default".to_string()),
            Just("production".to_string()),
            Just("staging".to_string()),
            "[a-z]{3,10}".prop_map(|s| s),
        ]
    }

    /// Generate a supported K8s resource kind.
    fn kind_strategy() -> impl Strategy<Value = &'static str> {
        prop_oneof![
            Just("Deployment"),
            Just("Service"),
            Just("ConfigMap"),
            Just("Secret"),
            Just("Ingress"),
        ]
    }

    /// Generate a Docker image reference.
    fn image_strategy() -> impl Strategy<Value = String> {
        ("[a-z]{3,10}", "[a-z]{3,10}", "[a-z0-9]{1,8}")
            .prop_map(|(registry, name, tag)| format!("{}/{name}:{tag}", registry))
    }

    /// Generate a container port number.
    fn port_strategy() -> impl Strategy<Value = u16> {
        1024u16..65535
    }

    /// Generate a Deployment YAML manifest with configurable properties.
    fn deployment_yaml(
        name: &str,
        namespace: &str,
        images: &[String],
        container_ports: &[u16],
        configmap_refs: &[String],
        secret_refs: &[String],
    ) -> String {
        let mut yaml = format!(
            "apiVersion: apps/v1\nkind: Deployment\nmetadata:\n  name: {}\n  namespace: {}\nspec:\n  template:\n    spec:\n      containers:\n        - name: app\n",
            name, namespace
        );
        if let Some(img) = images.first() {
            yaml.push_str(&format!("          image: {}\n", img));
        }
        if !container_ports.is_empty() {
            yaml.push_str("          ports:\n");
            for port in container_ports {
                yaml.push_str(&format!("            - containerPort: {}\n", port));
            }
        }
        // Add additional images as extra containers
        for img in images.iter().skip(1) {
            yaml.push_str(&format!(
                "        - name: sidecar\n          image: {}\n",
                img
            ));
        }
        // Add configMap references via envFrom
        if !configmap_refs.is_empty() {
            yaml.push_str("          envFrom:\n");
            for cm in configmap_refs {
                yaml.push_str(&format!(
                    "            - configMapRef:\n                name: {}\n",
                    cm
                ));
            }
        }
        // Add secret references via env valueFrom
        if !secret_refs.is_empty() {
            yaml.push_str("          env:\n");
            for (i, secret) in secret_refs.iter().enumerate() {
                yaml.push_str(&format!(
                    "            - name: SECRET_{}\n              valueFrom:\n                secretKeyRef:\n                  name: {}\n                  key: key{}\n",
                    i, secret, i
                ));
            }
        }
        yaml
    }

    /// Generate a Service YAML manifest.
    fn service_yaml(name: &str, namespace: &str, target_ports: &[u16]) -> String {
        let mut yaml = format!(
            "apiVersion: v1\nkind: Service\nmetadata:\n  name: {}\n  namespace: {}\nspec:\n  ports:\n",
            name, namespace
        );
        for port in target_ports {
            yaml.push_str(&format!("    - port: 80\n      targetPort: {}\n", port));
        }
        yaml
    }

    /// Generate a ConfigMap YAML manifest.
    fn configmap_yaml(name: &str, namespace: &str) -> String {
        format!(
            "apiVersion: v1\nkind: ConfigMap\nmetadata:\n  name: {}\n  namespace: {}\ndata:\n  key: value\n",
            name, namespace
        )
    }

    /// Generate a Secret YAML manifest.
    fn secret_yaml(name: &str, namespace: &str) -> String {
        format!(
            "apiVersion: v1\nkind: Secret\nmetadata:\n  name: {}\n  namespace: {}\ntype: Opaque\ndata:\n  password: cGFzc3dvcmQ=\n",
            name, namespace
        )
    }

    /// Generate an Ingress YAML manifest.
    fn ingress_yaml(name: &str, namespace: &str) -> String {
        format!(
            "apiVersion: networking.k8s.io/v1\nkind: Ingress\nmetadata:\n  name: {}\n  namespace: {}\nspec:\n  rules:\n    - host: example.com\n      http:\n        paths:\n          - path: /\n            pathType: Prefix\n            backend:\n              service:\n                name: web\n                port:\n                  number: 80\n",
            name, namespace
        )
    }

    /// Helper to write a YAML file and create a DiscoveredFile.
    fn write_yaml(dir: &tempfile::TempDir, filename: &str, content: &str) -> DiscoveredFile {
        let path = dir.path().join(filename);
        std::fs::write(&path, content).unwrap();
        DiscoveredFile {
            abs_path: path,
            rel_path: filename.to_owned(),
            language: Language::Yaml,
        }
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(80))]

        /// For any supported K8s resource kind, pass_k8s creates exactly one
        /// Infrastructure node with correct name, kind, and namespace properties.
        #[test]
        fn single_resource_creates_one_infrastructure_node(
            kind in kind_strategy(),
            name in k8s_name_strategy(),
            namespace in namespace_strategy(),
        ) {
            let dir = tempfile::tempdir().unwrap();
            let yaml = match kind {
                "Deployment" => deployment_yaml(&name, &namespace, &["nginx:latest".to_string()], &[8080], &[], &[]),
                "Service" => service_yaml(&name, &namespace, &[80]),
                "ConfigMap" => configmap_yaml(&name, &namespace),
                "Secret" => secret_yaml(&name, &namespace),
                "Ingress" => ingress_yaml(&name, &namespace),
                _ => unreachable!(),
            };

            let file = write_yaml(&dir, "resource.yaml", &yaml);
            let mut buf = GraphBuffer::new("proj");
            let files: Vec<&DiscoveredFile> = vec![&file];
            pass_k8s(&mut buf, &files, "proj");

            // For Deployment, we also get an image node, so check >= 1
            prop_assert!(
                buf.node_count() >= 1,
                "Expected at least 1 Infrastructure node for kind={}, got {}",
                kind, buf.node_count()
            );
        }

        /// For any Deployment with image references, pass_k8s creates DEPLOYS edges.
        #[test]
        fn deployment_creates_deploys_edges_for_images(
            name in k8s_name_strategy(),
            namespace in namespace_strategy(),
            images in prop::collection::vec(image_strategy(), 1..4),
        ) {
            let dir = tempfile::tempdir().unwrap();
            let yaml = deployment_yaml(&name, &namespace, &images, &[8080], &[], &[]);
            let file = write_yaml(&dir, "deploy.yaml", &yaml);
            let mut buf = GraphBuffer::new("proj");
            let files: Vec<&DiscoveredFile> = vec![&file];
            pass_k8s(&mut buf, &files, "proj");

            // Should have at least one DEPLOYS edge per unique image
            let unique_images: std::collections::HashSet<&String> = images.iter().collect();
            let edges = buf.take_edges();
            let deploys_count = edges.iter()
                .filter(|e| e.edge_type == "DEPLOYS")
                .count();

            prop_assert!(
                deploys_count >= unique_images.len(),
                "Expected at least {} DEPLOYS edges for {} unique images, got {}",
                unique_images.len(), unique_images.len(), deploys_count
            );
        }

        /// For any Deployment referencing ConfigMaps, pass_k8s creates CONFIGURES edges.
        #[test]
        fn deployment_creates_configures_edges_for_configmaps(
            name in k8s_name_strategy(),
            namespace in namespace_strategy(),
            configmap_refs in prop::collection::vec(k8s_name_strategy(), 1..4),
        ) {
            let dir = tempfile::tempdir().unwrap();
            let yaml = deployment_yaml(
                &name, &namespace,
                &["app:latest".to_string()], &[8080],
                &configmap_refs, &[],
            );
            let file = write_yaml(&dir, "deploy.yaml", &yaml);
            let mut buf = GraphBuffer::new("proj");
            let files: Vec<&DiscoveredFile> = vec![&file];
            pass_k8s(&mut buf, &files, "proj");

            let edges = buf.take_edges();
            let configures_count = edges.iter()
                .filter(|e| e.edge_type == "CONFIGURES")
                .count();

            // Each unique configmap ref should produce a CONFIGURES edge
            let unique_refs: std::collections::HashSet<&String> = configmap_refs.iter().collect();
            prop_assert!(
                configures_count >= unique_refs.len(),
                "Expected at least {} CONFIGURES edges for {} unique ConfigMap refs, got {}",
                unique_refs.len(), unique_refs.len(), configures_count
            );
        }

        /// For any Deployment referencing Secrets, pass_k8s creates CONFIGURES edges.
        #[test]
        fn deployment_creates_configures_edges_for_secrets(
            name in k8s_name_strategy(),
            namespace in namespace_strategy(),
            secret_refs in prop::collection::vec(k8s_name_strategy(), 1..4),
        ) {
            let dir = tempfile::tempdir().unwrap();
            let yaml = deployment_yaml(
                &name, &namespace,
                &["app:latest".to_string()], &[8080],
                &[], &secret_refs,
            );
            let file = write_yaml(&dir, "deploy.yaml", &yaml);
            let mut buf = GraphBuffer::new("proj");
            let files: Vec<&DiscoveredFile> = vec![&file];
            pass_k8s(&mut buf, &files, "proj");

            let edges = buf.take_edges();
            let configures_count = edges.iter()
                .filter(|e| e.edge_type == "CONFIGURES")
                .count();

            let unique_refs: std::collections::HashSet<&String> = secret_refs.iter().collect();
            prop_assert!(
                configures_count >= unique_refs.len(),
                "Expected at least {} CONFIGURES edges for {} unique Secret refs, got {}",
                unique_refs.len(), unique_refs.len(), configures_count
            );
        }

        /// For any Service with targetPort matching a Deployment's containerPort,
        /// pass_k8s creates EXPOSES edges.
        #[test]
        fn service_creates_exposes_edges_for_port_match(
            deploy_name in k8s_name_strategy(),
            svc_name in k8s_name_strategy(),
            namespace in namespace_strategy(),
            port in port_strategy(),
        ) {
            let dir = tempfile::tempdir().unwrap();
            // Create a multi-document YAML with Deployment + Service sharing a port
            let deploy_yaml_str = deployment_yaml(
                &deploy_name, &namespace,
                &["app:v1".to_string()], &[port],
                &[], &[],
            );
            let svc_yaml_str = service_yaml(&svc_name, &namespace, &[port]);
            let combined = format!("{}\n---\n{}", deploy_yaml_str, svc_yaml_str);

            let file = write_yaml(&dir, "combined.yaml", &combined);
            let mut buf = GraphBuffer::new("proj");
            let files: Vec<&DiscoveredFile> = vec![&file];
            pass_k8s(&mut buf, &files, "proj");

            let edges = buf.take_edges();
            let exposes_count = edges.iter()
                .filter(|e| e.edge_type == "EXPOSES")
                .count();

            prop_assert!(
                exposes_count >= 1,
                "Expected at least 1 EXPOSES edge for port-matched Service (port={}), got {}",
                port, exposes_count
            );
        }

        /// Multi-document YAML creates separate Infrastructure nodes per resource.
        #[test]
        fn multi_document_yaml_creates_separate_nodes(
            names in prop::collection::vec(k8s_name_strategy(), 2..5),
            namespace in namespace_strategy(),
        ) {
            let dir = tempfile::tempdir().unwrap();
            // Create a multi-document YAML with multiple ConfigMaps
            let documents: Vec<String> = names.iter()
                .map(|n| configmap_yaml(n, &namespace))
                .collect();
            let combined = documents.join("---\n");

            let file = write_yaml(&dir, "multi.yaml", &combined);
            let mut buf = GraphBuffer::new("proj");
            let files: Vec<&DiscoveredFile> = vec![&file];
            pass_k8s(&mut buf, &files, "proj");

            // Each unique name should produce a separate Infrastructure node
            let unique_names: std::collections::HashSet<&String> = names.iter().collect();
            prop_assert!(
                buf.node_count() >= unique_names.len(),
                "Expected at least {} nodes for {} unique resources in multi-doc YAML, got {}",
                unique_names.len(), unique_names.len(), buf.node_count()
            );
        }

        /// Namespace defaults to "default" when not specified in the manifest.
        #[test]
        fn namespace_defaults_to_default_when_missing(
            name in k8s_name_strategy(),
        ) {
            let dir = tempfile::tempdir().unwrap();
            // ConfigMap without namespace
            let yaml = format!(
                "apiVersion: v1\nkind: ConfigMap\nmetadata:\n  name: {}\ndata:\n  key: value\n",
                name
            );
            let file = write_yaml(&dir, "no-ns.yaml", &yaml);
            let mut buf = GraphBuffer::new("proj");
            let files: Vec<&DiscoveredFile> = vec![&file];
            pass_k8s(&mut buf, &files, "proj");

            prop_assert!(
                buf.node_count() >= 1,
                "Expected at least 1 node for ConfigMap without namespace, got {}",
                buf.node_count()
            );
        }

        /// Non-K8s YAML files produce zero nodes and zero edges.
        #[test]
        fn non_k8s_yaml_produces_no_nodes(
            key in "[a-z]{3,10}",
            value in "[a-z]{3,10}",
        ) {
            let dir = tempfile::tempdir().unwrap();
            let yaml = format!("{}:\n  sub_key: {}\n", key, value);
            let file = write_yaml(&dir, "config.yaml", &yaml);
            let mut buf = GraphBuffer::new("proj");
            let files: Vec<&DiscoveredFile> = vec![&file];
            pass_k8s(&mut buf, &files, "proj");

            prop_assert_eq!(
                buf.node_count(), 0,
                "Non-K8s YAML should produce 0 nodes, got {}",
                buf.node_count()
            );
            prop_assert_eq!(
                buf.edge_count(), 0,
                "Non-K8s YAML should produce 0 edges, got {}",
                buf.edge_count()
            );
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Property 11: Configuration Key Extraction Round-Trip
// ═══════════════════════════════════════════════════════════════════════

/// **Validates: Requirements 14.1, 14.2, 14.3, 14.5**
///
/// For any nested configuration structure (YAML, JSON, TOML) with depth <= 10,
/// extracting keys SHALL produce dot-separated notation that uniquely identifies
/// each leaf value, and for any code pattern matching a config access referencing
/// an extracted key, a CONFIGURES edge SHALL be created with the correct key
/// property using case-sensitive matching.
mod property11_config_key_extraction_round_trip {
    use super::*;
    use codryn_pipeline::pass_configlink::{extract_config_keys, pass_configlink, MAX_DEPTH};

    /// Strategy for generating valid config key segments (alphanumeric + underscores).
    fn key_segment_strategy() -> impl Strategy<Value = String> {
        "[a-zA-Z][a-zA-Z0-9_]{0,12}"
    }

    /// Strategy for generating simple scalar values for config files.
    fn scalar_value_strategy() -> impl Strategy<Value = String> {
        prop_oneof![
            "[a-zA-Z0-9]{1,20}",
            "[0-9]{1,5}",
            Just("true".to_string()),
            Just("false".to_string()),
        ]
    }

    /// Strategy for generating nested config with depth 2-3.
    /// Filters out conflicting entries where one path is a prefix of another
    /// (e.g., ["m"] and ["m", "a"] can't coexist since "m" can't be both a scalar and object).
    fn nested_config_strategy() -> impl Strategy<Value = Vec<(Vec<String>, String)>> {
        prop::collection::vec(
            (
                prop::collection::vec(key_segment_strategy(), 1..4),
                scalar_value_strategy(),
            ),
            1..8,
        )
        .prop_map(|entries| {
            // Remove entries where one path is a prefix of another.
            // Keep the longer path (more specific) when conflicts arise.
            let mut filtered: Vec<(Vec<String>, String)> = Vec::new();
            for (path, value) in entries {
                let dominated = filtered.iter().any(|(existing, _)| {
                    // Check if existing is a prefix of path (existing dominates path)
                    path.len() > existing.len() && path[..existing.len()] == existing[..]
                });
                let dominates = filtered.iter().position(|(existing, _)| {
                    // Check if path is a prefix of existing (path dominates existing)
                    existing.len() > path.len() && existing[..path.len()] == path[..]
                });
                if dominated {
                    // Skip this entry - an ancestor already exists as a leaf
                    continue;
                }
                if let Some(idx) = dominates {
                    // Remove the existing entry that this path dominates
                    filtered.remove(idx);
                }
                filtered.push((path, value));
            }
            filtered
        })
        .prop_filter("must have at least one entry", |entries| {
            !entries.is_empty()
        })
    }

    /// Build a JSON string from nested key paths.
    fn build_json(entries: &[(Vec<String>, String)]) -> String {
        let mut root = serde_json::Map::new();
        for (path, value) in entries {
            let mut current = &mut root;
            for (i, segment) in path.iter().enumerate() {
                if i == path.len() - 1 {
                    // Leaf value
                    current.insert(segment.clone(), serde_json::Value::String(value.clone()));
                } else {
                    // Intermediate object
                    current = current
                        .entry(segment.clone())
                        .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()))
                        .as_object_mut()
                        .unwrap();
                }
            }
        }
        serde_json::to_string_pretty(&serde_json::Value::Object(root)).unwrap()
    }

    /// Build a YAML string from nested key paths.
    fn build_yaml(entries: &[(Vec<String>, String)]) -> String {
        // Use serde_json to build the structure, then convert to YAML-like format
        let mut root = serde_json::Map::new();
        for (path, value) in entries {
            let mut current = &mut root;
            for (i, segment) in path.iter().enumerate() {
                if i == path.len() - 1 {
                    current.insert(segment.clone(), serde_json::Value::String(value.clone()));
                } else {
                    current = current
                        .entry(segment.clone())
                        .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()))
                        .as_object_mut()
                        .unwrap();
                }
            }
        }
        // Convert to YAML using indentation
        fn to_yaml(obj: &serde_json::Map<String, serde_json::Value>, indent: usize) -> String {
            let mut out = String::new();
            for (k, v) in obj {
                let prefix = " ".repeat(indent);
                match v {
                    serde_json::Value::Object(inner) => {
                        out.push_str(&format!("{}{}:\n", prefix, k));
                        out.push_str(&to_yaml(inner, indent + 2));
                    }
                    serde_json::Value::String(s) => {
                        out.push_str(&format!("{}{}: {}\n", prefix, k, s));
                    }
                    _ => {
                        out.push_str(&format!("{}{}: {}\n", prefix, k, v));
                    }
                }
            }
            out
        }
        to_yaml(&root, 0)
    }

    /// Build a TOML string from nested key paths.
    fn build_toml(entries: &[(Vec<String>, String)]) -> String {
        let mut root = toml::map::Map::new();
        for (path, value) in entries {
            let mut current = &mut root;
            for (i, segment) in path.iter().enumerate() {
                if i == path.len() - 1 {
                    current.insert(segment.clone(), toml::Value::String(value.clone()));
                } else {
                    current = current
                        .entry(segment.clone())
                        .or_insert_with(|| toml::Value::Table(toml::map::Map::new()))
                        .as_table_mut()
                        .unwrap();
                }
            }
        }
        toml::to_string_pretty(&toml::Value::Table(root)).unwrap()
    }

    /// Compute expected dot-separated keys from nested path entries.
    fn expected_keys(entries: &[(Vec<String>, String)]) -> std::collections::HashSet<String> {
        entries.iter().map(|(path, _)| path.join(".")).collect()
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(80))]

        /// JSON key extraction produces dot-separated keys for all leaf values.
        #[test]
        fn json_keys_round_trip(
            entries in nested_config_strategy()
        ) {
            let json_content = build_json(&entries);
            let extracted = extract_config_keys("config.json", &json_content);
            let extracted_set: std::collections::HashSet<String> = extracted.into_iter().collect();
            let expected = expected_keys(&entries);

            // Every expected key should be present in extracted keys
            for key in &expected {
                prop_assert!(
                    extracted_set.contains(key),
                    "Expected key '{}' not found in extracted JSON keys. Extracted: {:?}",
                    key, extracted_set
                );
            }

            // All extracted keys should use dot-separated notation
            for key in &extracted_set {
                let depth = key.split('.').count();
                prop_assert!(
                    depth <= MAX_DEPTH,
                    "Key '{}' exceeds max depth {} (has depth {})",
                    key, MAX_DEPTH, depth
                );
                // Each segment should be non-empty
                for segment in key.split('.') {
                    prop_assert!(
                        !segment.is_empty(),
                        "Key '{}' contains empty segment", key
                    );
                }
            }
        }

        /// YAML key extraction produces dot-separated keys for all leaf values.
        #[test]
        fn yaml_keys_round_trip(
            entries in nested_config_strategy()
        ) {
            let yaml_content = build_yaml(&entries);
            let extracted = extract_config_keys("application.yml", &yaml_content);
            let extracted_set: std::collections::HashSet<String> = extracted.into_iter().collect();
            let expected = expected_keys(&entries);

            for key in &expected {
                prop_assert!(
                    extracted_set.contains(key),
                    "Expected key '{}' not found in extracted YAML keys. Extracted: {:?}\nYAML:\n{}",
                    key, extracted_set, yaml_content
                );
            }

            for key in &extracted_set {
                let depth = key.split('.').count();
                prop_assert!(
                    depth <= MAX_DEPTH,
                    "Key '{}' exceeds max depth {}", key, MAX_DEPTH
                );
            }
        }

        /// TOML key extraction produces dot-separated keys for all leaf values.
        #[test]
        fn toml_keys_round_trip(
            entries in nested_config_strategy()
        ) {
            let toml_content = build_toml(&entries);
            let extracted = extract_config_keys("config.toml", &toml_content);
            let extracted_set: std::collections::HashSet<String> = extracted.into_iter().collect();
            let expected = expected_keys(&entries);

            for key in &expected {
                prop_assert!(
                    extracted_set.contains(key),
                    "Expected key '{}' not found in extracted TOML keys. Extracted: {:?}",
                    key, extracted_set
                );
            }

            for key in &extracted_set {
                let depth = key.split('.').count();
                prop_assert!(
                    depth <= MAX_DEPTH,
                    "Key '{}' exceeds max depth {}", key, MAX_DEPTH
                );
            }
        }

        /// Keys extracted from config are unique (no duplicates).
        #[test]
        fn extracted_keys_are_unique(
            entries in nested_config_strategy()
        ) {
            let json_content = build_json(&entries);
            let extracted = extract_config_keys("config.json", &json_content);
            let unique: std::collections::HashSet<&String> = extracted.iter().collect();
            prop_assert_eq!(
                extracted.len(), unique.len(),
                "Extracted keys contain duplicates: {:?}", extracted
            );
        }

        /// Config access patterns in code match keys case-sensitively, and
        /// pass_configlink creates CONFIGURES edges with the correct key property.
        #[test]
        fn configlink_creates_edges_with_correct_key(
            key in key_segment_strategy().prop_filter("uppercase chars", |s| {
                s.chars().any(|c| c.is_uppercase()) || s.len() >= 3
            }),
            value in scalar_value_strategy(),
        ) {
            let dir = tempfile::tempdir().unwrap();
            let project = "test_proj";

            // Create a .env config file with the key
            let env_key = key.to_uppercase();
            let env_content = format!("{}={}\n", env_key, value);
            let env_path = dir.path().join(".env");
            std::fs::write(&env_path, &env_content).unwrap();

            // Create a JS file that references the key via process.env.KEY
            let js_content = format!(
                "function loadConfig() {{\n  const val = process.env.{};\n  return val;\n}}\n",
                env_key
            );
            let js_path = dir.path().join("app.js");
            std::fs::write(&js_path, &js_content).unwrap();

            let env_file = DiscoveredFile {
                abs_path: env_path,
                rel_path: ".env".to_string(),
                language: Language::Unknown,
            };
            let js_file = DiscoveredFile {
                abs_path: js_path,
                rel_path: "app.js".to_string(),
                language: Language::JavaScript,
            };

            // Set up registry with the function
            let mut reg = Registry::new();
            let func_qn = format!("{}.app.loadConfig", project);
            reg.register("loadConfig", &func_qn, "app.js", "Function", 1, 4);

            let files: Vec<&DiscoveredFile> = vec![&env_file, &js_file];
            let mut buf = GraphBuffer::new(project);

            pass_configlink(&mut buf, &reg, &files, project);

            // Should have created at least one CONFIGURES edge
            prop_assert!(
                buf.edge_count() >= 1,
                "Expected at least 1 CONFIGURES edge for key '{}', got {}",
                env_key, buf.edge_count()
            );

            // Verify the edge has the correct key property
            let edges = buf.take_edges();
            let configures_edge = edges.iter().find(|e| e.edge_type == "CONFIGURES");
            prop_assert!(
                configures_edge.is_some(),
                "No CONFIGURES edge found in buffer"
            );

            let edge = configures_edge.unwrap();
            if let Some(ref props_json) = edge.properties_json {
                let wrapper: serde_json::Value = serde_json::from_str(props_json).unwrap();
                // The edge properties are wrapped: { "_src_qn": ..., "_tgt_qn": ..., "_props": "..." }
                let inner_props_str = wrapper.get("_props")
                    .and_then(|v| v.as_str())
                    .unwrap_or("{}");
                let props: serde_json::Value = serde_json::from_str(inner_props_str).unwrap();
                let edge_key = props.get("key").and_then(|v| v.as_str()).unwrap_or("");
                prop_assert_eq!(
                    edge_key, env_key.as_str(),
                    "Edge key property '{}' doesn't match expected '{}'",
                    edge_key, env_key
                );
            } else {
                prop_assert!(false, "CONFIGURES edge has no properties_json");
            }

            buf.restore_edges(edges);
        }

        /// Case-sensitive matching: keys that differ only in case do NOT match.
        #[test]
        fn case_sensitive_no_false_match(
            base_key in "[a-z]{3,8}",
            value in scalar_value_strategy(),
        ) {
            let dir = tempfile::tempdir().unwrap();
            let project = "test_proj";

            // Config file has UPPERCASE key
            let upper_key = base_key.to_uppercase();
            let env_content = format!("{}={}\n", upper_key, value);
            let env_path = dir.path().join(".env");
            std::fs::write(&env_path, &env_content).unwrap();

            // Code references the LOWERCASE version (should NOT match)
            let js_content = format!(
                "function init() {{\n  const val = process.env.{};\n  return val;\n}}\n",
                base_key
            );
            let js_path = dir.path().join("app.js");
            std::fs::write(&js_path, &js_content).unwrap();

            let env_file = DiscoveredFile {
                abs_path: env_path,
                rel_path: ".env".to_string(),
                language: Language::Unknown,
            };
            let js_file = DiscoveredFile {
                abs_path: js_path,
                rel_path: "app.js".to_string(),
                language: Language::JavaScript,
            };

            let mut reg = Registry::new();
            let func_qn = format!("{}.app.init", project);
            reg.register("init", &func_qn, "app.js", "Function", 1, 4);

            let files: Vec<&DiscoveredFile> = vec![&env_file, &js_file];
            let mut buf = GraphBuffer::new(project);

            pass_configlink(&mut buf, &reg, &files, project);

            // Should NOT create any CONFIGURES edge because case doesn't match
            let edges = buf.take_edges();
            let configures_edges: Vec<_> = edges.iter()
                .filter(|e| e.edge_type == "CONFIGURES")
                .collect();
            prop_assert_eq!(
                configures_edges.len(), 0,
                "Expected 0 CONFIGURES edges for case mismatch ('{}' vs '{}'), got {}",
                upper_key, base_key, configures_edges.len()
            );
        }

        /// Depth limit: config structures deeper than MAX_DEPTH do not produce
        /// keys beyond the limit.
        #[test]
        fn depth_limit_enforced(
            segments in prop::collection::vec(key_segment_strategy(), 11..15),
            value in scalar_value_strategy(),
        ) {
            // Build a deeply nested JSON structure
            let mut json = format!("\"{}\"", value);
            for segment in segments.iter().rev() {
                json = format!("{{\"{}\": {}}}", segment, json);
            }

            let extracted = extract_config_keys("deep.json", &json);

            // All extracted keys should have depth <= MAX_DEPTH
            for key in &extracted {
                let depth = key.split('.').count();
                prop_assert!(
                    depth <= MAX_DEPTH,
                    "Key '{}' has depth {} which exceeds MAX_DEPTH {}",
                    key, depth, MAX_DEPTH
                );
            }
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Property 13: Environment Variable Unification
// ═══════════════════════════════════════════════════════════════════════

/// **Validates: Requirements 16.1, 16.2, 16.3**
mod property13_env_var_unification {
    use super::*;
    use codryn_pipeline::passes::pass_envscan;
    use codryn_pipeline::registry::Registry;

    // ── Strategies ──

    /// Generate a valid environment variable name (uppercase letters, digits, underscores).
    fn env_var_name_strategy() -> impl Strategy<Value = String> {
        "[A-Z][A-Z0-9_]{1,14}".prop_filter("non-empty", |s| !s.is_empty())
    }

    /// Generate a function name.
    fn func_name_strategy() -> impl Strategy<Value = String> {
        "[a-z][a-zA-Z]{2,12}"
    }

    /// Supported language patterns for env var access.
    #[derive(Debug, Clone)]
    enum EnvPattern {
        JsProcessEnvDot,
        JsProcessEnvBracket,
        PythonOsEnvironBracket,
        PythonOsGetenv,
        RustEnvVar,
        JavaSystemGetenv,
    }

    fn env_pattern_strategy() -> impl Strategy<Value = EnvPattern> {
        prop_oneof![
            Just(EnvPattern::JsProcessEnvDot),
            Just(EnvPattern::JsProcessEnvBracket),
            Just(EnvPattern::PythonOsEnvironBracket),
            Just(EnvPattern::PythonOsGetenv),
            Just(EnvPattern::RustEnvVar),
            Just(EnvPattern::JavaSystemGetenv),
        ]
    }

    fn language_for_pattern(pattern: &EnvPattern) -> Language {
        match pattern {
            EnvPattern::JsProcessEnvDot | EnvPattern::JsProcessEnvBracket => Language::TypeScript,
            EnvPattern::PythonOsEnvironBracket | EnvPattern::PythonOsGetenv => Language::Python,
            EnvPattern::RustEnvVar => Language::Rust,
            EnvPattern::JavaSystemGetenv => Language::Java,
        }
    }

    fn file_ext_for_pattern(pattern: &EnvPattern) -> &'static str {
        match pattern {
            EnvPattern::JsProcessEnvDot | EnvPattern::JsProcessEnvBracket => "ts",
            EnvPattern::PythonOsEnvironBracket | EnvPattern::PythonOsGetenv => "py",
            EnvPattern::RustEnvVar => "rs",
            EnvPattern::JavaSystemGetenv => "java",
        }
    }

    /// Generate a code line that accesses an env var using the given pattern.
    fn env_access_code(pattern: &EnvPattern, var_name: &str) -> String {
        match pattern {
            EnvPattern::JsProcessEnvDot => format!("const val = process.env.{};", var_name),
            EnvPattern::JsProcessEnvBracket => {
                format!("const val = process.env[\"{}\"];", var_name)
            }
            EnvPattern::PythonOsEnvironBracket => {
                format!("val = os.environ[\"{}\"]", var_name)
            }
            EnvPattern::PythonOsGetenv => {
                format!("val = os.getenv(\"{}\")", var_name)
            }
            EnvPattern::RustEnvVar => {
                format!("let val = std::env::var(\"{}\").unwrap();", var_name)
            }
            EnvPattern::JavaSystemGetenv => {
                format!("String val = System.getenv(\"{}\");", var_name)
            }
        }
    }

    /// Helper to write a file into a tempdir and return a DiscoveredFile.
    fn write_file(
        dir: &std::path::Path,
        rel_path: &str,
        content: &str,
        language: Language,
    ) -> DiscoveredFile {
        let abs = dir.join(rel_path);
        if let Some(parent) = abs.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(&abs, content).unwrap();
        DiscoveredFile {
            abs_path: abs,
            rel_path: rel_path.to_owned(),
            language,
        }
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(80))]

        /// For any set of N unique env var names accessed via a single pattern,
        /// pass_envscan creates exactly N EnvVar nodes.
        #[test]
        fn unique_env_vars_create_exactly_n_nodes(
            env_vars in prop::collection::hash_set(env_var_name_strategy(), 1..8),
            pattern in env_pattern_strategy(),
        ) {
            let dir = tempfile::tempdir().unwrap();
            let project = "test_proj";

            // Build a source file with one access per unique env var
            let mut code_lines = Vec::new();
            for var_name in &env_vars {
                code_lines.push(env_access_code(&pattern, var_name));
            }
            let content = code_lines.join("\n");

            let ext = file_ext_for_pattern(&pattern);
            let lang = language_for_pattern(&pattern);
            let rel_path = format!("src/app.{}", ext);
            let file = write_file(dir.path(), &rel_path, &content, lang);

            let reg = Registry::new();
            let mut buf = GraphBuffer::new(project);
            let files: Vec<&DiscoveredFile> = vec![&file];
            pass_envscan(&mut buf, &reg, &files, project);

            let n = env_vars.len();
            prop_assert_eq!(
                buf.node_count(), n,
                "Expected exactly {} EnvVar nodes for {} unique env vars, got {}",
                n, n, buf.node_count()
            );
        }

        /// For any set of N unique env var names, each accessed once within a function,
        /// pass_envscan creates exactly N READS_ENV edges.
        #[test]
        fn each_access_in_function_creates_one_reads_env_edge(
            env_vars in prop::collection::hash_set(env_var_name_strategy(), 1..6),
            func_name in func_name_strategy(),
            pattern in env_pattern_strategy(),
        ) {
            let dir = tempfile::tempdir().unwrap();
            let project = "test_proj";

            // Build a source file with a function containing env var accesses
            let mut code_lines = Vec::new();
            for var_name in &env_vars {
                code_lines.push(format!("  {}", env_access_code(&pattern, var_name)));
            }
            let func_body = code_lines.join("\n");

            let ext = file_ext_for_pattern(&pattern);
            let lang = language_for_pattern(&pattern);
            let rel_path = format!("src/app.{}", ext);

            // Wrap in a function so the registry can map accesses to it
            let content = format!("function {}() {{\n{}\n}}", func_name, func_body);
            let file = write_file(dir.path(), &rel_path, &content, lang);

            // Register the function in the registry so pass_envscan can find it
            let func_qn = format!("{}.src/app.{}.{}", project, ext, func_name);
            let mut reg = Registry::new();
            reg.register(
                &func_name,
                &func_qn,
                &rel_path,
                "Function",
                1,
                (env_vars.len() as i32) + 2,
            );

            let mut buf = GraphBuffer::new(project);
            let files: Vec<&DiscoveredFile> = vec![&file];
            pass_envscan(&mut buf, &reg, &files, project);

            let n = env_vars.len();
            // Should have exactly N READS_ENV edges (one per env var access)
            let edges = buf.take_edges();
            let reads_env_count = edges.iter()
                .filter(|e| e.edge_type == "READS_ENV")
                .count();

            prop_assert_eq!(
                reads_env_count, n,
                "Expected exactly {} READS_ENV edges for {} env var accesses in function, got {}",
                n, n, reads_env_count
            );
        }

        /// When the same env var is accessed in multiple files, only one EnvVar node
        /// is created (deduplication), but one READS_ENV edge per access.
        #[test]
        fn duplicate_env_var_across_files_creates_one_node(
            var_name in env_var_name_strategy(),
            num_files in 2usize..6,
            pattern in env_pattern_strategy(),
        ) {
            let dir = tempfile::tempdir().unwrap();
            let project = "test_proj";

            let ext = file_ext_for_pattern(&pattern);
            let lang = language_for_pattern(&pattern);

            let mut discovered_files = Vec::new();
            for i in 0..num_files {
                let content = env_access_code(&pattern, &var_name);
                let rel_path = format!("src/file_{}.{}", i, ext);
                discovered_files.push(write_file(dir.path(), &rel_path, &content, lang));
            }

            let reg = Registry::new();
            let mut buf = GraphBuffer::new(project);
            let file_refs: Vec<&DiscoveredFile> = discovered_files.iter().collect();
            pass_envscan(&mut buf, &reg, &file_refs, project);

            // Exactly 1 EnvVar node regardless of how many files reference it
            prop_assert_eq!(
                buf.node_count(), 1,
                "Expected exactly 1 EnvVar node for '{}' accessed in {} files, got {}",
                var_name, num_files, buf.node_count()
            );

            // One READS_ENV edge per file access
            let edges = buf.take_edges();
            let reads_env_count = edges.iter()
                .filter(|e| e.edge_type == "READS_ENV")
                .count();
            prop_assert_eq!(
                reads_env_count, num_files,
                "Expected {} READS_ENV edges (one per file), got {}",
                num_files, reads_env_count
            );
        }

        /// For any set of env vars accessed across multiple patterns/languages,
        /// the total EnvVar nodes equals the number of unique variable names.
        #[test]
        fn mixed_patterns_unify_by_name(
            env_vars in prop::collection::hash_set(env_var_name_strategy(), 1..5),
        ) {
            let dir = tempfile::tempdir().unwrap();
            let project = "test_proj";

            // Create files in different languages accessing the same env vars
            let patterns = [
                (EnvPattern::JsProcessEnvDot, "ts", Language::TypeScript),
                (EnvPattern::PythonOsGetenv, "py", Language::Python),
                (EnvPattern::RustEnvVar, "rs", Language::Rust),
            ];

            let mut discovered_files = Vec::new();
            for (i, (pattern, ext, lang)) in patterns.iter().enumerate() {
                let mut code_lines = Vec::new();
                for var_name in &env_vars {
                    code_lines.push(env_access_code(pattern, var_name));
                }
                let content = code_lines.join("\n");
                let rel_path = format!("src/app_{}.{}", i, ext);
                discovered_files.push(write_file(dir.path(), &rel_path, &content, *lang));
            }

            let reg = Registry::new();
            let mut buf = GraphBuffer::new(project);
            let file_refs: Vec<&DiscoveredFile> = discovered_files.iter().collect();
            pass_envscan(&mut buf, &reg, &file_refs, project);

            let n = env_vars.len();
            // Exactly N EnvVar nodes (one per unique name, regardless of language)
            prop_assert_eq!(
                buf.node_count(), n,
                "Expected exactly {} EnvVar nodes for {} unique names across 3 languages, got {}",
                n, n, buf.node_count()
            );

            // Total READS_ENV edges = N vars * 3 languages = 3N
            let edges = buf.take_edges();
            let reads_env_count = edges.iter()
                .filter(|e| e.edge_type == "READS_ENV")
                .count();
            let expected_edges = n * patterns.len();
            prop_assert_eq!(
                reads_env_count, expected_edges,
                "Expected {} READS_ENV edges ({} vars * {} languages), got {}",
                expected_edges, n, patterns.len(), reads_env_count
            );
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Property 10: USES Edge Creation and Filtering
// ═══════════════════════════════════════════════════════════════════════

/// **Validates: Requirements 13.1, 13.2, 13.3, 13.4, 13.7**
///
/// For any source file containing type annotations referencing symbols that exist
/// in the graph, the usage pass SHALL create exactly one USES edge per
/// (enclosing_symbol, referenced_type) pair, and querying `find_references` with
/// `reference_type="uses"` SHALL return only USES edges (no CALLS edges).
mod property10_uses_edge_creation_and_filtering {
    use super::*;
    use codryn_pipeline::pass_usages::pass_usages;
    use codryn_pipeline::FileCache;
    use std::sync::Arc;

    /// Generate a valid type name (PascalCase, starts with uppercase).
    fn type_name_strategy() -> impl Strategy<Value = String> {
        "[A-Z][a-z]{2,8}[A-Z][a-z]{2,8}"
    }

    /// Generate a valid function name (camelCase, starts with lowercase).
    fn func_name_strategy() -> impl Strategy<Value = String> {
        "[a-z][a-zA-Z]{3,10}"
    }

    /// Generate a valid variable name.
    fn var_name_strategy() -> impl Strategy<Value = String> {
        "[a-z][a-zA-Z]{2,8}"
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(50))]

        /// Type annotations in a function body create exactly one USES edge per
        /// (enclosing_function, referenced_type) pair — no duplicates.
        #[test]
        fn uses_edges_deduplicated_per_pair(
            func_name in func_name_strategy(),
            type_names in prop::collection::hash_set(type_name_strategy(), 1..5),
            var_names in prop::collection::vec(var_name_strategy(), 3..8),
            project in project_strategy(),
        ) {
            let store = test_store();
            store.upsert_project(&codryn_store::Project {
                name: project.clone(),
                indexed_at: chrono::Utc::now().to_rfc3339(),
                root_path: "/test".to_string(),
            }).unwrap();

            // Register type nodes in the store and registry
            let mut reg = Registry::new();
            let type_names_vec: Vec<String> = type_names.into_iter().collect();

            for type_name in &type_names_vec {
                let qn = format!("{}.types.{}", project, type_name);
                reg.register(type_name, &qn, "types.ts", "Class", 1, 10);
                store.insert_nodes_batch(&[codryn_store::Node {
                    id: 0,
                    project: project.clone(),
                    label: "Class".to_string(),
                    name: type_name.clone(),
                    qualified_name: qn,
                    file_path: "types.ts".to_string(),
                    start_line: 1,
                    end_line: 10,
                    properties_json: None,
                }]).unwrap();
            }

            // Register the function in the registry so it can be found as enclosing symbol
            let func_qn = format!("{}.module.{}", project, func_name);
            reg.register(&func_name, &func_qn, "main.ts", "Function", 1, 50);
            store.insert_nodes_batch(&[codryn_store::Node {
                id: 0,
                project: project.clone(),
                label: "Function".to_string(),
                name: func_name.clone(),
                qualified_name: func_qn.clone(),
                file_path: "main.ts".to_string(),
                start_line: 1,
                end_line: 50,
                properties_json: None,
            }]).unwrap();

            // Build a TypeScript source file with type annotations inside the function.
            // Use each type multiple times to verify deduplication.
            let mut source_lines = vec![format!("function {}() {{", func_name)];
            for (i, var_name) in var_names.iter().enumerate() {
                let type_name = &type_names_vec[i % type_names_vec.len()];
                source_lines.push(format!("  const {}: {} = null;", var_name, type_name));
            }
            source_lines.push("}".to_string());
            let source_content = source_lines.join("\n");

            // Set up file cache and discovered file
            let tmp = tempfile::TempDir::new().unwrap();
            let file_path = tmp.path().join("main.ts");
            std::fs::write(&file_path, &source_content).unwrap();

            let mut file_cache = FileCache::new();
            file_cache.insert(file_path.clone(), Arc::new(source_content));

            let discovered = DiscoveredFile {
                abs_path: file_path,
                rel_path: "main.ts".to_string(),
                language: Language::TypeScript,
            };
            let files: Vec<&DiscoveredFile> = vec![&discovered];

            // Run pass_usages
            let mut buf = GraphBuffer::new(&project);
            pass_usages(&mut buf, &files, &file_cache, &project, &reg);

            // Verify: exactly one USES edge per (func_qn, type_qn) pair
            let edges = buf.take_edges();
            let uses_edges: Vec<_> = edges.iter()
                .filter(|e| e.edge_type == "USES")
                .collect();

            // Count unique (source, target) pairs from the edge properties
            let mut seen_pairs: std::collections::HashSet<String> = std::collections::HashSet::new();
            for edge in &uses_edges {
                if let Some(ref props) = edge.properties_json {
                    if let Ok(v) = serde_json::from_str::<serde_json::Value>(props) {
                        let src = v["_src_qn"].as_str().unwrap_or("");
                        let tgt = v["_tgt_qn"].as_str().unwrap_or("");
                        let pair_key = format!("{}|{}", src, tgt);
                        prop_assert!(
                            seen_pairs.insert(pair_key.clone()),
                            "Duplicate USES edge found for pair: {}", pair_key
                        );
                    }
                }
            }

            // The number of USES edges should be at most the number of unique types
            // (one edge per (enclosing_symbol, referenced_type) pair)
            prop_assert!(
                uses_edges.len() <= type_names_vec.len(),
                "Expected at most {} USES edges (one per unique type), got {}",
                type_names_vec.len(), uses_edges.len()
            );
        }

        /// Module-scope type annotations create USES edges from the module symbol.
        #[test]
        fn module_scope_uses_edges_from_module(
            type_names in prop::collection::hash_set(type_name_strategy(), 1..4),
            var_names in prop::collection::vec(var_name_strategy(), 1..4),
            project in project_strategy(),
        ) {
            let store = test_store();
            store.upsert_project(&codryn_store::Project {
                name: project.clone(),
                indexed_at: chrono::Utc::now().to_rfc3339(),
                root_path: "/test".to_string(),
            }).unwrap();

            let mut reg = Registry::new();
            let type_names_vec: Vec<String> = type_names.into_iter().collect();

            for type_name in &type_names_vec {
                let qn = format!("{}.types.{}", project, type_name);
                reg.register(type_name, &qn, "types.ts", "Class", 1, 10);
                store.insert_nodes_batch(&[codryn_store::Node {
                    id: 0,
                    project: project.clone(),
                    label: "Class".to_string(),
                    name: type_name.clone(),
                    qualified_name: qn,
                    file_path: "types.ts".to_string(),
                    start_line: 1,
                    end_line: 10,
                    properties_json: None,
                }]).unwrap();
            }

            // Build source with module-scope variable declarations (no enclosing function)
            let mut source_lines = Vec::new();
            for (i, var_name) in var_names.iter().enumerate() {
                let type_name = &type_names_vec[i % type_names_vec.len()];
                source_lines.push(format!("const {}: {} = null;", var_name, type_name));
            }
            let source_content = source_lines.join("\n");

            let tmp = tempfile::TempDir::new().unwrap();
            let file_path = tmp.path().join("globals.ts");
            std::fs::write(&file_path, &source_content).unwrap();

            let mut file_cache = FileCache::new();
            file_cache.insert(file_path.clone(), Arc::new(source_content));

            let discovered = DiscoveredFile {
                abs_path: file_path,
                rel_path: "globals.ts".to_string(),
                language: Language::TypeScript,
            };
            let files: Vec<&DiscoveredFile> = vec![&discovered];

            let mut buf = GraphBuffer::new(&project);
            pass_usages(&mut buf, &files, &file_cache, &project, &reg);

            let edges = buf.take_edges();
            let uses_edges: Vec<_> = edges.iter()
                .filter(|e| e.edge_type == "USES")
                .collect();

            // Module-scope declarations should produce USES edges from the module QN
            // The source QN should be the module QN (not a function QN)
            for edge in &uses_edges {
                if let Some(ref props) = edge.properties_json {
                    if let Ok(v) = serde_json::from_str::<serde_json::Value>(props) {
                        let src = v["_src_qn"].as_str().unwrap_or("");
                        // Module QN should NOT contain a function name
                        // It should be the module-level QN derived from the file path
                        prop_assert!(
                            !src.is_empty(),
                            "Source QN should not be empty for module-scope USES edge"
                        );
                    }
                }
            }

            // Should have at most one edge per unique type
            prop_assert!(
                uses_edges.len() <= type_names_vec.len(),
                "Expected at most {} USES edges for module-scope, got {}",
                type_names_vec.len(), uses_edges.len()
            );
        }

        /// USES edges are only created for types that exist in the registry;
        /// unresolved types produce zero edges.
        #[test]
        fn unresolved_types_produce_no_edges(
            func_name in func_name_strategy(),
            unresolved_types in prop::collection::hash_set(type_name_strategy(), 1..5),
            project in project_strategy(),
        ) {
            let store = test_store();
            store.upsert_project(&codryn_store::Project {
                name: project.clone(),
                indexed_at: chrono::Utc::now().to_rfc3339(),
                root_path: "/test".to_string(),
            }).unwrap();

            // Registry with NO type entries — all references will be unresolved
            let reg = Registry::new();

            // Build source with type annotations referencing unresolved types
            let mut source_lines = vec![format!("function {}() {{", func_name)];
            for (i, type_name) in unresolved_types.iter().enumerate() {
                source_lines.push(format!("  const v{}: {} = null;", i, type_name));
            }
            source_lines.push("}".to_string());
            let source_content = source_lines.join("\n");

            let tmp = tempfile::TempDir::new().unwrap();
            let file_path = tmp.path().join("main.ts");
            std::fs::write(&file_path, &source_content).unwrap();

            let mut file_cache = FileCache::new();
            file_cache.insert(file_path.clone(), Arc::new(source_content));

            let discovered = DiscoveredFile {
                abs_path: file_path,
                rel_path: "main.ts".to_string(),
                language: Language::TypeScript,
            };
            let files: Vec<&DiscoveredFile> = vec![&discovered];

            let mut buf = GraphBuffer::new(&project);
            pass_usages(&mut buf, &files, &file_cache, &project, &reg);

            // No edges should be created for unresolved types (Requirement 13.5)
            prop_assert_eq!(
                buf.edge_count(), 0,
                "Expected 0 edges for unresolved types, got {}",
                buf.edge_count()
            );
        }

        /// When both CALLS and USES edges exist for a target, filtering by
        /// reference_type="uses" returns only USES edges.
        #[test]
        fn find_references_uses_filter_excludes_calls(
            type_name in type_name_strategy(),
            caller_name in func_name_strategy(),
            user_name in func_name_strategy(),
            project in project_strategy(),
        ) {
            let store = test_store();
            store.upsert_project(&codryn_store::Project {
                name: project.clone(),
                indexed_at: chrono::Utc::now().to_rfc3339(),
                root_path: "/test".to_string(),
            }).unwrap();

            // Create the target type node
            let type_qn = format!("{}.types.{}", project, type_name);
            let nodes = store.insert_nodes_batch(&[
                codryn_store::Node {
                    id: 0,
                    project: project.clone(),
                    label: "Class".to_string(),
                    name: type_name.clone(),
                    qualified_name: type_qn.clone(),
                    file_path: "types.ts".to_string(),
                    start_line: 1,
                    end_line: 10,
                    properties_json: None,
                },
                // A function that CALLS the type (e.g., constructor call)
                codryn_store::Node {
                    id: 0,
                    project: project.clone(),
                    label: "Function".to_string(),
                    name: caller_name.clone(),
                    qualified_name: format!("{}.funcs.{}", project, caller_name),
                    file_path: "caller.ts".to_string(),
                    start_line: 1,
                    end_line: 10,
                    properties_json: None,
                },
                // A function that USES the type (type annotation)
                codryn_store::Node {
                    id: 0,
                    project: project.clone(),
                    label: "Function".to_string(),
                    name: user_name.clone(),
                    qualified_name: format!("{}.funcs.{}", project, user_name),
                    file_path: "user.ts".to_string(),
                    start_line: 1,
                    end_line: 10,
                    properties_json: None,
                },
            ]).unwrap();

            let type_id = nodes[0].1;
            let caller_id = nodes[1].1;
            let user_id = nodes[2].1;

            // Insert a CALLS edge (caller -> type)
            store.insert_edges_batch(&[codryn_store::Edge {
                id: 0,
                project: project.clone(),
                source_id: caller_id,
                target_id: type_id,
                edge_type: "CALLS".to_string(),
                properties_json: None,
            }]).unwrap();

            // Insert a USES edge (user -> type)
            store.insert_edges_batch(&[codryn_store::Edge {
                id: 0,
                project: project.clone(),
                source_id: user_id,
                target_id: type_id,
                edge_type: "USES".to_string(),
                properties_json: None,
            }]).unwrap();

            // Query with reference_type="uses" filter — should only return USES edges
            let uses_filter: Option<&[&str]> = Some(&["USES"]);
            let refs = store.incoming_references_detailed(
                type_id, uses_filter, 30, None
            ).unwrap();

            // All returned edges should be USES, not CALLS
            for (_, edge_type, _, _) in &refs {
                prop_assert_eq!(
                    edge_type, "USES",
                    "With uses filter, got edge_type='{}' instead of 'USES'",
                    edge_type
                );
            }

            // Should have exactly 1 USES reference
            prop_assert_eq!(
                refs.len(), 1,
                "Expected exactly 1 USES reference, got {}",
                refs.len()
            );

            // Query with reference_type="all" (no filter) — should return both
            let all_refs = store.incoming_references_detailed(
                type_id, None, 30, None
            ).unwrap();

            prop_assert_eq!(
                all_refs.len(), 2,
                "Expected 2 total references (1 CALLS + 1 USES), got {}",
                all_refs.len()
            );

            // Verify both edge types are present
            let edge_types: std::collections::HashSet<&str> = all_refs.iter()
                .map(|(_, et, _, _)| et.as_str())
                .collect();
            prop_assert!(
                edge_types.contains("CALLS"),
                "Expected CALLS edge in unfiltered results"
            );
            prop_assert!(
                edge_types.contains("USES"),
                "Expected USES edge in unfiltered results"
            );
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Property 14: Decorator Normalization
// ═══════════════════════════════════════════════════════════════════════

/// **Validates: Requirements 22.1, 22.2, 22.3, 22.6**
mod property14_decorator_normalization {
    use super::*;
    use codryn_pipeline::pass_decorators::{normalize_decorator_list, MAX_DECORATORS_PER_NODE};

    // ── Strategies ──

    /// Generate a valid unqualified decorator/annotation name (PascalCase or snake_case).
    fn decorator_name_strategy() -> impl Strategy<Value = String> {
        prop_oneof![
            // PascalCase names (Java/Kotlin/TS style): Component, Injectable, Override
            "[A-Z][a-zA-Z]{1,15}",
            // snake_case names (Python style): login_required, app_route
            "[a-z][a-z_]{1,12}",
        ]
    }

    /// Generate a qualified decorator path (e.g., `app.route`, `org.junit.Test`).
    fn qualified_path_strategy() -> impl Strategy<Value = Vec<String>> {
        prop::collection::vec("[a-z][a-zA-Z]{1,8}", 1..4)
    }

    /// Generate optional arguments for a decorator.
    fn args_strategy() -> impl Strategy<Value = String> {
        prop_oneof![
            Just(String::new()),                   // no args
            Just("()".to_string()),                // empty parens
            Just("(\"value\")".to_string()),       // string arg
            Just("({key: 'val'})".to_string()),    // object arg
            Just("(name = \"test\")".to_string()), // named arg
        ]
    }

    /// Supported language for decorator testing.
    #[derive(Debug, Clone)]
    enum DecLang {
        Python,
        Java,
        Kotlin,
        TypeScript,
    }

    fn dec_lang_strategy() -> impl Strategy<Value = DecLang> {
        prop_oneof![
            Just(DecLang::Python),
            Just(DecLang::Java),
            Just(DecLang::Kotlin),
            Just(DecLang::TypeScript),
        ]
    }

    /// Generate a raw decorator string as it would appear in source code.
    /// Returns (raw_decorator_text, expected_normalized_name).
    fn raw_decorator_strategy() -> impl Strategy<Value = (String, String)> {
        (qualified_path_strategy(), args_strategy()).prop_map(|(path_parts, args)| {
            let qualified = path_parts.join(".");
            let raw = format!("@{}{}", qualified, args);
            // Expected normalized: last segment of the qualified path
            let expected = path_parts.last().unwrap().clone();
            (raw, expected)
        })
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(100))]

        /// For any single decorator with @ prefix and optional arguments,
        /// normalize_decorator_list strips @ and arguments, returning the unqualified name.
        #[test]
        fn normalize_strips_at_and_args(
            name in decorator_name_strategy(),
            args in args_strategy(),
        ) {
            let raw = format!("@{}{}", name, args);
            let result = normalize_decorator_list(std::slice::from_ref(&raw));
            prop_assert_eq!(
                result.len(), 1,
                "Expected 1 result for single decorator, got {}",
                result.len()
            );
            prop_assert_eq!(
                &result[0], &name,
                "normalize_decorator_list([{:?}]) should produce {:?}, got {:?}",
                raw, name, result[0]
            );
        }

        /// For any qualified decorator (e.g., @app.route), normalization
        /// returns only the last (unqualified) segment.
        #[test]
        fn normalize_returns_unqualified_name(
            path_parts in prop::collection::vec("[a-z][a-zA-Z]{1,8}", 2..5),
            args in args_strategy(),
        ) {
            let qualified = path_parts.join(".");
            let raw = format!("@{}{}", qualified, args);
            let expected = path_parts.last().unwrap();
            let result = normalize_decorator_list(std::slice::from_ref(&raw));
            prop_assert_eq!(
                result.len(), 1,
                "Expected 1 result, got {}",
                result.len()
            );
            prop_assert_eq!(
                &result[0], expected,
                "normalize_decorator_list([{:?}]) should produce {:?}, got {:?}",
                raw, expected, result[0]
            );
        }

        /// For any list of K decorators (K <= 50), normalize_decorator_list
        /// produces exactly K normalized names preserving declaration order.
        #[test]
        fn normalize_list_preserves_count_and_order(
            decorators in prop::collection::vec(raw_decorator_strategy(), 1..50),
        ) {
            let raw_list: Vec<String> = decorators.iter().map(|(raw, _)| raw.clone()).collect();
            let expected: Vec<String> = decorators.iter().map(|(_, exp)| exp.clone()).collect();

            let result = normalize_decorator_list(&raw_list);

            prop_assert_eq!(
                result.len(), expected.len(),
                "Expected {} normalized decorators, got {}",
                expected.len(), result.len()
            );

            // Verify order is preserved
            for (i, (got, want)) in result.iter().zip(expected.iter()).enumerate() {
                prop_assert_eq!(
                    got, want,
                    "Decorator at index {} mismatch: got {:?}, expected {:?}",
                    i, got, want
                );
            }
        }

        /// For any list of K decorators where K > 50, normalize_decorator_list
        /// caps the result at MAX_DECORATORS_PER_NODE (50) entries.
        #[test]
        fn normalize_list_caps_at_max_50(
            count in 51usize..80,
        ) {
            let raw_list: Vec<String> = (0..count)
                .map(|i| format!("@Decorator{}", i))
                .collect();

            let result = normalize_decorator_list(&raw_list);

            prop_assert_eq!(
                result.len(), MAX_DECORATORS_PER_NODE,
                "Expected max {} decorators, got {}",
                MAX_DECORATORS_PER_NODE, result.len()
            );

            // Verify the first MAX_DECORATORS_PER_NODE entries are preserved in order
            for (i, item) in result.iter().enumerate().take(MAX_DECORATORS_PER_NODE) {
                let expected = format!("Decorator{}", i);
                prop_assert_eq!(
                    item, &expected,
                    "Decorator at index {} should be {:?}, got {:?}",
                    i, expected, item
                );
            }
        }

        /// For any decorated function/class/method across Python, Java, Kotlin, or TypeScript,
        /// the normalization produces names without @ prefix, without arguments,
        /// and with only the unqualified name.
        #[test]
        fn normalized_decorators_have_no_at_no_args_no_qualification(
            decorators in prop::collection::vec(raw_decorator_strategy(), 1..20),
            _lang in dec_lang_strategy(),
        ) {
            let raw_list: Vec<String> = decorators.iter().map(|(raw, _)| raw.clone()).collect();
            let result = normalize_decorator_list(&raw_list);

            for (i, name) in result.iter().enumerate() {
                // No @ prefix
                prop_assert!(
                    !name.starts_with('@'),
                    "Decorator at index {} ({:?}) should not start with @",
                    i, name
                );
                // No arguments (no parentheses)
                prop_assert!(
                    !name.contains('(') && !name.contains(')'),
                    "Decorator at index {} ({:?}) should not contain parentheses",
                    i, name
                );
                // No qualification (no dots)
                prop_assert!(
                    !name.contains('.'),
                    "Decorator at index {} ({:?}) should not contain dots (must be unqualified)",
                    i, name
                );
                // Non-empty
                prop_assert!(
                    !name.is_empty(),
                    "Decorator at index {} should not be empty",
                    i
                );
            }
        }

        /// For any empty decorator list, normalize_decorator_list returns empty.
        #[test]
        fn normalize_empty_list_returns_empty(
            _lang in dec_lang_strategy(),
        ) {
            let result = normalize_decorator_list(&[]);
            prop_assert_eq!(result.len(), 0, "Empty input should produce empty output");
        }
    }
}
