//! Semantic code search service using all-MiniLM-L6-v2 ONNX model.
//!
//! This service provides natural language search over code symbols by generating
//! 384-dimensional embeddings and ranking results by cosine similarity.
//!
//! ## Model Setup
//!
//! To use semantic search with the ONNX model, enable the `semantic-search` feature
//! and download the model files:
//!
//! 1. Download from HuggingFace:
//!    ```bash
//!    mkdir -p ~/.codryn/models
//!    wget https://huggingface.co/sentence-transformers/all-MiniLM-L6-v2/resolve/main/onnx/model.onnx \
//!         -O ~/.codryn/models/all-MiniLM-L6-v2.onnx
//!    wget https://huggingface.co/sentence-transformers/all-MiniLM-L6-v2/resolve/main/tokenizer.json \
//!         -O ~/.codryn/models/tokenizer.json
//!    ```
//!
//! 2. Alternatively, set the `CBM_MODEL_DIR` environment variable to the directory
//!    containing the model files.
//!
//! If the model is not available or the feature is not enabled, the service returns an error
//! indicating semantic search is unavailable, and callers should fall back to text-based
//! `search_graph`.

use anyhow::{bail, Result};
use codryn_store::{bytes_to_embedding, Store};
use serde::Serialize;

#[cfg(any(feature = "semantic-search", test))]
use std::path::PathBuf;

/// Embedding dimension for all-MiniLM-L6-v2.
pub const EMBEDDING_DIM: usize = 384;

/// Maximum number of results returned by semantic search.
const MAX_RESULTS: usize = 20;

/// Minimum cosine similarity threshold for results.
const MIN_SIMILARITY: f32 = 0.3;

/// Minimum query length in characters.
const MIN_QUERY_LEN: usize = 1;

/// Maximum query length in characters.
const MAX_QUERY_LEN: usize = 500;

/// A single semantic search result.
#[derive(Debug, Clone, Serialize)]
pub struct SemanticResult {
    pub name: String,
    pub qualified_name: String,
    pub file_path: String,
    pub similarity: f32,
    pub snippet: Option<String>,
    pub label: String,
    pub node_id: i64,
}

/// Error type for semantic search initialization failures.
#[derive(Debug, Clone, Serialize)]
pub struct SemanticSearchUnavailable {
    pub reason: String,
}

impl std::fmt::Display for SemanticSearchUnavailable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Semantic search unavailable: {}", self.reason)
    }
}

impl std::error::Error for SemanticSearchUnavailable {}

/// Compute cosine similarity between two embedding vectors.
pub fn cosine_similarity(a: &[f32; EMBEDDING_DIM], b: &[f32; EMBEDDING_DIM]) -> f32 {
    let mut dot = 0.0f32;
    let mut norm_a = 0.0f32;
    let mut norm_b = 0.0f32;

    for i in 0..EMBEDDING_DIM {
        dot += a[i] * b[i];
        norm_a += a[i] * a[i];
        norm_b += b[i] * b[i];
    }

    let denom = norm_a.sqrt() * norm_b.sqrt();
    if denom == 0.0 {
        0.0
    } else {
        dot / denom
    }
}

/// Validate query length constraints (1-500 characters).
pub fn validate_query(query: &str) -> Result<()> {
    let len = query.chars().count();
    if len < MIN_QUERY_LEN {
        bail!("Query must be at least {} character(s)", MIN_QUERY_LEN);
    }
    if len > MAX_QUERY_LEN {
        bail!(
            "Query must not exceed {} characters (got {})",
            MAX_QUERY_LEN,
            len
        );
    }
    Ok(())
}

/// Search stored embeddings for a project using a pre-computed query embedding.
///
/// Returns up to 20 results ranked by cosine similarity, excluding results
/// below the 0.3 similarity threshold.
pub fn search_with_embedding(
    store: &Store,
    project: &str,
    query_embedding: &[f32; EMBEDDING_DIM],
) -> Result<Vec<SemanticResult>> {
    // Load stored embeddings for the project using codryn-store API
    let records = store.get_embeddings_for_project(project)?;

    if records.is_empty() {
        bail!(
            "No embeddings available for project '{}'. \
             Run indexing with semantic embeddings enabled.",
            project
        );
    }

    // Compute cosine similarities and rank
    let mut scored: Vec<(i64, f32)> = records
        .iter()
        .filter_map(|record| {
            let embedding = bytes_to_embedding(&record.embedding)?;
            let sim = cosine_similarity(query_embedding, &embedding);
            if sim >= MIN_SIMILARITY {
                Some((record.node_id, sim))
            } else {
                None
            }
        })
        .collect();

    // Sort by similarity descending
    scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

    // Limit to MAX_RESULTS
    scored.truncate(MAX_RESULTS);

    // Fetch node details for results
    let mut results = Vec::with_capacity(scored.len());
    for (node_id, similarity) in scored {
        if let Ok(Some(node)) = store.get_node_by_id(node_id) {
            results.push(SemanticResult {
                name: node.name,
                qualified_name: node.qualified_name,
                file_path: node.file_path,
                similarity,
                snippet: None,
                label: node.label,
                node_id,
            });
        }
    }

    Ok(results)
}

// ── ONNX-based implementation (requires `semantic-search` feature) ────────

/// Maximum token sequence length for the model.
#[cfg(feature = "semantic-search")]
const MAX_SEQ_LEN: usize = 128;

/// The semantic search service. Holds the ONNX session and tokenizer.
#[cfg(feature = "semantic-search")]
pub struct SemanticSearchService {
    session: ort::session::Session,
    tokenizer: tokenizers::Tokenizer,
}

#[cfg(feature = "semantic-search")]
impl SemanticSearchService {
    /// Create a new SemanticSearchService by loading the ONNX model and tokenizer.
    ///
    /// Looks for model files in:
    /// 1. `CBM_MODEL_DIR` environment variable
    /// 2. `~/.codryn/models/`
    /// 3. `./models/` (relative to working directory)
    ///
    /// Returns an error if the model files are not found.
    pub fn new() -> Result<Self, SemanticSearchUnavailable> {
        let model_dir = find_model_dir().map_err(|e| SemanticSearchUnavailable {
            reason: e.to_string(),
        })?;

        let model_path = model_dir.join("all-MiniLM-L6-v2.onnx");
        let tokenizer_path = model_dir.join("tokenizer.json");

        Self::from_paths(&model_path, &tokenizer_path)
    }

    /// Create a new SemanticSearchService from explicit paths.
    pub fn from_paths(
        model_path: &std::path::Path,
        tokenizer_path: &std::path::Path,
    ) -> Result<Self, SemanticSearchUnavailable> {
        if !model_path.exists() {
            return Err(SemanticSearchUnavailable {
                reason: format!(
                    "ONNX model not found at {}. Download from: \
                     https://huggingface.co/sentence-transformers/all-MiniLM-L6-v2/resolve/main/onnx/model.onnx",
                    model_path.display()
                ),
            });
        }

        if !tokenizer_path.exists() {
            return Err(SemanticSearchUnavailable {
                reason: format!(
                    "Tokenizer not found at {}. Download from: \
                     https://huggingface.co/sentence-transformers/all-MiniLM-L6-v2/resolve/main/tokenizer.json",
                    tokenizer_path.display()
                ),
            });
        }

        let session = ort::session::Session::builder()
            .and_then(|b| Ok(b.with_intra_threads(1)?))
            .and_then(|mut b| b.commit_from_file(model_path))
            .map_err(|e| SemanticSearchUnavailable {
                reason: format!("Failed to load ONNX model: {}", e),
            })?;

        let tokenizer = tokenizers::Tokenizer::from_file(tokenizer_path).map_err(|e| {
            SemanticSearchUnavailable {
                reason: format!("Failed to load tokenizer: {}", e),
            }
        })?;

        Ok(Self { session, tokenizer })
    }

    /// Generate 384-dimensional embeddings for a batch of texts.
    ///
    /// Uses mean pooling over token embeddings with attention mask weighting,
    /// followed by L2 normalization.
    pub fn generate_embeddings(&mut self, texts: &[&str]) -> Result<Vec<[f32; EMBEDDING_DIM]>> {
        use anyhow::Context;
        use ort::value::TensorRef;

        if texts.is_empty() {
            return Ok(vec![]);
        }

        let mut all_embeddings = Vec::with_capacity(texts.len());

        for text in texts {
            let encoding = self
                .tokenizer
                .encode(*text, true)
                .map_err(|e| anyhow::anyhow!("Tokenization failed: {}", e))?;

            let ids = encoding.get_ids();
            let attention = encoding.get_attention_mask();

            // Truncate to max sequence length
            let seq_len = ids.len().min(MAX_SEQ_LEN);
            let ids_trunc: Vec<i64> = ids[..seq_len].iter().map(|&x| x as i64).collect();
            let attention_trunc: Vec<i64> =
                attention[..seq_len].iter().map(|&x| x as i64).collect();
            let token_type_ids: Vec<i64> = vec![0i64; seq_len];

            // Create input tensors with shape [1, seq_len]
            let input_ids_tensor =
                TensorRef::from_array_view(([1usize, seq_len], ids_trunc.as_slice()))
                    .context("Failed to create input_ids tensor")?;
            let attention_mask_tensor =
                TensorRef::from_array_view(([1usize, seq_len], attention_trunc.as_slice()))
                    .context("Failed to create attention_mask tensor")?;
            let token_type_ids_tensor =
                TensorRef::from_array_view(([1usize, seq_len], token_type_ids.as_slice()))
                    .context("Failed to create token_type_ids tensor")?;

            let outputs = self
                .session
                .run(ort::inputs![
                    "input_ids" => input_ids_tensor,
                    "attention_mask" => attention_mask_tensor,
                    "token_type_ids" => token_type_ids_tensor,
                ])
                .context("ONNX inference failed")?;

            // Extract the last_hidden_state output (shape: [1, seq_len, 384])
            let (shape, data) = outputs[0]
                .try_extract_tensor::<f32>()
                .context("Failed to extract output tensor")?;

            let hidden_size = if shape.len() == 3 {
                shape[2] as usize
            } else {
                EMBEDDING_DIM
            };

            // Mean pooling with attention mask
            let embedding = mean_pool(data, seq_len, hidden_size, &attention_trunc);

            // L2 normalize
            let normalized = l2_normalize_vec(&embedding);

            let mut result = [0f32; EMBEDDING_DIM];
            let copy_len = normalized.len().min(EMBEDDING_DIM);
            result[..copy_len].copy_from_slice(&normalized[..copy_len]);
            all_embeddings.push(result);
        }

        Ok(all_embeddings)
    }

    /// Search for symbols semantically similar to the query.
    ///
    /// Returns up to 20 results ranked by cosine similarity, excluding results
    /// below the 0.3 similarity threshold.
    pub fn search(
        &mut self,
        store: &Store,
        project: &str,
        query: &str,
    ) -> Result<Vec<SemanticResult>> {
        validate_query(query)?;

        let query_embeddings = self.generate_embeddings(&[query])?;
        let query_embedding = &query_embeddings[0];

        search_with_embedding(store, project, query_embedding)
    }
}

/// Stub implementation when the `semantic-search` feature is not enabled.
#[cfg(not(feature = "semantic-search"))]
#[derive(Debug)]
pub struct SemanticSearchService;

#[cfg(not(feature = "semantic-search"))]
impl SemanticSearchService {
    /// Attempt to create the service. Always returns an error when the feature is disabled.
    pub fn new() -> Result<Self, SemanticSearchUnavailable> {
        Err(SemanticSearchUnavailable {
            reason: "Semantic search requires the 'semantic-search' feature to be enabled. \
                     Rebuild with: cargo build --features semantic-search"
                .to_string(),
        })
    }

    /// Generate embeddings. Always returns an error when the feature is disabled.
    pub fn generate_embeddings(&mut self, _texts: &[&str]) -> Result<Vec<[f32; EMBEDDING_DIM]>> {
        bail!(
            "Semantic search requires the 'semantic-search' feature. \
             Rebuild with: cargo build --features semantic-search"
        )
    }

    /// Search. Always returns an error when the feature is disabled.
    pub fn search(
        &mut self,
        _store: &Store,
        _project: &str,
        _query: &str,
    ) -> Result<Vec<SemanticResult>> {
        bail!(
            "Semantic search requires the 'semantic-search' feature. \
             Rebuild with: cargo build --features semantic-search"
        )
    }
}

// ── Internal helpers ──────────────────────────────────────────────────────

/// Find the model directory by checking known locations.
#[cfg(any(feature = "semantic-search", test))]
#[allow(dead_code)]
fn find_model_dir() -> Result<PathBuf> {
    // 1. Check CBM_MODEL_DIR environment variable
    if let Ok(dir) = std::env::var("CBM_MODEL_DIR") {
        let path = PathBuf::from(&dir);
        if path.is_dir() {
            return Ok(path);
        }
    }

    // 2. Check ~/.codryn/models/
    if let Some(home) = dirs::home_dir() {
        let path = home.join(".codryn").join("models");
        if path.is_dir() {
            return Ok(path);
        }
    }

    // 3. Check ./models/ relative to working directory
    let local = PathBuf::from("models");
    if local.is_dir() {
        return Ok(local);
    }

    bail!(
        "Model directory not found. Set CBM_MODEL_DIR environment variable, \
         or place model files in ~/.codryn/models/ or ./models/. \
         Required files: all-MiniLM-L6-v2.onnx, tokenizer.json"
    )
}

/// Mean pooling: average token embeddings weighted by attention mask.
/// `data` is a flat array of shape [1, seq_len, hidden_size].
#[cfg(any(feature = "semantic-search", test))]
fn mean_pool(data: &[f32], seq_len: usize, hidden_size: usize, attention_mask: &[i64]) -> Vec<f32> {
    let mut sum = vec![0.0f32; hidden_size];
    let mut count = 0.0f32;

    for (t, &mask) in attention_mask.iter().enumerate().take(seq_len) {
        let mask_val = mask as f32;
        if mask_val > 0.0 {
            let offset = t * hidden_size;
            for h in 0..hidden_size {
                if offset + h < data.len() {
                    sum[h] += data[offset + h] * mask_val;
                }
            }
            count += mask_val;
        }
    }

    if count > 0.0 {
        for val in sum.iter_mut() {
            *val /= count;
        }
    }

    sum
}

/// L2-normalize a vector.
#[cfg(any(feature = "semantic-search", test))]
fn l2_normalize_vec(v: &[f32]) -> Vec<f32> {
    let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 0.0 {
        v.iter().map(|x| x / norm).collect()
    } else {
        v.to_vec()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cosine_similarity_identical() {
        let a = [1.0f32; EMBEDDING_DIM];
        let b = [1.0f32; EMBEDDING_DIM];
        let sim = cosine_similarity(&a, &b);
        assert!(
            (sim - 1.0).abs() < 1e-5,
            "Identical vectors should have similarity ~1.0, got {}",
            sim
        );
    }

    #[test]
    fn test_cosine_similarity_orthogonal() {
        let mut a = [0.0f32; EMBEDDING_DIM];
        let mut b = [0.0f32; EMBEDDING_DIM];
        a[0] = 1.0;
        b[1] = 1.0;
        let sim = cosine_similarity(&a, &b);
        assert!(
            sim.abs() < 1e-5,
            "Orthogonal vectors should have similarity ~0.0, got {}",
            sim
        );
    }

    #[test]
    fn test_cosine_similarity_opposite() {
        let a = [1.0f32; EMBEDDING_DIM];
        let b = [-1.0f32; EMBEDDING_DIM];
        let sim = cosine_similarity(&a, &b);
        assert!(
            (sim + 1.0).abs() < 1e-5,
            "Opposite vectors should have similarity ~-1.0, got {}",
            sim
        );
    }

    #[test]
    fn test_cosine_similarity_zero_vector() {
        let a = [0.0f32; EMBEDDING_DIM];
        let b = [1.0f32; EMBEDDING_DIM];
        let sim = cosine_similarity(&a, &b);
        assert_eq!(sim, 0.0, "Zero vector should have similarity 0.0");
    }

    #[test]
    fn test_cosine_similarity_normalized_vectors() {
        let mut a = [0.0f32; EMBEDDING_DIM];
        let mut b = [0.0f32; EMBEDDING_DIM];
        a[0] = 1.0;
        b[0] = 0.5;
        b[1] = (3.0f32).sqrt() / 2.0;
        let sim = cosine_similarity(&a, &b);
        assert!(
            (sim - 0.5).abs() < 1e-4,
            "60-degree vectors should have similarity ~0.5, got {}",
            sim
        );
    }

    #[test]
    fn test_validate_query_empty() {
        assert!(validate_query("").is_err());
    }

    #[test]
    fn test_validate_query_too_long() {
        let long_query: String = "a".repeat(501);
        assert!(validate_query(&long_query).is_err());
    }

    #[test]
    fn test_validate_query_valid() {
        assert!(validate_query("find authentication handler").is_ok());
    }

    #[test]
    fn test_validate_query_boundary_min() {
        assert!(validate_query("a").is_ok());
    }

    #[test]
    fn test_validate_query_boundary_max() {
        let query: String = "a".repeat(500);
        assert!(validate_query(&query).is_ok());
    }

    #[test]
    fn test_validate_query_unicode() {
        let query = "日本語のクエリ";
        assert!(validate_query(query).is_ok());
    }

    #[test]
    fn test_semantic_search_unavailable_display() {
        let err = SemanticSearchUnavailable {
            reason: "model not found".to_string(),
        };
        assert_eq!(
            err.to_string(),
            "Semantic search unavailable: model not found"
        );
    }

    #[test]
    fn test_service_new_returns_unavailable() {
        let result = SemanticSearchService::new();
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(!err.reason.is_empty());
    }

    #[test]
    fn test_search_with_store_no_embeddings() {
        let store = Store::open_in_memory().unwrap();
        let records = store.get_embeddings_for_project("test_project").unwrap();
        assert_eq!(records.len(), 0);
    }

    #[test]
    fn test_search_with_embedding_no_data() {
        let store = Store::open_in_memory().unwrap();
        let query_emb = [0.5f32; EMBEDDING_DIM];
        let result = search_with_embedding(&store, "test_project", &query_emb);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("No embeddings available"));
    }

    #[test]
    fn test_search_with_embedding_filters_below_threshold() {
        use codryn_store::{embedding_to_bytes, EmbeddingRecord, Node, Project};

        let store = Store::open_in_memory().unwrap();
        store
            .upsert_project(&Project {
                name: "p".into(),
                indexed_at: "now".into(),
                root_path: "/".into(),
            })
            .unwrap();

        // Insert a node
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

        // Store an embedding that is orthogonal to our query (similarity ~0)
        let mut node_emb = [0.0f32; EMBEDDING_DIM];
        node_emb[100] = 1.0; // Point in a different direction

        store
            .upsert_embedding(&EmbeddingRecord {
                node_id,
                project: "p".into(),
                embedding: embedding_to_bytes(&node_emb),
                text_hash: "hash1".into(),
                created_at: "now".into(),
            })
            .unwrap();

        // Query with a vector pointing in a completely different direction
        let mut query_emb = [0.0f32; EMBEDDING_DIM];
        query_emb[0] = 1.0;

        let results = search_with_embedding(&store, "p", &query_emb).unwrap();
        // Similarity should be ~0, below threshold of 0.3
        assert_eq!(results.len(), 0);
    }

    #[test]
    fn test_search_with_embedding_returns_similar() {
        use codryn_store::{embedding_to_bytes, EmbeddingRecord, Node, Project};

        let store = Store::open_in_memory().unwrap();
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
                name: "authenticate_user".into(),
                qualified_name: "p.authenticate_user".into(),
                file_path: "src/auth.rs".into(),
                start_line: 10,
                end_line: 30,
                properties_json: None,
            })
            .unwrap();

        // Store an embedding that is very similar to our query
        let node_emb = [1.0f32; EMBEDDING_DIM];
        store
            .upsert_embedding(&EmbeddingRecord {
                node_id,
                project: "p".into(),
                embedding: embedding_to_bytes(&node_emb),
                text_hash: "hash1".into(),
                created_at: "now".into(),
            })
            .unwrap();

        // Query with a very similar vector
        let query_emb = [1.0f32; EMBEDDING_DIM];

        let results = search_with_embedding(&store, "p", &query_emb).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "authenticate_user");
        assert!((results[0].similarity - 1.0).abs() < 1e-5);
    }

    #[test]
    fn test_search_with_embedding_limits_results() {
        use codryn_store::{embedding_to_bytes, EmbeddingRecord, Node, Project};

        let store = Store::open_in_memory().unwrap();
        store
            .upsert_project(&Project {
                name: "p".into(),
                indexed_at: "now".into(),
                root_path: "/".into(),
            })
            .unwrap();

        // Insert 25 nodes with similar embeddings
        for i in 0..25 {
            let node_id = store
                .insert_node(&Node {
                    id: 0,
                    project: "p".into(),
                    label: "Function".into(),
                    name: format!("func_{}", i),
                    qualified_name: format!("p.func_{}", i),
                    file_path: "src/lib.rs".into(),
                    start_line: i,
                    end_line: i + 5,
                    properties_json: None,
                })
                .unwrap();

            let emb = [1.0f32; EMBEDDING_DIM];
            store
                .upsert_embedding(&EmbeddingRecord {
                    node_id,
                    project: "p".into(),
                    embedding: embedding_to_bytes(&emb),
                    text_hash: format!("hash_{}", i),
                    created_at: "now".into(),
                })
                .unwrap();
        }

        let query_emb = [1.0f32; EMBEDDING_DIM];
        let results = search_with_embedding(&store, "p", &query_emb).unwrap();
        // Should be limited to MAX_RESULTS (20)
        assert_eq!(results.len(), 20);
    }

    #[test]
    fn test_l2_normalize_vec_basic() {
        let v = vec![3.0f32, 4.0];
        let normalized = l2_normalize_vec(&v);
        let norm: f32 = normalized.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 1e-5);
        assert!((normalized[0] - 0.6).abs() < 1e-5);
        assert!((normalized[1] - 0.8).abs() < 1e-5);
    }

    #[test]
    fn test_l2_normalize_vec_zero() {
        let v = vec![0.0f32; 5];
        let normalized = l2_normalize_vec(&v);
        assert_eq!(normalized.iter().sum::<f32>(), 0.0);
    }

    #[test]
    fn test_mean_pool_basic() {
        let data = vec![1.0f32, 2.0, 3.0, 4.0, 5.0, 6.0];
        let attention_mask = vec![1i64, 1];
        let result = mean_pool(&data, 2, 3, &attention_mask);
        assert!((result[0] - 2.5).abs() < 1e-5);
        assert!((result[1] - 3.5).abs() < 1e-5);
        assert!((result[2] - 4.5).abs() < 1e-5);
    }

    #[test]
    fn test_mean_pool_with_mask() {
        let data = vec![1.0f32, 2.0, 3.0, 4.0, 5.0, 6.0];
        let attention_mask = vec![1i64, 0];
        let result = mean_pool(&data, 2, 3, &attention_mask);
        assert!((result[0] - 1.0).abs() < 1e-5);
        assert!((result[1] - 2.0).abs() < 1e-5);
        assert!((result[2] - 3.0).abs() < 1e-5);
    }

    #[test]
    fn test_mean_pool_all_masked() {
        let data = vec![1.0f32, 2.0, 3.0, 4.0, 5.0, 6.0];
        let attention_mask = vec![0i64, 0];
        let result = mean_pool(&data, 2, 3, &attention_mask);
        assert_eq!(result, vec![0.0, 0.0, 0.0]);
    }
}
