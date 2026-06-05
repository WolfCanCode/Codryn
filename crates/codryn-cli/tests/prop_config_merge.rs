//! Property test for config merge preservation (Property 15).
//!
//! **Validates: Requirements 5.7**
//!
//! Property 15: Config Merge Preserves Existing Values
//! For any existing JSON object with user-modified fields and any proposed JSON object
//! with new fields, the merge operation SHALL:
//! (a) preserve all existing field values unchanged, AND
//! (b) add all new fields from the proposal that don't exist in the current entry.

use codryn_cli::mcp_config::McpConfigManager;
use proptest::prelude::*;
use serde_json::Value;

// ─── Strategies ──────────────────────────────────────────────────────────────

/// Generate a random string value suitable for JSON fields.
fn json_string_value_strategy() -> impl Strategy<Value = String> {
    "[a-z][a-z0-9_-]{0,20}".prop_map(|s| s)
}

/// Generate a random JSON object with 1-5 keys (string values).
fn json_object_strategy() -> impl Strategy<Value = Value> {
    proptest::collection::hash_map(
        "[a-z][a-z0-9_]{0,10}",       // key
        json_string_value_strategy(), // value
        1..=5,
    )
    .prop_map(|map| {
        let obj: serde_json::Map<String, Value> = map
            .into_iter()
            .map(|(k, v)| (k, Value::String(v)))
            .collect();
        Value::Object(obj)
    })
}

// ─── Property 15: Config Merge Preserves Existing Values ─────────────────────

/// **Validates: Requirements 5.7**
mod property15_config_merge_preserves_existing {
    use super::*;

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(200))]

        /// For any existing JSON object and any proposed JSON object,
        /// merge_entry preserves all existing key-value pairs unchanged.
        #[test]
        fn merge_preserves_all_existing_values(
            existing in json_object_strategy(),
            proposed in json_object_strategy(),
        ) {
            let merged = McpConfigManager::merge_entry(&existing, &proposed);

            // (a) All existing key-value pairs must be preserved unchanged in the result
            let existing_obj = existing.as_object().unwrap();
            let merged_obj = merged.as_object().unwrap();

            for (key, value) in existing_obj {
                prop_assert!(
                    merged_obj.contains_key(key),
                    "Merged result missing existing key: {}",
                    key
                );
                prop_assert_eq!(
                    merged_obj.get(key).unwrap(),
                    value,
                    "Existing value for key '{}' was modified during merge",
                    key
                );
            }
        }

        /// For any existing JSON object and any proposed JSON object,
        /// merge_entry adds all new keys from proposed that don't exist in existing.
        #[test]
        fn merge_adds_new_fields_from_proposed(
            existing in json_object_strategy(),
            proposed in json_object_strategy(),
        ) {
            let merged = McpConfigManager::merge_entry(&existing, &proposed);

            // (b) All new keys from proposed (not in existing) must be added to the result
            let existing_obj = existing.as_object().unwrap();
            let proposed_obj = proposed.as_object().unwrap();
            let merged_obj = merged.as_object().unwrap();

            for (key, value) in proposed_obj {
                if !existing_obj.contains_key(key) {
                    prop_assert!(
                        merged_obj.contains_key(key),
                        "Merged result missing new key from proposed: {}",
                        key
                    );
                    prop_assert_eq!(
                        merged_obj.get(key).unwrap(),
                        value,
                        "New key '{}' from proposed has wrong value in merged result",
                        key
                    );
                }
            }
        }

        /// Combined assertion: merged result contains exactly the existing keys
        /// plus new keys from proposed that weren't already in existing.
        #[test]
        fn merge_result_has_correct_key_set(
            existing in json_object_strategy(),
            proposed in json_object_strategy(),
        ) {
            let merged = McpConfigManager::merge_entry(&existing, &proposed);

            let existing_obj = existing.as_object().unwrap();
            let proposed_obj = proposed.as_object().unwrap();
            let merged_obj = merged.as_object().unwrap();

            // The result should contain all existing keys plus new keys from proposed
            let expected_keys: std::collections::HashSet<&String> = existing_obj
                .keys()
                .chain(proposed_obj.keys())
                .collect();

            let merged_keys: std::collections::HashSet<&String> = merged_obj.keys().collect();

            prop_assert_eq!(
                merged_keys,
                expected_keys,
                "Merged key set does not equal union of existing and proposed keys"
            );
        }
    }
}
