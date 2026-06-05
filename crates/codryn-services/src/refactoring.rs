use anyhow::Result;
use codryn_store::Store;
use rusqlite::params;
use serde::Serialize;

#[derive(Debug, Clone)]
pub enum RefactoringType {
    ExtractModule,
    SplitClass,
    MoveFunction,
    InlineFunction,
    ExtractInterface,
}

#[derive(Debug, Clone, Serialize)]
pub struct RefactoringStep {
    pub order: usize,
    pub description: String,
    pub file_path: String,
    pub risks: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RefactoringPlan {
    pub refactoring_type: String,
    pub target: String,
    pub steps: Vec<RefactoringStep>,
    pub estimated_breakages: usize,
}

pub struct RefactoringService;

impl RefactoringService {
    pub fn plan(
        store: &Store,
        project: &str,
        target: &str,
        refactoring_type: RefactoringType,
    ) -> Result<RefactoringPlan> {
        if target.trim().is_empty() {
            anyhow::bail!("Target symbol or file is required");
        }

        let symbols = store.find_symbol_ranked(project, target, None, false, 1)?;
        let node = symbols.first().map(|(n, _, _)| n);

        match refactoring_type {
            RefactoringType::MoveFunction => Self::plan_move_function(store, project, target, node),
            RefactoringType::ExtractModule => {
                Self::plan_extract_module(store, project, target, node)
            }
            RefactoringType::SplitClass => Self::plan_split_class(store, project, target, node),
            RefactoringType::InlineFunction => {
                Self::plan_inline_function(store, project, target, node)
            }
            RefactoringType::ExtractInterface => {
                Self::plan_extract_interface(store, project, target, node)
            }
        }
    }

    fn plan_move_function(
        store: &Store,
        _project: &str,
        target: &str,
        node: Option<&codryn_store::Node>,
    ) -> Result<RefactoringPlan> {
        let mut steps = Vec::new();
        let mut breakages = 0;

        let file_path = node.map(|n| n.file_path.as_str()).unwrap_or("");
        steps.push(RefactoringStep {
            order: 1,
            description: format!("Move function '{}' to new location", target),
            file_path: file_path.to_string(),
            risks: vec!["Function may have file-local dependencies".into()],
        });

        if let Some(n) = node {
            let (direct, _, _) = store.impact_bfs(n.id, 2, 50)?;
            breakages = direct.len();
            if !direct.is_empty() {
                steps.push(RefactoringStep {
                    order: 2,
                    description: format!(
                        "Update {} callers to reference new location",
                        direct.len()
                    ),
                    file_path: file_path.to_string(),
                    risks: vec![format!("{} files need import updates", direct.len())],
                });
            }
        }

        Ok(RefactoringPlan {
            refactoring_type: "move_function".into(),
            target: target.into(),
            steps,
            estimated_breakages: breakages,
        })
    }

    fn plan_extract_module(
        store: &Store,
        project: &str,
        target: &str,
        node: Option<&codryn_store::Node>,
    ) -> Result<RefactoringPlan> {
        let mut steps = Vec::new();
        let file_path = node.map(|n| n.file_path.as_str()).unwrap_or("");

        // Find all symbols in same file
        let conn = store.conn();
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM nodes WHERE project = ?1 AND file_path = ?2",
                params![project, file_path],
                |row| row.get(0),
            )
            .unwrap_or(0);

        steps.push(RefactoringStep {
            order: 1,
            description: format!(
                "Create new module file for extracted symbols from '{}'",
                file_path
            ),
            file_path: file_path.to_string(),
            risks: vec!["Circular dependencies may arise".into()],
        });
        steps.push(RefactoringStep {
            order: 2,
            description: format!("Move {} symbols to new module", count),
            file_path: file_path.to_string(),
            risks: vec!["Internal references between moved symbols must be preserved".into()],
        });
        steps.push(RefactoringStep {
            order: 3,
            description: "Update imports in all dependent files".into(),
            file_path: file_path.to_string(),
            risks: vec!["Re-exports may be needed for public API stability".into()],
        });

        Ok(RefactoringPlan {
            refactoring_type: "extract_module".into(),
            target: target.into(),
            steps,
            estimated_breakages: count as usize,
        })
    }

    fn plan_split_class(
        store: &Store,
        project: &str,
        target: &str,
        node: Option<&codryn_store::Node>,
    ) -> Result<RefactoringPlan> {
        let mut steps = Vec::new();
        let file_path = node.map(|n| n.file_path.as_str()).unwrap_or("");

        // Find methods of class
        let conn = store.conn();
        let method_count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM nodes WHERE project = ?1 AND label = 'Method' AND qualified_name LIKE ?2",
            params![project, format!("{}%", target)],
            |row| row.get(0),
        ).unwrap_or(0);

        steps.push(RefactoringStep {
            order: 1,
            description: format!(
                "Identify split boundary for class '{}' ({} methods)",
                target, method_count
            ),
            file_path: file_path.to_string(),
            risks: vec!["Shared state between method groups complicates splitting".into()],
        });
        steps.push(RefactoringStep {
            order: 2,
            description: "Create new class with extracted methods".into(),
            file_path: file_path.to_string(),
            risks: vec!["Inheritance hierarchy may need adjustment".into()],
        });
        steps.push(RefactoringStep {
            order: 3,
            description: "Update references to moved methods".into(),
            file_path: file_path.to_string(),
            risks: vec!["Polymorphic call sites may break".into()],
        });

        Ok(RefactoringPlan {
            refactoring_type: "split_class".into(),
            target: target.into(),
            steps,
            estimated_breakages: method_count as usize,
        })
    }

    fn plan_inline_function(
        store: &Store,
        _project: &str,
        target: &str,
        node: Option<&codryn_store::Node>,
    ) -> Result<RefactoringPlan> {
        let mut steps = Vec::new();
        let mut breakages = 0;
        let file_path = node.map(|n| n.file_path.as_str()).unwrap_or("");

        if let Some(n) = node {
            let (direct, _, _) = store.impact_bfs(n.id, 1, 50)?;
            breakages = direct.len();
            steps.push(RefactoringStep {
                order: 1,
                description: format!("Inline '{}' at {} call sites", target, direct.len()),
                file_path: file_path.to_string(),
                risks: vec!["Code duplication increases maintenance burden".into()],
            });
        }

        steps.push(RefactoringStep {
            order: steps.len() + 1,
            description: format!("Remove original function '{}'", target),
            file_path: file_path.to_string(),
            risks: vec!["Ensure no dynamic/reflection-based callers exist".into()],
        });

        Ok(RefactoringPlan {
            refactoring_type: "inline_function".into(),
            target: target.into(),
            steps,
            estimated_breakages: breakages,
        })
    }

    fn plan_extract_interface(
        store: &Store,
        project: &str,
        target: &str,
        node: Option<&codryn_store::Node>,
    ) -> Result<RefactoringPlan> {
        let mut steps = Vec::new();
        let file_path = node.map(|n| n.file_path.as_str()).unwrap_or("");

        // Find public methods
        let conn = store.conn();
        let method_count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM nodes WHERE project = ?1 AND label = 'Method' AND qualified_name LIKE ?2",
            params![project, format!("{}%", target)],
            |row| row.get(0),
        ).unwrap_or(0);

        let mut breakages = 0;
        if let Some(n) = node {
            let (direct, _, _) = store.impact_bfs(n.id, 2, 50)?;
            breakages = direct.len();
        }

        steps.push(RefactoringStep {
            order: 1,
            description: format!(
                "Create interface with {} public methods from '{}'",
                method_count, target
            ),
            file_path: file_path.to_string(),
            risks: vec!["Interface may be too broad; consider ISP".into()],
        });
        steps.push(RefactoringStep {
            order: 2,
            description: format!("Implement interface in '{}'", target),
            file_path: file_path.to_string(),
            risks: vec![],
        });
        steps.push(RefactoringStep {
            order: 3,
            description: format!("Update {} dependents to use interface type", breakages),
            file_path: file_path.to_string(),
            risks: vec!["Concrete type assertions will break".into()],
        });

        Ok(RefactoringPlan {
            refactoring_type: "extract_interface".into(),
            target: target.into(),
            steps,
            estimated_breakages: breakages,
        })
    }
}
