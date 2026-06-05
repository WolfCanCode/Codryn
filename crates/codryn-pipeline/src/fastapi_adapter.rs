//! FastAPI dependency injection pass.
//!
//! Parses Python source for `Depends(fn_name)` patterns in route handler parameters
//! and creates INJECTS edges with `chain_depth` property.
//!
//! Handles:
//! - Single: `def route(dep: SomeType = Depends(get_dep))`
//! - Multiple: multiple `Depends()` in one handler
//! - Chained: dependency functions that themselves use `Depends()`

use codryn_discover::{DiscoveredFile, Language};
use codryn_foundation::fqn;
use codryn_graph_buffer::GraphBuffer;
use std::collections::HashMap;
use std::sync::LazyLock;

/// Matches `Depends(identifier)` — captures the dependency function name.
static DEPENDS_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"\bDepends\s*\(\s*([A-Za-z_][A-Za-z0-9_]*)\s*\)").unwrap());

/// Matches a Python function definition: `async def name(` or `def name(`
static FUNC_DEF_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"^(?:async\s+)?def\s+([A-Za-z_][A-Za-z0-9_]*)\s*\(").unwrap()
});

/// A dependency injection relationship extracted from a Python file.
#[derive(Debug, Clone)]
pub struct DependsRelation {
    /// The handler or dependency function that uses `Depends(dep_fn)`.
    pub handler: String,
    /// The injected dependency function name.
    pub dep_fn: String,
    /// Depth in the dependency chain (0 = direct route handler dependency).
    pub chain_depth: u32,
}

/// Extract all `Depends(fn)` relations from Python source.
///
/// Returns a list of (handler_name, dep_fn_name) pairs for direct dependencies.
/// Chain depth is computed separately by `compute_chain_depths`.
pub fn extract_fastapi_depends(source: &str) -> Vec<(String, String)> {
    let mut relations = Vec::new();
    let lines: Vec<&str> = source.lines().collect();
    let mut i = 0;

    while i < lines.len() {
        let line = lines[i];

        // Check if this line is a function definition
        if let Some(cap) = FUNC_DEF_RE.captures(line) {
            let fn_name = cap[1].to_string();

            // Collect the full parameter list (may span multiple lines until `)`)
            let mut params = String::new();
            let mut depth = 0i32;
            let mut j = i;
            loop {
                let l = lines[j];
                for ch in l.chars() {
                    match ch {
                        '(' => depth += 1,
                        ')' => {
                            depth -= 1;
                            if depth == 0 {
                                break;
                            }
                        }
                        _ => {}
                    }
                }
                params.push_str(l);
                params.push('\n');
                if depth <= 0 {
                    break;
                }
                j += 1;
                if j >= lines.len() {
                    break;
                }
            }

            // Extract all Depends() calls from the parameter list
            for cap in DEPENDS_RE.captures_iter(&params) {
                relations.push((fn_name.clone(), cap[1].to_string()));
            }
        }

        i += 1;
    }

    relations
}

/// Compute chain depths for all dependency relations.
///
/// Direct route handler dependencies have depth 0.
/// If a dependency function itself has dependencies, those get depth 1, etc.
pub fn compute_chain_depths(relations: &[(String, String)]) -> HashMap<(String, String), u32> {
    // Build adjacency: handler -> [deps]
    let mut deps_of: HashMap<&str, Vec<&str>> = HashMap::new();
    for (handler, dep) in relations {
        deps_of
            .entry(handler.as_str())
            .or_default()
            .push(dep.as_str());
    }

    // Identify route handlers: functions that are depended upon by others are "dep functions"
    // Route handlers are those that appear as handlers but not as dep_fn of anyone
    let all_dep_fns: std::collections::HashSet<&str> =
        relations.iter().map(|(_, d)| d.as_str()).collect();

    let mut depths: HashMap<(String, String), u32> = HashMap::new();

    // BFS from route handlers (depth 0) through the dependency graph
    let mut queue: std::collections::VecDeque<(&str, u32)> = std::collections::VecDeque::new();

    // Seed: all handlers that are NOT themselves a dep_fn of someone else are route handlers
    for (handler, _) in relations {
        if !all_dep_fns.contains(handler.as_str()) {
            queue.push_back((handler.as_str(), 0));
        }
    }
    // Also seed dep_fns that have their own deps (chained)
    for dep_fn in &all_dep_fns {
        if deps_of.contains_key(dep_fn) {
            queue.push_back((dep_fn, 1));
        }
    }

    let mut visited: std::collections::HashSet<(&str, u32)> = std::collections::HashSet::new();
    while let Some((handler, depth)) = queue.pop_front() {
        if !visited.insert((handler, depth)) {
            continue;
        }
        if let Some(deps) = deps_of.get(handler) {
            for dep in deps {
                depths.insert((handler.to_string(), dep.to_string()), depth);
                // Recurse into dep's own dependencies at depth+1
                if deps_of.contains_key(dep) {
                    queue.push_back((dep, depth + 1));
                }
            }
        }
    }

    depths
}

/// Pipeline pass: create INJECTS edges for FastAPI `Depends()` patterns in Python files.
pub fn pass_fastapi_depends(buf: &mut GraphBuffer, files: &[&DiscoveredFile], project: &str) {
    for f in files {
        if f.language != Language::Python {
            continue;
        }

        let source = match std::fs::read_to_string(&f.abs_path) {
            Ok(s) => s,
            Err(_) => continue,
        };

        let relations = extract_fastapi_depends(&source);
        if relations.is_empty() {
            continue;
        }

        let depths = compute_chain_depths(&relations);
        let file_qn = fqn::fqn_module(project, &f.rel_path);

        for (handler, dep_fn) in &relations {
            let handler_qn = format!("{}::{}", file_qn, handler);
            let dep_qn = format!("{}::{}", file_qn, dep_fn);
            let depth = depths
                .get(&(handler.clone(), dep_fn.clone()))
                .copied()
                .unwrap_or(0);

            buf.add_edge_by_qn(
                &handler_qn,
                &dep_qn,
                "INJECTS",
                Some(serde_json::json!({ "chain_depth": depth }).to_string()),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_single_depends() {
        let src = r#"
@app.get("/items")
def get_items(db: Session = Depends(get_db)):
    pass
"#;
        let rels = extract_fastapi_depends(src);
        assert_eq!(rels.len(), 1);
        assert_eq!(rels[0].0, "get_items");
        assert_eq!(rels[0].1, "get_db");
    }

    #[test]
    fn extract_multiple_depends() {
        let src = r#"
@app.post("/items")
def create_item(
    db: Session = Depends(get_db),
    user: User = Depends(get_current_user),
):
    pass
"#;
        let rels = extract_fastapi_depends(src);
        assert_eq!(rels.len(), 2);
        let names: Vec<&str> = rels.iter().map(|(_, d)| d.as_str()).collect();
        assert!(names.contains(&"get_db"));
        assert!(names.contains(&"get_current_user"));
    }

    #[test]
    fn extract_chained_depends() {
        let src = r#"
def get_db():
    pass

def get_current_user(db: Session = Depends(get_db)):
    pass

@app.get("/me")
def get_me(user: User = Depends(get_current_user)):
    pass
"#;
        let rels = extract_fastapi_depends(src);
        // get_current_user -> get_db, get_me -> get_current_user
        assert!(rels
            .iter()
            .any(|(h, d)| h == "get_current_user" && d == "get_db"));
        assert!(rels
            .iter()
            .any(|(h, d)| h == "get_me" && d == "get_current_user"));
    }

    #[test]
    fn extract_no_depends() {
        let src = "def plain_fn(x: int) -> int:\n    return x\n";
        let rels = extract_fastapi_depends(src);
        assert!(rels.is_empty());
    }

    #[test]
    fn chain_depth_direct_is_zero() {
        let rels = vec![("get_items".to_string(), "get_db".to_string())];
        let depths = compute_chain_depths(&rels);
        assert_eq!(
            depths.get(&("get_items".to_string(), "get_db".to_string())),
            Some(&0)
        );
    }

    #[test]
    fn chain_depth_chained_is_one() {
        let rels = vec![
            ("get_me".to_string(), "get_current_user".to_string()),
            ("get_current_user".to_string(), "get_db".to_string()),
        ];
        let depths = compute_chain_depths(&rels);
        // get_me -> get_current_user: depth 0
        assert_eq!(
            depths.get(&("get_me".to_string(), "get_current_user".to_string())),
            Some(&0)
        );
        // get_current_user -> get_db: depth 1
        assert_eq!(
            depths.get(&("get_current_user".to_string(), "get_db".to_string())),
            Some(&1)
        );
    }

    #[test]
    fn pass_creates_injects_edges() {
        use codryn_discover::Language;
        use codryn_graph_buffer::GraphBuffer;
        use tempfile::TempDir;

        let tmp = TempDir::new().unwrap();
        let src_path = tmp.path().join("routes.py");
        std::fs::write(
            &src_path,
            "def get_items(db = Depends(get_db), user = Depends(get_user)):\n    pass\n",
        )
        .unwrap();

        let file = DiscoveredFile {
            abs_path: src_path,
            rel_path: "routes.py".to_string(),
            language: Language::Python,
        };

        let mut buf = GraphBuffer::new("proj");
        pass_fastapi_depends(&mut buf, &[&file], "proj");

        assert_eq!(buf.edge_count(), 2);
    }

    #[test]
    fn pass_skips_non_python_files() {
        use codryn_discover::Language;
        use codryn_graph_buffer::GraphBuffer;
        use tempfile::TempDir;

        let tmp = TempDir::new().unwrap();
        let src_path = tmp.path().join("routes.ts");
        std::fs::write(&src_path, "function get(db = Depends(getDb)) {}").unwrap();

        let file = DiscoveredFile {
            abs_path: src_path,
            rel_path: "routes.ts".to_string(),
            language: Language::TypeScript,
        };

        let mut buf = GraphBuffer::new("proj");
        pass_fastapi_depends(&mut buf, &[&file], "proj");

        assert_eq!(buf.edge_count(), 0);
    }
}
