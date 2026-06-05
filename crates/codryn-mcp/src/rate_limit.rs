use codryn_foundation::config::RateLimitConfig;
use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Tools that are exempt from rate limiting.
const EXEMPT_TOOLS: &[&str] = &["index_repository", "health_check"];

/// Error returned when a session exceeds the rate limit.
#[derive(Debug, Clone)]
pub struct RateLimitError {
    /// Number of seconds the client should wait before retrying.
    pub retry_after_seconds: u64,
}

impl std::fmt::Display for RateLimitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Rate limit exceeded. Retry after {} seconds.",
            self.retry_after_seconds
        )
    }
}

impl std::error::Error for RateLimitError {}

/// Sliding-window rate limiter for expensive queries.
///
/// Tracks per-session expensive query timestamps and rejects new queries
/// when the session exceeds `max_expensive` within the sliding `window`.
#[derive(Debug)]
pub struct RateLimiter {
    /// Sliding window duration.
    window: Duration,
    /// Maximum expensive queries allowed per window.
    max_expensive: usize,
    /// Threshold in milliseconds for a query to be considered expensive.
    expensive_threshold_ms: u64,
    /// Per-session tracking of expensive query timestamps.
    sessions: Mutex<HashMap<String, VecDeque<Instant>>>,
}

impl RateLimiter {
    /// Create a new rate limiter with the given parameters.
    pub fn new(window: Duration, max_expensive: usize, threshold_ms: u64) -> Self {
        Self {
            window,
            max_expensive,
            expensive_threshold_ms: threshold_ms,
            sessions: Mutex::new(HashMap::new()),
        }
    }

    /// Create a rate limiter from the application configuration.
    ///
    /// Uses defaults if config values are not specified:
    /// - window: 60 seconds
    /// - max_expensive: 10
    /// - threshold_ms: 500
    pub fn from_config(config: &RateLimitConfig) -> Self {
        let window_secs = config.window_secs.unwrap_or(60);
        let max_expensive = config.max_expensive.unwrap_or(10);
        let threshold_ms = config.threshold_ms.unwrap_or(500);
        Self::new(
            Duration::from_secs(window_secs),
            max_expensive,
            threshold_ms,
        )
    }

    /// Check whether a tool name is exempt from rate limiting.
    pub fn is_exempt(tool_name: &str) -> bool {
        EXEMPT_TOOLS.contains(&tool_name)
    }

    /// Record a query execution for the given session.
    ///
    /// If `duration_ms` is below the expensive threshold, the query is not
    /// counted and `Ok(())` is returned immediately.
    ///
    /// If the session has exceeded `max_expensive` queries within the window,
    /// returns `Err(RateLimitError)` with the number of seconds until the
    /// oldest entry expires.
    pub fn record(&self, session: &str, duration_ms: u64) -> Result<(), RateLimitError> {
        // Only track queries that exceed the expensive threshold
        if duration_ms < self.expensive_threshold_ms {
            return Ok(());
        }

        let now = Instant::now();
        let mut sessions = self.sessions.lock().unwrap();
        let entries = sessions.entry(session.to_owned()).or_default();

        // Expire old entries outside the window
        let cutoff = now - self.window;
        while let Some(&front) = entries.front() {
            if front < cutoff {
                entries.pop_front();
            } else {
                break;
            }
        }

        // Check if the session is over the limit
        if entries.len() >= self.max_expensive {
            // Calculate retry_after from the oldest entry in the window
            let oldest = entries.front().unwrap();
            let expires_at = *oldest + self.window;
            let retry_after = if expires_at > now {
                (expires_at - now).as_secs().max(1)
            } else {
                1
            };
            return Err(RateLimitError {
                retry_after_seconds: retry_after,
            });
        }

        // Record this expensive query
        entries.push_back(now);
        Ok(())
    }

    /// Check if a session is currently rate-limited without recording a new query.
    ///
    /// Returns `true` if the session has reached or exceeded the maximum
    /// number of expensive queries within the current window.
    pub fn is_limited(&self, session: &str) -> bool {
        let now = Instant::now();
        let mut sessions = self.sessions.lock().unwrap();

        let entries = match sessions.get_mut(session) {
            Some(e) => e,
            None => return false,
        };

        // Expire old entries outside the window
        let cutoff = now - self.window;
        while let Some(&front) = entries.front() {
            if front < cutoff {
                entries.pop_front();
            } else {
                break;
            }
        }

        entries.len() >= self.max_expensive
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;

    #[test]
    fn test_new_creates_limiter_with_correct_params() {
        let limiter = RateLimiter::new(Duration::from_secs(60), 10, 500);
        assert_eq!(limiter.window, Duration::from_secs(60));
        assert_eq!(limiter.max_expensive, 10);
        assert_eq!(limiter.expensive_threshold_ms, 500);
    }

    #[test]
    fn test_from_config_uses_provided_values() {
        let config = RateLimitConfig {
            window_secs: Some(120),
            max_expensive: Some(5),
            threshold_ms: Some(1000),
        };
        let limiter = RateLimiter::from_config(&config);
        assert_eq!(limiter.window, Duration::from_secs(120));
        assert_eq!(limiter.max_expensive, 5);
        assert_eq!(limiter.expensive_threshold_ms, 1000);
    }

    #[test]
    fn test_from_config_uses_defaults_for_none() {
        let config = RateLimitConfig {
            window_secs: None,
            max_expensive: None,
            threshold_ms: None,
        };
        let limiter = RateLimiter::from_config(&config);
        assert_eq!(limiter.window, Duration::from_secs(60));
        assert_eq!(limiter.max_expensive, 10);
        assert_eq!(limiter.expensive_threshold_ms, 500);
    }

    #[test]
    fn test_cheap_queries_not_tracked() {
        let limiter = RateLimiter::new(Duration::from_secs(60), 2, 500);
        // Queries below threshold should always succeed
        for _ in 0..100 {
            assert!(limiter.record("session1", 499).is_ok());
        }
        assert!(!limiter.is_limited("session1"));
    }

    #[test]
    fn test_expensive_queries_tracked_and_limited() {
        let limiter = RateLimiter::new(Duration::from_secs(60), 3, 500);

        // First 3 expensive queries should succeed
        assert!(limiter.record("session1", 600).is_ok());
        assert!(limiter.record("session1", 700).is_ok());
        assert!(limiter.record("session1", 800).is_ok());

        // 4th should fail
        let err = limiter.record("session1", 900).unwrap_err();
        assert!(err.retry_after_seconds > 0);
        assert!(err.retry_after_seconds <= 60);
    }

    #[test]
    fn test_is_limited_reflects_state() {
        let limiter = RateLimiter::new(Duration::from_secs(60), 2, 500);

        assert!(!limiter.is_limited("session1"));

        limiter.record("session1", 600).unwrap();
        assert!(!limiter.is_limited("session1"));

        limiter.record("session1", 700).unwrap();
        assert!(limiter.is_limited("session1"));
    }

    #[test]
    fn test_different_sessions_independent() {
        let limiter = RateLimiter::new(Duration::from_secs(60), 2, 500);

        limiter.record("session1", 600).unwrap();
        limiter.record("session1", 700).unwrap();

        // session1 is limited
        assert!(limiter.is_limited("session1"));

        // session2 is not
        assert!(!limiter.is_limited("session2"));
        assert!(limiter.record("session2", 600).is_ok());
    }

    #[test]
    fn test_sliding_window_expiration() {
        // Use a very short window for testing
        let limiter = RateLimiter::new(Duration::from_millis(50), 2, 100);

        limiter.record("s1", 200).unwrap();
        limiter.record("s1", 200).unwrap();
        assert!(limiter.is_limited("s1"));

        // Wait for the window to expire
        thread::sleep(Duration::from_millis(60));

        // Should no longer be limited
        assert!(!limiter.is_limited("s1"));
        assert!(limiter.record("s1", 200).is_ok());
    }

    #[test]
    fn test_exempt_tools() {
        assert!(RateLimiter::is_exempt("index_repository"));
        assert!(RateLimiter::is_exempt("health_check"));
        assert!(!RateLimiter::is_exempt("search_graph"));
        assert!(!RateLimiter::is_exempt("find_symbol"));
        assert!(!RateLimiter::is_exempt("query_graph"));
    }

    #[test]
    fn test_retry_after_calculation() {
        // Use a 1-second window with max 1 expensive query
        let limiter = RateLimiter::new(Duration::from_secs(1), 1, 100);

        limiter.record("s1", 200).unwrap();

        let err = limiter.record("s1", 200).unwrap_err();
        // retry_after should be approximately 1 second (the window duration)
        assert!(err.retry_after_seconds >= 1);
        assert!(err.retry_after_seconds <= 2);
    }

    #[test]
    fn test_record_at_exact_threshold() {
        let limiter = RateLimiter::new(Duration::from_secs(60), 2, 500);

        // Exactly at threshold should be counted as expensive
        assert!(limiter.record("s1", 500).is_ok());
        assert!(limiter.record("s1", 500).is_ok());
        assert!(limiter.record("s1", 500).is_err());
    }

    #[test]
    fn test_error_display() {
        let err = RateLimitError {
            retry_after_seconds: 42,
        };
        assert_eq!(
            err.to_string(),
            "Rate limit exceeded. Retry after 42 seconds."
        );
    }
}
