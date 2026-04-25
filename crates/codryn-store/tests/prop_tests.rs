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
                if let Some(&(exp_mcp, exp_ui)) = expected.get(&tc.tool_name) {
                    prop_assert_eq!(
                        tc.mcp_count, exp_mcp,
                        "mcp_count mismatch for tool {}", tc.tool_name
                    );
                    prop_assert_eq!(
                        tc.ui_count, exp_ui,
                        "ui_count mismatch for tool {}", tc.tool_name
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
/// per_agent and per_model counts should sum to total_calls (MCP-sourced rows only).
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
