<p align="center">
  <img src="assets/logo.png" alt="codryn logo" width="88" />
</p>

<h1 align="center">codryn</h1>

<p align="center">
  Open-source Rust knowledge graph for AI coding agents.
</p>

<p align="center">
  Fast indexing. Deep code understanding. Embedded web UI. Single binary.
</p>

`codryn` is an open-source Rust knowledge graph and MCP server for AI coding agents, built to make large codebases easier to explore, trace, and understand.

> Based on the paper: [Codebase-memory-mcp: A Persistent Knowledge Graph for AI Coding Agents](https://arxiv.org/abs/2603.27277)

If this project is useful, give it a star. It helps more people discover the project and support continued open-source work.

## Why this project

AI coding agents are much better when they can understand structure, not just grep files.

`codryn` indexes a repository into a persistent graph of:

- functions, classes, methods, files, and folders
- call paths and imports
- routes, DTOs, and service layers
- frontend component relationships and dependency injection
- cross-project links between systems

That gives agents fast answers for things like:

- where a request starts and where it ends
- who calls this function
- what breaks if I change this symbol
- which files matter next
- how frontend and backend connect

## Highlights

- **Open source + MIT licensed**
- **Rust implementation** with a strong focus on speed and portability
- **Persistent knowledge graph** stored in SQLite
- **Embedded dashboard** for visual graph exploration
- **Incremental indexing** so re-runs stay fast
- **Cross-project linking** for multi-repo systems
- **30 MCP tools** for search, tracing, navigation, analysis, and architecture
- **64 language support** through tree-sitter
- **Single binary** with no Docker required

## What makes it interesting

- **Built for AI agents**: not just search, but graph-aware navigation and analysis
- **Framework-aware**: strong support for Spring Boot and Angular structure
- **Useful locally**: run as an MCP server or open the web UI
- **Practical architecture**: one workspace, one binary, one local graph store

## Quick look

### Core capabilities

- Search symbols and code structure
- Trace call paths and backend flows
- Inspect architecture at a high level
- Find routes, references, tests, and likely entrypoints
- Query the graph directly with Cypher
- Visualize projects and relationships in the browser

### Web dashboard

The built-in UI includes:

- project overview cards
- interactive graph exploration
- backend flow and frontend component flow views
- Cypher query console
- analytics for tool usage

## MCP tools

Some of the most useful tools:

| Tool | Description |
|---|---|
| `index_repository` | Build the graph for a repository |
| `search_graph` | Search symbols, names, and indexed structure |
| `trace_call_path` | Follow calls between functions |
| `get_architecture` | Summarize packages, modules, and code shape |
| `get_code_snippet` | Read source for a symbol with context |
| `find_references` | Find symbol usage through graph edges |
| `impact_analysis` | Estimate what a change will affect |
| `find_routes` | Discover API routes and DTO relationships |
| `trace_backend_flow` | Trace route to controller to service to repository |
| `suggest_next_reads` | Help agents decide what to inspect next |

## Architecture

```text
crates/
├── foundation
├── store
├── discovery
├── indexing pipeline
├── graph buffer
├── cypher engine
├── services
├── mcp server
├── cli
├── ui
├── watcher
└── app binary

ui/
└── dashboard for graph exploration
```

## Local run

For now, keep it simple:

```bash
cargo build --release
./target/release/codryn --ui
```

Then open `http://127.0.0.1:9749`.

## Supported languages

Supports 64 languages through tree-sitter, including Rust, TypeScript, JavaScript, Java, Kotlin, Python, Go, C, C++, C#, PHP, Ruby, Scala, Swift, SQL, HTML, CSS, Vue, Svelte, Bash, Dockerfile, YAML, and more.

## Open source

This project is MIT licensed and intended to be easy to use, inspect, extend, and contribute to.

If you like the direction:

- star the repo
- open an issue
- suggest a tool or framework integration
- contribute improvements

## License

MIT

## References

- [Codebase-memory-mcp paper](https://arxiv.org/abs/2603.27277)
- [Original upstream inspiration](https://github.com/DeusData/codebase-memory-mcp)
