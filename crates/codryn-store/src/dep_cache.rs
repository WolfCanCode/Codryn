use anyhow::{Context, Result};
use rusqlite::{params, OptionalExtension};
use std::time::Duration;

use crate::Store;

/// Cache TTL: entries older than 1 hour are considered stale.
const CACHE_TTL: Duration = Duration::from_secs(3600);

/// A cached dependency registry response.
#[derive(Debug, Clone, PartialEq)]
pub struct DepCacheEntry {
    pub package_name: String,
    pub registry: String,
    pub latest_version: Option<String>,
    pub deprecated: bool,
    pub checked_at: String,
}

impl Store {
    /// Look up a cached dependency entry by package name and registry.
    /// Returns `None` if no entry exists or if the entry is stale (older than 1 hour).
    pub fn dep_cache_lookup(
        &self,
        package_name: &str,
        registry: &str,
    ) -> Result<Option<DepCacheEntry>> {
        let mut stmt = self.conn.prepare(
            "SELECT package_name, registry, latest_version, deprecated, checked_at \
             FROM dep_cache WHERE package_name = ?1 AND registry = ?2",
        )?;
        let result = stmt
            .query_row(params![package_name, registry], |row| {
                Ok(DepCacheEntry {
                    package_name: row.get(0)?,
                    registry: row.get(1)?,
                    latest_version: row.get(2)?,
                    deprecated: row.get::<_, i32>(3)? != 0,
                    checked_at: row.get(4)?,
                })
            })
            .optional()
            .context("failed to query dep_cache")?;

        match result {
            Some(entry) => {
                if is_entry_stale(&entry.checked_at) {
                    // Entry is stale, remove it and return None
                    self.dep_cache_invalidate(package_name, registry)?;
                    Ok(None)
                } else {
                    Ok(Some(entry))
                }
            }
            None => Ok(None),
        }
    }

    /// Insert or update a dependency cache entry.
    /// Uses INSERT OR REPLACE to upsert based on the (package_name, registry) primary key.
    pub fn dep_cache_upsert(&self, entry: &DepCacheEntry) -> Result<()> {
        self.conn
            .execute(
                "INSERT OR REPLACE INTO dep_cache (package_name, registry, latest_version, deprecated, checked_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    entry.package_name,
                    entry.registry,
                    entry.latest_version,
                    entry.deprecated as i32,
                    entry.checked_at,
                ],
            )
            .context("failed to upsert dep_cache entry")?;
        Ok(())
    }

    /// Remove a specific entry from the dependency cache.
    pub fn dep_cache_invalidate(&self, package_name: &str, registry: &str) -> Result<()> {
        self.conn
            .execute(
                "DELETE FROM dep_cache WHERE package_name = ?1 AND registry = ?2",
                params![package_name, registry],
            )
            .context("failed to invalidate dep_cache entry")?;
        Ok(())
    }

    /// Remove all stale entries from the dependency cache (older than 1 hour).
    /// Returns the number of entries removed.
    pub fn dep_cache_invalidate_stale(&self) -> Result<usize> {
        let cutoff = cache_cutoff_iso8601();
        let count = self
            .conn
            .execute(
                "DELETE FROM dep_cache WHERE checked_at < ?1",
                params![cutoff],
            )
            .context("failed to invalidate stale dep_cache entries")?;
        Ok(count)
    }

    /// Remove all entries from the dependency cache.
    /// Returns the number of entries removed.
    pub fn dep_cache_clear_all(&self) -> Result<usize> {
        let count = self
            .conn
            .execute("DELETE FROM dep_cache", [])
            .context("failed to clear dep_cache")?;
        Ok(count)
    }

    /// Count the number of entries in the dependency cache.
    pub fn dep_cache_count(&self) -> Result<usize> {
        let count: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM dep_cache", [], |row| row.get(0))?;
        Ok(count as usize)
    }
}

/// Check if a cache entry is stale based on its `checked_at` timestamp.
/// An entry is stale if it was checked more than `CACHE_TTL` (1 hour) ago.
fn is_entry_stale(checked_at: &str) -> bool {
    let Ok(checked_time) = chrono::DateTime::parse_from_rfc3339(checked_at) else {
        // If we can't parse the timestamp, consider it stale
        return true;
    };
    let now = chrono::Utc::now();
    let age = now.signed_duration_since(checked_time);
    age.to_std().unwrap_or(Duration::from_secs(u64::MAX)) > CACHE_TTL
}

/// Compute the ISO 8601 cutoff timestamp for cache invalidation.
/// Entries with `checked_at` before this timestamp are considered stale.
fn cache_cutoff_iso8601() -> String {
    let cutoff = chrono::Utc::now() - chrono::Duration::seconds(CACHE_TTL.as_secs() as i64);
    cutoff.to_rfc3339()
}

/// Get the current time as an ISO 8601 / RFC 3339 string.
/// Useful for callers that need to set `checked_at` when creating entries.
pub fn now_iso8601() -> String {
    chrono::Utc::now().to_rfc3339()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_store() -> Store {
        Store::open_in_memory().unwrap()
    }

    #[test]
    fn test_dep_cache_table_created() {
        let store = test_store();
        // Verify the table exists by counting rows (should be 0)
        let count = store.dep_cache_count().unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn test_dep_cache_upsert_and_lookup() {
        let store = test_store();
        let now = now_iso8601();

        let entry = DepCacheEntry {
            package_name: "serde".to_string(),
            registry: "crates.io".to_string(),
            latest_version: Some("1.0.200".to_string()),
            deprecated: false,
            checked_at: now.clone(),
        };

        store.dep_cache_upsert(&entry).unwrap();

        let cached = store
            .dep_cache_lookup("serde", "crates.io")
            .unwrap()
            .unwrap();
        assert_eq!(cached.package_name, "serde");
        assert_eq!(cached.registry, "crates.io");
        assert_eq!(cached.latest_version, Some("1.0.200".to_string()));
        assert!(!cached.deprecated);
    }

    #[test]
    fn test_dep_cache_upsert_replaces_existing() {
        let store = test_store();
        let now = now_iso8601();

        let entry1 = DepCacheEntry {
            package_name: "tokio".to_string(),
            registry: "crates.io".to_string(),
            latest_version: Some("1.30.0".to_string()),
            deprecated: false,
            checked_at: now.clone(),
        };
        store.dep_cache_upsert(&entry1).unwrap();

        let entry2 = DepCacheEntry {
            package_name: "tokio".to_string(),
            registry: "crates.io".to_string(),
            latest_version: Some("1.31.0".to_string()),
            deprecated: false,
            checked_at: now.clone(),
        };
        store.dep_cache_upsert(&entry2).unwrap();

        let cached = store
            .dep_cache_lookup("tokio", "crates.io")
            .unwrap()
            .unwrap();
        assert_eq!(cached.latest_version, Some("1.31.0".to_string()));
        assert_eq!(store.dep_cache_count().unwrap(), 1);
    }

    #[test]
    fn test_dep_cache_lookup_nonexistent() {
        let store = test_store();
        let result = store.dep_cache_lookup("nonexistent", "npm").unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_dep_cache_stale_entry_returns_none() {
        let store = test_store();

        // Insert an entry with a timestamp from 2 hours ago
        let two_hours_ago = chrono::Utc::now() - chrono::Duration::hours(2);
        let entry = DepCacheEntry {
            package_name: "express".to_string(),
            registry: "npm".to_string(),
            latest_version: Some("4.18.2".to_string()),
            deprecated: false,
            checked_at: two_hours_ago.to_rfc3339(),
        };
        store.dep_cache_upsert(&entry).unwrap();

        // Lookup should return None because the entry is stale
        let result = store.dep_cache_lookup("express", "npm").unwrap();
        assert!(result.is_none());

        // The stale entry should have been removed
        assert_eq!(store.dep_cache_count().unwrap(), 0);
    }

    #[test]
    fn test_dep_cache_fresh_entry_returns_some() {
        let store = test_store();

        // Insert an entry with a timestamp from 30 minutes ago (within TTL)
        let thirty_min_ago = chrono::Utc::now() - chrono::Duration::minutes(30);
        let entry = DepCacheEntry {
            package_name: "lodash".to_string(),
            registry: "npm".to_string(),
            latest_version: Some("4.17.21".to_string()),
            deprecated: false,
            checked_at: thirty_min_ago.to_rfc3339(),
        };
        store.dep_cache_upsert(&entry).unwrap();

        let result = store.dep_cache_lookup("lodash", "npm").unwrap();
        assert!(result.is_some());
        assert_eq!(result.unwrap().latest_version, Some("4.17.21".to_string()));
    }

    #[test]
    fn test_dep_cache_invalidate_specific() {
        let store = test_store();
        let now = now_iso8601();

        store
            .dep_cache_upsert(&DepCacheEntry {
                package_name: "serde".to_string(),
                registry: "crates.io".to_string(),
                latest_version: Some("1.0.200".to_string()),
                deprecated: false,
                checked_at: now.clone(),
            })
            .unwrap();
        store
            .dep_cache_upsert(&DepCacheEntry {
                package_name: "tokio".to_string(),
                registry: "crates.io".to_string(),
                latest_version: Some("1.31.0".to_string()),
                deprecated: false,
                checked_at: now.clone(),
            })
            .unwrap();

        assert_eq!(store.dep_cache_count().unwrap(), 2);

        store.dep_cache_invalidate("serde", "crates.io").unwrap();

        assert_eq!(store.dep_cache_count().unwrap(), 1);
        assert!(store
            .dep_cache_lookup("serde", "crates.io")
            .unwrap()
            .is_none());
        assert!(store
            .dep_cache_lookup("tokio", "crates.io")
            .unwrap()
            .is_some());
    }

    #[test]
    fn test_dep_cache_invalidate_stale() {
        let store = test_store();
        let now = now_iso8601();
        let two_hours_ago = (chrono::Utc::now() - chrono::Duration::hours(2)).to_rfc3339();

        // Fresh entry
        store
            .dep_cache_upsert(&DepCacheEntry {
                package_name: "fresh-pkg".to_string(),
                registry: "npm".to_string(),
                latest_version: Some("1.0.0".to_string()),
                deprecated: false,
                checked_at: now.clone(),
            })
            .unwrap();

        // Stale entry
        store
            .dep_cache_upsert(&DepCacheEntry {
                package_name: "stale-pkg".to_string(),
                registry: "npm".to_string(),
                latest_version: Some("2.0.0".to_string()),
                deprecated: false,
                checked_at: two_hours_ago,
            })
            .unwrap();

        assert_eq!(store.dep_cache_count().unwrap(), 2);

        let removed = store.dep_cache_invalidate_stale().unwrap();
        assert_eq!(removed, 1);
        assert_eq!(store.dep_cache_count().unwrap(), 1);
    }

    #[test]
    fn test_dep_cache_clear_all() {
        let store = test_store();
        let now = now_iso8601();

        store
            .dep_cache_upsert(&DepCacheEntry {
                package_name: "a".to_string(),
                registry: "npm".to_string(),
                latest_version: Some("1.0.0".to_string()),
                deprecated: false,
                checked_at: now.clone(),
            })
            .unwrap();
        store
            .dep_cache_upsert(&DepCacheEntry {
                package_name: "b".to_string(),
                registry: "pypi".to_string(),
                latest_version: None,
                deprecated: true,
                checked_at: now.clone(),
            })
            .unwrap();

        assert_eq!(store.dep_cache_count().unwrap(), 2);

        let removed = store.dep_cache_clear_all().unwrap();
        assert_eq!(removed, 2);
        assert_eq!(store.dep_cache_count().unwrap(), 0);
    }

    #[test]
    fn test_dep_cache_deprecated_flag() {
        let store = test_store();
        let now = now_iso8601();

        let entry = DepCacheEntry {
            package_name: "old-pkg".to_string(),
            registry: "npm".to_string(),
            latest_version: Some("3.0.0".to_string()),
            deprecated: true,
            checked_at: now,
        };
        store.dep_cache_upsert(&entry).unwrap();

        let cached = store.dep_cache_lookup("old-pkg", "npm").unwrap().unwrap();
        assert!(cached.deprecated);
    }

    #[test]
    fn test_dep_cache_null_latest_version() {
        let store = test_store();
        let now = now_iso8601();

        let entry = DepCacheEntry {
            package_name: "unknown-pkg".to_string(),
            registry: "crates.io".to_string(),
            latest_version: None,
            deprecated: false,
            checked_at: now,
        };
        store.dep_cache_upsert(&entry).unwrap();

        let cached = store
            .dep_cache_lookup("unknown-pkg", "crates.io")
            .unwrap()
            .unwrap();
        assert_eq!(cached.latest_version, None);
    }

    #[test]
    fn test_dep_cache_same_package_different_registries() {
        let store = test_store();
        let now = now_iso8601();

        // Same package name in different registries should be separate entries
        store
            .dep_cache_upsert(&DepCacheEntry {
                package_name: "requests".to_string(),
                registry: "pypi".to_string(),
                latest_version: Some("2.31.0".to_string()),
                deprecated: false,
                checked_at: now.clone(),
            })
            .unwrap();
        store
            .dep_cache_upsert(&DepCacheEntry {
                package_name: "requests".to_string(),
                registry: "npm".to_string(),
                latest_version: Some("1.0.0".to_string()),
                deprecated: false,
                checked_at: now.clone(),
            })
            .unwrap();

        assert_eq!(store.dep_cache_count().unwrap(), 2);

        let pypi = store.dep_cache_lookup("requests", "pypi").unwrap().unwrap();
        assert_eq!(pypi.latest_version, Some("2.31.0".to_string()));

        let npm = store.dep_cache_lookup("requests", "npm").unwrap().unwrap();
        assert_eq!(npm.latest_version, Some("1.0.0".to_string()));
    }

    #[test]
    fn test_is_entry_stale_with_invalid_timestamp() {
        // Invalid timestamp should be considered stale
        assert!(is_entry_stale("not-a-timestamp"));
        assert!(is_entry_stale(""));
    }

    #[test]
    fn test_is_entry_stale_with_old_timestamp() {
        let two_hours_ago = (chrono::Utc::now() - chrono::Duration::hours(2)).to_rfc3339();
        assert!(is_entry_stale(&two_hours_ago));
    }

    #[test]
    fn test_is_entry_stale_with_fresh_timestamp() {
        let five_min_ago = (chrono::Utc::now() - chrono::Duration::minutes(5)).to_rfc3339();
        assert!(!is_entry_stale(&five_min_ago));
    }

    #[test]
    fn test_now_iso8601_is_parseable() {
        let now = now_iso8601();
        let parsed = chrono::DateTime::parse_from_rfc3339(&now);
        assert!(parsed.is_ok());
    }
}
