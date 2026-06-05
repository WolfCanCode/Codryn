use anyhow::Result;
use codryn_store::Store;
use rusqlite::params;
use serde::Serialize;
use std::collections::HashMap;

#[derive(Debug, Serialize)]
pub struct TestGapEntry {
    pub symbol: String,
    pub file_path: String,
    pub label: String,
    pub fan_in: i64,
    pub risk: String,
}

#[derive(Debug, Serialize)]
pub struct ModuleCoverage {
    pub module: String,
    pub tested: usize,
    pub total: usize,
    pub ratio: f64,
}

#[derive(Debug, Serialize)]
pub struct TestGapResult {
    pub untested: Vec<TestGapEntry>,
    pub module_coverage: Vec<ModuleCoverage>,
    pub zero_coverage_modules: Vec<String>,
}

pub struct TestGapService;

impl TestGapService {
    pub fn test_coverage_map(
        store: &Store,
        project: &str,
        scope: Option<&str>,
        _untested_only: bool,
        limit: i32,
    ) -> Result<TestGapResult> {
        let conn = store.conn();
        let limit = if limit <= 0 { 50 } else { limit };
        let scope_filter = scope.map(|s| format!("{}%", s));

        // Get all non-test Function/Method/Class nodes
        let mut stmt = conn.prepare(
            "SELECT id, name, file_path, label FROM nodes \
             WHERE project = ?1 AND label IN ('Function', 'Method', 'Class') \
             AND (?2 IS NULL OR file_path LIKE ?2) \
             AND file_path NOT LIKE '%test%' AND file_path NOT LIKE '%spec%'",
        )?;
        let nodes: Vec<(i64, String, String, String)> = stmt
            .query_map(params![project, scope_filter], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
            })?
            .filter_map(|r| r.ok())
            .collect();

        let mut untested = Vec::new();
        let mut module_map: HashMap<String, (usize, usize)> = HashMap::new(); // module -> (tested, total)

        for (id, name, file_path, label) in &nodes {
            let module = file_path.split('/').next().unwrap_or("root").to_string();
            let entry = module_map.entry(module).or_insert((0, 0));
            entry.1 += 1;

            // Check if tested
            let has_test: i64 = conn.query_row(
                "SELECT COUNT(*) FROM edges WHERE type = 'TESTS' AND target_id = ?1",
                params![id],
                |r| r.get(0),
            )?;

            if has_test > 0 {
                entry.0 += 1;
            } else {
                // Count fan_in
                let fan_in: i64 = conn.query_row(
                    "SELECT COUNT(*) FROM edges WHERE type = 'CALLS' AND target_id = ?1",
                    params![id],
                    |r| r.get(0),
                )?;
                let risk = if fan_in > 10 {
                    "high"
                } else if fan_in > 3 {
                    "medium"
                } else {
                    "low"
                };
                untested.push(TestGapEntry {
                    symbol: name.clone(),
                    file_path: file_path.clone(),
                    label: label.clone(),
                    fan_in,
                    risk: risk.into(),
                });
            }
        }

        // Sort by fan_in descending, truncate
        untested.sort_by_key(|a| std::cmp::Reverse(a.fan_in));
        untested.truncate(limit as usize);

        let mut module_coverage: Vec<ModuleCoverage> = module_map
            .into_iter()
            .map(|(module, (tested, total))| {
                let ratio = if total > 0 {
                    tested as f64 / total as f64
                } else {
                    0.0
                };
                ModuleCoverage {
                    module,
                    tested,
                    total,
                    ratio,
                }
            })
            .collect();
        module_coverage.sort_by(|a, b| {
            a.ratio
                .partial_cmp(&b.ratio)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        let zero_coverage_modules: Vec<String> = module_coverage
            .iter()
            .filter(|m| m.tested == 0 && m.total > 0)
            .map(|m| m.module.clone())
            .collect();

        Ok(TestGapResult {
            untested,
            module_coverage,
            zero_coverage_modules,
        })
    }
}
