//! Connection pool for SQLite with reader/writer separation.
//!
//! Exploits SQLite WAL mode to allow concurrent reads while maintaining
//! exclusive write access. The pool uses a semaphore to limit concurrent
//! reader access and a tokio mutex for the single writer connection.

use anyhow::{Context, Result};
use rusqlite::Connection;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::{Mutex, OwnedSemaphorePermit, Semaphore};
use tracing::debug;

use crate::schema;
use codryn_foundation::config::AppConfig;

/// Configuration for the connection pool.
#[derive(Debug, Clone)]
pub struct PoolConfig {
    /// Maximum number of concurrent reader connections (default: 4).
    pub max_readers: usize,
    /// SQLite busy timeout in milliseconds (default: 10_000).
    pub busy_timeout_ms: u32,
}

impl Default for PoolConfig {
    fn default() -> Self {
        Self {
            max_readers: 4,
            busy_timeout_ms: 10_000,
        }
    }
}

impl PoolConfig {
    /// Create a `PoolConfig` from an `AppConfig`, using defaults for missing values.
    pub fn from_app_config(app_config: &AppConfig) -> Self {
        Self {
            max_readers: app_config.pool_size.unwrap_or(4),
            ..Default::default()
        }
    }
}

/// Diagnostics metrics for the connection pool.
#[derive(Debug, Clone)]
pub struct PoolMetrics {
    /// Number of reader connections in the pool.
    pub total_readers: usize,
    /// Number of currently available reader permits.
    pub available_readers: usize,
    /// Total number of read acquisitions since pool creation.
    pub total_reads: u64,
    /// Total number of write acquisitions since pool creation.
    pub total_writes: u64,
}

/// A pool of SQLite connections supporting concurrent reads with exclusive writes.
///
/// Uses SQLite WAL mode so readers don't block the writer and vice versa.
/// Reader access is controlled by a semaphore; writer access is exclusive via a mutex.
pub struct StorePool {
    /// Path to the database file.
    db_path: PathBuf,
    /// Pool of read-only connections.
    readers: Vec<Arc<Mutex<Connection>>>,
    /// Single write connection.
    writer: Arc<Mutex<Connection>>,
    /// Semaphore controlling reader access.
    reader_sem: Arc<Semaphore>,
    /// Configuration.
    config: PoolConfig,
    /// Counter for total read acquisitions.
    read_count: AtomicU64,
    /// Counter for total write acquisitions.
    write_count: AtomicU64,
}

impl StorePool {
    /// Open a new connection pool at the given database path.
    ///
    /// Creates the writer connection and `config.max_readers` reader connections,
    /// all configured with WAL mode and the specified busy timeout.
    pub fn open(path: &Path, config: PoolConfig) -> Result<Self> {
        debug!(
            "Opening StorePool at {} with {} readers, {}ms busy timeout",
            path.display(),
            config.max_readers,
            config.busy_timeout_ms
        );

        // Create the writer connection first (it will set up WAL mode)
        let writer_conn = Self::create_connection(path, &config, false)?;

        // Initialize schema on the writer connection
        writer_conn
            .execute_batch(schema::DDL)
            .context("failed to apply DDL schema on writer")?;
        writer_conn
            .execute_batch(schema::INDEXES)
            .context("failed to create indexes on writer")?;
        schema::migrate_tool_calls(&writer_conn);

        // Create reader connections
        let mut readers = Vec::with_capacity(config.max_readers);
        for i in 0..config.max_readers {
            let conn = Self::create_connection(path, &config, true)
                .with_context(|| format!("failed to create reader connection {}", i))?;
            readers.push(Arc::new(Mutex::new(conn)));
        }

        let reader_sem = Arc::new(Semaphore::new(config.max_readers));

        Ok(Self {
            db_path: path.to_path_buf(),
            readers,
            writer: Arc::new(Mutex::new(writer_conn)),
            reader_sem,
            config,
            read_count: AtomicU64::new(0),
            write_count: AtomicU64::new(0),
        })
    }

    /// Acquire a read-only connection from the pool.
    ///
    /// Blocks (asynchronously) if all reader connections are currently in use.
    /// The connection is returned to the pool when the `PooledReader` is dropped.
    pub async fn read(&self) -> Result<PooledReader> {
        let permit = self
            .reader_sem
            .clone()
            .acquire_owned()
            .await
            .context("reader semaphore closed")?;

        // Find the reader index based on available permits
        let reader_idx = self.config.max_readers - self.reader_sem.available_permits() - 1;
        let reader_idx = reader_idx.min(self.readers.len() - 1);

        let conn = self.readers[reader_idx].clone();
        self.read_count.fetch_add(1, Ordering::Relaxed);

        Ok(PooledReader {
            conn,
            _permit: permit,
        })
    }

    /// Acquire the exclusive write connection.
    ///
    /// Blocks (asynchronously) until the writer is available. Only one write
    /// operation can proceed at a time, but reads are not blocked.
    pub async fn write(&self) -> Result<PooledWriter> {
        let conn = self.writer.clone();
        self.write_count.fetch_add(1, Ordering::Relaxed);

        Ok(PooledWriter { conn })
    }

    /// Get current pool metrics for diagnostics.
    pub fn metrics(&self) -> PoolMetrics {
        PoolMetrics {
            total_readers: self.config.max_readers,
            available_readers: self.reader_sem.available_permits(),
            total_reads: self.read_count.load(Ordering::Relaxed),
            total_writes: self.write_count.load(Ordering::Relaxed),
        }
    }

    /// Returns the database path this pool is connected to.
    pub fn db_path(&self) -> &Path {
        &self.db_path
    }

    /// Create a single SQLite connection with appropriate pragmas.
    fn create_connection(path: &Path, config: &PoolConfig, read_only: bool) -> Result<Connection> {
        let conn = if read_only {
            Connection::open_with_flags(
                path,
                rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY
                    | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX
                    | rusqlite::OpenFlags::SQLITE_OPEN_URI,
            )
            .with_context(|| format!("failed to open read-only connection at {}", path.display()))?
        } else {
            Connection::open(path)
                .with_context(|| format!("failed to open write connection at {}", path.display()))?
        };

        // Configure WAL mode and pragmas
        conn.execute_batch(&format!(
            "PRAGMA journal_mode = WAL;\
             PRAGMA busy_timeout = {};\
             PRAGMA synchronous = NORMAL;\
             PRAGMA cache_size = -64000;\
             PRAGMA foreign_keys = ON;",
            config.busy_timeout_ms
        ))
        .context("failed to configure connection pragmas")?;

        Ok(conn)
    }
}

/// A read-only connection borrowed from the pool.
///
/// The connection is returned to the pool when this guard is dropped.
/// The semaphore permit ensures bounded concurrency.
pub struct PooledReader {
    conn: Arc<Mutex<Connection>>,
    _permit: OwnedSemaphorePermit,
}

impl PooledReader {
    /// Access the underlying connection. The caller must hold the lock
    /// for the duration of their operation.
    pub async fn conn(&self) -> tokio::sync::MutexGuard<'_, Connection> {
        self.conn.lock().await
    }
}

/// An exclusive write connection borrowed from the pool.
///
/// Only one `PooledWriter` can be active at a time due to the mutex.
/// The connection is released when this guard is dropped.
pub struct PooledWriter {
    conn: Arc<Mutex<Connection>>,
}

impl PooledWriter {
    /// Access the underlying connection. The caller must hold the lock
    /// for the duration of their operation.
    pub async fn conn(&self) -> tokio::sync::MutexGuard<'_, Connection> {
        self.conn.lock().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::NamedTempFile;

    fn temp_db_path() -> PathBuf {
        let tmp = NamedTempFile::new().unwrap();
        tmp.path().to_path_buf()
    }

    #[tokio::test]
    async fn test_pool_open_default_config() {
        let path = temp_db_path();
        let pool = StorePool::open(&path, PoolConfig::default()).unwrap();
        assert_eq!(pool.config.max_readers, 4);
        assert_eq!(pool.config.busy_timeout_ms, 10_000);
        assert_eq!(pool.readers.len(), 4);
    }

    #[tokio::test]
    async fn test_pool_open_custom_config() {
        let path = temp_db_path();
        let config = PoolConfig {
            max_readers: 2,
            busy_timeout_ms: 5_000,
        };
        let pool = StorePool::open(&path, config).unwrap();
        assert_eq!(pool.readers.len(), 2);
    }

    #[tokio::test]
    async fn test_pool_read_returns_connection() {
        let path = temp_db_path();
        let pool = StorePool::open(&path, PoolConfig::default()).unwrap();
        let reader = pool.read().await.unwrap();
        let conn = reader.conn().await;
        // Verify we can execute a simple query
        let result: i64 = conn.query_row("SELECT 1", [], |row| row.get(0)).unwrap();
        assert_eq!(result, 1);
    }

    #[tokio::test]
    async fn test_pool_write_returns_connection() {
        let path = temp_db_path();
        let pool = StorePool::open(&path, PoolConfig::default()).unwrap();
        let writer = pool.write().await.unwrap();
        let conn = writer.conn().await;
        // Verify we can execute a write operation
        conn.execute_batch("CREATE TABLE IF NOT EXISTS test_pool (id INTEGER)")
            .unwrap();
    }

    #[tokio::test]
    async fn test_pool_metrics_initial() {
        let path = temp_db_path();
        let pool = StorePool::open(&path, PoolConfig::default()).unwrap();
        let metrics = pool.metrics();
        assert_eq!(metrics.total_readers, 4);
        assert_eq!(metrics.available_readers, 4);
        assert_eq!(metrics.total_reads, 0);
        assert_eq!(metrics.total_writes, 0);
    }

    #[tokio::test]
    async fn test_pool_metrics_after_operations() {
        let path = temp_db_path();
        let pool = StorePool::open(&path, PoolConfig::default()).unwrap();

        // Perform some reads and writes
        {
            let _reader = pool.read().await.unwrap();
        }
        {
            let _writer = pool.write().await.unwrap();
        }

        let metrics = pool.metrics();
        assert_eq!(metrics.total_reads, 1);
        assert_eq!(metrics.total_writes, 1);
        // After dropping, all readers should be available again
        assert_eq!(metrics.available_readers, 4);
    }

    #[tokio::test]
    async fn test_pool_concurrent_reads() {
        let path = temp_db_path();
        let config = PoolConfig {
            max_readers: 2,
            ..Default::default()
        };
        let pool = Arc::new(StorePool::open(&path, config).unwrap());

        // Acquire two readers simultaneously
        let r1 = pool.read().await.unwrap();
        let r2 = pool.read().await.unwrap();

        // Both should work
        let conn1 = r1.conn().await;
        let conn2 = r2.conn().await;
        let v1: i64 = conn1.query_row("SELECT 1", [], |row| row.get(0)).unwrap();
        let v2: i64 = conn2.query_row("SELECT 2", [], |row| row.get(0)).unwrap();
        assert_eq!(v1, 1);
        assert_eq!(v2, 2);
    }

    #[tokio::test]
    async fn test_pool_from_app_config() {
        let app_config = AppConfig {
            pool_size: Some(8),
            ..Default::default()
        };
        let pool_config = PoolConfig::from_app_config(&app_config);
        assert_eq!(pool_config.max_readers, 8);
        assert_eq!(pool_config.busy_timeout_ms, 10_000);
    }

    #[tokio::test]
    async fn test_pool_from_app_config_defaults() {
        let app_config = AppConfig::default();
        let pool_config = PoolConfig::from_app_config(&app_config);
        assert_eq!(pool_config.max_readers, 4);
    }

    #[tokio::test]
    async fn test_pool_reader_semaphore_limits_concurrency() {
        let path = temp_db_path();
        let config = PoolConfig {
            max_readers: 2,
            ..Default::default()
        };
        let pool = StorePool::open(&path, config).unwrap();

        // Acquire all readers
        let _r1 = pool.read().await.unwrap();
        let _r2 = pool.read().await.unwrap();

        // Metrics should show 0 available
        let metrics = pool.metrics();
        assert_eq!(metrics.available_readers, 0);
    }

    #[tokio::test]
    async fn test_pool_reader_released_on_drop() {
        let path = temp_db_path();
        let config = PoolConfig {
            max_readers: 1,
            ..Default::default()
        };
        let pool = StorePool::open(&path, config).unwrap();

        // Acquire and release
        {
            let _r = pool.read().await.unwrap();
            assert_eq!(pool.metrics().available_readers, 0);
        }

        // Should be available again
        assert_eq!(pool.metrics().available_readers, 1);

        // Should be able to acquire again
        let _r = pool.read().await.unwrap();
        assert_eq!(pool.metrics().available_readers, 0);
    }

    #[tokio::test]
    async fn test_pool_wal_mode_enabled() {
        let path = temp_db_path();
        let pool = StorePool::open(&path, PoolConfig::default()).unwrap();
        let writer = pool.write().await.unwrap();
        let conn = writer.conn().await;
        let mode: String = conn
            .query_row("PRAGMA journal_mode", [], |row| row.get(0))
            .unwrap();
        assert_eq!(mode.to_lowercase(), "wal");
    }
}
