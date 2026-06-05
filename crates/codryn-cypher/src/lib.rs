mod executor;
mod lexer;
mod parser;

pub use executor::execute;

use serde::{Deserialize, Serialize};

/// A parsed Cypher query.
#[derive(Debug, Clone)]
pub struct CypherQuery {
    pub match_clause: MatchClause,
    pub where_clause: Option<WhereClause>,
    pub return_clause: ReturnClause,
    pub order_by: Option<Vec<OrderItem>>,
    pub limit: Option<i64>,
}

#[derive(Debug, Clone)]
pub struct MatchClause {
    pub patterns: Vec<Pattern>,
}

#[derive(Debug, Clone)]
pub enum Pattern {
    Node(NodePattern),
    Relationship(NodePattern, RelPattern, NodePattern),
}

#[derive(Debug, Clone)]
pub struct NodePattern {
    pub variable: Option<String>,
    pub label: Option<String>,
}

#[derive(Debug, Clone)]
pub struct RelPattern {
    pub variable: Option<String>,
    pub rel_type: Option<String>,
    pub direction: Direction,
}

#[derive(Debug, Clone, Copy)]
pub enum Direction {
    Right, // -[r:TYPE]->
    Left,  // <-[r:TYPE]-
    Both,  // -[r:TYPE]-
}

#[derive(Debug, Clone)]
pub struct WhereClause {
    pub conditions: Vec<Condition>,
}

#[derive(Debug, Clone)]
pub enum Condition {
    Eq(PropertyRef, Value),
    Contains(PropertyRef, String),
    StartsWith(PropertyRef, String),
    And(Box<Condition>, Box<Condition>),
    Or(Box<Condition>, Box<Condition>),
}

#[derive(Debug, Clone)]
pub struct PropertyRef {
    pub variable: String,
    pub property: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Value {
    String(String),
    Int(i64),
    Bool(bool),
}

#[derive(Debug, Clone)]
pub struct ReturnClause {
    pub items: Vec<ReturnItem>,
}

#[derive(Debug, Clone)]
pub enum ReturnItem {
    Property(PropertyRef),
    Variable(String),
    Count(String),
}

#[derive(Debug, Clone)]
pub struct OrderItem {
    pub expr: ReturnItem,
    pub descending: bool,
}

#[cfg(test)]
mod tests {
    use super::execute;
    use codryn_store::{Edge, Node, Project, Store};

    fn setup() -> (Store, &'static str) {
        let s = Store::open_in_memory().unwrap();
        s.upsert_project(&Project {
            name: "p".into(),
            indexed_at: "now".into(),
            root_path: "/".into(),
        })
        .unwrap();
        let nodes = [
            Node {
                id: 0,
                project: "p".into(),
                label: "Function".into(),
                name: "alpha".into(),
                qualified_name: "p.alpha".into(),
                file_path: "a.rs".into(),
                start_line: 1,
                end_line: 10,
                properties_json: None,
            },
            Node {
                id: 0,
                project: "p".into(),
                label: "Function".into(),
                name: "beta".into(),
                qualified_name: "p.beta".into(),
                file_path: "b.rs".into(),
                start_line: 5,
                end_line: 20,
                properties_json: None,
            },
            Node {
                id: 0,
                project: "p".into(),
                label: "Class".into(),
                name: "Gamma".into(),
                qualified_name: "p.Gamma".into(),
                file_path: "g.rs".into(),
                start_line: 1,
                end_line: 50,
                properties_json: None,
            },
        ];
        let ids: Vec<i64> = nodes.iter().map(|n| s.insert_node(n).unwrap()).collect();
        // alpha CALLS beta
        s.insert_edge(&Edge {
            id: 0,
            project: "p".into(),
            source_id: ids[0],
            target_id: ids[1],
            edge_type: "CALLS".into(),
            properties_json: None,
        })
        .unwrap();
        (s, "p")
    }

    #[test]
    fn test_match_node_returns_all() {
        let (s, p) = setup();
        let r = execute(&s, p, "MATCH (n:Function) RETURN n.name").unwrap();
        assert_eq!(r["count"], 2);
    }

    #[test]
    fn test_limit_applied() {
        let (s, p) = setup();
        let r = execute(&s, p, "MATCH (n:Function) RETURN n.name LIMIT 1").unwrap();
        assert_eq!(r["count"], 1);
    }

    #[test]
    fn test_order_by_name_asc() {
        let (s, p) = setup();
        let r = execute(&s, p, "MATCH (n:Function) RETURN n.name ORDER BY n.name").unwrap();
        let rows = r["rows"].as_array().unwrap();
        assert_eq!(rows[0]["n.name"], "alpha");
        assert_eq!(rows[1]["n.name"], "beta");
    }

    #[test]
    fn test_order_by_name_desc() {
        let (s, p) = setup();
        let r = execute(
            &s,
            p,
            "MATCH (n:Function) RETURN n.name ORDER BY n.name DESC",
        )
        .unwrap();
        let rows = r["rows"].as_array().unwrap();
        assert_eq!(rows[0]["n.name"], "beta");
    }

    #[test]
    fn test_count() {
        let (s, p) = setup();
        let r = execute(&s, p, "MATCH (n:Function) RETURN COUNT(n)").unwrap();
        assert_eq!(r["rows"][0]["count"], 2);
    }

    #[test]
    fn test_where_eq() {
        let (s, p) = setup();
        let r = execute(
            &s,
            p,
            "MATCH (n:Function) WHERE n.name = 'alpha' RETURN n.name",
        )
        .unwrap();
        assert_eq!(r["count"], 1);
        assert_eq!(r["rows"][0]["n.name"], "alpha");
    }

    #[test]
    fn test_rel_query() {
        let (s, p) = setup();
        let r = execute(
            &s,
            p,
            "MATCH (a:Function)-[r:CALLS]->(b:Function) RETURN a.name, b.name",
        )
        .unwrap();
        assert_eq!(r["count"], 1);
    }

    #[test]
    fn test_no_sql_injection_in_label() {
        let (s, p) = setup();
        // Should not panic or return unexpected results
        let r = execute(&s, p, "MATCH (n:Function') RETURN n.name");
        // Either error or empty — not a crash
        assert!(r.is_err() || r.unwrap()["count"].as_i64().unwrap_or(0) == 0);
    }
}
