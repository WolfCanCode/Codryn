# Installation

## One-Line Install (Recommended)

### macOS / Linux

```bash
curl -fsSL https://raw.githubusercontent.com/WolfCanCode/Codryn/main/install.sh | sh
```

### Windows (PowerShell)

```powershell
irm https://raw.githubusercontent.com/WolfCanCode/Codryn/main/install.ps1 | iex
```

> **Note:** On Windows, save the installer script to a file before running if your shell cannot execute piped scripts reliably.

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
git clone https://github.com/WolfCanCode/Codryn.git && cd Codryn
cargo build --release
```

`cargo build --release` does everything in one step:

| Step | What happens |
|:-----|:-------------|
| 1 | Runs `npm install` in `ui/` (if dependencies are missing) |
| 2 | Runs `npm run build` to compile the React dashboard |
| 3 | Embeds the built UI assets into the binary via `rust-embed` |
| 4 | Compiles the Rust binary with 30+ tree-sitter grammars |

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

## Configure Your Agent

```bash
codryn install          # interactive: choose scope, detected agents, steering intensity
codryn install --dry-run       # preview changes without writing
codryn install --non-interactive  # use defaults (workspace-only, all detected agents)
codryn install --mode cli      # CLI-first: no MCP config, just the binary + lite steering
codryn uninstall        # remove all MCP configuration
codryn uninstall --keep-data   # remove config but keep the graph database
codryn uninstall --workspace-only  # remove from current workspace only
```

### Workspace Activation (Per-Project)

Activate codryn per workspace instead of loading global steering on every session:

```bash
codryn activate              # write workspace steering (full intensity)
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

Manage MCP config entries without re-running full install:

```bash
codryn mcp-config show                    # show all configured entries
codryn mcp-config add <path-to-mcp.json>  # add codryn entry (with confirmation)
codryn mcp-config remove <path-to-mcp.json>  # remove codryn entry (with confirmation)
```

### What gets configured

`codryn install` detects MCP-compatible coding agents on your machine and writes:

- MCP server entries pointing at the `codryn` binary
- Steering instructions that teach the agent to use graph tools first
- Optional skill files for progressive disclosure

Run `codryn status` or `codryn mcp-config show` to see which agents were configured and where files were written.

Restart your coding agent after running `codryn install`. Then say **"Index this project"** — done.

---

## Update

```bash
codryn update
```

Or reinstall from the latest release:

```bash
curl -fsSL https://raw.githubusercontent.com/WolfCanCode/Codryn/main/install.sh | sh
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
| `codryn install` | Interactive install: choose scope, agents, steering intensity |
| `codryn install --dry-run` | Preview configuration changes |
| `codryn install --non-interactive` | Use defaults (workspace-only, all detected agents) |
| `codryn install --mode cli` | CLI-first mode: no MCP config, just binary + lite steering |
| `codryn uninstall` | Remove all agent configurations |
| `codryn uninstall --keep-data` | Remove config but preserve graph database |
| `codryn uninstall --workspace-only` | Remove from current workspace only |
| `codryn activate` | Write workspace steering (full intensity) |
| `codryn activate --global` | Write global steering (lite intensity) |
| `codryn deactivate` | Remove workspace steering |
| `codryn deactivate --global` | Remove global steering |
| `codryn steering --mode lite\|full` | Switch steering intensity |
| `codryn mcp-config show` | Show all configured MCP entries |
| `codryn mcp-config add <path>` | Add codryn entry to a specific MCP config file |
| `codryn mcp-config remove <path>` | Remove codryn entry from a specific MCP config file |
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
| `CODRYN_LOG_LEVEL` | Override log level from config |
| `CODRYN_LOG_FORMAT` | Log format: `compact` (default) or `json` (legacy alias: `CBM_LOG_FORMAT`) |
| `CODRYN_MAX_MEMORY_MB` | Memory pressure threshold (default: 512; legacy alias: `CBM_MAX_MEMORY_MB`) |
| `CODRYN_STORE_PATH` | Override graph database location |

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

Environment variables override config values. Legacy `CBM_*` names are still accepted for several settings.
