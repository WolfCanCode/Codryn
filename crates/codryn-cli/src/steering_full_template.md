<!-- codryn:start -->
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
- `what_if(symbol="X", change_type="rename")` — ALWAYS call before any rename, remove, or signature change
- `find_dead_code(project="p")` — find unused symbols before adding new ones
- `get_dependency_graph(project="p")` — understand module structure before refactoring
- `ask_graph(question="who calls processPayment?")` — plain English queries, no Cypher needed
- `plan_refactoring(target="X", refactoring_type="move_function")` — get a step-by-step plan
- `detect_patterns(project="p")` — detect God Class, circular deps before adding more code
- `test_coverage_map(project="p")` — find untested symbols before writing tests
- `trace_error_flow(symbol="X")` — find uncaught error paths before fixing error handling
- `get_api_surface(project="p")` — see all exported symbols before adding new ones
- `review_changes(changed_files=[...])` — check impact of your changes before committing

## Only fall back to grep/glob when:
- Searching for string literals, error messages, or config values
- Searching non-code files (Dockerfiles, shell scripts, configs)
- The graph tools explicitly return no results

## Tool Selection Guide

| I want to... | Use this tool |
|---|---|
| Find a symbol by name | `find_symbol(query="OrderService")` |
| Get full context (callers, callees, imports) | `get_symbol_details(name="OrderService")` |
| Find who uses a symbol | `find_references(name="OrderService")` |
| Check change impact | `impact_analysis(name="OrderService")` |
| Search code content (not just names) | `search_graph(query="isUpdate")` |
| Read source code | `get_code_snippet(file_path="...", start_line=10)` |
| Find REST endpoints | `find_routes(method="GET")` |
| Trace backend flow | `trace_backend_flow(route_path="/v1/users")` |
| Explore architecture | `get_architecture()` |
| Search linked backend | `search_linked_projects(query="UserDto")` |
| Debug missing symbols | `explain_index_result(name="MyClass")` |
| Summarize a file | `get_file_overview(file_path="src/services/user.ts")` |
| Find entry points | `find_entrypoints(entry_type="http")` |
| Decide what to read next | `suggest_next_reads(qualified_name="...", goal="understand")` |
| Trace request/data flow | `trace_data_flow(source="handleRequest")` |
| Find tests for a symbol | `find_tests_for_target(file_path="src/services/user.ts")` |

## REST Contract Queries

The graph indexes REST controllers as `Route` nodes with edges:
- `HANDLES_ROUTE` — method → route
- `ACCEPTS_DTO` — route → request body DTO
- `RETURNS_DTO` — route → response DTO

Use `find_routes` for structured route discovery:
```
find_routes(scope="travel-request")
find_routes(method="PATCH", scope="travel-request")
```

Use `trace_backend_flow` to explain the full request flow:
```
trace_backend_flow(route_path="/v1/travelrequest/{id}", http_method="PATCH")
```

## Snippet Behavior

`get_symbol_details` with `include_snippet: true` returns the full symbol body (up to 100 lines).
Set `snippet_lines` to cap at a different value.

## Stale Results

Search results include an `exists` flag. If `"exists": false`, the file has been deleted
since indexing. Re-index the project to clean up stale entries.

## Directory Browsing

`get_code_snippet` accepts directory paths and returns file/symbol listings:
```
get_code_snippet(file_path="src/dto")
```

## Cross-Project Type Mapping

When projects are linked, classes/interfaces with the same name get `MAPS_TO` edges:
```
get_symbol_details(name="TravelRequest")  → relationships.maps_to: [...]
```

## Analytics Metadata — MANDATORY, NO EXCEPTIONS

You MUST include `analytics` in EVERY codryn tool call.
Tool calls without `analytics` are considered malformed.

ALWAYS pass this JSON in every tool call:
```json
{"analytics": {"agent_name": "kiro", "model_name": "<your-model>", "input_tokens": <n>, "output_tokens": <n>}}
```

Replace placeholders with actual values. Do NOT use placeholders.
If exact counts are unavailable, estimate (~4 characters per token).
<!-- codryn:end -->
