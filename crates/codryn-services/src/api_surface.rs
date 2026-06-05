use anyhow::Result;
use codryn_store::Store;
use rusqlite::params;
use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct APISymbol {
    pub name: String,
    pub qualified_name: String,
    pub file_path: String,
    pub label: String,
    pub signature: Option<String>,
    pub docstring: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct APISurfaceResult {
    pub project: String,
    pub symbols: Vec<APISymbol>,
    pub total: usize,
}

pub struct APISurfaceService;

impl APISurfaceService {
    pub fn get_api_surface(
        store: &Store,
        project: &str,
        module_filter: Option<&str>,
        symbol_type: Option<&str>,
        limit: i32,
        undocumented: bool,
    ) -> Result<APISurfaceResult> {
        let conn = store.conn();
        let limit = if limit <= 0 { 50 } else { limit };
        let path_filter = module_filter.map(|m| format!("{}%", m));

        let undocumented_clause = if undocumented {
            " AND (json_extract(properties, '$.docstring') IS NULL \
              OR json_extract(properties, '$.docstring') = '')"
        } else {
            ""
        };

        let sql = format!(
            "SELECT name, qualified_name, file_path, label, properties \
             FROM nodes WHERE project = ?1 \
             AND json_extract(properties, '$.is_exported') = 1 \
             AND (?2 IS NULL OR file_path LIKE ?2) \
             AND (?3 IS NULL OR label = ?3){} \
             LIMIT ?4",
            undocumented_clause
        );

        let mut stmt = conn.prepare(&sql)?;

        let symbols: Vec<APISymbol> = stmt
            .query_map(params![project, path_filter, symbol_type, limit], |r| {
                let props: Option<String> = r.get(4)?;
                let (sig, doc) = props
                    .map(|p| {
                        let v: serde_json::Value = serde_json::from_str(&p).unwrap_or_default();
                        (
                            v.get("signature")
                                .and_then(|s| s.as_str())
                                .map(String::from),
                            v.get("docstring")
                                .and_then(|s| s.as_str())
                                .map(String::from),
                        )
                    })
                    .unwrap_or((None, None));
                Ok(APISymbol {
                    name: r.get(0)?,
                    qualified_name: r.get(1)?,
                    file_path: r.get(2)?,
                    label: r.get(3)?,
                    signature: sig,
                    docstring: doc,
                })
            })?
            .filter_map(|r| r.ok())
            .collect();

        let total = symbols.len();
        Ok(APISurfaceResult {
            project: project.into(),
            symbols,
            total,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use codryn_store::{Node, Project};

    fn setup() -> Store {
        let s = Store::open_in_memory().unwrap();
        s.upsert_project(&Project {
            name: "p".into(),
            indexed_at: "now".into(),
            root_path: "/".into(),
        })
        .unwrap();
        s
    }

    #[test]
    fn test_undocumented_filter_returns_only_undocumented() {
        let s = setup();
        // Documented symbol
        s.insert_node(&Node {
            id: 0,
            project: "p".into(),
            label: "Function".into(),
            name: "documented_fn".into(),
            qualified_name: "p::documented_fn".into(),
            file_path: "src/lib.rs".into(),
            start_line: 1,
            end_line: 10,
            properties_json: Some(r#"{"is_exported":true,"docstring":"Has documentation"}"#.into()),
        })
        .unwrap();
        // Undocumented symbol (no docstring)
        s.insert_node(&Node {
            id: 0,
            project: "p".into(),
            label: "Function".into(),
            name: "undocumented_fn".into(),
            qualified_name: "p::undocumented_fn".into(),
            file_path: "src/lib.rs".into(),
            start_line: 12,
            end_line: 20,
            properties_json: Some(r#"{"is_exported":true}"#.into()),
        })
        .unwrap();
        // Undocumented symbol (empty docstring)
        s.insert_node(&Node {
            id: 0,
            project: "p".into(),
            label: "Function".into(),
            name: "empty_doc_fn".into(),
            qualified_name: "p::empty_doc_fn".into(),
            file_path: "src/lib.rs".into(),
            start_line: 22,
            end_line: 30,
            properties_json: Some(r#"{"is_exported":true,"docstring":""}"#.into()),
        })
        .unwrap();

        // Without undocumented filter: all 3 exported symbols
        let result = APISurfaceService::get_api_surface(&s, "p", None, None, 50, false).unwrap();
        assert_eq!(result.total, 3);

        // With undocumented filter: only 2 (no docstring + empty docstring)
        let result = APISurfaceService::get_api_surface(&s, "p", None, None, 50, true).unwrap();
        assert_eq!(result.total, 2);
        assert!(result.symbols.iter().all(|sym| sym.name != "documented_fn"));
    }

    #[test]
    fn test_undocumented_filter_returns_empty_when_all_documented() {
        let s = setup();
        s.insert_node(&Node {
            id: 0,
            project: "p".into(),
            label: "Function".into(),
            name: "well_documented".into(),
            qualified_name: "p::well_documented".into(),
            file_path: "src/lib.rs".into(),
            start_line: 1,
            end_line: 10,
            properties_json: Some(
                r#"{"is_exported":true,"docstring":"Fully documented function"}"#.into(),
            ),
        })
        .unwrap();

        let result = APISurfaceService::get_api_surface(&s, "p", None, None, 50, true).unwrap();
        assert_eq!(result.total, 0);
        assert!(result.symbols.is_empty());
    }

    #[test]
    fn test_without_undocumented_filter_returns_all() {
        let s = setup();
        s.insert_node(&Node {
            id: 0,
            project: "p".into(),
            label: "Function".into(),
            name: "fn1".into(),
            qualified_name: "p::fn1".into(),
            file_path: "src/lib.rs".into(),
            start_line: 1,
            end_line: 10,
            properties_json: Some(r#"{"is_exported":true,"docstring":"docs"}"#.into()),
        })
        .unwrap();
        s.insert_node(&Node {
            id: 0,
            project: "p".into(),
            label: "Function".into(),
            name: "fn2".into(),
            qualified_name: "p::fn2".into(),
            file_path: "src/lib.rs".into(),
            start_line: 12,
            end_line: 20,
            properties_json: Some(r#"{"is_exported":true}"#.into()),
        })
        .unwrap();

        // undocumented=false should return all exported symbols
        let result = APISurfaceService::get_api_surface(&s, "p", None, None, 50, false).unwrap();
        assert_eq!(result.total, 2);
    }
}
