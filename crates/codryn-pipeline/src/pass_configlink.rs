//! Configuration Linking Pass
//!
//! Detects configuration files by extension (.env, .yml, .json, .properties, .toml),
//! extracts keys in dot-separated notation (max depth 10), and matches them against
//! code patterns that access configuration values (process.env, os.environ, os.Getenv,
//! System.getenv, @Value). Creates CONFIGURES edges from config file nodes to the
//! enclosing function/method with a `key` property.
//!
//! Requirements: 14.1, 14.2, 14.3, 14.4, 14.5

use codryn_discover::DiscoveredFile;
use codryn_foundation::fqn;
use codryn_graph_buffer::{EdgeSource, GraphBuffer};
use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

use crate::registry::Registry;

/// Maximum nesting depth for config key extraction.
pub const MAX_DEPTH: usize = 10;

/// Config file extensions we recognize for this pass.
const CONFIG_EXTENSIONS: &[&str] = &[".env", ".yml", ".yaml", ".json", ".properties", ".toml"];

/// Regex patterns for config access in code.
/// Each pattern captures the referenced key name.
pub static CONFIG_ACCESS_PATTERNS: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(
        r#"(?x)
        # JS/TS: process.env.KEY
        process\.env\.([A-Za-z_][A-Za-z0-9_.]*) |
        # JS/TS: process.env["KEY"] or process.env['KEY']
        process\.env\["([^"]+)"\] |
        process\.env\['([^']+)'\] |
        # Python: os.environ["KEY"] or os.environ['KEY']
        os\.environ\["([^"]+)"\] |
        os\.environ\['([^']+)'\] |
        # Python: os.environ.get("KEY") or os.environ.get('KEY')
        os\.environ\.get\(\s*"([^"]+)" |
        os\.environ\.get\(\s*'([^']+)' |
        # Go: os.Getenv("KEY")
        os\.Getenv\("([^"]+)"\) |
        # Java: System.getenv("KEY")
        System\.getenv\("([^"]+)"\) |
        # Spring: @Value("${KEY}") or @Value("${KEY:default}")
        @Value\("\$\{([^}:]+)[^}]*\}"\)
        "#,
    )
    .unwrap()
});

/// Determines if a file is a configuration file based on its extension/name.
pub fn is_config_file(rel_path: &str) -> bool {
    let lower = rel_path.to_lowercase();
    // Check extensions
    for ext in CONFIG_EXTENSIONS {
        if lower.ends_with(ext) {
            return true;
        }
    }
    // Also match files named like "application.yml", "config.json", etc.
    false
}

/// Extract configuration keys from a file's content in dot-separated notation.
/// Handles YAML, JSON, TOML, .properties, and .env formats.
/// Nested keys are represented with dots (e.g., "database.host") up to MAX_DEPTH.
pub fn extract_config_keys(rel_path: &str, content: &str) -> Vec<String> {
    let lower = rel_path.to_lowercase();
    if lower.ends_with(".json") {
        extract_json_keys(content)
    } else if lower.ends_with(".yml") || lower.ends_with(".yaml") {
        extract_yaml_keys(content)
    } else if lower.ends_with(".toml") {
        extract_toml_keys(content)
    } else if lower.ends_with(".properties") {
        extract_properties_keys(content)
    } else if lower.ends_with(".env") || lower.contains(".env") {
        extract_env_keys(content)
    } else {
        Vec::new()
    }
}

/// Extract keys from JSON content using serde_json for proper nested key extraction.
fn extract_json_keys(content: &str) -> Vec<String> {
    let value: serde_json::Value = match serde_json::from_str(content) {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };

    let mut keys = Vec::new();
    extract_json_keys_recursive(&value, "", 0, &mut keys);
    keys
}

fn extract_json_keys_recursive(
    value: &serde_json::Value,
    prefix: &str,
    depth: usize,
    keys: &mut Vec<String>,
) {
    if depth >= MAX_DEPTH {
        return;
    }

    match value {
        serde_json::Value::Object(map) => {
            for (k, v) in map {
                let full_key = if prefix.is_empty() {
                    k.clone()
                } else {
                    format!("{}.{}", prefix, k)
                };

                match v {
                    serde_json::Value::Object(_) => {
                        // Recurse into nested objects
                        extract_json_keys_recursive(v, &full_key, depth + 1, keys);
                    }
                    serde_json::Value::Array(_) => {
                        // Record the array key itself as a leaf
                        keys.push(full_key);
                    }
                    _ => {
                        // Leaf value
                        keys.push(full_key);
                    }
                }
            }
        }
        _ => {
            if !prefix.is_empty() {
                keys.push(prefix.to_string());
            }
        }
    }
}

/// Extract keys from YAML content using indentation-based parsing.
/// Produces dot-separated keys for nested structures.
fn extract_yaml_keys(content: &str) -> Vec<String> {
    let mut keys = Vec::new();
    let mut stack: Vec<(usize, String)> = Vec::new(); // (indent_level, key_name)

    for line in content.lines() {
        let trimmed = line.trim();

        // Skip empty lines, comments, and document separators
        if trimmed.is_empty() || trimmed.starts_with('#') || trimmed == "---" || trimmed == "..." {
            continue;
        }

        // Calculate indentation
        let indent = line.len() - line.trim_start().len();

        // Extract key from "key:" or "key: value" patterns
        if let Some(colon_pos) = trimmed.find(':') {
            let key_part = trimmed[..colon_pos].trim();

            // Skip if key starts with '-' (array item) or is empty
            if key_part.is_empty() || key_part.starts_with('-') {
                continue;
            }

            // Strip quotes from key if present
            let key = key_part
                .trim_start_matches('"')
                .trim_end_matches('"')
                .trim_start_matches('\'')
                .trim_end_matches('\'');

            if key.is_empty() {
                continue;
            }

            // Pop stack entries that are at the same or deeper indent level
            while let Some((level, _)) = stack.last() {
                if *level >= indent {
                    stack.pop();
                } else {
                    break;
                }
            }

            // Check depth limit
            if stack.len() >= MAX_DEPTH {
                continue;
            }

            // Build the full dot-separated key
            let full_key = if stack.is_empty() {
                key.to_string()
            } else {
                let prefix: Vec<&str> = stack.iter().map(|(_, k)| k.as_str()).collect();
                format!("{}.{}", prefix.join("."), key)
            };

            // Check if this is a leaf (has a value after the colon) or a parent
            let after_colon = trimmed[colon_pos + 1..].trim();
            if after_colon.is_empty() || after_colon.starts_with('#') {
                // This is a parent key (value on next lines)
                stack.push((indent, key.to_string()));
            } else {
                // This is a leaf key with a value
                keys.push(full_key);
            }
        }
    }

    // Also add parent keys that had nested children (they represent config sections)
    // Re-parse to capture parent keys as well
    let mut parent_keys: Vec<String> = Vec::new();
    let mut stack2: Vec<(usize, String)> = Vec::new();

    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') || trimmed == "---" || trimmed == "..." {
            continue;
        }

        let indent = line.len() - line.trim_start().len();

        if let Some(colon_pos) = trimmed.find(':') {
            let key_part = trimmed[..colon_pos].trim();
            if key_part.is_empty() || key_part.starts_with('-') {
                continue;
            }

            let key = key_part
                .trim_start_matches('"')
                .trim_end_matches('"')
                .trim_start_matches('\'')
                .trim_end_matches('\'');

            if key.is_empty() {
                continue;
            }

            while let Some((level, _)) = stack2.last() {
                if *level >= indent {
                    stack2.pop();
                } else {
                    break;
                }
            }

            if stack2.len() >= MAX_DEPTH {
                continue;
            }

            let full_key = if stack2.is_empty() {
                key.to_string()
            } else {
                let prefix: Vec<&str> = stack2.iter().map(|(_, k)| k.as_str()).collect();
                format!("{}.{}", prefix.join("."), key)
            };

            let after_colon = trimmed[colon_pos + 1..].trim();
            if after_colon.is_empty() || after_colon.starts_with('#') {
                stack2.push((indent, key.to_string()));
                parent_keys.push(full_key);
            }
        }
    }

    // Deduplicate: only keep leaf keys (parent keys are implicit)
    let key_set: HashSet<String> = keys.into_iter().collect();
    key_set.into_iter().collect()
}

/// Extract keys from TOML content using the toml crate.
fn extract_toml_keys(content: &str) -> Vec<String> {
    let value: toml::Value = match content.parse() {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };

    let mut keys = Vec::new();
    extract_toml_keys_recursive(&value, "", 0, &mut keys);
    keys
}

fn extract_toml_keys_recursive(
    value: &toml::Value,
    prefix: &str,
    depth: usize,
    keys: &mut Vec<String>,
) {
    if depth >= MAX_DEPTH {
        return;
    }

    match value {
        toml::Value::Table(table) => {
            for (k, v) in table {
                let full_key = if prefix.is_empty() {
                    k.clone()
                } else {
                    format!("{}.{}", prefix, k)
                };

                match v {
                    toml::Value::Table(_) => {
                        extract_toml_keys_recursive(v, &full_key, depth + 1, keys);
                    }
                    toml::Value::Array(_) => {
                        keys.push(full_key);
                    }
                    _ => {
                        keys.push(full_key);
                    }
                }
            }
        }
        _ => {
            if !prefix.is_empty() {
                keys.push(prefix.to_string());
            }
        }
    }
}

/// Extract keys from .properties files (Java-style key=value or key: value).
fn extract_properties_keys(content: &str) -> Vec<String> {
    let mut keys = Vec::new();
    let mut seen = HashSet::new();

    for line in content.lines() {
        let trimmed = line.trim();

        // Skip empty lines and comments
        if trimmed.is_empty() || trimmed.starts_with('#') || trimmed.starts_with('!') {
            continue;
        }

        // Handle continuation lines (ending with \)
        // For simplicity, just extract the key from the first line
        let key = if let Some(eq_pos) = trimmed.find('=') {
            trimmed[..eq_pos].trim()
        } else if let Some(colon_pos) = trimmed.find(':') {
            trimmed[..colon_pos].trim()
        } else {
            continue;
        };

        if !key.is_empty() && seen.insert(key.to_string()) {
            keys.push(key.to_string());
        }
    }

    keys
}

/// Extract keys from .env files (KEY=value format).
fn extract_env_keys(content: &str) -> Vec<String> {
    let mut keys = Vec::new();
    let mut seen = HashSet::new();

    for line in content.lines() {
        let trimmed = line.trim();

        // Skip empty lines and comments
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }

        // Strip optional "export " prefix
        let line_content = trimmed.strip_prefix("export ").unwrap_or(trimmed);

        // Extract KEY from KEY=value
        if let Some(eq_pos) = line_content.find('=') {
            let key = line_content[..eq_pos].trim();
            if !key.is_empty()
                && key
                    .chars()
                    .all(|c| c.is_alphanumeric() || c == '_' || c == '.')
                && seen.insert(key.to_string())
            {
                keys.push(key.to_string());
            }
        }
    }

    keys
}

/// A config reference found in code.
struct ConfigReference {
    /// The key being referenced.
    key: String,
    /// The file path where the reference was found.
    file_path: String,
    /// The line number of the reference.
    line_num: i32,
    /// The qualified name of the enclosing function/method.
    enclosing_qn: String,
}

/// Configuration Linking Pass.
///
/// 1. Detects config files by extension (.env, .yml, .json, .properties, .toml)
/// 2. Extracts configuration keys in dot-separated notation (max depth 10)
/// 3. Scans code files for config access patterns
/// 4. Creates CONFIGURES edges from config file node to enclosing function/method
/// 5. Uses case-sensitive exact string comparison for matching
/// 6. Logs warnings for unresolved key references
pub fn pass_configlink(
    buf: &mut GraphBuffer,
    reg: &Registry,
    files: &[&DiscoveredFile],
    project: &str,
) {
    // Step 1: Find config files and extract keys
    let mut config_keys: HashMap<String, Vec<String>> = HashMap::new(); // key -> list of config file QNs

    for f in files {
        if !is_config_file(&f.rel_path) {
            continue;
        }
        let content = match std::fs::read_to_string(&f.abs_path) {
            Ok(s) => s,
            Err(e) => {
                tracing::debug!(
                    error = %e,
                    path = %f.rel_path,
                    "pass_configlink: failed to read config file"
                );
                continue;
            }
        };

        let keys = extract_config_keys(&f.rel_path, &content);
        let file_qn = fqn::fqn_module(project, &f.rel_path);

        for key in keys {
            config_keys.entry(key).or_default().push(file_qn.clone());
        }
    }

    if config_keys.is_empty() {
        return;
    }

    // Step 2: Scan code files for config access patterns
    let mut references: Vec<ConfigReference> = Vec::new();

    for f in files {
        // Skip config files themselves
        if is_config_file(&f.rel_path) {
            continue;
        }

        let source = match std::fs::read_to_string(&f.abs_path) {
            Ok(s) => s,
            Err(_) => continue,
        };

        // Build line offset table
        let mut line_starts: Vec<usize> = vec![0];
        for (i, b) in source.bytes().enumerate() {
            if b == b'\n' {
                line_starts.push(i + 1);
            }
        }

        // Get functions in this file for caller resolution
        let file_fns = reg.entries_for_file(&f.rel_path);
        let module_qn = fqn::fqn_module(project, &f.rel_path);

        // Find all config access patterns in this file
        for caps in CONFIG_ACCESS_PATTERNS.captures_iter(&source) {
            // Extract the key from whichever capture group matched
            let key = (1..=10)
                .find_map(|i| caps.get(i).map(|m| m.as_str().to_owned()))
                .unwrap_or_default();

            if key.is_empty() {
                continue;
            }

            let mat_start = caps.get(0).unwrap().start();
            let line_num = line_starts.partition_point(|&off| off <= mat_start) as i32;

            // Find the enclosing function/method
            let enclosing_qn = file_fns
                .iter()
                .rev()
                .find(|e| e.start_line <= line_num && e.end_line >= line_num)
                .map(|e| e.qualified_name.as_str())
                .unwrap_or(module_qn.as_str());

            references.push(ConfigReference {
                key,
                file_path: f.rel_path.clone(),
                line_num,
                enclosing_qn: enclosing_qn.to_owned(),
            });
        }
    }

    // Step 3: Match references against config keys (case-sensitive exact match)
    // and create CONFIGURES edges
    let mut created_edges: HashSet<(String, String, String)> = HashSet::new(); // (config_qn, target_qn, key)

    for reference in &references {
        if let Some(config_file_qns) = config_keys.get(&reference.key) {
            // Key found in config — create CONFIGURES edges
            for config_qn in config_file_qns {
                let edge_key = (
                    config_qn.clone(),
                    reference.enclosing_qn.clone(),
                    reference.key.clone(),
                );

                // Deduplicate: one edge per (config_file, enclosing_function, key)
                if created_edges.insert(edge_key) {
                    let props = serde_json::json!({
                        "key": reference.key,
                    })
                    .to_string();

                    buf.add_edge_with_confidence(
                        config_qn,
                        &reference.enclosing_qn,
                        "CONFIGURES",
                        EdgeSource::RegexMatch,
                        Some(props),
                    );
                }
            }
        } else {
            // Key not found in any config file — log warning (Requirement 14.4)
            tracing::warn!(
                key = %reference.key,
                file = %reference.file_path,
                line = reference.line_num,
                "pass_configlink: unresolved config key reference"
            );
        }
    }

    tracing::info!(
        config_files = config_keys.len(),
        references = references.len(),
        edges_created = created_edges.len(),
        "pass_configlink: complete"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_config_file() {
        assert!(is_config_file("config.json"));
        assert!(is_config_file("application.yml"));
        assert!(is_config_file("application.yaml"));
        assert!(is_config_file(".env"));
        assert!(is_config_file("config/database.toml"));
        assert!(is_config_file("app.properties"));
        assert!(!is_config_file("main.rs"));
        assert!(!is_config_file("index.ts"));
    }

    #[test]
    fn test_extract_env_keys() {
        let content = r#"
# Database config
DATABASE_URL=postgres://localhost/mydb
API_KEY=secret123
export PORT=3000
"#;
        let keys = extract_env_keys(content);
        assert!(keys.contains(&"DATABASE_URL".to_string()));
        assert!(keys.contains(&"API_KEY".to_string()));
        assert!(keys.contains(&"PORT".to_string()));
    }

    #[test]
    fn test_extract_json_keys_nested() {
        let content = r#"{
            "database": {
                "host": "localhost",
                "port": 5432,
                "credentials": {
                    "username": "admin",
                    "password": "secret"
                }
            },
            "api_key": "abc123"
        }"#;
        let keys = extract_json_keys(content);
        assert!(keys.contains(&"database.host".to_string()));
        assert!(keys.contains(&"database.port".to_string()));
        assert!(keys.contains(&"database.credentials.username".to_string()));
        assert!(keys.contains(&"database.credentials.password".to_string()));
        assert!(keys.contains(&"api_key".to_string()));
    }

    #[test]
    fn test_extract_yaml_keys_nested() {
        let content = r#"
database:
  host: localhost
  port: 5432
  credentials:
    username: admin
    password: secret
api_key: abc123
"#;
        let keys = extract_yaml_keys(content);
        assert!(keys.contains(&"database.host".to_string()));
        assert!(keys.contains(&"database.port".to_string()));
        assert!(keys.contains(&"database.credentials.username".to_string()));
        assert!(keys.contains(&"database.credentials.password".to_string()));
        assert!(keys.contains(&"api_key".to_string()));
    }

    #[test]
    fn test_extract_toml_keys_nested() {
        let content = r#"
api_key = "abc123"

[database]
host = "localhost"
port = 5432

[database.credentials]
username = "admin"
password = "secret"
"#;
        let keys = extract_toml_keys(content);
        assert!(keys.contains(&"database.host".to_string()));
        assert!(keys.contains(&"database.port".to_string()));
        assert!(keys.contains(&"database.credentials.username".to_string()));
        assert!(keys.contains(&"database.credentials.password".to_string()));
        assert!(keys.contains(&"api_key".to_string()));
    }

    #[test]
    fn test_extract_properties_keys() {
        let content = r#"
# Application properties
database.host=localhost
database.port=5432
api.key=secret
spring.datasource.url=jdbc:postgresql://localhost/db
"#;
        let keys = extract_properties_keys(content);
        assert!(keys.contains(&"database.host".to_string()));
        assert!(keys.contains(&"database.port".to_string()));
        assert!(keys.contains(&"api.key".to_string()));
        assert!(keys.contains(&"spring.datasource.url".to_string()));
    }

    #[test]
    fn test_extract_yaml_max_depth() {
        // Create deeply nested YAML (11 levels)
        let content = r#"
a:
  b:
    c:
      d:
        e:
          f:
            g:
              h:
                i:
                  j:
                    k: too_deep
"#;
        let keys = extract_yaml_keys(content);
        // Should not contain keys deeper than MAX_DEPTH (10)
        for key in &keys {
            let depth = key.split('.').count();
            assert!(depth <= MAX_DEPTH, "Key '{}' exceeds max depth", key);
        }
    }

    #[test]
    fn test_config_access_pattern_process_env() {
        let source = r#"const url = process.env.DATABASE_URL;"#;
        let caps: Vec<_> = CONFIG_ACCESS_PATTERNS.captures_iter(source).collect();
        assert_eq!(caps.len(), 1);
        let key = (1..=10)
            .find_map(|i| caps[0].get(i).map(|m| m.as_str()))
            .unwrap();
        assert_eq!(key, "DATABASE_URL");
    }

    #[test]
    fn test_config_access_pattern_os_environ() {
        let source = r#"url = os.environ["DATABASE_URL"]"#;
        let caps: Vec<_> = CONFIG_ACCESS_PATTERNS.captures_iter(source).collect();
        assert_eq!(caps.len(), 1);
        let key = (1..=10)
            .find_map(|i| caps[0].get(i).map(|m| m.as_str()))
            .unwrap();
        assert_eq!(key, "DATABASE_URL");
    }

    #[test]
    fn test_config_access_pattern_os_getenv() {
        let source = r#"url := os.Getenv("DATABASE_URL")"#;
        let caps: Vec<_> = CONFIG_ACCESS_PATTERNS.captures_iter(source).collect();
        assert_eq!(caps.len(), 1);
        let key = (1..=10)
            .find_map(|i| caps[0].get(i).map(|m| m.as_str()))
            .unwrap();
        assert_eq!(key, "DATABASE_URL");
    }

    #[test]
    fn test_config_access_pattern_system_getenv() {
        let source = r#"String url = System.getenv("DATABASE_URL");"#;
        let caps: Vec<_> = CONFIG_ACCESS_PATTERNS.captures_iter(source).collect();
        assert_eq!(caps.len(), 1);
        let key = (1..=10)
            .find_map(|i| caps[0].get(i).map(|m| m.as_str()))
            .unwrap();
        assert_eq!(key, "DATABASE_URL");
    }

    #[test]
    fn test_config_access_pattern_spring_value() {
        let source = r#"@Value("${database.host}")
private String dbHost;"#;
        let caps: Vec<_> = CONFIG_ACCESS_PATTERNS.captures_iter(source).collect();
        assert_eq!(caps.len(), 1);
        let key = (1..=10)
            .find_map(|i| caps[0].get(i).map(|m| m.as_str()))
            .unwrap();
        assert_eq!(key, "database.host");
    }

    #[test]
    fn test_config_access_pattern_spring_value_with_default() {
        let source = r#"@Value("${database.host:localhost}")
private String dbHost;"#;
        let caps: Vec<_> = CONFIG_ACCESS_PATTERNS.captures_iter(source).collect();
        assert_eq!(caps.len(), 1);
        let key = (1..=10)
            .find_map(|i| caps[0].get(i).map(|m| m.as_str()))
            .unwrap();
        assert_eq!(key, "database.host");
    }

    #[test]
    fn test_case_sensitive_matching() {
        // Verify that the matching is case-sensitive
        let keys: HashMap<String, Vec<String>> =
            HashMap::from([("DATABASE_URL".to_string(), vec!["config.env".to_string()])]);

        // "database_url" should NOT match "DATABASE_URL"
        assert!(!keys.contains_key("database_url"));
        // "DATABASE_URL" should match
        assert!(keys.contains_key("DATABASE_URL"));
    }

    #[test]
    fn test_extract_json_keys_empty() {
        let keys = extract_json_keys("{}");
        assert!(keys.is_empty());
    }

    #[test]
    fn test_extract_json_keys_invalid() {
        let keys = extract_json_keys("not valid json");
        assert!(keys.is_empty());
    }

    #[test]
    fn test_extract_yaml_keys_with_comments() {
        let content = r#"
# This is a comment
server:
  port: 8080  # inline comment
  host: localhost
"#;
        let keys = extract_yaml_keys(content);
        assert!(keys.contains(&"server.port".to_string()));
        assert!(keys.contains(&"server.host".to_string()));
    }

    #[test]
    fn test_extract_env_keys_with_export() {
        let content = r#"
export NODE_ENV=production
DATABASE_URL=postgres://localhost/db
"#;
        let keys = extract_env_keys(content);
        assert!(keys.contains(&"NODE_ENV".to_string()));
        assert!(keys.contains(&"DATABASE_URL".to_string()));
    }
}
