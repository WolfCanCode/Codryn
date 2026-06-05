use anyhow::Result;
use codryn_store::Store;
use serde::Serialize;
use std::collections::HashSet;

#[derive(Debug, Serialize)]
pub enum Severity {
    Error,
    Warning,
    Info,
}

#[derive(Debug, Serialize)]
pub struct ReviewFinding {
    pub severity: Severity,
    pub file_path: String,
    pub symbol: String,
    pub message: String,
}

#[derive(Debug, Serialize)]
pub struct DiffReviewResult {
    pub findings: Vec<ReviewFinding>,
    pub summary: String,
}

pub struct DiffReviewService;

impl DiffReviewService {
    pub fn review_changes(
        store: &Store,
        project: &str,
        changed_files: &[String],
    ) -> Result<DiffReviewResult> {
        let changed_set: HashSet<&str> = changed_files.iter().map(|s| s.as_str()).collect();
        let mut findings = Vec::new();

        for file in changed_files {
            let nodes = store.get_nodes_for_file(project, file)?;
            for node in &nodes {
                let refs = store.incoming_references_detailed(node.id, None, 20, None)?;
                for (caller, _edge_type, _conf, _src) in &refs {
                    if !caller.file_path.is_empty()
                        && !changed_set.contains(caller.file_path.as_str())
                    {
                        findings.push(ReviewFinding {
                            severity: Severity::Warning,
                            file_path: file.clone(),
                            symbol: node.name.clone(),
                            message: format!(
                                "Caller '{}' in '{}' not in changeset",
                                caller.name, caller.file_path
                            ),
                        });
                        break; // one warning per symbol is enough
                    }
                }
            }

            // Check for missing test updates
            let test_candidates = test_path_candidates(file);
            let has_test = test_candidates
                .iter()
                .any(|t| changed_set.contains(t.as_str()));
            if !has_test {
                findings.push(ReviewFinding {
                    severity: Severity::Info,
                    file_path: file.clone(),
                    symbol: String::new(),
                    message: "No test updates for changed file".into(),
                });
            }
        }

        let summary = format!(
            "{} findings across {} changed files",
            findings.len(),
            changed_files.len()
        );
        Ok(DiffReviewResult { findings, summary })
    }
}

fn test_path_candidates(file_path: &str) -> Vec<String> {
    let mut candidates = Vec::new();
    if let Some(dot) = file_path.rfind('.') {
        let base = &file_path[..dot];
        let ext = &file_path[dot..];
        candidates.push(format!("{}_test{}", base, ext));
        candidates.push(format!("{}.test{}", base, ext));
        candidates.push(format!("{}.spec{}", base, ext));
    }
    if let Some(rest) = file_path.strip_prefix("src/") {
        candidates.push(format!("tests/{}", rest));
        candidates.push(format!("test/{}", rest));
    }
    candidates
}
