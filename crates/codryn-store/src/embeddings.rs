use anyhow::{Context, Result};
use rusqlite::params;

use crate::Store;

/// A stored embedding record for semantic search.
#[derive(Debug, Clone)]
pub struct EmbeddingRecord {
    pub node_id: i64,
    pub project: String,
    /// 384-dimensional f32 embedding stored as raw bytes (384 * 4 = 1536 bytes).
    pub embedding: Vec<u8>,
    /// SHA-256 hash of the text that was embedded, used to detect when re-embedding is needed.
    pub text_hash: String,
    pub created_at: String,
}

impl Store {
    /// Insert or replace an embedding for a node.
    /// The embedding blob should be 384 f32 values serialized as little-endian bytes (1536 bytes).
    pub fn upsert_embedding(&self, record: &EmbeddingRecord) -> Result<()> {
        self.conn
            .execute(
                "INSERT OR REPLACE INTO embeddings (node_id, project, embedding, text_hash, created_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    record.node_id,
                    record.project,
                    record.embedding,
                    record.text_hash,
                    record.created_at,
                ],
            )
            .context("failed to upsert embedding")?;
        Ok(())
    }

    /// Insert multiple embeddings in a single transaction for efficiency during indexing.
    pub fn upsert_embeddings_batch(&self, records: &[EmbeddingRecord]) -> Result<usize> {
        if records.is_empty() {
            return Ok(0);
        }
        let tx = self
            .conn
            .unchecked_transaction()
            .context("failed to begin embeddings batch transaction")?;
        {
            let mut stmt = tx.prepare_cached(
                "INSERT OR REPLACE INTO embeddings (node_id, project, embedding, text_hash, created_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5)",
            )?;
            for record in records {
                stmt.execute(params![
                    record.node_id,
                    record.project,
                    record.embedding,
                    record.text_hash,
                    record.created_at,
                ])?;
            }
        }
        tx.commit().context("failed to commit embeddings batch")?;
        Ok(records.len())
    }

    /// Get the embedding for a specific node.
    pub fn get_embedding(&self, node_id: i64) -> Result<Option<EmbeddingRecord>> {
        let mut stmt = self.conn.prepare(
            "SELECT node_id, project, embedding, text_hash, created_at \
             FROM embeddings WHERE node_id = ?1",
        )?;
        let result = stmt
            .query_row(params![node_id], |row| {
                Ok(EmbeddingRecord {
                    node_id: row.get(0)?,
                    project: row.get(1)?,
                    embedding: row.get(2)?,
                    text_hash: row.get(3)?,
                    created_at: row.get(4)?,
                })
            })
            .optional()
            .context("failed to query embedding")?;
        Ok(result)
    }

    /// Get all embeddings for a project. Used for brute-force cosine similarity search.
    pub fn get_embeddings_for_project(&self, project: &str) -> Result<Vec<EmbeddingRecord>> {
        let mut stmt = self.conn.prepare(
            "SELECT node_id, project, embedding, text_hash, created_at \
             FROM embeddings WHERE project = ?1",
        )?;
        let rows = stmt
            .query_map(params![project], |row| {
                Ok(EmbeddingRecord {
                    node_id: row.get(0)?,
                    project: row.get(1)?,
                    embedding: row.get(2)?,
                    text_hash: row.get(3)?,
                    created_at: row.get(4)?,
                })
            })
            .context("failed to query embeddings for project")?;
        let mut results = Vec::new();
        for row in rows {
            results.push(row?);
        }
        Ok(results)
    }

    /// Check if an embedding exists for a node with the given text hash.
    /// Returns true if the embedding is up-to-date (same text_hash), false otherwise.
    pub fn is_embedding_current(&self, node_id: i64, text_hash: &str) -> Result<bool> {
        let mut stmt = self
            .conn
            .prepare("SELECT text_hash FROM embeddings WHERE node_id = ?1")?;
        let result: Option<String> = stmt
            .query_row(params![node_id], |row| row.get(0))
            .optional()
            .context("failed to check embedding currency")?;
        Ok(result.as_deref() == Some(text_hash))
    }

    /// Delete all embeddings for a project (e.g., when the project is deleted).
    pub fn delete_embeddings_for_project(&self, project: &str) -> Result<usize> {
        let count = self
            .conn
            .execute(
                "DELETE FROM embeddings WHERE project = ?1",
                params![project],
            )
            .context("failed to delete embeddings for project")?;
        Ok(count)
    }

    /// Delete embeddings for specific node IDs (e.g., when nodes are removed during reindex).
    pub fn delete_embeddings_for_nodes(&self, node_ids: &[i64]) -> Result<usize> {
        if node_ids.is_empty() {
            return Ok(0);
        }
        let placeholders: String = node_ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        let sql = format!("DELETE FROM embeddings WHERE node_id IN ({})", placeholders);
        let mut stmt = self.conn.prepare(&sql)?;
        let params: Vec<&dyn rusqlite::ToSql> = node_ids
            .iter()
            .map(|id| id as &dyn rusqlite::ToSql)
            .collect();
        let count = stmt
            .execute(params.as_slice())
            .context("failed to delete embeddings for nodes")?;
        Ok(count)
    }

    /// Count embeddings for a project.
    pub fn count_embeddings(&self, project: &str) -> Result<usize> {
        let count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM embeddings WHERE project = ?1",
            params![project],
            |row| row.get(0),
        )?;
        Ok(count as usize)
    }
}

use rusqlite::OptionalExtension;

/// Helper to serialize a 384-dim f32 vector to bytes (little-endian).
pub fn embedding_to_bytes(embedding: &[f32; 384]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(384 * 4);
    for &val in embedding.iter() {
        bytes.extend_from_slice(&val.to_le_bytes());
    }
    bytes
}

/// Helper to deserialize bytes back to a 384-dim f32 vector.
pub fn bytes_to_embedding(bytes: &[u8]) -> Option<[f32; 384]> {
    if bytes.len() != 384 * 4 {
        return None;
    }
    let mut embedding = [0.0f32; 384];
    for (i, chunk) in bytes.chunks_exact(4).enumerate() {
        embedding[i] = f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
    }
    Some(embedding)
}

/// Build the text to embed for a node based on its signature and docstring.
/// This combines the node's name, qualified_name, and any docstring from properties.
pub fn build_embedding_text(
    name: &str,
    qualified_name: &str,
    label: &str,
    properties_json: Option<&str>,
) -> String {
    let mut text = String::new();

    // Include the label for context
    text.push_str(label);
    text.push(' ');

    // Include the name
    text.push_str(name);

    // Include qualified name if different from name
    if qualified_name != name {
        text.push(' ');
        text.push_str(qualified_name);
    }

    // Extract signature and docstring from properties if available
    if let Some(props) = properties_json {
        if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(props) {
            if let Some(sig) = parsed.get("signature").and_then(|v| v.as_str()) {
                text.push(' ');
                text.push_str(sig);
            }
            if let Some(doc) = parsed.get("docstring").and_then(|v| v.as_str()) {
                text.push(' ');
                text.push_str(doc);
            }
            if let Some(doc) = parsed.get("doc").and_then(|v| v.as_str()) {
                text.push(' ');
                text.push_str(doc);
            }
        }
    }

    text
}

/// Determine if a node should have an embedding generated.
/// Only function and class nodes get embeddings (per Requirement 23.1).
pub fn should_embed_node(label: &str) -> bool {
    matches!(
        label,
        "Function" | "Class" | "Method" | "Interface" | "Trait" | "Struct"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Node, Project};

    fn test_store() -> Store {
        Store::open_in_memory().unwrap()
    }

    #[test]
    fn test_embeddings_table_created() {
        let store = test_store();
        // Verify the table exists by counting rows (should be 0)
        let count = store.count_embeddings("nonexistent").unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn test_upsert_and_get_embedding() {
        let store = test_store();
        store
            .upsert_project(&Project {
                name: "p".into(),
                indexed_at: "now".into(),
                root_path: "/".into(),
            })
            .unwrap();
        let node_id = store
            .insert_node(&Node {
                id: 0,
                project: "p".into(),
                label: "Function".into(),
                name: "hello".into(),
                qualified_name: "p.hello".into(),
                file_path: "src/lib.rs".into(),
                start_line: 1,
                end_line: 5,
                properties_json: None,
            })
            .unwrap();

        let embedding = [0.5f32; 384];
        let bytes = embedding_to_bytes(&embedding);
        let record = EmbeddingRecord {
            node_id,
            project: "p".into(),
            embedding: bytes,
            text_hash: "abc123".into(),
            created_at: "2025-01-01T00:00:00Z".into(),
        };

        store.upsert_embedding(&record).unwrap();

        let retrieved = store.get_embedding(node_id).unwrap().unwrap();
        assert_eq!(retrieved.node_id, node_id);
        assert_eq!(retrieved.project, "p");
        assert_eq!(retrieved.text_hash, "abc123");
        assert_eq!(retrieved.embedding.len(), 384 * 4);

        let decoded = bytes_to_embedding(&retrieved.embedding).unwrap();
        assert_eq!(decoded[0], 0.5);
        assert_eq!(decoded[383], 0.5);
    }

    #[test]
    fn test_upsert_replaces_existing() {
        let store = test_store();
        store
            .upsert_project(&Project {
                name: "p".into(),
                indexed_at: "now".into(),
                root_path: "/".into(),
            })
            .unwrap();
        let node_id = store
            .insert_node(&Node {
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

        let record1 = EmbeddingRecord {
            node_id,
            project: "p".into(),
            embedding: embedding_to_bytes(&[1.0f32; 384]),
            text_hash: "hash1".into(),
            created_at: "2025-01-01T00:00:00Z".into(),
        };
        store.upsert_embedding(&record1).unwrap();

        let record2 = EmbeddingRecord {
            node_id,
            project: "p".into(),
            embedding: embedding_to_bytes(&[2.0f32; 384]),
            text_hash: "hash2".into(),
            created_at: "2025-01-02T00:00:00Z".into(),
        };
        store.upsert_embedding(&record2).unwrap();

        let retrieved = store.get_embedding(node_id).unwrap().unwrap();
        assert_eq!(retrieved.text_hash, "hash2");
        assert_eq!(store.count_embeddings("p").unwrap(), 1);
    }

    #[test]
    fn test_get_embeddings_for_project() {
        let store = test_store();
        store
            .upsert_project(&Project {
                name: "p".into(),
                indexed_at: "now".into(),
                root_path: "/".into(),
            })
            .unwrap();
        let id1 = store
            .insert_node(&Node {
                id: 0,
                project: "p".into(),
                label: "Function".into(),
                name: "f1".into(),
                qualified_name: "p.f1".into(),
                file_path: "".into(),
                start_line: 0,
                end_line: 0,
                properties_json: None,
            })
            .unwrap();
        let id2 = store
            .insert_node(&Node {
                id: 0,
                project: "p".into(),
                label: "Function".into(),
                name: "f2".into(),
                qualified_name: "p.f2".into(),
                file_path: "".into(),
                start_line: 0,
                end_line: 0,
                properties_json: None,
            })
            .unwrap();

        store
            .upsert_embedding(&EmbeddingRecord {
                node_id: id1,
                project: "p".into(),
                embedding: embedding_to_bytes(&[1.0; 384]),
                text_hash: "h1".into(),
                created_at: "now".into(),
            })
            .unwrap();
        store
            .upsert_embedding(&EmbeddingRecord {
                node_id: id2,
                project: "p".into(),
                embedding: embedding_to_bytes(&[2.0; 384]),
                text_hash: "h2".into(),
                created_at: "now".into(),
            })
            .unwrap();

        let all = store.get_embeddings_for_project("p").unwrap();
        assert_eq!(all.len(), 2);
    }

    #[test]
    fn test_is_embedding_current() {
        let store = test_store();
        store
            .upsert_project(&Project {
                name: "p".into(),
                indexed_at: "now".into(),
                root_path: "/".into(),
            })
            .unwrap();
        let node_id = store
            .insert_node(&Node {
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

        // No embedding yet
        assert!(!store.is_embedding_current(node_id, "hash1").unwrap());

        store
            .upsert_embedding(&EmbeddingRecord {
                node_id,
                project: "p".into(),
                embedding: embedding_to_bytes(&[1.0; 384]),
                text_hash: "hash1".into(),
                created_at: "now".into(),
            })
            .unwrap();

        // Same hash = current
        assert!(store.is_embedding_current(node_id, "hash1").unwrap());
        // Different hash = not current
        assert!(!store.is_embedding_current(node_id, "hash2").unwrap());
    }

    #[test]
    fn test_delete_embeddings_for_project() {
        let store = test_store();
        store
            .upsert_project(&Project {
                name: "p".into(),
                indexed_at: "now".into(),
                root_path: "/".into(),
            })
            .unwrap();
        let node_id = store
            .insert_node(&Node {
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

        store
            .upsert_embedding(&EmbeddingRecord {
                node_id,
                project: "p".into(),
                embedding: embedding_to_bytes(&[1.0; 384]),
                text_hash: "h".into(),
                created_at: "now".into(),
            })
            .unwrap();

        assert_eq!(store.count_embeddings("p").unwrap(), 1);
        store.delete_embeddings_for_project("p").unwrap();
        assert_eq!(store.count_embeddings("p").unwrap(), 0);
    }

    #[test]
    fn test_delete_embeddings_for_nodes() {
        let store = test_store();
        store
            .upsert_project(&Project {
                name: "p".into(),
                indexed_at: "now".into(),
                root_path: "/".into(),
            })
            .unwrap();
        let id1 = store
            .insert_node(&Node {
                id: 0,
                project: "p".into(),
                label: "Function".into(),
                name: "f1".into(),
                qualified_name: "p.f1".into(),
                file_path: "".into(),
                start_line: 0,
                end_line: 0,
                properties_json: None,
            })
            .unwrap();
        let id2 = store
            .insert_node(&Node {
                id: 0,
                project: "p".into(),
                label: "Function".into(),
                name: "f2".into(),
                qualified_name: "p.f2".into(),
                file_path: "".into(),
                start_line: 0,
                end_line: 0,
                properties_json: None,
            })
            .unwrap();

        store
            .upsert_embedding(&EmbeddingRecord {
                node_id: id1,
                project: "p".into(),
                embedding: embedding_to_bytes(&[1.0; 384]),
                text_hash: "h1".into(),
                created_at: "now".into(),
            })
            .unwrap();
        store
            .upsert_embedding(&EmbeddingRecord {
                node_id: id2,
                project: "p".into(),
                embedding: embedding_to_bytes(&[2.0; 384]),
                text_hash: "h2".into(),
                created_at: "now".into(),
            })
            .unwrap();

        store.delete_embeddings_for_nodes(&[id1]).unwrap();
        assert_eq!(store.count_embeddings("p").unwrap(), 1);
        assert!(store.get_embedding(id1).unwrap().is_none());
        assert!(store.get_embedding(id2).unwrap().is_some());
    }

    #[test]
    fn test_batch_upsert_embeddings() {
        let store = test_store();
        store
            .upsert_project(&Project {
                name: "p".into(),
                indexed_at: "now".into(),
                root_path: "/".into(),
            })
            .unwrap();
        let id1 = store
            .insert_node(&Node {
                id: 0,
                project: "p".into(),
                label: "Function".into(),
                name: "f1".into(),
                qualified_name: "p.f1".into(),
                file_path: "".into(),
                start_line: 0,
                end_line: 0,
                properties_json: None,
            })
            .unwrap();
        let id2 = store
            .insert_node(&Node {
                id: 0,
                project: "p".into(),
                label: "Class".into(),
                name: "C1".into(),
                qualified_name: "p.C1".into(),
                file_path: "".into(),
                start_line: 0,
                end_line: 0,
                properties_json: None,
            })
            .unwrap();

        let records = vec![
            EmbeddingRecord {
                node_id: id1,
                project: "p".into(),
                embedding: embedding_to_bytes(&[1.0; 384]),
                text_hash: "h1".into(),
                created_at: "now".into(),
            },
            EmbeddingRecord {
                node_id: id2,
                project: "p".into(),
                embedding: embedding_to_bytes(&[2.0; 384]),
                text_hash: "h2".into(),
                created_at: "now".into(),
            },
        ];

        let count = store.upsert_embeddings_batch(&records).unwrap();
        assert_eq!(count, 2);
        assert_eq!(store.count_embeddings("p").unwrap(), 2);
    }

    #[test]
    fn test_embedding_to_bytes_roundtrip() {
        let mut original = [0.0f32; 384];
        for (i, val) in original.iter_mut().enumerate() {
            *val = i as f32 * 0.01;
        }
        let bytes = embedding_to_bytes(&original);
        assert_eq!(bytes.len(), 384 * 4);
        let decoded = bytes_to_embedding(&bytes).unwrap();
        assert_eq!(original, decoded);
    }

    #[test]
    fn test_bytes_to_embedding_invalid_length() {
        assert!(bytes_to_embedding(&[0u8; 100]).is_none());
        assert!(bytes_to_embedding(&[]).is_none());
    }

    #[test]
    fn test_build_embedding_text_basic() {
        let text = build_embedding_text("hello", "src.hello", "Function", None);
        assert!(text.contains("Function"));
        assert!(text.contains("hello"));
        assert!(text.contains("src.hello"));
    }

    #[test]
    fn test_build_embedding_text_with_signature() {
        let props = r#"{"signature": "fn hello(name: &str) -> String"}"#;
        let text = build_embedding_text("hello", "src.hello", "Function", Some(props));
        assert!(text.contains("fn hello(name: &str) -> String"));
    }

    #[test]
    fn test_build_embedding_text_with_docstring() {
        let props = r#"{"docstring": "Greets the user by name"}"#;
        let text = build_embedding_text("hello", "src.hello", "Function", Some(props));
        assert!(text.contains("Greets the user by name"));
    }

    #[test]
    fn test_should_embed_node() {
        assert!(should_embed_node("Function"));
        assert!(should_embed_node("Class"));
        assert!(should_embed_node("Method"));
        assert!(should_embed_node("Interface"));
        assert!(should_embed_node("Trait"));
        assert!(should_embed_node("Struct"));
        assert!(!should_embed_node("Module"));
        assert!(!should_embed_node("File"));
        assert!(!should_embed_node("Variable"));
    }
}
