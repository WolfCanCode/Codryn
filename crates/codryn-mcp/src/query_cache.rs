use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

#[derive(Clone, Hash, Eq, PartialEq)]
pub struct CacheKey {
    pub tool: String,
    pub project: String,
    pub args_hash: u64,
}

struct CacheEntry {
    result: String,
    cached_at: Instant,
}

#[derive(Default, Clone)]
pub struct CacheStats {
    pub hits: u64,
    pub misses: u64,
    pub invalidations: u64,
}

#[derive(Clone)]
pub struct QueryCache {
    inner: Arc<RwLock<HashMap<CacheKey, CacheEntry>>>,
    stats: Arc<RwLock<CacheStats>>,
    ttl: Duration,
}

impl std::fmt::Debug for QueryCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("QueryCache")
            .field("ttl", &self.ttl)
            .finish()
    }
}

impl QueryCache {
    pub fn new(ttl: Duration) -> Self {
        Self {
            inner: Arc::new(RwLock::new(HashMap::new())),
            stats: Arc::new(RwLock::new(CacheStats::default())),
            ttl,
        }
    }

    pub fn get(&self, key: &CacheKey) -> Option<String> {
        let inner = self.inner.read().unwrap();
        if let Some(entry) = inner.get(key) {
            if entry.cached_at.elapsed() < self.ttl {
                let mut stats = self.stats.write().unwrap();
                stats.hits += 1;
                return Some(entry.result.clone());
            }
        }
        let mut stats = self.stats.write().unwrap();
        stats.misses += 1;
        None
    }

    pub fn put(&self, key: CacheKey, result: String) {
        let mut inner = self.inner.write().unwrap();
        inner.insert(
            key,
            CacheEntry {
                result,
                cached_at: Instant::now(),
            },
        );
    }

    pub fn invalidate_project(&self, project: &str) {
        let mut inner = self.inner.write().unwrap();
        inner.retain(|k, _| k.project != project);
        let mut stats = self.stats.write().unwrap();
        stats.invalidations += 1;
    }

    pub fn invalidate_all(&self) {
        let mut inner = self.inner.write().unwrap();
        inner.clear();
        let mut stats = self.stats.write().unwrap();
        stats.invalidations += 1;
    }

    pub fn stats(&self) -> CacheStats {
        self.stats.read().unwrap().clone()
    }
}

pub fn cache_key(tool: &str, project: &str, args_json: &str) -> CacheKey {
    let mut hasher = DefaultHasher::new();
    args_json.hash(&mut hasher);
    CacheKey {
        tool: tool.to_string(),
        project: project.to_string(),
        args_hash: hasher.finish(),
    }
}
