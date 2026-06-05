use serde::Serialize;
use std::time::Instant;

/// Health status response for the `health_check` MCP tool.
#[derive(Debug, Clone, Serialize)]
pub struct HealthStatus {
    /// "ok" or "degraded"
    pub status: String,
    /// Seconds since the server started.
    pub uptime_seconds: u64,
    /// Crate version from Cargo.toml.
    pub version: String,
    /// Number of indexed projects in the store.
    pub indexed_projects: usize,
    /// Whether the store connection is healthy.
    pub store_ok: bool,
    /// Number of currently running index operations.
    pub active_index_runs: usize,
    /// Error description if status is "degraded".
    pub error: Option<String>,
}

impl HealthStatus {
    /// Build a health status by probing the store and auto-indexer.
    pub fn check(
        start_time: Instant,
        store_result: Result<usize, String>,
        active_index_runs: usize,
    ) -> Self {
        let uptime_seconds = start_time.elapsed().as_secs();
        let version = env!("CARGO_PKG_VERSION").to_string();

        match store_result {
            Ok(project_count) => Self {
                status: "ok".to_string(),
                uptime_seconds,
                version,
                indexed_projects: project_count,
                store_ok: true,
                active_index_runs,
                error: None,
            },
            Err(err) => Self {
                status: "degraded".to_string(),
                uptime_seconds,
                version,
                indexed_projects: 0,
                store_ok: false,
                active_index_runs,
                error: Some(err),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_health_status_ok() {
        let start = Instant::now();
        let status = HealthStatus::check(start, Ok(5), 0);
        assert_eq!(status.status, "ok");
        assert_eq!(status.indexed_projects, 5);
        assert!(status.store_ok);
        assert_eq!(status.active_index_runs, 0);
        assert!(status.error.is_none());
        assert!(!status.version.is_empty());
    }

    #[test]
    fn test_health_status_degraded() {
        let start = Instant::now();
        let status = HealthStatus::check(start, Err("connection failed".to_string()), 2);
        assert_eq!(status.status, "degraded");
        assert_eq!(status.indexed_projects, 0);
        assert!(!status.store_ok);
        assert_eq!(status.active_index_runs, 2);
        assert_eq!(status.error, Some("connection failed".to_string()));
    }

    #[test]
    fn test_health_status_uptime() {
        let start = Instant::now();
        // Small sleep to ensure uptime > 0
        std::thread::sleep(std::time::Duration::from_millis(10));
        let status = HealthStatus::check(start, Ok(0), 0);
        // Uptime should be at least 0 (could be 0 if < 1s elapsed)
        assert!(status.uptime_seconds < 5);
    }

    #[test]
    fn test_health_status_serialization() {
        let start = Instant::now();
        let status = HealthStatus::check(start, Ok(3), 1);
        let json = serde_json::to_string(&status).unwrap();
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["status"], "ok");
        assert_eq!(value["indexed_projects"], 3);
        assert_eq!(value["store_ok"], true);
        assert_eq!(value["active_index_runs"], 1);
        assert!(value["error"].is_null());
        assert!(value.get("uptime_seconds").is_some());
        assert!(value.get("version").is_some());
    }
}
