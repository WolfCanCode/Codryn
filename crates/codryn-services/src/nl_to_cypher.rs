use anyhow::Result;
use codryn_store::Store;
use regex::Regex;
use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct NLQueryResult {
    pub question: String,
    pub cypher: String,
    pub matched_template: Option<String>,
    pub results: Vec<serde_json::Value>,
}

pub struct NLToCypherService;

impl NLToCypherService {
    pub fn translate_and_execute(
        store: &Store,
        project: &str,
        question: &str,
    ) -> Result<NLQueryResult> {
        let q = question.trim().to_lowercase();
        let (cypher, template) = Self::translate(&q, project);

        let results = match codryn_cypher::execute(store, project, &cypher) {
            Ok(val) => match val {
                serde_json::Value::Array(arr) => arr,
                other => vec![other],
            },
            Err(_) => {
                // Fallback: broad search
                let nodes = store.search_nodes_broad(project, question, None, 10)?;
                nodes
                    .iter()
                    .map(|n| {
                        serde_json::json!({
                            "name": n.name, "qualified_name": n.qualified_name,
                            "file_path": n.file_path, "label": n.label,
                        })
                    })
                    .collect()
            }
        };

        Ok(NLQueryResult {
            question: question.to_string(),
            cypher,
            matched_template: template,
            results,
        })
    }

    fn translate(q: &str, project: &str) -> (String, Option<String>) {
        if let Some(entity) = Self::match_pattern(q, &["who calls ", "what calls "]) {
            let cypher = format!(
                "MATCH (caller)-[:CALLS]->(f {{name:'{}', project:'{}'}}) RETURN caller",
                entity, project
            );
            return (cypher, Some("who_calls".into()));
        }
        if let Some(entity) = Self::match_pattern(q, &["what does ", "what functions does "]) {
            // "what does X call?" pattern
            if entity.ends_with(" call") || entity.ends_with(" call?") {
                let name = entity
                    .trim_end_matches('?')
                    .trim_end_matches(" call")
                    .trim();
                let cypher = format!(
                    "MATCH (f {{name:'{}', project:'{}'}})-[:CALLS]->(callee) RETURN callee",
                    name, project
                );
                return (cypher, Some("what_calls".into()));
            }
        }
        if let Some(entity) = Self::match_pattern(q, &["what imports ", "who imports "]) {
            let cypher = format!(
                "MATCH (f)-[:IMPORTS]->(m {{name:'{}', project:'{}'}}) RETURN f",
                entity, project
            );
            return (cypher, Some("who_imports".into()));
        }
        if let Some(entity) = Self::match_pattern(q, &["what does ", "what modules does "]) {
            if entity.ends_with(" import") || entity.ends_with(" import?") {
                let name = entity
                    .trim_end_matches('?')
                    .trim_end_matches(" import")
                    .trim();
                let cypher = format!(
                    "MATCH (f {{name:'{}', project:'{}'}})-[:IMPORTS]->(m) RETURN m",
                    name, project
                );
                return (cypher, Some("imports_what".into()));
            }
        }
        if q.contains("show all controllers") || q.contains("list controllers") {
            let cypher = format!(
                "MATCH (n {{label:'Class', project:'{}'}}) WHERE n.name CONTAINS 'Controller' RETURN n",
                project
            );
            return (cypher, Some("list_controllers".into()));
        }
        if q.contains("show all services") || q.contains("list services") {
            let cypher = format!(
                "MATCH (n {{label:'Class', project:'{}'}}) WHERE n.name CONTAINS 'Service' RETURN n",
                project
            );
            return (cypher, Some("list_services".into()));
        }
        if q.contains("unused") || q.contains("dead code") || q.contains("unreferenced") {
            let cypher = format!(
                "MATCH (n {{project:'{}'}}) WHERE n.label IN ['Function','Method','Class'] AND NOT EXISTS {{ MATCH (x)-[]->(n) }} RETURN n LIMIT 20",
                project
            );
            return (cypher, Some("find_unused".into()));
        }
        if let Some(entity) =
            Self::match_pattern(q, &["functions in file ", "symbols in ", "functions in "])
        {
            let cypher = format!(
                "MATCH (n {{file_path:'{}', project:'{}'}}) RETURN n",
                entity, project
            );
            return (cypher, Some("symbols_in_file".into()));
        }
        if let Some(entity) =
            Self::match_pattern(q, &["inheritance of ", "extends ", "subclasses of "])
        {
            let cypher = format!(
                "MATCH (child)-[:INHERITS]->(parent {{name:'{}', project:'{}'}}) RETURN child",
                entity, project
            );
            return (cypher, Some("inheritance".into()));
        }
        if q.contains("show dependencies")
            || q.contains("list dependencies")
            || q.contains("imports")
        {
            let entity = Self::extract_entity(q);
            if !entity.is_empty() {
                let cypher = format!(
                    "MATCH (f {{name:'{}', project:'{}'}})-[:IMPORTS]->(m) RETURN m LIMIT 20",
                    entity, project
                );
                return (cypher, Some("show_dependencies".into()));
            }
        }

        // Fallback: search by name
        let entity = Self::extract_entity(q);
        let cypher = format!(
            "MATCH (n {{project:'{}'}}) WHERE n.name CONTAINS '{}' RETURN n LIMIT 10",
            project, entity
        );
        (cypher, None)
    }

    fn match_pattern(q: &str, prefixes: &[&str]) -> Option<String> {
        for prefix in prefixes {
            if let Some(rest) = q.strip_prefix(prefix) {
                let entity = rest.trim().trim_end_matches('?').trim();
                if !entity.is_empty() {
                    return Some(entity.to_string());
                }
            }
        }
        None
    }

    fn extract_entity(q: &str) -> String {
        let re = Regex::new(r"(?:calls|imports|of|in)\s+(\S+)").unwrap();
        if let Some(caps) = re.captures(q) {
            return caps[1].to_string();
        }
        // Last word
        q.split_whitespace()
            .last()
            .unwrap_or("")
            .trim_end_matches('?')
            .to_string()
    }
}
