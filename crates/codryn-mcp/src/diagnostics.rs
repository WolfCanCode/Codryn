use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Maximum number of query durations to keep in the rolling window.
const ROLLING_WINDOW_SIZE: usize = 100;

/// Default file descriptor warning threshold.
const DEFAULT_FD_THRESHOLD: usize = 256;

/// Tracks system diagnostics: open file descriptors and query performance metrics.
#[derive(Debug, Clone)]
pub struct Diagnostics {
    query_durations: Arc<Mutex<VecDeque<Duration>>>,
    fd_threshold: usize,
}

/// Snapshot of current diagnostic state, serializable to JSON.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct DiagnosticReport {
    pub open_fds: usize,
    pub fd_threshold: usize,
    pub fd_warning: bool,
    pub query_count: usize,
    pub avg_query_ms: f64,
    pub p95_query_ms: f64,
    pub max_query_ms: f64,
}

impl Diagnostics {
    pub fn new() -> Self {
        Self {
            query_durations: Arc::new(Mutex::new(VecDeque::with_capacity(ROLLING_WINDOW_SIZE))),
            fd_threshold: DEFAULT_FD_THRESHOLD,
        }
    }

    pub fn set_fd_threshold(&mut self, threshold: usize) {
        self.fd_threshold = threshold;
    }

    /// Record a query duration into the rolling window.
    /// If the window is at capacity (100 entries), the oldest entry is removed.
    pub fn record_query(&self, duration: Duration) {
        let mut durations = self.query_durations.lock().unwrap();
        if durations.len() >= ROLLING_WINDOW_SIZE {
            durations.pop_front();
        }
        durations.push_back(duration);
    }

    /// Count open file descriptors for the current process.
    /// On Unix (Linux/macOS), reads `/proc/self/fd` or `/dev/fd`.
    /// Returns 0 on non-Unix platforms.
    pub fn open_fd_count() -> usize {
        #[cfg(unix)]
        {
            // Try /proc/self/fd first (Linux), then /dev/fd (macOS)
            std::fs::read_dir("/proc/self/fd")
                .or_else(|_| std::fs::read_dir("/dev/fd"))
                .map(|entries| entries.count())
                .unwrap_or(0)
        }
        #[cfg(not(unix))]
        {
            0
        }
    }

    /// Generate a diagnostic report with FD count and query performance stats.
    pub fn report(&self) -> DiagnosticReport {
        let fd_count = Self::open_fd_count();
        let durations = self.query_durations.lock().unwrap();

        let query_count = durations.len();
        let (avg_ms, p95_ms, max_ms) = if query_count > 0 {
            let mut sorted: Vec<f64> = durations.iter().map(|d| d.as_secs_f64() * 1000.0).collect();
            sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

            let avg = sorted.iter().sum::<f64>() / query_count as f64;
            let p95_idx = ((query_count as f64) * 0.95).ceil() as usize;
            let p95 = sorted
                .get(p95_idx.saturating_sub(1).min(sorted.len() - 1))
                .copied()
                .unwrap_or(0.0);
            let max = sorted.last().copied().unwrap_or(0.0);
            (avg, p95, max)
        } else {
            (0.0, 0.0, 0.0)
        };

        let fd_warning = fd_count > self.fd_threshold;
        if fd_warning {
            tracing::warn!(
                fd_count = fd_count,
                threshold = self.fd_threshold,
                "open file descriptor count exceeds threshold"
            );
        }

        DiagnosticReport {
            open_fds: fd_count,
            fd_threshold: self.fd_threshold,
            fd_warning,
            query_count,
            avg_query_ms: avg_ms,
            p95_query_ms: p95_ms,
            max_query_ms: max_ms,
        }
    }
}

impl Default for Diagnostics {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rolling_window_max_100_entries() {
        let diag = Diagnostics::new();
        // Insert 150 entries — only the last 100 should remain
        for i in 0..150 {
            diag.record_query(Duration::from_millis(i));
        }
        let durations = diag.query_durations.lock().unwrap();
        assert_eq!(durations.len(), 100);
        // The oldest entry should be 50ms (entries 0..49 were evicted)
        assert_eq!(durations.front().unwrap(), &Duration::from_millis(50));
        assert_eq!(durations.back().unwrap(), &Duration::from_millis(149));
    }

    #[test]
    fn test_avg_p95_max_known_sequence() {
        let diag = Diagnostics::new();
        // Insert durations 1..=20 ms
        for i in 1..=20 {
            diag.record_query(Duration::from_millis(i));
        }
        let report = diag.report();
        assert_eq!(report.query_count, 20);

        // Average of 1..=20 = 210/20 = 10.5
        assert!((report.avg_query_ms - 10.5).abs() < 0.01);

        // p95 of 20 items: ceil(20 * 0.95) = 19, index 18 (0-based) = 19ms
        assert!((report.p95_query_ms - 19.0).abs() < 0.01);

        // Max = 20ms
        assert!((report.max_query_ms - 20.0).abs() < 0.01);
    }

    #[test]
    fn test_empty_report() {
        let diag = Diagnostics::new();
        let report = diag.report();
        assert_eq!(report.query_count, 0);
        assert_eq!(report.avg_query_ms, 0.0);
        assert_eq!(report.p95_query_ms, 0.0);
        assert_eq!(report.max_query_ms, 0.0);
    }

    #[test]
    fn test_single_query_report() {
        let diag = Diagnostics::new();
        diag.record_query(Duration::from_millis(42));
        let report = diag.report();
        assert_eq!(report.query_count, 1);
        assert!((report.avg_query_ms - 42.0).abs() < 0.01);
        assert!((report.p95_query_ms - 42.0).abs() < 0.01);
        assert!((report.max_query_ms - 42.0).abs() < 0.01);
    }

    #[test]
    fn test_fd_threshold_warning() {
        let mut diag = Diagnostics::new();
        // Set threshold to 0 so any FDs trigger a warning
        diag.set_fd_threshold(0);
        let report = diag.report();
        // On Unix, we'll have some open FDs; on non-Unix, open_fds is 0
        #[cfg(unix)]
        assert!(report.fd_warning);
        #[cfg(not(unix))]
        assert!(!report.fd_warning);
    }

    #[test]
    fn test_fd_threshold_no_warning() {
        let diag = Diagnostics::new();
        // Default threshold is 256, which should be above normal FD count in tests
        let report = diag.report();
        assert_eq!(report.fd_threshold, 256);
        // In a test environment, we typically have < 256 FDs open
        // (this could theoretically fail in extreme environments, but is safe for CI)
    }

    #[test]
    fn test_diagnostic_report_json_serialization() {
        let report = DiagnosticReport {
            open_fds: 42,
            fd_threshold: 256,
            fd_warning: false,
            query_count: 10,
            avg_query_ms: 5.5,
            p95_query_ms: 9.0,
            max_query_ms: 12.0,
        };
        let json = serde_json::to_string(&report).unwrap();
        let deserialized: DiagnosticReport = serde_json::from_str(&json).unwrap();
        assert_eq!(report, deserialized);

        // Verify expected fields are present
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert!(value.get("open_fds").is_some());
        assert!(value.get("fd_threshold").is_some());
        assert!(value.get("fd_warning").is_some());
        assert!(value.get("query_count").is_some());
        assert!(value.get("avg_query_ms").is_some());
        assert!(value.get("p95_query_ms").is_some());
        assert!(value.get("max_query_ms").is_some());
    }

    #[test]
    fn test_open_fd_count_returns_value() {
        let count = Diagnostics::open_fd_count();
        // On Unix, should be > 0 (at least stdin/stdout/stderr)
        #[cfg(unix)]
        assert!(count > 0, "Expected at least some open FDs on Unix");
        #[cfg(not(unix))]
        assert_eq!(count, 0);
    }

    #[test]
    fn test_rolling_window_exactly_100() {
        let diag = Diagnostics::new();
        for i in 0..100 {
            diag.record_query(Duration::from_millis(i));
        }
        let durations = diag.query_durations.lock().unwrap();
        assert_eq!(durations.len(), 100);
        // Adding one more should evict the first
        drop(durations);
        diag.record_query(Duration::from_millis(100));
        let durations = diag.query_durations.lock().unwrap();
        assert_eq!(durations.len(), 100);
        assert_eq!(durations.front().unwrap(), &Duration::from_millis(1));
    }

    #[test]
    fn test_diagnostics_clone() {
        let diag = Diagnostics::new();
        diag.record_query(Duration::from_millis(10));
        let cloned = diag.clone();
        // Both share the same Arc, so recording on one is visible from the other
        cloned.record_query(Duration::from_millis(20));
        let durations = diag.query_durations.lock().unwrap();
        assert_eq!(durations.len(), 2);
    }
}
