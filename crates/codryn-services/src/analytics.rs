use std::path::Path;

pub struct AnalyticsService;

pub struct AnalyticsContext {
    pub agent_name: String,
    pub model_name: String,
    pub input_tokens: i64,
    pub output_tokens: i64,
}

impl AnalyticsService {
    /// Extract analytics context. Priority: primary → fallback → defaults.
    #[allow(clippy::too_many_arguments)]
    pub fn extract(
        agent_name: Option<&str>,
        model_name: Option<&str>,
        input_tokens: Option<i64>,
        output_tokens: Option<i64>,
        fb_agent: Option<&str>,
        fb_model: Option<&str>,
        fb_input: Option<i64>,
        fb_output: Option<i64>,
    ) -> AnalyticsContext {
        AnalyticsContext {
            agent_name: agent_name.or(fb_agent).unwrap_or("unknown").to_string(),
            model_name: model_name.or(fb_model).unwrap_or("unknown").to_string(),
            input_tokens: input_tokens.or(fb_input).unwrap_or(0),
            output_tokens: output_tokens.or(fb_output).unwrap_or(0),
        }
    }

    /// Estimate tokens from text: char_count / 4
    pub fn estimate_tokens(text: &str) -> i64 {
        text.len() as i64 / 4
    }

    /// Log a tool call. Opens store, logs, ignores errors silently.
    pub fn log_call(
        store_path: &Path,
        ctx: &AnalyticsContext,
        tool: &str,
        project: &str,
        duration_ms: i64,
        result: &str,
    ) {
        if store_path.to_string_lossy() == ":memory:" {
            return;
        }
        let response_bytes = result.len() as i64;
        let success = !result.contains("\"error\"");
        // Use estimated tokens if caller provided 0
        let input_tokens = if ctx.input_tokens > 0 {
            ctx.input_tokens
        } else {
            Self::estimate_tokens(tool) + 50 // rough estimate for request
        };
        let output_tokens = if ctx.output_tokens > 0 {
            ctx.output_tokens
        } else {
            Self::estimate_tokens(result)
        };
        if let Ok(s) = codryn_store::Store::open(&store_path.join("graph.db")) {
            let _ = s.log_tool_call(
                tool,
                project,
                "mcp",
                duration_ms,
                success,
                &ctx.agent_name,
                &ctx.model_name,
                input_tokens,
                output_tokens,
                response_bytes,
                "",
                result,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_estimate_tokens() {
        assert_eq!(AnalyticsService::estimate_tokens("hello world!"), 3); // 12 chars / 4
        assert_eq!(AnalyticsService::estimate_tokens(""), 0);
    }

    #[test]
    fn test_extract_primary_wins() {
        let ctx = AnalyticsService::extract(
            Some("kiro"),
            Some("claude"),
            Some(100),
            Some(200),
            Some("fallback"),
            Some("fb-model"),
            Some(1),
            Some(2),
        );
        assert_eq!(ctx.agent_name, "kiro");
        assert_eq!(ctx.model_name, "claude");
        assert_eq!(ctx.input_tokens, 100);
        assert_eq!(ctx.output_tokens, 200);
    }

    #[test]
    fn test_extract_fallback() {
        let ctx = AnalyticsService::extract(
            None,
            None,
            None,
            None,
            Some("fb-agent"),
            Some("fb-model"),
            Some(10),
            Some(20),
        );
        assert_eq!(ctx.agent_name, "fb-agent");
        assert_eq!(ctx.model_name, "fb-model");
        assert_eq!(ctx.input_tokens, 10);
        assert_eq!(ctx.output_tokens, 20);
    }

    #[test]
    fn test_extract_defaults() {
        let ctx = AnalyticsService::extract(None, None, None, None, None, None, None, None);
        assert_eq!(ctx.agent_name, "unknown");
        assert_eq!(ctx.model_name, "unknown");
        assert_eq!(ctx.input_tokens, 0);
        assert_eq!(ctx.output_tokens, 0);
    }
}
