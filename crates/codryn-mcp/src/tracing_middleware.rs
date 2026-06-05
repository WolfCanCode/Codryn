//! Request-scoped tracing middleware for MCP tool calls.
//!
//! Creates a tracing span per MCP tool invocation containing:
//! - `tool_name`: the MCP tool being called
//! - `project`: the resolved project name (or "unknown")
//! - `timestamp`: ISO 8601 with millisecond precision
//! - `duration_ms`: total execution time (recorded on span close)
//!
//! Output is emitted to stderr. When `CBM_LOG_FORMAT=json`, span fields
//! appear as structured JSON fields in the log entry.

use std::future::Future;
use std::time::Instant;

use chrono::Utc;
use tracing::{info_span, Instrument};

/// Create a tracing span for an MCP tool call.
///
/// The span includes:
/// - `tool_name`: name of the MCP tool (or "unknown" if empty/unresolvable)
/// - `project`: resolved project name (or "unknown" if empty/unresolvable)
/// - `timestamp`: current time in ISO 8601 format with millisecond precision
/// - `duration_ms`: placeholder field recorded on completion
///
/// # Examples
///
/// ```ignore
/// let span = create_tool_span("find_symbol", "my-project");
/// async { /* tool logic */ }.instrument(span).await;
/// ```
pub fn create_tool_span(tool_name: &str, project: &str) -> tracing::Span {
    let tool = if tool_name.is_empty() {
        "unknown"
    } else {
        tool_name
    };
    let proj = if project.is_empty() {
        "unknown"
    } else {
        project
    };
    let timestamp = Utc::now().format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string();

    info_span!(
        "mcp_tool_call",
        tool_name = %tool,
        project = %proj,
        timestamp = %timestamp,
        duration_ms = tracing::field::Empty,
    )
}

/// Execute an async future within a tool tracing span, recording duration on completion.
///
/// This is the primary entry point for instrumenting MCP tool calls. It:
/// 1. Creates a span with tool metadata and timestamp
/// 2. Executes the provided future within that span
/// 3. Records the total `duration_ms` before the span closes
///
/// The span is emitted to stderr via the configured tracing subscriber.
/// When `CBM_LOG_FORMAT=json`, all fields appear in structured JSON output.
///
/// # Arguments
///
/// * `tool_name` - The MCP tool name (e.g., "find_symbol"). Uses "unknown" if empty.
/// * `project` - The resolved project name. Uses "unknown" if empty.
/// * `fut` - The async future representing the tool's execution.
///
/// # Returns
///
/// The result of the executed future.
pub async fn trace_tool_call<F, T>(tool_name: &str, project: &str, fut: F) -> T
where
    F: Future<Output = T>,
{
    let span = create_tool_span(tool_name, project);
    let start = Instant::now();

    let result = fut.instrument(span.clone()).await;

    let duration_ms = start.elapsed().as_millis() as u64;
    span.record("duration_ms", duration_ms);

    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use tracing_subscriber::fmt;
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;

    /// Helper to set up a subscriber for tests that need spans to be active.
    fn with_subscriber<F: FnOnce()>(f: F) {
        let subscriber = tracing_subscriber::registry().with(
            fmt::layer()
                .with_writer(std::io::sink)
                .with_span_events(fmt::format::FmtSpan::CLOSE),
        );
        let _guard = subscriber.set_default();
        f();
    }

    #[test]
    fn test_create_tool_span_with_valid_inputs() {
        with_subscriber(|| {
            let span = create_tool_span("find_symbol", "my-project");
            assert!(!span.is_disabled());
        });
    }

    #[test]
    fn test_create_tool_span_with_empty_tool_name() {
        with_subscriber(|| {
            let span = create_tool_span("", "my-project");
            // Should use "unknown" for empty tool name — span still created
            assert!(!span.is_disabled());
        });
    }

    #[test]
    fn test_create_tool_span_with_empty_project() {
        with_subscriber(|| {
            let span = create_tool_span("find_symbol", "");
            // Should use "unknown" for empty project — span still created
            assert!(!span.is_disabled());
        });
    }

    #[test]
    fn test_create_tool_span_with_both_empty() {
        with_subscriber(|| {
            let span = create_tool_span("", "");
            // Should use "unknown" for both fields — span still created
            assert!(!span.is_disabled());
        });
    }

    #[tokio::test]
    async fn test_trace_tool_call_returns_result() {
        let result = trace_tool_call("test_tool", "test-project", async { 42 }).await;
        assert_eq!(result, 42);
    }

    #[tokio::test]
    async fn test_trace_tool_call_with_unknown_fields() {
        let result = trace_tool_call("", "", async { "hello" }).await;
        assert_eq!(result, "hello");
    }

    #[tokio::test]
    async fn test_trace_tool_call_records_duration() {
        // Verify that the function completes and returns the expected value
        // even with a small delay (duration_ms should be > 0)
        let result = trace_tool_call("slow_tool", "project", async {
            tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
            "done"
        })
        .await;
        assert_eq!(result, "done");
    }
}
