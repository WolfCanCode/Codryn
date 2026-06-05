//! What-if analysis service.

use codryn_store::Store;
use rusqlite::params;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeType {
    Rename,
    Remove,
    ChangeSignature,
    MoveFile,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WhatIfRequest {
    pub project: String,
    pub symbol: String,
    pub change_type: ChangeType,
    pub new_value: Option<String>,
    pub max_depth: Option<i32>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Breakage {
    pub file_path: String,
    pub line: i32,
    pub symbol: String,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct FixSuggestion {
    pub file_path: String,
    pub description: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct WhatIfResult {
    pub symbol: String,
    pub change_type: String,
    pub breakage_count: usize,
    pub breakages: Vec<Breakage>,
    pub fix_plan: Vec<FixSuggestion>,
}

pub struct WhatIfService;

impl WhatIfService {
    pub fn analyze(store: &Store, req: &WhatIfRequest) -> anyhow::Result<WhatIfResult> {
        let max_depth = req.max_depth.unwrap_or(3);

        if req.symbol.trim().is_empty() {
            anyhow::bail!("Symbol name is required");
        }

        let symbols = store.find_symbol_ranked(&req.project, &req.symbol, None, false, 1)?;
        if symbols.is_empty() {
            return Ok(WhatIfResult {
                symbol: req.symbol.clone(),
                change_type: format!("{:?}", req.change_type).to_lowercase(),
                breakage_count: 0,
                breakages: Vec::new(),
                fix_plan: Vec::new(),
            });
        }
        let target = symbols.first().map(|(node, _, _)| node);

        let mut breakages = Vec::new();
        let mut fix_plan = Vec::new();

        match req.change_type {
            ChangeType::Rename => {
                if let Some(sym) = target {
                    let refs = store.incoming_references_detailed(sym.id, None, 50, None)?;
                    for r in &refs {
                        breakages.push(Breakage {
                            file_path: r.0.file_path.clone(),
                            line: r.0.start_line,
                            symbol: r.0.name.clone(),
                            reason: format!("references '{}' which will be renamed", req.symbol),
                        });
                        if let Some(ref new_name) = req.new_value {
                            fix_plan.push(FixSuggestion {
                                file_path: r.0.file_path.clone(),
                                description: format!(
                                    "Update reference '{}' → '{}'",
                                    req.symbol, new_name
                                ),
                            });
                        }
                    }
                }
            }
            ChangeType::Remove => {
                if let Some(sym) = target {
                    let (direct, _, _) = store.impact_bfs(sym.id, max_depth, 100)?;
                    for dep in &direct {
                        breakages.push(Breakage {
                            file_path: dep.file_path.clone(),
                            line: dep.start_line,
                            symbol: dep.name.clone(),
                            reason: format!("depends on '{}' which will be removed", req.symbol),
                        });
                        fix_plan.push(FixSuggestion {
                            file_path: dep.file_path.clone(),
                            description: format!("Remove or replace usage of '{}'", req.symbol),
                        });
                    }
                }
            }
            ChangeType::ChangeSignature => {
                if let Some(sym) = target {
                    let refs = store.incoming_references_detailed(sym.id, None, 50, None)?;
                    for r in &refs {
                        breakages.push(Breakage {
                            file_path: r.0.file_path.clone(),
                            line: r.0.start_line,
                            symbol: r.0.name.clone(),
                            reason: format!("calls '{}' with old signature", req.symbol),
                        });
                        fix_plan.push(FixSuggestion {
                            file_path: r.0.file_path.clone(),
                            description: format!(
                                "Update call to '{}' to match new signature: {}",
                                req.symbol,
                                req.new_value.as_deref().unwrap_or("(unspecified)")
                            ),
                        });
                    }
                }
            }
            ChangeType::MoveFile => {
                let conn = store.conn();
                let new_path = req.new_value.as_deref().unwrap_or("(unknown)");
                let mut stmt = conn.prepare(
                    "SELECT DISTINCT n_src.file_path, n_src.name, n_src.start_line \
                     FROM edges e \
                     JOIN nodes n_src ON n_src.id = e.source_id \
                     JOIN nodes n_tgt ON n_tgt.id = e.target_id \
                     WHERE e.project = ?1 AND e.type = 'IMPORTS' \
                       AND n_tgt.file_path = ?2 \
                     LIMIT 50",
                )?;
                let rows = stmt.query_map(params![req.project, req.symbol], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, i32>(2)?,
                    ))
                })?;
                for row in rows.flatten() {
                    breakages.push(Breakage {
                        file_path: row.0.clone(),
                        line: row.2,
                        symbol: row.1.clone(),
                        reason: format!("imports from '{}' which will move", req.symbol),
                    });
                    fix_plan.push(FixSuggestion {
                        file_path: row.0,
                        description: format!(
                            "Update import path '{}' → '{}'",
                            req.symbol, new_path
                        ),
                    });
                }
            }
        }

        Ok(WhatIfResult {
            symbol: req.symbol.clone(),
            change_type: format!("{:?}", req.change_type).to_lowercase(),
            breakage_count: breakages.len(),
            breakages,
            fix_plan,
        })
    }
}
