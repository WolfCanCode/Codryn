//! Tests for the reindex clear-cache bug fix.
//!
//! Property 1 (bug condition): when `clear_cache=true` and `get_store()` fails,
//!   `index_repository` MUST return `{"error":"failed to clear cache:…"}`.
//!   This test FAILS on unfixed code (the `if let Ok` guard silently swallows the error).
//!
//! Property 2 (preservation): when `clear_cache` is false/absent,
//!   `index_repository` MUST never return `"failed to clear cache"` in the response.

use codryn_mcp::{CodrynServer, IndexArgs};
use proptest::prelude::*;
use serde_json::Value;
use tempfile::TempDir;

fn parse(s: &str) -> Value {
    serde_json::from_str(s).unwrap_or(Value::Null)
}

/// Returns a store_path that `Store::open` will always fail on:
/// a path whose parent is a *file*, so `create_dir_all` fails.
fn unwritable_store_path(tmp: &TempDir) -> std::path::PathBuf {
    let blocker = tmp.path().join("blocker");
    std::fs::write(&blocker, b"I am a file").unwrap();
    blocker.join("nested") // create_dir_all("blocker/nested") fails — blocker is a file
}

fn valid_store_path(tmp: &TempDir) -> std::path::PathBuf {
    let p = tmp.path().join("store");
    std::fs::create_dir_all(&p).unwrap();
    p
}

fn repo_path() -> String {
    std::env::current_dir()
        .unwrap()
        .to_string_lossy()
        .into_owned()
}

// ── Property 1: Bug Condition ─────────────────────────────────────────────────
//
// CRITICAL: This test MUST FAIL on unfixed code.
// The `if let Ok` guard silently ignores the store-open error, so the pipeline
// runs and returns a pipeline error (or success) instead of the expected
// `{"error":"failed to clear cache:…"}`.

#[tokio::test]
async fn p1_clear_cache_with_bad_store_returns_error() {
    let tmp = TempDir::new().unwrap();
    let server = CodrynServer::new(&unwritable_store_path(&tmp));

    let response = server
        .index_repository_test(IndexArgs {
            path: repo_path(),
            mode: None,
            clear_cache: Some(true),
            analytics: None,
        })
        .await;

    let v = parse(&response);
    assert!(
        v.get("error").is_some(),
        "BUG: expected {{\"error\":\"failed to clear cache:…\"}} but got: {response}"
    );
    assert!(
        v["error"]
            .as_str()
            .unwrap_or("")
            .contains("failed to clear cache"),
        "BUG: error should contain 'failed to clear cache', got: {response}"
    );
}

// ── Property 2: Preservation ──────────────────────────────────────────────────
//
// Non-clear-cache calls must never produce "failed to clear cache" in the response.
// These tests PASS on both unfixed and fixed code.

#[tokio::test]
async fn p2_no_clear_cache_never_returns_cache_error() {
    let tmp = TempDir::new().unwrap();
    let server = CodrynServer::new(&valid_store_path(&tmp));
    let repo = repo_path();

    for (mode, clear_cache) in [
        (None, Some(false)),
        (Some("full".to_string()), Some(false)),
        (Some("fast".to_string()), Some(false)),
        (None, None),
    ] {
        let response = server
            .index_repository_test(IndexArgs {
                path: repo.clone(),
                mode,
                clear_cache,
                analytics: None,
            })
            .await;

        assert!(
            !response.contains("failed to clear cache"),
            "REGRESSION: non-clear-cache call returned 'failed to clear cache': {response}"
        );
    }
}

// ── Proptest variant of Property 1 ───────────────────────────────────────────

proptest! {
    #![proptest_config(ProptestConfig::with_cases(10))]

    #[test]
    fn p1_prop_clear_cache_bad_store_always_errors(
        extra in prop::collection::vec("[a-z]{3,8}", 1usize..4),
    ) {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let tmp = TempDir::new().unwrap();
            let blocker = tmp.path().join("blocker");
            std::fs::write(&blocker, b"x").unwrap();
            let store_path = extra.iter().fold(blocker, |p, seg| p.join(seg));

            let server = CodrynServer::new(&store_path);
            let response = server
                .index_repository_test(IndexArgs {
                    path: repo_path(),
                    mode: None,
                    clear_cache: Some(true),
                    analytics: None,
                })
                .await;

            let v = parse(&response);
            prop_assert!(v.get("error").is_some(), "expected error, got: {response}");
            prop_assert!(
                v["error"].as_str().unwrap_or("").contains("failed to clear cache"),
                "expected 'failed to clear cache' in error, got: {response}"
            );
            Ok(())
        }).unwrap();
    }
}
