use codryn_store::Store;
use proptest::prelude::*;
use std::collections::HashMap;

fn test_store() -> Store {
    Store::open_in_memory().unwrap()
}

fn source_strategy() -> impl Strategy<Value = String> {
    prop_oneof![Just("mcp".to_string()), Just("ui".to_string())]
}

/// **Validates: Requirements 4.1, 5.1, 6.1, 6.4**
/// Property 1: Tool call round-trip preservation
/// For any valid tool call, logging it and retrieving it via get_tool_analytics
/// should return a record where all fields match the original values.
mod property1_round_trip {
    use super::*;

    proptest! {
        #[test]
        fn tool_call_round_trip(
            tool_name in "[a-z_]{1,20}",
            project in "[a-z_]{1,20}",
            source in source_strategy(),
            agent_name in "[a-z_]{1,20}",
            model_name in "[a-z_]{1,20}",
            input_tokens in 0i64..10000,
            output_tokens in 0i64..10000,
        ) {
            let store = test_store();
            store.log_tool_call(
                &tool_name, &project, &source,
                42, true,
                &agent_name, &model_name,
                input_tokens, output_tokens, 0,
                "", "",
            ).unwrap();

            let analytics = store.get_tool_analytics(100).unwrap();

            if source == "mcp" {
                prop_assert_eq!(analytics.recent.len(), 1);

                let record = &analytics.recent[0];
                prop_assert_eq!(&record.tool_name, &tool_name);
                prop_assert_eq!(&record.project, &project);
                prop_assert_eq!(&record.source, &source);
                prop_assert_eq!(&record.agent_name, &agent_name);
                prop_assert_eq!(&record.model_name, &model_name);
                prop_assert_eq!(record.input_tokens, input_tokens);
                prop_assert_eq!(record.output_tokens, output_tokens);
                prop_assert_eq!(record.success, true);
                prop_assert_eq!(record.duration_ms, 42);
            } else {
                // UI calls are excluded from analytics
                prop_assert_eq!(analytics.recent.len(), 0);
                prop_assert_eq!(analytics.total_calls, 0);
            }
        }
    }
}

/// **Validates: Requirements 2.2, 2.3**
/// Property 2: Per-tool source breakdown correctness
/// For any set of tool calls with varying sources, mcp_count and ui_count
/// in each ToolCount should equal the actual count of records with that source.
mod property2_source_breakdown {
    use super::*;

    fn tool_name_strategy() -> impl Strategy<Value = String> {
        prop_oneof![
            Just("search_graph".to_string()),
            Just("query_graph".to_string()),
            Just("index_repository".to_string()),
            Just("list_projects".to_string()),
        ]
    }

    proptest! {
        #[test]
        fn per_tool_source_breakdown(
            calls in prop::collection::vec(
                (tool_name_strategy(), source_strategy()),
                1..30
            )
        ) {
            let store = test_store();

            // Track expected counts
            let mut expected: HashMap<String, (i64, i64)> = HashMap::new();
            for (tool, source) in &calls {
                let entry = expected.entry(tool.clone()).or_insert((0, 0));
                if source == "mcp" {
                    entry.0 += 1;
                } else {
                    entry.1 += 1;
                }
            }

            // Log all calls
            for (tool, source) in &calls {
                store.log_tool_call(
                    tool, "proj", source,
                    10, true,
                    "unknown", "unknown", 0, 0, 0,
                    "", "",
                ).unwrap();
            }

            let analytics = store.get_tool_analytics(100).unwrap();

            for tc in &analytics.per_tool {
                if let Some(&(exp_mcp, _exp_ui)) = expected.get(&tc.tool_name) {
                    prop_assert_eq!(
                        tc.mcp_count, exp_mcp,
                        "mcp_count mismatch for tool {}", tc.tool_name
                    );
                    prop_assert_eq!(
                        tc.ui_count, 0,
                        "ui_count should always be 0 for tool {}", tc.tool_name
                    );
                }
            }
        }
    }
}

/// **Validates: Requirements 3.4, 8.4**
/// Property 3: Recent records cap at 100
/// For any number N of tool call records inserted, get_tool_analytics(100)
/// should return at most 100 recent records, ordered by most recent first.
mod property3_recent_cap {
    use super::*;

    proptest! {
        #[test]
        fn recent_records_capped_at_100(n in 0usize..200) {
            let store = test_store();

            for i in 0..n {
                store.log_tool_call(
                    &format!("tool_{}", i % 5), "proj", "mcp",
                    10, true,
                    "agent", "model", 0, 0, 0,
                    "", "",
                ).unwrap();
            }

            let analytics = store.get_tool_analytics(100).unwrap();
            prop_assert!(
                analytics.recent.len() <= 100,
                "recent.len() = {} exceeds 100", analytics.recent.len()
            );

            if n > 0 {
                prop_assert!(
                    !analytics.recent.is_empty(),
                    "expected non-empty recent for n={}", n
                );
                // Verify ordering: IDs should be descending (most recent first)
                for w in analytics.recent.windows(2) {
                    prop_assert!(
                        w[0].id > w[1].id,
                        "records not in descending id order: {} vs {}", w[0].id, w[1].id
                    );
                }
            }
        }
    }
}

/// **Validates: Requirements 4.5, 5.5, 7.3**
/// Property 5: Aggregation correctness for agent and model groupings
/// For any set of tool calls with varying agent_name and model_name,
/// per_agent and per_model counts should sum to total_calls.
mod property5_aggregation {
    use super::*;

    fn agent_strategy() -> impl Strategy<Value = String> {
        prop_oneof![
            Just("kiro".to_string()),
            Just("copilot".to_string()),
            Just("claude".to_string()),
            Just("unknown".to_string()),
        ]
    }

    fn model_strategy() -> impl Strategy<Value = String> {
        prop_oneof![
            Just("claude-opus-4".to_string()),
            Just("gpt-4o".to_string()),
            Just("unknown".to_string()),
        ]
    }

    proptest! {
        #[test]
        fn aggregation_sums_match_total(
            calls in prop::collection::vec(
                (agent_strategy(), model_strategy()),
                1..50
            )
        ) {
            let store = test_store();

            let mut expected_agents: HashMap<String, i64> = HashMap::new();
            let mut expected_models: HashMap<String, i64> = HashMap::new();

            for (agent, model) in &calls {
                *expected_agents.entry(agent.clone()).or_insert(0) += 1;
                *expected_models.entry(model.clone()).or_insert(0) += 1;
            }

            for (agent, model) in &calls {
                store.log_tool_call(
                    "some_tool", "proj", "mcp",
                    10, true,
                    agent, model, 0, 0, 0,
                    "", "",
                ).unwrap();
            }

            let analytics = store.get_tool_analytics(100).unwrap();

            // per_agent counts should sum to total_calls
            let agent_sum: i64 = analytics.per_agent.iter().map(|a| a.count).sum();
            prop_assert_eq!(
                agent_sum, analytics.total_calls,
                "per_agent sum {} != total_calls {}", agent_sum, analytics.total_calls
            );

            // per_model counts should sum to total_calls
            let model_sum: i64 = analytics.per_model.iter().map(|m| m.count).sum();
            prop_assert_eq!(
                model_sum, analytics.total_calls,
                "per_model sum {} != total_calls {}", model_sum, analytics.total_calls
            );

            // Verify individual agent counts
            for ac in &analytics.per_agent {
                if let Some(&expected) = expected_agents.get(&ac.agent_name) {
                    prop_assert_eq!(
                        ac.count, expected,
                        "agent {} count mismatch", ac.agent_name
                    );
                }
            }

            // Verify individual model counts
            for mc in &analytics.per_model {
                if let Some(&expected) = expected_models.get(&mc.model_name) {
                    prop_assert_eq!(
                        mc.count, expected,
                        "model {} count mismatch", mc.model_name
                    );
                }
            }
        }
    }
}

/// **Validates: Requirements 7.1, 7.2**
/// Property 6: Migration preserves existing records
/// Insert a call using the old schema (no new columns), run migration,
/// verify defaults are applied correctly.
mod property6_migration {
    use super::*;
    use rusqlite::Connection;

    fn create_old_schema_store() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        // Create tool_calls with old schema (no agent/model/token columns)
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS tool_calls (\
                id INTEGER PRIMARY KEY AUTOINCREMENT,\
                tool_name TEXT NOT NULL,\
                project TEXT DEFAULT '',\
                source TEXT DEFAULT 'ui',\
                duration_ms INTEGER DEFAULT 0,\
                success INTEGER DEFAULT 1,\
                called_at TEXT NOT NULL\
            );",
        )
        .unwrap();
        conn
    }

    proptest! {
        #[test]
        fn migration_preserves_records_with_defaults(
            tool_name in "[a-z_]{1,20}",
            project in "[a-z_]{1,20}",
            source in source_strategy(),
        ) {
            let conn = create_old_schema_store();

            // Insert a record using old schema
            conn.execute(
                "INSERT INTO tool_calls (tool_name, project, source, duration_ms, success, called_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                rusqlite::params![tool_name, project, source, 100, 1, "2025-01-01T00:00:00Z"],
            ).unwrap();

            // Run migration (same function used by Store::init_schema)
            codryn_store::schema_migrate_tool_calls(&conn);

            // Read back the record and verify defaults
            let (agent_name, model_name, input_tokens, output_tokens): (String, String, i64, i64) =
                conn.query_row(
                    "SELECT agent_name, model_name, input_tokens, output_tokens FROM tool_calls WHERE id = 1",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
                ).unwrap();

            prop_assert_eq!(&agent_name, "unknown");
            prop_assert_eq!(&model_name, "unknown");
            prop_assert_eq!(input_tokens, 0);
            prop_assert_eq!(output_tokens, 0);

            // Verify original fields are preserved
            let (read_tool, read_project, read_source): (String, String, String) =
                conn.query_row(
                    "SELECT tool_name, project, source FROM tool_calls WHERE id = 1",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                ).unwrap();

            prop_assert_eq!(&read_tool, &tool_name);
            prop_assert_eq!(&read_project, &project);
            prop_assert_eq!(&read_source, &source);
        }
    }
}

/// **Validates: Requirements 10.2, 10.4**
/// Property 13: BM25 ranking orders more relevant results higher
/// Generate random indexed documents and multi-term queries.
/// Assert documents matching more terms rank higher than those matching fewer.
mod property13_bm25_ranking {
    use super::*;

    /// Strategy for generating a single keyword (2-6 lowercase alpha chars).
    fn keyword_strategy() -> impl Strategy<Value = String> {
        "[a-z]{2,6}"
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(100))]
        #[test]
        fn bm25_ranks_more_matching_terms_higher(
            // Generate 3 distinct keywords for the query
            kw1 in keyword_strategy(),
            kw2 in keyword_strategy(),
            kw3 in keyword_strategy(),
        ) {
            // Skip if keywords collide (need distinct terms)
            prop_assume!(kw1 != kw2 && kw2 != kw3 && kw1 != kw3);
            // Skip if any keyword is a stop word
            let stop_words: &[&str] = &[
                "a", "an", "the", "is", "are", "was", "were", "be", "been", "being",
                "have", "has", "had", "do", "does", "did", "will", "would", "could",
                "should", "may", "might", "shall", "can", "need", "must",
                "in", "on", "at", "to", "for", "of", "with", "by", "from", "as",
                "into", "through", "during", "before", "after", "above", "below",
                "and", "or", "not", "but", "if", "then", "else", "when", "where",
                "this", "that", "these", "those", "it", "its",
            ];
            prop_assume!(!stop_words.contains(&kw1.as_str()));
            prop_assume!(!stop_words.contains(&kw2.as_str()));
            prop_assume!(!stop_words.contains(&kw3.as_str()));

            let store = test_store();
            store.upsert_project(&codryn_store::Project {
                name: "p".into(),
                indexed_at: "now".into(),
                root_path: "/".into(),
            }).unwrap();

            // Doc A matches all 3 keywords
            let qn_a = "p.src.docA".to_string();
            store.insert_node(&codryn_store::Node {
                id: 0, project: "p".into(), label: "Function".into(),
                name: "docA".into(), qualified_name: qn_a.clone(),
                file_path: "src/a.ts".into(), start_line: 1, end_line: 10,
                properties_json: None,
            }).unwrap();

            // Doc B matches only 1 keyword
            let qn_b = "p.src.docB".to_string();
            store.insert_node(&codryn_store::Node {
                id: 0, project: "p".into(), label: "Function".into(),
                name: "docB".into(), qualified_name: qn_b.clone(),
                file_path: "src/b.ts".into(), start_line: 1, end_line: 10,
                properties_json: None,
            }).unwrap();

            // Content: docA has all 3 keywords, docB has only kw1
            let content_a = format!("impl {} {} {} handler logic", kw1, kw2, kw3);
            let content_b = format!("impl {} handler logic only", kw1);

            store.upsert_code_content_batch(&[
                ("p".into(), qn_a.clone(), content_a),
                ("p".into(), qn_b.clone(), content_b),
            ]).unwrap();

            let query = format!("{} {} {}", kw1, kw2, kw3);
            let results = store.search_code_fts_bm25("p", &query, 10).unwrap();

            // Both docs should appear (at least docA which matches all terms)
            prop_assert!(!results.is_empty(), "BM25 should return results");

            if results.len() >= 2 {
                // The doc matching more terms (docA) should rank higher
                // BM25 scores are negative; more negative = better
                let first_qn = &results[0].0.qualified_name;
                let second_qn = &results[1].0.qualified_name;
                prop_assert_eq!(
                    first_qn, &qn_a,
                    "doc matching all 3 terms should rank first, got first={} second={}",
                    first_qn, second_qn
                );
                prop_assert!(
                    results[0].1 <= results[1].1,
                    "first result BM25 score {} should be <= second {} (more negative = better)",
                    results[0].1, results[1].1
                );
            }
        }
    }
}

/// **Validates: Requirements 3.5**
/// Property 3 (index-speed-optimization): Bulk Mode Pragma Round-Trip
/// For any Store instance, enabling bulk indexing mode and then disabling it
/// SHALL restore all SQLite pragmas (foreign_keys, temp_store, mmap_size,
/// wal_autocheckpoint) to their pre-bulk-mode values. This holds regardless
/// of how many enable/disable cycles are performed.
mod property3_bulk_mode_pragma_round_trip {
    use super::*;

    /// Helper to read a single pragma value from the store connection.
    /// Uses prepare + query to handle pragmas that may return empty result sets.
    fn read_pragma(store: &Store, pragma: &str) -> Option<i64> {
        let conn = store.conn();
        let mut stmt = conn.prepare(&format!("PRAGMA {}", pragma)).unwrap();
        let mut rows = stmt.query([]).unwrap();
        rows.next().unwrap().map(|row| row.get(0).unwrap())
    }

    /// Helper to read current pragma values from the store connection.
    /// Returns (foreign_keys, temp_store, mmap_size, wal_autocheckpoint).
    fn read_pragmas(store: &Store) -> (Option<i64>, Option<i64>, Option<i64>, Option<i64>) {
        (
            read_pragma(store, "foreign_keys"),
            read_pragma(store, "temp_store"),
            read_pragma(store, "mmap_size"),
            read_pragma(store, "wal_autocheckpoint"),
        )
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(100))]
        #[test]
        fn bulk_mode_pragma_round_trip(cycles in 1u32..20) {
            let store = test_store();

            // Record initial pragma values before any bulk mode operations
            let initial_pragmas = read_pragmas(&store);

            // Verify initial state has foreign_keys ON (1)
            prop_assert_eq!(initial_pragmas.0, Some(1), "foreign_keys should start ON");

            for _ in 0..cycles {
                // Enable bulk indexing mode
                store.enable_bulk_indexing_mode().unwrap();

                // Verify pragmas changed to bulk values
                let bulk_pragmas = read_pragmas(&store);
                prop_assert_eq!(bulk_pragmas.0, Some(0), "foreign_keys should be OFF in bulk mode");
                if let Some(ts) = bulk_pragmas.1 {
                    prop_assert_eq!(ts, 2, "temp_store should be MEMORY (2) in bulk mode");
                }
                if let Some(mmap) = bulk_pragmas.2 {
                    prop_assert_eq!(mmap, 268435456, "mmap_size should be 256MB in bulk mode");
                }
                if let Some(wal_ac) = bulk_pragmas.3 {
                    prop_assert_eq!(wal_ac, 0, "wal_autocheckpoint should be 0 in bulk mode");
                }

                // Disable bulk indexing mode
                store.disable_bulk_indexing_mode().unwrap();

                // Verify all pragmas are restored to pre-bulk values
                let restored_pragmas = read_pragmas(&store);
                prop_assert_eq!(
                    restored_pragmas.0, initial_pragmas.0,
                    "foreign_keys not restored after cycle"
                );
                prop_assert_eq!(
                    restored_pragmas.1, initial_pragmas.1,
                    "temp_store not restored after cycle"
                );
                prop_assert_eq!(
                    restored_pragmas.2, initial_pragmas.2,
                    "mmap_size not restored after cycle"
                );
                prop_assert_eq!(
                    restored_pragmas.3, initial_pragmas.3,
                    "wal_autocheckpoint not restored after cycle"
                );
            }
        }
    }
}

/// **Validates: Requirements 3.7**
/// Property 4 (index-speed-optimization): Data Integrity After Bulk Mode
/// For any set of nodes and edges inserted while bulk indexing mode is enabled,
/// after disabling bulk indexing mode and re-enabling foreign key checks,
/// `PRAGMA foreign_key_check` SHALL report zero violations, and the UNIQUE
/// constraints on `nodes(project, qualified_name)` and `edges(source_id, target_id, type)`
/// SHALL hold.
mod property4_data_integrity_after_bulk_mode {
    use super::*;

    /// Strategy for generating a valid edge type.
    fn edge_type_strategy() -> impl Strategy<Value = String> {
        prop_oneof![
            Just("CALLS".to_string()),
            Just("IMPORTS".to_string()),
            Just("INHERITS".to_string()),
            Just("IMPLEMENTS".to_string()),
            Just("USES".to_string()),
        ]
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(100))]
        #[test]
        fn data_integrity_after_bulk_mode(
            node_count in 2usize..20,
            edge_indices in prop::collection::vec((0usize..20, 0usize..20, edge_type_strategy()), 1..15),
        ) {
            let store = test_store();
            store.upsert_project(&codryn_store::Project {
                name: "p".into(),
                indexed_at: "now".into(),
                root_path: "/".into(),
            }).unwrap();

            // Enable bulk indexing mode
            store.enable_bulk_indexing_mode().unwrap();

            // Generate and insert nodes
            let mut nodes = Vec::new();
            for i in 0..node_count {
                nodes.push(codryn_store::Node {
                    id: 0,
                    project: "p".into(),
                    label: "Function".into(),
                    name: format!("func_{}", i),
                    qualified_name: format!("p.src.func_{}", i),
                    file_path: format!("src/file_{}.ts", i),
                    start_line: 1,
                    end_line: 10,
                    properties_json: None,
                });
            }
            let results = store.insert_nodes_batch(&nodes).unwrap();
            let node_ids: Vec<i64> = results.iter().map(|(_, id)| *id).collect();

            // Generate and insert edges (only between valid node indices)
            let mut edges = Vec::new();
            for (src_idx, tgt_idx, edge_type) in &edge_indices {
                let src = src_idx % node_count;
                let tgt = tgt_idx % node_count;
                if src != tgt {
                    edges.push(codryn_store::Edge {
                        id: 0,
                        project: "p".into(),
                        source_id: node_ids[src],
                        target_id: node_ids[tgt],
                        edge_type: edge_type.clone(),
                        properties_json: None,
                    });
                }
            }
            if !edges.is_empty() {
                store.insert_edges_batch(&edges).unwrap();
            }

            // Disable bulk indexing mode (re-enables foreign key checks)
            store.disable_bulk_indexing_mode().unwrap();

            // Verify PRAGMA foreign_key_check returns zero violations
            let conn = store.conn();
            let mut stmt = conn.prepare("PRAGMA foreign_key_check").unwrap();
            let violations: Vec<String> = stmt
                .query_map([], |row| row.get::<_, String>(0))
                .unwrap()
                .filter_map(|r| r.ok())
                .collect();
            prop_assert!(
                violations.is_empty(),
                "foreign_key_check reported {} violations: {:?}",
                violations.len(),
                violations
            );

            // Verify UNIQUE constraint on nodes(project, qualified_name):
            // Attempt to insert duplicate nodes and confirm no duplicates are created
            let results_dup = store.insert_nodes_batch(&nodes).unwrap();
            // The IDs should be the same (INSERT OR IGNORE returns existing IDs)
            for (i, (_, dup_id)) in results_dup.iter().enumerate() {
                prop_assert_eq!(
                    *dup_id, node_ids[i],
                    "duplicate node insert returned different ID for node {}: expected {}, got {}",
                    i, node_ids[i], dup_id
                );
            }

            // Verify total node count hasn't increased
            let total_nodes: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM nodes WHERE project = 'p'",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            prop_assert_eq!(
                total_nodes, node_count as i64,
                "expected {} nodes but found {} (duplicates were created)",
                node_count, total_nodes
            );

            // Verify UNIQUE constraint on edges(source_id, target_id, type):
            // Re-insert the same edges and verify count doesn't increase
            let edge_count_before: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM edges WHERE project = 'p'",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            if !edges.is_empty() {
                store.insert_edges_batch(&edges).unwrap();
            }
            let edge_count_after: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM edges WHERE project = 'p'",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            prop_assert_eq!(
                edge_count_before, edge_count_after,
                "edge count changed after re-inserting same edges: before={}, after={} (duplicates were created)",
                edge_count_before, edge_count_after
            );
        }
    }
}

/// **Validates: Requirements 2.1, 2.2, 2.5**
/// Property 1 (index-speed-optimization): Batch QN Resolution Equivalence
/// For any set of nodes in the store and any set of QN references,
/// the batch resolution approach (using `resolve_qns_batch` + `resolve_qns_suffix_batch`)
/// SHALL produce the same mappings as the original one-by-one resolution approach
/// (`find_node_by_qn` + `find_nodes_by_qn_suffix`).
mod property1_batch_qn_resolution_equivalence {
    use super::*;

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(100))]
        #[test]
        fn batch_qn_resolution_matches_one_by_one(
            node_count in 5usize..20,
            // Generate extra QN references (some will exist, some won't)
            extra_qn_count in 2usize..10,
            seed in 0u64..10000,
        ) {
            use std::collections::HashSet;

            let store = test_store();
            store.upsert_project(&codryn_store::Project {
                name: "p".into(),
                indexed_at: "now".into(),
                root_path: "/".into(),
            }).unwrap();

            // Generate unique qualified names for nodes
            let mut qns: Vec<String> = Vec::new();
            for i in 0..node_count {
                // Use index-based generation to ensure uniqueness
                let qn = format!("p.src.mod{}.func{}", i / 5, i);
                qns.push(qn);
            }

            // Insert nodes into the store
            let mut nodes = Vec::new();
            for (i, qn) in qns.iter().enumerate() {
                let name = qn.rsplit('.').next().unwrap_or(qn).to_string();
                nodes.push(codryn_store::Node {
                    id: 0,
                    project: "p".into(),
                    label: "Function".into(),
                    name,
                    qualified_name: qn.clone(),
                    file_path: format!("src/file_{}.ts", i),
                    start_line: 1,
                    end_line: 10,
                    properties_json: None,
                });
            }
            let results = store.insert_nodes_batch(&nodes).unwrap();
            let _node_id_map: HashMap<String, i64> = results.into_iter().collect();

            // Build QN references to resolve: mix of existing QNs and non-existing ones
            let mut qn_refs: Vec<String> = Vec::new();
            // Add some existing QNs
            for qn in qns.iter().take(std::cmp::min(node_count, extra_qn_count)) {
                qn_refs.push(qn.clone());
            }
            // Add some non-existing QNs
            for i in 0..extra_qn_count {
                qn_refs.push(format!("p.nonexistent.missing{}", i as u64 + seed));
            }

            // --- One-by-one exact resolution ---
            let mut one_by_one_exact: HashMap<String, i64> = HashMap::new();
            for qn in &qn_refs {
                if let Some(node) = store.find_node_by_qn("p", qn).unwrap() {
                    one_by_one_exact.insert(qn.clone(), node.id);
                }
            }

            // --- Batch exact resolution ---
            let qn_ref_strs: Vec<&str> = qn_refs.iter().map(|s| s.as_str()).collect();
            let batch_exact = store.resolve_qns_batch("p", &qn_ref_strs).unwrap();

            // Compare exact resolution results
            prop_assert_eq!(
                one_by_one_exact.len(), batch_exact.len(),
                "exact resolution count mismatch: one_by_one={}, batch={}",
                one_by_one_exact.len(), batch_exact.len()
            );
            for (qn, id) in &one_by_one_exact {
                prop_assert_eq!(
                    batch_exact.get(qn), Some(id),
                    "batch exact resolution missing or wrong for QN '{}'", qn
                );
            }

            // --- Suffix resolution ---
            // Extract suffixes (last segment after '.') from unresolved QNs
            let unresolved: Vec<&String> = qn_refs.iter()
                .filter(|qn| !one_by_one_exact.contains_key(*qn))
                .collect();

            let suffixes: Vec<String> = unresolved.iter()
                .map(|qn| qn.rsplit('.').next().unwrap_or(qn).to_string())
                .collect();
            let unique_suffixes: Vec<String> = suffixes.iter()
                .collect::<HashSet<_>>()
                .into_iter()
                .cloned()
                .collect();

            // --- One-by-one suffix resolution ---
            let mut one_by_one_suffix: HashMap<String, i64> = HashMap::new();
            for suffix in &unique_suffixes {
                let candidates = store.find_nodes_by_qn_suffix("p", suffix).unwrap();
                if !candidates.is_empty() {
                    // Same disambiguation logic as batch: prefer exact name match, then lowest id
                    let exact_matches: Vec<&codryn_store::Node> = candidates.iter()
                        .filter(|n| n.name == *suffix)
                        .collect();
                    let chosen_id = if exact_matches.len() == 1 {
                        exact_matches[0].id
                    } else if !exact_matches.is_empty() {
                        exact_matches.iter().map(|n| n.id).min().unwrap()
                    } else {
                        candidates.iter().map(|n| n.id).min().unwrap()
                    };
                    one_by_one_suffix.insert(suffix.clone(), chosen_id);
                }
            }

            // --- Batch suffix resolution ---
            let suffix_strs: Vec<&str> = unique_suffixes.iter().map(|s| s.as_str()).collect();
            let batch_suffix = store.resolve_qns_suffix_batch("p", &suffix_strs).unwrap();

            // Compare suffix resolution results
            prop_assert_eq!(
                one_by_one_suffix.len(), batch_suffix.len(),
                "suffix resolution count mismatch: one_by_one={}, batch={}",
                one_by_one_suffix.len(), batch_suffix.len()
            );
            for (suffix, id) in &one_by_one_suffix {
                prop_assert_eq!(
                    batch_suffix.get(suffix), Some(id),
                    "batch suffix resolution missing or wrong for suffix '{}'", suffix
                );
            }
        }
    }
}

/// **Validates: Requirements 10.3**
/// Property 14: Stop word removal from search queries
/// Generate random query strings containing stop words.
/// Assert `extract_semantic_keywords` output contains no stop words and all tokens are lowercase.
mod property14_stop_word_removal {
    use proptest::prelude::*;

    const STOP_WORDS: &[&str] = &[
        "a", "an", "the", "is", "are", "was", "were", "be", "been", "being", "have", "has", "had",
        "do", "does", "did", "will", "would", "could", "should", "may", "might", "shall", "can",
        "need", "must", "in", "on", "at", "to", "for", "of", "with", "by", "from", "as", "into",
        "through", "during", "before", "after", "above", "below", "and", "or", "not", "but", "if",
        "then", "else", "when", "where", "this", "that", "these", "those", "it", "its",
    ];

    /// Strategy: generate a query mixing stop words and real keywords.
    fn query_with_stop_words() -> impl Strategy<Value = String> {
        let stop_word = prop::sample::select(STOP_WORDS).prop_map(|s| s.to_string());
        let real_word = "[a-zA-Z]{2,8}";
        // Build a vec of 2-8 tokens, each either a stop word or a real word
        prop::collection::vec(
            prop_oneof![stop_word, real_word.prop_map(String::from)],
            2..8,
        )
        .prop_map(|tokens| tokens.join(" "))
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(100))]
        #[test]
        fn no_stop_words_in_output(query in query_with_stop_words()) {
            let result = codryn_store::extract_semantic_keywords(&query);

            for token in &result {
                // All tokens must be lowercase
                prop_assert_eq!(
                    token, &token.to_lowercase(),
                    "token '{}' should be lowercase", token
                );
                // No token should be a stop word
                prop_assert!(
                    !STOP_WORDS.contains(&token.as_str()),
                    "token '{}' is a stop word and should have been removed", token
                );
                // No single-character tokens
                prop_assert!(
                    token.len() > 1,
                    "token '{}' is single-char and should have been removed", token
                );
            }
        }
    }
}

/// **Validates: Requirements 12.1, 12.2, 12.3, 12.4**
/// Property 16: Compressed code storage round-trip
/// Generate random strings of varying sizes (below and above threshold).
/// Assert `maybe_decompress(maybe_compress(s))` returns the original string.
/// Assert snippets below threshold are stored uncompressed.
mod property16_compression_round_trip {
    use codryn_store::compressed_store::{
        maybe_compress, maybe_decompress, COMPRESSED_PREFIX, COMPRESSION_THRESHOLD,
    };
    use proptest::prelude::*;

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(100))]
        #[test]
        fn round_trip_preserves_content(
            content in "[\\x20-\\x7E]{0,4096}"
        ) {
            let compressed = maybe_compress(&content);
            let decompressed = maybe_decompress(&compressed);
            prop_assert_eq!(
                &decompressed, &content,
                "round-trip failed: decompress(compress(s)) != s for len={}",
                content.len()
            );
        }

        #[test]
        fn below_threshold_stored_uncompressed(
            content in "[\\x20-\\x7E]{0,1023}"
        ) {
            let compressed = maybe_compress(&content);
            // Below threshold: output should be raw UTF-8 bytes, no ZSTD prefix
            prop_assert!(
                !compressed.starts_with(COMPRESSED_PREFIX),
                "content of len {} is below threshold {} but was compressed",
                content.len(), COMPRESSION_THRESHOLD
            );
            prop_assert_eq!(
                compressed, content.as_bytes(),
                "below-threshold content should be stored as raw bytes"
            );
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Property 2 (incremental): File Hash Round-Trip
// ═══════════════════════════════════════════════════════════════════════

/// **Validates: Requirements 4.1, 4.4**
/// Storing a batch of file hashes and retrieving them must return identical values.
mod property_hash_round_trip {
    use codryn_store::{FileHash, Project, Store};
    use proptest::prelude::*;

    fn hash_strategy() -> impl Strategy<Value = String> {
        "[0-9a-f]{64}"
    }

    fn path_strategy() -> impl Strategy<Value = String> {
        "[a-z]{1,8}/[a-z]{1,8}\\.[a-z]{1,4}"
    }

    fn setup_project(store: &Store, project: &str) {
        store
            .upsert_project(&Project {
                name: project.to_string(),
                indexed_at: "2024-01-01T00:00:00Z".to_string(),
                root_path: "/tmp".to_string(),
            })
            .unwrap();
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(50))]

        #[test]
        fn hash_round_trip(
            project in "[a-z]{3,10}",
            paths in prop::collection::vec(path_strategy(), 1..10),
            hashes in prop::collection::vec(hash_strategy(), 1..10),
        ) {
            let store = Store::open_in_memory().unwrap();
            setup_project(&store, &project);
            let n = paths.len().min(hashes.len());
            let file_hashes: Vec<FileHash> = (0..n).map(|i| FileHash {
                project: project.clone(),
                rel_path: paths[i].clone(),
                sha256: hashes[i].clone(),
                mtime_ns: 0,
                size: 100,
            }).collect();

            store.store_file_hashes_batch(&file_hashes).unwrap();
            let retrieved = store.get_file_hashes(&project).unwrap();

            let map: std::collections::HashMap<_, _> = retrieved
                .iter()
                .map(|h| (h.rel_path.clone(), h.sha256.clone()))
                .collect();

            for fh in &file_hashes {
                prop_assert_eq!(
                    map.get(&fh.rel_path),
                    Some(&fh.sha256),
                    "hash mismatch for {}", fh.rel_path
                );
            }
        }

        #[test]
        fn upsert_overwrites_old_hash(
            project in "[a-z]{3,10}",
            path in path_strategy(),
            hash1 in hash_strategy(),
            hash2 in hash_strategy(),
        ) {
            prop_assume!(hash1 != hash2);
            let store = Store::open_in_memory().unwrap();
            setup_project(&store, &project);

            store.store_file_hashes_batch(&[FileHash {
                project: project.clone(), rel_path: path.clone(),
                sha256: hash1.clone(), mtime_ns: 0, size: 100,
            }]).unwrap();
            store.store_file_hashes_batch(&[FileHash {
                project: project.clone(), rel_path: path.clone(),
                sha256: hash2.clone(), mtime_ns: 0, size: 100,
            }]).unwrap();

            let retrieved = store.get_file_hashes(&project).unwrap();
            let found: Vec<_> = retrieved.iter().filter(|h| h.rel_path == path).collect();
            prop_assert_eq!(found.len(), 1, "should have exactly one entry after upsert");
            prop_assert_eq!(&found[0].sha256, &hash2, "upsert should overwrite old hash");
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Property 3 (incremental): Stale Data Removal
// ═══════════════════════════════════════════════════════════════════════

/// **Validates: Requirements 4.4, 4.5**
/// delete_nodes_for_files must remove all nodes for the given paths.
mod property_stale_data_removal {
    use codryn_store::{Project, Store};
    use proptest::prelude::*;

    fn setup_project(store: &Store, project: &str) {
        store
            .upsert_project(&Project {
                name: project.to_string(),
                indexed_at: "2024-01-01T00:00:00Z".to_string(),
                root_path: "/tmp".to_string(),
            })
            .unwrap();
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(50))]

        #[test]
        fn delete_nodes_removes_all_for_file(
            project in "[a-z]{3,10}",
            file_path in "[a-z]{3,8}/[a-z]{3,8}\\.rs",
            node_count in 1usize..8,
        ) {
            let store = Store::open_in_memory().unwrap();
            setup_project(&store, &project);
            let conn = store.conn();

            for i in 0..node_count {
                conn.execute(
                    "INSERT OR IGNORE INTO nodes (project, qualified_name, name, label, file_path, start_line, end_line) \
                     VALUES (?1, ?2, ?3, 'Function', ?4, 1, 10)",
                    rusqlite::params![
                        project, format!("{}::fn{}", file_path, i),
                        format!("fn{}", i), file_path
                    ],
                ).unwrap();
            }

            let count_before: i64 = conn.query_row(
                "SELECT COUNT(*) FROM nodes WHERE project = ?1 AND file_path = ?2",
                rusqlite::params![project, file_path],
                |r| r.get(0),
            ).unwrap();
            prop_assert_eq!(count_before, node_count as i64);

            let deleted = store.delete_nodes_for_files(&project, &[file_path.as_str()]).unwrap();
            prop_assert!(deleted > 0, "should have deleted at least one node");

            let count_after: i64 = conn.query_row(
                "SELECT COUNT(*) FROM nodes WHERE project = ?1 AND file_path = ?2",
                rusqlite::params![project, file_path],
                |r| r.get(0),
            ).unwrap();
            prop_assert_eq!(count_after, 0, "all nodes for file should be deleted");
        }

        #[test]
        fn delete_nodes_does_not_affect_other_files(
            project in "[a-z]{3,10}",
            suffix_a in "[a-z]{3,8}",
            suffix_b in "[a-z]{3,8}",
        ) {
            prop_assume!(suffix_a != suffix_b);
            let file_a = format!("src/{}.rs", suffix_a);
            let file_b = format!("src/{}.rs", suffix_b);
            let store = Store::open_in_memory().unwrap();
            setup_project(&store, &project);
            let conn = store.conn();

            for (fp, qn) in [(&file_a, "fn_a"), (&file_b, "fn_b")] {
                conn.execute(
                    "INSERT OR IGNORE INTO nodes (project, qualified_name, name, label, file_path, start_line, end_line) \
                     VALUES (?1, ?2, ?3, 'Function', ?4, 1, 10)",
                    rusqlite::params![project, format!("{}::{}", fp, qn), qn, fp],
                ).unwrap();
            }

            store.delete_nodes_for_files(&project, &[file_a.as_str()]).unwrap();

            let count_b: i64 = conn.query_row(
                "SELECT COUNT(*) FROM nodes WHERE project = ?1 AND file_path = ?2",
                rusqlite::params![project, file_b],
                |r| r.get(0),
            ).unwrap();
            prop_assert_eq!(count_b, 1, "nodes for other files must not be deleted");
        }
    }
}
