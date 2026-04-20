use anyhow::Result;
use codryn_store::Store;
use serde::Serialize;
use std::collections::HashSet;

pub struct TestDiscoveryService;

#[derive(Debug, Serialize)]
pub struct TestResult {
    pub project: String,
    pub target: serde_json::Value,
    pub tests: Vec<TestCandidate>,
    pub count: usize,
}

#[derive(Debug, Serialize)]
pub struct TestCandidate {
    pub file_path: String,
    pub matched_symbols: Vec<String>,
    pub reason: String,
    pub score: f64,
}

fn is_test_file(path: &str) -> bool {
    let p = path.to_lowercase();
    p.contains(".test.")
        || p.contains(".spec.")
        || p.contains("__tests__/")
        || p.contains("/tests/")
        || p.contains("/test/")
        || p.ends_with("_test.go")
        || p.ends_with("_test.rs")
        || p.contains("test_")
}

/// Generate candidate test file paths from a source file path.
fn test_path_candidates(file_path: &str) -> Vec<String> {
    let mut candidates = Vec::new();
    // Strip extension, try .test.ext and .spec.ext
    if let Some(dot) = file_path.rfind('.') {
        let base = &file_path[..dot];
        let ext = &file_path[dot..];
        candidates.push(format!("{}.test{}", base, ext));
        candidates.push(format!("{}.spec{}", base, ext));
    }
    // Mirrored: src/X → tests/X, test/X
    if let Some(rest) = file_path.strip_prefix("src/") {
        candidates.push(format!("tests/{}", rest));
        candidates.push(format!("test/{}", rest));
        if let Some(dot) = rest.rfind('.') {
            let base = &rest[..dot];
            let ext = &rest[dot..];
            candidates.push(format!("tests/{}.test{}", base, ext));
            candidates.push(format!("test/{}.test{}", base, ext));
        }
    }
    // __tests__ variant
    if let Some(slash) = file_path.rfind('/') {
        let dir = &file_path[..slash];
        let name = &file_path[slash + 1..];
        candidates.push(format!("{}/__tests__/{}", dir, name));
    }
    candidates
}

impl TestDiscoveryService {
    pub fn find_tests(
        store: &Store,
        project: &str,
        qualified_name: Option<&str>,
        name: Option<&str>,
        file_path: Option<&str>,
        limit: i32,
    ) -> Result<TestResult> {
        let limit = if limit <= 0 { 10usize } else { limit as usize };

        // Resolve target
        let (target_file, target_name, target_json) = if let Some(qn) = qualified_name {
            let node = store.find_node_by_qn(project, qn)?;
            let (fp, nm) = node
                .as_ref()
                .map(|n| (n.file_path.clone(), n.name.clone()))
                .unwrap_or_default();
            (fp, nm, serde_json::json!({"qualified_name": qn}))
        } else if let Some(fp) = file_path {
            let nm = name.unwrap_or("").to_string();
            (fp.to_string(), nm, serde_json::json!({"file_path": fp}))
        } else if let Some(n) = name {
            let nodes = store.search_nodes(project, n, 1)?;
            let fp = nodes
                .first()
                .map(|nd| nd.file_path.clone())
                .unwrap_or_default();
            (fp, n.to_string(), serde_json::json!({"name": n}))
        } else {
            return Err(anyhow::anyhow!(
                "Provide qualified_name, name, or file_path"
            ));
        };

        let all_files = store.list_files(project)?;
        let all_files_set: HashSet<&str> = all_files.iter().map(|s| s.as_str()).collect();
        let mut candidates: Vec<TestCandidate> = Vec::new();
        let mut seen = HashSet::new();

        // Strategy 1: Naming convention (score 0.95)
        if !target_file.is_empty() {
            for candidate in test_path_candidates(&target_file) {
                if all_files_set.contains(candidate.as_str()) && seen.insert(candidate.clone()) {
                    let matched = find_matching_symbols(store, project, &candidate, &target_name);
                    candidates.push(TestCandidate {
                        file_path: candidate,
                        matched_symbols: matched,
                        reason: "test file naming convention".into(),
                        score: 0.95,
                    });
                }
            }
        }

        // Strategy 2: Mirrored folder (score 0.85) — already covered by test_path_candidates
        // Strategy 3: Test files in same directory (score 0.80)
        if !target_file.is_empty() {
            if let Some(slash) = target_file.rfind('/') {
                let dir = &target_file[..slash + 1];
                for f in &all_files {
                    if f.starts_with(dir) && is_test_file(f) && seen.insert(f.clone()) {
                        let matched = find_matching_symbols(store, project, f, &target_name);
                        candidates.push(TestCandidate {
                            file_path: f.clone(),
                            matched_symbols: matched,
                            reason: "test file in same directory".into(),
                            score: 0.80,
                        });
                    }
                }
            }
        }

        // Strategy 4: Any test file referencing target name (score 0.65)
        if !target_name.is_empty() && candidates.len() < limit {
            for f in &all_files {
                if is_test_file(f) && seen.insert(f.clone()) {
                    let matched = find_matching_symbols(store, project, f, &target_name);
                    if !matched.is_empty() {
                        candidates.push(TestCandidate {
                            file_path: f.clone(),
                            matched_symbols: matched,
                            reason: "test file references target symbol".into(),
                            score: 0.65,
                        });
                    }
                }
            }
        }

        candidates.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        candidates.truncate(limit);
        let count = candidates.len();

        Ok(TestResult {
            project: project.to_string(),
            target: target_json,
            tests: candidates,
            count,
        })
    }
}

fn find_matching_symbols(
    store: &Store,
    project: &str,
    file_path: &str,
    target_name: &str,
) -> Vec<String> {
    if target_name.is_empty() {
        return vec![];
    }
    store
        .get_nodes_for_file(project, file_path)
        .unwrap_or_default()
        .iter()
        .filter(|n| n.name.contains(target_name) || target_name.contains(&n.name))
        .map(|n| n.name.clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use codryn_store::{FileHash, Node, Project};

    fn setup_store() -> Store {
        let s = Store::open_in_memory().unwrap();
        s.upsert_project(&Project {
            name: "p".into(),
            indexed_at: "now".into(),
            root_path: "/tmp".into(),
        })
        .unwrap();
        s
    }

    fn insert_node(s: &Store, name: &str, qn: &str, label: &str, fp: &str) -> i64 {
        s.insert_node(&Node {
            id: 0,
            project: "p".into(),
            label: label.into(),
            name: name.into(),
            qualified_name: qn.into(),
            file_path: fp.into(),
            start_line: 1,
            end_line: 10,
            properties_json: None,
        })
        .unwrap()
    }

    #[test]
    fn test_find_tests_by_naming_convention() {
        let s = setup_store();
        s.upsert_file_hash_batch(&[
            FileHash {
                project: "p".into(),
                rel_path: "src/user.ts".into(),
                sha256: "a".into(),
                mtime_ns: 0,
                size: 0,
            },
            FileHash {
                project: "p".into(),
                rel_path: "src/user.test.ts".into(),
                sha256: "b".into(),
                mtime_ns: 0,
                size: 0,
            },
        ])
        .unwrap();
        insert_node(&s, "getUser", "p::getUser", "Function", "src/user.ts");
        insert_node(
            &s,
            "testGetUser",
            "p::testGetUser",
            "Function",
            "src/user.test.ts",
        );

        let res =
            TestDiscoveryService::find_tests(&s, "p", None, None, Some("src/user.ts"), 10).unwrap();
        assert!(!res.tests.is_empty());
        assert_eq!(res.tests[0].file_path, "src/user.test.ts");
        assert!(res.tests[0].score >= 0.9);
    }

    #[test]
    fn test_find_tests_by_symbol_name() {
        let s = setup_store();
        s.upsert_file_hash_batch(&[
            FileHash {
                project: "p".into(),
                rel_path: "src/order.ts".into(),
                sha256: "a".into(),
                mtime_ns: 0,
                size: 0,
            },
            FileHash {
                project: "p".into(),
                rel_path: "tests/order.spec.ts".into(),
                sha256: "b".into(),
                mtime_ns: 0,
                size: 0,
            },
        ])
        .unwrap();
        insert_node(
            &s,
            "processOrder",
            "p::processOrder",
            "Function",
            "src/order.ts",
        );
        insert_node(
            &s,
            "processOrder",
            "p::test::processOrder",
            "Function",
            "tests/order.spec.ts",
        );

        let res =
            TestDiscoveryService::find_tests(&s, "p", Some("p::processOrder"), None, None, 10)
                .unwrap();
        assert!(!res.tests.is_empty());
    }
}
