use crate::CodrynServer;
use codryn_store::Store;
use serde_json::{json, Value};
use std::path::Path;

impl CodrynServer {
    pub(crate) fn explain_file(&self, store: &Store, project: &str, fp: &str) -> String {
        let projects = store.list_projects().unwrap_or_default();
        let root = projects
            .iter()
            .find(|p| p.name == project)
            .map(|p| p.root_path.as_str())
            .unwrap_or("");
        let full_path = Path::new(root).join(fp);
        let file_exists = full_path.exists();
        let has_hash = store.has_file_hash(project, fp).unwrap_or(false);
        let lang = codryn_discover::detect_language(fp);
        let lang_str = if lang == codryn_discover::Language::Unknown {
            "unknown".to_string()
        } else {
            format!("{:?}", lang)
        };
        let node_count = store.count_nodes_for_file(project, fp).unwrap_or(0);
        let (label_counts, out_edges, in_edges) =
            store.file_diagnostics(project, fp).unwrap_or_default();

        let symbol_counts: Value = label_counts
            .iter()
            .map(|(l, c)| (l.clone(), json!(c)))
            .collect::<serde_json::Map<String, Value>>()
            .into();
        let out_edge_counts: Value = out_edges
            .iter()
            .map(|(t, c)| (t.clone(), json!(c)))
            .collect::<serde_json::Map<String, Value>>()
            .into();
        let in_edge_counts: Value = in_edges
            .iter()
            .map(|(t, c)| (t.clone(), json!(c)))
            .collect::<serde_json::Map<String, Value>>()
            .into();
        let total_out: i64 = out_edges.iter().map(|(_, c)| c).sum();
        let total_in: i64 = in_edges.iter().map(|(_, c)| c).sum();

        let mut diagnostics: Vec<String> = Vec::new();
        if !file_exists {
            diagnostics
                .push("File does not exist on disk — may have been deleted after indexing".into());
        }
        if lang == codryn_discover::Language::Unknown {
            diagnostics.push(
                "Unsupported or unrecognized language — file is skipped during indexing".into(),
            );
        }
        if file_exists && !has_hash {
            diagnostics.push("File exists but has no hash — it may not be discoverable (e.g. gitignored or generated)".into());
        }
        if has_hash && node_count == 0 {
            diagnostics.push("File was discovered but has no symbols — possible parse failure or no extractable definitions".into());
        }
        if node_count > 0 && total_out == 0 {
            diagnostics.push("Symbols exist but no outgoing edges — semantic pass may have been skipped (fast mode)".into());
        }
        if diagnostics.is_empty() && node_count > 0 {
            diagnostics.push("File is indexed normally".into());
        }

        json!({
            "project": project, "target": {"file_path": fp},
            "status": { "file_exists_on_disk": file_exists, "has_file_hash": has_hash, "language": lang_str, "indexed_nodes": node_count, "outgoing_edges": total_out, "incoming_edges": total_in },
            "symbol_counts": symbol_counts, "outgoing_edge_counts": out_edge_counts, "incoming_edge_counts": in_edge_counts, "diagnostics": diagnostics,
        }).to_string()
    }

    pub(crate) fn explain_symbol_qn(&self, store: &Store, project: &str, qn: &str) -> String {
        match store.find_node_by_qn(project, qn) {
            Ok(Some(n)) => {
                let (in_deg, out_deg) = store.node_degree(n.id).unwrap_or((0, 0));
                let has_hash = store.has_file_hash(project, &n.file_path).unwrap_or(false);
                let mut diags: Vec<String> = Vec::new();
                if in_deg == 0 && out_deg == 0 {
                    diags.push(
                        "Symbol has no edges — may be isolated or semantic pass was skipped".into(),
                    );
                }
                if !has_hash {
                    diags.push("Parent file has no hash — may not be properly indexed".into());
                }
                if diags.is_empty() {
                    diags.push("Symbol is indexed normally".into());
                }
                json!({
                    "project": project, "target": {"qualified_name": qn},
                    "status": { "exists": true, "resolved_via": "exact_qualified_name", "label": n.label, "file_path": n.file_path, "start_line": n.start_line, "end_line": n.end_line, "file_indexed": has_hash, "incoming_edges": in_deg, "outgoing_edges": out_deg },
                    "diagnostics": diags,
                }).to_string()
            }
            Ok(None) => {
                let suffix_matches = store
                    .find_nodes_by_qn_suffix(project, qn)
                    .unwrap_or_default();
                let mut diags = vec!["Symbol not found by exact qualified name".to_string()];
                if !suffix_matches.is_empty() {
                    diags.push(format!(
                        "Found {} similar symbols by suffix match",
                        suffix_matches.len()
                    ));
                }
                let similar: Vec<Value> = suffix_matches.iter().take(5).map(|n| json!({"qualified_name": n.qualified_name, "label": n.label, "file_path": n.file_path})).collect();
                json!({"project": project, "target": {"qualified_name": qn}, "status": {"exists": false}, "similar_symbols": similar, "diagnostics": diags}).to_string()
            }
            Err(e) => json!({"error": e.to_string()}).to_string(),
        }
    }

    pub(crate) fn explain_symbol_name(&self, store: &Store, project: &str, name: &str) -> String {
        match store.find_symbol_ranked(project, name, None, false, 5) {
            Ok(matches) if !matches.is_empty() => {
                let (n, mt, _) = &matches[0];
                let (in_deg, out_deg) = store.node_degree(n.id).unwrap_or((0, 0));
                let mut diags = vec![format!("Resolved via {}", mt)];
                if matches.len() > 1 { diags.push(format!("{} total matches found — showing best", matches.len())); }
                if in_deg == 0 && out_deg == 0 { diags.push("Symbol has no edges".into()); }
                let alts: Vec<Value> = matches.iter().skip(1).map(|(n, _, _)| json!({"qualified_name": n.qualified_name, "label": n.label})).collect();
                json!({
                    "project": project, "target": {"name": name},
                    "status": { "exists": true, "resolved_via": mt, "qualified_name": n.qualified_name, "label": n.label, "file_path": n.file_path, "incoming_edges": in_deg, "outgoing_edges": out_deg },
                    "alternatives": alts, "diagnostics": diags,
                }).to_string()
            }
            Ok(_) => json!({"project": project, "target": {"name": name}, "status": {"exists": false}, "diagnostics": ["Symbol not found by any resolution strategy"]}).to_string(),
            Err(e) => json!({"error": e.to_string()}).to_string(),
        }
    }
}
