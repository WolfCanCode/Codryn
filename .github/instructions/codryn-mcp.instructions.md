# codryn mcp — Agent Steering Guide

## When to Use Graph Tools

Use the codryn mcp knowledge graph as your **primary** method for code discovery:

- **Symbol lookup**: `find_symbol` — fast ranked search by name or qualified name
- **Full context**: `get_symbol_details` — callers, callees, imports, inheritance, source snippet in one call
- **Usage analysis**: `find_references` — who calls/imports a symbol, grouped by file
- **Impact analysis**: `impact_analysis` — blast radius of changing a symbol or file
- **Architecture**: `get_architecture` — high-level module/package structure
- **Cross-project**: `search_linked_projects` — query linked projects (e.g. frontend↔backend)
- **Code search**: `search_graph` — searches both symbol names AND code content (FTS)
- **REST contracts**: Search for `Route` nodes to find endpoints, DTOs, and their relationships
- **Call tracing**: `trace_call_path` — trace call chains between functions

## When NOT to Use Graph Tools

Fall back to **grep/glob** for:

- String literals and error messages
- HTML template content and attribute bindings
- CSS class names and selectors
- Config values (`.env`, `application.yml`, `package.json`)
- Regex patterns in code
- File content that isn't inside a function/class body
- Comments and documentation text

The graph indexes **symbol definitions and their relationships** — not raw text content. For anything that isn't a named symbol or code body, grep is faster and more reliable.

## Tool Selection Guide

| I want to... | Use this tool |
|---|---|
| Find a symbol by name | `find_symbol(query="OrderService")` |
| Get full context (callers, callees, imports) | `get_symbol_details(name="OrderService")` |
| Find who uses a symbol | `find_references(name="OrderService")` |
| Check change impact | `impact_analysis(name="OrderService")` |
| Search code content (not just names) | `search_graph(query="isUpdate")` |
| Read source code | `get_code_snippet(file_path="...", start_line=10)` |
| Browse a directory's symbols | `get_code_snippet(file_path="src/dto")` |
| Find REST endpoints | `search_graph(query="PATCH /travelrequest")` |
| Query endpoint DTOs | `query_graph(query="MATCH (r:Route)-[:ACCEPTS_DTO]->(d) RETURN r.name, d.name")` |
| Explore architecture | `get_architecture()` |
| Search linked backend | `search_linked_projects(query="UserDto")` |
| Debug missing symbols | `explain_index_result(name="MyClass")` |
| Summarize a file without reading it | `get_file_overview(file_path="src/services/user.ts")` |
| Find where to start reading | `find_entrypoints(entry_type="http")` |
| Decide what to read next | `suggest_next_reads(qualified_name="...", goal="understand")` |
| Trace request/data flow | `trace_data_flow(source="handleRequest")` |
| Find tests for a symbol | `find_tests_for_target(file_path="src/services/user.ts")` |
| Suggest cross-project links | `suggest_project_links()` |
| Find REST API routes | `find_routes(method="GET")` |
| Explain backend flow | `trace_backend_flow(route_path="/v1/users", http_method="GET")` |

## REST Contract Queries

The graph indexes Java/Kotlin REST controllers as `Route` nodes with edges:

- `HANDLES_ROUTE` — method → route (which method handles this endpoint)
- `ACCEPTS_DTO` — route → DTO class (request body type)
- `RETURNS_DTO` — route → DTO class (response type)

Use `find_routes` for structured route discovery with fuzzy scope matching:
```
# Find routes matching a domain concept (normalizes kebab/snake/camelCase)
find_routes(scope="travel-request")

# Filter by HTTP method
find_routes(method="PATCH", scope="travel-request")
```

Use `trace_backend_flow` to explain the full request flow in one call:
```
# Trace a specific route's flow
trace_backend_flow(route_path="/v1/travelrequest/{id}", http_method="PATCH")

# Returns: entry (method, path, handler), flow (controller, service_chain, repository_chain, DTOs), summary (confidence, flow_type), graph (nodes + edges for visualization)
```

Example Cypher queries:
```
# Find all endpoints
query_graph("MATCH (r:Route) RETURN r.name, r.properties")

# What does PATCH /travelrequest accept?
query_graph("MATCH (r:Route)-[:ACCEPTS_DTO]->(d) WHERE r.name CONTAINS 'PATCH' RETURN d.name")

# Which method handles GET /users?
query_graph("MATCH (m)-[:HANDLES_ROUTE]->(r:Route) WHERE r.name CONTAINS 'GET /users' RETURN m.name, m.file_path")
```

## Snippet Behavior

`get_symbol_details` with `include_snippet: true` returns the **full symbol body** (up to 100 lines) by default. Set `snippet_lines` explicitly to cap at a different value.

## Stale Results

Search results include an `exists` flag. If `"exists": false`, the file has been deleted since indexing — don't try to read it. Routes with deleted files are automatically filtered out (use `include_deleted: true` to see them for debugging). Re-index the project to clean up stale entries.

## Directory Browsing

`get_code_snippet` accepts directory paths. Instead of erroring, it returns a listing of files and their symbols:
```
get_code_snippet(file_path="src/dto")
→ {"directory": "src/dto", "files": [{"file_path": "...", "symbols": [...]}]}
```

## Angular Template Queries

The graph creates `RENDERS` edges when a component's HTML template uses another component's selector:
```
# Which components does this one render?
get_symbol_details(name="AppComponent")  → relationships.renders: [...]

# Find all components that render TravelRequestComponent
query_graph("MATCH (parent)-[:RENDERS]->(child) WHERE child.name = 'TravelRequestComponent' RETURN parent.name")
```

## Pagination

List/search responses include `has_more: true` when results may be truncated. Increase `limit` to get more results:
```
find_routes(scope="user", limit=50)
search_graph(query="Service", limit=50)
```

## Cross-Project Type Mapping

When projects are linked, classes/interfaces with the same name get `MAPS_TO` edges automatically:
```
# Find the backend equivalent of a frontend type
get_symbol_details(name="TravelRequest")  → relationships.maps_to: [{name: "TravelRequest", source_project: "backend"}]
```
