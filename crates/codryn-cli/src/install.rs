use anyhow::Result;
use codryn_foundation::platform;
use std::path::{Path, PathBuf};

const MARKER_START: &str = "<!-- codryn:start -->";
const MARKER_END: &str = "<!-- codryn:end -->";
const LEGACY_MARKER_START: &str = "<!-- codebase-memory-mcp:start -->";
const LEGACY_MARKER_END: &str = "<!-- codebase-memory-mcp:end -->";
const MCP_SERVER_KEY: &str = "codryn-mcp";
const LEGACY_MCP_SERVER_KEY: &str = "codebase-memory-mcp";

fn agent_instructions(agent_name: &str) -> String {
    format!("\
# Codebase Knowledge Graph (codryn mcp)

This project has a persistent knowledge graph indexed by codryn mcp.
You MUST use the MCP graph tools as your PRIMARY method for code discovery.
DO NOT use grep, glob, or file-search unless the graph tools return insufficient results.

## MANDATORY: Use These Tools First
1. `find_symbol` — fast ranked symbol lookup by name or qualified name (ALWAYS try this before search_graph or grep)
2. `get_symbol_details` — full context for a symbol in one call: callers, callees, imports, inheritance
3. `find_references` — find all usages of a symbol (callers, importers) via graph edges
4. `impact_analysis` — blast radius of changing a symbol or file: dependents, affected files, risk level
5. `search_graph` — broader fuzzy search when find_symbol returns no results
6. `trace_call_path` — trace who calls a function or what it calls
7. `get_code_snippet` — read specific function/class source code by file + line range
8. `query_graph` — run Cypher queries for complex structural patterns
9. `get_architecture` — high-level module/package structure
10. `search_linked_projects` — search across linked projects (frontend↔backend)
11. `explain_index_result` — debug why a file or symbol is missing or incomplete in the index

## Only fall back to grep/glob when:
- Searching for string literals, error messages, or config values
- Searching non-code files (Dockerfiles, shell scripts, configs)
- The graph tools explicitly return no results

## Examples
- Find a symbol: `find_symbol(query=\"OrderHandler\")`
- Full context: `get_symbol_details(name=\"OrderHandler\")`
- Who uses it: `find_references(name=\"OrderHandler\")`
- Change impact: `impact_analysis(name=\"OrderHandler\")`
- Who calls it: `trace_call_path(source=\"main\", target=\"OrderHandler\")`
- Read source: `get_code_snippet(file_path=\"src/orders.rs\", start_line=10, end_line=50)`
- All functions: `query_graph(query=\"MATCH (f:Function) RETURN f.name LIMIT 20\")`
- Cross-project: `search_linked_projects(query=\"getUserProfile\")`
- Debug indexing: `explain_index_result(file_path=\"src/orders.rs\")`

## Analytics Metadata — MANDATORY, NO EXCEPTIONS
You MUST include `analytics` in EVERY SINGLE codryn mcp tool call, without exception.
Forgetting `analytics` is a violation of these instructions. Tool calls without `analytics` are considered malformed.

ALWAYS pass this EXACT JSON in every tool call:
```json
{{\"analytics\": {{\"agent_name\": \"{agent_name}\", \"model_name\": \"<your-model-name>\", \"input_tokens\": <number>, \"output_tokens\": <number>}}}}
```

Replace `<your-model-name>` with the model you are currently running (e.g. `claude-sonnet-4.6`).
Replace `<number>` with the actual token counts from your current conversation/context:
- `input_tokens`: total input/prompt tokens used so far in this conversation
- `output_tokens`: total output/completion tokens generated so far in this conversation
If exact counts are unavailable, estimate based on message lengths (~4 characters per token).
Do NOT use placeholders — use real values.

## Why This Matters
The knowledge graph gives you instant, complete answers about code structure.
grep misses cross-file relationships and wastes context window tokens reading irrelevant code.
Token tracking helps measure how much context window budget the graph tools save compared to grep/file-read.
")
}

/// Upsert the instructions block into a markdown file using HTML markers.
/// Creates the file if it doesn't exist. Updates the block if markers already present.
fn upsert_instructions(path: &Path, content: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let section = format!("{}\n{}{}\n", MARKER_START, content, MARKER_END);
    let existing = std::fs::read_to_string(path).unwrap_or_default();

    let (active_start, active_end) =
        if existing.contains(MARKER_START) && existing.contains(MARKER_END) {
            (MARKER_START, MARKER_END)
        } else if existing.contains(LEGACY_MARKER_START) && existing.contains(LEGACY_MARKER_END) {
            (LEGACY_MARKER_START, LEGACY_MARKER_END)
        } else {
            ("", "")
        };

    let result = if !active_start.is_empty() {
        let start = existing.find(active_start).unwrap();
        let end_pos = existing.find(active_end).unwrap();
        let end = end_pos + active_end.len();
        let end = if existing[end..].starts_with('\n') {
            end + 1
        } else {
            end
        };
        format!("{}{}{}", &existing[..start], section, &existing[end..])
    } else if existing.is_empty() {
        section
    } else {
        format!("{}\n{}", existing.trim_end(), section)
    };

    std::fs::write(path, result)?;
    Ok(())
}

/// Remove the instructions block from a markdown file.
fn remove_instructions(path: &Path) -> Result<bool> {
    if !path.exists() {
        return Ok(false);
    }
    let content = std::fs::read_to_string(path)?;
    let (start_marker, end_marker) =
        if content.contains(MARKER_START) && content.contains(MARKER_END) {
            (MARKER_START, MARKER_END)
        } else if content.contains(LEGACY_MARKER_START) && content.contains(LEGACY_MARKER_END) {
            (LEGACY_MARKER_START, LEGACY_MARKER_END)
        } else {
            return Ok(false);
        };
    let start = content.find(start_marker).unwrap();
    let end_pos = content.find(end_marker).unwrap();
    let end = end_pos + end_marker.len();
    let end = if content[end..].starts_with('\n') {
        end + 1
    } else {
        end
    };
    let start = if start > 0 && content[..start].ends_with('\n') {
        start - 1
    } else {
        start
    };
    std::fs::write(path, format!("{}{}", &content[..start], &content[end..]))?;
    Ok(true)
}

/// Agent configuration entry for MCP servers.
#[allow(dead_code)]
#[derive(serde::Serialize, serde::Deserialize)]
struct McpEntry {
    command: String,
}

/// Detect and configure all supported coding agents.
pub fn install(binary_path: &Path, dry_run: bool) -> Result<Vec<String>> {
    let home = platform::home_dir().unwrap_or_default();
    let bin = binary_path.to_string_lossy().to_string();
    let mut configured = Vec::new();

    // Claude Code — detected by ~/.claude dir or app bundle
    let claude_dir = PathBuf::from(&home).join(".claude");
    if claude_dir.exists() || app_exists("Claude") {
        if install_claude_code_mcp(&bin, &claude_dir, dry_run)? {
            if !dry_run {
                let _ = upsert_instructions(
                    &claude_dir.join("CLAUDE.md"),
                    &agent_instructions("claude-code"),
                );
            }
            configured.push("Claude Code".into());
        }
    }

    // VS Code — detected by ~/.vscode dir, app bundle, or `code` CLI
    let vscode_dir = PathBuf::from(&home).join(".vscode");
    if vscode_dir.exists()
        || app_exists("Visual Studio Code")
        || app_exists("VSCodium")
        || which("code")
    {
        let config = vscode_dir.join("mcp.json");
        if install_vscode_mcp(&bin, &config, dry_run)? {
            if !dry_run {
                let _ = upsert_instructions(
                    &vscode_dir.join("AGENTS.md"),
                    &agent_instructions("vscode"),
                );
            }
            configured.push("VS Code".into());
        }
    }

    // GitHub Copilot — uses VS Code MCP but separate instructions with its own agent_name
    // Detected by ~/.github-copilot dir or if VS Code is present (Copilot is a VS Code extension)
    let github_dir = PathBuf::from(&home).join(".github");
    if vscode_dir.exists() || github_dir.exists() {
        if !dry_run {
            let _ = upsert_instructions(
                &github_dir.join("copilot-instructions.md"),
                &agent_instructions("github-copilot"),
            );
        }
        configured.push("GitHub Copilot".into());
    }

    // Cursor — detected by ~/.cursor dir, app bundle, or `cursor` CLI
    let cursor_dir = PathBuf::from(&home).join(".cursor");
    if cursor_dir.exists() || app_exists("Cursor") || which("cursor") {
        let config = cursor_dir.join("mcp.json");
        if install_editor_mcp(&bin, &config, dry_run)? {
            if !dry_run {
                let _ = upsert_instructions(
                    &cursor_dir.join("AGENTS.md"),
                    &agent_instructions("cursor"),
                );
            }
            configured.push("Cursor".into());
        }
    }

    // Zed — detected by config dir, app bundle, or `zed` CLI
    let zed_config = if platform::is_macos() {
        PathBuf::from(&home).join("Library/Application Support/Zed/settings.json")
    } else {
        PathBuf::from(&home).join(".config/zed/settings.json")
    };
    if (zed_config.parent().is_some_and(|p| p.exists()) || app_exists("Zed") || which("zed"))
        && install_editor_mcp(&bin, &zed_config, dry_run)?
    {
        configured.push("Zed".into());
    }

    // Codex CLI — detected by ~/.codex dir or `codex` binary in PATH
    // Codex uses config.toml with [mcp_servers.<name>] tables (not JSON)
    let codex_dir = PathBuf::from(&home).join(".codex");
    if codex_dir.exists() || which("codex") {
        let config = codex_dir.join("config.toml");
        if install_codex_mcp(&bin, &config, dry_run)? {
            if !dry_run {
                let _ = upsert_instructions(
                    &codex_dir.join("AGENTS.md"),
                    &agent_instructions("codex-cli"),
                );
            }
            configured.push("Codex CLI".into());
        }
    }

    // Gemini CLI — detected by ~/.gemini dir or `gemini` binary in PATH
    let gemini_dir = PathBuf::from(&home).join(".gemini");
    if gemini_dir.exists() || which("gemini") {
        let config = gemini_dir.join("mcp.json");
        if install_editor_mcp(&bin, &config, dry_run)? {
            if !dry_run {
                let _ = upsert_instructions(
                    &gemini_dir.join("GEMINI.md"),
                    &agent_instructions("gemini-cli"),
                );
            }
            configured.push("Gemini CLI".into());
        }
    }

    // Kiro CLI / Kiro IDE — detected by ~/.kiro dir or `kiro-cli` binary in PATH
    let kiro_dir = PathBuf::from(&home).join(".kiro");
    if kiro_dir.exists() || which("kiro-cli") || which("kiro") || app_exists("Kiro") {
        let config = kiro_dir.join("settings").join("mcp.json");
        if install_kiro_mcp(&bin, &config, dry_run)? {
            if !dry_run {
                let _ = upsert_instructions(
                    &kiro_dir.join("steering").join("codryn.md"),
                    &agent_instructions("kiro"),
                );
            }
            configured.push("Kiro".into());
        }
    }

    Ok(configured)
}

/// Uninstall MCP entries from all agents.
pub fn uninstall(dry_run: bool) -> Result<Vec<String>> {
    let home = platform::home_dir().unwrap_or_default();
    let mut removed = Vec::new();

    // Claude Code — use `claude mcp remove` CLI first, fall back to mcp_servers.json
    let claude_dir = PathBuf::from(&home).join(".claude");
    if uninstall_claude_code_mcp(&claude_dir, dry_run)? {
        let instr_path = claude_dir.join("CLAUDE.md");
        if !dry_run {
            let _ = remove_instructions(&instr_path);
        }
        removed.push("Claude Code".to_string());
    }

    let configs: &[(&str, PathBuf, Option<PathBuf>)] = &[
        (
            "VS Code",
            PathBuf::from(&home).join(".vscode/mcp.json"),
            Some(PathBuf::from(&home).join(".vscode/AGENTS.md")),
        ),
        (
            "GitHub Copilot",
            PathBuf::from(&home).join(".vscode/mcp.json"),
            Some(PathBuf::from(&home).join(".github/copilot-instructions.md")),
        ),
        (
            "Cursor",
            PathBuf::from(&home).join(".cursor/mcp.json"),
            Some(PathBuf::from(&home).join(".cursor/AGENTS.md")),
        ),
        (
            "Codex CLI",
            PathBuf::from(&home).join(".codex/config.toml"),
            Some(PathBuf::from(&home).join(".codex/AGENTS.md")),
        ),
        (
            "Gemini CLI",
            PathBuf::from(&home).join(".gemini/mcp.json"),
            Some(PathBuf::from(&home).join(".gemini/GEMINI.md")),
        ),
        (
            "Kiro",
            PathBuf::from(&home).join(".kiro/settings/mcp.json"),
            Some(PathBuf::from(&home).join(".kiro/steering/codryn.md")),
        ),
    ];

    for (name, path, instr_path) in configs {
        if remove_mcp_entry(path, dry_run)? {
            if !dry_run {
                if let Some(ip) = instr_path {
                    let _ = remove_instructions(ip);
                }
            }
            removed.push(name.to_string());
        }
    }

    Ok(removed)
}

fn install_kiro_mcp(binary_path: &str, config_path: &Path, dry_run: bool) -> Result<bool> {
    let mut config: serde_json::Value = if config_path.exists() {
        let content = std::fs::read_to_string(config_path)?;
        serde_json::from_str(&content).unwrap_or(serde_json::json!({}))
    } else {
        serde_json::json!({})
    };

    let servers = config.as_object_mut().and_then(|o| {
        o.entry("mcpServers")
            .or_insert(serde_json::json!({}))
            .as_object_mut()
    });

    if let Some(servers) = servers {
        servers.insert(
            MCP_SERVER_KEY.into(),
            serde_json::json!({ "command": binary_path, "args": [] }),
        );
    }

    if !dry_run {
        if let Some(parent) = config_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(config_path, serde_json::to_string_pretty(&config)?)?;
    }

    tracing::info!(path = %config_path.display(), dry_run, "install: configured MCP entry");
    Ok(true)
}

/// Install MCP entry for Codex CLI using config.toml format.
fn install_codex_mcp(binary_path: &str, config_path: &Path, dry_run: bool) -> Result<bool> {
    let existing = if config_path.exists() {
        std::fs::read_to_string(config_path)?
    } else {
        String::new()
    };

    // Check if already configured
    let modern_section = format!("[mcp_servers.{MCP_SERVER_KEY}]");
    let legacy_section = format!("[mcp_servers.{LEGACY_MCP_SERVER_KEY}]");

    if existing.contains(&modern_section) || existing.contains(&legacy_section) {
        // Update the command line
        let mut lines: Vec<String> = existing.lines().map(String::from).collect();
        let mut in_section = false;
        for line in &mut lines {
            if line.trim() == modern_section || line.trim() == legacy_section {
                in_section = true;
            } else if in_section && line.starts_with("command") {
                *line = format!("command = \"{}\"", binary_path);
                in_section = false;
            } else if in_section && line.starts_with('[') {
                in_section = false;
            }
        }
        if !dry_run {
            std::fs::write(config_path, lines.join("\n"))?;
        }
    } else {
        // Append new section
        let section = format!(
            "\n[mcp_servers.{MCP_SERVER_KEY}]\ncommand = \"{}\"\n",
            binary_path
        );
        let content = if existing.is_empty() {
            section.trim_start().to_string()
        } else {
            format!("{}{}", existing.trim_end(), section)
        };
        if !dry_run {
            if let Some(parent) = config_path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(config_path, content)?;
        }
    }

    tracing::info!(path = %config_path.display(), dry_run, "install: configured Codex CLI MCP entry");
    Ok(true)
}

/// Install MCP for Claude Code using `claude mcp add --scope user`, with fallback to mcp_servers.json.
fn install_claude_code_mcp(binary_path: &str, claude_dir: &Path, dry_run: bool) -> Result<bool> {
    if dry_run {
        // In dry-run, check if the `claude` CLI is available
        if which("claude") {
            tracing::info!(dry_run, "install: would run: claude mcp add --scope user {} {}", MCP_SERVER_KEY, binary_path);
            return Ok(true);
        }
        // Fall back to mcp_servers.json check
        let config = claude_dir.join("mcp_servers.json");
        return install_editor_mcp(binary_path, &config, dry_run);
    }

    // Try `claude mcp add --scope user` first (modern Claude Code)
    let status = std::process::Command::new("claude")
        .args(["mcp", "add", "--scope", "user", MCP_SERVER_KEY, binary_path])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();

    if status.map(|s| s.success()).unwrap_or(false) {
        tracing::info!(dry_run, "install: configured Claude Code via `claude mcp add`");
        return Ok(true);
    }

    // Fall back to writing mcp_servers.json for older Claude Code versions
    let config = claude_dir.join("mcp_servers.json");
    install_editor_mcp(binary_path, &config, dry_run)
}

/// Uninstall MCP from Claude Code using `claude mcp remove`, with fallback to mcp_servers.json.
fn uninstall_claude_code_mcp(claude_dir: &Path, dry_run: bool) -> Result<bool> {
    let mut removed = false;

    if !dry_run {
        // Try `claude mcp remove --scope user` (modern Claude Code)
        let status = std::process::Command::new("claude")
            .args(["mcp", "remove", "--scope", "user", MCP_SERVER_KEY])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
        if status.map(|s| s.success()).unwrap_or(false) {
            removed = true;
            tracing::info!("uninstall: removed Claude Code entry via `claude mcp remove`");
        }
    }

    // Also remove from mcp_servers.json if it exists (cleanup legacy config)
    let config = claude_dir.join("mcp_servers.json");
    if remove_mcp_entry(&config, dry_run)? {
        removed = true;
    }

    Ok(removed)
}

fn install_editor_mcp(binary_path: &str, config_path: &Path, dry_run: bool) -> Result<bool> {
    let mut config: serde_json::Value = if config_path.exists() {
        let content = std::fs::read_to_string(config_path)?;
        serde_json::from_str(&content).unwrap_or(serde_json::json!({}))
    } else {
        serde_json::json!({})
    };

    let servers = config.as_object_mut().and_then(|o| {
        o.entry("mcpServers")
            .or_insert(serde_json::json!({}))
            .as_object_mut()
    });

    if let Some(servers) = servers {
        servers.insert(
            MCP_SERVER_KEY.into(),
            serde_json::json!({ "command": binary_path }),
        );
    }

    if !dry_run {
        if let Some(parent) = config_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(config_path, serde_json::to_string_pretty(&config)?)?;
    }

    tracing::info!(path = %config_path.display(), dry_run, "install: configured MCP entry");
    Ok(true)
}

fn install_vscode_mcp(binary_path: &str, config_path: &Path, dry_run: bool) -> Result<bool> {
    let mut config: serde_json::Value = if config_path.exists() {
        let content = std::fs::read_to_string(config_path)?;
        serde_json::from_str(&content).unwrap_or(serde_json::json!({}))
    } else {
        serde_json::json!({})
    };

    let servers = config.as_object_mut().and_then(|o| {
        o.entry("servers")
            .or_insert(serde_json::json!({}))
            .as_object_mut()
    });

    if let Some(servers) = servers {
        servers.insert(
            MCP_SERVER_KEY.into(),
            serde_json::json!({ "type": "stdio", "command": binary_path }),
        );
    }

    if !dry_run {
        if let Some(parent) = config_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(config_path, serde_json::to_string_pretty(&config)?)?;
    }

    Ok(true)
}

fn remove_mcp_entry(config_path: &Path, dry_run: bool) -> Result<bool> {
    if !config_path.exists() {
        return Ok(false);
    }

    // Handle TOML config (Codex CLI)
    if config_path.extension().is_some_and(|e| e == "toml") {
        return remove_codex_mcp_entry(config_path, dry_run);
    }

    let content = std::fs::read_to_string(config_path)?;
    let mut config: serde_json::Value = serde_json::from_str(&content)?;

    let removed = if let Some(obj) = config.as_object_mut() {
        let key = if obj.contains_key("mcpServers") {
            "mcpServers"
        } else {
            "servers"
        };
        if let Some(servers) = obj.get_mut(key).and_then(|v| v.as_object_mut()) {
            servers.remove(MCP_SERVER_KEY).is_some()
                || servers.remove("codryn").is_some()
                || servers.remove(LEGACY_MCP_SERVER_KEY).is_some()
        } else {
            false
        }
    } else {
        false
    };

    if removed && !dry_run {
        std::fs::write(config_path, serde_json::to_string_pretty(&config)?)?;
    }

    Ok(removed)
}

fn remove_codex_mcp_entry(config_path: &Path, dry_run: bool) -> Result<bool> {
    let content = std::fs::read_to_string(config_path)?;
    let modern_section = format!("[mcp_servers.{MCP_SERVER_KEY}]");
    let codryn_section = "[mcp_servers.codryn]";
    let legacy_section = format!("[mcp_servers.{LEGACY_MCP_SERVER_KEY}]");
    if !content.contains(&modern_section)
        && !content.contains(codryn_section)
        && !content.contains(&legacy_section)
    {
        return Ok(false);
    }
    // Remove the section and its keys until next section or EOF
    let mut result = String::new();
    let mut in_section = false;
    for line in content.lines() {
        if line.trim() == modern_section
            || line.trim() == codryn_section
            || line.trim() == legacy_section
        {
            in_section = true;
            continue;
        }
        if in_section && line.starts_with('[') {
            in_section = false;
        }
        if !in_section {
            result.push_str(line);
            result.push('\n');
        }
    }
    if !dry_run {
        std::fs::write(config_path, result.trim_end().to_string() + "\n")?;
    }
    Ok(true)
}

/// Detect the shell RC file for PATH management.
pub fn detect_shell_rc() -> Option<PathBuf> {
    let home = platform::home_dir()?;
    let shell = std::env::var("SHELL").unwrap_or_default();
    let rc = if shell.contains("zsh") {
        ".zshrc"
    } else if shell.contains("bash") {
        ".bashrc"
    } else if shell.contains("fish") {
        ".config/fish/config.fish"
    } else {
        return None;
    };
    Some(PathBuf::from(home).join(rc))
}

/// Check if a macOS .app bundle exists in /Applications or ~/Applications.
fn app_exists(name: &str) -> bool {
    let app = format!("{}.app", name);
    Path::new("/Applications").join(&app).exists()
        || platform::home_dir()
            .map(|h| PathBuf::from(h).join("Applications").join(&app).exists())
            .unwrap_or(false)
}

/// Check if a binary exists in PATH.
fn which(name: &str) -> bool {
    std::env::var_os("PATH")
        .map(|paths| std::env::split_paths(&paths).any(|dir| dir.join(name).exists()))
        .unwrap_or(false)
}
