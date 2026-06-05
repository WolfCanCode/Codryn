use codryn_store::{Node, Project, Store};
use std::path::Path;
use tempfile::TempDir;

fn create_store_with_data(dir: &Path) -> Store {
    let db_path = dir.join("graph.db");
    let store = Store::open(&db_path).unwrap();
    store
        .upsert_project(&Project {
            name: "test_project".into(),
            indexed_at: "2025-01-01T00:00:00Z".into(),
            root_path: "/tmp/test".into(),
        })
        .unwrap();
    store
        .insert_node(&Node {
            id: 0,
            project: "test_project".into(),
            label: "Function".into(),
            name: "hello".into(),
            qualified_name: "test_project.hello".into(),
            file_path: "src/main.rs".into(),
            start_line: 1,
            end_line: 5,
            properties_json: None,
        })
        .unwrap();
    store
}

#[test]
fn test_backup_creates_valid_database_copy() {
    let dir = TempDir::new().unwrap();
    let store = create_store_with_data(dir.path());

    let backup_path = dir.path().join("graph.db.backup");
    store.backup_to(&backup_path).unwrap();

    // Verify the backup file exists
    assert!(backup_path.exists());

    // Open the backup and verify it contains the same data
    let backup_store = Store::open(&backup_path).unwrap();
    let projects = backup_store.list_projects().unwrap();
    assert_eq!(projects.len(), 1);
    assert_eq!(projects[0].name, "test_project");

    let nodes = backup_store
        .search_nodes("test_project", "hello", 10)
        .unwrap();
    assert_eq!(nodes.len(), 1);
    assert_eq!(nodes[0].name, "hello");
}

#[test]
fn test_backup_overwrites_existing_file() {
    let dir = TempDir::new().unwrap();
    let store = create_store_with_data(dir.path());

    let backup_path = dir.path().join("graph.db.backup");

    // Create first backup
    store.backup_to(&backup_path).unwrap();
    let _size1 = std::fs::metadata(&backup_path).unwrap().len();

    // Add more data
    store
        .insert_node(&Node {
            id: 0,
            project: "test_project".into(),
            label: "Function".into(),
            name: "world".into(),
            qualified_name: "test_project.world".into(),
            file_path: "src/lib.rs".into(),
            start_line: 1,
            end_line: 3,
            properties_json: None,
        })
        .unwrap();

    // Create second backup (should overwrite)
    store.backup_to(&backup_path).unwrap();

    // Verify the backup contains the new data
    let backup_store = Store::open(&backup_path).unwrap();
    let nodes = backup_store
        .search_nodes("test_project", "world", 10)
        .unwrap();
    assert_eq!(nodes.len(), 1);
}

#[test]
fn test_restore_replaces_database() {
    let dir = TempDir::new().unwrap();
    let db_path = dir.path().join("graph.db");

    // Create a store with data and back it up
    {
        let store = Store::open(&db_path).unwrap();
        store
            .upsert_project(&Project {
                name: "original".into(),
                indexed_at: "2025-01-01".into(),
                root_path: "/tmp".into(),
            })
            .unwrap();
        store
            .insert_node(&Node {
                id: 0,
                project: "original".into(),
                label: "Function".into(),
                name: "original_fn".into(),
                qualified_name: "original.original_fn".into(),
                file_path: "src/main.rs".into(),
                start_line: 1,
                end_line: 5,
                properties_json: None,
            })
            .unwrap();

        let backup_path = dir.path().join("backup.db");
        store.backup_to(&backup_path).unwrap();
    }

    // Modify the original database
    {
        let store = Store::open(&db_path).unwrap();
        store.delete_project("original").unwrap();
        store
            .upsert_project(&Project {
                name: "modified".into(),
                indexed_at: "2025-02-01".into(),
                root_path: "/tmp2".into(),
            })
            .unwrap();
    }

    // Restore from backup
    let backup_path = dir.path().join("backup.db");
    Store::restore_from(&backup_path, &db_path).unwrap();

    // Verify the restored database has the original data
    let store = Store::open(&db_path).unwrap();
    let projects = store.list_projects().unwrap();
    assert_eq!(projects.len(), 1);
    assert_eq!(projects[0].name, "original");

    let nodes = store.search_nodes("original", "original_fn", 10).unwrap();
    assert_eq!(nodes.len(), 1);
}

#[test]
fn test_restore_fails_with_nonexistent_source() {
    let dir = TempDir::new().unwrap();
    let db_path = dir.path().join("graph.db");
    let fake_backup = dir.path().join("nonexistent.db");

    let result = Store::restore_from(&fake_backup, &db_path);
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("does not exist"));
}

#[test]
fn test_restore_fails_when_database_is_locked() {
    let dir = TempDir::new().unwrap();
    let db_path = dir.path().join("graph.db");

    // Create a store and backup
    let store = Store::open(&db_path).unwrap();
    store
        .upsert_project(&Project {
            name: "p".into(),
            indexed_at: "now".into(),
            root_path: "/".into(),
        })
        .unwrap();

    let backup_path = dir.path().join("backup.db");
    store.backup_to(&backup_path).unwrap();

    // Hold an exclusive transaction on the database to simulate the MCP server running
    // We use a raw connection to hold the lock
    let lock_conn = rusqlite::Connection::open(&db_path).unwrap();
    lock_conn.execute_batch("BEGIN EXCLUSIVE").unwrap();

    // Attempt restore — should fail because the database is locked
    let result = Store::restore_from(&backup_path, &db_path);
    assert!(result.is_err());
    let err_msg = format!("{:#}", result.unwrap_err());
    assert!(
        err_msg.contains("exclusive lock") || err_msg.contains("database is locked"),
        "Expected lock error, got: {}",
        err_msg
    );

    // Clean up: release the lock
    lock_conn.execute_batch("ROLLBACK").unwrap();
}

#[test]
fn test_backup_to_custom_path() {
    let dir = TempDir::new().unwrap();
    let store = create_store_with_data(dir.path());

    let custom_path = dir.path().join("custom").join("my_backup.db");
    std::fs::create_dir_all(custom_path.parent().unwrap()).unwrap();

    store.backup_to(&custom_path).unwrap();
    assert!(custom_path.exists());

    // Verify it's a valid store
    let backup_store = Store::open(&custom_path).unwrap();
    let projects = backup_store.list_projects().unwrap();
    assert_eq!(projects.len(), 1);
}
