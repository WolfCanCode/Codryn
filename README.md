<p align="center">
  <img src="assets/logo-2.png" alt="codryn logo" width="180" />
</p>

<p align="center">
  Open-source Rust knowledge graph for AI coding agents.
</p>

<p align="center">
  Fast indexing. Deep code understanding. Embedded web UI. Single binary.
</p>

`codryn` is an open-source Rust knowledge graph and MCP server for AI coding agents, built to make large codebases easier to explore, trace, and understand.

> Based on the paper: [Codebase-memory-mcp: A Persistent Knowledge Graph for AI Coding Agents](https://arxiv.org/abs/2603.27277)

If this project is useful, give it a star. It helps more people discover the project and support continued open-source work.

## Install

```bash
curl -fsSL https://raw.githubusercontent.com/WolfCanCode/Codryn/main/install.sh | sh
```

> Tries pre-built binaries first. Falls back to building from source if none are available for your platform. Requires Rust and Node.js 20+ for source builds.

## Why this project

AI coding agents are much better when they can understand structure, not just grep files.

`codryn` indexes a repository into a persistent graph of:

- functions, classes, methods, files, and folders
- call paths and imports
- routes, DTOs, and service layers
- frontend component relationships and dependency injection
- CI/CD pipelines, jobs, stages, includes, and dependency edges
- infrastructure resources from Docker, Kubernetes, Kustomize, Helm, and Terraform-style manifests
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
- **CI/CD and infrastructure discovery** for GitHub Actions, GitLab CI, CircleCI, Azure Pipelines, Bitbucket Pipelines, Jenkinsfile-style jobs, Docker, Kubernetes, Kustomize, Helm, and Terraform resources
- **64 language detection** plus tree-sitter walkers for Rust, TypeScript, JavaScript, Python, Go, C/C++, C#, Ruby, PHP, Swift, Scala, Elixir, and Bash
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
- Inspect pipeline DAGs and infrastructure resources

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
| `find_pipelines` | Discover CI/CD pipelines with stages, jobs, and dependencies |
| `find_infrastructure` | Discover Docker, Kubernetes, Helm, and Terraform-style resources |
| `suggest_next_reads` | Help agents decide what to inspect next |

## Web API

The embedded dashboard also exposes JSON endpoints for local UI features:

| Endpoint | Description |
|---|---|
| `GET /api/analytics/{id}` | Return a single analytics call detail |
| `GET /api/pipelines?project=<name>` | List pipeline DAGs for a project |
| `GET /api/pipelines?project=<name>&name=<pipeline>` | Return one pipeline DAG |
| `GET /api/infrastructure?project=<name>[&type=<kind>]` | List indexed infrastructure resources |

## Architecture

```text
crates/
├── foundation
├── store
├── discovery
├── indexing pipeline
├── tree-sitter walkers
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

Supports 64 language mappings, including Rust, TypeScript, JavaScript, Java, Kotlin, Python, Go, C, C++, C#, PHP, Ruby, Scala, Swift, SQL, HTML, CSS, Vue, Svelte, Bash, Dockerfile, YAML, and more. Codryn also includes tree-sitter symbol walkers for the main backend, frontend, scripting, and systems languages used by the indexing pipeline.

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
