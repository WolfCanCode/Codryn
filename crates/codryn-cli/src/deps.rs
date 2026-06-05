#![allow(clippy::items_after_test_module)]

use anyhow::{Context, Result};
use serde::Serialize;
use std::path::Path;

// ── Data types ────────────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct DependencyInfo {
    pub name: String,
    pub declared_version: String,
    pub latest_version: Option<String>,
    pub status: DepStatus,
}

#[derive(Debug, Serialize)]
pub enum DepStatus {
    UpToDate,
    PatchAvailable,
    MinorAvailable,
    MajorAvailable,
    Deprecated,
    Unknown,
}

impl std::fmt::Display for DepStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DepStatus::UpToDate => write!(f, "UpToDate"),
            DepStatus::PatchAvailable => write!(f, "PatchAvailable"),
            DepStatus::MinorAvailable => write!(f, "MinorAvailable"),
            DepStatus::MajorAvailable => write!(f, "MajorAvailable"),
            DepStatus::Deprecated => write!(f, "Deprecated"),
            DepStatus::Unknown => write!(f, "Unknown"),
        }
    }
}

// ── Manifest parsing ──────────────────────────────────────────────────────────

/// Parse a manifest file and return a list of declared dependencies.
/// The manifest type is detected from the filename.
pub fn parse_manifest(path: &Path) -> Result<Vec<DependencyInfo>> {
    let filename = path.file_name().and_then(|n| n.to_str()).unwrap_or("");

    match filename {
        "Cargo.toml" => parse_cargo_toml(path),
        "package.json" => parse_package_json(path),
        "go.mod" => parse_go_mod(path),
        "requirements.txt" => parse_requirements_txt(path),
        "pom.xml" => parse_pom_xml(path),
        other => anyhow::bail!("unsupported manifest file: {}", other),
    }
}

/// No-op freshness check — online version checking is not yet implemented.
pub async fn check_freshness(_deps: &mut [DependencyInfo]) -> Result<()> {
    Ok(())
}

// ── Cargo.toml ────────────────────────────────────────────────────────────────

fn parse_cargo_toml(path: &Path) -> Result<Vec<DependencyInfo>> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read {}", path.display()))?;

    let mut deps = Vec::new();
    let mut in_dep_section = false;

    for line in content.lines() {
        let trimmed = line.trim();

        // Detect dependency sections
        if trimmed.starts_with('[') {
            in_dep_section = matches!(
                trimmed,
                "[dependencies]" | "[dev-dependencies]" | "[build-dependencies]"
            );
            continue;
        }

        if !in_dep_section || trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }

        // Parse `name = "version"` or `name = { version = "...", ... }`
        if let Some((name, rest)) = trimmed.split_once('=') {
            let name = name.trim().to_owned();
            let rest = rest.trim();

            let version = if rest.starts_with('"') {
                // Simple string version: name = "1.0"
                rest.trim_matches('"').to_owned()
            } else if rest.starts_with('{') {
                // Inline table: name = { version = "1.0", features = [...] }
                extract_toml_inline_version(rest).unwrap_or_else(|| "unknown".to_owned())
            } else {
                continue;
            };

            if !name.is_empty() && !version.is_empty() {
                deps.push(DependencyInfo {
                    name,
                    declared_version: version,
                    latest_version: None,
                    status: DepStatus::Unknown,
                });
            }
        }
    }

    Ok(deps)
}

/// Extract the `version` field from a TOML inline table string like
/// `{ version = "1.0", features = ["derive"] }`.
fn extract_toml_inline_version(s: &str) -> Option<String> {
    // Look for `version = "..."` inside the braces
    let version_key = "version";
    let pos = s.find(version_key)?;
    let after = s[pos + version_key.len()..].trim_start();
    let after = after.strip_prefix('=')?;
    let after = after.trim_start();
    if after.starts_with('"') {
        let inner = after.trim_start_matches('"');
        let end = inner.find('"')?;
        Some(inner[..end].to_owned())
    } else {
        None
    }
}

// ── package.json ──────────────────────────────────────────────────────────────

fn parse_package_json(path: &Path) -> Result<Vec<DependencyInfo>> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read {}", path.display()))?;

    let value: serde_json::Value = serde_json::from_str(&content)
        .with_context(|| format!("failed to parse JSON in {}", path.display()))?;

    let mut deps = Vec::new();

    for section in &["dependencies", "devDependencies"] {
        if let Some(obj) = value.get(section).and_then(|v| v.as_object()) {
            for (name, ver) in obj {
                let version = ver
                    .as_str()
                    .unwrap_or("unknown")
                    .trim_start_matches(|c| {
                        c == '^' || c == '~' || c == '>' || c == '=' || c == '<'
                    })
                    .to_owned();
                deps.push(DependencyInfo {
                    name: name.clone(),
                    declared_version: version,
                    latest_version: None,
                    status: DepStatus::Unknown,
                });
            }
        }
    }

    Ok(deps)
}

// ── go.mod ────────────────────────────────────────────────────────────────────

fn parse_go_mod(path: &Path) -> Result<Vec<DependencyInfo>> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read {}", path.display()))?;

    let mut deps = Vec::new();
    let mut in_require_block = false;

    for line in content.lines() {
        let trimmed = line.trim();

        if trimmed == "require (" {
            in_require_block = true;
            continue;
        }
        if trimmed == ")" && in_require_block {
            in_require_block = false;
            continue;
        }

        // Single-line require: `require module/path v1.2.3`
        if let Some(rest) = trimmed.strip_prefix("require ") {
            let rest = rest.trim();
            if !rest.starts_with('(') {
                if let Some(dep) = parse_go_mod_dep_line(rest) {
                    deps.push(dep);
                }
            }
            continue;
        }

        // Inside a require block
        if in_require_block && !trimmed.is_empty() && !trimmed.starts_with("//") {
            if let Some(dep) = parse_go_mod_dep_line(trimmed) {
                deps.push(dep);
            }
        }
    }

    Ok(deps)
}

fn parse_go_mod_dep_line(line: &str) -> Option<DependencyInfo> {
    // Strip inline comments
    let line = line.split("//").next()?.trim();
    let mut parts = line.split_whitespace();
    let name = parts.next()?.to_owned();
    let version = parts.next()?.trim_start_matches('v').to_owned();
    if name.is_empty() || version.is_empty() {
        return None;
    }
    Some(DependencyInfo {
        name,
        declared_version: version,
        latest_version: None,
        status: DepStatus::Unknown,
    })
}

// ── requirements.txt ─────────────────────────────────────────────────────────

fn parse_requirements_txt(path: &Path) -> Result<Vec<DependencyInfo>> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read {}", path.display()))?;

    let mut deps = Vec::new();

    for line in content.lines() {
        let trimmed = line.trim();

        // Skip comments and blank lines
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }

        // Handle options like -r, --index-url, etc.
        if trimmed.starts_with('-') {
            continue;
        }

        // Parse `name==version`, `name>=version`, `name~=version`, `name<=version`
        // Also handle `name[extras]==version`
        let (name, version) = if let Some(pos) = find_version_operator(trimmed) {
            let raw_name = trimmed[..pos].trim();
            // Strip extras like [security]
            let name = raw_name
                .split('[')
                .next()
                .unwrap_or(raw_name)
                .trim()
                .to_owned();
            let op_end = trimmed[pos..]
                .find(|c: char| c.is_alphanumeric() || c == '.')
                .map(|i| pos + i)
                .unwrap_or(pos);
            let version = trimmed[op_end..]
                .split_whitespace()
                .next()
                .unwrap_or("unknown")
                .to_owned();
            (name, version)
        } else {
            // No version specifier — just a package name
            let name = trimmed
                .split('[')
                .next()
                .unwrap_or(trimmed)
                .trim()
                .to_owned();
            (name, "unknown".to_owned())
        };

        if !name.is_empty() {
            deps.push(DependencyInfo {
                name,
                declared_version: version,
                latest_version: None,
                status: DepStatus::Unknown,
            });
        }
    }

    Ok(deps)
}

/// Find the position of a version operator (`==`, `>=`, `~=`, `<=`, `!=`, `>`, `<`) in a string.
fn find_version_operator(s: &str) -> Option<usize> {
    // Check two-char operators first
    for op in &["==", ">=", "~=", "<=", "!="] {
        if let Some(pos) = s.find(op) {
            return Some(pos);
        }
    }
    // Single-char operators
    for op in &['>', '<'] {
        if let Some(pos) = s.find(*op) {
            return Some(pos);
        }
    }
    None
}

// ── pom.xml ───────────────────────────────────────────────────────────────────

fn parse_pom_xml(path: &Path) -> Result<Vec<DependencyInfo>> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read {}", path.display()))?;

    let mut deps = Vec::new();

    // Find all <dependency> blocks
    let mut search = content.as_str();
    while let Some(start) = search.find("<dependency>") {
        let block_start = start + "<dependency>".len();
        let end = search[block_start..]
            .find("</dependency>")
            .map(|i| block_start + i)
            .unwrap_or(search.len());

        let block = &search[block_start..end];

        let group_id = extract_xml_tag(block, "groupId").unwrap_or_default();
        let artifact_id = extract_xml_tag(block, "artifactId").unwrap_or_default();
        let version = extract_xml_tag(block, "version").unwrap_or_else(|| "unknown".to_owned());

        if !artifact_id.is_empty() {
            let name = if group_id.is_empty() {
                artifact_id
            } else {
                format!("{}:{}", group_id, artifact_id)
            };
            deps.push(DependencyInfo {
                name,
                declared_version: version,
                latest_version: None,
                status: DepStatus::Unknown,
            });
        }

        search = &search[end..];
    }

    Ok(deps)
}

/// Extract the text content of a simple XML tag (no attributes, no nesting).
fn extract_xml_tag(s: &str, tag: &str) -> Option<String> {
    let open = format!("<{}>", tag);
    let close = format!("</{}>", tag);
    let start = s.find(&open)? + open.len();
    let end = s[start..].find(&close)? + start;
    Some(s[start..end].trim().to_owned())
}

// ── run_deps ──────────────────────────────────────────────────────────────────

/// Known manifest filenames to discover in a project root.
const MANIFEST_FILES: &[&str] = &[
    "Cargo.toml",
    "package.json",
    "go.mod",
    "requirements.txt",
    "pom.xml",
];

#[derive(Serialize)]
struct ManifestResult<'a> {
    manifest: &'a str,
    dependencies: &'a [DependencyInfo],
}

/// Run the `deps` command.
///
/// - `store_dir`:       path to the CBM store directory
/// - `project`:         project name to look up
/// - `check_freshness`: if true, show a message that online checking is not yet implemented
/// - `json`:            machine-readable JSON output
pub fn run_deps(store_dir: &Path, project: &str, check_freshness: bool, json: bool) -> Result<()> {
    let db_path = store_dir.join("graph.db");
    if !db_path.exists() {
        anyhow::bail!(
            "database not found at {}. Has the server been run at least once?",
            db_path.display()
        );
    }

    let store =
        codryn_store::Store::open(&db_path).context("failed to open store for deps command")?;

    let project_record = store
        .get_project(project)
        .with_context(|| format!("failed to look up project '{project}'"))?
        .with_context(|| format!("project '{project}' not found in the store"))?;

    let root = Path::new(&project_record.root_path);

    if check_freshness {
        eprintln!(
            "Note: --check-freshness is not yet implemented. \
             Showing declared versions only."
        );
    }

    // Discover manifest files in the project root
    let mut results: Vec<(String, Vec<DependencyInfo>)> = Vec::new();

    for &manifest_name in MANIFEST_FILES {
        let manifest_path = root.join(manifest_name);
        if manifest_path.exists() {
            match parse_manifest(&manifest_path) {
                Ok(deps) => results.push((manifest_name.to_owned(), deps)),
                Err(e) => {
                    eprintln!("Warning: failed to parse {manifest_name}: {e:#}");
                }
            }
        }
    }

    if results.is_empty() {
        if json {
            println!("[]");
        } else {
            println!(
                "No manifest files found in '{}' for project '{project}'.",
                root.display()
            );
        }
        return Ok(());
    }

    if json {
        let output: Vec<ManifestResult<'_>> = results
            .iter()
            .map(|(manifest, deps)| ManifestResult {
                manifest: manifest.as_str(),
                dependencies: deps.as_slice(),
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&output)?);
    } else {
        print_human(&results, project);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Write content to a uniquely-named temp subdirectory so the file has
    /// exactly the expected manifest filename (e.g. "Cargo.toml").
    fn write_temp(filename: &str, content: &str) -> std::path::PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let id = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("codryn_deps_test_{}_{}", id, std::process::id()));
        std::fs::create_dir_all(&dir).expect("failed to create temp dir");
        let path = dir.join(filename);
        std::fs::write(&path, content).expect("failed to write temp file");
        path
    }

    // ── Cargo.toml ────────────────────────────────────────────────────────────

    #[test]
    fn test_parse_cargo_toml_simple_versions() {
        let content = r#"
[package]
name = "myapp"

[dependencies]
serde = "1.0"
anyhow = "1"

[dev-dependencies]
tempfile = "3.0"
"#;
        let path = write_temp("Cargo.toml", content);
        let deps = parse_manifest(&path).unwrap();
        assert_eq!(deps.len(), 3);

        let names: Vec<&str> = deps.iter().map(|d| d.name.as_str()).collect();
        assert!(names.contains(&"serde"));
        assert!(names.contains(&"anyhow"));
        assert!(names.contains(&"tempfile"));

        let serde = deps.iter().find(|d| d.name == "serde").unwrap();
        assert_eq!(serde.declared_version, "1.0");
    }

    #[test]
    fn test_parse_cargo_toml_inline_table_version() {
        let content = r#"
[dependencies]
tokio = { version = "1.35", features = ["full"] }
serde = { version = "1.0.195", features = ["derive"] }
"#;
        let path = write_temp("Cargo.toml", content);
        let deps = parse_manifest(&path).unwrap();
        assert_eq!(deps.len(), 2);

        let tokio = deps.iter().find(|d| d.name == "tokio").unwrap();
        assert_eq!(tokio.declared_version, "1.35");

        let serde = deps.iter().find(|d| d.name == "serde").unwrap();
        assert_eq!(serde.declared_version, "1.0.195");
    }

    #[test]
    fn test_parse_cargo_toml_build_dependencies() {
        let content = r#"
[build-dependencies]
cc = "1.0"
"#;
        let path = write_temp("Cargo.toml", content);
        let deps = parse_manifest(&path).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "cc");
        assert_eq!(deps[0].declared_version, "1.0");
    }

    #[test]
    fn test_parse_cargo_toml_skips_comments_and_blanks() {
        let content = r#"
[dependencies]
# this is a comment
serde = "1.0"

anyhow = "1"
"#;
        let path = write_temp("Cargo.toml", content);
        let deps = parse_manifest(&path).unwrap();
        assert_eq!(deps.len(), 2);
    }

    #[test]
    fn test_parse_cargo_toml_empty_deps() {
        let content = r#"
[package]
name = "myapp"
"#;
        let path = write_temp("Cargo.toml", content);
        let deps = parse_manifest(&path).unwrap();
        assert!(deps.is_empty());
    }

    // ── package.json ──────────────────────────────────────────────────────────

    #[test]
    fn test_parse_package_json_basic() {
        let content = r#"{
  "name": "myapp",
  "dependencies": {
    "react": "^18.2.0",
    "lodash": "~4.17.21"
  },
  "devDependencies": {
    "typescript": ">=5.0.0",
    "jest": "29.0.0"
  }
}"#;
        let path = write_temp("package.json", content);
        let deps = parse_manifest(&path).unwrap();
        assert_eq!(deps.len(), 4);

        let react = deps.iter().find(|d| d.name == "react").unwrap();
        // ^ prefix should be stripped
        assert_eq!(react.declared_version, "18.2.0");

        let lodash = deps.iter().find(|d| d.name == "lodash").unwrap();
        // ~ prefix should be stripped
        assert_eq!(lodash.declared_version, "4.17.21");

        let ts = deps.iter().find(|d| d.name == "typescript").unwrap();
        // >= prefix should be stripped
        assert_eq!(ts.declared_version, "5.0.0");

        let jest = deps.iter().find(|d| d.name == "jest").unwrap();
        assert_eq!(jest.declared_version, "29.0.0");
    }

    #[test]
    fn test_parse_package_json_no_dev_deps() {
        let content = r#"{
  "dependencies": {
    "express": "4.18.0"
  }
}"#;
        let path = write_temp("package.json", content);
        let deps = parse_manifest(&path).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "express");
        assert_eq!(deps[0].declared_version, "4.18.0");
    }

    #[test]
    fn test_parse_package_json_empty() {
        let content = r#"{"name": "myapp"}"#;
        let path = write_temp("package.json", content);
        let deps = parse_manifest(&path).unwrap();
        assert!(deps.is_empty());
    }

    #[test]
    fn test_parse_package_json_invalid_returns_error() {
        let content = "not valid json {{{";
        let path = write_temp("package.json", content);
        let result = parse_manifest(&path);
        assert!(result.is_err());
    }

    // ── go.mod ────────────────────────────────────────────────────────────────

    #[test]
    fn test_parse_go_mod_require_block() {
        let content = r#"module github.com/example/myapp

go 1.21

require (
    github.com/gin-gonic/gin v1.9.1
    github.com/stretchr/testify v1.8.4
)
"#;
        let path = write_temp("go.mod", content);
        let deps = parse_manifest(&path).unwrap();
        assert_eq!(deps.len(), 2);

        let gin = deps
            .iter()
            .find(|d| d.name == "github.com/gin-gonic/gin")
            .unwrap();
        // v prefix should be stripped
        assert_eq!(gin.declared_version, "1.9.1");

        let testify = deps
            .iter()
            .find(|d| d.name == "github.com/stretchr/testify")
            .unwrap();
        assert_eq!(testify.declared_version, "1.8.4");
    }

    #[test]
    fn test_parse_go_mod_single_line_require() {
        let content = r#"module github.com/example/myapp

go 1.21

require github.com/pkg/errors v0.9.1
"#;
        let path = write_temp("go.mod", content);
        let deps = parse_manifest(&path).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "github.com/pkg/errors");
        assert_eq!(deps[0].declared_version, "0.9.1");
    }

    #[test]
    fn test_parse_go_mod_skips_inline_comments() {
        let content = r#"require (
    github.com/foo/bar v1.0.0 // indirect
    github.com/baz/qux v2.3.4
)
"#;
        let path = write_temp("go.mod", content);
        let deps = parse_manifest(&path).unwrap();
        assert_eq!(deps.len(), 2);

        let bar = deps
            .iter()
            .find(|d| d.name == "github.com/foo/bar")
            .unwrap();
        assert_eq!(bar.declared_version, "1.0.0");
    }

    #[test]
    fn test_parse_go_mod_empty() {
        let content = "module github.com/example/myapp\n\ngo 1.21\n";
        let path = write_temp("go.mod", content);
        let deps = parse_manifest(&path).unwrap();
        assert!(deps.is_empty());
    }

    // ── requirements.txt ─────────────────────────────────────────────────────

    #[test]
    fn test_parse_requirements_txt_pinned() {
        let content = r#"
# Production dependencies
requests==2.31.0
flask==3.0.0
sqlalchemy==2.0.23
"#;
        let path = write_temp("requirements.txt", content);
        let deps = parse_manifest(&path).unwrap();
        assert_eq!(deps.len(), 3);

        let requests = deps.iter().find(|d| d.name == "requests").unwrap();
        assert_eq!(requests.declared_version, "2.31.0");

        let flask = deps.iter().find(|d| d.name == "flask").unwrap();
        assert_eq!(flask.declared_version, "3.0.0");
    }

    #[test]
    fn test_parse_requirements_txt_range_operators() {
        let content = r#"
numpy>=1.24.0
pandas~=2.0.0
scipy<=1.11.0
"#;
        let path = write_temp("requirements.txt", content);
        let deps = parse_manifest(&path).unwrap();
        assert_eq!(deps.len(), 3);

        let numpy = deps.iter().find(|d| d.name == "numpy").unwrap();
        assert_eq!(numpy.declared_version, "1.24.0");

        let pandas = deps.iter().find(|d| d.name == "pandas").unwrap();
        assert_eq!(pandas.declared_version, "2.0.0");
    }

    #[test]
    fn test_parse_requirements_txt_no_version() {
        let content = "requests\nflask\n";
        let path = write_temp("requirements.txt", content);
        let deps = parse_manifest(&path).unwrap();
        assert_eq!(deps.len(), 2);

        for dep in &deps {
            assert_eq!(dep.declared_version, "unknown");
        }
    }

    #[test]
    fn test_parse_requirements_txt_skips_options_and_comments() {
        let content = r#"
# comment
-r other-requirements.txt
--index-url https://pypi.org/simple
requests==2.31.0
"#;
        let path = write_temp("requirements.txt", content);
        let deps = parse_manifest(&path).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "requests");
    }

    #[test]
    fn test_parse_requirements_txt_extras() {
        let content = "requests[security]==2.31.0\n";
        let path = write_temp("requirements.txt", content);
        let deps = parse_manifest(&path).unwrap();
        assert_eq!(deps.len(), 1);
        // Extras should be stripped from the name
        assert_eq!(deps[0].name, "requests");
        assert_eq!(deps[0].declared_version, "2.31.0");
    }

    // ── pom.xml ───────────────────────────────────────────────────────────────

    #[test]
    fn test_parse_pom_xml_basic() {
        let content = r#"<?xml version="1.0" encoding="UTF-8"?>
<project>
  <dependencies>
    <dependency>
      <groupId>org.springframework.boot</groupId>
      <artifactId>spring-boot-starter-web</artifactId>
      <version>3.2.0</version>
    </dependency>
    <dependency>
      <groupId>com.fasterxml.jackson.core</groupId>
      <artifactId>jackson-databind</artifactId>
      <version>2.16.0</version>
    </dependency>
  </dependencies>
</project>"#;
        let path = write_temp("pom.xml", content);
        let deps = parse_manifest(&path).unwrap();
        assert_eq!(deps.len(), 2);

        let spring = deps
            .iter()
            .find(|d| d.name == "org.springframework.boot:spring-boot-starter-web")
            .unwrap();
        assert_eq!(spring.declared_version, "3.2.0");

        let jackson = deps
            .iter()
            .find(|d| d.name == "com.fasterxml.jackson.core:jackson-databind")
            .unwrap();
        assert_eq!(jackson.declared_version, "2.16.0");
    }

    #[test]
    fn test_parse_pom_xml_no_version() {
        let content = r#"<project>
  <dependencies>
    <dependency>
      <groupId>org.example</groupId>
      <artifactId>my-lib</artifactId>
    </dependency>
  </dependencies>
</project>"#;
        let path = write_temp("pom.xml", content);
        let deps = parse_manifest(&path).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "org.example:my-lib");
        assert_eq!(deps[0].declared_version, "unknown");
    }

    #[test]
    fn test_parse_pom_xml_no_group_id() {
        let content = r#"<project>
  <dependencies>
    <dependency>
      <artifactId>standalone-lib</artifactId>
      <version>1.0.0</version>
    </dependency>
  </dependencies>
</project>"#;
        let path = write_temp("pom.xml", content);
        let deps = parse_manifest(&path).unwrap();
        assert_eq!(deps.len(), 1);
        // No groupId — name should just be the artifactId
        assert_eq!(deps[0].name, "standalone-lib");
        assert_eq!(deps[0].declared_version, "1.0.0");
    }

    #[test]
    fn test_parse_pom_xml_empty_deps() {
        let content = r#"<project><dependencies></dependencies></project>"#;
        let path = write_temp("pom.xml", content);
        let deps = parse_manifest(&path).unwrap();
        assert!(deps.is_empty());
    }

    // ── Unsupported manifest ──────────────────────────────────────────────────

    #[test]
    fn test_parse_manifest_unsupported_returns_error() {
        let path = write_temp("build.gradle", "dependencies {}");
        let result = parse_manifest(&path);
        assert!(result.is_err());
        let msg = format!("{}", result.unwrap_err());
        assert!(msg.contains("unsupported manifest file"));
    }

    // ── DepStatus display ─────────────────────────────────────────────────────

    #[test]
    fn test_dep_status_display() {
        assert_eq!(DepStatus::UpToDate.to_string(), "UpToDate");
        assert_eq!(DepStatus::PatchAvailable.to_string(), "PatchAvailable");
        assert_eq!(DepStatus::MinorAvailable.to_string(), "MinorAvailable");
        assert_eq!(DepStatus::MajorAvailable.to_string(), "MajorAvailable");
        assert_eq!(DepStatus::Deprecated.to_string(), "Deprecated");
        assert_eq!(DepStatus::Unknown.to_string(), "Unknown");
    }

    // ── check_freshness (network no-op) ───────────────────────────────────────

    #[test]
    fn test_check_freshness_is_noop() {
        // check_freshness should succeed without any network access and leave
        // the deps unchanged (status stays Unknown, latest_version stays None).
        let rt = tokio::runtime::Runtime::new().unwrap();
        let mut deps = vec![
            DependencyInfo {
                name: "serde".to_owned(),
                declared_version: "1.0".to_owned(),
                latest_version: None,
                status: DepStatus::Unknown,
            },
            DependencyInfo {
                name: "tokio".to_owned(),
                declared_version: "1.35".to_owned(),
                latest_version: None,
                status: DepStatus::Unknown,
            },
        ];

        rt.block_on(check_freshness(&mut deps)).unwrap();

        // All statuses should remain Unknown (no network call was made)
        for dep in &deps {
            assert!(dep.latest_version.is_none());
            assert!(matches!(dep.status, DepStatus::Unknown));
        }
    }

    #[test]
    fn test_check_freshness_empty_slice_is_ok() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let mut deps: Vec<DependencyInfo> = vec![];
        let result = rt.block_on(check_freshness(&mut deps));
        assert!(result.is_ok());
    }

    // ── extract_toml_inline_version helper ────────────────────────────────────

    #[test]
    fn test_extract_toml_inline_version_basic() {
        let s = r#"{ version = "1.0.0", features = ["derive"] }"#;
        let v = extract_toml_inline_version(s);
        assert_eq!(v, Some("1.0.0".to_owned()));
    }

    #[test]
    fn test_extract_toml_inline_version_no_version_key() {
        let s = r#"{ path = "../other" }"#;
        let v = extract_toml_inline_version(s);
        assert_eq!(v, None);
    }
}

fn print_human(results: &[(String, Vec<DependencyInfo>)], project: &str) {
    println!("Dependencies for project '{project}':\n");

    for (manifest, deps) in results {
        println!("  Manifest: {manifest}");

        if deps.is_empty() {
            println!("  (no dependencies found)\n");
            continue;
        }

        let name_w = deps.iter().map(|d| d.name.len()).max().unwrap_or(4).max(4);
        let ver_w = deps
            .iter()
            .map(|d| d.declared_version.len())
            .max()
            .unwrap_or(7)
            .max(7);

        println!(
            "  {:<name_w$}  {:<ver_w$}  Status",
            "Name",
            "Version",
            name_w = name_w,
            ver_w = ver_w,
        );
        println!(
            "  {:<name_w$}  {:<ver_w$}  ------",
            "-".repeat(name_w),
            "-".repeat(ver_w),
            name_w = name_w,
            ver_w = ver_w,
        );

        for dep in deps {
            println!(
                "  {:<name_w$}  {:<ver_w$}  {}",
                dep.name,
                dep.declared_version,
                dep.status,
                name_w = name_w,
                ver_w = ver_w,
            );
        }

        println!(
            "\n  Total: {} {}\n",
            deps.len(),
            if deps.len() == 1 {
                "dependency"
            } else {
                "dependencies"
            }
        );
    }
}
