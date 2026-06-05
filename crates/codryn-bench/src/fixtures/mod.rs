//! Benchmark fixture generators for deterministic, reproducible synthetic projects.
//!
//! Fixtures are cached on disk at `target/bench-fixtures/<name>/` and only regenerated
//! when the schema version changes (detected via SHA-256 hash comparison).

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::PathBuf;

/// Configuration for fixture generation.
#[derive(Debug, Clone)]
pub struct FixtureConfig {
    pub name: &'static str,
    pub seed: u64,
    pub schema_version: &'static str,
}

/// A generated synthetic project ready for benchmarking.
#[derive(Debug)]
pub struct GeneratedFixture {
    pub root: PathBuf,
    pub file_count: usize,
    pub node_count: usize,
    pub edge_count: usize,
    pub metadata: FixtureMetadata,
}

/// Metadata stored alongside cached fixtures for cache validation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FixtureMetadata {
    pub schema_version_hash: String,
    pub generated_at: String,
    pub config: FixtureConfigSerialized,
}

/// Serializable form of FixtureConfig for metadata persistence.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FixtureConfigSerialized {
    pub name: String,
    pub seed: u64,
    pub file_count: usize,
    pub node_count: usize,
    pub edge_count: usize,
}

/// Compute the SHA-256 hash of a schema version string.
fn compute_schema_hash(schema_version: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(schema_version.as_bytes());
    hex::encode(hasher.finalize())
}

/// Get the cache directory path for a given fixture name.
fn cache_dir(name: &str) -> PathBuf {
    // Use the workspace target directory
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.pop(); // crates/
    path.pop(); // workspace root
    path.push("target");
    path.push("bench-fixtures");
    path.push(name);
    path
}

/// Get the metadata file path for a given fixture name.
fn metadata_path(name: &str) -> PathBuf {
    cache_dir(name).join("metadata.json")
}

/// Load cached metadata from disk, if it exists.
fn load_cached_metadata(name: &str) -> Option<FixtureMetadata> {
    let path = metadata_path(name);
    let content = fs::read_to_string(&path).ok()?;
    serde_json::from_str(&content).ok()
}

/// Save metadata to the cache directory.
fn save_metadata(name: &str, metadata: &FixtureMetadata) -> Result<()> {
    let path = metadata_path(name);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create cache directory: {}", parent.display()))?;
    }
    let content =
        serde_json::to_string_pretty(metadata).context("Failed to serialize fixture metadata")?;
    fs::write(&path, content)
        .with_context(|| format!("Failed to write metadata file: {}", path.display()))?;
    Ok(())
}

/// Load a fixture from cache or generate it if the cache is invalid.
///
/// Cache validation logic:
/// 1. Check if `target/bench-fixtures/<name>/metadata.json` exists
/// 2. If exists: load metadata, compare `schema_version_hash` with current
/// 3. If hash matches: return the cached fixture (read counts from metadata)
/// 4. If no match or doesn't exist: call `generator(config.seed)`, save metadata, return fixture
pub fn load_or_generate(
    config: &FixtureConfig,
    generator: impl FnOnce(u64) -> GeneratedFixture,
) -> GeneratedFixture {
    let current_hash = compute_schema_hash(config.schema_version);

    // Check for valid cached fixture
    if let Some(cached_metadata) = load_cached_metadata(config.name) {
        if cached_metadata.schema_version_hash == current_hash {
            let root = cache_dir(config.name);
            if root.exists() {
                return GeneratedFixture {
                    root,
                    file_count: cached_metadata.config.file_count,
                    node_count: cached_metadata.config.node_count,
                    edge_count: cached_metadata.config.edge_count,
                    metadata: cached_metadata,
                };
            }
        }
    }

    // Cache miss or schema mismatch — regenerate
    let fixture = generator(config.seed);

    // Save metadata for future cache hits
    let metadata = FixtureMetadata {
        schema_version_hash: current_hash,
        generated_at: chrono::Utc::now().to_rfc3339(),
        config: FixtureConfigSerialized {
            name: config.name.to_string(),
            seed: config.seed,
            file_count: fixture.file_count,
            node_count: fixture.node_count,
            edge_count: fixture.edge_count,
        },
    };

    if let Err(e) = save_metadata(config.name, &metadata) {
        eprintln!("Warning: Failed to save fixture metadata: {}", e);
    }

    GeneratedFixture {
        root: fixture.root,
        file_count: fixture.file_count,
        node_count: fixture.node_count,
        edge_count: fixture.edge_count,
        metadata,
    }
}

/// Get the cache directory for a fixture (useful for generators to write into).
pub fn get_cache_dir(name: &str) -> PathBuf {
    cache_dir(name)
}

/// Clean a fixture's cache directory (useful before regeneration).
pub fn clean_cache(name: &str) -> Result<()> {
    let dir = cache_dir(name);
    if dir.exists() {
        fs::remove_dir_all(&dir)
            .with_context(|| format!("Failed to clean cache directory: {}", dir.display()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::path::Path;
    use tempfile::TempDir;

    /// Override cache_dir for tests by using a custom function that writes to a tempdir.
    /// Since we can't easily override the module-level function, we test with a helper
    /// that mimics the logic but uses a controlled path.
    fn test_load_or_generate_with_dir(
        cache_root: &Path,
        config: &FixtureConfig,
        generator: impl FnOnce(u64) -> GeneratedFixture,
    ) -> (GeneratedFixture, bool) {
        let current_hash = compute_schema_hash(config.schema_version);
        let meta_path = cache_root.join(config.name).join("metadata.json");

        // Check for valid cached fixture
        if let Ok(content) = fs::read_to_string(&meta_path) {
            if let Ok(cached_metadata) = serde_json::from_str::<FixtureMetadata>(&content) {
                if cached_metadata.schema_version_hash == current_hash {
                    let root = cache_root.join(config.name);
                    if root.exists() {
                        let fixture = GeneratedFixture {
                            root,
                            file_count: cached_metadata.config.file_count,
                            node_count: cached_metadata.config.node_count,
                            edge_count: cached_metadata.config.edge_count,
                            metadata: cached_metadata,
                        };
                        return (fixture, false); // generator was NOT called
                    }
                }
            }
        }

        // Cache miss — generate
        let fixture = generator(config.seed);

        let metadata = FixtureMetadata {
            schema_version_hash: current_hash,
            generated_at: chrono::Utc::now().to_rfc3339(),
            config: FixtureConfigSerialized {
                name: config.name.to_string(),
                seed: config.seed,
                file_count: fixture.file_count,
                node_count: fixture.node_count,
                edge_count: fixture.edge_count,
            },
        };

        // Write metadata
        let dir = cache_root.join(config.name);
        fs::create_dir_all(&dir).unwrap();
        let content = serde_json::to_string_pretty(&metadata).unwrap();
        fs::write(dir.join("metadata.json"), content).unwrap();

        let result = GeneratedFixture {
            root: dir,
            file_count: fixture.file_count,
            node_count: fixture.node_count,
            edge_count: fixture.edge_count,
            metadata,
        };

        (result, true) // generator WAS called
    }

    #[test]
    fn test_compute_schema_hash_deterministic() {
        let hash1 = compute_schema_hash("v1.0.0");
        let hash2 = compute_schema_hash("v1.0.0");
        assert_eq!(hash1, hash2);
    }

    #[test]
    fn test_compute_schema_hash_different_versions() {
        let hash1 = compute_schema_hash("v1.0.0");
        let hash2 = compute_schema_hash("v2.0.0");
        assert_ne!(hash1, hash2);
    }

    #[test]
    fn test_cache_miss_calls_generator() {
        let temp = TempDir::new().unwrap();
        let config = FixtureConfig {
            name: "test_fixture",
            seed: 42,
            schema_version: "v1.0.0",
        };

        let generator_called = Cell::new(false);
        let (fixture, was_called) = test_load_or_generate_with_dir(temp.path(), &config, |seed| {
            generator_called.set(true);
            GeneratedFixture {
                root: temp.path().join("test_fixture"),
                file_count: 100,
                node_count: 1000,
                edge_count: 3000,
                metadata: FixtureMetadata {
                    schema_version_hash: compute_schema_hash("v1.0.0"),
                    generated_at: chrono::Utc::now().to_rfc3339(),
                    config: FixtureConfigSerialized {
                        name: "test_fixture".to_string(),
                        seed,
                        file_count: 100,
                        node_count: 1000,
                        edge_count: 3000,
                    },
                },
            }
        });

        assert!(was_called);
        assert_eq!(fixture.file_count, 100);
        assert_eq!(fixture.node_count, 1000);
        assert_eq!(fixture.edge_count, 3000);
    }

    #[test]
    fn test_cache_hit_avoids_regeneration() {
        let temp = TempDir::new().unwrap();
        let config = FixtureConfig {
            name: "test_fixture",
            seed: 42,
            schema_version: "v1.0.0",
        };

        // First call — generates and caches
        let (_fixture, was_called) =
            test_load_or_generate_with_dir(temp.path(), &config, |seed| GeneratedFixture {
                root: temp.path().join("test_fixture"),
                file_count: 100,
                node_count: 1000,
                edge_count: 3000,
                metadata: FixtureMetadata {
                    schema_version_hash: compute_schema_hash("v1.0.0"),
                    generated_at: chrono::Utc::now().to_rfc3339(),
                    config: FixtureConfigSerialized {
                        name: "test_fixture".to_string(),
                        seed,
                        file_count: 100,
                        node_count: 1000,
                        edge_count: 3000,
                    },
                },
            });
        assert!(was_called);

        // Second call — should use cache
        let (_fixture, was_called) =
            test_load_or_generate_with_dir(temp.path(), &config, |_seed| {
                panic!("Generator should not be called on cache hit");
            });
        assert!(!was_called);
    }

    #[test]
    fn test_schema_version_change_invalidates_cache() {
        let temp = TempDir::new().unwrap();
        let config_v1 = FixtureConfig {
            name: "test_fixture",
            seed: 42,
            schema_version: "v1.0.0",
        };

        // First call with v1 — generates and caches
        let (_fixture, was_called) =
            test_load_or_generate_with_dir(temp.path(), &config_v1, |seed| GeneratedFixture {
                root: temp.path().join("test_fixture"),
                file_count: 100,
                node_count: 1000,
                edge_count: 3000,
                metadata: FixtureMetadata {
                    schema_version_hash: compute_schema_hash("v1.0.0"),
                    generated_at: chrono::Utc::now().to_rfc3339(),
                    config: FixtureConfigSerialized {
                        name: "test_fixture".to_string(),
                        seed,
                        file_count: 100,
                        node_count: 1000,
                        edge_count: 3000,
                    },
                },
            });
        assert!(was_called);

        // Second call with v2 — should regenerate
        let config_v2 = FixtureConfig {
            name: "test_fixture",
            seed: 42,
            schema_version: "v2.0.0",
        };

        let (fixture, was_called) =
            test_load_or_generate_with_dir(temp.path(), &config_v2, |seed| GeneratedFixture {
                root: temp.path().join("test_fixture"),
                file_count: 200,
                node_count: 2000,
                edge_count: 6000,
                metadata: FixtureMetadata {
                    schema_version_hash: compute_schema_hash("v2.0.0"),
                    generated_at: chrono::Utc::now().to_rfc3339(),
                    config: FixtureConfigSerialized {
                        name: "test_fixture".to_string(),
                        seed,
                        file_count: 200,
                        node_count: 2000,
                        edge_count: 6000,
                    },
                },
            });
        assert!(was_called);
        assert_eq!(fixture.file_count, 200);
        assert_eq!(fixture.node_count, 2000);
    }

    #[test]
    fn test_metadata_serialization_roundtrip() {
        let metadata = FixtureMetadata {
            schema_version_hash: compute_schema_hash("v1.0.0"),
            generated_at: "2024-12-01T00:00:00Z".to_string(),
            config: FixtureConfigSerialized {
                name: "test".to_string(),
                seed: 42,
                file_count: 100,
                node_count: 1000,
                edge_count: 3000,
            },
        };

        let json = serde_json::to_string(&metadata).unwrap();
        let deserialized: FixtureMetadata = serde_json::from_str(&json).unwrap();

        assert_eq!(
            metadata.schema_version_hash,
            deserialized.schema_version_hash
        );
        assert_eq!(metadata.generated_at, deserialized.generated_at);
        assert_eq!(metadata.config.name, deserialized.config.name);
        assert_eq!(metadata.config.seed, deserialized.config.seed);
        assert_eq!(metadata.config.file_count, deserialized.config.file_count);
        assert_eq!(metadata.config.node_count, deserialized.config.node_count);
        assert_eq!(metadata.config.edge_count, deserialized.config.edge_count);
    }

    #[test]
    fn test_cache_dir_path_structure() {
        let dir = cache_dir("my_fixture");
        assert!(dir.ends_with("target/bench-fixtures/my_fixture"));
    }

    #[test]
    fn test_metadata_path_structure() {
        let path = metadata_path("my_fixture");
        assert!(path.ends_with("target/bench-fixtures/my_fixture/metadata.json"));
    }
}
