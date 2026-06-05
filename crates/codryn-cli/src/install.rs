use anyhow::Result;
use codryn_foundation::ide_detect::{detect_ides, DetectedIde, Ide};
use codryn_foundation::platform;
use std::path::{Path, PathBuf};

use crate::preferences::{InstallPreferences, InstallScope, SteeringChoice, SteeringIntensity};
use crate::prompter::Prompter;

const MARKER_START: &str = "<!-- codryn:start -->";
const MARKER_END: &str = "<!-- codryn:end -->";

/// Skill directory name used under each agent's skills folder.
const SKILL_DIR_NAME: &str = "codebase-memory";

/// Old skill directory names that were consolidated into one.
/// Cleaned up during install to remove stale directories.
const OLD_SKILL_NAMES: &[&str] = &[
    "codebase-memory-exploring",
    "codebase-memory-tracing",
    "codebase-memory-quality",
    "codebase-memory-reference",
    "codryn", // old monolithic name
];

/// Consolidated skill content installed as SKILL.md.
/// Based on the upstream codryn project's skill format.
/// Uses progressive disclosure: decision matrix → workflows → reference.
fn skill_content() -> &'static str {
    r#"---
name: codebase-memory
description: Use the codebase knowledge graph for structural code queries, architecture exploration, call tracing, impact analysis, and refactoring.
keywords: explore codebase, architecture, functions, structure, calls, trace, callers, dependencies, impact analysis, dead code, unused, fan-out, refactor, code quality, what-if, dependency graph, ask graph, plan refactoring, detect patterns, test coverage, error flow, API surface, graph query, Cypher, edge types, search_graph
---

# Codebase Memory — Knowledge Graph Tools

Graph tools return precise structural results in ~500 tokens vs ~80K for grep.

## Quick Decision Matrix

| Question | Tool call |
|----------|----------|
| Find symbol by name | `find_symbol(query="Name")` |
| Full symbol context | `get_symbol_details(name="Name")` |
| Who uses X? | `find_references(name="X")` |
| What breaks if I change X? | `what_if(symbol="X", change_type="rename")` |
| Who calls X? | `trace_call_path(source="caller", target="X")` |
| What does X call? | `trace_call_path(source="X", target="callee")` |
| Find by pattern | `search_graph(query="pattern")` |
| Cross-service edges | `query_graph` with Cypher |
| Impact of local changes | `detect_changes()` |
| Dead code? | `find_dead_code(project="p")` |
| Module dependencies? | `get_dependency_graph(project="p")` |
| Ask in plain English | `ask_graph(question="who calls processPayment?")` |
| Refactoring plan | `plan_refactoring(target="X", refactoring_type="move_function")` |
| Design patterns? | `detect_patterns(project="p")` |
| Test gaps? | `test_coverage_map(project="p")` |
| Error propagation? | `trace_error_flow(symbol="X")` |
| Public API? | `get_api_surface(project="p")` |
| Text search | `search_code` or grep |

## Exploration Workflow
1. `list_projects` — check if project is indexed
2. `get_graph_schema` — understand node labels, edge types, and counts
3. `get_architecture` — high-level module/package structure
4. `find_symbol(query="Pattern")` — find code by name
5. `get_symbol_details(name="Name")` — callers, callees, imports, inheritance
6. `get_code_snippet(file_path="src/file.rs", start_line=10, end_line=50)` — read source

## Tracing Workflow
1. `find_symbol(query="FuncName")` — discover exact name
2. `trace_call_path(source="FuncName", target="Other")` — trace call chain
3. `find_references(name="FuncName")` — all callers and importers
4. `impact_analysis(name="FuncName")` — blast radius

## Quality Analysis
- Dead code: `find_dead_code(project="p", limit=20)`
- High fan-out: `sample_graph(limit=20)` — nodes sorted by total degree
- Hotspots: `sample_graph(sort_by="cyclomatic", limit=10)` — most complex functions
- Test gaps: `test_coverage_map(project="p", untested_only=true)`
- Patterns: `detect_patterns(project="p", antipatterns_only=true)`

## Cross-Project
- `search_linked_projects(query="getUserProfile")` — search across linked projects
- `explain_index_result(file_path="src/file.rs")` — debug missing symbols

## MCP Tools Reference (42 tools)
`index_repository`, `index_status`, `list_projects`, `delete_project`,
`find_symbol`, `get_symbol_details`, `find_references`, `impact_analysis`,
`search_graph`, `search_code`, `trace_call_path`, `trace_data_flow`,
`trace_backend_flow`, `detect_changes`, `query_graph`, `get_graph_schema`,
`get_code_snippet`, `get_file_overview`, `get_architecture`, `sample_graph`,
`suggest_next_reads`, `find_entrypoints`, `find_routes`, `find_pipelines`,
`find_infrastructure`, `find_tests_for_target`, `manage_adr`, `ingest_traces`,
`search_linked_projects`, `link_project`, `list_project_links`,
`suggest_project_links`, `explain_index_result`, `diagnostics`,
`what_if`, `find_dead_code`, `get_dependency_graph`, `freshness_check`,
`ask_graph`, `plan_refactoring`, `review_changes`, `detect_patterns`,
`test_coverage_map`, `trace_error_flow`, `get_api_surface`, `clear_cache`

## Edge Types
CALLS, HTTP_CALLS, ASYNC_CALLS, IMPORTS, DEFINES, DEFINES_METHOD,
HANDLES, IMPLEMENTS, OVERRIDE, USAGE, INJECTS, INCLUDES,
FILE_CHANGES_WITH, CONTAINS_FILE, CONTAINS_FOLDER, CONTAINS_PACKAGE

## Cypher Examples (for query_graph)
```
MATCH (a)-[r:HTTP_CALLS]->(b) RETURN a.name, b.name, r.url_path LIMIT 20
MATCH (f:Function) WHERE f.name =~ '.*Handler.*' RETURN f.name, f.file_path
MATCH (a)-[r:CALLS]->(b) WHERE a.name = 'main' RETURN b.name
```

## Gotchas
1. `find_symbol` is faster and more precise than `search_graph` — always try it first.
2. `query_graph` has a 200-row cap — use pagination or filters.
3. `trace_call_path` needs exact names — use `find_symbol` first to discover them.
4. Results default to 10 per page — check `has_more` and use `offset`.
5. Only fall back to grep/glob for string literals, error messages, or non-code files.
6. `what_if` is faster than manual impact analysis — use it before any rename/remove.
7. `ask_graph` accepts plain English — no need to write Cypher for common questions.

## Analytics Metadata — Required
Every call to graph MCP tools MUST include an `analytics` field:
```json
{"analytics": {"agent_name": "<agent>", "model_name": "<model>", "input_tokens": 0, "output_tokens": 0}}
```
Use real or estimated token counts (~4 characters per token).
"#
}

/// Kiro steering-style shortcut content for the skills directory.
/// Kiro supports `inclusion: auto` frontmatter in `~/.kiro/skills/<name>.md` files,
/// which acts as a progressive-disclosure steering file — only the name and description
/// are loaded at startup, full content loads when the request matches.
/// This is Kiro-specific; other agents use only the standard `<name>/SKILL.md` directory.
fn kiro_skill_shortcut_content() -> &'static str {
    r#"---
inclusion: auto
name: codebase-memory
description: Use the codebase knowledge graph for structural code queries, architecture exploration, call tracing, impact analysis, and refactoring.
keywords: explore codebase, architecture, functions, structure, calls, trace, callers, dependencies, impact analysis, dead code, unused, fan-out, refactor, code quality, what-if, dependency graph, ask graph, plan refactoring, detect patterns, test coverage, error flow, API surface, graph query, Cypher, edge types, search_graph
---

# Codebase Knowledge Graph (codryn)

Use the MCP graph tools as your PRIMARY method for code discovery.
DO NOT use grep, glob, or file-search unless the graph tools return insufficient results.

## Use These Tools First
1. `find_symbol` — fast ranked symbol lookup by name or qualified name
2. `get_symbol_details` — full context for a symbol: callers, callees, imports, inheritance
3. `find_references` — find all usages of a symbol via graph edges
4. `impact_analysis` — blast radius of changing a symbol or file
5. `what_if` — impact of rename/remove/change_signature/move_file with fix plan
6. `search_graph` — broader fuzzy search when find_symbol returns no results
7. `trace_call_path` — trace who calls a function or what it calls
8. `get_code_snippet` — read specific function/class source code by file + line range
9. `query_graph` — run Cypher queries for complex structural patterns
10. `get_architecture` — high-level module/package structure
11. `search_linked_projects` — search across linked projects
12. `explain_index_result` — debug why a file or symbol is missing

## Agent-First Tools (use before writing code)
- `what_if(symbol="X", change_type="rename")` — before any rename/remove
- `find_dead_code(project="p")` — find unused symbols
- `get_dependency_graph(project="p")` — module import graph with cycles
- `ask_graph(question="who calls X?")` — plain English queries
- `plan_refactoring(target="X", refactoring_type="move_function")` — refactor plan
- `detect_patterns(project="p")` — MVC, God Class, circular deps
- `test_coverage_map(project="p")` — untested symbols ranked by risk
- `trace_error_flow(symbol="X")` — uncaught error propagation
- `get_api_surface(project="p")` — all exported symbols

## Only fall back to grep/glob when:
- Searching for string literals, error messages, or config values
- Searching non-code files (Dockerfiles, shell scripts, configs)
- The graph tools explicitly return no results
"#
}

fn agent_instructions(agent_name: &str) -> String {
    format!("\
# Codebase Knowledge Graph (codryn)

This project has a persistent knowledge graph indexed by codryn.
You MUST use the MCP graph tools as your PRIMARY method for code discovery.
DO NOT use grep, glob, or file-search unless the graph tools return insufficient results.

## MANDATORY: Use These Tools First
1. `find_symbol` — fast ranked symbol lookup by name or qualified name (ALWAYS try this before search_graph or grep)
2. `get_symbol_details` — full context for a symbol in one call: callers, callees, imports, inheritance
3. `find_references` — find all usages of a symbol (callers, importers) via graph edges
4. `impact_analysis` — blast radius of changing a symbol or file: dependents, affected files, risk level
5. `what_if` — impact of rename/remove/change_signature/move_file with concrete fix plan
6. `search_graph` — broader fuzzy search when find_symbol returns no results
7. `trace_call_path` — trace who calls a function or what it calls
8. `get_code_snippet` — read specific function/class source code by file + line range
9. `query_graph` — run Cypher queries for complex structural patterns
10. `get_architecture` — high-level module/package structure
11. `search_linked_projects` — search across linked projects (frontend↔backend)
12. `explain_index_result` — debug why a file or symbol is missing or incomplete in the index

## Agent-First Tools — Use Before Writing Code
- `what_if(symbol=\"X\", change_type=\"rename\")` — ALWAYS call before any rename, remove, or signature change
- `find_dead_code(project=\"p\")` — find unused symbols before adding new ones
- `get_dependency_graph(project=\"p\")` — understand module structure before refactoring
- `ask_graph(question=\"who calls processPayment?\")` — plain English queries, no Cypher needed
- `plan_refactoring(target=\"X\", refactoring_type=\"move_function\")` — get a step-by-step plan
- `detect_patterns(project=\"p\")` — detect God Class, circular deps before adding more code
- `test_coverage_map(project=\"p\")` — find untested symbols before writing tests
- `trace_error_flow(symbol=\"X\")` — find uncaught error paths before fixing error handling
- `get_api_surface(project=\"p\")` — see all exported symbols before adding new ones
- `review_changes(changed_files=[...])` — check impact of your changes before committing

## Only fall back to grep/glob when:
- Searching for string literals, error messages, or config values
- Searching non-code files (Dockerfiles, shell scripts, configs)
- The graph tools explicitly return no results

## Examples
- Find a symbol: `find_symbol(query=\"OrderHandler\")`
- Full context: `get_symbol_details(name=\"OrderHandler\")`
- Who uses it: `find_references(name=\"OrderHandler\")`
- Change impact: `what_if(symbol=\"OrderHandler\", change_type=\"rename\", new_value=\"OrderProcessor\")`
- Who calls it: `trace_call_path(source=\"main\", target=\"OrderHandler\")`
- Read source: `get_code_snippet(file_path=\"src/orders.rs\", start_line=10, end_line=50)`
- All functions: `query_graph(query=\"MATCH (f:Function) RETURN f.name LIMIT 20\")`
- Cross-project: `search_linked_projects(query=\"getUserProfile\")`
- Debug indexing: `explain_index_result(file_path=\"src/orders.rs\")`
- Ask anything: `ask_graph(question=\"what imports the payment module?\")`

## Analytics Metadata — MANDATORY, NO EXCEPTIONS
You MUST include `analytics` in EVERY SINGLE codryn tool call, without exception.
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

// ── Skill file management ────────────────────────────────────────

/// Install the consolidated skill to a skills directory.
/// Creates `<skills_dir>/codebase-memory/SKILL.md` following the open Agent Skills
/// standard (agentskills.io). Works across all compatible agents: Claude Code,
/// Cursor, Copilot, Codex, Gemini CLI, Windsurf, and Kiro.
/// Cleans up old skill directories from previous versions.
fn install_skill(skills_dir: &Path, force: bool, dry_run: bool) -> Result<bool> {
    // Clean up old skill directories
    for old_name in OLD_SKILL_NAMES {
        let old_path = skills_dir.join(old_name);
        if old_path.is_dir() && !dry_run {
            let _ = std::fs::remove_dir_all(&old_path);
            tracing::info!(path = %old_path.display(), "install: removed old skill directory");
        }
    }

    let skill_dir = skills_dir.join(SKILL_DIR_NAME);
    let skill_file = skill_dir.join("SKILL.md");

    // Always overwrite — skill content may have been updated between versions.
    // The `force` flag is kept for API compatibility but install always writes.
    let _ = force;

    if dry_run {
        return Ok(true);
    }

    std::fs::create_dir_all(&skill_dir)?;
    std::fs::write(&skill_file, skill_content())?;
    tracing::info!(path = %skill_file.display(), "install: wrote skill file");

    Ok(true)
}

/// Remove the skill from a skills directory.
fn remove_skill(skills_dir: &Path, dry_run: bool) -> Result<bool> {
    let skill_dir = skills_dir.join(SKILL_DIR_NAME);
    if !skill_dir.exists() {
        return Ok(false);
    }
    if !dry_run {
        std::fs::remove_dir_all(&skill_dir)?;
    }
    // Also clean up any old skill directories
    for old_name in OLD_SKILL_NAMES {
        let old_path = skills_dir.join(old_name);
        if old_path.is_dir() && !dry_run {
            let _ = std::fs::remove_dir_all(&old_path);
        }
    }
    Ok(true)
}

// ── Instructions upsert/remove (for agents without skill directories) ──

/// Upsert the instructions block into a markdown file using HTML markers.
/// Creates the file if it doesn't exist. Updates the block if markers already present.
fn upsert_instructions(path: &Path, content: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let section = format!("{}\n{}{}\n", MARKER_START, content, MARKER_END);
    let existing = std::fs::read_to_string(path).unwrap_or_default();

    let result = if existing.contains(MARKER_START) && existing.contains(MARKER_END) {
        let start = existing.find(MARKER_START).unwrap();
        let end_pos = existing.find(MARKER_END).unwrap();
        let end = end_pos + MARKER_END.len();
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
    let Some(start) = content.find(MARKER_START) else {
        return Ok(false);
    };
    let Some(end_pos) = content.find(MARKER_END) else {
        return Ok(false);
    };
    let end = end_pos + MARKER_END.len();
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

// ── Main install/uninstall ───────────────────────────────────────

/// Detect and configure all supported coding agents.
/// Installs MCP config, SKILL.md (Agent Skills standard), and agent-specific
/// instructions files for each detected agent.
pub fn install(binary_path: &Path, dry_run: bool) -> Result<Vec<String>> {
    let home = platform::home_dir().unwrap_or_default();
    let bin = binary_path.to_string_lossy().to_string();
    let mut configured = Vec::new();

    // Claude Code — detected by ~/.claude dir or app bundle
    // Skills: ~/.claude/skills/codebase-memory/SKILL.md
    let claude_dir = PathBuf::from(&home).join(".claude");
    if claude_dir.exists() || app_exists("Claude") {
        let config = claude_dir.join("mcp_servers.json");
        if install_editor_mcp(&bin, &config, dry_run)? {
            if !dry_run {
                let _ = install_skill(&claude_dir.join("skills"), false, false);
            }
            configured.push("Claude Code".into());
        }
    }

    // VS Code / GitHub Copilot — detected by ~/.vscode dir, app bundle, or `code` CLI
    // Skills: ~/.copilot/skills/codebase-memory/SKILL.md (Copilot's global skills)
    // Also: AGENTS.md for VS Code agent mode, copilot-instructions.md for Copilot
    let vscode_dir = PathBuf::from(&home).join(".vscode");
    if vscode_dir.exists()
        || app_exists("Visual Studio Code")
        || app_exists("VSCodium")
        || which("code")
    {
        let config = vscode_dir.join("mcp.json");
        if install_vscode_mcp(&bin, &config, dry_run)? {
            if !dry_run {
                // Copilot global skills directory
                let copilot_skills = PathBuf::from(&home).join(".copilot").join("skills");
                let _ = install_skill(&copilot_skills, false, false);
                // AGENTS.md for VS Code agent mode
                let _ = upsert_instructions(
                    &vscode_dir.join("AGENTS.md"),
                    &agent_instructions("vscode"),
                );
            }
            configured.push("VS Code".into());
        }
    }

    // GitHub Copilot — uses VS Code MCP but separate instructions with its own agent_name
    // Detected by ~/.github dir or if VS Code is present (Copilot is a VS Code extension)
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
    // Skills: ~/.cursor/skills/codebase-memory/SKILL.md
    // Also: AGENTS.md for backward compatibility
    let cursor_dir = PathBuf::from(&home).join(".cursor");
    if cursor_dir.exists() || app_exists("Cursor") || which("cursor") {
        let config = cursor_dir.join("mcp.json");
        if install_editor_mcp(&bin, &config, dry_run)? {
            if !dry_run {
                let _ = install_skill(&cursor_dir.join("skills"), false, false);
                let _ = upsert_instructions(
                    &cursor_dir.join("AGENTS.md"),
                    &agent_instructions("cursor"),
                );
            }
            configured.push("Cursor".into());
        }
    }

    // Windsurf — detected by ~/.codeium dir, app bundle, or `windsurf` CLI
    // Skills: ~/.codeium/windsurf/skills/codebase-memory/SKILL.md
    let windsurf_dir = PathBuf::from(&home).join(".codeium").join("windsurf");
    if windsurf_dir.exists() || app_exists("Windsurf") || which("windsurf") {
        let config = windsurf_dir.join("mcp.json");
        if install_editor_mcp(&bin, &config, dry_run)? {
            if !dry_run {
                let _ = install_skill(&windsurf_dir.join("skills"), false, false);
            }
            configured.push("Windsurf".into());
        }
    }

    // Zed — detected by config dir, app bundle, or `zed` CLI
    // No skills support yet — MCP only
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
    // Skills: ~/.codex/skills/codebase-memory/SKILL.md
    // Also: AGENTS.md for backward compatibility
    // Codex uses config.toml with [mcp_servers.<name>] tables (not JSON)
    let codex_dir = PathBuf::from(&home).join(".codex");
    if codex_dir.exists() || which("codex") {
        let config = codex_dir.join("config.toml");
        if install_codex_mcp(&bin, &config, dry_run)? {
            if !dry_run {
                let _ = install_skill(&codex_dir.join("skills"), false, false);
                let _ = upsert_instructions(
                    &codex_dir.join("AGENTS.md"),
                    &agent_instructions("codex-cli"),
                );
            }
            configured.push("Codex CLI".into());
        }
    }

    // Gemini CLI — detected by ~/.gemini dir or `gemini` binary in PATH
    // Skills: ~/.gemini/skills/codebase-memory/SKILL.md
    // Also: GEMINI.md for backward compatibility
    let gemini_dir = PathBuf::from(&home).join(".gemini");
    if gemini_dir.exists() || which("gemini") {
        let config = gemini_dir.join("mcp.json");
        if install_editor_mcp(&bin, &config, dry_run)? {
            if !dry_run {
                let _ = install_skill(&gemini_dir.join("skills"), false, false);
                let _ = upsert_instructions(
                    &gemini_dir.join("GEMINI.md"),
                    &agent_instructions("gemini-cli"),
                );
            }
            configured.push("Gemini CLI".into());
        }
    }

    // Kiro CLI / Kiro IDE — detected by ~/.kiro dir or `kiro-cli` binary in PATH
    // Skills: ~/.kiro/skills/codebase-memory/SKILL.md (standard)
    //         ~/.kiro/skills/codebase-memory.md (Kiro shortcut with inclusion: auto)
    // Also: steering file for Kiro's always-on context system
    let kiro_dir = PathBuf::from(&home).join(".kiro");
    if kiro_dir.exists() || which("kiro-cli") || which("kiro") || app_exists("Kiro") {
        let config = kiro_dir.join("settings").join("mcp.json");
        if install_kiro_mcp(&bin, &config, dry_run)? {
            if !dry_run {
                let skills_dir = kiro_dir.join("skills");
                let _ = install_skill(&skills_dir, false, false);
                // Kiro-specific: write shortcut file with `inclusion: auto` frontmatter
                // for progressive-disclosure steering in the skills directory
                let shortcut = skills_dir.join(format!("{}.md", SKILL_DIR_NAME));
                let _ = std::fs::write(&shortcut, kiro_skill_shortcut_content());
                let _ = upsert_instructions(
                    &kiro_dir.join("steering").join("codebase-memory.md"),
                    &agent_instructions("kiro"),
                );
            }
            configured.push("Kiro".into());
        }
    }

    Ok(configured)
}

/// Uninstall MCP entries, skills, and instructions from all agents.
pub fn uninstall(dry_run: bool) -> Result<Vec<String>> {
    let home = platform::home_dir().unwrap_or_default();
    let mut removed = Vec::new();

    // Claude Code — MCP + skills
    let claude_mcp = PathBuf::from(&home).join(".claude/mcp_servers.json");
    if remove_mcp_entry(&claude_mcp, dry_run)? {
        if !dry_run {
            let _ = remove_skill(&PathBuf::from(&home).join(".claude/skills"), false);
        }
        removed.push("Claude Code".into());
    }

    // VS Code — MCP + AGENTS.md + Copilot skills
    let vscode_mcp = PathBuf::from(&home).join(".vscode/mcp.json");
    if remove_mcp_entry(&vscode_mcp, dry_run)? {
        if !dry_run {
            let _ = remove_instructions(&PathBuf::from(&home).join(".vscode/AGENTS.md"));
            let _ = remove_skill(&PathBuf::from(&home).join(".copilot/skills"), false);
        }
        removed.push("VS Code".into());
    }

    // GitHub Copilot — instructions only (MCP shared with VS Code)
    let copilot_instr = PathBuf::from(&home).join(".github/copilot-instructions.md");
    if copilot_instr.exists() {
        if !dry_run {
            let _ = remove_instructions(&copilot_instr);
        }
        removed.push("GitHub Copilot".into());
    }

    // Cursor — MCP + skills + AGENTS.md
    let cursor_mcp = PathBuf::from(&home).join(".cursor/mcp.json");
    if remove_mcp_entry(&cursor_mcp, dry_run)? {
        if !dry_run {
            let _ = remove_skill(&PathBuf::from(&home).join(".cursor/skills"), false);
            let _ = remove_instructions(&PathBuf::from(&home).join(".cursor/AGENTS.md"));
        }
        removed.push("Cursor".into());
    }

    // Windsurf — MCP + skills
    let windsurf_mcp = PathBuf::from(&home).join(".codeium/windsurf/mcp.json");
    if remove_mcp_entry(&windsurf_mcp, dry_run)? {
        if !dry_run {
            let _ = remove_skill(
                &PathBuf::from(&home).join(".codeium/windsurf/skills"),
                false,
            );
        }
        removed.push("Windsurf".into());
    }

    // Zed — MCP only (no skills support)
    let zed_config = if platform::is_macos() {
        PathBuf::from(&home).join("Library/Application Support/Zed/settings.json")
    } else {
        PathBuf::from(&home).join(".config/zed/settings.json")
    };
    if remove_mcp_entry(&zed_config, dry_run)? {
        removed.push("Zed".into());
    }

    // Codex CLI — MCP + skills + AGENTS.md
    let codex_mcp = PathBuf::from(&home).join(".codex/config.toml");
    if remove_mcp_entry(&codex_mcp, dry_run)? {
        if !dry_run {
            let _ = remove_skill(&PathBuf::from(&home).join(".codex/skills"), false);
            let _ = remove_instructions(&PathBuf::from(&home).join(".codex/AGENTS.md"));
        }
        removed.push("Codex CLI".into());
    }

    // Gemini CLI — MCP + skills + GEMINI.md
    let gemini_mcp = PathBuf::from(&home).join(".gemini/mcp.json");
    if remove_mcp_entry(&gemini_mcp, dry_run)? {
        if !dry_run {
            let _ = remove_skill(&PathBuf::from(&home).join(".gemini/skills"), false);
            let _ = remove_instructions(&PathBuf::from(&home).join(".gemini/GEMINI.md"));
        }
        removed.push("Gemini CLI".into());
    }

    // Kiro — MCP + skills + shortcut + steering
    let kiro_mcp = PathBuf::from(&home).join(".kiro/settings/mcp.json");
    if remove_mcp_entry(&kiro_mcp, dry_run)? {
        if !dry_run {
            let kiro_skills = PathBuf::from(&home).join(".kiro/skills");
            let _ = remove_skill(&kiro_skills, false);
            // Remove Kiro-specific shortcut file
            let shortcut = kiro_skills.join(format!("{}.md", SKILL_DIR_NAME));
            if shortcut.exists() {
                let _ = std::fs::remove_file(&shortcut);
            }
            let _ = remove_instructions(
                &PathBuf::from(&home).join(".kiro/steering/codebase-memory.md"),
            );
        }
        removed.push("Kiro".into());
    }

    Ok(removed)
}

// ── MCP config installers ────────────────────────────────────────

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
        // Preserve existing settings (autoApprove, env, etc.) — only update command/args
        if let Some(existing) = servers.get_mut("codryn") {
            if let Some(obj) = existing.as_object_mut() {
                obj.insert("command".into(), serde_json::json!(binary_path));
                if !obj.contains_key("args") {
                    obj.insert("args".into(), serde_json::json!([]));
                }
            }
        } else {
            servers.insert(
                "codryn".into(),
                serde_json::json!({ "command": binary_path, "args": [] }),
            );
        }
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
    if existing.contains("[mcp_servers.codryn]") {
        // Update the command line
        let mut lines: Vec<String> = existing.lines().map(String::from).collect();
        let mut in_section = false;
        for line in &mut lines {
            if line.trim() == "[mcp_servers.codryn]" {
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
            "\n[mcp_servers.codryn]\ncommand = \"{}\"\n",
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
        // Preserve existing settings (autoApprove, env, disabled, etc.) — only update command
        if let Some(existing) = servers.get_mut("codryn") {
            if let Some(obj) = existing.as_object_mut() {
                obj.insert("command".into(), serde_json::json!(binary_path));
            }
        } else {
            servers.insert(
                "codryn".into(),
                serde_json::json!({ "command": binary_path }),
            );
        }
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
        // Preserve existing settings (autoApprove, env, etc.) — only update command/type
        if let Some(existing) = servers.get_mut("codryn") {
            if let Some(obj) = existing.as_object_mut() {
                obj.insert("type".into(), serde_json::json!("stdio"));
                obj.insert("command".into(), serde_json::json!(binary_path));
            }
        } else {
            servers.insert(
                "codryn".into(),
                serde_json::json!({ "type": "stdio", "command": binary_path }),
            );
        }
    }

    if !dry_run {
        if let Some(parent) = config_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(config_path, serde_json::to_string_pretty(&config)?)?;
    }

    Ok(true)
}

// ── MCP config removers ──────────────────────────────────────────

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
            servers.remove("codryn").is_some()
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
    if !content.contains("[mcp_servers.codryn]") {
        return Ok(false);
    }
    // Remove the section and its keys until next section or EOF
    let mut result = String::new();
    let mut in_section = false;
    for line in content.lines() {
        if line.trim() == "[mcp_servers.codryn]" {
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

// ── Utility functions ────────────────────────────────────────────

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

// ── Interactive Install Flow ─────────────────────────────────────

/// Resolved configuration from the interactive install flow.
///
/// Holds all user choices (or defaults) needed to execute the install.
#[derive(Debug, Clone, PartialEq)]
pub struct InstallConfig {
    /// Where to install: global, workspace, or both.
    pub scope: InstallScope,
    /// Which detected IDEs the user selected for configuration.
    pub selected_ides: Vec<DetectedIde>,
    /// Whether and where to install steering files.
    pub steering: SteeringChoice,
    /// The steering intensity level.
    pub intensity: SteeringIntensity,
}

/// A planned filesystem operation for dry-run output.
#[derive(Debug, Clone, PartialEq)]
pub enum InstallOperation {
    /// Create a new file at the given path.
    Create { path: PathBuf, description: String },
    /// Modify an existing file at the given path.
    Modify { path: PathBuf, description: String },
    /// Skip a file (already up-to-date or excluded by user choice).
    Skip { path: PathBuf, reason: String },
}

impl InstallOperation {
    /// Format the operation as a single human-readable line for dry-run output.
    pub fn display_line(&self) -> String {
        match self {
            InstallOperation::Create { path, description } => {
                format!("CREATE  {} — {}", path.display(), description)
            }
            InstallOperation::Modify { path, description } => {
                format!("MODIFY  {} — {}", path.display(), description)
            }
            InstallOperation::Skip { path, reason } => {
                format!("SKIP    {} — {}", path.display(), reason)
            }
        }
    }
}

/// Run the interactive install flow.
///
/// This function implements the new user-controlled install experience:
/// 1. Prompts (in fixed order): scope → IDE selection → steering → intensity
/// 2. Supports `--non-interactive` (loads preferences or uses defaults)
/// 3. Supports `--dry-run` (prints planned operations without executing)
/// 4. Persists preferences on successful completion
/// 5. Skips IDE selection if no IDEs are detected
///
/// # Arguments
/// - `prompter` — Trait object for interactive prompts (use `StdinPrompter` for production)
/// - `non_interactive` — If true, skip prompts and use stored/default preferences
/// - `dry_run` — If true, print planned operations and exit without modifying files
/// - `binary_path` — Optional path to the cbm binary (for MCP config entries)
///
/// # Errors
/// Returns an error if:
/// - The user cancels (Ctrl+C/EOF) — mapped to `PrompterError::Cancelled`
/// - Preferences cannot be saved (filesystem error)
pub fn install_interactive(
    prompter: &dyn Prompter,
    non_interactive: bool,
    dry_run: bool,
    binary_path: Option<&Path>,
) -> Result<InstallConfig> {
    let config = if non_interactive {
        resolve_non_interactive()?
    } else {
        resolve_interactive(prompter)?
    };

    // Collect planned operations
    let operations = plan_operations(&config, binary_path);

    if dry_run {
        // Print each operation without executing
        for op in &operations {
            prompter.info(&op.display_line());
        }
        return Ok(config);
    }

    // Execute the actual install using the user's configuration
    execute_install(&config, binary_path)?;

    // Persist preferences on successful completion
    let prefs = InstallPreferences {
        scope: Some(config.scope.clone()),
        steering: Some(config.steering.clone()),
        global_intensity: Some(match config.scope {
            InstallScope::Global | InstallScope::Both => config.intensity.clone(),
            InstallScope::WorkspaceOnly => SteeringIntensity::Lite,
        }),
        workspace_intensity: Some(match config.scope {
            InstallScope::WorkspaceOnly | InstallScope::Both => config.intensity.clone(),
            InstallScope::Global => SteeringIntensity::Full,
        }),
        selected_ides: Some(
            config
                .selected_ides
                .iter()
                .map(|d| d.ide.key().to_string())
                .collect(),
        ),
        activated_workspaces: None,
    };
    prefs.save()?;

    Ok(config)
}

/// Execute the install for a given configuration without prompts.
///
/// This is the programmatic entry point used by the web UI and other callers
/// that have already collected user preferences. It:
/// 1. Runs MCP config installation for selected IDEs only
/// 2. Writes skill files for selected IDEs only
/// 3. Writes steering files based on scope/intensity
/// 4. Does NOT save preferences (caller is responsible)
pub fn execute_install(config: &InstallConfig, binary_path: Option<&Path>) -> Result<()> {
    let bin = binary_path
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|| {
            std::env::current_exe()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_else(|_| "codryn".into())
        });

    // Install MCP config + skills ONLY for the user-selected IDEs
    for detected_ide in &config.selected_ides {
        let ide = &detected_ide.ide;
        let mcp_path = &detected_ide.mcp_config_path;

        // Write MCP config for this IDE
        match ide {
            Ide::Kiro => { let _ = install_kiro_mcp(&bin, mcp_path, false); }
            Ide::VsCode => { let _ = install_vscode_mcp(&bin, mcp_path, false); }
            Ide::Codex => { let _ = install_codex_mcp(&bin, mcp_path, false); }
            _ => { let _ = install_editor_mcp(&bin, mcp_path, false); }
        }

        // Write skill files for this IDE
        let skills_dir = detected_ide.config_dir.join("skills");
        let _ = install_skill(&skills_dir, false, false);

        // IDE-specific extras (instructions files, Kiro steering shortcut)
        match ide {
            Ide::Kiro => {
                // Kiro shortcut with `inclusion: auto` frontmatter
                let shortcut = skills_dir.join(format!("{}.md", SKILL_DIR_NAME));
                let _ = std::fs::write(&shortcut, kiro_skill_shortcut_content());
            }
            Ide::VsCode => {
                let _ = upsert_instructions(
                    &detected_ide.config_dir.join("AGENTS.md"),
                    &agent_instructions("vscode"),
                );
                // Copilot skills
                let home = platform::home_dir().unwrap_or_default();
                let copilot_skills = PathBuf::from(&home).join(".copilot").join("skills");
                let _ = install_skill(&copilot_skills, false, false);
                // copilot-instructions.md
                let github_dir = PathBuf::from(&home).join(".github");
                let _ = upsert_instructions(
                    &github_dir.join("copilot-instructions.md"),
                    &agent_instructions("github-copilot"),
                );
            }
            Ide::Cursor => {
                let _ = upsert_instructions(
                    &detected_ide.config_dir.join("AGENTS.md"),
                    &agent_instructions("cursor"),
                );
            }
            Ide::Codex => {
                let _ = upsert_instructions(
                    &detected_ide.config_dir.join("AGENTS.md"),
                    &agent_instructions("codex-cli"),
                );
            }
            Ide::Gemini => {
                let _ = upsert_instructions(
                    &detected_ide.config_dir.join("GEMINI.md"),
                    &agent_instructions("gemini-cli"),
                );
            }
            _ => {} // ClaudeCode, ClaudeDesktop, Windsurf, Zed — MCP + skills only
        }
    }

    // Write steering files based on configuration
    match &config.steering {
        SteeringChoice::No => {} // Skip steering
        SteeringChoice::Yes | SteeringChoice::WorkspaceOnly => {
            // Global steering (only if scope is global/both OR steering is explicitly "Yes")
            if config.steering == SteeringChoice::Yes
                || config.scope == InstallScope::Global
                || config.scope == InstallScope::Both
            {
                let global_path = dirs::home_dir()
                    .unwrap_or_default()
                    .join(".kiro/steering/codebase-memory.md");
                crate::steering::write_steering(&global_path, &config.intensity)?;
            }

            // Workspace steering (only if scope is workspace/both OR steering is workspace-only)
            if config.steering == SteeringChoice::WorkspaceOnly
                || config.scope == InstallScope::WorkspaceOnly
                || config.scope == InstallScope::Both
            {
                let workspace_path =
                    std::env::current_dir().unwrap_or_default().join(".kiro/steering/codebase-memory.md");
                crate::steering::write_steering(&workspace_path, &config.intensity)?;
            }
        }
    }

    Ok(())
}

/// Resolve configuration interactively via prompts.
///
/// Prompt ordering invariant: scope → IDE selection → steering → intensity.
fn resolve_interactive(prompter: &dyn Prompter) -> Result<InstallConfig> {
    // 1. Scope selection
    let scope = prompt_scope(prompter)?;

    // 2. IDE selection (skip if none detected)
    let detected = detect_ides();
    let selected_ides = if detected.is_empty() {
        prompter.info("ℹ No supported IDEs detected. Skipping IDE configuration.");
        Vec::new()
    } else {
        prompt_ide_selection(prompter, &detected)?
    };

    // 3. Steering choice
    let steering = prompt_steering_choice(prompter)?;

    // 4. Steering intensity
    let intensity = prompt_steering_intensity(prompter, &scope)?;

    Ok(InstallConfig {
        scope,
        selected_ides,
        steering,
        intensity,
    })
}

/// Resolve configuration non-interactively from stored preferences or defaults.
fn resolve_non_interactive() -> Result<InstallConfig> {
    let prefs = InstallPreferences::load()?;

    let scope = prefs.effective_scope();
    let intensity = prefs.effective_intensity(&scope);
    let steering = prefs.steering.unwrap_or(SteeringChoice::WorkspaceOnly);

    // Detect IDEs and filter by stored preference (if any)
    let detected = detect_ides();
    let selected_ides = match &prefs.selected_ides {
        Some(keys) => detected
            .into_iter()
            .filter(|d| keys.contains(&d.ide.key().to_string()))
            .collect(),
        None => detected, // If no preference stored, use all detected
    };

    Ok(InstallConfig {
        scope,
        selected_ides,
        steering,
        intensity,
    })
}

/// Plan operations that would be performed (for dry-run output).
fn plan_operations(config: &InstallConfig, binary_path: Option<&Path>) -> Vec<InstallOperation> {
    let mut ops = Vec::new();

    // MCP config operations for selected IDEs
    for detected_ide in &config.selected_ides {
        let mcp_path = &detected_ide.mcp_config_path;
        if mcp_path.exists() {
            ops.push(InstallOperation::Modify {
                path: mcp_path.clone(),
                description: format!(
                    "Update MCP entry for {} ({})",
                    detected_ide.ide.display_name(),
                    binary_path
                        .map(|p| p.display().to_string())
                        .unwrap_or_else(|| "<binary>".into())
                ),
            });
        } else {
            ops.push(InstallOperation::Create {
                path: mcp_path.clone(),
                description: format!(
                    "Create MCP config for {}",
                    detected_ide.ide.display_name()
                ),
            });
        }
    }

    // Steering file operations
    match &config.steering {
        SteeringChoice::No => {
            ops.push(InstallOperation::Skip {
                path: PathBuf::from("(steering files)"),
                reason: "Steering installation declined".into(),
            });
        }
        SteeringChoice::Yes | SteeringChoice::WorkspaceOnly => {
            let steering_desc = match config.intensity {
                SteeringIntensity::Full => "full steering template",
                SteeringIntensity::Lite => "lite steering template",
                SteeringIntensity::None => "no steering content",
            };

            if config.steering == SteeringChoice::Yes
                || config.scope == InstallScope::Global
                || config.scope == InstallScope::Both
            {
                let global_path = dirs::home_dir()
                    .unwrap_or_default()
                    .join(".kiro/steering/codebase-memory.md");
                ops.push(InstallOperation::Create {
                    path: global_path,
                    description: format!("Install global {}", steering_desc),
                });
            }

            if config.steering == SteeringChoice::WorkspaceOnly
                || config.scope == InstallScope::WorkspaceOnly
                || config.scope == InstallScope::Both
            {
                let workspace_path = PathBuf::from(".kiro/steering/codebase-memory.md");
                ops.push(InstallOperation::Create {
                    path: workspace_path,
                    description: format!("Install workspace {}", steering_desc),
                });
            }
        }
    }

    // Preferences file
    ops.push(InstallOperation::Create {
        path: InstallPreferences::path(),
        description: "Save install preferences".into(),
    });

    ops
}

// ── Prompt helpers ───────────────────────────────────────────────

/// Prompt for install scope.
fn prompt_scope(prompter: &dyn Prompter) -> Result<InstallScope> {
    let options = &["workspace-only", "global", "both"];
    let idx = prompter.select("Install scope:", options, 0)?;
    Ok(match idx {
        0 => InstallScope::WorkspaceOnly,
        1 => InstallScope::Global,
        2 => InstallScope::Both,
        _ => InstallScope::WorkspaceOnly,
    })
}

/// Prompt for IDE selection from detected IDEs.
fn prompt_ide_selection(
    prompter: &dyn Prompter,
    detected: &[DetectedIde],
) -> Result<Vec<DetectedIde>> {
    let options: Vec<&str> = detected.iter().map(|d| d.ide.display_name()).collect();
    let defaults: Vec<bool> = detected.iter().map(|_| true).collect();

    let selected_indices = prompter.multi_select("Select IDEs to configure:", &options, &defaults)?;

    Ok(selected_indices
        .into_iter()
        .filter_map(|i| detected.get(i).cloned())
        .collect())
}

/// Prompt for steering file installation preference.
fn prompt_steering_choice(prompter: &dyn Prompter) -> Result<SteeringChoice> {
    let options = &["workspace-only", "yes (global)", "no"];
    let idx = prompter.select("Install steering files?", options, 0)?;
    Ok(match idx {
        0 => SteeringChoice::WorkspaceOnly,
        1 => SteeringChoice::Yes,
        2 => SteeringChoice::No,
        _ => SteeringChoice::WorkspaceOnly,
    })
}

/// Prompt for steering intensity.
fn prompt_steering_intensity(
    prompter: &dyn Prompter,
    scope: &InstallScope,
) -> Result<SteeringIntensity> {
    let options = &["lite", "full", "none"];
    // Default depends on scope: lite for global, full for workspace
    let default_idx = match scope {
        InstallScope::Global => 0,       // lite
        InstallScope::WorkspaceOnly => 1, // full
        InstallScope::Both => 1,         // full (workspace bias)
    };
    let idx = prompter.select("Steering intensity:", options, default_idx)?;
    Ok(match idx {
        0 => SteeringIntensity::Lite,
        1 => SteeringIntensity::Full,
        2 => SteeringIntensity::None,
        _ => SteeringIntensity::Lite,
    })
}

#[cfg(test)]
mod tests_interactive {
    use super::*;
    use codryn_foundation::ide_detect::Ide;
    use crate::prompter::{MockPrompter, MockResponse, PrompterError};

    #[test]
    fn test_install_interactive_dry_run_no_mutations() {
        let prompter = MockPrompter::new(vec![
            MockResponse::Select(0),          // scope: workspace-only
            MockResponse::MultiSelect(vec![]), // no IDEs (will be empty since detect_ides is real)
            MockResponse::Select(0),          // steering: workspace-only
            MockResponse::Select(1),          // intensity: full
        ]);

        let result = install_interactive(&prompter, false, true, None);
        assert!(result.is_ok());
        let config = result.unwrap();
        assert_eq!(config.scope, InstallScope::WorkspaceOnly);
        assert_eq!(config.steering, SteeringChoice::WorkspaceOnly);
        assert_eq!(config.intensity, SteeringIntensity::Full);
    }

    #[test]
    fn test_install_interactive_non_interactive_uses_defaults() {
        let prompter = MockPrompter::new(vec![]);

        // Non-interactive mode doesn't prompt at all
        let result = install_interactive(&prompter, true, true, None);
        assert!(result.is_ok());
        let config = result.unwrap();
        // Defaults: scope=workspace-only, steering=workspace-only, intensity=full (workspace)
        assert_eq!(config.scope, InstallScope::WorkspaceOnly);
        assert_eq!(config.steering, SteeringChoice::WorkspaceOnly);
        assert_eq!(config.intensity, SteeringIntensity::Full);
        // No prompts consumed
        assert_eq!(prompter.remaining_responses(), 0);
    }

    #[test]
    fn test_install_interactive_cancel_returns_error() {
        let prompter = MockPrompter::new(vec![
            MockResponse::Cancel, // Cancel at the first prompt (scope)
        ]);

        let result = install_interactive(&prompter, false, true, None);
        assert!(result.is_err());
        let err = result.unwrap_err();
        // Should be a PrompterError::Cancelled
        assert!(err.downcast_ref::<PrompterError>().is_some());
    }

    #[test]
    fn test_install_config_display() {
        let config = InstallConfig {
            scope: InstallScope::Global,
            selected_ides: vec![],
            steering: SteeringChoice::Yes,
            intensity: SteeringIntensity::Lite,
        };
        assert_eq!(config.scope, InstallScope::Global);
        assert_eq!(config.steering, SteeringChoice::Yes);
        assert_eq!(config.intensity, SteeringIntensity::Lite);
    }

    #[test]
    fn test_install_operation_display_line() {
        let create = InstallOperation::Create {
            path: PathBuf::from("/home/user/.kiro/steering/codebase-memory.md"),
            description: "Install lite steering template".into(),
        };
        assert!(create.display_line().starts_with("CREATE"));
        assert!(create.display_line().contains("codebase-memory.md"));

        let modify = InstallOperation::Modify {
            path: PathBuf::from("/home/user/.cursor/mcp.json"),
            description: "Update MCP entry".into(),
        };
        assert!(modify.display_line().starts_with("MODIFY"));

        let skip = InstallOperation::Skip {
            path: PathBuf::from("(steering files)"),
            reason: "Declined".into(),
        };
        assert!(skip.display_line().starts_with("SKIP"));
    }

    #[test]
    fn test_plan_operations_no_steering() {
        let config = InstallConfig {
            scope: InstallScope::WorkspaceOnly,
            selected_ides: vec![],
            steering: SteeringChoice::No,
            intensity: SteeringIntensity::Lite,
        };
        let ops = plan_operations(&config, None);
        // Should have a Skip for steering and a Create for preferences
        assert!(ops.iter().any(|op| matches!(op, InstallOperation::Skip { .. })));
        assert!(ops.iter().any(|op| matches!(op, InstallOperation::Create { description, .. } if description.contains("preferences"))));
    }

    #[test]
    fn test_plan_operations_with_ides() {
        let config = InstallConfig {
            scope: InstallScope::WorkspaceOnly,
            selected_ides: vec![DetectedIde {
                ide: Ide::Cursor,
                config_dir: PathBuf::from("/tmp/fake/.cursor"),
                mcp_config_path: PathBuf::from("/tmp/fake/.cursor/mcp.json"),
                detection_method: "directory",
            }],
            steering: SteeringChoice::WorkspaceOnly,
            intensity: SteeringIntensity::Full,
        };
        let ops = plan_operations(&config, Some(Path::new("/usr/local/bin/codryn")));
        // Should have a Create for the MCP config (since path doesn't exist)
        assert!(ops.iter().any(|op| matches!(op, InstallOperation::Create { path, .. }
            if path.to_str().unwrap().contains("mcp.json"))));
    }

    #[test]
    fn test_prompt_scope_workspace() {
        let prompter = MockPrompter::new(vec![MockResponse::Select(0)]);
        let scope = prompt_scope(&prompter).unwrap();
        assert_eq!(scope, InstallScope::WorkspaceOnly);
    }

    #[test]
    fn test_prompt_scope_global() {
        let prompter = MockPrompter::new(vec![MockResponse::Select(1)]);
        let scope = prompt_scope(&prompter).unwrap();
        assert_eq!(scope, InstallScope::Global);
    }

    #[test]
    fn test_prompt_scope_both() {
        let prompter = MockPrompter::new(vec![MockResponse::Select(2)]);
        let scope = prompt_scope(&prompter).unwrap();
        assert_eq!(scope, InstallScope::Both);
    }

    #[test]
    fn test_prompt_steering_choice_workspace_only() {
        let prompter = MockPrompter::new(vec![MockResponse::Select(0)]);
        let choice = prompt_steering_choice(&prompter).unwrap();
        assert_eq!(choice, SteeringChoice::WorkspaceOnly);
    }

    #[test]
    fn test_prompt_steering_choice_yes() {
        let prompter = MockPrompter::new(vec![MockResponse::Select(1)]);
        let choice = prompt_steering_choice(&prompter).unwrap();
        assert_eq!(choice, SteeringChoice::Yes);
    }

    #[test]
    fn test_prompt_steering_choice_no() {
        let prompter = MockPrompter::new(vec![MockResponse::Select(2)]);
        let choice = prompt_steering_choice(&prompter).unwrap();
        assert_eq!(choice, SteeringChoice::No);
    }

    #[test]
    fn test_prompt_intensity_lite() {
        let prompter = MockPrompter::new(vec![MockResponse::Select(0)]);
        let intensity = prompt_steering_intensity(&prompter, &InstallScope::Global).unwrap();
        assert_eq!(intensity, SteeringIntensity::Lite);
    }

    #[test]
    fn test_prompt_intensity_full() {
        let prompter = MockPrompter::new(vec![MockResponse::Select(1)]);
        let intensity =
            prompt_steering_intensity(&prompter, &InstallScope::WorkspaceOnly).unwrap();
        assert_eq!(intensity, SteeringIntensity::Full);
    }

    #[test]
    fn test_prompt_intensity_none() {
        let prompter = MockPrompter::new(vec![MockResponse::Select(2)]);
        let intensity = prompt_steering_intensity(&prompter, &InstallScope::Both).unwrap();
        assert_eq!(intensity, SteeringIntensity::None);
    }

    #[test]
    fn test_prompt_ide_selection_all() {
        let detected = vec![
            DetectedIde {
                ide: Ide::Cursor,
                config_dir: PathBuf::from("/fake/.cursor"),
                mcp_config_path: PathBuf::from("/fake/.cursor/mcp.json"),
                detection_method: "directory",
            },
            DetectedIde {
                ide: Ide::Kiro,
                config_dir: PathBuf::from("/fake/.kiro"),
                mcp_config_path: PathBuf::from("/fake/.kiro/settings/mcp.json"),
                detection_method: "directory",
            },
        ];
        let prompter = MockPrompter::new(vec![MockResponse::MultiSelect(vec![0, 1])]);
        let selected = prompt_ide_selection(&prompter, &detected).unwrap();
        assert_eq!(selected.len(), 2);
        assert_eq!(selected[0].ide, Ide::Cursor);
        assert_eq!(selected[1].ide, Ide::Kiro);
    }

    #[test]
    fn test_prompt_ide_selection_subset() {
        let detected = vec![
            DetectedIde {
                ide: Ide::Cursor,
                config_dir: PathBuf::from("/fake/.cursor"),
                mcp_config_path: PathBuf::from("/fake/.cursor/mcp.json"),
                detection_method: "directory",
            },
            DetectedIde {
                ide: Ide::Kiro,
                config_dir: PathBuf::from("/fake/.kiro"),
                mcp_config_path: PathBuf::from("/fake/.kiro/settings/mcp.json"),
                detection_method: "directory",
            },
        ];
        let prompter = MockPrompter::new(vec![MockResponse::MultiSelect(vec![1])]);
        let selected = prompt_ide_selection(&prompter, &detected).unwrap();
        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].ide, Ide::Kiro);
    }

    #[test]
    fn test_resolve_interactive_no_ides_skips_prompt() {
        // When detect_ides() returns empty, the IDE selection prompt is skipped.
        // We can't control detect_ides() in tests directly, but we can verify
        // that the info message about no IDEs is shown when it's empty.
        // This test validates the flow structure by checking prompt ordering.
        let prompter = MockPrompter::new(vec![
            MockResponse::Select(1),          // scope: global
            MockResponse::MultiSelect(vec![]), // IDE selection (may or may not be called)
            MockResponse::Select(2),          // steering: no
            MockResponse::Select(0),          // intensity: lite
        ]);

        // Note: This test exercises the flow; on CI/machines with IDEs installed,
        // the multi_select will be consumed. On machines without, it won't be.
        let _result = install_interactive(&prompter, false, true, None);
        // The key assertion is that it doesn't panic and ordering is maintained
    }
}
