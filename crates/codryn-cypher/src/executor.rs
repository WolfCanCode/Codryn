use crate::parser;
use crate::*;
use anyhow::Result;
use codryn_store::Store;
use rusqlite::params;
use serde_json::{json, Value as JsonValue};

/// Execute a Cypher query against the store and return JSON results.
pub fn execute(store: &Store, project: &str, query_str: &str) -> Result<JsonValue> {
    let query = parser::parse(query_str)?;

    match &query.match_clause.patterns[0] {
        Pattern::Node(node_pat) => execute_node_query(store, project, &query, node_pat),
        Pattern::Relationship(src, rel, tgt) => {
            execute_rel_query(store, project, &query, src, rel, tgt)
        }
    }
}

fn execute_node_query(
    store: &Store,
    project: &str,
    query: &CypherQuery,
    node_pat: &NodePattern,
) -> Result<JsonValue> {
    let label_filter = node_pat.label.as_deref();
    let limit = query.limit.unwrap_or(100);

    let mut sql = String::from(
        "SELECT id, project, label, name, qualified_name, file_path, start_line, end_line, properties FROM nodes WHERE project = ?1",
    );
    if label_filter.is_some() {
        sql.push_str(" AND label = ?2");
    }

    // Apply WHERE conditions
    if let Some(ref wc) = query.where_clause {
        for cond in &wc.conditions {
            match cond {
                Condition::Eq(prop, val) => {
                    let col = prop_to_column(&prop.property);
                    let val_str = value_to_sql(val);
                    sql.push_str(&format!(" AND {} = {}", col, val_str));
                }
                Condition::Contains(prop, s) => {
                    let col = prop_to_column(&prop.property);
                    sql.push_str(&format!(" AND {} LIKE '%{}%'", col, s.replace('\'', "''")));
                }
                Condition::StartsWith(prop, s) => {
                    let col = prop_to_column(&prop.property);
                    sql.push_str(&format!(" AND {} LIKE '{}%'", col, s.replace('\'', "''")));
                }
                _ => {}
            }
        }
    }

    // ORDER BY
    if let Some(ref order_items) = query.order_by {
        let parts: Vec<String> = order_items
            .iter()
            .map(|o| {
                let col = match &o.expr {
                    ReturnItem::Property(p) => prop_to_column(&p.property).to_owned(),
                    _ => "name".to_owned(),
                };
                if o.descending {
                    format!("{} DESC", col)
                } else {
                    col
                }
            })
            .collect();
        sql.push_str(&format!(" ORDER BY {}", parts.join(", ")));
    }

    sql.push_str(&format!(" LIMIT {}", limit));

    let conn = store.conn();
    let mut stmt = conn.prepare(&sql)?;
    let rows = if let Some(lbl) = label_filter {
        stmt.query_map(params![project, lbl], |row| {
            Ok(json!({
                "id": row.get::<_, i64>(0)?,
                "label": row.get::<_, String>(2)?,
                "name": row.get::<_, String>(3)?,
                "qualified_name": row.get::<_, String>(4)?,
                "file_path": row.get::<_, String>(5)?,
                "start_line": row.get::<_, i32>(6)?,
                "end_line": row.get::<_, i32>(7)?,
            }))
        })?
        .filter_map(|r| r.ok())
        .collect()
    } else {
        stmt.query_map(params![project], |row| {
            Ok(json!({
                "id": row.get::<_, i64>(0)?,
                "label": row.get::<_, String>(2)?,
                "name": row.get::<_, String>(3)?,
                "qualified_name": row.get::<_, String>(4)?,
                "file_path": row.get::<_, String>(5)?,
                "start_line": row.get::<_, i32>(6)?,
                "end_line": row.get::<_, i32>(7)?,
            }))
        })?
        .filter_map(|r| r.ok())
        .collect()
    };

    let results: Vec<JsonValue> = rows;
    Ok(format_results(
        &query.return_clause,
        &results,
        node_pat.variable.as_deref(),
    ))
}

fn execute_rel_query(
    store: &Store,
    project: &str,
    query: &CypherQuery,
    src: &NodePattern,
    rel: &RelPattern,
    tgt: &NodePattern,
) -> Result<JsonValue> {
    let limit = query.limit.unwrap_or(100);
    let src_label = src.label.as_deref().unwrap_or("%");
    let tgt_label = tgt.label.as_deref().unwrap_or("%");
    let rel_type = rel.rel_type.as_deref().unwrap_or("%");

    let mut sql =
        "SELECT s.name, s.label, s.qualified_name, t.name, t.label, t.qualified_name, e.type \
         FROM edges e \
         JOIN nodes s ON s.id = e.source_id \
         JOIN nodes t ON t.id = e.target_id \
         WHERE e.project = ?1"
            .to_string();

    if src_label != "%" {
        sql.push_str(&format!(" AND s.label = '{}'", src_label));
    }
    if tgt_label != "%" {
        sql.push_str(&format!(" AND t.label = '{}'", tgt_label));
    }
    if rel_type != "%" {
        sql.push_str(&format!(" AND e.type = '{}'", rel_type));
    }

    // Apply WHERE
    if let Some(ref wc) = query.where_clause {
        for cond in &wc.conditions {
            if let Condition::Eq(prop, val) = cond {
                let table = if Some(prop.variable.as_str()) == src.variable.as_deref() {
                    "s"
                } else if Some(prop.variable.as_str()) == tgt.variable.as_deref() {
                    "t"
                } else {
                    "s"
                };
                let col = prop_to_column(&prop.property);
                sql.push_str(&format!(" AND {}.{} = {}", table, col, value_to_sql(val)));
            }
        }
    }

    // ORDER BY for rel queries
    if let Some(ref order_items) = query.order_by {
        let parts: Vec<String> = order_items
            .iter()
            .map(|o| {
                let (table, col) = match &o.expr {
                    ReturnItem::Property(p) => {
                        let t = if Some(p.variable.as_str()) == src.variable.as_deref() {
                            "s"
                        } else {
                            "t"
                        };
                        (t, prop_to_column(&p.property).to_owned())
                    }
                    _ => ("s", "name".to_owned()),
                };
                if o.descending {
                    format!("{}.{} DESC", table, col)
                } else {
                    format!("{}.{}", table, col)
                }
            })
            .collect();
        sql.push_str(&format!(" ORDER BY {}", parts.join(", ")));
    }

    sql.push_str(&format!(" LIMIT {}", limit));

    let conn = store.conn();
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params![project], |row| {
        let src_var = src.variable.as_deref().unwrap_or("a");
        let tgt_var = tgt.variable.as_deref().unwrap_or("b");
        Ok(json!({
            format!("{}.name", src_var): row.get::<_, String>(0)?,
            format!("{}.label", src_var): row.get::<_, String>(1)?,
            format!("{}.qualified_name", src_var): row.get::<_, String>(2)?,
            format!("{}.name", tgt_var): row.get::<_, String>(3)?,
            format!("{}.label", tgt_var): row.get::<_, String>(4)?,
            format!("{}.qualified_name", tgt_var): row.get::<_, String>(5)?,
            "type": row.get::<_, String>(6)?,
        }))
    })?;

    let results: Vec<JsonValue> = rows.filter_map(|r| r.ok()).collect();
    Ok(json!({ "rows": results, "count": results.len() }))
}

fn format_results(ret: &ReturnClause, rows: &[JsonValue], _var: Option<&str>) -> JsonValue {
    let mapped: Vec<JsonValue> = rows
        .iter()
        .map(|row| {
            let mut obj = serde_json::Map::new();
            for item in &ret.items {
                match item {
                    ReturnItem::Property(prop) => {
                        let key = format!("{}.{}", prop.variable, prop.property);
                        if let Some(v) = row.get(&prop.property) {
                            obj.insert(key, v.clone());
                        }
                    }
                    ReturnItem::Variable(_) => {
                        return row.clone();
                    }
                    ReturnItem::Count(_) => {
                        obj.insert("count".into(), json!(rows.len()));
                    }
                }
            }
            JsonValue::Object(obj)
        })
        .collect();

    json!({ "rows": mapped, "count": mapped.len() })
}

fn prop_to_column(prop: &str) -> &str {
    match prop {
        "name" => "name",
        "label" => "label",
        "qualified_name" | "qualifiedName" => "qualified_name",
        "file_path" | "filePath" => "file_path",
        "start_line" | "startLine" => "start_line",
        "end_line" | "endLine" => "end_line",
        _ => "name",
    }
}

fn value_to_sql(val: &Value) -> String {
    match val {
        Value::String(s) => format!("'{}'", s.replace('\'', "''")),
        Value::Int(n) => n.to_string(),
        Value::Bool(b) => {
            if *b {
                "1".into()
            } else {
                "0".into()
            }
        }
    }
}
