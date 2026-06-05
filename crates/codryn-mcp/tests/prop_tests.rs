use codryn_mcp::auto_index::is_stale;
use codryn_mcp::diagnostics::Diagnostics;
use proptest::prelude::*;
use std::collections::VecDeque;
use std::time::Duration;

// ═══════════════════════════════════════════════════════════════════════
// Property 15 — Staleness detection correctness
// ═══════════════════════════════════════════════════════════════════════

/// **Validates: Requirements 11.1, 11.4**
mod property15_staleness_detection {
    use super::*;

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(100))]

        #[test]
        fn stale_iff_elapsed_exceeds_threshold(
            age_secs in 0u64..200_000,
            threshold_secs in 1u64..100_000,
        ) {
            let now = chrono::Utc::now();
            let indexed_at = (now - chrono::Duration::seconds(age_secs as i64)).to_rfc3339();
            let threshold = Duration::from_secs(threshold_secs);

            let result = is_stale(&indexed_at, threshold);

            // The project should be stale iff the age >= threshold.
            // There may be a 1-second rounding difference because `is_stale`
            // re-reads `Utc::now()` internally, so allow a 2-second tolerance.
            if age_secs >= threshold_secs + 2 {
                prop_assert!(
                    result,
                    "Expected stale: age={}s >= threshold={}s", age_secs, threshold_secs
                );
            } else if age_secs + 2 < threshold_secs {
                prop_assert!(
                    !result,
                    "Expected fresh: age={}s < threshold={}s", age_secs, threshold_secs
                );
            }
            // Within the 2-second tolerance window, either result is acceptable.
        }

        #[test]
        fn invalid_timestamp_always_stale(
            threshold_secs in 1u64..100_000,
        ) {
            let threshold = Duration::from_secs(threshold_secs);
            prop_assert!(
                is_stale("not-a-valid-timestamp", threshold),
                "Invalid timestamps should always be treated as stale"
            );
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Property 17 — Diagnostics rolling window and statistics
// ═══════════════════════════════════════════════════════════════════════

/// **Validates: Requirements 13.2, 13.3**
mod property17_diagnostics_statistics {
    use super::*;

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(100))]

        #[test]
        fn rolling_window_at_most_100_entries(
            durations_ms in prop::collection::vec(0u64..10_000, 0..300),
        ) {
            let diag = Diagnostics::new();
            for ms in &durations_ms {
                diag.record_query(Duration::from_millis(*ms));
            }

            let report = diag.report();
            prop_assert!(
                report.query_count <= 100,
                "Rolling window should contain at most 100 entries, got {}",
                report.query_count
            );

            let expected_count = durations_ms.len().min(100);
            prop_assert_eq!(
                report.query_count, expected_count,
                "Expected {} entries in window, got {}",
                expected_count, report.query_count
            );
        }

        #[test]
        fn statistics_correct_for_window_contents(
            durations_ms in prop::collection::vec(1u64..10_000, 1..300),
        ) {
            let diag = Diagnostics::new();
            for ms in &durations_ms {
                diag.record_query(Duration::from_millis(*ms));
            }

            let report = diag.report();

            // The window should contain the most recent min(N, 100) entries
            let window: VecDeque<u64> = durations_ms.iter()
                .copied()
                .skip(durations_ms.len().saturating_sub(100))
                .collect();

            let count = window.len();
            prop_assert_eq!(report.query_count, count);

            // Compute expected statistics
            let mut sorted: Vec<f64> = window.iter().map(|ms| *ms as f64).collect();
            sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());

            let expected_avg = sorted.iter().sum::<f64>() / count as f64;
            let p95_idx = ((count as f64) * 0.95).ceil() as usize;
            let expected_p95 = sorted[p95_idx.saturating_sub(1).min(sorted.len() - 1)];
            let expected_max = sorted.last().copied().unwrap_or(0.0);

            prop_assert!(
                (report.avg_query_ms - expected_avg).abs() < 0.01,
                "Average mismatch: got {}, expected {}", report.avg_query_ms, expected_avg
            );
            prop_assert!(
                (report.p95_query_ms - expected_p95).abs() < 0.01,
                "P95 mismatch: got {}, expected {}", report.p95_query_ms, expected_p95
            );
            prop_assert!(
                (report.max_query_ms - expected_max).abs() < 0.01,
                "Max mismatch: got {}, expected {}", report.max_query_ms, expected_max
            );
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Property: Query cache put/get/invalidate
// ═══════════════════════════════════════════════════════════════════════

mod property_query_cache {
    use codryn_mcp::query_cache::{cache_key, QueryCache};
    use proptest::prelude::*;
    use std::time::Duration;

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(100))]

        #[test]
        fn put_then_get_returns_value(
            tool in "[a-z]{3,10}",
            project in "[a-z]{3,10}",
            args in "[a-z0-9]{5,20}",
            value in "[a-z0-9 ]{5,50}",
        ) {
            let cache = QueryCache::new(Duration::from_secs(60));
            let key = cache_key(&tool, &project, &args);
            cache.put(key.clone(), value.clone());
            let got = cache.get(&key);
            prop_assert_eq!(got, Some(value));
        }

        #[test]
        fn invalidate_all_clears_cache(
            tool in "[a-z]{3,10}",
            project in "[a-z]{3,10}",
            args in "[a-z0-9]{5,20}",
            value in "[a-z0-9 ]{5,50}",
        ) {
            let cache = QueryCache::new(Duration::from_secs(60));
            let key = cache_key(&tool, &project, &args);
            cache.put(key.clone(), value);
            cache.invalidate_all();
            let got = cache.get(&key);
            prop_assert_eq!(got, None);
        }

        #[test]
        fn hits_increment_on_cache_hit(
            tool in "[a-z]{3,10}",
            project in "[a-z]{3,10}",
            args in "[a-z0-9]{5,20}",
            value in "[a-z0-9 ]{5,50}",
        ) {
            let cache = QueryCache::new(Duration::from_secs(60));
            let key = cache_key(&tool, &project, &args);
            cache.put(key.clone(), value);

            let before = cache.stats().hits;
            let _ = cache.get(&key);
            let after = cache.stats().hits;
            prop_assert_eq!(after, before + 1);
        }
    }
}
