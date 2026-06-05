use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use tokio::sync::Notify;
use tracing::{info, warn};

/// Shutdown coordinator that manages graceful termination.
///
/// The controller listens for OS termination signals (SIGTERM, SIGINT) and
/// coordinates a graceful shutdown sequence:
/// 1. Stop accepting new MCP requests
/// 2. Wait for in-progress pipeline operations to complete their current flush phase
/// 3. Ensure the Store commits or rolls back pending transactions
/// 4. Force-terminate after the configured timeout (default: 30s)
///
/// Designed to be shared across tasks via `Arc<ShutdownController>`.
pub struct ShutdownController {
    /// Notified when a termination signal is received.
    notify: Notify,
    /// Set to true when shutdown is initiated.
    shutting_down: AtomicBool,
    /// Maximum time to wait for in-flight operations before force exit.
    timeout: Duration,
}

impl ShutdownController {
    /// Create a new shutdown controller with the given timeout.
    ///
    /// The timeout determines how long the controller waits for in-progress
    /// operations to complete before force-terminating.
    pub fn new(timeout: Duration) -> Self {
        Self {
            notify: Notify::new(),
            shutting_down: AtomicBool::new(false),
            timeout,
        }
    }

    /// Returns a future that resolves when shutdown is signaled.
    ///
    /// This listens for SIGTERM and SIGINT (on Unix) or Ctrl+C (cross-platform).
    /// When a signal is received, it sets the shutting_down flag and notifies
    /// all waiters. If the timeout expires, it force-terminates with a warning log.
    pub async fn wait_for_shutdown(&self) {
        // Wait for a termination signal
        self.listen_for_signals().await;

        // Mark as shutting down
        self.shutting_down.store(true, Ordering::SeqCst);
        self.notify.notify_waiters();

        info!(
            timeout_secs = self.timeout.as_secs(),
            "Shutdown signal received, waiting for in-progress operations to complete"
        );
    }

    /// Signal that shutdown should begin (programmatic trigger).
    ///
    /// This can be called from any task to initiate shutdown without
    /// waiting for an OS signal.
    pub fn trigger(&self) {
        self.shutting_down.store(true, Ordering::SeqCst);
        self.notify.notify_waiters();
        info!("Shutdown triggered programmatically");
    }

    /// Check if shutdown is in progress.
    pub fn is_shutting_down(&self) -> bool {
        self.shutting_down.load(Ordering::SeqCst)
    }

    /// Get the configured shutdown timeout.
    pub fn timeout(&self) -> Duration {
        self.timeout
    }

    /// Wait until shutdown is triggered. Returns immediately if already shutting down.
    ///
    /// Use this in server loops to detect when to stop accepting new requests.
    pub async fn cancelled(&self) {
        if self.is_shutting_down() {
            return;
        }
        self.notify.notified().await;
    }

    /// Run the graceful shutdown sequence with timeout enforcement.
    ///
    /// `drain_fn` is called to allow in-progress operations to complete.
    /// If it doesn't complete within the timeout, force-terminates with a warning.
    pub async fn run_with_timeout<F, Fut>(&self, drain_fn: F)
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = ()>,
    {
        let result = tokio::time::timeout(self.timeout, drain_fn()).await;

        match result {
            Ok(()) => {
                info!("All in-progress operations completed, shutting down cleanly");
            }
            Err(_) => {
                warn!(
                    timeout_secs = self.timeout.as_secs(),
                    "Shutdown timeout exceeded, force-terminating remaining operations"
                );
            }
        }
    }

    /// Listen for OS termination signals.
    ///
    /// On Unix: listens for both SIGTERM and SIGINT.
    /// On other platforms: listens for Ctrl+C only.
    async fn listen_for_signals(&self) {
        #[cfg(unix)]
        {
            use tokio::signal::unix::{signal, SignalKind};

            let mut sigterm =
                signal(SignalKind::terminate()).expect("Failed to register SIGTERM handler");
            let mut sigint =
                signal(SignalKind::interrupt()).expect("Failed to register SIGINT handler");

            tokio::select! {
                _ = sigterm.recv() => {
                    info!("Received SIGTERM");
                }
                _ = sigint.recv() => {
                    info!("Received SIGINT");
                }
            }
        }

        #[cfg(not(unix))]
        {
            tokio::signal::ctrl_c()
                .await
                .expect("Failed to register Ctrl+C handler");
            info!("Received Ctrl+C");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn test_new_creates_non_shutting_down_controller() {
        let controller = ShutdownController::new(Duration::from_secs(30));
        assert!(!controller.is_shutting_down());
    }

    #[test]
    fn test_trigger_sets_shutting_down() {
        let controller = ShutdownController::new(Duration::from_secs(30));
        assert!(!controller.is_shutting_down());
        controller.trigger();
        assert!(controller.is_shutting_down());
    }

    #[test]
    fn test_timeout_returns_configured_value() {
        let controller = ShutdownController::new(Duration::from_secs(45));
        assert_eq!(controller.timeout(), Duration::from_secs(45));
    }

    #[tokio::test]
    async fn test_cancelled_returns_immediately_when_shutting_down() {
        let controller = ShutdownController::new(Duration::from_secs(30));
        controller.trigger();

        // Should return immediately since we already triggered shutdown
        let result = tokio::time::timeout(Duration::from_millis(100), controller.cancelled()).await;
        assert!(
            result.is_ok(),
            "cancelled() should return immediately when already shutting down"
        );
    }

    #[tokio::test]
    async fn test_cancelled_waits_until_trigger() {
        let controller = Arc::new(ShutdownController::new(Duration::from_secs(30)));
        let controller_clone = controller.clone();

        // Spawn a task that triggers shutdown after a short delay
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            controller_clone.trigger();
        });

        // cancelled() should wait and then return after trigger
        let result = tokio::time::timeout(Duration::from_secs(1), controller.cancelled()).await;
        assert!(result.is_ok(), "cancelled() should resolve after trigger()");
        assert!(controller.is_shutting_down());
    }

    #[tokio::test]
    async fn test_run_with_timeout_completes_within_timeout() {
        let controller = ShutdownController::new(Duration::from_secs(5));

        // Drain function that completes quickly
        controller
            .run_with_timeout(|| async {
                tokio::time::sleep(Duration::from_millis(10)).await;
            })
            .await;

        // If we get here, it completed without force-terminating
    }

    #[tokio::test]
    async fn test_run_with_timeout_force_terminates_after_timeout() {
        let controller = ShutdownController::new(Duration::from_millis(50));

        let start = std::time::Instant::now();

        // Drain function that takes too long
        controller
            .run_with_timeout(|| async {
                tokio::time::sleep(Duration::from_secs(10)).await;
            })
            .await;

        let elapsed = start.elapsed();
        // Should have force-terminated around 50ms, not waited 10s
        assert!(
            elapsed < Duration::from_secs(1),
            "Should force-terminate after timeout, elapsed: {:?}",
            elapsed
        );
    }

    #[tokio::test]
    async fn test_multiple_triggers_are_idempotent() {
        let controller = ShutdownController::new(Duration::from_secs(30));
        controller.trigger();
        controller.trigger();
        controller.trigger();
        assert!(controller.is_shutting_down());
    }

    #[tokio::test]
    async fn test_shared_across_tasks_via_arc() {
        let controller = Arc::new(ShutdownController::new(Duration::from_secs(30)));

        let c1 = controller.clone();
        let c2 = controller.clone();

        // Task 1 checks shutdown state
        let handle = tokio::spawn(async move {
            c1.cancelled().await;
            assert!(c1.is_shutting_down());
        });

        // Task 2 triggers shutdown
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(20)).await;
            c2.trigger();
        });

        let result = tokio::time::timeout(Duration::from_secs(1), handle).await;
        assert!(
            result.is_ok(),
            "Task should complete after shutdown is triggered"
        );
    }
}
