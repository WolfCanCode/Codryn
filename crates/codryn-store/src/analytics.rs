use crate::types::*;
use anyhow::Result;
use rusqlite::params;

impl crate::Store {
    #[allow(clippy::too_many_arguments)]
    pub fn log_tool_call(
        &self,
        tool_name: &str,
        project: &str,
        source: &str,
        duration_ms: i64,
        success: bool,
        agent_name: &str,
        model_name: &str,
        input_tokens: i64,
        output_tokens: i64,
        response_bytes: i64,
    ) -> Result<()> {
        let now = chrono::Utc::now().to_rfc3339();
        self.conn.execute(
            "INSERT INTO tool_calls (tool_name, project, source, duration_ms, success, called_at, agent_name, model_name, input_tokens, output_tokens, response_bytes) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![tool_name, project, source, duration_ms, if success { 1 } else { 0 }, now, agent_name, model_name, input_tokens, output_tokens, response_bytes],
        )?;
        Ok(())
    }

    pub fn get_tool_analytics(&self, limit: i32) -> Result<ToolAnalytics> {
        // Dashboard "ui" traffic is logged separately; headline totals and agent/model
        // breakdowns should reflect MCP (coding agent) invocations only.
        let total_calls: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM tool_calls WHERE source = 'mcp'",
            [],
            |r| r.get(0),
        )?;

        let mut per_tool = Vec::new();
        {
            let mut stmt = self.conn.prepare(
                "SELECT tool_name, COUNT(*) as cnt, AVG(duration_ms) as avg_ms, \
                 SUM(CASE WHEN source='mcp' THEN 1 ELSE 0 END) as mcp_count, \
                 SUM(CASE WHEN source='ui' THEN 1 ELSE 0 END) as ui_count \
                 FROM tool_calls GROUP BY tool_name ORDER BY cnt DESC",
            )?;
            let rows = stmt.query_map([], |r| {
                Ok(ToolCount {
                    tool_name: r.get(0)?,
                    count: r.get(1)?,
                    avg_ms: r.get(2)?,
                    mcp_count: r.get(3)?,
                    ui_count: r.get(4)?,
                })
            })?;
            for r in rows.flatten() {
                per_tool.push(r);
            }
        }

        let mut per_source = Vec::new();
        {
            let mut stmt = self.conn.prepare(
                "SELECT source, COUNT(*) FROM tool_calls WHERE source = 'mcp' GROUP BY source ORDER BY COUNT(*) DESC",
            )?;
            let rows = stmt.query_map([], |r| {
                Ok(SourceCount {
                    source: r.get(0)?,
                    count: r.get(1)?,
                })
            })?;
            for r in rows.flatten() {
                per_source.push(r);
            }
        }

        let mut per_agent = Vec::new();
        {
            let mut stmt = self.conn.prepare(
                "SELECT agent_name, COUNT(*) as count FROM tool_calls WHERE source = 'mcp' GROUP BY agent_name ORDER BY count DESC",
            )?;
            let rows = stmt.query_map([], |r| {
                Ok(AgentCount {
                    agent_name: r.get(0)?,
                    count: r.get(1)?,
                })
            })?;
            for r in rows.flatten() {
                per_agent.push(r);
            }
        }

        let mut per_model = Vec::new();
        {
            let mut stmt = self.conn.prepare(
                "SELECT model_name, COUNT(*) as count FROM tool_calls WHERE source = 'mcp' GROUP BY model_name ORDER BY count DESC",
            )?;
            let rows = stmt.query_map([], |r| {
                Ok(ModelCount {
                    model_name: r.get(0)?,
                    count: r.get(1)?,
                })
            })?;
            for r in rows.flatten() {
                per_model.push(r);
            }
        }

        let (total_input_tokens, total_output_tokens): (i64, i64) = self.conn.query_row(
            "SELECT COALESCE(SUM(input_tokens),0), COALESCE(SUM(output_tokens),0) FROM tool_calls WHERE source = 'mcp'",
            [], |r| Ok((r.get(0)?, r.get(1)?)),
        )?;

        let total_response_bytes: i64 = self.conn.query_row(
            "SELECT COALESCE(SUM(response_bytes),0) FROM tool_calls WHERE source = 'mcp'",
            [],
            |r| r.get(0),
        )?;

        let response_tokens = total_response_bytes / 4;
        let call_count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM tool_calls WHERE source = 'mcp'",
            [],
            |r| r.get(0),
        )?;
        let estimated_tokens_used = call_count * 50 + response_tokens;

        let mut without_tools: i64 = 0;
        {
            let mut stmt = self.conn.prepare("SELECT tool_name, COUNT(*) FROM tool_calls WHERE source = 'mcp' GROUP BY tool_name")?;
            let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?;
            for r in rows.flatten() {
                let (tool, count) = r;
                let cost_per_call: i64 = match tool.as_str() {
                    "search_graph" | "find_symbol" | "search_code" | "search_linked_projects" => {
                        15000
                    }
                    "find_references" | "impact_analysis" => 12000,
                    "trace_call_path" => 10000,
                    "get_symbol_details" => 8000,
                    "query_graph" => 6000,
                    "get_code_snippet" => 800,
                    "get_architecture" | "get_graph_schema" | "index_status" | "list_projects"
                    | "list_project_links" => 2000,
                    _ => 3000,
                };
                without_tools += count * cost_per_call;
            }
        }
        let estimated_tokens_without_tools = without_tools.max(estimated_tokens_used);
        let estimated_tokens_saved = estimated_tokens_without_tools - estimated_tokens_used;

        let mut recent = Vec::new();
        {
            let mut stmt = self.conn.prepare(
                "SELECT id, tool_name, project, source, duration_ms, success, called_at, \
                 agent_name, model_name, input_tokens, output_tokens, response_bytes \
                 FROM tool_calls ORDER BY id DESC LIMIT ?1",
            )?;
            let rows = stmt.query_map(params![limit], |r| {
                Ok(ToolCall {
                    id: r.get(0)?,
                    tool_name: r.get(1)?,
                    project: r.get(2)?,
                    source: r.get(3)?,
                    duration_ms: r.get(4)?,
                    success: r.get::<_, i32>(5)? == 1,
                    called_at: r.get(6)?,
                    agent_name: r.get(7)?,
                    model_name: r.get(8)?,
                    input_tokens: r.get(9)?,
                    output_tokens: r.get(10)?,
                    response_bytes: r.get(11)?,
                })
            })?;
            for r in rows.flatten() {
                recent.push(r);
            }
        }

        Ok(ToolAnalytics {
            total_calls,
            per_tool,
            per_source,
            per_agent,
            per_model,
            total_input_tokens,
            total_output_tokens,
            total_response_bytes,
            estimated_tokens_used,
            estimated_tokens_without_tools,
            estimated_tokens_saved,
            recent,
        })
    }
}
