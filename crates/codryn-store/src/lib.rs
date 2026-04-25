mod analytics;
pub mod compressed_store;
mod edges;
mod nodes;
mod projects;
mod queries;
mod schema;
mod types;

pub use queries::extract_semantic_keywords;
pub use types::*;

/// (name, qualified_name, label, file_path, start_line, edge_type)
pub type NeighborInfo = (String, String, String, String, i32, String);
/// (direct_dependents, all_dependents_with_depth, affected_file_paths)
pub type ImpactResult = (Vec<Node>, Vec<(Node, i32)>, Vec<String>);
/// (label_counts, outgoing_edge_counts, incoming_edge_counts)
pub type FileDiagnostics = (Vec<(String, i64)>, Vec<(String, i64)>, Vec<(String, i64)>);

/// Re-export migration function for testing purposes.
pub fn schema_migrate_tool_calls(conn: &rusqlite::Connection) {
    schema::migrate_tool_calls(conn);
}

use anyhow::Result;
use rusqlite::Connection;
use std::path::Path;

pub struct Store {
    conn: Connection,
}

impl std::fmt::Debug for Store {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Store").finish_non_exhaustive()
    }
}

impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path)?;
        let mut s = Self { conn };
        s.configure_pragmas(false)?;
        s.init_schema()?;
        Ok(s)
    }

    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        let mut s = Self { conn };
        s.configure_pragmas(true)?;
        s.init_schema()?;
        Ok(s)
    }

    fn configure_pragmas(&self, in_memory: bool) -> Result<()> {
        self.conn.execute_batch("PRAGMA foreign_keys = ON;")?;
        if !in_memory {
            self.conn.execute_batch(
                "PRAGMA journal_mode = WAL;\
                 PRAGMA synchronous = NORMAL;\
                 PRAGMA cache_size = -64000;",
            )?;
        }
        Ok(())
    }

    fn init_schema(&mut self) -> Result<()> {
        self.conn.execute_batch(schema::DDL)?;
        self.conn.execute_batch(schema::INDEXES)?;
        schema::migrate_tool_calls(&self.conn);
        Ok(())
    }
}

pub(crate) fn node_from_row(row: &rusqlite::Row) -> rusqlite::Result<Node> {
    Ok(Node {
        id: row.get(0)?,
        project: row.get(1)?,
        label: row.get(2)?,
        name: row.get(3)?,
        qualified_name: row.get(4)?,
        file_path: row.get(5)?,
        start_line: row.get(6)?,
        end_line: row.get(7)?,
        properties_json: row.get(8)?,
    })
}

pub(crate) fn edge_from_row(row: &rusqlite::Row) -> rusqlite::Result<Edge> {
    Ok(Edge {
        id: row.get(0)?,
        project: row.get(1)?,
        source_id: row.get(2)?,
        target_id: row.get(3)?,
        edge_type: row.get(4)?,
        properties_json: row.get(5)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_store() -> Store {
        Store::open_in_memory().unwrap()
    }

    #[test]
    fn test_project_crud() {
        let s = test_store();
        let p = Project {
            name: "test".into(),
            indexed_at: "2025-01-01".into(),
            root_path: "/tmp".into(),
        };
        s.upsert_project(&p).unwrap();
        let projects = s.list_projects().unwrap();
        assert_eq!(projects.len(), 1);
        assert_eq!(projects[0].name, "test");
        s.delete_project("test").unwrap();
        assert!(s.list_projects().unwrap().is_empty());
    }

    #[test]
    fn test_node_insert_and_search() {
        let s = test_store();
        s.upsert_project(&Project {
            name: "p".into(),
            indexed_at: "now".into(),
            root_path: "/".into(),
        })
        .unwrap();
        s.insert_node(&Node {
            id: 0,
            project: "p".into(),
            label: "Function".into(),
            name: "main".into(),
            qualified_name: "p.src.main".into(),
            file_path: "src/main.rs".into(),
            start_line: 1,
            end_line: 10,
            properties_json: None,
        })
        .unwrap();
        let found = s.search_nodes("p", "main", 10).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].name, "main");
    }

    #[test]
    fn test_edge_and_degree() {
        let s = test_store();
        s.upsert_project(&Project {
            name: "p".into(),
            indexed_at: "now".into(),
            root_path: "/".into(),
        })
        .unwrap();
        let id1 = s
            .insert_node(&Node {
                id: 0,
                project: "p".into(),
                label: "Function".into(),
                name: "a".into(),
                qualified_name: "p.a".into(),
                file_path: "".into(),
                start_line: 0,
                end_line: 0,
                properties_json: None,
            })
            .unwrap();
        let id2 = s
            .insert_node(&Node {
                id: 0,
                project: "p".into(),
                label: "Function".into(),
                name: "b".into(),
                qualified_name: "p.b".into(),
                file_path: "".into(),
                start_line: 0,
                end_line: 0,
                properties_json: None,
            })
            .unwrap();
        s.insert_edge(&Edge {
            id: 0,
            project: "p".into(),
            source_id: id1,
            target_id: id2,
            edge_type: "CALLS".into(),
            properties_json: None,
        })
        .unwrap();
        let (in_d, out_d) = s.node_degree(id1).unwrap();
        assert_eq!(out_d, 1);
        assert_eq!(in_d, 0);
    }

    #[test]
    fn test_schema_info() {
        let s = test_store();
        s.upsert_project(&Project {
            name: "p".into(),
            indexed_at: "now".into(),
            root_path: "/".into(),
        })
        .unwrap();
        s.insert_node(&Node {
            id: 0,
            project: "p".into(),
            label: "Function".into(),
            name: "f".into(),
            qualified_name: "p.f".into(),
            file_path: "".into(),
            start_line: 0,
            end_line: 0,
            properties_json: None,
        })
        .unwrap();
        let schema = s.get_graph_schema("p").unwrap();
        assert_eq!(schema.total_nodes, 1);
        assert_eq!(schema.node_labels[0].label, "Function");
    }

    #[test]
    fn test_imports_edge_via_suffix_lookup() {
        let s = test_store();
        s.upsert_project(&Project {
            name: "proj".into(),
            indexed_at: "now".into(),
            root_path: "/".into(),
        })
        .unwrap();
        let src_id = s
            .insert_node(&Node {
                id: 0,
                project: "proj".into(),
                label: "Module".into(),
                name: "main".into(),
                qualified_name: "proj.src.main".into(),
                file_path: "src/main.ts".into(),
                start_line: 0,
                end_line: 0,
                properties_json: None,
            })
            .unwrap();
        let tgt_id = s
            .insert_node(&Node {
                id: 0,
                project: "proj".into(),
                label: "Module".into(),
                name: "utils".into(),
                qualified_name: "proj.src.utils".into(),
                file_path: "src/utils.ts".into(),
                start_line: 0,
                end_line: 0,
                properties_json: None,
            })
            .unwrap();
        let candidates = s.find_nodes_by_qn_suffix("proj", "utils").unwrap();
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].id, tgt_id);
        s.insert_edge(&Edge {
            id: 0,
            project: "proj".into(),
            source_id: src_id,
            target_id: tgt_id,
            edge_type: "IMPORTS".into(),
            properties_json: None,
        })
        .unwrap();
        let schema = s.get_graph_schema("proj").unwrap();
        assert_eq!(schema.total_edges, 1);
    }

    #[test]
    fn test_inherits_edge() {
        let s = test_store();
        s.upsert_project(&Project {
            name: "p".into(),
            indexed_at: "now".into(),
            root_path: "/".into(),
        })
        .unwrap();
        let child_id = s
            .insert_node(&Node {
                id: 0,
                project: "p".into(),
                label: "Class".into(),
                name: "Dog".into(),
                qualified_name: "p.src.Dog".into(),
                file_path: "src/dog.ts".into(),
                start_line: 0,
                end_line: 0,
                properties_json: None,
            })
            .unwrap();
        let parent_id = s
            .insert_node(&Node {
                id: 0,
                project: "p".into(),
                label: "Class".into(),
                name: "Animal".into(),
                qualified_name: "p.src.Animal".into(),
                file_path: "src/animal.ts".into(),
                start_line: 0,
                end_line: 0,
                properties_json: None,
            })
            .unwrap();
        s.insert_edge(&Edge {
            id: 0,
            project: "p".into(),
            source_id: child_id,
            target_id: parent_id,
            edge_type: "INHERITS".into(),
            properties_json: None,
        })
        .unwrap();
        let schema = s.get_graph_schema("p").unwrap();
        assert!(schema.edge_types.iter().any(|e| e.edge_type == "INHERITS"));
    }

    #[test]
    fn test_implements_edge() {
        let s = test_store();
        s.upsert_project(&Project {
            name: "p".into(),
            indexed_at: "now".into(),
            root_path: "/".into(),
        })
        .unwrap();
        let cls_id = s
            .insert_node(&Node {
                id: 0,
                project: "p".into(),
                label: "Class".into(),
                name: "MyService".into(),
                qualified_name: "p.src.MyService".into(),
                file_path: "src/service.ts".into(),
                start_line: 0,
                end_line: 0,
                properties_json: None,
            })
            .unwrap();
        let iface_id = s
            .insert_node(&Node {
                id: 0,
                project: "p".into(),
                label: "Interface".into(),
                name: "IService".into(),
                qualified_name: "p.src.IService".into(),
                file_path: "src/iservice.ts".into(),
                start_line: 0,
                end_line: 0,
                properties_json: None,
            })
            .unwrap();
        s.insert_edge(&Edge {
            id: 0,
            project: "p".into(),
            source_id: cls_id,
            target_id: iface_id,
            edge_type: "IMPLEMENTS".into(),
            properties_json: None,
        })
        .unwrap();
        let schema = s.get_graph_schema("p").unwrap();
        assert!(schema
            .edge_types
            .iter()
            .any(|e| e.edge_type == "IMPLEMENTS"));
    }

    #[test]
    fn test_link_projects_bidirectional() {
        let s = test_store();
        s.upsert_project(&Project {
            name: "frontend".into(),
            indexed_at: "now".into(),
            root_path: "/fe".into(),
        })
        .unwrap();
        s.upsert_project(&Project {
            name: "backend".into(),
            indexed_at: "now".into(),
            root_path: "/be".into(),
        })
        .unwrap();
        s.link_projects("frontend", "backend").unwrap();
        assert_eq!(s.get_linked_projects("frontend").unwrap().len(), 1);
        assert_eq!(s.get_linked_projects("backend").unwrap().len(), 1);
    }

    #[test]
    fn test_unlink_projects() {
        let s = test_store();
        s.upsert_project(&Project {
            name: "a".into(),
            indexed_at: "now".into(),
            root_path: "/a".into(),
        })
        .unwrap();
        s.upsert_project(&Project {
            name: "b".into(),
            indexed_at: "now".into(),
            root_path: "/b".into(),
        })
        .unwrap();
        s.link_projects("a", "b").unwrap();
        s.unlink_projects("a", "b").unwrap();
        assert!(s.get_linked_projects("a").unwrap().is_empty());
    }

    #[test]
    fn test_link_idempotent() {
        let s = test_store();
        s.upsert_project(&Project {
            name: "x".into(),
            indexed_at: "now".into(),
            root_path: "/x".into(),
        })
        .unwrap();
        s.upsert_project(&Project {
            name: "y".into(),
            indexed_at: "now".into(),
            root_path: "/y".into(),
        })
        .unwrap();
        s.link_projects("x", "y").unwrap();
        s.link_projects("x", "y").unwrap();
        assert_eq!(s.get_linked_projects("x").unwrap().len(), 1);
    }

    #[test]
    fn test_delete_project_cleans_links() {
        let s = test_store();
        s.upsert_project(&Project {
            name: "a".into(),
            indexed_at: "now".into(),
            root_path: "/a".into(),
        })
        .unwrap();
        s.upsert_project(&Project {
            name: "b".into(),
            indexed_at: "now".into(),
            root_path: "/b".into(),
        })
        .unwrap();
        s.link_projects("a", "b").unwrap();
        s.delete_project("a").unwrap();
        assert!(s.get_linked_projects("b").unwrap().is_empty());
    }

    #[test]
    fn test_adr_create_list_get() {
        let s = test_store();
        s.upsert_project(&Project {
            name: "p".into(),
            indexed_at: "now".into(),
            root_path: "/".into(),
        })
        .unwrap();
        s.create_adr("p", "ADR-001", "Use SQLite", "We chose SQLite because...")
            .unwrap();
        let list = s.list_adrs("p").unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].title, "Use SQLite");
        let adr = s.get_adr("p", "ADR-001").unwrap().unwrap();
        assert_eq!(adr.content, "We chose SQLite because...");
    }

    #[test]
    fn test_ingest_trace_creates_edge() {
        let s = test_store();
        s.upsert_project(&Project {
            name: "p".into(),
            indexed_at: "now".into(),
            root_path: "/".into(),
        })
        .unwrap();
        s.insert_node(&Node {
            id: 0,
            project: "p".into(),
            label: "Function".into(),
            name: "caller".into(),
            qualified_name: "p.caller".into(),
            file_path: "".into(),
            start_line: 0,
            end_line: 0,
            properties_json: None,
        })
        .unwrap();
        s.insert_node(&Node {
            id: 0,
            project: "p".into(),
            label: "Function".into(),
            name: "callee".into(),
            qualified_name: "p.callee".into(),
            file_path: "".into(),
            start_line: 0,
            end_line: 0,
            properties_json: None,
        })
        .unwrap();
        s.ingest_trace("p", "caller", "callee", "CALLS").unwrap();
        assert_eq!(s.get_graph_schema("p").unwrap().total_edges, 1);
    }

    #[test]
    fn test_insert_nodes_batch_returns_correct_ids() {
        let s = test_store();
        s.upsert_project(&Project {
            name: "p".into(),
            indexed_at: "now".into(),
            root_path: "/".into(),
        })
        .unwrap();
        let nodes = vec![
            Node {
                id: 0,
                project: "p".into(),
                label: "Function".into(),
                name: "f1".into(),
                qualified_name: "p.f1".into(),
                file_path: "".into(),
                start_line: 0,
                end_line: 0,
                properties_json: None,
            },
            Node {
                id: 0,
                project: "p".into(),
                label: "Function".into(),
                name: "f2".into(),
                qualified_name: "p.f2".into(),
                file_path: "".into(),
                start_line: 0,
                end_line: 0,
                properties_json: None,
            },
        ];
        let results = s.insert_nodes_batch(&nodes).unwrap();
        assert_eq!(results.len(), 2);
        assert!(results[0].1 > 0);
        assert_ne!(results[0].1, results[1].1);
    }

    #[test]
    fn test_insert_nodes_batch_idempotent() {
        let s = test_store();
        s.upsert_project(&Project {
            name: "p".into(),
            indexed_at: "now".into(),
            root_path: "/".into(),
        })
        .unwrap();
        let nodes = vec![Node {
            id: 0,
            project: "p".into(),
            label: "Function".into(),
            name: "f1".into(),
            qualified_name: "p.f1".into(),
            file_path: "".into(),
            start_line: 0,
            end_line: 0,
            properties_json: None,
        }];
        let r1 = s.insert_nodes_batch(&nodes).unwrap();
        let r2 = s.insert_nodes_batch(&nodes).unwrap();
        assert_eq!(r1[0].1, r2[0].1);
    }

    fn setup_graph() -> (Store, i64, i64, i64) {
        let s = test_store();
        s.upsert_project(&Project {
            name: "p".into(),
            indexed_at: "now".into(),
            root_path: "/tmp".into(),
        })
        .unwrap();
        let a_id = s
            .insert_node(&Node {
                id: 0,
                project: "p".into(),
                label: "Function".into(),
                name: "getUserProfile".into(),
                qualified_name: "src/user.ts::getUserProfile".into(),
                file_path: "src/user.ts".into(),
                start_line: 10,
                end_line: 30,
                properties_json: None,
            })
            .unwrap();
        let b_id = s
            .insert_node(&Node {
                id: 0,
                project: "p".into(),
                label: "Function".into(),
                name: "handleGetUser".into(),
                qualified_name: "src/controller.ts::handleGetUser".into(),
                file_path: "src/controller.ts".into(),
                start_line: 5,
                end_line: 20,
                properties_json: None,
            })
            .unwrap();
        let c_id = s
            .insert_node(&Node {
                id: 0,
                project: "p".into(),
                label: "Class".into(),
                name: "UserService".into(),
                qualified_name: "src/service.ts::UserService".into(),
                file_path: "src/service.ts".into(),
                start_line: 1,
                end_line: 50,
                properties_json: None,
            })
            .unwrap();
        s.insert_edge(&Edge {
            id: 0,
            project: "p".into(),
            source_id: b_id,
            target_id: a_id,
            edge_type: "CALLS".into(),
            properties_json: None,
        })
        .unwrap();
        s.insert_edge(&Edge {
            id: 0,
            project: "p".into(),
            source_id: c_id,
            target_id: a_id,
            edge_type: "CALLS".into(),
            properties_json: None,
        })
        .unwrap();
        s.insert_edge(&Edge {
            id: 0,
            project: "p".into(),
            source_id: a_id,
            target_id: c_id,
            edge_type: "IMPORTS".into(),
            properties_json: None,
        })
        .unwrap();
        (s, a_id, b_id, c_id)
    }

    #[test]
    fn test_find_symbol_exact_qn() {
        let (s, _, _, _) = setup_graph();
        let results = s
            .find_symbol_ranked("p", "src/user.ts::getUserProfile", None, false, 10)
            .unwrap();
        assert!(!results.is_empty());
        assert_eq!(results[0].1, "exact_qualified_name");
    }

    #[test]
    fn test_find_symbol_exact_name() {
        let (s, _, _, _) = setup_graph();
        let results = s
            .find_symbol_ranked("p", "getUserProfile", None, false, 10)
            .unwrap();
        assert!(!results.is_empty());
        assert!(results[0].2 >= 0.9);
    }

    #[test]
    fn test_find_symbol_with_label_filter() {
        let (s, _, _, _) = setup_graph();
        assert_eq!(
            s.find_symbol_ranked("p", "UserService", Some("Class"), false, 10)
                .unwrap()
                .len(),
            1
        );
        assert!(s
            .find_symbol_ranked("p", "UserService", Some("Function"), false, 10)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn test_find_symbol_exact_mode_no_fuzzy() {
        let (s, _, _, _) = setup_graph();
        assert!(s
            .find_symbol_ranked("p", "getUser", None, true, 10)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn test_find_symbol_fuzzy_fallback() {
        let (s, _, _, _) = setup_graph();
        let results = s
            .find_symbol_ranked("p", "getUser", None, false, 10)
            .unwrap();
        assert!(results.iter().any(|(n, _, _)| n.name == "getUserProfile"));
    }

    #[test]
    fn test_neighbors_incoming_calls() {
        let (s, a_id, _, _) = setup_graph();
        assert_eq!(
            s.node_neighbors_detailed(a_id, "in", Some(&["CALLS"]), 10)
                .unwrap()
                .len(),
            2
        );
    }

    #[test]
    fn test_neighbors_outgoing_imports() {
        let (s, a_id, _, _) = setup_graph();
        let imports = s
            .node_neighbors_detailed(a_id, "out", Some(&["IMPORTS"]), 10)
            .unwrap();
        assert_eq!(imports.len(), 1);
        assert_eq!(imports[0].0, "UserService");
    }

    #[test]
    fn test_incoming_references_all() {
        let (s, a_id, _, _) = setup_graph();
        assert_eq!(s.incoming_references(a_id, None, 10).unwrap().len(), 2);
    }

    #[test]
    fn test_incoming_references_filtered() {
        let (s, a_id, _, _) = setup_graph();
        assert!(s
            .incoming_references(a_id, Some(&["IMPORTS"]), 10)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn test_impact_bfs_direct() {
        let (s, a_id, _, _) = setup_graph();
        let (direct, _all, files) = s.impact_bfs(a_id, 3, 50).unwrap();
        assert_eq!(direct.len(), 2);
        assert!(files.contains(&"src/controller.ts".to_string()));
    }

    #[test]
    fn test_impact_bfs_depth_limit() {
        let (s, a_id, _, _) = setup_graph();
        let (_direct, all, _) = s.impact_bfs(a_id, 1, 50).unwrap();
        assert!(all.iter().all(|(_, d)| *d == 1));
    }

    #[test]
    fn test_file_diagnostics() {
        let (s, _, _, _) = setup_graph();
        let (labels, out_edges, in_edges) = s.file_diagnostics("p", "src/user.ts").unwrap();
        assert!(labels.iter().any(|(l, c)| l == "Function" && *c == 1));
        assert!(!out_edges.is_empty());
        assert!(!in_edges.is_empty());
    }

    #[test]
    fn test_has_file_hash() {
        let (s, _, _, _) = setup_graph();
        assert!(!s.has_file_hash("p", "src/user.ts").unwrap());
        s.upsert_file_hash_batch(&[FileHash {
            project: "p".into(),
            rel_path: "src/user.ts".into(),
            sha256: "abc".into(),
            mtime_ns: 0,
            size: 100,
        }])
        .unwrap();
        assert!(s.has_file_hash("p", "src/user.ts").unwrap());
    }

    #[test]
    fn test_count_nodes_for_file() {
        let (s, _, _, _) = setup_graph();
        assert_eq!(s.count_nodes_for_file("p", "src/user.ts").unwrap(), 1);
        assert_eq!(s.count_nodes_for_file("p", "nonexistent.ts").unwrap(), 0);
    }

    #[test]
    fn test_find_nodes_by_name() {
        let (s, _, _, _) = setup_graph();
        let nodes = s.find_nodes_by_name("p", "getUserProfile", 10).unwrap();
        assert_eq!(nodes.len(), 1);
    }

    #[test]
    fn test_search_multi_word() {
        let s = test_store();
        s.upsert_project(&Project {
            name: "p".into(),
            indexed_at: "now".into(),
            root_path: "/".into(),
        })
        .unwrap();
        s.insert_node(&Node {
            id: 0,
            project: "p".into(),
            label: "Class".into(),
            name: "EditTravelRequestComponent".into(),
            qualified_name: "p.EditTravelRequestComponent".into(),
            file_path: "src/edit.ts".into(),
            start_line: 1,
            end_line: 50,
            properties_json: None,
        })
        .unwrap();
        assert_eq!(s.search_nodes("p", "edit travel", 10).unwrap().len(), 1);
        assert_eq!(s.search_nodes("p", "EditTravel", 10).unwrap().len(), 1);
    }

    #[test]
    fn test_get_nodes_for_file() {
        let s = test_store();
        s.upsert_project(&Project {
            name: "p".into(),
            indexed_at: "now".into(),
            root_path: "/".into(),
        })
        .unwrap();
        s.insert_node(&Node {
            id: 0,
            project: "p".into(),
            label: "Function".into(),
            name: "a".into(),
            qualified_name: "p.a".into(),
            file_path: "src/main.ts".into(),
            start_line: 10,
            end_line: 20,
            properties_json: None,
        })
        .unwrap();
        s.insert_node(&Node {
            id: 0,
            project: "p".into(),
            label: "Function".into(),
            name: "b".into(),
            qualified_name: "p.b".into(),
            file_path: "src/main.ts".into(),
            start_line: 1,
            end_line: 5,
            properties_json: None,
        })
        .unwrap();
        let nodes = s.get_nodes_for_file("p", "src/main.ts").unwrap();
        assert_eq!(nodes.len(), 2);
        assert_eq!(nodes[0].name, "b");
    }

    #[test]
    fn test_get_file_edge_counts() {
        let s = test_store();
        s.upsert_project(&Project {
            name: "p".into(),
            indexed_at: "now".into(),
            root_path: "/".into(),
        })
        .unwrap();
        let id1 = s
            .insert_node(&Node {
                id: 0,
                project: "p".into(),
                label: "Function".into(),
                name: "a".into(),
                qualified_name: "p.a".into(),
                file_path: "src/a.ts".into(),
                start_line: 0,
                end_line: 0,
                properties_json: None,
            })
            .unwrap();
        let id2 = s
            .insert_node(&Node {
                id: 0,
                project: "p".into(),
                label: "Function".into(),
                name: "b".into(),
                qualified_name: "p.b".into(),
                file_path: "src/b.ts".into(),
                start_line: 0,
                end_line: 0,
                properties_json: None,
            })
            .unwrap();
        s.insert_edge(&Edge {
            id: 0,
            project: "p".into(),
            source_id: id1,
            target_id: id2,
            edge_type: "CALLS".into(),
            properties_json: None,
        })
        .unwrap();
        let (inbound, outbound) = s.get_file_edge_counts("p", "src/a.ts").unwrap();
        assert_eq!(outbound, 1);
        assert_eq!(inbound, 0);
    }

    #[test]
    fn test_get_edges_from_node() {
        let s = test_store();
        s.upsert_project(&Project {
            name: "p".into(),
            indexed_at: "now".into(),
            root_path: "/".into(),
        })
        .unwrap();
        let id1 = s
            .insert_node(&Node {
                id: 0,
                project: "p".into(),
                label: "Function".into(),
                name: "a".into(),
                qualified_name: "p.a".into(),
                file_path: "".into(),
                start_line: 0,
                end_line: 0,
                properties_json: None,
            })
            .unwrap();
        let id2 = s
            .insert_node(&Node {
                id: 0,
                project: "p".into(),
                label: "Function".into(),
                name: "b".into(),
                qualified_name: "p.b".into(),
                file_path: "".into(),
                start_line: 0,
                end_line: 0,
                properties_json: None,
            })
            .unwrap();
        s.insert_edge(&Edge {
            id: 0,
            project: "p".into(),
            source_id: id1,
            target_id: id2,
            edge_type: "CALLS".into(),
            properties_json: None,
        })
        .unwrap();
        assert_eq!(s.get_edges_from_node(id1, "out", 10).unwrap().len(), 1);
        assert_eq!(s.get_edges_from_node(id2, "in", 10).unwrap().len(), 1);
    }

    #[test]
    fn test_find_routes_scope_normalization() {
        let s = test_store();
        s.upsert_project(&Project {
            name: "p".into(),
            indexed_at: "now".into(),
            root_path: "/tmp".into(),
        })
        .unwrap();
        // Add file hash so route isn't filtered as stale
        s.upsert_file_hash_batch(&[FileHash {
            project: "p".into(),
            rel_path: "src/controller/TravelRequestController.java".into(),
            sha256: "abc".into(),
            mtime_ns: 0,
            size: 100,
        }])
        .unwrap();
        s.insert_node(&Node {
            id: 0,
            project: "p".into(),
            label: "Route".into(),
            name: "PATCH /v1/travelrequest/{id}".into(),
            qualified_name: "p.route.PATCH./v1/travelrequest/{id}".into(),
            file_path: "src/controller/TravelRequestController.java".into(),
            start_line: 1,
            end_line: 1,
            properties_json: Some(
                r#"{"http_method":"PATCH","path":"/v1/travelrequest/{id}"}"#.into(),
            ),
        })
        .unwrap();
        // Scope "travel-request" should match via normalization
        let routes = s
            .find_routes("p", Some("travel-request"), None, 20, false)
            .unwrap();
        assert!(!routes.is_empty(), "scope 'travel-request' should match");
        assert!(routes[0].score > 0.5);
    }

    #[test]
    fn test_find_routes_handler_fallback() {
        let s = test_store();
        s.upsert_project(&Project {
            name: "p".into(),
            indexed_at: "now".into(),
            root_path: "/tmp".into(),
        })
        .unwrap();
        s.upsert_file_hash_batch(&[FileHash {
            project: "p".into(),
            rel_path: "src/api.java".into(),
            sha256: "abc".into(),
            mtime_ns: 0,
            size: 100,
        }])
        .unwrap();
        s.insert_node(&Node {
            id: 0,
            project: "p".into(),
            label: "Route".into(),
            name: "GET /users".into(),
            qualified_name: "p.route.GET.users".into(),
            file_path: "src/api.java".into(),
            start_line: 1,
            end_line: 1,
            properties_json: Some(r#"{"http_method":"GET","path":"/users"}"#.into()),
        })
        .unwrap();
        // No HANDLES_ROUTE edge — handler should be derived
        let routes = s.find_routes("p", None, None, 20, false).unwrap();
        assert!(!routes.is_empty());
        assert!(!routes[0].handler.is_empty(), "handler should not be empty");
        assert!(
            routes[0].extraction_confidence < 1.0,
            "confidence should be < 1.0 for derived handler"
        );
    }

    #[test]
    fn test_find_routes_stale_filtering() {
        let s = test_store();
        s.upsert_project(&Project {
            name: "p".into(),
            indexed_at: "now".into(),
            root_path: "/tmp".into(),
        })
        .unwrap();
        // Route with NO file hash → stale
        s.insert_node(&Node {
            id: 0,
            project: "p".into(),
            label: "Route".into(),
            name: "GET /stale".into(),
            qualified_name: "p.route.GET.stale".into(),
            file_path: "src/deleted.java".into(),
            start_line: 1,
            end_line: 1,
            properties_json: Some(r#"{"http_method":"GET","path":"/stale"}"#.into()),
        })
        .unwrap();
        // Default: stale filtered out
        let routes = s.find_routes("p", None, None, 20, false).unwrap();
        assert!(routes.is_empty(), "stale route should be filtered");
        // include_deleted: stale included
        let routes = s.find_routes("p", None, None, 20, true).unwrap();
        assert!(
            !routes.is_empty(),
            "stale route should be included with include_deleted"
        );
    }

    #[test]
    fn test_get_nodes_by_label() {
        let s = test_store();
        s.upsert_project(&Project {
            name: "p".into(),
            indexed_at: "now".into(),
            root_path: "/".into(),
        })
        .unwrap();
        s.insert_node(&Node {
            id: 0,
            project: "p".into(),
            label: "Function".into(),
            name: "f".into(),
            qualified_name: "p.f".into(),
            file_path: "".into(),
            start_line: 0,
            end_line: 0,
            properties_json: None,
        })
        .unwrap();
        s.insert_node(&Node {
            id: 0,
            project: "p".into(),
            label: "Class".into(),
            name: "C".into(),
            qualified_name: "p.C".into(),
            file_path: "".into(),
            start_line: 0,
            end_line: 0,
            properties_json: None,
        })
        .unwrap();
        assert_eq!(s.get_nodes_by_label("p", "Function", 10).unwrap().len(), 1);
    }

    #[test]
    fn test_mark_deleted_files() {
        let s = test_store();
        s.upsert_project(&Project {
            name: "p".into(),
            indexed_at: "now".into(),
            root_path: "/".into(),
        })
        .unwrap();
        s.upsert_file_hash_batch(&[
            FileHash {
                project: "p".into(),
                rel_path: "src/a.rs".into(),
                sha256: "a".into(),
                mtime_ns: 0,
                size: 1,
            },
            FileHash {
                project: "p".into(),
                rel_path: "src/b.rs".into(),
                sha256: "b".into(),
                mtime_ns: 0,
                size: 1,
            },
            FileHash {
                project: "p".into(),
                rel_path: "src/c.rs".into(),
                sha256: "c".into(),
                mtime_ns: 0,
                size: 1,
            },
        ])
        .unwrap();
        // Only a.rs and c.rs are still live
        let deleted = s
            .mark_deleted_files("p", &["src/a.rs".into(), "src/c.rs".into()])
            .unwrap();
        assert_eq!(deleted, 1); // b.rs marked deleted
        assert!(s.has_file_hash("p", "src/a.rs").unwrap());
        assert!(!s.has_file_hash("p", "src/b.rs").unwrap()); // deleted
        assert!(s.has_file_hash("p", "src/c.rs").unwrap());
    }

    #[test]
    fn test_has_file_hash_excludes_deleted() {
        let s = test_store();
        s.upsert_project(&Project {
            name: "p".into(),
            indexed_at: "now".into(),
            root_path: "/".into(),
        })
        .unwrap();
        s.upsert_file_hash_batch(&[FileHash {
            project: "p".into(),
            rel_path: "src/alive.rs".into(),
            sha256: "x".into(),
            mtime_ns: 0,
            size: 1,
        }])
        .unwrap();
        assert!(s.has_file_hash("p", "src/alive.rs").unwrap());
        // Mark as deleted
        s.mark_deleted_files("p", &[]).unwrap();
        assert!(!s.has_file_hash("p", "src/alive.rs").unwrap());
    }

    // ── BM25 Full-Text Search Tests ──────────────────────

    #[test]
    fn test_extract_semantic_keywords_removes_stop_words() {
        use crate::queries::extract_semantic_keywords;
        let result = extract_semantic_keywords("the function is in the module");
        // "the", "is", "in" are stop words; "function" and "module" remain
        assert!(result.contains(&"function".to_string()));
        assert!(result.contains(&"module".to_string()));
        assert!(!result.contains(&"the".to_string()));
        assert!(!result.contains(&"is".to_string()));
        assert!(!result.contains(&"in".to_string()));
    }

    #[test]
    fn test_extract_semantic_keywords_filters_single_char() {
        use crate::queries::extract_semantic_keywords;
        let result = extract_semantic_keywords("a b function c module");
        // Single-char tokens "a", "b", "c" should be removed
        assert_eq!(result, vec!["function", "module"]);
    }

    #[test]
    fn test_extract_semantic_keywords_lowercases() {
        use crate::queries::extract_semantic_keywords;
        let result = extract_semantic_keywords("UserProfile Handler");
        assert!(result.iter().all(|t| t == &t.to_lowercase()));
    }

    #[test]
    fn test_extract_semantic_keywords_strips_non_alphanumeric() {
        use crate::queries::extract_semantic_keywords;
        let result = extract_semantic_keywords("user-profile handler.method");
        assert!(result.contains(&"userprofile".to_string()));
        assert!(result.contains(&"handlermethod".to_string()));
    }

    #[test]
    fn test_extract_semantic_keywords_empty_after_stop_words() {
        use crate::queries::extract_semantic_keywords;
        let result = extract_semantic_keywords("the is a an");
        assert!(result.is_empty());
    }

    #[test]
    fn test_bm25_ranking_orders_multi_term_matches_higher() {
        let s = test_store();
        s.upsert_project(&Project {
            name: "p".into(),
            indexed_at: "now".into(),
            root_path: "/".into(),
        })
        .unwrap();

        // Insert nodes
        s.insert_node(&Node {
            id: 0,
            project: "p".into(),
            label: "Function".into(),
            name: "getUserProfile".into(),
            qualified_name: "p.src.getUserProfile".into(),
            file_path: "src/user.ts".into(),
            start_line: 1,
            end_line: 10,
            properties_json: None,
        })
        .unwrap();
        s.insert_node(&Node {
            id: 0,
            project: "p".into(),
            label: "Function".into(),
            name: "getProfile".into(),
            qualified_name: "p.src.getProfile".into(),
            file_path: "src/profile.ts".into(),
            start_line: 1,
            end_line: 10,
            properties_json: None,
        })
        .unwrap();

        // Index FTS content — first doc matches both "user" and "profile",
        // second doc matches only "profile"
        s.upsert_code_content_batch(&[
            (
                "p".into(),
                "p.src.getUserProfile".into(),
                "function getUserProfile user profile data retrieval".into(),
            ),
            (
                "p".into(),
                "p.src.getProfile".into(),
                "function getProfile profile data".into(),
            ),
        ])
        .unwrap();

        let results = s.search_code_fts_bm25("p", "user profile", 10).unwrap();
        assert!(
            !results.is_empty(),
            "expected at least 1 BM25 result, got {}",
            results.len()
        );

        // If both results are returned, the one matching more terms should rank higher
        // (BM25 returns negative scores; more negative = better match)
        if results.len() >= 2 {
            let first_name = &results[0].0.name;
            assert_eq!(
                first_name, "getUserProfile",
                "multi-term match should rank higher"
            );
            // First result should have a better (more negative) score
            assert!(
                results[0].1 <= results[1].1,
                "first result score {} should be <= second {} (more negative = better)",
                results[0].1,
                results[1].1
            );
        }
    }

    #[test]
    fn test_bm25_relevance_score_exposed() {
        let s = test_store();
        s.upsert_project(&Project {
            name: "p".into(),
            indexed_at: "now".into(),
            root_path: "/".into(),
        })
        .unwrap();
        s.insert_node(&Node {
            id: 0,
            project: "p".into(),
            label: "Function".into(),
            name: "processOrder".into(),
            qualified_name: "p.src.processOrder".into(),
            file_path: "src/order.ts".into(),
            start_line: 1,
            end_line: 10,
            properties_json: None,
        })
        .unwrap();
        s.upsert_code_content_batch(&[(
            "p".into(),
            "p.src.processOrder".into(),
            "function processOrder order processing logic".into(),
        )])
        .unwrap();

        let results = s.search_code_fts_bm25("p", "order processing", 10).unwrap();
        assert!(!results.is_empty(), "expected BM25 results");
        // Score should be a finite number (BM25 returns negative values)
        let score = results[0].1;
        assert!(
            score.is_finite(),
            "BM25 score should be finite, got {}",
            score
        );
    }

    #[test]
    fn test_bm25_empty_query_returns_empty() {
        let s = test_store();
        s.upsert_project(&Project {
            name: "p".into(),
            indexed_at: "now".into(),
            root_path: "/".into(),
        })
        .unwrap();
        // Query with only stop words
        let results = s.search_code_fts_bm25("p", "the is a an", 10).unwrap();
        assert!(
            results.is_empty(),
            "stop-word-only query should return empty results"
        );
    }

    #[test]
    fn test_search_nodes_broad_bm25_returns_scored_results() {
        let s = test_store();
        s.upsert_project(&Project {
            name: "p".into(),
            indexed_at: "now".into(),
            root_path: "/".into(),
        })
        .unwrap();
        s.insert_node(&Node {
            id: 0,
            project: "p".into(),
            label: "Function".into(),
            name: "handleRequest".into(),
            qualified_name: "p.src.handleRequest".into(),
            file_path: "src/handler.ts".into(),
            start_line: 1,
            end_line: 10,
            properties_json: None,
        })
        .unwrap();
        s.upsert_code_content_batch(&[(
            "p".into(),
            "p.src.handleRequest".into(),
            "function handleRequest request handler logic".into(),
        )])
        .unwrap();

        let results = s
            .search_nodes_broad_bm25("p", "handleRequest", None, 10)
            .unwrap();
        assert!(!results.is_empty(), "expected scored results");
        // Each result should have a score
        for (node, score) in &results {
            assert!(
                score.is_finite(),
                "score for {} should be finite",
                node.name
            );
        }
    }

    #[test]
    fn test_search_nodes_broad_uses_bm25() {
        let s = test_store();
        s.upsert_project(&Project {
            name: "p".into(),
            indexed_at: "now".into(),
            root_path: "/".into(),
        })
        .unwrap();
        s.insert_node(&Node {
            id: 0,
            project: "p".into(),
            label: "Function".into(),
            name: "calculateTotal".into(),
            qualified_name: "p.src.calculateTotal".into(),
            file_path: "src/calc.ts".into(),
            start_line: 1,
            end_line: 10,
            properties_json: None,
        })
        .unwrap();
        s.upsert_code_content_batch(&[(
            "p".into(),
            "p.src.calculateTotal".into(),
            "function calculateTotal total calculation with tax and discount".into(),
        )])
        .unwrap();

        // search_nodes_broad should still work and find results via BM25
        let results = s
            .search_nodes_broad("p", "total calculation", None, 10)
            .unwrap();
        assert!(
            !results.is_empty(),
            "search_nodes_broad should find results via BM25"
        );
    }

    #[test]
    fn test_search_nodes_broad_bm25_label_filter() {
        let s = test_store();
        s.upsert_project(&Project {
            name: "p".into(),
            indexed_at: "now".into(),
            root_path: "/".into(),
        })
        .unwrap();
        s.insert_node(&Node {
            id: 0,
            project: "p".into(),
            label: "Function".into(),
            name: "myHelper".into(),
            qualified_name: "p.src.myHelper".into(),
            file_path: "src/helper.ts".into(),
            start_line: 1,
            end_line: 10,
            properties_json: None,
        })
        .unwrap();
        s.insert_node(&Node {
            id: 0,
            project: "p".into(),
            label: "Class".into(),
            name: "HelperService".into(),
            qualified_name: "p.src.HelperService".into(),
            file_path: "src/helper.ts".into(),
            start_line: 20,
            end_line: 50,
            properties_json: None,
        })
        .unwrap();

        // Filter by Class label — should only return HelperService
        let results = s
            .search_nodes_broad_bm25("p", "helper", Some("Class"), 10)
            .unwrap();
        assert!(results.iter().all(|(n, _)| n.label == "Class"));
    }

    // ── Compressed Code Storage Tests ─────────────────────

    #[test]
    fn test_compressed_round_trip_small() {
        // Content below threshold should round-trip unchanged
        let content = "fn main() { println!(\"hello\"); }";
        let compressed = crate::compressed_store::maybe_compress(content);
        let decompressed = crate::compressed_store::maybe_decompress(&compressed);
        assert_eq!(decompressed, content);
    }

    #[test]
    fn test_compressed_round_trip_large() {
        // Content above threshold should round-trip unchanged
        let content: String = "fn process_data() { /* lots of code */ }\n".repeat(100);
        assert!(content.len() > crate::compressed_store::COMPRESSION_THRESHOLD);
        let compressed = crate::compressed_store::maybe_compress(&content);
        let decompressed = crate::compressed_store::maybe_decompress(&compressed);
        assert_eq!(decompressed, content);
    }

    #[test]
    fn test_below_threshold_stored_uncompressed() {
        let content = "short snippet";
        assert!(content.len() < crate::compressed_store::COMPRESSION_THRESHOLD);
        let compressed = crate::compressed_store::maybe_compress(content);
        // Should be raw bytes, no prefix
        assert!(!compressed.starts_with(crate::compressed_store::COMPRESSED_PREFIX));
        assert_eq!(compressed, content.as_bytes());
    }

    #[test]
    fn test_above_threshold_stored_compressed() {
        // Repetitive content compresses well
        let content: String =
            "function processUserData(userId, userData) {\n    // process\n}\n".repeat(50);
        assert!(content.len() > crate::compressed_store::COMPRESSION_THRESHOLD);
        let compressed = crate::compressed_store::maybe_compress(&content);
        // Should have the prefix and be smaller than original
        assert!(compressed.starts_with(crate::compressed_store::COMPRESSED_PREFIX));
        assert!(compressed.len() < content.len());
    }

    #[test]
    fn test_decompression_failure_returns_raw() {
        // Craft data with the prefix but invalid zstd payload
        let mut bad_data = crate::compressed_store::COMPRESSED_PREFIX.to_vec();
        bad_data.extend_from_slice(b"\xff\xff\xff\xff");
        let result = crate::compressed_store::maybe_decompress(&bad_data);
        // Should return the raw bytes as lossy UTF-8 rather than panicking
        assert!(!result.is_empty());
    }

    #[test]
    fn test_store_compressed_round_trip() {
        let s = test_store();
        s.upsert_project(&Project {
            name: "p".into(),
            indexed_at: "now".into(),
            root_path: "/".into(),
        })
        .unwrap();

        // Small content (below threshold)
        let small = "fn hello() {}";
        s.upsert_code_content_compressed(&[("p".into(), "p.hello".into(), small.into())])
            .unwrap();
        let read = s.get_code_content("p", "p.hello").unwrap();
        assert_eq!(read, Some(small.to_string()));

        // Large content (above threshold)
        let large: String = "pub fn big_function() { /* code */ }\n".repeat(100);
        s.upsert_code_content_compressed(&[("p".into(), "p.big".into(), large.clone())])
            .unwrap();
        let read = s.get_code_content("p", "p.big").unwrap();
        assert_eq!(read, Some(large));
    }

    #[test]
    fn test_get_code_content_nonexistent() {
        let s = test_store();
        s.upsert_project(&Project {
            name: "p".into(),
            indexed_at: "now".into(),
            root_path: "/".into(),
        })
        .unwrap();
        let result = s.get_code_content("p", "p.nonexistent").unwrap();
        assert_eq!(result, None);
    }

    #[test]
    fn test_compressed_fts_still_searchable() {
        // Verify that compressed content is still searchable via FTS
        let s = test_store();
        s.upsert_project(&Project {
            name: "p".into(),
            indexed_at: "now".into(),
            root_path: "/".into(),
        })
        .unwrap();
        s.insert_node(&Node {
            id: 0,
            project: "p".into(),
            label: "Function".into(),
            name: "processOrder".into(),
            qualified_name: "p.processOrder".into(),
            file_path: "src/order.ts".into(),
            start_line: 1,
            end_line: 50,
            properties_json: None,
        })
        .unwrap();

        let content: String =
            "function processOrder(orderId) {\n    // order processing logic\n}\n".repeat(50);
        s.upsert_code_content_compressed(&[("p".into(), "p.processOrder".into(), content.clone())])
            .unwrap();

        // FTS search should still find it (uncompressed content in FTS)
        let results = s.search_code_fts("p", "processOrder", 10).unwrap();
        assert!(!results.is_empty());

        // And blob read should return original content
        let read = s.get_code_content("p", "p.processOrder").unwrap();
        assert_eq!(read, Some(content));
    }

    #[test]
    fn test_delete_project_cleans_code_blobs() {
        let s = test_store();
        s.upsert_project(&Project {
            name: "p".into(),
            indexed_at: "now".into(),
            root_path: "/".into(),
        })
        .unwrap();
        s.upsert_code_content_compressed(&[("p".into(), "p.fn1".into(), "some content".into())])
            .unwrap();
        assert!(s.get_code_content("p", "p.fn1").unwrap().is_some());

        s.delete_project("p").unwrap();
        // After project deletion, code_blobs should be cleaned
        // Re-create project to query (schema still exists)
        s.upsert_project(&Project {
            name: "p".into(),
            indexed_at: "now".into(),
            root_path: "/".into(),
        })
        .unwrap();
        assert!(s.get_code_content("p", "p.fn1").unwrap().is_none());
    }
}
