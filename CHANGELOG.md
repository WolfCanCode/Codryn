# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [1.4.0] - 2026-04-25

- Initial release


### Added

- Add Codryn-native full pipeline/walker parity: tree-sitter extraction crate, broad language mappings, Go/Vue adapters, type registry, package/import maps, semantic enrichment, similarity detection, and config/event/cross-repo passes.
- Add richer DevOps indexing for GitLab CI, GitHub Actions, CircleCI, Azure Pipelines, Bitbucket Pipelines, Jenkinsfile-style jobs, Kubernetes, Kustomize, Docker, Helm, and Terraform resources.
- Add compressed code-content storage and BM25-ranked search support in the Codryn store.

### Changed

- Keep Codryn-only route/framework improvements while upgrading the backend indexing flow to the linked implementation’s broader pass set.

## [1.3.0] - 2026-04-25

- Initial release


### Added

- Add Codryn-native pipeline and infrastructure service support for CI/CD DAGs and infrastructure resources.
- Add MCP tools `find_pipelines` and `find_infrastructure`.
- Add web API endpoints for analytics detail, pipeline DAGs, and infrastructure resources.

### Changed

- Extend analytics records with request and response body fields for detail views while preserving existing summary analytics.

## [1.2.0] - 2026-04-20

- Initial release

## [1.1.0] - 2026-04-20

- Initial release


## [1.0.0] - 2026-04-20

### Features

- Initialize codryn project with workspace structure and installation scripts (`8714934`)
- Enhance installation experience for codryn (`f09beb0`)
- Enhance codryn installation script with detailed user guidance (`411abc5`)

### Bug Fixes

- Use `claude mcp add --scope user` for Claude Code MCP registration instead of legacy `mcp_servers.json` (`8b0cebe`)
- Ensure cleanup guard functions correctly in installation script (`1271931`)
- Improve spinner output in installation script (`628241c`)

### Refactoring

- Update installation script for improved compatibility and functionality (`0f1b15a`)
- Enhance installation scripts and improve code formatting (`9757aa6`)

### Documentation

- Add graph page enhancement design spec and implementation plan (`489ef64`)
- Update logo to logo-2, add one-line install command (`62d8394`)

### Style

- Apply `cargo fmt` formatting (`e5677b3`)

[Unreleased]: https://github.com/WolfCanCode/Codryn/compare/v1.4.0...HEAD
[1.4.0]: https://github.com/WolfCanCode/Codryn/compare/v1.3.0...v1.4.0
[1.3.0]: https://github.com/WolfCanCode/Codryn/compare/v1.2.0...v1.3.0
[1.2.0]: https://github.com/WolfCanCode/Codryn/compare/v1.1.0...v1.2.0
[1.1.0]: https://github.com/WolfCanCode/Codryn/compare/v1.0.0...v1.1.0
[1.0.0]: https://github.com/WolfCanCode/Codryn/releases/tag/v1.0.0
