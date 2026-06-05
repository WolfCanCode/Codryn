# Installation

## One-Line Install (Recommended)

### macOS / Linux

```bash
bash <(cd /tmp && git archive --remote=ssh://git@code.swisscom.com:2222/tommy.le/codryn.git HEAD install.sh | tar -xO)
```

### Windows (PowerShell)

```powershell
$a="$env:TEMP\codryn-a.tar"; git archive --remote=ssh://git@code.swisscom.com:2222/tommy.le/codryn.git HEAD install.ps1 > $a; tar -xf $a -C $env:TEMP; Remove-Item $a; & "$env:TEMP\install.ps1"; Remove-Item "$env:TEMP\install.ps1" -ErrorAction SilentlyContinue
```

> **Note:** The archive must be saved to a file before extracting — Windows `tar.exe` cannot read piped streams reliably.

### What the installer does

| Step | Action |
|:-----|:-------|
| 1 | Downloads a pre-built binary for your platform (Linux/macOS, x86_64/arm64) |
| 2 | Falls back to building from source if no binary is available |
| 3 | Installs `codryn` to `~/.local/bin` |
| 4 | On macOS, code-signs the binary (required by Gatekeeper) |

---

## Build from Source

```bash
git clone <repo-url> && cd codryn-rs
cargo build --release
```

`cargo build --release` does everything in one step:

| Step | What happens |
|:-----|:-------------|
| 1 | Runs `npm install` in `graph-ui/` (if `node_modules` missing) |
| 2 | Runs `npm run build` to compile the Angular dashboard |
| 3 | Embeds the built UI assets into the binary via `rust-embed` |
| 4 | Compiles the Rust binary with 14+ tree-sitter grammars |

The final binary is at `./target/release/codryn`. Node.js is **not required at runtime**.

---

## Install Globally

| Method | Command | Notes |
|:-------|:--------|:------|
| **make** (recommended) | `make install` | Builds + installs to `~/.local/bin` + code-signs |
| **Manual copy** | `sudo cp ./target/release/codryn /usr/local/bin/codryn` | Requires sudo |
| **User bin (no sudo)** | `cp ./target/release/codryn ~/.local/bin/codryn` | Add `~/.local/bin` to PATH |
| **cargo install** | `cargo install --path crates/codryn-bin` | Uses `~/.cargo/bin` |

Verify:
```bash
codryn --version
```

---

## Configure Your Agents

```bash
codryn install          # interactive: choose scope, IDEs, steering intensity
codryn install --dry-run       # preview changes without writing
codryn install --non-interactive  # use defaults (workspace-only, all detected IDEs)
codryn install --mode cli      # CLI-first: no MCP config, just the binary + lite steering
codryn uninstall        # remove all MCP configuration
codryn uninstall --keep-data   # remove config but keep the graph database
codryn uninstall --workspace-only  # remove from current workspace only
```

### Workspace Activation (Per-Project)

Instead of global steering that loads on every session, activate codryn per workspace:

```bash
codryn activate              # write steering to .kiro/steering/ in current workspace (full intensity)
codryn activate --global     # global steering (lite intensity by default)
codryn deactivate            # remove workspace steering
codryn deactivate --global   # remove global steering
```

### Steering Intensity

Switch between full (MANDATORY directives) and lite (~10 lines, tools available without forcing):

```bash
codryn steering --mode lite   # switch workspace steering to lite
codryn steering --mode full   # switch workspace steering to full
```

### Selective MCP Configuration

Manage mcp.json entries without re-running full install:

```bash
codryn mcp-config show                    # show all configured entries across IDEs
codryn mcp-config add <path-to-mcp.json>  # add codryn entry (with confirmation)
codryn mcp-config remove <path-to-mcp.json>  # remove codryn entry (with confirmation)
```

### What gets configured

| Agent | Config path | Instructions path |
|:------|:------------|:------------------|
| Claude Code | `~/.claude/mcp_servers.json` | `~/.claude/CLAUDE.md` |
| VS Code | `~/.vscode/mcp.json` | `~/.vscode/AGENTS.md` |
| GitHub Copilot | `~/.vscode/mcp.json` | `~/.github/copilot-instructions.md` + `~/.copilot/skills/` |
| Cursor | `~/.cursor/mcp.json` | `~/.cursor/AGENTS.md` + `~/.cursor/skills/` |
| Zed | `~/Library/Application Support/Zed/settings.json` | — |
| Codex CLI | `~/.codex/config.toml` | `~/.codex/AGENTS.md` + `~/.codex/skills/` |
| Gemini CLI | `~/.gemini/mcp.json` | `~/.gemini/GEMINI.md` + `~/.gemini/skills/` |
| Kiro | `~/.kiro/settings/mcp.json` | `~/.kiro/steering/codebase-memory.md` + `~/.kiro/skills/` |
| Windsurf | `~/.codeium/windsurf/mcp.json` | `~/.codeium/windsurf/skills/` |

All agents also receive a `SKILL.md` file following the [Agent Skills standard](https://agentskills.io/) for progressive-disclosure steering.

Restart your coding agent after running `codryn install`. Then say **"Index this project"** — done.

---

## Update

```bash
codryn update
```

Or one-line update if `codryn update` isn't available yet:

**macOS / Linux:**
```bash
bash <(cd /tmp && git archive --remote=ssh://git@code.swisscom.com:2222/tommy.le/codryn.git HEAD install.sh | tar -xO) update
```

**Windows:**
```powershell
$a="$env:TEMP\codryn-a.tar"; git archive --remote=ssh://git@code.swisscom.com:2222/tommy.le/codryn.git HEAD install.ps1 > $a; tar -xf $a -C $env:TEMP; Remove-Item $a; & "$env:TEMP\install.ps1" update; Remove-Item "$env:TEMP\install.ps1" -ErrorAction SilentlyContinue
```

---

## Usage

### MCP Server (default)

```bash
codryn    # runs on stdin/stdout — your agent starts this automatically
```

### Web Dashboard

```bash
codryn --ui              # start at http://127.0.0.1:9749
codryn --ui --port=8080  # custom port
```

### CLI Reference

| Command | Description |
|:--------|:------------|
| `codryn` | MCP server mode (agents start this automatically) |
| `codryn --ui` | MCP server + web dashboard |
| `codryn --ui --port=N` | Custom dashboard port |
| `codryn install` | Interactive install: choose scope, IDEs, steering intensity |
| `codryn install --dry-run` | Preview configuration changes |
| `codryn install --non-interactive` | Use defaults (workspace-only, all detected IDEs) |
| `codryn install --mode cli` | CLI-first mode: no MCP config, just binary + lite steering |
| `codryn uninstall` | Remove all agent configurations |
| `codryn uninstall --keep-data` | Remove config but preserve graph database |
| `codryn uninstall --workspace-only` | Remove from current workspace only |
| `codryn activate` | Write steering to workspace `.kiro/steering/` |
| `codryn activate --global` | Write steering globally (lite intensity) |
| `codryn deactivate` | Remove workspace steering |
| `codryn deactivate --global` | Remove global steering |
| `codryn steering --mode lite\|full` | Switch steering intensity |
| `codryn mcp-config show` | Show all configured MCP entries across IDEs |
| `codryn mcp-config add <path>` | Add codryn entry to a specific mcp.json |
| `codryn mcp-config remove <path>` | Remove codryn entry from a specific mcp.json |
| `codryn status` | Check agent installation status |
| `codryn update` | Self-update to latest version |
| `codryn query <tool> --<key> <val>` | Run any MCP tool from the CLI (one-shot) |
| `codryn query` | List all available MCP tools |
| `codryn validate --project <p>` | Check graph structural integrity |
| `codryn dedupe --project <p>` | Detect and merge duplicate nodes |
| `codryn complexity --project <p>` | Report most complex symbols |
| `codryn doc-coverage --project <p>` | Documentation coverage by module |
| `codryn deps --project <p>` | List dependencies from manifest files |
| `codryn query --project <p> "<cypher>"` | Execute raw Cypher queries |
| `codryn symbol --project <p> "<name>"` | Find symbols by name |
| `codryn refs --project <p> "<qn>"` | Find incoming references |
| `codryn impact --project <p> "<qn>"` | Impact analysis from CLI |
| `codryn index-runs --project <p>` | List recent index runs |
| `codryn snapshots --project <p>` | List historical graph snapshots |
| `codryn diff --project <p>` | Compare two snapshots |
| `codryn backup [path]` | Back up the graph database |
| `codryn restore <path>` | Restore from backup |
| `codryn --version` | Print version |
| `codryn --help` | Show help |

### Environment Variables

| Variable | Description |
|:---------|:------------|
| `RUST_LOG` | Log level (e.g. `codryn=debug`, `codryn=trace`) |
| `CBM_LOG_LEVEL` | Override log level from config |
| `CBM_LOG_FORMAT` | Log format: `compact` (default) or `json` |
| `CBM_MAX_MEMORY_MB` | Memory pressure threshold (default: 512) |

### Data Storage

All graph data is stored in `~/.codryn/store/graph.db` (SQLite). No configuration needed. Delete this file to reset all indexed data.

### Configuration File

Optional configuration at `~/.config/codryn/config.toml`:

```toml
log_level = "info"
log_format = "compact"    # or "json"
max_memory_mb = 512
pool_size = 4

[rate_limit]
window_seconds = 60
max_calls = 100
max_expensive = 10
```

Environment variables override config values: `CBM_LOG_LEVEL`, `CBM_LOG_FORMAT`, `CBM_MAX_MEMORY_MB`.
